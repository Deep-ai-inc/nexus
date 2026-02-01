//! macOS native drag source using NSPasteboard + NSDraggingSession.
//!
//! Uses `[NSApp currentEvent]` and `[[NSApp mainWindow] contentView]` to
//! initiate OS-level drags without needing Iced to expose its NSView/NSEvent.
//!
//! The key insight: winit's WinitView (which is the NSView backing Iced's window)
//! already conforms to `NSDraggingSource`. We use `msg_send_id!` to call
//! `beginDraggingSessionWithItems:event:source:` on it, bypassing Rust type
//! checking for the protocol conformance (which is handled at the ObjC level).

use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send_id, ClassType};
use objc2_app_kit::{
    NSApplication, NSDraggingItem, NSDraggingSession, NSImage,
    NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString,
    NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, NSString, NSURL,
};

use crate::app::DragSource;

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
fn write_drag_temp_file(filename: &str, data: &[u8]) -> Result<PathBuf, std::io::Error> {
    let temp_dir = std::env::temp_dir().join("nexus-drag");
    std::fs::create_dir_all(&temp_dir)?;
    let path = temp_dir.join(filename);
    std::fs::write(&path, data)?;
    Ok(path)
}
