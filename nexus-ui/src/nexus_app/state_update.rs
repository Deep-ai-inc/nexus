//! Message dispatch and domain handlers for NexusState.

use nexus_api::Value;
use strata::Command;

use crate::blocks::Focus;

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::input::InputOutput;
use super::message::{ContextMenuMsg, NexusMessage};
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
        }
        Command::none()
    }
}
