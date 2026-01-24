//! Evaluator - AST walker that executes commands.

mod builtins;
mod expand;

use nexus_api::ShellEvent;
use tokio::sync::broadcast::Sender;

use crate::commands::{CommandContext, CommandRegistry};
use crate::parser::*;
use crate::process;
use crate::state::{next_block_id, ShellState};

pub use builtins::is_builtin;

/// Execute an AST and return the final exit code.
pub fn execute(
    state: &mut ShellState,
    ast: &Ast,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    for command in &ast.commands {
        last_exit = execute_command(state, command, events, commands)?;
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
) -> anyhow::Result<i32> {
    match command {
        Command::Simple(simple) => execute_simple(state, simple, events, commands),
        Command::Pipeline(pipeline) => execute_pipeline(state, pipeline, events, commands),
        Command::List(list) => execute_list(state, list, events, commands),
        Command::Subshell(subshell) => execute_subshell(state, subshell, events, commands),
        Command::Assignment(assignment) => execute_assignment(state, assignment),
        Command::If(if_stmt) => execute_if(state, if_stmt, events, commands),
        Command::While(while_stmt) => execute_while(state, while_stmt, events, commands),
        Command::For(for_stmt) => execute_for(state, for_stmt, events, commands),
    }
}

/// Execute a simple command.
fn execute_simple(
    state: &mut ShellState,
    cmd: &SimpleCommand,
    events: &Sender<ShellEvent>,
    commands: &CommandRegistry,
) -> anyhow::Result<i32> {
    // Expand the command name and arguments
    let name = expand::expand_word_to_string(&Word::Literal(cmd.name.clone()), state);
    let args: Vec<String> = cmd
        .args
        .iter()
        .map(|w| expand::expand_word_to_string(w, state))
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

    // Check for native commands (in-process: ls, cat, etc.)
    if let Some(native_cmd) = commands.get(&name) {
        return execute_native(state, native_cmd, &args, events);
    }

    // External command - spawn a process via PTY (legacy)
    execute_external(state, &name, args, env_overrides, &cmd.redirects, events)
}

/// Execute a native (in-process) command.
fn execute_native(
    state: &mut ShellState,
    cmd: &dyn crate::commands::NexusCommand,
    args: &[String],
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    let block_id = next_block_id();

    // Emit command started event
    let _ = events.send(ShellEvent::CommandStarted {
        block_id,
        command: format!("{} {}", cmd.name(), args.join(" ")),
        cwd: state.cwd.clone(),
    });

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

    match result {
        Ok(value) => {
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
) -> anyhow::Result<i32> {
    let block_id = next_block_id();

    // Emit command started event
    let _ = events.send(ShellEvent::CommandStarted {
        block_id,
        command: format!("{} {}", name, args.join(" ")),
        cwd: state.cwd.clone(),
    });

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
) -> anyhow::Result<i32> {
    if pipeline.commands.len() == 1 {
        return execute_command(state, &pipeline.commands[0], events, commands);
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
        execute_native_pipeline(state, pipeline, events, commands)
    } else {
        // All external - use legacy path
        let block_id = next_block_id();

        let _ = events.send(ShellEvent::CommandStarted {
            block_id,
            command: "[pipeline]".to_string(),
            cwd: state.cwd.clone(),
        });

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
) -> anyhow::Result<i32> {
    use nexus_api::Value;

    let block_id = next_block_id();

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

    let _ = events.send(ShellEvent::CommandStarted {
        block_id,
        command: cmd_str,
        cwd: state.cwd.clone(),
    });

    let start = std::time::Instant::now();
    let mut current_value: Option<Value> = None;
    let mut last_exit = 0;

    for cmd in &pipeline.commands {
        let Command::Simple(simple) = cmd else {
            continue;
        };

        // Expand command name and args
        let name = expand::expand_word_to_string(&Word::Literal(simple.name.clone()), state);
        let args: Vec<String> = simple
            .args
            .iter()
            .map(|w| expand::expand_word_to_string(w, state))
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

    // Emit final output if we have a value from a native command
    if let Some(value) = current_value {
        let _ = events.send(ShellEvent::CommandOutput { block_id, value });
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
                last_exit = execute_command(state, cmd, events, commands)?;
            } else {
                last_exit = execute_command(state, cmd, events, commands)?;
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
) -> anyhow::Result<i32> {
    // Create a copy of state for the subshell
    // In a real implementation, this would fork
    let mut subshell_state = ShellState::new()?;
    subshell_state.env = state.env.clone();
    subshell_state.vars = state.vars.clone();
    subshell_state.cwd = state.cwd.clone();

    let mut last_exit = 0;
    for cmd in &subshell.commands {
        last_exit = execute_command(&mut subshell_state, cmd, events, commands)?;
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
) -> anyhow::Result<i32> {
    // Execute condition
    let mut condition_exit = 0;
    for cmd in &if_stmt.condition {
        condition_exit = execute_command(state, cmd, events, commands)?;
    }

    if condition_exit == 0 {
        // Execute then branch
        let mut last_exit = 0;
        for cmd in &if_stmt.then_branch {
            last_exit = execute_command(state, cmd, events, commands)?;
        }
        Ok(last_exit)
    } else if let Some(else_branch) = &if_stmt.else_branch {
        // Execute else branch
        let mut last_exit = 0;
        for cmd in else_branch {
            last_exit = execute_command(state, cmd, events, commands)?;
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
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    loop {
        // Execute condition
        let mut condition_exit = 0;
        for cmd in &while_stmt.condition {
            condition_exit = execute_command(state, cmd, events, commands)?;
        }

        if condition_exit != 0 {
            break;
        }

        // Execute body
        for cmd in &while_stmt.body {
            last_exit = execute_command(state, cmd, events, commands)?;
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
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    let items: Vec<String> = for_stmt
        .items
        .iter()
        .map(|w| expand::expand_word_to_string(w, state))
        .collect();

    for item in items {
        state.set_var(for_stmt.variable.clone(), item);

        for cmd in &for_stmt.body {
            last_exit = execute_command(state, cmd, events, commands)?;
        }
    }

    Ok(last_exit)
}
