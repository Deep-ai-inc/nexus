//! Bridge from Component to StrataApp.
//!
//! `ComponentApp<C>` implements `StrataApp` by delegating to a root `Component`.
//! The component IS the state — `StrataApp::State = C`.

use std::marker::PhantomData;

use crate::app::{Command, MouseResponse, StrataApp, Subscription};
use crate::content_address::Selection;
use crate::event_context::{CaptureState, FileDropEvent, KeyEvent, MouseEvent};
use crate::gpu::ImageStore;
use crate::layout_snapshot::{HitResult, LayoutSnapshot};
use crate::primitives::Color;

use super::{Component, Ctx, IdSpace};

/// Extension of `Component` for top-level application roots.
///
/// Provides the `create()` factory (matching `StrataApp::init()`) and
/// app-level metadata (title, background color, exit condition).
pub trait RootComponent: Component + Sized {
    /// Shared state across all windows. Clone-based (use Arc internally).
    type SharedState: Clone + Default + 'static;

    /// Create the root component and any initial async commands.
    fn create(shared: &Self::SharedState, images: &mut ImageStore) -> (Self, Command<Self::Message>);

    /// Create state for a new window. Returns `None` if unsupported.
    fn create_window(_shared: &Self::SharedState, _images: &mut ImageStore) -> Option<(Self, Command<Self::Message>)> {
        None
    }

    /// Check if a message is a request to open a new window.
    fn is_new_window_request(_msg: &Self::Message) -> bool {
        false
    }

    /// Check if a message is a request to quit the entire application.
    fn is_exit_request(_msg: &Self::Message) -> bool {
        false
    }

    /// Application window title.
    fn title(&self) -> String {
        String::from("Strata App")
    }

    /// Application background color.
    fn background_color(&self) -> Color {
        Color::BLACK
    }

    /// Whether the application should exit.
    fn should_exit(&self) -> bool {
        false
    }

    /// Optional integer tag for the window (used for external scripting).
    fn window_tag(&self) -> isize {
        0
    }

    /// Called after the NSWindow is created, with the raw window pointer.
    fn on_window_ready(&self, _nswindow_ptr: usize) {}

    /// The root `IdSpace` for the component tree.
    fn root_ids() -> IdSpace {
        IdSpace::new(0)
    }
}

/// Bridges a `RootComponent` to `StrataApp`.
///
/// The component instance IS the application state.
/// `Clone` is required on Message only here (to satisfy `StrataApp`),
/// not on `Component::Message` in general.
pub struct ComponentApp<C: RootComponent> {
    _phantom: PhantomData<C>,
}

impl<C: RootComponent + 'static> StrataApp for ComponentApp<C>
where
    C::Message: Clone,
{
    type State = C;
    type Message = C::Message;
    type SharedState = C::SharedState;

    fn init(shared: &Self::SharedState, images: &mut ImageStore) -> (C, Command<C::Message>) {
        C::create(shared, images)
    }

    fn create_window(shared: &Self::SharedState, images: &mut ImageStore) -> Option<(C, Command<C::Message>)> {
        C::create_window(shared, images)
    }

    fn is_new_window_request(msg: &C::Message) -> bool {
        C::is_new_window_request(msg)
    }

    fn is_exit_request(msg: &C::Message) -> bool {
        C::is_exit_request(msg)
    }

    fn update(
        state: &mut C,
        msg: C::Message,
        images: &mut ImageStore,
    ) -> Command<C::Message> {
        let mut ctx = Ctx { images };
        let (cmd, _output) = state.update(msg, &mut ctx);
        cmd
    }

    fn view(state: &C, snapshot: &mut LayoutSnapshot) {
        state.view(snapshot, C::root_ids())
    }

    fn on_key(state: &C, event: KeyEvent) -> Option<C::Message> {
        state.on_key(event)
    }

    fn on_mouse(
        state: &C,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<C::Message> {
        state.on_mouse(event, hit, capture)
    }

    fn on_file_drop(
        state: &C,
        event: FileDropEvent,
        hit: Option<HitResult>,
    ) -> Option<C::Message> {
        state.on_file_drop(event, hit)
    }

    fn subscription(state: &C) -> Subscription<C::Message> {
        state.subscription()
    }

    fn selection(state: &C) -> Option<&Selection> {
        state.selection()
    }

    fn title(state: &C) -> String {
        RootComponent::title(state)
    }

    fn background_color(state: &C) -> Color {
        RootComponent::background_color(state)
    }

    fn should_exit(state: &C) -> bool {
        RootComponent::should_exit(state)
    }

    fn zoom_level(state: &C) -> f32 {
        state.zoom_level()
    }

    fn force_click_lookup(
        state: &C,
        addr: &crate::content_address::ContentAddress,
    ) -> Option<(String, crate::content_address::ContentAddress, f32)> {
        state.force_click_lookup(addr)
    }

    fn on_tick(state: &mut C) -> (bool, crate::app::Command<C::Message>) {
        state.on_tick()
    }

    fn window_tag(state: &C) -> isize {
        state.window_tag()
    }

    fn on_window_ready(state: &C, nswindow_ptr: usize) {
        state.on_window_ready(nswindow_ptr);
    }

    fn on_native_menu_result(state: &mut C, index: usize) -> Option<C::Message> {
        state.on_native_menu_result(index)
    }
}
