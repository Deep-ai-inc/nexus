//! Session management utilities for Claude Code CLI interactions.

use crate::features::agent::events::{UserQuestion, UserQuestionOption};

/// Parse the AskUserQuestion tool input into our UserQuestion types.
pub fn parse_user_questions(tool_input: &serde_json::Value) -> Option<Vec<UserQuestion>> {
    let questions = tool_input.get("questions")?.as_array()?;
    let mut result = Vec::new();
    for q in questions {
        let question = q.get("question")?.as_str()?.to_string();
        let header = q.get("header").and_then(|h| h.as_str()).unwrap_or("").to_string();
        let multi_select = q.get("multiSelect").and_then(|m| m.as_bool()).unwrap_or(false);
        let options = q.get("options")
            .and_then(|o| o.as_array())
            .map(|arr| {
                arr.iter().filter_map(|opt| {
                    Some(UserQuestionOption {
                        label: opt.get("label")?.as_str()?.to_string(),
                        description: opt.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                    })
                }).collect()
            })
            .unwrap_or_default();
        result.push(UserQuestion { question, header, options, multi_select });
    }
    Some(result)
}

// =============================================================================
// JSONL Surgery
// =============================================================================

/// Patch a Claude Code session JSONL file: replace the error tool_result for
/// `tool_use_id` with a success result containing `answer_content`, and
/// truncate everything after (the assistant's error-path response).
pub fn patch_session_jsonl(
    path: &std::path::Path,
    tool_use_id: &str,
    answer_content: &str,
) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Write};

    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines: Vec<serde_json::Value> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        match serde_json::from_str(&line) {
            Ok(v) => lines.push(v),
            Err(_) => continue,
        }
    }

    // Find the user message containing the error tool_result for this tool_use_id
    let mut truncate_at = None;
    for (i, line) in lines.iter_mut().enumerate() {
        if line.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let content = match line.pointer_mut("/message/content") {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => continue,
        };
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            if block.get("tool_use_id").and_then(|t| t.as_str()) != Some(tool_use_id) {
                continue;
            }
            // Found it — replace error with success
            block["content"] = serde_json::Value::String(answer_content.to_string());
            block["is_error"] = serde_json::Value::Bool(false);
            // Also fix the top-level tool_use_result if present
            if line.get("tool_use_result").is_some() {
                line["tool_use_result"] = serde_json::json!({
                    "type": "text",
                    "text": answer_content,
                });
            }
            truncate_at = Some(i + 1); // keep up to and including this line
            break;
        }
        if truncate_at.is_some() { break; }
    }

    let truncate_at = truncate_at.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "tool_result not found in session")
    })?;

    // Truncate and write back
    lines.truncate(truncate_at);
    let mut file = std::fs::File::create(path)?;
    for line in &lines {
        writeln!(file, "{}", serde_json::to_string(line).unwrap())?;
    }

    Ok(())
}

/// Compute the Claude Code session file path for a given working directory and session ID.
pub fn session_file_path(cwd: &str, session_id: &str) -> std::path::PathBuf {
    let encoded = cwd.replace('/', "-");
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    home.join(".claude/projects")
        .join(&encoded)
        .join(format!("{}.jsonl", session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------------------------------------------------------------------------
    // parse_user_questions tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_user_questions_basic() {
        let input = json!({
            "questions": [{
                "question": "Which option do you prefer?",
                "header": "Choice",
                "multiSelect": false,
                "options": [
                    {"label": "Option A", "description": "First choice"},
                    {"label": "Option B", "description": "Second choice"}
                ]
            }]
        });
        let result = parse_user_questions(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].question, "Which option do you prefer?");
        assert_eq!(result[0].header, "Choice");
        assert!(!result[0].multi_select);
        assert_eq!(result[0].options.len(), 2);
        assert_eq!(result[0].options[0].label, "Option A");
        assert_eq!(result[0].options[0].description, "First choice");
    }

    #[test]
    fn test_parse_user_questions_multi_select() {
        let input = json!({
            "questions": [{
                "question": "Select all that apply",
                "header": "Multi",
                "multiSelect": true,
                "options": [
                    {"label": "A", "description": ""},
                    {"label": "B", "description": ""}
                ]
            }]
        });
        let result = parse_user_questions(&input).unwrap();
        assert!(result[0].multi_select);
    }

    #[test]
    fn test_parse_user_questions_multiple_questions() {
        let input = json!({
            "questions": [
                {
                    "question": "First question?",
                    "header": "Q1",
                    "options": [{"label": "Yes", "description": ""}]
                },
                {
                    "question": "Second question?",
                    "header": "Q2",
                    "options": [{"label": "No", "description": ""}]
                }
            ]
        });
        let result = parse_user_questions(&input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].question, "First question?");
        assert_eq!(result[1].question, "Second question?");
    }

    #[test]
    fn test_parse_user_questions_no_questions_field() {
        let input = json!({"other": "data"});
        let result = parse_user_questions(&input);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_user_questions_empty_questions() {
        let input = json!({"questions": []});
        let result = parse_user_questions(&input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_user_questions_missing_optional_fields() {
        let input = json!({
            "questions": [{
                "question": "What?",
                "options": [{"label": "X"}]
            }]
        });
        let result = parse_user_questions(&input).unwrap();
        assert_eq!(result[0].header, ""); // default
        assert!(!result[0].multi_select); // default false
        assert_eq!(result[0].options[0].description, ""); // default
    }

    #[test]
    fn test_parse_user_questions_invalid_question_missing_text() {
        // When a question is missing required "question" field, the ? operator
        // causes early return of None from the function
        let input = json!({
            "questions": [{
                "header": "H",
                "options": []
            }]
        });
        let result = parse_user_questions(&input);
        assert!(result.is_none());
    }

    // -------------------------------------------------------------------------
    // session_file_path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_session_file_path_basic() {
        let path = session_file_path("/home/user/project", "abc123");
        let path_str = path.to_string_lossy();
        assert!(path_str.ends_with("abc123.jsonl"));
        assert!(path_str.contains(".claude/projects"));
        assert!(path_str.contains("-home-user-project")); // slashes replaced with dashes
    }

    #[test]
    fn test_session_file_path_root() {
        let path = session_file_path("/", "session-id");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("-")); // "/" becomes "-"
        assert!(path_str.ends_with("session-id.jsonl"));
    }

    #[test]
    fn test_session_file_path_nested() {
        let path = session_file_path("/a/b/c/d/e", "test");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("-a-b-c-d-e"));
    }
}
