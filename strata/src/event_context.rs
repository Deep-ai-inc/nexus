//! Event Context
//!
//! Provides `EventContext` which gives widgets access to:
//! - The current `LayoutSnapshot` for hit-testing
//! - Pointer capture (all mouse events go to capturing widget)
//! - Message emission
//!
//! Global pointer capture ensures drag selection works correctly
//! even when the cursor leaves the widget bounds.

use std::cell::RefCell;
use std::path::PathBuf;

use crate::content_address::SourceId;
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::Point;

/// Capture state for pointer events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    /// No capture - events route based on hit-testing.
    None,

    /// Captured by a specific source.
    /// All pointer events go to this source until released.
    Captured(SourceId),
}

impl Default for CaptureState {
    fn default() -> Self {
        Self::None
    }
}

impl CaptureState {
    /// Check if the pointer is currently captured.
    pub fn is_captured(&self) -> bool {
        matches!(self, CaptureState::Captured(_))
    }

    /// Get the source that has captured the pointer, if any.
    pub fn captured_by(&self) -> Option<SourceId> {
        match self {
            CaptureState::Captured(source) => Some(*source),
            CaptureState::None => None,
        }
    }
}

/// Event context provided to widget event handlers.
///
/// Provides access to:
/// - The layout snapshot for hit-testing
/// - Pointer capture control
/// - The ability to emit messages
pub struct EventContext<'a> {
    /// The current frame's layout snapshot.
    pub layout: &'a LayoutSnapshot,

    /// Current pointer capture state.
    capture: RefCell<CaptureState>,
}

impl<'a> EventContext<'a> {
    /// Create a new event context.
    pub fn new(layout: &'a LayoutSnapshot) -> Self {
        Self {
            layout,
            capture: RefCell::new(CaptureState::None),
        }
    }

    /// Create an event context with existing capture state.
    pub fn with_capture(layout: &'a LayoutSnapshot, capture: CaptureState) -> Self {
        Self {
            layout,
            capture: RefCell::new(capture),
        }
    }

    /// Capture pointer events for the given source.
    ///
    /// All subsequent pointer events (mouse move, button release) will be
    /// directed to this source until `release_capture()` is called.
    ///
    /// This is essential for drag operations (like text selection) where
    /// the user may drag outside the widget bounds.
    pub fn capture_pointer(&self, source: SourceId) {
        *self.capture.borrow_mut() = CaptureState::Captured(source);
    }

    /// Release pointer capture.
    ///
    /// Events will route normally based on hit-testing.
    pub fn release_capture(&self) {
        *self.capture.borrow_mut() = CaptureState::None;
    }

    /// Get the current capture state.
    pub fn capture_state(&self) -> CaptureState {
        *self.capture.borrow()
    }

    /// Check if the pointer is currently captured.
    pub fn is_captured(&self) -> bool {
        matches!(*self.capture.borrow(), CaptureState::Captured(_))
    }

    /// Get the source that has captured the pointer, if any.
    pub fn captured_by(&self) -> Option<SourceId> {
        match *self.capture.borrow() {
            CaptureState::Captured(source) => Some(source),
            CaptureState::None => None,
        }
    }

    /// Check if a specific source has captured the pointer.
    pub fn is_captured_by(&self, source: SourceId) -> bool {
        self.captured_by() == Some(source)
    }

    /// Take the capture state (for transferring to next frame).
    pub fn take_capture(&self) -> CaptureState {
        self.capture.take()
    }
}

/// Mouse button types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Mouse event types.
#[derive(Debug, Clone)]
pub enum MouseEvent {
    /// Mouse button pressed.
    ButtonPressed {
        button: MouseButton,
        position: Point,
    },

    /// Mouse button released.
    ButtonReleased {
        button: MouseButton,
        position: Point,
    },

    /// Mouse cursor moved.
    CursorMoved {
        position: Point,
    },

    /// Mouse cursor entered the window.
    CursorEntered,

    /// Mouse cursor left the window.
    CursorLeft,

    /// Mouse wheel scrolled.
    WheelScrolled {
        delta: ScrollDelta,
        position: Point,
    },
}

/// File drop events from the OS (drag files onto the window).
#[derive(Debug, Clone)]
pub enum FileDropEvent {
    /// A file is being hovered over the window.
    Hovered(PathBuf),
    /// A file was dropped onto the window.
    Dropped(PathBuf),
    /// All hovered files left the window.
    HoverLeft,
}

/// Phase of a scroll gesture (trackpad or momentum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPhase {
    /// Finger on trackpad (NSEventPhase began/changed).
    Contact,
    /// OS momentum after finger lift (momentumPhase began/changed).
    Momentum,
    /// Gesture or momentum finished.
    Ended,
}

/// Scroll delta types.
#[derive(Debug, Clone, Copy)]
pub enum ScrollDelta {
    /// Scroll by lines (discrete, e.g., mouse wheel notches).
    Lines { x: f32, y: f32 },

    /// Scroll by pixels (smooth, e.g., trackpad).
    Pixels { x: f32, y: f32, phase: Option<ScrollPhase> },
}

/// Keyboard modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool, // Command on macOS, Windows key on Windows
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };

    pub const SHIFT: Self = Self {
        shift: true,
        ctrl: false,
        alt: false,
        meta: false,
    };

    pub const CTRL: Self = Self {
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    };

    pub const ALT: Self = Self {
        shift: false,
        ctrl: false,
        alt: true,
        meta: false,
    };

    pub const META: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        meta: true,
    };

    /// Check if the command key is pressed (Ctrl on non-Mac, Meta on Mac).
    #[cfg(target_os = "macos")]
    pub fn command(&self) -> bool {
        self.meta
    }

    #[cfg(not(target_os = "macos"))]
    pub fn command(&self) -> bool {
        self.ctrl
    }

    /// Check if any modifier is pressed.
    pub fn any(&self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }

    /// Check if no modifiers are pressed.
    pub fn none(&self) -> bool {
        !self.any()
    }
}

/// Named keys (non-character keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    // Navigation
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,

    // Editing
    Backspace,
    Delete,
    Insert,
    Enter,
    Tab,

    // Modifiers (for key events, not Modifiers struct)
    Shift,
    Control,
    Alt,
    Meta,

    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    // Special
    Escape,
    Space,
    CapsLock,
    NumLock,
    ScrollLock,
    PrintScreen,
    Pause,

    // Other
    ContextMenu,
    Unknown,
}

/// A key event (pressed or released).
#[derive(Debug, Clone)]
pub enum KeyEvent {
    /// A key was pressed.
    Pressed {
        key: Key,
        modifiers: Modifiers,
        /// The text produced by the key press (OS-level, handles shift/compose/dead keys).
        text: Option<String>,
    },

    /// A key was released.
    Released {
        key: Key,
        modifiers: Modifiers,
    },
}

/// A keyboard key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A named (special) key.
    Named(NamedKey),

    /// A character key.
    Character(String),
}

impl Key {
    /// Create a named key.
    pub fn named(key: NamedKey) -> Self {
        Self::Named(key)
    }

    /// Create a character key.
    pub fn character(c: impl Into<String>) -> Self {
        Self::Character(c.into())
    }
}

/// Generic event type combining mouse and keyboard events.
#[derive(Debug, Clone)]
pub enum Event {
    Mouse(MouseEvent),
    Keyboard(KeyEvent),
}

impl From<MouseEvent> for Event {
    fn from(event: MouseEvent) -> Self {
        Self::Mouse(event)
    }
}

impl From<KeyEvent> for Event {
    fn from(event: KeyEvent) -> Self {
        Self::Keyboard(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_snapshot::LayoutSnapshot;

    #[test]
    fn capture_pointer() {
        let snapshot = LayoutSnapshot::new();
        let ctx = EventContext::new(&snapshot);

        assert!(!ctx.is_captured());
        assert_eq!(ctx.captured_by(), None);

        let source = SourceId::new();
        ctx.capture_pointer(source);

        assert!(ctx.is_captured());
        assert_eq!(ctx.captured_by(), Some(source));
        assert!(ctx.is_captured_by(source));

        let other_source = SourceId::new();
        assert!(!ctx.is_captured_by(other_source));

        ctx.release_capture();
        assert!(!ctx.is_captured());
        assert_eq!(ctx.captured_by(), None);
    }

    #[test]
    fn modifiers() {
        let mods = Modifiers {
            shift: true,
            ctrl: true,
            alt: false,
            meta: false,
        };

        assert!(mods.any());
        assert!(!mods.none());

        let no_mods = Modifiers::NONE;
        assert!(!no_mods.any());
        assert!(no_mods.none());
    }

    #[test]
    fn capture_state_is_captured() {
        let none = CaptureState::None;
        assert!(!none.is_captured());
        assert!(none.captured_by().is_none());

        let source = SourceId::new();
        let captured = CaptureState::Captured(source);
        assert!(captured.is_captured());
        assert_eq!(captured.captured_by(), Some(source));
    }

    #[test]
    fn capture_state_default() {
        let state: CaptureState = Default::default();
        assert!(!state.is_captured());
    }

    #[test]
    fn event_context_with_capture() {
        let snapshot = LayoutSnapshot::new();
        let source = SourceId::new();
        let ctx = EventContext::with_capture(&snapshot, CaptureState::Captured(source));

        assert!(ctx.is_captured());
        assert_eq!(ctx.capture_state(), CaptureState::Captured(source));
    }

    #[test]
    fn event_context_take_capture() {
        let snapshot = LayoutSnapshot::new();
        let source = SourceId::new();
        let ctx = EventContext::new(&snapshot);
        ctx.capture_pointer(source);

        let taken = ctx.take_capture();
        assert_eq!(taken, CaptureState::Captured(source));
        // After take, context should be None
        assert!(!ctx.is_captured());
    }

    #[test]
    fn key_constructors() {
        let named = Key::named(NamedKey::Enter);
        assert!(matches!(named, Key::Named(NamedKey::Enter)));

        let char_key = Key::character("a");
        assert!(matches!(char_key, Key::Character(s) if s == "a"));

        let char_key_string = Key::character(String::from("abc"));
        assert!(matches!(char_key_string, Key::Character(s) if s == "abc"));
    }

    #[test]
    fn modifiers_command() {
        let meta = Modifiers::META;
        let ctrl = Modifiers::CTRL;

        #[cfg(target_os = "macos")]
        {
            assert!(meta.command());
            assert!(!ctrl.command());
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert!(!meta.command());
            assert!(ctrl.command());
        }
    }

    #[test]
    fn modifiers_constants() {
        assert!(Modifiers::SHIFT.shift);
        assert!(!Modifiers::SHIFT.ctrl);

        assert!(Modifiers::CTRL.ctrl);
        assert!(!Modifiers::CTRL.shift);

        assert!(Modifiers::ALT.alt);
        assert!(!Modifiers::ALT.meta);

        assert!(Modifiers::META.meta);
        assert!(!Modifiers::META.alt);
    }

    #[test]
    fn event_from_mouse() {
        let mouse = MouseEvent::CursorEntered;
        let event: Event = mouse.into();
        assert!(matches!(event, Event::Mouse(MouseEvent::CursorEntered)));
    }

    #[test]
    fn event_from_keyboard() {
        let key = KeyEvent::Released {
            key: Key::named(NamedKey::Escape),
            modifiers: Modifiers::NONE,
        };
        let event: Event = key.into();
        assert!(matches!(event, Event::Keyboard(KeyEvent::Released { .. })));
    }
}
