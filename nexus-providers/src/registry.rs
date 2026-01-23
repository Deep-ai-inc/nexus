//! Provider registry - manages and routes to providers.

use std::collections::HashMap;

use nexus_api::{
    Anchor, Completion, Documentation, ParsedSidecarOutput, ProviderRequest, ProviderResponse,
    SidecarSpec,
};

/// Trait that all providers must implement.
pub trait Provider: Send + Sync {
    /// Get the provider name.
    fn name(&self) -> &str;

    /// Get the command patterns this provider handles.
    fn handles(&self) -> &[&str];

    /// Check if this provider handles a command.
    fn matches(&self, command: &str) -> bool {
        let cmd_name = command.split_whitespace().next().unwrap_or("");
        self.handles().iter().any(|pattern| {
            if pattern.ends_with(" *") {
                let prefix = &pattern[..pattern.len() - 2];
                cmd_name == prefix
            } else {
                cmd_name == *pattern
            }
        })
    }

    /// Get completions for a command line.
    fn get_completions(&self, command_line: &str, cursor_pos: usize) -> Vec<Completion>;

    /// Get documentation for an argument.
    fn get_documentation(&self, command_line: &str, cursor_pos: usize) -> Option<Documentation>;

    /// Get the sidecar command for a user command.
    fn get_sidecar(&self, command: &str) -> Option<SidecarSpec>;

    /// Parse sidecar output into structured data.
    fn parse_sidecar_output(&self, output: &str) -> ParsedSidecarOutput;
}

/// Registry of all available providers.
pub struct ProviderRegistry {
    providers: Vec<Box<dyn Provider>>,
    command_map: HashMap<String, usize>,
}

impl ProviderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            command_map: HashMap::new(),
        }
    }

    /// Create a registry with built-in providers.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::GitProvider::new()));
        registry.register(Box::new(super::FilesystemProvider::new()));
        registry
    }

    /// Register a provider.
    pub fn register(&mut self, provider: Box<dyn Provider>) {
        let index = self.providers.len();

        for pattern in provider.handles() {
            let key = if pattern.ends_with(" *") {
                pattern[..pattern.len() - 2].to_string()
            } else {
                pattern.to_string()
            };
            self.command_map.insert(key, index);
        }

        self.providers.push(provider);
    }

    /// Find a provider for a command.
    pub fn find(&self, command: &str) -> Option<&dyn Provider> {
        let cmd_name = command.split_whitespace().next()?;

        self.command_map
            .get(cmd_name)
            .map(|&idx| self.providers[idx].as_ref())
    }

    /// Handle a provider request.
    pub fn handle(&self, command: &str, request: ProviderRequest) -> ProviderResponse {
        let provider = match self.find(command) {
            Some(p) => p,
            None => {
                return ProviderResponse::Error {
                    message: "No provider found for command".to_string(),
                }
            }
        };

        match request {
            ProviderRequest::GetCompletions {
                command_line,
                cursor_position,
            } => {
                let completions = provider.get_completions(&command_line, cursor_position);
                ProviderResponse::Completions(completions)
            }

            ProviderRequest::GetDocumentation {
                command_line,
                cursor_position,
            } => {
                let doc = provider.get_documentation(&command_line, cursor_position);
                ProviderResponse::Documentation(doc)
            }

            ProviderRequest::GetSidecar { command } => {
                let sidecar = provider.get_sidecar(&command);
                ProviderResponse::Sidecar(sidecar)
            }

            ProviderRequest::ParseSidecarOutput { output } => {
                let parsed = provider.parse_sidecar_output(&output);
                ProviderResponse::ParsedOutput(parsed)
            }

            ProviderRequest::GetActions { anchor: _ } => {
                // TODO: Implement actions
                ProviderResponse::Actions(vec![])
            }
        }
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
