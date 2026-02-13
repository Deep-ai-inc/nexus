//! Native macOS Backend
//!
//! Replaces the iced adapter with a direct NSApplication + NSWindow + wgpu
//! backend. Renders on a dedicated thread with three-layer architecture for
//! flicker-free resize.
//!
//! This is the ONLY module that bridges Strata to the macOS window system.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::declare::ClassBuilder;
use objc2::{msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent,
    NSEventModifierFlags, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    CGFloat, MainThreadMarker, NSPoint, NSRect, NSSize, NSString, ns_string,
};

use crate::app::{AppConfig, CaptureRequest, Command, StrataApp};
use crate::content_address::Selection;
use crate::event_context::{
    CaptureState, Key, KeyEvent, Modifiers, MouseButton, MouseEvent, NamedKey,
    ScrollDelta,
};
use crate::gpu::{ImageHandle, ImageStore, PendingImage, StrataPipeline};
use crate::layout_snapshot::{HitResult, LayoutSnapshot};
use crate::primitives::{Color, Point, Rect};

/// Error type for shell operations.
#[derive(Debug)]
pub enum Error {
    /// Window creation failed.
    Window(String),
    /// GPU initialization failed.
    Gpu(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Window(e) => write!(f, "window error: {e}"),
            Self::Gpu(e) => write!(f, "GPU error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Base font size in logical points.
const BASE_FONT_SIZE: f32 = 14.0;

// ============================================================================
// Scene (built each frame for rendering)
// ============================================================================

/// A scene snapshot ready for rendering.
struct Scene {
    snapshot: Arc<LayoutSnapshot>,
    selection: Option<Selection>,
    background: Color,
    pending_images: Vec<PendingImage>,
    pending_unloads: Vec<ImageHandle>,
}

// ============================================================================
// Render Resources (main thread only)
// ============================================================================

struct RenderResources {
    gpu: GpuState,
    pipeline: StrataPipeline,
    current_scale: f32,
}

// ============================================================================
// Per-Window State (main thread only)
// ============================================================================

struct WindowState<A: StrataApp> {
    app: A::State,
    #[allow(dead_code)]
    shared: A::SharedState,
    capture: CaptureState,
    window_size: (f32, f32),
    base_size: (f32, f32),
    current_zoom: f32,
    cursor_position: Option<Point>,
    image_store: ImageStore,
    cached_snapshot: Option<Arc<LayoutSnapshot>>,
    render: RenderResources,
    overlay_layer_ptr: *mut AnyObject,
    resize_timer: *mut c_void, // CFRunLoopTimerRef, null when inactive
    needs_render: bool,
    surface_dirty: bool,
    dpi_scale: f32,
    tokio_rt: Arc<tokio::runtime::Runtime>,
    command_tx: std::sync::mpsc::Sender<A::Message>,
    command_rx: std::sync::mpsc::Receiver<A::Message>,
}

// ============================================================================
// Public API
// ============================================================================

/// Run a Strata application with default configuration.
pub fn run<A: StrataApp>() -> Result<(), Error> {
    run_with_config::<A>(AppConfig::default())
}

/// Run a Strata application with custom configuration.
pub fn run_with_config<A: StrataApp>(config: AppConfig) -> Result<(), Error> {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    // Tokio runtime for async tasks.
    let tokio_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Window(format!("Failed to create tokio runtime: {e}")))?,
    );

    // NSApplication setup.
    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // Create NSWindow.
    let (win_w, win_h) = config.window_size;
    let content_rect = NSRect::new(
        NSPoint::new(200.0, 200.0),
        NSSize::new(win_w as f64, win_h as f64),
    );
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            content_rect,
            style,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        )
    };

    let bg_color = unsafe {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            config.background_color.r as CGFloat,
            config.background_color.g as CGFloat,
            config.background_color.b as CGFloat,
            1.0,
        )
    };
    window.setBackgroundColor(Some(&bg_color));
    window.setTitle(&NSString::from_str(&config.title));
    window.setMinSize(NSSize::new(400.0, 300.0));

    let dpi_scale = window.backingScaleFactor() as f32;

    // Create custom NSView with layer hierarchy.
    let view_state = create_view_and_layers(mtm, &window, &config, dpi_scale)?;

    // Initialize wgpu.
    let gpu = init_wgpu(view_state.metal_layer_ptr, win_w, win_h, dpi_scale)?;

    // Initialize pipeline on main thread.
    let scale = dpi_scale;
    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();
    let pipeline = StrataPipeline::new(
        &gpu.device, &gpu.queue, gpu.surface_config.format,
        BASE_FONT_SIZE * scale, &mut font_system,
    );
    drop(font_system);

    let render = RenderResources {
        gpu,
        pipeline,
        current_scale: scale,
    };

    // Initialize application.
    let mut image_store = ImageStore::new();
    let shared = A::SharedState::default();
    let (app_state, init_cmd) = A::init(&shared, &mut image_store);

    let (command_tx, command_rx) = std::sync::mpsc::channel();
    spawn_commands(&tokio_rt, init_cmd, command_tx.clone());

    let mut win_state = WindowState::<A> {
        app: app_state,
        shared,
        capture: CaptureState::None,
        window_size: (win_w, win_h),
        base_size: (win_w, win_h),
        current_zoom: 1.0,
        cursor_position: None,
        image_store,
        cached_snapshot: None,
        render,
        overlay_layer_ptr: view_state.overlay_layer_ptr,
        resize_timer: std::ptr::null_mut(),
        needs_render: true,
        surface_dirty: false,
        dpi_scale,
        tokio_rt: tokio_rt.clone(),
        command_tx: command_tx.clone(),
        command_rx,
    };

    // Render first frame synchronously before showing window.
    {
        let scene = build_scene::<A>(&win_state);
        win_state.cached_snapshot = Some(scene.snapshot.clone());
        render_frame(&mut win_state.render, &scene, dpi_scale);
        win_state.needs_render = false;
    }

    // Store state pointer in the view's ivar.
    let win_state_ptr = Box::into_raw(Box::new(RefCell::new(win_state)));
    unsafe {
        let view_ptr = (&*view_state.view) as *const AnyObject as *mut u8;
        let ivar = (*view_state.view).class().instance_variable("_strata_state")
            .expect("_strata_state ivar not found");
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut *mut c_void;
        *ivar_ptr = win_state_ptr as *mut c_void;
    }

    // Install typed event handlers.
    install_event_handlers::<A>();

    // Main thread timer: polls async commands AND renders (~16ms).
    install_main_thread_timer::<A>(win_state_ptr);

    // Show window and run.
    window.makeKeyAndOrderFront(None);
    unsafe { ns_app.activate() };
    unsafe { ns_app.run() };

    // Cleanup (unreachable).
    drop(unsafe { Box::from_raw(win_state_ptr) });
    Ok(())
}

// ============================================================================
// View + Layer Setup
// ============================================================================

struct ViewState {
    view: Retained<AnyObject>,
    metal_layer_ptr: *mut c_void,
    overlay_layer_ptr: *mut AnyObject,
}

fn create_view_and_layers(
    _mtm: MainThreadMarker,
    window: &NSWindow,
    config: &AppConfig,
    dpi_scale: f32,
) -> Result<ViewState, Error> {
    let view_class = register_strata_view_class();

    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(config.window_size.0 as f64, config.window_size.1 as f64),
    );

    let view: Retained<AnyObject> = unsafe {
        let raw: *mut AnyObject = msg_send![view_class, alloc];
        let raw: *mut AnyObject = msg_send![raw, initWithFrame: frame];
        // alloc+init returns +1, use from_raw to take ownership without extra retain.
        Retained::from_raw(raw).expect("Failed to create StrataView")
    };

    // Enable layer backing.
    unsafe {
        let _: () = msg_send![&*view, setWantsLayer: Bool::YES];
        // NSViewLayerContentsRedrawOnSetNeedsDisplay = 3
        let _: () = msg_send![&*view, setLayerContentsRedrawPolicy: 3i64];
    }

    // Get root layer.
    let root_layer: *mut AnyObject = unsafe { msg_send![&*view, layer] };
    if root_layer.is_null() {
        return Err(Error::Window("Failed to get view layer".into()));
    }

    let bg_cg = crate::platform::macos::create_cg_color(
        config.background_color.r as f64,
        config.background_color.g as f64,
        config.background_color.b as f64,
    );

    unsafe {
        disable_layer_animations(root_layer);
        let _: () = msg_send![root_layer, setBackgroundColor: bg_cg];
    }

    // Create CAMetalLayer sublayer.
    let metal_layer_class = AnyClass::get("CAMetalLayer")
        .ok_or_else(|| Error::Window("CAMetalLayer not found".into()))?;
    let metal_layer: *mut AnyObject = unsafe { msg_send![metal_layer_class, layer] };

    let (win_w, win_h) = config.window_size;
    let phys_w = (win_w * dpi_scale) as f64;
    let phys_h = (win_h * dpi_scale) as f64;

    unsafe {
        let _: () = msg_send![metal_layer, setPixelFormat: 80u64]; // MTLPixelFormatBGRA8Unorm_sRGB
        let _: () = msg_send![metal_layer, setFramebufferOnly: Bool::YES];
        let _: () = msg_send![metal_layer, setAllowsNextDrawableTimeout: Bool::YES];
        let _: () = msg_send![metal_layer, setPresentsWithTransaction: Bool::NO];
        let _: () = msg_send![metal_layer, setMaximumDrawableCount: 3u64];

        // sRGB colorspace.
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut c_void;
        }
        unsafe extern "C" {
            static kCGColorSpaceSRGB: *const c_void;
        }
        let srgb_space = CGColorSpacePtr(CGColorSpaceCreateWithName(kCGColorSpaceSRGB));
        let _: () = msg_send![metal_layer, setColorspace: srgb_space];

        let bounds: NSRect = msg_send![root_layer, bounds];
        let _: () = msg_send![metal_layer, setFrame: bounds];
        let _: () = msg_send![metal_layer, setContentsScale: dpi_scale as f64];
        let drawable_size = NSSize::new(phys_w, phys_h);
        let _: () = msg_send![metal_layer, setDrawableSize: drawable_size];

        disable_layer_animations(metal_layer);
        let gravity = ns_string!("topLeft");
        let _: () = msg_send![metal_layer, setContentsGravity: &**gravity];
        let _: () = msg_send![metal_layer, setBackgroundColor: bg_cg];
        let _: () = msg_send![root_layer, addSublayer: metal_layer];
    }

    // Create overlay CALayer (above metal layer, for resize IOSurface).
    let overlay_class = AnyClass::get("CALayer")
        .ok_or_else(|| Error::Window("CALayer not found".into()))?;
    let overlay_layer: *mut AnyObject = unsafe { msg_send![overlay_class, layer] };

    unsafe {
        disable_layer_animations(overlay_layer);
        let gravity = ns_string!("topLeft");
        let _: () = msg_send![overlay_layer, setContentsGravity: &**gravity];
        let _: () = msg_send![overlay_layer, setHidden: Bool::YES];
        let bounds: NSRect = msg_send![root_layer, bounds];
        let _: () = msg_send![overlay_layer, setFrame: bounds];
        let _: () = msg_send![overlay_layer, setContentsScale: dpi_scale as f64];
        let _: () = msg_send![root_layer, addSublayer: overlay_layer];
    }

    // Store layer pointers in ivars.
    unsafe {
        let view_ptr = (&*view) as *const AnyObject as *mut u8;
        let cls = (*view).class();
        let ivar = cls.instance_variable("_metal_layer").unwrap();
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut *mut c_void;
        *ivar_ptr = metal_layer as *mut c_void;
        let ivar = cls.instance_variable("_overlay_layer").unwrap();
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut *mut c_void;
        *ivar_ptr = overlay_layer as *mut c_void;
    }

    // Set as content view.
    unsafe {
        let view_ref: &NSView =
            &*((&*view) as *const AnyObject as *const NSView);
        window.setContentView(Some(view_ref));
    }

    Ok(ViewState {
        view,
        metal_layer_ptr: metal_layer as *mut c_void,
        overlay_layer_ptr: overlay_layer,
    })
}

unsafe fn disable_layer_animations(layer: *mut AnyObject) {
    unsafe {
        let null_cls = AnyClass::get("NSNull").unwrap();
        let null_obj: *mut AnyObject = msg_send![null_cls, null];
        let dict_cls = AnyClass::get("NSMutableDictionary").unwrap();
        let actions: *mut AnyObject = msg_send![dict_cls, new];
        for key in [
            ns_string!("bounds"),
            ns_string!("position"),
            ns_string!("contents"),
            ns_string!("contentsScale"),
            ns_string!("hidden"),
        ] {
            let _: () = msg_send![actions, setObject: null_obj forKey: &*key];
        }
        let _: () = msg_send![layer, setActions: actions];
    }
}

// ============================================================================
// NSView Subclass
// ============================================================================

fn register_strata_view_class() -> &'static AnyClass {
    static CLASS: std::sync::OnceLock<&'static AnyClass> = std::sync::OnceLock::new();
    CLASS.get_or_init(|| {
        let superclass = AnyClass::get("NSView").unwrap();
        let mut builder = ClassBuilder::new("StrataView", superclass).unwrap();

        builder.add_ivar::<*mut c_void>("_metal_layer");
        builder.add_ivar::<*mut c_void>("_overlay_layer");
        builder.add_ivar::<*mut c_void>("_strata_state");
        builder.add_ivar::<u8>("_is_resizing");

        let cls = builder.register();
        let cls_ptr = cls as *const _ as *mut objc2::ffi::objc_class;

        // Helper to add a method via the ObjC runtime (avoids ClassBuilder lifetime issues).
        unsafe fn add_method_raw(
            cls: *mut objc2::ffi::objc_class,
            sel: Sel,
            imp: objc2::ffi::IMP,
            types: &std::ffi::CStr,
        ) {
            unsafe { objc2::ffi::class_addMethod(cls, sel.as_ptr(), imp, types.as_ptr()); }
        }

        // Type encodings:  B = BOOL, v = void, @ = id, : = SEL, {CGSize=dd} = NSSize
        unsafe {
            add_method_raw(cls_ptr, sel!(acceptsFirstResponder),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel) -> Bool, unsafe extern "C" fn()>(accepts_first_responder)), c"B@:");
            add_method_raw(cls_ptr, sel!(isFlipped),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel) -> Bool, unsafe extern "C" fn()>(is_flipped)), c"B@:");
            add_method_raw(cls_ptr, sel!(mouseDown:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(mouse_down)), c"v@:@");
            add_method_raw(cls_ptr, sel!(mouseUp:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(mouse_up)), c"v@:@");
            add_method_raw(cls_ptr, sel!(mouseDragged:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(mouse_dragged)), c"v@:@");
            add_method_raw(cls_ptr, sel!(mouseMoved:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(mouse_moved)), c"v@:@");
            add_method_raw(cls_ptr, sel!(rightMouseDown:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(right_mouse_down)), c"v@:@");
            add_method_raw(cls_ptr, sel!(rightMouseUp:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(right_mouse_up)), c"v@:@");
            add_method_raw(cls_ptr, sel!(rightMouseDragged:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(right_mouse_dragged)), c"v@:@");
            add_method_raw(cls_ptr, sel!(scrollWheel:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(scroll_wheel)), c"v@:@");
            add_method_raw(cls_ptr, sel!(keyDown:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(key_down)), c"v@:@");
            add_method_raw(cls_ptr, sel!(keyUp:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(key_up)), c"v@:@");
            add_method_raw(cls_ptr, sel!(flagsChanged:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(flags_changed)), c"v@:@");
            add_method_raw(cls_ptr, sel!(setFrameSize:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, NSSize), unsafe extern "C" fn()>(set_frame_size)), c"v@:{CGSize=dd}");
            add_method_raw(cls_ptr, sel!(viewWillStartLiveResize),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel), unsafe extern "C" fn()>(view_will_start_live_resize)), c"v@:");
            add_method_raw(cls_ptr, sel!(viewDidEndLiveResize),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel), unsafe extern "C" fn()>(view_did_end_live_resize)), c"v@:");
            add_method_raw(cls_ptr, sel!(mouseEntered:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(mouse_entered)), c"v@:@");
            add_method_raw(cls_ptr, sel!(mouseExited:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(mouse_exited)), c"v@:@");
        }

        cls
    })
}

// ============================================================================
// NSView Method Implementations
// ============================================================================

extern "C" fn accepts_first_responder(_this: &AnyObject, _sel: Sel) -> Bool {
    Bool::YES
}
extern "C" fn is_flipped(_this: &AnyObject, _sel: Sel) -> Bool {
    Bool::YES
}

/// Type-erased event handlers (set during run).
static mut MOUSE_HANDLER: Option<fn(&AnyObject, MouseEvent)> = None;
static mut KEY_HANDLER: Option<fn(&AnyObject, KeyEvent)> = None;
static mut RESIZE_HANDLER: Option<fn(&AnyObject, f32, f32)> = None;
static mut RESIZE_START_HANDLER: Option<fn(&AnyObject)> = None;
static mut RESIZE_END_HANDLER: Option<fn(&AnyObject)> = None;

fn install_event_handlers<A: StrataApp>() {
    unsafe {
        MOUSE_HANDLER = Some(handle_mouse_event::<A>);
        KEY_HANDLER = Some(handle_key_event::<A>);
        RESIZE_HANDLER = Some(handle_resize::<A>);
        RESIZE_START_HANDLER = Some(handle_resize_start::<A>);
        RESIZE_END_HANDLER = Some(handle_resize_end::<A>);
        RESIZE_IDLE_HANDLER = Some(handle_resize_idle::<A>);
    }
}

fn dispatch_mouse(view: &AnyObject, event: MouseEvent) {
    unsafe {
        if let Some(handler) = MOUSE_HANDLER {
            handler(view, event);
        }
    }
}

fn dispatch_key(view: &AnyObject, event: KeyEvent) {
    unsafe {
        if let Some(handler) = KEY_HANDLER {
            handler(view, event);
        }
    }
}

fn dispatch_resize(view: &AnyObject, w: f32, h: f32) {
    unsafe {
        if let Some(handler) = RESIZE_HANDLER {
            handler(view, w, h);
        }
    }
}

/// Get typed state from view ivar.
unsafe fn get_state<A: StrataApp>(view: &AnyObject) -> Option<&RefCell<WindowState<A>>> {
    unsafe {
        let ivar = view.class().instance_variable("_strata_state")?;
        let ptr = *ivar.load::<*mut c_void>(view);
        if ptr.is_null() { return None; }
        Some(&*(ptr as *const RefCell<WindowState<A>>))
    }
}

fn event_position(event: &NSEvent, view: &AnyObject) -> Point {
    unsafe {
        let loc_window: NSPoint = event.locationInWindow();
        let loc_view: NSPoint = msg_send![view, convertPoint: loc_window fromView: std::ptr::null::<AnyObject>()];
        Point::new(loc_view.x as f32, loc_view.y as f32)
    }
}

/// Cast raw event pointer to &NSEvent reference (safe for ObjC callback args).
unsafe fn event_ref(raw: *mut AnyObject) -> &'static NSEvent {
    unsafe { &*(raw as *const NSEvent) }
}

extern "C" fn mouse_down(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::ButtonPressed { button: MouseButton::Left, position: pos });
}

extern "C" fn mouse_up(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::ButtonReleased { button: MouseButton::Left, position: pos });
}

extern "C" fn mouse_dragged(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::CursorMoved { position: pos });
}

extern "C" fn mouse_moved(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::CursorMoved { position: pos });
}

extern "C" fn right_mouse_down(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::ButtonPressed { button: MouseButton::Right, position: pos });
}

extern "C" fn right_mouse_up(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::ButtonReleased { button: MouseButton::Right, position: pos });
}

extern "C" fn right_mouse_dragged(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    dispatch_mouse(this, MouseEvent::CursorMoved { position: pos });
}

extern "C" fn scroll_wheel(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    let (dx, dy) = unsafe { (event.scrollingDeltaX(), event.scrollingDeltaY()) };
    let has_precise: Bool = unsafe { msg_send![event, hasPreciseScrollingDeltas] };

    let delta = if has_precise.as_bool() {
        ScrollDelta::Pixels { x: dx as f32, y: dy as f32 }
    } else {
        ScrollDelta::Lines { x: dx as f32, y: dy as f32 }
    };

    dispatch_mouse(this, MouseEvent::WheelScrolled { delta, position: pos });
}

extern "C" fn mouse_entered(this: &AnyObject, _sel: Sel, _event: *mut AnyObject) {
    dispatch_mouse(this, MouseEvent::CursorEntered);
}

extern "C" fn mouse_exited(this: &AnyObject, _sel: Sel, _event: *mut AnyObject) {
    dispatch_mouse(this, MouseEvent::CursorLeft);
}

extern "C" fn key_down(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    if let Some(ke) = convert_ns_key_event(event, true) {
        dispatch_key(this, ke);
    }
}

extern "C" fn key_up(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    if let Some(ke) = convert_ns_key_event(event, false) {
        dispatch_key(this, ke);
    }
}

extern "C" fn flags_changed(_this: &AnyObject, _sel: Sel, _event: *mut AnyObject) {
    // Modifiers are read from each key/mouse event.
}

extern "C" fn set_frame_size(this: &AnyObject, _sel: Sel, new_size: NSSize) {
    let superclass = AnyClass::get("NSView").unwrap();
    let _: () = unsafe { msg_send![super(this, superclass), setFrameSize: new_size] };

    // Update layer frames.
    unsafe {
        let root: *mut AnyObject = msg_send![this, layer];
        if root.is_null() { return; }

        let bounds: NSRect = msg_send![root, bounds];
        let scale: f64 = msg_send![root, contentsScale];

        let ivar = this.class().instance_variable("_metal_layer").unwrap();
        let metal_layer = *ivar.load::<*mut c_void>(this) as *mut AnyObject;
        if !metal_layer.is_null() {
            let _: () = msg_send![metal_layer, setFrame: bounds];
            let _: () = msg_send![metal_layer, setContentsScale: scale];
            let drawable_size = NSSize::new(new_size.width * scale, new_size.height * scale);
            let _: () = msg_send![metal_layer, setDrawableSize: drawable_size];
        }

        let ivar = this.class().instance_variable("_overlay_layer").unwrap();
        let overlay_layer = *ivar.load::<*mut c_void>(this) as *mut AnyObject;
        if !overlay_layer.is_null() {
            let _: () = msg_send![overlay_layer, setFrame: bounds];
            let _: () = msg_send![overlay_layer, setContentsScale: scale];
        }
    }

    dispatch_resize(this, new_size.width as f32, new_size.height as f32);
}

extern "C" fn view_will_start_live_resize(this: &AnyObject, _sel: Sel) {
    let superclass = AnyClass::get("NSView").unwrap();
    let _: () = unsafe { msg_send![super(this, superclass), viewWillStartLiveResize] };
    unsafe {
        if let Some(handler) = RESIZE_START_HANDLER {
            handler(this);
        }
    }
}

extern "C" fn view_did_end_live_resize(this: &AnyObject, _sel: Sel) {
    let superclass = AnyClass::get("NSView").unwrap();
    let _: () = unsafe { msg_send![super(this, superclass), viewDidEndLiveResize] };
    unsafe {
        if let Some(handler) = RESIZE_END_HANDLER {
            handler(this);
        }
    }
}

// ============================================================================
// Event Handling
// ============================================================================

fn handle_mouse_event<A: StrataApp>(view: &AnyObject, strata_event: MouseEvent) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    let mut state = state_cell.borrow_mut();

    // Update cursor position.
    match &strata_event {
        MouseEvent::CursorMoved { position } |
        MouseEvent::ButtonPressed { position, .. } |
        MouseEvent::ButtonReleased { position, .. } |
        MouseEvent::WheelScrolled { position, .. } => {
            state.cursor_position = Some(*position);
        }
        _ => {}
    }

    let zoom = state.current_zoom;
    let adjusted_cursor = state.cursor_position.map(|p| Point::new(p.x / zoom, p.y / zoom));

    let hit = state.cached_snapshot.as_ref().and_then(|snapshot| {
        let raw_hit = adjusted_cursor.and_then(|pos| snapshot.hit_test(pos));
        if state.capture.is_captured() && !matches!(&raw_hit, Some(HitResult::Content(_))) {
            adjusted_cursor.and_then(|pos| snapshot.nearest_content(pos.x, pos.y)).or(raw_hit)
        } else {
            raw_hit
        }
    });

    let is_cursor_moved = matches!(strata_event, MouseEvent::CursorMoved { .. });
    if !hit.is_some() && !state.capture.is_captured() && !is_cursor_moved {
        return;
    }

    let response = A::on_mouse(&state.app, strata_event, hit, &state.capture);

    match response.capture {
        CaptureRequest::Capture(source) => state.capture = CaptureState::Captured(source),
        CaptureRequest::Release => state.capture = CaptureState::None,
        CaptureRequest::None => {}
    }

    if let Some(msg) = response.message {
        process_message::<A>(&mut state, msg);
    }
}

fn handle_key_event<A: StrataApp>(view: &AnyObject, key_event: KeyEvent) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    let mut state = state_cell.borrow_mut();

    if let Some(msg) = A::on_key(&state.app, key_event) {
        process_message::<A>(&mut state, msg);
    }
}

fn handle_resize<A: StrataApp>(view: &AnyObject, new_w: f32, new_h: f32) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    let mut state = state_cell.borrow_mut();

    state.window_size = (new_w, new_h);
    let zoom = state.current_zoom;
    state.base_size = (new_w / zoom, new_h / zoom);

    let phys_w = (new_w * state.dpi_scale) as u32;
    let phys_h = (new_h * state.dpi_scale) as u32;
    if phys_w == 0 || phys_h == 0 { return; }

    // Check if we're in live resize (ivar set by viewWillStartLiveResize).
    let is_resizing = unsafe {
        let ivar = view.class().instance_variable("_is_resizing").unwrap();
        *ivar.load::<u8>(view) != 0
    };

    if is_resizing {
        // Sync render path: render directly to overlay layer.
        let dpi_scale = state.dpi_scale;
        let overlay_layer = state.overlay_layer_ptr;

        // Build scene and update cached snapshot for hit-testing.
        let scene = build_scene::<A>(&state);
        state.cached_snapshot = Some(scene.snapshot.clone());

        // Reconfigure surface at new size and sync render.
        state.render.gpu.surface_config.width = phys_w;
        state.render.gpu.surface_config.height = phys_h;
        render_sync_to_overlay(&mut state.render, &scene, overlay_layer, dpi_scale);

        // Reset the resize idle timer.
        let timer_info = state_cell as *const _ as *mut c_void;
        reset_resize_idle_timer(&mut state.resize_timer, timer_info);
    } else {
        // Normal path: mark surface dirty and request render.
        state.render.gpu.surface_config.width = phys_w;
        state.render.gpu.surface_config.height = phys_h;
        state.surface_dirty = true;
        state.needs_render = true;
    }
}

fn handle_resize_start<A: StrataApp>(view: &AnyObject) {
    let Some(_state_cell) = (unsafe { get_state::<A>(view) }) else { return };

    // Set _is_resizing ivar.
    unsafe {
        let view_ptr = view as *const AnyObject as *mut u8;
        let ivar = view.class().instance_variable("_is_resizing").unwrap();
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut u8;
        *ivar_ptr = 1;
    }
}

fn handle_resize_end<A: StrataApp>(view: &AnyObject) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    let mut state = state_cell.borrow_mut();

    // Clear _is_resizing ivar.
    unsafe {
        let view_ptr = view as *const AnyObject as *mut u8;
        let ivar = view.class().instance_variable("_is_resizing").unwrap();
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut u8;
        *ivar_ptr = 0;
    }

    // Invalidate resize timer.
    invalidate_resize_timer(&mut state.resize_timer);

    // Hide overlay, clear contents.
    let overlay = state.overlay_layer_ptr;
    if !overlay.is_null() {
        unsafe {
            let _: () = msg_send![overlay, setHidden: Bool::YES];
            let _: () = msg_send![overlay, setContents: std::ptr::null::<AnyObject>()];
        }
    }

    // Reconfigure surface and trigger a normal render on the next timer tick.
    state.surface_dirty = true;
    state.needs_render = true;
}

fn process_message<A: StrataApp>(state: &mut WindowState<A>, msg: A::Message) {
    if A::is_exit_request(&msg) {
        std::process::exit(0);
    }

    let cmd = A::update(&mut state.app, msg, &mut state.image_store);
    spawn_commands(&state.tokio_rt, cmd, state.command_tx.clone());

    let new_zoom = A::zoom_level(&state.app);
    if (new_zoom - state.current_zoom).abs() > 0.001 {
        state.current_zoom = new_zoom;
    }

    invalidate_and_request_render::<A>(state);
}

fn build_scene<A: StrataApp>(state: &WindowState<A>) -> Scene {
    let zoom = A::zoom_level(&state.app);
    let mut snapshot = LayoutSnapshot::new();
    snapshot.set_viewport(Rect::new(0.0, 0.0, state.base_size.0, state.base_size.1));
    snapshot.set_zoom_level(zoom);
    A::view(&state.app, &mut snapshot);

    let snapshot = Arc::new(snapshot);

    Scene {
        snapshot,
        selection: A::selection(&state.app).cloned(),
        background: A::background_color(&state.app),
        pending_images: Vec::new(),
        pending_unloads: Vec::new(),
    }
}

fn invalidate_and_request_render<A: StrataApp>(state: &mut WindowState<A>) {
    let scene = build_scene::<A>(state);
    state.cached_snapshot = Some(scene.snapshot.clone());
    state.needs_render = true;
}

// ============================================================================
// Key Event Conversion
// ============================================================================

fn convert_ns_key_event(event: &NSEvent, pressed: bool) -> Option<KeyEvent> {
    let key_code: u16 = unsafe { event.keyCode() };
    let modifiers = convert_ns_modifiers(event);
    let key = convert_key_code(key_code, event);

    let text = if pressed {
        unsafe { event.characters().map(|s| s.to_string()) }
    } else {
        None
    };

    if pressed {
        Some(KeyEvent::Pressed { key, modifiers, text })
    } else {
        Some(KeyEvent::Released { key, modifiers })
    }
}

fn convert_ns_modifiers(event: &NSEvent) -> Modifiers {
    let flags = unsafe { event.modifierFlags() };
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
        ctrl: flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
        alt: flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
        meta: flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
    }
}

fn convert_key_code(key_code: u16, event: &NSEvent) -> Key {
    match key_code {
        0x7E => Key::Named(NamedKey::ArrowUp),
        0x7D => Key::Named(NamedKey::ArrowDown),
        0x7B => Key::Named(NamedKey::ArrowLeft),
        0x7C => Key::Named(NamedKey::ArrowRight),
        0x73 => Key::Named(NamedKey::Home),
        0x77 => Key::Named(NamedKey::End),
        0x74 => Key::Named(NamedKey::PageUp),
        0x79 => Key::Named(NamedKey::PageDown),
        0x33 => Key::Named(NamedKey::Backspace),
        0x75 => Key::Named(NamedKey::Delete),
        0x72 => Key::Named(NamedKey::Insert),
        0x24 | 0x4C => Key::Named(NamedKey::Enter),
        0x30 => Key::Named(NamedKey::Tab),
        0x35 => Key::Named(NamedKey::Escape),
        0x31 => Key::Named(NamedKey::Space),
        0x7A => Key::Named(NamedKey::F1),
        0x78 => Key::Named(NamedKey::F2),
        0x63 => Key::Named(NamedKey::F3),
        0x76 => Key::Named(NamedKey::F4),
        0x60 => Key::Named(NamedKey::F5),
        0x61 => Key::Named(NamedKey::F6),
        0x62 => Key::Named(NamedKey::F7),
        0x64 => Key::Named(NamedKey::F8),
        0x65 => Key::Named(NamedKey::F9),
        0x6D => Key::Named(NamedKey::F10),
        0x67 => Key::Named(NamedKey::F11),
        0x6F => Key::Named(NamedKey::F12),
        _ => {
            let chars = unsafe { event.charactersIgnoringModifiers() };
            match chars {
                Some(s) => {
                    let s = s.to_string();
                    if s.is_empty() { Key::Named(NamedKey::Unknown) }
                    else { Key::Character(s) }
                }
                None => Key::Named(NamedKey::Unknown),
            }
        }
    }
}

// ============================================================================
// wgpu Initialization
// ============================================================================

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
}

fn init_wgpu(
    metal_layer_ptr: *mut c_void,
    win_w: f32,
    win_h: f32,
    dpi_scale: f32,
) -> Result<GpuState, Error> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..Default::default()
    });

    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(metal_layer_ptr))
    }.map_err(|e| Error::Gpu(format!("Failed to create surface: {e}")))?;

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    })).ok_or_else(|| Error::Gpu("No suitable GPU adapter".into()))?;

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("Strata Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    )).map_err(|e| Error::Gpu(format!("Failed to create device: {e}")))?;

    let phys_w = (win_w * dpi_scale) as u32;
    let phys_h = (win_h * dpi_scale) as u32;

    let surface_config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: phys_w,
        height: phys_h,
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &surface_config);

    Ok(GpuState { surface, device, queue, surface_config })
}

// ============================================================================
// IOSurface Extraction (for resize overlay)
// ============================================================================

/// Extract IOSurfaceRef from a wgpu texture's underlying Metal drawable.
///
/// Uses wgpu's `as_hal` to access the private metal::Texture, then sends
/// the `iosurface` ObjC message to get the backing IOSurface.
/// Must be called BEFORE the SurfaceTexture is dropped or presented.
fn extract_iosurface(texture: &wgpu::Texture) -> Option<IOSurfacePtr> {
    unsafe {
        texture.as_hal::<wgpu::hal::api::Metal, _, _>(|hal_texture| {
            hal_texture.and_then(|t| {
                let metal_tex = t.raw_handle();
                // metal::Texture uses foreign-types; as_ptr() gives *mut MTLTexture.
                // Cast to *mut AnyObject for objc2's msg_send! receiver.
                // Return type is IOSurfacePtr (encoding: ^{__IOSurface=}).
                use metal::foreign_types::ForeignType;
                let raw_ptr = metal_tex.as_ptr() as *mut AnyObject;
                let iosurface: IOSurfacePtr = msg_send![raw_ptr, iosurface];
                if iosurface.0.is_null() { None } else { Some(iosurface) }
            })
        })
    }
}

/// Synchronous render to the overlay layer during resize.
///
/// Acquires a drawable via wgpu::Surface, renders the scene, waits for GPU
/// completion, extracts the IOSurface, and sets it on the overlay CALayer.
/// The frame is NOT presented — it's dropped, returning the drawable to the pool.
fn render_sync_to_overlay(
    res: &mut RenderResources,
    scene: &Scene,
    overlay_layer: *mut AnyObject,
    dpi_scale: f32,
) {
    let gpu = &mut res.gpu;

    // Reconfigure surface at current size.
    gpu.surface.configure(&gpu.device, &gpu.surface_config);

    let frame = match gpu.surface.get_current_texture() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Sync render: surface error: {e:?}");
            return;
        }
    };

    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
    let zoom = scene.snapshot.zoom_level();
    let scale = dpi_scale * zoom;

    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();

    if (res.current_scale - scale).abs() > 0.01 {
        res.pipeline = StrataPipeline::new(
            &gpu.device, &gpu.queue, gpu.surface_config.format,
            BASE_FONT_SIZE * scale, &mut font_system,
        );
        res.current_scale = scale;
    }

    // Upload/unload images.
    for img in &scene.pending_images {
        res.pipeline.load_image_rgba(&gpu.device, &gpu.queue, img.width, img.height, &img.data);
    }
    for handle in &scene.pending_unloads {
        res.pipeline.unload_image(*handle);
    }

    res.pipeline.after_frame();
    res.pipeline.clear();
    res.pipeline.set_background(scene.background);

    populate_pipeline(&mut res.pipeline, &scene.snapshot, scene.selection.as_ref(), scale, &mut font_system);
    drop(font_system);

    // Prepare (staging upload).
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Sync Staging Upload"),
    });
    res.pipeline.prepare(
        &gpu.device, &gpu.queue, &mut encoder,
        gpu.surface_config.width as f32, gpu.surface_config.height as f32,
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));

    // Render pass.
    let mut render_encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Sync Render"),
    });
    let clip = ClipBounds {
        x: 0, y: 0,
        width: gpu.surface_config.width,
        height: gpu.surface_config.height,
    };
    res.pipeline.render(&mut render_encoder, &view, &clip);
    gpu.queue.submit(std::iter::once(render_encoder.finish()));

    // Wait for GPU to finish writing pixels.
    gpu.device.poll(wgpu::Maintain::Wait);

    // Extract IOSurface BEFORE dropping the frame.
    if let Some(iosurface) = extract_iosurface(&frame.texture) {
        unsafe {
            // setContents: takes `id` (@) — bridge-cast IOSurfaceRef to id.
            let contents_id = iosurface.0 as *mut AnyObject;
            let _: () = msg_send![overlay_layer, setContents: contents_id];
            let _: () = msg_send![overlay_layer, setHidden: Bool::NO];
        }
    }

    // Drop frame WITHOUT calling present() — drawable returns to pool via refcount.
    drop(frame);
}

fn render_frame(res: &mut RenderResources, scene: &Scene, dpi_scale: f32) {
    let gpu = &mut res.gpu;

    let frame = match gpu.surface.get_current_texture() {
        Ok(f) => f,
        Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
            gpu.surface.configure(&gpu.device, &gpu.surface_config);
            return;
        }
        Err(_) => return,
    };

    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
    let zoom = scene.snapshot.zoom_level();
    let scale = dpi_scale * zoom;

    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();

    if (res.current_scale - scale).abs() > 0.01 {
        res.pipeline = StrataPipeline::new(
            &gpu.device, &gpu.queue, gpu.surface_config.format,
            BASE_FONT_SIZE * scale, &mut font_system,
        );
        res.current_scale = scale;
    }

    // Upload/unload images.
    for img in &scene.pending_images {
        res.pipeline.load_image_rgba(&gpu.device, &gpu.queue, img.width, img.height, &img.data);
    }
    for handle in &scene.pending_unloads {
        res.pipeline.unload_image(*handle);
    }

    res.pipeline.after_frame();
    res.pipeline.clear();
    res.pipeline.set_background(scene.background);

    populate_pipeline(&mut res.pipeline, &scene.snapshot, scene.selection.as_ref(), scale, &mut font_system);

    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Strata Staging Upload"),
    });
    res.pipeline.prepare(
        &gpu.device, &gpu.queue, &mut encoder,
        gpu.surface_config.width as f32, gpu.surface_config.height as f32,
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));

    let mut render_encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Strata Render"),
    });
    let clip = ClipBounds {
        x: 0, y: 0,
        width: gpu.surface_config.width,
        height: gpu.surface_config.height,
    };
    res.pipeline.render(&mut render_encoder, &view, &clip);
    gpu.queue.submit(std::iter::once(render_encoder.finish()));

    frame.present();
}

// ============================================================================
// Pipeline Population (from iced_adapter)
// ============================================================================

fn populate_pipeline(
    pipeline: &mut StrataPipeline,
    snapshot: &LayoutSnapshot,
    selection: Option<&Selection>,
    scale: f32,
    font_system: &mut cosmic_text::FontSystem,
) {
    let primitives = snapshot.primitives();

    #[inline]
    fn clip_to_gpu(clip: &Option<Rect>, scale: f32) -> Option<[f32; 4]> {
        clip.map(|c| [c.x * scale, c.y * scale, c.width * scale, c.height * scale])
    }
    #[inline]
    fn maybe_clip(pipeline: &mut StrataPipeline, start: usize, clip: &Option<Rect>, scale: f32) {
        if let Some(gpu_clip) = clip_to_gpu(clip, scale) {
            pipeline.apply_clip_since(start, gpu_clip);
        }
    }
    #[inline]
    fn hash_grid_row(row: &crate::layout_snapshot::GridRow) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for run in &row.runs {
            run.text.hash(&mut hasher);
            run.fg.hash(&mut hasher);
            run.bg.hash(&mut hasher);
            run.col_offset.hash(&mut hasher);
            run.cell_len.hash(&mut hasher);
            use crate::layout_snapshot::UnderlineStyle;
            let ul_bits: u8 = match run.style.underline {
                UnderlineStyle::None => 0, UnderlineStyle::Single => 1,
                UnderlineStyle::Double => 2, UnderlineStyle::Curly => 3,
                UnderlineStyle::Dotted => 4, UnderlineStyle::Dashed => 5,
            };
            let style_bits: u16 = (run.style.bold as u16)
                | ((run.style.italic as u16) << 1)
                | ((run.style.strikethrough as u16) << 2)
                | ((run.style.dim as u16) << 3)
                | ((ul_bits as u16) << 4);
            style_bits.hash(&mut hasher);
        }
        hasher.finish()
    }

    for decoration in snapshot.background_decorations() { render_decoration(pipeline, decoration, scale); }

    for prim in &primitives.shadows {
        let start = pipeline.instance_count();
        pipeline.add_shadow(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.blur_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.rounded_rects {
        let start = pipeline.instance_count();
        pipeline.add_rounded_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.circles {
        let start = pipeline.instance_count();
        pipeline.add_circle(prim.center.x * scale, prim.center.y * scale, prim.radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.solid_rects {
        let start = pipeline.instance_count();
        pipeline.add_solid_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.borders {
        let start = pipeline.instance_count();
        pipeline.add_border(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.border_width * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.lines {
        let start = pipeline.instance_count();
        pipeline.add_line_styled(prim.p1.x * scale, prim.p1.y * scale, prim.p2.x * scale, prim.p2.y * scale, prim.thickness * scale, prim.color, convert_line_style(prim.style));
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.polylines {
        let start = pipeline.instance_count();
        let scaled_points: Vec<[f32; 2]> = prim.points.iter().map(|p| [p.x * scale, p.y * scale]).collect();
        pipeline.add_polyline_styled(&scaled_points, prim.thickness * scale, prim.color, convert_line_style(prim.style));
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.images {
        let start = pipeline.instance_count();
        pipeline.add_image(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.handle, prim.corner_radius * scale, prim.tint);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }

    if let Some(sel) = selection {
        if !sel.is_collapsed() {
            for (r, clip) in &snapshot.selection_bounds(sel) {
                let start = pipeline.instance_count();
                let scaled = Rect { x: r.x * scale, y: r.y * scale, width: r.width * scale, height: r.height * scale };
                pipeline.add_solid_rects(&[scaled], crate::gpu::SELECTION_COLOR);
                maybe_clip(pipeline, start, clip, scale);
            }
        }
    }

    for (_source_id, source_layout) in snapshot.sources_in_order() {
        for item in &source_layout.items {
            if let crate::layout_snapshot::ItemLayout::Grid(grid_layout) = item {
                let grid_clip = &grid_layout.clip_rect;
                let cell_w = grid_layout.cell_width * scale;
                let cell_h = grid_layout.cell_height * scale;
                pipeline.ensure_grid_cache(grid_layout.cols, grid_layout.rows_content.len(), grid_layout.bounds.x);

                for (row_idx, row) in grid_layout.rows_content.iter().enumerate() {
                    if row.runs.is_empty() { continue; }
                    let signature = hash_grid_row(row);
                    let Some(build_start) = pipeline.begin_grid_row(row_idx, signature) else { continue; };
                    let row_y = (grid_layout.bounds.y + row_idx as f32 * grid_layout.cell_height) * scale;
                    let base_x = grid_layout.bounds.x * scale;

                    for run in &row.runs {
                        let run_x = base_x + run.col_offset as f32 * cell_w;
                        let run_w = run.cell_len as f32 * cell_w;
                        let is_whitespace = run.text.trim().is_empty();

                        if run.bg != 0 {
                            pipeline.add_solid_rect(run_x, row_y, run_w, cell_h, Color::unpack(run.bg));
                        }

                        let mut fg_color = Color::unpack(run.fg);
                        if run.style.dim { fg_color.a *= 0.5; }

                        if !is_whitespace {
                            let has_custom = run.text.chars().any(crate::gpu::is_custom_drawn);
                            if has_custom {
                                use unicode_width::UnicodeWidthChar;
                                let mut col = 0usize;
                                let mut text_buf = String::new();
                                let mut text_col_start = 0usize;
                                for ch in run.text.chars() {
                                    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                                    if ch_width == 0 { text_buf.push(ch); continue; }
                                    if crate::gpu::is_custom_drawn(ch) {
                                        if !text_buf.is_empty() {
                                            pipeline.add_text_grid(&text_buf, run_x + text_col_start as f32 * cell_w, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, font_system);
                                            text_buf.clear();
                                        }
                                        let cx = run_x + col as f32 * cell_w;
                                        if !pipeline.draw_box_char(ch, cx, row_y, cell_w, cell_h, fg_color)
                                            && !pipeline.draw_block_char(ch, cx, row_y, cell_w, cell_h, fg_color) {
                                            if text_buf.is_empty() { text_col_start = col; }
                                            text_buf.push(ch);
                                        }
                                        col += 1;
                                    } else {
                                        if text_buf.is_empty() { text_col_start = col; }
                                        text_buf.push(ch);
                                        col += ch_width;
                                    }
                                }
                                if !text_buf.is_empty() {
                                    pipeline.add_text_grid(&text_buf, run_x + text_col_start as f32 * cell_w, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, font_system);
                                }
                            } else {
                                pipeline.add_text_grid(&run.text, run_x, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, font_system);
                            }
                        }

                        {
                            use crate::layout_snapshot::UnderlineStyle;
                            let ul_thickness = scale.max(1.0);
                            match run.style.underline {
                                UnderlineStyle::None => {}
                                UnderlineStyle::Single | UnderlineStyle::Curly | UnderlineStyle::Dotted | UnderlineStyle::Dashed => {
                                    pipeline.add_solid_rect(run_x, row_y + cell_h * 0.85, run_w, ul_thickness, fg_color);
                                }
                                UnderlineStyle::Double => {
                                    let gap = (2.0 * scale).max(2.0);
                                    pipeline.add_solid_rect(run_x, row_y + cell_h * 0.82, run_w, ul_thickness, fg_color);
                                    pipeline.add_solid_rect(run_x, row_y + cell_h * 0.82 + gap, run_w, ul_thickness, fg_color);
                                }
                            }
                        }
                        if run.style.strikethrough {
                            pipeline.add_solid_rect(run_x, row_y + cell_h * 0.5, run_w, 1.0 * scale, fg_color);
                        }
                    }
                    pipeline.end_grid_row(row_idx, signature, build_start, row_y);
                }
                let grid_base_y = grid_layout.bounds.y * scale;
                pipeline.gather_grid_rows(grid_base_y, cell_h, grid_layout.rows_content.len(), clip_to_gpu(grid_clip, scale));
            }
        }
    }

    let viewport_bottom = snapshot.viewport().height;
    for prim in &primitives.text_runs {
        if prim.position.y > viewport_bottom || prim.position.y + prim.font_size * 1.5 < 0.0 { continue; }
        if let Some(clip) = &prim.clip_rect {
            if clip.y > viewport_bottom || (clip.y + clip.height) < 0.0 { continue; }
        }
        let start = pipeline.instance_count();
        pipeline.add_text(&prim.text, prim.position.x * scale, prim.position.y * scale, prim.color, prim.font_size * scale, font_system);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }

    for decoration in snapshot.foreground_decorations() { render_decoration(pipeline, decoration, scale); }

    let overlays = snapshot.overlay_primitives();
    for prim in &overlays.shadows {
        let start = pipeline.instance_count();
        pipeline.add_shadow(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.blur_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.rounded_rects {
        let start = pipeline.instance_count();
        pipeline.add_rounded_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.solid_rects {
        let start = pipeline.instance_count();
        pipeline.add_solid_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.borders {
        let start = pipeline.instance_count();
        pipeline.add_border(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.border_width * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.text_runs {
        let start = pipeline.instance_count();
        pipeline.add_text_styled(&prim.text, prim.position.x * scale, prim.position.y * scale, prim.color, prim.font_size * scale, prim.bold, prim.italic, font_system);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
}

fn render_decoration(pipeline: &mut StrataPipeline, decoration: &crate::layout_snapshot::Decoration, scale: f32) {
    use crate::layout_snapshot::Decoration;
    match decoration {
        Decoration::SolidRect { rect, color } => {
            pipeline.add_solid_rect(rect.x * scale, rect.y * scale, rect.width * scale, rect.height * scale, *color);
        }
        Decoration::RoundedRect { rect, corner_radius, color } => {
            pipeline.add_rounded_rect(rect.x * scale, rect.y * scale, rect.width * scale, rect.height * scale, corner_radius * scale, *color);
        }
        Decoration::Circle { center, radius, color } => {
            pipeline.add_circle(center.x * scale, center.y * scale, radius * scale, *color);
        }
    }
}

fn convert_line_style(style: crate::layout::primitives::LineStyle) -> crate::gpu::LineStyle {
    match style {
        crate::layout::primitives::LineStyle::Solid => crate::gpu::LineStyle::Solid,
        crate::layout::primitives::LineStyle::Dashed => crate::gpu::LineStyle::Dashed,
        crate::layout::primitives::LineStyle::Dotted => crate::gpu::LineStyle::Dotted,
    }
}

// ============================================================================
// Async Commands
// ============================================================================

fn spawn_commands<M: Send + 'static>(
    rt: &tokio::runtime::Runtime,
    mut cmd: Command<M>,
    tx: std::sync::mpsc::Sender<M>,
) {
    for fut in cmd.take_futures() {
        let tx = tx.clone();
        rt.spawn(async move {
            let msg = fut.await;
            let _ = tx.send(msg);
        });
    }
}

// ============================================================================
// Resize Idle Timer (CFRunLoopTimer)
// ============================================================================

// CoreFoundation FFI shared by both timers.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: *const c_void);
    fn CFRunLoopTimerCreate(
        allocator: *const c_void, fire_date: f64, interval: f64,
        flags: u64, order: i64,
        callout: extern "C" fn(*mut c_void, *mut c_void),
        context: *mut CFRunLoopTimerContext,
    ) -> *mut c_void;
    fn CFRunLoopTimerInvalidate(timer: *mut c_void);
    fn CFAbsoluteTimeGetCurrent() -> f64;
}

unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
}

#[repr(C)]
struct CFRunLoopTimerContext {
    version: i64,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

/// Reset (or create) the resize idle timer. Fires ~16ms after the last setFrameSize.
/// When it fires, we hide the overlay and do one presentDrawable render
/// (which works when the mouse is still during the resize tracking loop).
fn reset_resize_idle_timer(timer_ptr: &mut *mut c_void, state_info: *mut c_void) {
    // Invalidate any existing timer.
    if !timer_ptr.is_null() {
        unsafe { CFRunLoopTimerInvalidate(*timer_ptr); }
        *timer_ptr = std::ptr::null_mut();
    }

    unsafe {
        let mut context = CFRunLoopTimerContext {
            version: 0,
            info: state_info,
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };

        // One-shot timer: fire_date = now + 16ms, interval = 0 (non-repeating).
        let fire_date = CFAbsoluteTimeGetCurrent() + 0.016;
        let timer = CFRunLoopTimerCreate(
            std::ptr::null(), fire_date, 0.0,
            0, 0, resize_idle_timer_callback, &mut context,
        );

        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        *timer_ptr = timer;
    }
}

fn invalidate_resize_timer(timer_ptr: &mut *mut c_void) {
    if !timer_ptr.is_null() {
        unsafe { CFRunLoopTimerInvalidate(*timer_ptr); }
        *timer_ptr = std::ptr::null_mut();
    }
}

/// Resize idle timer callback. Called on main thread when mouse has been still
/// for ~16ms during resize. Hides overlay and does a normal presentDrawable render.
extern "C" fn resize_idle_timer_callback(_timer: *mut c_void, _info: *mut c_void) {
    // NOTE: This callback fires for ALL StrataApp instances, but we only have one.
    // The info pointer is the raw RefCell<WindowState<A>> pointer, but we don't
    // know A here. Instead, we use a type-erased handler set during install_event_handlers.
    unsafe {
        if let Some(handler) = RESIZE_IDLE_HANDLER {
            handler(_info);
        }
    }
}

/// Type-erased resize idle handler (set during install_event_handlers).
static mut RESIZE_IDLE_HANDLER: Option<fn(*mut c_void)> = None;

fn handle_resize_idle<A: StrataApp>(info: *mut c_void) {
    let state_cell = unsafe { &*(info as *const RefCell<WindowState<A>>) };
    let mut state = state_cell.borrow_mut();

    // Hide overlay, clear contents.
    let overlay = state.overlay_layer_ptr;
    if !overlay.is_null() {
        unsafe {
            let _: () = msg_send![overlay, setHidden: Bool::YES];
            let _: () = msg_send![overlay, setContents: std::ptr::null::<AnyObject>()];
        }
    }

    // Reconfigure surface and do one normal presentDrawable render
    // (works when mouse is still during the resize tracking loop).
    state.render.gpu.surface.configure(
        &state.render.gpu.device,
        &state.render.gpu.surface_config,
    );
    let scene = build_scene::<A>(&state);
    let dpi_scale = state.dpi_scale;
    render_frame(&mut state.render, &scene, dpi_scale);
}

// ============================================================================
// Main Thread Timer
// ============================================================================

fn install_main_thread_timer<A: StrataApp>(state_ptr: *mut RefCell<WindowState<A>>) {
    extern "C" fn timer_callback<A: StrataApp>(_timer: *mut c_void, info: *mut c_void) {
        let state_ptr = info as *mut RefCell<WindowState<A>>;
        let state_cell = unsafe { &*state_ptr };
        let mut state = state_cell.borrow_mut();

        // Drain pending async results.
        let mut messages = Vec::new();
        while let Ok(msg) = state.command_rx.try_recv() {
            messages.push(msg);
        }

        // Poll subscriptions for new events.
        let mut sub = A::subscription(&state.app);
        for stream in &mut sub.streams {
            while let Some(msg) = stream.try_recv() {
                messages.push(msg);
            }
        }

        if !messages.is_empty() {
            for msg in messages {
                process_message::<A>(&mut state, msg);
            }
        }

        // Render if needed.
        if state.needs_render {
            state.needs_render = false;

            if state.surface_dirty {
                state.surface_dirty = false;
                state.render.gpu.surface.configure(
                    &state.render.gpu.device,
                    &state.render.gpu.surface_config,
                );
            }

            let scene = build_scene::<A>(&state);
            state.cached_snapshot = Some(scene.snapshot.clone());

            // Drain pending images/unloads into the scene.
            let pending_images = state.image_store.drain_pending();
            let pending_unloads = state.image_store.drain_pending_unloads();
            let scene = Scene {
                pending_images,
                pending_unloads,
                ..scene
            };

            let dpi_scale = state.dpi_scale;
            render_frame(&mut state.render, &scene, dpi_scale);
        }
    }

    unsafe {
        let mut context = CFRunLoopTimerContext {
            version: 0,
            info: state_ptr as *mut c_void,
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };

        let timer = CFRunLoopTimerCreate(
            std::ptr::null(), CFAbsoluteTimeGetCurrent(), 0.001,
            0, 0, timer_callback::<A>, &mut context,
        );

        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
    }
}

// ============================================================================
// ClipBounds (replaces iced::Rectangle<u32>)
// ============================================================================

/// Clip rectangle in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct ClipBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

// ============================================================================
// Newtype wrappers for CoreGraphics pointers (objc2 type encoding validation)
// ============================================================================

/// Wrapper for IOSurfaceRef that satisfies objc2's type encoding checks.
/// The `iosurface` selector on MTLTexture returns `^{__IOSurface=}`, not `@`.
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct IOSurfacePtr(*mut c_void);

unsafe impl objc2::encode::Encode for IOSurfacePtr {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &objc2::encode::Encoding::Struct("__IOSurface", &[]),
    );
}

unsafe impl objc2::encode::RefEncode for IOSurfacePtr {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &<Self as objc2::encode::Encode>::ENCODING,
    );
}

/// Wrapper for CGColorSpaceRef that satisfies objc2's type encoding checks.
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct CGColorSpacePtr(*const c_void);

unsafe impl objc2::encode::Encode for CGColorSpacePtr {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &objc2::encode::Encoding::Struct("CGColorSpace", &[]),
    );
}

unsafe impl objc2::encode::RefEncode for CGColorSpacePtr {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &<Self as objc2::encode::Encode>::ENCODING,
    );
}
