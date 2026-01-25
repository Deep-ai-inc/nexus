//! Interactive table widget for structured data.
//!
//! Features:
//! - Clickable column headers for sorting
//! - Hover highlighting
//! - Right-click context menus (future)

use iced::widget::{button, container, text, Column, Row};
use iced::{Alignment, Background, Border, Color, Element, Padding};
use nexus_api::Value;

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
pub fn interactive_table<'a, Message>(
    columns: &[String],
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

    // Calculate column widths based on content
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.to_text().len());
            }
        }
    }

    // Build header row with clickable buttons
    let header_cells: Vec<Element<'a, Message>> = columns
        .iter()
        .enumerate()
        .map(|(i, col_name)| {
            // Add sort indicator
            let indicator = if sort.column == Some(i) {
                if sort.ascending { " ▲" } else { " ▼" }
            } else {
                ""
            };

            let label = format!("{}{}", col_name.to_uppercase(), indicator);
            let on_sort = on_sort.clone();

            button(
                text(label)
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
            })
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
                    let cell_text = cell.to_text();
                    let width = widths.get(col_idx).copied().unwrap_or(10);

                    // Right-align numbers, left-align text
                    let formatted = match cell {
                        Value::Int(_) | Value::Float(_) => format!("{:>width$}", cell_text, width = width),
                        _ => format!("{:<width$}", cell_text, width = width),
                    };

                    // Color based on cell type and content
                    let color = cell_color(cell, &formatted);

                    // Wrap in button if we have a click handler
                    if let Some(ref on_click) = on_cell_click {
                        let on_click = on_click.clone();
                        let cell_clone = cell.clone();
                        button(
                            text(formatted)
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
                        })
                        .into()
                    } else {
                        container(
                            text(formatted)
                                .size(font_size)
                                .color(color)
                                .font(iced::Font::MONOSPACE),
                        )
                        .padding(Padding::from([1, 4]))
                        .into()
                    }
                })
                .collect();

            Row::with_children(cells)
                .spacing(8)
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
    columns: &[String],
    rows: &[Vec<Value>],
    font_size: f32,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    // Calculate column widths
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.to_text().len());
            }
        }
    }

    // Build rows
    let lines: Vec<Element<'a, Message>> = rows
        .iter()
        .map(|row| {
            let mut line = String::new();
            for (i, cell) in row.iter().enumerate() {
                let cell_text = cell.to_text();
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

/// Compare two Values for sorting.
fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => {
            // Compare by type, then by value
            match (a, b) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
                (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal),
                (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal),
                (Value::String(a), Value::String(b)) => a.cmp(b),
                (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
                // For other types, compare string representation
                _ => a.to_text().cmp(&b.to_text()),
            }
        }
    }
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
