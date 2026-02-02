//! JSON commands - from-json, to-json.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs;
use std::path::PathBuf;

// ============================================================================
// from-json - parse JSON into Value
// ============================================================================

pub struct FromJsonCommand;

impl NexusCommand for FromJsonCommand {
    fn name(&self) -> &'static str {
        "from-json"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            let text = stdin_value.to_text();
            return parse_json(&text);
        }

        // Read from file if provided
        if let Some(file) = args.first() {
            let path = if PathBuf::from(file).is_absolute() {
                PathBuf::from(file)
            } else {
                ctx.state.cwd.join(file)
            };

            let content = fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("{}: {}", file, e))?;
            return parse_json(&content);
        }

        Ok(Value::Unit)
    }
}

fn parse_json(text: &str) -> anyhow::Result<Value> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| anyhow::anyhow!("JSON parse error: {}", e))?;

    Ok(json_to_value(json))
}

fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Unit,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(arr) => {
            Value::List(arr.into_iter().map(json_to_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let entries: Vec<(String, Value)> = obj
                .into_iter()
                .map(|(k, v)| (k, json_to_value(v)))
                .collect();
            Value::Record(entries)
        }
    }
}

// ============================================================================
// to-json - convert Value to JSON
// ============================================================================

pub struct ToJsonCommand;

impl NexusCommand for ToJsonCommand {
    fn name(&self) -> &'static str {
        "to-json"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let pretty = args.iter().any(|a| a == "-p" || a == "--pretty");

        if let Some(stdin_value) = ctx.stdin.take() {
            let json = value_to_json(&stdin_value);
            let text = if pretty {
                serde_json::to_string_pretty(&json)?
            } else {
                serde_json::to_string(&json)?
            };
            return Ok(Value::String(text));
        }

        Ok(Value::String("null".to_string()))
    }
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Unit => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::Value::Number((*n).into()),
        Value::Float(f) => {
            if let Some(n) = serde_json::Number::from_f64(*f) {
                serde_json::Value::Number(n)
            } else {
                serde_json::Value::Null
            }
        }
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Path(p) => serde_json::Value::String(p.to_string_lossy().to_string()),
        Value::Bytes(b) => {
            // Encode as base64 or array of numbers
            serde_json::Value::Array(b.iter().map(|&byte| serde_json::Value::Number(byte.into())).collect())
        }
        Value::List(items) => {
            serde_json::Value::Array(items.iter().map(value_to_json).collect())
        }
        Value::Record(entries) => {
            let map: serde_json::Map<String, serde_json::Value> = entries
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        Value::Table { columns, rows } => {
            // Convert table to array of objects
            let arr: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    let map: serde_json::Map<String, serde_json::Value> = columns
                        .iter()
                        .zip(row.iter())
                        .map(|(col, val)| (col.name.clone(), value_to_json(val)))
                        .collect();
                    serde_json::Value::Object(map)
                })
                .collect();
            serde_json::Value::Array(arr)
        }
        Value::FileEntry(entry) => {
            let mut map = serde_json::Map::new();
            map.insert("name".to_string(), serde_json::Value::String(entry.name.clone()));
            map.insert("path".to_string(), serde_json::Value::String(entry.path.to_string_lossy().to_string()));
            map.insert("size".to_string(), serde_json::Value::Number(entry.size.into()));
            map.insert("is_dir".to_string(), serde_json::Value::Bool(entry.file_type == nexus_api::FileType::Directory));
            if let Some(modified) = entry.modified {
                map.insert("modified".to_string(), serde_json::Value::Number(modified.into()));
            }
            serde_json::Value::Object(map)
        }
        Value::Error { code, message } => {
            let mut map = serde_json::Map::new();
            map.insert("error".to_string(), serde_json::Value::String(message.clone()));
            map.insert("code".to_string(), serde_json::Value::Number((*code).into()));
            serde_json::Value::Object(map)
        }
        Value::Media { content_type, metadata, data } => {
            use base64::Engine;
            let mut map = serde_json::Map::new();
            map.insert("type".to_string(), serde_json::Value::String("media".to_string()));
            map.insert("content_type".to_string(), serde_json::Value::String(content_type.clone()));
            map.insert("size".to_string(), serde_json::Value::Number((data.len() as u64).into()));
            // Encode data as base64
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            map.insert("data".to_string(), serde_json::Value::String(b64));
            if let Some(w) = metadata.width {
                map.insert("width".to_string(), serde_json::Value::Number(w.into()));
            }
            if let Some(h) = metadata.height {
                map.insert("height".to_string(), serde_json::Value::Number(h.into()));
            }
            if let Some(name) = &metadata.filename {
                map.insert("filename".to_string(), serde_json::Value::String(name.clone()));
            }
            serde_json::Value::Object(map)
        }
        Value::Process(proc) => {
            let mut map = serde_json::Map::new();
            map.insert("_type".to_string(), serde_json::Value::String("process".to_string()));
            map.insert("pid".to_string(), serde_json::Value::Number(proc.pid.into()));
            map.insert("ppid".to_string(), serde_json::Value::Number(proc.ppid.into()));
            map.insert("user".to_string(), serde_json::Value::String(proc.user.clone()));
            map.insert("command".to_string(), serde_json::Value::String(proc.command.clone()));
            map.insert("args".to_string(), serde_json::Value::Array(
                proc.args.iter().map(|s| serde_json::Value::String(s.clone())).collect()
            ));
            map.insert("cpu_percent".to_string(),
                serde_json::Number::from_f64(proc.cpu_percent)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null));
            map.insert("mem_bytes".to_string(), serde_json::Value::Number(proc.mem_bytes.into()));
            map.insert("mem_percent".to_string(),
                serde_json::Number::from_f64(proc.mem_percent)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null));
            map.insert("status".to_string(), serde_json::Value::String(format!("{:?}", proc.status)));
            if let Some(started) = proc.started {
                map.insert("started".to_string(), serde_json::Value::Number(started.into()));
            }
            serde_json::Value::Object(map)
        }
        Value::GitStatus(status) => {
            let mut map = serde_json::Map::new();
            map.insert("_type".to_string(), serde_json::Value::String("git-status".to_string()));
            map.insert("branch".to_string(), serde_json::Value::String(status.branch.clone()));
            if let Some(upstream) = &status.upstream {
                map.insert("upstream".to_string(), serde_json::Value::String(upstream.clone()));
            }
            map.insert("ahead".to_string(), serde_json::Value::Number(status.ahead.into()));
            map.insert("behind".to_string(), serde_json::Value::Number(status.behind.into()));
            map.insert("has_conflicts".to_string(), serde_json::Value::Bool(status.has_conflicts));
            map.insert("staged".to_string(), serde_json::Value::Array(
                status.staged.iter().map(|f| {
                    let mut m = serde_json::Map::new();
                    m.insert("path".to_string(), serde_json::Value::String(f.path.clone()));
                    m.insert("status".to_string(), serde_json::Value::String(format!("{:?}", f.status)));
                    if let Some(orig) = &f.orig_path {
                        m.insert("orig_path".to_string(), serde_json::Value::String(orig.clone()));
                    }
                    serde_json::Value::Object(m)
                }).collect()
            ));
            map.insert("unstaged".to_string(), serde_json::Value::Array(
                status.unstaged.iter().map(|f| {
                    let mut m = serde_json::Map::new();
                    m.insert("path".to_string(), serde_json::Value::String(f.path.clone()));
                    m.insert("status".to_string(), serde_json::Value::String(format!("{:?}", f.status)));
                    serde_json::Value::Object(m)
                }).collect()
            ));
            map.insert("untracked".to_string(), serde_json::Value::Array(
                status.untracked.iter().map(|s| serde_json::Value::String(s.clone())).collect()
            ));
            serde_json::Value::Object(map)
        }
        Value::GitCommit(commit) => {
            let mut map = serde_json::Map::new();
            map.insert("_type".to_string(), serde_json::Value::String("git-commit".to_string()));
            map.insert("hash".to_string(), serde_json::Value::String(commit.hash.clone()));
            map.insert("short_hash".to_string(), serde_json::Value::String(commit.short_hash.clone()));
            map.insert("author".to_string(), serde_json::Value::String(commit.author.clone()));
            map.insert("author_email".to_string(), serde_json::Value::String(commit.author_email.clone()));
            map.insert("date".to_string(), serde_json::Value::Number(commit.date.into()));
            map.insert("message".to_string(), serde_json::Value::String(commit.message.clone()));
            if let Some(body) = &commit.body {
                map.insert("body".to_string(), serde_json::Value::String(body.clone()));
            }
            if let Some(n) = commit.files_changed {
                map.insert("files_changed".to_string(), serde_json::Value::Number(n.into()));
            }
            if let Some(n) = commit.insertions {
                map.insert("insertions".to_string(), serde_json::Value::Number(n.into()));
            }
            if let Some(n) = commit.deletions {
                map.insert("deletions".to_string(), serde_json::Value::Number(n.into()));
            }
            serde_json::Value::Object(map)
        }
        Value::Structured { kind, data } => {
            let mut map: serde_json::Map<String, serde_json::Value> = data
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect();
            if let Some(k) = kind {
                map.insert("_type".to_string(), serde_json::Value::String(k.clone()));
            }
            serde_json::Value::Object(map)
        }

        // New domain types: serialize via serde
        _ => {
            // Use to_text() as a string fallback for JSON
            serde_json::Value::String(value.to_text())
        }
    }
}

// ============================================================================
// jq-style access: get (get field/index from value)
// ============================================================================

pub struct GetCommand;

impl NexusCommand for GetCommand {
    fn name(&self) -> &'static str {
        "get"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let key = args.first().map(|s| s.as_str()).unwrap_or("");

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(get_value(stdin_value, key));
        }

        Ok(Value::Unit)
    }
}

fn get_value(value: Value, key: &str) -> Value {
    // Try numeric index first
    if let Ok(index) = key.parse::<usize>() {
        match value {
            Value::List(items) => {
                return items.into_iter().nth(index).unwrap_or(Value::Unit);
            }
            Value::Table { columns: _, rows } => {
                return rows.into_iter().nth(index)
                    .map(Value::List)
                    .unwrap_or(Value::Unit);
            }
            _ => {}
        }
    }

    // String key for records
    match value {
        Value::Record(entries) => {
            for (k, v) in entries {
                if k == key {
                    return v;
                }
            }
            Value::Unit
        }
        Value::Table { columns, rows } => {
            // Get column by name
            if let Some(col_idx) = columns.iter().position(|c| c.name == key) {
                let values: Vec<Value> = rows
                    .into_iter()
                    .filter_map(|row| row.into_iter().nth(col_idx))
                    .collect();
                Value::List(values)
            } else {
                Value::Unit
            }
        }
        Value::List(items) => {
            // Get field from each item if they're records
            let values: Vec<Value> = items
                .into_iter()
                .filter_map(|item| {
                    if let Value::Record(entries) = item {
                        entries.into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
                    } else {
                        None
                    }
                })
                .collect();
            if values.is_empty() {
                Value::Unit
            } else {
                Value::List(values)
            }
        }
        _ => Value::Unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_parse() {
        let json = r#"{"name": "test", "value": 42}"#;
        let result = parse_json(json).unwrap();

        if let Value::Record(entries) = result {
            assert_eq!(entries.len(), 2);
        } else {
            panic!("Expected Record");
        }
    }

    #[test]
    fn test_json_array() {
        let json = r#"[1, 2, 3]"#;
        let result = parse_json(json).unwrap();

        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], Value::Int(1));
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_to_json() {
        let value = Value::Record(vec![
            ("name".to_string(), Value::String("test".to_string())),
            ("value".to_string(), Value::Int(42)),
        ]);
        let json = value_to_json(&value);
        assert!(json.is_object());
    }

    #[test]
    fn test_get_record() {
        let value = Value::Record(vec![
            ("name".to_string(), Value::String("test".to_string())),
            ("value".to_string(), Value::Int(42)),
        ]);
        let result = get_value(value, "name");
        assert_eq!(result, Value::String("test".to_string()));
    }

    #[test]
    fn test_get_list_index() {
        let value = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = get_value(value, "1");
        assert_eq!(result, Value::Int(2));
    }
}
