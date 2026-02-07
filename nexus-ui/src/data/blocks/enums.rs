//! Simple enums: Focus, InputMode, ProcSort.

use nexus_api::BlockId;

/// Focus state - makes illegal states unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    /// The command input field is focused.
    Input,
    /// A specific block is focused for interaction.
    Block(BlockId),
    /// The agent question text input is focused.
    AgentInput,
}

/// Input mode - determines how commands are processed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum InputMode {
    /// Normal shell mode - commands are executed by the kernel.
    #[default]
    Shell,
    /// Agent mode - input is sent to the AI agent.
    Agent,
}

/// Sort criteria for process monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcSort {
    Cpu,
    Mem,
    Pid,
    Command,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Focus tests ==========

    #[test]
    fn test_focus_input() {
        let focus = Focus::Input;
        assert_eq!(focus, Focus::Input);
    }

    #[test]
    fn test_focus_block() {
        let focus = Focus::Block(BlockId(42));
        if let Focus::Block(id) = focus {
            assert_eq!(id.0, 42);
        } else {
            panic!("Expected Focus::Block");
        }
    }

    #[test]
    fn test_focus_agent_input() {
        let focus = Focus::AgentInput;
        assert_eq!(focus, Focus::AgentInput);
    }

    #[test]
    fn test_focus_ne() {
        assert_ne!(Focus::Input, Focus::AgentInput);
        assert_ne!(Focus::Input, Focus::Block(BlockId(1)));
        assert_ne!(Focus::Block(BlockId(1)), Focus::Block(BlockId(2)));
    }

    #[test]
    fn test_focus_clone() {
        let focus = Focus::Block(BlockId(5));
        let cloned = focus;
        assert_eq!(cloned, Focus::Block(BlockId(5)));
    }

    // ========== InputMode tests ==========

    #[test]
    fn test_input_mode_default_is_shell() {
        let mode: InputMode = Default::default();
        assert_eq!(mode, InputMode::Shell);
    }

    #[test]
    fn test_input_mode_variants() {
        assert_eq!(InputMode::Shell, InputMode::Shell);
        assert_eq!(InputMode::Agent, InputMode::Agent);
        assert_ne!(InputMode::Shell, InputMode::Agent);
    }

    #[test]
    fn test_input_mode_clone() {
        let mode = InputMode::Agent;
        let cloned = mode;
        assert_eq!(cloned, InputMode::Agent);
    }

    // ========== ProcSort tests ==========

    #[test]
    fn test_proc_sort_variants() {
        assert_eq!(ProcSort::Cpu, ProcSort::Cpu);
        assert_eq!(ProcSort::Mem, ProcSort::Mem);
        assert_eq!(ProcSort::Pid, ProcSort::Pid);
        assert_eq!(ProcSort::Command, ProcSort::Command);
    }

    #[test]
    fn test_proc_sort_ne() {
        assert_ne!(ProcSort::Cpu, ProcSort::Mem);
        assert_ne!(ProcSort::Pid, ProcSort::Command);
    }

    #[test]
    fn test_proc_sort_clone() {
        let sort = ProcSort::Cpu;
        let cloned = sort;
        assert_eq!(cloned, ProcSort::Cpu);
    }

    #[test]
    fn test_proc_sort_debug() {
        let debug_str = format!("{:?}", ProcSort::Cpu);
        assert_eq!(debug_str, "Cpu");
    }
}
