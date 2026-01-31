//! Message dispatch and domain handlers for NexusState.

use nexus_api::Value;
use strata::event_context::KeyEvent;
use strata::{Command, ImageStore};

use crate::blocks::Focus;

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::input::InputOutput;
use super::message::NexusMessage;
use super::selection;
use super::shell::ShellOutput;
use super::agent::AgentOutput;
use super::{ApplyOutput, NexusState};
use crate::shell_context::build_shell_context;

// =========================================================================
// ApplyOutput implementations
// =========================================================================

impl ApplyOutput<ShellOutput> for NexusState {
    fn apply_output(&mut self, output: ShellOutput) {
        match output {
            ShellOutput::None => {}
            ShellOutput::FocusInput => {
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
        }
    }
}

impl ApplyOutput<AgentOutput> for NexusState {
    fn apply_output(&mut self, output: AgentOutput) {
        match output {
            AgentOutput::ScrollToBottom => {
                self.scroll.hint();
            }
        }
    }
}

// =========================================================================
// Top-level dispatch
// =========================================================================

impl NexusState {
    /// Top-level message dispatch. Called from StrataApp::update().
    pub(super) fn update(
        &mut self,
        msg: NexusMessage,
        images: &mut ImageStore,
    ) -> Command<NexusMessage> {
        self.dispatch(msg, images).unwrap_or_else(Command::none)
    }

    fn dispatch(
        &mut self,
        msg: NexusMessage,
        images: &mut ImageStore,
    ) -> Option<Command<NexusMessage>> {
        use NexusMessage::*;
        match msg {
            // --- Input / Completion / History search → dispatch_input ---
            InputKey(_) | InputMouse(_) | Submit(_) | ToggleMode |
            HistoryUp | HistoryDown | InsertNewline | RemoveAttachment(_) |
            TabComplete | CompletionNav(_) | CompletionAccept | CompletionDismiss |
            CompletionDismissAndForward(_) | CompletionSelect(_) | CompletionScroll(_) |
            HistorySearchToggle | HistorySearchKey(_) | HistorySearchAccept |
            HistorySearchDismiss | HistorySearchSelect(_) | HistorySearchAcceptIndex(_) |
            HistorySearchScroll(_) => {
                self.dispatch_input(msg)
            }

            // --- Shell ---
            PtyOutput(..) | PtyExited(..) | KernelEvent(_) | SendInterrupt |
            KillBlock(_) | PtyInput(_) | SortTable(..) => {
                self.dispatch_shell(msg, images)
            }

            // --- Agent ---
            AgentEvent(_) | ToggleThinking(_) | ToggleTool(..) |
            PermissionGrant(..) | PermissionGrantSession(..) | PermissionDeny(..) |
            AgentInterrupt => {
                self.dispatch_agent(msg)
            }

            // --- Selection ---
            SelectionStart(addr) => { self.selection.start(addr); None }
            SelectionExtend(addr) => { self.selection.extend(addr); None }
            SelectionEnd => { self.selection.end(); None }
            ClearSelection => { self.selection.clear(); None }
            Copy => { self.copy_selection_or_input(); None }

            // --- Scroll ---
            HistoryScroll(action) => { self.scroll.apply_user_scroll(action); None }
            ScrollToJob(_) => { self.scroll.force(); None }

            // --- Context menu ---
            ShowContextMenu(x, y, items, target) => {
                self.transient.show_context_menu(x, y, items, target);
                None
            }
            ContextMenuAction(item) => Some(self.exec_context_menu_item(item)),
            DismissContextMenu => { self.transient.dismiss_context_menu(); None }

            // --- Clipboard ---
            Paste => { self.paste_from_clipboard(images); None }

            // --- Window ---
            ClearScreen => { self.clear_screen(); None }
            CloseWindow => { self.exit_requested = true; None }
            BlurAll => {
                self.transient.dismiss_all(&mut self.input);
                self.set_focus_input();
                None
            }
            Tick => { self.on_output_arrived(); None }
        }
    }
}

// =========================================================================
// Domain dispatchers
// =========================================================================

impl NexusState {
    fn dispatch_input(&mut self, msg: NexusMessage) -> Option<Command<NexusMessage>> {
        use NexusMessage::*;
        match msg {
            InputKey(event) => return Some(self.on_input_key(event)),
            InputMouse(action) => { self.input.handle_mouse(action); }
            Submit(text) => return Some(self.on_submit_message(text)),
            ToggleMode => { self.input.toggle_mode(); }
            HistoryUp => { self.input.history_up(); }
            HistoryDown => { self.input.history_down(); }
            InsertNewline => { self.input.insert_newline(); }
            RemoveAttachment(idx) => { self.input.remove_attachment(idx); }

            TabComplete => { self.input.tab_complete(&self.kernel); }
            CompletionNav(delta) => { self.input.completion_nav(delta); }
            CompletionAccept => { self.input.completion_accept(); }
            CompletionDismiss => { self.input.completion_dismiss(); }
            CompletionDismissAndForward(event) => {
                return Some(self.on_completion_dismiss_and_forward(event));
            }
            CompletionSelect(index) => { self.input.completion_select(index); }
            CompletionScroll(action) => { self.input.completion.apply_scroll(action); }

            HistorySearchToggle => { self.input.history_search_toggle(); }
            HistorySearchKey(key_event) => {
                self.input.history_search_key(key_event, &self.kernel);
            }
            HistorySearchAccept => { self.input.history_search_accept(); }
            HistorySearchDismiss => { self.input.history_search_dismiss(); }
            HistorySearchSelect(index) => { self.input.history_search_select(index); }
            HistorySearchAcceptIndex(index) => { self.input.history_search_accept_index(index); }
            HistorySearchScroll(action) => { self.input.history_search.apply_scroll(action); }

            _ => unreachable!(),
        }
        None
    }

    fn dispatch_shell(
        &mut self,
        msg: NexusMessage,
        images: &mut ImageStore,
    ) -> Option<Command<NexusMessage>> {
        use NexusMessage::*;
        match msg {
            PtyOutput(id, data) => {
                let out = self.shell.handle_pty_output(id, data);
                self.apply_output(out);
            }
            PtyExited(id, exit_code) => {
                let out = self.shell.handle_pty_exited(id, exit_code, &self.focus);
                self.apply_output(out);
            }
            KernelEvent(evt) => {
                let out = self.shell.handle_kernel_event(evt, images);
                self.apply_output(out);
            }
            SendInterrupt => { self.shell.send_interrupt(&self.focus); }
            KillBlock(id) => { self.shell.kill_block(id); }
            PtyInput(event) => {
                if let Focus::Block(block_id) = self.focus {
                    if !self.shell.forward_key(block_id, &event) {
                        self.set_focus_input();
                    }
                }
            }
            SortTable(block_id, col_idx) => { self.shell.sort_table(block_id, col_idx); }
            _ => unreachable!(),
        }
        None
    }

    fn dispatch_agent(&mut self, msg: NexusMessage) -> Option<Command<NexusMessage>> {
        use NexusMessage::*;
        match msg {
            AgentEvent(evt) => {
                self.agent.dirty = true;
                let out = self.agent.handle_event(evt);
                self.apply_output(out);
            }
            ToggleThinking(id) => { self.agent.toggle_thinking(id); }
            ToggleTool(id, idx) => { self.agent.toggle_tool(id, idx); }
            PermissionGrant(block_id, perm_id) => { self.agent.permission_grant(block_id, perm_id); }
            PermissionGrantSession(block_id, perm_id) => { self.agent.permission_grant_session(block_id, perm_id); }
            PermissionDeny(block_id, perm_id) => { self.agent.permission_deny(block_id, perm_id); }
            AgentInterrupt => { self.agent.interrupt(); }
            _ => unreachable!(),
        }
        None
    }
}

// =========================================================================
// Domain handlers
// =========================================================================

impl NexusState {
    fn on_input_key(&mut self, event: KeyEvent) -> Command<NexusMessage> {
        if let InputOutput::Submit { text, is_agent, attachments } = self.input.handle_key(&event) {
            return self.submit(text, is_agent, attachments);
        }
        Command::none()
    }

    fn on_submit_message(&mut self, text: String) -> Command<NexusMessage> {
        if let InputOutput::Submit { text, is_agent, attachments } = self.input.submit(text) {
            return self.submit(text, is_agent, attachments);
        }
        Command::none()
    }

    fn on_completion_dismiss_and_forward(&mut self, event: KeyEvent) -> Command<NexusMessage> {
        self.input.completion_dismiss();
        self.on_input_key(event)
    }

    fn submit(
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
            self.apply_output(output);
        }

        Command::none()
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
        }
        Command::none()
    }
}
