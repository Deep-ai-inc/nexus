//! Word expansion - variable substitution, command substitution, etc.
//!
//! Expansion order (POSIX):
//! 1. Brace expansion (bash extension)
//! 2. Tilde expansion
//! 3. Parameter/variable expansion
//! 4. Command substitution
//! 5. Arithmetic expansion
//! 6. Word splitting
//! 7. Pathname expansion (globbing)

use crate::parser::Word;
use crate::ShellState;

/// Expand a word to a string (no glob expansion).
pub fn expand_word_to_string(word: &Word, state: &ShellState) -> String {
    match word {
        Word::Literal(s) => expand_literal(s, state),
        Word::Variable(name) => expand_variable(name, state),
        Word::CommandSubstitution(cmd) => expand_command_substitution(cmd, state),
    }
}

/// Expand a word to potentially multiple strings (with brace and glob expansion).
///
/// This is the full expansion including:
/// - Brace expansion: `{a,b,c}` → `a b c`, `{1..5}` → `1 2 3 4 5`
/// - Pathname expansion (globbing): `*.txt` → matching files
///
/// Use this for command arguments.
pub fn expand_word_to_strings(word: &Word, state: &ShellState) -> Vec<String> {
    let expanded = expand_word_to_string(word, state);

    // Step 1: Brace expansion (before glob expansion)
    let brace_expanded = expand_braces(&expanded);

    // Step 2: Glob expansion on each brace-expanded word
    let mut results = Vec::new();

    for word in brace_expanded {
        // If noglob is set, don't expand globs
        if state.options.noglob {
            results.push(word);
            continue;
        }

        // Check if the expanded string contains glob metacharacters
        if !contains_glob_chars(&word) {
            results.push(word);
            continue;
        }

        // Perform pathname expansion
        let matches = expand_glob(&word, state);

        if matches.is_empty() {
            // No matches - return the original pattern (POSIX behavior)
            results.push(word);
        } else {
            results.extend(matches);
        }
    }

    results
}

// ============================================================================
// Brace Expansion
// ============================================================================

/// Expand brace expressions like `{a,b,c}` and `{1..10}`.
///
/// Examples:
/// - `{a,b,c}` → `["a", "b", "c"]`
/// - `{1..5}` → `["1", "2", "3", "4", "5"]`
/// - `{1..10..2}` → `["1", "3", "5", "7", "9"]`
/// - `{a..e}` → `["a", "b", "c", "d", "e"]`
/// - `file{1,2}.txt` → `["file1.txt", "file2.txt"]`
/// - `{a,b}{1,2}` → `["a1", "a2", "b1", "b2"]`
pub fn expand_braces(s: &str) -> Vec<String> {
    // Find the first brace expression (respecting nesting)
    let Some((start, end, content)) = find_brace_expr(s) else {
        return vec![s.to_string()];
    };

    let preamble = &s[..start];
    let postscript = &s[end + 1..];

    // Parse the brace content
    let alternatives = parse_brace_content(content);

    // Generate expansions
    let mut results = Vec::new();
    for alt in alternatives {
        let expanded = format!("{}{}{}", preamble, alt, postscript);
        // Recursively expand remaining braces
        results.extend(expand_braces(&expanded));
    }

    results
}

/// Find the first top-level brace expression in a string.
/// Returns (start_index, end_index, content_between_braces).
fn find_brace_expr(s: &str) -> Option<(usize, usize, &str)> {
    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut start_idx = None;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    start_idx = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start_idx {
                        let content = &s[start + 1..i];
                        // Only treat as brace expansion if it contains , or ..
                        if content.contains(',') || content.contains("..") {
                            return Some((start, i, content));
                        }
                        // Otherwise, not a brace expansion - continue looking
                        start_idx = None;
                    }
                }
            }
            b'\\' => {
                // Skip escaped character (simplified - doesn't handle all cases)
            }
            _ => {}
        }
    }

    None
}

/// Parse the content of a brace expression.
/// Handles both comma-separated lists and range expressions.
fn parse_brace_content(content: &str) -> Vec<String> {
    // Check for range expression: {1..10} or {a..z} or {1..10..2}
    if let Some(range_result) = try_parse_range(content) {
        return range_result;
    }

    // Parse comma-separated alternatives (respecting nested braces)
    parse_comma_list(content)
}

/// Try to parse a range expression like `1..10` or `a..z` or `1..10..2`.
fn try_parse_range(content: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = content.split("..").collect();

    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }

    let start_str = parts[0].trim();
    let end_str = parts[1].trim();
    let step_str = parts.get(2).map(|s| s.trim());

    // Try numeric range
    if let (Ok(start), Ok(end)) = (start_str.parse::<i64>(), end_str.parse::<i64>()) {
        let step: i64 = step_str
            .and_then(|s| s.parse().ok())
            .unwrap_or(if start <= end { 1 } else { -1 });

        if step == 0 {
            return Some(vec![content.to_string()]);
        }

        let mut results = Vec::new();
        let mut current = start;

        if step > 0 {
            while current <= end {
                results.push(current.to_string());
                current += step;
            }
        } else {
            while current >= end {
                results.push(current.to_string());
                current += step;
            }
        }

        return Some(results);
    }

    // Try character range (single characters only)
    let start_chars: Vec<char> = start_str.chars().collect();
    let end_chars: Vec<char> = end_str.chars().collect();

    if start_chars.len() == 1 && end_chars.len() == 1 {
        let start_char = start_chars[0];
        let end_char = end_chars[0];

        // Only expand if both are letters or both are digits
        let valid = (start_char.is_ascii_lowercase() && end_char.is_ascii_lowercase())
            || (start_char.is_ascii_uppercase() && end_char.is_ascii_uppercase())
            || (start_char.is_ascii_digit() && end_char.is_ascii_digit());

        if valid {
            let step: i32 = step_str
                .and_then(|s| s.parse().ok())
                .unwrap_or(if start_char <= end_char { 1 } else { -1 });

            if step == 0 {
                return Some(vec![content.to_string()]);
            }

            let mut results = Vec::new();
            let mut current = start_char as i32;
            let end_val = end_char as i32;

            if step > 0 {
                while current <= end_val {
                    if let Some(c) = char::from_u32(current as u32) {
                        results.push(c.to_string());
                    }
                    current += step;
                }
            } else {
                while current >= end_val {
                    if let Some(c) = char::from_u32(current as u32) {
                        results.push(c.to_string());
                    }
                    current += step;
                }
            }

            return Some(results);
        }
    }

    None
}

/// Parse a comma-separated list, respecting nested braces.
fn parse_comma_list(content: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for c in content.chars() {
        match c {
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                results.push(current);
                current = String::new();
            }
            _ => {
                current.push(c);
            }
        }
    }

    // Don't forget the last element
    results.push(current);

    results
}

// ============================================================================
// Arithmetic Expansion
// ============================================================================

/// Collect an arithmetic expression until matching )).
fn collect_arithmetic_expr(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut expr = String::new();
    let mut depth = 1; // We've already consumed the opening ((

    while let Some(c) = chars.next() {
        if c == ')' {
            if chars.peek() == Some(&')') && depth == 1 {
                chars.next(); // consume the second )
                break;
            }
            depth -= 1;
            if depth > 0 {
                expr.push(c);
            }
        } else if c == '(' {
            depth += 1;
            expr.push(c);
        } else {
            expr.push(c);
        }
    }

    expr
}

/// Evaluate an arithmetic expression.
///
/// Supports:
/// - Integers: 42, -17
/// - Variables: x, $x, ${x}
/// - Operators: + - * / % ** (power)
/// - Comparison: < > <= >= == !=
/// - Logical: && || !
/// - Bitwise: & | ^ ~ << >>
/// - Ternary: a ? b : c
/// - Parentheses for grouping
/// - Assignment: x = 5, x += 1
pub fn evaluate_arithmetic(expr: &str, state: &ShellState) -> i64 {
    let expr = expr.trim();
    if expr.is_empty() {
        return 0;
    }

    // Tokenize
    let tokens = tokenize_arithmetic(expr, state);

    // Parse and evaluate using a simple recursive descent parser
    let mut parser = ArithParser::new(tokens);
    parser.parse_expr()
}

/// Arithmetic token types.
#[derive(Debug, Clone, PartialEq)]
enum ArithToken {
    Num(i64),
    Op(String),
    LParen,
    RParen,
}

/// Tokenize an arithmetic expression.
fn tokenize_arithmetic(expr: &str, state: &ShellState) -> Vec<ArithToken> {
    let mut tokens = Vec::new();
    let mut chars = expr.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '0'..='9' => {
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(ArithToken::Num(num.parse().unwrap_or(0)));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // Look up variable value
                let val = state
                    .get_var(&name)
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);
                tokens.push(ArithToken::Num(val));
            }
            '$' => {
                chars.next();
                // Handle $var or ${var}
                if chars.peek() == Some(&'{') {
                    chars.next();
                    let mut name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '}' {
                            chars.next();
                            break;
                        }
                        name.push(c);
                        chars.next();
                    }
                    let val = state
                        .get_var(&name)
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    tokens.push(ArithToken::Num(val));
                } else {
                    let mut name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_alphanumeric() || c == '_' {
                            name.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let val = state
                        .get_var(&name)
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    tokens.push(ArithToken::Num(val));
                }
            }
            '(' => {
                chars.next();
                tokens.push(ArithToken::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(ArithToken::RParen);
            }
            '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '~' | '<' | '>' | '=' | '!' | '?' | ':' => {
                let mut op = String::new();
                op.push(c);
                chars.next();

                // Check for two-character operators
                if let Some(&next) = chars.peek() {
                    let two_char = format!("{}{}", c, next);
                    if matches!(
                        two_char.as_str(),
                        "**" | "<<" | ">>" | "<=" | ">=" | "==" | "!=" | "&&" | "||" | "+=" | "-=" | "*=" | "/="
                    ) {
                        op.push(next);
                        chars.next();
                    }
                }
                tokens.push(ArithToken::Op(op));
            }
            _ => {
                chars.next(); // Skip unknown characters
            }
        }
    }

    tokens
}

/// Simple recursive descent parser for arithmetic expressions.
struct ArithParser {
    tokens: Vec<ArithToken>,
    pos: usize,
}

impl ArithParser {
    fn new(tokens: Vec<ArithToken>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&ArithToken> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<ArithToken> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse_expr(&mut self) -> i64 {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> i64 {
        let cond = self.parse_logical_or();

        if matches!(self.peek(), Some(ArithToken::Op(op)) if op == "?") {
            self.next(); // consume ?
            let then_val = self.parse_expr();
            if matches!(self.peek(), Some(ArithToken::Op(op)) if op == ":") {
                self.next(); // consume :
            }
            let else_val = self.parse_expr();
            if cond != 0 { then_val } else { else_val }
        } else {
            cond
        }
    }

    fn parse_logical_or(&mut self) -> i64 {
        let mut left = self.parse_logical_and();

        while matches!(self.peek(), Some(ArithToken::Op(op)) if op == "||") {
            self.next();
            let right = self.parse_logical_and();
            left = if left != 0 || right != 0 { 1 } else { 0 };
        }

        left
    }

    fn parse_logical_and(&mut self) -> i64 {
        let mut left = self.parse_bitwise_or();

        while matches!(self.peek(), Some(ArithToken::Op(op)) if op == "&&") {
            self.next();
            let right = self.parse_bitwise_or();
            left = if left != 0 && right != 0 { 1 } else { 0 };
        }

        left
    }

    fn parse_bitwise_or(&mut self) -> i64 {
        let mut left = self.parse_bitwise_xor();

        while matches!(self.peek(), Some(ArithToken::Op(op)) if op == "|" && !matches!(self.tokens.get(self.pos + 1), Some(ArithToken::Op(o)) if o == "|")) {
            self.next();
            let right = self.parse_bitwise_xor();
            left |= right;
        }

        left
    }

    fn parse_bitwise_xor(&mut self) -> i64 {
        let mut left = self.parse_bitwise_and();

        while matches!(self.peek(), Some(ArithToken::Op(op)) if op == "^") {
            self.next();
            let right = self.parse_bitwise_and();
            left ^= right;
        }

        left
    }

    fn parse_bitwise_and(&mut self) -> i64 {
        let mut left = self.parse_equality();

        while matches!(self.peek(), Some(ArithToken::Op(op)) if op == "&" && !matches!(self.tokens.get(self.pos + 1), Some(ArithToken::Op(o)) if o == "&")) {
            self.next();
            let right = self.parse_equality();
            left &= right;
        }

        left
    }

    fn parse_equality(&mut self) -> i64 {
        let mut left = self.parse_comparison();

        loop {
            match self.peek() {
                Some(ArithToken::Op(op)) if op == "==" => {
                    self.next();
                    let right = self.parse_comparison();
                    left = if left == right { 1 } else { 0 };
                }
                Some(ArithToken::Op(op)) if op == "!=" => {
                    self.next();
                    let right = self.parse_comparison();
                    left = if left != right { 1 } else { 0 };
                }
                _ => break,
            }
        }

        left
    }

    fn parse_comparison(&mut self) -> i64 {
        let mut left = self.parse_shift();

        loop {
            match self.peek() {
                Some(ArithToken::Op(op)) if op == "<" => {
                    self.next();
                    let right = self.parse_shift();
                    left = if left < right { 1 } else { 0 };
                }
                Some(ArithToken::Op(op)) if op == ">" => {
                    self.next();
                    let right = self.parse_shift();
                    left = if left > right { 1 } else { 0 };
                }
                Some(ArithToken::Op(op)) if op == "<=" => {
                    self.next();
                    let right = self.parse_shift();
                    left = if left <= right { 1 } else { 0 };
                }
                Some(ArithToken::Op(op)) if op == ">=" => {
                    self.next();
                    let right = self.parse_shift();
                    left = if left >= right { 1 } else { 0 };
                }
                _ => break,
            }
        }

        left
    }

    fn parse_shift(&mut self) -> i64 {
        let mut left = self.parse_additive();

        loop {
            match self.peek() {
                Some(ArithToken::Op(op)) if op == "<<" => {
                    self.next();
                    let right = self.parse_additive();
                    left <<= right;
                }
                Some(ArithToken::Op(op)) if op == ">>" => {
                    self.next();
                    let right = self.parse_additive();
                    left >>= right;
                }
                _ => break,
            }
        }

        left
    }

    fn parse_additive(&mut self) -> i64 {
        let mut left = self.parse_multiplicative();

        loop {
            match self.peek() {
                Some(ArithToken::Op(op)) if op == "+" => {
                    self.next();
                    let right = self.parse_multiplicative();
                    left += right;
                }
                Some(ArithToken::Op(op)) if op == "-" => {
                    self.next();
                    let right = self.parse_multiplicative();
                    left -= right;
                }
                _ => break,
            }
        }

        left
    }

    fn parse_multiplicative(&mut self) -> i64 {
        let mut left = self.parse_power();

        loop {
            match self.peek() {
                Some(ArithToken::Op(op)) if op == "*" && !matches!(self.tokens.get(self.pos + 1), Some(ArithToken::Op(o)) if o == "*") => {
                    self.next();
                    let right = self.parse_power();
                    left *= right;
                }
                Some(ArithToken::Op(op)) if op == "/" => {
                    self.next();
                    let right = self.parse_power();
                    if right != 0 {
                        left /= right;
                    } else {
                        left = 0; // Division by zero
                    }
                }
                Some(ArithToken::Op(op)) if op == "%" => {
                    self.next();
                    let right = self.parse_power();
                    if right != 0 {
                        left %= right;
                    } else {
                        left = 0;
                    }
                }
                _ => break,
            }
        }

        left
    }

    fn parse_power(&mut self) -> i64 {
        let base = self.parse_unary();

        if matches!(self.peek(), Some(ArithToken::Op(op)) if op == "**") {
            self.next();
            let exp = self.parse_power(); // Right associative
            base.pow(exp as u32)
        } else {
            base
        }
    }

    fn parse_unary(&mut self) -> i64 {
        match self.peek() {
            Some(ArithToken::Op(op)) if op == "-" => {
                self.next();
                -self.parse_unary()
            }
            Some(ArithToken::Op(op)) if op == "+" => {
                self.next();
                self.parse_unary()
            }
            Some(ArithToken::Op(op)) if op == "!" => {
                self.next();
                if self.parse_unary() == 0 { 1 } else { 0 }
            }
            Some(ArithToken::Op(op)) if op == "~" => {
                self.next();
                !self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> i64 {
        match self.peek() {
            Some(ArithToken::Num(_)) => {
                if let Some(ArithToken::Num(n)) = self.next() {
                    n
                } else {
                    0
                }
            }
            Some(ArithToken::LParen) => {
                self.next(); // consume (
                let val = self.parse_expr();
                if matches!(self.peek(), Some(ArithToken::RParen)) {
                    self.next(); // consume )
                }
                val
            }
            _ => 0,
        }
    }
}

/// Check if a string contains glob metacharacters.
fn contains_glob_chars(s: &str) -> bool {
    // Don't treat * or ? inside quotes as globs
    // This is a simplification - proper handling would track quote state
    s.chars().any(|c| c == '*' || c == '?' || c == '[')
}

/// Expand a glob pattern to matching paths.
fn expand_glob(pattern: &str, state: &ShellState) -> Vec<String> {
    // Split pattern into directory part and filename part
    let (dir_part, file_pattern) = if let Some(pos) = pattern.rfind('/') {
        let dir = &pattern[..=pos];
        let file = &pattern[pos + 1..];
        (dir.to_string(), file.to_string())
    } else {
        (".".to_string(), pattern.to_string())
    };

    // Resolve the directory relative to cwd
    let search_dir = if dir_part == "." {
        state.cwd.clone()
    } else if dir_part.starts_with('/') {
        std::path::PathBuf::from(&dir_part)
    } else {
        state.cwd.join(&dir_part)
    };

    // If the directory doesn't exist, no matches
    if !search_dir.is_dir() {
        return vec![];
    }

    // Read directory and match entries
    let mut matches: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless pattern starts with .
            if name.starts_with('.') && !file_pattern.starts_with('.') {
                continue;
            }

            if glob_match(&file_pattern, &name) {
                let full_path = if dir_part == "." {
                    name
                } else {
                    format!("{}{}", dir_part, name)
                };
                matches.push(full_path);
            }
        }
    }

    // Sort matches alphabetically (POSIX requirement)
    matches.sort();
    matches
}

/// Match a filename against a glob pattern.
///
/// Supports:
/// - `*` matches any sequence of characters
/// - `?` matches any single character
/// - `[abc]` matches any character in the set
/// - `[a-z]` matches any character in the range
/// - `[!abc]` or `[^abc]` matches any character not in the set
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    glob_match_impl(&pattern_chars, &name_chars, 0, 0)
}

fn glob_match_impl(pattern: &[char], name: &[char], mut pi: usize, mut ni: usize) -> bool {
    while pi < pattern.len() || ni < name.len() {
        if pi < pattern.len() {
            match pattern[pi] {
                '*' => {
                    // Try matching zero or more characters
                    // First, skip consecutive *'s
                    while pi < pattern.len() && pattern[pi] == '*' {
                        pi += 1;
                    }
                    // If * is at end of pattern, it matches everything
                    if pi == pattern.len() {
                        return true;
                    }
                    // Try matching * against increasingly longer substrings
                    while ni <= name.len() {
                        if glob_match_impl(pattern, name, pi, ni) {
                            return true;
                        }
                        ni += 1;
                    }
                    return false;
                }
                '?' => {
                    // Must have a character to match
                    if ni >= name.len() {
                        return false;
                    }
                    pi += 1;
                    ni += 1;
                }
                '[' => {
                    // Character class
                    if ni >= name.len() {
                        return false;
                    }
                    let (matched, new_pi) = match_char_class(&pattern[pi..], name[ni]);
                    if !matched {
                        return false;
                    }
                    pi += new_pi;
                    ni += 1;
                }
                c => {
                    // Literal character
                    if ni >= name.len() || name[ni] != c {
                        return false;
                    }
                    pi += 1;
                    ni += 1;
                }
            }
        } else {
            // Pattern exhausted but name has more characters
            return false;
        }
    }
    true
}

/// Match a character class like [abc] or [a-z] or [!abc].
/// Returns (matched, chars_consumed_from_pattern).
fn match_char_class(pattern: &[char], c: char) -> (bool, usize) {
    if pattern.is_empty() || pattern[0] != '[' {
        return (false, 0);
    }

    let mut i = 1;
    let negated = if i < pattern.len() && (pattern[i] == '!' || pattern[i] == '^') {
        i += 1;
        true
    } else {
        false
    };

    let mut matched = false;

    while i < pattern.len() && pattern[i] != ']' {
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            // Range like a-z
            let start = pattern[i];
            let end = pattern[i + 2];
            if c >= start && c <= end {
                matched = true;
            }
            i += 3;
        } else {
            // Single character
            if pattern[i] == c {
                matched = true;
            }
            i += 1;
        }
    }

    // Skip closing ]
    if i < pattern.len() && pattern[i] == ']' {
        i += 1;
    }

    let result = if negated { !matched } else { matched };
    (result, i)
}

/// Expand a literal string, handling embedded variables and escapes.
fn expand_literal(s: &str, state: &ShellState) -> String {
    // Handle tilde expansion at the start of the word
    let s = expand_tilde(s, state);

    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Escape sequence
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            '$' => {
                // Check for arithmetic expansion $((expr))
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume first '('
                    if chars.peek() == Some(&'(') {
                        chars.next(); // consume second '('
                        // Find matching ))
                        let expr = collect_arithmetic_expr(&mut chars);
                        let value = evaluate_arithmetic(&expr, state);
                        result.push_str(&value.to_string());
                        continue;
                    } else {
                        // Command substitution $(cmd) - collect until matching )
                        let mut depth = 1;
                        let mut cmd = String::new();
                        while let Some(ch) = chars.next() {
                            if ch == '(' {
                                depth += 1;
                                cmd.push(ch);
                            } else if ch == ')' {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                                cmd.push(ch);
                            } else {
                                cmd.push(ch);
                            }
                        }
                        result.push_str(&expand_command_substitution(&format!("$({})", cmd), state));
                        continue;
                    }
                }

                // Variable expansion
                if chars.peek() == Some(&'{') {
                    // ${var} form
                    chars.next(); // consume '{'
                    let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
                    result.push_str(&expand_variable(&var_name, state));
                } else {
                    // $var form
                    let var_name: String = chars
                        .by_ref()
                        .take_while(|&c| c.is_alphanumeric() || c == '_')
                        .collect();
                    if var_name.is_empty() {
                        result.push('$');
                    } else {
                        result.push_str(&expand_variable(&var_name, state));
                    }
                }
            }
            '\'' => {
                // Single quotes - literal, no expansion
                let quoted: String = chars.by_ref().take_while(|&c| c != '\'').collect();
                result.push_str(&quoted);
            }
            '"' => {
                // Double quotes - expand variables but not globs
                let mut quoted = String::new();
                while let Some(c) = chars.next() {
                    if c == '"' {
                        break;
                    } else if c == '\\' {
                        if let Some(next) = chars.next() {
                            quoted.push(next);
                        }
                    } else if c == '$' {
                        if chars.peek() == Some(&'{') {
                            chars.next();
                            let var_name: String =
                                chars.by_ref().take_while(|&c| c != '}').collect();
                            quoted.push_str(&expand_variable(&var_name, state));
                        } else {
                            let var_name: String = chars
                                .by_ref()
                                .take_while(|&c| c.is_alphanumeric() || c == '_')
                                .collect();
                            if var_name.is_empty() {
                                quoted.push('$');
                            } else {
                                quoted.push_str(&expand_variable(&var_name, state));
                            }
                        }
                    } else {
                        quoted.push(c);
                    }
                }
                result.push_str(&quoted);
            }
            _ => result.push(c),
        }
    }

    result
}

/// Expand a variable reference.
fn expand_variable(name: &str, state: &ShellState) -> String {
    // Handle special variables
    match name {
        "?" => state.last_exit_code.to_string(),
        "$" => std::process::id().to_string(),
        "!" => state
            .last_bg_pid
            .map(|p| p.to_string())
            .unwrap_or_default(),
        "0" => "nexus".to_string(),
        "PWD" => state.cwd.to_string_lossy().to_string(),
        "HOME" => state
            .get_env("HOME")
            .unwrap_or_default()
            .to_string(),
        _ => {
            // Check for parameter expansion modifiers
            if let Some((var, modifier)) = parse_parameter_expansion(name) {
                apply_parameter_expansion(&var, &modifier, state)
            } else {
                state.get_var(name).unwrap_or_default().to_string()
            }
        }
    }
}

/// Parse parameter expansion like ${var:-default}.
fn parse_parameter_expansion(name: &str) -> Option<(String, String)> {
    for pattern in &[":-", ":=", ":+", ":?", "-", "=", "+", "?", "#", "##", "%", "%%"] {
        if let Some(pos) = name.find(pattern) {
            let var = name[..pos].to_string();
            let modifier = name[pos..].to_string();
            return Some((var, modifier));
        }
    }
    None
}

/// Apply parameter expansion modifiers.
fn apply_parameter_expansion(var: &str, modifier: &str, state: &ShellState) -> String {
    let value = state.get_var(var);

    if modifier.starts_with(":-") {
        // Use default if unset or null
        let default = &modifier[2..];
        match value {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => default.to_string(),
        }
    } else if modifier.starts_with("-") {
        // Use default if unset
        let default = &modifier[1..];
        value.map(|v| v.to_string()).unwrap_or_else(|| default.to_string())
    } else if modifier.starts_with(":=") {
        // Assign default if unset or null
        // Note: We can't actually assign here without &mut state
        let default = &modifier[2..];
        match value {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => default.to_string(),
        }
    } else if modifier.starts_with(":+") {
        // Use alternative if set and non-null
        let alt = &modifier[2..];
        match value {
            Some(v) if !v.is_empty() => alt.to_string(),
            _ => String::new(),
        }
    } else if modifier.starts_with("+") {
        // Use alternative if set
        let alt = &modifier[1..];
        value.map(|_| alt.to_string()).unwrap_or_default()
    } else if modifier.starts_with("#") {
        // Remove shortest prefix pattern
        let pattern = &modifier[1..];
        value
            .map(|v| remove_prefix(v, pattern, false))
            .unwrap_or_default()
    } else if modifier.starts_with("##") {
        // Remove longest prefix pattern
        let pattern = &modifier[2..];
        value
            .map(|v| remove_prefix(v, pattern, true))
            .unwrap_or_default()
    } else if modifier.starts_with("%%") {
        // Remove longest suffix pattern
        let pattern = &modifier[2..];
        value
            .map(|v| remove_suffix(v, pattern, true))
            .unwrap_or_default()
    } else if modifier.starts_with("%") {
        // Remove shortest suffix pattern
        let pattern = &modifier[1..];
        value
            .map(|v| remove_suffix(v, pattern, false))
            .unwrap_or_default()
    } else {
        value.unwrap_or_default().to_string()
    }
}

/// Remove prefix matching pattern.
fn remove_prefix(s: &str, pattern: &str, longest: bool) -> String {
    // Simplified glob matching - just handle * at the end
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        if s.starts_with(prefix) {
            if longest {
                // Find longest match
                for i in (prefix.len()..=s.len()).rev() {
                    if s[..i].starts_with(prefix) {
                        return s[i..].to_string();
                    }
                }
            }
            return s[prefix.len()..].to_string();
        }
    } else if s.starts_with(pattern) {
        return s[pattern.len()..].to_string();
    }
    s.to_string()
}

/// Remove suffix matching pattern.
fn remove_suffix(s: &str, pattern: &str, longest: bool) -> String {
    // Simplified glob matching - just handle * at the beginning
    if pattern.starts_with('*') {
        let suffix = &pattern[1..];
        if s.ends_with(suffix) {
            if longest {
                // Find longest match
                for i in 0..=(s.len() - suffix.len()) {
                    if s[i..].ends_with(suffix) {
                        return s[..i].to_string();
                    }
                }
            }
            return s[..s.len() - suffix.len()].to_string();
        }
    } else if s.ends_with(pattern) {
        return s[..s.len() - pattern.len()].to_string();
    }
    s.to_string()
}

/// Expand tilde at the start of a word.
///
/// Supports:
/// - `~` or `~/path` → $HOME or $HOME/path
/// - `~+` or `~+/path` → $PWD or $PWD/path
/// - `~-` or `~-/path` → $OLDPWD or $OLDPWD/path
fn expand_tilde(s: &str, state: &ShellState) -> String {
    if !s.starts_with('~') {
        return s.to_string();
    }

    // Check what follows the tilde
    let rest = &s[1..];

    // ~+ (PWD)
    if rest.is_empty() || rest.starts_with('/') {
        // Plain ~ or ~/path
        let home = state.get_env("HOME").unwrap_or_default();
        if rest.is_empty() {
            return home.to_string();
        } else {
            return format!("{}{}", home, rest);
        }
    }

    if rest == "+" || rest.starts_with("+/") {
        // ~+ or ~+/path (PWD)
        let pwd = state.cwd.to_string_lossy();
        if rest == "+" {
            return pwd.to_string();
        } else {
            return format!("{}{}", pwd, &rest[1..]);
        }
    }

    if rest == "-" || rest.starts_with("-/") {
        // ~- or ~-/path (OLDPWD)
        let oldpwd = state.get_env("OLDPWD").unwrap_or_default();
        if rest == "-" {
            return oldpwd.to_string();
        } else {
            return format!("{}{}", oldpwd, &rest[1..]);
        }
    }

    // ~user form - look up user's home directory
    let (username, path_rest) = if let Some(slash_pos) = rest.find('/') {
        (&rest[..slash_pos], &rest[slash_pos..])
    } else {
        (rest, "")
    };

    // Try to get user's home directory
    if let Some(home) = get_user_home(username) {
        format!("{}{}", home, path_rest)
    } else {
        // User not found, return unchanged
        s.to_string()
    }
}

/// Get a user's home directory.
#[cfg(unix)]
fn get_user_home(username: &str) -> Option<String> {
    use nix::libc;
    use std::ffi::CString;

    let c_username = CString::new(username).ok()?;

    unsafe {
        let pwd = libc::getpwnam(c_username.as_ptr());
        if pwd.is_null() {
            return None;
        }
        let home = (*pwd).pw_dir;
        if home.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(home)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(not(unix))]
fn get_user_home(_username: &str) -> Option<String> {
    None
}

/// Expand command substitution $(cmd) or `cmd`.
fn expand_command_substitution(cmd: &str, _state: &ShellState) -> String {
    // Strip $() or ``
    let inner = cmd
        .trim_start_matches("$(")
        .trim_end_matches(')')
        .trim_start_matches('`')
        .trim_end_matches('`');

    // Execute the command and capture output
    // For now, use std::process::Command as a simple implementation
    match std::process::Command::new("sh")
        .arg("-c")
        .arg(inner)
        .output()
    {
        Ok(output) => {
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_string()
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_state() -> ShellState {
        let mut state = ShellState::from_cwd(PathBuf::from("/tmp"));
        state.set_env("HOME".to_string(), "/home/testuser".to_string());
        state.set_env("OLDPWD".to_string(), "/old/path".to_string());
        state
    }

    #[test]
    fn test_tilde_home() {
        let state = make_state();
        assert_eq!(expand_tilde("~", &state), "/home/testuser");
    }

    #[test]
    fn test_tilde_home_path() {
        let state = make_state();
        assert_eq!(expand_tilde("~/projects", &state), "/home/testuser/projects");
        assert_eq!(
            expand_tilde("~/a/b/c", &state),
            "/home/testuser/a/b/c"
        );
    }

    #[test]
    fn test_tilde_pwd() {
        let state = make_state();
        assert_eq!(expand_tilde("~+", &state), "/tmp");
        assert_eq!(expand_tilde("~+/foo", &state), "/tmp/foo");
    }

    #[test]
    fn test_tilde_oldpwd() {
        let state = make_state();
        assert_eq!(expand_tilde("~-", &state), "/old/path");
        assert_eq!(expand_tilde("~-/bar", &state), "/old/path/bar");
    }

    #[test]
    fn test_tilde_no_expansion() {
        let state = make_state();
        // No tilde at start
        assert_eq!(expand_tilde("foo~bar", &state), "foo~bar");
        assert_eq!(expand_tilde("/path/to/~", &state), "/path/to/~");
    }

    #[test]
    fn test_tilde_in_literal() {
        let state = make_state();
        // Full expansion through expand_literal
        assert_eq!(expand_literal("~", &state), "/home/testuser");
        assert_eq!(expand_literal("~/bin", &state), "/home/testuser/bin");
    }

    #[test]
    fn test_expand_variable_basic() {
        let mut state = make_state();
        state.set_var("FOO".to_string(), "bar".to_string());
        assert_eq!(expand_variable("FOO", &state), "bar");
    }

    #[test]
    fn test_expand_variable_default() {
        let state = make_state();
        // ${var:-default}
        assert_eq!(
            expand_variable("UNDEFINED:-fallback", &state),
            "fallback"
        );
    }

    #[test]
    fn test_expand_special_vars() {
        let state = make_state();
        assert_eq!(expand_variable("0", &state), "nexus");
        assert_eq!(expand_variable("PWD", &state), "/tmp");
        assert_eq!(expand_variable("HOME", &state), "/home/testuser");
    }

    // Glob matching tests
    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("*.rs", "foo.rs"));
        assert!(glob_match("*.rs", "bar.rs"));
        assert!(!glob_match("*.rs", "foo.txt"));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("**", "anything"));
    }

    #[test]
    fn test_glob_match_question() {
        assert!(glob_match("f?o", "foo"));
        assert!(glob_match("f??", "foo"));
        assert!(!glob_match("f?o", "fooo"));
        assert!(!glob_match("f?", "foo"));
    }

    #[test]
    fn test_glob_match_char_class() {
        assert!(glob_match("[abc]", "a"));
        assert!(glob_match("[abc]", "b"));
        assert!(!glob_match("[abc]", "d"));
        assert!(glob_match("[a-z]", "m"));
        assert!(!glob_match("[a-z]", "M"));
        assert!(glob_match("[!abc]", "d"));
        assert!(!glob_match("[!abc]", "a"));
    }

    #[test]
    fn test_glob_match_combined() {
        assert!(glob_match("*.t?t", "file.txt"));
        assert!(glob_match("*.t?t", "file.tst"));
        assert!(!glob_match("*.t?t", "file.text"));
        assert!(glob_match("[a-z]*.rs", "foo.rs"));
        assert!(!glob_match("[a-z]*.rs", "123.rs"));
    }

    #[test]
    fn test_contains_glob_chars() {
        assert!(contains_glob_chars("*.rs"));
        assert!(contains_glob_chars("file?.txt"));
        assert!(contains_glob_chars("[abc]"));
        assert!(!contains_glob_chars("file.txt"));
        assert!(!contains_glob_chars("path/to/file"));
    }

    #[test]
    fn test_expand_glob_no_glob() {
        let state = make_state();
        // No glob chars - should return as-is
        let result = expand_word_to_strings(&Word::Literal("file.txt".to_string()), &state);
        assert_eq!(result, vec!["file.txt"]);
    }

    #[test]
    fn test_expand_glob_noglob_option() {
        let mut state = make_state();
        state.options.noglob = true;
        // With noglob set, should return pattern as-is
        let result = expand_word_to_strings(&Word::Literal("*.txt".to_string()), &state);
        assert_eq!(result, vec!["*.txt"]);
    }

    // ========================================================================
    // Brace expansion tests
    // ========================================================================

    #[test]
    fn test_brace_comma_simple() {
        assert_eq!(expand_braces("{a,b,c}"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_brace_comma_with_preamble() {
        assert_eq!(expand_braces("file{1,2,3}.txt"), vec!["file1.txt", "file2.txt", "file3.txt"]);
    }

    #[test]
    fn test_brace_comma_multiple() {
        assert_eq!(expand_braces("{a,b}{1,2}"), vec!["a1", "a2", "b1", "b2"]);
    }

    #[test]
    fn test_brace_numeric_range() {
        assert_eq!(expand_braces("{1..5}"), vec!["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn test_brace_numeric_range_reverse() {
        assert_eq!(expand_braces("{5..1}"), vec!["5", "4", "3", "2", "1"]);
    }

    #[test]
    fn test_brace_numeric_range_step() {
        assert_eq!(expand_braces("{1..10..2}"), vec!["1", "3", "5", "7", "9"]);
    }

    #[test]
    fn test_brace_alpha_range() {
        assert_eq!(expand_braces("{a..e}"), vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_brace_alpha_range_upper() {
        assert_eq!(expand_braces("{A..E}"), vec!["A", "B", "C", "D", "E"]);
    }

    #[test]
    fn test_brace_alpha_range_reverse() {
        assert_eq!(expand_braces("{e..a}"), vec!["e", "d", "c", "b", "a"]);
    }

    #[test]
    fn test_brace_nested() {
        assert_eq!(expand_braces("{a,b{1,2}}"), vec!["a", "b1", "b2"]);
    }

    #[test]
    fn test_brace_no_expansion() {
        // No comma or range - not a brace expansion
        assert_eq!(expand_braces("{foo}"), vec!["{foo}"]);
    }

    #[test]
    fn test_brace_mkdir_pattern() {
        assert_eq!(
            expand_braces("src/{components,utils,hooks}"),
            vec!["src/components", "src/utils", "src/hooks"]
        );
    }

    #[test]
    fn test_brace_mv_pattern() {
        assert_eq!(expand_braces("app.{js,ts}"), vec!["app.js", "app.ts"]);
    }

    // ========================================================================
    // Arithmetic expansion tests
    // ========================================================================

    #[test]
    fn test_arithmetic_basic() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("1 + 2", &state), 3);
        assert_eq!(evaluate_arithmetic("10 - 3", &state), 7);
        assert_eq!(evaluate_arithmetic("4 * 5", &state), 20);
        assert_eq!(evaluate_arithmetic("20 / 4", &state), 5);
        assert_eq!(evaluate_arithmetic("17 % 5", &state), 2);
    }

    #[test]
    fn test_arithmetic_power() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("2 ** 10", &state), 1024);
        assert_eq!(evaluate_arithmetic("3 ** 3", &state), 27);
    }

    #[test]
    fn test_arithmetic_precedence() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("2 + 3 * 4", &state), 14);
        assert_eq!(evaluate_arithmetic("(2 + 3) * 4", &state), 20);
        assert_eq!(evaluate_arithmetic("2 ** 3 ** 2", &state), 512); // Right associative: 2^(3^2) = 2^9
    }

    #[test]
    fn test_arithmetic_comparison() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("5 > 3", &state), 1);
        assert_eq!(evaluate_arithmetic("5 < 3", &state), 0);
        assert_eq!(evaluate_arithmetic("5 == 5", &state), 1);
        assert_eq!(evaluate_arithmetic("5 != 3", &state), 1);
        assert_eq!(evaluate_arithmetic("5 >= 5", &state), 1);
        assert_eq!(evaluate_arithmetic("5 <= 4", &state), 0);
    }

    #[test]
    fn test_arithmetic_logical() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("1 && 1", &state), 1);
        assert_eq!(evaluate_arithmetic("1 && 0", &state), 0);
        assert_eq!(evaluate_arithmetic("0 || 1", &state), 1);
        assert_eq!(evaluate_arithmetic("0 || 0", &state), 0);
        assert_eq!(evaluate_arithmetic("!0", &state), 1);
        assert_eq!(evaluate_arithmetic("!1", &state), 0);
    }

    #[test]
    fn test_arithmetic_bitwise() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("5 & 3", &state), 1);
        assert_eq!(evaluate_arithmetic("5 | 3", &state), 7);
        assert_eq!(evaluate_arithmetic("5 ^ 3", &state), 6);
        assert_eq!(evaluate_arithmetic("1 << 4", &state), 16);
        assert_eq!(evaluate_arithmetic("16 >> 2", &state), 4);
    }

    #[test]
    fn test_arithmetic_ternary() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("1 ? 10 : 20", &state), 10);
        assert_eq!(evaluate_arithmetic("0 ? 10 : 20", &state), 20);
        assert_eq!(evaluate_arithmetic("5 > 3 ? 100 : 200", &state), 100);
    }

    #[test]
    fn test_arithmetic_unary() {
        let state = make_state();
        assert_eq!(evaluate_arithmetic("-5", &state), -5);
        assert_eq!(evaluate_arithmetic("--5", &state), 5);
        assert_eq!(evaluate_arithmetic("~0", &state), -1);
    }

    #[test]
    fn test_arithmetic_variables() {
        let mut state = make_state();
        state.set_var("x".to_string(), "10".to_string());
        state.set_var("y".to_string(), "3".to_string());

        assert_eq!(evaluate_arithmetic("x + y", &state), 13);
        assert_eq!(evaluate_arithmetic("x * y", &state), 30);
        assert_eq!(evaluate_arithmetic("$x + $y", &state), 13);
    }

    #[test]
    fn test_arithmetic_in_literal() {
        let mut state = make_state();
        state.set_var("n".to_string(), "5".to_string());

        let result = expand_literal("Result: $((2 + 3))", &state);
        assert_eq!(result, "Result: 5");

        let result = expand_literal("$((n * 2))", &state);
        assert_eq!(result, "10");
    }
}
