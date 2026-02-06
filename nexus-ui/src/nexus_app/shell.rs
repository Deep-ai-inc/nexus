//! Shell widget — owns terminal blocks, PTY handles, jobs, and image handles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

use nexus_api::{BlockId, BlockState, DomainValue, ShellEvent, Value};
use nexus_kernel::{CommandClassification, Kernel};

use crate::blocks::{Block, PtyEvent, VisualJob, VisualJobState};
use crate::systems::{kernel_subscription, pty_subscription};
use strata::{ImageHandle, ImageStore, Subscription};
use strata::content_address::SourceId;

use crate::blocks::Focus;
use crate::nexus_widgets::{JobBar, ShellBlockWidget};

use super::pty_backend::PtyBackend;

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::message::{AnchorAction, ContextMenuMsg, NexusMessage, ShellMsg};
use super::source_ids;

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
        block_id: BlockId,
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
    /// A directory was expanded — orchestrator should load its children.
    LoadTreeChildren(BlockId, PathBuf),
}

impl Default for ShellOutput {
    fn default() -> Self { Self::None }
}

/// An anchor entry resolved during rendering — stores both the click action
/// and drag payload so click/drag handling is an O(1) HashMap lookup.
///
/// Populated during `view()` (the single source of truth), read during
/// click and drag handling.
#[derive(Debug, Clone)]
pub(crate) struct AnchorEntry {
    pub block_id: BlockId,
    pub action: AnchorAction,
    pub drag_payload: super::drag_state::DragPayload,
}

/// Unified click action — resolved during rendering, dispatched on click.
#[derive(Debug, Clone)]
pub(crate) enum ClickAction {
    Anchor(AnchorEntry),
    TreeToggle { block_id: BlockId, path: PathBuf },
}

/// Register an anchor click action in the click registry.
pub(crate) fn register_anchor(
    registry: &RefCell<HashMap<SourceId, ClickAction>>,
    id: SourceId,
    entry: AnchorEntry,
) {
    registry.borrow_mut().insert(id, ClickAction::Anchor(entry));
}

/// Register a tree-toggle click action in the click registry.
pub(crate) fn register_tree_toggle(
    registry: &RefCell<HashMap<SourceId, ClickAction>>,
    id: SourceId,
    block_id: BlockId,
    path: PathBuf,
) {
    registry.borrow_mut().insert(id, ClickAction::TreeToggle { block_id, path });
}

/// Manages all shell-related state: terminal blocks, PTY handles, jobs, images.
pub(crate) struct ShellWidget {
    pub blocks: Vec<Block>,
    pub block_index: HashMap<BlockId, usize>,
    pub(crate) pty: PtyBackend,
    pub terminal_dirty: bool,
    pub last_exit_code: Option<i32>,
    pub image_handles: HashMap<BlockId, (ImageHandle, u32, u32)>,
    pub jobs: Vec<VisualJob>,

    /// Unified click registry — populated during view(), read during click/drag handling.
    /// Keyed by SourceId, provides O(1) lookup for anchors and tree toggles.
    pub(crate) click_registry: RefCell<HashMap<SourceId, ClickAction>>,

    // --- Subscription channel (kernel events) ---
    kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
}

impl ShellWidget {
    pub fn new(
        kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
    ) -> Self {
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            pty: PtyBackend::new(),
            terminal_dirty: false,
            last_exit_code: None,
            image_handles: HashMap::new(),
            jobs: Vec::new(),
            click_registry: RefCell::new(HashMap::new()),
            kernel_rx,
        }
    }

    /// Whether the shell has pending output that needs a redraw tick.
    pub fn needs_redraw(&self) -> bool {
        self.terminal_dirty
    }

    /// Clear the click registry. Called at the start of each view() pass
    /// before blocks re-populate it.
    pub fn clear_click_registry(&self) {
        self.click_registry.borrow_mut().clear();
    }

    // ---- View contributions ----

    /// Push a single shell block into the given scroll column.
    pub fn push_block<'a>(
        &'a self,
        scroll: strata::ScrollColumn<'a>,
        block: &'a Block,
        focus: &Focus,
    ) -> strata::ScrollColumn<'a> {
        let is_focused = matches!(focus, Focus::Block(id) if *id == block.id);
        scroll.push(ShellBlockWidget {
            block,
            kill_id: source_ids::kill(block.id),
            image_info: self.image_handles.get(&block.id).copied(),
            is_focused,
            click_registry: &self.click_registry,
        })
    }

    /// Build the job bar widget, if any jobs exist.
    pub fn view_job_bar(&self) -> Option<JobBar<'_>> {
        if self.jobs.is_empty() {
            None
        } else {
            Some(JobBar { jobs: &self.jobs })
        }
    }

    // ---- Event handling ----

    /// Handle a widget click within shell-owned UI. Returns None if not our widget.
    pub fn on_click(&self, id: SourceId) -> Option<ShellMsg> {
        // Kill buttons
        for block in &self.blocks {
            if block.is_running() && id == source_ids::kill(block.id) {
                return Some(ShellMsg::KillBlock(block.id));
            }
        }
        // Table sort headers (check both native_output and stream_latest)
        for block in &self.blocks {
            let tables = [&block.native_output, &block.stream_latest];
            for table in &tables {
                if let Some(Value::Table { columns, .. }) = table {
                    for col_idx in 0..columns.len() {
                        if id == source_ids::table_sort(block.id, col_idx) {
                            return Some(ShellMsg::SortTable(block.id, col_idx));
                        }
                    }
                }
            }
        }
        // Unified click registry (anchors, tree toggles, etc.)
        match self.click_registry.borrow().get(&id)? {
            ClickAction::TreeToggle { block_id, path } => {
                Some(ShellMsg::ToggleTreeExpand(*block_id, path.clone()))
            }
            _ => None, // Anchors handled via drag intent path
        }
    }

    /// Look up a block by ID (immutable).
    pub fn block_by_id(&self, id: BlockId) -> Option<&Block> {
        self.block_index.get(&id).and_then(|&idx| self.blocks.get(idx))
    }

    /// Look up a block by ID (mutable).
    pub fn block_by_id_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        if let Some(&idx) = self.block_index.get(&id) {
            self.blocks.get_mut(idx)
        } else {
            None
        }
    }

    /// Find the most recent block with an active viewer (e.g. top, less, tree).
    /// Used as a fallback when focus is Input but a viewer is still running.
    pub fn active_viewer_block(&self) -> Option<BlockId> {
        self.blocks.iter().rev()
            .find(|b| b.view_state.is_some())
            .map(|b| b.id)
    }

    /// The block that should receive an interrupt (Ctrl+C).
    /// Prefers the focused block if provided, otherwise the last running block.
    pub fn interrupt_target(&self, focused: Option<BlockId>) -> Option<BlockId> {
        if let Some(id) = focused {
            if self.pty.has_handle(id) {
                return Some(id);
            }
        }
        self.blocks.iter().rev().find(|b| b.is_running()).map(|b| b.id)
    }

    /// Build a context menu for a right-click on shell content.
    pub fn context_menu_for_source(
        &self,
        source_id: SourceId,
        x: f32,
        y: f32,
    ) -> Option<ContextMenuMsg> {
        let block_id = self.block_for_source(source_id)?;
        let block = self.block_index.get(&block_id)
            .and_then(|&idx| self.blocks.get(idx));

        let mut items = vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll];

        if let Some(block) = block {
            // Offer CopyCommand for any block with a command
            if !block.command.is_empty() {
                items.push(ContextMenuItem::CopyCommand);
            }
            // Offer structured export for table output
            if let Some(Value::Table { .. }) = &block.native_output {
                items.push(ContextMenuItem::CopyAsTsv);
                items.push(ContextMenuItem::CopyAsJson);
            }
            // Offer CopyOutput for finished blocks
            if !block.is_running() {
                items.push(ContextMenuItem::CopyOutput);
            }
            // Offer Rerun for finished blocks
            if !block.is_running() && !block.command.is_empty() {
                items.push(ContextMenuItem::Rerun);
            }
        }

        Some(ContextMenuMsg::Show(x, y, items, ContextTarget::Block(block_id)))
    }

    /// Build a fallback context menu (last block) for right-click on empty area.
    pub fn fallback_context_menu(&self, x: f32, y: f32) -> Option<ContextMenuMsg> {
        let block = self.blocks.last()?;
        Some(ContextMenuMsg::Show(
            x, y,
            vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
            ContextTarget::Block(block.id),
        ))
    }

    /// Handle a click on an anchor widget. Returns None if not an anchor we own.
    /// Look up an anchor by SourceId in the unified click registry (O(1)).
    pub fn on_click_anchor(&self, id: SourceId) -> Option<ShellMsg> {
        let registry = self.click_registry.borrow();
        match registry.get(&id)? {
            ClickAction::Anchor(entry) => Some(ShellMsg::OpenAnchor(entry.block_id, entry.action.clone())),
            _ => None,
        }
    }

    /// Look up a drag payload by SourceId in the unified click registry (O(1)).
    pub fn drag_payload_for_anchor(&self, id: SourceId) -> Option<super::drag_state::DragPayload> {
        let registry = self.click_registry.borrow();
        match registry.get(&id)? {
            ClickAction::Anchor(entry) => Some(entry.drag_payload.clone()),
            _ => None,
        }
    }

    /// If a source belongs to a block with image output, return an Image drag payload.
    pub fn image_drag_payload(&self, source_id: strata::content_address::SourceId) -> Option<super::drag_state::DragPayload> {
        for block in &self.blocks {
            if source_id == source_ids::native(block.id)
                || source_id == source_ids::image_output(block.id)
            {
                if let Some(ref value) = block.native_output {
                    if let Some((data, content_type, metadata)) = value.as_media() {
                        if content_type.starts_with("image/") {
                            let ext = match content_type {
                                "image/png" => "png",
                                "image/jpeg" | "image/jpg" => "jpg",
                                "image/gif" => "gif",
                                "image/webp" => "webp",
                                "image/svg+xml" => "svg",
                                _ => "png",
                            };
                            let filename = metadata.filename.clone()
                                .unwrap_or_else(|| format!("image-{}.{}", block.id.0, ext));
                            return Some(super::drag_state::DragPayload::Image {
                                data: data.to_vec(),
                                filename,
                            });
                        }
                    }
                }
            }
        }
        None
    }

    /// Check if a hit address belongs to a shell block. Returns the block_id if so.
    pub fn block_for_source(&self, source_id: SourceId) -> Option<BlockId> {
        for block in &self.blocks {
            let id = block.id;
            if source_id == source_ids::block_container(id)
                || source_id == source_ids::shell_term(id)
                || source_id == source_ids::shell_header(id)
                || source_id == source_ids::native(id)
                || source_id == source_ids::table(id)
                || source_id == source_ids::image_output(id)
                || source_id == source_ids::kill(id)
                || source_id == source_ids::viewer_exit(id)
            {
                return Some(id);
            }
        }
        None
    }

    /// Get display text for a table row (tab-separated cell values).
    pub fn row_display_text(&self, block_id: nexus_api::BlockId, row_index: usize) -> String {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get(idx) {
                if let Some(nexus_api::Value::Table { rows, .. }) = &block.native_output {
                    if let Some(row) = rows.get(row_index) {
                        return row.iter().map(|v| v.to_text()).collect::<Vec<_>>().join("\t");
                    }
                }
            }
        }
        String::new()
    }

    /// Create the subscription for PTY and kernel events.
    ///
    /// Returns `Subscription<NexusMessage>` directly.
    pub fn subscription(&self) -> Subscription<NexusMessage> {
        let mut subs = Vec::new();

        let pty_rx = self.pty.rx.clone();
        subs.push(
            pty_subscription(pty_rx).map(|batch| {
                NexusMessage::Shell(ShellMsg::PtyBatch(batch))
            }),
        );

        let kernel_rx = self.kernel_rx.clone();
        subs.push(
            kernel_subscription(kernel_rx).map(|evt| NexusMessage::Shell(ShellMsg::KernelEvent(evt))),
        );

        Subscription::batch(subs)
    }

    /// Handle a message, returning commands and cross-cutting output.
    pub fn update(&mut self, msg: ShellMsg, ctx: &mut strata::component::Ctx) -> (strata::Command<ShellMsg>, ShellOutput) {
        let output = match msg {
            ShellMsg::PtyBatch(batch) => self.handle_pty_batch(batch),
            ShellMsg::PtyOutput(id, data) => self.handle_pty_output(id, data),
            ShellMsg::PtyExited(id, exit_code) => self.handle_pty_exited(id, exit_code),
            ShellMsg::KernelEvent(evt) => self.handle_kernel_event(evt, ctx.images),
            ShellMsg::SendInterrupt(id) => { self.pty.send_interrupt(id); ShellOutput::None }
            ShellMsg::KillBlock(id) => {
                self.pty.kill(id);
                // Also cancel kernel-native commands (e.g. top) which have
                // no PTY handle — they use a cancel flag instead.
                nexus_kernel::commands::cancel_block(id);
                if let Some(block) = self.block_by_id_mut(id).filter(|b| b.view_state.is_some()) {
                    block.view_state = None;
                    block.version += 1;
                }
                ShellOutput::None
            }
            ShellMsg::PtyInput(block_id, event) => {
                let block = self.block_by_id(block_id);
                if self.pty.forward_key(block, block_id, &event) {
                    ShellOutput::None
                } else {
                    ShellOutput::PtyInputFailed
                }
            }
            ShellMsg::SortTable(block_id, col_idx) => { self.sort_table(block_id, col_idx); ShellOutput::None }
            ShellMsg::OpenAnchor(_, _) => {
                // Handled at the root level in state_update.rs
                ShellOutput::None
            }
            ShellMsg::ToggleTreeExpand(block_id, path) => {
                self.toggle_tree_expand(block_id, path)
            }
            ShellMsg::TreeChildrenLoaded(block_id, path, entries) => {
                self.set_tree_children(block_id, path, entries);
                ShellOutput::None
            }
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

    /// Handle a batch of PTY events. Coalesces consecutive Output events
    /// for the same block into a single `feed()` call, preserving ordering
    /// relative to Exited events.
    pub fn handle_pty_batch(&mut self, batch: Vec<(BlockId, PtyEvent)>) -> ShellOutput {
        // Coalesce: merge consecutive Output(data) for the same block.
        // When we hit an Exited or a different block, flush the accumulator.
        let mut acc_id: Option<BlockId> = None;
        let mut acc_data: Vec<u8> = Vec::new();
        let mut last_output = ShellOutput::None;

        let flush = |acc_id: &mut Option<BlockId>,
                     acc_data: &mut Vec<u8>,
                     blocks: &mut Vec<Block>,
                     block_index: &HashMap<BlockId, usize>| {
            if let Some(id) = acc_id.take() {
                if !acc_data.is_empty() {
                    if let Some(&idx) = block_index.get(&id) {
                        if let Some(block) = blocks.get_mut(idx) {
                            block.parser.feed(acc_data);
                            if let Some(title) = block.parser.take_title() {
                                block.osc_title = Some(title);
                            }
                            block.version += 1;
                        }
                    }
                    acc_data.clear();
                }
            }
        };

        for (id, evt) in batch {
            match evt {
                PtyEvent::Output(data) => {
                    if acc_id == Some(id) {
                        // Same block — just append.
                        acc_data.extend_from_slice(&data);
                    } else {
                        // Different block — flush previous, start new accumulator.
                        flush(
                            &mut acc_id,
                            &mut acc_data,
                            &mut self.blocks,
                            &self.block_index,
                        );
                        acc_id = Some(id);
                        acc_data = data;
                    }
                }
                PtyEvent::Exited(code) => {
                    // Flush any pending output for this or previous block first.
                    flush(
                        &mut acc_id,
                        &mut acc_data,
                        &mut self.blocks,
                        &self.block_index,
                    );
                    last_output = self.handle_pty_exited(id, code);
                }
            }
        }

        // Flush remaining accumulated output.
        if let Some(id) = acc_id {
            if !acc_data.is_empty() {
                if let Some(&idx) = self.block_index.get(&id) {
                    if let Some(block) = self.blocks.get_mut(idx) {
                        block.parser.feed(&acc_data);
                        if let Some(title) = block.parser.take_title() {
                            block.osc_title = Some(title);
                        }
                        block.version += 1;
                    }
                }
            }
        }

        // Don't set terminal_dirty here — the batch message itself triggers
        // a render via the iced adapter (every App message bumps frame).
        // Setting dirty would activate the 16ms tick, which fires yet
        // another redundant render with no new content.

        // If the last event was an exit, propagate that; otherwise scroll.
        match last_output {
            ShellOutput::None => ShellOutput::ScrollToBottom,
            other => other,
        }
    }

    /// Handle a single PTY output event (unbatched fallback).
    pub fn handle_pty_output(&mut self, id: BlockId, data: Vec<u8>) -> ShellOutput {
        if let Some(&idx) = self.block_index.get(&id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.parser.feed(&data);
                if let Some(title) = block.parser.take_title() {
                    block.osc_title = Some(title);
                }
                block.version += 1;
            }
        }
        self.terminal_dirty = true;
        ShellOutput::ScrollToBottom
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
        self.pty.remove_handle(id);
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
                    block.parser = self.pty.new_parser();
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
                // Handle Interactive values: set up viewer state
                if let Some(DomainValue::Interactive(req)) = value.as_domain() {
                    let content = req.content.clone();
                    let is_monitor = matches!(req.viewer, nexus_api::ViewerKind::ProcessMonitor { .. });
                    let view_state = match &req.viewer {
                        nexus_api::ViewerKind::Pager | nexus_api::ViewerKind::ManPage => {
                            Some(crate::blocks::ViewState::Pager {
                                scroll_line: 0,
                                search: None,
                                current_match: 0,
                            })
                        }
                        nexus_api::ViewerKind::ProcessMonitor { interval_ms } => {
                            Some(crate::blocks::ViewState::ProcessMonitor {
                                sort_by: crate::blocks::ProcSort::Cpu,
                                sort_desc: true,
                                interval_ms: *interval_ms,
                            })
                        }
                        nexus_api::ViewerKind::TreeBrowser => {
                            Some(crate::blocks::ViewState::TreeBrowser {
                                collapsed: std::collections::HashSet::new(),
                                selected: Some(0),
                            })
                        }
                        nexus_api::ViewerKind::DiffViewer => {
                            Some(crate::blocks::ViewState::DiffViewer {
                                scroll_line: 0,
                                current_file: 0,
                                collapsed_indices: std::collections::HashSet::new(),
                            })
                        }
                    };
                    if let Some(&idx) = self.block_index.get(&block_id) {
                        if let Some(block) = self.blocks.get_mut(idx) {
                            block.native_output = Some(content);
                            block.view_state = view_state;
                            // Default sort for ProcessMonitor: %CPU (index 2) descending
                            if is_monitor {
                                block.table_sort = crate::blocks::TableSort {
                                    column: Some(2),
                                    ascending: false,
                                };
                                // Sort initial data
                                if let Some(Value::Table { ref mut rows, .. }) = block.native_output {
                                    Self::sort_rows(rows, 2, false);
                                }
                            }
                        }
                    }
                    return ShellOutput::FocusBlock(block_id);
                }

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
                // Auto-enable frame timing for large tables (performance debugging)
                if let Value::Table { ref rows, .. } = value {
                    if rows.len() > 100 {
                        strata::frame_timing::enable();
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
                    block_id,
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
            ShellEvent::StreamingUpdate {
                block_id,
                seq,
                update,
                coalesce,
            } => {
                if let Some(&idx) = self.block_index.get(&block_id) {
                    if let Some(block) = self.blocks.get_mut(idx) {
                        if seq > block.stream_seq {
                            block.stream_seq = seq;
                            if coalesce {
                                block.stream_latest = Some(update);
                                // Re-apply current table sort to new data
                                if let Some(col_idx) = block.table_sort.column {
                                    let ascending = block.table_sort.ascending;
                                    if let Some(Value::Table { ref mut rows, .. }) = block.stream_latest {
                                        Self::sort_rows(rows, col_idx, ascending);
                                    }
                                }
                            } else {
                                block.stream_log.push_back(update);
                                while block.stream_log.len() > 1000 {
                                    block.stream_log.pop_front();
                                }
                            }
                            block.version += 1;
                        }
                    }
                }
                ShellOutput::None
            }
            ShellEvent::CwdChanged { new, .. } => ShellOutput::CwdChanged(new),
            _ => ShellOutput::None,
        }
    }

    /// Paste text into a PTY, respecting Bracketed Paste mode.
    pub fn paste_to_pty(&self, block_id: BlockId, text: &str) -> bool {
        let block = self.block_by_id(block_id);
        self.pty.paste_to_pty(block, block_id, text)
    }

    /// Sort a table by column (works on both native_output and stream_latest).
    pub fn sort_table(&mut self, block_id: BlockId, col_idx: usize) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.table_sort.toggle(col_idx);
                let ascending = block.table_sort.ascending;
                // Sort whichever table is present (native_output or stream_latest)
                if let Some(Value::Table { ref mut rows, .. }) = block.native_output {
                    Self::sort_rows(rows, col_idx, ascending);
                }
                if let Some(Value::Table { ref mut rows, .. }) = block.stream_latest {
                    Self::sort_rows(rows, col_idx, ascending);
                }
            }
        }
    }

    /// Sort table rows by a column index.
    pub(super) fn sort_rows(rows: &mut [Vec<Value>], col_idx: usize, ascending: bool) {
        rows.sort_by(|a, b| {
            let va = a.get(col_idx).map(|v| v.to_text()).unwrap_or_default();
            let vb = b.get(col_idx).map(|v| v.to_text()).unwrap_or_default();
            if let (Ok(na), Ok(nb)) = (va.parse::<f64>(), vb.parse::<f64>()) {
                let cmp = na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal);
                if ascending { cmp } else { cmp.reverse() }
            } else {
                let cmp = va.cmp(&vb);
                if ascending { cmp } else { cmp.reverse() }
            }
        });
    }

    /// Toggle tree expansion for a directory.
    /// Returns LoadTreeChildren if the directory was expanded and needs loading.
    pub fn toggle_tree_expand(&mut self, block_id: BlockId, path: PathBuf) -> ShellOutput {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                let tree = block.ensure_file_tree();
                let now_expanded = tree.toggle(path.clone());
                block.version += 1; // Trigger re-render
                if now_expanded && !block.ensure_file_tree().children.contains_key(&path) {
                    return ShellOutput::LoadTreeChildren(block_id, path);
                }
            }
        }
        ShellOutput::None
    }

    /// Store loaded children for a tree node.
    pub fn set_tree_children(&mut self, block_id: BlockId, path: PathBuf, entries: Vec<nexus_api::FileEntry>) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.ensure_file_tree().set_children(path, entries);
                block.version += 1; // Trigger re-render
            }
        }
    }

    /// Clear all blocks, kill PTYs, cancel kernel commands, clear jobs.
    pub fn clear(&mut self) {
        // Cancel any running kernel commands (e.g. `top`) so they release
        // the kernel mutex.  Must happen before clearing blocks.
        for block in &self.blocks {
            nexus_kernel::commands::cancel_block(block.id);
        }
        self.pty.kill_all();
        self.blocks.clear();
        self.block_index.clear();
        self.jobs.clear();
    }

    /// Propagate terminal size changes to block parsers (delegates to PtyBackend).
    pub fn sync_terminal_size(&mut self) {
        self.pty.sync_terminal_size(&mut self.blocks);
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
        block.parser = self.pty.new_parser();
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
        block.parser = self.pty.new_parser();
        let block_idx = self.blocks.len();
        self.blocks.push(block);
        self.block_index.insert(block_id, block_idx);

        match self.pty.spawn(&cmd, block_id, cwd) {
            Ok(()) => ShellOutput::FocusBlock(block_id),
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

/// Convert a structured Value to the appropriate anchor action.
/// Extract the most semantically useful text for drag-and-drop from a value.
/// For file entries, returns the path. For processes, returns the PID.
/// For git commits, returns the hash. Falls back to `to_text()`.
pub(crate) fn semantic_text_for_value(value: &Value, _column: Option<&nexus_api::TableColumn>) -> String {
    match value {
        Value::Path(p) => super::file_drop::shell_quote(p),
        Value::FileEntry(entry) => super::file_drop::shell_quote(&entry.path),
        Value::Process(info) => info.pid.to_string(),
        Value::GitCommit(info) => info.short_hash.clone(),
        _ => value.to_text(),
    }
}

pub(crate) fn value_to_anchor_action(value: &Value) -> AnchorAction {
    match value {
        Value::Path(p) => AnchorAction::QuickLook(p.clone()),
        Value::FileEntry(entry) => AnchorAction::QuickLook(entry.path.clone()),
        Value::Process(info) => AnchorAction::CopyToClipboard(info.pid.to_string()),
        Value::GitCommit(info) => AnchorAction::CopyToClipboard(info.short_hash.clone()),
        _ => AnchorAction::CopyToClipboard(value.to_text()),
    }
}
