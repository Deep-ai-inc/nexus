//! macOS native platform integration.
//!
//! Provides:
//! - Native drag source using NSPasteboard + NSDraggingSession
//! - Quick Look preview using QLPreviewPanel with proper delegate
//!
//! Uses `[NSApp currentEvent]` and `[[NSApp mainWindow] contentView]` to
//! initiate OS-level drags without needing Iced to expose its NSView/NSEvent.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSDraggingItem, NSDraggingSession, NSImage,
    NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString,
    NSPasteboardWriting, NSResponder, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSInteger, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL,
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

fn show_quicklook_native(path: &Path, source_rect: Option<NSRect>) -> Result<(), String> {
    // Update global state
    {
        let mut state = QUICKLOOK_STATE.lock().unwrap();
        state.path = Some(path.to_path_buf());
        state.source_rect = source_rect;
    }

    unsafe {
        // Get QLPreviewPanel class
        let ql_class: &AnyClass = class!(QLPreviewPanel);

        // Get shared panel: [QLPreviewPanel sharedPreviewPanel]
        let panel: *mut AnyObject = msg_send![ql_class, sharedPreviewPanel];
        if panel.is_null() {
            return Err("Failed to get QLPreviewPanel".into());
        }

        // Check if panel is visible
        let is_visible: Bool = msg_send![panel, isVisible];

        if is_visible.as_bool() {
            // If already showing, toggle off
            let _: () = msg_send![panel, orderOut: std::ptr::null::<AnyObject>()];
        } else {
            // Create a fresh data source (lightweight, just provides callbacks to global state)
            let data_source: Retained<NexusPreviewDataSource> =
                msg_send_id![NexusPreviewDataSource::class(), new];

            // Set data source and delegate
            let _: () = msg_send![panel, setDataSource: &*data_source];
            let _: () = msg_send![panel, setDelegate: &*data_source];

            // Refresh and show
            let _: () = msg_send![panel, reloadData];
            let _: () = msg_send![panel, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];

            // Note: data_source will be released when this function returns,
            // but QLPreviewPanel retains its data source/delegate internally
        }

        Ok(())
    }
}

/// Close the Quick Look panel if it's open.
pub fn close_quicklook() {
    unsafe {
        let ql_class: &AnyClass = class!(QLPreviewPanel);
        let exists: Bool = msg_send![ql_class, sharedPreviewPanelExists];
        if exists.as_bool() {
            let panel: *mut AnyObject = msg_send![ql_class, sharedPreviewPanel];
            if !panel.is_null() {
                let _: () = msg_send![panel, orderOut: std::ptr::null::<AnyObject>()];
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
/// Must be called on the main thread during event processing (i.e., from within
/// an Iced update cycle) so that `[NSApp currentEvent]` returns the triggering
/// mouse event.
pub fn start_drag(source: &DragSource) -> Result<(), String> {
    // Safety: we are on the main thread (called from iced update loop).
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
