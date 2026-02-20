//! Command registry for looking up in-process commands.

use super::NexusCommand;
use std::collections::HashMap;

// Import all commands
use super::basic::{
    EchoCommand, FalseCommand, HostnameCommand, PwdCommand, SleepCommand, TrueCommand,
    WhoamiCommand, YesCommand,
};
use super::cat::CatCommand;
use super::date::DateCommand;
use super::df::DfCommand;
use super::du::DuCommand;
use super::env::{EnvCommand, ExportCommand, PrintenvCommand, UnsetCommand};
use super::find::FindCommand;
use super::fs::{CpCommand, MkdirCommand, MvCommand, RmCommand, RmdirCommand, TouchCommand};
use super::grep::GrepCommand;
use super::less::LessCommand;
use super::head::HeadCommand;
use super::history::{FcCommand, HistoryCommand};
use super::iterators::{
    AllCommand, AnyCommand, EachCommand, FilterCommand, GroupByCommand, MapCommand, ReduceCommand,
    WhereCommand,
};
use super::jobs::{BgCommand, FgCommand, JobsCommand, WaitCommand};
use super::json::{FromJsonCommand, GetCommand, ToJsonCommand};
use super::ls::LsCommand;
use super::man::ManCommand;
use super::math::{AvgCommand, CountCommand, MaxCommand, MinCommand, SumCommand};
use super::path::{BasenameCommand, DirnameCommand, ExtnameCommand, RealpathCommand, StemCommand};
use super::prev::{OutputsCommand, Prev1Command, Prev2Command, Prev3Command, PrevCommand};
use super::ps::PsCommand;
use super::select::{
    CompactCommand, EnumerateCommand, FirstCommand, FlattenCommand, LastCommand, NthCommand,
    ReverseCommand, SkipCommand, TakeCommand,
};
use super::seq::SeqCommand;
use super::shuf::ShufCommand;
use super::signal::KillCommand;
use super::sort::SortCommand;
use super::system::{TtyCommand, UmaskCommand, UnameCommand};
use super::split::{
    BytesCommand, CharsCommand, JoinCommand, LinesCommand, SplitCommand, WordsCommand,
};
use super::tail::TailCommand;
use super::tee::TeeCommand;
use super::top::TopCommand;
use super::tree::TreeCommand;
use super::unicode_stress::UnicodeStressCommand;
use super::uniq::UniqCommand;
use super::wc::WcCommand;
use super::which::{TypeCommand, WhichCommand};

/// Registry of all available in-process commands.
pub struct CommandRegistry {
    commands: HashMap<&'static str, Box<dyn NexusCommand>>,
}

impl CommandRegistry {
    /// Create a new registry with all built-in commands registered.
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };

        // Basic commands
        registry.register(EchoCommand);
        registry.register(PwdCommand);
        registry.register(TrueCommand);
        registry.register(FalseCommand);
        registry.register(WhoamiCommand);
        registry.register(HostnameCommand);
        registry.register(YesCommand);
        registry.register(SleepCommand);

        // File listing/viewing
        registry.register(LsCommand);
        registry.register(CatCommand);
        registry.register(HeadCommand);
        registry.register(TailCommand);

        // Text processing
        registry.register(GrepCommand);
        registry.register(SortCommand);
        registry.register(UniqCommand);
        registry.register(WcCommand);

        // Math/aggregation
        registry.register(SumCommand);
        registry.register(AvgCommand);
        registry.register(MinCommand);
        registry.register(MaxCommand);
        registry.register(CountCommand);

        // Path manipulation
        registry.register(BasenameCommand);
        registry.register(DirnameCommand);
        registry.register(RealpathCommand);
        registry.register(ExtnameCommand);
        registry.register(StemCommand);

        // Splitting/joining
        registry.register(LinesCommand);
        registry.register(WordsCommand);
        registry.register(CharsCommand);
        registry.register(BytesCommand);
        registry.register(SplitCommand);
        registry.register(JoinCommand);

        // Selection
        registry.register(FirstCommand);
        registry.register(LastCommand);
        registry.register(NthCommand);
        registry.register(SkipCommand);
        registry.register(TakeCommand);
        registry.register(FlattenCommand);
        registry.register(CompactCommand);
        registry.register(ReverseCommand);
        registry.register(EnumerateCommand);

        // Iterators (next-gen structured data commands)
        registry.register(EachCommand);
        registry.register(MapCommand);
        registry.register(FilterCommand);
        registry.register(WhereCommand);
        registry.register(ReduceCommand);
        registry.register(AnyCommand);
        registry.register(AllCommand);
        registry.register(GroupByCommand);

        // Random/sequence
        registry.register(ShufCommand);
        registry.register(SeqCommand);

        // JSON
        registry.register(FromJsonCommand);
        registry.register(ToJsonCommand);
        registry.register(GetCommand);

        // Environment
        registry.register(EnvCommand);
        registry.register(PrintenvCommand);
        registry.register(ExportCommand);
        registry.register(UnsetCommand);

        // Date/time
        registry.register(DateCommand);

        // Job control
        registry.register(JobsCommand);
        registry.register(FgCommand);
        registry.register(BgCommand);
        registry.register(WaitCommand);
        registry.register(KillCommand);

        // File finding
        registry.register(FindCommand);

        // Filesystem operations
        registry.register(TouchCommand);
        registry.register(MkdirCommand);
        registry.register(RmCommand);
        registry.register(RmdirCommand);
        registry.register(CpCommand);
        registry.register(MvCommand);

        // I/O
        registry.register(TeeCommand);

        // System info
        registry.register(TtyCommand);
        registry.register(UnameCommand);
        registry.register(UmaskCommand);

        // Disk usage
        registry.register(DuCommand);
        registry.register(DfCommand);

        // Resource/process info
        registry.register(PsCommand);
        registry.register(FcCommand);
        registry.register(HistoryCommand);

        // Command lookup
        registry.register(WhichCommand);
        registry.register(TypeCommand);

        // Persistent memory - access previous outputs
        registry.register(PrevCommand);    // _ - last output
        registry.register(Prev1Command);   // _1 - most recent
        registry.register(Prev2Command);   // _2 - second most recent
        registry.register(Prev3Command);   // _3 - third most recent
        registry.register(OutputsCommand); // outputs - list recent outputs

        // Interactive viewers
        registry.register(LessCommand);
        registry.register(TopCommand);
        registry.register(ManCommand);
        registry.register(TreeCommand);

        // Testing
        registry.register(UnicodeStressCommand);

        registry
    }

    /// Register a command.
    fn register<C: NexusCommand + 'static>(&mut self, cmd: C) {
        self.commands.insert(cmd.name(), Box::new(cmd));
    }

    /// Look up a command by name.
    pub fn get(&self, name: &str) -> Option<&dyn NexusCommand> {
        self.commands.get(name).map(|c| c.as_ref())
    }

    /// Check if a command is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    /// List all registered command names.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.commands.keys().copied()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new_creates_commands() {
        let registry = CommandRegistry::new();
        // Should have many commands registered
        let count = registry.names().count();
        assert!(count > 70, "Expected 70+ commands, got {}", count);
    }

    #[test]
    fn test_registry_default_same_as_new() {
        let new_registry = CommandRegistry::new();
        let default_registry = CommandRegistry::default();
        assert_eq!(
            new_registry.names().count(),
            default_registry.names().count()
        );
    }

    #[test]
    fn test_registry_contains_basic_commands() {
        let registry = CommandRegistry::new();
        assert!(registry.contains("echo"));
        assert!(registry.contains("ls"));
        assert!(registry.contains("cat"));
        assert!(registry.contains("grep"));
        assert!(registry.contains("pwd"));
    }

    #[test]
    fn test_registry_contains_math_commands() {
        let registry = CommandRegistry::new();
        assert!(registry.contains("sum"));
        assert!(registry.contains("avg"));
        assert!(registry.contains("min"));
        assert!(registry.contains("max"));
        assert!(registry.contains("count"));
    }

    #[test]
    fn test_registry_contains_path_commands() {
        let registry = CommandRegistry::new();
        assert!(registry.contains("basename"));
        assert!(registry.contains("dirname"));
        assert!(registry.contains("realpath"));
    }

    #[test]
    fn test_registry_does_not_contain_invalid() {
        let registry = CommandRegistry::new();
        assert!(!registry.contains("nonexistent-command"));
        assert!(!registry.contains(""));
        assert!(!registry.contains("invalid_cmd_xyz"));
    }

    #[test]
    fn test_registry_get_returns_command() {
        let registry = CommandRegistry::new();
        let cmd = registry.get("echo");
        assert!(cmd.is_some());
        assert_eq!(cmd.unwrap().name(), "echo");
    }

    #[test]
    fn test_registry_get_returns_none_for_invalid() {
        let registry = CommandRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_names_returns_all() {
        let registry = CommandRegistry::new();
        let names: Vec<&str> = registry.names().collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"ls"));
        assert!(names.contains(&"grep"));
    }

    #[test]
    fn test_registered_commands_have_correct_names() {
        let registry = CommandRegistry::new();
        // Verify a few commands return their expected names
        assert_eq!(registry.get("wc").unwrap().name(), "wc");
        assert_eq!(registry.get("sort").unwrap().name(), "sort");
        assert_eq!(registry.get("head").unwrap().name(), "head");
        assert_eq!(registry.get("tail").unwrap().name(), "tail");
    }
}
