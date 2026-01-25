//! printf - formatted output command returning structured Value::String.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct PrintfCommand;

impl NexusCommand for PrintfCommand {
    fn name(&self) -> &'static str {
        "printf"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            anyhow::bail!("usage: printf format [arguments]");
        }

        let format = &args[0];
        let result = format_string(format, &args[1..])?;
        Ok(Value::String(result))
    }
}

/// Format a string according to printf format specifiers.
fn format_string(format: &str, args: &[String]) -> anyhow::Result<String> {
    let mut result = String::new();
    let mut chars = format.chars().peekable();
    let mut arg_idx = 0;

    while let Some(c) = chars.next() {
        if c == '\\' {
            // Handle escape sequences
            result.push(parse_escape(&mut chars));
        } else if c == '%' {
            // Handle format specifiers
            match chars.peek() {
                Some('%') => {
                    chars.next();
                    result.push('%');
                }
                Some(_) => {
                    let formatted = parse_format_specifier(&mut chars, args, &mut arg_idx)?;
                    result.push_str(&formatted);
                }
                None => result.push('%'),
            }
        } else {
            result.push(c);
        }
    }

    Ok(result)
}

/// Parse an escape sequence and return the resulting character.
fn parse_escape(chars: &mut std::iter::Peekable<std::str::Chars>) -> char {
    match chars.next() {
        Some('n') => '\n',
        Some('t') => '\t',
        Some('r') => '\r',
        Some('\\') => '\\',
        Some('"') => '"',
        Some('\'') => '\'',
        Some('a') => '\x07', // Bell
        Some('b') => '\x08', // Backspace
        Some('f') => '\x0C', // Form feed
        Some('v') => '\x0B', // Vertical tab
        Some('0') => {
            // Octal escape \0nnn
            let mut octal = String::new();
            for _ in 0..3 {
                if let Some(&c) = chars.peek() {
                    if c.is_digit(8) {
                        octal.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
            }
            if octal.is_empty() {
                '\0'
            } else {
                u8::from_str_radix(&octal, 8)
                    .map(|b| b as char)
                    .unwrap_or('\0')
            }
        }
        Some('x') => {
            // Hex escape \xHH
            let mut hex = String::new();
            for _ in 0..2 {
                if let Some(&c) = chars.peek() {
                    if c.is_ascii_hexdigit() {
                        hex.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
            }
            if hex.is_empty() {
                'x'
            } else {
                u8::from_str_radix(&hex, 16)
                    .map(|b| b as char)
                    .unwrap_or('\0')
            }
        }
        Some(c) => c, // Unknown escape, return as-is
        None => '\\',
    }
}

/// Parse a format specifier and return the formatted string.
fn parse_format_specifier(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    args: &[String],
    arg_idx: &mut usize,
) -> anyhow::Result<String> {
    // Parse flags
    let mut left_align = false;
    let mut sign_plus = false;
    let mut space_sign = false;
    let mut alt_form = false;
    let mut zero_pad = false;

    while let Some(&c) = chars.peek() {
        match c {
            '-' => {
                left_align = true;
                chars.next();
            }
            '+' => {
                sign_plus = true;
                chars.next();
            }
            ' ' => {
                space_sign = true;
                chars.next();
            }
            '#' => {
                alt_form = true;
                chars.next();
            }
            '0' => {
                zero_pad = true;
                chars.next();
            }
            _ => break,
        }
    }

    // Parse width
    let mut width = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            width.push(chars.next().unwrap());
        } else {
            break;
        }
    }
    let width: Option<usize> = if width.is_empty() {
        None
    } else {
        width.parse().ok()
    };

    // Parse precision
    let precision = if chars.peek() == Some(&'.') {
        chars.next(); // consume '.'
        let mut prec = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                prec.push(chars.next().unwrap());
            } else {
                break;
            }
        }
        if prec.is_empty() {
            Some(0)
        } else {
            prec.parse().ok()
        }
    } else {
        None
    };

    // Parse conversion specifier
    let conv = chars.next().ok_or_else(|| anyhow::anyhow!("missing format specifier"))?;
    let arg = args.get(*arg_idx).map(|s| s.as_str()).unwrap_or("");
    *arg_idx += 1;

    let formatted = match conv {
        's' => format_string_spec(arg, width, precision, left_align),
        'd' | 'i' => {
            let n: i64 = parse_number(arg);
            format_int(n, width, zero_pad, left_align, sign_plus, space_sign)
        }
        'u' => {
            let n: u64 = arg.parse().unwrap_or(0);
            format_uint(n, width, zero_pad, left_align, 10, false)
        }
        'o' => {
            let n: u64 = parse_number(arg) as u64;
            let prefix = if alt_form && n != 0 { "0" } else { "" };
            format_uint_with_prefix(n, width, zero_pad, left_align, 8, false, prefix)
        }
        'x' => {
            let n: u64 = parse_number(arg) as u64;
            let prefix = if alt_form && n != 0 { "0x" } else { "" };
            format_uint_with_prefix(n, width, zero_pad, left_align, 16, false, prefix)
        }
        'X' => {
            let n: u64 = parse_number(arg) as u64;
            let prefix = if alt_form && n != 0 { "0X" } else { "" };
            format_uint_with_prefix(n, width, zero_pad, left_align, 16, true, prefix)
        }
        'f' | 'F' => {
            let n: f64 = arg.parse().unwrap_or(0.0);
            format_float(n, width, precision.unwrap_or(6), zero_pad, left_align, sign_plus, space_sign)
        }
        'e' => {
            let n: f64 = arg.parse().unwrap_or(0.0);
            format_scientific(n, width, precision.unwrap_or(6), zero_pad, left_align, false)
        }
        'E' => {
            let n: f64 = arg.parse().unwrap_or(0.0);
            format_scientific(n, width, precision.unwrap_or(6), zero_pad, left_align, true)
        }
        'g' | 'G' => {
            let n: f64 = arg.parse().unwrap_or(0.0);
            let prec = precision.unwrap_or(6).max(1);
            // Use %e if exponent < -4 or >= precision, otherwise %f
            let exp = if n == 0.0 { 0 } else { n.abs().log10().floor() as i32 };
            if exp < -4 || exp >= prec as i32 {
                format_scientific(n, width, prec.saturating_sub(1), zero_pad, left_align, conv == 'G')
            } else {
                format_float(n, width, (prec as i32 - 1 - exp).max(0) as usize, zero_pad, left_align, sign_plus, space_sign)
            }
        }
        'c' => {
            let c = arg.chars().next().unwrap_or('\0');
            pad_string(&c.to_string(), width, left_align, ' ')
        }
        'b' => {
            // %b interprets escape sequences in the argument
            let mut result = String::new();
            let mut arg_chars = arg.chars().peekable();
            while let Some(c) = arg_chars.next() {
                if c == '\\' {
                    result.push(parse_escape(&mut arg_chars));
                } else {
                    result.push(c);
                }
            }
            result
        }
        'q' => {
            // %q quotes the argument for shell reuse
            shell_quote(arg)
        }
        _ => {
            // Unknown specifier, return as-is
            format!("%{}", conv)
        }
    };

    Ok(formatted)
}

/// Parse a number, handling 0x prefix for hex and 0 prefix for octal.
fn parse_number(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    // Handle negative sign
    let (negative, s) = if s.starts_with('-') {
        (true, &s[1..])
    } else if s.starts_with('+') {
        (false, &s[1..])
    } else {
        (false, s)
    };

    let n = if s.starts_with("0x") || s.starts_with("0X") {
        i64::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else if s.starts_with('0') && s.len() > 1 {
        i64::from_str_radix(&s[1..], 8).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    };

    if negative { -n } else { n }
}

/// Format a string with width and precision.
fn format_string_spec(s: &str, width: Option<usize>, precision: Option<usize>, left_align: bool) -> String {
    let s = if let Some(prec) = precision {
        &s[..s.len().min(prec)]
    } else {
        s
    };

    if let Some(w) = width {
        pad_string(s, Some(w), left_align, ' ')
    } else {
        s.to_string()
    }
}

/// Format a signed integer.
fn format_int(n: i64, width: Option<usize>, zero_pad: bool, left_align: bool, sign_plus: bool, space_sign: bool) -> String {
    let sign = if n < 0 {
        "-"
    } else if sign_plus {
        "+"
    } else if space_sign {
        " "
    } else {
        ""
    };

    let num_str = n.abs().to_string();
    let full = format!("{}{}", sign, num_str);

    if let Some(w) = width {
        if zero_pad && !left_align {
            let pad_len = w.saturating_sub(full.len());
            format!("{}{}{}", sign, "0".repeat(pad_len), num_str)
        } else {
            pad_string(&full, Some(w), left_align, ' ')
        }
    } else {
        full
    }
}

/// Format an unsigned integer.
fn format_uint(n: u64, width: Option<usize>, zero_pad: bool, left_align: bool, radix: u32, uppercase: bool) -> String {
    format_uint_with_prefix(n, width, zero_pad, left_align, radix, uppercase, "")
}

/// Format an unsigned integer with a prefix.
fn format_uint_with_prefix(n: u64, width: Option<usize>, zero_pad: bool, left_align: bool, radix: u32, uppercase: bool, prefix: &str) -> String {
    let num_str = match radix {
        8 => format!("{:o}", n),
        16 if uppercase => format!("{:X}", n),
        16 => format!("{:x}", n),
        _ => n.to_string(),
    };

    let full = format!("{}{}", prefix, num_str);

    if let Some(w) = width {
        if zero_pad && !left_align {
            let pad_len = w.saturating_sub(full.len());
            format!("{}{}{}", prefix, "0".repeat(pad_len), num_str)
        } else {
            pad_string(&full, Some(w), left_align, ' ')
        }
    } else {
        full
    }
}

/// Format a floating point number.
fn format_float(n: f64, width: Option<usize>, precision: usize, zero_pad: bool, left_align: bool, sign_plus: bool, space_sign: bool) -> String {
    let sign = if n.is_sign_negative() && !n.is_nan() {
        "-"
    } else if sign_plus {
        "+"
    } else if space_sign {
        " "
    } else {
        ""
    };

    let num_str = format!("{:.prec$}", n.abs(), prec = precision);
    let full = format!("{}{}", sign, num_str);

    if let Some(w) = width {
        if zero_pad && !left_align {
            let pad_len = w.saturating_sub(full.len());
            format!("{}{}{}", sign, "0".repeat(pad_len), num_str)
        } else {
            pad_string(&full, Some(w), left_align, ' ')
        }
    } else {
        full
    }
}

/// Format a number in scientific notation.
fn format_scientific(n: f64, width: Option<usize>, precision: usize, zero_pad: bool, left_align: bool, uppercase: bool) -> String {
    let formatted = if uppercase {
        format!("{:.prec$E}", n, prec = precision)
    } else {
        format!("{:.prec$e}", n, prec = precision)
    };

    if let Some(w) = width {
        if zero_pad && !left_align {
            // Find position after sign
            let sign_len = if n.is_sign_negative() { 1 } else { 0 };
            let pad_len = w.saturating_sub(formatted.len());
            let (sign, rest) = formatted.split_at(sign_len);
            format!("{}{}{}", sign, "0".repeat(pad_len), rest)
        } else {
            pad_string(&formatted, Some(w), left_align, ' ')
        }
    } else {
        formatted
    }
}

/// Pad a string to a given width.
fn pad_string(s: &str, width: Option<usize>, left_align: bool, pad_char: char) -> String {
    if let Some(w) = width {
        if s.len() >= w {
            s.to_string()
        } else if left_align {
            format!("{}{}", s, pad_char.to_string().repeat(w - s.len()))
        } else {
            format!("{}{}", pad_char.to_string().repeat(w - s.len()), s)
        }
    } else {
        s.to_string()
    }
}

/// Quote a string for shell reuse.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }

    // Check if quoting is needed
    let needs_quoting = s.chars().any(|c| {
        matches!(c, ' ' | '\t' | '\n' | '\'' | '"' | '\\' | '$' | '`' | '!' | '*' | '?' | '[' | ']' | '{' | '}' | '|' | '&' | ';' | '<' | '>' | '(' | ')' | '#' | '~')
    });

    if !needs_quoting {
        return s.to_string();
    }

    // Use single quotes, escaping single quotes as '\''
    let mut result = String::with_capacity(s.len() + 2);
    result.push('\'');
    for c in s.chars() {
        if c == '\'' {
            result.push_str("'\\''");
        } else {
            result.push(c);
        }
    }
    result.push('\'');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_string() {
        let result = format_string("hello %s", &["world".to_string()]).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_integer() {
        let result = format_string("%d", &["42".to_string()]).unwrap();
        assert_eq!(result, "42");
    }

    #[test]
    fn test_float() {
        let result = format_string("%.2f", &["3.14159".to_string()]).unwrap();
        assert_eq!(result, "3.14");
    }

    #[test]
    fn test_hex() {
        let result = format_string("%x", &["255".to_string()]).unwrap();
        assert_eq!(result, "ff");
    }

    #[test]
    fn test_escape() {
        let result = format_string("hello\\nworld", &[]).unwrap();
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_width() {
        let result = format_string("%10s", &["hi".to_string()]).unwrap();
        assert_eq!(result, "        hi");
    }

    #[test]
    fn test_left_align() {
        let result = format_string("%-10s", &["hi".to_string()]).unwrap();
        assert_eq!(result, "hi        ");
    }

    #[test]
    fn test_zero_pad() {
        let result = format_string("%05d", &["42".to_string()]).unwrap();
        assert_eq!(result, "00042");
    }
}
