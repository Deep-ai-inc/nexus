//! Built-in shell commands.

use tokio::sync::broadcast::Sender;
use nexus_api::ShellEvent;
use std::path::PathBuf;

use crate::ShellState;

/// Check if a command name is a builtin.
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "pwd"
            | "echo"
            | "exit"
            | "export"
            | "unset"
            | "set"
            | "true"
            | "false"
            | ":"
            | "test"
            | "["
            | "type"
            | "alias"
            | "unalias"
            | "source"
            | "."
            | "jobs"
            | "fg"
            | "bg"
            | "read"
            | "shift"
            | "return"
            | "break"
            | "continue"
    )
}

/// Try to execute a builtin. Returns None if not a builtin.
pub fn try_builtin(
    name: &str,
    args: &[String],
    state: &mut ShellState,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<Option<i32>> {
    match name {
        "cd" => Ok(Some(builtin_cd(args, state, events)?)),
        "pwd" => Ok(Some(builtin_pwd(state)?)),
        "echo" => Ok(Some(builtin_echo(args)?)),
        "exit" => Ok(Some(builtin_exit(args)?)),
        "export" => Ok(Some(builtin_export(args, state, events)?)),
        "unset" => Ok(Some(builtin_unset(args, state, events)?)),
        "true" | ":" => Ok(Some(0)),
        "false" => Ok(Some(1)),
        "test" | "[" => Ok(Some(builtin_test(args)?)),
        "type" => Ok(Some(builtin_type(args, state)?)),
        _ => Ok(None),
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

fn builtin_pwd(state: &ShellState) -> anyhow::Result<i32> {
    println!("{}", state.cwd.display());
    Ok(0)
}

fn builtin_echo(args: &[String]) -> anyhow::Result<i32> {
    let mut newline = true;
    let mut interpret_escapes = false;
    let mut start_idx = 0;

    // Parse flags
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-n" => newline = false,
            "-e" => interpret_escapes = true,
            "-E" => interpret_escapes = false,
            _ => {
                start_idx = i;
                break;
            }
        }
        start_idx = i + 1;
    }

    let output = args[start_idx..].join(" ");

    let output = if interpret_escapes {
        interpret_escape_sequences(&output)
    } else {
        output
    };

    if newline {
        println!("{}", output);
    } else {
        print!("{}", output);
    }

    Ok(0)
}

fn interpret_escape_sequences(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('0') => {
                    // Octal escape
                    let mut octal = String::new();
                    for _ in 0..3 {
                        if let Some(&c) = chars.peek() {
                            if c.is_digit(8) {
                                octal.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(code) = u8::from_str_radix(&octal, 8) {
                        result.push(code as char);
                    }
                }
                Some(c) => {
                    result.push('\\');
                    result.push(c);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }

    result
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
            "-s" => {
                if let Ok(meta) = std::fs::metadata(val) {
                    if meta.len() > 0 { 0 } else { 1 }
                } else {
                    1
                }
            }
            "!" => if val.is_empty() { 0 } else { 1 },
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
            _ => 1,
        });
    }

    Ok(1)
}

fn builtin_type(args: &[String], state: &ShellState) -> anyhow::Result<i32> {
    let mut exit_code = 0;

    for arg in args {
        if is_builtin(arg) {
            println!("{} is a shell builtin", arg);
        } else if let Some(path) = find_in_path(arg, state) {
            println!("{} is {}", arg, path.display());
        } else {
            eprintln!("type: {}: not found", arg);
            exit_code = 1;
        }
    }

    Ok(exit_code)
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
