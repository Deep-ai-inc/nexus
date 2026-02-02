//! The `tree` command - display directory structure as a tree.

use super::{CommandContext, NexusCommand};
use nexus_api::{FileType, InteractiveRequest, TreeInfo, TreeNodeFlat, Value, ViewerKind};
use std::fs;
use std::path::PathBuf;

pub struct TreeCommand;

impl NexusCommand for TreeCommand {
    fn name(&self) -> &'static str {
        "tree"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut max_depth: Option<usize> = None;
        let mut show_hidden = false;
        let mut target: Option<String> = None;
        let mut i = 0;

        while i < args.len() {
            match args[i].as_str() {
                "-L" | "--level" => {
                    if i + 1 < args.len() {
                        max_depth = args[i + 1].parse().ok();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "-a" | "--all" => {
                    show_hidden = true;
                    i += 1;
                }
                arg if !arg.starts_with('-') => {
                    target = Some(arg.to_string());
                    i += 1;
                }
                _ => i += 1,
            }
        }

        let root_path = if let Some(ref t) = target {
            if PathBuf::from(t).is_absolute() {
                PathBuf::from(t)
            } else {
                ctx.state.cwd.join(t)
            }
        } else {
            ctx.state.cwd.clone()
        };

        if !root_path.is_dir() {
            return Err(anyhow::anyhow!("tree: '{}' is not a directory", root_path.display()));
        }

        let mut nodes: Vec<TreeNodeFlat> = Vec::new();
        let mut id_counter = 0;

        // Build flat arena
        build_tree_nodes(
            &root_path,
            None,
            0,
            max_depth.unwrap_or(usize::MAX),
            show_hidden,
            &mut nodes,
            &mut id_counter,
        )?;

        let tree = TreeInfo {
            root: 0,
            nodes,
        };

        Ok(Value::interactive(InteractiveRequest {
            viewer: ViewerKind::TreeBrowser,
            content: Value::tree(tree),
        }))
    }
}

fn build_tree_nodes(
    path: &PathBuf,
    parent: Option<usize>,
    depth: usize,
    max_depth: usize,
    show_hidden: bool,
    nodes: &mut Vec<TreeNodeFlat>,
    id_counter: &mut usize,
) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let metadata = fs::symlink_metadata(path).ok();
    let node_type = metadata.as_ref().map(|m| {
        if m.is_dir() {
            FileType::Directory
        } else if m.is_symlink() {
            FileType::Symlink
        } else {
            FileType::File
        }
    }).unwrap_or(FileType::Unknown);

    let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

    let node_id = *id_counter;
    *id_counter += 1;

    // Count children before adding (we'll update after)
    let child_count = if node_type == FileType::Directory && depth < max_depth {
        fs::read_dir(path)
            .map(|entries| entries.count())
            .unwrap_or(0)
    } else {
        0
    };

    nodes.push(TreeNodeFlat {
        id: node_id,
        parent,
        name,
        path: path.clone(),
        node_type,
        size,
        depth,
        child_count,
    });

    // Recurse into directory children
    if node_type == FileType::Directory && depth < max_depth {
        if let Ok(entries) = fs::read_dir(path) {
            let mut children: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    if show_hidden {
                        true
                    } else {
                        p.file_name()
                            .map(|n| !n.to_string_lossy().starts_with('.'))
                            .unwrap_or(true)
                    }
                })
                .collect();

            // Sort: directories first, then alphabetical
            children.sort_by(|a, b| {
                let a_dir = a.is_dir();
                let b_dir = b.is_dir();
                match (a_dir, b_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.file_name().cmp(&b.file_name()),
                }
            });

            for child in children {
                build_tree_nodes(
                    &child,
                    Some(node_id),
                    depth + 1,
                    max_depth,
                    show_hidden,
                    nodes,
                    id_counter,
                )?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        File::create(dir.path().join("file1.txt")).unwrap().write_all(b"hello").unwrap();
        File::create(dir.path().join("subdir/file2.txt")).unwrap().write_all(b"world").unwrap();
        fs::create_dir(dir.path().join("subdir/nested")).unwrap();
        File::create(dir.path().join("subdir/nested/deep.txt")).unwrap().write_all(b"deep").unwrap();
        File::create(dir.path().join(".hidden")).unwrap();
        dir
    }

    #[test]
    fn test_tree_basic() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = TreeCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        let Some(nexus_api::DomainValue::Interactive(req)) = result.as_domain() else {
            panic!("Expected Interactive value");
        };
        assert!(matches!(req.viewer, ViewerKind::TreeBrowser));
        let Some(nexus_api::DomainValue::Tree(tree)) = req.content.as_domain() else {
            panic!("Expected Tree content");
        };
        assert!(!tree.nodes.is_empty());
        assert_eq!(tree.root, 0);
        assert_eq!(tree.nodes[0].depth, 0);
        assert!(matches!(tree.nodes[0].node_type, FileType::Directory));
    }

    #[test]
    fn test_tree_depth_limit() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = TreeCommand;
        let result = cmd
            .execute(&["-L".to_string(), "1".to_string()], &mut test_ctx.ctx())
            .unwrap();

        let Some(nexus_api::DomainValue::Interactive(req)) = result.as_domain() else {
            panic!("Expected Interactive value");
        };
        let Some(nexus_api::DomainValue::Tree(tree)) = req.content.as_domain() else {
            panic!("Expected Tree content");
        };
        for node in &tree.nodes {
            assert!(node.depth <= 1, "Node {} has depth {} > 1", node.name, node.depth);
        }
    }

    #[test]
    fn test_tree_hidden_files() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = TreeCommand;

        // Without -a: should not include .hidden
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();
        let tree = match result.as_domain() {
            Some(nexus_api::DomainValue::Interactive(req)) => match req.content.as_domain() {
                Some(nexus_api::DomainValue::Tree(t)) => t,
                _ => panic!("Expected Tree"),
            },
            _ => panic!("Expected Interactive"),
        };
        assert!(!tree.nodes.iter().any(|n| n.name == ".hidden"));

        // With -a: should include .hidden
        let result = cmd.execute(&["-a".to_string()], &mut test_ctx.ctx()).unwrap();
        let tree = match result.as_domain() {
            Some(nexus_api::DomainValue::Interactive(req)) => match req.content.as_domain() {
                Some(nexus_api::DomainValue::Tree(t)) => t,
                _ => panic!("Expected Tree"),
            },
            _ => panic!("Expected Interactive"),
        };
        assert!(tree.nodes.iter().any(|n| n.name == ".hidden"));
    }

    #[test]
    fn test_tree_not_a_directory() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = TreeCommand;
        let result = cmd.execute(
            &["file1.txt".to_string()],
            &mut test_ctx.ctx(),
        );
        assert!(result.is_err());
    }
}
