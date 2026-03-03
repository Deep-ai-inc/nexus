//! Linux Shell Backend (winit + wgpu)
//!
//! Provides the `run()` function for Linux. Uses winit for windowing and input,
//! and wgpu for GPU rendering.

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{NamedKey as WinitNamedKey, PhysicalKey};
use winit::window::{Window, WindowId};
use std::sync::Arc;

use crate::app::{AppConfig, Command, StrataApp};
use crate::event_context::{
    CaptureState, Key, KeyEvent, Modifiers, MouseButton, MouseEvent, NamedKey, ScrollDelta,
};
use crate::gpu::{ImageStore, WgpuRenderer};
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Point, Rect};

use super::populate::{populate_pipeline, BASE_FONT_SIZE};

/// Error type for shell operations.
#[derive(Debug)]
pub enum Error {
    /// Window creation failed.
    Window(String),
    /// GPU initialization failed.
    Gpu(String),
    /// Event loop error.
    EventLoop(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Window(s) => write!(f, "Window error: {}", s),
            Error::Gpu(s) => write!(f, "GPU error: {}", s),
            Error::EventLoop(s) => write!(f, "Event loop error: {}", s),
        }
    }
}

impl std::error::Error for Error {}

/// Clip bounds for rendering (scissor rect).
pub struct ClipBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Run a Strata application on Linux with default configuration.
pub fn run<A: StrataApp>() -> Result<(), Error> {
    run_with_config::<A>(AppConfig::default())
}

/// Run a Strata application on Linux with the given configuration.
pub fn run_with_config<A: StrataApp>(config: AppConfig) -> Result<(), Error> {
    let event_loop = EventLoop::new()
        .map_err(|e| Error::EventLoop(format!("Failed to create event loop: {}", e)))?;

    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);

    let mut app = StrataRunner::<A>::new(config);
    event_loop
        .run_app(&mut app)
        .map_err(|e| Error::EventLoop(format!("Event loop error: {}", e)))?;

    Ok(())
}

struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
}

struct StrataRunner<A: StrataApp> {
    config: AppConfig,
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    renderer: Option<WgpuRenderer>,
    app_state: Option<A::State>,
    shared: A::SharedState,
    images: ImageStore,
    capture: CaptureState,
    current_modifiers: Modifiers,
    cursor_position: Point,
    scale_factor: f32,
    command_tx: Option<std::sync::mpsc::Sender<A::Message>>,
    command_rx: Option<std::sync::mpsc::Receiver<A::Message>>,
    tokio_rt: tokio::runtime::Runtime,
}

impl<A: StrataApp> StrataRunner<A> {
    fn new(config: AppConfig) -> Self {
        let tokio_rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        Self {
            config,
            window: None,
            gpu: None,
            renderer: None,
            app_state: None,
            shared: A::SharedState::default(),
            images: ImageStore::new(),
            capture: CaptureState::None,
            current_modifiers: Modifiers::NONE,
            cursor_position: Point { x: 0.0, y: 0.0 },
            scale_factor: 1.0,
            command_tx: None,
            command_rx: None,
            tokio_rt,
        }
    }

    fn init_gpu(&mut self, window: Arc<Window>) -> Result<(), Error> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| Error::Gpu(format!("Failed to create surface: {}", e)))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| Error::Gpu("No suitable GPU adapter found".into()))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("strata_device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| Error::Gpu(format!("Failed to create device: {}", e)))?;

        let size = window.inner_size();
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        self.scale_factor = window.scale_factor() as f32;
        let font_size = BASE_FONT_SIZE * self.scale_factor;

        let fs_mutex = crate::text_engine::get_font_system();
        let mut font_system = fs_mutex.lock().unwrap();

        let renderer = WgpuRenderer::new(
            &device,
            &queue,
            surface_format,
            font_size,
            &mut font_system,
        );

        self.gpu = Some(GpuState {
            device,
            queue,
            surface,
            surface_config,
        });
        self.renderer = Some(renderer);

        Ok(())
    }

    fn render(&mut self) {
        let Some(gpu) = &self.gpu else { return };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        if self.app_state.is_none() {
            return;
        }

        // Process pending async commands
        if let Some(rx) = &self.command_rx {
            while let Ok(msg) = rx.try_recv() {
                if let Some(state) = &mut self.app_state {
                    let cmd = A::update(state, msg, &mut self.images);
                    if let Some(tx) = &self.command_tx {
                        spawn_commands(&self.tokio_rt, cmd, tx.clone());
                    }
                }
            }
        }

        // Get app state again after potential mutation
        let app_state = self.app_state.as_ref().unwrap();

        let zoom = A::zoom_level(app_state);
        let scale = self.scale_factor * zoom;

        // Drain pending image uploads
        for img in self.images.drain_pending() {
            renderer.load_image_rgba(
                &gpu.device,
                &gpu.queue,
                img.width,
                img.height,
                &img.data,
            );
        }
        for handle in self.images.drain_pending_unloads() {
            renderer.pipeline.unload_image(handle);
        }

        // Build scene
        let mut snapshot = LayoutSnapshot::new();
        snapshot.set_viewport(Rect {
            x: 0.0,
            y: 0.0,
            width: gpu.surface_config.width as f32 / scale,
            height: gpu.surface_config.height as f32 / scale,
        });
        snapshot.set_zoom_level(zoom);
        A::view(app_state, &mut snapshot);

        let selection = A::selection(app_state);
        let background = A::background_color(app_state);

        renderer.pipeline.clear();
        renderer.pipeline.set_background(background);

        let fs_mutex = crate::text_engine::get_font_system();
        let mut font_system = fs_mutex.lock().unwrap();

        populate_pipeline(
            &mut renderer.pipeline,
            &snapshot,
            selection,
            scale,
            &mut font_system,
        );
        drop(font_system);

        // Prepare
        renderer.prepare(
            &gpu.device,
            &gpu.queue,
            gpu.surface_config.width as f32,
            gpu.surface_config.height as f32,
        );

        // Render
        let output = match gpu.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                // Reconfigure and retry
                gpu.surface.configure(&gpu.device, &gpu.surface_config);
                match gpu.surface.get_current_texture() {
                    Ok(t) => t,
                    Err(_) => return,
                }
            }
            Err(_) => return,
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            gpu.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("strata_encoder"),
                });

        renderer.render(&mut encoder, &view);

        gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    fn handle_key_event(&mut self, event: &winit::event::KeyEvent) {
        let key = map_key(&event.logical_key, &event.physical_key);
        let text = if event.state == ElementState::Pressed {
            event.text.as_ref().map(|s| s.to_string())
        } else {
            None
        };

        let strata_event = match event.state {
            ElementState::Pressed => KeyEvent::Pressed {
                key,
                modifiers: self.current_modifiers,
                text,
            },
            ElementState::Released => KeyEvent::Released {
                key,
                modifiers: self.current_modifiers,
            },
        };

        if let Some(app_state) = &self.app_state {
            if let Some(msg) = A::on_key(app_state, strata_event) {
                if let Some(state) = &mut self.app_state {
                    let cmd = A::update(state, msg, &mut self.images);
                    if let Some(tx) = &self.command_tx {
                        spawn_commands(&self.tokio_rt, cmd, tx.clone());
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) {
        if let Some(app_state) = &self.app_state {
            let snapshot = {
                let mut s = LayoutSnapshot::new();
                if let Some(gpu) = &self.gpu {
                    let zoom = A::zoom_level(app_state);
                    let scale = self.scale_factor * zoom;
                    s.set_viewport(Rect {
                        x: 0.0,
                        y: 0.0,
                        width: gpu.surface_config.width as f32 / scale,
                        height: gpu.surface_config.height as f32 / scale,
                    });
                    s.set_zoom_level(zoom);
                }
                A::view(app_state, &mut s);
                s
            };

            let zoom = A::zoom_level(app_state);
            let scale = self.scale_factor * zoom;
            let logical_pos = Point {
                x: self.cursor_position.x / scale,
                y: self.cursor_position.y / scale,
            };
            let hit = snapshot.hit_test(logical_pos);

            let response = A::on_mouse(app_state, event, hit, &self.capture);

            match response.capture {
                crate::app::CaptureRequest::Capture(source) => {
                    self.capture = CaptureState::Captured(source);
                }
                crate::app::CaptureRequest::Release => {
                    self.capture = CaptureState::None;
                }
                crate::app::CaptureRequest::None => {}
            }

            if let Some(msg) = response.message {
                if let Some(state) = &mut self.app_state {
                    let cmd = A::update(state, msg, &mut self.images);
                    if let Some(tx) = &self.command_tx {
                        spawn_commands(&self.tokio_rt, cmd, tx.clone());
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }
}

impl<A: StrataApp> ApplicationHandler for StrataRunner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let (w, h) = self.config.window_size;
        let window_attrs = Window::default_attributes()
            .with_title(&self.config.title)
            .with_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("Failed to create window"),
        );

        if let Err(e) = self.init_gpu(window.clone()) {
            eprintln!("[strata] GPU init failed: {}", e);
            event_loop.exit();
            return;
        }

        // Initialize app
        let (app_state, init_cmd) = A::init(&self.shared, &mut self.images);
        self.app_state = Some(app_state);

        let (tx, rx) = std::sync::mpsc::channel();
        spawn_commands(&self.tokio_rt, init_cmd, tx.clone());
        self.command_tx = Some(tx);
        self.command_rx = Some(rx);

        // Set window title from app
        if let Some(state) = &self.app_state {
            window.set_title(&A::title(state));
        }

        self.window = Some(window);

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::RedrawRequested => {
                self.render();
            }

            WindowEvent::Resized(new_size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.surface_config.width = new_size.width.max(1);
                    gpu.surface_config.height = new_size.height.max(1);
                    gpu.surface.configure(&gpu.device, &gpu.surface_config);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let new_scale = scale_factor as f32;
                if (self.scale_factor - new_scale).abs() > 0.01 {
                    self.scale_factor = new_scale;

                    // Recreate renderer with new font size
                    if let Some(gpu) = &self.gpu {
                        let font_size = BASE_FONT_SIZE * self.scale_factor;
                        let fs_mutex = crate::text_engine::get_font_system();
                        let mut font_system = fs_mutex.lock().unwrap();
                        self.renderer = Some(WgpuRenderer::new(
                            &gpu.device,
                            &gpu.queue,
                            gpu.surface_config.format,
                            font_size,
                            &mut font_system,
                        ));
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.current_modifiers = Modifiers {
                    shift: state.shift_key(),
                    ctrl: state.control_key(),
                    alt: state.alt_key(),
                    meta: state.super_key(),
                };
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                self.handle_key_event(&key_event);
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Point {
                    x: position.x as f32,
                    y: position.y as f32,
                };
                let zoom = self.app_state.as_ref().map(|s| A::zoom_level(s)).unwrap_or(1.0);
                let scale = self.scale_factor * zoom;
                self.handle_mouse_event(MouseEvent::CursorMoved {
                    position: Point {
                        x: position.x as f32 / scale,
                        y: position.y as f32 / scale,
                    },
                });
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let btn = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    winit::event::MouseButton::Back => MouseButton::Back,
                    winit::event::MouseButton::Forward => MouseButton::Forward,
                    winit::event::MouseButton::Other(id) => MouseButton::Other(id),
                };
                let zoom = self.app_state.as_ref().map(|s| A::zoom_level(s)).unwrap_or(1.0);
                let scale = self.scale_factor * zoom;
                let pos = Point {
                    x: self.cursor_position.x / scale,
                    y: self.cursor_position.y / scale,
                };
                let event = match state {
                    ElementState::Pressed => MouseEvent::ButtonPressed {
                        button: btn,
                        position: pos,
                        modifiers: self.current_modifiers,
                    },
                    ElementState::Released => MouseEvent::ButtonReleased {
                        button: btn,
                        position: pos,
                        modifiers: self.current_modifiers,
                    },
                };
                self.handle_mouse_event(event);
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let zoom = self.app_state.as_ref().map(|s| A::zoom_level(s)).unwrap_or(1.0);
                let scale = self.scale_factor * zoom;
                let scroll_delta = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        ScrollDelta::Lines { x, y }
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        ScrollDelta::Pixels {
                            x: pos.x as f32,
                            y: pos.y as f32,
                            phase: None,
                        }
                    }
                };
                let pos = Point {
                    x: self.cursor_position.x / scale,
                    y: self.cursor_position.y / scale,
                };
                self.handle_mouse_event(MouseEvent::WheelScrolled {
                    delta: scroll_delta,
                    position: pos,
                });
            }

            WindowEvent::CursorEntered { .. } => {
                self.handle_mouse_event(MouseEvent::CursorEntered);
            }

            WindowEvent::CursorLeft { .. } => {
                self.handle_mouse_event(MouseEvent::CursorLeft);
            }

            _ => {}
        }
    }
}

// =============================================================================
// Key mapping
// =============================================================================

fn map_key(
    logical: &winit::keyboard::Key,
    _physical: &PhysicalKey,
) -> Key {
    match logical {
        winit::keyboard::Key::Named(named) => {
            let strata_key = match named {
                WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
                WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
                WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
                WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
                WinitNamedKey::Home => NamedKey::Home,
                WinitNamedKey::End => NamedKey::End,
                WinitNamedKey::PageUp => NamedKey::PageUp,
                WinitNamedKey::PageDown => NamedKey::PageDown,
                WinitNamedKey::Backspace => NamedKey::Backspace,
                WinitNamedKey::Delete => NamedKey::Delete,
                WinitNamedKey::Insert => NamedKey::Insert,
                WinitNamedKey::Enter => NamedKey::Enter,
                WinitNamedKey::Tab => NamedKey::Tab,
                WinitNamedKey::Shift => NamedKey::Shift,
                WinitNamedKey::Control => NamedKey::Control,
                WinitNamedKey::Alt => NamedKey::Alt,
                WinitNamedKey::Super => NamedKey::Meta,
                WinitNamedKey::Escape => NamedKey::Escape,
                WinitNamedKey::Space => NamedKey::Space,
                WinitNamedKey::CapsLock => NamedKey::CapsLock,
                WinitNamedKey::NumLock => NamedKey::NumLock,
                WinitNamedKey::ScrollLock => NamedKey::ScrollLock,
                WinitNamedKey::PrintScreen => NamedKey::PrintScreen,
                WinitNamedKey::Pause => NamedKey::Pause,
                WinitNamedKey::ContextMenu => NamedKey::ContextMenu,
                WinitNamedKey::F1 => NamedKey::F1,
                WinitNamedKey::F2 => NamedKey::F2,
                WinitNamedKey::F3 => NamedKey::F3,
                WinitNamedKey::F4 => NamedKey::F4,
                WinitNamedKey::F5 => NamedKey::F5,
                WinitNamedKey::F6 => NamedKey::F6,
                WinitNamedKey::F7 => NamedKey::F7,
                WinitNamedKey::F8 => NamedKey::F8,
                WinitNamedKey::F9 => NamedKey::F9,
                WinitNamedKey::F10 => NamedKey::F10,
                WinitNamedKey::F11 => NamedKey::F11,
                WinitNamedKey::F12 => NamedKey::F12,
                _ => NamedKey::Unknown,
            };
            Key::Named(strata_key)
        }
        winit::keyboard::Key::Character(c) => Key::Character(c.to_string()),
        winit::keyboard::Key::Unidentified(_) | winit::keyboard::Key::Dead(_) => {
            Key::Named(NamedKey::Unknown)
        }
    }
}

// =============================================================================
// Async Commands
// =============================================================================

fn spawn_commands<M: Send + 'static>(
    rt: &tokio::runtime::Runtime,
    mut cmd: Command<M>,
    tx: std::sync::mpsc::Sender<M>,
) {
    for fut in cmd.take_futures() {
        let tx = tx.clone();
        rt.spawn(async move {
            let msg = fut.await;
            let _ = tx.send(msg);
        });
    }
}
