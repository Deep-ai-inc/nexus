//! Evaluator - AST walker that executes commands.

mod expand;
mod builtins;

use tokio::sync::broadcast::Sender;
use nexus_api::ShellEvent;

use crate::parser::*;
use crate::process;
use crate::state::{next_block_id, ShellState};

pub use builtins::is_builtin;

/// Execute an AST and return the final exit code.
pub fn execute(
    state: &mut ShellState,
    ast: &Ast,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    for command in &ast.commands {
        last_exit = execute_command(state, command, events)?;
        state.last_exit_code = last_exit;
    }

    Ok(last_exit)
}

/// Execute a single command.
fn execute_command(
    state: &mut ShellState,
    command: &Command,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    match command {
        Command::Simple(simple) => execute_simple(state, simple, events),
        Command::Pipeline(pipeline) => execute_pipeline(state, pipeline, events),
        Command::List(list) => execute_list(state, list, events),
        Command::Subshell(subshell) => execute_subshell(state, subshell, events),
        Command::Assignment(assignment) => execute_assignment(state, assignment),
        Command::If(if_stmt) => execute_if(state, if_stmt, events),
        Command::While(while_stmt) => execute_while(state, while_stmt, events),
        Command::For(for_stmt) => execute_for(state, for_stmt, events),
    }
}

/// Execute a simple command.
fn execute_simple(
    state: &mut ShellState,
    cmd: &SimpleCommand,
    events: &Sender<ShellEvent>,
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

    // Check for builtins
    if let Some(exit_code) = builtins::try_builtin(&name, &args, state, events)? {
        return Ok(exit_code);
    }

    // External command - spawn a process
    let block_id = next_block_id();

    // Emit command started event
    let _ = events.send(ShellEvent::CommandStarted {
        block_id,
        command: format!("{} {}", name, args.join(" ")),
        cwd: state.cwd.clone(),
    });

    // Build the full argv
    let mut argv = vec![name];
    argv.extend(args);

    // Spawn the process
    let handle = process::spawn(&argv, &state.cwd, &state.env, &env_overrides, &cmd.redirects)?;

    // Wait for completion and stream output
    let exit_code = process::wait_with_events(handle, block_id, events)?;

    Ok(exit_code)
}

/// Execute a pipeline.
fn execute_pipeline(
    state: &mut ShellState,
    pipeline: &Pipeline,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    if pipeline.commands.len() == 1 {
        return execute_command(state, &pipeline.commands[0], events);
    }

    // For now, execute sequentially and connect with pipes
    // TODO: Proper pipe setup with pump threads
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

/// Execute a list of commands with &&, ||, ;, &.
fn execute_list(
    state: &mut ShellState,
    list: &List,
    events: &Sender<ShellEvent>,
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
                last_exit = execute_command(state, cmd, events)?;
            } else {
                last_exit = execute_command(state, cmd, events)?;
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
) -> anyhow::Result<i32> {
    // Create a copy of state for the subshell
    // In a real implementation, this would fork
    let mut subshell_state = ShellState::new()?;
    subshell_state.env = state.env.clone();
    subshell_state.vars = state.vars.clone();
    subshell_state.cwd = state.cwd.clone();

    let mut last_exit = 0;
    for cmd in &subshell.commands {
        last_exit = execute_command(&mut subshell_state, cmd, events)?;
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
) -> anyhow::Result<i32> {
    // Execute condition
    let mut condition_exit = 0;
    for cmd in &if_stmt.condition {
        condition_exit = execute_command(state, cmd, events)?;
    }

    if condition_exit == 0 {
        // Execute then branch
        let mut last_exit = 0;
        for cmd in &if_stmt.then_branch {
            last_exit = execute_command(state, cmd, events)?;
        }
        Ok(last_exit)
    } else if let Some(else_branch) = &if_stmt.else_branch {
        // Execute else branch
        let mut last_exit = 0;
        for cmd in else_branch {
            last_exit = execute_command(state, cmd, events)?;
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
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    loop {
        // Execute condition
        let mut condition_exit = 0;
        for cmd in &while_stmt.condition {
            condition_exit = execute_command(state, cmd, events)?;
        }

        if condition_exit != 0 {
            break;
        }

        // Execute body
        for cmd in &while_stmt.body {
            last_exit = execute_command(state, cmd, events)?;
        }
    }

    Ok(last_exit)
}

/// Execute a for loop.
fn execute_for(
    state: &mut ShellState,
    for_stmt: &ForStatement,
    events: &Sender<ShellEvent>,
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
            last_exit = execute_command(state, cmd, events)?;
        }
    }

    Ok(last_exit)
}
