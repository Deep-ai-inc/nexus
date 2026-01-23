//! Stream sniffing - detect output format from content.

use nexus_api::OutputFormat;

/// Detected format with confidence level.
#[derive(Debug, Clone)]
pub struct Format {
    pub kind: OutputFormat,
    pub confidence: Confidence,
}

/// Confidence level of format detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// Magic bytes for binary format detection.
const MAGIC_PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const MAGIC_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];
const MAGIC_GIF: &[u8] = b"GIF8";
const MAGIC_PDF: &[u8] = b"%PDF";
const MAGIC_GZIP: &[u8] = &[0x1F, 0x8B];
const MAGIC_ZIP: &[u8] = &[0x50, 0x4B, 0x03, 0x04];
const MAGIC_ELF: &[u8] = &[0x7F, 0x45, 0x4C, 0x46];

/// Detect the format of the given data.
pub fn detect_format(data: &[u8]) -> Format {
    if data.is_empty() {
        return Format {
            kind: OutputFormat::PlainText,
            confidence: Confidence::Low,
        };
    }

    // Check for binary formats via magic bytes
    if let Some(format) = detect_binary_magic(data) {
        return format;
    }

    // Check for ANSI escape codes
    if contains_ansi_escapes(data) {
        return Format {
            kind: OutputFormat::AnsiText,
            confidence: Confidence::High,
        };
    }

    // Try to detect structured text formats
    if let Some(format) = detect_structured_text(data) {
        return format;
    }

    // Default to plain text
    Format {
        kind: OutputFormat::PlainText,
        confidence: Confidence::Medium,
    }
}

/// Check for binary format magic bytes.
fn detect_binary_magic(data: &[u8]) -> Option<Format> {
    if data.starts_with(MAGIC_PNG)
        || data.starts_with(MAGIC_JPEG)
        || data.starts_with(MAGIC_GIF)
    {
        return Some(Format {
            kind: OutputFormat::Binary,
            confidence: Confidence::High,
        });
    }

    if data.starts_with(MAGIC_PDF) {
        return Some(Format {
            kind: OutputFormat::Binary,
            confidence: Confidence::High,
        });
    }

    if data.starts_with(MAGIC_GZIP)
        || data.starts_with(MAGIC_ZIP)
        || data.starts_with(MAGIC_ELF)
    {
        return Some(Format {
            kind: OutputFormat::Binary,
            confidence: Confidence::High,
        });
    }

    // Check for non-text bytes
    let non_text_count = data
        .iter()
        .take(512)
        .filter(|&&b| b < 0x09 || (b > 0x0D && b < 0x20 && b != 0x1B))
        .count();

    if non_text_count > data.len().min(512) / 10 {
        return Some(Format {
            kind: OutputFormat::Binary,
            confidence: Confidence::Medium,
        });
    }

    None
}

/// Check if data contains ANSI escape sequences.
fn contains_ansi_escapes(data: &[u8]) -> bool {
    // Look for ESC [ sequences
    data.windows(2).any(|w| w == [0x1B, b'['])
}

/// Try to detect structured text formats (JSON, CSV, etc.).
fn detect_structured_text(data: &[u8]) -> Option<Format> {
    let text = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return None,
    };

    let trimmed = text.trim_start();

    // JSON detection
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if is_valid_json_start(trimmed) {
            // Check if it's JSON Lines (multiple JSON objects)
            let lines: Vec<&str> = trimmed.lines().take(5).collect();
            if lines.len() > 1 && lines.iter().all(|l| {
                let t = l.trim();
                t.is_empty() || t.starts_with('{') || t.starts_with('[')
            }) {
                return Some(Format {
                    kind: OutputFormat::JsonLines,
                    confidence: Confidence::Medium,
                });
            }

            return Some(Format {
                kind: OutputFormat::Json,
                confidence: Confidence::High,
            });
        }
    }

    // XML detection
    if trimmed.starts_with("<?xml") || trimmed.starts_with('<') {
        if trimmed.contains("?>") || (trimmed.contains('<') && trimmed.contains('>')) {
            return Some(Format {
                kind: OutputFormat::Xml,
                confidence: Confidence::Medium,
            });
        }
    }

    // CSV/TSV detection - look at delimiter frequency in first few lines
    let lines: Vec<&str> = trimmed.lines().take(5).collect();
    if lines.len() >= 2 {
        if let Some(format) = detect_delimited(&lines) {
            return Some(format);
        }
    }

    None
}

/// Check if a string looks like valid JSON start.
fn is_valid_json_start(s: &str) -> bool {
    // Simple heuristic: count matching braces/brackets in first 1KB
    let sample = &s[..s.len().min(1024)];

    let mut brace_count = 0i32;
    let mut bracket_count = 0i32;

    for c in sample.chars() {
        match c {
            '{' => brace_count += 1,
            '}' => brace_count -= 1,
            '[' => bracket_count += 1,
            ']' => bracket_count -= 1,
            _ => {}
        }

        // Invalid if we go negative
        if brace_count < 0 || bracket_count < 0 {
            return false;
        }
    }

    // Should have at least some structure
    brace_count > 0 || bracket_count > 0 || sample.contains(':')
}

/// Detect CSV or TSV based on delimiter frequency.
fn detect_delimited(lines: &[&str]) -> Option<Format> {
    if lines.len() < 2 {
        return None;
    }

    // Count delimiters per line
    let comma_counts: Vec<usize> = lines.iter().map(|l| l.matches(',').count()).collect();
    let tab_counts: Vec<usize> = lines.iter().map(|l| l.matches('\t').count()).collect();

    // Check for consistent comma counts
    if comma_counts.iter().all(|&c| c > 0)
        && comma_counts.iter().all(|&c| c == comma_counts[0])
    {
        return Some(Format {
            kind: OutputFormat::Csv,
            confidence: Confidence::Medium,
        });
    }

    // Check for consistent tab counts
    if tab_counts.iter().all(|&c| c > 0)
        && tab_counts.iter().all(|&c| c == tab_counts[0])
    {
        return Some(Format {
            kind: OutputFormat::Tsv,
            confidence: Confidence::Medium,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_detection() {
        let format = detect_format(b"{\"name\": \"test\", \"value\": 42}");
        assert_eq!(format.kind, OutputFormat::Json);
    }

    #[test]
    fn test_ansi_detection() {
        let format = detect_format(b"\x1b[31mred text\x1b[0m");
        assert_eq!(format.kind, OutputFormat::AnsiText);
    }

    #[test]
    fn test_binary_detection() {
        let format = detect_format(MAGIC_PNG);
        assert_eq!(format.kind, OutputFormat::Binary);
    }

    #[test]
    fn test_csv_detection() {
        let format = detect_format(b"name,age,city\nalice,30,nyc\nbob,25,la\n");
        assert_eq!(format.kind, OutputFormat::Csv);
    }
}
