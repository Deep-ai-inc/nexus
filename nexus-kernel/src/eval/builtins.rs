//! Built-in shell commands that require special shell integration.
//!
//! These are commands that must be builtins because they:
//! - Control flow signals (break, continue, return)
//! - Modify parser/evaluator state (alias expansion)
//! - Modify shell variables (shift, read, getopts)
//! - Replace shell process (exec)
//! - Register signal handlers (trap)

use nexus_api::{ShellEvent, TableColumn, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use tokio::sync::broadcast::Sender;

use crate::commands::CommandRegistry;
use crate::state::TrapAction;
use crate::ShellState;

/// Check if a command name is a builtin.
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "exit"
            | "export"
            | "unset"
            | "set"
            | ":"
            | "test"
            | "["
            | "[["
            | "alias"
            | "unalias"
            | "source"
            | "."
            | "eval"
            | "read"
            | "shift"
            | "return"
            | "break"
            | "continue"
            | "readonly"
            | "command"
            | "getopts"
            | "trap"
            | "exec"
            | "local"
    )
}

/// Try to execute a builtin. Returns None if not a builtin.
pub fn try_builtin(
    name: &str,
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<Option<i32>> {
    match name {
        "cd" => Ok(Some(builtin_cd(args, state, events)?)),
        "exit" => Ok(Some(builtin_exit(args)?)),
        "export" => Ok(Some(builtin_export(args, state, events)?)),
        "unset" => Ok(Some(builtin_unset(args, state, events)?)),
        "set" => Ok(Some(builtin_set(args, state)?)),
        ":" => Ok(Some(0)),
        "test" | "[" => Ok(Some(builtin_test(args)?)),
        "[[" => Ok(Some(builtin_extended_test(args)?)),
        "source" | "." => Ok(Some(builtin_source(args, state, events, commands)?)),
        "eval" => Ok(Some(builtin_eval(args, state, events, commands)?)),
        "readonly" => Ok(Some(builtin_readonly(args, state)?)),
        "command" => Ok(Some(builtin_command(args, state, events, commands)?)),
        "alias" => Ok(Some(builtin_alias(args, state)?)),
        "unalias" => Ok(Some(builtin_unalias(args, state)?)),
        "read" => Ok(Some(builtin_read(args, state)?)),
        "shift" => Ok(Some(builtin_shift(args, state)?)),
        "getopts" => Ok(Some(builtin_getopts(args, state)?)),
        "trap" => Ok(Some(builtin_trap(args, state)?)),
        "exec" => Ok(Some(builtin_exec(args, state)?)),
        // Control flow builtins - these return special exit codes
        // but the evaluator needs to handle break/continue/return specially
        "break" => Ok(Some(builtin_break(args)?)),
        "continue" => Ok(Some(builtin_continue(args)?)),
        "return" => Ok(Some(builtin_return(args)?)),
        // Variable scoping
        "local" => Ok(Some(builtin_local(args, state)?)),
        _ => Ok(None),
    }
}

/// Try to execute a builtin that returns structured output.
/// This handles listing modes (no-arg invocations) that should return
/// a Value instead of printing to stdout.
pub fn try_builtin_value(
    name: &str,
    args: &[String],
    state: &ShellState,
) -> Option<Value> {
    match name {
        "export" if args.is_empty() => {
            let rows: Vec<Vec<Value>> = state
                .env
                .iter()
                .map(|(k, v)| {
                    vec![
                        Value::String(k.clone()),
                        Value::String(v.clone()),
                    ]
                })
                .collect();
            Some(Value::Table {
                columns: vec![
                    TableColumn::new("name"),
                    TableColumn::new("value"),
                ],
                rows,
            })
        }
        "set" if args.is_empty() => {
            let mut rows: Vec<Vec<Value>> = state
                .vars
                .iter()
                .map(|(k, v)| {
                    vec![
                        Value::String(k.clone()),
                        Value::String(v.clone()),
                        Value::String("var".to_string()),
                    ]
                })
                .collect();
            rows.extend(state.env.iter().map(|(k, v)| {
                vec![
                    Value::String(k.clone()),
                    Value::String(v.clone()),
                    Value::String("env".to_string()),
                ]
            }));
            Some(Value::Table {
                columns: vec![
                    TableColumn::new("name"),
                    TableColumn::new("value"),
                    TableColumn::new("scope"),
                ],
                rows,
            })
        }
        "alias" if args.is_empty() => {
            let rows: Vec<Vec<Value>> = state
                .aliases
                .iter()
                .map(|(k, v)| {
                    vec![
                        Value::String(k.clone()),
                        Value::String(v.clone()),
                    ]
                })
                .collect();
            Some(Value::Table {
                columns: vec![
                    TableColumn::new("name"),
                    TableColumn::new("command"),
                ],
                rows,
            })
        }
        _ => None,
    }
}

fn builtin_cd(
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    let target = if args.is_empty() {
        // cd with no args goes to $HOME
        state
            .get_env("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    } else if args[0] == "-" {
        // cd - goes to $OLDPWD
        state
            .get_env("OLDPWD")
            .map(PathBuf::from)
            .unwrap_or_else(|| state.cwd.clone())
    } else {
        let path = PathBuf::from(&args[0]);
        if path.is_absolute() {
            path
        } else {
            state.cwd.join(path)
        }
    };

    let target = target.canonicalize().unwrap_or(target);

    if target.is_dir() {
        let old_cwd = state.cwd.clone();
        state.set_cwd(target.clone())?;
        state.set_env("OLDPWD", old_cwd.to_string_lossy().to_string());
        state.set_env("PWD", target.to_string_lossy().to_string());

        let _ = events.send(ShellEvent::CwdChanged {
            old: old_cwd,
            new: target,
        });

        Ok(0)
    } else {
        eprintln!("cd: {}: No such file or directory", args.get(0).unwrap_or(&String::new()));
        Ok(1)
    }
}

fn builtin_exit(args: &[String]) -> anyhow::Result<i32> {
    let code = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    std::process::exit(code);
}

fn builtin_export(
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    if args.is_empty() {
        // Print all exported variables
        for (key, value) in &state.env {
            println!("export {}={:?}", key, value);
        }
        return Ok(0);
    }

    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            state.set_env(name, value);
            let _ = events.send(ShellEvent::EnvChanged {
                key: name.to_string(),
                value: Some(value.to_string()),
            });
        } else {
            // Export existing variable
            if let Some(value) = state.vars.get(arg) {
                let value = value.clone();
                state.set_env(arg, &value);
                let _ = events.send(ShellEvent::EnvChanged {
                    key: arg.to_string(),
                    value: Some(value),
                });
            }
        }
    }
    Ok(0)
}

fn builtin_unset(
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    for arg in args {
        state.unset_env(arg);
        state.vars.remove(arg);
        let _ = events.send(ShellEvent::EnvChanged {
            key: arg.to_string(),
            value: None,
        });
    }
    Ok(0)
}

fn builtin_set(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.is_empty() {
        // Print all variables
        for (key, value) in &state.vars {
            println!("{}={}", key, value);
        }
        for (key, value) in &state.env {
            println!("{}={}", key, value);
        }
        return Ok(0);
    }

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        if arg.starts_with('-') || arg.starts_with('+') {
            let enable = arg.starts_with('-');
            let flags = &arg[1..];

            for flag in flags.chars() {
                match flag {
                    'o' => {
                        // set -o option_name
                        i += 1;
                        if i < args.len() {
                            let opt_name = &args[i];
                            // Map long option names to short flags
                            let short_flag = match opt_name.as_str() {
                                "errexit" => Some('e'),
                                "nounset" => Some('u'),
                                "xtrace" => Some('x'),
                                "verbose" => Some('v'),
                                "noexec" => Some('n'),
                                "noglob" => Some('f'),
                                "noclobber" => Some('C'),
                                "allexport" => Some('a'),
                                "notify" => Some('b'),
                                "hashall" => Some('h'),
                                _ => None,
                            };
                            if let Some(f) = short_flag {
                                state.options.set_option(f, enable);
                            } else {
                                eprintln!("set: {}: invalid option name", opt_name);
                                return Ok(1);
                            }
                        } else if !enable {
                            // set +o with no arg prints options
                            println!("{}", state.options.print_options());
                        }
                    }
                    '-' => {
                        // set -- ends option processing
                        i += 1;
                        // Remaining args become positional parameters
                        state.positional_params = args[i..].to_vec();
                        return Ok(0);
                    }
                    _ => {
                        if !state.options.set_option(flag, enable) {
                            eprintln!("set: -{}: invalid option", flag);
                            return Ok(1);
                        }
                    }
                }
            }
        } else {
            // Positional parameters
            state.positional_params = args[i..].to_vec();
            return Ok(0);
        }

        i += 1;
    }

    Ok(0)
}

fn builtin_test(args: &[String]) -> anyhow::Result<i32> {
    // Remove trailing ] if present
    let args: Vec<&str> = args
        .iter()
        .map(|s| s.as_str())
        .filter(|&s| s != "]")
        .collect();

    if args.is_empty() {
        return Ok(1);
    }

    // Single argument: true if non-empty
    if args.len() == 1 {
        return Ok(if args[0].is_empty() { 1 } else { 0 });
    }

    // Two arguments: unary operators
    if args.len() == 2 {
        let op = args[0];
        let val = args[1];

        return Ok(match op {
            "-n" => if !val.is_empty() { 0 } else { 1 },
            "-z" => if val.is_empty() { 0 } else { 1 },
            "-e" | "-a" => if PathBuf::from(val).exists() { 0 } else { 1 },
            "-f" => if PathBuf::from(val).is_file() { 0 } else { 1 },
            "-d" => if PathBuf::from(val).is_dir() { 0 } else { 1 },
            "-r" => if PathBuf::from(val).exists() { 0 } else { 1 }, // Simplified
            "-w" => if PathBuf::from(val).exists() { 0 } else { 1 }, // Simplified
            "-x" => if PathBuf::from(val).exists() { 0 } else { 1 }, // Simplified
            "-L" | "-h" => if PathBuf::from(val).is_symlink() { 0 } else { 1 },
            "-s" => {
                if let Ok(meta) = std::fs::metadata(val) {
                    if meta.len() > 0 { 0 } else { 1 }
                } else {
                    1
                }
            }
            "!" => builtin_test(args[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())
                .map(|code| if code == 0 { 1 } else { 0 })?,
            _ => 1,
        });
    }

    // Three arguments: binary operators
    if args.len() == 3 {
        let left = args[0];
        let op = args[1];
        let right = args[2];

        return Ok(match op {
            "=" | "==" => if left == right { 0 } else { 1 },
            "!=" => if left != right { 0 } else { 1 },
            "-eq" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l == r { 0 } else { 1 }
            }
            "-ne" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l != r { 0 } else { 1 }
            }
            "-lt" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l < r { 0 } else { 1 }
            }
            "-le" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l <= r { 0 } else { 1 }
            }
            "-gt" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l > r { 0 } else { 1 }
            }
            "-ge" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l >= r { 0 } else { 1 }
            }
            "-nt" => {
                // Newer than
                let left_time = std::fs::metadata(left).and_then(|m| m.modified()).ok();
                let right_time = std::fs::metadata(right).and_then(|m| m.modified()).ok();
                match (left_time, right_time) {
                    (Some(l), Some(r)) => if l > r { 0 } else { 1 },
                    _ => 1,
                }
            }
            "-ot" => {
                // Older than
                let left_time = std::fs::metadata(left).and_then(|m| m.modified()).ok();
                let right_time = std::fs::metadata(right).and_then(|m| m.modified()).ok();
                match (left_time, right_time) {
                    (Some(l), Some(r)) => if l < r { 0 } else { 1 },
                    _ => 1,
                }
            }
            _ => 1,
        });
    }

    // Handle -a (AND) and -o (OR) for longer expressions
    if args.len() > 3 {
        // Look for -a or -o
        for (i, &arg) in args.iter().enumerate() {
            if arg == "-a" {
                let left_result = builtin_test(args[..i].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())?;
                let right_result = builtin_test(args[i+1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())?;
                return Ok(if left_result == 0 && right_result == 0 { 0 } else { 1 });
            }
            if arg == "-o" {
                let left_result = builtin_test(args[..i].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())?;
                let right_result = builtin_test(args[i+1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())?;
                return Ok(if left_result == 0 || right_result == 0 { 0 } else { 1 });
            }
        }
    }

    Ok(1)
}

/// Extended test builtin [[.
/// Supports pattern matching, regex, and logical operators inside brackets.
fn builtin_extended_test(args: &[String]) -> anyhow::Result<i32> {
    // Remove trailing ]] if present
    let args: Vec<&str> = args
        .iter()
        .map(|s| s.as_str())
        .filter(|&s| s != "]]")
        .collect();

    if args.is_empty() {
        return Ok(1);
    }

    // Handle logical operators && and ||
    // Find the first && or || not inside parentheses
    let mut paren_depth = 0;
    for (i, &arg) in args.iter().enumerate() {
        if arg == "(" {
            paren_depth += 1;
        } else if arg == ")" {
            paren_depth -= 1;
        } else if paren_depth == 0 {
            if arg == "&&" {
                let left_result = builtin_extended_test(
                    args[..i].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()
                )?;
                if left_result != 0 {
                    return Ok(1); // Short-circuit
                }
                return builtin_extended_test(
                    args[i+1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()
                );
            }
            if arg == "||" {
                let left_result = builtin_extended_test(
                    args[..i].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()
                )?;
                if left_result == 0 {
                    return Ok(0); // Short-circuit
                }
                return builtin_extended_test(
                    args[i+1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()
                );
            }
        }
    }

    // Handle parentheses for grouping
    if args.first() == Some(&"(") && args.last() == Some(&")") {
        return builtin_extended_test(
            args[1..args.len()-1].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()
        );
    }

    // Single argument: true if non-empty
    if args.len() == 1 {
        return Ok(if args[0].is_empty() { 1 } else { 0 });
    }

    // Two arguments: unary operators or negation
    if args.len() == 2 {
        let op = args[0];
        let val = args[1];

        return Ok(match op {
            "-n" => if !val.is_empty() { 0 } else { 1 },
            "-z" => if val.is_empty() { 0 } else { 1 },
            "-e" | "-a" => if PathBuf::from(val).exists() { 0 } else { 1 },
            "-f" => if PathBuf::from(val).is_file() { 0 } else { 1 },
            "-d" => if PathBuf::from(val).is_dir() { 0 } else { 1 },
            "-r" => if PathBuf::from(val).exists() { 0 } else { 1 },
            "-w" => if PathBuf::from(val).exists() { 0 } else { 1 },
            "-x" => if PathBuf::from(val).exists() { 0 } else { 1 },
            "-L" | "-h" => if PathBuf::from(val).is_symlink() { 0 } else { 1 },
            "-s" => {
                if let Ok(meta) = std::fs::metadata(val) {
                    if meta.len() > 0 { 0 } else { 1 }
                } else {
                    1
                }
            }
            "-v" => {
                // -v VAR: true if variable is set
                // Note: we don't have access to state here, simplified to always false
                1
            }
            "!" => builtin_extended_test(args[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())
                .map(|code| if code == 0 { 1 } else { 0 })?,
            _ => 1,
        });
    }

    // Three arguments: binary operators
    if args.len() == 3 {
        let left = args[0];
        let op = args[1];
        let right = args[2];

        return Ok(match op {
            // String comparison
            "=" | "==" => {
                // Pattern matching in [[: right side can be a glob pattern
                if extended_pattern_match(left, right) { 0 } else { 1 }
            }
            "!=" => {
                if !extended_pattern_match(left, right) { 0 } else { 1 }
            }
            // Regex matching
            "=~" => {
                if let Ok(re) = regex::Regex::new(right) {
                    if re.is_match(left) { 0 } else { 1 }
                } else {
                    1 // Invalid regex
                }
            }
            // String ordering
            "<" => if left < right { 0 } else { 1 },
            ">" => if left > right { 0 } else { 1 },
            // Numeric comparison
            "-eq" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l == r { 0 } else { 1 }
            }
            "-ne" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l != r { 0 } else { 1 }
            }
            "-lt" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l < r { 0 } else { 1 }
            }
            "-le" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l <= r { 0 } else { 1 }
            }
            "-gt" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l > r { 0 } else { 1 }
            }
            "-ge" => {
                let l: i64 = left.parse().unwrap_or(0);
                let r: i64 = right.parse().unwrap_or(0);
                if l >= r { 0 } else { 1 }
            }
            "-nt" => {
                let left_time = std::fs::metadata(left).and_then(|m| m.modified()).ok();
                let right_time = std::fs::metadata(right).and_then(|m| m.modified()).ok();
                match (left_time, right_time) {
                    (Some(l), Some(r)) => if l > r { 0 } else { 1 },
                    _ => 1,
                }
            }
            "-ot" => {
                let left_time = std::fs::metadata(left).and_then(|m| m.modified()).ok();
                let right_time = std::fs::metadata(right).and_then(|m| m.modified()).ok();
                match (left_time, right_time) {
                    (Some(l), Some(r)) => if l < r { 0 } else { 1 },
                    _ => 1,
                }
            }
            _ => 1,
        });
    }

    // Handle negation with more than 2 args
    if !args.is_empty() && args[0] == "!" {
        return builtin_extended_test(
            args[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()
        ).map(|code| if code == 0 { 1 } else { 0 });
    }

    Ok(1)
}

/// Pattern matching for [[ == ]] operator.
/// Supports shell glob patterns: *, ?, [...]
fn extended_pattern_match(s: &str, pattern: &str) -> bool {
    // Check for glob chars
    if !pattern.chars().any(|c| c == '*' || c == '?' || c == '[') {
        return s == pattern;
    }

    // Simple glob matching
    glob_match_str(s, pattern)
}

/// Simple glob matching for pattern strings.
fn glob_match_str(s: &str, pattern: &str) -> bool {
    let s_chars: Vec<char> = s.chars().collect();
    let p_chars: Vec<char> = pattern.chars().collect();
    glob_match_impl(&s_chars, &p_chars, 0, 0)
}

fn glob_match_impl(s: &[char], p: &[char], mut si: usize, mut pi: usize) -> bool {
    while pi < p.len() || si < s.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    // Skip consecutive stars
                    while pi < p.len() && p[pi] == '*' {
                        pi += 1;
                    }
                    // * at end matches everything
                    if pi == p.len() {
                        return true;
                    }
                    // Try matching * against 0 or more chars
                    while si <= s.len() {
                        if glob_match_impl(s, p, si, pi) {
                            return true;
                        }
                        si += 1;
                    }
                    return false;
                }
                '?' => {
                    if si >= s.len() {
                        return false;
                    }
                    si += 1;
                    pi += 1;
                }
                '[' => {
                    if si >= s.len() {
                        return false;
                    }
                    let (matched, consumed) = match_char_class(&p[pi..], s[si]);
                    if !matched {
                        return false;
                    }
                    si += 1;
                    pi += consumed;
                }
                c => {
                    if si >= s.len() || s[si] != c {
                        return false;
                    }
                    si += 1;
                    pi += 1;
                }
            }
        } else {
            return false;
        }
    }
    true
}

/// Match a character class like [abc] or [a-z] or [!abc].
fn match_char_class(pattern: &[char], c: char) -> (bool, usize) {
    if pattern.is_empty() || pattern[0] != '[' {
        return (false, 0);
    }

    let mut i = 1;
    let negated = if i < pattern.len() && (pattern[i] == '!' || pattern[i] == '^') {
        i += 1;
        true
    } else {
        false
    };

    let mut matched = false;

    while i < pattern.len() && pattern[i] != ']' {
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            let start = pattern[i];
            let end = pattern[i + 2];
            if c >= start && c <= end {
                matched = true;
            }
            i += 3;
        } else {
            if pattern[i] == c {
                matched = true;
            }
            i += 1;
        }
    }

    if i < pattern.len() && pattern[i] == ']' {
        i += 1;
    }

    let result = if negated { !matched } else { matched };
    (result, i)
}

fn builtin_source(
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    if args.is_empty() {
        eprintln!("source: filename argument required");
        return Ok(2);
    }

    let filename = &args[0];
    let path = if PathBuf::from(filename).is_absolute() {
        PathBuf::from(filename)
    } else {
        state.cwd.join(filename)
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("source: {}: {}", filename, e);
            return Ok(1);
        }
    };

    // Parse and execute each line
    let mut parser = crate::Parser::new()?;
    let mut last_exit = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match parser.parse(line) {
            Ok(ast) => {
                last_exit = crate::eval::execute(state, &ast, events, commands)?;
            }
            Err(e) => {
                eprintln!("source: {}: parse error: {}", filename, e);
                return Ok(1);
            }
        }
    }

    Ok(last_exit)
}

fn builtin_eval(
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    if args.is_empty() {
        return Ok(0);
    }

    let command = args.join(" ");
    let mut parser = crate::Parser::new()?;

    match parser.parse(&command) {
        Ok(ast) => crate::eval::execute(state, &ast, events, commands),
        Err(e) => {
            eprintln!("eval: parse error: {}", e);
            Ok(1)
        }
    }
}

fn builtin_readonly(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.is_empty() {
        // Print all readonly variables
        for name in &state.readonly_vars {
            if let Some(value) = state.vars.get(name) {
                println!("readonly {}={}", name, value);
            } else if let Some(value) = state.get_env(name) {
                println!("readonly {}={}", name, value);
            } else {
                println!("readonly {}", name);
            }
        }
        return Ok(0);
    }

    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            state.set_var(name.to_string(), value.to_string());
            state.readonly_vars.insert(name.to_string());
        } else {
            state.readonly_vars.insert(arg.to_string());
        }
    }

    Ok(0)
}

fn builtin_command(
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    // command runs a command bypassing aliases and functions
    if args.is_empty() {
        return Ok(0);
    }

    let name = &args[0];
    let cmd_args = &args[1..];

    // Handle flags
    let (name, cmd_args, use_default_path) = if name == "-p" {
        if args.len() < 2 {
            return Ok(0);
        }
        (&args[1], &args[2..], true)
    } else if name == "-v" || name == "-V" {
        // command -v prints path/type info
        let mut exit_code = 0;
        for arg in &args[1..] {
            if is_builtin(arg) {
                println!("{}", arg);
            } else if commands.contains(arg) {
                println!("{}", arg);
            } else if state.aliases.contains_key(arg) {
                if name == "-V" {
                    println!("{} is aliased to `{}'", arg, state.aliases.get(arg).unwrap());
                } else {
                    println!("alias {}='{}'", arg, state.aliases.get(arg).unwrap());
                }
            } else if let Some(path) = find_in_path(arg, state) {
                println!("{}", path.display());
            } else {
                exit_code = 1;
            }
        }
        return Ok(exit_code);
    } else {
        (name, cmd_args, false)
    };

    // If it's a builtin, run it
    if is_builtin(name) {
        return try_builtin(name, &cmd_args.to_vec(), state, events, commands)?
            .ok_or_else(|| anyhow::anyhow!("builtin not found"));
    }

    // Find in PATH and execute
    let path = if use_default_path {
        find_in_default_path(name)
    } else {
        find_in_path(name, state)
    };

    if let Some(_path) = path {
        // External command - would be handled by process module
        // For now, return that we couldn't execute it internally
        Ok(127)
    } else {
        eprintln!("command: {}: not found", name);
        Ok(127)
    }
}

/// Find a command in PATH.
fn find_in_path(cmd: &str, state: &ShellState) -> Option<PathBuf> {
    let path_var = state.get_env("PATH")?;

    for dir in path_var.split(':') {
        let candidate = PathBuf::from(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

/// Find a command in the default PATH (for command -p).
fn find_in_default_path(cmd: &str) -> Option<PathBuf> {
    let default_path = "/usr/bin:/bin:/usr/sbin:/sbin";

    for dir in default_path.split(':') {
        let candidate = PathBuf::from(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

// ============================================================================
// alias / unalias - Manage shell aliases
// ============================================================================

fn builtin_alias(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.is_empty() {
        // Print all aliases
        for (name, value) in &state.aliases {
            println!("alias {}='{}'", name, value);
        }
        return Ok(0);
    }

    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            // Set alias
            state.aliases.insert(name.to_string(), value.to_string());
        } else {
            // Print specific alias
            if let Some(value) = state.aliases.get(arg) {
                println!("alias {}='{}'", arg, value);
            } else {
                eprintln!("alias: {}: not found", arg);
                return Ok(1);
            }
        }
    }
    Ok(0)
}

fn builtin_unalias(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.is_empty() {
        eprintln!("unalias: usage: unalias [-a] name [name ...]");
        return Ok(2);
    }

    if args.len() == 1 && args[0] == "-a" {
        // Remove all aliases
        state.aliases.clear();
        return Ok(0);
    }

    let mut exit_code = 0;
    for arg in args {
        if arg == "-a" {
            continue;
        }
        if state.aliases.remove(arg).is_none() {
            eprintln!("unalias: {}: not found", arg);
            exit_code = 1;
        }
    }
    Ok(exit_code)
}

// ============================================================================
// read - Read input into shell variables
// ============================================================================

fn builtin_read(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    let mut raw_mode = false;
    let mut prompt = String::new();
    let mut var_names: Vec<&str> = Vec::new();

    // Parse options
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-r" => raw_mode = true,
            "-p" => {
                i += 1;
                if i < args.len() {
                    prompt = args[i].clone();
                }
            }
            arg if arg.starts_with('-') => {
                // Ignore other options for simplicity
            }
            _ => {
                var_names = args[i..].iter().map(|s| s.as_str()).collect();
                break;
            }
        }
        i += 1;
    }

    // Default variable name
    if var_names.is_empty() {
        var_names.push("REPLY");
    }

    // Print prompt if specified
    if !prompt.is_empty() {
        eprint!("{}", prompt);
        let _ = std::io::stderr().flush();
    }

    // Read a line from stdin
    let stdin = std::io::stdin();
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(0) => return Ok(1), // EOF
        Ok(_) => {}
        Err(_) => return Ok(1),
    }

    // Remove trailing newline
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }

    // Process backslashes unless -r
    let line = if raw_mode {
        line
    } else {
        line.replace("\\", "")
    };

    // Split and assign to variables
    let words: Vec<&str> = line.split_whitespace().collect();

    for (idx, var_name) in var_names.iter().enumerate() {
        if idx < words.len() {
            if idx == var_names.len() - 1 {
                // Last variable gets remaining words
                let remaining: Vec<&str> = words[idx..].to_vec();
                state.set_var(var_name.to_string(), remaining.join(" "));
            } else {
                state.set_var(var_name.to_string(), words[idx].to_string());
            }
        } else {
            // Variable gets empty string if no more words
            state.set_var(var_name.to_string(), String::new());
        }
    }

    Ok(0)
}

// ============================================================================
// shift - Shift positional parameters
// ============================================================================

fn builtin_shift(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    let n: usize = args
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    if n > state.positional_params.len() {
        eprintln!("shift: shift count out of range");
        return Ok(1);
    }

    state.positional_params = state.positional_params[n..].to_vec();
    Ok(0)
}

// ============================================================================
// getopts - Parse positional parameters (simplified)
// ============================================================================

fn builtin_getopts(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.len() < 2 {
        eprintln!("getopts: usage: getopts optstring name [args]");
        return Ok(2);
    }

    let optstring = &args[0];
    let var_name = &args[1];

    // Get current OPTIND (1-based index into positional params)
    let optind: usize = state
        .get_var("OPTIND")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    // Get the arguments to parse (either from args or positional_params)
    let parse_args = if args.len() > 2 {
        &args[2..]
    } else {
        &state.positional_params[..]
    };

    if optind > parse_args.len() {
        state.set_var(var_name.to_string(), "?".to_string());
        return Ok(1); // No more options
    }

    let arg = &parse_args[optind - 1];

    // Check if it's an option
    if !arg.starts_with('-') || arg == "-" || arg == "--" {
        state.set_var(var_name.to_string(), "?".to_string());
        return Ok(1);
    }

    // Get the option character (simplified: only handle single char options)
    let opt_char = arg.chars().nth(1).unwrap_or('?');
    let opt_str = opt_char.to_string();

    // Check if option is valid
    if optstring.contains(opt_char) {
        // Check if option requires an argument
        let char_pos = optstring.find(opt_char).unwrap();
        let needs_arg = optstring.chars().nth(char_pos + 1) == Some(':');

        if needs_arg {
            // Check for argument in same string (-oARG) or next arg
            if arg.len() > 2 {
                state.set_var("OPTARG".to_string(), arg[2..].to_string());
                state.set_var("OPTIND".to_string(), (optind + 1).to_string());
            } else if optind < parse_args.len() {
                state.set_var("OPTARG".to_string(), parse_args[optind].clone());
                state.set_var("OPTIND".to_string(), (optind + 2).to_string());
            } else {
                // Missing argument
                if optstring.starts_with(':') {
                    state.set_var(var_name.to_string(), ":".to_string());
                    state.set_var("OPTARG".to_string(), opt_str.clone());
                } else {
                    eprintln!("getopts: option requires an argument -- {}", opt_char);
                    state.set_var(var_name.to_string(), "?".to_string());
                }
                state.set_var("OPTIND".to_string(), (optind + 1).to_string());
                return Ok(0);
            }
        } else {
            state.set_var("OPTIND".to_string(), (optind + 1).to_string());
        }
        state.set_var(var_name.to_string(), opt_str);
        Ok(0)
    } else {
        // Invalid option
        if optstring.starts_with(':') {
            state.set_var(var_name.to_string(), "?".to_string());
            state.set_var("OPTARG".to_string(), opt_str);
        } else {
            eprintln!("getopts: illegal option -- {}", opt_char);
            state.set_var(var_name.to_string(), "?".to_string());
        }
        state.set_var("OPTIND".to_string(), (optind + 1).to_string());
        Ok(0)
    }
}

// ============================================================================
// trap - Register signal handlers (simplified)
// ============================================================================

fn builtin_trap(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.is_empty() {
        // Print current traps
        for (signal, action) in &state.traps {
            let sig_name = signal_name(*signal);
            match action {
                TrapAction::Default => {}
                TrapAction::Ignore => println!("trap -- '' {}", sig_name),
                TrapAction::Command(cmd) => println!("trap -- '{}' {}", cmd, sig_name),
            }
        }
        return Ok(0);
    }

    // trap -l: list signal names
    if args.len() == 1 && args[0] == "-l" {
        println!(" 1) SIGHUP   2) SIGINT   3) SIGQUIT  4) SIGILL");
        println!(" 5) SIGTRAP  6) SIGABRT  9) SIGKILL 10) SIGUSR1");
        println!("11) SIGSEGV 12) SIGUSR2 13) SIGPIPE 14) SIGALRM");
        println!("15) SIGTERM 17) SIGCHLD 18) SIGCONT 19) SIGSTOP");
        return Ok(0);
    }

    // Parse: trap [-p] [[action] signal ...]
    let mut list_mode = false;
    let mut arg_start = 0;

    if args[0] == "-p" {
        list_mode = true;
        arg_start = 1;
    }

    if list_mode {
        // Print traps for specified signals
        for sig_arg in &args[arg_start..] {
            if let Some(sig_num) = parse_signal(sig_arg) {
                if let Some(action) = state.traps.get(&sig_num) {
                    let sig_name = signal_name(sig_num);
                    match action {
                        TrapAction::Ignore => println!("trap -- '' {}", sig_name),
                        TrapAction::Command(cmd) => println!("trap -- '{}' {}", cmd, sig_name),
                        TrapAction::Default => {}
                    }
                }
            }
        }
        return Ok(0);
    }

    // trap action signal [signal ...]
    if args.len() < 2 {
        eprintln!("trap: usage: trap [action] signal [signal ...]");
        return Ok(2);
    }

    let action_str = &args[0];
    let action = if action_str.is_empty() || action_str == "-" {
        TrapAction::Default
    } else if action_str == "''" || action_str == "\"\"" {
        TrapAction::Ignore
    } else {
        TrapAction::Command(action_str.clone())
    };

    for sig_arg in &args[1..] {
        if let Some(sig_num) = parse_signal(sig_arg) {
            state.traps.insert(sig_num, action.clone());
        } else {
            eprintln!("trap: {}: invalid signal specification", sig_arg);
            return Ok(1);
        }
    }

    Ok(0)
}

fn parse_signal(s: &str) -> Option<i32> {
    // Try as number
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }

    // Try as signal name
    let s = s.to_uppercase();
    let s = s.strip_prefix("SIG").unwrap_or(&s);

    match s.as_ref() {
        "EXIT" | "0" => Some(0),
        "HUP" => Some(1),
        "INT" => Some(2),
        "QUIT" => Some(3),
        "ILL" => Some(4),
        "TRAP" => Some(5),
        "ABRT" => Some(6),
        "KILL" => Some(9),
        "USR1" => Some(10),
        "SEGV" => Some(11),
        "USR2" => Some(12),
        "PIPE" => Some(13),
        "ALRM" => Some(14),
        "TERM" => Some(15),
        "CHLD" => Some(17),
        "CONT" => Some(18),
        "STOP" => Some(19),
        "ERR" => Some(-1),   // Special: on error
        "DEBUG" => Some(-2), // Special: before each command
        _ => None,
    }
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        0 => "EXIT",
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        17 => "SIGCHLD",
        18 => "SIGCONT",
        19 => "SIGSTOP",
        -1 => "ERR",
        -2 => "DEBUG",
        _ => "UNKNOWN",
    }
}

// ============================================================================
// exec - Replace shell with command
// ============================================================================

fn builtin_exec(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if args.is_empty() {
        // exec with no args - just modify file descriptors (not implemented)
        return Ok(0);
    }

    // Find the command
    let cmd_name = &args[0];
    let cmd_path = find_in_path(cmd_name, state)
        .or_else(|| {
            let p = PathBuf::from(cmd_name);
            if p.exists() { Some(p) } else { None }
        });

    let Some(cmd_path) = cmd_path else {
        eprintln!("exec: {}: not found", cmd_name);
        return Ok(127);
    };

    // Use nix to exec - this replaces the current process
    use std::ffi::CString;
    use nix::unistd::execv;

    // Convert args to CStrings
    let argv: Vec<CString> = args
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap())
        .collect();

    let path_cstr = CString::new(cmd_path.to_string_lossy().as_ref()).unwrap();

    // Set up environment
    // Safety: We're about to exec, so modifying the environment is safe
    for (key, value) in &state.env {
        unsafe { std::env::set_var(key, value); }
    }

    // This replaces the process - never returns on success
    execv(&path_cstr, &argv)?;
    unreachable!()
}

// ============================================================================
// Control flow builtins
// These return special exit codes that the evaluator should handle.
// For now, they just return fixed codes that can be detected.
// ============================================================================

// Special exit codes for control flow (high values to avoid conflicts)
pub const BREAK_EXIT_CODE: i32 = 256;
pub const CONTINUE_EXIT_CODE: i32 = 356; // Allow for break levels
pub const RETURN_EXIT_CODE: i32 = 456;   // Base for return codes

fn builtin_break(args: &[String]) -> anyhow::Result<i32> {
    let n: u32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
    if n == 0 {
        eprintln!("break: loop count out of range");
        return Ok(1);
    }
    // Encode the break level in the exit code
    Ok(BREAK_EXIT_CODE + n as i32 - 1)
}

fn builtin_continue(args: &[String]) -> anyhow::Result<i32> {
    let n: u32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
    if n == 0 {
        eprintln!("continue: loop count out of range");
        return Ok(1);
    }
    // Encode the continue level in the exit code
    Ok(CONTINUE_EXIT_CODE + n as i32 - 1)
}

fn builtin_return(args: &[String]) -> anyhow::Result<i32> {
    let code: i32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    // Signal a return with the specified code
    // The evaluator needs to handle this specially for functions/sourced scripts
    Ok(RETURN_EXIT_CODE + code)
}

fn builtin_local(args: &[String], state: &mut ShellState) -> anyhow::Result<i32> {
    if !state.in_function() {
        eprintln!("local: can only be used in a function");
        return Ok(1);
    }

    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            state.declare_local(name.to_string(), value.to_string());
        } else {
            // Declare local with empty value
            state.declare_local(arg.to_string(), String::new());
        }
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // is_builtin tests
    // =========================================================================

    #[test]
    fn test_is_builtin_recognized() {
        assert!(is_builtin("cd"));
        assert!(is_builtin("exit"));
        assert!(is_builtin("export"));
        assert!(is_builtin("unset"));
        assert!(is_builtin("set"));
        assert!(is_builtin(":"));
        assert!(is_builtin("test"));
        assert!(is_builtin("["));
        assert!(is_builtin("[["));
        assert!(is_builtin("alias"));
        assert!(is_builtin("unalias"));
        assert!(is_builtin("source"));
        assert!(is_builtin("."));
        assert!(is_builtin("eval"));
        assert!(is_builtin("read"));
        assert!(is_builtin("shift"));
        assert!(is_builtin("return"));
        assert!(is_builtin("break"));
        assert!(is_builtin("continue"));
        assert!(is_builtin("readonly"));
        assert!(is_builtin("command"));
        assert!(is_builtin("getopts"));
        assert!(is_builtin("trap"));
        assert!(is_builtin("exec"));
        assert!(is_builtin("local"));
    }

    #[test]
    fn test_is_builtin_not_recognized() {
        assert!(!is_builtin("ls"));
        assert!(!is_builtin("grep"));
        assert!(!is_builtin("cat"));
        assert!(!is_builtin("echo")); // echo is a native command, not a builtin
        assert!(!is_builtin("pwd"));
        assert!(!is_builtin(""));
        assert!(!is_builtin("CD")); // case sensitive
        assert!(!is_builtin("Exit"));
    }

    // =========================================================================
    // parse_signal tests
    // =========================================================================

    #[test]
    fn test_parse_signal_numeric() {
        assert_eq!(parse_signal("1"), Some(1));
        assert_eq!(parse_signal("2"), Some(2));
        assert_eq!(parse_signal("9"), Some(9));
        assert_eq!(parse_signal("15"), Some(15));
        assert_eq!(parse_signal("0"), Some(0));
    }

    #[test]
    fn test_parse_signal_named() {
        assert_eq!(parse_signal("HUP"), Some(1));
        assert_eq!(parse_signal("INT"), Some(2));
        assert_eq!(parse_signal("QUIT"), Some(3));
        assert_eq!(parse_signal("KILL"), Some(9));
        assert_eq!(parse_signal("TERM"), Some(15));
        assert_eq!(parse_signal("USR1"), Some(10));
        assert_eq!(parse_signal("USR2"), Some(12));
    }

    #[test]
    fn test_parse_signal_with_sig_prefix() {
        assert_eq!(parse_signal("SIGHUP"), Some(1));
        assert_eq!(parse_signal("SIGINT"), Some(2));
        assert_eq!(parse_signal("SIGKILL"), Some(9));
        assert_eq!(parse_signal("SIGTERM"), Some(15));
    }

    #[test]
    fn test_parse_signal_case_insensitive() {
        assert_eq!(parse_signal("hup"), Some(1));
        assert_eq!(parse_signal("Hup"), Some(1));
        assert_eq!(parse_signal("sigint"), Some(2));
        assert_eq!(parse_signal("SigInt"), Some(2));
    }

    #[test]
    fn test_parse_signal_special() {
        assert_eq!(parse_signal("EXIT"), Some(0));
        assert_eq!(parse_signal("ERR"), Some(-1));
        assert_eq!(parse_signal("DEBUG"), Some(-2));
    }

    #[test]
    fn test_parse_signal_invalid() {
        assert_eq!(parse_signal("INVALID"), None);
        assert_eq!(parse_signal("SIGFOO"), None);
        assert_eq!(parse_signal(""), None);
        assert_eq!(parse_signal("abc"), None);
    }

    // =========================================================================
    // signal_name tests
    // =========================================================================

    #[test]
    fn test_signal_name_common() {
        assert_eq!(signal_name(0), "EXIT");
        assert_eq!(signal_name(1), "SIGHUP");
        assert_eq!(signal_name(2), "SIGINT");
        assert_eq!(signal_name(9), "SIGKILL");
        assert_eq!(signal_name(15), "SIGTERM");
    }

    #[test]
    fn test_signal_name_special() {
        assert_eq!(signal_name(-1), "ERR");
        assert_eq!(signal_name(-2), "DEBUG");
    }

    #[test]
    fn test_signal_name_unknown() {
        assert_eq!(signal_name(99), "UNKNOWN");
        assert_eq!(signal_name(-99), "UNKNOWN");
        assert_eq!(signal_name(1000), "UNKNOWN");
    }

    // =========================================================================
    // glob_match_str tests
    // =========================================================================

    #[test]
    fn test_glob_exact_match() {
        assert!(glob_match_str("hello", "hello"));
        assert!(!glob_match_str("hello", "world"));
        assert!(!glob_match_str("hello", "hell"));
        assert!(!glob_match_str("hell", "hello"));
    }

    #[test]
    fn test_glob_star_matches_everything() {
        assert!(glob_match_str("anything", "*"));
        assert!(glob_match_str("", "*"));
        assert!(glob_match_str("hello world", "*"));
    }

    #[test]
    fn test_glob_star_prefix() {
        assert!(glob_match_str("test.txt", "*.txt"));
        assert!(glob_match_str("file.txt", "*.txt"));
        assert!(glob_match_str(".txt", "*.txt"));
        assert!(!glob_match_str("test.rs", "*.txt"));
        assert!(!glob_match_str("testtxt", "*.txt"));
    }

    #[test]
    fn test_glob_star_suffix() {
        assert!(glob_match_str("test.txt", "test*"));
        assert!(glob_match_str("test", "test*"));
        assert!(glob_match_str("test123", "test*"));
        assert!(!glob_match_str("tes", "test*"));
        assert!(!glob_match_str("atest", "test*"));
    }

    #[test]
    fn test_glob_star_middle() {
        assert!(glob_match_str("test.txt", "test*txt"));
        assert!(glob_match_str("test123txt", "test*txt"));
        assert!(glob_match_str("testtxt", "test*txt"));
        assert!(!glob_match_str("test.rs", "test*txt"));
    }

    #[test]
    fn test_glob_multiple_stars() {
        assert!(glob_match_str("a/b/c", "*/*"));
        assert!(glob_match_str("abc/def/ghi", "*/*/*"));
        assert!(glob_match_str("test.min.js", "*.*.js"));
    }

    #[test]
    fn test_glob_question_mark() {
        assert!(glob_match_str("a", "?"));
        assert!(glob_match_str("ab", "??"));
        assert!(glob_match_str("abc", "???"));
        assert!(!glob_match_str("", "?"));
        assert!(!glob_match_str("ab", "?"));
        assert!(!glob_match_str("a", "??"));
    }

    #[test]
    fn test_glob_question_mark_mixed() {
        assert!(glob_match_str("test1.txt", "test?.txt"));
        assert!(glob_match_str("testA.txt", "test?.txt"));
        assert!(!glob_match_str("test.txt", "test?.txt"));
        assert!(!glob_match_str("test12.txt", "test?.txt"));
    }

    #[test]
    fn test_glob_char_class_simple() {
        assert!(glob_match_str("a", "[abc]"));
        assert!(glob_match_str("b", "[abc]"));
        assert!(glob_match_str("c", "[abc]"));
        assert!(!glob_match_str("d", "[abc]"));
        assert!(!glob_match_str("", "[abc]"));
    }

    #[test]
    fn test_glob_char_class_range() {
        assert!(glob_match_str("a", "[a-z]"));
        assert!(glob_match_str("m", "[a-z]"));
        assert!(glob_match_str("z", "[a-z]"));
        assert!(!glob_match_str("A", "[a-z]"));
        assert!(!glob_match_str("0", "[a-z]"));

        assert!(glob_match_str("5", "[0-9]"));
        assert!(!glob_match_str("a", "[0-9]"));
    }

    #[test]
    fn test_glob_char_class_negation() {
        assert!(!glob_match_str("a", "[!abc]"));
        assert!(glob_match_str("d", "[!abc]"));
        assert!(glob_match_str("z", "[!abc]"));

        // ^ also works for negation
        assert!(!glob_match_str("a", "[^abc]"));
        assert!(glob_match_str("d", "[^abc]"));
    }

    #[test]
    fn test_glob_char_class_in_pattern() {
        assert!(glob_match_str("file1.txt", "file[0-9].txt"));
        assert!(glob_match_str("file9.txt", "file[0-9].txt"));
        assert!(!glob_match_str("filea.txt", "file[0-9].txt"));
        assert!(!glob_match_str("file12.txt", "file[0-9].txt"));
    }

    #[test]
    fn test_glob_complex_patterns() {
        assert!(glob_match_str("test_file_1.rs", "test_*_[0-9].rs"));
        assert!(glob_match_str("test_anything_5.rs", "test_*_[0-9].rs"));
        assert!(!glob_match_str("test_file_a.rs", "test_*_[0-9].rs"));
    }

    // =========================================================================
    // match_char_class tests
    // =========================================================================

    #[test]
    fn test_match_char_class_simple() {
        let pattern: Vec<char> = "[abc]".chars().collect();
        assert_eq!(match_char_class(&pattern, 'a'), (true, 5));
        assert_eq!(match_char_class(&pattern, 'b'), (true, 5));
        assert_eq!(match_char_class(&pattern, 'c'), (true, 5));
        assert_eq!(match_char_class(&pattern, 'd'), (false, 5));
    }

    #[test]
    fn test_match_char_class_range() {
        let pattern: Vec<char> = "[a-z]".chars().collect();
        assert_eq!(match_char_class(&pattern, 'a'), (true, 5));
        assert_eq!(match_char_class(&pattern, 'm'), (true, 5));
        assert_eq!(match_char_class(&pattern, 'z'), (true, 5));
        assert_eq!(match_char_class(&pattern, 'A'), (false, 5));
    }

    #[test]
    fn test_match_char_class_negation_exclamation() {
        let pattern: Vec<char> = "[!abc]".chars().collect();
        assert_eq!(match_char_class(&pattern, 'a'), (false, 6));
        assert_eq!(match_char_class(&pattern, 'd'), (true, 6));
    }

    #[test]
    fn test_match_char_class_negation_caret() {
        let pattern: Vec<char> = "[^abc]".chars().collect();
        assert_eq!(match_char_class(&pattern, 'a'), (false, 6));
        assert_eq!(match_char_class(&pattern, 'd'), (true, 6));
    }

    #[test]
    fn test_match_char_class_not_a_class() {
        let pattern: Vec<char> = "abc".chars().collect();
        assert_eq!(match_char_class(&pattern, 'a'), (false, 0));
    }

    #[test]
    fn test_match_char_class_empty() {
        let pattern: Vec<char> = vec![];
        assert_eq!(match_char_class(&pattern, 'a'), (false, 0));
    }

    // =========================================================================
    // extended_pattern_match tests
    // =========================================================================

    #[test]
    fn test_extended_pattern_exact() {
        assert!(extended_pattern_match("hello", "hello"));
        assert!(!extended_pattern_match("hello", "world"));
    }

    #[test]
    fn test_extended_pattern_with_glob() {
        assert!(extended_pattern_match("hello.txt", "*.txt"));
        assert!(extended_pattern_match("test", "t?st"));
        assert!(extended_pattern_match("a", "[abc]"));
    }

    #[test]
    fn test_extended_pattern_no_glob_chars() {
        // When no glob chars, should do exact match
        assert!(extended_pattern_match("hello", "hello"));
        assert!(!extended_pattern_match("hello", "hell"));
    }
}
