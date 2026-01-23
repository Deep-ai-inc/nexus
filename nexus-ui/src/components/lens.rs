//! Lenses - different views for command output.

use nexus_api::OutputFormat;

/// Trait for output lenses.
pub trait Lens {
    /// Get the name of this lens.
    fn name(&self) -> &'static str;

    /// Check if this lens can handle the given format.
    fn can_handle(&self, format: OutputFormat) -> bool;

    /// Render the data. Returns rendered lines for display.
    fn render(&self, data: &[u8]) -> Vec<String>;
}

/// Raw text lens - displays output as-is.
pub struct RawLens;

impl Lens for RawLens {
    fn name(&self) -> &'static str {
        "Raw"
    }

    fn can_handle(&self, _format: OutputFormat) -> bool {
        true // Can handle anything
    }

    fn render(&self, data: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(data)
            .lines()
            .map(|s| s.to_string())
            .collect()
    }
}

/// JSON lens - displays JSON with syntax highlighting and tree view.
pub struct JsonLens {
    /// Whether to show as tree or formatted text.
    tree_view: bool,

    /// Indentation level.
    indent: usize,
}

impl JsonLens {
    pub fn new() -> Self {
        Self {
            tree_view: false,
            indent: 2,
        }
    }

    pub fn with_tree_view(mut self, enabled: bool) -> Self {
        self.tree_view = enabled;
        self
    }
}

impl Default for JsonLens {
    fn default() -> Self {
        Self::new()
    }
}

impl Lens for JsonLens {
    fn name(&self) -> &'static str {
        "JSON"
    }

    fn can_handle(&self, format: OutputFormat) -> bool {
        matches!(format, OutputFormat::Json | OutputFormat::JsonLines)
    }

    fn render(&self, data: &[u8]) -> Vec<String> {
        let text = match std::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => return vec!["[Invalid UTF-8]".to_string()],
        };

        // Try to parse and pretty-print
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => {
                let formatted = serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|_| text.to_string());
                formatted.lines().map(|s| s.to_string()).collect()
            }
            Err(_) => {
                // Maybe it's JSON Lines?
                let mut lines = Vec::new();
                for line in text.lines() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                        let formatted = serde_json::to_string_pretty(&value)
                            .unwrap_or_else(|_| line.to_string());
                        lines.extend(formatted.lines().map(|s| s.to_string()));
                        lines.push(String::new()); // Separator
                    } else {
                        lines.push(line.to_string());
                    }
                }
                lines
            }
        }
    }
}

/// Table lens - displays CSV/TSV as a table.
pub struct TableLens;

impl Lens for TableLens {
    fn name(&self) -> &'static str {
        "Table"
    }

    fn can_handle(&self, format: OutputFormat) -> bool {
        matches!(format, OutputFormat::Csv | OutputFormat::Tsv)
    }

    fn render(&self, data: &[u8]) -> Vec<String> {
        let text = match std::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => return vec!["[Invalid UTF-8]".to_string()],
        };

        // Detect delimiter
        let delimiter = if text.lines().next().map(|l| l.contains('\t')).unwrap_or(false) {
            '\t'
        } else {
            ','
        };

        // Parse rows
        let rows: Vec<Vec<&str>> = text
            .lines()
            .map(|line| line.split(delimiter).collect())
            .collect();

        if rows.is_empty() {
            return vec![];
        }

        // Calculate column widths
        let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut col_widths = vec![0usize; col_count];

        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }

        // Render table
        let mut lines = Vec::new();

        // Header separator
        let separator: String = col_widths
            .iter()
            .map(|&w| "-".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("+");

        for (i, row) in rows.iter().enumerate() {
            let line: String = row
                .iter()
                .enumerate()
                .map(|(j, cell)| {
                    let width = col_widths.get(j).copied().unwrap_or(0);
                    format!(" {:width$} ", cell, width = width)
                })
                .collect::<Vec<_>>()
                .join("|");

            lines.push(format!("|{}|", line));

            // Add separator after header
            if i == 0 {
                lines.push(format!("+{}+", separator));
            }
        }

        lines
    }
}

/// Hex lens - displays binary data as hex dump.
pub struct HexLens {
    /// Bytes per line.
    bytes_per_line: usize,
}

impl HexLens {
    pub fn new() -> Self {
        Self { bytes_per_line: 16 }
    }
}

impl Default for HexLens {
    fn default() -> Self {
        Self::new()
    }
}

impl Lens for HexLens {
    fn name(&self) -> &'static str {
        "Hex"
    }

    fn can_handle(&self, format: OutputFormat) -> bool {
        matches!(format, OutputFormat::Binary)
    }

    fn render(&self, data: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();

        for (i, chunk) in data.chunks(self.bytes_per_line).enumerate() {
            let offset = i * self.bytes_per_line;

            // Hex part
            let hex: String = chunk
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");

            // ASCII part
            let ascii: String = chunk
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();

            // Pad hex part if needed
            let hex_width = self.bytes_per_line * 3 - 1;
            lines.push(format!(
                "{:08x}  {:hex_width$}  |{}|",
                offset,
                hex,
                ascii,
                hex_width = hex_width
            ));
        }

        lines
    }
}
