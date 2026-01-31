//! Message dispatch and domain handlers for NexusState.

use nexus_api::Value;
use strata::event_context::KeyEvent;
use strata::{Command, ImageStore};

use crate::blocks::Focus;

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::input::InputOutput;
use super::message::{
    AgentMsg, ContextMenuMsg, InputMsg, NexusMessage, SelectionMsg, ShellMsg,
};
use super::selection;
use super::shell::ShellOutput;
use super::agent::AgentOutput;
use super::NexusState;
use crate::shell_context::build_shell_context;

// =========================================================================
// Cross-cutting output handlers
// =========================================================================

impl NexusState {
    pub(super) fn apply_shell_output(&mut self, output: ShellOutput) {
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

    pub(super) fn apply_agent_output(&mut self, output: AgentOutput) {
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
    /// Top-level message dispatch. Called from Component::update().
    pub(super) fn dispatch_update(
        &mut self,
        msg: NexusMessage,
        images: &mut ImageStore,
    ) -> Command<NexusMessage> {
        match msg {
            NexusMessage::Input(m) => self.dispatch_input(m),
            NexusMessage::Shell(m) => self.dispatch_shell(m, images),
            NexusMessage::Agent(m) => self.dispatch_agent(m),
            NexusMessage::Selection(m) => { self.dispatch_selection(m); Command::none() }
            NexusMessage::ContextMenu(m) => self.dispatch_context_menu(m),
            NexusMessage::Scroll(action) => { self.scroll.apply_user_scroll(action); Command::none() }
            NexusMessage::ScrollToJob(_) => { self.scroll.force(); Command::none() }
            NexusMessage::Copy => { self.copy_selection_or_input(); Command::none() }
            NexusMessage::Paste => { self.paste_from_clipboard(images); Command::none() }
            NexusMessage::ClearScreen => { self.clear_screen(); Command::none() }
            NexusMessage::CloseWindow => { self.exit_requested = true; Command::none() }
            NexusMessage::BlurAll => {
                self.transient.dismiss_all(&mut self.input);
                self.set_focus_input();
                Command::none()
            }
            NexusMessage::Tick => { self.on_output_arrived(); Command::none() }
        }
    }
}

// =========================================================================
// Domain dispatchers
// =========================================================================

impl NexusState {
    fn dispatch_input(&mut self, msg: InputMsg) -> Command<NexusMessage> {
        match msg {
            InputMsg::Key(event) => self.on_input_key(event),
            InputMsg::Mouse(action) => { self.input.handle_mouse(action); Command::none() }
            InputMsg::Submit(text) => self.on_submit_message(text),
            InputMsg::ToggleMode => { self.input.toggle_mode(); Command::none() }
            InputMsg::HistoryUp => { self.input.history_up(); Command::none() }
            InputMsg::HistoryDown => { self.input.history_down(); Command::none() }
            InputMsg::InsertNewline => { self.input.insert_newline(); Command::none() }
            InputMsg::RemoveAttachment(idx) => { self.input.remove_attachment(idx); Command::none() }

            InputMsg::TabComplete => { self.input.tab_complete(&self.kernel); Command::none() }
            InputMsg::CompletionNav(delta) => { self.input.completion_nav(delta); Command::none() }
            InputMsg::CompletionAccept => { self.input.completion_accept(); Command::none() }
            InputMsg::CompletionDismiss => { self.input.completion_dismiss(); Command::none() }
            InputMsg::CompletionDismissAndForward(event) => {
                self.on_completion_dismiss_and_forward(event)
            }
            InputMsg::CompletionSelect(index) => { self.input.completion_select(index); Command::none() }
            InputMsg::CompletionScroll(action) => { self.input.completion.apply_scroll(action); Command::none() }

            InputMsg::HistorySearchToggle => { self.input.history_search_toggle(); Command::none() }
            InputMsg::HistorySearchKey(key_event) => {
                self.input.history_search_key(key_event, &self.kernel);
                Command::none()
            }
            InputMsg::HistorySearchAccept => { self.input.history_search_accept(); Command::none() }
            InputMsg::HistorySearchDismiss => { self.input.history_search_dismiss(); Command::none() }
            InputMsg::HistorySearchSelect(index) => { self.input.history_search_select(index); Command::none() }
            InputMsg::HistorySearchAcceptIndex(index) => { self.input.history_search_accept_index(index); Command::none() }
            InputMsg::HistorySearchScroll(action) => { self.input.history_search.apply_scroll(action); Command::none() }
        }
    }

    fn dispatch_shell(
        &mut self,
        msg: ShellMsg,
        images: &mut ImageStore,
    ) -> Command<NexusMessage> {
        match msg {
            ShellMsg::PtyOutput(id, data) => {
                let out = self.shell.handle_pty_output(id, data);
                self.apply_shell_output(out);
            }
            ShellMsg::PtyExited(id, exit_code) => {
                let out = self.shell.handle_pty_exited(id, exit_code, &self.focus);
                self.apply_shell_output(out);
            }
            ShellMsg::KernelEvent(evt) => {
                let out = self.shell.handle_kernel_event(evt, images);
                self.apply_shell_output(out);
            }
            ShellMsg::SendInterrupt(id) => { self.shell.send_interrupt_to(id); }
            ShellMsg::KillBlock(id) => { self.shell.kill_block(id); }
            ShellMsg::PtyInput(event) => {
                if let Focus::Block(block_id) = self.focus {
                    if !self.shell.forward_key(block_id, &event) {
                        self.set_focus_input();
                    }
                }
            }
            ShellMsg::SortTable(block_id, col_idx) => { self.shell.sort_table(block_id, col_idx); }
        }
        Command::none()
    }

    fn dispatch_agent(&mut self, msg: AgentMsg) -> Command<NexusMessage> {
        match msg {
            AgentMsg::Event(evt) => {
                self.agent.dirty = true;
                let out = self.agent.handle_event(evt);
                self.apply_agent_output(out);
            }
            AgentMsg::ToggleThinking(id) => { self.agent.toggle_thinking(id); }
            AgentMsg::ToggleTool(id, idx) => { self.agent.toggle_tool(id, idx); }
            AgentMsg::PermissionGrant(block_id, perm_id) => { self.agent.permission_grant(block_id, perm_id); }
            AgentMsg::PermissionGrantSession(block_id, perm_id) => { self.agent.permission_grant_session(block_id, perm_id); }
            AgentMsg::PermissionDeny(block_id, perm_id) => { self.agent.permission_deny(block_id, perm_id); }
            AgentMsg::Interrupt => { self.agent.interrupt(); }
        }
        Command::none()
    }

    fn dispatch_selection(&mut self, msg: SelectionMsg) {
        match msg {
            SelectionMsg::Start(addr) => { self.selection.start(addr); }
            SelectionMsg::Extend(addr) => { self.selection.extend(addr); }
            SelectionMsg::End => { self.selection.end(); }
            SelectionMsg::Clear => { self.selection.clear(); }
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
            self.apply_shell_output(output);
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
