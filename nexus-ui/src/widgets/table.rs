//! Interactive table widget for structured data.
//!
//! Features:
//! - Clickable column headers for sorting
//! - Hover highlighting
//! - Right-click context menus (future)

use iced::widget::{button, container, text, Column, Row};
use iced::{Alignment, Background, Border, Color, Element, Padding};
use nexus_api::{format_value_for_display, TableColumn, Value};

use crate::constants::CHAR_WIDTH_RATIO;

/// Sort state for a table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableSort {
    /// Which column is being sorted (by index).
    pub column: Option<usize>,
    /// Sort direction (true = ascending, false = descending).
    pub ascending: bool,
}

impl TableSort {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle sort on a column. If already sorting by this column, reverse direction.
    /// If sorting by a different column, start ascending.
    pub fn toggle(&mut self, column_index: usize) {
        if self.column == Some(column_index) {
            self.ascending = !self.ascending;
        } else {
            self.column = Some(column_index);
            self.ascending = true;
        }
    }
}

/// Messages from the interactive table.
#[derive(Debug, Clone)]
pub enum TableMessage {
    /// User clicked a column header to sort.
    SortByColumn(usize),
    /// User right-clicked a cell (row, column).
    CellContextMenu(usize, usize),
    /// User clicked a cell (row, column).
    CellClicked(usize, usize),
}

/// Render an interactive table with sortable headers.
///
/// The table uses `TableColumn` which includes optional display format hints.
/// These hints control how values are rendered without changing the underlying data,
/// so sorting always works on the real value (e.g., bytes sort numerically even
/// when displayed as "202.8K").
pub fn interactive_table<'a, Message>(
    columns: &[TableColumn],
    rows: &[Vec<Value>],
    sort: &TableSort,
    font_size: f32,
    on_sort: impl Fn(usize) -> Message + 'a + Clone,
    on_cell_click: Option<impl Fn(usize, usize, &Value) -> Message + 'a + Clone>,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    // Sort rows if needed
    let sorted_rows = if let Some(col_idx) = sort.column {
        let mut rows: Vec<(usize, &Vec<Value>)> = rows.iter().enumerate().collect();
        rows.sort_by(|(_, a), (_, b)| {
            let a_val = a.get(col_idx);
            let b_val = b.get(col_idx);
            let cmp = compare_values(a_val, b_val);
            if sort.ascending { cmp } else { cmp.reverse() }
        });
        rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>()
    } else {
        rows.iter().collect::<Vec<_>>()
    };

    // Helper to get display text for a cell, applying format hint if present
    let get_display_text = |cell: &Value, col_idx: usize| -> String {
        if let Some(col) = columns.get(col_idx) {
            if let Some(format) = col.format {
                return format_value_for_display(cell, format);
            }
        }
        cell.to_text()
    };

    // Calculate column widths based on content (using formatted display)
    // For multi-line content, use the widest line
    let mut widths: Vec<usize> = columns.iter().map(|c| c.name.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                let text = get_display_text(cell, i);
                // Find the widest line for multi-line content
                let max_line_width = text.lines().map(|line| line.len()).max().unwrap_or(0);
                widths[i] = widths[i].max(max_line_width);
            }
        }
    }

    // Character width for monospace font - add buffer to prevent wrapping
    let char_width = font_size * CHAR_WIDTH_RATIO + 0.5;

    // Build header row with clickable buttons (using same pixel widths as data)
    let header_cells: Vec<Element<'a, Message>> = columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            // Add sort indicator
            let indicator = if sort.column == Some(i) {
                if sort.ascending { " ▲" } else { " ▼" }
            } else {
                ""
            };

            let col_width = widths.get(i).copied().unwrap_or(10);
            let pixel_width = (col_width as f32 * char_width) + 16.0; // padding buffer
            let header_text = format!("{}{}", col.name.to_uppercase(), indicator);
            let on_sort = on_sort.clone();

            container(
                button(
                    text(header_text)
                        .size(font_size)
                        .color(Color::from_rgb(0.6, 0.7, 0.9))
                        .font(iced::Font::MONOSPACE),
                )
                .on_press(on_sort(i))
                .padding(Padding::from([2, 4]))
                .style(|_theme, status| {
                    let background = match status {
                        button::Status::Hovered => Some(Background::Color(Color::from_rgba(0.3, 0.4, 0.6, 0.3))),
                        button::Status::Pressed => Some(Background::Color(Color::from_rgba(0.3, 0.4, 0.6, 0.5))),
                        _ => None,
                    };
                    button::Style {
                        background,
                        text_color: Color::from_rgb(0.6, 0.7, 0.9),
                        border: Border {
                            radius: 2.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            )
            .width(pixel_width)
            .into()
        })
        .collect();

    let header_row = Row::with_children(header_cells)
        .spacing(8)
        .align_y(Alignment::Center);

    // Build data rows
    let data_rows: Vec<Element<'a, Message>> = sorted_rows
        .iter()
        .enumerate()
        .map(|(row_idx, row)| {
            let cells: Vec<Element<'a, Message>> = row
                .iter()
                .enumerate()
                .map(|(col_idx, cell)| {
                    // Apply display format hint if present
                    let cell_text = get_display_text(cell, col_idx);
                    let col_width = widths.get(col_idx).copied().unwrap_or(10);
                    let pixel_width = (col_width as f32 * char_width) + 16.0; // padding buffer

                    // Color based on cell type and content
                    let color = cell_color(cell, &cell_text);

                    // Right-align numbers, left-align others
                    let h_align = match cell {
                        Value::Int(_) | Value::Float(_) => iced::alignment::Horizontal::Right,
                        _ => iced::alignment::Horizontal::Left,
                    };

                    // Wrap in button if we have a click handler
                    if let Some(ref on_click) = on_cell_click {
                        let on_click = on_click.clone();
                        let cell_clone = cell.clone();
                        container(
                            button(
                                text(cell_text)
                                    .size(font_size)
                                    .color(color)
                                    .font(iced::Font::MONOSPACE),
                            )
                            .on_press(on_click(row_idx, col_idx, &cell_clone))
                            .padding(Padding::from([1, 4]))
                            .style(move |_theme, status| {
                                let background = match status {
                                    button::Status::Hovered => Some(Background::Color(Color::from_rgba(0.5, 0.5, 0.5, 0.2))),
                                    _ => None,
                                };
                                button::Style {
                                    background,
                                    text_color: color,
                                    border: Border::default(),
                                    ..Default::default()
                                }
                            }),
                        )
                        .width(pixel_width)
                        .align_x(h_align)
                        .into()
                    } else {
                        container(
                            text(cell_text)
                                .size(font_size)
                                .color(color)
                                .font(iced::Font::MONOSPACE),
                        )
                        .width(pixel_width)
                        .padding(Padding::from([1, 4]))
                        .align_x(h_align)
                        .into()
                    }
                })
                .collect();

            Row::with_children(cells)
                .spacing(8)
                .align_y(Alignment::Start) // Top-align cells for variable row heights
                .into()
        })
        .collect();

    // Combine header and data
    let mut all_rows: Vec<Element<'a, Message>> = Vec::with_capacity(data_rows.len() + 1);
    all_rows.push(header_row.into());
    all_rows.extend(data_rows);

    Column::with_children(all_rows)
        .spacing(2)
        .into()
}

/// Render a simple (non-interactive) table for when we don't need sorting.
pub fn simple_table<'a, Message>(
    columns: &[TableColumn],
    rows: &[Vec<Value>],
    font_size: f32,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    // Helper to get display text with format hint
    let get_display_text = |cell: &Value, col_idx: usize| -> String {
        if let Some(col) = columns.get(col_idx) {
            if let Some(format) = col.format {
                return format_value_for_display(cell, format);
            }
        }
        cell.to_text()
    };

    // Calculate column widths
    let mut widths: Vec<usize> = columns.iter().map(|c| c.name.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(get_display_text(cell, i).len());
            }
        }
    }

    // Build rows
    let lines: Vec<Element<'a, Message>> = rows
        .iter()
        .map(|row| {
            let mut line = String::new();
            for (i, cell) in row.iter().enumerate() {
                let cell_text = get_display_text(cell, i);
                let width = widths.get(i).copied().unwrap_or(0);

                let formatted = match cell {
                    Value::Int(_) | Value::Float(_) => format!("{:>width$}", cell_text, width = width),
                    _ => format!("{:<width$}", cell_text, width = width),
                };
                line.push_str(&formatted);
                line.push_str("  ");
            }

            let color = row_color(&line);

            text(line.trim_end().to_string())
                .size(font_size)
                .color(color)
                .font(iced::Font::MONOSPACE)
                .into()
        })
        .collect();

    Column::with_children(lines).spacing(0).into()
}

/// Compare two Values for sorting - type-aware with smart string handling.
fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => compare_typed_values(a, b),
    }
}

/// Type-aware value comparison leveraging rich types.
fn compare_typed_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        // Numeric types - compare numerically
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal),

        // Strings - try numeric first, then natural sort
        (Value::String(a), Value::String(b)) => smart_string_cmp(a, b),

        // Booleans
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),

        // FileEntry - natural sort by name
        (Value::FileEntry(a), Value::FileEntry(b)) => natural_cmp(&a.name, &b.name),

        // Process - sort by PID
        (Value::Process(a), Value::Process(b)) => a.pid.cmp(&b.pid),

        // GitCommit - sort by date (most recent first makes sense as default)
        (Value::GitCommit(a), Value::GitCommit(b)) => b.date.cmp(&a.date),

        // Path - natural sort
        (Value::Path(a), Value::Path(b)) => natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()),

        // Cross-type: try to compare numerically if both look like numbers
        (Value::String(s), Value::Int(i)) => {
            if let Ok(n) = s.trim().parse::<i64>() {
                n.cmp(i)
            } else {
                Ordering::Greater // non-numeric strings after numbers
            }
        }
        (Value::Int(i), Value::String(s)) => {
            if let Ok(n) = s.trim().parse::<i64>() {
                i.cmp(&n)
            } else {
                Ordering::Less // numbers before non-numeric strings
            }
        }

        // Fallback: natural sort on text representation
        _ => natural_cmp(&a.to_text(), &b.to_text()),
    }
}

/// Smart string comparison: if both strings are pure numbers, compare numerically.
/// Otherwise use natural sort which handles embedded numbers correctly.
fn smart_string_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    // Try parsing both as numbers first (handles "10" vs "2" correctly)
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
        _ => natural_cmp(a, b),
    }
}

/// Natural sort comparison - handles embedded numbers correctly.
/// "file2" < "file10", "v1.9" < "v1.10", etc.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        match (a_chars.peek(), b_chars.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ac), Some(&bc)) => {
                // Both have digits - compare as numbers
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let a_num = collect_number(&mut a_chars);
                    let b_num = collect_number(&mut b_chars);
                    match a_num.cmp(&b_num) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
                // Compare characters (case-insensitive, then case-sensitive for ties)
                let ac_lower = ac.to_ascii_lowercase();
                let bc_lower = bc.to_ascii_lowercase();
                match ac_lower.cmp(&bc_lower) {
                    Ordering::Equal => {
                        // Same letter - check case, then continue
                        if ac != bc {
                            return ac.cmp(&bc); // uppercase before lowercase
                        }
                        a_chars.next();
                        b_chars.next();
                    }
                    other => return other,
                }
            }
        }
    }
}

/// Collect consecutive digits into a number for natural sort comparison.
fn collect_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> u64 {
    let mut num: u64 = 0;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            num = num.saturating_mul(10).saturating_add((c as u64) - ('0' as u64));
            chars.next();
        } else {
            break;
        }
    }
    num
}

/// Get color for a cell based on its value.
fn cell_color(value: &Value, formatted: &str) -> Color {
    match value {
        Value::Path(_) => Color::from_rgb(0.4, 0.6, 1.0), // Blue for paths
        Value::Bool(true) => Color::from_rgb(0.4, 0.9, 0.4), // Green for true
        Value::Bool(false) => Color::from_rgb(0.9, 0.4, 0.4), // Red for false
        Value::Error { .. } => Color::from_rgb(0.9, 0.3, 0.3), // Red for errors
        Value::String(s) if s.starts_with('/') => Color::from_rgb(0.4, 0.6, 1.0), // Blue for paths
        Value::String(s) if s.starts_with("http") => Color::from_rgb(0.4, 0.8, 0.9), // Cyan for URLs
        _ => {
            // Check formatted string for file-type coloring
            if formatted.starts_with('d') {
                Color::from_rgb(0.4, 0.6, 1.0) // Directory
            } else if formatted.contains(" -> ") {
                Color::from_rgb(0.4, 0.9, 0.9) // Symlink
            } else {
                Color::from_rgb(0.8, 0.8, 0.8) // Default
            }
        }
    }
}

/// Get color for a row based on its content (for simple_table).
fn row_color(line: &str) -> Color {
    if line.starts_with('d') {
        Color::from_rgb(0.4, 0.6, 1.0) // Blue for directories
    } else if line.contains(" -> ") {
        Color::from_rgb(0.4, 0.9, 0.9) // Cyan for symlinks
    } else if line.contains('x') {
        Color::from_rgb(0.4, 0.9, 0.4) // Green for executables
    } else {
        Color::from_rgb(0.8, 0.8, 0.8) // Default
    }
}
