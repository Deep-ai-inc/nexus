//! Rendering structured values (tables, media, file lists).

use iced::widget::{column, text, Column};
use iced::{Element, Length};

use nexus_api::{BlockId, FileEntry, FileType, Value};

use crate::msg::{Message, TerminalMessage};
use crate::utils::format_file_size;
use crate::widgets::table::{interactive_table, TableSort};

/// Render a structured Value from a native command.
pub fn render_value<'a>(
    value: &'a Value,
    block_id: BlockId,
    table_sort: &'a TableSort,
    font_size: f32,
) -> Element<'a, Message> {
    match value {
        Value::Unit => column![].into(),

        Value::List(items) => {
            // Check if it's a list of FileEntry
            let file_entries: Vec<&FileEntry> = items
                .iter()
                .filter_map(|v| match v {
                    Value::FileEntry(entry) => Some(entry.as_ref()),
                    _ => None,
                })
                .collect();

            if file_entries.len() == items.len() && !file_entries.is_empty() {
                // Render as file list
                render_file_list(&file_entries, font_size)
            } else {
                // Generic list rendering
                let lines: Vec<Element<Message>> = items
                    .iter()
                    .map(|item| {
                        text(item.to_text())
                            .size(font_size)
                            .color(iced::Color::from_rgb(0.8, 0.8, 0.8))
                            .font(iced::Font::MONOSPACE)
                            .into()
                    })
                    .collect();
                Column::with_children(lines).spacing(0).into()
            }
        }

        Value::Table { columns, rows } => {
            // Use interactive table with sortable headers
            interactive_table(
                columns,
                rows,
                table_sort,
                font_size,
                move |col_idx| Message::Terminal(TerminalMessage::TableSort(block_id, col_idx)),
                None::<fn(usize, usize, &Value) -> Message>,
            )
        }

        Value::FileEntry(entry) => render_file_list(&[entry.as_ref()], font_size),

        Value::Media {
            data,
            content_type,
            metadata,
        } => render_media(data, content_type, metadata, font_size),

        // For other types, just render as text
        _ => text(value.to_text())
            .size(font_size)
            .color(iced::Color::from_rgb(0.8, 0.8, 0.8))
            .font(iced::Font::MONOSPACE)
            .into(),
    }
}

/// Render media content (images, audio, video, documents).
pub fn render_media<'a>(
    data: &'a [u8],
    content_type: &'a str,
    metadata: &'a nexus_api::MediaMetadata,
    font_size: f32,
) -> Element<'a, Message> {
    use iced::widget::image;

    // Images: render inline
    if content_type.starts_with("image/") {
        let handle = image::Handle::from_bytes(data.to_vec());

        // Determine size - use metadata if available, otherwise default max
        let (width, height) = match (metadata.width, metadata.height) {
            (Some(w), Some(h)) => {
                // Scale down if too large, max 600px width
                let max_width = 600.0;
                let max_height = 400.0;
                let scale = (max_width / w as f32).min(max_height / h as f32).min(1.0);
                ((w as f32 * scale) as u16, (h as f32 * scale) as u16)
            }
            _ => (400, 300), // Default size if dimensions unknown
        };

        let img = image::Image::new(handle)
            .width(Length::Fixed(width as f32))
            .height(Length::Fixed(height as f32));

        let label = if let Some(name) = &metadata.filename {
            format!("{} ({})", name, content_type)
        } else {
            content_type.to_string()
        };

        column![
            img,
            text(label)
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(4)
        .into()
    }
    // Audio: show info placeholder (actual player would need more work)
    else if content_type.starts_with("audio/") {
        let duration = metadata
            .duration_secs
            .map(|d| format!(" ({:.1}s)", d))
            .unwrap_or_default();
        let name = metadata.filename.as_deref().unwrap_or("audio");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("🔊 {}{}", name, duration))
                .size(font_size)
                .color(iced::Color::from_rgb(0.5, 0.8, 0.5)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
    // Video: show info placeholder
    else if content_type.starts_with("video/") {
        let duration = metadata
            .duration_secs
            .map(|d| format!(" ({:.1}s)", d))
            .unwrap_or_default();
        let dims = match (metadata.width, metadata.height) {
            (Some(w), Some(h)) => format!(" {}x{}", w, h),
            _ => String::new(),
        };
        let name = metadata.filename.as_deref().unwrap_or("video");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("🎬 {}{}{}", name, dims, duration))
                .size(font_size)
                .color(iced::Color::from_rgb(0.5, 0.7, 0.9)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
    // PDF and other documents
    else if content_type == "application/pdf" {
        let name = metadata.filename.as_deref().unwrap_or("document.pdf");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("📄 {}", name))
                .size(font_size)
                .color(iced::Color::from_rgb(0.9, 0.6, 0.5)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
    // Generic binary: show type and size
    else {
        let name = metadata.filename.as_deref().unwrap_or("file");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("📎 {}", name))
                .size(font_size)
                .color(iced::Color::from_rgb(0.7, 0.7, 0.7)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
}

/// Render a list of file entries (simple ls-style output).
pub fn render_file_list(entries: &[&FileEntry], font_size: f32) -> Element<'static, Message> {
    let lines: Vec<Element<Message>> = entries
        .iter()
        .map(|entry| {
            // Color based on file type
            let color = match entry.file_type {
                FileType::Directory => iced::Color::from_rgb(0.4, 0.6, 1.0), // Blue for dirs
                FileType::Symlink => iced::Color::from_rgb(0.4, 0.9, 0.9),   // Cyan for symlinks
                _ if entry.permissions & 0o111 != 0 => iced::Color::from_rgb(0.4, 0.9, 0.4), // Green for executables
                _ => iced::Color::from_rgb(0.8, 0.8, 0.8), // White for regular files
            };

            let display_name = if let Some(target) = &entry.symlink_target {
                format!("{} -> {}", entry.name, target.display())
            } else {
                entry.name.clone()
            };

            text(display_name)
                .size(font_size)
                .color(color)
                .font(iced::Font::MONOSPACE)
                .into()
        })
        .collect();

    Column::with_children(lines).spacing(0).into()
}
