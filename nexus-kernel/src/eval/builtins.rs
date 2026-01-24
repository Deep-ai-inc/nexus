//! Built-in shell commands.

use nexus_api::ShellEvent;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::broadcast::Sender;

use crate::commands::CommandRegistry;
use crate::ShellState;

/// Check if a command name is a builtin.
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "pwd"
            | "echo"
            | "printf"
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
            | "eval"
            | "jobs"
            | "fg"
            | "bg"
            | "read"
            | "shift"
            | "return"
            | "break"
            | "continue"
            | "readonly"
            | "command"
            | "basename"
            | "dirname"
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
        "pwd" => Ok(Some(builtin_pwd(state)?)),
        "echo" => Ok(Some(builtin_echo(args)?)),
        "printf" => Ok(Some(builtin_printf(args)?)),
        "exit" => Ok(Some(builtin_exit(args)?)),
        "export" => Ok(Some(builtin_export(args, state, events)?)),
        "unset" => Ok(Some(builtin_unset(args, state, events)?)),
        "set" => Ok(Some(builtin_set(args, state)?)),
        "true" | ":" => Ok(Some(0)),
        "false" => Ok(Some(1)),
        "test" | "[" => Ok(Some(builtin_test(args)?)),
        "type" => Ok(Some(builtin_type(args, state, commands)?)),
        "source" | "." => Ok(Some(builtin_source(args, state, events, commands)?)),
        "eval" => Ok(Some(builtin_eval(args, state, events, commands)?)),
        "readonly" => Ok(Some(builtin_readonly(args, state)?)),
        "command" => Ok(Some(builtin_command(args, state, events, commands)?)),
        "basename" => Ok(Some(builtin_basename(args)?)),
        "dirname" => Ok(Some(builtin_dirname(args)?)),
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
        let _ = std::io::stdout().flush();
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

fn builtin_printf(args: &[String]) -> anyhow::Result<i32> {
    if args.is_empty() {
        eprintln!("printf: usage: printf format [arguments]");
        return Ok(1);
    }

    let format = &args[0];
    let mut arg_idx = 1;

    let mut result = String::new();
    let mut chars = format.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            // Handle escape sequences
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('0') => {
                    // Octal
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
                    if !octal.is_empty() {
                        if let Ok(code) = u8::from_str_radix(&octal, 8) {
                            result.push(code as char);
                        }
                    } else {
                        result.push('\0');
                    }
                }
                Some(c) => {
                    result.push('\\');
                    result.push(c);
                }
                None => result.push('\\'),
            }
        } else if c == '%' {
            // Handle format specifiers
            match chars.peek() {
                Some('%') => {
                    chars.next();
                    result.push('%');
                }
                Some(_) => {
                    // Parse width/precision
                    let mut spec = String::new();
                    spec.push('%');

                    // Flags
                    while let Some(&c) = chars.peek() {
                        if c == '-' || c == '+' || c == ' ' || c == '#' || c == '0' {
                            spec.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }

                    // Width
                    while let Some(&c) = chars.peek() {
                        if c.is_ascii_digit() {
                            spec.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }

                    // Precision
                    if chars.peek() == Some(&'.') {
                        spec.push(chars.next().unwrap());
                        while let Some(&c) = chars.peek() {
                            if c.is_ascii_digit() {
                                spec.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }

                    // Conversion specifier
                    if let Some(conv) = chars.next() {
                        spec.push(conv);
                        let arg = args.get(arg_idx).map(|s| s.as_str()).unwrap_or("");
                        arg_idx += 1;

                        match conv {
                            's' => result.push_str(arg),
                            'd' | 'i' => {
                                let n: i64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{}", n));
                            }
                            'u' => {
                                let n: u64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{}", n));
                            }
                            'o' => {
                                let n: u64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{:o}", n));
                            }
                            'x' => {
                                let n: u64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{:x}", n));
                            }
                            'X' => {
                                let n: u64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{:X}", n));
                            }
                            'f' | 'F' => {
                                let n: f64 = arg.parse().unwrap_or(0.0);
                                result.push_str(&format!("{}", n));
                            }
                            'e' => {
                                let n: f64 = arg.parse().unwrap_or(0.0);
                                result.push_str(&format!("{:e}", n));
                            }
                            'E' => {
                                let n: f64 = arg.parse().unwrap_or(0.0);
                                result.push_str(&format!("{:E}", n));
                            }
                            'c' => {
                                if let Some(c) = arg.chars().next() {
                                    result.push(c);
                                }
                            }
                            'b' => {
                                // %b interprets escape sequences in the argument
                                result.push_str(&interpret_escape_sequences(arg));
                            }
                            _ => {
                                result.push_str(&spec);
                            }
                        }
                    }
                }
                None => result.push('%'),
            }
        } else {
            result.push(c);
        }
    }

    print!("{}", result);
    let _ = std::io::stdout().flush();
    Ok(0)
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

fn builtin_type(
    args: &[String],
    state: &ShellState,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    let mut exit_code = 0;

    for arg in args {
        if is_builtin(arg) {
            println!("{} is a shell builtin", arg);
        } else if commands.contains(arg) {
            println!("{} is a native command", arg);
        } else if state.aliases.contains_key(arg) {
            println!("{} is aliased to `{}'", arg, state.aliases.get(arg).unwrap());
        } else if let Some(path) = find_in_path(arg, state) {
            println!("{} is {}", arg, path.display());
        } else {
            eprintln!("type: {}: not found", arg);
            exit_code = 1;
        }
    }

    Ok(exit_code)
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
    // For now, just execute normally since we don't have full alias support
    if args.is_empty() {
        return Ok(0);
    }

    let name = &args[0];
    let cmd_args = &args[1..];

    // Skip builtin check for -p flag
    let (name, cmd_args, use_default_path) = if name == "-p" {
        if args.len() < 2 {
            return Ok(0);
        }
        (&args[1], &args[2..], true)
    } else if name == "-v" || name == "-V" {
        // command -v acts like type
        return builtin_type(&args[1..].to_vec(), state, commands);
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

fn builtin_basename(args: &[String]) -> anyhow::Result<i32> {
    if args.is_empty() {
        eprintln!("basename: missing operand");
        return Ok(1);
    }

    let path = PathBuf::from(&args[0]);
    let name = path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // Optional suffix removal
    let result = if args.len() > 1 {
        let suffix = &args[1];
        name.strip_suffix(suffix).unwrap_or(&name).to_string()
    } else {
        name
    };

    println!("{}", result);
    Ok(0)
}

fn builtin_dirname(args: &[String]) -> anyhow::Result<i32> {
    if args.is_empty() {
        eprintln!("dirname: missing operand");
        return Ok(1);
    }

    let path = PathBuf::from(&args[0]);
    let parent = path.parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let result = if parent.is_empty() { "." } else { &parent };
    println!("{}", result);
    Ok(0)
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
