//! Value renderer — transforms structured data into UI layouts.
//!
//! This module contains the rendering engine that converts `nexus_api::Value`
//! and domain-specific types into Strata layout trees. It handles:
//! - Tables with virtualized rendering
//! - File trees with expand/collapse
//! - Diffs with syntax highlighting
//! - Images, HTTP responses, DNS records, etc.

mod color;
mod domain;
mod table;

pub(crate) use color::term_color_to_strata;

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::{FileEntry, Value};

use crate::data::Block;
use crate::ui::theme;
use crate::features::selection::drag::DragPayload;
use crate::features::shell::{
    AnchorEntry, ClickAction, register_anchor, value_to_anchor_action,
};
use crate::utils::ids;
use strata::content_address::SourceId;
use strata::gpu::ImageHandle;
use strata::layout::{Column, ImageElement, Row, TextElement};
use strata::layout_snapshot::CursorIcon;

use color::file_entry_color;
use domain::{render_domain_value, render_file_entries};
pub(crate) use table::TableLayoutCache;
use table::render_table;

const IMAGE_MAX_W: f32 = 600.0;
const IMAGE_MAX_H: f32 = 400.0;

// =========================================================================
// Public API
// =========================================================================

/// Whether a Value is anchor-worthy (clickable in the UI).
fn is_anchor_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Path(_) | Value::FileEntry(_) | Value::Process(_) | Value::GitCommit(_)
    )
}

/// Render a structured Value from a native (kernel) command into the layout.
pub(crate) fn render_native_value<'a>(
    mut parent: Column<'a>,
    value: &Value,
    block: &Block,
    image_info: Option<(ImageHandle, u32, u32)>,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
    table_layout_cache: &TableLayoutCache,
    table_cell_images: &HashMap<(nexus_api::BlockId, usize, usize), (ImageHandle, u32, u32)>,
) -> Column<'a> {
    let block_id = block.id;
    match value {
        Value::Unit => parent,

        Value::Media { content_type, metadata, .. } => {
            if content_type.starts_with("image/") {
                if let Some((handle, orig_w, orig_h)) = image_info {
                    let scale = (IMAGE_MAX_W / orig_w as f32).min(IMAGE_MAX_H / orig_h as f32).min(1.0);
                    let w = orig_w as f32 * scale;
                    let h = orig_h as f32 * scale;

                    parent = parent.image(
                        ImageElement::new(handle, w, h)
                            .corner_radius(4.0)
                            .widget_id(ids::image_output(block_id))
                            .cursor(CursorIcon::Grab),
                    );

                    // Label
                    let label = if let Some(ref name) = metadata.filename {
                        format!("{} ({})", name, content_type)
                    } else {
                        format!("{} {}x{}", content_type, orig_w, orig_h)
                    };
                    parent = parent.push(TextElement::new(label).color(theme::TEXT_MUTED));
                } else {
                    // Image not yet loaded
                    parent = parent.push(TextElement::new(format!("[{}: loading...]", content_type)).color(theme::TEXT_MUTED));
                }
            } else {
                // Non-image media
                let label = if let Some(ref name) = metadata.filename {
                    format!("[{}: {}]", content_type, name)
                } else {
                    format!("[{}]", content_type)
                };
                parent = parent.push(TextElement::new(label).color(theme::TEXT_MUTED));
            }
            parent
        }

        Value::Table { columns, rows } => {
            render_table(parent, columns, rows, block, click_registry, table_layout_cache, table_cell_images)
        }

        Value::List(items) => {
            // Check for file entries
            let file_entries: Vec<&FileEntry> = items
                .iter()
                .filter_map(|v| match v {
                    Value::FileEntry(entry) => Some(entry.as_ref()),
                    _ => None,
                })
                .collect();

            let source_id = ids::native(block_id);

            if file_entries.len() == items.len() && !file_entries.is_empty() {
                // Render as file list with tree expansion support
                let mut anchor_idx = 0usize;
                let mut expand_idx = 0usize;
                render_file_entries(
                    &mut parent,
                    &file_entries,
                    block,
                    0, // depth
                    &mut anchor_idx,
                    &mut expand_idx,
                    click_registry,
                );
                parent
            } else {
                // Generic list — recurse for structured types, inline for simple ones
                let has_structured = items.iter().any(|v| matches!(v,
                    Value::Domain(_) |
                    Value::GitStatus(_) | Value::GitCommit(_) | Value::Record(_) |
                    Value::Table { .. }
                ));
                if has_structured {
                    for item in items {
                        parent = render_native_value(parent, item, block, None, click_registry, table_layout_cache, table_cell_images);
                    }
                    parent
                } else {
                    for item in items {
                        parent = parent.push(
                            TextElement::new(item.to_text()).color(theme::TEXT_PRIMARY).source(source_id),
                        );
                    }
                    parent
                }
            }
        }

        Value::FileEntry(entry) => {
            let color = file_entry_color(entry);
            let display = if let Some(target) = &entry.symlink_target {
                format!("{} -> {}", entry.name, target.display())
            } else {
                entry.name.clone()
            };
            let anchor_id = ids::anchor(block_id, 0);
            register_anchor(click_registry, anchor_id, AnchorEntry {
                block_id,
                action: value_to_anchor_action(value),
                drag_payload: DragPayload::FilePath(entry.path.clone()),
                table_cell: None,
            });
            let source_id = ids::native(block_id);
            parent.push(
                TextElement::new(display)
                    .color(color)
                    .source(source_id)
                    .widget_id(anchor_id)
                    .cursor_hint(CursorIcon::Pointer),
            )
        }

        Value::Record(fields) => {
            let source_id = ids::native(block_id);
            for (key, val) in fields {
                parent = parent.push(
                    Row::new()
                        .spacing(8.0)
                        .push(TextElement::new(format!("{}:", key)).color(theme::TEXT_SECONDARY).source(source_id))
                        .push(TextElement::new(val.to_text()).color(theme::TEXT_PRIMARY).source(source_id)),
                );
            }
            parent
        }

        Value::Domain(domain) => {
            render_domain_value(parent, domain, block, image_info, click_registry, table_layout_cache, table_cell_images)
        }

        Value::Error { message, .. } => {
            let source_id = ids::native(block_id);
            parent.push(TextElement::new(message).color(theme::ERROR).source(source_id))
        }

        // All other types: render as text
        _ => {
            let text = value.to_text();
            if text.is_empty() {
                parent
            } else {
                let source_id = ids::native(block_id);
                for line in text.lines() {
                    parent = parent.push(TextElement::new(line).color(theme::TEXT_PRIMARY).source(source_id));
                }
                parent
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_anchor_value() {
        assert!(is_anchor_value(&Value::Path("/foo".into())));
        assert!(!is_anchor_value(&Value::String("hello".into())));
    }
}
