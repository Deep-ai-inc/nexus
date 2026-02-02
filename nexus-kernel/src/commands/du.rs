//! The `du` command - estimate file space usage.

use super::{CommandContext, NexusCommand};
use nexus_api::{DisplayFormat, FileType, TableColumn, TreeInfo, TreeNodeFlat, Value};
use std::fs;
use std::path::PathBuf;

pub struct DuCommand;

struct DuOptions {
    /// Show all files, not just directories
    all: bool,
    /// Print human-readable sizes
    human_readable: bool,
    /// Display only a total for each argument
    summarize: bool,
    /// Produce a grand total
    total: bool,
    /// Max depth to descend
    max_depth: Option<usize>,
    /// Output as tree structure
    tree: bool,
}

impl DuOptions {
    fn parse(args: &[String]) -> (Self, Vec<PathBuf>) {
        let mut opts = DuOptions {
            all: false,
            human_readable: false,
            summarize: false,
            total: false,
            max_depth: None,
            tree: false,
        };

        let mut paths = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];

            if arg == "-a" || arg == "--all" {
                opts.all = true;
            } else if arg == "-h" || arg == "--human-readable" {
                opts.human_readable = true;
            } else if arg == "-s" || arg == "--summarize" {
                opts.summarize = true;
            } else if arg == "-c" || arg == "--total" {
                opts.total = true;
            } else if arg == "-d" || arg == "--max-depth" {
                if i + 1 < args.len() {
                    opts.max_depth = args[i + 1].parse().ok();
                    i += 1;
                }
            } else if arg.starts_with("--max-depth=") {
                opts.max_depth = arg.strip_prefix("--max-depth=").and_then(|s| s.parse().ok());
            } else if arg.starts_with("-d") {
                opts.max_depth = arg.strip_prefix("-d").and_then(|s| s.parse().ok());
            } else if arg == "--tree" || arg == "-t" {
                opts.tree = true;
            } else if !arg.starts_with('-') {
                paths.push(PathBuf::from(arg));
            }

            i += 1;
        }

        if paths.is_empty() {
            paths.push(PathBuf::from("."));
        }

        (opts, paths)
    }
}

impl NexusCommand for DuCommand {
    fn name(&self) -> &'static str {
        "du"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let (opts, paths) = DuOptions::parse(args);

        // Tree output mode
        if opts.tree {
            let mut all_nodes: Vec<TreeNodeFlat> = Vec::new();
            let mut id_counter = 0;

            for path in &paths {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ctx.state.cwd.join(path)
                };
                build_du_tree(&resolved, None, 0, opts.max_depth.unwrap_or(usize::MAX), &mut all_nodes, &mut id_counter);
            }

            return Ok(Value::Tree(Box::new(TreeInfo {
                root: 0,
                nodes: all_nodes,
            })));
        }

        let columns = vec![
            if opts.human_readable {
                TableColumn::with_format("size", DisplayFormat::HumanBytes)
            } else {
                TableColumn::new("size")
            },
            TableColumn::new("path"),
        ];

        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut grand_total: u64 = 0;

        for path in paths {
            let resolved = if path.is_absolute() {
                path
            } else {
                ctx.state.cwd.join(path)
            };

            let (size, entries) = calculate_du(&resolved, &opts, 0)?;
            grand_total += size;

            if opts.summarize {
                // Only show the total for this path
                rows.push(vec![
                    Value::Int(size as i64),
                    Value::String(resolved.to_string_lossy().to_string()),
                ]);
            } else {
                // Show all entries
                rows.extend(entries);
            }
        }

        if opts.total && (rows.len() > 1 || opts.summarize) {
            rows.push(vec![
                Value::Int(grand_total as i64),
                Value::String("total".to_string()),
            ]);
        }

        Ok(Value::Table { columns, rows })
    }
}

/// Calculate disk usage for a path, returning (total_size, list_of_entries)
fn calculate_du(
    path: &PathBuf,
    opts: &DuOptions,
    depth: usize,
) -> anyhow::Result<(u64, Vec<Vec<Value>>)> {
    let mut entries = Vec::new();
    let mut total_size: u64 = 0;

    if path.is_file() {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if opts.all || depth == 0 {
            entries.push(vec![
                Value::Int(size as i64),
                Value::String(path.to_string_lossy().to_string()),
            ]);
        }
        return Ok((size, entries));
    }

    if !path.is_dir() {
        return Ok((0, entries));
    }

    // Check max depth
    if let Some(max) = opts.max_depth {
        if depth > max {
            return Ok((0, entries));
        }
    }

    // Process directory contents
    if let Ok(dir_entries) = fs::read_dir(path) {
        for entry in dir_entries.flatten() {
            let entry_path = entry.path();

            if entry_path.is_dir() {
                let (subdir_size, subdir_entries) = calculate_du(&entry_path, opts, depth + 1)?;
                total_size += subdir_size;
                entries.extend(subdir_entries);
            } else {
                let size = fs::metadata(&entry_path).map(|m| m.len()).unwrap_or(0);
                total_size += size;

                if opts.all {
                    // Check depth for files too
                    let show = match opts.max_depth {
                        Some(max) => depth < max,
                        None => true,
                    };
                    if show {
                        entries.push(vec![
                            Value::Int(size as i64),
                            Value::String(entry_path.to_string_lossy().to_string()),
                        ]);
                    }
                }
            }
        }
    }

    // Add this directory's entry (after children so it appears at the end like du)
    let show_dir = match opts.max_depth {
        Some(max) => depth <= max,
        None => true,
    };
    if show_dir {
        entries.push(vec![
            Value::Int(total_size as i64),
            Value::String(path.to_string_lossy().to_string()),
        ]);
    }

    Ok((total_size, entries))
}

/// Build a flat tree arena with size annotations for `du --tree`.
fn build_du_tree(
    path: &PathBuf,
    parent: Option<usize>,
    depth: usize,
    max_depth: usize,
    nodes: &mut Vec<TreeNodeFlat>,
    id_counter: &mut usize,
) -> u64 {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let metadata = fs::metadata(path).ok();
    let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

    let node_id = *id_counter;
    *id_counter += 1;

    let child_count = if is_dir && depth < max_depth {
        fs::read_dir(path).map(|e| e.count()).unwrap_or(0)
    } else {
        0
    };

    // Insert node (size will be updated after children are processed for dirs)
    let node_idx = nodes.len();
    nodes.push(TreeNodeFlat {
        id: node_id,
        parent,
        name,
        path: path.clone(),
        node_type: if is_dir { FileType::Directory } else { FileType::File },
        size: file_size,
        depth,
        child_count,
    });

    if is_dir && depth < max_depth {
        let mut total: u64 = 0;
        if let Ok(entries) = fs::read_dir(path) {
            let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            children.sort();
            for child in children {
                total += build_du_tree(&child, Some(node_id), depth + 1, max_depth, nodes, id_counter);
            }
        }
        nodes[node_idx].size = total;
        total
    } else {
        file_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create some files with known sizes
        let mut f1 = File::create(dir.path().join("file1.txt")).unwrap();
        f1.write_all(b"hello").unwrap(); // 5 bytes

        let mut f2 = File::create(dir.path().join("file2.txt")).unwrap();
        f2.write_all(b"world!").unwrap(); // 6 bytes

        // Create a subdirectory with a file
        fs::create_dir(dir.path().join("subdir")).unwrap();
        let mut f3 = File::create(dir.path().join("subdir/file3.txt")).unwrap();
        f3.write_all(b"nested").unwrap(); // 6 bytes

        dir
    }

    #[test]
    fn test_du_summarize() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DuCommand;
        let result = cmd
            .execute(&["-s".to_string(), ".".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Table { columns, rows } => {
                let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                assert_eq!(col_names, vec!["size", "path"]);
                assert_eq!(rows.len(), 1); // Just the summary
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_du_human_readable() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DuCommand;
        let result = cmd
            .execute(&["-h".to_string(), "-s".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Table { columns, .. } => {
                // Check that size column has HumanBytes format
                let size_col = columns.iter().find(|c| c.name == "size").unwrap();
                assert_eq!(size_col.format, Some(DisplayFormat::HumanBytes));
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_du_tree() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DuCommand;
        let result = cmd
            .execute(&["--tree".to_string(), ".".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Tree(tree) => {
                assert!(!tree.nodes.is_empty());
                assert_eq!(tree.root, 0);
                // Root should be a directory
                assert!(matches!(tree.nodes[0].node_type, FileType::Directory));
                // Should contain subdirectory and files
                let names: Vec<&str> = tree.nodes.iter().map(|n| n.name.as_str()).collect();
                assert!(names.iter().any(|n| *n == "subdir"));
            }
            _ => panic!("Expected Tree value"),
        }
    }

    #[test]
    fn test_du_with_total() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = DuCommand;
        let result = cmd
            .execute(
                &["-s".to_string(), "-c".to_string(), ".".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::Table { rows, .. } => {
                // Should have directory entry and total (but only 1 path so maybe just 1)
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }
}
