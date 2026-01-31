//! Scroll State
//!
//! Encapsulates all scroll-related state and operations for scroll containers.
//! Eliminates duplicated scroll logic when apps have multiple scroll panels.

use std::cell::Cell;

use crate::app::MouseResponse;
use crate::content_address::SourceId;
use crate::event_context::{CaptureState, MouseButton, MouseEvent, ScrollDelta};
use crate::layout_snapshot::{HitResult, LayoutSnapshot, ScrollTrackInfo};
use crate::primitives::{Point, Rect};

/// Grab tolerance for scrollbar thumb clicks (absorbs float rounding).
const GRAB_TOLERANCE: f32 = 4.0;

/// An action on a scroll container, produced by event handling.
#[derive(Debug, Clone)]
pub enum ScrollAction {
    /// Scroll by a delta (positive = scroll content up / towards start).
    ScrollBy(f32),
    /// Start dragging the scrollbar thumb at this mouse Y.
    DragStart(f32),
    /// Continue dragging the scrollbar thumb to this mouse Y.
    DragMove(f32),
    /// End the thumb drag.
    DragEnd,
}

/// Encapsulates all scroll state for a single scroll container.
///
/// Use this in your app state instead of managing separate offset, max,
/// track, grab_offset, and bounds fields.
///
/// # Example
/// ```ignore
/// struct MyState {
///     left_scroll: ScrollState,
///     right_scroll: ScrollState,
/// }
/// ```
pub struct ScrollState {
    /// Current scroll offset (0 = top).
    pub offset: f32,
    /// Maximum scroll offset (set from layout snapshot each frame).
    pub max: Cell<f32>,
    /// Scroll track geometry (set from layout snapshot each frame).
    pub track: Cell<Option<ScrollTrackInfo>>,
    /// Distance from mouse click to top of thumb during drag.
    grab_offset: f32,
    /// Scroll container bounds (set from layout snapshot each frame).
    pub bounds: Cell<Rect>,
    /// The SourceId for the ScrollColumn container.
    id: SourceId,
    /// The SourceId for the scrollbar thumb widget.
    thumb_id: SourceId,
}

impl ScrollState {
    /// Create a new scroll state with auto-generated SourceIds.
    pub fn new() -> Self {
        Self {
            offset: 0.0,
            max: Cell::new(f32::MAX),
            track: Cell::new(None),
            grab_offset: 0.0,
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            id: SourceId::new(),
            thumb_id: SourceId::new(),
        }
    }

    /// Create a scroll state with explicit SourceIds.
    pub fn with_ids(id: SourceId, thumb_id: SourceId) -> Self {
        Self {
            offset: 0.0,
            max: Cell::new(f32::MAX),
            track: Cell::new(None),
            grab_offset: 0.0,
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            id,
            thumb_id,
        }
    }

    /// Get the ScrollColumn SourceId.
    pub fn id(&self) -> SourceId {
        self.id
    }

    /// Get the scrollbar thumb SourceId.
    pub fn thumb_id(&self) -> SourceId {
        self.thumb_id
    }

    // =====================================================================
    // Scroll operations
    // =====================================================================

    /// Apply a scroll action (call from update()).
    pub fn apply(&mut self, action: ScrollAction) {
        match action {
            ScrollAction::ScrollBy(delta) => self.scroll_by(delta),
            ScrollAction::DragStart(mouse_y) => self.start_drag(mouse_y),
            ScrollAction::DragMove(mouse_y) => self.drag_to(mouse_y),
            ScrollAction::DragEnd => self.end_drag(),
        }
    }

    /// Scroll by a delta (positive = scroll content up).
    pub fn scroll_by(&mut self, delta: f32) {
        let max = self.max.get();
        self.offset = (self.offset - delta).clamp(0.0, max);
    }

    /// Start a thumb drag at the given mouse Y position.
    pub fn start_drag(&mut self, mouse_y: f32) {
        if let Some(track) = self.track.get() {
            let effective_offset = self.offset.clamp(0.0, self.max.get());
            let thumb_top = track.thumb_y(effective_offset);
            let thumb_bottom = thumb_top + track.thumb_height;

            if mouse_y >= (thumb_top - GRAB_TOLERANCE)
                && mouse_y <= (thumb_bottom + GRAB_TOLERANCE)
            {
                // Clicked on the thumb: preserve grab offset so it doesn't jump.
                self.grab_offset = mouse_y - thumb_top;
            } else {
                // Clicked on the track: jump thumb center to click point.
                self.grab_offset = track.thumb_height / 2.0;
                let new_offset = track.offset_from_y(mouse_y, self.grab_offset);
                self.offset = new_offset.clamp(0.0, self.max.get());
            }
        }
    }

    /// Continue a thumb drag to the given mouse Y position.
    pub fn drag_to(&mut self, mouse_y: f32) {
        if let Some(track) = self.track.get() {
            let new_offset = track.offset_from_y(mouse_y, self.grab_offset);
            self.offset = new_offset.clamp(0.0, self.max.get());
        }
    }

    /// End the thumb drag.
    pub fn end_drag(&mut self) {
        self.grab_offset = 0.0;
    }

    // =====================================================================
    // Composable mouse handler
    // =====================================================================

    /// Handle a mouse event for this scroll container.
    ///
    /// Returns `Some(MouseResponse<ScrollAction>)` if this scroll container
    /// consumed the event, `None` otherwise. Use with `MouseResponse::map()`
    /// to convert to your app's message type:
    ///
    /// ```ignore
    /// if let Some(r) = state.left_scroll.handle_mouse(&event, &hit, capture) {
    ///     return r.map(AppMessage::LeftScroll);
    /// }
    /// ```
    ///
    /// Handles: thumb press/drag/release and wheel scrolling.
    pub fn handle_mouse(
        &self,
        event: &MouseEvent,
        hit: &Option<HitResult>,
        capture: &CaptureState,
    ) -> Option<MouseResponse<ScrollAction>> {
        match event {
            MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                position,
            } => {
                if let Some(HitResult::Widget(id)) = hit {
                    if *id == self.thumb_id {
                        return Some(MouseResponse::message_and_capture(
                            ScrollAction::DragStart(position.y),
                            self.thumb_id,
                        ));
                    }
                }
                None
            }
            MouseEvent::CursorMoved { position } => {
                if let CaptureState::Captured(id) = capture {
                    if *id == self.thumb_id {
                        return Some(MouseResponse::message(
                            ScrollAction::DragMove(position.y),
                        ));
                    }
                }
                None
            }
            MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            } => {
                if let CaptureState::Captured(id) = capture {
                    if *id == self.thumb_id {
                        return Some(MouseResponse::message_and_release(
                            ScrollAction::DragEnd,
                        ));
                    }
                }
                None
            }
            MouseEvent::WheelScrolled { delta, position } => {
                if self.contains(*position) {
                    let dy = match delta {
                        ScrollDelta::Lines { y, .. } => y * 40.0,
                        ScrollDelta::Pixels { y, .. } => *y,
                    };
                    return Some(MouseResponse::message(ScrollAction::ScrollBy(dy)));
                }
                None
            }
            _ => None,
        }
    }

    // =====================================================================
    // Layout sync
    // =====================================================================

    /// Sync scroll state from the layout snapshot after layout.
    ///
    /// Call this in `view()` after calling `.layout()`. Uses `Cell` for
    /// interior mutability since `view()` takes `&Self::State`.
    pub fn sync_from_snapshot(&self, snapshot: &LayoutSnapshot) {
        if let Some(max) = snapshot.scroll_limit(&self.id) {
            self.max.set(max);
        }
        if let Some(track) = snapshot.scroll_track(&self.id) {
            self.track.set(Some(*track));
        }
        if let Some(bounds) = snapshot.widget_bounds(&self.id) {
            self.bounds.set(bounds);
        }
    }

    /// Check if a point is within this scroll container's bounds.
    pub fn contains(&self, point: Point) -> bool {
        self.bounds.get().contains_xy(point.x, point.y)
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_by_clamps() {
        let mut state = ScrollState::new();
        state.max.set(100.0);

        state.scroll_by(-50.0); // scroll down
        assert_eq!(state.offset, 50.0);

        state.scroll_by(-200.0); // over-scroll
        assert_eq!(state.offset, 100.0);

        state.scroll_by(300.0); // scroll up past 0
        assert_eq!(state.offset, 0.0);
    }

    #[test]
    fn end_drag_resets_grab() {
        let mut state = ScrollState::new();
        state.grab_offset = 42.0;
        state.end_drag();
        assert_eq!(state.grab_offset, 0.0);
    }
}
