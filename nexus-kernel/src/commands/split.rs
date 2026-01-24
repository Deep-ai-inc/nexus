//! String splitting commands - lines, words, chars, bytes.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs;
use std::path::PathBuf;

// ============================================================================
// lines - split string into lines
// ============================================================================

pub struct LinesCommand;

impl NexusCommand for LinesCommand {
    fn name(&self) -> &'static str {
        "lines"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(lines_value(stdin_value));
        }

        // Read from files
        let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        if files.is_empty() {
            return Ok(Value::Unit);
        }

        let mut all_lines = Vec::new();
        for path in &files {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                ctx.state.cwd.join(path)
            };

            match fs::read_to_string(&resolved) {
                Ok(content) => {
                    all_lines.extend(content.lines().map(|s| Value::String(s.to_string())));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                }
            }
        }

        Ok(Value::List(all_lines))
    }
}

fn lines_value(value: Value) -> Value {
    match value {
        Value::String(s) => {
            Value::List(s.lines().map(|l| Value::String(l.to_string())).collect())
        }
        Value::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            Value::List(s.lines().map(|l| Value::String(l.to_string())).collect())
        }
        // Already a list - pass through
        Value::List(items) => Value::List(items),
        other => {
            let text = other.to_text();
            Value::List(text.lines().map(|l| Value::String(l.to_string())).collect())
        }
    }
}

// ============================================================================
// words - split string into words
// ============================================================================

pub struct WordsCommand;

impl NexusCommand for WordsCommand {
    fn name(&self) -> &'static str {
        "words"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(words_value(stdin_value));
        }

        let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        if files.is_empty() {
            return Ok(Value::Unit);
        }

        let mut all_words = Vec::new();
        for path in &files {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                ctx.state.cwd.join(path)
            };

            match fs::read_to_string(&resolved) {
                Ok(content) => {
                    all_words.extend(
                        content
                            .split_whitespace()
                            .map(|s| Value::String(s.to_string())),
                    );
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                }
            }
        }

        Ok(Value::List(all_words))
    }
}

fn words_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::List(
            s.split_whitespace()
                .map(|w| Value::String(w.to_string()))
                .collect(),
        ),
        Value::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            Value::List(
                s.split_whitespace()
                    .map(|w| Value::String(w.to_string()))
                    .collect(),
            )
        }
        Value::List(items) => {
            // Flatten words from each item
            let mut all_words = Vec::new();
            for item in items {
                let text = item.to_text();
                all_words.extend(
                    text.split_whitespace()
                        .map(|w| Value::String(w.to_string())),
                );
            }
            Value::List(all_words)
        }
        other => {
            let text = other.to_text();
            Value::List(
                text.split_whitespace()
                    .map(|w| Value::String(w.to_string()))
                    .collect(),
            )
        }
    }
}

// ============================================================================
// chars - split string into characters
// ============================================================================

pub struct CharsCommand;

impl NexusCommand for CharsCommand {
    fn name(&self) -> &'static str {
        "chars"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(chars_value(stdin_value));
        }

        // Join args as input
        if !args.is_empty() {
            let text = args.join(" ");
            return Ok(chars_value(Value::String(text)));
        }

        Ok(Value::Unit)
    }
}

fn chars_value(value: Value) -> Value {
    match value {
        Value::String(s) => {
            Value::List(s.chars().map(|c| Value::String(c.to_string())).collect())
        }
        Value::List(items) => {
            // Flatten chars from each item
            let mut all_chars = Vec::new();
            for item in items {
                let text = item.to_text();
                all_chars.extend(text.chars().map(|c| Value::String(c.to_string())));
            }
            Value::List(all_chars)
        }
        other => {
            let text = other.to_text();
            Value::List(text.chars().map(|c| Value::String(c.to_string())).collect())
        }
    }
}

// ============================================================================
// bytes - get byte values
// ============================================================================

pub struct BytesCommand;

impl NexusCommand for BytesCommand {
    fn name(&self) -> &'static str {
        "bytes"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(bytes_value(stdin_value));
        }

        if !args.is_empty() {
            let text = args.join(" ");
            return Ok(bytes_value(Value::String(text)));
        }

        Ok(Value::Unit)
    }
}

fn bytes_value(value: Value) -> Value {
    match value {
        Value::String(s) => {
            Value::List(s.bytes().map(|b| Value::Int(b as i64)).collect())
        }
        Value::Bytes(b) => {
            Value::List(b.into_iter().map(|b| Value::Int(b as i64)).collect())
        }
        other => {
            let text = other.to_text();
            Value::List(text.bytes().map(|b| Value::Int(b as i64)).collect())
        }
    }
}

// ============================================================================
// split - split by delimiter
// ============================================================================

pub struct SplitCommand;

impl NexusCommand for SplitCommand {
    fn name(&self) -> &'static str {
        "split"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let delimiter = args.first().map(|s| s.as_str()).unwrap_or("\n");

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(split_value(stdin_value, delimiter));
        }

        Ok(Value::Unit)
    }
}

fn split_value(value: Value, delimiter: &str) -> Value {
    match value {
        Value::String(s) => Value::List(
            s.split(delimiter)
                .map(|p| Value::String(p.to_string()))
                .collect(),
        ),
        Value::List(items) => {
            let mut result = Vec::new();
            for item in items {
                let text = item.to_text();
                result.extend(text.split(delimiter).map(|p| Value::String(p.to_string())));
            }
            Value::List(result)
        }
        other => {
            let text = other.to_text();
            Value::List(
                text.split(delimiter)
                    .map(|p| Value::String(p.to_string()))
                    .collect(),
            )
        }
    }
}

// ============================================================================
// join - join list with delimiter
// ============================================================================

pub struct JoinCommand;

impl NexusCommand for JoinCommand {
    fn name(&self) -> &'static str {
        "join"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let delimiter = args.first().map(|s| s.as_str()).unwrap_or("\n");

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(join_value(stdin_value, delimiter));
        }

        Ok(Value::Unit)
    }
}

fn join_value(value: Value, delimiter: &str) -> Value {
    match value {
        Value::List(items) => {
            let texts: Vec<String> = items.into_iter().map(|i| i.to_text()).collect();
            Value::String(texts.join(delimiter))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lines() {
        let result = lines_value(Value::String("a\nb\nc".to_string()));
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn test_words() {
        let result = words_value(Value::String("hello world foo".to_string()));
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn test_chars() {
        let result = chars_value(Value::String("abc".to_string()));
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn test_split() {
        let result = split_value(Value::String("a,b,c".to_string()), ",");
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
        }
    }

    #[test]
    fn test_join() {
        let list = Value::List(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]);
        let result = join_value(list, ",");
        assert_eq!(result, Value::String("a,b".to_string()));
    }
}
