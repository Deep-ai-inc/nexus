//! The `tr` command - translate or delete characters.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::collections::HashMap;

pub struct TrCommand;

struct TrOptions {
    set1: String,
    set2: Option<String>,
    delete: bool,
    squeeze: bool,
    complement: bool,
}

impl TrOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = TrOptions {
            set1: String::new(),
            set2: None,
            delete: false,
            squeeze: false,
            complement: false,
        };

        let mut positional = Vec::new();

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'd' => opts.delete = true,
                        's' => opts.squeeze = true,
                        'c' | 'C' => opts.complement = true,
                        _ => {}
                    }
                }
            } else {
                positional.push(arg.clone());
            }
        }

        if !positional.is_empty() {
            opts.set1 = expand_set(&positional[0]);
        }
        if positional.len() > 1 {
            opts.set2 = Some(expand_set(&positional[1]));
        }

        opts
    }
}

fn expand_set(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some(other) => result.push(other),
                None => result.push('\\'),
            }
        } else if chars.peek() == Some(&'-') {
            chars.next(); // consume '-'
            if let Some(end) = chars.next() {
                for ch in c..=end {
                    result.push(ch);
                }
            } else {
                result.push(c);
                result.push('-');
            }
        } else if s.starts_with("[:") && s.ends_with(":]") {
            // Character classes
            let class = &s[2..s.len() - 2];
            match class {
                "lower" => result.push_str("abcdefghijklmnopqrstuvwxyz"),
                "upper" => result.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
                "alpha" => {
                    result.push_str("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ")
                }
                "digit" => result.push_str("0123456789"),
                "alnum" => {
                    result.push_str("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
                }
                "space" => result.push_str(" \t\n\r"),
                "blank" => result.push_str(" \t"),
                _ => {}
            }
            return result;
        } else {
            result.push(c);
        }
    }

    result
}

impl NexusCommand for TrCommand {
    fn name(&self) -> &'static str {
        "tr"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = TrOptions::parse(args);

        if opts.set1.is_empty() {
            return Err(anyhow::anyhow!("Usage: tr [-dsc] SET1 [SET2]"));
        }

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(tr_value(stdin_value, &opts));
        }

        Ok(Value::Unit)
    }
}

fn tr_value(value: Value, opts: &TrOptions) -> Value {
    match value {
        Value::String(s) => Value::String(tr_string(&s, opts)),
        Value::List(items) => {
            Value::List(
                items
                    .into_iter()
                    .map(|item| {
                        let text = item.to_text();
                        Value::String(tr_string(&text, opts))
                    })
                    .collect(),
            )
        }
        other => {
            let text = other.to_text();
            Value::String(tr_string(&text, opts))
        }
    }
}

fn tr_string(s: &str, opts: &TrOptions) -> String {
    let set1_chars: Vec<char> = opts.set1.chars().collect();

    if opts.delete {
        // Delete mode
        let delete_set: std::collections::HashSet<char> = set1_chars.into_iter().collect();
        let result: String = if opts.complement {
            s.chars().filter(|c| delete_set.contains(c)).collect()
        } else {
            s.chars().filter(|c| !delete_set.contains(c)).collect()
        };

        if opts.squeeze {
            squeeze(&result, opts.set2.as_deref().unwrap_or(""))
        } else {
            result
        }
    } else if let Some(set2) = &opts.set2 {
        // Translate mode
        let set2_chars: Vec<char> = set2.chars().collect();
        let mut map: HashMap<char, char> = HashMap::new();

        for (i, &c1) in set1_chars.iter().enumerate() {
            let c2 = set2_chars.get(i).or(set2_chars.last()).copied().unwrap_or(c1);
            map.insert(c1, c2);
        }

        let result: String = if opts.complement {
            let set1_set: std::collections::HashSet<char> = set1_chars.into_iter().collect();
            let replacement = set2_chars.last().copied().unwrap_or(' ');
            s.chars()
                .map(|c| {
                    if set1_set.contains(&c) {
                        c
                    } else {
                        replacement
                    }
                })
                .collect()
        } else {
            s.chars().map(|c| *map.get(&c).unwrap_or(&c)).collect()
        };

        if opts.squeeze {
            squeeze(&result, set2)
        } else {
            result
        }
    } else if opts.squeeze {
        squeeze(s, &opts.set1)
    } else {
        s.to_string()
    }
}

fn squeeze(s: &str, set: &str) -> String {
    let squeeze_chars: std::collections::HashSet<char> = set.chars().collect();
    let mut result = String::new();
    let mut prev: Option<char> = None;

    for c in s.chars() {
        if squeeze_chars.contains(&c) && prev == Some(c) {
            continue;
        }
        result.push(c);
        prev = Some(c);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tr_lowercase_to_uppercase() {
        let opts = TrOptions {
            set1: "abcdefghijklmnopqrstuvwxyz".to_string(),
            set2: Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string()),
            delete: false,
            squeeze: false,
            complement: false,
        };
        let result = tr_string("hello", &opts);
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_tr_delete() {
        let opts = TrOptions {
            set1: "aeiou".to_string(),
            set2: None,
            delete: true,
            squeeze: false,
            complement: false,
        };
        let result = tr_string("hello world", &opts);
        assert_eq!(result, "hll wrld");
    }

    #[test]
    fn test_tr_squeeze() {
        let opts = TrOptions {
            set1: " ".to_string(),
            set2: None,
            delete: false,
            squeeze: true,
            complement: false,
        };
        let result = tr_string("hello   world", &opts);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_expand_range() {
        assert_eq!(expand_set("a-d"), "abcd");
        assert_eq!(expand_set("0-3"), "0123");
    }
}
