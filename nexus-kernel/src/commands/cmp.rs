//! cmp - Compare two files byte by byte.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;

pub struct CmpCommand;

impl NexusCommand for CmpCommand {
    fn name(&self) -> &'static str {
        "cmp"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut silent = false;
        let mut show_all = false;
        let mut arg_start = 0;

        // Parse options
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "-s" | "--silent" | "--quiet" => silent = true,
                "-l" | "--verbose" => show_all = true,
                arg if arg.starts_with('-') && arg != "-" => {
                    anyhow::bail!("cmp: unrecognized option: {}", arg);
                }
                _ => {
                    arg_start = i;
                    break;
                }
            }
        }

        let files = &args[arg_start..];

        if files.len() < 2 {
            anyhow::bail!("cmp: missing operand after '{}'", files.get(0).unwrap_or(&String::new()));
        }

        let file1_path = resolve_path(&files[0], ctx);
        let file2_path = resolve_path(&files[1], ctx);

        // Open both files
        let file1 = File::open(&file1_path)
            .map_err(|e| anyhow::anyhow!("cmp: {}: {}", files[0], e))?;
        let file2 = File::open(&file2_path)
            .map_err(|e| anyhow::anyhow!("cmp: {}: {}", files[1], e))?;

        let mut reader1 = BufReader::new(file1);
        let mut reader2 = BufReader::new(file2);

        let mut byte_num: u64 = 0;
        let mut line_num: u64 = 1;
        let mut differences = Vec::new();

        loop {
            let mut buf1 = [0u8; 1];
            let mut buf2 = [0u8; 1];

            let n1 = reader1.read(&mut buf1)?;
            let n2 = reader2.read(&mut buf2)?;

            byte_num += 1;

            match (n1, n2) {
                (0, 0) => {
                    // Both files ended at same position - they're equal
                    break;
                }
                (0, _) => {
                    // File 1 ended first
                    if !silent {
                        eprintln!("cmp: EOF on {}", files[0]);
                    }
                    return Ok(Value::Record(vec![
                        ("equal".to_string(), Value::Bool(false)),
                        ("reason".to_string(), Value::String(format!("EOF on {}", files[0]))),
                    ]));
                }
                (_, 0) => {
                    // File 2 ended first
                    if !silent {
                        eprintln!("cmp: EOF on {}", files[1]);
                    }
                    return Ok(Value::Record(vec![
                        ("equal".to_string(), Value::Bool(false)),
                        ("reason".to_string(), Value::String(format!("EOF on {}", files[1]))),
                    ]));
                }
                (1, 1) => {
                    if buf1[0] != buf2[0] {
                        if show_all {
                            differences.push(Value::Record(vec![
                                ("byte".to_string(), Value::Int(byte_num as i64)),
                                ("file1".to_string(), Value::Int(buf1[0] as i64)),
                                ("file2".to_string(), Value::Int(buf2[0] as i64)),
                            ]));
                        } else if !silent {
                            eprintln!(
                                "{} {} differ: byte {}, line {}",
                                files[0], files[1], byte_num, line_num
                            );
                            return Ok(Value::Record(vec![
                                ("equal".to_string(), Value::Bool(false)),
                                ("byte".to_string(), Value::Int(byte_num as i64)),
                                ("line".to_string(), Value::Int(line_num as i64)),
                            ]));
                        } else {
                            return Ok(Value::Bool(false));
                        }
                    }

                    if buf1[0] == b'\n' {
                        line_num += 1;
                    }
                }
                _ => unreachable!(),
            }
        }

        if show_all && !differences.is_empty() {
            Ok(Value::Record(vec![
                ("equal".to_string(), Value::Bool(false)),
                ("differences".to_string(), Value::List(differences)),
            ]))
        } else {
            Ok(Value::Bool(true))
        }
    }
}

/// Resolve a file path relative to the current working directory.
fn resolve_path(file: &str, ctx: &CommandContext) -> PathBuf {
    if file == "-" {
        // stdin - not fully supported yet
        PathBuf::from("/dev/stdin")
    } else {
        let path = PathBuf::from(file);
        if path.is_absolute() {
            path
        } else {
            ctx.state.cwd.join(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::{create_test_file, TestContext};
    use tempfile::TempDir;

    #[test]
    fn test_cmp_command_name() {
        let cmd = CmpCommand;
        assert_eq!(cmd.name(), "cmp");
    }

    #[test]
    fn test_cmp_identical_files() {
        let dir = TempDir::new().unwrap();
        let file1 = create_test_file(&dir, "file1.txt", b"hello world");
        let file2 = create_test_file(&dir, "file2.txt", b"hello world");

        let cmd = CmpCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd
            .execute(
                &[
                    file1.to_string_lossy().to_string(),
                    file2.to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_cmp_different_files_silent() {
        let dir = TempDir::new().unwrap();
        let file1 = create_test_file(&dir, "file1.txt", b"hello");
        let file2 = create_test_file(&dir, "file2.txt", b"world");

        let cmd = CmpCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd
            .execute(
                &[
                    "-s".to_string(),
                    file1.to_string_lossy().to_string(),
                    file2.to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_cmp_file1_shorter() {
        let dir = TempDir::new().unwrap();
        let file1 = create_test_file(&dir, "file1.txt", b"hello");
        let file2 = create_test_file(&dir, "file2.txt", b"hello world");

        let cmd = CmpCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd
            .execute(
                &[
                    file1.to_string_lossy().to_string(),
                    file2.to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        // Should return a record with equal=false
        match result {
            Value::Record(fields) => {
                let equal = fields.iter().find(|(k, _)| k == "equal").map(|(_, v)| v);
                assert_eq!(equal, Some(&Value::Bool(false)));
            }
            _ => panic!("Expected Record, got {:?}", result),
        }
    }

    #[test]
    fn test_cmp_missing_operand() {
        let cmd = CmpCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["file1.txt".to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_cmp_verbose_mode() {
        let dir = TempDir::new().unwrap();
        let file1 = create_test_file(&dir, "file1.txt", b"abc");
        let file2 = create_test_file(&dir, "file2.txt", b"aXc");

        let cmd = CmpCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd
            .execute(
                &[
                    "-l".to_string(),
                    file1.to_string_lossy().to_string(),
                    file2.to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        // Should return a record with differences list
        match result {
            Value::Record(fields) => {
                let has_differences = fields.iter().any(|(k, _)| k == "differences");
                assert!(has_differences);
            }
            _ => panic!("Expected Record with differences"),
        }
    }
}
