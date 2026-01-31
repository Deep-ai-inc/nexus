//! Shell widget — owns terminal blocks, PTY handles, jobs, and image handles.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::{BlockId, BlockState, ShellEvent, Value};
use nexus_kernel::{CommandClassification, Kernel};
use nexus_term::TerminalParser;

use crate::blocks::{Block, PtyEvent, VisualJob, VisualJobState};
use crate::pty::PtyHandle;
use crate::systems::{kernel_subscription, pty_subscription};
use strata::{ImageHandle, ImageStore, Subscription};
use strata::event_context::{Key, KeyEvent, NamedKey};

use super::message::{NexusMessage, ShellMsg};

/// Typed output from ShellWidget → orchestrator.
pub(crate) enum ShellOutput {
    /// Nothing happened.
    None,
    /// Orchestrator should focus input.
    FocusInput,
    /// Orchestrator should focus a specific PTY block.
    FocusBlock(BlockId),
    /// Orchestrator should scroll history to bottom.
    ScrollToBottom,
    /// Working directory changed.
    CwdChanged(PathBuf),
    /// A kernel command finished. Orchestrator should update context.
    CommandFinished {
        exit_code: i32,
        command: String,
        output: String,
    },
    /// A PTY block exited. Root decides focus based on current focus state.
    BlockExited {
        id: BlockId,
    },
    /// PTY input forwarding failed (block gone). Root should focus input.
    PtyInputFailed,
}

impl Default for ShellOutput {
    fn default() -> Self { Self::None }
}

/// Manages all shell-related state: terminal blocks, PTY handles, jobs, images.
pub(crate) struct ShellWidget {
    pub blocks: Vec<Block>,
    pub block_index: HashMap<BlockId, usize>,
    pub pty_handles: Vec<PtyHandle>,
    pub pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    pub terminal_size: Cell<(u16, u16)>,
    pub last_terminal_size: (u16, u16),
    pub terminal_dirty: bool,
    pub last_exit_code: Option<i32>,
    pub image_handles: HashMap<BlockId, (ImageHandle, u32, u32)>,
    pub jobs: Vec<VisualJob>,

    // --- Subscription channels (owned by this widget) ---
    pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
}

impl ShellWidget {
    pub fn new(
        pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
        pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
        kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
    ) -> Self {
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            pty_handles: Vec::new(),
            pty_tx,
            terminal_size: Cell::new((120, 24)),
            last_terminal_size: (120, 24),
            terminal_dirty: false,
            last_exit_code: None,
            image_handles: HashMap::new(),
            jobs: Vec::new(),
            pty_rx,
            kernel_rx,
        }
    }

    /// Whether the shell has pending output that needs a redraw tick.
    pub fn needs_redraw(&self) -> bool {
        self.terminal_dirty
    }

    /// Create the subscription for PTY and kernel events.
    ///
    /// Returns `Subscription<NexusMessage>` directly because iced's
    /// `Subscription::map` panics on capturing closures, so we can't
    /// return `Subscription<ShellMsg>` and `map_msg` at the root.
    pub fn subscription(&self) -> Subscription<NexusMessage> {
        let mut subs = Vec::new();

        let pty_rx = self.pty_rx.clone();
        subs.push(Subscription::from_iced(
            pty_subscription(pty_rx).map(|(id, evt)| match evt {
                PtyEvent::Output(data) => NexusMessage::Shell(ShellMsg::PtyOutput(id, data)),
                PtyEvent::Exited(code) => NexusMessage::Shell(ShellMsg::PtyExited(id, code)),
            }),
        ));

        let kernel_rx = self.kernel_rx.clone();
        subs.push(Subscription::from_iced(
            kernel_subscription(kernel_rx).map(|evt| NexusMessage::Shell(ShellMsg::KernelEvent(evt))),
        ));

        Subscription::batch(subs)
    }

    /// Handle a message, returning commands and cross-cutting output.
    pub fn update(&mut self, msg: ShellMsg, ctx: &mut strata::component::Ctx) -> (strata::Command<ShellMsg>, ShellOutput) {
        let output = match msg {
            ShellMsg::PtyOutput(id, data) => self.handle_pty_output(id, data),
            ShellMsg::PtyExited(id, exit_code) => self.handle_pty_exited(id, exit_code),
            ShellMsg::KernelEvent(evt) => self.handle_kernel_event(evt, ctx.images),
            ShellMsg::SendInterrupt(id) => { self.send_interrupt_to(id); ShellOutput::None }
            ShellMsg::KillBlock(id) => { self.kill_block(id); ShellOutput::None }
            ShellMsg::PtyInput(block_id, event) => {
                if self.forward_key(block_id, &event) {
                    ShellOutput::None
                } else {
                    ShellOutput::PtyInputFailed
                }
            }
            ShellMsg::SortTable(block_id, col_idx) => { self.sort_table(block_id, col_idx); ShellOutput::None }
        };
        (strata::Command::none(), output)
    }

    /// Execute a command (kernel or external PTY).
    /// The orchestrator should handle "clear" before calling this.
    pub fn execute(
        &mut self,
        command: String,
        block_id: BlockId,
        cwd: &str,
        kernel: &Arc<Mutex<Kernel>>,
        kernel_tx: &broadcast::Sender<ShellEvent>,
    ) -> ShellOutput {
        let trimmed = command.trim().to_string();

        let classification = kernel.blocking_lock().classify_command(&trimmed);

        if classification == CommandClassification::Kernel {
            self.execute_kernel_command(trimmed, block_id, cwd, kernel, kernel_tx)
        } else {
            self.execute_pty_command(trimmed, block_id, cwd)
        }
    }

    /// Handle PTY output data.
    pub fn handle_pty_output(&mut self, id: BlockId, data: Vec<u8>) -> ShellOutput {
        if let Some(&idx) = self.block_index.get(&id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.parser.feed(&data);
                block.version += 1;
            }
        }
        if data.len() < 128 {
            self.terminal_dirty = false;
            ShellOutput::ScrollToBottom
        } else {
            self.terminal_dirty = true;
            ShellOutput::None
        }
    }

    /// Handle PTY exit. Returns the exited block ID so root can decide focus.
    pub fn handle_pty_exited(&mut self, id: BlockId, exit_code: i32) -> ShellOutput {
        if let Some(&idx) = self.block_index.get(&id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.state = if exit_code == 0 {
                    BlockState::Success
                } else {
                    BlockState::Failed(exit_code)
                };
                block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
                block.version += 1;
            }
        }
        self.pty_handles.retain(|h| h.block_id != id);
        self.last_exit_code = Some(exit_code);
        ShellOutput::BlockExited { id }
    }

    /// Handle a kernel event.
    pub fn handle_kernel_event(
        &mut self,
        evt: ShellEvent,
        images: &mut ImageStore,
    ) -> ShellOutput {
        match evt {
            ShellEvent::CommandStarted { block_id, command, .. } => {
                if !self.block_index.contains_key(&block_id) {
                    let mut block = Block::new(block_id, command);
                    let (ts_cols, ts_rows) = self.terminal_size.get();
                    block.parser = TerminalParser::new(ts_cols, ts_rows);
                    let block_idx = self.blocks.len();
                    self.blocks.push(block);
                    self.block_index.insert(block_id, block_idx);
                }
                ShellOutput::None
            }
            ShellEvent::StdoutChunk { block_id, data }
            | ShellEvent::StderrChunk { block_id, data } => {
                if let Some(&idx) = self.block_index.get(&block_id) {
                    if let Some(block) = self.blocks.get_mut(idx) {
                        block.parser.feed(&data);
                        block.version += 1;
                    }
                }
                self.terminal_dirty = true;
                ShellOutput::None
            }
            ShellEvent::CommandOutput { block_id, value } => {
                if let Value::Media {
                    ref data,
                    ref content_type,
                    ..
                } = value
                {
                    if content_type.starts_with("image/") {
                        if let Ok(img) = image::load_from_memory(data) {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            let handle = images.load_rgba(w, h, rgba.into_raw());
                            self.image_handles.insert(block_id, (handle, w, h));
                        }
                    }
                }
                if let Some(&idx) = self.block_index.get(&block_id) {
                    if let Some(block) = self.blocks.get_mut(idx) {
                        block.native_output = Some(value);
                    }
                }
                ShellOutput::None
            }
            ShellEvent::CommandFinished {
                block_id,
                exit_code,
                duration_ms,
            } => {
                let mut cmd = String::new();
                let mut output = String::new();
                if let Some(&idx) = self.block_index.get(&block_id) {
                    if let Some(block) = self.blocks.get_mut(idx) {
                        block.state = if exit_code == 0 {
                            BlockState::Success
                        } else {
                            BlockState::Failed(exit_code)
                        };
                        block.duration_ms = Some(duration_ms);
                        block.version += 1;
                        cmd = block.command.clone();
                        let raw = block.parser.grid_with_scrollback().to_string();
                        output = if raw.len() > 10_000 {
                            raw[raw.len() - 10_000..].to_string()
                        } else {
                            raw
                        };
                    }
                }
                self.last_exit_code = Some(exit_code);
                ShellOutput::CommandFinished {
                    exit_code,
                    command: cmd,
                    output,
                }
            }
            ShellEvent::JobStateChanged {
                job_id,
                state: job_state,
            } => {
                match job_state {
                    nexus_api::JobState::Running => {
                        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                            job.state = VisualJobState::Running;
                        } else {
                            self.jobs.push(VisualJob::new(
                                job_id,
                                format!("Job {}", job_id),
                                VisualJobState::Running,
                            ));
                        }
                    }
                    nexus_api::JobState::Stopped => {
                        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                            job.state = VisualJobState::Stopped;
                        } else {
                            self.jobs.push(VisualJob::new(
                                job_id,
                                format!("Job {}", job_id),
                                VisualJobState::Stopped,
                            ));
                        }
                    }
                    nexus_api::JobState::Done(_) => {
                        self.jobs.retain(|j| j.id != job_id);
                    }
                }
                ShellOutput::None
            }
            ShellEvent::CwdChanged { new, .. } => ShellOutput::CwdChanged(new),
            _ => ShellOutput::None,
        }
    }

    /// Send interrupt (Ctrl+C) to focused or last running PTY.
    pub fn send_interrupt_to(&self, id: BlockId) {
        if let Some(handle) = self.pty_handles.iter().find(|h| h.block_id == id) {
            let _ = handle.send_interrupt();
        }
    }

    /// Kill a specific block's PTY.
    pub fn kill_block(&self, id: BlockId) {
        if let Some(handle) = self.pty_handles.iter().find(|h| h.block_id == id) {
            let _ = handle.send_interrupt();
            handle.kill();
        }
    }

    /// Forward a key event to a focused PTY block. Returns false if PTY is gone.
    pub fn forward_key(&self, block_id: BlockId, event: &KeyEvent) -> bool {
        if let Some(handle) = self.pty_handles.iter().find(|h| h.block_id == block_id) {
            if let Some(bytes) = strata_key_to_bytes(event) {
                let _ = handle.write(&bytes);
            }
            true
        } else {
            false
        }
    }

    /// Sort a table by column.
    pub fn sort_table(&mut self, block_id: BlockId, col_idx: usize) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.table_sort.toggle(col_idx);
                if let Some(Value::Table { ref mut rows, .. }) = block.native_output {
                    let ascending = block.table_sort.ascending;
                    rows.sort_by(|a, b| {
                        let va = a.get(col_idx).map(|v| v.to_text()).unwrap_or_default();
                        let vb = b.get(col_idx).map(|v| v.to_text()).unwrap_or_default();
                        if let (Ok(na), Ok(nb)) = (va.parse::<f64>(), vb.parse::<f64>()) {
                            let cmp = na
                                .partial_cmp(&nb)
                                .unwrap_or(std::cmp::Ordering::Equal);
                            if ascending {
                                cmp
                            } else {
                                cmp.reverse()
                            }
                        } else {
                            let cmp = va.cmp(&vb);
                            if ascending {
                                cmp
                            } else {
                                cmp.reverse()
                            }
                        }
                    });
                }
            }
        }
    }

    /// Clear all blocks, kill PTYs, clear jobs.
    pub fn clear(&mut self) {
        for handle in &self.pty_handles {
            let _ = handle.send_interrupt();
            handle.kill();
        }
        self.pty_handles.clear();
        self.blocks.clear();
        self.block_index.clear();
        self.jobs.clear();
    }

    /// Propagate terminal size changes to running PTY handles.
    pub fn sync_terminal_size(&mut self) {
        let current_size = self.terminal_size.get();
        if current_size != self.last_terminal_size {
            self.last_terminal_size = current_size;
            let (cols, rows) = current_size;
            for handle in &self.pty_handles {
                let _ = handle.resize(cols, rows);
            }
        }
    }

    // ---- Internal ----

    fn execute_kernel_command(
        &mut self,
        cmd: String,
        block_id: BlockId,
        cwd: &str,
        kernel: &Arc<Mutex<Kernel>>,
        kernel_tx: &broadcast::Sender<ShellEvent>,
    ) -> ShellOutput {
        let mut block = Block::new(block_id, cmd.clone());
        let (ts_cols, ts_rows) = self.terminal_size.get();
        block.parser = TerminalParser::new(ts_cols, ts_rows);
        let block_idx = self.blocks.len();
        self.blocks.push(block);
        self.block_index.insert(block_id, block_idx);

        let kernel = kernel.clone();
        let kernel_tx = kernel_tx.clone();
        let cwd = cwd.to_string();

        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let mut kernel = kernel.lock().await;
                    let _ = kernel
                        .state_mut()
                        .set_cwd(std::path::PathBuf::from(&cwd));
                    let _ = kernel.execute_with_block_id(&cmd, Some(block_id));
                });
            }));

            if let Err(panic_info) = result {
                let error_msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    format!("Command panicked: {}", s)
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    format!("Command panicked: {}", s)
                } else {
                    "Command panicked (unknown error)".to_string()
                };
                let _ = kernel_tx.send(ShellEvent::StderrChunk {
                    block_id,
                    data: format!("{}\n", error_msg).into_bytes(),
                });
                let _ = kernel_tx.send(ShellEvent::CommandFinished {
                    block_id,
                    exit_code: 1,
                    duration_ms: 0,
                });
            }
        });

        ShellOutput::ScrollToBottom
    }

    fn execute_pty_command(
        &mut self,
        cmd: String,
        block_id: BlockId,
        cwd: &str,
    ) -> ShellOutput {
        let mut block = Block::new(block_id, cmd.clone());
        let (ts_cols, ts_rows) = self.terminal_size.get();
        block.parser = TerminalParser::new(ts_cols, ts_rows);
        let block_idx = self.blocks.len();
        self.blocks.push(block);
        self.block_index.insert(block_id, block_idx);

        let (cols, rows) = self.terminal_size.get();

        match PtyHandle::spawn_with_size(&cmd, cwd, block_id, self.pty_tx.clone(), cols, rows) {
            Ok(handle) => {
                self.pty_handles.push(handle);
                ShellOutput::FocusBlock(block_id)
            }
            Err(e) => {
                tracing::error!("Failed to spawn PTY: {}", e);
                if let Some(&idx) = self.block_index.get(&block_id) {
                    if let Some(block) = self.blocks.get_mut(idx) {
                        block.state = BlockState::Failed(1);
                        block.parser.feed(format!("Error: {}\n", e).as_bytes());
                        block.version += 1;
                    }
                }
                ShellOutput::FocusInput
            }
        }
    }
}

// =========================================================================
// Key-to-bytes conversion for PTY input
// =========================================================================

/// Convert a Strata KeyEvent to bytes suitable for writing to a PTY.
pub(crate) fn strata_key_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    let (key, modifiers, text) = match event {
        KeyEvent::Pressed {
            key,
            modifiers,
            text,
        } => (key, modifiers, text.as_deref()),
        KeyEvent::Released { .. } => return None,
    };

    match key {
        Key::Character(c) => {
            if modifiers.ctrl && c.len() == 1 {
                let ch = c.chars().next()?;
                if ch.is_ascii_alphabetic() {
                    let ctrl_code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    return Some(vec![ctrl_code]);
                }
            }
            if let Some(t) = text {
                if !t.is_empty() {
                    return Some(t.as_bytes().to_vec());
                }
            }
            Some(c.as_bytes().to_vec())
        }
        Key::Named(named) => {
            if modifiers.ctrl {
                match named {
                    NamedKey::ArrowLeft => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'5', b'D'])
                    }
                    NamedKey::ArrowRight => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'5', b'C'])
                    }
                    NamedKey::ArrowUp => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'5', b'A'])
                    }
                    NamedKey::ArrowDown => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'5', b'B'])
                    }
                    _ => {}
                }
            }
            if modifiers.shift {
                match named {
                    NamedKey::ArrowLeft => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'2', b'D'])
                    }
                    NamedKey::ArrowRight => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'2', b'C'])
                    }
                    NamedKey::ArrowUp => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'2', b'A'])
                    }
                    NamedKey::ArrowDown => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'2', b'B'])
                    }
                    _ => {}
                }
            }
            if modifiers.alt {
                match named {
                    NamedKey::ArrowLeft => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'3', b'D'])
                    }
                    NamedKey::ArrowRight => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'3', b'C'])
                    }
                    NamedKey::ArrowUp => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'3', b'A'])
                    }
                    NamedKey::ArrowDown => {
                        return Some(vec![0x1b, b'[', b'1', b';', b'3', b'B'])
                    }
                    _ => {}
                }
            }
            match named {
                NamedKey::Enter => Some(vec![b'\r']),
                NamedKey::Backspace => Some(vec![0x7f]),
                NamedKey::Tab => Some(vec![b'\t']),
                NamedKey::Escape => Some(vec![0x1b]),
                NamedKey::Space => Some(vec![b' ']),
                NamedKey::ArrowUp => Some(vec![0x1b, b'[', b'A']),
                NamedKey::ArrowDown => Some(vec![0x1b, b'[', b'B']),
                NamedKey::ArrowRight => Some(vec![0x1b, b'[', b'C']),
                NamedKey::ArrowLeft => Some(vec![0x1b, b'[', b'D']),
                NamedKey::Home => Some(vec![0x1b, b'[', b'H']),
                NamedKey::End => Some(vec![0x1b, b'[', b'F']),
                NamedKey::PageUp => Some(vec![0x1b, b'[', b'5', b'~']),
                NamedKey::PageDown => Some(vec![0x1b, b'[', b'6', b'~']),
                NamedKey::Delete => Some(vec![0x1b, b'[', b'3', b'~']),
                _ => None,
            }
        }
    }
}
