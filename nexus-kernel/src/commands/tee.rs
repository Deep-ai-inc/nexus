//! The `tee` command - read from stdin and write to files.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct TeeCommand;

impl NexusCommand for TeeCommand {
    fn name(&self) -> &'static str {
        "tee"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut append = false;
        let mut files = Vec::new();

        for arg in args {
            if arg == "-a" || arg == "--append" {
                append = true;
            } else if !arg.starts_with('-') {
                files.push(arg.clone());
            }
        }

        let value = if let Some(stdin_value) = ctx.stdin.take() {
            stdin_value
        } else {
            return Ok(Value::Unit);
        };

        // Convert value to text for writing to files
        let text = value.to_text();

        // Write to each file
        for file_path in &files {
            let path = if PathBuf::from(file_path).is_absolute() {
                PathBuf::from(file_path)
            } else {
                ctx.state.cwd.join(file_path)
            };

            let mut file = if append {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?
            } else {
                File::create(&path)?
            };

            writeln!(file, "{}", text)?;
        }

        // Pass through the original value (not just text)
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    // Tests would require temp files
}
