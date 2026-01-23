//! Nexus Providers - Built-in providers for command intelligence.

mod registry;
mod git;
mod filesystem;

pub use registry::{Provider, ProviderRegistry};
pub use git::GitProvider;
pub use filesystem::FilesystemProvider;
