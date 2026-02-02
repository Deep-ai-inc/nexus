//! Native git commands with structured output.
//!
//! These commands use libgit2 directly instead of spawning the git binary,
//! returning structured data that the UI can render richly.
//!
//! The type system provides:
//! - `GitStatus`: Rich status with branch info, ahead/behind, staged/unstaged files
//! - `GitCommit`: Commit info with hash, author, message, stats
//!
//! Usage: `git <subcommand> [args]`
//! Supported subcommands: status, log, branch, diff, add, commit, remote, stash

use super::{CommandContext, NexusCommand};
use git2::{Repository, StatusOptions, DiffOptions};
use nexus_api::{
    DiffFileInfo, DiffHunk, DiffLine, DiffLineKind, GitChangeType, GitCommitInfo, GitFileStatus,
    GitStatusInfo, InteractiveRequest, Value, ViewerKind,
};
use std::path::Path;

/// Main git command dispatcher - handles `git <subcommand>` syntax.
pub struct GitCommand;

impl NexusCommand for GitCommand {
    fn name(&self) -> &'static str {
        "git"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let subcommand = args.first().map(|s| s.as_str()).unwrap_or("status");
        let subargs: Vec<String> = args.iter().skip(1).cloned().collect();

        match subcommand {
            "status" => GitStatusCommand.execute(&subargs, ctx),
            "log" => GitLogCommand.execute(&subargs, ctx),
            "branch" => GitBranchCommand.execute(&subargs, ctx),
            "diff" => GitDiffCommand.execute(&subargs, ctx),
            "add" => GitAddCommand.execute(&subargs, ctx),
            "commit" => GitCommitCommand.execute(&subargs, ctx),
            "remote" => GitRemoteCommand.execute(&subargs, ctx),
            "stash" => GitStashCommand.execute(&subargs, ctx),
            _ => anyhow::bail!("git: '{}' is not a git command", subcommand),
        }
    }
}

/// Find the git repository for the current directory.
fn find_repo(cwd: &Path) -> anyhow::Result<Repository> {
    Repository::discover(cwd).map_err(|e| anyhow::anyhow!("not a git repository: {}", e))
}

/// git status - show working tree status
///
/// Returns a typed `GitStatusInfo` with:
/// - Branch name and upstream tracking info
/// - Staged files with change type
/// - Unstaged modifications
/// - Untracked files
///
/// The GUI can render this with branch badges, status icons, and staging checkboxes.
pub struct GitStatusCommand;

impl NexusCommand for GitStatusCommand {
    fn name(&self) -> &'static str {
        "git-status"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;

        let head = repo.head().ok();
        let branch = head
            .as_ref()
            .and_then(|h| h.shorthand())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "HEAD (detached)".to_string());

        // Get upstream tracking info
        let (upstream, ahead, behind) = get_upstream_info(&repo, &branch);

        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true);

        let statuses = repo.statuses(Some(&mut opts))?;

        let mut staged: Vec<GitFileStatus> = Vec::new();
        let mut unstaged: Vec<GitFileStatus> = Vec::new();
        let mut untracked: Vec<String> = Vec::new();
        let mut has_conflicts = false;

        for entry in statuses.iter() {
            let path = entry.path().unwrap_or("").to_string();
            let status = entry.status();

            // Check for conflicts
            if status.is_conflicted() {
                has_conflicts = true;
                staged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Unmerged,
                    orig_path: None,
                });
                continue;
            }

            // Staged changes (index)
            if status.is_index_new() {
                staged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Added,
                    orig_path: None,
                });
            } else if status.is_index_modified() {
                staged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Modified,
                    orig_path: None,
                });
            } else if status.is_index_deleted() {
                staged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Deleted,
                    orig_path: None,
                });
            } else if status.is_index_renamed() {
                staged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Renamed,
                    orig_path: None,
                });
            }

            // Unstaged changes (working tree)
            if status.is_wt_modified() {
                unstaged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Modified,
                    orig_path: None,
                });
            } else if status.is_wt_deleted() {
                unstaged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Deleted,
                    orig_path: None,
                });
            } else if status.is_wt_renamed() {
                unstaged.push(GitFileStatus {
                    path: path.clone(),
                    status: GitChangeType::Renamed,
                    orig_path: None,
                });
            }

            // Untracked files
            if status.is_wt_new() {
                untracked.push(path);
            }
        }

        Ok(Value::GitStatus(Box::new(GitStatusInfo {
            branch,
            upstream,
            ahead,
            behind,
            staged,
            unstaged,
            untracked,
            has_conflicts,
        })))
    }
}

/// Get upstream tracking info for a branch.
fn get_upstream_info(repo: &Repository, branch_name: &str) -> (Option<String>, u32, u32) {
    let branch = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (None, 0, 0),
    };

    let upstream = match branch.upstream() {
        Ok(u) => u,
        Err(_) => return (None, 0, 0),
    };

    let upstream_name = upstream
        .name()
        .ok()
        .flatten()
        .map(|s| s.to_string());

    // Get ahead/behind counts
    let local_oid = match branch.get().peel_to_commit() {
        Ok(c) => c.id(),
        Err(_) => return (upstream_name, 0, 0),
    };

    let upstream_oid = match upstream.get().peel_to_commit() {
        Ok(c) => c.id(),
        Err(_) => return (upstream_name, 0, 0),
    };

    let (ahead, behind) = match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((a, b)) => (a as u32, b as u32),
        Err(_) => (0, 0),
    };

    (upstream_name, ahead, behind)
}

/// git log - show commit history
///
/// Returns a list of typed `GitCommitInfo` values with:
/// - Hash (full and short)
/// - Author name and email
/// - Commit date
/// - Message (subject and body)
///
/// The GUI can render clickable hashes, author avatars, and relative timestamps.
pub struct GitLogCommand;

impl NexusCommand for GitLogCommand {
    fn name(&self) -> &'static str {
        "git-log"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;

        // Parse arguments
        let mut max_count: usize = 10;
        let mut table_output = false;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-n" | "--max-count" => {
                    if i + 1 < args.len() {
                        max_count = args[i + 1].parse().unwrap_or(10);
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                arg if arg.starts_with("-n") => {
                    max_count = arg[2..].parse().unwrap_or(10);
                    i += 1;
                }
                "-t" | "--table" | "--oneline" => {
                    table_output = true;
                    i += 1;
                }
                _ => i += 1,
            }
        }

        let mut revwalk = repo.revwalk()?;
        revwalk.push_head()?;

        let mut commits: Vec<GitCommitInfo> = Vec::new();

        for (count, oid) in revwalk.enumerate() {
            if count >= max_count {
                break;
            }

            let oid = oid?;
            let commit = repo.find_commit(oid)?;

            let hash = oid.to_string();
            let short_hash = hash[..7.min(hash.len())].to_string();

            let author = commit.author();
            let author_name = author.name().unwrap_or("Unknown").to_string();
            let author_email = author.email().unwrap_or("").to_string();

            let time = commit.time();
            let timestamp = time.seconds() as u64;

            let message = commit.message().unwrap_or("").to_string();
            let mut lines = message.lines();
            let subject = lines.next().unwrap_or("").to_string();
            let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();

            commits.push(GitCommitInfo {
                hash,
                short_hash,
                author: author_name,
                author_email,
                date: timestamp,
                message: subject,
                body: if body.is_empty() { None } else { Some(body) },
                files_changed: None,
                insertions: None,
                deletions: None,
            });
        }

        if table_output {
            // Return as table for compact, sortable view
            use nexus_api::{DisplayFormat, TableColumn};

            let columns = vec![
                TableColumn::new("hash"),
                TableColumn::new("author"),
                TableColumn::with_format("date", DisplayFormat::RelativeTime),
                TableColumn::new("message"),
            ];

            let rows: Vec<Vec<Value>> = commits
                .into_iter()
                .map(|c| {
                    vec![
                        Value::String(c.short_hash),
                        Value::String(c.author),
                        Value::Int(c.date as i64),
                        Value::String(c.message),
                    ]
                })
                .collect();

            Ok(Value::Table { columns, rows })
        } else {
            // Return as list of GitCommit values (rich type for UI rendering)
            Ok(Value::List(commits.into_iter().map(|c| Value::GitCommit(Box::new(c))).collect()))
        }
    }
}

/// git branch - list branches
pub struct GitBranchCommand;

impl NexusCommand for GitBranchCommand {
    fn name(&self) -> &'static str {
        "git-branch"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;

        let show_all = args.iter().any(|a| a == "-a" || a == "--all");
        let show_remote = args.iter().any(|a| a == "-r" || a == "--remotes");

        let head = repo.head().ok();
        let current_branch = head
            .as_ref()
            .and_then(|h| h.shorthand())
            .map(|s| s.to_string());

        let branches = repo.branches(None)?;
        let mut rows: Vec<Vec<Value>> = Vec::new();

        for branch_result in branches {
            let (branch, branch_type) = branch_result?;
            let name = branch.name()?.unwrap_or("").to_string();

            let is_remote = branch_type == git2::BranchType::Remote;

            // Filter based on flags
            if is_remote && !show_all && !show_remote {
                continue;
            }
            if !is_remote && show_remote {
                continue;
            }

            let is_current = current_branch.as_ref().map(|c| c == &name).unwrap_or(false);

            // Get the commit this branch points to
            let commit = branch.get().peel_to_commit().ok();
            let last_commit = commit
                .as_ref()
                .and_then(|c| c.message())
                .map(|m| m.lines().next().unwrap_or("").to_string())
                .unwrap_or_default();

            rows.push(vec![
                Value::Bool(is_current),
                Value::String(name),
                Value::String(if is_remote { "remote".to_string() } else { "local".to_string() }),
                Value::String(last_commit),
            ]);
        }

        Ok(Value::table(
            vec!["current", "name", "type", "last_commit"],
            rows,
        ))
    }
}

/// git diff - show changes with structured diff output.
///
/// Returns `Value::List(Vec<Value::DiffFile>)` with full hunk-level detail.
/// `to_text()` produces valid unified diff format for pipe compatibility.
pub struct GitDiffCommand;

impl NexusCommand for GitDiffCommand {
    fn name(&self) -> &'static str {
        "git-diff"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;

        let staged = args.iter().any(|a| a == "--staged" || a == "--cached");

        let mut diff_opts = DiffOptions::new();

        let diff = if staged {
            let head = repo.head()?.peel_to_tree()?;
            repo.diff_tree_to_index(Some(&head), None, Some(&mut diff_opts))?
        } else {
            repo.diff_index_to_workdir(None, Some(&mut diff_opts))?
        };

        // Build structured diff files using the Patch API (no RefCell needed)
        let num_deltas = diff.deltas().count();
        let mut values: Vec<Value> = Vec::new();

        for idx in 0..num_deltas {
            let patch = git2::Patch::from_diff(&diff, idx)?;
            let Some(patch) = patch else { continue };

            let delta = patch.delta();
            let old_path = delta.old_file().path().map(|p| p.to_string_lossy().to_string());
            let new_path = delta.new_file().path().map(|p| p.to_string_lossy().to_string());
            let file_path = new_path.clone().or_else(|| old_path.clone()).unwrap_or_default();

            let change_type = match delta.status() {
                git2::Delta::Added => GitChangeType::Added,
                git2::Delta::Deleted => GitChangeType::Deleted,
                git2::Delta::Modified => GitChangeType::Modified,
                git2::Delta::Renamed => GitChangeType::Renamed,
                git2::Delta::Copied => GitChangeType::Copied,
                _ => GitChangeType::Modified,
            };

            let old_path_opt = if delta.status() == git2::Delta::Renamed {
                old_path
            } else {
                None
            };

            let mut hunks = Vec::new();
            let mut additions = 0usize;
            let mut deletions = 0usize;

            for hunk_idx in 0..patch.num_hunks() {
                let (hunk, _) = patch.hunk(hunk_idx)?;
                let header = std::str::from_utf8(hunk.header())
                    .unwrap_or("")
                    .trim()
                    .to_string();

                let mut lines = Vec::new();
                let num_lines = patch.num_lines_in_hunk(hunk_idx)?;
                for line_idx in 0..num_lines {
                    let line = patch.line_in_hunk(hunk_idx, line_idx)?;
                    let kind = match line.origin() {
                        '+' => { additions += 1; DiffLineKind::Addition }
                        '-' => { deletions += 1; DiffLineKind::Deletion }
                        _ => DiffLineKind::Context,
                    };
                    let content = std::str::from_utf8(line.content())
                        .unwrap_or("")
                        .trim_end_matches('\n')
                        .to_string();
                    lines.push(DiffLine {
                        kind,
                        content,
                        old_lineno: line.old_lineno().map(|n| n as usize),
                        new_lineno: line.new_lineno().map(|n| n as usize),
                    });
                }

                hunks.push(DiffHunk {
                    header,
                    old_start: hunk.old_start() as usize,
                    old_count: hunk.old_lines() as usize,
                    new_start: hunk.new_start() as usize,
                    new_count: hunk.new_lines() as usize,
                    lines,
                });
            }

            values.push(Value::diff_file(DiffFileInfo {
                file_path,
                old_path: old_path_opt,
                change_type,
                hunks,
                additions,
                deletions,
            }));
        }

        if values.is_empty() {
            Ok(Value::Unit)
        } else {
            Ok(Value::interactive(InteractiveRequest {
                viewer: ViewerKind::DiffViewer,
                content: Value::List(values),
            }))
        }
    }
}

/// git add - stage files
pub struct GitAddCommand;

impl NexusCommand for GitAddCommand {
    fn name(&self) -> &'static str {
        "git-add"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;
        let mut index = repo.index()?;

        let mut added: Vec<Value> = Vec::new();

        for arg in args {
            if arg.starts_with('-') {
                continue; // Skip flags for now
            }

            if arg == "." {
                // Add all files
                index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
                added.push(Value::String(".".to_string()));
            } else {
                index.add_path(Path::new(arg))?;
                added.push(Value::String(arg.clone()));
            }
        }

        index.write()?;

        Ok(Value::Record(vec![
            ("added".to_string(), Value::List(added)),
        ]))
    }
}

/// git commit - record changes
pub struct GitCommitCommand;

impl NexusCommand for GitCommitCommand {
    fn name(&self) -> &'static str {
        "git-commit"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;

        // Parse -m message
        let mut message: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-m" && i + 1 < args.len() {
                message = Some(args[i + 1].clone());
                i += 2;
            } else {
                i += 1;
            }
        }

        let message = message.ok_or_else(|| anyhow::anyhow!("commit message required (-m)"))?;

        let sig = repo.signature()?;
        let mut index = repo.index()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;

        let parent = repo.head()?.peel_to_commit()?;

        let commit_id = repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &message,
            &tree,
            &[&parent],
        )?;

        let short_hash = &commit_id.to_string()[..7];

        Ok(Value::Record(vec![
            ("hash".to_string(), Value::String(short_hash.to_string())),
            ("message".to_string(), Value::String(message)),
            ("author".to_string(), Value::String(sig.name().unwrap_or("").to_string())),
        ]))
    }
}

/// git remote - manage remotes
pub struct GitRemoteCommand;

impl NexusCommand for GitRemoteCommand {
    fn name(&self) -> &'static str {
        "git-remote"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let repo = find_repo(&ctx.state.cwd)?;

        let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");

        let remotes = repo.remotes()?;
        let mut rows: Vec<Vec<Value>> = Vec::new();

        for name in remotes.iter().flatten() {
            let remote = repo.find_remote(name)?;
            let url = remote.url().unwrap_or("").to_string();
            let push_url = remote.pushurl().unwrap_or(remote.url().unwrap_or("")).to_string();

            if verbose {
                rows.push(vec![
                    Value::String(name.to_string()),
                    Value::String(url),
                    Value::String(push_url),
                ]);
            } else {
                rows.push(vec![
                    Value::String(name.to_string()),
                ]);
            }
        }

        if verbose {
            Ok(Value::table(
                vec!["name", "fetch", "push"],
                rows,
            ))
        } else {
            Ok(Value::List(
                rows.into_iter()
                    .filter_map(|r| r.into_iter().next())
                    .collect(),
            ))
        }
    }
}

/// git stash - stash changes
pub struct GitStashCommand;

impl NexusCommand for GitStashCommand {
    fn name(&self) -> &'static str {
        "git-stash"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut repo = find_repo(&ctx.state.cwd)?;

        let subcommand = args.first().map(|s| s.as_str()).unwrap_or("push");

        match subcommand {
            "list" => {
                let mut stashes: Vec<Value> = Vec::new();
                repo.stash_foreach(|index, message, _oid| {
                    stashes.push(Value::Record(vec![
                        ("index".to_string(), Value::Int(index as i64)),
                        ("message".to_string(), Value::String(message.to_string())),
                    ]));
                    true
                })?;
                Ok(Value::List(stashes))
            }
            "push" | "save" => {
                let sig = repo.signature()?;
                let message = args.get(1).cloned();

                let stash_id = repo.stash_save(
                    &sig,
                    message.as_deref().unwrap_or("WIP"),
                    None,
                )?;

                Ok(Value::Record(vec![
                    ("stash".to_string(), Value::String(stash_id.to_string()[..7].to_string())),
                    ("message".to_string(), Value::String(message.unwrap_or_else(|| "WIP".to_string()))),
                ]))
            }
            _ => anyhow::bail!("git stash: unknown subcommand '{}'", subcommand),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Configure user for commits
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test User").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
        }

        // Create initial commit
        {
            let sig = repo.signature().unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[]).unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn test_git_status_clean() {
        let (dir, _repo) = setup_test_repo();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = GitStatusCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::GitStatus(status) => {
                assert!(!status.branch.is_empty());
                assert!(status.staged.is_empty());
                assert!(status.unstaged.is_empty());
            }
            _ => panic!("Expected GitStatus"),
        }
    }

    #[test]
    fn test_git_status_with_changes() {
        let (dir, _repo) = setup_test_repo();

        // Create an untracked file
        fs::write(dir.path().join("new_file.txt"), "content").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = GitStatusCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::GitStatus(status) => {
                assert!(!status.untracked.is_empty());
                assert!(status.untracked.iter().any(|f| f.contains("new_file.txt")));
            }
            _ => panic!("Expected GitStatus"),
        }
    }

    #[test]
    fn test_git_log() {
        let (dir, _repo) = setup_test_repo();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = GitLogCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::List(commits) => {
                assert!(!commits.is_empty());
                // First commit should be a GitCommit
                match &commits[0] {
                    Value::GitCommit(c) => {
                        assert!(!c.hash.is_empty());
                        assert!(!c.author.is_empty());
                    }
                    _ => panic!("Expected GitCommit in list"),
                }
            }
            _ => panic!("Expected List of GitCommit"),
        }
    }

    #[test]
    fn test_git_branch() {
        let (dir, _repo) = setup_test_repo();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = GitBranchCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert!(columns.iter().any(|c| c.name == "name"));
                assert!(columns.iter().any(|c| c.name == "current"));
                // Should have at least main/master branch
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_git_diff_clean() {
        let (dir, _repo) = setup_test_repo();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = GitDiffCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        // Clean repo should return Unit (no changes)
        assert_eq!(result, Value::Unit, "Expected Unit for clean repo diff");
    }

    #[test]
    fn test_git_diff_with_changes() {
        let (dir, repo) = setup_test_repo();

        // Create a tracked file, commit it, then modify it
        fs::write(dir.path().join("tracked.txt"), "original content").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("tracked.txt")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            let sig = repo.signature().unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Add tracked file", &tree, &[&head]).unwrap();
        }

        // Modify the tracked file (unstaged change)
        fs::write(dir.path().join("tracked.txt"), "modified content").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = GitDiffCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        // Now returns Interactive(DiffViewer, List)
        match result.as_domain() {
            Some(nexus_api::DomainValue::Interactive(req)) => {
                assert_eq!(req.viewer, nexus_api::ViewerKind::DiffViewer);
                match &req.content {
                    Value::List(files) => {
                        assert!(!files.is_empty(), "Expected at least one diff file");
                        match files[0].as_domain() {
                            Some(nexus_api::DomainValue::DiffFile(diff)) => {
                                assert!(diff.file_path.contains("tracked.txt"));
                                assert!(diff.additions > 0 || diff.deletions > 0);
                                assert!(!diff.hunks.is_empty());
                                assert!(!diff.hunks[0].lines.is_empty());
                            }
                            _ => panic!("Expected DiffFile in list"),
                        }
                    }
                    _ => panic!("Expected List content"),
                }
            }
            _ => panic!("Expected Interactive DiffViewer"),
        }
    }

    #[test]
    fn test_git_diff_staged() {
        let (dir, repo) = setup_test_repo();

        // Create and stage a new file
        fs::write(dir.path().join("staged.txt"), "staged content").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("staged.txt")).unwrap();
            index.write().unwrap();
        }

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = GitDiffCommand;
        let result = cmd
            .execute(&["--staged".to_string()], &mut test_ctx.ctx())
            .unwrap();

        // Now returns Interactive(DiffViewer, List)
        match result.as_domain() {
            Some(nexus_api::DomainValue::Interactive(req)) => {
                assert_eq!(req.viewer, nexus_api::ViewerKind::DiffViewer);
                match &req.content {
                    Value::List(files) => {
                        assert!(!files.is_empty(), "Expected staged diff");
                        match files[0].as_domain() {
                            Some(nexus_api::DomainValue::DiffFile(diff)) => {
                                assert!(diff.file_path.contains("staged.txt"));
                            }
                            _ => panic!("Expected DiffFile"),
                        }
                    }
                    _ => panic!("Expected List content"),
                }
            }
            _ => panic!("Expected Interactive DiffViewer"),
        }
    }

    #[test]
    fn test_git_remote() {
        let (dir, _repo) = setup_test_repo();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = GitRemoteCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        // New repo has no remotes
        match result {
            Value::List(remotes) => {
                assert!(remotes.is_empty());
            }
            _ => panic!("Expected List"),
        }
    }
}
