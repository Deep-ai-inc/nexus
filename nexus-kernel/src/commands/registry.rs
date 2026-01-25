//! Command registry for looking up in-process commands.

use super::NexusCommand;
use std::collections::HashMap;

// Import all commands
use super::basic::{
    EchoCommand, FalseCommand, HostnameCommand, PwdCommand, SleepCommand, TrueCommand,
    WhoamiCommand, YesCommand,
};
use super::cat::CatCommand;
use super::claude::ClaudeCommand;
use super::cmp::CmpCommand;
use super::cut::CutCommand;
use super::date::DateCommand;
use super::env::{EnvCommand, ExportCommand, PrintenvCommand, UnsetCommand};
use super::find::FindCommand;
use super::fs::{CpCommand, MkdirCommand, MvCommand, RmCommand, RmdirCommand, TouchCommand};
use super::git::{
    GitAddCommand, GitBranchCommand, GitCommand, GitCommitCommand, GitDiffCommand,
    GitLogCommand, GitRemoteCommand, GitStashCommand, GitStatusCommand,
};
use super::grep::GrepCommand;
use super::hash::HashCommand;
use super::head::HeadCommand;
use super::history::{FcCommand, HistoryCommand};
use super::jobs::{BgCommand, FgCommand, JobsCommand, WaitCommand};
use super::json::{FromJsonCommand, GetCommand, ToJsonCommand};
use super::links::{LinkCommand, LnCommand, UnlinkCommand};
use super::ls::LsCommand;
use super::math::{AvgCommand, CountCommand, MaxCommand, MinCommand, SumCommand};
use super::nl::NlCommand;
use super::path::{BasenameCommand, DirnameCommand, ExtnameCommand, RealpathCommand, StemCommand};
use super::perms::{ChgrpCommand, ChmodCommand, ChownCommand};
use super::prev::{OutputsCommand, Prev1Command, Prev2Command, Prev3Command, PrevCommand};
use super::printf::PrintfCommand;
use super::rev::{RevCommand, TacCommand};
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
use super::times::TimesCommand;
use super::ulimit::UlimitCommand;
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

        // Formatting
        registry.register(PrintfCommand);

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

        // Permissions
        registry.register(ChmodCommand);
        registry.register(ChownCommand);
        registry.register(ChgrpCommand);

        // Links
        registry.register(LnCommand);
        registry.register(LinkCommand);
        registry.register(UnlinkCommand);

        // I/O
        registry.register(TeeCommand);

        // System info
        registry.register(TtyCommand);
        registry.register(UnameCommand);
        registry.register(UmaskCommand);
        registry.register(CmpCommand);

        // Resource/process info
        registry.register(UlimitCommand);
        registry.register(TimesCommand);
        registry.register(HashCommand);
        registry.register(FcCommand);
        registry.register(HistoryCommand);

        // Command lookup
        registry.register(WhichCommand);
        registry.register(TypeCommand);

        // Claude integration
        registry.register(ClaudeCommand);

        // Persistent memory - access previous outputs
        registry.register(PrevCommand);    // _ - last output
        registry.register(Prev1Command);   // _1 - most recent
        registry.register(Prev2Command);   // _2 - second most recent
        registry.register(Prev3Command);   // _3 - third most recent
        registry.register(OutputsCommand); // outputs - list recent outputs

        // Git commands (native, structured output)
        registry.register(GitCommand);  // Main dispatcher: git <subcommand>
        registry.register(GitStatusCommand);
        registry.register(GitLogCommand);
        registry.register(GitBranchCommand);
        registry.register(GitDiffCommand);
        registry.register(GitAddCommand);
        registry.register(GitCommitCommand);
        registry.register(GitRemoteCommand);
        registry.register(GitStashCommand);

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
