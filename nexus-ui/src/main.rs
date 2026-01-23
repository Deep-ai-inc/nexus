//! Nexus - The converged shell runtime.
//!
//! This is the main entry point for the Nexus application.

mod view_model;
mod components;

use std::io::{self, BufRead, Write};

use crossbeam_channel::unbounded;
use nexus_api::ShellEvent;
use nexus_kernel::Kernel;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    // Set up logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Nexus shell");

    // Create event channel
    let (event_tx, event_rx) = unbounded::<ShellEvent>();

    // Create the kernel
    let mut kernel = Kernel::new(event_tx)?;

    // For now, run in CLI mode until GPUI is integrated
    run_cli_mode(&mut kernel, event_rx)?;

    Ok(())
}

/// Run in basic CLI mode (no GUI).
fn run_cli_mode(
    kernel: &mut Kernel,
    event_rx: crossbeam_channel::Receiver<ShellEvent>,
) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Spawn a thread to print events
    let _event_thread = std::thread::spawn(move || {
        for event in event_rx {
            match event {
                ShellEvent::StdoutChunk { data, .. } => {
                    let _ = io::stdout().write_all(&data);
                    let _ = io::stdout().flush();
                }
                ShellEvent::StderrChunk { data, .. } => {
                    let _ = io::stderr().write_all(&data);
                    let _ = io::stderr().flush();
                }
                ShellEvent::CommandFinished { exit_code, .. } => {
                    if exit_code != 0 {
                        tracing::debug!("Command exited with code {}", exit_code);
                    }
                }
                ShellEvent::CwdChanged { new, .. } => {
                    tracing::debug!("Changed directory to {}", new.display());
                }
                _ => {}
            }
        }
    });

    loop {
        // Print prompt
        let cwd = kernel.state().cwd.display();
        print!("\x1b[1;34m{}\x1b[0m \x1b[1;32m❯\x1b[0m ", cwd);
        stdout.flush()?;

        // Read input
        let mut input = String::new();
        match stdin.lock().read_line(&mut input) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                continue;
            }
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Execute
        match kernel.execute(input) {
            Ok(_exit_code) => {
                // Give the event thread time to print output
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }

    Ok(())
}
