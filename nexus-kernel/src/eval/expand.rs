//! Word expansion - variable substitution, command substitution, etc.

use crate::parser::Word;
use crate::ShellState;

/// Expand a word to a string.
pub fn expand_word_to_string(word: &Word, state: &ShellState) -> String {
    match word {
        Word::Literal(s) => expand_literal(s, state),
        Word::Variable(name) => expand_variable(name, state),
        Word::CommandSubstitution(cmd) => expand_command_substitution(cmd, state),
    }
}

/// Expand a literal string, handling embedded variables and escapes.
fn expand_literal(s: &str, state: &ShellState) -> String {
    // Handle tilde expansion at the start of the word
    let s = expand_tilde(s, state);

    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Escape sequence
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            '$' => {
                // Variable expansion
                if chars.peek() == Some(&'{') {
                    // ${var} form
                    chars.next(); // consume '{'
                    let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
                    result.push_str(&expand_variable(&var_name, state));
                } else {
                    // $var form
                    let var_name: String = chars
                        .by_ref()
                        .take_while(|&c| c.is_alphanumeric() || c == '_')
                        .collect();
                    if var_name.is_empty() {
                        result.push('$');
                    } else {
                        result.push_str(&expand_variable(&var_name, state));
                    }
                }
            }
            '\'' => {
                // Single quotes - literal, no expansion
                let quoted: String = chars.by_ref().take_while(|&c| c != '\'').collect();
                result.push_str(&quoted);
            }
            '"' => {
                // Double quotes - expand variables but not globs
                let mut quoted = String::new();
                while let Some(c) = chars.next() {
                    if c == '"' {
                        break;
                    } else if c == '\\' {
                        if let Some(next) = chars.next() {
                            quoted.push(next);
                        }
                    } else if c == '$' {
                        if chars.peek() == Some(&'{') {
                            chars.next();
                            let var_name: String =
                                chars.by_ref().take_while(|&c| c != '}').collect();
                            quoted.push_str(&expand_variable(&var_name, state));
                        } else {
                            let var_name: String = chars
                                .by_ref()
                                .take_while(|&c| c.is_alphanumeric() || c == '_')
                                .collect();
                            if var_name.is_empty() {
                                quoted.push('$');
                            } else {
                                quoted.push_str(&expand_variable(&var_name, state));
                            }
                        }
                    } else {
                        quoted.push(c);
                    }
                }
                result.push_str(&quoted);
            }
            _ => result.push(c),
        }
    }

    result
}

/// Expand a variable reference.
fn expand_variable(name: &str, state: &ShellState) -> String {
    // Handle special variables
    match name {
        "?" => state.last_exit_code.to_string(),
        "$" => std::process::id().to_string(),
        "!" => state
            .last_bg_pid
            .map(|p| p.to_string())
            .unwrap_or_default(),
        "0" => "nexus".to_string(),
        "PWD" => state.cwd.to_string_lossy().to_string(),
        "HOME" => state
            .get_env("HOME")
            .unwrap_or_default()
            .to_string(),
        _ => {
            // Check for parameter expansion modifiers
            if let Some((var, modifier)) = parse_parameter_expansion(name) {
                apply_parameter_expansion(&var, &modifier, state)
            } else {
                state.get_var(name).unwrap_or_default().to_string()
            }
        }
    }
}

/// Parse parameter expansion like ${var:-default}.
fn parse_parameter_expansion(name: &str) -> Option<(String, String)> {
    for pattern in &[":-", ":=", ":+", ":?", "-", "=", "+", "?", "#", "##", "%", "%%"] {
        if let Some(pos) = name.find(pattern) {
            let var = name[..pos].to_string();
            let modifier = name[pos..].to_string();
            return Some((var, modifier));
        }
    }
    None
}

/// Apply parameter expansion modifiers.
fn apply_parameter_expansion(var: &str, modifier: &str, state: &ShellState) -> String {
    let value = state.get_var(var);

    if modifier.starts_with(":-") {
        // Use default if unset or null
        let default = &modifier[2..];
        match value {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => default.to_string(),
        }
    } else if modifier.starts_with("-") {
        // Use default if unset
        let default = &modifier[1..];
        value.map(|v| v.to_string()).unwrap_or_else(|| default.to_string())
    } else if modifier.starts_with(":=") {
        // Assign default if unset or null
        // Note: We can't actually assign here without &mut state
        let default = &modifier[2..];
        match value {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => default.to_string(),
        }
    } else if modifier.starts_with(":+") {
        // Use alternative if set and non-null
        let alt = &modifier[2..];
        match value {
            Some(v) if !v.is_empty() => alt.to_string(),
            _ => String::new(),
        }
    } else if modifier.starts_with("+") {
        // Use alternative if set
        let alt = &modifier[1..];
        value.map(|_| alt.to_string()).unwrap_or_default()
    } else if modifier.starts_with("#") {
        // Remove shortest prefix pattern
        let pattern = &modifier[1..];
        value
            .map(|v| remove_prefix(v, pattern, false))
            .unwrap_or_default()
    } else if modifier.starts_with("##") {
        // Remove longest prefix pattern
        let pattern = &modifier[2..];
        value
            .map(|v| remove_prefix(v, pattern, true))
            .unwrap_or_default()
    } else if modifier.starts_with("%%") {
        // Remove longest suffix pattern
        let pattern = &modifier[2..];
        value
            .map(|v| remove_suffix(v, pattern, true))
            .unwrap_or_default()
    } else if modifier.starts_with("%") {
        // Remove shortest suffix pattern
        let pattern = &modifier[1..];
        value
            .map(|v| remove_suffix(v, pattern, false))
            .unwrap_or_default()
    } else {
        value.unwrap_or_default().to_string()
    }
}

/// Remove prefix matching pattern.
fn remove_prefix(s: &str, pattern: &str, longest: bool) -> String {
    // Simplified glob matching - just handle * at the end
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        if s.starts_with(prefix) {
            if longest {
                // Find longest match
                for i in (prefix.len()..=s.len()).rev() {
                    if s[..i].starts_with(prefix) {
                        return s[i..].to_string();
                    }
                }
            }
            return s[prefix.len()..].to_string();
        }
    } else if s.starts_with(pattern) {
        return s[pattern.len()..].to_string();
    }
    s.to_string()
}

/// Remove suffix matching pattern.
fn remove_suffix(s: &str, pattern: &str, longest: bool) -> String {
    // Simplified glob matching - just handle * at the beginning
    if pattern.starts_with('*') {
        let suffix = &pattern[1..];
        if s.ends_with(suffix) {
            if longest {
                // Find longest match
                for i in 0..=(s.len() - suffix.len()) {
                    if s[i..].ends_with(suffix) {
                        return s[..i].to_string();
                    }
                }
            }
            return s[..s.len() - suffix.len()].to_string();
        }
    } else if s.ends_with(pattern) {
        return s[..s.len() - pattern.len()].to_string();
    }
    s.to_string()
}

/// Expand tilde at the start of a word.
///
/// Supports:
/// - `~` or `~/path` → $HOME or $HOME/path
/// - `~+` or `~+/path` → $PWD or $PWD/path
/// - `~-` or `~-/path` → $OLDPWD or $OLDPWD/path
fn expand_tilde(s: &str, state: &ShellState) -> String {
    if !s.starts_with('~') {
        return s.to_string();
    }

    // Check what follows the tilde
    let rest = &s[1..];

    // ~+ (PWD)
    if rest.is_empty() || rest.starts_with('/') {
        // Plain ~ or ~/path
        let home = state.get_env("HOME").unwrap_or_default();
        if rest.is_empty() {
            return home.to_string();
        } else {
            return format!("{}{}", home, rest);
        }
    }

    if rest == "+" || rest.starts_with("+/") {
        // ~+ or ~+/path (PWD)
        let pwd = state.cwd.to_string_lossy();
        if rest == "+" {
            return pwd.to_string();
        } else {
            return format!("{}{}", pwd, &rest[1..]);
        }
    }

    if rest == "-" || rest.starts_with("-/") {
        // ~- or ~-/path (OLDPWD)
        let oldpwd = state.get_env("OLDPWD").unwrap_or_default();
        if rest == "-" {
            return oldpwd.to_string();
        } else {
            return format!("{}{}", oldpwd, &rest[1..]);
        }
    }

    // ~user form - look up user's home directory
    let (username, path_rest) = if let Some(slash_pos) = rest.find('/') {
        (&rest[..slash_pos], &rest[slash_pos..])
    } else {
        (rest, "")
    };

    // Try to get user's home directory
    if let Some(home) = get_user_home(username) {
        format!("{}{}", home, path_rest)
    } else {
        // User not found, return unchanged
        s.to_string()
    }
}

/// Get a user's home directory.
#[cfg(unix)]
fn get_user_home(username: &str) -> Option<String> {
    use nix::libc;
    use std::ffi::CString;

    let c_username = CString::new(username).ok()?;

    unsafe {
        let pwd = libc::getpwnam(c_username.as_ptr());
        if pwd.is_null() {
            return None;
        }
        let home = (*pwd).pw_dir;
        if home.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(home)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(not(unix))]
fn get_user_home(_username: &str) -> Option<String> {
    None
}

/// Expand command substitution $(cmd) or `cmd`.
fn expand_command_substitution(cmd: &str, _state: &ShellState) -> String {
    // Strip $() or ``
    let inner = cmd
        .trim_start_matches("$(")
        .trim_end_matches(')')
        .trim_start_matches('`')
        .trim_end_matches('`');

    // Execute the command and capture output
    // For now, use std::process::Command as a simple implementation
    match std::process::Command::new("sh")
        .arg("-c")
        .arg(inner)
        .output()
    {
        Ok(output) => {
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_string()
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_state() -> ShellState {
        let mut state = ShellState::from_cwd(PathBuf::from("/tmp"));
        state.set_env("HOME".to_string(), "/home/testuser".to_string());
        state.set_env("OLDPWD".to_string(), "/old/path".to_string());
        state
    }

    #[test]
    fn test_tilde_home() {
        let state = make_state();
        assert_eq!(expand_tilde("~", &state), "/home/testuser");
    }

    #[test]
    fn test_tilde_home_path() {
        let state = make_state();
        assert_eq!(expand_tilde("~/projects", &state), "/home/testuser/projects");
        assert_eq!(
            expand_tilde("~/a/b/c", &state),
            "/home/testuser/a/b/c"
        );
    }

    #[test]
    fn test_tilde_pwd() {
        let state = make_state();
        assert_eq!(expand_tilde("~+", &state), "/tmp");
        assert_eq!(expand_tilde("~+/foo", &state), "/tmp/foo");
    }

    #[test]
    fn test_tilde_oldpwd() {
        let state = make_state();
        assert_eq!(expand_tilde("~-", &state), "/old/path");
        assert_eq!(expand_tilde("~-/bar", &state), "/old/path/bar");
    }

    #[test]
    fn test_tilde_no_expansion() {
        let state = make_state();
        // No tilde at start
        assert_eq!(expand_tilde("foo~bar", &state), "foo~bar");
        assert_eq!(expand_tilde("/path/to/~", &state), "/path/to/~");
    }

    #[test]
    fn test_tilde_in_literal() {
        let state = make_state();
        // Full expansion through expand_literal
        assert_eq!(expand_literal("~", &state), "/home/testuser");
        assert_eq!(expand_literal("~/bin", &state), "/home/testuser/bin");
    }

    #[test]
    fn test_expand_variable_basic() {
        let mut state = make_state();
        state.set_var("FOO".to_string(), "bar".to_string());
        assert_eq!(expand_variable("FOO", &state), "bar");
    }

    #[test]
    fn test_expand_variable_default() {
        let state = make_state();
        // ${var:-default}
        assert_eq!(
            expand_variable("UNDEFINED:-fallback", &state),
            "fallback"
        );
    }

    #[test]
    fn test_expand_special_vars() {
        let state = make_state();
        assert_eq!(expand_variable("0", &state), "nexus");
        assert_eq!(expand_variable("PWD", &state), "/tmp");
        assert_eq!(expand_variable("HOME", &state), "/home/testuser");
    }
}
