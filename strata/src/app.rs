//! Strata Application Trait
//!
//! Defines the `StrataApp` trait that applications implement to use Strata.
//! This is similar to iced's application pattern but with Strata primitives.

use std::future::Future;
use std::pin::Pin;

use crate::content_address::{Selection, SourceId};
use crate::event_context::{CaptureState, FileDropEvent, KeyEvent, MouseEvent};
use crate::gpu::ImageStore;
use crate::layout_snapshot::{HitResult, LayoutSnapshot};

/// Response from a mouse event handler.
///
/// Combines an optional message with optional pointer capture state changes.
/// This allows widgets to both update state AND request pointer capture atomically.
#[derive(Debug)]
pub struct MouseResponse<M> {
    /// Optional message to send to update().
    pub message: Option<M>,

    /// Pointer capture request.
    pub capture: CaptureRequest,
}

impl<M> MouseResponse<M> {
    /// No response (no message, no capture change).
    pub fn none() -> Self {
        Self {
            message: None,
            capture: CaptureRequest::None,
        }
    }

    /// Response with just a message.
    pub fn message(msg: M) -> Self {
        Self {
            message: Some(msg),
            capture: CaptureRequest::None,
        }
    }

    /// Response that captures the pointer for a source.
    pub fn capture(source: SourceId) -> Self {
        Self {
            message: None,
            capture: CaptureRequest::Capture(source),
        }
    }

    /// Response with message that also captures the pointer.
    pub fn message_and_capture(msg: M, source: SourceId) -> Self {
        Self {
            message: Some(msg),
            capture: CaptureRequest::Capture(source),
        }
    }

    /// Response that releases pointer capture.
    pub fn release() -> Self {
        Self {
            message: None,
            capture: CaptureRequest::Release,
        }
    }

    /// Response with message that also releases capture.
    pub fn message_and_release(msg: M) -> Self {
        Self {
            message: Some(msg),
            capture: CaptureRequest::Release,
        }
    }

    /// Transform the message type, preserving capture state.
    ///
    /// This enables composable mouse handling: widget-level handlers return
    /// `MouseResponse<WidgetAction>`, and the app maps to its message type:
    /// ```ignore
    /// if let Some(r) = state.scroll.handle_mouse(&event, &hit, capture) {
    ///     return r.map(AppMessage::Scroll);
    /// }
    /// ```
    pub fn map<N>(self, f: impl FnOnce(M) -> N) -> MouseResponse<N> {
        MouseResponse {
            message: self.message.map(f),
            capture: self.capture,
        }
    }
}

impl<M> Default for MouseResponse<M> {
    fn default() -> Self {
        Self::none()
    }
}

/// Zero-cost mouse event router for composable handlers.
///
/// Expands at compile time into a flat sequence of `if let Some(r) = ... { return r.map(...) }`
/// checks. No tree traversal, no heap allocation — identical assembly to hand-written chains.
///
/// # Usage
/// ```ignore
/// route_mouse!(event, hit, capture, [
///     state.left_scroll  => DemoMessage::LeftScroll,
///     state.right_scroll => DemoMessage::RightScroll,
///     state.input        => DemoMessage::InputMouse,
/// ]);
/// ```
#[macro_export]
macro_rules! route_mouse {
    ($event:expr, $hit:expr, $capture:expr, [ $($target:expr => $msg:expr),* $(,)? ]) => {
        $(
            if let Some(r) = $target.handle_mouse($event, $hit, $capture) {
                return r.map($msg);
            }
        )*
    };
}

/// Request to change pointer capture state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRequest {
    /// No change to capture state.
    None,

    /// Capture the pointer for the specified source.
    /// While captured, mouse events will be dispatched even when outside widget bounds.
    Capture(SourceId),

    /// Release pointer capture.
    Release,
}

/// A command that produces a message asynchronously.
pub struct Command<M> {
    futures: Vec<Pin<Box<dyn Future<Output = M> + Send + 'static>>>,
}

impl<M> Command<M> {
    /// Create an empty command (no async work).
    pub fn none() -> Self {
        Self {
            futures: Vec::new(),
        }
    }

    /// Create a command from a future.
    pub fn perform<F>(future: F) -> Self
    where
        F: Future<Output = M> + Send + 'static,
    {
        Self {
            futures: vec![Box::pin(future)],
        }
    }

    /// Create a command that immediately produces a message.
    pub fn message(msg: M) -> Self
    where
        M: Send + 'static,
    {
        Self::perform(async move { msg })
    }

    /// Batch multiple commands together.
    pub fn batch(commands: impl IntoIterator<Item = Command<M>>) -> Self {
        Self {
            futures: commands.into_iter().flat_map(|c| c.futures).collect(),
        }
    }

    /// Map the message type using a function item.
    ///
    /// Wraps each future in an async adapter (one `Box::pin` per future).
    /// Commands are not hot-path, so this allocation is acceptable.
    /// Uses `fn` pointer (not closure) so it's `Copy` — ideal for enum
    /// variant constructors like `ParentMsg::Child`.
    pub fn map_msg<N: Send + 'static>(self, f: fn(M) -> N) -> Command<N>
    where
        M: Send + 'static,
    {
        Command {
            futures: self
                .futures
                .into_iter()
                .map(|fut| {
                    Box::pin(async move { f(fut.await) })
                        as Pin<Box<dyn Future<Output = N> + Send>>
                })
                .collect(),
        }
    }

    /// Check if this command has no work to do.
    pub fn is_empty(&self) -> bool {
        self.futures.is_empty()
    }

    /// Take the futures from this command.
    pub fn take_futures(&mut self) -> Vec<Pin<Box<dyn Future<Output = M> + Send + 'static>>> {
        std::mem::take(&mut self.futures)
    }
}

impl<M> Default for Command<M> {
    fn default() -> Self {
        Self::none()
    }
}

/// A subscription to external events.
///
/// Thin wrapper around `iced::Subscription` for zero-overhead pass-through.
/// Apps construct these using `from_iced()` and the adapter wires them directly
/// into iced's subscription system.
pub struct Subscription<M> {
    pub(crate) subs: Vec<iced::Subscription<M>>,
}

impl<M> Subscription<M> {
    /// Create an empty subscription.
    pub fn none() -> Self {
        Self { subs: Vec::new() }
    }

    /// Create a subscription from a native iced subscription.
    pub fn from_iced(sub: iced::Subscription<M>) -> Self {
        Self { subs: vec![sub] }
    }

    /// Batch multiple subscriptions together.
    pub fn batch(subscriptions: impl IntoIterator<Item = Subscription<M>>) -> Self {
        Self {
            subs: subscriptions.into_iter().flat_map(|s| s.subs).collect(),
        }
    }

    /// Map the message type using a closure.
    ///
    /// The closure must be Clone because subscriptions are rebuilt per-frame.
    pub fn map<F, N>(self, f: F) -> Subscription<N>
    where
        M: 'static,
        N: 'static,
        F: Fn(M) -> N + Clone + Send + 'static,
    {
        Subscription {
            subs: self.subs.into_iter().map(|s| s.map(f.clone())).collect(),
        }
    }

    /// Map the message type using a function pointer.
    ///
    /// Useful when you have a named function or method reference.
    pub fn map_msg<N: 'static>(self, f: fn(M) -> N) -> Subscription<N>
    where
        M: 'static,
    {
        self.map(f)
    }

    /// Check if this subscription is empty.
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }
}

impl<M> Default for Subscription<M> {
    fn default() -> Self {
        Self::none()
    }
}

/// The main application trait for Strata.
///
/// Applications implement this trait and run via `strata::shell::run()`.
/// The architecture follows the Elm pattern: init → update → view.
///
/// Multi-window support: Apps that want multiple windows implement
/// `SharedState` (for cross-window resources like a shared kernel)
/// and `create_window()`. The shell adapter manages window lifecycle.
pub trait StrataApp: Sized + 'static {
    /// Application state type (per-window).
    type State: 'static;

    /// Message type that drives state updates.
    type Message: Clone + Send + std::fmt::Debug + 'static;

    /// Shared state across all windows. Clone-based (use Arc internally).
    /// Default `()` for single-window apps.
    type SharedState: Clone + Default + 'static;

    /// Initialize the first window's application state.
    ///
    /// Returns the initial state and an optional command to run.
    /// The `images` store can be used to load images (PNG, raw RGBA)
    /// that will be uploaded to the GPU before the first frame.
    fn init(shared: &Self::SharedState, images: &mut ImageStore) -> (Self::State, Command<Self::Message>);

    /// Create state for a new window (e.g. Cmd+N).
    /// Returns `None` if multi-window is not supported.
    fn create_window(_shared: &Self::SharedState, _images: &mut ImageStore) -> Option<(Self::State, Command<Self::Message>)> {
        None
    }

    /// Check if a message is a request to open a new window.
    /// The adapter intercepts these before dispatching to `update()`.
    fn is_new_window_request(_msg: &Self::Message) -> bool {
        false
    }

    /// Check if a message is a request to quit the entire application.
    fn is_exit_request(_msg: &Self::Message) -> bool {
        false
    }

    /// Update state in response to a message.
    ///
    /// Returns a command for any async work to perform.
    /// The `images` store can be used to dynamically load new images.
    fn update(state: &mut Self::State, message: Self::Message, images: &mut ImageStore) -> Command<Self::Message>;

    /// Build the view and populate the layout snapshot.
    ///
    /// This is called each frame. Widgets should register their content
    /// with the snapshot during this call.
    fn view(state: &Self::State, snapshot: &mut LayoutSnapshot);

    /// Get the current selection, if any.
    ///
    /// Used by the renderer to draw selection highlights.
    fn selection(state: &Self::State) -> Option<&Selection>;

    /// Handle a mouse event.
    ///
    /// Called by the shell when a mouse event occurs. The `hit` parameter
    /// contains the ContentAddress at the mouse position (if any).
    /// The `capture` parameter indicates if the pointer is currently captured,
    /// which is essential for handling drag operations outside widget bounds.
    ///
    /// Returns a `MouseResponse` that can include:
    /// - An optional message to send to `update()`
    /// - A capture request to capture or release the pointer
    ///
    /// Use `MouseResponse::message_and_capture()` to start drag selection,
    /// and `MouseResponse::message_and_release()` to end it.
    fn on_mouse(
        _state: &Self::State,
        _event: MouseEvent,
        _hit: Option<HitResult>,
        _capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        MouseResponse::none()
    }

    /// Handle a file drop event from the OS.
    ///
    /// Called when the user drags files onto the window. The `hit` parameter
    /// contains what's at the cursor position for drop target resolution.
    fn on_file_drop(
        _state: &Self::State,
        _event: FileDropEvent,
        _hit: Option<HitResult>,
    ) -> Option<Self::Message> {
        None
    }

    /// Handle a keyboard event.
    ///
    /// Called by the shell on key press/release. Dispatched globally
    /// (no hit-testing). The app decides routing based on focus state.
    fn on_key(
        _state: &Self::State,
        _event: KeyEvent,
    ) -> Option<Self::Message> {
        None
    }

    /// Create subscriptions based on current state.
    ///
    /// Subscriptions are recreated each frame. The shell will
    /// deduplicate and manage the actual subscription streams.
    fn subscription(_state: &Self::State) -> Subscription<Self::Message> {
        Subscription::none()
    }

    /// Application title (shown in window title bar).
    fn title(_state: &Self::State) -> String {
        String::from("Strata App")
    }

    /// Look up the word at a content address for Force Click dictionary lookup.
    ///
    /// Returns `(word_text, word_start_addr, font_size)` so the adapter can
    /// resolve the exact pixel position from the layout snapshot.
    fn force_click_lookup(
        _state: &Self::State,
        _addr: &crate::content_address::ContentAddress,
    ) -> Option<(String, crate::content_address::ContentAddress, f32)> {
        None
    }

    /// Background color for the application window.
    fn background_color(_state: &Self::State) -> crate::primitives::Color {
        crate::primitives::Color::BLACK
    }

    /// Whether the application should exit.
    fn should_exit(_state: &Self::State) -> bool {
        false
    }

    /// Current zoom level (1.0 = 100%). Used by the shell adapter for GPU scaling
    /// and window resize on zoom change.
    fn zoom_level(_state: &Self::State) -> f32 {
        1.0
    }
}

/// Request to start an OS-level outbound drag.
///
/// The app layer decides *what* to drag; the platform layer handles *how*.
#[derive(Debug, Clone)]
pub enum DragSource {
    /// Drag a file — the OS shows the file icon, Finder accepts it.
    /// For ephemeral data (e.g. table exports), the app layer writes to a temp
    /// file and passes the path here — keeping the platform layer I/O-free.
    File(std::path::PathBuf),
    /// Drag plain text.
    Text(String),
    /// Drag TSV (spreadsheets accept structured paste).
    Tsv(String),
    /// Drag an image file — the OS shows the file icon.
    Image(std::path::PathBuf),
}

/// Configuration for running a Strata application.
#[derive(Clone)]
pub struct AppConfig {
    /// Window title.
    pub title: String,

    /// Initial window size.
    pub window_size: (f32, f32),

    /// Whether to enable antialiasing.
    pub antialiasing: bool,

    /// Background color.
    pub background_color: crate::primitives::Color,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: String::from("Strata App"),
            window_size: (1200.0, 800.0),
            antialiasing: true,
            background_color: crate::primitives::Color::BLACK,
        }
    }
}
