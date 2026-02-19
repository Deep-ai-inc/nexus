//! Native macOS Backend
//!
//! Direct NSApplication + NSWindow + Metal backend. Renders on the main thread
//! with three-layer architecture for flicker-free resize (CAMetalLayer + overlay
//! CALayer for IOSurface during active resize).
//!
//! This is the ONLY module that bridges Strata to the macOS window system.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

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
use crate::layout_snapshot::{CursorIcon, HitResult, LayoutSnapshot};
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
    /// Pre-compiled Metal shader library (compiled once, reused on pipeline recreation).
    library: metal::Library,
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
    last_render_time: Instant,
    dpi_scale: f32,
    tokio_rt: Arc<tokio::runtime::Runtime>,
    command_tx: std::sync::mpsc::Sender<A::Message>,
    command_rx: std::sync::mpsc::Receiver<A::Message>,
    window: *mut AnyObject, // Weak back-pointer to NSWindow (prevent retain cycle)
    current_title: String, // Cached to avoid NSString allocation + ObjC call every tick
    current_cursor: CursorIcon, // Cached to avoid redundant NSCursor calls
    last_tick_time: Instant, // Rate-limit on_tick to display refresh rate
    tick_interval_ms: u64,   // Display refresh interval (e.g. 8 for 120Hz, 16 for 60Hz)
    pending_window_resize: Option<(f32, f32)>, // Deferred setContentSize (avoid reentrant borrow)
    poll_timer: *mut c_void, // CFRunLoopTimerRef for main-thread polling (invalidated on close)
    transactional_present: bool, // When true, render_if_needed uses presentsWithTransaction
}

// ============================================================================
// Multi-Window Globals (main thread only)
// ============================================================================

/// App-wide resources shared across all windows.
struct AppGlobals<A: StrataApp> {
    config: AppConfig,
    shared: A::SharedState,
    tokio_rt: Arc<tokio::runtime::Runtime>,
}

/// Type-erased pointer to AppGlobals<A>. Set once in run_with_config, never changed.
static mut APP_GLOBALS_PTR: *mut c_void = std::ptr::null_mut();

/// Monomorphized function pointer to create a new window. Set once in run_with_config.
static mut CREATE_WINDOW_FN: Option<fn()> = None;

/// Number of open windows. When decremented to 0, app stays alive for dock reopen.
static WINDOW_COUNT: AtomicUsize = AtomicUsize::new(0);


// ============================================================================
// Window Sizing & Positioning
// ============================================================================

/// Compute the initial content rect for a new window.
///
/// Size: 50% of the screen in each dimension (matching macOS corner-snap).
/// Position: near the mouse cursor, preferring empty space over existing windows.
fn compute_window_rect(style: NSWindowStyleMask, config: &AppConfig) -> (NSRect, f32, f32) {
    unsafe {
        let screen_class = AnyClass::get("NSScreen").unwrap();
        let event_class = AnyClass::get("NSEvent").unwrap();
        let window_class = AnyClass::get("NSWindow").unwrap();

        // Find the screen containing the mouse cursor.
        let mouse: NSPoint = msg_send![event_class, mouseLocation];
        let screens: *mut AnyObject = msg_send![screen_class, screens];
        let screen_count: usize = msg_send![screens, count];
        let mut screen: *mut AnyObject = msg_send![screen_class, mainScreen];
        for i in 0..screen_count {
            let s: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let f: NSRect = msg_send![s, frame];
            if mouse.x >= f.origin.x && mouse.x < f.origin.x + f.size.width
                && mouse.y >= f.origin.y && mouse.y < f.origin.y + f.size.height
            {
                screen = s;
                break;
            }
        }
        if screen.is_null() {
            let r = NSRect::new(
                NSPoint::new(200.0, 200.0),
                NSSize::new(config.window_size.0 as f64, config.window_size.1 as f64),
            );
            return (r, config.window_size.0, config.window_size.1);
        }

        let visible: NSRect = msg_send![screen, visibleFrame];

        // Window frame = 50% of visible screen in each dimension.
        // Convert to content size (subtracts title bar height).
        let frame_w = visible.size.width * 0.5;
        let frame_h = visible.size.height * 0.5;
        let frame_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(frame_w, frame_h));
        let content: NSRect = msg_send![window_class,
            contentRectForFrameRect: frame_rect styleMask: style];
        let win_w = content.size.width;
        let win_h = content.size.height;

        // Gather frames of existing visible windows in our app as obstacles.
        let app: *mut AnyObject = msg_send![
            AnyClass::get("NSApplication").unwrap(), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        let win_count: usize = msg_send![windows, count];
        let mut obstacles = Vec::new();
        for i in 0..win_count {
            let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let vis: bool = msg_send![w, isVisible];
            if vis {
                let f: NSRect = msg_send![w, frame];
                obstacles.push(f);
            }
        }

        // Candidate positions: the 4 screen corners plus centered on mouse.
        // With heavy overlap penalty, windows naturally fill corners first
        // (exclusion principle), using mouse proximity as tiebreaker.
        let vx = visible.origin.x;
        let vy = visible.origin.y;
        let vr = vx + visible.size.width;
        let vt = vy + visible.size.height;

        let candidates = [
            NSPoint::new(vx, vt - frame_h),             // top-left
            NSPoint::new(vr - frame_w, vt - frame_h),   // top-right
            NSPoint::new(vx, vy),                        // bottom-left
            NSPoint::new(vr - frame_w, vy),              // bottom-right
            // Centered on mouse (clamped below)
            NSPoint::new(mouse.x - frame_w / 2.0, mouse.y - frame_h / 2.0),
        ];

        let window_area = frame_w * frame_h;
        let mut best_origin = candidates[0];
        let mut best_score = f64::MAX;

        for origin in candidates {
            let x = origin.x.max(vx).min(vr - frame_w);
            let y = origin.y.max(vy).min(vt - frame_h);

            // Overlap fraction (0.0 = no overlap, 1.0 = fully covered).
            let mut overlap = 0.0_f64;
            for ob in &obstacles {
                let iw = ((x + frame_w).min(ob.origin.x + ob.size.width)
                    - x.max(ob.origin.x)).max(0.0);
                let ih = ((y + frame_h).min(ob.origin.y + ob.size.height)
                    - y.max(ob.origin.y)).max(0.0);
                overlap += iw * ih;
            }
            let overlap_frac = overlap / window_area;

            // Normalized distance (0.0 = on mouse, 1.0 = opposite corner).
            let cx = x + frame_w / 2.0;
            let cy = y + frame_h / 2.0;
            let diag = (visible.size.width.powi(2) + visible.size.height.powi(2)).sqrt();
            let dist = ((cx - mouse.x).powi(2) + (cy - mouse.y).powi(2)).sqrt() / diag;

            // Overlap dominates: any overlap outweighs any distance.
            let score = overlap_frac * 10.0 + dist;
            if score < best_score {
                best_score = score;
                best_origin = NSPoint::new(x, y);
            }
        }

        // best_origin is the frame origin; initWithContentRect wants content origin.
        // They differ by the title bar. Convert using contentRectForFrameRect.
        let best_frame = NSRect::new(best_origin, NSSize::new(frame_w, frame_h));
        let best_content: NSRect = msg_send![window_class,
            contentRectForFrameRect: best_frame styleMask: style];

        (best_content, win_w as f32, win_h as f32)
    }
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

    // Tokio runtime for async tasks (shared across all windows).
    let tokio_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Window(format!("Failed to create tokio runtime: {e}")))?,
    );

    // NSApplication setup.
    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // Store app-wide globals for window creation.
    let shared = A::SharedState::default();
    let globals = Box::new(AppGlobals::<A> {
        config: config.clone(),
        shared: shared.clone(),
        tokio_rt: tokio_rt.clone(),
    });
    unsafe {
        APP_GLOBALS_PTR = Box::into_raw(globals) as *mut c_void;
        CREATE_WINDOW_FN = Some(open_new_window::<A>);
    }

    // Install typed event handlers (before first window — handlers must be ready).
    install_event_handlers::<A>();

    // Create first window with initial app state.
    let mut image_store = ImageStore::new();
    let (app_state, init_cmd) = A::init(&shared, &mut image_store);
    open_new_window_with_state::<A>(app_state, init_cmd, image_store)?;

    // Set up native menu bar, force click, and app delegate.
    setup_native_menu_bar(mtm, &ns_app);
    crate::platform::macos::install_force_click_handler();
    crate::platform::macos::setup_force_click_monitor();
    install_app_delegate(mtm, &ns_app);

    // Enter tokio runtime context so tokio::spawn and async I/O work on main thread.
    let _tokio_guard = tokio_rt.enter();

    // Run the macOS event loop (blocks until app terminates).
    unsafe { ns_app.activate() };
    unsafe { ns_app.run() };

    Ok(())
}

/// Create a new window (called from Cmd+N, dock reopen, or NewWindow message).
/// Reads AppGlobals from the global static — must be called after run_with_config sets it up.
fn open_new_window<A: StrataApp>() {
    let globals = unsafe { &*(APP_GLOBALS_PTR as *const AppGlobals<A>) };
    let mut image_store = ImageStore::new();
    let Some((app_state, cmd)) = A::create_window(&globals.shared, &mut image_store) else {
        return; // App doesn't support multi-window
    };
    if let Err(e) = open_new_window_with_state::<A>(app_state, cmd, image_store) {
        eprintln!("Failed to create new window: {e}");
    }
}

/// Shared window creation logic for both initial and subsequent windows.
fn open_new_window_with_state<A: StrataApp>(
    app_state: A::State,
    init_cmd: Command<A::Message>,
    image_store: ImageStore,
) -> Result<(), Error> {
    let globals = unsafe { &*(APP_GLOBALS_PTR as *const AppGlobals<A>) };
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let config = &globals.config;

    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;

    // Size and position the window using gravitational best-fit:
    // - Size: 50% of screen in each dimension (quarter of screen area)
    // - Position: near the mouse cursor, avoiding overlap with existing windows
    let (content_rect, win_w, win_h) = compute_window_rect(style, config);

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            content_rect,
            style,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        )
    };

    // We manage the window lifetime manually — prevent AppKit from releasing
    // it during close (which would free it while the timer still holds a pointer).
    unsafe { let _: () = msg_send![&*window, setReleasedWhenClosed: Bool::NO]; }

    let bg_color = unsafe {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            config.background_color.r as CGFloat,
            config.background_color.g as CGFloat,
            config.background_color.b as CGFloat,
            1.0,
        )
    };
    window.setBackgroundColor(Some(&bg_color));
    window.setMinSize(NSSize::new(400.0, 300.0));

    let dpi_scale = window.backingScaleFactor() as f32;

    // Create custom NSView with layer hierarchy.
    let view_state = create_view_and_layers(mtm, &window, config, dpi_scale)?;

    // Set the view as the window's delegate (for windowWillClose: handling).
    unsafe {
        let _: () = msg_send![&*window, setDelegate: &*view_state.view];
    }

    // Initialize Metal.
    let gpu = init_metal(view_state.metal_layer_ptr, win_w, win_h, dpi_scale)?;

    // Compile Metal shader library and initialize pipeline.
    let scale = dpi_scale;
    let library = StrataPipeline::compile_library(&gpu.device);
    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();
    let pipeline = StrataPipeline::new(
        &gpu.device, &library, gpu.pixel_format,
        BASE_FONT_SIZE * scale, &mut font_system,
    );
    drop(font_system);

    let render = RenderResources {
        gpu,
        library,
        pipeline,
        current_scale: scale,
    };

    let (command_tx, command_rx) = std::sync::mpsc::channel();
    spawn_commands(&globals.tokio_rt, init_cmd, command_tx.clone());

    let window_ptr = &*window as *const NSWindow as *mut AnyObject;

    // Read the app's initial zoom level so the window is sized correctly.
    // base_size is the logical (unzoomed) content area; window_size = base_size * zoom.
    let initial_zoom = A::zoom_level(&app_state);
    let (base_w, base_h) = (win_w / initial_zoom, win_h / initial_zoom);

    let mut win_state = WindowState::<A> {
        app: app_state,
        shared: globals.shared.clone(),
        capture: CaptureState::None,
        window_size: (win_w, win_h),
        base_size: (base_w, base_h),
        current_zoom: initial_zoom,
        cursor_position: None,
        image_store,
        cached_snapshot: None,
        render,
        overlay_layer_ptr: view_state.overlay_layer_ptr,
        resize_timer: std::ptr::null_mut(),
        needs_render: true,
        surface_dirty: false,
        last_render_time: Instant::now(),
        dpi_scale,
        tokio_rt: globals.tokio_rt.clone(),
        command_tx: command_tx.clone(),
        command_rx,
        window: window_ptr,
        current_title: String::new(),
        current_cursor: CursorIcon::Arrow,
        last_tick_time: Instant::now(),
        tick_interval_ms: {
            let screen: *mut AnyObject = unsafe { msg_send![&*window, screen] };
            let max_fps: isize = if !screen.is_null() {
                unsafe { msg_send![screen, maximumFramesPerSecond] }
            } else { 0 };
            if max_fps > 0 { (1000 / max_fps) as u64 } else { 16 }
        },
        pending_window_resize: None,
        poll_timer: std::ptr::null_mut(),
        transactional_present: false,
    };

    // Render first frame synchronously before showing window.
    {
        let scene = build_scene::<A>(&win_state);
        win_state.cached_snapshot = Some(scene.snapshot.clone());
        render_frame(&mut win_state.render, &scene, dpi_scale, false);
        win_state.needs_render = false;
    }

    // Set window title from app state.
    let title = A::title(&win_state.app);
    window.setTitle(&NSString::from_str(&title));
    win_state.current_title = title;

    // Store state pointer in the view's ivar.
    let win_state_ptr = Box::into_raw(Box::new(RefCell::new(win_state)));
    unsafe {
        let view_ptr = (&*view_state.view) as *const AnyObject as *mut u8;
        let ivar = (*view_state.view).class().instance_variable("_strata_state")
            .expect("_strata_state ivar not found");
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut *mut c_void;
        *ivar_ptr = win_state_ptr as *mut c_void;
    }

    // Install per-window poll timer and store the reference for cleanup.
    let poll_timer = install_main_thread_timer::<A>(win_state_ptr);
    unsafe { (*win_state_ptr).borrow_mut().poll_timer = poll_timer; }

    window.makeKeyAndOrderFront(None);

    WINDOW_COUNT.fetch_add(1, Ordering::Relaxed);

    // Leak the Retained wrapper's +1 retain. Balanced by autorelease
    // in handle_window_close (deferred past the close call stack).
    std::mem::forget(window);

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
        // Mark layer opaque so the compositor ignores the alpha channel.
        let _: () = msg_send![metal_layer, setOpaque: Bool::YES];
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

    // Add NSTrackingArea for mouseMoved/mouseEntered/mouseExited.
    // NSTrackingInVisibleRect auto-adjusts to view bounds on resize.
    unsafe {
        let flags: usize = 0x01 | 0x02 | 0x80 | 0x200; // enteredAndExited | mouseMoved | activeAlways | inVisibleRect
        let tracking_class = AnyClass::get("NSTrackingArea").unwrap();
        let tracking_area: *mut AnyObject = msg_send![tracking_class, alloc];
        let tracking_area: *mut AnyObject = msg_send![tracking_area,
            initWithRect: NSRect::ZERO
            options: flags
            owner: &*view
            userInfo: std::ptr::null::<AnyObject>()
        ];
        let _: () = msg_send![&*view, addTrackingArea: tracking_area];
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
            add_method_raw(cls_ptr, sel!(performKeyEquivalent:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject) -> Bool, unsafe extern "C" fn()>(perform_key_equivalent)), c"B@:@");
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
            // NSWindowDelegate method — view acts as its window's delegate.
            add_method_raw(cls_ptr, sel!(windowWillClose:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(window_will_close)), c"v@:@");
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
static mut WINDOW_CLOSE_HANDLER: Option<fn(&AnyObject)> = None;

fn install_event_handlers<A: StrataApp>() {
    unsafe {
        MOUSE_HANDLER = Some(handle_mouse_event::<A>);
        KEY_HANDLER = Some(handle_key_event::<A>);
        RESIZE_HANDLER = Some(handle_resize::<A>);
        RESIZE_START_HANDLER = Some(handle_resize_start::<A>);
        RESIZE_END_HANDLER = Some(handle_resize_end::<A>);
        RESIZE_IDLE_HANDLER = Some(handle_resize_idle::<A>);
        WINDOW_CLOSE_HANDLER = Some(handle_window_close::<A>);
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

fn dispatch_window_close(view: &AnyObject) {
    unsafe {
        if let Some(handler) = WINDOW_CLOSE_HANDLER {
            handler(view);
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

/// Scale a MouseEvent's position from physical to logical coordinates.
fn scale_mouse_event(event: MouseEvent, zoom: f32) -> MouseEvent {
    let scale = |p: Point| Point::new(p.x / zoom, p.y / zoom);
    match event {
        MouseEvent::ButtonPressed { button, position, modifiers } =>
            MouseEvent::ButtonPressed { button, position: scale(position), modifiers },
        MouseEvent::ButtonReleased { button, position, modifiers } =>
            MouseEvent::ButtonReleased { button, position: scale(position), modifiers },
        MouseEvent::CursorMoved { position } =>
            MouseEvent::CursorMoved { position: scale(position) },
        MouseEvent::WheelScrolled { delta, position } =>
            MouseEvent::WheelScrolled { delta, position: scale(position) },
        other => other,
    }
}

/// Cast raw event pointer to &NSEvent reference (safe for ObjC callback args).
unsafe fn event_ref(raw: *mut AnyObject) -> &'static NSEvent {
    unsafe { &*(raw as *const NSEvent) }
}

extern "C" fn mouse_down(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    let mods = convert_ns_modifiers(event);
    dispatch_mouse(this, MouseEvent::ButtonPressed { button: MouseButton::Left, position: pos, modifiers: mods });
}

extern "C" fn mouse_up(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    let mods = convert_ns_modifiers(event);
    dispatch_mouse(this, MouseEvent::ButtonReleased { button: MouseButton::Left, position: pos, modifiers: mods });
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
    let mods = convert_ns_modifiers(event);
    dispatch_mouse(this, MouseEvent::ButtonPressed { button: MouseButton::Right, position: pos, modifiers: mods });
}

extern "C" fn right_mouse_up(this: &AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event = unsafe { event_ref(event) };
    let pos = event_position(event, this);
    let mods = convert_ns_modifiers(event);
    dispatch_mouse(this, MouseEvent::ButtonReleased { button: MouseButton::Right, position: pos, modifiers: mods });
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
        // Read NSEvent phase and momentumPhase as raw bitmasks.
        // NSEventPhase: Began=1, Changed=2, Stationary=4, Ended=8, Cancelled=16, MayBegin=32
        let phase_raw: usize = unsafe { msg_send![event, phase] };
        let momentum_raw: usize = unsafe { msg_send![event, momentumPhase] };

        // When a new finger touch interrupts ongoing momentum, macOS sends
        // momentumPhase=Ended AND phase=Began/Changed on the same event.
        // Detect this overlap first so we don't lose the new gesture start.
        let phase = if (momentum_raw == 8 || momentum_raw == 16)
            && (phase_raw == 1 || phase_raw == 2)
        {
            // New gesture interrupting momentum — treat as Contact
            Some(crate::event_context::ScrollPhase::Contact)
        } else if momentum_raw == 8 || momentum_raw == 16 {
            // Momentum ended or cancelled
            Some(crate::event_context::ScrollPhase::Ended)
        } else if momentum_raw != 0 {
            // Momentum began or changed
            Some(crate::event_context::ScrollPhase::Momentum)
        } else if phase_raw == 8 || phase_raw == 16 {
            // Gesture ended or cancelled, no momentum follows
            Some(crate::event_context::ScrollPhase::Ended)
        } else if phase_raw != 0 {
            // Finger on trackpad (began or changed)
            Some(crate::event_context::ScrollPhase::Contact)
        } else {
            None
        };

        ScrollDelta::Pixels { x: dx as f32, y: dy as f32, phase }
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

/// Handle Cmd+key shortcuts that macOS intercepts before `keyDown:`.
/// Cmd+. is the standard "Cancel" shortcut — AppKit eats it in the
/// responder chain so it never reaches `keyDown:`.  We claim it here
/// and return YES to prevent AppKit from consuming it.
extern "C" fn perform_key_equivalent(this: &AnyObject, _sel: Sel, event: *mut AnyObject) -> Bool {
    let event = unsafe { event_ref(event) };
    let flags = unsafe { event.modifierFlags() };
    if flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand) {
        let chars = unsafe { event.charactersIgnoringModifiers() };
        if let Some(s) = chars {
            let s = s.to_string();
            // Only intercept keys that AppKit would eat before keyDown:.
            // Most Cmd+key combos reach keyDown: fine; Cmd+. does not.
            if s == "." {
                if let Some(ke) = convert_ns_key_event(event, true) {
                    dispatch_key(this, ke);
                    return Bool::YES;
                }
            }
        }
    }
    Bool::NO
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

extern "C" fn window_will_close(this: &AnyObject, _sel: Sel, _notification: *mut AnyObject) {
    dispatch_window_close(this);
}

// ============================================================================
// Event Handling
// ============================================================================

/// Apply deferred window resize (must be called after dropping the state borrow,
/// since setContentSize triggers a synchronous setFrameSize callback).
fn flush_pending_resize<A: StrataApp>(state_cell: &RefCell<WindowState<A>>) {
    let pending = state_cell.borrow_mut().pending_window_resize.take();
    if let Some((w, h)) = pending {
        // Enable transactional presentation: the drawable and window geometry
        // change are committed atomically via CATransaction, preventing the
        // compositor from showing a frame where the window has new bounds but
        // stale Metal content (which manifests as a 1-frame vertical misalignment).
        let (window_ptr, layer_ptr) = {
            let mut state = state_cell.borrow_mut();
            let window_ptr = state.window;
            let layer_ptr = {
                use metal::foreign_types::ForeignTypeRef;
                state.render.gpu.layer.as_ptr() as *mut AnyObject
            };
            state.transactional_present = true;
            (window_ptr, layer_ptr)
        };

        if !window_ptr.is_null() {
            unsafe {
                // Enable presentsWithTransaction so drawable.present() defers
                // to the CA transaction commit instead of the next vsync.
                let _: () = msg_send![layer_ptr, setPresentsWithTransaction: true];

                let ca_transaction = AnyClass::get("CATransaction").unwrap();
                let _: () = msg_send![ca_transaction, begin];
                let _: () = msg_send![ca_transaction, setDisableActions: true];

                // setContentSize triggers setFrameSize → handle_resize, which
                // calls render_if_needed in transactional mode (commit + wait +
                // drawable.present). The drawable is registered with this transaction.
                let size = NSSize::new(w as f64, h as f64);
                let _: () = msg_send![window_ptr, setContentSize: size];

                // Commit: atomically applies window geometry + drawable.
                let _: () = msg_send![ca_transaction, commit];

                // Restore normal async presentation for subsequent renders.
                let _: () = msg_send![layer_ptr, setPresentsWithTransaction: false];
            }
        }

        state_cell.borrow_mut().transactional_present = false;
    }
}

fn handle_mouse_event<A: StrataApp>(view: &AnyObject, strata_event: MouseEvent) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    {
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

        // HACK: Area-based heuristic to decide whether a widget "claims" a click
        // or lets it pass through to nearest_content for browser-style gap selection.
        //
        // The right solution is a proper event bubbling/capturing system (like the
        // DOM): events dispatch to the most specific target, which can handle them
        // or let them propagate up to parent containers. Each widget would declare
        // whether it's interactive (claims clicks) or a passive layout container
        // (lets clicks pass through to content beneath it).
        //
        // What we do instead: use the widget's screen area as a proxy. Small widgets
        // (< 40k sq px — buttons, inputs, pills) are assumed interactive. Large
        // widgets (scroll areas, panels) are assumed to be passive containers. This
        // breaks if:
        //   - A large widget IS interactive (e.g., a big custom canvas)
        //   - A small widget is NOT interactive (unlikely but possible)
        //   - Widget sizes change dynamically across the threshold
        //
        // The 40k threshold mirrors INTERACTIVE_MAX_AREA in hit_test_xy(). If that
        // changes, this must change too.
        let hit = state.cached_snapshot.as_ref().and_then(|snapshot| {
            let raw_hit = adjusted_cursor.and_then(|pos| snapshot.hit_test(pos));
            let claims_click = match &raw_hit {
                Some(HitResult::Content(_)) => true,
                Some(HitResult::Widget(id)) => {
                    const INTERACTIVE_MAX_AREA: f32 = 40_000.0; // keep in sync with hit_test_xy
                    snapshot.widget_bounds(id)
                        .map(|r| r.width * r.height <= INTERACTIVE_MAX_AREA)
                        .unwrap_or(false)
                }
                None => false,
            };
            let needs_fallback = state.capture.is_captured()
                || (matches!(&strata_event, MouseEvent::ButtonPressed { .. }) && !claims_click);
            if needs_fallback {
                adjusted_cursor.and_then(|pos| snapshot.nearest_content(pos.x, pos.y)).or(raw_hit)
            } else {
                raw_hit
            }
        });

        let is_button_pressed = matches!(strata_event, MouseEvent::ButtonPressed { .. });
        let is_cursor_moved = matches!(strata_event, MouseEvent::CursorMoved { .. });
        let is_scroll = matches!(strata_event, MouseEvent::WheelScrolled { .. });
        if !hit.is_some() && !state.capture.is_captured() && !is_cursor_moved && !is_button_pressed {
            return;
        }

        // Convert event positions from physical to logical coordinates.
        // Layout and hit-testing operate in logical space; without this,
        // scrollbar drag and widget interactions are offset when zoom != 1.0.
        let logical_event = if zoom != 1.0 {
            scale_mouse_event(strata_event, zoom)
        } else {
            strata_event
        };
        let response = A::on_mouse(&state.app, logical_event, hit, &state.capture);

        match response.capture {
            CaptureRequest::Capture(source) => state.capture = CaptureState::Captured(source),
            CaptureRequest::Release => state.capture = CaptureState::None,
            CaptureRequest::None => {}
        }

        if let Some(msg) = response.message {
            process_message::<A>(&mut state, msg);
        }

        // Render synchronously for non-scroll events (clicks, drags, etc.)
        // where immediate visual feedback matters. Scroll events are deferred
        // to the tick-rate render in the timer callback — rendering on every
        // WheelScrolled would cause double-renders per vsync when PTY batches
        // also trigger renders, creating choppy frame pacing.
        if !is_scroll && state.pending_window_resize.is_none() {
            render_if_needed::<A>(&mut state);
        }

        // Update system cursor based on hover target.
        // Always call set_cursor — AppKit resets the cursor via cursor-rect
        // evaluation (on frame changes, focus changes, etc.) which can silently
        // undo our [NSCursor set] between events. Without re-asserting every
        // move, the dedup would see "same icon" and skip, leaving the arrow.
        if let Some(snapshot) = &state.cached_snapshot {
            let icon = if let Some(source) = state.capture.captured_by() {
                snapshot.cursor_for_capture(source)
            } else {
                adjusted_cursor.map(|p| snapshot.cursor_at(p)).unwrap_or_default()
            };
            state.current_cursor = icon;
            crate::platform::set_cursor(icon);
        }
    }
    flush_pending_resize::<A>(state_cell);
}

fn handle_key_event<A: StrataApp>(view: &AnyObject, key_event: KeyEvent) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    {
        let mut state = state_cell.borrow_mut();
        if let Some(msg) = A::on_key(&state.app, key_event) {
            process_message::<A>(&mut state, msg);
        }
        // Only render here for non-zoom key events (typing, shortcuts, etc.).
        // For zoom, skip — presenting a drawable at the old surface size would
        // give the compositor a stale frame. handle_resize renders at the
        // correct size inside setFrameSize when the pending resize flushes.
        if state.pending_window_resize.is_none() {
            render_if_needed::<A>(&mut state);
        }
    }
    flush_pending_resize::<A>(state_cell);
}

fn handle_resize<A: StrataApp>(view: &AnyObject, new_w: f32, new_h: f32) {
    let Some(state_cell) = (unsafe { get_state::<A>(view) }) else { return };
    let mut state = state_cell.borrow_mut();

    state.window_size = (new_w, new_h);
    let zoom = state.current_zoom;
    // Only update base_size if this looks like a manual resize (user dragged
    // the window edge). Zoom-triggered resizes land close to base_size * zoom —
    // preserve the existing base_size to avoid sub-pixel drift.
    let expected_w = state.base_size.0 * zoom;
    let expected_h = state.base_size.1 * zoom;
    if (new_w - expected_w).abs() > 2.0 || (new_h - expected_h).abs() > 2.0 {
        state.base_size = (new_w / zoom, new_h / zoom);
    }

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

        // Reconfigure layer at new size and sync render.
        state.render.gpu.surface_width = phys_w;
        state.render.gpu.surface_height = phys_h;
        state.render.gpu.layer.set_drawable_size(core_graphics_types::geometry::CGSize::new(phys_w as f64, phys_h as f64));
        render_sync_to_overlay(&mut state.render, &scene, overlay_layer, dpi_scale);

        // Reset the resize idle timer.
        let timer_info = state_cell as *const _ as *mut c_void;
        reset_resize_idle_timer(&mut state.resize_timer, timer_info);
    } else {
        // Programmatic resize (zoom, tiling, etc.) — render immediately at the
        // new size. For zoom, flush_pending_resize wraps this in a CATransaction
        // with presentsWithTransaction so the drawable and geometry are atomic.
        state.render.gpu.surface_width = phys_w;
        state.render.gpu.surface_height = phys_h;
        state.surface_dirty = true;
        state.needs_render = true;
        render_if_needed::<A>(&mut state);
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
    if A::is_new_window_request(&msg) {
        if let Some(f) = unsafe { CREATE_WINDOW_FN } { f(); }
        return; // Don't pass to update()
    }

    let cmd = A::update(&mut state.app, msg, &mut state.image_store);
    spawn_commands(&state.tokio_rt, cmd, state.command_tx.clone());

    let new_zoom = A::zoom_level(&state.app);
    if (new_zoom - state.current_zoom).abs() > 0.001 {
        state.current_zoom = new_zoom;
        // Defer window resize — setContentSize triggers setFrameSize synchronously,
        // which would re-enter borrow_mut on the RefCell. flush_pending_resize
        // wraps the resize in a CATransaction with presentsWithTransaction so
        // handle_resize → render_if_needed presents atomically with the geometry.
        let new_w = (state.base_size.0 * new_zoom).ceil().max(200.0);
        let new_h = (state.base_size.1 * new_zoom).ceil().max(150.0);
        state.pending_window_resize = Some((new_w, new_h));
        // Just flag render — skip scene build since surface dims are stale.
        // render_if_needed will build the scene at the correct size.
        state.needs_render = true;
    } else {
        // Just flag dirty — render_if_needed (called by the event handler or
        // timer callback) will build the scene once and render.  Avoids the
        // double scene-build that invalidate_and_request_render + render_if_needed
        // used to cause.
        state.needs_render = true;
    }
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

/// Render a frame if `needs_render` is set.
///
/// Reconfigures the drawable surface if `surface_dirty` is set (e.g. after a
/// zoom-triggered resize), builds the scene, drains pending images, and
/// presents. Called from both the timer callback and `handle_key_event` so
/// that zoom changes render in the same event — no stale frame visible.
fn render_if_needed<A: StrataApp>(state: &mut WindowState<A>) {
    if !state.needs_render { return; }
    state.needs_render = false;

    if state.surface_dirty {
        state.surface_dirty = false;
        state.render.gpu.layer.set_drawable_size(core_graphics_types::geometry::CGSize::new(
            state.render.gpu.surface_width as f64,
            state.render.gpu.surface_height as f64,
        ));
    }

    let scene = build_scene::<A>(state);
    state.cached_snapshot = Some(scene.snapshot.clone());

    let pending_images = state.image_store.drain_pending();
    let pending_unloads = state.image_store.drain_pending_unloads();
    let scene = Scene {
        pending_images,
        pending_unloads,
        ..scene
    };

    let dpi_scale = state.dpi_scale;
    render_frame(&mut state.render, &scene, dpi_scale, state.transactional_present);
    state.last_render_time = Instant::now();
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
// Metal Initialization
// ============================================================================

struct GpuState {
    device: metal::Device,
    queue: metal::CommandQueue,
    layer: metal::MetalLayer,
    pixel_format: metal::MTLPixelFormat,
    surface_width: u32,
    surface_height: u32,
    /// dispatch_semaphore_t for triple-buffered in-flight frame gating.
    in_flight_semaphore: *mut c_void,
}

unsafe extern "C" {
    fn dispatch_semaphore_create(value: isize) -> *mut c_void;
    fn dispatch_semaphore_wait(dsema: *mut c_void, timeout: u64) -> isize;
    fn dispatch_semaphore_signal(dsema: *mut c_void) -> isize;
}
/// DISPATCH_TIME_FOREVER
const DISPATCH_TIME_FOREVER: u64 = !0;

fn init_metal(
    metal_layer_ptr: *mut c_void,
    win_w: f32,
    win_h: f32,
    dpi_scale: f32,
) -> Result<GpuState, Error> {
    let device = metal::Device::system_default()
        .ok_or_else(|| Error::Gpu("No Metal device found".into()))?;
    let queue = device.new_command_queue();

    let pixel_format = metal::MTLPixelFormat::BGRA8Unorm_sRGB;
    let phys_w = (win_w * dpi_scale) as u32;
    let phys_h = (win_h * dpi_scale) as u32;

    // Wrap the existing CAMetalLayer. The layer was created via [CAMetalLayer layer]
    // (autoreleased) and retained by the view's layer hierarchy via addSublayer:.
    // from_ptr consumes a +1 retain, so we must retain first to give it its own reference.
    // Without this, dropping MetalLayer would over-release, freeing the layer while
    // the hierarchy still holds it — causing a crash when the view deallocs.
    use metal::foreign_types::ForeignType;
    unsafe { let _: *mut AnyObject = msg_send![metal_layer_ptr as *mut AnyObject, retain]; }
    let layer = unsafe { metal::MetalLayer::from_ptr(metal_layer_ptr as *mut _) };
    layer.set_device(&device);
    layer.set_pixel_format(pixel_format);
    layer.set_drawable_size(core_graphics_types::geometry::CGSize::new(phys_w as f64, phys_h as f64));
    layer.set_framebuffer_only(true);

    let in_flight_semaphore = unsafe { dispatch_semaphore_create(3) };

    Ok(GpuState {
        device,
        queue,
        layer,
        pixel_format,
        surface_width: phys_w,
        surface_height: phys_h,
        in_flight_semaphore,
    })
}

// ============================================================================
// IOSurface Extraction
// ============================================================================

/// Extract IOSurfaceRef from a Metal texture (the drawable's backing surface).
///
/// Extract the IOSurface backing a Metal texture via `msg_send!`.
/// Must be called BEFORE the drawable is dropped or presented.
fn extract_iosurface(texture: &metal::TextureRef) -> Option<IOSurfacePtr> {
    unsafe {
        use metal::foreign_types::ForeignTypeRef;
        let raw_ptr = texture.as_ptr() as *mut AnyObject;
        let iosurface: IOSurfacePtr = msg_send![raw_ptr, iosurface];
        if iosurface.0.is_null() { None } else { Some(iosurface) }
    }
}

/// Synchronous render to the overlay layer during resize.
///
/// Acquires a drawable from CAMetalLayer, renders the scene, waits for GPU
/// completion, extracts the IOSurface, and sets it on the overlay CALayer.
/// The frame is NOT presented — the drawable returns to the pool via drop.
fn render_sync_to_overlay(
    res: &mut RenderResources,
    scene: &Scene,
    overlay_layer: *mut AnyObject,
    dpi_scale: f32,
) {
    let gpu = &mut res.gpu;

    // Resize the drawable surface
    gpu.layer.set_drawable_size(core_graphics_types::geometry::CGSize::new(gpu.surface_width as f64, gpu.surface_height as f64));

    let drawable = match gpu.layer.next_drawable() {
        Some(d) => d,
        None => {
            eprintln!("Sync render: no drawable available");
            return;
        }
    };

    let zoom = scene.snapshot.zoom_level();
    let scale = dpi_scale * zoom;

    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();

    if (res.current_scale - scale).abs() > 0.01 {
        res.pipeline = StrataPipeline::new(
            &gpu.device, &res.library, gpu.pixel_format,
            BASE_FONT_SIZE * scale, &mut font_system,
        );
        res.current_scale = scale;
    }

    for img in &scene.pending_images {
        res.pipeline.load_image_rgba(&gpu.device, img.width, img.height, &img.data);
    }
    for handle in &scene.pending_unloads {
        res.pipeline.unload_image(*handle);
    }

    res.pipeline.clear();
    res.pipeline.set_background(scene.background);

    populate_pipeline(&mut res.pipeline, &scene.snapshot, scene.selection.as_ref(), scale, &mut font_system);
    drop(font_system);

    // Prepare (writes directly to unified memory buffers)
    res.pipeline.prepare(&gpu.device, gpu.surface_width as f32, gpu.surface_height as f32);

    // Render
    let cmd_buf = gpu.queue.new_command_buffer();
    let clip = ClipBounds { x: 0, y: 0, width: gpu.surface_width, height: gpu.surface_height };
    res.pipeline.render(cmd_buf, drawable.texture(), &clip);
    res.pipeline.advance_frame();

    // Wait for GPU completion, then extract IOSurface
    cmd_buf.commit();
    cmd_buf.wait_until_completed();

    if let Some(iosurface) = extract_iosurface(drawable.texture()) {
        unsafe {
            let contents_id = iosurface.0 as *mut AnyObject;
            let _: () = msg_send![overlay_layer, setContents: contents_id];
            let _: () = msg_send![overlay_layer, setHidden: Bool::NO];
        }
    }
    // Drop drawable WITHOUT presenting — returns to pool via refcount.
}

fn render_frame(res: &mut RenderResources, scene: &Scene, dpi_scale: f32, transactional: bool) {
    let gpu = &mut res.gpu;

    // Gate on semaphore — wait until a triple-buffer slot is free
    unsafe { dispatch_semaphore_wait(gpu.in_flight_semaphore, DISPATCH_TIME_FOREVER); }

    let drawable = match gpu.layer.next_drawable() {
        Some(d) => d,
        None => {
            // Drawable pool exhausted — signal semaphore back and skip frame
            unsafe { dispatch_semaphore_signal(gpu.in_flight_semaphore); }
            return;
        }
    };

    let zoom = scene.snapshot.zoom_level();
    let scale = dpi_scale * zoom;

    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();

    if (res.current_scale - scale).abs() > 0.01 {
        res.pipeline = StrataPipeline::new(
            &gpu.device, &res.library, gpu.pixel_format,
            BASE_FONT_SIZE * scale, &mut font_system,
        );
        res.current_scale = scale;
    }

    for img in &scene.pending_images {
        res.pipeline.load_image_rgba(&gpu.device, img.width, img.height, &img.data);
    }
    for handle in &scene.pending_unloads {
        res.pipeline.unload_image(*handle);
    }

    res.pipeline.clear();
    res.pipeline.set_background(scene.background);

    populate_pipeline(&mut res.pipeline, &scene.snapshot, scene.selection.as_ref(), scale, &mut font_system);
    drop(font_system);

    // Prepare (writes directly to unified memory buffers)
    res.pipeline.prepare(&gpu.device, gpu.surface_width as f32, gpu.surface_height as f32);

    // Render
    let cmd_buf = gpu.queue.new_command_buffer();
    let clip = ClipBounds { x: 0, y: 0, width: gpu.surface_width, height: gpu.surface_height };
    res.pipeline.render(cmd_buf, drawable.texture(), &clip);
    res.pipeline.advance_frame();

    if transactional {
        // Synchronous transactional present: the drawable is registered with the
        // enclosing CATransaction so it's composited atomically with the window
        // geometry change. Must wait for GPU completion before calling present().
        cmd_buf.commit();
        cmd_buf.wait_until_completed();
        unsafe {
            use metal::foreign_types::ForeignTypeRef;
            let drawable_ptr = drawable.as_ptr() as *mut AnyObject;
            let _: () = msg_send![drawable_ptr, present];
        }
        unsafe { dispatch_semaphore_signal(gpu.in_flight_semaphore); }
    } else {
        // Async present: drawable is presented at the next vsync.
        cmd_buf.present_drawable(&drawable);

        // Signal semaphore on GPU completion (non-blocking — runs on Metal's callback thread)
        let semaphore = gpu.in_flight_semaphore;
        let block = block2::StackBlock::new(move |_buf: *mut AnyObject| {
            unsafe { dispatch_semaphore_signal(semaphore); }
        });
        unsafe {
            use metal::foreign_types::ForeignTypeRef;
            let cmd_ptr = cmd_buf.as_ptr() as *mut AnyObject;
            let _: () = msg_send![cmd_ptr, addCompletedHandler: &*block];
        }

        cmd_buf.commit();
    }
}

// ============================================================================
// Grid Run Rendering Helpers
// ============================================================================

/// Render a grid text run's foreground: text glyphs (including custom-drawn
/// box/block characters), underlines, and strikethrough. Background is NOT
/// rendered — the caller handles that separately.
fn render_run_foreground(
    pipeline: &mut StrataPipeline,
    run: &crate::layout_snapshot::TextRun,
    base_x: f32,
    row_y: f32,
    fg_color: Color,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    font_system: &mut cosmic_text::FontSystem,
) {
    let run_x = base_x + run.col_offset as f32 * cell_w;
    let run_w = run.cell_len as f32 * cell_w;
    let is_whitespace = run.text.trim().is_empty();

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

/// Render grid selection overlay: opaque background + white text for selected cells.
/// Called after gather_grid_rows so the overlay draws on top of cached grid content.
///
/// For partially-selected runs, we render the full run text (preserving grapheme
/// clusters like emoji, flags, ZWJ sequences) and clip GPU instances to the
/// selection column boundaries. This avoids the character-level iteration that
/// would break multi-codepoint sequences.
fn render_grid_selection(
    pipeline: &mut StrataPipeline,
    snapshot: &LayoutSnapshot,
    source_id: &crate::content_address::SourceId,
    item_index: usize,
    grid_layout: &crate::layout_snapshot::GridLayout,
    selection: &Selection,
    scale: f32,
    font_system: &mut cosmic_text::FontSystem,
    grid_clip: &Option<Rect>,
) {
    let cell_count = grid_layout.cell_count();
    let Some((sel_start, sel_end)) = snapshot.grid_selection_offsets(
        source_id, item_index, cell_count, selection,
    ) else {
        return;
    };

    let cols = grid_layout.cols as usize;
    let cell_w = grid_layout.cell_width * scale;
    let cell_h = grid_layout.cell_height * scale;
    let base_x = grid_layout.bounds.x * scale;
    let base_y = grid_layout.bounds.y * scale;
    let sel_fg = crate::gpu::GRID_SELECTION_FG;
    let sel_bg = crate::gpu::GRID_SELECTION_BG;

    let gpu_grid_clip = grid_clip.map(|c| [c.x * scale, c.y * scale, c.width * scale, c.height * scale]);

    let (start_col, start_row) = grid_layout.offset_to_grid(sel_start);
    let (end_col, end_row) = grid_layout.offset_to_grid(sel_end.saturating_sub(1));
    let last_row = (end_row as usize).min(grid_layout.rows_content.len().saturating_sub(1));

    // For rectangular selection, compute fixed column range from x-coordinates
    let rect_col_range = if let crate::content_address::SelectionShape::Rectangular { x_min, x_max } = selection.shape {
        let col_start = ((x_min - grid_layout.bounds.x) / grid_layout.cell_width).floor().max(0.0) as usize;
        let col_end = ((x_max - grid_layout.bounds.x) / grid_layout.cell_width).ceil().min(cols as f32) as usize;
        Some((col_start, col_end))
    } else {
        None
    };

    for row_idx in (start_row as usize)..=last_row {
        let row = &grid_layout.rows_content[row_idx];
        let row_y = base_y + row_idx as f32 * grid_layout.cell_height * scale;

        // Selected column range for this row [start, end)
        let (row_sel_start, row_sel_end) = if let Some((cs, ce)) = rect_col_range {
            // Rectangular: every row gets the same column range
            (cs, ce)
        } else {
            // Linear: first/last rows may be partial
            let rs = if row_idx == start_row as usize { start_col as usize } else { 0 };
            let re = if row_idx == end_row as usize { end_col as usize + 1 } else { cols };
            (rs, re)
        };

        // 1. Opaque selection background (clipped to grid)
        let bg_inst = pipeline.instance_count();
        let bg_x = base_x + row_sel_start as f32 * cell_w;
        let bg_w = (row_sel_end - row_sel_start) as f32 * cell_w;
        pipeline.add_solid_rect(bg_x, row_y, bg_w, cell_h, sel_bg);
        if let Some(gc) = gpu_grid_clip {
            pipeline.apply_clip_since(bg_inst, gc);
        }

        // 2. Re-render text in white for selected cells
        for run in &row.runs {
            let run_start = run.col_offset as usize;
            let run_end = run_start + run.cell_len as usize;

            if run_end <= row_sel_start || run_start >= row_sel_end {
                continue; // Run doesn't overlap selection
            }

            let run_inst = pipeline.instance_count();

            // Always render the full run to preserve grapheme clusters.
            render_run_foreground(pipeline, run, base_x, row_y, sel_fg, cell_w, cell_h, scale, font_system);

            // For partial runs, clip to the selection column boundaries.
            // For full runs, just apply grid clip if present.
            if run_start < row_sel_start || run_end > row_sel_end {
                // Partial: clip to selection columns, intersected with grid clip
                let clip_x = base_x + row_sel_start.max(run_start) as f32 * cell_w;
                let clip_r = base_x + row_sel_end.min(run_end) as f32 * cell_w;
                let mut clip = [clip_x, row_y, clip_r - clip_x, cell_h];
                if let Some(gc) = gpu_grid_clip {
                    clip = intersect_clips(clip, gc);
                }
                pipeline.apply_clip_since(run_inst, clip);
            } else if let Some(gc) = gpu_grid_clip {
                pipeline.apply_clip_since(run_inst, gc);
            }
        }
    }
}

/// Intersect two clip rects [x, y, w, h]. Returns the overlapping region.
#[inline]
fn intersect_clips(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let r = (a[0] + a[2]).min(b[0] + b[2]);
    let bot = (a[1] + a[3]).min(b[1] + b[3]);
    [x, y, (r - x).max(0.0), (bot - y).max(0.0)]
}

// ============================================================================
// Pipeline Population
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

    for (source_id, source_layout) in snapshot.sources_in_order() {
        for (phys_idx, item) in source_layout.items.iter().enumerate() {
            let item_index = source_layout.logical_index(phys_idx);
            if let crate::layout_snapshot::ItemLayout::Grid(grid_layout) = item {
                let grid_clip = &grid_layout.clip_rect;
                let cell_w = grid_layout.cell_width * scale;
                let cell_h = grid_layout.cell_height * scale;
                pipeline.ensure_grid_cache(grid_layout.cols, grid_layout.rows_content.len(), grid_layout.bounds.x);

                // Row-level viewport culling: only hash/render rows that
                // intersect the clip rect. For a 5000-row terminal with
                // ~45 visible rows this skips >99% of per-row work.
                let num_rows = grid_layout.rows_content.len();
                let (first_vis, last_vis) = if let Some(ref clip) = *grid_clip {
                    let first = ((clip.y - grid_layout.bounds.y) / grid_layout.cell_height)
                        .floor().max(0.0) as usize;
                    let last = ((clip.y + clip.height - grid_layout.bounds.y) / grid_layout.cell_height)
                        .ceil().max(0.0) as usize;
                    (first.min(num_rows), last.min(num_rows))
                } else {
                    (0, num_rows)
                };

                for row_idx in first_vis..last_vis {
                    let row = &grid_layout.rows_content[row_idx];
                    if row.runs.is_empty() { continue; }
                    let signature = hash_grid_row(row);
                    let Some(build_start) = pipeline.begin_grid_row(row_idx, signature) else { continue; };
                    let row_y = (grid_layout.bounds.y + row_idx as f32 * grid_layout.cell_height) * scale;
                    let base_x = grid_layout.bounds.x * scale;

                    for run in &row.runs {
                        let run_x = base_x + run.col_offset as f32 * cell_w;
                        let run_w = run.cell_len as f32 * cell_w;

                        if run.bg != 0 {
                            pipeline.add_solid_rect(run_x, row_y, run_w, cell_h, Color::unpack(run.bg));
                        }

                        let mut fg_color = Color::unpack(run.fg);
                        if run.style.dim { fg_color.a *= 0.5; }

                        render_run_foreground(pipeline, run, base_x, row_y, fg_color, cell_w, cell_h, scale, font_system);
                    }
                    pipeline.end_grid_row(row_idx, signature, build_start, row_y);
                }
                let grid_base_y = grid_layout.bounds.y * scale;
                pipeline.gather_grid_rows(grid_base_y, cell_h, grid_layout.rows_content.len(), clip_to_gpu(grid_clip, scale));

                // Draw terminal cursor, clipped to the grid rect.
                if let Some(ref cursor) = grid_layout.cursor {
                    let cursor_start = pipeline.instance_count();
                    use crate::layout_snapshot::GridCursorShape;
                    let cx = (grid_layout.bounds.x + cursor.col as f32 * grid_layout.cell_width) * scale;
                    let cy = (grid_layout.bounds.y + cursor.row as f32 * grid_layout.cell_height) * scale;
                    let cursor_fg = if cursor.fg != 0 {
                        Color::unpack(cursor.fg)
                    } else {
                        Color::rgb(0.9, 0.9, 0.9)
                    };
                    let cursor_bg = if cursor.bg != 0 {
                        Color::unpack(cursor.bg)
                    } else {
                        Color::rgb(0.12, 0.12, 0.12)
                    };

                    match cursor.shape {
                        GridCursorShape::Block => {
                            // Solid block: fill with fg color, redraw char in bg color.
                            pipeline.add_solid_rect(cx, cy, cell_w, cell_h, cursor_fg);
                            if cursor.ch != ' ' && cursor.ch != '\0' {
                                let mut ch_buf = [0u8; 4];
                                let ch_str = cursor.ch.encode_utf8(&mut ch_buf);
                                pipeline.add_text_grid(ch_str, cx, cy, cursor_bg, BASE_FONT_SIZE * scale, false, false, font_system);
                            }
                        }
                        GridCursorShape::HollowBlock => {
                            let t = scale.max(1.0);
                            pipeline.add_solid_rect(cx, cy, cell_w, t, cursor_fg);               // top
                            pipeline.add_solid_rect(cx, cy + cell_h - t, cell_w, t, cursor_fg);   // bottom
                            pipeline.add_solid_rect(cx, cy, t, cell_h, cursor_fg);                // left
                            pipeline.add_solid_rect(cx + cell_w - t, cy, t, cell_h, cursor_fg);   // right
                        }
                        GridCursorShape::Beam => {
                            pipeline.add_solid_rect(cx, cy, (2.0 * scale).max(1.0), cell_h, cursor_fg);
                        }
                        GridCursorShape::Underline => {
                            pipeline.add_solid_rect(cx, cy + cell_h - (2.0 * scale).max(1.0), cell_w, (2.0 * scale).max(1.0), cursor_fg);
                        }
                    }
                    maybe_clip(pipeline, cursor_start, grid_clip, scale);
                }

                // Grid selection overlay: opaque background + white text.
                if let Some(sel) = selection {
                    render_grid_selection(
                        pipeline, snapshot, &source_id, item_index, grid_layout,
                        sel, scale, font_system, grid_clip,
                    );
                }
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

    // Text selection overlay (non-grid content only). Grid selections use
    // opaque background + white text rendered in the grid loop above.
    if let Some(sel) = selection {
        if !sel.is_collapsed() {
            for (r, clip) in &snapshot.text_selection_bounds(sel) {
                let start = pipeline.instance_count();
                let scaled = Rect { x: r.x * scale, y: r.y * scale, width: r.width * scale, height: r.height * scale };
                pipeline.add_solid_rects(&[scaled], crate::gpu::SELECTION_COLOR);
                maybe_clip(pipeline, start, clip, scale);
            }
            // Gap-filling rects between selected sources
            for (r, clip) in &snapshot.selection_gap_rects(sel) {
                let start = pipeline.instance_count();
                let scaled = Rect { x: r.x * scale, y: r.y * scale, width: r.width * scale, height: r.height * scale };
                pipeline.add_solid_rects(&[scaled], crate::gpu::SELECTION_COLOR);
                maybe_clip(pipeline, start, clip, scale);
            }
        }
    }

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

    // Reconfigure layer and do one normal presentDrawable render
    // (works when mouse is still during the resize tracking loop).
    state.render.gpu.layer.set_drawable_size(core_graphics_types::geometry::CGSize::new(
        state.render.gpu.surface_width as f64,
        state.render.gpu.surface_height as f64,
    ));
    let scene = build_scene::<A>(&state);
    let dpi_scale = state.dpi_scale;
    render_frame(&mut state.render, &scene, dpi_scale, false);
}

// ============================================================================
// Window Close Cleanup
// ============================================================================

/// Cleanup handler for windowWillClose:. Invalidates timers, drops Rust state,
/// and autoreleases the window (balancing the mem::forget from creation).
fn handle_window_close<A: StrataApp>(view: &AnyObject) {
    unsafe {
        // Get state pointer from ivar.
        let Some(ivar) = view.class().instance_variable("_strata_state") else { return };
        let state_ptr = *ivar.load::<*mut c_void>(view);
        if state_ptr.is_null() { return; } // Already cleaned up.

        // Null out the ivar FIRST to prevent reentrant access from callbacks
        // triggered during cleanup (e.g. setFrameSize: during orderOut).
        let view_ptr = view as *const AnyObject as *mut u8;
        let ivar_ptr = view_ptr.offset(ivar.offset()) as *mut *mut c_void;
        *ivar_ptr = std::ptr::null_mut();

        // Extract window pointer and invalidate timers before dropping state.
        let state_cell = &*(state_ptr as *const RefCell<WindowState<A>>);
        let window_ptr = {
            let mut state = state_cell.borrow_mut();

            // Invalidate the per-window poll timer so the callback never fires
            // again (it holds a raw pointer to this state).
            if !state.poll_timer.is_null() {
                CFRunLoopTimerInvalidate(state.poll_timer);
                state.poll_timer = std::ptr::null_mut();
            }

            // Invalidate resize timer if active.
            invalidate_resize_timer(&mut state.resize_timer);

            state.window
        };

        // Drop the state (releases GPU resources, app state, channels, etc.).
        // State only holds a raw *mut to the window — no release is sent.
        drop(Box::from_raw(state_ptr as *mut RefCell<WindowState<A>>));

        WINDOW_COUNT.fetch_sub(1, Ordering::Relaxed);

        // Autorelease the window to balance the mem::forget from creation.
        // We use autorelease (not release) because close is still on the call
        // stack — the pool drains after the current event finishes processing.
        // This is safe now because the timer is invalidated (the earlier segfault
        // was the timer callback sending setTitle: to a freed window, not the
        // release itself).
        if !window_ptr.is_null() {
            let _: *mut AnyObject = msg_send![window_ptr, autorelease];
        }
    }
}

// ============================================================================
// Main Thread Timer
// ============================================================================

fn install_main_thread_timer<A: StrataApp>(state_ptr: *mut RefCell<WindowState<A>>) -> *mut c_void {
    extern "C" fn timer_callback<A: StrataApp>(_timer: *mut c_void, info: *mut c_void) {
        // Autorelease pool: ensures temporary ObjC objects (NSEvent, NSString, etc.)
        // created by msg_send! during this tick are released deterministically rather
        // than lingering until the run loop's outer pool drains.
        objc2::rc::autoreleasepool(|_| {
            let state_ptr = info as *mut RefCell<WindowState<A>>;
            let state_cell = unsafe { &*state_ptr };
            // try_borrow_mut: if state is already borrowed (e.g. QuickLook modal
            // panel pumps the run loop during a mouse handler), skip this tick.
            let Ok(mut state) = state_cell.try_borrow_mut() else { return };

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

            // Poll force click events (thread-local queue, no channel needed).
            for (x, y) in crate::platform::macos::drain_force_click_events() {
                let zoom = state.current_zoom;
                let content_pos = Point::new(x / zoom, y / zoom);
                let hit = state.cached_snapshot.as_ref()
                    .and_then(|s| s.hit_test(content_pos));
                if let Some(HitResult::Content(addr)) = hit {
                    if let Some((word, word_start, font_size)) = A::force_click_lookup(&state.app, &addr) {
                        let popup_pos = state.cached_snapshot.as_ref()
                            .and_then(|s| s.char_bounds(&word_start))
                            .map(|rect| Point::new(rect.x * zoom, rect.y * zoom + font_size * 0.8))
                            .unwrap_or(Point::new(x, y));
                        let _ = crate::platform::macos::show_definition(&word, popup_pos, font_size * zoom);
                    }
                }
            }

            if !messages.is_empty() {
                for msg in messages {
                    process_message::<A>(&mut state, msg);
                }
            }

            // Call on_tick at the display's refresh rate for periodic effects
            // (spring animation, auto-scroll, output polling). Only flag a
            // render when on_tick reports state actually changed.
            let at_tick = state.last_tick_time.elapsed().as_millis() >= state.tick_interval_ms as u128;
            if at_tick {
                state.last_tick_time = Instant::now();
                if A::on_tick(&mut state.app) {
                    state.needs_render = true;
                }
            }

            // Update window title only when changed (avoids NSString alloc + ObjC call per tick).
            let new_title = A::title(&state.app);
            if new_title != state.current_title && !state.window.is_null() {
                unsafe {
                    let ns_title = NSString::from_str(&new_title);
                    let _: () = msg_send![state.window, setTitle: &*ns_title];
                }
                state.current_title = new_title;
            }

            // Drop the borrow before flushing deferred resize.
            // setContentSize triggers setFrameSize → handle_resize synchronously,
            // which borrows the state to update surface dimensions.
            drop(state);
            flush_pending_resize::<A>(state_cell);

            // Render at display refresh rate only. Scroll events and PTY
            // batches update state above but don't render immediately —
            // this single render-per-vsync combines all changes, avoiding
            // the double-render choppiness of event-driven rendering.
            // Key events still render synchronously in handle_key_event
            // for lowest typing latency.
            let Ok(mut state) = state_cell.try_borrow_mut() else { return };
            if at_tick {
                render_if_needed::<A>(&mut state);
            }

            // Update cursor after render (widgets may have shifted under cursor).
            // Always re-assert — AppKit can reset cursor between timer ticks.
            if let (Some(pos), Some(snapshot)) = (state.cursor_position, state.cached_snapshot.as_ref()) {
                let zoom = state.current_zoom;
                let adjusted = Point::new(pos.x / zoom, pos.y / zoom);
                let icon = if let Some(source) = state.capture.captured_by() {
                    snapshot.cursor_for_capture(source)
                } else {
                    snapshot.cursor_at(adjusted)
                };
                state.current_cursor = icon;
                crate::platform::set_cursor(icon);
            }
        });
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
        timer
    }
}

// ============================================================================
// ClipBounds
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

// ============================================================================
// Native Menu Bar
// ============================================================================

fn setup_native_menu_bar(mtm: MainThreadMarker, ns_app: &NSApplication) {
    use objc2_app_kit::{NSMenu, NSMenuItem};

    // Create menu bar.
    let menubar = NSMenu::new(mtm);

    // --- App menu (About, Hide, Quit) ---
    let app_menu = NSMenu::new(mtm);
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(), ns_string!("Quit"), Some(sel!(terminate:)), ns_string!("q"),
        )
    };
    app_menu.addItem(&quit);
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&app_menu));
    menubar.addItem(&app_item);

    // --- File menu ---
    let file_menu = NSMenu::new(mtm);
    unsafe { file_menu.setTitle(ns_string!("File")) };
    let new_window = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(), ns_string!("New Window"), Some(sel!(newDocument:)), ns_string!("n"),
        )
    };
    file_menu.addItem(&new_window);
    let close_window = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(), ns_string!("Close Window"), Some(sel!(performClose:)), ns_string!("w"),
        )
    };
    file_menu.addItem(&close_window);
    let file_item = NSMenuItem::new(mtm);
    file_item.setSubmenu(Some(&file_menu));
    menubar.addItem(&file_item);

    // --- Window menu ---
    let window_menu = NSMenu::new(mtm);
    unsafe { window_menu.setTitle(ns_string!("Window")) };
    let minimize = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(), ns_string!("Minimize"), Some(sel!(performMiniaturize:)), ns_string!("m"),
        )
    };
    window_menu.addItem(&minimize);
    let window_item = NSMenuItem::new(mtm);
    window_item.setSubmenu(Some(&window_menu));
    menubar.addItem(&window_item);
    unsafe { ns_app.setWindowsMenu(Some(&window_menu)) };

    ns_app.setMainMenu(Some(&menubar));
}

// ============================================================================
// Application Delegate (dock reopen, Cmd+N, quit behavior)
// ============================================================================

fn register_app_delegate_class() -> &'static AnyClass {
    static CLASS: std::sync::OnceLock<&'static AnyClass> = std::sync::OnceLock::new();
    CLASS.get_or_init(|| {
        let superclass = AnyClass::get("NSObject").unwrap();
        let builder = ClassBuilder::new("StrataAppDelegate", superclass).unwrap();

        let cls = builder.register();
        let cls_ptr = cls as *const _ as *mut objc2::ffi::objc_class;

        unsafe fn add_method_raw(
            cls: *mut objc2::ffi::objc_class,
            sel: Sel,
            imp: objc2::ffi::IMP,
            types: &std::ffi::CStr,
        ) {
            unsafe { objc2::ffi::class_addMethod(cls, sel.as_ptr(), imp, types.as_ptr()); }
        }

        unsafe {
            // Keep app alive after last window closes (for dock reopen).
            add_method_raw(cls_ptr, sel!(applicationShouldTerminateAfterLastWindowClosed:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject) -> Bool, unsafe extern "C" fn()>(
                    app_should_terminate_after_last_window_closed)), c"B@:@");
            // Dock click with no visible windows → open a new window.
            add_method_raw(cls_ptr, sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject, Bool) -> Bool, unsafe extern "C" fn()>(
                    app_should_handle_reopen)), c"B@:@B");
            // Cmd+N routes here via responder chain (newDocument: on NSApplication).
            add_method_raw(cls_ptr, sel!(newDocument:),
                Some(std::mem::transmute::<extern "C" fn(&AnyObject, Sel, *mut AnyObject), unsafe extern "C" fn()>(
                    new_document)), c"v@:@");
        }

        cls
    })
}

extern "C" fn app_should_terminate_after_last_window_closed(
    _this: &AnyObject, _sel: Sel, _app: *mut AnyObject,
) -> Bool {
    Bool::NO
}

extern "C" fn app_should_handle_reopen(
    _this: &AnyObject, _sel: Sel, _app: *mut AnyObject, has_visible_windows: Bool,
) -> Bool {
    if !has_visible_windows.as_bool() {
        if let Some(f) = unsafe { CREATE_WINDOW_FN } { f(); }
    }
    Bool::YES
}

extern "C" fn new_document(_this: &AnyObject, _sel: Sel, _sender: *mut AnyObject) {
    if let Some(f) = unsafe { CREATE_WINDOW_FN } { f(); }
}

fn install_app_delegate(_mtm: MainThreadMarker, ns_app: &NSApplication) {
    let delegate_class = register_app_delegate_class();
    let delegate: Retained<AnyObject> = unsafe {
        let raw: *mut AnyObject = msg_send![delegate_class, alloc];
        let raw: *mut AnyObject = msg_send![raw, init];
        Retained::from_raw(raw).expect("Failed to create StrataAppDelegate")
    };
    unsafe {
        let _: () = msg_send![ns_app, setDelegate: &*delegate];
    }
    // Keep delegate alive for the app's lifetime.
    std::mem::forget(delegate);
}
