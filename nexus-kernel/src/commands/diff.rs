//! `diff` — compare files and produce structured diffs.

use super::{CommandContext, NexusCommand};
use nexus_api::{DiffFileInfo, DiffHunk, DiffLine, DiffLineKind, GitChangeType, Value};
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;

pub struct DiffCommand;

impl NexusCommand for DiffCommand {
    fn name(&self) -> &'static str {
        "diff"
    }

    fn description(&self) -> &'static str {
        "Compare two files"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut context_lines: usize = 3;
        let mut files = Vec::new();

        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-u" | "--unified" => {} // default
                "-U" | "--context" => {
                    if let Some(n) = iter.next() {
                        context_lines = n.parse().unwrap_or(3);
                    }
                }
                s if s.starts_with("-U") => {
                    context_lines = s[2..].parse().unwrap_or(3);
                }
                s if !s.starts_with('-') => files.push(s.to_string()),
                _ => {}
            }
        }

        if files.len() != 2 {
            anyhow::bail!("diff: requires exactly two files");
        }

        let path_a = resolve_path(&files[0], &ctx.state.cwd);
        let path_b = resolve_path(&files[1], &ctx.state.cwd);

        let text_a = std::fs::read_to_string(&path_a)
            .map_err(|e| anyhow::anyhow!("diff: {}: {}", path_a.display(), e))?;
        let text_b = std::fs::read_to_string(&path_b)
            .map_err(|e| anyhow::anyhow!("diff: {}: {}", path_b.display(), e))?;

        let diff = TextDiff::from_lines(&text_a, &text_b);

        let mut hunks = Vec::new();
        let mut additions: usize = 0;
        let mut deletions: usize = 0;

        for group in diff.grouped_ops(context_lines) {
            let mut lines = Vec::new();

            // Compute hunk header ranges
            let first_op = group.first().unwrap();
            let last_op = group.last().unwrap();
            let old_start = first_op.old_range().start + 1;
            let old_count = last_op.old_range().end - first_op.old_range().start;
            let new_start = first_op.new_range().start + 1;
            let new_count = last_op.new_range().end - first_op.new_range().start;

            for op in &group {
                for change in diff.iter_changes(op) {
                    let (kind, content) = match change.tag() {
                        ChangeTag::Equal => (
                            DiffLineKind::Context,
                            change.as_str().unwrap_or("").to_string(),
                        ),
                        ChangeTag::Insert => {
                            additions += 1;
                            (
                                DiffLineKind::Addition,
                                change.as_str().unwrap_or("").to_string(),
                            )
                        }
                        ChangeTag::Delete => {
                            deletions += 1;
                            (
                                DiffLineKind::Deletion,
                                change.as_str().unwrap_or("").to_string(),
                            )
                        }
                    };

                    // Strip trailing newline from content (our renderer adds them)
                    let content = content.trim_end_matches('\n').to_string();

                    lines.push(DiffLine {
                        kind,
                        content,
                        old_lineno: change.old_index().map(|i| i + 1),
                        new_lineno: change.new_index().map(|i| i + 1),
                    });
                }
            }

            hunks.push(DiffHunk {
                header: String::new(),
                old_start,
                old_count,
                new_start,
                new_count,
                lines,
            });
        }

        // If files are identical, return a message
        if hunks.is_empty() {
            return Ok(Value::String("Files are identical".to_string()));
        }

        let info = DiffFileInfo {
            file_path: files[1].clone(),
            old_path: Some(files[0].clone()),
            change_type: GitChangeType::Modified,
            hunks,
            additions,
            deletions,
        };

        Ok(Value::diff_file(info))
    }
}

fn resolve_path(file: &str, cwd: &PathBuf) -> PathBuf {
    let p = PathBuf::from(file);
    if p.is_absolute() {
        p
    } else {
        cwd.join(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use tempfile::TempDir;

    fn setup_diff_files(content_a: &str, content_b: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), content_a).unwrap();
        std::fs::write(dir.path().join("b.txt"), content_b).unwrap();
        dir
    }

    #[test]
    fn test_diff_identical() {
        let dir = setup_diff_files("hello\nworld\n", "hello\nworld\n");
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DiffCommand;
        let result = cmd
            .execute(
                &["a.txt".to_string(), "b.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::String(s) => assert_eq!(s, "Files are identical"),
            _ => panic!("Expected String for identical files"),
        }
    }

    #[test]
    fn test_diff_changed() {
        let dir = setup_diff_files("line1\nline2\nline3\n", "line1\nmodified\nline3\n");
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DiffCommand;
        let result = cmd
            .execute(
                &["a.txt".to_string(), "b.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::DiffFile(diff)) => {
                assert_eq!(diff.additions, 1);
                assert_eq!(diff.deletions, 1);
                assert!(!diff.hunks.is_empty());
            }
            _ => panic!("Expected DiffFile"),
        }
    }

    #[test]
    fn test_diff_added_lines() {
        let dir = setup_diff_files("a\nb\n", "a\nb\nc\nd\n");
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DiffCommand;
        let result = cmd
            .execute(
                &["a.txt".to_string(), "b.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::DiffFile(diff)) => {
                assert_eq!(diff.additions, 2);
                assert_eq!(diff.deletions, 0);
            }
            _ => panic!("Expected DiffFile"),
        }
    }

    #[test]
    fn test_diff_missing_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DiffCommand;
        let result = cmd.execute(
            &["a.txt".to_string(), "nonexistent.txt".to_string()],
            &mut test_ctx.ctx(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_diff_wrong_arg_count() {
        let mut test_ctx = TestContext::new_default();
        let cmd = DiffCommand;
        assert!(cmd
            .execute(&["only_one".to_string()], &mut test_ctx.ctx())
            .is_err());
    }
}
