//! Tool formatter system for regenerating tool blocks in different syntaxes

use crate::agent::ToolSyntax;
use crate::tools::core::ToolRegistry;
use crate::tools::parser_registry::is_multiline_param;
use crate::tools::ToolRequest;
use anyhow::Result;
use serde_json::Value;

/// Trait for formatting tool requests into their string representation in different syntaxes
pub trait ToolFormatter {
    /// Format a tool request as a string in the appropriate syntax
    fn format_tool_request(&self, request: &ToolRequest) -> Result<String>;
}

/// Formatter for Native tool syntax (JSON-based)
pub struct NativeFormatter;

impl ToolFormatter for NativeFormatter {
    fn format_tool_request(&self, request: &ToolRequest) -> Result<String> {
        // Native tools are represented as JSON function calls
        // Return the input serialized as JSON string
        Ok(serde_json::to_string(&request.input)?)
    }
}

/// Formatter for XML tool syntax
pub struct XmlFormatter;

impl ToolFormatter for XmlFormatter {
    fn format_tool_request(&self, request: &ToolRequest) -> Result<String> {
        let mut formatted = format!("<tool:{}>\n", request.name);

        // Get tool spec to understand parameter types and defaults
        let registry = ToolRegistry::global();
        let tool_spec = registry
            .get(&request.name)
            .map(|tool| tool.spec())
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not found in registry", request.name))?;

        if let Value::Object(map) = &request.input {
            // Get required parameters from schema
            let required_params = tool_spec
                .parameters_schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<std::collections::HashSet<_>>()
                })
                .unwrap_or_default();

            // Get parameter properties from schema
            let empty_map = serde_json::Map::new();
            let properties = tool_spec
                .parameters_schema
                .get("properties")
                .and_then(|p| p.as_object())
                .unwrap_or(&empty_map);

            for (key, value) in map {
                // Check if this parameter should be omitted (has default value and matches it)
                if let Some(param_schema) = properties.get(key) {
                    if let Some(default_value) = param_schema.get("default") {
                        // Skip if the value matches the default and parameter is not required
                        if !required_params.contains(key.as_str()) && value == default_value {
                            continue;
                        }
                    }
                }

                // Check if this is an array parameter and handle it specially
                if let Some(param_schema) = properties.get(key) {
                    if let Some(param_type) = param_schema.get("type").and_then(|t| t.as_str()) {
                        if param_type == "array" {
                            // For arrays in XML, we repeat the parameter tag for each item
                            if let Value::Array(items) = value {
                                for item in items {
                                    let item_str = match item {
                                        Value::String(s) => s.clone(),
                                        _ => serde_json::to_string(item)?,
                                    };

                                    // Use singular form of the parameter name for XML tags
                                    let singular_name = if key.ends_with('s') && key.len() > 1 {
                                        &key[..key.len() - 1]
                                    } else {
                                        key
                                    };

                                    if is_multiline_param(key) {
                                        formatted.push_str(&format!(
                                            "<param:{singular_name}>\n{item_str}\n</param:{singular_name}>\n"
                                        ));
                                    } else {
                                        formatted.push_str(&format!(
                                            "<param:{singular_name}>{item_str}</param:{singular_name}>\n"
                                        ));
                                    }
                                }
                            }
                            continue; // Skip the normal parameter processing for arrays
                        }
                    }
                }

                // For non-array parameters, use normal formatting
                let param_value = match value {
                    Value::String(s) => s.clone(),
                    _ => serde_json::to_string(value)?,
                };

                if is_multiline_param(key) {
                    formatted.push_str(&format!("<param:{key}>\n{param_value}\n</param:{key}>\n"));
                } else {
                    formatted.push_str(&format!("<param:{key}>{param_value}</param:{key}>\n"));
                }
            }
        }

        formatted.push_str(&format!("</tool:{}>\n", request.name));
        Ok(formatted)
    }
}

/// Formatter for Caret tool syntax
pub struct CaretFormatter;

impl ToolFormatter for CaretFormatter {
    fn format_tool_request(&self, request: &ToolRequest) -> Result<String> {
        let mut formatted = format!("^^^{}\n", request.name);

        // Get tool spec to understand parameter types and defaults
        let registry = ToolRegistry::global();
        let tool_spec = registry
            .get(&request.name)
            .map(|tool| tool.spec())
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not found in registry", request.name))?;

        if let Value::Object(map) = &request.input {
            // Get required parameters from schema
            let required_params = tool_spec
                .parameters_schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<std::collections::HashSet<_>>()
                })
                .unwrap_or_default();

            // Get parameter properties from schema
            let empty_map = serde_json::Map::new();
            let properties = tool_spec
                .parameters_schema
                .get("properties")
                .and_then(|p| p.as_object())
                .unwrap_or(&empty_map);

            for (key, value) in map {
                // Check if this parameter should be omitted (has default value and matches it)
                if let Some(param_schema) = properties.get(key) {
                    if let Some(default_value) = param_schema.get("default") {
                        // Skip if the value matches the default and parameter is not required
                        if !required_params.contains(key.as_str()) && value == default_value {
                            continue;
                        }
                    }
                }

                // Format the parameter value based on its type
                let param_value = format_parameter_value_for_caret(key, value, properties)?;

                if is_multiline_param(key) {
                    formatted.push_str(&format!("{key} ---\n{param_value}\n--- {key}\n"));
                } else {
                    formatted.push_str(&format!("{key}: {param_value}\n"));
                }
            }
        }

        formatted.push_str("^^^\n");
        Ok(formatted)
    }
}

/// Format a parameter value for Caret syntax based on its schema type
fn format_parameter_value_for_caret(
    key: &str,
    value: &Value,
    properties: &serde_json::Map<String, Value>,
) -> Result<String> {
    // Check if this is an array parameter
    if let Some(param_schema) = properties.get(key) {
        if let Some(param_type) = param_schema.get("type").and_then(|t| t.as_str()) {
            if param_type == "array" {
                // For arrays in Caret, we use array syntax: [item1, item2, ...]
                if let Value::Array(items) = value {
                    let mut result = String::from("[\n");
                    for item in items {
                        let item_str = match item {
                            Value::String(s) => s.clone(),
                            _ => serde_json::to_string(item)?,
                        };
                        result.push_str(&format!("{item_str}\n"));
                    }
                    result.push(']');
                    return Ok(result);
                }
            }
        }
    }

    // For non-array parameters, use the value as-is
    match value {
        Value::String(s) => Ok(s.clone()),
        _ => Ok(serde_json::to_string(value)?),
    }
}

/// Get the appropriate formatter for a tool syntax
pub fn get_formatter(syntax: ToolSyntax) -> Box<dyn ToolFormatter> {
    match syntax {
        ToolSyntax::Native => Box::new(NativeFormatter),
        ToolSyntax::Xml => Box::new(XmlFormatter),
        ToolSyntax::Caret => Box::new(CaretFormatter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// Helper to compare tool output in an order-independent way.
    /// Checks that the wrapper (first/last lines) matches and all expected
    /// content lines are present, regardless of parameter ordering.
    fn assert_tool_output_contains(result: &str, wrapper_start: &str, wrapper_end: &str, expected_params: &[&str]) {
        let lines: Vec<&str> = result.lines().collect();
        assert!(!lines.is_empty(), "Result should not be empty");
        assert_eq!(lines[0], wrapper_start, "First line should be tool wrapper start");
        assert_eq!(lines[lines.len() - 1], wrapper_end, "Last line should be tool wrapper end");

        // Check all expected params are present (order-independent)
        for param in expected_params {
            assert!(result.contains(param), "Missing expected content: {}", param);
        }
    }

    #[test]
    fn test_xml_formatter_with_array_parameters() {
        let formatter = XmlFormatter;
        let request = ToolRequest {
            id: "test-1".to_string(),
            name: "read_files".to_string(),
            input: json!({
                "project": "test-project",
                "paths": ["file1.txt", "file2.txt", "file3.txt"]
            }),
            start_offset: None,
            end_offset: None,
        };

        let result = formatter.format_tool_request(&request).unwrap();

        assert_tool_output_contains(
            &result,
            "<tool:read_files>",
            "</tool:read_files>",
            &[
                "<param:project>test-project</param:project>",
                "<param:path>file1.txt</param:path>",
                "<param:path>file2.txt</param:path>",
                "<param:path>file3.txt</param:path>",
            ]
        );
    }

    #[test]
    fn test_xml_formatter_omits_default_values() {
        let formatter = XmlFormatter;
        let request = ToolRequest {
            id: "test-2".to_string(),
            name: "write_file".to_string(),
            input: json!({
                "project": "test-project",
                "path": "test.txt",
                "content": "Hello world",
                "append": false // This is the default value, should be omitted
            }),
            start_offset: None,
            end_offset: None,
        };

        let result = formatter.format_tool_request(&request).unwrap();

        // Should not contain the append parameter since it matches the default
        assert_tool_output_contains(
            &result,
            "<tool:write_file>",
            "</tool:write_file>",
            &[
                "<param:project>test-project</param:project>",
                "<param:path>test.txt</param:path>",
                "<param:content>",
                "Hello world",
                "</param:content>",
            ]
        );
        assert!(!result.contains("<param:append>"), "Should not contain default append value");
    }

    #[test]
    fn test_xml_formatter_includes_non_default_values() {
        let formatter = XmlFormatter;
        let request = ToolRequest {
            id: "test-3".to_string(),
            name: "write_file".to_string(),
            input: json!({
                "project": "test-project",
                "path": "test.txt",
                "content": "Hello world",
                "append": true // This is NOT the default value, should be included
            }),
            start_offset: None,
            end_offset: None,
        };

        let result = formatter.format_tool_request(&request).unwrap();

        // Should contain the append parameter since it's not the default
        assert_tool_output_contains(
            &result,
            "<tool:write_file>",
            "</tool:write_file>",
            &[
                "<param:project>test-project</param:project>",
                "<param:path>test.txt</param:path>",
                "<param:content>",
                "Hello world",
                "</param:content>",
                "<param:append>true</param:append>",
            ]
        );
    }

    #[test]
    fn test_caret_formatter_with_array_parameters() {
        let formatter = CaretFormatter;
        let request = ToolRequest {
            id: "test-4".to_string(),
            name: "read_files".to_string(),
            input: json!({
                "project": "test-project",
                "paths": ["file1.txt", "file2.txt"]
            }),
            start_offset: None,
            end_offset: None,
        };

        let result = formatter.format_tool_request(&request).unwrap();

        assert_tool_output_contains(
            &result,
            "^^^read_files",
            "^^^",
            &[
                "project: test-project",
                "paths: [",
                "file1.txt",
                "file2.txt",
                "]",
            ]
        );
    }

    #[test]
    fn test_caret_formatter_omits_default_values() {
        let formatter = CaretFormatter;
        let request = ToolRequest {
            id: "test-5".to_string(),
            name: "write_file".to_string(),
            input: json!({
                "project": "test-project",
                "path": "test.txt",
                "content": "Hello world",
                "append": false // This is the default value, should be omitted
            }),
            start_offset: None,
            end_offset: None,
        };

        let result = formatter.format_tool_request(&request).unwrap();

        // Should not contain the append parameter since it matches the default
        assert_tool_output_contains(
            &result,
            "^^^write_file",
            "^^^",
            &[
                "project: test-project",
                "path: test.txt",
                "content ---",
                "Hello world",
                "--- content",
            ]
        );
        assert!(!result.contains("append:"), "Should not contain default append value");
    }

    #[test]
    fn test_caret_formatter_includes_non_default_values() {
        let formatter = CaretFormatter;
        let request = ToolRequest {
            id: "test-5".to_string(),
            name: "write_file".to_string(),
            input: json!({
                "project": "test-project",
                "path": "test.txt",
                "content": "Hello world",
                "append": true // This is NOT the default value, should be included
            }),
            start_offset: None,
            end_offset: None,
        };

        let result = formatter.format_tool_request(&request).unwrap();

        // Should contain the append parameter since it's not the default
        assert_tool_output_contains(
            &result,
            "^^^write_file",
            "^^^",
            &[
                "project: test-project",
                "path: test.txt",
                "content ---",
                "Hello world",
                "--- content",
                "append: true",
            ]
        );
    }
}
