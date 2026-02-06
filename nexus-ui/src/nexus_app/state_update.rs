//! Message dispatch and domain handlers for NexusState.

use nexus_api::{DomainValue, Value, TableColumn};
use strata::Command;



use crate::blocks::{Focus, ViewState, ProcSort};

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::drag_state::{ActiveKind, DragStatus, PendingIntent};
use super::file_drop;
use super::input::InputOutput;
use super::message::{AnchorAction, ContextMenuMsg, DragMsg, DropZone, FileDropMsg, NexusMessage, ShellMsg, ViewerMsg};
use super::selection;
use super::shell::ShellOutput;
use super::agent::AgentOutput;
use super::NexusState;
use crate::shell_context::build_shell_context;

// =========================================================================
// Cross-cutting output handlers
// =========================================================================

impl NexusState {
    fn apply_shell_output(&mut self, output: ShellOutput) -> Command<NexusMessage> {
        match output {
            ShellOutput::None => Command::none(),
            ShellOutput::FocusInput | ShellOutput::PtyInputFailed => {
                self.set_focus(Focus::Input);
                self.scroll.snap_to_bottom();
                Command::none()
            }
            ShellOutput::FocusBlock(id) => {
                self.set_focus(Focus::Block(id));
                self.scroll.snap_to_bottom();
                Command::none()
            }
            ShellOutput::ScrollToBottom => {
                self.scroll.hint_bottom();
                Command::none()
            }
            ShellOutput::CwdChanged(path) => {
                self.cwd = path.display().to_string();
                self.context.set_cwd(path);
                Command::none()
            }
            ShellOutput::CommandFinished { block_id, exit_code, command, output } => {
                self.context.on_command_finished(command, output, exit_code);
                // Don't reset focus if the block has an active viewer (e.g. top, less)
                let has_viewer = self.shell.block_by_id(block_id)
                    .map(|b| b.view_state.is_some())
                    .unwrap_or(false);
                if !has_viewer {
                    self.set_focus(Focus::Input);
                }
                self.scroll.snap_to_bottom();
                Command::none()
            }
            ShellOutput::BlockExited { id } => {
                if self.focus == Focus::Block(id) {
                    self.set_focus(Focus::Input);
                    self.scroll.snap_to_bottom();
                }
                Command::none()
            }
            ShellOutput::LoadTreeChildren(block_id, path) => {
                // Spawn async directory listing
                let path_clone = path.clone();
                Command::perform(async move {
                    let mut entries = Vec::new();
                    if let Ok(read_dir) = std::fs::read_dir(&path_clone) {
                        for entry in read_dir.flatten() {
                            if let Ok(file_entry) = nexus_api::FileEntry::from_path(entry.path()) {
                                entries.push(file_entry);
                            }
                        }
                    }
                    // Sort alphabetically
                    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                    NexusMessage::Shell(ShellMsg::TreeChildrenLoaded(block_id, path_clone, entries))
                })
            }
        }
    }

    fn apply_agent_output(&mut self, output: AgentOutput) {
        match output {
            AgentOutput::None => {}
            AgentOutput::ScrollToBottom => {
                self.scroll.hint_bottom();
            }
            AgentOutput::FocusQuestionInput => {
                self.set_focus(Focus::AgentInput);
                self.scroll.hint_bottom();
            }
        }
    }

    fn apply_input_output(&mut self, output: InputOutput) -> Command<NexusMessage> {
        match output {
            InputOutput::None => Command::none(),
            InputOutput::Submit { text, is_agent, attachments } => {
                self.handle_submit(text, is_agent, attachments)
            }
        }
    }
}

// =========================================================================
// Top-level dispatch
// =========================================================================

impl NexusState {
    /// Top-level message dispatch. Called from Component::update().
    pub(super) fn dispatch_update(
        &mut self,
        msg: NexusMessage,
        ctx: &mut strata::component::Ctx,
    ) -> Command<NexusMessage> {
        // Apply deferred scroll offset from view() (scroll-to-block)
        self.scroll.apply_pending();

        match msg {
            NexusMessage::Input(m) => {
                if matches!(m, super::message::InputMsg::Mouse(_)) {
                    self.set_focus(Focus::Input);
                } else {
                    self.scroll.snap_to_bottom();
                }
                let (_cmd, output) = self.input.update(m, ctx);
                self.apply_input_output(output)
            }
            NexusMessage::Shell(m) => {
                // Anchor actions are cross-cutting (clipboard, spawn process)
                if let ShellMsg::OpenAnchor(_, ref action) = m {
                    self.exec_anchor_action(action);
                    return Command::none();
                }
                let (_cmd, output) = self.shell.update(m, ctx);
                self.apply_shell_output(output)
            }
            NexusMessage::Agent(m) => {
                if matches!(m, super::message::AgentMsg::QuestionInputMouse(_)) {
                    self.set_focus(Focus::AgentInput);
                }
                let (_cmd, output) = self.agent.update(m, ctx);
                self.apply_agent_output(output);
                Command::none()
            }
            NexusMessage::Selection(m) => {
                let (_cmd, _) = self.selection.update(m, ctx);
                Command::none()
            }
            NexusMessage::Viewer(m) => { self.dispatch_viewer_msg(m); Command::none() }
            NexusMessage::FocusBlock(id) => {
                self.set_focus(Focus::Block(id));
                Command::none()
            }
            NexusMessage::ContextMenu(m) => self.dispatch_context_menu(m),
            NexusMessage::Scroll(action) => { self.scroll.apply_user_scroll(action); Command::none() }
            NexusMessage::ScrollToJob(_) => { self.scroll.snap_to_bottom(); Command::none() }
            NexusMessage::Copy => { self.copy_selection_or_input(); Command::none() }
            NexusMessage::Paste => { self.paste_from_clipboard(ctx.images); Command::none() }
            NexusMessage::ClearScreen => { self.clear_screen(); Command::none() }
            NexusMessage::CloseWindow => { self.exit_requested = true; Command::none() }
            // NewWindow and QuitApp are intercepted by the shell adapter before
            // reaching update(). If they somehow arrive here, treat as no-ops.
            NexusMessage::NewWindow | NexusMessage::QuitApp => Command::none(),
            NexusMessage::BlurAll => {
                self.transient.dismiss_all(&mut self.input);
                self.set_focus(Focus::Input);
                Command::none()
            }
            NexusMessage::Tick => { self.on_output_arrived(); Command::none() }
            NexusMessage::FileDrop(m) => self.dispatch_file_drop(m),
            NexusMessage::Drag(m) => { self.dispatch_drag(m, ctx); Command::none() }
            NexusMessage::FocusPrevBlock => {
                let target = match self.focus {
                    Focus::Block(id) => self.prev_block_id(id),
                    Focus::Input => self.last_block_id(),
                    _ => None,
                };
                match target {
                    Some(id) => {
                        self.set_focus(Focus::Block(id));
                        self.scroll.scroll_to_block(id);
                    }
                    None => {
                        self.set_focus(Focus::Input);
                        self.scroll.snap_to_bottom();
                    }
                }
                Command::none()
            }
            NexusMessage::FocusNextBlock => {
                if let Focus::Block(id) = self.focus {
                    match self.next_block_id(id) {
                        Some(next) => {
                            self.set_focus(Focus::Block(next));
                            self.scroll.scroll_to_block(next);
                        }
                        None => {
                            self.set_focus(Focus::Input);
                            self.scroll.snap_to_bottom();
                        }
                    }
                }
                Command::none()
            }
            NexusMessage::FocusFirstBlock => {
                if let Some(id) = self.all_block_ids_ordered().first().copied() {
                    self.set_focus(Focus::Block(id));
                    self.scroll.scroll_to_block(id);
                }
                Command::none()
            }
            NexusMessage::FocusLastBlock => {
                if let Some(id) = self.last_block_id() {
                    self.set_focus(Focus::Block(id));
                    self.scroll.scroll_to_block(id);
                }
                Command::none()
            }
            NexusMessage::FocusAgentInput => {
                self.set_focus(Focus::AgentInput);
                self.scroll.snap_to_bottom();
                Command::none()
            }
            NexusMessage::TypeThrough(event) => {
                self.set_focus(Focus::Input);
                self.scroll.snap_to_bottom();
                if let Some(msg) = self.input.on_key(&event) {
                    let (_cmd, output) = self.input.update(msg, ctx);
                    self.apply_input_output(output)
                } else {
                    Command::none()
                }
            }
            NexusMessage::ZoomIn | NexusMessage::ZoomOut | NexusMessage::ZoomReset => {
                // Zoom stubs — shortcuts wired, rendering deferred
                Command::none()
            }
            #[cfg(debug_assertions)]
            NexusMessage::ToggleDebugLayout => {
                self.debug_layout = !self.debug_layout;
                Command::none()
            }
        }
    }
}

// =========================================================================
// Cross-cutting handlers (root policy)
// =========================================================================

impl NexusState {
    fn handle_submit(
        &mut self,
        text: String,
        is_agent: bool,
        attachments: Vec<Value>,
    ) -> Command<NexusMessage> {
        // Short-circuit built-in "clear" before any side effects.
        if !is_agent && text.trim() == "clear" {
            return Command::message(NexusMessage::ClearScreen);
        }

        self.input.reset_history_nav();

        if is_agent {
            let block_id = self.next_id();
            let contextualized_query = if self.agent.session_id.is_some() {
                format!("[CWD: {}]\n{}", self.cwd, text)
            } else {
                let shell_context = build_shell_context(
                    &self.cwd,
                    &self.shell.blocks,
                    self.input.shell_history(),
                );
                format!("{}{}", shell_context, text)
            };
            self.agent.spawn(block_id, text, contextualized_query, attachments, &self.cwd);
            self.scroll.snap_to_bottom();
        } else {
            let block_id = self.next_id();
            let output =
                self.shell
                    .execute(text, block_id, &self.cwd, &self.kernel, &self.kernel_tx);
            self.apply_shell_output(output);
        }

        Command::none()
    }

    fn dispatch_drag(&mut self, msg: DragMsg, ctx: &mut strata::component::Ctx) {
        match msg {
            DragMsg::Start(intent, origin) => {
                self.drag.status = DragStatus::Pending {
                    origin,
                    intent,
                };
            }
            DragMsg::StartSelecting(addr, mode) => {
                // If the click landed on a shell block, focus it so keyboard
                // input flows to its PTY.
                if let Some(block_id) = self.shell.block_for_source(addr.source_id) {
                    self.set_focus(crate::blocks::Focus::Block(block_id));
                }
                // Immediate Active — no Pending hysteresis for raw text clicks.
                self.selection.update(
                    super::message::SelectionMsg::Start(addr.clone(), mode),
                    ctx,
                );
                self.drag.status = DragStatus::Active(ActiveKind::Selecting);
            }
            DragMsg::Activate(_position) => {
                if let DragStatus::Pending { intent, .. } =
                    std::mem::replace(&mut self.drag.status, DragStatus::Inactive)
                {
                    let drag_source = match intent {
                        PendingIntent::Anchor { payload, .. } => {
                            Some(self.payload_to_drag_source(&payload))
                        }
                        PendingIntent::SelectionDrag { text, .. } => {
                            Some(strata::DragSource::Text(text))
                        }
                        PendingIntent::RowDrag { block_id, row_index, .. } => {
                            let payload = super::drag_state::DragPayload::TableRow {
                                block_id,
                                row_index,
                                display: self.shell.row_display_text(block_id, row_index),
                            };
                            Some(self.payload_to_drag_source(&payload))
                        }
                        // ColumnResize, ColumnReorder, TerminalCapture — not drag sources
                        _ => None,
                    };
                    if let Some(source) = drag_source {
                        if let Err(e) = strata::platform::start_drag(&source) {
                            tracing::warn!("Native drag failed: {}", e);
                        }
                    }
                }
            }
            DragMsg::Cancel => {
                let prev = std::mem::replace(&mut self.drag.status, DragStatus::Inactive);
                match prev {
                    DragStatus::Pending { intent, .. } => {
                        match intent {
                            PendingIntent::Anchor { source, source_rect, .. } => {
                                // Re-dispatch click to anchor handler
                                if let Some(msg) = self.shell.on_click_anchor(source) {
                                    if let ShellMsg::OpenAnchor(_, ref action) = msg {
                                        self.exec_anchor_action_with_rect(action, source_rect);
                                    }
                                }
                            }
                            PendingIntent::SelectionDrag { origin_addr, .. } => {
                                // Click inside selection without drag → clear selection, place caret
                                self.selection.update(
                                    super::message::SelectionMsg::Start(origin_addr, super::drag_state::SelectMode::Char),
                                    ctx,
                                );
                            }
                            // Future intents — no-op
                            _ => {}
                        }
                    }
                    DragStatus::Active(ActiveKind::Selecting { .. }) => {
                        // Selection ended (mouse released or cursor left)
                        self.selection.update(
                            super::message::SelectionMsg::End,
                            ctx,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    fn payload_to_drag_source(&self, payload: &super::drag_state::DragPayload) -> strata::DragSource {
        use super::drag_state::DragPayload;

        match payload {
            DragPayload::FilePath(p) => {
                if p.exists() {
                    strata::DragSource::File(p.clone())
                } else {
                    strata::DragSource::Text(p.to_string_lossy().into_owned())
                }
            }
            DragPayload::Text(s) => strata::DragSource::Text(s.clone()),
            DragPayload::TableRow { block_id, row_index, display } => {
                if let Some(&idx) = self.shell.block_index.get(block_id) {
                    if let Some(block) = self.shell.blocks.get(idx) {
                        if let Some(nexus_api::Value::Table { columns, rows }) = &block.native_output {
                            if let Some(row) = rows.get(*row_index) {
                                let header: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                                let cells: Vec<String> = row.iter().map(|v| v.to_text()).collect();
                                let tsv = format!("{}\n{}", header.join("\t"), cells.join("\t"));
                                return strata::DragSource::Tsv(tsv);
                            }
                        }
                    }
                }
                strata::DragSource::Text(display.clone())
            }
            DragPayload::Block(id) => {
                if let Some(&idx) = self.shell.block_index.get(id) {
                    if let Some(block) = self.shell.blocks.get(idx) {
                        if let Some(nexus_api::Value::Table { columns, rows }) = &block.native_output {
                            let tsv = table_to_tsv(columns, rows);
                            let filename = format!("{}-output.tsv", block.command.split_whitespace().next().unwrap_or("block"));
                            match file_drop::write_drag_temp_file(&filename, tsv.as_bytes()) {
                                Ok(path) => return strata::DragSource::File(path),
                                Err(e) => {
                                    tracing::warn!("Failed to write drag temp file: {}", e);
                                    return strata::DragSource::Tsv(tsv);
                                }
                            }
                        }
                        if let Some(ref value) = block.native_output {
                            return strata::DragSource::Text(value.to_text());
                        }
                        return strata::DragSource::Text(
                            block.parser.grid_with_scrollback().to_string(),
                        );
                    }
                }
                strata::DragSource::Text(format!("block#{}", id.0))
            }
            DragPayload::Image { data, filename } => {
                let temp_dir = std::env::temp_dir().join("nexus-drag");
                let _ = std::fs::create_dir_all(&temp_dir);
                let path = temp_dir.join(filename);
                match std::fs::write(&path, data) {
                    Ok(()) => strata::DragSource::Image(path),
                    Err(e) => {
                        tracing::warn!("Failed to write image temp file: {}", e);
                        strata::DragSource::Text(filename.clone())
                    }
                }
            }
            DragPayload::Selection { text, structured } => {
                if let Some(super::drag_state::StructuredSelection::TableRows { columns, rows }) = structured {
                    let mut tsv = columns.join("\t");
                    tsv.push('\n');
                    for row in rows {
                        tsv.push_str(&row.join("\t"));
                        tsv.push('\n');
                    }
                    strata::DragSource::Tsv(tsv)
                } else {
                    strata::DragSource::Text(text.clone())
                }
            }
        }
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        if !self.input.text_input.text.is_empty()
            && !self.input.text_input.text.ends_with(' ')
        {
            self.input.text_input.text.push(' ');
        }
        self.input.text_input.text.push_str(text);
        self.input.text_input.cursor = self.input.text_input.text.len();
    }

    fn dispatch_file_drop(&mut self, msg: FileDropMsg) -> Command<NexusMessage> {
        match msg {
            FileDropMsg::Hovered(_path, zone) => {
                self.drop_highlight = Some(zone);
                Command::none()
            }
            FileDropMsg::Dropped(path, zone) => {
                self.drop_highlight = None;
                // Check if this is our own drag data coming back via native round-trip
                if let Some(text) = file_drop::read_temp_file_content(&path) {
                    self.insert_text_at_cursor(&text);
                    return Command::none();
                }
                match zone {
                    DropZone::InputBar | DropZone::Empty => {
                        // Insert shell-quoted path at cursor
                        let quoted = file_drop::shell_quote(&path);
                        if !self.input.text_input.text.is_empty()
                            && !self.input.text_input.text.ends_with(' ')
                        {
                            self.input.text_input.text.push(' ');
                        }
                        self.input.text_input.text.push_str(&quoted);
                        self.input.text_input.cursor = self.input.text_input.text.len();
                        Command::none()
                    }
                    DropZone::AgentPanel => {
                        // Async read — don't block the UI thread
                        let path_clone = path.clone();
                        Command::perform(async move {
                            match tokio::fs::read(&path_clone).await {
                                Ok(data) => {
                                    if data.len() > 10 * 1024 * 1024 {
                                        NexusMessage::FileDrop(FileDropMsg::FileLoadFailed(
                                            path_clone,
                                            "File exceeds 10 MB limit".into(),
                                        ))
                                    } else {
                                        NexusMessage::FileDrop(FileDropMsg::FileLoaded(path_clone, data))
                                    }
                                }
                                Err(e) => NexusMessage::FileDrop(FileDropMsg::FileLoadFailed(
                                    path_clone,
                                    e.to_string(),
                                )),
                            }
                        })
                    }
                    DropZone::ShellBlock(_) => {
                        // Treat same as input bar
                        let quoted = file_drop::shell_quote(&path);
                        if !self.input.text_input.text.is_empty()
                            && !self.input.text_input.text.ends_with(' ')
                        {
                            self.input.text_input.text.push(' ');
                        }
                        self.input.text_input.text.push_str(&quoted);
                        self.input.text_input.cursor = self.input.text_input.text.len();
                        Command::none()
                    }
                }
            }
            FileDropMsg::HoverLeft => {
                self.drop_highlight = None;
                Command::none()
            }
            FileDropMsg::FileLoaded(path, data) => {
                // Create an attachment from the loaded file data
                // For now, insert the path into the input as context
                let filename = path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                tracing::info!("File loaded for agent: {} ({} bytes)", filename, data.len());
                // TODO: Create proper attachment when agent attachment API is ready
                let quoted = file_drop::shell_quote(&path);
                if !self.input.text_input.text.is_empty()
                    && !self.input.text_input.text.ends_with(' ')
                {
                    self.input.text_input.text.push(' ');
                }
                self.input.text_input.text.push_str(&quoted);
                self.input.text_input.cursor = self.input.text_input.text.len();
                Command::none()
            }
            FileDropMsg::FileLoadFailed(path, reason) => {
                tracing::warn!("File drop failed for {}: {}", path.display(), reason);
                Command::none()
            }
        }
    }

    fn exec_anchor_action(&self, action: &AnchorAction) {
        self.exec_anchor_action_with_rect(action, None);
    }

    fn exec_anchor_action_with_rect(&self, action: &AnchorAction, source_rect: Option<strata::primitives::Rect>) {
        match action {
            AnchorAction::QuickLook(path) => {
                // Preview with native Quick Look (macOS)
                let result = if let Some(local_rect) = source_rect {
                    // Use local rect for zoom animation
                    strata::platform::preview_file_with_local_rect(path, local_rect)
                } else {
                    // No animation
                    strata::platform::preview_file(path)
                };
                if let Err(e) = result {
                    tracing::warn!("Quick Look failed: {}", e);
                }
            }
            AnchorAction::RevealPath(path) => {
                // Reveal in Finder (macOS) — `open -R <path>`
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(path)
                    .spawn();
            }
            AnchorAction::Open(path) => {
                // Open with default application (macOS) — `open <path>`
                let _ = std::process::Command::new("open")
                    .arg(path)
                    .spawn();
            }
            AnchorAction::OpenUrl(url) => {
                let _ = std::process::Command::new("open")
                    .arg(url)
                    .spawn();
            }
            AnchorAction::CopyToClipboard(text) => {
                Self::set_clipboard_text(text);
            }
        }
    }

    fn copy_selection_or_input(&mut self) {
        // Try content selection first
        if let Some(text) =
            self.selection
                .extract_selected_text(&self.shell.blocks, &self.agent.blocks)
        {
            Self::set_clipboard_text(&text);
            return;
        }

        // Fall back to input text selection
        if let Some((sel_start, sel_end)) = self.input.text_input.selection {
            let start = sel_start.min(sel_end);
            let end = sel_start.max(sel_end);
            if start != end {
                let selected: String = self.input.text_input.text
                    .chars()
                    .skip(start)
                    .take(end - start)
                    .collect();
                if !selected.is_empty() {
                    Self::set_clipboard_text(&selected);
                }
            }
        }
    }

    fn dispatch_context_menu(&mut self, msg: ContextMenuMsg) -> Command<NexusMessage> {
        match msg {
            ContextMenuMsg::Show(x, y, items, target) => {
                self.transient.show_context_menu(x, y, items, target);
                Command::none()
            }
            ContextMenuMsg::Action(item) => self.exec_context_menu_item(item),
            ContextMenuMsg::Dismiss => {
                self.transient.dismiss_context_menu();
                Command::none()
            }
        }
    }

    fn exec_context_menu_item(&mut self, item: ContextMenuItem) -> Command<NexusMessage> {
        let target = self.transient.context_menu().map(|m| m.target.clone());
        self.transient.dismiss_context_menu();
        match item {
            ContextMenuItem::Copy => {
                // First try to copy the selected text (respects user's selection)
                if let Some(text) = self
                    .selection
                    .extract_selected_text(&self.shell.blocks, &self.agent.blocks)
                {
                    Self::set_clipboard_text(&text);
                    return Command::none();
                }
                // Fall back to input selection if in input context
                if matches!(target, Some(ContextTarget::Input)) {
                    if let Some((sel_start, sel_end)) = self.input.text_input.selection {
                        let start = sel_start.min(sel_end);
                        let end = sel_start.max(sel_end);
                        if start != end {
                            let selected: String = self.input.text_input.text
                                .chars()
                                .skip(start)
                                .take(end - start)
                                .collect();
                            if !selected.is_empty() {
                                Self::set_clipboard_text(&selected);
                                return Command::none();
                            }
                        }
                    }
                }
                // Fall back to entire block text only if no selection
                if let Some(text) = target.and_then(|t| {
                    selection::extract_block_text(
                        &self.shell.blocks,
                        &self.shell.block_index,
                        &self.agent.blocks,
                        &self.agent.block_index,
                        &self.input.text_input.text,
                        &t,
                    )
                }) {
                    Self::set_clipboard_text(&text);
                }
            }
            ContextMenuItem::Paste => {
                return Command::message(NexusMessage::Paste);
            }
            ContextMenuItem::SelectAll => match target.as_ref() {
                Some(ContextTarget::Input) | None => {
                    self.input.text_input.select_all();
                }
                Some(ContextTarget::Block(_)) | Some(ContextTarget::AgentBlock(_)) => {
                    self.selection
                        .select_all(&self.shell.blocks, &self.agent.blocks);
                }
            },
            ContextMenuItem::Clear => {
                self.input.text_input.text.clear();
                self.input.text_input.cursor = 0;
                self.input.text_input.selection = None;
            }
            ContextMenuItem::CopyCommand => {
                if let Some(block) = self.target_shell_block(&target) {
                    Self::set_clipboard_text(&block.command);
                }
            }
            ContextMenuItem::CopyOutput => {
                if let Some(block) = self.target_shell_block(&target) {
                    let text = if let Some(ref value) = block.native_output {
                        value.to_text()
                    } else {
                        block.parser.grid_with_scrollback().to_string()
                    };
                    Self::set_clipboard_text(&text);
                }
            }
            ContextMenuItem::CopyAsTsv => {
                if let Some(block) = self.target_shell_block(&target) {
                    if let Some(Value::Table { columns, rows }) = &block.native_output {
                        let tsv = table_to_tsv(columns, rows);
                        Self::set_clipboard_text(&tsv);
                    }
                }
            }
            ContextMenuItem::CopyAsJson => {
                if let Some(block) = self.target_shell_block(&target) {
                    if let Some(ref value) = block.native_output {
                        if let Ok(json) = serde_json::to_string_pretty(value) {
                            Self::set_clipboard_text(&json);
                        }
                    }
                }
            }
            ContextMenuItem::Rerun => {
                if let Some(block) = self.target_shell_block(&target) {
                    let cmd = block.command.clone();
                    return self.handle_submit(cmd, false, Vec::new());
                }
            }
            ContextMenuItem::QuickLook(path) => {
                if let Err(e) = strata::platform::preview_file(&path) {
                    tracing::warn!("Quick Look failed: {}", e);
                }
            }
            ContextMenuItem::Open(path) => {
                let _ = std::process::Command::new("open")
                    .arg(&path)
                    .spawn();
            }
            ContextMenuItem::CopyPath(path) => {
                Self::set_clipboard_text(&path.display().to_string());
            }
            ContextMenuItem::RevealInFinder(path) => {
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn();
            }
        }
        Command::none()
    }

    /// Resolve a context target to the shell block it refers to.
    fn target_shell_block<'a>(&'a self, target: &Option<ContextTarget>) -> Option<&'a crate::blocks::Block> {
        match target {
            Some(ContextTarget::Block(id)) => {
                self.shell.block_index.get(id).and_then(|&idx| self.shell.blocks.get(idx))
            }
            _ => None,
        }
    }
}

// =========================================================================
// Streaming update handler
// =========================================================================
// Viewer message handler
// =========================================================================

impl NexusState {
    fn dispatch_viewer_msg(&mut self, msg: ViewerMsg) {
        match msg {
            ViewerMsg::ScrollUp(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let scrolled = match &mut block.view_state {
                        Some(ViewState::Pager { scroll_line, .. })
                        | Some(ViewState::DiffViewer { scroll_line, .. }) => {
                            *scroll_line = scroll_line.saturating_sub(1);
                            true
                        }
                        _ => false,
                    };
                    if scrolled { block.version += 1; }
                }
            }
            ViewerMsg::ScrollDown(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let scrolled = match &mut block.view_state {
                        Some(ViewState::Pager { scroll_line, .. })
                        | Some(ViewState::DiffViewer { scroll_line, .. }) => {
                            *scroll_line += 1;
                            true
                        }
                        _ => false,
                    };
                    if scrolled { block.version += 1; }
                }
            }
            ViewerMsg::PageUp(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let scrolled = match &mut block.view_state {
                        Some(ViewState::Pager { scroll_line, .. })
                        | Some(ViewState::DiffViewer { scroll_line, .. }) => {
                            *scroll_line = scroll_line.saturating_sub(30);
                            true
                        }
                        _ => false,
                    };
                    if scrolled { block.version += 1; }
                }
            }
            ViewerMsg::PageDown(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let scrolled = match &mut block.view_state {
                        Some(ViewState::Pager { scroll_line, .. })
                        | Some(ViewState::DiffViewer { scroll_line, .. }) => {
                            *scroll_line += 30;
                            true
                        }
                        _ => false,
                    };
                    if scrolled { block.version += 1; }
                }
            }
            ViewerMsg::GoToTop(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let scrolled = match &mut block.view_state {
                        Some(ViewState::Pager { scroll_line, .. })
                        | Some(ViewState::DiffViewer { scroll_line, .. }) => {
                            *scroll_line = 0;
                            true
                        }
                        _ => false,
                    };
                    if scrolled { block.version += 1; }
                }
            }
            ViewerMsg::GoToBottom(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let scrolled = match &mut block.view_state {
                        Some(ViewState::Pager { scroll_line, .. })
                        | Some(ViewState::DiffViewer { scroll_line, .. }) => {
                            // Set to a very large value; rendering will clamp
                            *scroll_line = usize::MAX / 2;
                            true
                        }
                        _ => false,
                    };
                    if scrolled { block.version += 1; }
                }
            }
            ViewerMsg::SearchStart(_id) | ViewerMsg::SearchNext(_id) => {
                // Search TBD — no-op for now
            }
            ViewerMsg::SortBy(id, sort) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    if let Some(ViewState::ProcessMonitor { ref mut sort_by, ref mut sort_desc, .. }) = block.view_state {
                        if *sort_by == sort {
                            *sort_desc = !*sort_desc;
                        } else {
                            *sort_by = sort;
                            *sort_desc = true;
                        }
                        // Map ProcSort to column index (%CPU=2, %MEM=3, PID=1)
                        let col_idx = match sort {
                            ProcSort::Cpu => 2,
                            ProcSort::Mem => 3,
                            ProcSort::Pid => 1,
                            ProcSort::Command => 10,
                        };
                        let ascending = !*sort_desc;
                        block.table_sort = crate::blocks::TableSort {
                            column: Some(col_idx),
                            ascending,
                        };
                        // Re-sort current data
                        if let Some(Value::Table { ref mut rows, .. }) = block.native_output {
                            super::shell::ShellWidget::sort_rows(rows, col_idx, ascending);
                        }
                        if let Some(Value::Table { ref mut rows, .. }) = block.stream_latest {
                            super::shell::ShellWidget::sort_rows(rows, col_idx, ascending);
                        }
                        block.version += 1;
                    }
                }
            }
            ViewerMsg::TreeToggle(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    if let Some(ViewState::TreeBrowser { ref mut collapsed, ref selected, .. }) = block.view_state {
                        if let Some(sel) = selected {
                            if collapsed.contains(sel) {
                                collapsed.remove(sel);
                            } else {
                                collapsed.insert(*sel);
                            }
                            block.version += 1;
                        }
                    }
                }
            }
            ViewerMsg::TreeUp(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    if let Some(ViewState::TreeBrowser { selected, .. }) = &mut block.view_state {
                        if let Some(sel) = selected {
                            *sel = sel.saturating_sub(1);
                        }
                        block.version += 1;
                    }
                }
            }
            ViewerMsg::TreeDown(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    let node_count = block.native_output.as_ref().map(|v| {
                        if let Some(DomainValue::Tree(tree)) = v.as_domain() { tree.nodes.len() } else { 0 }
                    }).unwrap_or(0);
                    if let Some(ViewState::TreeBrowser { selected, .. }) = &mut block.view_state {
                        if let Some(sel) = selected {
                            if *sel + 1 < node_count {
                                *sel += 1;
                            }
                        }
                        block.version += 1;
                    }
                }
            }
            ViewerMsg::DiffNextFile(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    if let Some(ViewState::DiffViewer { current_file, .. }) = &mut block.view_state {
                        // Count diff files in content
                        let file_count = block.native_output.as_ref()
                            .and_then(|v| if let Value::List(items) = v { Some(items.len()) } else { None })
                            .unwrap_or(0);
                        if *current_file + 1 < file_count {
                            *current_file += 1;
                        }
                        block.version += 1;
                    }
                }
            }
            ViewerMsg::DiffPrevFile(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    if let Some(ViewState::DiffViewer { current_file, .. }) = &mut block.view_state {
                        *current_file = current_file.saturating_sub(1);
                        block.version += 1;
                    }
                }
            }
            ViewerMsg::DiffToggleFile(id) => {
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    if let Some(ViewState::DiffViewer { current_file, collapsed_indices, .. }) =
                        &mut block.view_state
                    {
                        let idx = *current_file;
                        if !collapsed_indices.remove(&idx) {
                            collapsed_indices.insert(idx);
                        }
                        block.version += 1;
                    }
                }
            }
            ViewerMsg::Exit(id) => {
                // Cancel directly via the free function — does NOT require the kernel
                // mutex, which may be held by the command's blocking loop (e.g. top).
                nexus_kernel::commands::cancel_block(id);
                if let Some(block) = self.shell.block_by_id_mut(id) {
                    block.view_state = None;
                    block.version += 1;
                }
                self.set_focus(Focus::Input);
            }
        }
    }
}

fn table_to_tsv(columns: &[TableColumn], rows: &[Vec<Value>]) -> String {
    let mut buf = String::new();
    // Header row
    for (i, col) in columns.iter().enumerate() {
        if i > 0 { buf.push('\t'); }
        buf.push_str(&col.name);
    }
    buf.push('\n');
    // Data rows
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 { buf.push('\t'); }
            let text = cell.to_text();
            // Escape tabs/newlines within cell text
            buf.push_str(&text.replace('\t', " ").replace('\n', " "));
        }
        buf.push('\n');
    }
    buf
}
