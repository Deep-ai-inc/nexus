//! The `find` command - search for files.

use super::{CommandContext, NexusCommand};
use nexus_api::{FileEntry, FileType, Value};
use std::fs;
use std::path::PathBuf;

pub struct FindCommand;

struct FindOptions {
    name_pattern: Option<String>,
    iname_pattern: Option<String>,
    file_type: Option<FindFileType>,
    max_depth: Option<usize>,
    min_depth: Option<usize>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    empty: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum FindFileType {
    File,
    Directory,
    Symlink,
}

impl FindOptions {
    fn parse(args: &[String]) -> (Self, Vec<PathBuf>) {
        let mut opts = FindOptions {
            name_pattern: None,
            iname_pattern: None,
            file_type: None,
            max_depth: None,
            min_depth: None,
            min_size: None,
            max_size: None,
            empty: false,
        };

        let mut paths = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];

            if arg == "-name" {
                if i + 1 < args.len() {
                    opts.name_pattern = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            } else if arg == "-iname" {
                if i + 1 < args.len() {
                    opts.iname_pattern = Some(args[i + 1].to_lowercase());
                    i += 2;
                    continue;
                }
            } else if arg == "-type" {
                if i + 1 < args.len() {
                    opts.file_type = match args[i + 1].as_str() {
                        "f" => Some(FindFileType::File),
                        "d" => Some(FindFileType::Directory),
                        "l" => Some(FindFileType::Symlink),
                        _ => None,
                    };
                    i += 2;
                    continue;
                }
            } else if arg == "-maxdepth" {
                if i + 1 < args.len() {
                    opts.max_depth = args[i + 1].parse().ok();
                    i += 2;
                    continue;
                }
            } else if arg == "-mindepth" {
                if i + 1 < args.len() {
                    opts.min_depth = args[i + 1].parse().ok();
                    i += 2;
                    continue;
                }
            } else if arg == "-size" {
                if i + 1 < args.len() {
                    let size_str = &args[i + 1];
                    if let Some(size) = parse_size(size_str) {
                        if size_str.starts_with('+') {
                            opts.min_size = Some(size);
                        } else if size_str.starts_with('-') {
                            opts.max_size = Some(size);
                        } else {
                            opts.min_size = Some(size);
                            opts.max_size = Some(size);
                        }
                    }
                    i += 2;
                    continue;
                }
            } else if arg == "-empty" {
                opts.empty = true;
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

    fn matches(&self, entry: &FileEntry, depth: usize) -> bool {
        // Check depth constraints
        if let Some(max) = self.max_depth {
            if depth > max {
                return false;
            }
        }
        if let Some(min) = self.min_depth {
            if depth < min {
                return false;
            }
        }

        // Check file type
        if let Some(ft) = self.file_type {
            let is_match = match ft {
                FindFileType::File => entry.file_type == FileType::File,
                FindFileType::Directory => entry.file_type == FileType::Directory,
                FindFileType::Symlink => entry.is_symlink,
            };
            if !is_match {
                return false;
            }
        }

        // Check name pattern (glob-style)
        if let Some(ref pattern) = self.name_pattern {
            if !glob_match(pattern, &entry.name) {
                return false;
            }
        }

        // Check iname pattern (case-insensitive)
        if let Some(ref pattern) = self.iname_pattern {
            if !glob_match(pattern, &entry.name.to_lowercase()) {
                return false;
            }
        }

        // Check size constraints
        if let Some(min) = self.min_size {
            if entry.size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if entry.size > max {
                return false;
            }
        }

        // Check empty
        if self.empty {
            if entry.file_type == FileType::Directory {
                // Would need to check if directory is empty - skip for now
            } else if entry.size != 0 {
                return false;
            }
        }

        true
    }
}

fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim_start_matches(|c| c == '+' || c == '-');

    let (num_str, multiplier) = if let Some(stripped) = s.strip_suffix('G') {
        (stripped, 1024 * 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix('M') {
        (stripped, 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix('k') {
        (stripped, 1024)
    } else if let Some(stripped) = s.strip_suffix('c') {
        (stripped, 1)
    } else {
        // Default to 512-byte blocks like find
        (s, 512)
    };

    num_str.parse::<u64>().ok().map(|n| n * multiplier)
}

fn glob_match(pattern: &str, name: &str) -> bool {
    // Simple glob matching: * matches anything, ? matches single char
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();

    glob_match_impl(&pattern_chars, &name_chars)
}

fn glob_match_impl(pattern: &[char], name: &[char]) -> bool {
    let mut pi = 0;
    let mut ni = 0;
    let mut star_pi = None;
    let mut star_ni = None;

    while ni < name.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == name[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = Some(pi);
            star_ni = Some(ni);
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ni = Some(star_ni.unwrap() + 1);
            ni = star_ni.unwrap();
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }

    pi == pattern.len()
}

impl NexusCommand for FindCommand {
    fn name(&self) -> &'static str {
        "find"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let (opts, paths) = FindOptions::parse(args);

        let mut results: Vec<Value> = Vec::new();

        for path in paths {
            let resolved = if path.is_absolute() {
                path
            } else {
                ctx.state.cwd.join(path)
            };

            find_recursive(&resolved, &opts, 0, &mut results)?;
        }

        Ok(Value::List(results))
    }
}

fn find_recursive(
    path: &PathBuf,
    opts: &FindOptions,
    depth: usize,
    results: &mut Vec<Value>,
) -> anyhow::Result<()> {
    // Check max depth for recursion
    if let Some(max) = opts.max_depth {
        if depth > max {
            return Ok(());
        }
    }

    // Use FileEntry::from_path to create the entry
    let entry = match FileEntry::from_path(path.clone()) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    let is_dir = entry.file_type == FileType::Directory;

    if opts.matches(&entry, depth) {
        results.push(Value::FileEntry(Box::new(entry)));
    }

    if is_dir {
        if let Ok(entries) = fs::read_dir(path) {
            for dir_entry in entries.flatten() {
                find_recursive(&dir_entry.path(), opts, depth + 1, results)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "foo.rs"));
        assert!(glob_match("*.rs", "bar.rs"));
        assert!(!glob_match("*.rs", "foo.txt"));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("f?o", "foo"));
        assert!(!glob_match("f?o", "fooo"));
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("100"), Some(51200)); // 100 * 512
        assert_eq!(parse_size("1k"), Some(1024));
        assert_eq!(parse_size("1M"), Some(1024 * 1024));
        assert_eq!(parse_size("+1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("100c"), Some(100));
    }
}
