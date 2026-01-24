//! Command registry for looking up in-process commands.

use super::{head::HeadCommand, ls::LsCommand, NexusCommand};
use std::collections::HashMap;

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

        // Register built-in commands
        registry.register(HeadCommand);
        registry.register(LsCommand);

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
