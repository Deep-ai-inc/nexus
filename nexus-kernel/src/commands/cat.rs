//! The `cat` command - concatenate and display files.

use super::{CommandContext, NexusCommand};
use nexus_api::{detect_mime_type, mime_from_extension, MediaMetadata, Value};
use std::fs;
use std::path::PathBuf;

pub struct CatCommand;

struct CatOptions {
    number_lines: bool,
    number_nonblank: bool,
    show_ends: bool,
    squeeze_blank: bool,
    files: Vec<PathBuf>,
}

impl CatOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = CatOptions {
            number_lines: false,
            number_nonblank: false,
            show_ends: false,
            squeeze_blank: false,
            files: Vec::new(),
        };

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
                for c in arg[1..].chars() {
                    match c {
                        'n' => opts.number_lines = true,
                        'b' => opts.number_nonblank = true,
                        'E' => opts.show_ends = true,
                        's' => opts.squeeze_blank = true,
                        _ => {}
                    }
                }
            } else if arg.starts_with("--") {
                match arg.as_str() {
                    "--number" => opts.number_lines = true,
                    "--number-nonblank" => opts.number_nonblank = true,
                    "--show-ends" => opts.show_ends = true,
                    "--squeeze-blank" => opts.squeeze_blank = true,
                    _ => {}
                }
            } else if !arg.starts_with('-') || arg == "-" {
                opts.files.push(PathBuf::from(arg));
            }
        }

        opts
    }
}

impl NexusCommand for CatCommand {
    fn name(&self) -> &'static str {
        "cat"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = CatOptions::parse(args);

        // If we have piped input and no files, pass through
        if let Some(stdin_value) = ctx.stdin.take() {
            if opts.files.is_empty() {
                return Ok(process_value(stdin_value, &opts));
            }
        }

        // Read from files
        if opts.files.is_empty() {
            return Ok(Value::Unit);
        }

        // Single file: can return Media for binary content
        if opts.files.len() == 1 {
            let path = &opts.files[0];
            if path.to_string_lossy() == "-" {
                return Ok(Value::Unit);
            }

            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                ctx.state.cwd.join(path)
            };

            // Read raw bytes first
            let data = fs::read(&resolved)
                .map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;

            // Detect content type
            let ext = resolved
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let mime_from_ext = mime_from_extension(ext);
            let mime_from_magic = detect_mime_type(&data);

            // Prefer magic detection, fall back to extension
            let content_type = if mime_from_magic != "application/octet-stream" {
                mime_from_magic
            } else if mime_from_ext != "application/octet-stream" {
                mime_from_ext
            } else {
                mime_from_magic
            };

            // For text files, use string processing with options
            if content_type.starts_with("text/") || content_type == "application/json" {
                let text = String::from_utf8_lossy(&data).to_string();
                return Ok(process_string(text, &opts));
            }

            // For binary/media files, return as Media value
            let metadata = MediaMetadata::new()
                .with_filename(path.file_name().unwrap_or_default().to_string_lossy())
                .with_size(data.len() as u64);

            return Ok(Value::media_with_metadata(data, content_type, metadata));
        }

        // Multiple files: concatenate as text
        let mut all_content = String::new();

        for path in &opts.files {
            if path.to_string_lossy() == "-" {
                continue;
            }

            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                ctx.state.cwd.join(path)
            };

            match fs::read_to_string(&resolved) {
                Ok(content) => {
                    if !all_content.is_empty() {
                        all_content.push('\n');
                    }
                    all_content.push_str(&content);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                }
            }
        }

        Ok(process_string(all_content, &opts))
    }
}

fn process_value(value: Value, opts: &CatOptions) -> Value {
    match value {
        Value::String(s) => process_string(s, opts),
        Value::Bytes(bytes) => {
            let s = String::from_utf8_lossy(&bytes).to_string();
            process_string(s, opts)
        }
        // For other types, just pass through
        other => other,
    }
}

fn process_string(s: String, opts: &CatOptions) -> Value {
    if !opts.number_lines && !opts.number_nonblank && !opts.show_ends && !opts.squeeze_blank {
        return Value::String(s);
    }

    let mut lines: Vec<String> = Vec::new();
    let mut line_num = 1;
    let mut prev_blank = false;

    for line in s.lines() {
        let is_blank = line.trim().is_empty();

        // Squeeze blank lines
        if opts.squeeze_blank && is_blank && prev_blank {
            continue;
        }
        prev_blank = is_blank;

        let mut output_line = String::new();

        // Add line number
        if opts.number_lines {
            output_line.push_str(&format!("{:6}\t", line_num));
            line_num += 1;
        } else if opts.number_nonblank && !is_blank {
            output_line.push_str(&format!("{:6}\t", line_num));
            line_num += 1;
        }

        output_line.push_str(line);

        // Show line endings
        if opts.show_ends {
            output_line.push('$');
        }

        lines.push(output_line);
    }

    Value::String(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cat_passthrough() {
        let opts = CatOptions {
            number_lines: false,
            number_nonblank: false,
            show_ends: false,
            squeeze_blank: false,
            files: vec![],
        };
        let result = process_string("hello\nworld".to_string(), &opts);
        assert_eq!(result, Value::String("hello\nworld".to_string()));
    }

    #[test]
    fn test_cat_number_lines() {
        let opts = CatOptions {
            number_lines: true,
            number_nonblank: false,
            show_ends: false,
            squeeze_blank: false,
            files: vec![],
        };
        let result = process_string("a\nb\nc".to_string(), &opts);
        if let Value::String(s) = result {
            assert!(s.contains("1\ta"));
            assert!(s.contains("2\tb"));
        }
    }

    #[test]
    fn test_cat_squeeze_blank() {
        let opts = CatOptions {
            number_lines: false,
            number_nonblank: false,
            show_ends: false,
            squeeze_blank: true,
            files: vec![],
        };
        let result = process_string("a\n\n\nb".to_string(), &opts);
        if let Value::String(s) = result {
            assert_eq!(s.matches('\n').count(), 2); // Only one blank line kept
        }
    }
}
