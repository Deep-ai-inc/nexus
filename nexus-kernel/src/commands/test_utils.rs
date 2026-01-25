//! Test utilities for command testing.
//!
//! Provides a TestContext helper that sets up all the infrastructure
//! needed to test commands in isolation.

#[cfg(test)]
pub mod test_helpers {
    use crate::commands::CommandContext;
    use crate::state::ShellState;
    use nexus_api::{BlockId, ShellEvent, Value};
    use std::path::PathBuf;
    use tokio::sync::broadcast;

    /// A test context that owns all the resources needed for CommandContext.
    pub struct TestContext {
        pub state: ShellState,
        sender: broadcast::Sender<ShellEvent>,
        #[allow(dead_code)]
        receiver: broadcast::Receiver<ShellEvent>,
    }

    impl TestContext {
        /// Create a new test context with the given working directory.
        pub fn new(cwd: PathBuf) -> Self {
            let (sender, receiver) = broadcast::channel(16);
            Self {
                state: ShellState::from_cwd(cwd),
                sender,
                receiver,
            }
        }

        /// Create a new test context with /tmp as the working directory.
        pub fn new_default() -> Self {
            Self::new(PathBuf::from("/tmp"))
        }

        /// Get a CommandContext that borrows from this TestContext.
        pub fn ctx(&mut self) -> CommandContext<'_> {
            CommandContext {
                state: &mut self.state,
                events: &self.sender,
                block_id: BlockId(1),
                stdin: None,
            }
        }

        /// Get a CommandContext with stdin data.
        pub fn ctx_with_stdin(&mut self, stdin: Value) -> CommandContext<'_> {
            CommandContext {
                state: &mut self.state,
                events: &self.sender,
                block_id: BlockId(1),
                stdin: Some(stdin),
            }
        }
    }

    /// Helper to create a test file in a directory.
    #[allow(dead_code)]
    pub fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> PathBuf {
        use std::io::Write;
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content).unwrap();
        path
    }
}
