//! Evaluator - AST walker that executes commands.

mod builtins;
mod expand;

use nexus_api::{ShellEvent, Value};
use tokio::sync::broadcast::Sender;

use nexus_api::BlockId;

use crate::commands::{CommandContext, CommandRegistry};
use crate::parser::*;
use crate::process;
use crate::state::{get_or_create_block_id, ShellState};

pub use builtins::is_builtin;
use builtins::{BREAK_EXIT_CODE, CONTINUE_EXIT_CODE, RETURN_EXIT_CODE};

/// Check if an exit code represents a break signal.
fn is_break(exit_code: i32) -> Option<u32> {
    if exit_code >= BREAK_EXIT_CODE && exit_code < CONTINUE_EXIT_CODE {
        Some((exit_code - BREAK_EXIT_CODE + 1) as u32)
    } else {
        None
    }
}

/// Check if an exit code represents a continue signal.
fn is_continue(exit_code: i32) -> Option<u32> {
    if exit_code >= CONTINUE_EXIT_CODE && exit_code < CONTINUE_EXIT_CODE + 100 {
        Some((exit_code - CONTINUE_EXIT_CODE + 1) as u32)
    } else {
        None
    }
}

/// Decrement a break/continue level and return the new exit code.
fn decrement_level(exit_code: i32) -> i32 {
    if let Some(level) = is_break(exit_code) {
        if level > 1 {
            BREAK_EXIT_CODE + level as i32 - 2
        } else {
            0 // Break consumed at this level
        }
    } else if let Some(level) = is_continue(exit_code) {
        if level > 1 {
            CONTINUE_EXIT_CODE + level as i32 - 2
        } else {
            0 // Continue consumed at this level
        }
    } else {
        exit_code
    }
}

/// Execute an AST and return the final exit code.
pub fn execute(
    state: &mut ShellState,
    ast: &Ast,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    execute_with_block_id(state, ast, events, commands, None)
}

/// Execute an AST with a specific block ID (for UI integration).
pub fn execute_with_block_id(
    state: &mut ShellState,
    ast: &Ast,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    for command in &ast.commands {
        last_exit = execute_command(state, command, events, commands, block_id)?;
        state.last_exit_code = last_exit;
    }

    Ok(last_exit)
}

/// Execute a single command.
fn execute_command(
    state: &mut ShellState,
    command: &Command,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    match command {
        Command::Simple(simple) => execute_simple(state, simple, events, commands, block_id),
        Command::Pipeline(pipeline) => execute_pipeline(state, pipeline, events, commands, block_id),
        Command::List(list) => execute_list(state, list, events, commands, block_id),
        Command::Subshell(subshell) => execute_subshell(state, subshell, events, commands, block_id),
        Command::Assignment(assignment) => execute_assignment(state, assignment),
        Command::If(if_stmt) => execute_if(state, if_stmt, events, commands, block_id),
        Command::While(while_stmt) => execute_while(state, while_stmt, events, commands, block_id),
        Command::For(for_stmt) => execute_for(state, for_stmt, events, commands, block_id),
        Command::Function(func_def) => execute_function_def(state, func_def),
        Command::Case(case_stmt) => execute_case(state, case_stmt, events, commands, block_id),
    }
}

/// Execute a simple command.
fn execute_simple(
    state: &mut ShellState,
    cmd: &SimpleCommand,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    // Expand the command name and arguments
    let name = expand::expand_word_to_string(&Word::Literal(cmd.name.clone()), state);
    // Use expand_word_to_strings to handle glob expansion (*.txt -> multiple files)
    let args: Vec<String> = cmd
        .args
        .iter()
        .flat_map(|w| expand::expand_word_to_strings(w, state))
        .collect();

    // Apply any command-specific environment assignments
    let env_overrides: Vec<(String, String)> = cmd
        .env_assignments
        .iter()
        .map(|a| {
            (
                a.name.clone(),
                expand::expand_word_to_string(&a.value, state),
            )
        })
        .collect();

    // Check for builtins (shell-specific: cd, export, etc.)
    if let Some(exit_code) = builtins::try_builtin(&name, &args, state, events, commands)? {
        return Ok(exit_code);
    }

    // Check for user-defined functions
    if let Some(func_def) = state.get_function(&name).cloned() {
        return execute_function_call(state, &func_def, &args, events, commands, block_id);
    }

    // Check for native commands (in-process: ls, cat, etc.)
    if let Some(native_cmd) = commands.get(&name) {
        return execute_native(state, native_cmd, &args, events, block_id);
    }

    // External command - spawn a process via PTY (legacy)
    execute_external(state, &name, args, env_overrides, &cmd.redirects, events, block_id)
}

/// Execute a native (in-process) command.
fn execute_native(
    state: &mut ShellState,
    cmd: &dyn crate::commands::NexusCommand,
    args: &[String],
    events: &Sender<ShellEvent>,
    external_block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    let block_id = get_or_create_block_id(external_block_id);

    // Only emit CommandStarted if we created a new block_id
    // (if external_block_id was provided, the UI already created the block)
    if external_block_id.is_none() {
        let _ = events.send(ShellEvent::CommandStarted {
            block_id,
            command: format!("{} {}", cmd.name(), args.join(" ")),
            cwd: state.cwd.clone(),
        });
    }

    let start = std::time::Instant::now();

    // Create command context
    let mut ctx = CommandContext {
        state,
        events,
        block_id,
        stdin: None, // TODO: piped input support
    };

    // Execute the command
    let result = cmd.execute(args, &mut ctx);

    let duration_ms = start.elapsed().as_millis() as u64;

    let command_str = format!("{} {}", cmd.name(), args.join(" "));

    match result {
        Ok(value) => {
            // Store output for $_ / $prev and $_N references (Persistent Memory)
            if !matches!(value, Value::Unit) {
                ctx.state.store_output(block_id, command_str.clone(), value.clone());
            }

            // Emit structured output
            let _ = events.send(ShellEvent::CommandOutput {
                block_id,
                value: value.clone(),
            });

            // Emit command finished
            let _ = events.send(ShellEvent::CommandFinished {
                block_id,
                exit_code: 0,
                duration_ms,
            });

            Ok(0)
        }
        Err(e) => {
            // Emit error as stderr
            let _ = events.send(ShellEvent::StderrChunk {
                block_id,
                data: format!("{}: {}\n", cmd.name(), e).into_bytes(),
            });

            let _ = events.send(ShellEvent::CommandFinished {
                block_id,
                exit_code: 1,
                duration_ms,
            });

            Ok(1)
        }
    }
}

/// Execute an external command via PTY (legacy path).
fn execute_external(
    state: &mut ShellState,
    name: &str,
    args: Vec<String>,
    env_overrides: Vec<(String, String)>,
    redirects: &[Redirect],
    events: &Sender<ShellEvent>,
    external_block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    let block_id = get_or_create_block_id(external_block_id);

    // Only emit CommandStarted if we created a new block_id
    if external_block_id.is_none() {
        let _ = events.send(ShellEvent::CommandStarted {
            block_id,
            command: format!("{} {}", name, args.join(" ")),
            cwd: state.cwd.clone(),
        });
    }

    // Build the full argv
    let mut argv = vec![name.to_string()];
    argv.extend(args);

    // Spawn the process
    let handle = process::spawn(&argv, &state.cwd, &state.env, &env_overrides, redirects)?;

    // Wait for completion and stream output
    let exit_code = process::wait_with_events(handle, block_id, events)?;

    Ok(exit_code)
}

/// Execute a pipeline.
fn execute_pipeline(
    state: &mut ShellState,
    pipeline: &Pipeline,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    external_block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    if pipeline.commands.len() == 1 {
        return execute_command(state, &pipeline.commands[0], events, commands, external_block_id);
    }

    // Check if any command in the pipeline is a native command
    let has_native = pipeline.commands.iter().any(|cmd| {
        if let Command::Simple(simple) = cmd {
            commands.contains(&simple.name)
        } else {
            false
        }
    });

    if has_native {
        // Use native pipeline execution for mixed or all-native pipelines
        execute_native_pipeline(state, pipeline, events, commands, external_block_id)
    } else {
        // All external - use legacy path
        let block_id = get_or_create_block_id(external_block_id);

        if external_block_id.is_none() {
            let _ = events.send(ShellEvent::CommandStarted {
                block_id,
                command: "[pipeline]".to_string(),
                cwd: state.cwd.clone(),
            });
        }

        let handles = process::spawn_pipeline(state, &pipeline.commands)?;
        let exit_code = process::wait_pipeline(handles, block_id, events)?;

        Ok(exit_code)
    }
}

/// Execute a pipeline containing native commands.
///
/// For each stage in the pipeline:
/// 1. If native command: execute in-process, capture `Value` output
/// 2. If external command AND previous was native: serialize `Value` to text, pipe to stdin
/// 3. If external command AND previous was external: currently unsupported (use legacy path)
fn execute_native_pipeline(
    state: &mut ShellState,
    pipeline: &Pipeline,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    external_block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    use nexus_api::Value;

    let block_id = get_or_create_block_id(external_block_id);

    // Build command string for display
    let cmd_str = pipeline
        .commands
        .iter()
        .filter_map(|cmd| {
            if let Command::Simple(s) = cmd {
                let args_str = s.args.iter().filter_map(|w| w.as_literal()).collect::<Vec<_>>().join(" ");
                if args_str.is_empty() {
                    Some(s.name.clone())
                } else {
                    Some(format!("{} {}", s.name, args_str))
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");

    // Only emit CommandStarted if we created a new block_id
    if external_block_id.is_none() {
        let _ = events.send(ShellEvent::CommandStarted {
            block_id,
            command: cmd_str,
            cwd: state.cwd.clone(),
        });
    }

    let start = std::time::Instant::now();
    let mut current_value: Option<Value> = None;
    let mut last_exit = 0;

    for cmd in &pipeline.commands {
        let Command::Simple(simple) = cmd else {
            continue;
        };

        // Expand command name and args (with glob expansion)
        let name = expand::expand_word_to_string(&Word::Literal(simple.name.clone()), state);
        let args: Vec<String> = simple
            .args
            .iter()
            .flat_map(|w| expand::expand_word_to_strings(w, state))
            .collect();

        if let Some(native_cmd) = commands.get(&name) {
            // Native command: pass Value via ctx.stdin
            let mut ctx = CommandContext {
                state,
                events,
                block_id,
                stdin: current_value.take(),
            };

            match native_cmd.execute(&args, &mut ctx) {
                Ok(value) => {
                    // Don't pass Unit values down the pipeline
                    current_value = if matches!(value, Value::Unit) {
                        None
                    } else {
                        Some(value)
                    };
                    last_exit = 0;
                }
                Err(e) => {
                    let _ = events.send(ShellEvent::StderrChunk {
                        block_id,
                        data: format!("{}: {}\n", name, e).into_bytes(),
                    });
                    let _ = events.send(ShellEvent::CommandFinished {
                        block_id,
                        exit_code: 1,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                    return Ok(1);
                }
            }
        } else {
            // External command: serialize Value to text, spawn process with stdin
            let input_text = current_value.take().map(|v| v.to_text());
            last_exit = process::spawn_with_stdin(
                &name,
                &args,
                input_text,
                state,
                block_id,
                events,
            )?;
            current_value = None; // External commands produce bytes, not Value
        }
    }

    // Build command string for storage
    let cmd_str = pipeline
        .commands
        .iter()
        .filter_map(|cmd| {
            if let Command::Simple(s) = cmd {
                let args_str = s.args.iter().filter_map(|w| w.as_literal()).collect::<Vec<_>>().join(" ");
                if args_str.is_empty() {
                    Some(s.name.clone())
                } else {
                    Some(format!("{} {}", s.name, args_str))
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");

    // Emit final output if we have a value from a native command
    if let Some(ref value) = current_value {
        // Store output for $_ / $prev (Persistent Memory)
        state.store_output(block_id, cmd_str, value.clone());

        let _ = events.send(ShellEvent::CommandOutput {
            block_id,
            value: value.clone(),
        });
    }

    let _ = events.send(ShellEvent::CommandFinished {
        block_id,
        exit_code: last_exit,
        duration_ms: start.elapsed().as_millis() as u64,
    });

    Ok(last_exit)
}

/// Execute a list of commands with &&, ||, ;, &.
fn execute_list(
    state: &mut ShellState,
    list: &List,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    for (i, cmd) in list.items.iter().enumerate() {
        // Determine whether to execute based on previous result and operator
        let should_execute = if i == 0 {
            true
        } else {
            match list.operators.get(i - 1) {
                Some(ListOperator::And) => last_exit == 0,
                Some(ListOperator::Or) => last_exit != 0,
                Some(ListOperator::Semi) | Some(ListOperator::Background) | None => true,
            }
        };

        if should_execute {
            let is_background = list
                .operators
                .get(i)
                .map(|op| *op == ListOperator::Background)
                .unwrap_or(false);

            if is_background {
                // TODO: Spawn in background
                last_exit = execute_command(state, cmd, events, commands, block_id)?;
            } else {
                last_exit = execute_command(state, cmd, events, commands, block_id)?;
            }
        }
    }

    Ok(last_exit)
}

/// Execute a subshell.
fn execute_subshell(
    state: &mut ShellState,
    subshell: &Subshell,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    // Create a copy of state for the subshell
    // In a real implementation, this would fork
    let mut subshell_state = ShellState::new()?;
    subshell_state.env = state.env.clone();
    subshell_state.vars = state.vars.clone();
    subshell_state.cwd = state.cwd.clone();

    let mut last_exit = 0;
    for cmd in &subshell.commands {
        last_exit = execute_command(&mut subshell_state, cmd, events, commands, block_id)?;
    }

    Ok(last_exit)
}

/// Execute a variable assignment.
fn execute_assignment(state: &mut ShellState, assignment: &Assignment) -> anyhow::Result<i32> {
    let value = expand::expand_word_to_string(&assignment.value, state);
    state.set_var(assignment.name.clone(), value);
    Ok(0)
}

/// Execute an if statement.
fn execute_if(
    state: &mut ShellState,
    if_stmt: &IfStatement,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    // Execute condition
    let mut condition_exit = 0;
    for cmd in &if_stmt.condition {
        condition_exit = execute_command(state, cmd, events, commands, block_id)?;
    }

    if condition_exit == 0 {
        // Execute then branch
        let mut last_exit = 0;
        for cmd in &if_stmt.then_branch {
            last_exit = execute_command(state, cmd, events, commands, block_id)?;
        }
        Ok(last_exit)
    } else if let Some(else_branch) = &if_stmt.else_branch {
        // Execute else branch
        let mut last_exit = 0;
        for cmd in else_branch {
            last_exit = execute_command(state, cmd, events, commands, block_id)?;
        }
        Ok(last_exit)
    } else {
        Ok(0)
    }
}

/// Execute a while loop.
fn execute_while(
    state: &mut ShellState,
    while_stmt: &WhileStatement,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    'outer: loop {
        // Execute condition
        let mut condition_exit = 0;
        for cmd in &while_stmt.condition {
            condition_exit = execute_command(state, cmd, events, commands, block_id)?;
        }

        if condition_exit != 0 {
            break;
        }

        // Execute body
        for cmd in &while_stmt.body {
            last_exit = execute_command(state, cmd, events, commands, block_id)?;

            // Handle break
            if let Some(level) = is_break(last_exit) {
                if level == 1 {
                    last_exit = 0;
                    break 'outer;
                } else {
                    // Propagate break to outer loop
                    return Ok(decrement_level(last_exit));
                }
            }

            // Handle continue
            if let Some(level) = is_continue(last_exit) {
                if level == 1 {
                    last_exit = 0;
                    continue 'outer;
                } else {
                    // Propagate continue to outer loop
                    return Ok(decrement_level(last_exit));
                }
            }
        }
    }

    Ok(last_exit)
}

/// Execute a for loop.
fn execute_for(
    state: &mut ShellState,
    for_stmt: &ForStatement,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    // Expand items with glob and brace expansion (e.g., for f in *.rs, for i in {1..10})
    let items: Vec<String> = for_stmt
        .items
        .iter()
        .flat_map(|w| expand::expand_word_to_strings(w, state))
        .collect();

    'outer: for item in items {
        state.set_var(for_stmt.variable.clone(), item);

        for cmd in &for_stmt.body {
            last_exit = execute_command(state, cmd, events, commands, block_id)?;

            // Handle break
            if let Some(level) = is_break(last_exit) {
                if level == 1 {
                    last_exit = 0;
                    break 'outer;
                } else {
                    // Propagate break to outer loop
                    return Ok(decrement_level(last_exit));
                }
            }

            // Handle continue
            if let Some(level) = is_continue(last_exit) {
                if level == 1 {
                    last_exit = 0;
                    continue 'outer;
                } else {
                    // Propagate continue to outer loop
                    return Ok(decrement_level(last_exit));
                }
            }
        }
    }

    Ok(last_exit)
}

/// Define a function (store it in state).
fn execute_function_def(state: &mut ShellState, func_def: &FunctionDef) -> anyhow::Result<i32> {
    state.define_function(func_def.name.clone(), func_def.clone());
    Ok(0)
}

/// Execute a function call.
fn execute_function_call(
    state: &mut ShellState,
    func_def: &FunctionDef,
    args: &[String],
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    // Save old positional parameters
    let old_params = std::mem::take(&mut state.positional_params);

    // Set new positional parameters from function arguments
    state.positional_params = args.to_vec();

    // Enter a new local scope
    state.push_scope();

    let mut last_exit = 0;

    // Execute function body
    for cmd in &func_def.body {
        last_exit = execute_command(state, cmd, events, commands, block_id)?;

        // Handle return builtin (exit code >= RETURN_EXIT_CODE)
        if last_exit >= RETURN_EXIT_CODE {
            last_exit = last_exit - RETURN_EXIT_CODE;
            break;
        }
    }

    // Exit local scope (restore local variables)
    state.pop_scope();

    // Restore old positional parameters
    state.positional_params = old_params;

    Ok(last_exit)
}

/// Execute a case statement.
fn execute_case(
    state: &mut ShellState,
    case_stmt: &CaseStatement,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
    block_id: Option<BlockId>,
) -> anyhow::Result<i32> {
    // Expand the word to match against
    let word = expand::expand_word_to_string(&case_stmt.word, state);

    let mut last_exit = 0;

    // Find matching case
    for case_item in &case_stmt.cases {
        let matches = case_item.patterns.iter().any(|pattern| {
            // Expand pattern (handles variables, etc.)
            let expanded_pattern = expand::expand_tilde(pattern, state);
            pattern_matches(&word, &expanded_pattern)
        });

        if matches {
            // Execute commands for this case
            for cmd in &case_item.commands {
                last_exit = execute_command(state, cmd, events, commands, block_id)?;
            }
            break; // Only execute first matching case (unless ;& is used, but we don't support that yet)
        }
    }

    Ok(last_exit)
}

/// Check if a word matches a shell pattern (glob-style).
fn pattern_matches(word: &str, pattern: &str) -> bool {
    // Handle special case: * matches everything
    if pattern == "*" {
        return true;
    }

    // Convert shell pattern to regex
    let regex_pattern = pattern
        .chars()
        .map(|c| match c {
            '*' => ".*".to_string(),
            '?' => ".".to_string(),
            '[' | ']' => c.to_string(), // Character classes pass through
            '.' | '+' | '^' | '$' | '(' | ')' | '{' | '}' | '|' | '\\' => {
                format!("\\{}", c)
            }
            _ => c.to_string(),
        })
        .collect::<String>();

    // Anchor the pattern
    let anchored = format!("^{}$", regex_pattern);

    regex::Regex::new(&anchored)
        .map(|re| re.is_match(word))
        .unwrap_or(false)
}
