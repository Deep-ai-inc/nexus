//! Nestable Component System
//!
//! A zero-cost, statically-dispatched component system for Strata.
//!
//! Components are struct instances that own their state and children as fields.
//! All calls are monomorphized — no trait objects, no vtables in the hot path.
//!
//! # Architecture
//!
//! - **`Component`**: Core trait. The component IS the state (`&mut self`).
//! - **`RootComponent`**: Extension for top-level components that bridge to `StrataApp`.
//! - **`IdSpace`**: Zero-allocation, const-fn ID namespacing.
//! - **`Slot` + `MsgMap`**: Optional typed boundaries for local message types.
//! - **`ComponentApp`**: Bridges a `RootComponent` to `StrataApp`.

mod component_app;
mod id_space;
mod slot;

pub use component_app::{ComponentApp, RootComponent};
pub use id_space::IdSpace;
pub use slot::{MsgMap, Slot};

use crate::app::{Command, MouseResponse, Subscription};
use crate::content_address::Selection;
use crate::event_context::{CaptureState, FileDropEvent, KeyEvent, MouseEvent};
use crate::gpu::ImageStore;
use crate::layout_snapshot::{HitResult, LayoutSnapshot};

/// Shared context passed through the component tree during update.
pub struct Ctx<'a> {
    pub images: &'a mut ImageStore,
}

/// A nestable, statically-dispatched UI component.
///
/// Components own their state and children as struct fields.
/// `update()` takes `&mut self` for mutations; `on_key()`/`on_mouse()` take
/// `&self` (events produce messages, mutations happen in `update()`).
///
/// `Message` requires only `Send + Debug + 'static`, NOT `Clone`.
/// Messages are routed by value. `Clone` is only required at the
/// `ComponentApp` bridge to satisfy `StrataApp`'s bound.
pub trait Component {
    type Message: Send + std::fmt::Debug + 'static;

    /// Cross-cutting output returned alongside commands from `update()`.
    ///
    /// Use `()` for components with no cross-cutting output.
    /// Parent components read this to apply effects that span children
    /// (focus changes, scroll-to-bottom, cwd propagation, etc.).
    type Output: Default;

    /// Handle a message, returning async commands and any cross-cutting output.
    fn update(&mut self, msg: Self::Message, ctx: &mut Ctx) -> (Command<Self::Message>, Self::Output);

    /// Render the component into the layout snapshot.
    fn view(&self, snapshot: &mut LayoutSnapshot, ids: IdSpace);

    /// Handle a keyboard event. Returns a message to dispatch, or None.
    ///
    /// Takes `&self` (read-only) — emit a message, mutate in `update()`.
    /// For hover/drag state, emit a message rather than using `Cell`/`RefCell`.
    fn on_key(&self, _event: KeyEvent) -> Option<Self::Message> {
        None
    }

    /// Handle a mouse event. Returns a response with optional message + capture.
    ///
    /// Takes `&self` (read-only) — matches StrataApp's immutable event pattern.
    fn on_mouse(
        &self,
        _event: MouseEvent,
        _hit: Option<HitResult>,
        _capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        MouseResponse::none()
    }

    /// Handle a file drop event from the OS.
    fn on_file_drop(
        &self,
        _event: FileDropEvent,
        _hit: Option<HitResult>,
    ) -> Option<Self::Message> {
        None
    }

    /// Create subscriptions for external event streams.
    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::none()
    }

    /// Get the current selection, if any.
    fn selection(&self) -> Option<&Selection> {
        None
    }

    /// Current zoom level (1.0 = 100%).
    fn zoom_level(&self) -> f32 {
        1.0
    }

    /// Look up the word at a content address for Force Click dictionary lookup.
    ///
    /// Returns `(word_text, word_start_addr, font_size)`.
    fn force_click_lookup(
        &self,
        _addr: &crate::content_address::ContentAddress,
    ) -> Option<(String, crate::content_address::ContentAddress, f32)> {
        None
    }

    /// Called by the native backend at ~60fps. Use for periodic effects like
    /// auto-scroll during drag selection. Returns `true` if state changed and
    /// a render is needed.
    fn on_tick(&mut self) -> bool {
        false
    }
}
