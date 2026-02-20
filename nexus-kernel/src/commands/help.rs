//! `help` — display command help and documentation.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct HelpCommand;

impl NexusCommand for HelpCommand {
    fn name(&self) -> &'static str {
        "help"
    }

    fn description(&self) -> &'static str {
        "Show help for commands"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(cmd_name) = args.first() {
            show_command_help(cmd_name)
        } else {
            show_overview()
        }
    }
}

fn show_command_help(name: &str) -> anyhow::Result<Value> {
    // Search native commands
    for (_, members) in CATEGORIES {
        for &(cmd_name, desc) in *members {
            if cmd_name == name {
                return Ok(Value::String(format!("{} — {}", name, desc)));
            }
        }
    }

    // Search builtins
    if let Some(desc) = builtin_description(name) {
        return Ok(Value::String(format!("{} — {} (shell builtin)", name, desc)));
    }

    // Search keywords
    let keywords = ["if", "while", "until", "for", "case", "function", "watch"];
    if keywords.contains(&name) {
        return Ok(Value::String(format!("{} — shell keyword", name)));
    }

    anyhow::bail!("help: no help for '{}'", name)
}

fn show_overview() -> anyhow::Result<Value> {
    let mut lines = Vec::new();
    lines.push("Nexus Shell Commands".to_string());
    lines.push("====================".to_string());

    for (category, members) in CATEGORIES {
        lines.push(String::new());
        lines.push(format!("{}:", category));
        for &(name, desc) in *members {
            if desc.is_empty() {
                lines.push(format!("  {}", name));
            } else {
                lines.push(format!("  {:16} {}", name, desc));
            }
        }
    }

    // Builtins
    lines.push(String::new());
    lines.push("Shell Builtins:".to_string());
    for &(name, desc) in BUILTINS {
        lines.push(format!("  {:16} {}", name, desc));
    }

    // Keywords
    lines.push(String::new());
    lines.push("Shell Keywords:".to_string());
    lines.push("  if, while, until, for, case, function, watch".to_string());

    lines.push(String::new());
    lines.push("Type 'help COMMAND' for details on a specific command.".to_string());

    Ok(Value::String(lines.join("\n")))
}

fn builtin_description(name: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|&&(n, _)| *n == name).map(|&&(_, d)| d)
}

const BUILTINS: &[&(&str, &str)] = &[
    &("cd", "Change the working directory"),
    &("exit", "Exit the shell"),
    &("export", "Set an environment variable"),
    &("unset", "Remove a variable"),
    &("set", "Set shell options"),
    &(":", "No-op (always succeeds)"),
    &("test", "Evaluate conditional expression"),
    &("[", "Evaluate conditional expression"),
    &("[[", "Evaluate conditional expression"),
    &("alias", "Define a command alias"),
    &("unalias", "Remove an alias"),
    &("source", "Execute a script in the current shell"),
    &(".", "Execute a script in the current shell"),
    &("eval", "Evaluate a string as a command"),
    &("read", "Read a line into a variable"),
    &("shift", "Shift positional parameters"),
    &("return", "Return from a function"),
    &("break", "Break out of a loop"),
    &("continue", "Continue to next loop iteration"),
    &("readonly", "Mark a variable as read-only"),
    &("command", "Run a command bypassing aliases"),
    &("getopts", "Parse option arguments"),
    &("trap", "Register a signal handler"),
    &("exec", "Replace the shell with a command"),
    &("local", "Declare a local variable"),
];

/// Command catalog, organized by category.
/// Each entry is (category_name, &[(command_name, description)]).
const CATEGORIES: &[(&str, &[(&str, &str)])] = &[
    ("File Listing & Viewing", &[
        ("ls", "List directory contents"),
        ("cat", "Concatenate and display files"),
        ("head", "Show first N lines/items"),
        ("tail", "Show last N lines/items"),
        ("less", "Interactive pager"),
        ("tree", "Display directory tree"),
    ]),
    ("File Operations", &[
        ("touch", "Create files or update timestamps"),
        ("mkdir", "Create directories"),
        ("rm", "Remove files"),
        ("rmdir", "Remove empty directories"),
        ("cp", "Copy files"),
        ("mv", "Move or rename files"),
        ("ln", "Create hard or symbolic links"),
        ("chmod", "Change file permissions"),
    ]),
    ("Text Processing", &[
        ("grep", "Search for patterns"),
        ("sort", "Sort lines or values"),
        ("uniq", "Filter duplicate adjacent lines"),
        ("wc", "Count lines, words, and bytes"),
        ("diff", "Compare two files"),
    ]),
    ("Data Iteration", &[
        ("each", "Run command for each item"),
        ("map", "Transform each item"),
        ("filter", "Keep items matching condition"),
        ("where", "Filter by field condition"),
        ("reduce", "Reduce list to single value"),
        ("any", "Check if any item matches"),
        ("all", "Check if all items match"),
        ("group-by", "Group items by key"),
    ]),
    ("Selection & Slicing", &[
        ("first", "First element"),
        ("last", "Last element"),
        ("nth", "Nth element"),
        ("skip", "Skip first N items"),
        ("take", "Take first N items"),
        ("flatten", "Flatten nested lists"),
        ("compact", "Remove null/empty values"),
        ("reverse", "Reverse order"),
        ("enumerate", "Add index to each item"),
    ]),
    ("Splitting & Joining", &[
        ("lines", "Split string into lines"),
        ("words", "Split string into words"),
        ("chars", "Split string into characters"),
        ("bytes", "Split string into bytes"),
        ("split", "Split by delimiter"),
        ("join", "Join items with delimiter"),
    ]),
    ("Math & Aggregation", &[
        ("sum", "Sum numeric values"),
        ("avg", "Average numeric values"),
        ("min", "Minimum value"),
        ("max", "Maximum value"),
        ("count", "Count items"),
    ]),
    ("Path Manipulation", &[
        ("basename", "Extract filename from path"),
        ("dirname", "Extract directory from path"),
        ("realpath", "Resolve to absolute path"),
        ("extname", "Get file extension"),
        ("stem", "Get filename without extension"),
    ]),
    ("JSON", &[
        ("from-json", "Parse JSON string"),
        ("to-json", "Serialize to JSON"),
        ("get", "Get field from record"),
    ]),
    ("Encoding & Hashing", &[
        ("base64", "Encode or decode base64 data"),
        ("hash", "Compute a hash (md5, sha256, sha512)"),
        ("md5sum", "Compute MD5 hash"),
        ("sha256sum", "Compute SHA-256 hash"),
    ]),
    ("Environment", &[
        ("env", "List environment variables"),
        ("printenv", "Print an environment variable"),
        ("export", "Set environment variable"),
        ("unset", "Remove a variable"),
    ]),
    ("Process & Jobs", &[
        ("ps", "List running processes"),
        ("kill", "Send signal to a process"),
        ("jobs", "List background jobs"),
        ("fg", "Bring job to foreground"),
        ("bg", "Resume job in background"),
        ("wait", "Wait for background jobs"),
        ("top", "Interactive process viewer"),
    ]),
    ("System Info", &[
        ("whoami", "Print current username"),
        ("hostname", "Print hostname"),
        ("uname", "Print system information"),
        ("tty", "Print terminal device"),
        ("umask", "Get or set file creation mask"),
        ("date", "Print current date/time"),
    ]),
    ("Disk & Filesystem", &[
        ("du", "Disk usage"),
        ("df", "Filesystem disk space"),
        ("find", "Find files by name/pattern"),
    ]),
    ("Utilities", &[
        ("echo", "Print arguments"),
        ("printf", "Format and print data"),
        ("pwd", "Print working directory"),
        ("true", "Return success"),
        ("false", "Return failure"),
        ("yes", "Repeatedly output a string"),
        ("sleep", "Pause for N seconds"),
        ("seq", "Generate number sequence"),
        ("shuf", "Shuffle lines"),
        ("tee", "Write to file and pass through"),
    ]),
    ("Clipboard & Desktop", &[
        ("clip", "Copy to or paste from clipboard"),
        ("open", "Open files or URLs in default app"),
    ]),
    ("Command Info", &[
        ("which", "Locate a command"),
        ("type", "Describe a command"),
        ("man", "Display manual pages"),
        ("help", "Show help for commands"),
        ("history", "Command history"),
        ("fc", "Fix command (edit and re-run)"),
    ]),
    ("Pipeline Memory", &[
        ("_", "Last command output"),
        ("_1", "Most recent output"),
        ("_2", "Second most recent output"),
        ("_3", "Third most recent output"),
        ("outputs", "List recent outputs"),
    ]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_help_overview() {
        let mut test_ctx = TestContext::new_default();
        let cmd = HelpCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::String(s) => {
                assert!(s.contains("Nexus Shell Commands"));
                assert!(s.contains("File Operations"));
                assert!(s.contains("Shell Builtins"));
                assert!(s.contains("Shell Keywords"));
            }
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_help_specific_command() {
        let mut test_ctx = TestContext::new_default();
        let cmd = HelpCommand;
        let result = cmd
            .execute(&["ls".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::String(s) => {
                assert!(s.contains("ls"));
                assert!(s.contains("List directory"));
            }
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_help_builtin() {
        let mut test_ctx = TestContext::new_default();
        let cmd = HelpCommand;
        let result = cmd
            .execute(&["cd".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::String(s) => {
                assert!(s.contains("cd"));
                assert!(s.contains("builtin"));
            }
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_help_unknown_command() {
        let mut test_ctx = TestContext::new_default();
        let cmd = HelpCommand;
        let result = cmd.execute(&["nonexistent_xyz".to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
