//! The `top` command - interactive process monitor.
//!
//! Sends an Interactive value to set up the viewer, then loops
//! sending StreamingUpdate events until cancelled. The entire
//! refresh loop runs kernel-side — the UI only renders.

use std::sync::atomic::Ordering;

use nexus_api::{InteractiveRequest, ShellEvent, Value, ViewerKind};

use super::{CommandContext, NexusCommand};

pub struct TopCommand;

impl NexusCommand for TopCommand {
    fn name(&self) -> &'static str {
        "top"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut interval_ms: u64 = 2000;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-d" | "--delay" => {
                    if i + 1 < args.len() {
                        if let Ok(secs) = args[i + 1].parse::<f64>() {
                            interval_ms = (secs * 1000.0) as u64;
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }

        // Get initial process snapshot
        let initial = super::ps::get_process_table()?;

        // Send the Interactive value so the UI sets up the viewer
        let _ = ctx.events.send(ShellEvent::CommandOutput {
            block_id: ctx.block_id,
            value: Value::Interactive(Box::new(InteractiveRequest {
                viewer: ViewerKind::ProcessMonitor { interval_ms },
                content: initial,
            })),
        });

        // Register for cancellation (UI sets this when user exits the viewer)
        let cancel = super::register_cancel(ctx.block_id);

        // Refresh loop — runs until cancelled
        let mut seq: u64 = 0;
        while !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            match super::ps::get_process_table() {
                Ok(table) => {
                    seq += 1;
                    let _ = ctx.events.send(ShellEvent::StreamingUpdate {
                        block_id: ctx.block_id,
                        seq,
                        update: table,
                        coalesce: true,
                    });
                }
                Err(_) => break,
            }
        }

        super::unregister_cancel(ctx.block_id);

        // Return Unit — the real output was already sent via events
        Ok(Value::Unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_top_default() {
        let block_id = nexus_api::BlockId(9001);
        let bid = block_id;
        // Poll until the cancel flag is registered, then set it.
        // This avoids a race where a fixed sleep fires before register_cancel().
        std::thread::spawn(move || {
            while !crate::commands::cancel_block(bid) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });

        let mut test_ctx = TestContext::new_default();
        let mut ctx = test_ctx.ctx();
        ctx.block_id = block_id;
        let cmd = TopCommand;
        // Use a short interval so the loop checks the cancel flag quickly.
        let result = cmd
            .execute(&["-d".to_string(), "0.05".to_string()], &mut ctx)
            .unwrap();

        // top returns Unit (output sent via events)
        assert!(matches!(result, Value::Unit));
    }

    #[test]
    fn test_top_custom_delay() {
        let block_id = nexus_api::BlockId(9002);
        let bid = block_id;
        std::thread::spawn(move || {
            while !crate::commands::cancel_block(bid) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });

        let mut test_ctx = TestContext::new_default();
        let mut ctx = test_ctx.ctx();
        ctx.block_id = block_id;
        let cmd = TopCommand;
        let result = cmd
            .execute(&["-d".to_string(), "0.05".to_string()], &mut ctx)
            .unwrap();

        assert!(matches!(result, Value::Unit));
    }
}
