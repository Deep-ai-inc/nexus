//! macOS native platform integration.
//!
//! Provides:
//! - Native drag source using NSPasteboard + NSDraggingSession
//! - Quick Look preview using QLPreviewPanel with proper delegate
//!
//! Uses `[NSApp currentEvent]` and `[[NSApp mainWindow] contentView]` to
//! initiate OS-level drags via the native NSView/NSEvent API.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, Once, mpsc};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSColor, NSDraggingItem, NSDraggingSession,
    NSImage, NSMenu, NSMenuItem, NSPasteboardItem, NSPasteboardTypeFileURL,
    NSPasteboardTypeString, NSPasteboardWriting,
    NSViewLayerContentsRedrawPolicy, NSWorkspace,
};
use objc2_foundation::{
    CGFloat, MainThreadMarker, NSArray, NSInteger, NSObject, NSPoint, NSRect, NSSize, NSString,
    NSURL, ns_string,
};

use crate::app::DragSource;

// =============================================================================
// Global State for Quick Look
// =============================================================================

/// Current file being previewed (accessed by QLPreviewPanel data source)
static QUICKLOOK_STATE: Mutex<QuickLookState> = Mutex::new(QuickLookState {
    path: None,
    source_rect: None,
});

struct QuickLookState {
    path: Option<PathBuf>,
    source_rect: Option<NSRect>,
}

// =============================================================================
// QLPreviewItem Implementation
// =============================================================================

declare_class!(
    /// A preview item that wraps a file URL for Quick Look.
    struct NexusPreviewItem;

    // SAFETY: NSObject is the superclass
    unsafe impl ClassType for NexusPreviewItem {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "NexusPreviewItem";
    }

    impl DeclaredClass for NexusPreviewItem {
        type Ivars = ();
    }

    // QLPreviewItem protocol methods
    unsafe impl NexusPreviewItem {
        #[method_id(previewItemURL)]
        fn preview_item_url(&self) -> Option<Retained<NSURL>> {
            let state = QUICKLOOK_STATE.lock().unwrap();
            state.path.as_ref().and_then(|path| {
                path.to_str().map(|s| {
                    let ns_path = NSString::from_str(s);
                    unsafe { NSURL::fileURLWithPath(&ns_path) }
                })
            })
        }

        #[method_id(previewItemTitle)]
        fn preview_item_title(&self) -> Option<Retained<NSString>> {
            let state = QUICKLOOK_STATE.lock().unwrap();
            state.path.as_ref().and_then(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(NSString::from_str)
            })
        }
    }
);

unsafe impl NSObjectProtocol for NexusPreviewItem {}

// =============================================================================
// QLPreviewPanel Data Source
// =============================================================================

declare_class!(
    /// Data source for QLPreviewPanel that provides preview items.
    struct NexusPreviewDataSource;

    unsafe impl ClassType for NexusPreviewDataSource {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "NexusPreviewDataSource";
    }

    impl DeclaredClass for NexusPreviewDataSource {
        type Ivars = ();
    }

    // QLPreviewPanelDataSource protocol methods
    unsafe impl NexusPreviewDataSource {
        #[method(numberOfPreviewItemsInPreviewPanel:)]
        fn number_of_items(&self, _panel: *mut AnyObject) -> NSInteger {
            let state = QUICKLOOK_STATE.lock().unwrap();
            if state.path.is_some() { 1 } else { 0 }
        }

        #[method_id(previewPanel:previewItemAtIndex:)]
        fn preview_item_at_index(&self, _panel: *mut AnyObject, _index: NSInteger) -> Retained<NexusPreviewItem> {
            // Create and return a preview item
            let item: Retained<NexusPreviewItem> = unsafe {
                msg_send_id![NexusPreviewItem::class(), new]
            };
            item
        }
    }

    // QLPreviewPanelDelegate protocol methods (optional, for source frame animation)
    unsafe impl NexusPreviewDataSource {
        #[method(previewPanel:sourceFrameOnScreenForPreviewItem:)]
        fn source_frame(&self, _panel: *mut AnyObject, _item: *mut AnyObject) -> NSRect {
            let state = QUICKLOOK_STATE.lock().unwrap();
            state.source_rect.unwrap_or(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(0.0, 0.0),
            ))
        }
    }
);

unsafe impl NSObjectProtocol for NexusPreviewDataSource {}

// =============================================================================
// Quick Look Public API
// =============================================================================

/// Show a Quick Look preview for a file using the native QLPreviewPanel.
///
/// Provides:
/// - Instant display
/// - Click-outside-to-dismiss
/// - Zoom animation from source_rect (if provided)
///
/// If Quick Look is already showing the same file, toggles it off.
pub fn preview_file(path: &Path) -> Result<(), String> {
    show_quicklook_native(path, None)
}

/// Show Quick Look with a source rect for zoom animation.
pub fn preview_file_with_rect(path: &Path, source_rect: NSRect) -> Result<(), String> {
    show_quicklook_native(path, Some(source_rect))
}

/// Ensure the QuickLook framework is loaded (lazy, idempotent).
fn ensure_quicklook_loaded() {
    use std::sync::Once;
    static LOAD: Once = Once::new();
    LOAD.call_once(|| {
        unsafe {
            let path = std::ffi::CStr::from_bytes_with_nul_unchecked(
                b"/System/Library/Frameworks/Quartz.framework/Quartz\0",
            );
            libc::dlopen(path.as_ptr(), libc::RTLD_LAZY);
        }
    });
}

fn show_quicklook_native(path: &Path, source_rect: Option<NSRect>) -> Result<(), String> {
    // Snapshot previous path for same-file toggle detection
    let previous_path = {
        let state = QUICKLOOK_STATE.lock().unwrap();
        state.path.clone()
    };

    // Update global state with new path
    {
        let mut state = QUICKLOOK_STATE.lock().unwrap();
        state.path = Some(path.to_path_buf());
        state.source_rect = source_rect;
    }

    ensure_quicklook_loaded();

    unsafe {
        // Get QLPreviewPanel class (safe lookup, no panic)
        let ql_class = AnyClass::get("QLPreviewPanel")
            .ok_or("QLPreviewPanel class not available")?;

        // Get shared panel: [QLPreviewPanel sharedPreviewPanel]
        let panel: *mut AnyObject = msg_send![ql_class, sharedPreviewPanel];
        if panel.is_null() {
            return Err("Failed to get QLPreviewPanel".into());
        }

        // Check if panel is visible
        let is_visible: Bool = msg_send![panel, isVisible];

        if is_visible.as_bool() {
            if previous_path.as_deref() == Some(path) {
                // Same file — toggle off
                let _: () = msg_send![panel, orderOut: std::ptr::null::<AnyObject>()];
                // Clear stored path so data source reports 0 items while hidden
                QUICKLOOK_STATE.lock().unwrap().path = None;
            } else {
                // Different file — switch preview in place
                let _: () = msg_send![panel, reloadData];
                let _: () = msg_send![panel, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
            }
        } else {
            // Not visible — set up data source and show
            let data_source: Retained<NexusPreviewDataSource> =
                msg_send_id![NexusPreviewDataSource::class(), new];

            let _: () = msg_send![panel, setDataSource: &*data_source];
            let _: () = msg_send![panel, setDelegate: &*data_source];

            let _: () = msg_send![panel, reloadData];
            let _: () = msg_send![panel, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];

            // data_source released here, but QLPreviewPanel retains its delegate internally
        }

        Ok(())
    }
}

/// Close the Quick Look panel if it's open.
pub fn close_quicklook() {
    let Some(ql_class) = AnyClass::get("QLPreviewPanel") else {
        return; // Framework not loaded, nothing to close
    };
    unsafe {
        let exists: Bool = msg_send![ql_class, sharedPreviewPanelExists];
        if exists.as_bool() {
            let panel: *mut AnyObject = msg_send![ql_class, sharedPreviewPanel];
            if !panel.is_null() {
                let _: () = msg_send![panel, orderOut: std::ptr::null::<AnyObject>()];
                QUICKLOOK_STATE.lock().unwrap().path = None;
            }
        }
    }
}

/// Convert a local (window-relative) rect to screen coordinates.
///
/// macOS uses a coordinate system with origin at bottom-left of the screen,
/// while our local coordinates have origin at top-left of the window content.
fn local_to_screen_rect(local_rect: crate::primitives::Rect) -> Option<NSRect> {
    // Safety: we are on the main thread
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let window = app.mainWindow()?;
        let content_view = window.contentView()?;

        // Get content view's bounds height for coordinate flip
        let content_bounds = content_view.bounds();
        let content_height = content_bounds.size.height;

        // Convert from top-left origin (our system) to bottom-left origin (NSView)
        // In our system: y=0 is at top, y increases downward
        // In NSView: y=0 is at bottom, y increases upward
        let flipped_y = content_height - local_rect.y as f64 - local_rect.height as f64;

        let view_rect = NSRect::new(
            NSPoint::new(local_rect.x as f64, flipped_y),
            NSSize::new(local_rect.width as f64, local_rect.height as f64),
        );

        // Use NSWindow's convertRectToScreen to properly convert to screen coordinates
        let screen_rect: NSRect = msg_send![&*window, convertRectToScreen: view_rect];

        Some(screen_rect)
    }
}

/// Show Quick Look with a local (window-relative) rect for zoom animation.
/// Converts the local rect to screen coordinates internally.
pub fn preview_file_with_local_rect(path: &Path, local_rect: crate::primitives::Rect) -> Result<(), String> {
    if let Some(screen_rect) = local_to_screen_rect(local_rect) {
        show_quicklook_native(path, Some(screen_rect))
    } else {
        // Fallback: no animation
        show_quicklook_native(path, None)
    }
}

// =============================================================================
// Drag and Drop
// =============================================================================

/// Initiate an OS-level outbound drag.
///
/// Must be called on the main thread during event processing so that
/// `[NSApp currentEvent]` returns the triggering mouse event.
pub fn start_drag(source: &DragSource) -> Result<(), String> {
    // Safety: we are on the main thread (called from native backend update loop).
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    unsafe {
        let app = NSApplication::sharedApplication(mtm);

        // Get the current event (the mouse event that triggered the drag).
        let current_event = app
            .currentEvent()
            .ok_or("No current event — start_drag must be called during event processing")?;

        // Get the content view of the main window (this is winit's WinitView / NSView).
        let window = app
            .mainWindow()
            .ok_or("No main window")?;
        let ns_view = window
            .contentView()
            .ok_or("No content view on main window")?;

        // Build the pasteboard item based on the drag source type.
        let pb_item = NSPasteboardItem::new();
        let drag_image: Retained<NSImage>;

        match source {
            DragSource::File(path) => {
                set_file_url_on_pasteboard(&pb_item, path)?;
                drag_image = file_icon(path);
            }
            DragSource::Text(text) => {
                // Write temp file so winit FileDropped works for internal round-trip
                let temp_path = write_drag_temp_file("drag.txt", text.as_bytes())
                    .map_err(|e| format!("Failed to write drag temp file: {}", e))?;
                set_file_url_on_pasteboard(&pb_item, &temp_path)?;
                // Also set text for apps that accept plain text
                let ns_text = NSString::from_str(text);
                pb_item.setString_forType(&ns_text, NSPasteboardTypeString);
                drag_image = file_icon(&temp_path);
            }
            DragSource::Tsv(tsv) => {
                let temp_path = write_drag_temp_file("drag.tsv", tsv.as_bytes())
                    .map_err(|e| format!("Failed to write drag temp file: {}", e))?;
                set_file_url_on_pasteboard(&pb_item, &temp_path)?;
                let ns_text = NSString::from_str(tsv);
                pb_item.setString_forType(&ns_text, NSPasteboardTypeString);
                drag_image = file_icon(&temp_path);
            }
            DragSource::Image(path) => {
                set_file_url_on_pasteboard(&pb_item, path)?;
                drag_image = file_icon(path);
            }
        }

        // Create the dragging item.
        // NSPasteboardItem conforms to NSPasteboardWriting — cast via ProtocolObject.
        let pb_writer: &ProtocolObject<dyn NSPasteboardWriting> =
            ProtocolObject::from_ref(&*pb_item);
        let drag_item = NSDraggingItem::initWithPasteboardWriter(
            NSDraggingItem::alloc(),
            pb_writer,
        );

        // Set the drag image frame (centered near cursor).
        let image_size = drag_image.size();
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(image_size.width.min(64.0), image_size.height.min(64.0)),
        );
        drag_item.setDraggingFrame_contents(frame, Some(&*drag_image));

        // Start the dragging session.
        // We use msg_send_id! to call beginDraggingSessionWithItems:event:source:
        // because winit's WinitView conforms to NSDraggingSource at the ObjC level,
        // but we can't express this in Rust's type system (the class is private to winit).
        let items = NSArray::from_vec(vec![drag_item]);
        let _session: Retained<NSDraggingSession> = msg_send_id![
            &*ns_view,
            beginDraggingSessionWithItems: &*items,
            event: &*current_event,
            source: &*ns_view  // WinitView conforms to NSDraggingSource
        ];

        Ok(())
    }
}

/// Read file URLs from the general pasteboard and return the first path that
/// points to an image file.  Returns `None` when the clipboard doesn't
/// contain a file URL or the file isn't an image.
pub fn clipboard_image_file_path() -> Option<PathBuf> {
    use objc2_app_kit::NSPasteboard;

    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        let items = pb.pasteboardItems()?;

        // Debug: log all pasteboard types so we can diagnose mismatches.
        #[cfg(debug_assertions)]
        for i in 0..items.len() {
            let item: &NSPasteboardItem = &items[i];
            let types = item.types();
            let type_strs: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            eprintln!("[paste] item {i} types: {type_strs:?}");
        }

        for i in 0..items.len() {
            let item: &NSPasteboardItem = &items[i];
            if let Some(path) = file_url_from_item(item) {
                if is_image_extension(&path) {
                    return Some(path);
                }
            }
        }
        None
    }
}

fn file_url_from_item(item: &NSPasteboardItem) -> Option<PathBuf> {
    let url_str = unsafe { item.stringForType(NSPasteboardTypeFileURL) }?;

    // macOS may return a file-reference URL (file:///.file/id=...) instead of
    // a path-based URL.  Resolve it via NSURL.filePathURL → .path.
    unsafe {
        let ns_url_str = NSString::from_str(&url_str.to_string());
        let url: Option<Retained<NSURL>> = msg_send_id![
            NSURL::class(), URLWithString: &*ns_url_str
        ];
        let url = url?;
        // filePathURL resolves file-reference URLs to path-based URLs.
        let path_url: Option<Retained<NSURL>> = msg_send_id![&url, filePathURL];
        let path_url = path_url.as_ref().unwrap_or(&url);
        let ns_path: Option<Retained<NSString>> = msg_send_id![path_url, path];
        let path = PathBuf::from(ns_path?.to_string());
        #[cfg(debug_assertions)]
        eprintln!("[paste] resolved path = {path:?}");
        Some(path)
    }
}

fn is_image_extension(path: &Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_ascii_lowercase(),
        None => return false,
    };
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif" | "heic" | "svg")
}

/// Set a file URL on a pasteboard item.
fn set_file_url_on_pasteboard(pb_item: &NSPasteboardItem, path: &Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("Non-UTF8 path")?;
    let ns_path = NSString::from_str(path_str);
    let url = unsafe { NSURL::fileURLWithPath(&ns_path) };
    let url_str = unsafe { url.absoluteString() }
        .ok_or("Failed to get absolute URL string")?;
    let success: bool = unsafe {
        pb_item.setString_forType(&url_str, NSPasteboardTypeFileURL)
    };
    if !success {
        return Err("Failed to set file URL on pasteboard".into());
    }
    Ok(())
}

/// Get the Finder icon for a file path.
fn file_icon(path: &Path) -> Retained<NSImage> {
    let ws = unsafe { NSWorkspace::sharedWorkspace() };
    let ns_path = NSString::from_str(path.to_str().unwrap_or(""));
    unsafe { ws.iconForFile(&ns_path) }
}

/// Write drag data to a temp file for pasteboard use.
/// Cleans any stale files from previous drags before writing.
fn write_drag_temp_file(filename: &str, data: &[u8]) -> Result<PathBuf, std::io::Error> {
    let temp_dir = std::env::temp_dir().join("nexus-drag");
    // Clean stale files from previous drags
    if temp_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&temp_dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    std::fs::create_dir_all(&temp_dir)?;
    let path = temp_dir.join(filename);
    std::fs::write(&path, data)?;
    Ok(path)
}

// =============================================================================
// Dock Icon Reopen Handler
// =============================================================================

/// Sender for the reopen channel — the delegate method sends on this.
static REOPEN_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// Receiver half, taken once by the subscription system.
static REOPEN_RX: Mutex<Option<mpsc::Receiver<()>>> = Mutex::new(None);

/// C function injected into the NSApplicationDelegate at runtime.
///
/// Signature matches `applicationShouldHandleReopen:hasVisibleWindows:`:
///   `- (BOOL)applicationShouldHandleReopen:(NSApplication *)sender hasVisibleWindows:(BOOL)flag`
extern "C" fn handle_reopen(
    _this: &AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: &AnyObject,
    has_visible_windows: Bool,
) -> Bool {
    if !has_visible_windows.as_bool() {
        if let Some(tx) = REOPEN_TX.get() {
            let _ = tx.send(());
        }
    }
    Bool::YES
}

/// Add `applicationShouldHandleReopen:hasVisibleWindows:` to winit's
/// `WinitApplicationDelegate` class.
///
/// Winit registers its own NSApplicationDelegate but doesn't implement this
/// method, so we inject it via the ObjC runtime. When the user clicks the
/// dock icon with no visible windows, our implementation fires and sends a
/// message through the reopen channel.
///
/// Idempotent — safe to call multiple times, only the first call registers.
pub fn install_reopen_handler() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let (tx, rx) = mpsc::channel();
        REOPEN_TX.set(tx).ok();
        *REOPEN_RX.lock().unwrap() = Some(rx);

        unsafe {
            let cls = AnyClass::get("WinitApplicationDelegate")
                .expect("WinitApplicationDelegate class not found");

            // Cast to raw pointer for class_addMethod (needs *mut).
            let cls_ptr = cls as *const _ as *mut objc2::ffi::objc_class;
            let sel = sel!(applicationShouldHandleReopen:hasVisibleWindows:);
            let imp: objc2::ffi::IMP = Some(std::mem::transmute::<
                extern "C" fn(&AnyObject, objc2::runtime::Sel, &AnyObject, Bool) -> Bool,
                unsafe extern "C" fn(),
            >(handle_reopen));

            // Type encoding: returns BOOL, params (id self, SEL _cmd, id app, BOOL flag)
            let types = c"B@:@B";
            objc2::ffi::class_addMethod(cls_ptr, sel.as_ptr(), imp, types.as_ptr());
        }
    });
}

/// Take the reopen receiver (call exactly once from the subscription setup).
pub fn take_reopen_receiver() -> Option<mpsc::Receiver<()>> {
    REOPEN_RX.lock().unwrap().take()
}

// =============================================================================
// Native Menu Bar
// =============================================================================

/// C function injected into WinitApplicationDelegate for Cmd+N (newDocument:).
///
/// Reuses the same reopen channel — the subscription treats it as
/// "platform requests a new window".
extern "C" fn handle_new_document(
    _this: &AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut AnyObject,
) {
    if let Some(tx) = REOPEN_TX.get() {
        let _ = tx.send(());
    }
}

/// Set up the macOS menu bar with File and Window menus.
///
/// Winit already creates an app menu (About, Hide, Quit). This adds:
/// - **File**: New Window (Cmd+N), Close Window (Cmd+W)
/// - **Window**: Minimize (Cmd+M), Bring All to Front
///
/// Also injects `newDocument:` into WinitApplicationDelegate so Cmd+N
/// works even with no windows open.
pub fn setup_menu_bar() {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);

    let Some(menubar) = (unsafe { app.mainMenu() }) else { return };

    // --- Inject newDocument: into WinitApplicationDelegate ---
    unsafe {
        let cls = AnyClass::get("WinitApplicationDelegate")
            .expect("WinitApplicationDelegate class not found");
        let cls_ptr = cls as *const _ as *mut objc2::ffi::objc_class;
        let imp: objc2::ffi::IMP = Some(std::mem::transmute::<
            extern "C" fn(&AnyObject, objc2::runtime::Sel, *mut AnyObject),
            unsafe extern "C" fn(),
        >(handle_new_document));
        let types = c"v@:@";
        objc2::ffi::class_addMethod(
            cls_ptr,
            sel!(newDocument:).as_ptr(),
            imp,
            types.as_ptr(),
        );
    }

    // --- File menu ---
    let file_menu = NSMenu::new(mtm);
    unsafe { file_menu.setTitle(ns_string!("File")) };

    let new_window = make_menu_item(
        mtm,
        ns_string!("New Window"),
        sel!(newDocument:),
        ns_string!("n"),
    );
    file_menu.addItem(&new_window);

    let close_window = make_menu_item(
        mtm,
        ns_string!("Close Window"),
        sel!(performClose:),
        ns_string!("w"),
    );
    file_menu.addItem(&close_window);

    let file_item = NSMenuItem::new(mtm);
    file_item.setSubmenu(Some(&file_menu));
    unsafe { menubar.insertItem_atIndex(&file_item, 1) }; // after app menu

    // --- Window menu ---
    let window_menu = NSMenu::new(mtm);
    unsafe { window_menu.setTitle(ns_string!("Window")) };

    let minimize = make_menu_item(
        mtm,
        ns_string!("Minimize"),
        sel!(performMiniaturize:),
        ns_string!("m"),
    );
    window_menu.addItem(&minimize);

    let bring_all = make_menu_item(
        mtm,
        ns_string!("Bring All to Front"),
        sel!(arrangeInFront:),
        ns_string!(""),
    );
    window_menu.addItem(&bring_all);

    let window_item = NSMenuItem::new(mtm);
    window_item.setSubmenu(Some(&window_menu));
    unsafe { menubar.insertItem_atIndex(&window_item, 2) }; // after File menu

    // Tell AppKit this is the Window menu (enables automatic window list).
    unsafe { app.setWindowsMenu(Some(&window_menu)) };
}

// =============================================================================
// Force Click (Dictionary Lookup)
// =============================================================================

/// Show the macOS dictionary/definition popup for the given text.
///
/// Uses `NSView.showDefinitionForAttributedString:atPoint:` to display the
/// system dictionary popup at the given position. The `position` is in
/// window-local, top-left-origin coordinates (Strata's coordinate system).
/// `font_size` should match the rendered text size (base_size * zoom).
pub fn show_definition(text: &str, position: crate::primitives::Point, font_size: f32) -> Result<(), String> {
    unsafe {
        let mtm = MainThreadMarker::new_unchecked();
        let app = NSApplication::sharedApplication(mtm);
        let window = app.keyWindow().or_else(|| app.mainWindow()).ok_or("No window")?;
        let view = window.contentView().ok_or("No content view")?;

        // position is in Strata coords (top-left origin).
        // showDefinitionForAttributedString:atPoint: expects view-local coords.
        // If the view isFlipped (top-left origin, common for framework views),
        // no Y conversion needed. Otherwise flip from top-left to bottom-left.
        let flipped: bool = msg_send![&*view, isFlipped];
        let bounds: NSRect = msg_send![&*view, bounds];
        let view_point = if flipped {
            NSPoint::new(position.x as f64, position.y as f64)
        } else {
            NSPoint::new(position.x as f64, bounds.size.height - position.y as f64)
        };

        // Create NSAttributedString with matching monospace font
        let ns_text = NSString::from_str(text);
        let font: Retained<AnyObject> = msg_send_id![
            AnyClass::get("NSFont").unwrap(),
            monospacedSystemFontOfSize: font_size as f64,
            weight: 0.0_f64  // NSFontWeightRegular
        ];
        let font_key = NSString::from_str("NSFont");
        let attrs: Retained<AnyObject> = msg_send_id![
            AnyClass::get("NSDictionary").unwrap(),
            dictionaryWithObject: &*font,
            forKey: &*font_key
        ];
        let attr_string: Retained<AnyObject> = msg_send_id![
            msg_send_id![AnyClass::get("NSAttributedString").unwrap(), alloc],
            initWithString: &*ns_text,
            attributes: &*attrs
        ];

        // showDefinitionForAttributedString:atPoint: on NSView
        let _: () = msg_send![&*view, showDefinitionForAttributedString: &*attr_string atPoint: view_point];
    }

    Ok(())
}

// =============================================================================
// Force Click Event Monitor
// =============================================================================

/// Sender for force click events: (x, y) in screen coordinates.
static FORCE_CLICK_TX: OnceLock<mpsc::Sender<(f32, f32)>> = OnceLock::new();

/// Receiver half, taken once by the subscription system.
static FORCE_CLICK_RX: Mutex<Option<mpsc::Receiver<(f32, f32)>>> = Mutex::new(None);

/// Thread-local queue for force click events (native backend, same-thread).
use std::cell::RefCell;
thread_local! {
    static FORCE_CLICK_QUEUE: RefCell<Vec<(f32, f32)>> = const { RefCell::new(Vec::new()) };
}

/// Drain pending force click events from the thread-local queue.
pub fn drain_force_click_events() -> Vec<(f32, f32)> {
    FORCE_CLICK_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        std::mem::take(&mut *q)
    })
}

/// Install a local NSEvent monitor for pressure (Force Touch) events.
///
/// When the trackpad transitions to stage 2 (deep click), sends the
/// cursor position through the channel. Idempotent — safe to call
/// multiple times, only the first call registers.
pub fn install_force_click_handler() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let (tx, rx) = mpsc::channel();
        FORCE_CLICK_TX.set(tx).ok();
        *FORCE_CLICK_RX.lock().unwrap() = Some(rx);

        // The rest (NSEvent monitor) is set up in SetupNative phase
        // where we have access to the main thread and event loop.
    });
}

/// Actually register the NSEvent pressure monitor (must be called on main thread
/// after the event loop has started).
pub fn setup_force_click_monitor() {
    use block2::RcBlock;

    // Track stage transitions to avoid repeated firing.
    static WAS_DEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    unsafe {
        let ns_event_class = AnyClass::get("NSEvent")
            .expect("NSEvent class not found");

        // NSEventMaskPressure = 1 << 34
        let mask: u64 = 1 << 34;

        // The handler block receives an NSEvent and returns it (or nil to discard).
        // We use RcBlock to create a closure-based ObjC block.
        let block = RcBlock::new(|event: *mut AnyObject| -> *mut AnyObject {
            if event.is_null() {
                return event;
            }
            // Read event.stage (NSInteger)
            let stage: NSInteger = msg_send![event, stage];
            let is_deep = stage >= 2;
            let was = WAS_DEEP.swap(is_deep, std::sync::atomic::Ordering::Relaxed);
            if is_deep && !was {
                // Transition to deep click — get cursor position
                if let Some(tx) = FORCE_CLICK_TX.get() {
                    let mtm = MainThreadMarker::new_unchecked();
                    let app = NSApplication::sharedApplication(mtm);
                    if let Some(window) = app.mainWindow().or(app.keyWindow()) {
                        if let Some(view) = window.contentView() {
                            // locationInWindow is in window coords (bottom-left origin).
                            // Convert to view-local via convertPoint:fromView:nil to
                            // account for title bar / window chrome offset.
                            let loc_window: NSPoint = msg_send![event, locationInWindow];
                            let loc_view: NSPoint = msg_send![
                                &*view, convertPoint: loc_window
                                fromView: std::ptr::null::<AnyObject>()
                            ];
                            // Convert to Strata top-left origin.
                            // If the view isFlipped, convertPoint already gives top-left;
                            // otherwise we need to flip Y manually.
                            let flipped: bool = msg_send![&*view, isFlipped];
                            let x = loc_view.x as f32;
                            let y = if flipped {
                                loc_view.y as f32
                            } else {
                                let view_height = view.frame().size.height;
                                (view_height - loc_view.y) as f32
                            };
                            let _ = tx.send((x, y));
                            FORCE_CLICK_QUEUE.with(|q| q.borrow_mut().push((x, y)));
                        }
                    }
                }
            }
            event
        });

        // [NSEvent addLocalMonitorForEventsMatchingMask:mask handler:block]
        let _monitor: *mut AnyObject = msg_send![
            ns_event_class,
            addLocalMonitorForEventsMatchingMask: mask,
            handler: &*block
        ];
        // Monitor is retained by AppKit; block is leaked (lives for app lifetime).
        std::mem::forget(block);
    }
}

/// Take the force click receiver (call exactly once from the subscription setup).
pub fn take_force_click_receiver() -> Option<mpsc::Receiver<(f32, f32)>> {
    FORCE_CLICK_RX.lock().unwrap().take()
}

// =============================================================================
// Window Resize Appearance
// =============================================================================

/// Configure window appearance for flicker-free resize.
///
/// Sets:
/// 1. **NSWindow.backgroundColor** — fills any gap with the app's dark color.
/// 2. **NSView.layerContentsRedrawPolicy = OnSetNeedsDisplay** — prevents
///    macOS from forcing system redraws during resize.
/// 3. **Configure root + Metal sublayer** — gravity, background, no animations.
///
/// Safe to call multiple times (idempotent).
pub fn configure_resize_appearance(r: f32, g: f32, b: f32) {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);

    let bg_color = unsafe {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            r as CGFloat,
            g as CGFloat,
            b as CGFloat,
            1.0,
        )
    };

    let cg_color = create_cg_color(r as f64, g as f64, b as f64);

    let windows = app.windows();
    for window in windows.iter() {
        window.setBackgroundColor(Some(&bg_color));
        if let Some(view) = window.contentView() {
            unsafe {
                // Prevent macOS from forcing redraws during resize.
                view.setLayerContentsRedrawPolicy(
                    NSViewLayerContentsRedrawPolicy::NSViewLayerContentsRedrawOnSetNeedsDisplay,
                );
            }

            unsafe {
                let root: *mut AnyObject = msg_send![&*view, layer];
                if root.is_null() {
                    continue;
                }

                // Configure root layer.
                configure_layer_for_resize(root, cg_color);

                // Also configure any sublayers (Metal layer + overlay).
                configure_metal_sublayers(root, cg_color);
            }
        }
    }
}

/// Walk a layer's sublayers and configure any CAMetalLayer found.
unsafe fn configure_metal_sublayers(root: *mut AnyObject, bg_cg_color: CGColorPtr) {
    let sublayers: *mut AnyObject = msg_send![root, sublayers];
    if sublayers.is_null() {
        return;
    }
    let count: usize = msg_send![sublayers, count];
    let Some(metal_cls) = AnyClass::get("CAMetalLayer") else {
        return;
    };
    for i in 0..count {
        let layer: *mut AnyObject = msg_send![sublayers, objectAtIndex: i];
        if layer.is_null() {
            continue;
        }
        let is_metal: Bool = msg_send![layer, isKindOfClass: metal_cls];
        if is_metal.as_bool() {
            unsafe { configure_layer_for_resize(layer, bg_cg_color) };
        }
    }
}

/// Opaque CGColorRef wrapper with correct objc2 type encoding (`^{CGColor=}`).
///
/// objc2's `msg_send!` validates argument/return encodings at runtime.
/// Raw `*const c_void` encodes as `^v` which mismatches CoreGraphics'
/// `^{CGColor=}`, causing a panic. This newtype carries the right encoding.
#[repr(transparent)]
#[derive(Copy, Clone)]
pub(crate) struct CGColorPtr(*const std::ffi::c_void);

unsafe impl objc2::encode::Encode for CGColorPtr {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &objc2::encode::Encoding::Struct("CGColor", &[]),
    );
}

unsafe impl objc2::encode::RefEncode for CGColorPtr {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &<Self as objc2::encode::Encode>::ENCODING,
    );
}

/// Create a CGColorRef directly via CoreGraphics C API.
pub(crate) fn create_cg_color(r: f64, g: f64, b: f64) -> CGColorPtr {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> *mut std::ffi::c_void;
        fn CGColorCreate(
            space: *const std::ffi::c_void,
            components: *const f64,
        ) -> *const std::ffi::c_void;
        fn CGColorSpaceRelease(space: *mut std::ffi::c_void);
    }

    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        let components: [f64; 4] = [r, g, b, 1.0];
        let color = CGColorCreate(space, components.as_ptr());
        CGColorSpaceRelease(space);
        CGColorPtr(color)
    }
}

/// Configure a single CALayer for flicker-free resize: set gravity,
/// background color, and disable implicit CA animations.
///
/// Uses `kCAGravityResize` (stretch) so that on rare missed frames,
/// stale content fills the entire layer without position desync.
unsafe fn configure_layer_for_resize(layer: *mut AnyObject, bg_cg_color: CGColorPtr) {
    let gravity = ns_string!("resize");
    let _: () = msg_send![layer, setContentsGravity: &*gravity];
    let _: () = msg_send![layer, setBackgroundColor: bg_cg_color];

    // Disable implicit CA animations on bounds/position/contents changes.
    // Without this, CA animates frame changes during live resize.
    let null_cls = AnyClass::get("NSNull").unwrap();
    let null_obj: *mut AnyObject = msg_send![null_cls, null];
    let dict_cls = AnyClass::get("NSMutableDictionary").unwrap();
    let actions: *mut AnyObject = msg_send![dict_cls, new];
    for key in [
        ns_string!("bounds"),
        ns_string!("position"),
        ns_string!("contents"),
        ns_string!("contentsScale"),
    ] {
        let _: () = msg_send![actions, setObject: null_obj forKey: &*key];
    }
    let _: () = msg_send![layer, setActions: actions];
}


// =============================================================================
// System Cursor
// =============================================================================

/// Set the system cursor to match a CursorIcon.
pub fn set_cursor(icon: crate::layout_snapshot::CursorIcon) {
    use crate::layout_snapshot::CursorIcon;
    let cls = AnyClass::get("NSCursor").unwrap();
    unsafe {
        let cursor: *mut AnyObject = match icon {
            CursorIcon::Arrow => msg_send![cls, arrowCursor],
            CursorIcon::Text => msg_send![cls, IBeamCursor],
            CursorIcon::Pointer => msg_send![cls, pointingHandCursor],
            CursorIcon::Grab => msg_send![cls, openHandCursor],
            CursorIcon::Grabbing => msg_send![cls, closedHandCursor],
            CursorIcon::Copy => msg_send![cls, dragCopyCursor],
        };
        let _: () = msg_send![cursor, set];
    }
}

// =============================================================================
// Native Context Menu
// =============================================================================

/// Menu item descriptor for `show_context_menu`.
pub struct NativeMenuItem {
    pub label: String,
    pub shortcut: String,
    /// If true, renders as a separator instead of a clickable item.
    pub separator: bool,
}

/// Global storage for the selected menu item index.
/// Safe because context menus are synchronous and only run on the main thread.
static CONTEXT_MENU_SELECTION: Mutex<Option<usize>> = Mutex::new(None);

declare_class!(
    struct ContextMenuTarget;

    unsafe impl ClassType for ContextMenuTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "NexusContextMenuTarget";
    }

    impl DeclaredClass for ContextMenuTarget {
        type Ivars = ();
    }

    unsafe impl ContextMenuTarget {
        #[method(menuItemClicked:)]
        fn menu_item_clicked(&self, sender: &NSMenuItem) {
            let tag = unsafe { sender.tag() };
            if let Ok(mut sel) = CONTEXT_MENU_SELECTION.lock() {
                *sel = Some(tag as usize);
            }
        }
    }
);

/// Show a native macOS context menu at the given position (top-left origin).
///
/// Blocks the run loop until the user selects an item or dismisses the menu.
/// Returns the index of the selected item, or `None` if dismissed.
///
/// Coordinates are in the app's logical coordinate system (top-left origin,
/// matching the layout system). Internally converted to NSView's bottom-left
/// origin for `popUpMenuPositioningItem:atLocation:inView:`.
pub fn show_context_menu(items: &[NativeMenuItem], x: f32, y: f32) -> Option<usize> {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let window = unsafe { app.keyWindow().or_else(|| app.mainWindow())? };
    let content_view = window.contentView()?;

    // Reset selection state
    if let Ok(mut sel) = CONTEXT_MENU_SELECTION.lock() {
        *sel = None;
    }

    let target: Retained<ContextMenuTarget> = unsafe { msg_send_id![mtm.alloc::<ContextMenuTarget>(), init] };

    let menu = NSMenu::new(mtm);
    unsafe { menu.setAutoenablesItems(false) };

    for (i, item) in items.iter().enumerate() {
        if item.separator {
            let sep: Retained<NSMenuItem> = unsafe { msg_send_id![NSMenuItem::class(), separatorItem] };
            menu.addItem(&sep);
            continue;
        }

        let title = NSString::from_str(&item.label);
        let key = NSString::from_str(&item.shortcut);
        let menu_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &title,
                Some(sel!(menuItemClicked:)),
                &key,
            )
        };
        unsafe { menu_item.setTarget(Some(&target)) };
        unsafe { menu_item.setTag(i as isize) };
        menu.addItem(&menu_item);
    }

    // Our coordinates are already in the view's coordinate system (top-left origin,
    // since isFlipped returns YES). No Y-flip needed.
    let location = NSPoint::new(x as f64, y as f64);

    // popUpMenuPositioningItem:atLocation:inView: blocks until dismissed
    let _: bool = unsafe {
        msg_send![&*menu, popUpMenuPositioningItem: std::ptr::null::<AnyObject>(), atLocation: location, inView: &*content_view]
    };

    CONTEXT_MENU_SELECTION.lock().ok().and_then(|sel| *sel)
}

// =============================================================================
// Native Menu Bar (make_menu_item helper)
// =============================================================================

/// Create a menu item with a Cmd+key shortcut.
fn make_menu_item(
    mtm: MainThreadMarker,
    title: &NSString,
    action: objc2::runtime::Sel,
    key: &NSString,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            title,
            Some(action),
            key,
        )
    }
}
