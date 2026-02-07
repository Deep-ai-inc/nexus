//! UpdateContext — shared mutable state passed into widget update methods.
//!
//! Replaces the Output enum indirection (ShellOutput, AgentOutput) with direct
//! method calls. Widgets call `ctx.snap_to_bottom()` instead of returning
//! `ShellOutput::ScrollToBottom`.

use std::path::PathBuf;

use strata::Command;

use crate::blocks::Focus;
use crate::context::NexusContext;

use super::message::NexusMessage;
use super::scroll_model::ScrollModel;

/// Shared mutable state that widget update methods can modify directly.
///
/// Created via borrow-splitting helpers on NexusState (e.g. `shell_ctx()`).
/// Widgets call methods on this instead of returning Output enums.
pub(crate) struct UpdateContext<'a> {
    pub scroll: &'a mut ScrollModel,
    pub focus: &'a mut Focus,
    pub cwd: &'a mut String,
    pub context: &'a mut NexusContext,
    pub commands: Vec<Command<NexusMessage>>,
}

impl<'a> UpdateContext<'a> {
    pub fn new(
        scroll: &'a mut ScrollModel,
        focus: &'a mut Focus,
        cwd: &'a mut String,
        context: &'a mut NexusContext,
    ) -> Self {
        Self {
            scroll,
            focus,
            cwd,
            context,
            commands: Vec::new(),
        }
    }

    /// Set the application focus target.
    pub fn set_focus(&mut self, focus: Focus) {
        *self.focus = focus;
    }

    /// Active snap to bottom — viewport will follow new output.
    pub fn snap_to_bottom(&mut self) {
        self.scroll.snap_to_bottom();
    }

    /// Passive hint — returns true if already at bottom.
    pub fn hint_bottom(&mut self) {
        self.scroll.hint_bottom();
    }

    /// Update the working directory and refresh context.
    pub fn set_cwd(&mut self, path: PathBuf) {
        *self.cwd = path.display().to_string();
        self.context.set_cwd(path);
    }

    /// Notify context that a command finished (for error parsing).
    pub fn on_command_finished(&mut self, command: String, output: String, exit_code: i32) {
        self.context.on_command_finished(command, output, exit_code);
    }

    /// Enqueue an async command (e.g. LoadTreeChildren).
    pub fn push_command(&mut self, cmd: Command<NexusMessage>) {
        self.commands.push(cmd);
    }

    /// Drain accumulated commands into a single batched Command.
    pub fn into_commands(self) -> Command<NexusMessage> {
        if self.commands.is_empty() {
            Command::none()
        } else {
            Command::batch(self.commands)
        }
    }
}

/// Synchronize widget focus flags after an UpdateContext-based update.
///
/// UpdateContext can't hold references to input/agent widgets (they're borrowed
/// mutably by the update call), so we sync the boolean flags afterward.
pub(super) fn sync_focus_flags(
    focus: &Focus,
    input: &mut super::input::InputWidget,
    agent: &mut super::agent::AgentWidget,
) {
    input.text_input.focused = matches!(focus, Focus::Input);
    agent.question_input.focused = matches!(focus, Focus::AgentInput);
}
