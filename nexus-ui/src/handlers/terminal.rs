//! Terminal domain handler.
//!
//! Handles PTY output, kernel events, and block interactions.

use iced::keyboard::{self, Key, Modifiers};
use iced::widget::{scrollable, text_input};
use iced::Task;

use nexus_api::{BlockId, BlockState, ShellEvent};
use nexus_term::TerminalParser;

use crate::blocks::{Block, Focus};
use crate::constants::{HISTORY_SCROLLABLE, INPUT_FIELD};
use crate::keymap::key_to_bytes;
use crate::msg::{Message, TerminalMessage};
use crate::pty::PtyHandle;
use crate::state::Nexus;
use crate::utils::home_dir;
use crate::widgets::job_indicator::{VisualJob, VisualJobState};

/// Update the terminal domain state.
pub fn update(state: &mut Nexus, msg: TerminalMessage) -> Task<Message> {
    match msg {
        TerminalMessage::PtyOutput(id, data) => handle_pty_output(state, id, data),
        TerminalMessage::PtyExited(id, code) => handle_pty_exited(state, id, code),
        TerminalMessage::KeyPressed(key, mods) => handle_key(state, key, mods),
        TerminalMessage::KernelEvent(evt) => handle_kernel_event(state, evt),
        TerminalMessage::TableSort(id, col) => sort_table(state, id, col),
        TerminalMessage::TableCellClick(..) => Task::none(),
        TerminalMessage::JobClicked(id) => foreground_job(state, id),
        TerminalMessage::RetryWithSudo => retry_sudo(state),
        TerminalMessage::DismissPermissionPrompt => dismiss_permission(state),
        TerminalMessage::RunSuggestedCommand(cmd) => execute(state, cmd),
        TerminalMessage::DismissCommandNotFound => dismiss_not_found(state),
        TerminalMessage::KillBlock(id) => kill_block(state, id),
    }
}

/// Force kill a running PTY block.
/// Sends Ctrl+C first, then SIGKILL after 500ms.
fn kill_block(state: &mut Nexus, block_id: BlockId) -> Task<Message> {
    if let Some(handle) = state.terminal.pty_handles.iter().find(|h| h.block_id == block_id) {
        // Send Ctrl+C first (graceful)
        let _ = handle.send_interrupt();
        // Then force kill
        handle.kill();
    }
    Task::none()
}

// =============================================================================
// PTY Handlers
// =============================================================================

/// Handle PTY output data.
pub fn handle_pty_output(state: &mut Nexus, block_id: BlockId, data: Vec<u8>) -> Task<Message> {
    let terminal = &mut state.terminal;

    if let Some(&idx) = terminal.block_index.get(&block_id) {
        if let Some(block) = terminal.blocks.get_mut(idx) {
            block.parser.feed(&data);
            block.version += 1;
            // Check for permission denied
            if !block.has_permission_denied {
                if let Ok(text) = std::str::from_utf8(&data) {
                    if text.to_lowercase().contains("permission denied") {
                        block.has_permission_denied = true;
                    }
                }
            }
        }
    }

    // VSYNC-BATCHED THROTTLING
    if data.len() < 128 {
        terminal.is_dirty = false;
        return scrollable::snap_to(
            scrollable::Id::new(HISTORY_SCROLLABLE),
            scrollable::RelativeOffset::END,
        );
    } else {
        terminal.is_dirty = true;
        return Task::none();
    }
}

/// Handle PTY exit.
pub fn handle_pty_exited(state: &mut Nexus, block_id: BlockId, exit_code: i32) -> Task<Message> {
    let terminal = &mut state.terminal;
    let mut show_permission_prompt = false;
    let mut failed_command = None;

    if let Some(&idx) = terminal.block_index.get(&block_id) {
        if let Some(block) = terminal.blocks.get_mut(idx) {
            block.state = if exit_code == 0 {
                BlockState::Success
            } else {
                BlockState::Failed(exit_code)
            };
            block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
            block.version += 1;
            if exit_code != 0 && block.has_permission_denied {
                show_permission_prompt = true;
                failed_command = Some(block.command.clone());
            }
        }
    }
    terminal.pty_handles.retain(|h| h.block_id != block_id);
    terminal.last_exit_code = Some(exit_code);

    if show_permission_prompt {
        terminal.permission_denied_command = failed_command;
    }

    if terminal.focus == Focus::Block(block_id) {
        terminal.focus = Focus::Input;
        return focus_input();
    }

    Task::none()
}

/// Return a Task that focuses the main input field.
fn focus_input() -> Task<Message> {
    text_input::focus(text_input::Id::new(INPUT_FIELD))
}

/// Handle key press when a PTY block is focused.
pub fn handle_key(state: &mut Nexus, key: Key, modifiers: Modifiers) -> Task<Message> {
    let terminal = &mut state.terminal;

    if let Focus::Block(block_id) = terminal.focus {
        if let Some(handle) = terminal.pty_handles.iter().find(|h| h.block_id == block_id) {
            // Handle Ctrl+C/D/Z
            if modifiers.control() {
                if let Key::Character(c) = &key {
                    match c.to_lowercase().as_str() {
                        "c" => {
                            let _ = handle.send_interrupt();
                            return Task::none();
                        }
                        "d" => {
                            let _ = handle.send_eof();
                            return Task::none();
                        }
                        "z" => {
                            let _ = handle.send_suspend();
                            return Task::none();
                        }
                        _ => {}
                    }
                }
            }

            // All keys (including Escape) go to running PTY - let vim/etc handle them
            // User must exit the program normally, or use Ctrl+Shift+Escape to force-exit
            if let Some(bytes) = key_to_bytes(&key, &modifiers) {
                let _ = handle.write(&bytes);
            }
        } else {
            // PTY handle not found (finished block) - Escape returns to input
            if matches!(key, Key::Named(keyboard::key::Named::Escape)) {
                terminal.focus = Focus::Input;
                return focus_input();
            }
        }
    }
    Task::none()
}

// =============================================================================
// Kernel Event Handler
// =============================================================================

/// Handle kernel events from pipeline/native command execution.
pub fn handle_kernel_event(state: &mut Nexus, shell_event: ShellEvent) -> Task<Message> {
    let terminal = &mut state.terminal;

    match shell_event {
        ShellEvent::CommandStarted {
            block_id,
            command,
            cwd: _,
        } => {
            if !terminal.block_index.contains_key(&block_id) {
                let mut block = Block::new(block_id, command);
                block.parser = TerminalParser::new(terminal.terminal_size.0, terminal.terminal_size.1);
                let block_idx = terminal.blocks.len();
                terminal.blocks.push(block);
                terminal.block_index.insert(block_id, block_idx);
            }
        }
        ShellEvent::StdoutChunk { block_id, data } => {
            if let Some(&idx) = terminal.block_index.get(&block_id) {
                if let Some(block) = terminal.blocks.get_mut(idx) {
                    block.parser.feed(&data);
                    block.version += 1;
                }
            }
            terminal.is_dirty = true;
        }
        ShellEvent::StderrChunk { block_id, data } => {
            if let Some(&idx) = terminal.block_index.get(&block_id) {
                if let Some(block) = terminal.blocks.get_mut(idx) {
                    block.parser.feed(&data);
                    block.version += 1;
                    if !block.has_permission_denied {
                        if let Ok(text) = std::str::from_utf8(&data) {
                            if text.to_lowercase().contains("permission denied") {
                                block.has_permission_denied = true;
                            }
                        }
                    }
                }
            }
            terminal.is_dirty = true;
        }
        ShellEvent::CommandOutput { block_id, value } => {
            if let Some(&idx) = terminal.block_index.get(&block_id) {
                if let Some(block) = terminal.blocks.get_mut(idx) {
                    block.native_output = Some(value);
                }
            }
        }
        ShellEvent::CommandFinished {
            block_id,
            exit_code,
            duration_ms,
        } => {
            let mut show_permission_prompt = false;
            let mut failed_command = None;
            if let Some(&idx) = terminal.block_index.get(&block_id) {
                if let Some(block) = terminal.blocks.get_mut(idx) {
                    block.state = if exit_code == 0 {
                        BlockState::Success
                    } else {
                        BlockState::Failed(exit_code)
                    };
                    block.duration_ms = Some(duration_ms);
                    block.version += 1;
                    if exit_code != 0 && block.has_permission_denied {
                        show_permission_prompt = true;
                        failed_command = Some(block.command.clone());
                    }
                }
            }
            terminal.last_exit_code = Some(exit_code);
            terminal.focus = Focus::Input;

            if show_permission_prompt {
                terminal.permission_denied_command = failed_command;
            }

            return Task::batch([
                focus_input(),
                scrollable::snap_to(
                    scrollable::Id::new(HISTORY_SCROLLABLE),
                    scrollable::RelativeOffset::END,
                ),
            ]);
        }
        ShellEvent::OpenClaudePanel { .. } => {}
        ShellEvent::JobStateChanged { job_id, state: job_state } => {
            match job_state {
                nexus_api::JobState::Running => {
                    if let Some(job) = terminal.jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = VisualJobState::Running;
                    } else {
                        terminal.jobs.push(VisualJob::new(
                            job_id,
                            format!("Job {}", job_id),
                            VisualJobState::Running,
                        ));
                    }
                }
                nexus_api::JobState::Stopped => {
                    if let Some(job) = terminal.jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = VisualJobState::Stopped;
                    } else {
                        terminal.jobs.push(VisualJob::new(
                            job_id,
                            format!("Job {}", job_id),
                            VisualJobState::Stopped,
                        ));
                    }
                }
                nexus_api::JobState::Done(_) => {
                    terminal.jobs.retain(|j| j.id != job_id);
                }
            }
        }
        _ => {}
    }
    Task::none()
}

// =============================================================================
// Table & UI Handlers
// =============================================================================

/// Sort a table by column.
pub fn sort_table(state: &mut Nexus, block_id: BlockId, column_index: usize) -> Task<Message> {
    if let Some(&idx) = state.terminal.block_index.get(&block_id) {
        if let Some(block) = state.terminal.blocks.get_mut(idx) {
            block.table_sort.toggle(column_index);
            block.version += 1;
        }
    }
    Task::none()
}

// =============================================================================
// Command Execution Helpers
// =============================================================================

/// Retry last failed command with sudo.
pub fn retry_sudo(state: &mut Nexus) -> Task<Message> {
    if let Some(cmd) = state.terminal.permission_denied_command.take() {
        let sudo_cmd = format!("sudo {}", cmd);
        return execute(state, sudo_cmd);
    }
    Task::none()
}

/// Dismiss the permission denied prompt.
pub fn dismiss_permission(state: &mut Nexus) -> Task<Message> {
    state.terminal.permission_denied_command = None;
    Task::none()
}

/// Dismiss the command not found prompt.
pub fn dismiss_not_found(state: &mut Nexus) -> Task<Message> {
    state.terminal.command_not_found = None;
    Task::none()
}

/// Bring a background job to foreground.
pub fn foreground_job(state: &mut Nexus, job_id: u32) -> Task<Message> {
    let command = format!("fg %{}", job_id);
    state.input.buffer.clear();
    execute(state, command)
}

/// Execute a shell command.
pub fn execute(state: &mut Nexus, command: String) -> Task<Message> {
    let trimmed = command.trim().to_string();

    // History is already recorded in submit(), just reset navigation state
    state.input.shell_history_index = None;
    state.input.agent_history_index = None;
    state.input.saved_input.clear();

    let block_id = state.terminal.next_id();

    // Handle built-in: clear
    if trimmed == "clear" {
        use std::sync::atomic::Ordering;
        state.agent.cancel_flag.store(true, Ordering::SeqCst);
        state.terminal.blocks.clear();
        state.terminal.block_index.clear();
        state.agent.blocks.clear();
        state.agent.block_index.clear();
        state.agent.active_block = None;
        return Task::none();
    }

    // Handle built-in: cd
    if command.trim().starts_with("cd ") {
        let path = command.trim().strip_prefix("cd ").unwrap().trim();
        let new_path = if path.starts_with('/') {
            std::path::PathBuf::from(path)
        } else if path == "~" {
            home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"))
        } else {
            std::path::PathBuf::from(&state.terminal.cwd).join(path)
        };

        if let Ok(canonical) = new_path.canonicalize() {
            if canonical.is_dir() {
                state.terminal.cwd = canonical.display().to_string();
                let _ = std::env::set_current_dir(&canonical);
            }
        }
        return Task::none();
    }

    // Execute through kernel or PTY
    execute_kernel(state, block_id, command)
}

/// Execute a command through the kernel (pipeline or native).
pub fn execute_kernel(state: &mut Nexus, block_id: BlockId, command: String) -> Task<Message> {
    let terminal = &mut state.terminal;
    let has_pipe = command.contains('|');
    let first_word = command.split_whitespace().next().unwrap_or("");
    let is_native = terminal.commands.contains(first_word);

    if has_pipe || is_native {
        // Create block for kernel command
        let mut block = Block::new(block_id, command.clone());
        block.parser = TerminalParser::new(terminal.terminal_size.0, terminal.terminal_size.1);
        let block_idx = terminal.blocks.len();
        terminal.blocks.push(block);
        terminal.block_index.insert(block_id, block_idx);

        // Pipeline/native execution via kernel
        let kernel = terminal.kernel.clone();
        let cwd = terminal.cwd.clone();
        let cmd = command.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut kernel = kernel.lock().await;
                let _ = kernel
                    .state_mut()
                    .set_cwd(std::path::PathBuf::from(&cwd));
                let _ = kernel.execute_with_block_id(&cmd, Some(block_id));
            });
        });

        return scrollable::snap_to(
            scrollable::Id::new(HISTORY_SCROLLABLE),
            scrollable::RelativeOffset::END,
        );
    }

    // Single external command - use PTY
    let mut block = Block::new(block_id, command.clone());
    block.parser = TerminalParser::new(terminal.terminal_size.0, terminal.terminal_size.1);
    let block_idx = terminal.blocks.len();
    terminal.blocks.push(block);
    terminal.block_index.insert(block_id, block_idx);

    terminal.focus = Focus::Block(block_id);

    let tx = terminal.pty_tx.clone();
    let cwd = terminal.cwd.clone();
    let (cols, rows) = terminal.terminal_size;

    match PtyHandle::spawn_with_size(&command, &cwd, block_id, tx, cols, rows) {
        Ok(handle) => {
            terminal.pty_handles.push(handle);
            // Blur text input so PTY gets keyboard events
            return Task::batch([
                iced::widget::focus_next(),
                scrollable::snap_to(
                    scrollable::Id::new(HISTORY_SCROLLABLE),
                    scrollable::RelativeOffset::END,
                ),
            ]);
        }
        Err(e) => {
            tracing::error!("Failed to spawn PTY: {}", e);
            if let Some(&idx) = terminal.block_index.get(&block_id) {
                if let Some(block) = terminal.blocks.get_mut(idx) {
                    block.state = BlockState::Failed(1);
                    block.parser.feed(format!("Error: {}\n", e).as_bytes());
                    block.version += 1;
                }
            }
            terminal.focus = Focus::Input;
            return focus_input();
        }
    }
}
