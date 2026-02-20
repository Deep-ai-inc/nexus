//! Scroll State
//!
//! Encapsulates all scroll-related state and operations for scroll containers.
//! Eliminates duplicated scroll logic when apps have multiple scroll panels.

use std::cell::Cell;
use std::time::Instant;

use crate::app::MouseResponse;
use crate::content_address::SourceId;
use crate::event_context::{CaptureState, MouseButton, MouseEvent, ScrollDelta, ScrollPhase};
use crate::layout_snapshot::{HitResult, LayoutSnapshot, ScrollTrackInfo};
use crate::primitives::{Point, Rect};

/// Grab tolerance for scrollbar thumb clicks (absorbs float rounding).
const GRAB_TOLERANCE: f32 = 4.0;


/// An action on a scroll container, produced by event handling.
#[derive(Debug, Clone)]
pub enum ScrollAction {
    /// Scroll by a delta (positive = scroll content up / towards start).
    /// `phase` carries trackpad gesture phase for overscroll/momentum.
    ScrollBy { delta: f32, phase: Option<ScrollPhase> },
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

    // --- Overscroll / rubber-band state ---
    /// Displacement beyond scroll bounds (neg = past top, pos = past bottom).
    pub overscroll: f32,
    /// True during Contact or Momentum phases.
    gesture_active: bool,
    /// True while the spring-back animation is running.
    pub animating: bool,
    /// Start time of the current spring animation.
    spring_start: Instant,
    /// Initial overscroll when the spring started (C1 in analytical solution).
    spring_x0: f32,
    /// Initial velocity when the spring started (used to compute C2).
    spring_v0: f32,
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
            overscroll: 0.0,
            gesture_active: false,
            animating: false,
            spring_start: Instant::now(),
            spring_x0: 0.0,
            spring_v0: 0.0,
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
            overscroll: 0.0,
            gesture_active: false,
            animating: false,
            spring_start: Instant::now(),
            spring_x0: 0.0,
            spring_v0: 0.0,
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
            ScrollAction::ScrollBy { delta, phase } => {
                if phase.is_some() {
                    self.scroll_with_phase(delta, phase);
                } else {
                    self.scroll_by(delta);
                }
            }
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
    // Overscroll / rubber-band
    // =====================================================================

    /// Effective scroll offset including overscroll displacement.
    /// Use this for layout positioning instead of `offset` directly.
    pub fn effective_offset(&self) -> f32 {
        self.offset + self.overscroll
    }

    /// Reset overscroll state (call when snapping to bottom, clearing, etc.)
    pub fn reset_overscroll(&mut self) {
        self.overscroll = 0.0;
        self.gesture_active = false;
        self.animating = false;
        self.spring_x0 = 0.0;
        self.spring_v0 = 0.0;
    }

    /// Phase-aware scroll: allows overscroll with rubber-band resistance
    /// at boundaries during trackpad/momentum gestures.
    ///
    /// Contact (finger): incremental rubber-band with 1:1 pull-back.
    /// Momentum: on boundary impact, starts a spring-bounce immediately
    /// with the impact velocity. All further momentum deltas are ignored —
    /// the spring handles the animation. No more "pinning" at max overscroll.
    fn scroll_with_phase(&mut self, delta: f32, phase: Option<ScrollPhase>) {
        let max = self.max.get();

        // Re-clamp offset to current bounds. Content may have resized
        // since the last scroll event, leaving offset beyond the new max.
        // Without this, stale offset creates spurious overscroll on the
        // next delta (momentum or contact), causing a visible jump.
        self.offset = self.offset.clamp(0.0, max);

        match phase {
            Some(ScrollPhase::Contact) => {
                self.gesture_active = true;
                self.animating = false; // Kill momentum/bounce on touch

                // Snap small overscroll to zero on finger touch — prevents
                // entering overscroll interaction mode for residual amounts
                // left by momentum grazing the boundary or spring settling.
                if self.overscroll.abs() < 15.0 && self.overscroll != 0.0 {
                    self.overscroll = 0.0;
                }

                if self.overscroll != 0.0 {
                    // Finger is down while overscrolled (caught mid-bounce or
                    // dragging past edge). Give 1:1 control when pulling back
                    // toward bounds, resistance only when stretching further.
                    let pulling_back = (self.overscroll > 0.0 && delta > 0.0)
                        || (self.overscroll < 0.0 && delta < 0.0);

                    if pulling_back {
                        self.overscroll -= delta;
                        // If we crossed zero, put the remainder into normal scroll
                        if (self.overscroll > 0.0) != (delta < 0.0) {
                            // sign didn't flip — still overscrolled, done
                        } else {
                            // Crossed zero: leftover goes into normal offset
                            let leftover = self.overscroll;
                            self.overscroll = 0.0;
                            self.offset = (self.offset + leftover).clamp(0.0, max);
                        }
                    } else {
                        // Stretching further: factor=1.0 (no boundary discontinuity)
                        self.overscroll -= apply_resistance(delta, self.overscroll, 1.0);
                    }
                } else {
                    self.apply_contact_scroll(delta, max);
                }
            }
            Some(ScrollPhase::Momentum) => {
                // Must set gesture_active BEFORE the animating check — otherwise
                // momentum events that return early never set it, and the spring
                // settle code sees gesture_active=false and clears animating,
                // letting the next momentum event trigger a second bounce.
                self.gesture_active = true;
                if self.animating {
                    // Bounce spring is already running — momentum has been
                    // absorbed. Advance the spring to the current time so this
                    // render shows the correct position (analytical evaluation
                    // is idempotent — calling it at any frequency is fine).
                    self.tick_spring_back();
                    return;
                }

                // Normal in-bounds scrolling until we hit a boundary
                let new_offset = self.offset - delta;
                if new_offset < 0.0 {
                    // Hit top boundary — start spring bounce
                    self.offset = 0.0;
                    self.overscroll = new_offset; // negative (past top)
                    self.start_spring(self.overscroll, (-delta * 20.0).clamp(-3000.0, 3000.0));
                } else if new_offset > max {
                    // Hit bottom boundary — start spring bounce
                    self.offset = max;
                    self.overscroll = new_offset - max;
                    self.start_spring(self.overscroll, (-delta * 20.0).clamp(-3000.0, 3000.0));
                } else {
                    self.offset = new_offset;
                }
            }
            Some(ScrollPhase::Ended) => {
                // Apply final delta if significant (Ended can carry movement)
                if delta.abs() > 0.1 && !self.animating {
                    self.offset = (self.offset - delta).clamp(0.0, max);
                }

                self.gesture_active = false;
                if self.overscroll.abs() > 0.5 {
                    if !self.animating {
                        // Finger released while overscrolled — start spring from rest
                        self.start_spring(self.overscroll, 0.0);
                    }
                    // If already animating (momentum bounce), let it continue
                } else {
                    self.overscroll = 0.0;
                    self.animating = false;
                }
            }
            None => {
                // No phase info — hard clamp (mouse wheel, keyboard)
                self.scroll_by(delta);
            }
        }
    }

    /// Contact (finger) scroll with boundary-to-overscroll transition.
    /// Uses factor=1.0 so there's no velocity discontinuity at the boundary.
    fn apply_contact_scroll(&mut self, delta: f32, max: f32) {
        let new_offset = self.offset - delta;
        if new_offset < 0.0 {
            self.offset = 0.0;
            let excess = -new_offset;
            self.overscroll -= apply_resistance(excess, self.overscroll, 1.0);
        } else if new_offset > max {
            self.offset = max;
            let excess = new_offset - max;
            self.overscroll += apply_resistance(excess, self.overscroll, 1.0);
        } else {
            self.offset = new_offset;
        }
    }

    /// Start a spring-back animation from the given initial conditions.
    fn start_spring(&mut self, x0: f32, v0: f32) {
        self.spring_x0 = x0;
        self.spring_v0 = v0;
        self.spring_start = Instant::now();
        self.animating = true;
    }

    /// Advance the spring-back animation by one tick.
    /// Returns `true` if the animation is still running and needs another tick.
    ///
    /// Uses the **analytical** critically-damped solution instead of numerical
    /// integration. This guarantees zero oscillation — the position is computed
    /// exactly from `(C1 + C2*t) * e^(-γt)`, which mathematically never
    /// crosses zero when the initial conditions have the same sign.
    pub fn tick_spring_back(&mut self) -> bool {
        if !self.animating {
            return false;
        }

        // Critically damped: γ = friction/(2*mass)
        // x(t) = (C1 + C2*t) * e^(-γt)
        // where C1 = x0, C2 = v0 + γ*x0
        // Lower γ = slower, more luxurious return.
        let gamma = 12.0_f32;
        let t = self.spring_start.elapsed().as_secs_f32();
        let c1 = self.spring_x0;
        let c2 = self.spring_v0 + gamma * self.spring_x0;

        self.overscroll = (c1 + c2 * t) * (-gamma * t).exp();

        if self.overscroll.abs() < 0.5 {
            self.overscroll = 0.0;
            // Only clear animating if the gesture is done. Keeping it true
            // during an active gesture prevents remaining momentum events
            // from re-triggering a new bounce.
            if !self.gesture_active {
                self.animating = false;
            }
            return false;
        }
        true
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
                ..
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
                    let (dy, phase) = match delta {
                        ScrollDelta::Lines { y, .. } => (y * 40.0, None),
                        ScrollDelta::Pixels { y, phase, .. } => (*y, *phase),
                    };
                    return Some(MouseResponse::message(ScrollAction::ScrollBy { delta: dy, phase }));
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
        // Always update bounds — reset to zero when the scroll container is
        // absent from the current layout.  Without this, dismissed popups
        // (completion, history search) retain stale bounds that silently
        // consume WheelScrolled events via `contains()`, blocking the main
        // scroll handler.
        self.bounds.set(
            snapshot.widget_bounds(&self.id).unwrap_or(Rect::ZERO),
        );
    }

    /// Check if a point is within this scroll container's bounds.
    pub fn contains(&self, point: Point) -> bool {
        self.bounds.get().contains_xy(point.x, point.y)
    }
}

/// Rubber-band resistance with a fixed overscroll limit.
///
/// Uses quadratic decay: `factor * (1 - (|os|/SCALE)²)`.  SCALE is a fixed
/// constant (200pt) so the maximum overscroll distance is the same regardless
/// of window size — matching native macOS/iOS behaviour.  Resistance starts
/// at `factor` at the boundary and decays to 0 as overscroll approaches SCALE.
fn apply_resistance(delta: f32, current_overscroll: f32, factor: f32) -> f32 {
    const SCALE: f32 = 120.0; // max overscroll distance in logical points
    let ratio = (current_overscroll.abs() / SCALE).min(1.0);
    let coeff = factor * (1.0 - ratio * ratio);
    delta * coeff
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
    fn scroll_state_new_defaults() {
        let state = ScrollState::new();
        assert_eq!(state.offset, 0.0);
        assert_eq!(state.grab_offset, 0.0);
    }

    #[test]
    fn scroll_state_with_ids() {
        let id = SourceId::new();
        let thumb_id = SourceId::new();
        let state = ScrollState::with_ids(id, thumb_id);
        assert_eq!(state.id(), id);
        assert_eq!(state.thumb_id(), thumb_id);
    }

    #[test]
    fn scroll_state_default() {
        let state: ScrollState = Default::default();
        assert_eq!(state.offset, 0.0);
    }

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

    #[test]
    fn apply_scroll_by() {
        let mut state = ScrollState::new();
        state.max.set(100.0);
        state.apply(ScrollAction::ScrollBy { delta: -30.0, phase: None });
        assert_eq!(state.offset, 30.0);
    }

    #[test]
    fn apply_drag_end() {
        let mut state = ScrollState::new();
        state.grab_offset = 10.0;
        state.apply(ScrollAction::DragEnd);
        assert_eq!(state.grab_offset, 0.0);
    }

    #[test]
    fn contains_uses_bounds() {
        let state = ScrollState::new();
        state.bounds.set(Rect::new(0.0, 0.0, 100.0, 100.0));

        assert!(state.contains(Point::new(50.0, 50.0)));
        assert!(!state.contains(Point::new(150.0, 50.0)));
    }

    #[test]
    fn scroll_action_debug() {
        // Ensure ScrollAction variants can be debug printed
        let actions = [
            ScrollAction::ScrollBy { delta: 10.0, phase: None },
            ScrollAction::DragStart(50.0),
            ScrollAction::DragMove(60.0),
            ScrollAction::DragEnd,
        ];
        for action in actions {
            let _ = format!("{:?}", action);
        }
    }
}
