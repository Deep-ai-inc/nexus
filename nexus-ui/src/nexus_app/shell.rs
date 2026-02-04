//! Shell widget — owns terminal blocks, PTY handles, jobs, and image handles.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::{BlockId, BlockState, DomainValue, ShellEvent, Value};
use nexus_kernel::{CommandClassification, Kernel};
use nexus_term::TerminalParser;

use crate::blocks::{Block, PtyEvent, VisualJob, VisualJobState};
use crate::pty::PtyHandle;
use crate::systems::{kernel_subscription, pty_subscription};
use strata::{ImageHandle, ImageStore, Subscription};
use strata::content_address::SourceId;
use strata::event_context::{Key, KeyEvent, NamedKey};

use crate::blocks::Focus;
use crate::nexus_widgets::{JobBar, ShellBlockWidget};

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
    pub pty_handles: Vec<PtyHandle>,
    pub pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    pub terminal_size: Cell<(u16, u16)>,
    /// Last size committed to all block parsers.
    last_parser_size: Cell<(u16, u16)>,
    /// Last size sent to PTY handles (avoids redundant SIGWINCH).
    last_pty_size: Cell<(u16, u16)>,
    /// Pending column downsize: `(target_size, first_seen)`. The timer
    /// restarts whenever the target changes, so the reflow only commits
    /// once the size has been stable for the debounce window.
    pending_downsize: Cell<Option<((u16, u16), Instant)>>,
    pub terminal_dirty: bool,
    pub last_exit_code: Option<i32>,
    pub image_handles: HashMap<BlockId, (ImageHandle, u32, u32)>,
    pub jobs: Vec<VisualJob>,

    /// Unified click registry — populated during view(), read during click/drag handling.
    /// Keyed by SourceId, provides O(1) lookup for anchors and tree toggles.
    pub(crate) click_registry: RefCell<HashMap<SourceId, ClickAction>>,

    // --- Subscription channels (owned by this widget) ---
    pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
}

impl ShellWidget {
    pub fn new(
        kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
    ) -> Self {
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            pty_handles: Vec::new(),
            pty_tx,
            terminal_size: Cell::new((120, 24)),
            last_parser_size: Cell::new((120, 24)),
            last_pty_size: Cell::new((120, 24)),
            pending_downsize: Cell::new(None),
            terminal_dirty: false,
            last_exit_code: None,
            image_handles: HashMap::new(),
            jobs: Vec::new(),
            click_registry: RefCell::new(HashMap::new()),
            pty_rx: Arc::new(Mutex::new(pty_rx)),
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
    pub fn push_block(
        &self,
        scroll: strata::ScrollColumn,
        block: &Block,
        focus: &Focus,
    ) -> strata::ScrollColumn {
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
            if self.pty_handles.iter().any(|h| h.block_id == id) {
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
    /// Returns `Subscription<NexusMessage>` directly because iced's
    /// `Subscription::map` panics on capturing closures, so we can't
    /// return `Subscription<ShellMsg>` and `map_msg` at the root.
    pub fn subscription(&self) -> Subscription<NexusMessage> {
        let mut subs = Vec::new();

        let pty_rx = self.pty_rx.clone();
        subs.push(Subscription::from_iced(
            pty_subscription(pty_rx).map(|batch| {
                NexusMessage::Shell(ShellMsg::PtyBatch(batch))
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
            ShellMsg::PtyBatch(batch) => self.handle_pty_batch(batch),
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
            let flags = self
                .block_by_id(block_id)
                .map(|b| TermKeyFlags { app_cursor: b.parser.app_cursor(), ..TermKeyFlags::default() })
                .unwrap_or_default();
            if let Some(bytes) = strata_key_to_bytes(event, flags) {
                let _ = handle.write(&bytes);
            }
            true
        } else {
            false
        }
    }

    /// Paste text into a PTY, respecting Bracketed Paste mode.
    ///
    /// If the terminal has enabled bracketed paste (`\x1b[?2004h`), the text
    /// is wrapped in `\x1b[200~` / `\x1b[201~` to prevent accidental command
    /// execution.
    pub fn paste_to_pty(&self, block_id: BlockId, text: &str) -> bool {
        if let Some(handle) = self.pty_handles.iter().find(|h| h.block_id == block_id) {
            let bracketed = self
                .block_by_id(block_id)
                .map(|b| b.parser.bracketed_paste())
                .unwrap_or(false);
            if bracketed {
                let _ = handle.write(b"\x1b[200~");
                let _ = handle.write(text.as_bytes());
                let _ = handle.write(b"\x1b[201~");
            } else {
                let _ = handle.write(text.as_bytes());
            }
            true
        } else {
            false
        }
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
        for handle in &self.pty_handles {
            let _ = handle.send_interrupt();
            handle.kill();
        }
        self.pty_handles.clear();
        self.blocks.clear();
        self.block_index.clear();
        self.jobs.clear();
    }

    /// Propagate terminal size changes to PTY handles and block parsers.
    ///
    /// Uses an asymmetric strategy:
    ///   - **Upsizing / height-only**: resize parser immediately — padding
    ///     appears instantly and is visually stable.
    ///   - **Column downsize**: delay the parser column reflow until the
    ///     target size has been **stable** for ~32ms. During the drag the
    ///     parser stays wide and the window frame just crops the right
    ///     edge. Row changes are still applied immediately to keep scroll
    ///     regions correct.
    ///
    /// PTY handles are resized via `sync_pty_sizes()` in `view()`.
    pub fn sync_terminal_size(&mut self) {
        let current_size = self.terminal_size.get();
        let (target_cols, target_rows) = current_size;
        let (parser_cols, parser_rows) = self.last_parser_size.get();

        if (target_cols, target_rows) == (parser_cols, parser_rows) {
            self.pending_downsize.set(None);
            return;
        }

        // Upsizing or width unchanged: resize parser immediately.
        if target_cols >= parser_cols {
            self.last_parser_size.set(current_size);
            self.pending_downsize.set(None);
            for block in &mut self.blocks {
                block.parser.resize(target_cols, target_rows);
            }
            return;
        }

        // Column downsize: apply row changes immediately (keeps scroll
        // regions correct while child is using new row count), but delay
        // the column reflow until the target has been stable.
        if target_rows != parser_rows {
            self.last_parser_size.set((parser_cols, target_rows));
            for block in &mut self.blocks {
                block.parser.resize(parser_cols, target_rows);
            }
        }

        // If the target size changed since the last pending observation,
        // (re)start the debounce timer. The reflow only commits once the
        // size stops changing.
        const DEBOUNCE: Duration = Duration::from_millis(32);
        match self.pending_downsize.get() {
            Some((pending_size, started))
                if pending_size == current_size && started.elapsed() >= DEBOUNCE =>
            {
                // Size has been stable for long enough — commit the reflow.
                self.last_parser_size.set(current_size);
                self.pending_downsize.set(None);
                for block in &mut self.blocks {
                    block.parser.resize(target_cols, target_rows);
                }
            }
            Some((pending_size, _)) if pending_size == current_size => {
                // Still waiting for debounce to expire.
            }
            _ => {
                // First observation or size changed again — (re)start timer.
                self.pending_downsize.set(Some((current_size, Instant::now())));
            }
        }
    }

    /// Send PTY resize immediately from `view()`.
    ///
    /// Only needs `&self` since PTY handles use `Arc<Mutex<>>`.
    /// Sends SIGWINCH only when the size actually changes (avoids
    /// redundant signals every frame). Does NOT touch `last_parser_size`
    /// so `sync_terminal_size()` in `update()` still detects the change.
    pub fn sync_pty_sizes(&self) {
        let current_size = self.terminal_size.get();
        if current_size != self.last_pty_size.get() {
            self.last_pty_size.set(current_size);
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
// Terminal key encoding — converts GUI key events to PTY byte sequences
// =========================================================================

/// Flags from the terminal that affect how keys are encoded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TermKeyFlags {
    /// DECCKM: Application Cursor Keys mode.  When true, arrow keys use
    /// SS3 (`\x1bO`) instead of CSI (`\x1b[`).
    pub app_cursor: bool,

    /// macOS "Option as Meta" toggle.  When true, Option+key sends
    /// `\x1b` + key (Meta/Alt behaviour for shells and Emacs).  When false,
    /// the OS-composed character is sent (e.g., Option+a → å).
    pub option_as_meta: bool,
}

impl Default for TermKeyFlags {
    fn default() -> Self {
        Self {
            app_cursor: false,
            // Default to true — terminal users on macOS almost always want
            // Option to behave as Meta for readline/Emacs keybindings.
            option_as_meta: true,
        }
    }
}

/// Encode a key event into the byte sequence a real terminal would send.
///
/// `flags` carries live terminal state (DECCKM, etc.) that affects encoding.
pub(crate) fn strata_key_to_bytes(
    event: &KeyEvent,
    flags: TermKeyFlags,
) -> Option<Vec<u8>> {
    let (key, modifiers, text) = match event {
        KeyEvent::Pressed {
            key,
            modifiers,
            text,
        } => (key, modifiers, text.as_deref()),
        KeyEvent::Released { .. } => return None,
    };

    match key {
        Key::Character(c) => encode_character(c, modifiers, text, flags),
        Key::Named(named) => encode_named(*named, modifiers, flags),
    }
}

// -- Character keys ---------------------------------------------------------

fn encode_character(
    c: &str,
    modifiers: &strata::event_context::Modifiers,
    text: Option<&str>,
    flags: TermKeyFlags,
) -> Option<Vec<u8>> {
    // Ctrl+letter → ASCII control code (0x01–0x1a)
    if modifiers.ctrl && c.len() == 1 {
        let ch = c.chars().next()?;
        if ch.is_ascii_alphabetic() {
            let ctrl_code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
            return Some(vec![ctrl_code]);
        }
        // Ctrl+special punctuation
        match ch {
            ' ' | '2' | '@' => return Some(vec![0x00]), // Ctrl+Space / Ctrl+@
            '[' => return Some(vec![0x1b]),               // Ctrl+[ = Escape
            '\\' => return Some(vec![0x1c]),              // Ctrl+\ = FS (SIGQUIT)
            ']' => return Some(vec![0x1d]),               // Ctrl+] = GS
            '/' => return Some(vec![0x1f]),               // Ctrl+/ = US
            '_' => return Some(vec![0x1f]),               // Ctrl+_ = US
            _ => {}
        }
    }

    // Alt/Option+character handling.
    //
    // When `option_as_meta` is true (default for terminal users):
    //   Option+b → \x1b b  (Meta-b = backward-word in readline/Emacs)
    //   Ignore the OS-composed text (å, ∫, etc.)
    //
    // When `option_as_meta` is false:
    //   Option+a → å  (fall through to normal text path, using OS-composed text)
    if modifiers.alt && flags.option_as_meta {
        let raw = c.as_bytes();
        let mut bytes = Vec::with_capacity(1 + raw.len());
        bytes.push(0x1b);
        bytes.extend_from_slice(raw);
        return Some(bytes);
    }

    // Normal character — prefer OS-composed text (handles Shift, dead keys, IME)
    if let Some(t) = text {
        if !t.is_empty() {
            return Some(t.as_bytes().to_vec());
        }
    }
    Some(c.as_bytes().to_vec())
}

// -- Named keys -------------------------------------------------------------

/// Compute the xterm modifier parameter: shift=2, alt=3, shift+alt=4, ctrl=5,
/// ctrl+shift=6, ctrl+alt=7, ctrl+shift+alt=8.  Returns 0 when no modifiers.
fn modifier_param(m: &strata::event_context::Modifiers) -> u8 {
    let mut p: u8 = 0;
    if m.shift { p |= 1; }
    if m.alt { p |= 2; }
    if m.ctrl { p |= 4; }
    if p == 0 { 0 } else { p + 1 }
}

/// Build a CSI sequence with an optional modifier parameter.
///
/// *Letter-terminated* keys (arrows, Home, End):
///   unmodified  → `\x1b[ <suffix>`
///   modified    → `\x1b[1;<mod> <suffix>`
///
/// *Tilde-terminated* keys (Insert, Delete, PgUp, PgDn, F5+):
///   unmodified  → `\x1b[ <code> ~`
///   modified    → `\x1b[ <code>;<mod> ~`
fn csi_modified_letter(suffix: u8, m: &strata::event_context::Modifiers) -> Vec<u8> {
    let p = modifier_param(m);
    if p == 0 {
        vec![0x1b, b'[', suffix]
    } else {
        vec![0x1b, b'[', b'1', b';', b'0' + p, suffix]
    }
}

fn csi_modified_tilde(code: &[u8], m: &strata::event_context::Modifiers) -> Vec<u8> {
    let p = modifier_param(m);
    let mut v = vec![0x1b, b'['];
    v.extend_from_slice(code);
    if p != 0 {
        v.push(b';');
        v.push(b'0' + p);
    }
    v.push(b'~');
    v
}

/// SS3 sequence (used for F1-F4 unmodified, and application-mode arrows).
fn ss3(suffix: u8) -> Vec<u8> {
    vec![0x1b, b'O', suffix]
}

fn encode_named(
    named: NamedKey,
    modifiers: &strata::event_context::Modifiers,
    flags: TermKeyFlags,
) -> Option<Vec<u8>> {
    let m = modifiers;
    let has_mods = m.shift || m.alt || m.ctrl;

    match named {
        // -- Simple keys (no CSI) ------------------------------------------
        NamedKey::Enter => Some(vec![b'\r']),
        NamedKey::Escape => Some(vec![0x1b]),
        NamedKey::Space => {
            if m.ctrl {
                Some(vec![0x00]) // Ctrl+Space = NUL
            } else {
                Some(vec![b' '])
            }
        }
        NamedKey::Backspace => {
            if m.ctrl {
                Some(vec![0x08]) // Ctrl+Backspace = BS
            } else if m.alt {
                Some(vec![0x1b, 0x7f]) // Alt+Backspace = ESC DEL
            } else {
                Some(vec![0x7f]) // Backspace = DEL
            }
        }
        NamedKey::Tab => {
            if m.shift {
                Some(vec![0x1b, b'[', b'Z']) // Shift+Tab = backtab
            } else {
                Some(vec![b'\t'])
            }
        }

        // -- Arrow keys (DECCKM-aware) -------------------------------------
        NamedKey::ArrowUp | NamedKey::ArrowDown |
        NamedKey::ArrowRight | NamedKey::ArrowLeft => {
            let suffix = match named {
                NamedKey::ArrowUp => b'A',
                NamedKey::ArrowDown => b'B',
                NamedKey::ArrowRight => b'C',
                NamedKey::ArrowLeft => b'D',
                _ => unreachable!(),
            };
            if has_mods {
                Some(csi_modified_letter(suffix, m))
            } else if flags.app_cursor {
                Some(ss3(suffix))
            } else {
                Some(vec![0x1b, b'[', suffix])
            }
        }

        // -- Home / End (letter-terminated) --------------------------------
        NamedKey::Home => Some(csi_modified_letter(b'H', m)),
        NamedKey::End => Some(csi_modified_letter(b'F', m)),

        // -- Tilde-terminated keys -----------------------------------------
        NamedKey::Insert => Some(csi_modified_tilde(b"2", m)),
        NamedKey::Delete => Some(csi_modified_tilde(b"3", m)),
        NamedKey::PageUp => Some(csi_modified_tilde(b"5", m)),
        NamedKey::PageDown => Some(csi_modified_tilde(b"6", m)),

        // -- Function keys -------------------------------------------------
        // F1-F4: SS3 when unmodified, CSI with modifier when modified
        NamedKey::F1 => if has_mods { Some(csi_modified_tilde(b"11", m)) } else { Some(ss3(b'P')) },
        NamedKey::F2 => if has_mods { Some(csi_modified_tilde(b"12", m)) } else { Some(ss3(b'Q')) },
        NamedKey::F3 => if has_mods { Some(csi_modified_tilde(b"13", m)) } else { Some(ss3(b'R')) },
        NamedKey::F4 => if has_mods { Some(csi_modified_tilde(b"14", m)) } else { Some(ss3(b'S')) },
        // F5-F12: always tilde-terminated
        NamedKey::F5 => Some(csi_modified_tilde(b"15", m)),
        NamedKey::F6 => Some(csi_modified_tilde(b"17", m)),
        NamedKey::F7 => Some(csi_modified_tilde(b"18", m)),
        NamedKey::F8 => Some(csi_modified_tilde(b"19", m)),
        NamedKey::F9 => Some(csi_modified_tilde(b"20", m)),
        NamedKey::F10 => Some(csi_modified_tilde(b"21", m)),
        NamedKey::F11 => Some(csi_modified_tilde(b"23", m)),
        NamedKey::F12 => Some(csi_modified_tilde(b"24", m)),

        _ => None,
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
