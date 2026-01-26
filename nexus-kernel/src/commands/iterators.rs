//! Iterator commands - each, map, filter, where, reduce, any, all.
//!
//! These commands provide functional-style iteration over structured data,
//! replacing traditional xargs workflows with type-safe transformations.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

// ============================================================================
// each - iterate over items (returns list of items unchanged, useful for side effects)
// ============================================================================

pub struct EachCommand;

impl NexusCommand for EachCommand {
    fn name(&self) -> &'static str {
        "each"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // If a field name is given, extract that field from each item (like pluck)
        let field = args.first().map(|s| s.as_str());

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(each_value(stdin_value, field));
        }

        Ok(Value::Unit)
    }
}

fn each_value(value: Value, field: Option<&str>) -> Value {
    match value {
        Value::List(items) => {
            if let Some(key) = field {
                // Extract field from each record
                let extracted: Vec<Value> = items
                    .into_iter()
                    .filter_map(|item| extract_field(&item, key))
                    .collect();
                Value::List(extracted)
            } else {
                // Just return the list (no-op, but useful in pipelines)
                Value::List(items)
            }
        }
        Value::Table { columns, rows } => {
            if let Some(key) = field {
                // Get column by name
                if let Some(col_idx) = columns.iter().position(|c| c == key) {
                    let values: Vec<Value> = rows
                        .into_iter()
                        .filter_map(|row| row.into_iter().nth(col_idx))
                        .collect();
                    Value::List(values)
                } else {
                    Value::Unit
                }
            } else {
                // Convert table to list of records
                let records: Vec<Value> = rows
                    .into_iter()
                    .map(|row| {
                        let entries: Vec<(String, Value)> = columns
                            .iter()
                            .cloned()
                            .zip(row)
                            .collect();
                        Value::Record(entries)
                    })
                    .collect();
                Value::List(records)
            }
        }
        other => {
            if field.is_some() {
                // Try to extract field from single record
                field.and_then(|key| extract_field(&other, key)).unwrap_or(Value::Unit)
            } else {
                other
            }
        }
    }
}

fn extract_field(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Record(entries) => {
            entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        }
        Value::FileEntry(entry) => {
            match key {
                "name" => Some(Value::String(entry.name.clone())),
                "path" => Some(Value::Path(entry.path.clone())),
                "size" => Some(Value::Int(entry.size as i64)),
                "is_dir" => Some(Value::Bool(entry.file_type == nexus_api::FileType::Directory)),
                "modified" => entry.modified.map(|m| Value::Int(m as i64)),
                _ => None,
            }
        }
        _ => None,
    }
}

// ============================================================================
// map - transform each item by extracting a field (simplified map)
// ============================================================================

pub struct MapCommand;

impl NexusCommand for MapCommand {
    fn name(&self) -> &'static str {
        "map"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // map <field> - extract field from each item
        let field = args.first().map(|s| s.as_str());

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(map_value(stdin_value, field));
        }

        Ok(Value::Unit)
    }
}

fn map_value(value: Value, field: Option<&str>) -> Value {
    // For now, map is an alias for each with field extraction
    each_value(value, field)
}

// ============================================================================
// filter / where - filter items by condition
// ============================================================================

pub struct FilterCommand;

impl NexusCommand for FilterCommand {
    fn name(&self) -> &'static str {
        "filter"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Parse filter condition: field=value, field!=value, field>value, etc.
        let condition = parse_condition(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(filter_value(stdin_value, &condition));
        }

        Ok(Value::Unit)
    }
}

pub struct WhereCommand;

impl NexusCommand for WhereCommand {
    fn name(&self) -> &'static str {
        "where"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // where is an alias for filter
        let condition = parse_condition(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(filter_value(stdin_value, &condition));
        }

        Ok(Value::Unit)
    }
}

#[derive(Debug)]
enum Condition {
    None,
    Equals(String, String),
    NotEquals(String, String),
    GreaterThan(String, f64),
    LessThan(String, f64),
    GreaterOrEqual(String, f64),
    LessOrEqual(String, f64),
    Contains(String, String),
    StartsWith(String, String),
    EndsWith(String, String),
    Matches(String, String), // regex pattern
    IsEmpty(String),
    IsNotEmpty(String),
}

fn parse_condition(args: &[String]) -> Condition {
    if args.is_empty() {
        return Condition::None;
    }

    // Join args and try to parse different formats
    let expr = args.join(" ");

    // Try: field = value or field == value
    if let Some((field, value)) = expr.split_once("==") {
        return Condition::Equals(field.trim().to_string(), value.trim().to_string());
    }
    if let Some((field, value)) = expr.split_once("!=") {
        return Condition::NotEquals(field.trim().to_string(), value.trim().to_string());
    }
    if let Some((field, value)) = expr.split_once(">=") {
        if let Ok(n) = value.trim().parse() {
            return Condition::GreaterOrEqual(field.trim().to_string(), n);
        }
    }
    if let Some((field, value)) = expr.split_once("<=") {
        if let Ok(n) = value.trim().parse() {
            return Condition::LessOrEqual(field.trim().to_string(), n);
        }
    }
    if let Some((field, value)) = expr.split_once('>') {
        if let Ok(n) = value.trim().parse() {
            return Condition::GreaterThan(field.trim().to_string(), n);
        }
    }
    if let Some((field, value)) = expr.split_once('<') {
        if let Ok(n) = value.trim().parse() {
            return Condition::LessThan(field.trim().to_string(), n);
        }
    }
    // Single = for equality
    if let Some((field, value)) = expr.split_once('=') {
        return Condition::Equals(field.trim().to_string(), value.trim().to_string());
    }

    // Check for named operators: field contains value, field starts-with value
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() >= 3 {
        let field = parts[0].to_string();
        let op = parts[1];
        let value = parts[2..].join(" ");

        match op {
            "contains" => return Condition::Contains(field, value),
            "starts-with" | "startswith" => return Condition::StartsWith(field, value),
            "ends-with" | "endswith" => return Condition::EndsWith(field, value),
            "matches" => return Condition::Matches(field, value),
            _ => {}
        }
    }

    // Check for: is-empty, is-not-empty
    if parts.len() == 2 {
        let field = parts[0].to_string();
        let op = parts[1];
        match op {
            "is-empty" | "empty" => return Condition::IsEmpty(field),
            "is-not-empty" | "not-empty" => return Condition::IsNotEmpty(field),
            _ => {}
        }
    }

    Condition::None
}

fn filter_value(value: Value, condition: &Condition) -> Value {
    match value {
        Value::List(items) => {
            let filtered: Vec<Value> = items
                .into_iter()
                .filter(|item| matches_condition(item, condition))
                .collect();
            Value::List(filtered)
        }
        Value::Table { columns, rows } => {
            // Convert each row to a record for filtering, then back to table
            let filtered_rows: Vec<Vec<Value>> = rows
                .into_iter()
                .filter(|row| {
                    let record = Value::Record(
                        columns.iter().cloned().zip(row.iter().cloned()).collect()
                    );
                    matches_condition(&record, condition)
                })
                .collect();
            Value::Table { columns, rows: filtered_rows }
        }
        other => {
            if matches_condition(&other, condition) {
                other
            } else {
                Value::Unit
            }
        }
    }
}

fn matches_condition(value: &Value, condition: &Condition) -> bool {
    match condition {
        Condition::None => true,
        Condition::Equals(field, expected) => {
            if let Some(actual) = extract_field(value, field) {
                value_equals(&actual, expected)
            } else {
                false
            }
        }
        Condition::NotEquals(field, expected) => {
            if let Some(actual) = extract_field(value, field) {
                !value_equals(&actual, expected)
            } else {
                true
            }
        }
        Condition::GreaterThan(field, num) => {
            if let Some(actual) = extract_field(value, field) {
                value_to_f64(&actual).map(|n| n > *num).unwrap_or(false)
            } else {
                false
            }
        }
        Condition::LessThan(field, num) => {
            if let Some(actual) = extract_field(value, field) {
                value_to_f64(&actual).map(|n| n < *num).unwrap_or(false)
            } else {
                false
            }
        }
        Condition::GreaterOrEqual(field, num) => {
            if let Some(actual) = extract_field(value, field) {
                value_to_f64(&actual).map(|n| n >= *num).unwrap_or(false)
            } else {
                false
            }
        }
        Condition::LessOrEqual(field, num) => {
            if let Some(actual) = extract_field(value, field) {
                value_to_f64(&actual).map(|n| n <= *num).unwrap_or(false)
            } else {
                false
            }
        }
        Condition::Contains(field, substr) => {
            if let Some(actual) = extract_field(value, field) {
                actual.to_text().contains(substr)
            } else {
                false
            }
        }
        Condition::StartsWith(field, prefix) => {
            if let Some(actual) = extract_field(value, field) {
                actual.to_text().starts_with(prefix)
            } else {
                false
            }
        }
        Condition::EndsWith(field, suffix) => {
            if let Some(actual) = extract_field(value, field) {
                actual.to_text().ends_with(suffix)
            } else {
                false
            }
        }
        Condition::Matches(field, pattern) => {
            if let Some(actual) = extract_field(value, field) {
                regex::Regex::new(pattern)
                    .map(|re| re.is_match(&actual.to_text()))
                    .unwrap_or(false)
            } else {
                false
            }
        }
        Condition::IsEmpty(field) => {
            if let Some(actual) = extract_field(value, field) {
                is_value_empty(&actual)
            } else {
                true // Missing field is considered empty
            }
        }
        Condition::IsNotEmpty(field) => {
            if let Some(actual) = extract_field(value, field) {
                !is_value_empty(&actual)
            } else {
                false
            }
        }
    }
}

fn value_equals(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(s) => s == expected,
        Value::Int(n) => expected.parse::<i64>().map(|e| *n == e).unwrap_or(false),
        Value::Float(f) => expected.parse::<f64>().map(|e| (*f - e).abs() < f64::EPSILON).unwrap_or(false),
        Value::Bool(b) => expected.parse::<bool>().map(|e| *b == e).unwrap_or(false),
        other => other.to_text() == expected,
    }
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn is_value_empty(value: &Value) -> bool {
    match value {
        Value::Unit => true,
        Value::String(s) => s.is_empty(),
        Value::List(items) => items.is_empty(),
        Value::Record(entries) => entries.is_empty(),
        _ => false,
    }
}

// ============================================================================
// reduce - reduce list to single value
// ============================================================================

pub struct ReduceCommand;

impl NexusCommand for ReduceCommand {
    fn name(&self) -> &'static str {
        "reduce"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // reduce sum - sum all numeric values
        // reduce min - find minimum
        // reduce max - find maximum
        // reduce concat - concatenate strings
        // reduce count - count items
        let op = args.first().map(|s| s.as_str()).unwrap_or("sum");
        let field = args.get(1).map(|s| s.as_str());

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(reduce_value(stdin_value, op, field));
        }

        Ok(Value::Unit)
    }
}

fn reduce_value(value: Value, op: &str, field: Option<&str>) -> Value {
    let items = match value {
        Value::List(items) => items,
        Value::Table { columns, rows } => {
            // If a field is specified, extract that column
            if let Some(key) = field {
                if let Some(col_idx) = columns.iter().position(|c| c == key) {
                    rows.into_iter()
                        .filter_map(|row| row.into_iter().nth(col_idx))
                        .collect()
                } else {
                    return Value::Unit;
                }
            } else {
                // Reduce the first numeric column
                if let Some(col_idx) = rows.first().and_then(|row| {
                    row.iter().position(|v| matches!(v, Value::Int(_) | Value::Float(_)))
                }) {
                    rows.into_iter()
                        .filter_map(|row| row.into_iter().nth(col_idx))
                        .collect()
                } else {
                    return Value::Unit;
                }
            }
        }
        single => {
            if let Some(key) = field {
                if let Some(v) = extract_field(&single, key) {
                    vec![v]
                } else {
                    vec![single]
                }
            } else {
                vec![single]
            }
        }
    };

    // If a field is specified for list items, extract it
    let items: Vec<Value> = if let Some(key) = field {
        items.into_iter()
            .filter_map(|item| extract_field(&item, key))
            .collect()
    } else {
        items
    };

    match op {
        "sum" => {
            let total: f64 = items.iter()
                .filter_map(value_to_f64)
                .sum();
            if total.fract() == 0.0 {
                Value::Int(total as i64)
            } else {
                Value::Float(total)
            }
        }
        "min" => {
            items.into_iter()
                .filter_map(|v| value_to_f64(&v).map(|n| (n, v)))
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(_, v)| v)
                .unwrap_or(Value::Unit)
        }
        "max" => {
            items.into_iter()
                .filter_map(|v| value_to_f64(&v).map(|n| (n, v)))
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(_, v)| v)
                .unwrap_or(Value::Unit)
        }
        "avg" | "average" | "mean" => {
            let nums: Vec<f64> = items.iter().filter_map(value_to_f64).collect();
            if nums.is_empty() {
                Value::Unit
            } else {
                Value::Float(nums.iter().sum::<f64>() / nums.len() as f64)
            }
        }
        "count" => Value::Int(items.len() as i64),
        "concat" | "join" => {
            let texts: Vec<String> = items.iter().map(|v| v.to_text()).collect();
            Value::String(texts.join(""))
        }
        "first" => items.into_iter().next().unwrap_or(Value::Unit),
        "last" => items.into_iter().last().unwrap_or(Value::Unit),
        _ => Value::Unit,
    }
}

// ============================================================================
// any - check if any item matches condition
// ============================================================================

pub struct AnyCommand;

impl NexusCommand for AnyCommand {
    fn name(&self) -> &'static str {
        "any"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let condition = parse_condition(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            let result = any_matches(&stdin_value, &condition);
            return Ok(Value::Bool(result));
        }

        Ok(Value::Bool(false))
    }
}

fn any_matches(value: &Value, condition: &Condition) -> bool {
    match value {
        Value::List(items) => items.iter().any(|item| matches_condition(item, condition)),
        Value::Table { columns, rows } => {
            rows.iter().any(|row| {
                let record = Value::Record(
                    columns.iter().cloned().zip(row.iter().cloned()).collect()
                );
                matches_condition(&record, condition)
            })
        }
        other => matches_condition(other, condition),
    }
}

// ============================================================================
// all - check if all items match condition
// ============================================================================

pub struct AllCommand;

impl NexusCommand for AllCommand {
    fn name(&self) -> &'static str {
        "all"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let condition = parse_condition(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            let result = all_match(&stdin_value, &condition);
            return Ok(Value::Bool(result));
        }

        Ok(Value::Bool(true)) // Empty is vacuously true
    }
}

fn all_match(value: &Value, condition: &Condition) -> bool {
    match value {
        Value::List(items) => items.iter().all(|item| matches_condition(item, condition)),
        Value::Table { columns, rows } => {
            rows.iter().all(|row| {
                let record = Value::Record(
                    columns.iter().cloned().zip(row.iter().cloned()).collect()
                );
                matches_condition(&record, condition)
            })
        }
        other => matches_condition(other, condition),
    }
}

// ============================================================================
// group-by - group items by a field
// ============================================================================

pub struct GroupByCommand;

impl NexusCommand for GroupByCommand {
    fn name(&self) -> &'static str {
        "group-by"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let field = args.first().map(|s| s.as_str()).unwrap_or("");

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(group_by_value(stdin_value, field));
        }

        Ok(Value::Unit)
    }
}

fn group_by_value(value: Value, field: &str) -> Value {
    use std::collections::HashMap;

    match value {
        Value::List(items) => {
            let mut groups: HashMap<String, Vec<Value>> = HashMap::new();

            for item in items {
                let key = extract_field(&item, field)
                    .map(|v| v.to_text())
                    .unwrap_or_else(|| "(none)".to_string());
                groups.entry(key).or_default().push(item);
            }

            // Convert to list of records with group key and items
            let result: Vec<Value> = groups
                .into_iter()
                .map(|(key, items)| {
                    let count = items.len();
                    Value::Record(vec![
                        ("key".to_string(), Value::String(key)),
                        ("items".to_string(), Value::List(items)),
                        ("count".to_string(), Value::Int(count as i64)),
                    ])
                })
                .collect();

            Value::List(result)
        }
        Value::Table { columns, rows } => {
            // Find the column index
            let col_idx = columns.iter().position(|c| c == field);
            if col_idx.is_none() {
                return Value::Unit;
            }
            let col_idx = col_idx.unwrap();

            let mut groups: HashMap<String, Vec<Vec<Value>>> = HashMap::new();

            for row in rows {
                let key = row.get(col_idx)
                    .map(|v| v.to_text())
                    .unwrap_or_else(|| "(none)".to_string());
                groups.entry(key).or_default().push(row);
            }

            // Return as list of grouped tables
            let result: Vec<Value> = groups
                .into_iter()
                .map(|(key, rows)| {
                    Value::Record(vec![
                        ("key".to_string(), Value::String(key)),
                        ("table".to_string(), Value::Table {
                            columns: columns.clone(),
                            rows
                        }),
                    ])
                })
                .collect();

            Value::List(result)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_equals() {
        let items = Value::List(vec![
            Value::Record(vec![("name".to_string(), Value::String("alice".to_string()))]),
            Value::Record(vec![("name".to_string(), Value::String("bob".to_string()))]),
        ]);

        let condition = Condition::Equals("name".to_string(), "alice".to_string());
        let result = filter_value(items, &condition);

        if let Value::List(items) = result {
            assert_eq!(items.len(), 1);
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_filter_greater_than() {
        let items = Value::List(vec![
            Value::Record(vec![
                ("name".to_string(), Value::String("a".to_string())),
                ("val".to_string(), Value::Int(10)),
            ]),
            Value::Record(vec![
                ("name".to_string(), Value::String("b".to_string())),
                ("val".to_string(), Value::Int(20)),
            ]),
            Value::Record(vec![
                ("name".to_string(), Value::String("c".to_string())),
                ("val".to_string(), Value::Int(5)),
            ]),
        ]);

        let condition = Condition::GreaterThan("val".to_string(), 8.0);
        let result = filter_value(items, &condition);

        if let Value::List(items) = result {
            assert_eq!(items.len(), 2); // 10 and 20 are > 8
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_reduce_sum() {
        let items = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = reduce_value(items, "sum", None);
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn test_reduce_max() {
        let items = Value::List(vec![Value::Int(5), Value::Int(2), Value::Int(8)]);
        let result = reduce_value(items, "max", None);
        assert_eq!(result, Value::Int(8));
    }

    #[test]
    fn test_any() {
        let items = Value::List(vec![
            Value::Record(vec![("val".to_string(), Value::Int(5))]),
            Value::Record(vec![("val".to_string(), Value::Int(15))]),
        ]);

        let condition = Condition::GreaterThan("val".to_string(), 10.0);
        assert!(any_matches(&items, &condition));
    }

    #[test]
    fn test_all() {
        let items = Value::List(vec![
            Value::Record(vec![("val".to_string(), Value::Int(15))]),
            Value::Record(vec![("val".to_string(), Value::Int(20))]),
        ]);

        let condition = Condition::GreaterThan("val".to_string(), 10.0);
        assert!(all_match(&items, &condition));
    }
}
