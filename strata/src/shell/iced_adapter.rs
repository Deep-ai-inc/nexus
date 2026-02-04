//! Iced Shell Adapter
//!
//! This module bridges Strata applications to iced for window management,
//! event handling, and GPU rendering.
//!
//! **This is the ONLY file in Strata that imports iced.**

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iced::widget::shader::{self, wgpu};
use iced::{Element, Event, Length, Subscription, Task, Theme};

use crate::app::{AppConfig, Command, StrataApp};
use crate::content_address::Selection;
use crate::layout_snapshot::HitResult;
use crate::gpu::{ImageHandle, PendingImage, StrataPipeline};
use crate::event_context::{
    CaptureState, KeyEvent, Modifiers, MouseButton, MouseEvent, NamedKey, ScrollDelta,
};
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Point, Rect};

/// Error type for shell operations.
#[derive(Debug)]
pub enum Error {
    /// iced error during initialization or runtime.
    Iced(iced::Error),
}

impl From<iced::Error> for Error {
    fn from(err: iced::Error) -> Self {
        Self::Iced(err)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Iced(e) => write!(f, "iced error: {}", e),
        }
    }
}

impl std::error::Error for Error {}

/// Run a Strata application with default configuration.
pub fn run<A: StrataApp>() -> Result<(), Error> {
    run_with_config::<A>(AppConfig::default())
}

/// Run a Strata application with custom configuration.
///
/// Uses `iced::daemon()` for multi-window support. The first window is opened
/// explicitly during initialization; additional windows via `ShellMessage::OpenNewWindow`.
pub fn run_with_config<A: StrataApp>(config: AppConfig) -> Result<(), Error> {
    iced::daemon(
        title::<A>,
        update::<A>,
        view::<A>,
    )
    .subscription(subscription::<A>)
    .theme(|_, _| Theme::Dark)
    .antialiasing(config.antialiasing)
    .run_with(move || init::<A>(config))
    .map_err(Error::from)
}

// ============================================================================
// Multi-Window State
// ============================================================================

/// Top-level state holding all windows and shared cross-window resources.
struct MultiWindowState<A: StrataApp> {
    /// Shared state across all windows (kernel, block IDs, etc.)
    shared: A::SharedState,

    /// Per-window state indexed by iced window ID.
    windows: HashMap<iced::window::Id, WindowState<A>>,

    /// Window configuration for spawning new windows.
    window_config: WindowConfig,
}

/// Per-window state.
struct WindowState<A: StrataApp> {
    /// The application state for this window.
    app: A::State,

    /// Current pointer capture state.
    capture: CaptureState,

    /// Current window size.
    window_size: (f32, f32),

    /// Current cursor position.
    cursor_position: Option<Point>,

    /// Frame counter (forces shader redraw when changed).
    frame: u64,

    /// Shared image store for dynamic image loading.
    image_store: crate::gpu::ImageStore,

    /// Cached layout snapshot from the most recent view() call.
    cached_snapshot: RefCell<Option<Arc<LayoutSnapshot>>>,
}

/// Saved window settings for spawning new windows.
struct WindowConfig {
    size: (f32, f32),
}

/// Messages handled by the shell.
enum ShellMessage<M> {
    /// Message from a specific window's application.
    App(iced::window::Id, M),

    /// Event from iced (mouse, keyboard, window).
    Event(Event, iced::window::Id),

    /// Frame tick for animation/rendering.
    Tick,

    /// Request to open a new window (from app via is_new_window_request).
    OpenNewWindow,

    /// Request to exit the entire application (from app via is_exit_request).
    ExitApp,

    /// A window was closed by the OS.
    WindowClosed(iced::window::Id),

    /// macOS platform requests a new window (dock icon click or menu Cmd+N).
    #[cfg(target_os = "macos")]
    PlatformNewWindow,

    /// One-shot: set up native platform integration after the event loop starts.
    #[cfg(target_os = "macos")]
    SetupNative,
}

impl<M: std::fmt::Debug> std::fmt::Debug for ShellMessage<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellMessage::App(wid, msg) => f.debug_tuple("App").field(wid).field(msg).finish(),
            ShellMessage::Event(_, window_id) => {
                f.debug_tuple("Event").field(&"...").field(window_id).finish()
            }
            ShellMessage::Tick => write!(f, "Tick"),
            ShellMessage::OpenNewWindow => write!(f, "OpenNewWindow"),
            ShellMessage::ExitApp => write!(f, "ExitApp"),
            ShellMessage::WindowClosed(wid) => f.debug_tuple("WindowClosed").field(wid).finish(),
            #[cfg(target_os = "macos")]
            ShellMessage::PlatformNewWindow => write!(f, "PlatformNewWindow"),
            #[cfg(target_os = "macos")]
            ShellMessage::SetupNative => write!(f, "SetupNative"),
        }
    }
}

// ============================================================================
// Init, Update, View, Subscription
// ============================================================================

/// Initialize the multi-window state with the first window.
fn init<A: StrataApp>(config: AppConfig) -> (MultiWindowState<A>, Task<ShellMessage<A::Message>>) {
    let shared = A::SharedState::default();
    let mut image_store = crate::gpu::ImageStore::new();
    let (app_state, cmd) = A::init(&shared, &mut image_store);

    // Open the first window explicitly (daemon doesn't auto-create one)
    let window_settings = iced::window::Settings {
        size: iced::Size::new(config.window_size.0, config.window_size.1),
        ..Default::default()
    };
    let (window_id, open_task) = iced::window::open(window_settings);

    let window_state = WindowState {
        app: app_state,
        capture: CaptureState::None,
        window_size: (config.window_size.0, config.window_size.1),
        cursor_position: None,
        frame: 0,
        image_store,
        cached_snapshot: RefCell::new(None),
    };

    let mut windows = HashMap::new();
    windows.insert(window_id, window_state);

    let state = MultiWindowState {
        shared,
        windows,
        window_config: WindowConfig {
            size: (config.window_size.0, config.window_size.1),
        },
    };

    // Install macOS reopen handler now (needs channel before event loop).
    // Menu bar setup is deferred to SetupNative (needs winit's menu to exist first).
    #[cfg(target_os = "macos")]
    crate::platform::macos::install_reopen_handler();

    let app_task = command_to_task(cmd, window_id);
    let tick_task = Task::done(ShellMessage::Tick);

    #[cfg(target_os = "macos")]
    let native_task = Task::done(ShellMessage::SetupNative);
    #[cfg(not(target_os = "macos"))]
    let native_task = Task::none();

    let task = Task::batch([open_task.discard(), app_task, tick_task, native_task]);

    (state, task)
}

/// Handle a shell message, routing to the correct window.
fn update<A: StrataApp>(
    state: &mut MultiWindowState<A>,
    message: ShellMessage<A::Message>,
) -> Task<ShellMessage<A::Message>> {
    match message {
        ShellMessage::App(wid, msg) => {
            // Check for window management requests before dispatching
            if A::is_new_window_request(&msg) {
                return Task::done(ShellMessage::OpenNewWindow);
            }
            if A::is_exit_request(&msg) {
                return Task::done(ShellMessage::ExitApp);
            }

            if let Some(window) = state.windows.get_mut(&wid) {
                let cmd = A::update(&mut window.app, msg, &mut window.image_store);
                window.frame = window.frame.wrapping_add(1);
                let task = command_to_task(cmd, wid);

                // Check if this window wants to close
                if A::should_exit(&window.app) {
                    return Task::batch([task, close_window::<A>(state, wid)]);
                }
                task
            } else {
                Task::none()
            }
        }

        ShellMessage::Event(event, wid) => {
            let Some(window) = state.windows.get_mut(&wid) else {
                return Task::none();
            };

            window.frame = window.frame.wrapping_add(1);

            // Handle window events
            if let Event::Window(ref win_event) = event {
                match win_event {
                    iced::window::Event::Resized(size) => {
                        window.window_size = (size.width, size.height);
                    }
                    iced::window::Event::CloseRequested => {
                        return close_window::<A>(state, wid);
                    }
                    _ => {}
                }
            }

            // Handle mouse events
            if let Event::Mouse(mouse_event) = &event {
                if let iced::mouse::Event::CursorMoved { position } = mouse_event {
                    window.cursor_position = Some(Point::new(position.x, position.y));
                }

                if let Some(strata_event) = convert_mouse_event(mouse_event, window.cursor_position)
                {
                    let hit: Option<HitResult> = {
                        let cache = window.cached_snapshot.borrow();
                        match cache.as_ref() {
                            Some(snapshot) => {
                                let raw_hit = window.cursor_position
                                    .and_then(|pos| snapshot.hit_test(pos));

                                if window.capture.is_captured()
                                    && !matches!(&raw_hit, Some(HitResult::Content(_)))
                                {
                                    window.cursor_position
                                        .and_then(|pos| snapshot.nearest_content(pos.x, pos.y))
                                        .or(raw_hit)
                                } else {
                                    raw_hit
                                }
                            }
                            None => {
                                drop(cache);
                                let mut snapshot = LayoutSnapshot::new();
                                snapshot.set_viewport(Rect::new(
                                    0.0, 0.0,
                                    window.window_size.0, window.window_size.1,
                                ));
                                A::view(&window.app, &mut snapshot);
                                let hit = window.cursor_position
                                    .and_then(|pos| snapshot.hit_test(pos));
                                *window.cached_snapshot.borrow_mut() = Some(Arc::new(snapshot));
                                hit
                            }
                        }
                    };

                    let is_cursor_moved = matches!(strata_event, MouseEvent::CursorMoved { .. });
                    let should_dispatch = hit.is_some() || window.capture.is_captured() || is_cursor_moved;

                    if should_dispatch {
                        let response = A::on_mouse(&window.app, strata_event, hit, &window.capture);

                        use crate::app::CaptureRequest;
                        match response.capture {
                            CaptureRequest::Capture(source) => {
                                window.capture = CaptureState::Captured(source);
                            }
                            CaptureRequest::Release => {
                                window.capture = CaptureState::None;
                            }
                            CaptureRequest::None => {}
                        }

                        if let Some(msg) = response.message {
                            let cmd = A::update(&mut window.app, msg, &mut window.image_store);
                            return command_to_task(cmd, wid);
                        }
                    }
                }
            }

            // Handle keyboard events
            if let Event::Keyboard(keyboard_event) = &event {
                match keyboard_event {
                    iced::keyboard::Event::KeyPressed { key, modifiers, text, .. } => {
                        let strata_event = KeyEvent::Pressed {
                            key: convert_key(key),
                            modifiers: convert_modifiers(*modifiers),
                            text: text.as_ref().map(|s| s.to_string()),
                        };
                        if let Some(msg) = A::on_key(&window.app, strata_event) {
                            // Check for window management before dispatching
                            if A::is_new_window_request(&msg) {
                                return Task::done(ShellMessage::OpenNewWindow);
                            }
                            if A::is_exit_request(&msg) {
                                return Task::done(ShellMessage::ExitApp);
                            }
                            let cmd = A::update(&mut window.app, msg, &mut window.image_store);
                            let task = command_to_task(cmd, wid);
                            if A::should_exit(&window.app) {
                                return Task::batch([task, close_window::<A>(state, wid)]);
                            }
                            return task;
                        }
                    }
                    iced::keyboard::Event::KeyReleased { key, modifiers, .. } => {
                        let strata_event = KeyEvent::Released {
                            key: convert_key(key),
                            modifiers: convert_modifiers(*modifiers),
                        };
                        if let Some(msg) = A::on_key(&window.app, strata_event) {
                            let cmd = A::update(&mut window.app, msg, &mut window.image_store);
                            let task = command_to_task(cmd, wid);
                            if A::should_exit(&window.app) {
                                return Task::batch([task, close_window::<A>(state, wid)]);
                            }
                            return task;
                        }
                    }
                    _ => {}
                }
            }

            // Handle file drop events
            if let Event::Window(ref win_event) = event {
                use crate::event_context::FileDropEvent;
                let file_event = match win_event {
                    iced::window::Event::FileHovered(path) => Some(FileDropEvent::Hovered(path.clone())),
                    iced::window::Event::FileDropped(path) => Some(FileDropEvent::Dropped(path.clone())),
                    iced::window::Event::FilesHoveredLeft => Some(FileDropEvent::HoverLeft),
                    _ => None,
                };
                if let Some(fe) = file_event {
                    let hit = {
                        let cache = window.cached_snapshot.borrow();
                        cache.as_ref().and_then(|snapshot| {
                            window.cursor_position.and_then(|pos| snapshot.hit_test(pos))
                        })
                    };
                    if let Some(msg) = A::on_file_drop(&window.app, fe, hit) {
                        let cmd = A::update(&mut window.app, msg, &mut window.image_store);
                        return command_to_task(cmd, wid);
                    }
                }
            }

            Task::none()
        }

        ShellMessage::Tick => {
            for window in state.windows.values_mut() {
                window.frame = window.frame.wrapping_add(1);
            }
            Task::none()
        }

        ShellMessage::OpenNewWindow => {
            let mut image_store = crate::gpu::ImageStore::new();
            if let Some((app_state, cmd)) = A::create_window(&state.shared, &mut image_store) {
                let window_settings = iced::window::Settings {
                    size: iced::Size::new(state.window_config.size.0, state.window_config.size.1),
                    ..Default::default()
                };
                let (new_id, open_task) = iced::window::open(window_settings);

                state.windows.insert(new_id, WindowState {
                    app: app_state,
                    capture: CaptureState::None,
                    window_size: state.window_config.size,
                    cursor_position: None,
                    frame: 0,
                    image_store,
                    cached_snapshot: RefCell::new(None),
                });

                Task::batch([open_task.discard(), command_to_task(cmd, new_id)])
            } else {
                Task::none()
            }
        }

        ShellMessage::ExitApp => {
            iced::exit()
        }

        #[cfg(target_os = "macos")]
        ShellMessage::SetupNative => {
            crate::platform::macos::setup_menu_bar();
            Task::none()
        }

        #[cfg(target_os = "macos")]
        ShellMessage::PlatformNewWindow => {
            Task::done(ShellMessage::OpenNewWindow)
        }

        ShellMessage::WindowClosed(wid) => {
            state.windows.remove(&wid);
            // Daemon stays alive when all windows close — dock icon or
            // Cmd+N (via macOS app menu) can reopen a window.
            Task::none()
        }
    }
}

/// Close a specific window.
fn close_window<A: StrataApp>(
    state: &mut MultiWindowState<A>,
    wid: iced::window::Id,
) -> Task<ShellMessage<A::Message>> {
    state.windows.remove(&wid);
    iced::window::close(wid)
}

/// Get the title for a specific window.
fn title<A: StrataApp>(state: &MultiWindowState<A>, window_id: iced::window::Id) -> String {
    state.windows.get(&window_id)
        .map(|w| A::title(&w.app))
        .unwrap_or_else(|| String::from("Strata App"))
}

/// Build the view for a specific window.
fn view<A: StrataApp>(state: &MultiWindowState<A>, window_id: iced::window::Id) -> Element<'_, ShellMessage<A::Message>> {
    let Some(window) = state.windows.get(&window_id) else {
        return iced::widget::text("").into();
    };

    let mut snapshot = LayoutSnapshot::new();
    snapshot.set_viewport(Rect::new(0.0, 0.0, window.window_size.0, window.window_size.1));

    A::view(&window.app, &mut snapshot);

    let snapshot = Arc::new(snapshot);
    *window.cached_snapshot.borrow_mut() = Some(snapshot.clone());

    let pending = window.image_store.drain_pending();
    let pending_images = Arc::new(Mutex::new(pending));
    let pending_unloads = window.image_store.drain_pending_unloads();
    let pending_image_unloads = Arc::new(Mutex::new(pending_unloads));

    let program = StrataShaderProgram {
        snapshot,
        selection: A::selection(&window.app).cloned(),
        background: A::background_color(&window.app),
        frame: window.frame,
        pending_images,
        pending_image_unloads,
        is_selecting: window.capture.is_captured(),
    };

    shader::Shader::new(program)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Create subscriptions for all windows.
fn subscription<A: StrataApp>(state: &MultiWindowState<A>) -> Subscription<ShellMessage<A::Message>> {
    // Global event listener (already provides window_id per event)
    let events = iced::event::listen_with(|event, _status, window_id| {
        Some(ShellMessage::Event(event, window_id))
    });

    // Global animation tick
    let tick = iced::window::frames()
        .map(|_| ShellMessage::Tick);

    // Window close events (for cleanup after OS closes a window)
    let close_events = iced::window::close_events()
        .map(ShellMessage::WindowClosed);

    let mut all_subs = vec![events, tick, close_events];

    // macOS platform → iced bridge: channel-based subscription (no polling).
    // Handles dock-click reopen and menu bar Cmd+N (newDocument:).
    // A dedicated thread blocks on std::sync::mpsc::recv until the ObjC
    // handler fires, then forwards to the async iced stream.
    #[cfg(target_os = "macos")]
    {
        let reopen_stream = iced::stream::channel(1, |mut output| async move {
            let Some(rx) = crate::platform::macos::take_reopen_receiver() else {
                // Already taken by a previous subscription — idle forever.
                std::future::pending::<()>().await;
                return;
            };

            // Bridge: one thread that sleeps until the Apple Event fires,
            // then forwards to an async-compatible unbounded channel.
            let (atx, mut arx) = iced::futures::channel::mpsc::unbounded::<()>();
            std::thread::Builder::new()
                .name("dock-reopen".into())
                .spawn(move || {
                    while rx.recv().is_ok() {
                        if atx.unbounded_send(()).is_err() {
                            break;
                        }
                    }
                })
                .ok();

            use iced::futures::{SinkExt, StreamExt};
            while arx.next().await.is_some() {
                let _ = output.send(ShellMessage::PlatformNewWindow).await;
            }
        });
        all_subs.push(Subscription::run_with_id("dock-reopen", reopen_stream));
    }

    // Per-window app subscriptions, tagged with window ID.
    // Use `with(wid)` to attach the window ID as data, then a non-capturing
    // map to restructure — iced panics if map closures capture variables.
    for (&wid, window) in &state.windows {
        let app_sub = A::subscription(&window.app);
        for s in app_sub.subs {
            all_subs.push(
                s.with(wid)
                    .map(|(wid, m)| ShellMessage::App(wid, m))
            );
        }
    }

    Subscription::batch(all_subs)
}

/// Convert a Strata Command to an iced Task, tagged with a window ID.
fn command_to_task<M: Send + 'static>(mut cmd: Command<M>, wid: iced::window::Id) -> Task<ShellMessage<M>> {
    let futures = cmd.take_futures();

    if futures.is_empty() {
        return Task::none();
    }

    let tasks: Vec<Task<ShellMessage<M>>> = futures
        .into_iter()
        .map(|fut| Task::future(async move { ShellMessage::App(wid, fut.await) }))
        .collect();

    Task::batch(tasks)
}

/// Convert an iced mouse event to a Strata MouseEvent.
fn convert_mouse_event(
    event: &iced::mouse::Event,
    cursor_position: Option<Point>,
) -> Option<MouseEvent> {
    let pos = cursor_position.unwrap_or(Point::ORIGIN);

    match event {
        iced::mouse::Event::ButtonPressed(button) => Some(MouseEvent::ButtonPressed {
            button: convert_mouse_button(*button),
            position: pos,
        }),
        iced::mouse::Event::ButtonReleased(button) => Some(MouseEvent::ButtonReleased {
            button: convert_mouse_button(*button),
            position: pos,
        }),
        iced::mouse::Event::CursorMoved { position } => Some(MouseEvent::CursorMoved {
            position: Point::new(position.x, position.y),
        }),
        iced::mouse::Event::CursorEntered => Some(MouseEvent::CursorEntered),
        iced::mouse::Event::CursorLeft => Some(MouseEvent::CursorLeft),
        iced::mouse::Event::WheelScrolled { delta } => Some(MouseEvent::WheelScrolled {
            delta: match delta {
                iced::mouse::ScrollDelta::Lines { x, y } => ScrollDelta::Lines { x: *x, y: *y },
                iced::mouse::ScrollDelta::Pixels { x, y } => ScrollDelta::Pixels { x: *x, y: *y },
            },
            position: pos,
        }),
    }
}

fn convert_mouse_button(button: iced::mouse::Button) -> MouseButton {
    match button {
        iced::mouse::Button::Left => MouseButton::Left,
        iced::mouse::Button::Right => MouseButton::Right,
        iced::mouse::Button::Middle => MouseButton::Middle,
        iced::mouse::Button::Back => MouseButton::Back,
        iced::mouse::Button::Forward => MouseButton::Forward,
        iced::mouse::Button::Other(n) => MouseButton::Other(n),
    }
}

fn convert_key(key: &iced::keyboard::Key) -> crate::event_context::Key {
    use crate::event_context::Key;

    match key {
        iced::keyboard::Key::Named(named) => {
            let named_key = match named {
                iced::keyboard::key::Named::ArrowUp => NamedKey::ArrowUp,
                iced::keyboard::key::Named::ArrowDown => NamedKey::ArrowDown,
                iced::keyboard::key::Named::ArrowLeft => NamedKey::ArrowLeft,
                iced::keyboard::key::Named::ArrowRight => NamedKey::ArrowRight,
                iced::keyboard::key::Named::Home => NamedKey::Home,
                iced::keyboard::key::Named::End => NamedKey::End,
                iced::keyboard::key::Named::PageUp => NamedKey::PageUp,
                iced::keyboard::key::Named::PageDown => NamedKey::PageDown,
                iced::keyboard::key::Named::Backspace => NamedKey::Backspace,
                iced::keyboard::key::Named::Delete => NamedKey::Delete,
                iced::keyboard::key::Named::Insert => NamedKey::Insert,
                iced::keyboard::key::Named::Enter => NamedKey::Enter,
                iced::keyboard::key::Named::Tab => NamedKey::Tab,
                iced::keyboard::key::Named::Escape => NamedKey::Escape,
                iced::keyboard::key::Named::Space => NamedKey::Space,
                iced::keyboard::key::Named::F1 => NamedKey::F1,
                iced::keyboard::key::Named::F2 => NamedKey::F2,
                iced::keyboard::key::Named::F3 => NamedKey::F3,
                iced::keyboard::key::Named::F4 => NamedKey::F4,
                iced::keyboard::key::Named::F5 => NamedKey::F5,
                iced::keyboard::key::Named::F6 => NamedKey::F6,
                iced::keyboard::key::Named::F7 => NamedKey::F7,
                iced::keyboard::key::Named::F8 => NamedKey::F8,
                iced::keyboard::key::Named::F9 => NamedKey::F9,
                iced::keyboard::key::Named::F10 => NamedKey::F10,
                iced::keyboard::key::Named::F11 => NamedKey::F11,
                iced::keyboard::key::Named::F12 => NamedKey::F12,
                _ => NamedKey::Unknown,
            };
            Key::Named(named_key)
        }
        iced::keyboard::Key::Character(c) => Key::Character(c.to_string()),
        iced::keyboard::Key::Unidentified => Key::Named(NamedKey::Unknown),
    }
}

fn convert_modifiers(mods: iced::keyboard::Modifiers) -> Modifiers {
    Modifiers {
        shift: mods.shift(),
        ctrl: mods.control(),
        alt: mods.alt(),
        meta: mods.logo(),
    }
}

// ============================================================================
// Shader Program for GPU Rendering
// ============================================================================

/// Shader program that renders Strata content.
#[derive(Clone)]
struct StrataShaderProgram {
    /// Layout snapshot wrapped in Arc to avoid deep copying when iced clones.
    snapshot: Arc<LayoutSnapshot>,
    selection: Option<Selection>,
    background: crate::primitives::Color,
    /// Frame counter - changing this triggers iced to redraw.
    frame: u64,
    /// Pending image uploads (drained by prepare on first access).
    pending_images: Arc<Mutex<Vec<PendingImage>>>,
    /// Pending image unloads (drained by prepare on first access).
    pending_image_unloads: Arc<Mutex<Vec<ImageHandle>>>,
    /// Whether a selection drag is active (locks cursor to I-beam).
    is_selecting: bool,
}

/// Primitive passed to the GPU.
#[derive(Clone)]
struct StrataPrimitive {
    /// Layout snapshot wrapped in Arc to avoid deep copying.
    snapshot: Arc<LayoutSnapshot>,
    selection: Option<Selection>,
    background: crate::primitives::Color,
    frame: u64,
    /// Pending image uploads (drained by prepare on first access).
    pending_images: Arc<Mutex<Vec<PendingImage>>>,
    /// Pending image unloads (drained by prepare on first access).
    pending_image_unloads: Arc<Mutex<Vec<ImageHandle>>>,
}

impl std::fmt::Debug for StrataPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrataPrimitive")
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

impl<Message> shader::Program<Message> for StrataShaderProgram {
    type State = ();
    type Primitive = StrataPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: iced::Rectangle,
    ) -> Self::Primitive {
        StrataPrimitive {
            snapshot: self.snapshot.clone(),
            selection: self.selection.clone(),
            background: self.background,
            frame: self.frame,
            pending_images: self.pending_images.clone(),
            pending_image_unloads: self.pending_image_unloads.clone(),
        }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        if self.is_selecting {
            return iced::mouse::Interaction::Text;
        }

        use crate::layout_snapshot::CursorIcon;

        let Some(pos) = cursor.position() else {
            return iced::mouse::Interaction::default();
        };

        match self.snapshot.cursor_at(Point::new(pos.x, pos.y)) {
            CursorIcon::Arrow => iced::mouse::Interaction::Idle,
            CursorIcon::Text => iced::mouse::Interaction::Text,
            CursorIcon::Pointer => iced::mouse::Interaction::Pointer,
            CursorIcon::Grab => iced::mouse::Interaction::Grab,
            CursorIcon::Grabbing => iced::mouse::Interaction::Grabbing,
            CursorIcon::Copy => iced::mouse::Interaction::Pointer, // Iced has no Copy cursor; use Pointer
        }
    }
}

impl shader::Primitive for StrataPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut shader::Storage,
        bounds: &iced::Rectangle,
        viewport: &iced::advanced::graphics::Viewport,
    ) {
        // Get or create the pipeline wrapper
        if !storage.has::<PipelineWrapper>() {
            let wrapper = PipelineWrapper::new(device, format);
            storage.store(wrapper);
        }

        let wrapper = storage.get_mut::<PipelineWrapper>().unwrap();

        // Drain pending image loads/unloads — pass them into prepare() so they're
        // applied after the pipeline is guaranteed to exist.
        let pending = std::mem::take(&mut *self.pending_images.lock().unwrap());
        let pending_unloads = std::mem::take(&mut *self.pending_image_unloads.lock().unwrap());

        wrapper.prepare(
            device,
            queue,
            &*self.snapshot, // Deref Arc to get &LayoutSnapshot
            self.selection.as_ref(),
            bounds,
            viewport,
            self.background,
            pending,
            pending_unloads,
        );
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &shader::Storage,
        target: &wgpu::TextureView,
        clip_bounds: &iced::Rectangle<u32>,
    ) {
        let Some(wrapper) = storage.get::<PipelineWrapper>() else {
            return;
        };

        wrapper.render(encoder, target, clip_bounds);
    }
}

// ============================================================================
// Pipeline Wrapper (Bridges iced shader::Primitive to StrataPipeline)
// ============================================================================

/// Base font size in logical points.
const BASE_FONT_SIZE: f32 = 14.0;

/// Wrapper around StrataPipeline for use with iced's shader storage.
struct PipelineWrapper {
    pipeline: Option<StrataPipeline>,
    format: wgpu::TextureFormat,
    current_scale: f32,
}

impl PipelineWrapper {
    fn new(_device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Don't create pipeline yet - we need scale factor first
        Self {
            pipeline: None,
            format,
            current_scale: 0.0,
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        snapshot: &LayoutSnapshot,
        selection: Option<&Selection>,
        bounds: &iced::Rectangle,
        viewport: &iced::advanced::graphics::Viewport,
        background: crate::primitives::Color,
        pending_images: Vec<PendingImage>,
        pending_unloads: Vec<ImageHandle>,
    ) {
        let scale = viewport.scale_factor() as f32;

        // Lock FontSystem once for the entire frame
        let fs_mutex = crate::text_engine::get_font_system();
        let mut font_system = fs_mutex.lock().unwrap();

        // Create or recreate pipeline if scale factor changed
        if self.pipeline.is_none() || (self.current_scale - scale).abs() > 0.01 {
            let font_size = BASE_FONT_SIZE * scale;
            self.pipeline = Some(StrataPipeline::new(device, queue, self.format, font_size, &mut font_system));
            self.current_scale = scale;
        }

        // Upload pending images now that the pipeline is guaranteed to exist.
        if !pending_images.is_empty() {
            self.upload_pending_images(device, queue, pending_images);
        }

        // Apply pending image unloads.
        if !pending_unloads.is_empty() {
            if let Some(pipeline) = self.pipeline.as_mut() {
                for handle in pending_unloads {
                    pipeline.unload_image(handle);
                }
            }
        }

        let pipeline = self.pipeline.as_mut().unwrap();

        // Reclaim staging belt memory from previous frame.
        // This should be called after GPU work completes, but calling at the
        // start of the next frame is safe (the staging buffers are no longer
        // referenced by the previous frame's command buffer at this point).
        pipeline.after_frame();

        // Clear previous frame data
        pipeline.clear();
        pipeline.set_background(background);


        // =====================================================================
        // RENDER ORDER (back to front via ubershader instance order):
        //   1. Background decorations
        //   2. Primitive backgrounds (solid rects, rounded rects, circles)
        //   3. Selection highlight
        //   4. Grid content (terminal rows)
        //   5. Primitive text runs
        //   6. Foreground decorations
        // =====================================================================

        let primitives = snapshot.primitives();

        /// Convert an optional clip rect to GPU format [x, y, w, h] with scaling.
        #[inline]
        fn clip_to_gpu(clip: &Option<crate::primitives::Rect>, scale: f32) -> Option<[f32; 4]> {
            clip.map(|c| [c.x * scale, c.y * scale, c.width * scale, c.height * scale])
        }

        /// Apply clip from a primitive to the pipeline instances added since `start`.
        #[inline]
        fn maybe_clip(
            pipeline: &mut crate::gpu::StrataPipeline,
            start: usize,
            clip: &Option<crate::primitives::Rect>,
            scale: f32,
        ) {
            if let Some(gpu_clip) = clip_to_gpu(clip, scale) {
                pipeline.apply_clip_since(start, gpu_clip);
            }
        }

        /// Compute a content signature for a grid row (for row-dirty tracking).
        ///
        /// Hashes all run data that affects rendering: text, colors, position,
        /// width, and style flags. Hash collisions are harmless (just a missed
        /// cache opportunity — row is rebuilt unnecessarily, never rendered wrong).
        #[inline]
        fn hash_grid_row(row: &crate::layout_snapshot::GridRow) -> u64 {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for run in &row.runs {
                run.text.hash(&mut hasher);
                run.fg.hash(&mut hasher);
                run.bg.hash(&mut hasher);
                run.col_offset.hash(&mut hasher);
                run.cell_len.hash(&mut hasher);
                // Pack style flags into a u16 for hashing
                use crate::layout_snapshot::UnderlineStyle;
                let ul_bits: u8 = match run.style.underline {
                    UnderlineStyle::None => 0,
                    UnderlineStyle::Single => 1,
                    UnderlineStyle::Double => 2,
                    UnderlineStyle::Curly => 3,
                    UnderlineStyle::Dotted => 4,
                    UnderlineStyle::Dashed => 5,
                };
                let style_bits: u16 = (run.style.bold as u16)
                    | ((run.style.italic as u16) << 1)
                    | ((run.style.strikethrough as u16) << 2)
                    | ((run.style.dim as u16) << 3)
                    | ((ul_bits as u16) << 4);
                style_bits.hash(&mut hasher);
            }
            hasher.finish()
        }

        // 1. Background decorations
        for decoration in snapshot.background_decorations() {
            render_decoration(pipeline, decoration, scale);
        }

        // 2. Shadows (behind everything they shadow)
        for prim in &primitives.shadows {
            let start = pipeline.instance_count();
            pipeline.add_shadow(
                prim.rect.x * scale,
                prim.rect.y * scale,
                prim.rect.width * scale,
                prim.rect.height * scale,
                prim.corner_radius * scale,
                prim.blur_radius * scale,
                prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // 2b. Primitive backgrounds (rounded rects first, then solid rects on top)
        for prim in &primitives.rounded_rects {
            let start = pipeline.instance_count();
            pipeline.add_rounded_rect(
                prim.rect.x * scale,
                prim.rect.y * scale,
                prim.rect.width * scale,
                prim.rect.height * scale,
                prim.corner_radius * scale,
                prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        for prim in &primitives.circles {
            let start = pipeline.instance_count();
            pipeline.add_circle(
                prim.center.x * scale,
                prim.center.y * scale,
                prim.radius * scale,
                prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        // Solid rects (selection highlights, cursors) — after rounded rect backgrounds
        for prim in &primitives.solid_rects {
            let start = pipeline.instance_count();
            pipeline.add_solid_rect(
                prim.rect.x * scale,
                prim.rect.y * scale,
                prim.rect.width * scale,
                prim.rect.height * scale,
                prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // 2c. Borders (outlines)
        for prim in &primitives.borders {
            let start = pipeline.instance_count();
            pipeline.add_border(
                prim.rect.x * scale,
                prim.rect.y * scale,
                prim.rect.width * scale,
                prim.rect.height * scale,
                prim.corner_radius * scale,
                prim.border_width * scale,
                prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // 2d. Line segments
        for prim in &primitives.lines {
            let start = pipeline.instance_count();
            pipeline.add_line_styled(
                prim.p1.x * scale,
                prim.p1.y * scale,
                prim.p2.x * scale,
                prim.p2.y * scale,
                prim.thickness * scale,
                prim.color,
                convert_line_style(prim.style),
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        // 2e. Polylines (each expands to N-1 line segment instances)
        for prim in &primitives.polylines {
            let start = pipeline.instance_count();
            let scaled_points: Vec<[f32; 2]> = prim
                .points
                .iter()
                .map(|p| [p.x * scale, p.y * scale])
                .collect();
            pipeline.add_polyline_styled(
                &scaled_points,
                prim.thickness * scale,
                prim.color,
                convert_line_style(prim.style),
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // 2f. Images
        for prim in &primitives.images {
            let start = pipeline.instance_count();
            pipeline.add_image(
                prim.rect.x * scale,
                prim.rect.y * scale,
                prim.rect.width * scale,
                prim.rect.height * scale,
                prim.handle,
                prim.corner_radius * scale,
                prim.tint,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // 3. Selection highlight (on top of backgrounds, behind text)
        if let Some(sel) = selection {
            if !sel.is_collapsed() {
                let selection_rects = snapshot.selection_bounds(sel);
                let scaled_rects: Vec<_> = selection_rects
                    .iter()
                    .map(|r| crate::primitives::Rect {
                        x: r.x * scale,
                        y: r.y * scale,
                        width: r.width * scale,
                        height: r.height * scale,
                    })
                    .collect();
                pipeline.add_solid_rects(&scaled_rects, crate::gpu::SELECTION_COLOR);
            }
        }

        // 4. Grid content from sources (terminals use this path)
        //
        // Row-dirty tracking: only rebuild instances for rows whose content
        // changed. Cached rows are stored with relative Y (0.0) and gathered
        // with the correct absolute offset each frame. This makes scrolling
        // free (just changes the Y offset, not the cached instances).
        for (_source_id, source_layout) in snapshot.sources_in_order() {
            for item in &source_layout.items {
                if let crate::layout_snapshot::ItemLayout::Grid(grid_layout) = item {
                    let grid_clip = &grid_layout.clip_rect;
                    let cell_w = grid_layout.cell_width * scale;
                    let cell_h = grid_layout.cell_height * scale;

                    // Ensure row cache matches current grid dimensions
                    pipeline.ensure_grid_cache(
                        grid_layout.cols,
                        grid_layout.rows_content.len(),
                        grid_layout.bounds.x,
                    );

                    for (row_idx, row) in grid_layout.rows_content.iter().enumerate() {
                        if row.runs.is_empty() {
                            continue;
                        }

                        let signature = hash_grid_row(row);

                        // Check cache — None = hit (skip), Some(start) = miss (build)
                        let Some(build_start) = pipeline.begin_grid_row(row_idx, signature) else {
                            continue;
                        };

                        // Cache miss: build instances with absolute row_y (as before).
                        // end_grid_row will subtract row_y to store relative coordinates.
                        let row_y = (grid_layout.bounds.y + row_idx as f32 * grid_layout.cell_height) * scale;
                        let base_x = grid_layout.bounds.x * scale;

                        for run in &row.runs {
                            let run_x = base_x + run.col_offset as f32 * cell_w;
                            let run_w = run.cell_len as f32 * cell_w;
                            let is_whitespace = run.text.trim().is_empty();

                            // Background color rect
                            if run.bg != 0 {
                                let bg_color = crate::primitives::Color::unpack(run.bg);
                                pipeline.add_solid_rect(run_x, row_y, run_w, cell_h, bg_color);
                            }

                            // Foreground color (used for text and decorations)
                            let mut fg_color = crate::primitives::Color::unpack(run.fg);
                            if run.style.dim {
                                fg_color.a *= 0.5;
                            }

                            // Text shaping (skip for whitespace-only runs)
                            if !is_whitespace {
                                // Check if run contains custom-drawn characters (box drawing / block elements)
                                let has_custom = run.text.chars().any(crate::gpu::is_custom_drawn);
                                if has_custom {
                                    // Mixed or pure custom run: iterate per-cell.
                                    use unicode_width::UnicodeWidthChar;
                                    let mut col = 0usize;
                                    let mut text_buf = String::new();
                                    let mut text_col_start = 0usize;
                                    for ch in run.text.chars() {
                                        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                                        if ch_width == 0 {
                                            text_buf.push(ch);
                                            continue;
                                        }
                                        if crate::gpu::is_custom_drawn(ch) {
                                            if !text_buf.is_empty() {
                                                let tx = run_x + text_col_start as f32 * cell_w;
                                                pipeline.add_text_grid(&text_buf, tx, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, &mut font_system);
                                                text_buf.clear();
                                            }
                                            let cx = run_x + col as f32 * cell_w;
                                            if !pipeline.draw_box_char(ch, cx, row_y, cell_w, cell_h, fg_color)
                                                && !pipeline.draw_block_char(ch, cx, row_y, cell_w, cell_h, fg_color)
                                            {
                                                if text_buf.is_empty() {
                                                    text_col_start = col;
                                                }
                                                text_buf.push(ch);
                                            }
                                            col += 1;
                                        } else {
                                            if text_buf.is_empty() {
                                                text_col_start = col;
                                            }
                                            text_buf.push(ch);
                                            col += ch_width;
                                        }
                                    }
                                    if !text_buf.is_empty() {
                                        let tx = run_x + text_col_start as f32 * cell_w;
                                        pipeline.add_text_grid(&text_buf, tx, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, &mut font_system);
                                    }
                                } else {
                                    pipeline.add_text_grid(&run.text, run_x, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, &mut font_system);
                                }
                            }

                            // Underline variants (render on whitespace too)
                            {
                                use crate::layout_snapshot::UnderlineStyle;
                                let ul_thickness = scale.max(1.0);
                                match run.style.underline {
                                    UnderlineStyle::None => {}
                                    UnderlineStyle::Single | UnderlineStyle::Curly | UnderlineStyle::Dotted | UnderlineStyle::Dashed => {
                                        let ul_y = row_y + cell_h * 0.85;
                                        pipeline.add_solid_rect(run_x, ul_y, run_w, ul_thickness, fg_color);
                                    }
                                    UnderlineStyle::Double => {
                                        let gap = (2.0 * scale).max(2.0);
                                        let ul_y1 = row_y + cell_h * 0.82;
                                        let ul_y2 = ul_y1 + gap;
                                        pipeline.add_solid_rect(run_x, ul_y1, run_w, ul_thickness, fg_color);
                                        pipeline.add_solid_rect(run_x, ul_y2, run_w, ul_thickness, fg_color);
                                    }
                                }
                            }

                            // Strikethrough (render on whitespace too)
                            if run.style.strikethrough {
                                let st_y = row_y + cell_h * 0.5;
                                pipeline.add_solid_rect(run_x, st_y, run_w, 1.0 * scale, fg_color);
                            }
                        }

                        // Store built instances in cache (subtracts row_y for relative coords)
                        pipeline.end_grid_row(row_idx, signature, build_start, row_y);
                    }

                    // Gather all cached rows with absolute Y offsets and clip rect
                    let grid_base_y = grid_layout.bounds.y * scale;
                    let grid_clip_gpu = clip_to_gpu(grid_clip, scale);
                    pipeline.gather_grid_rows(
                        grid_base_y,
                        cell_h,
                        grid_layout.rows_content.len(),
                        grid_clip_gpu,
                    );
                }
            }
        }
        // 5. Primitive text runs (with viewport culling)
        let viewport_bottom = bounds.height;
        for prim in &primitives.text_runs {
            // Cull text runs entirely outside the viewport (always check position)
            if prim.position.y > viewport_bottom || prim.position.y + prim.font_size * 1.5 < 0.0 {
                continue;
            }
            // Also cull if clip rect is entirely offscreen
            if let Some(clip) = &prim.clip_rect {
                if clip.y > viewport_bottom || (clip.y + clip.height) < 0.0 {
                    continue;
                }
            }
            let start = pipeline.instance_count();
            pipeline.add_text(
                &prim.text,
                prim.position.x * scale,
                prim.position.y * scale,
                prim.color,
                prim.font_size * scale,
                &mut font_system,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // Render foreground decorations (on top of text).
        for decoration in snapshot.foreground_decorations() {
            render_decoration(pipeline, decoration, scale);
        }

        // 7. Overlay primitives — rendered LAST, on top of everything.
        // Used for context menus, tooltips, popups.
        let overlays = snapshot.overlay_primitives();

        for prim in &overlays.shadows {
            let start = pipeline.instance_count();
            pipeline.add_shadow(
                prim.rect.x * scale, prim.rect.y * scale,
                prim.rect.width * scale, prim.rect.height * scale,
                prim.corner_radius * scale, prim.blur_radius * scale, prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        for prim in &overlays.rounded_rects {
            let start = pipeline.instance_count();
            pipeline.add_rounded_rect(
                prim.rect.x * scale, prim.rect.y * scale,
                prim.rect.width * scale, prim.rect.height * scale,
                prim.corner_radius * scale, prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        for prim in &overlays.solid_rects {
            let start = pipeline.instance_count();
            pipeline.add_solid_rect(
                prim.rect.x * scale, prim.rect.y * scale,
                prim.rect.width * scale, prim.rect.height * scale, prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        for prim in &overlays.borders {
            let start = pipeline.instance_count();
            pipeline.add_border(
                prim.rect.x * scale, prim.rect.y * scale,
                prim.rect.width * scale, prim.rect.height * scale,
                prim.corner_radius * scale, prim.border_width * scale, prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }
        for prim in &overlays.text_runs {
            let start = pipeline.instance_count();
            pipeline.add_text(
                &prim.text, prim.position.x * scale, prim.position.y * scale, prim.color,
                prim.font_size * scale, &mut font_system,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // Create command encoder for staging belt uploads.
        // The StagingBelt writes directly to unified memory on Apple Silicon,
        // avoiding intermediate buffer copies.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Strata Staging Upload"),
        });

        // Prepare for GPU (upload buffers via staging belt)
        // Prepare for GPU (upload buffers via staging belt)
        pipeline.prepare(
            device,
            queue,
            &mut encoder,
            bounds.width * scale,
            bounds.height * scale,
        );

        // Submit staging commands. The staging belt's copy commands need to
        // execute before the render commands that use the buffers.
        queue.submit(std::iter::once(encoder.finish()));
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &iced::Rectangle<u32>,
    ) {
        if let Some(pipeline) = &self.pipeline {
            pipeline.render(encoder, target, clip_bounds);
        }
    }

    /// Upload pending images to the GPU atlas.
    ///
    /// Called each frame before prepare() when new images have been queued
    /// via `ImageStore::load_rgba()` or `ImageStore::load_png()`.
    fn upload_pending_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pending: Vec<PendingImage>,
    ) {
        let Some(pipeline) = self.pipeline.as_mut() else {
            return;
        };
        for img in pending {
            pipeline.load_image_rgba(device, queue, img.width, img.height, &img.data);
        }
    }
}

/// Helper to render a decoration primitive via the ubershader pipeline.
fn render_decoration(
    pipeline: &mut StrataPipeline,
    decoration: &crate::layout_snapshot::Decoration,
    scale: f32,
) {
    use crate::layout_snapshot::Decoration;

    match decoration {
        Decoration::SolidRect { rect, color } => {
            pipeline.add_solid_rect(
                rect.x * scale,
                rect.y * scale,
                rect.width * scale,
                rect.height * scale,
                *color,
            );
        }
        Decoration::RoundedRect {
            rect,
            corner_radius,
            color,
        } => {
            pipeline.add_rounded_rect(
                rect.x * scale,
                rect.y * scale,
                rect.width * scale,
                rect.height * scale,
                corner_radius * scale,
                *color,
            );
        }
        Decoration::Circle {
            center,
            radius,
            color,
        } => {
            pipeline.add_circle(center.x * scale, center.y * scale, radius * scale, *color);
        }
    }
}

/// Convert layout LineStyle to GPU LineStyle.
fn convert_line_style(
    style: crate::layout::primitives::LineStyle,
) -> crate::gpu::LineStyle {
    match style {
        crate::layout::primitives::LineStyle::Solid => crate::gpu::LineStyle::Solid,
        crate::layout::primitives::LineStyle::Dashed => {
            crate::gpu::LineStyle::Dashed
        }
        crate::layout::primitives::LineStyle::Dotted => {
            crate::gpu::LineStyle::Dotted
        }
    }
}
