//! Strata Application Trait
//!
//! Defines the `StrataApp` trait that applications implement to use Strata.
//! This is similar to iced's application pattern but with Strata primitives.

use std::future::Future;
use std::pin::Pin;

use crate::strata::content_address::{ContentAddress, Selection, SourceId};
use crate::strata::event_context::{CaptureState, MouseEvent};
use crate::strata::layout_snapshot::LayoutSnapshot;

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
}

impl<M> Default for MouseResponse<M> {
    fn default() -> Self {
        Self::none()
    }
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
/// Subscriptions are recreated each frame based on application state.
/// They produce messages when external events occur.
pub struct Subscription<M> {
    /// Recipes for creating subscription streams.
    /// Each recipe is identified by a type ID for deduplication.
    recipes: Vec<Box<dyn SubscriptionRecipe<Output = M>>>,
}

impl<M> Subscription<M> {
    /// Create an empty subscription.
    pub fn none() -> Self {
        Self {
            recipes: Vec::new(),
        }
    }

    /// Batch multiple subscriptions together.
    pub fn batch(subscriptions: impl IntoIterator<Item = Subscription<M>>) -> Self {
        Self {
            recipes: subscriptions.into_iter().flat_map(|s| s.recipes).collect(),
        }
    }

    /// Check if this subscription has no recipes.
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

impl<M> Default for Subscription<M> {
    fn default() -> Self {
        Self::none()
    }
}

/// A recipe for creating a subscription stream.
///
/// This is a placeholder - the actual implementation will integrate
/// with iced's subscription system.
pub trait SubscriptionRecipe: Send {
    type Output;
}

/// The main application trait for Strata.
///
/// Applications implement this trait and run via `strata::shell::run()`.
/// The architecture follows the Elm pattern: init → update → view.
pub trait StrataApp: Sized + 'static {
    /// Application state type.
    type State: Send + 'static;

    /// Message type that drives state updates.
    type Message: Clone + Send + std::fmt::Debug + 'static;

    /// Initialize the application state.
    ///
    /// Returns the initial state and an optional command to run.
    fn init() -> (Self::State, Command<Self::Message>);

    /// Update state in response to a message.
    ///
    /// Returns a command for any async work to perform.
    fn update(state: &mut Self::State, message: Self::Message) -> Command<Self::Message>;

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
        _hit: Option<ContentAddress>,
        _capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        MouseResponse::none()
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

    /// Whether the application should exit.
    fn should_exit(_state: &Self::State) -> bool {
        false
    }
}

/// Configuration for running a Strata application.
pub struct AppConfig {
    /// Window title.
    pub title: String,

    /// Initial window size.
    pub window_size: (f32, f32),

    /// Whether to enable antialiasing.
    pub antialiasing: bool,

    /// Background color.
    pub background_color: crate::strata::primitives::Color,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: String::from("Strata App"),
            window_size: (1200.0, 800.0),
            antialiasing: true,
            background_color: crate::strata::primitives::Color::BLACK,
        }
    }
}
