//! Command registry for looking up in-process commands.

use super::NexusCommand;
use std::collections::HashMap;

// Import all commands
use super::basic::{
    EchoCommand, FalseCommand, HostnameCommand, PwdCommand, SleepCommand, TrueCommand,
    WhoamiCommand, YesCommand,
};
use super::cat::CatCommand;
use super::cut::CutCommand;
use super::date::DateCommand;
use super::env::{EnvCommand, ExportCommand, PrintenvCommand, UnsetCommand};
use super::find::FindCommand;
use super::fs::{CpCommand, MkdirCommand, MvCommand, RmCommand, RmdirCommand, TouchCommand};
use super::grep::GrepCommand;
use super::head::HeadCommand;
use super::json::{FromJsonCommand, GetCommand, ToJsonCommand};
use super::ls::LsCommand;
use super::math::{AvgCommand, CountCommand, MaxCommand, MinCommand, SumCommand};
use super::nl::NlCommand;
use super::path::{BasenameCommand, DirnameCommand, ExtnameCommand, RealpathCommand, StemCommand};
use super::rev::{RevCommand, TacCommand};
use super::select::{
    CompactCommand, EnumerateCommand, FirstCommand, FlattenCommand, LastCommand, NthCommand,
    ReverseCommand, SkipCommand, TakeCommand,
};
use super::seq::SeqCommand;
use super::shuf::ShufCommand;
use super::sort::SortCommand;
use super::split::{
    BytesCommand, CharsCommand, JoinCommand, LinesCommand, SplitCommand, WordsCommand,
};
use super::tail::TailCommand;
use super::tee::TeeCommand;
use super::tr::TrCommand;
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
        registry.register(CutCommand);
        registry.register(TrCommand);
        registry.register(RevCommand);
        registry.register(TacCommand);
        registry.register(NlCommand);

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

        // Command lookup
        registry.register(WhichCommand);
        registry.register(TypeCommand);

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
