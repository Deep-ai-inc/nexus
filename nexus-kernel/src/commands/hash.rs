//! hash - Display or manipulate the command hash table.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

pub struct HashCommand;

impl NexusCommand for HashCommand {
    fn name(&self) -> &'static str {
        "hash"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut clear = false;
        let mut delete = Vec::new();
        let mut lookup = Vec::new();

        // Parse options
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "-r" => clear = true,
                "-d" => {
                    i += 1;
                    if i < args.len() {
                        delete.push(args[i].clone());
                    }
                }
                "-t" => {
                    i += 1;
                    while i < args.len() && !args[i].starts_with('-') {
                        lookup.push(args[i].clone());
                        i += 1;
                    }
                    continue;
                }
                arg if !arg.starts_with('-') => {
                    // Add command to hash
                    if let Some(path) = find_in_path(arg, ctx) {
                        ctx.state.command_hash.insert(arg.to_string(), path);
                    } else {
                        eprintln!("hash: {}: not found", arg);
                    }
                }
                _ => {}
            }
            i += 1;
        }

        // Clear hash table
        if clear {
            ctx.state.command_hash.clear();
            return Ok(Value::Unit);
        }

        // Delete specific entries
        for name in &delete {
            if ctx.state.command_hash.remove(name).is_none() {
                eprintln!("hash: {}: not found", name);
            }
        }

        // Lookup specific commands
        if !lookup.is_empty() {
            let mut results = Vec::new();
            for name in &lookup {
                if let Some(path) = ctx.state.command_hash.get(name) {
                    results.push(Value::Record(vec![
                        ("name".to_string(), Value::String(name.clone())),
                        ("path".to_string(), Value::Path(path.clone())),
                    ]));
                } else if let Some(path) = find_in_path(name, ctx) {
                    // Add to cache and return
                    ctx.state.command_hash.insert(name.clone(), path.clone());
                    results.push(Value::Record(vec![
                        ("name".to_string(), Value::String(name.clone())),
                        ("path".to_string(), Value::Path(path)),
                    ]));
                } else {
                    eprintln!("hash: {}: not found", name);
                }
            }
            return Ok(Value::List(results));
        }

        // If no args (after parsing options), display hash table
        if args.is_empty() || (clear && args.len() == 1) {
            if ctx.state.command_hash.is_empty() {
                return Ok(Value::Table {
                    columns: vec!["hits".to_string(), "command".to_string()],
                    rows: vec![],
                });
            }

            let rows: Vec<Vec<Value>> = ctx
                .state
                .command_hash
                .iter()
                .map(|(name, path)| {
                    vec![
                        Value::Int(1), // We don't track hits, so default to 1
                        Value::String(format!("{} -> {}", name, path.display())),
                    ]
                })
                .collect();

            return Ok(Value::Table {
                columns: vec!["hits".to_string(), "command".to_string()],
                rows,
            });
        }

        Ok(Value::Unit)
    }
}

/// Find a command in PATH.
fn find_in_path(cmd: &str, ctx: &CommandContext) -> Option<PathBuf> {
    let path_var = ctx.state.get_env("PATH")?;

    for dir in path_var.split(':') {
        let candidate = PathBuf::from(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_hash_command_name() {
        let cmd = HashCommand;
        assert_eq!(cmd.name(), "hash");
    }

    #[test]
    fn test_hash_empty_table() {
        let cmd = HashCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert_eq!(columns, vec!["hits".to_string(), "command".to_string()]);
                assert!(rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_hash_clear() {
        let cmd = HashCommand;
        let mut test_ctx = TestContext::new_default();

        // Add something to the hash
        test_ctx
            .state
            .command_hash
            .insert("test".to_string(), PathBuf::from("/usr/bin/test"));
        assert!(!test_ctx.state.command_hash.is_empty());

        // Clear it
        cmd.execute(&["-r".to_string()], &mut test_ctx.ctx())
            .unwrap();

        assert!(test_ctx.state.command_hash.is_empty());
    }

    #[test]
    fn test_hash_delete() {
        let cmd = HashCommand;
        let mut test_ctx = TestContext::new_default();

        // Add something to the hash
        test_ctx
            .state
            .command_hash
            .insert("test".to_string(), PathBuf::from("/usr/bin/test"));
        test_ctx
            .state
            .command_hash
            .insert("other".to_string(), PathBuf::from("/usr/bin/other"));

        // Delete one entry
        cmd.execute(&["-d".to_string(), "test".to_string()], &mut test_ctx.ctx())
            .unwrap();

        assert!(!test_ctx.state.command_hash.contains_key("test"));
        assert!(test_ctx.state.command_hash.contains_key("other"));
    }

    #[test]
    fn test_hash_lookup_cached() {
        let cmd = HashCommand;
        let mut test_ctx = TestContext::new_default();

        // Pre-populate cache
        test_ctx.state.command_hash.insert(
            "cached_cmd".to_string(),
            PathBuf::from("/usr/bin/cached_cmd"),
        );

        let result = cmd
            .execute(
                &["-t".to_string(), "cached_cmd".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::List(entries) => {
                assert_eq!(entries.len(), 1);
                match &entries[0] {
                    Value::Record(fields) => {
                        let name = fields.iter().find(|(k, _)| k == "name").map(|(_, v)| v);
                        assert_eq!(name, Some(&Value::String("cached_cmd".to_string())));
                    }
                    _ => panic!("Expected Record"),
                }
            }
            _ => panic!("Expected List"),
        }
    }
}
