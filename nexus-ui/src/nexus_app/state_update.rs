//! Message dispatch and domain handlers for NexusState.

use nexus_api::{Value, TableColumn};
use strata::Command;

use crate::blocks::Focus;

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::drag_state::{ActiveDrag, DragStatus};
use super::file_drop;
use super::input::InputOutput;
use super::message::{AnchorAction, ContextMenuMsg, DragMsg, DropZone, FileDropMsg, NexusMessage, ShellMsg};
use super::selection;
use super::shell::ShellOutput;
use super::agent::AgentOutput;
use super::NexusState;
use crate::shell_context::build_shell_context;

// =========================================================================
// Cross-cutting output handlers
// =========================================================================

impl NexusState {
    fn apply_shell_output(&mut self, output: ShellOutput) {
        match output {
            ShellOutput::None => {}
            ShellOutput::FocusInput | ShellOutput::PtyInputFailed => {
                self.set_focus_input();
                self.scroll.force();
            }
            ShellOutput::FocusBlock(id) => {
                self.set_focus_block(id);
                self.scroll.force();
            }
            ShellOutput::ScrollToBottom => {
                self.scroll.hint();
            }
            ShellOutput::CwdChanged(path) => {
                self.cwd = path.display().to_string();
                let _ = std::env::set_current_dir(&path);
            }
            ShellOutput::CommandFinished { exit_code, command, output } => {
                self.context.on_command_finished(command, output, exit_code);
                self.set_focus_input();
                self.scroll.force();
            }
            ShellOutput::BlockExited { id } => {
                if self.focus == Focus::Block(id) {
                    self.set_focus_input();
                    self.scroll.force();
                }
            }
        }
    }

    fn apply_agent_output(&mut self, output: AgentOutput) {
        match output {
            AgentOutput::None => {}
            AgentOutput::ScrollToBottom => {
                self.scroll.hint();
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
        match msg {
            NexusMessage::Input(m) => {
                let (_cmd, output) = self.input.update(m, ctx);
                // InputWidget never produces async commands currently
                self.apply_input_output(output)
            }
            NexusMessage::Shell(m) => {
                // Anchor actions are cross-cutting (clipboard, spawn process)
                if let ShellMsg::OpenAnchor(_, ref action) = m {
                    self.exec_anchor_action(action);
                    return Command::none();
                }
                let (_cmd, output) = self.shell.update(m, ctx);
                self.apply_shell_output(output);
                Command::none()
            }
            NexusMessage::Agent(m) => {
                let (_cmd, output) = self.agent.update(m, ctx);
                self.apply_agent_output(output);
                Command::none()
            }
            NexusMessage::Selection(m) => {
                let (_cmd, _) = self.selection.update(m, ctx);
                Command::none()
            }
            NexusMessage::ContextMenu(m) => self.dispatch_context_menu(m),
            NexusMessage::Scroll(action) => { self.scroll.apply_user_scroll(action); Command::none() }
            NexusMessage::ScrollToJob(_) => { self.scroll.force(); Command::none() }
            NexusMessage::Copy => { self.copy_selection_or_input(); Command::none() }
            NexusMessage::Paste => { self.paste_from_clipboard(ctx.images); Command::none() }
            NexusMessage::ClearScreen => { self.clear_screen(); Command::none() }
            NexusMessage::CloseWindow => { self.exit_requested = true; Command::none() }
            NexusMessage::BlurAll => {
                self.transient.dismiss_all(&mut self.input);
                self.set_focus_input();
                Command::none()
            }
            NexusMessage::Tick => { self.on_output_arrived(); Command::none() }
            NexusMessage::FileDrop(m) => self.dispatch_file_drop(m),
            NexusMessage::Drag(m) => { self.dispatch_drag(m); Command::none() }
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
            self.scroll.force();
        } else {
            let block_id = self.next_id();
            let output =
                self.shell
                    .execute(text, block_id, &self.cwd, &self.kernel, &self.kernel_tx);
            self.apply_shell_output(output);
        }

        Command::none()
    }

    fn dispatch_drag(&mut self, msg: DragMsg) {
        match msg {
            DragMsg::Start(payload, origin, source) => {
                self.drag.status = DragStatus::Pending {
                    origin,
                    payload,
                    source,
                };
            }
            DragMsg::Activate(position) => {
                if let DragStatus::Pending { origin, payload, source } =
                    std::mem::replace(&mut self.drag.status, DragStatus::Inactive)
                {
                    self.drag.status = DragStatus::Active(ActiveDrag {
                        payload,
                        origin,
                        current_pos: position,
                        source,
                    });
                }
            }
            DragMsg::Move(position) => {
                if let DragStatus::Active(ref mut active) = self.drag.status {
                    active.current_pos = position;
                }
            }
            DragMsg::Drop(zone) => {
                if let DragStatus::Active(active) =
                    std::mem::replace(&mut self.drag.status, DragStatus::Inactive)
                {
                    self.handle_internal_drop(active, zone);
                }
            }
            DragMsg::GoOutbound => {
                if let DragStatus::Active(active) =
                    std::mem::replace(&mut self.drag.status, DragStatus::Inactive)
                {
                    let source = self.payload_to_drag_source(&active);
                    if let Err(e) = strata::platform::start_drag(&source) {
                        tracing::warn!("Outbound drag failed: {}", e);
                    }
                }
            }
            DragMsg::Cancel => {
                // If Pending, treat as normal click — forward to the anchor handler.
                if let DragStatus::Pending { source, .. } =
                    std::mem::replace(&mut self.drag.status, DragStatus::Inactive)
                {
                    // Re-dispatch click to anchor handler (no fake mouse events)
                    if let Some(msg) = self.shell.on_click_anchor(source) {
                        if let ShellMsg::OpenAnchor(_, ref action) = msg {
                            self.exec_anchor_action(action);
                        }
                    }
                } else {
                    self.drag.status = DragStatus::Inactive;
                }
            }
        }
    }

    fn handle_internal_drop(&mut self, active: ActiveDrag, _zone: DropZone) {
        use super::drag_state::DragPayload;

        let text_to_insert = match &active.payload {
            DragPayload::Text(s) => Some(s.clone()),
            DragPayload::FilePath(p) => Some(file_drop::shell_quote(p)),
            DragPayload::TableRow { display, .. } => Some(display.clone()),
            DragPayload::Block(id) => {
                // Insert the command from the referenced block as pipe composition
                self.shell.block_index.get(id)
                    .and_then(|&idx| self.shell.blocks.get(idx))
                    .map(|b| b.command.clone())
            }
            DragPayload::Selection { text, .. } => Some(text.clone()),
        };

        if let Some(text) = text_to_insert {
            self.insert_text_at_cursor(&text);
        }
    }

    fn payload_to_drag_source(&self, active: &ActiveDrag) -> strata::DragSource {
        use super::drag_state::DragPayload;

        match &active.payload {
            DragPayload::FilePath(p) => {
                if p.exists() {
                    strata::DragSource::File(p.clone())
                } else {
                    strata::DragSource::Text(p.to_string_lossy().into_owned())
                }
            }
            DragPayload::Text(s) => strata::DragSource::Text(s.clone()),
            DragPayload::TableRow { block_id, row_index, display } => {
                // If the block has table output, export the row as TSV.
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
                // Export block output as text or TSV.
                // For table data, write a temp file so the platform layer stays I/O-free.
                if let Some(&idx) = self.shell.block_index.get(id) {
                    if let Some(block) = self.shell.blocks.get(idx) {
                        if let Some(nexus_api::Value::Table { columns, rows }) = &block.native_output {
                            let tsv = table_to_tsv(columns, rows);
                            let filename = format!("{}-output.tsv", block.command.split_whitespace().next().unwrap_or("block"));
                            match write_drag_temp_file(&filename, tsv.as_bytes()) {
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
        match action {
            AnchorAction::RevealPath(path) => {
                // Reveal in Finder (macOS) — `open -R <path>`
                let _ = std::process::Command::new("open")
                    .arg("-R")
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

/// Convert a table Value to TSV (tab-separated values) string.
/// Write ephemeral drag data to a temp file, returning the path.
/// Keeps the platform drag layer I/O-free.
fn write_drag_temp_file(filename: &str, data: &[u8]) -> Result<std::path::PathBuf, std::io::Error> {
    let temp_dir = std::env::temp_dir().join("nexus-drag");
    std::fs::create_dir_all(&temp_dir)?;
    let path = temp_dir.join(filename);
    std::fs::write(&path, data)?;
    Ok(path)
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
