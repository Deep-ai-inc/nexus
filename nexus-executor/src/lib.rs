use anyhow::Result;
use std::path::{Path, PathBuf};

mod default_executor;
mod nexus_executor;
mod sandboxed_executor;

pub use default_executor::DefaultCommandExecutor;
pub use nexus_executor::{KernelLike, NexusCommandExecutor, serialize_value_for_llm};
pub use sandboxed_executor::SandboxedCommandExecutor;

#[derive(Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub output: String,
}

#[derive(Clone, Debug, Default)]
pub struct SandboxCommandRequest {
    pub writable_roots: Vec<PathBuf>,
    pub read_only: bool,
    pub bypass_sandbox: bool,
}

/// Callback trait for streaming command output
pub trait StreamingCallback: Send + Sync {
    fn on_output_chunk(&self, chunk: &str) -> Result<()>;

    fn on_terminal_attached(&self, _terminal_id: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait CommandExecutor: Send + Sync {
    async fn execute(
        &self,
        command_line: &str,
        working_dir: Option<&PathBuf>,
        sandbox_request: Option<&SandboxCommandRequest>,
    ) -> Result<CommandOutput>;

    /// Execute command with streaming output callback
    async fn execute_streaming(
        &self,
        command_line: &str,
        working_dir: Option<&PathBuf>,
        callback: Option<&dyn StreamingCallback>,
        sandbox_request: Option<&SandboxCommandRequest>,
    ) -> Result<CommandOutput>;
}

/// Quote a path for the current platform so spaces and special chars are preserved when passed
/// through the shell. This is a best-effort helper; it does not aim to be a full shell-quoting lib.
pub fn shell_quote_path(path: &Path) -> String {
    #[cfg(target_family = "unix")]
    {
        let s = path.to_string_lossy();
        // Only quote if whitespace is present; basic behavior for tests
        if s.chars().any(|c| c.is_whitespace()) {
            let escaped = s.replace('\'', "'\\''");
            format!("'{escaped}'")
        } else {
            s.to_string()
        }
    }

    #[cfg(target_family = "windows")]
    {
        let s = path.to_string_lossy();
        if s.chars().any(|c| c.is_whitespace()) {
            // Surround with double quotes and escape internal quotes by doubling them
            let escaped = s.replace('"', "\"\"");
            format!("\"{escaped}\"")
        } else {
            s.to_string()
        }
    }
}

/// Build a formatter command line from a template. If the template contains the {path} placeholder,
/// it will be replaced with the (quoted) relative path. If not present, the template is returned as-is.
pub fn build_format_command(template: &str, relative_path: &Path) -> String {
    if template.contains("{path}") {
        let quoted = shell_quote_path(relative_path);
        template.replace("{path}", &quoted)
    } else {
        template.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // shell_quote_path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_shell_quote_path_simple() {
        let path = Path::new("/usr/bin/test");
        let quoted = shell_quote_path(path);
        assert_eq!(quoted, "/usr/bin/test");
    }

    #[test]
    fn test_shell_quote_path_with_spaces() {
        let path = Path::new("/path/with spaces/file.txt");
        let quoted = shell_quote_path(path);
        #[cfg(target_family = "unix")]
        assert_eq!(quoted, "'/path/with spaces/file.txt'");
        #[cfg(target_family = "windows")]
        assert_eq!(quoted, "\"/path/with spaces/file.txt\"");
    }

    #[test]
    fn test_shell_quote_path_with_tab() {
        let path = Path::new("/path/with\ttab");
        let quoted = shell_quote_path(path);
        // Should be quoted since tab is whitespace
        assert!(quoted.starts_with('\'') || quoted.starts_with('"'));
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn test_shell_quote_path_with_single_quote_unix() {
        let path = Path::new("/path/with'quote");
        let quoted = shell_quote_path(path);
        // No whitespace, so not quoted
        assert_eq!(quoted, "/path/with'quote");
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn test_shell_quote_path_with_spaces_and_quote_unix() {
        let path = Path::new("/path with 'quote");
        let quoted = shell_quote_path(path);
        // Has whitespace + quote, quote should be escaped
        assert!(quoted.contains("'\\''"));
    }

    // -------------------------------------------------------------------------
    // build_format_command tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_format_command_with_placeholder() {
        let template = "rustfmt {path}";
        let path = Path::new("src/main.rs");
        let result = build_format_command(template, path);
        assert_eq!(result, "rustfmt src/main.rs");
    }

    #[test]
    fn test_build_format_command_without_placeholder() {
        let template = "cargo fmt";
        let path = Path::new("src/main.rs");
        let result = build_format_command(template, path);
        assert_eq!(result, "cargo fmt");
    }

    #[test]
    fn test_build_format_command_path_with_spaces() {
        let template = "format {path}";
        let path = Path::new("my file.rs");
        let result = build_format_command(template, path);
        #[cfg(target_family = "unix")]
        assert_eq!(result, "format 'my file.rs'");
        #[cfg(target_family = "windows")]
        assert_eq!(result, "format \"my file.rs\"");
    }

    #[test]
    fn test_build_format_command_multiple_placeholders() {
        let template = "process {path} --output {path}.out";
        let path = Path::new("input.txt");
        let result = build_format_command(template, path);
        assert_eq!(result, "process input.txt --output input.txt.out");
    }

    // -------------------------------------------------------------------------
    // CommandOutput tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_command_output_clone() {
        let output = CommandOutput {
            success: true,
            output: "hello".to_string(),
        };
        let cloned = output.clone();
        assert_eq!(output.success, cloned.success);
        assert_eq!(output.output, cloned.output);
    }

    // -------------------------------------------------------------------------
    // SandboxCommandRequest tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sandbox_command_request_default() {
        let request = SandboxCommandRequest::default();
        assert!(request.writable_roots.is_empty());
        assert!(!request.read_only);
        assert!(!request.bypass_sandbox);
    }

    #[test]
    fn test_sandbox_command_request_clone() {
        let request = SandboxCommandRequest {
            writable_roots: vec![PathBuf::from("/tmp")],
            read_only: true,
            bypass_sandbox: false,
        };
        let cloned = request.clone();
        assert_eq!(request.writable_roots, cloned.writable_roots);
        assert_eq!(request.read_only, cloned.read_only);
    }

    #[test]
    fn test_sandbox_command_request_debug() {
        let request = SandboxCommandRequest::default();
        let debug_str = format!("{:?}", request);
        assert!(debug_str.contains("writable_roots"));
    }
}
