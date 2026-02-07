//! Value renderer — transforms structured data into UI layouts.
//!
//! This module contains the rendering engine that converts `nexus_api::Value`
//! and domain-specific types into Strata layout trees. It handles:
//! - Tables with virtualized rendering
//! - File trees with expand/collapse
//! - Diffs with syntax highlighting
//! - Images, HTTP responses, DNS records, etc.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::{FileEntry, FileType, Value, format_value_for_display};

use crate::blocks::Block;
use crate::nexus_app::colors;
use crate::nexus_app::drag_state::DragPayload;
use crate::nexus_app::shell::{
    AnchorEntry, ClickAction, register_anchor, register_tree_toggle,
    semantic_text_for_value, value_to_anchor_action,
};
use crate::nexus_app::source_ids;
use strata::content_address::SourceId;
use strata::gpu::ImageHandle;
use strata::layout::{
    Column, CrossAxisAlignment, ImageElement, Row, TextElement, VirtualCell, VirtualTableElement,
};
use strata::layout_snapshot::CursorIcon;
use strata::primitives::Color;

// =========================================================================
// Public API
// =========================================================================

/// Convert nexus-term color to Strata color.
pub(crate) fn term_color_to_strata(c: nexus_term::Color) -> Color {
    // ANSI palette matched from theme.rs ANSI_* constants
    fn ansi_color(n: u8) -> Color {
        match n {
            0  => Color::rgb(0.0, 0.0, 0.0),       // Black
            1  => Color::rgb(0.8, 0.2, 0.2),        // Red
            2  => Color::rgb(0.05, 0.74, 0.47),     // Green
            3  => Color::rgb(0.9, 0.9, 0.06),       // Yellow
            4  => Color::rgb(0.14, 0.45, 0.78),     // Blue
            5  => Color::rgb(0.74, 0.25, 0.74),     // Magenta
            6  => Color::rgb(0.07, 0.66, 0.8),      // Cyan
            7  => Color::rgb(0.9, 0.9, 0.9),        // White
            8  => Color::rgb(0.4, 0.4, 0.4),        // Bright Black
            9  => Color::rgb(0.95, 0.3, 0.3),       // Bright Red
            10 => Color::rgb(0.14, 0.82, 0.55),     // Bright Green
            11 => Color::rgb(0.96, 0.96, 0.26),     // Bright Yellow
            12 => Color::rgb(0.23, 0.56, 0.92),     // Bright Blue
            13 => Color::rgb(0.84, 0.44, 0.84),     // Bright Magenta
            14 => Color::rgb(0.16, 0.72, 0.86),     // Bright Cyan
            15 => Color::rgb(1.0, 1.0, 1.0),        // Bright White
            // 216-color cube (indices 16-231)
            16..=231 => {
                let idx = n - 16;
                let r = (idx / 36) % 6;
                let g = (idx / 6) % 6;
                let b = idx % 6;
                let to_val = |v: u8| if v == 0 { 0.0 } else { (55.0 + v as f32 * 40.0) / 255.0 };
                Color::rgb(to_val(r), to_val(g), to_val(b))
            }
            // Grayscale (indices 232-255)
            232..=255 => {
                let gray = (8.0 + (n - 232) as f32 * 10.0) / 255.0;
                Color::rgb(gray, gray, gray)
            }
        }
    }

    match c {
        nexus_term::Color::Default => Color::rgb(0.9, 0.9, 0.9),
        nexus_term::Color::Named(n) => ansi_color(n),
        nexus_term::Color::Rgb(r, g, b) => Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
        nexus_term::Color::Indexed(n) => ansi_color(n),
    }
}

/// Whether a Value is anchor-worthy (clickable in the UI).
pub(crate) fn is_anchor_value(value: &Value) -> bool {
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
) -> Column<'a> {
    let block_id = block.id;
    match value {
        Value::Unit => parent,

        Value::Media { content_type, metadata, .. } => {
            if content_type.starts_with("image/") {
                if let Some((handle, orig_w, orig_h)) = image_info {
                    // Scale down to fit, max 600px wide, 400px tall
                    let max_w = 600.0_f32;
                    let max_h = 400.0_f32;
                    let scale = (max_w / orig_w as f32).min(max_h / orig_h as f32).min(1.0);
                    let w = orig_w as f32 * scale;
                    let h = orig_h as f32 * scale;

                    parent = parent.image(
                        ImageElement::new(handle, w, h)
                            .corner_radius(4.0)
                            .widget_id(source_ids::image_output(block_id))
                            .cursor(CursorIcon::Grab),
                    );

                    // Label
                    let label = if let Some(ref name) = metadata.filename {
                        format!("{} ({})", name, content_type)
                    } else {
                        format!("{} {}x{}", content_type, orig_w, orig_h)
                    };
                    parent = parent.push(TextElement::new(label).color(colors::TEXT_MUTED));
                } else {
                    // Image not yet loaded
                    parent = parent.push(TextElement::new(format!("[{}: loading...]", content_type)).color(colors::TEXT_MUTED));
                }
            } else {
                // Non-image media
                let label = if let Some(ref name) = metadata.filename {
                    format!("[{}: {}]", content_type, name)
                } else {
                    format!("[{}]", content_type)
                };
                parent = parent.push(TextElement::new(label).color(colors::TEXT_MUTED));
            }
            parent
        }

        Value::Table { columns, rows } => {
            let _t0 = std::time::Instant::now();
            let source_id = source_ids::table(block_id);

            let char_w = 8.4_f32;
            let cell_padding = 16.0_f32;
            let num_cols = columns.len();

            // Column width estimation: sample first 100 rows (O(1) vs O(n))
            let sample_count = rows.len().min(100);
            let mut max_col_lens = vec![0usize; num_cols];
            for row in rows[..sample_count].iter() {
                for (col_idx, cell) in row.iter().enumerate() {
                    if col_idx >= num_cols { break; }
                    let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                        format_value_for_display(cell, fmt)
                    } else {
                        cell.to_text()
                    };
                    let line_len = text.lines()
                        .map(|l| unicode_width::UnicodeWidthStr::width(l))
                        .max().unwrap_or(0);
                    if line_len > max_col_lens[col_idx] {
                        max_col_lens[col_idx] = line_len;
                    }
                }
            }

            let col_widths: Vec<f32> = columns.iter().enumerate().map(|(i, col)| {
                let header_width = unicode_width::UnicodeWidthStr::width(col.name.as_str());
                let max_len = header_width.max(max_col_lens[i]).max(4);
                (max_len as f32 * char_w + cell_padding).min(400.0)
            }).collect();

            // Build VirtualTableElement — lightweight, no wrapping
            let mut table = VirtualTableElement::new(source_id);

            // Add column headers with sort support
            for (i, col) in columns.iter().enumerate() {
                let sort_id = source_ids::table_sort(block_id, i);
                let header_name = if block.table_sort.column == Some(i) {
                    if block.table_sort.ascending {
                        format!("{} \u{25B2}", col.name) // ▲
                    } else {
                        format!("{} \u{25BC}", col.name) // ▼
                    }
                } else {
                    col.name.clone()
                };
                table = table.column_sortable(&header_name, col_widths[i], sort_id);
            }

            // Build lightweight VirtualCell rows — no wrapping, no line splitting
            let mut anchor_idx = 0usize;
            for (_row_idx, row) in rows.iter().enumerate() {
                let cells: Vec<VirtualCell> = row.iter().enumerate().map(|(col_idx, cell)| {
                    let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                        format_value_for_display(cell, fmt)
                    } else {
                        cell.to_text()
                    };
                    let widget_id = if is_anchor_value(cell) {
                        let id = source_ids::anchor(block_id, anchor_idx);
                        register_anchor(click_registry, id, AnchorEntry {
                            block_id,
                            action: value_to_anchor_action(cell),
                            drag_payload: DragPayload::TableRow {
                                block_id,
                                row_index: anchor_idx,
                                display: semantic_text_for_value(cell, columns.get(col_idx)),
                            },
                        });
                        anchor_idx += 1;
                        Some(id)
                    } else {
                        None
                    };
                    VirtualCell {
                        text,
                        color: value_text_color(cell),
                        widget_id,
                    }
                }).collect();
                table = table.row(cells);
            }

            let _t1 = _t0.elapsed();
            let result = parent.push(table);
            let _t2 = _t0.elapsed();
            if strata::frame_timing::is_enabled() {
                let frame = strata::frame_timing::current_frame();
                if frame % 60 == 0 {
                    eprintln!("[frame {}] vtable build: {:.2?} layout={:.2?} ({}rows x {}cols)",
                        frame, _t1, _t2 - _t1, rows.len(), num_cols);
                }
            }
            result
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

            let source_id = source_ids::native(block_id);

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
                        parent = render_native_value(parent, item, block, None, click_registry);
                    }
                    parent
                } else {
                    for item in items {
                        parent = parent.push(
                            TextElement::new(item.to_text()).color(colors::TEXT_PRIMARY).source(source_id),
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
            let anchor_id = source_ids::anchor(block_id, 0);
            register_anchor(click_registry, anchor_id, AnchorEntry {
                block_id,
                action: value_to_anchor_action(value),
                drag_payload: DragPayload::FilePath(entry.path.clone()),
            });
            let source_id = source_ids::native(block_id);
            parent.push(
                TextElement::new(display)
                    .color(color)
                    .source(source_id)
                    .widget_id(anchor_id)
                    .cursor_hint(CursorIcon::Pointer),
            )
        }

        Value::Record(fields) => {
            let source_id = source_ids::native(block_id);
            for (key, val) in fields {
                parent = parent.push(
                    Row::new()
                        .spacing(8.0)
                        .push(TextElement::new(format!("{}:", key)).color(colors::TEXT_SECONDARY).source(source_id))
                        .push(TextElement::new(val.to_text()).color(colors::TEXT_PRIMARY).source(source_id)),
                );
            }
            parent
        }

        Value::Domain(domain) => {
            render_domain_value(parent, domain, block, image_info, click_registry)
        }

        Value::Error { message, .. } => {
            let source_id = source_ids::native(block_id);
            parent.push(TextElement::new(message).color(colors::ERROR).source(source_id))
        }

        // All other types: render as text
        _ => {
            let text = value.to_text();
            if text.is_empty() {
                parent
            } else {
                let source_id = source_ids::native(block_id);
                for line in text.lines() {
                    parent = parent.push(TextElement::new(line).color(colors::TEXT_PRIMARY).source(source_id));
                }
                parent
            }
        }
    }
}

// =========================================================================
// Domain-specific rendering
// =========================================================================

/// Render a domain-specific value (FileOp, Tree, DiffFile, etc.).
fn render_domain_value<'a>(
    mut parent: Column<'a>,
    domain: &nexus_api::DomainValue,
    block: &Block,
    image_info: Option<(ImageHandle, u32, u32)>,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
) -> Column<'a> {
    use nexus_api::DomainValue;
    let block_id = block.id;
    let source_id = source_ids::native(block_id);

    match domain {
        DomainValue::FileOp(info) => {
            let (icon, phase_color) = match info.phase {
                nexus_api::FileOpPhase::Planning => ("\u{1F50D}", colors::WARNING),
                nexus_api::FileOpPhase::Executing => ("\u{25B6}", colors::RUNNING),
                nexus_api::FileOpPhase::Completed => ("\u{2714}", colors::SUCCESS),
                nexus_api::FileOpPhase::Failed => ("\u{2718}", colors::ERROR),
            };
            let op_label = match info.op_type {
                nexus_api::FileOpKind::Copy => "Copy",
                nexus_api::FileOpKind::Move => "Move",
                nexus_api::FileOpKind::Remove => "Remove",
                nexus_api::FileOpKind::Chmod => "Chmod",
                nexus_api::FileOpKind::Chown => "Chown",
            };
            parent = parent.push(
                TextElement::new(format!("{} {} {:?}", icon, op_label, info.phase))
                    .color(phase_color)
                    .source(source_id),
            );
            if let Some(total) = info.total_bytes {
                if total > 0 {
                    let pct = (info.bytes_processed as f64 / total as f64 * 100.0).min(100.0);
                    let bar_len = 40;
                    let filled = (pct / 100.0 * bar_len as f64) as usize;
                    let bar: String = "\u{2588}".repeat(filled)
                        + &"\u{2591}".repeat(bar_len - filled);
                    parent = parent.push(
                        TextElement::new(format!("[{}] {:.1}%", bar, pct))
                            .color(colors::TEXT_PRIMARY)
                            .source(source_id),
                    );
                }
            } else if info.phase == nexus_api::FileOpPhase::Planning {
                parent = parent.push(
                    TextElement::new("[estimating...]".to_string())
                        .color(colors::TEXT_MUTED)
                        .source(source_id),
                );
            }
            let files_str = if let Some(total) = info.files_total {
                format!("{}/{} files", info.files_processed, total)
            } else {
                format!("{} files processed", info.files_processed)
            };
            let bytes_str = if let Some(total) = info.total_bytes {
                format!(", {}/{} bytes", info.bytes_processed, total)
            } else {
                String::new()
            };
            parent = parent.push(
                TextElement::new(format!("{}{}", files_str, bytes_str))
                    .color(colors::TEXT_SECONDARY)
                    .source(source_id),
            );
            // Throughput + ETA based on cumulative rate
            if info.phase == nexus_api::FileOpPhase::Executing && info.start_time_ms > 0 {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let elapsed_s = (now_ms.saturating_sub(info.start_time_ms)) as f64 / 1000.0;
                if elapsed_s > 0.1 {
                    let throughput_str = if let Some(total_bytes) = info.total_bytes {
                        if total_bytes > 0 {
                            let rate = info.bytes_processed as f64 / elapsed_s;
                            let remaining_bytes = total_bytes.saturating_sub(info.bytes_processed);
                            let eta_s = if rate > 0.0 { remaining_bytes as f64 / rate } else { 0.0 };
                            format!("  {}/s ETA: {}", nexus_api::format_size(rate as u64), format_eta(eta_s))
                        } else {
                            String::new()
                        }
                    } else if let Some(files_total) = info.files_total {
                        if files_total > 0 {
                            let rate = info.files_processed as f64 / elapsed_s;
                            let remaining = files_total.saturating_sub(info.files_processed);
                            let eta_s = if rate > 0.0 { remaining as f64 / rate } else { 0.0 };
                            format!("  {:.0} files/s ETA: {}", rate, format_eta(eta_s))
                        } else {
                            String::new()
                        }
                    } else {
                        format!("  {:.1}s elapsed", elapsed_s)
                    };
                    if !throughput_str.is_empty() {
                        parent = parent.push(
                            TextElement::new(throughput_str)
                                .color(colors::TEXT_MUTED)
                                .source(source_id),
                        );
                    }
                }
            }
            if let Some(ref current) = info.current_file {
                parent = parent.push(
                    TextElement::new(format!("  {}", current.display()))
                        .color(colors::TEXT_PATH)
                        .source(source_id),
                );
            }
            for err in &info.errors {
                parent = parent.push(
                    TextElement::new(format!("  error: {}: {}", err.path.display(), err.message))
                        .color(colors::ERROR)
                        .source(source_id),
                );
            }
            parent
        }

        DomainValue::Tree(tree) => {
            for node in &tree.nodes {
                let indent = if node.depth == 0 {
                    String::new()
                } else {
                    let prefix = "    ".repeat(node.depth.saturating_sub(1));
                    let is_last = tree.nodes.iter()
                        .filter(|n| n.parent == node.parent && n.depth == node.depth)
                        .last()
                        .map(|n| n.id == node.id)
                        .unwrap_or(true);
                    if is_last {
                        format!("{}\u{2514}\u{2500}\u{2500} ", prefix)
                    } else {
                        format!("{}\u{251C}\u{2500}\u{2500} ", prefix)
                    }
                };
                let color = match node.node_type {
                    nexus_api::FileType::Directory => colors::TEXT_PATH,
                    _ => colors::TEXT_PRIMARY,
                };
                parent = parent.push(
                    TextElement::new(format!("{}{}", indent, node.name))
                        .color(color)
                        .source(source_id),
                );
            }
            parent
        }

        DomainValue::DiffFile(diff) => {
            let stats_str = format!("+{} -{}", diff.additions, diff.deletions);
            parent = parent.push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new(&diff.file_path).color(colors::TEXT_PRIMARY).source(source_id))
                    .push(TextElement::new(format!("  +{}", diff.additions)).color(colors::DIFF_ADD).source(source_id))
                    .push(TextElement::new(format!("-{}", diff.deletions)).color(colors::DIFF_REMOVE).source(source_id)),
            );
            for hunk in &diff.hunks {
                parent = parent.push(
                    TextElement::new(format!("@@ -{},{} +{},{} @@ {}",
                        hunk.old_start, hunk.old_count,
                        hunk.new_start, hunk.new_count,
                        hunk.header))
                        .color(colors::TEXT_PATH)
                        .source(source_id),
                );
                for line in &hunk.lines {
                    let (prefix, color) = match line.kind {
                        nexus_api::DiffLineKind::Context => (" ", colors::TEXT_MUTED),
                        nexus_api::DiffLineKind::Addition => ("+", colors::DIFF_ADD),
                        nexus_api::DiffLineKind::Deletion => ("-", colors::DIFF_REMOVE),
                    };
                    parent = parent.push(
                        TextElement::new(format!("{}{}", prefix, line.content))
                            .color(color)
                            .source(source_id),
                    );
                }
            }
            let _ = stats_str;
            parent
        }

        DomainValue::NetEvent(evt) => {
            let (icon, color) = if evt.success {
                ("\u{2714}", colors::SUCCESS)
            } else {
                ("\u{2718}", colors::ERROR)
            };
            let ip_str = evt.ip.as_ref().map(|ip| format!(" ({})", ip)).unwrap_or_default();
            let rtt_str = evt.rtt_ms.map(|r| format!(" {:.1}ms", r)).unwrap_or_default();
            parent.push(
                TextElement::new(format!("{} {}{}{}", icon, evt.host, ip_str, rtt_str))
                    .color(color)
                    .source(source_id),
            )
        }

        DomainValue::DnsAnswer(dns) => {
            parent = parent.push(
                TextElement::new(format!(";; {} {} query", dns.query, dns.record_type))
                    .color(colors::TEXT_SECONDARY)
                    .source(source_id),
            );
            for record in &dns.answers {
                parent = parent.push(
                    TextElement::new(format!("  {} {} IN {} {}",
                        record.name, record.ttl, record.record_type, record.data))
                        .color(colors::TEXT_PRIMARY)
                        .source(source_id),
                );
            }
            parent = parent.push(
                TextElement::new(format!(";; Query time: {:.0} msec, Server: {}",
                    dns.query_time_ms, dns.server))
                    .color(colors::TEXT_MUTED)
                    .source(source_id),
            );
            parent
        }

        DomainValue::HttpResponse(resp) => {
            let status_color = if resp.status_code < 300 {
                colors::SUCCESS
            } else if resp.status_code < 400 {
                colors::WARNING
            } else {
                colors::ERROR
            };
            parent = parent.push(
                TextElement::new(format!("{} {} {} ({:.0}ms)",
                    resp.method, resp.status_code, resp.status_text, resp.timing.total_ms))
                    .color(status_color)
                    .source(source_id),
            );
            // Timing waterfall
            {
                let t = &resp.timing;
                let phases: Vec<(&str, Option<f64>, Color)> = vec![
                    ("DNS",     t.dns_ms,      Color::rgb(0.4, 0.7, 1.0)),
                    ("Connect", t.connect_ms,  Color::rgb(0.5, 0.8, 0.5)),
                    ("TLS",     t.tls_ms,      Color::rgb(0.8, 0.6, 1.0)),
                    ("TTFB",    t.ttfb_ms,     Color::rgb(1.0, 0.8, 0.3)),
                    ("Transfer",t.transfer_ms, Color::rgb(0.3, 0.9, 0.9)),
                ];
                let has_phases = phases.iter().any(|(_, v, _)| v.is_some());
                if has_phases {
                    let total = t.total_ms.max(0.001);
                    let bar_width = 40usize;
                    let mut waterfall = String::with_capacity(bar_width);
                    let mut legend_parts = Vec::new();
                    for (label, ms_opt, _color) in &phases {
                        if let Some(ms) = ms_opt {
                            let fraction = ms / total;
                            let chars = (fraction * bar_width as f64).round().max(0.0) as usize;
                            let ch = match *label {
                                "DNS" => 'D',
                                "Connect" => 'C',
                                "TLS" => 'S',
                                "TTFB" => 'W',
                                "Transfer" => 'T',
                                _ => '?',
                            };
                            for _ in 0..chars { waterfall.push(ch); }
                            legend_parts.push(format!("{}:{:.0}ms", label, ms));
                        }
                    }
                    // Pad to bar_width
                    while waterfall.len() < bar_width {
                        waterfall.push('\u{2591}');
                    }
                    parent = parent.push(
                        TextElement::new(format!("  [{}] {:.0}ms", waterfall, total))
                            .color(colors::TEXT_MUTED)
                            .source(source_id),
                    );
                    parent = parent.push(
                        TextElement::new(format!("  {}", legend_parts.join(" | ")))
                            .color(colors::TEXT_MUTED)
                            .source(source_id),
                    );
                }
            }
            for (name, value) in resp.headers.iter().take(10) {
                parent = parent.push(
                    TextElement::new(format!("  {}: {}", name, value))
                        .color(colors::TEXT_SECONDARY)
                        .source(source_id),
                );
            }
            if let Some(ref preview) = resp.body_preview {
                parent = parent.push(
                    TextElement::new("").source(source_id),
                );
                for line in preview.lines().take(20) {
                    parent = parent.push(
                        TextElement::new(line).color(colors::TEXT_PRIMARY).source(source_id),
                    );
                }
                if resp.body_truncated {
                    parent = parent.push(
                        TextElement::new(format!("[truncated, {} bytes total]", resp.body_len))
                            .color(colors::TEXT_MUTED)
                            .source(source_id),
                    );
                }
            } else if resp.body_len > 0 {
                parent = parent.push(
                    TextElement::new(format!("[binary, {} bytes]", resp.body_len))
                        .color(colors::TEXT_MUTED)
                        .source(source_id),
                );
            }
            parent
        }

        DomainValue::Interactive(req) => {
            // Check if this is a DiffViewer
            if let Some(crate::blocks::ViewState::DiffViewer { scroll_line, current_file, collapsed_indices }) = &block.view_state {
                if let Value::List(items) = &req.content {
                    return render_diff_viewer(parent, items, *scroll_line, *current_file, collapsed_indices, source_id);
                }
            }
            render_native_value(parent, &req.content, block, image_info, click_registry)
        }

        DomainValue::BlobChunk(chunk) => {
            let size = chunk.total_size.unwrap_or(chunk.data.len() as u64);
            let src = chunk.source.as_deref().unwrap_or("binary");
            parent = parent.push(
                TextElement::new(format!("[{}: {} {}]", src, chunk.content_type, nexus_api::format_size(size)))
                    .color(colors::TEXT_MUTED)
                    .source(source_id),
            );
            parent
        }
    }
}

// =========================================================================
// File tree rendering
// =========================================================================

/// Render file entries with tree expansion support.
/// Recursively renders children for expanded directories.
fn render_file_entries<'a>(
    parent: &mut Column<'a>,
    entries: &[&FileEntry],
    block: &Block,
    depth: usize,
    anchor_idx: &mut usize,
    expand_idx: &mut usize,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
) {
    let block_id = block.id;
    let indent_px = depth as f32 * 20.0;

    for entry in entries {
        let is_dir = matches!(entry.file_type, FileType::Directory);
        let is_expanded = is_dir && block.file_tree().map_or(false, |t| t.is_expanded(&entry.path));
        let color = file_entry_color(entry);

        // Build the row: [chevron (if dir)] [name]
        let mut row = Row::new()
            .spacing(4.0)
            .cross_align(CrossAxisAlignment::Center);

        // Indentation
        if depth > 0 {
            row = row.push(TextElement::new(" ".repeat((indent_px / 8.0) as usize)));
        }

        // Expand/collapse chevron for directories
        if is_dir {
            let chevron = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
            let expand_id = source_ids::tree_expand(block_id, *expand_idx);
            register_tree_toggle(click_registry, expand_id, block_id, entry.path.clone());
            *expand_idx += 1;

            row = row.push(
                TextElement::new(chevron)
                    .color(colors::TEXT_MUTED)
                    .widget_id(expand_id)
                    .cursor_hint(CursorIcon::Pointer),
            );
        } else {
            // Placeholder to align with directories
            row = row.push(TextElement::new("  ").color(colors::TEXT_MUTED));
        }

        // File/directory name (clickable anchor)
        let display = if let Some(target) = &entry.symlink_target {
            format!("{} -> {}", entry.name, target.display())
        } else {
            entry.name.clone()
        };

        let anchor_id = source_ids::anchor(block_id, *anchor_idx);
        let file_value = Value::FileEntry(Box::new((*entry).clone()));
        register_anchor(click_registry, anchor_id, AnchorEntry {
            block_id,
            action: value_to_anchor_action(&file_value),
            drag_payload: DragPayload::FilePath(entry.path.clone()),
        });
        *anchor_idx += 1;

        let source_id = source_ids::native(block_id);
        row = row.push(
            TextElement::new(display)
                .color(color)
                .source(source_id)
                .widget_id(anchor_id)
                .cursor_hint(CursorIcon::Pointer),
        );

        *parent = std::mem::take(parent).push(row);

        // Recursively render children if expanded
        if is_expanded {
            if let Some(children) = block.file_tree().and_then(|t| t.get_children(&entry.path)) {
                let child_refs: Vec<&FileEntry> = children.iter().collect();
                render_file_entries(
                    parent,
                    &child_refs,
                    block,
                    depth + 1,
                    anchor_idx,
                    expand_idx,
                    click_registry,
                );
            } else {
                // Children not loaded yet — show loading indicator
                let mut loading_row = Row::new().spacing(4.0);
                if depth > 0 {
                    loading_row = loading_row.push(TextElement::new(" ".repeat(((depth + 1) as f32 * 20.0 / 8.0) as usize)));
                } else {
                    loading_row = loading_row.push(TextElement::new("    ")); // indent for loading
                }
                loading_row = loading_row.push(TextElement::new("Loading...").color(colors::TEXT_MUTED));
                *parent = std::mem::take(parent).push(loading_row);
            }
        }
    }
}

// =========================================================================
// Diff viewer
// =========================================================================

/// Render the diff viewer for reviewing changes.
fn render_diff_viewer<'a>(
    mut parent: Column<'a>,
    items: &[Value],
    scroll_line: usize,
    current_file: usize,
    collapsed_indices: &std::collections::HashSet<usize>,
    source_id: SourceId,
) -> Column<'a> {
    use nexus_api::DomainValue;

    // Header with keybinding hints
    parent = parent.push(
        TextElement::new("j/k: scroll | n/p: next/prev file | space: toggle | q: quit")
            .color(colors::TEXT_MUTED)
            .source(source_id),
    );

    let viewport_height = 50;
    let viewport_start = scroll_line;
    let viewport_end = scroll_line + viewport_height;

    // First pass: count total lines per file to find viewport boundaries.
    // This avoids allocating strings for lines outside the viewport.
    struct FileSpan {
        file_idx: usize,
        line_start: usize,
        line_count: usize,
    }
    let mut spans: Vec<FileSpan> = Vec::new();
    let mut total_lines = 0usize;

    for (file_idx, item) in items.iter().enumerate() {
        let diff = match item {
            Value::Domain(d) => match d.as_ref() {
                DomainValue::DiffFile(diff) => diff,
                _ => continue,
            },
            _ => continue,
        };

        let is_collapsed = collapsed_indices.contains(&file_idx);
        // 1 line for header + hunks + blank separator
        let mut count = 1; // header
        if !is_collapsed {
            for hunk in &diff.hunks {
                count += 1 + hunk.lines.len(); // hunk header + diff lines
            }
            count += 1; // blank separator
        }
        spans.push(FileSpan { file_idx, line_start: total_lines, line_count: count });
        total_lines += count;
    }

    // Second pass: only generate text for lines within the viewport.
    // Collect DiffFile references matching the order of spans.
    let diffs: Vec<&nexus_api::DiffFileInfo> = items.iter().filter_map(|item| {
        if let Value::Domain(d) = item {
            if let DomainValue::DiffFile(diff) = d.as_ref() {
                return Some(diff);
            }
        }
        None
    }).collect();

    for (span_idx, span) in spans.iter().enumerate() {
        let span_end = span.line_start + span.line_count;

        // Skip files entirely before viewport
        if span_end <= viewport_start {
            continue;
        }
        // Stop once past viewport
        if span.line_start >= viewport_end {
            break;
        }

        let item = diffs[span_idx];
        let mut line_num = span.line_start;

        let is_collapsed = collapsed_indices.contains(&span.file_idx);

        // Header line
        if line_num >= viewport_start && line_num < viewport_end {
            let cursor = if span.file_idx == current_file { "\u{25B6} " } else { "  " };
            let collapse_marker = if is_collapsed { "\u{25B8}" } else { "\u{25BE}" };
            let header_color = if span.file_idx == current_file {
                Color::rgb(1.0, 1.0, 0.6)
            } else {
                colors::TEXT_PATH
            };
            let old_path_suffix = if let Some(ref old) = item.old_path {
                format!(" (from {})", old)
            } else {
                String::new()
            };
            parent = parent.push(
                TextElement::new(format!("{}{} {} (+{} -{}){}", cursor, collapse_marker,
                    item.file_path, item.additions, item.deletions, old_path_suffix))
                    .color(header_color)
                    .source(source_id),
            );
        }
        line_num += 1;

        if is_collapsed {
            continue;
        }

        for hunk in &item.hunks {
            // Hunk header
            if line_num >= viewport_start && line_num < viewport_end {
                parent = parent.push(
                    TextElement::new(format!("@@ -{},{} +{},{} @@ {}",
                        hunk.old_start, hunk.old_count,
                        hunk.new_start, hunk.new_count, hunk.header))
                        .color(Color::rgb(0.5, 0.5, 1.0))
                        .source(source_id),
                );
            }
            line_num += 1;

            for diff_line in &hunk.lines {
                if line_num >= viewport_start && line_num < viewport_end {
                    let (prefix, color) = match diff_line.kind {
                        nexus_api::DiffLineKind::Addition => ("+", Color::rgb(0.4, 0.9, 0.4)),
                        nexus_api::DiffLineKind::Deletion => ("-", Color::rgb(0.9, 0.4, 0.4)),
                        nexus_api::DiffLineKind::Context => (" ", colors::TEXT_SECONDARY),
                    };
                    parent = parent.push(
                        TextElement::new(format!("{}{}", prefix, diff_line.content))
                            .color(color)
                            .source(source_id),
                    );
                }
                line_num += 1;
                if line_num >= viewport_end { break; }
            }
            if line_num >= viewport_end { break; }
        }

        // Blank separator
        if line_num >= viewport_start && line_num < viewport_end {
            parent = parent.push(
                TextElement::new("")
                    .color(colors::TEXT_PRIMARY)
                    .source(source_id),
            );
        }
    }

    // Footer with position info
    let end = viewport_end.min(total_lines);
    if total_lines > viewport_height {
        parent = parent.push(
            TextElement::new(format!("  [{}-{}/{}]", scroll_line + 1, end, total_lines))
                .color(colors::TEXT_MUTED)
                .source(source_id),
        );
    }

    parent
}

// =========================================================================
// Helpers
// =========================================================================

/// Format ETA in human-readable form.
pub(crate) fn format_eta(seconds: f64) -> String {
    if seconds < 1.0 {
        "<1s".to_string()
    } else if seconds < 60.0 {
        format!("{}s", seconds as u64)
    } else if seconds < 3600.0 {
        let m = (seconds / 60.0) as u64;
        let s = (seconds % 60.0) as u64;
        format!("{}m {}s", m, s)
    } else {
        let h = (seconds / 3600.0) as u64;
        let m = ((seconds % 3600.0) / 60.0) as u64;
        format!("{}h {}m", h, m)
    }
}

fn value_text_color(value: &Value) -> Color {
    match value {
        Value::Int(_) | Value::Float(_) => Color::rgb(0.6, 0.8, 1.0),
        Value::Bool(true) => colors::SUCCESS,
        Value::Bool(false) => colors::ERROR,
        Value::Path(_) => colors::TEXT_PATH,
        Value::FileEntry(e) => file_entry_color(e),
        Value::Error { .. } => colors::ERROR,
        _ => colors::TEXT_PRIMARY,
    }
}

/// Get display color for a file entry.
fn file_entry_color(entry: &FileEntry) -> Color {
    match entry.file_type {
        FileType::Directory => Color::rgb(0.4, 0.6, 1.0),
        FileType::Symlink => Color::rgb(0.4, 0.9, 0.9),
        _ if entry.permissions & 0o111 != 0 => Color::rgb(0.4, 0.9, 0.4),
        _ => Color::rgb(0.8, 0.8, 0.8),
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_eta_subsecond() {
        assert_eq!(format_eta(0.0), "<1s");
        assert_eq!(format_eta(0.5), "<1s");
        assert_eq!(format_eta(0.99), "<1s");
    }

    #[test]
    fn test_format_eta_seconds() {
        assert_eq!(format_eta(1.0), "1s");
        assert_eq!(format_eta(30.0), "30s");
        assert_eq!(format_eta(59.9), "59s");
    }

    #[test]
    fn test_format_eta_minutes() {
        assert_eq!(format_eta(60.0), "1m 0s");
        assert_eq!(format_eta(90.0), "1m 30s");
        assert_eq!(format_eta(125.0), "2m 5s");
        assert_eq!(format_eta(3599.0), "59m 59s");
    }

    #[test]
    fn test_format_eta_hours() {
        assert_eq!(format_eta(3600.0), "1h 0m");
        assert_eq!(format_eta(3660.0), "1h 1m");
        assert_eq!(format_eta(7200.0), "2h 0m");
        assert_eq!(format_eta(7320.0), "2h 2m");
    }

    #[test]
    fn test_is_anchor_value() {
        assert!(is_anchor_value(&Value::Path("/foo".into())));
        assert!(!is_anchor_value(&Value::String("hello".into())));
    }
}
