//! Iced Shell Adapter
//!
//! This module bridges Strata applications to iced for window management,
//! event handling, and GPU rendering.
//!
//! **This is the ONLY file in Strata that imports iced.**

use iced::widget::shader::{self, wgpu};
use iced::{Element, Event, Length, Subscription, Task, Theme};

use crate::strata::app::{AppConfig, Command, StrataApp};
use crate::strata::content_address::Selection;
use crate::strata::event_context::{
    CaptureState, EventContext, KeyEvent, Modifiers, MouseButton, MouseEvent, NamedKey,
    ScrollDelta,
};
use crate::strata::layout_snapshot::LayoutSnapshot;
use crate::strata::primitives::{Point, Rect};

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
pub fn run_with_config<A: StrataApp>(config: AppConfig) -> Result<(), Error> {
    iced::application(
        |state: &ShellState<A>| A::title(&state.app),
        update::<A>,
        view::<A>,
    )
    .subscription(subscription::<A>)
    .theme(|_| Theme::Dark)
    .window_size(iced::Size::new(config.window_size.0, config.window_size.1))
    .antialiasing(config.antialiasing)
    .run_with(init::<A>)
    .map_err(Error::from)
}

/// Internal shell state wrapping the application state.
struct ShellState<A: StrataApp> {
    /// The application state.
    app: A::State,

    /// Current layout snapshot (rebuilt each frame).
    snapshot: LayoutSnapshot,

    /// Current pointer capture state.
    capture: CaptureState,

    /// Current window size.
    window_size: (f32, f32),

    /// Whether we need to rebuild the snapshot.
    dirty: bool,
}

/// Messages handled by the shell.
enum ShellMessage<M> {
    /// Message from the application.
    App(M),

    /// Event from iced (mouse, keyboard, window).
    Event(Event, iced::window::Id),

    /// Frame tick for animation/rendering.
    Tick,
}

impl<M: std::fmt::Debug> std::fmt::Debug for ShellMessage<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellMessage::App(msg) => f.debug_tuple("App").field(msg).finish(),
            ShellMessage::Event(_, window_id) => {
                f.debug_tuple("Event").field(&"...").field(window_id).finish()
            }
            ShellMessage::Tick => write!(f, "Tick"),
        }
    }
}

/// Initialize the shell state.
fn init<A: StrataApp>() -> (ShellState<A>, Task<ShellMessage<A::Message>>) {
    let (app_state, cmd) = A::init();

    let shell_state = ShellState {
        app: app_state,
        snapshot: LayoutSnapshot::new(),
        capture: CaptureState::None,
        window_size: (1200.0, 800.0),
        dirty: true,
    };

    // Convert app command to shell tasks
    let task = command_to_task(cmd);

    (shell_state, task)
}

/// Handle a shell message.
fn update<A: StrataApp>(
    state: &mut ShellState<A>,
    message: ShellMessage<A::Message>,
) -> Task<ShellMessage<A::Message>> {
    match message {
        ShellMessage::App(msg) => {
            let cmd = A::update(&mut state.app, msg);
            state.dirty = true;
            command_to_task(cmd)
        }

        ShellMessage::Event(event, _window_id) => {
            // Convert iced event to Strata event and dispatch
            if let Some(strata_event) = convert_event(&event) {
                // Rebuild snapshot if dirty
                if state.dirty {
                    state.snapshot.clear();
                    state.snapshot.set_viewport(Rect::new(
                        0.0,
                        0.0,
                        state.window_size.0,
                        state.window_size.1,
                    ));
                    A::view(&state.app, &mut state.snapshot);
                    state.dirty = false;
                }

                // Create event context with current capture state
                let ctx = EventContext::with_capture(&state.snapshot, state.capture);

                // TODO: Dispatch event to widgets and collect messages
                // For now, just handle capture state changes
                state.capture = ctx.take_capture();
            }

            // Handle window resize
            if let Event::Window(iced::window::Event::Resized(size)) = event {
                state.window_size = (size.width, size.height);
                state.dirty = true;
            }

            Task::none()
        }

        ShellMessage::Tick => {
            // Mark dirty to trigger view rebuild
            state.dirty = true;
            Task::none()
        }
    }
}

/// Build the view.
fn view<A: StrataApp>(state: &ShellState<A>) -> Element<'_, ShellMessage<A::Message>> {
    // Create the shader widget that will render Strata content
    let program = StrataShaderProgram {
        snapshot: state.snapshot.clone(),
        selection: A::selection(&state.app).cloned(),
        background: crate::strata::primitives::Color::rgb(0.1, 0.1, 0.1),
    };

    shader::Shader::new(program)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Create subscriptions.
fn subscription<A: StrataApp>(state: &ShellState<A>) -> Subscription<ShellMessage<A::Message>> {
    // Listen to window events
    let events = iced::event::listen_with(|event, _status, window_id| {
        Some(ShellMessage::Event(event, window_id))
    });

    // Get app subscriptions
    let _app_sub = A::subscription(&state.app);
    // TODO: Convert app subscription to iced subscription

    // Combine subscriptions
    Subscription::batch([events])
}

/// Convert a Strata Command to an iced Task.
fn command_to_task<M: Send + 'static>(mut cmd: Command<M>) -> Task<ShellMessage<M>> {
    let futures = cmd.take_futures();

    if futures.is_empty() {
        return Task::none();
    }

    let tasks: Vec<Task<ShellMessage<M>>> = futures
        .into_iter()
        .map(|fut| Task::future(async move { ShellMessage::App(fut.await) }))
        .collect();

    Task::batch(tasks)
}

/// Convert an iced Event to a Strata Event.
fn convert_event(event: &Event) -> Option<crate::strata::event_context::Event> {
    match event {
        Event::Mouse(mouse_event) => {
            let strata_event = match mouse_event {
                iced::mouse::Event::ButtonPressed(button) => MouseEvent::ButtonPressed {
                    button: convert_mouse_button(*button),
                    position: Point::ORIGIN, // Will be filled from cursor position
                },
                iced::mouse::Event::ButtonReleased(button) => MouseEvent::ButtonReleased {
                    button: convert_mouse_button(*button),
                    position: Point::ORIGIN,
                },
                iced::mouse::Event::CursorMoved { position } => MouseEvent::CursorMoved {
                    position: Point::new(position.x, position.y),
                },
                iced::mouse::Event::CursorEntered => MouseEvent::CursorEntered,
                iced::mouse::Event::CursorLeft => MouseEvent::CursorLeft,
                iced::mouse::Event::WheelScrolled { delta } => MouseEvent::WheelScrolled {
                    delta: match delta {
                        iced::mouse::ScrollDelta::Lines { x, y } => ScrollDelta::Lines {
                            x: *x,
                            y: *y,
                        },
                        iced::mouse::ScrollDelta::Pixels { x, y } => ScrollDelta::Pixels {
                            x: *x,
                            y: *y,
                        },
                    },
                },
            };
            Some(crate::strata::event_context::Event::Mouse(strata_event))
        }

        Event::Keyboard(keyboard_event) => {
            let strata_event = match keyboard_event {
                iced::keyboard::Event::KeyPressed { key, modifiers, .. } => {
                    KeyEvent::Pressed {
                        key: convert_key(key),
                        modifiers: convert_modifiers(*modifiers),
                    }
                }
                iced::keyboard::Event::KeyReleased { key, modifiers, .. } => {
                    KeyEvent::Released {
                        key: convert_key(key),
                        modifiers: convert_modifiers(*modifiers),
                    }
                }
                _ => return None,
            };
            Some(crate::strata::event_context::Event::Keyboard(strata_event))
        }

        _ => None,
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

fn convert_key(key: &iced::keyboard::Key) -> crate::strata::event_context::Key {
    use crate::strata::event_context::Key;

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
    snapshot: LayoutSnapshot,
    selection: Option<Selection>,
    background: crate::strata::primitives::Color,
}

/// Primitive passed to the GPU.
#[derive(Clone, Debug)]
struct StrataPrimitive {
    snapshot: LayoutSnapshot,
    selection: Option<Selection>,
    background: crate::strata::primitives::Color,
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
        // Get or create the pipeline
        if !storage.has::<StrataPipeline>() {
            let pipeline = StrataPipeline::new(device, format);
            storage.store(pipeline);
        }

        let pipeline = storage.get_mut::<StrataPipeline>().unwrap();
        pipeline.prepare(
            device,
            queue,
            &self.snapshot,
            self.selection.as_ref(),
            bounds,
            viewport,
            self.background,
        );
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &shader::Storage,
        target: &wgpu::TextureView,
        clip_bounds: &iced::Rectangle<u32>,
    ) {
        let Some(pipeline) = storage.get::<StrataPipeline>() else {
            return;
        };

        pipeline.render(encoder, target, clip_bounds);
    }
}

// ============================================================================
// GPU Pipeline (Minimal Implementation)
// ============================================================================

/// GPU pipeline for rendering Strata content.
///
/// This is a minimal implementation that just clears to a background color.
/// The full implementation will integrate text rendering and selection.
struct StrataPipeline {
    clear_color: wgpu::Color,
}

impl StrataPipeline {
    fn new(_device: &wgpu::Device, _format: wgpu::TextureFormat) -> Self {
        Self {
            clear_color: wgpu::Color {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 1.0,
            },
        }
    }

    fn prepare(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _snapshot: &LayoutSnapshot,
        _selection: Option<&Selection>,
        _bounds: &iced::Rectangle,
        _viewport: &iced::advanced::graphics::Viewport,
        background: crate::strata::primitives::Color,
    ) {
        // Update clear color
        self.clear_color = wgpu::Color {
            r: background.r as f64,
            g: background.g as f64,
            b: background.b as f64,
            a: background.a as f64,
        };

        // TODO: Build render data from snapshot
        // - Generate glyph instances
        // - Generate selection quads
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        _clip_bounds: &iced::Rectangle<u32>,
    ) {
        // For now, just clear to background color
        let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Strata Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.clear_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // TODO: Draw content
        // - Set pipeline
        // - Set bind groups
        // - Draw instances
    }
}
