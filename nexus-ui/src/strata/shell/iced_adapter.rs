//! Iced Shell Adapter
//!
//! This module bridges Strata applications to iced for window management,
//! event handling, and GPU rendering.
//!
//! **This is the ONLY file in Strata that imports iced.**

use std::sync::Arc;

use iced::widget::shader::{self, wgpu};
use iced::{Element, Event, Length, Subscription, Task, Theme};

use crate::strata::app::{AppConfig, Command, StrataApp};
use crate::strata::content_address::Selection;
use crate::strata::layout_snapshot::HitResult;
use crate::strata::gpu::StrataPipeline;
use crate::strata::event_context::{
    CaptureState, KeyEvent, Modifiers, MouseButton, MouseEvent, NamedKey, ScrollDelta,
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

    /// Current pointer capture state.
    capture: CaptureState,

    /// Current window size.
    window_size: (f32, f32),

    /// Current cursor position.
    cursor_position: Option<Point>,

    /// Frame counter (forces shader redraw when changed).
    frame: u64,
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
        capture: CaptureState::None,
        window_size: (1200.0, 800.0),
        cursor_position: None,
        frame: 0,
    };

    // Convert app command to shell tasks, and send initial tick to trigger first render
    let app_task = command_to_task(cmd);
    let tick_task = Task::done(ShellMessage::Tick);
    let task = Task::batch([app_task, tick_task]);

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
            state.frame = state.frame.wrapping_add(1); // Trigger redraw
            command_to_task(cmd)
        }

        ShellMessage::Event(event, _window_id) => {
            // Increment frame counter to trigger redraw
            state.frame = state.frame.wrapping_add(1);

            // Handle window resize
            if let Event::Window(iced::window::Event::Resized(size)) = event {
                state.window_size = (size.width, size.height);
            }

            // Handle mouse events
            if let Event::Mouse(mouse_event) = &event {
                // Update cursor position
                if let iced::mouse::Event::CursorMoved { position } = mouse_event {
                    state.cursor_position = Some(Point::new(position.x, position.y));
                }

                // Convert to Strata mouse event and dispatch
                if let Some(strata_event) = convert_mouse_event(mouse_event, state.cursor_position)
                {
                    // Build snapshot for hit-testing
                    let mut snapshot = LayoutSnapshot::new();
                    snapshot.set_viewport(Rect::new(
                        0.0,
                        0.0,
                        state.window_size.0,
                        state.window_size.1,
                    ));
                    A::view(&state.app, &mut snapshot);

                    // Hit-test at cursor position → HitResult
                    let hit: Option<HitResult> = state
                        .cursor_position
                        .and_then(|pos| snapshot.hit_test(pos));

                    // CAPTURE LOGIC FIX:
                    // If pointer is captured, we MUST dispatch the event even if hit is None.
                    // This allows dragging outside the widget/window bounds.
                    let should_dispatch = hit.is_some() || state.capture.is_captured();

                    if should_dispatch {
                        let response = A::on_mouse(&state.app, strata_event, hit, &state.capture);

                        // Process capture request
                        use crate::strata::app::CaptureRequest;
                        match response.capture {
                            CaptureRequest::Capture(source) => {
                                state.capture = CaptureState::Captured(source);
                            }
                            CaptureRequest::Release => {
                                state.capture = CaptureState::None;
                            }
                            CaptureRequest::None => {}
                        }

                        // Process message
                        if let Some(msg) = response.message {
                            let cmd = A::update(&mut state.app, msg);
                            return command_to_task(cmd);
                        }
                    }
                }
            }

            Task::none()
        }

        ShellMessage::Tick => {
            // Increment frame to trigger view rebuild
            state.frame = state.frame.wrapping_add(1);
            Task::none()
        }
    }
}

/// Build the view.
fn view<A: StrataApp>(state: &ShellState<A>) -> Element<'_, ShellMessage<A::Message>> {
    // Build snapshot fresh each frame
    // (iced calls view() whenever it needs to render)
    let mut snapshot = LayoutSnapshot::new();
    snapshot.set_viewport(Rect::new(0.0, 0.0, state.window_size.0, state.window_size.1));
    A::view(&state.app, &mut snapshot);

    // Wrap in Arc to prevent deep copying when iced clones the primitive.
    // The shader only needs read access.
    let snapshot = Arc::new(snapshot);

    // Create the shader widget that will render Strata content
    let program = StrataShaderProgram {
        snapshot, // Cheap Arc clone
        selection: A::selection(&state.app).cloned(),
        background: crate::strata::primitives::Color::rgb(0.1, 0.1, 0.1),
        frame: state.frame,
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

    // Animation tick synced to monitor refresh rate (vsync)
    let tick = iced::window::frames()
        .map(|_| ShellMessage::Tick);

    // Get app subscriptions
    let _app_sub = A::subscription(&state.app);
    // TODO: Convert app subscription to iced subscription

    // Combine subscriptions
    Subscription::batch([events, tick])
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
    /// Layout snapshot wrapped in Arc to avoid deep copying when iced clones.
    snapshot: Arc<LayoutSnapshot>,
    selection: Option<Selection>,
    background: crate::strata::primitives::Color,
    /// Frame counter - changing this triggers iced to redraw.
    frame: u64,
}

/// Primitive passed to the GPU.
#[derive(Clone, Debug)]
struct StrataPrimitive {
    /// Layout snapshot wrapped in Arc to avoid deep copying.
    snapshot: Arc<LayoutSnapshot>,
    selection: Option<Selection>,
    background: crate::strata::primitives::Color,
    frame: u64,
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
        wrapper.prepare(
            device,
            queue,
            &*self.snapshot, // Deref Arc to get &LayoutSnapshot
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
        background: crate::strata::primitives::Color,
    ) {
        let scale = viewport.scale_factor() as f32;

        // Create or recreate pipeline if scale factor changed
        if self.pipeline.is_none() || (self.current_scale - scale).abs() > 0.01 {
            let font_size = BASE_FONT_SIZE * scale;
            self.pipeline = Some(StrataPipeline::new(device, self.format, font_size));
            self.current_scale = scale;
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
        fn clip_to_gpu(clip: &Option<crate::strata::primitives::Rect>, scale: f32) -> Option<[f32; 4]> {
            clip.map(|c| [c.x * scale, c.y * scale, c.width * scale, c.height * scale])
        }

        /// Apply clip from a primitive to the pipeline instances added since `start`.
        #[inline]
        fn maybe_clip(
            pipeline: &mut crate::strata::gpu::StrataPipeline,
            start: usize,
            clip: &Option<crate::strata::primitives::Rect>,
            scale: f32,
        ) {
            if let Some(gpu_clip) = clip_to_gpu(clip, scale) {
                pipeline.apply_clip_since(start, gpu_clip);
            }
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

        // 2b. Primitive backgrounds (solid rects, rounded rects, circles)
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

        // 3. Selection highlight (on top of backgrounds, behind text)
        if let Some(sel) = selection {
            if !sel.is_collapsed() {
                let selection_rects = snapshot.selection_bounds(sel);
                let scaled_rects: Vec<_> = selection_rects
                    .iter()
                    .map(|r| crate::strata::primitives::Rect {
                        x: r.x * scale,
                        y: r.y * scale,
                        width: r.width * scale,
                        height: r.height * scale,
                    })
                    .collect();
                pipeline.add_solid_rects(&scaled_rects, crate::strata::gpu::SELECTION_COLOR);
            }
        }

        // 4. Grid content from sources (terminals use this path)
        for (_source_id, source_layout) in snapshot.sources_in_order() {
            for item in &source_layout.items {
                if let crate::strata::layout_snapshot::ItemLayout::Grid(grid_layout) = item {
                    let grid_clip = &grid_layout.clip_rect;
                    for (row_idx, row) in grid_layout.rows_content.iter().enumerate() {
                        if row.text.trim().is_empty() {
                            continue;
                        }
                        let start = pipeline.instance_count();
                        let x = grid_layout.bounds.x * scale;
                        let y = (grid_layout.bounds.y + row_idx as f32 * grid_layout.cell_height) * scale;
                        let color = crate::strata::primitives::Color::unpack(row.color);
                        pipeline.add_text(&row.text, x, y, color);
                        maybe_clip(pipeline, start, grid_clip, scale);
                    }
                }
            }
        }

        // 5. Primitive text runs
        for prim in &primitives.text_runs {
            let start = pipeline.instance_count();
            pipeline.add_text(
                &prim.text,
                prim.position.x * scale,
                prim.position.y * scale,
                prim.color,
            );
            maybe_clip(pipeline, start, &prim.clip_rect, scale);
        }

        // Render foreground decorations LAST (on top of everything).
        for decoration in snapshot.foreground_decorations() {
            render_decoration(pipeline, decoration, scale);
        }

        // Create command encoder for staging belt uploads.
        // The StagingBelt writes directly to unified memory on Apple Silicon,
        // avoiding intermediate buffer copies.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Strata Staging Upload"),
        });

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
}

/// Helper to render a decoration primitive via the ubershader pipeline.
fn render_decoration(
    pipeline: &mut StrataPipeline,
    decoration: &crate::strata::layout_snapshot::Decoration,
    scale: f32,
) {
    use crate::strata::layout_snapshot::Decoration;

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
    style: crate::strata::layout::primitives::LineStyle,
) -> crate::strata::gpu::LineStyle {
    match style {
        crate::strata::layout::primitives::LineStyle::Solid => crate::strata::gpu::LineStyle::Solid,
        crate::strata::layout::primitives::LineStyle::Dashed => {
            crate::strata::gpu::LineStyle::Dashed
        }
        crate::strata::layout::primitives::LineStyle::Dotted => {
            crate::strata::gpu::LineStyle::Dotted
        }
    }
}
