//! `clip` — clipboard read/write.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct ClipCommand;

impl NexusCommand for ClipCommand {
    fn name(&self) -> &'static str {
        "clip"
    }

    fn description(&self) -> &'static str {
        "Copy to or paste from the system clipboard"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut force_copy = false;
        let mut force_paste = false;

        for arg in args {
            match arg.as_str() {
                "-i" | "--copy" => force_copy = true,
                "-o" | "--paste" => force_paste = true,
                _ => {}
            }
        }

        // Determine direction:
        // - explicit -i/-o flags override
        // - if stdin is present, copy to clipboard
        // - otherwise, paste from clipboard
        let should_copy = force_copy || (!force_paste && ctx.stdin.is_some());

        if should_copy {
            let text = match &ctx.stdin {
                Some(value) => value.to_text(),
                None => anyhow::bail!("clip: no input to copy (pipe data to clip)"),
            };

            let mut clipboard = arboard::Clipboard::new()
                .map_err(|e| anyhow::anyhow!("clip: failed to access clipboard: {}", e))?;
            clipboard
                .set_text(&text)
                .map_err(|e| anyhow::anyhow!("clip: failed to copy: {}", e))?;

            Ok(Value::Unit)
        } else {
            let mut clipboard = arboard::Clipboard::new()
                .map_err(|e| anyhow::anyhow!("clip: failed to access clipboard: {}", e))?;
            let text = clipboard
                .get_text()
                .map_err(|e| anyhow::anyhow!("clip: failed to paste: {}", e))?;

            Ok(Value::String(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_description() {
        let cmd = ClipCommand;
        assert_eq!(cmd.name(), "clip");
        assert!(!cmd.description().is_empty());
    }

    // Note: clipboard tests require a display server / pasteboard daemon,
    // so we keep integration tests minimal. The logic is straightforward:
    // arboard handles the platform-specific clipboard access.
}
