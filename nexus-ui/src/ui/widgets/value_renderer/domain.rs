//! Domain-specific rendering: file ops, HTTP responses, diff viewer, file trees.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::{FileEntry, FileType, Value};

use crate::data::Block;
use crate::ui::theme;
use crate::features::selection::drag::DragPayload;
use crate::features::shell::{
    AnchorEntry, ClickAction, register_anchor, register_tree_toggle, value_to_anchor_action,
};
use crate::utils::ids;
use strata::content_address::SourceId;
use strata::gpu::ImageHandle;
use strata::layout::{Column, CrossAxisAlignment, Row, TextElement};
use strata::layout_snapshot::CursorIcon;
use strata::primitives::Color;

use super::color::file_entry_color;
use super::{render_native_value, TableLayoutCache};

const PROGRESS_BAR_LEN: usize = 40;
const WATERFALL_BAR_LEN: usize = 40;
const DIFF_VIEWPORT_HEIGHT: usize = 50;
const TREE_INDENT_PX: f32 = 20.0;

// =========================================================================
// Domain dispatch
// =========================================================================

/// Render a domain-specific value (FileOp, Tree, DiffFile, etc.).
pub(super) fn render_domain_value<'a>(
    mut parent: Column<'a>,
    domain: &nexus_api::DomainValue,
    block: &Block,
    image_info: Option<(ImageHandle, u32, u32)>,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
    table_layout_cache: &TableLayoutCache,
) -> Column<'a> {
    use nexus_api::DomainValue;
    let block_id = block.id;
    let source_id = ids::native(block_id);

    match domain {
        DomainValue::FileOp(info) => {
            render_file_op(parent, info, source_id)
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
                    nexus_api::FileType::Directory => theme::TEXT_PATH,
                    _ => theme::TEXT_PRIMARY,
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
            parent = parent.push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new(&diff.file_path).color(theme::TEXT_PRIMARY).source(source_id))
                    .push(TextElement::new(format!("  +{}", diff.additions)).color(theme::DIFF_ADD).source(source_id))
                    .push(TextElement::new(format!("-{}", diff.deletions)).color(theme::DIFF_REMOVE).source(source_id)),
            );
            for hunk in &diff.hunks {
                parent = parent.push(
                    TextElement::new(format!("@@ -{},{} +{},{} @@ {}",
                        hunk.old_start, hunk.old_count,
                        hunk.new_start, hunk.new_count,
                        hunk.header))
                        .color(theme::TEXT_PATH)
                        .source(source_id),
                );
                for line in &hunk.lines {
                    let (prefix, color) = match line.kind {
                        nexus_api::DiffLineKind::Context => (" ", theme::TEXT_MUTED),
                        nexus_api::DiffLineKind::Addition => ("+", theme::DIFF_ADD),
                        nexus_api::DiffLineKind::Deletion => ("-", theme::DIFF_REMOVE),
                    };
                    parent = parent.push(
                        TextElement::new(format!("{}{}", prefix, line.content))
                            .color(color)
                            .source(source_id),
                    );
                }
            }
            parent
        }

        DomainValue::NetEvent(evt) => {
            let (icon, color) = if evt.success {
                ("\u{2714}", theme::SUCCESS)
            } else {
                ("\u{2718}", theme::ERROR)
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
                    .color(theme::TEXT_SECONDARY)
                    .source(source_id),
            );
            for record in &dns.answers {
                parent = parent.push(
                    TextElement::new(format!("  {} {} IN {} {}",
                        record.name, record.ttl, record.record_type, record.data))
                        .color(theme::TEXT_PRIMARY)
                        .source(source_id),
                );
            }
            parent = parent.push(
                TextElement::new(format!(";; Query time: {:.0} msec, Server: {}",
                    dns.query_time_ms, dns.server))
                    .color(theme::TEXT_MUTED)
                    .source(source_id),
            );
            parent
        }

        DomainValue::HttpResponse(resp) => {
            render_http_response(parent, resp, source_id)
        }

        DomainValue::Interactive(req) => {
            // Check if this is a DiffViewer
            if let Some(crate::data::ViewState::DiffViewer { scroll_line, current_file, collapsed_indices }) = &block.view_state {
                if let Value::List(items) = &req.content {
                    return render_diff_viewer(parent, items, *scroll_line, *current_file, collapsed_indices, source_id);
                }
            }
            render_native_value(parent, &req.content, block, image_info, click_registry, table_layout_cache)
        }

        DomainValue::BlobChunk(chunk) => {
            let size = chunk.total_size.unwrap_or(chunk.data.len() as u64);
            let src = chunk.source.as_deref().unwrap_or("binary");
            parent = parent.push(
                TextElement::new(format!("[{}: {} {}]", src, chunk.content_type, nexus_api::format_size(size)))
                    .color(theme::TEXT_MUTED)
                    .source(source_id),
            );
            parent
        }
    }
}

// =========================================================================
// File operation rendering
// =========================================================================

/// Render a file operation with progress bar, throughput, and ETA.
fn render_file_op<'a>(
    mut parent: Column<'a>,
    info: &nexus_api::FileOpInfo,
    source_id: SourceId,
) -> Column<'a> {
    let (icon, phase_color) = match info.phase {
        nexus_api::FileOpPhase::Planning => ("\u{1F50D}", theme::WARNING),
        nexus_api::FileOpPhase::Executing => ("\u{25B6}", theme::RUNNING),
        nexus_api::FileOpPhase::Completed => ("\u{2714}", theme::SUCCESS),
        nexus_api::FileOpPhase::Failed => ("\u{2718}", theme::ERROR),
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
            let filled = (pct / 100.0 * PROGRESS_BAR_LEN as f64) as usize;
            let bar: String = "\u{2588}".repeat(filled)
                + &"\u{2591}".repeat(PROGRESS_BAR_LEN - filled);
            parent = parent.push(
                TextElement::new(format!("[{}] {:.1}%", bar, pct))
                    .color(theme::TEXT_PRIMARY)
                    .source(source_id),
            );
        }
    } else if info.phase == nexus_api::FileOpPhase::Planning {
        parent = parent.push(
            TextElement::new("[estimating...]".to_string())
                .color(theme::TEXT_MUTED)
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
            .color(theme::TEXT_SECONDARY)
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
                        .color(theme::TEXT_MUTED)
                        .source(source_id),
                );
            }
        }
    }
    if let Some(ref current) = info.current_file {
        parent = parent.push(
            TextElement::new(format!("  {}", current.display()))
                .color(theme::TEXT_PATH)
                .source(source_id),
        );
    }
    for err in &info.errors {
        parent = parent.push(
            TextElement::new(format!("  error: {}: {}", err.path.display(), err.message))
                .color(theme::ERROR)
                .source(source_id),
        );
    }
    parent
}

// =========================================================================
// HTTP response rendering
// =========================================================================

/// Render an HTTP response with timing waterfall and body preview.
fn render_http_response<'a>(
    mut parent: Column<'a>,
    resp: &nexus_api::HttpResponseInfo,
    source_id: SourceId,
) -> Column<'a> {
    let status_color = if resp.status_code < 300 {
        theme::SUCCESS
    } else if resp.status_code < 400 {
        theme::WARNING
    } else {
        theme::ERROR
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
        let phases: Vec<(&str, Option<f64>, char)> = vec![
            ("DNS",      t.dns_ms,      'D'),
            ("Connect",  t.connect_ms,  'C'),
            ("TLS",      t.tls_ms,      'S'),
            ("TTFB",     t.ttfb_ms,     'W'),
            ("Transfer", t.transfer_ms, 'T'),
        ];
        let has_phases = phases.iter().any(|(_, v, _)| v.is_some());
        if has_phases {
            let total = t.total_ms.max(0.001);
            let mut waterfall = String::with_capacity(WATERFALL_BAR_LEN);
            let mut legend_parts = Vec::new();
            for (label, ms_opt, ch) in &phases {
                if let Some(ms) = ms_opt {
                    let fraction = ms / total;
                    let chars = (fraction * WATERFALL_BAR_LEN as f64).round().max(0.0) as usize;
                    for _ in 0..chars { waterfall.push(*ch); }
                    legend_parts.push(format!("{}:{:.0}ms", label, ms));
                }
            }
            while waterfall.len() < WATERFALL_BAR_LEN {
                waterfall.push('\u{2591}');
            }
            parent = parent.push(
                TextElement::new(format!("  [{}] {:.0}ms", waterfall, total))
                    .color(theme::TEXT_MUTED)
                    .source(source_id),
            );
            parent = parent.push(
                TextElement::new(format!("  {}", legend_parts.join(" | ")))
                    .color(theme::TEXT_MUTED)
                    .source(source_id),
            );
        }
    }
    for (name, value) in resp.headers.iter().take(10) {
        parent = parent.push(
            TextElement::new(format!("  {}: {}", name, value))
                .color(theme::TEXT_SECONDARY)
                .source(source_id),
        );
    }
    if let Some(ref preview) = resp.body_preview {
        parent = parent.push(TextElement::new("").source(source_id));
        for line in preview.lines().take(20) {
            parent = parent.push(
                TextElement::new(line).color(theme::TEXT_PRIMARY).source(source_id),
            );
        }
        if resp.body_truncated {
            parent = parent.push(
                TextElement::new(format!("[truncated, {} bytes total]", resp.body_len))
                    .color(theme::TEXT_MUTED)
                    .source(source_id),
            );
        }
    } else if resp.body_len > 0 {
        parent = parent.push(
            TextElement::new(format!("[binary, {} bytes]", resp.body_len))
                .color(theme::TEXT_MUTED)
                .source(source_id),
        );
    }
    parent
}

// =========================================================================
// File tree rendering
// =========================================================================

/// Render file entries with tree expansion support.
/// Recursively renders children for expanded directories.
pub(super) fn render_file_entries<'a>(
    parent: &mut Column<'a>,
    entries: &[&FileEntry],
    block: &Block,
    depth: usize,
    anchor_idx: &mut usize,
    expand_idx: &mut usize,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
) {
    let block_id = block.id;
    let indent_px = depth as f32 * TREE_INDENT_PX;

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
            let expand_id = ids::tree_expand(block_id, *expand_idx);
            register_tree_toggle(click_registry, expand_id, block_id, entry.path.clone());
            *expand_idx += 1;

            row = row.push(
                TextElement::new(chevron)
                    .color(theme::TEXT_MUTED)
                    .widget_id(expand_id)
                    .cursor_hint(CursorIcon::Pointer),
            );
        } else {
            // Placeholder to align with directories
            row = row.push(TextElement::new("  ").color(theme::TEXT_MUTED));
        }

        // File/directory name (clickable anchor)
        let display = if let Some(target) = &entry.symlink_target {
            format!("{} -> {}", entry.name, target.display())
        } else {
            entry.name.clone()
        };

        let anchor_id = ids::anchor(block_id, *anchor_idx);
        let file_value = Value::FileEntry(Box::new((*entry).clone()));
        register_anchor(click_registry, anchor_id, AnchorEntry {
            block_id,
            action: value_to_anchor_action(&file_value),
            drag_payload: DragPayload::FilePath(entry.path.clone()),
            table_cell: None,
        });
        *anchor_idx += 1;

        let source_id = ids::native(block_id);
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
                loading_row = loading_row.push(TextElement::new("Loading...").color(theme::TEXT_MUTED));
                *parent = std::mem::take(parent).push(loading_row);
            }
        }
    }
}

// =========================================================================
// Diff viewer
// =========================================================================

/// Render the diff viewer for reviewing changes.
pub(super) fn render_diff_viewer<'a>(
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
            .color(theme::TEXT_MUTED)
            .source(source_id),
    );

    let viewport_height = DIFF_VIEWPORT_HEIGHT;
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
                theme::TEXT_PATH
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
                        nexus_api::DiffLineKind::Context => (" ", theme::TEXT_SECONDARY),
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
                    .color(theme::TEXT_PRIMARY)
                    .source(source_id),
            );
        }
    }

    // Footer with position info
    let end = viewport_end.min(total_lines);
    if total_lines > viewport_height {
        parent = parent.push(
            TextElement::new(format!("  [{}-{}/{}]", scroll_line + 1, end, total_lines))
                .color(theme::TEXT_MUTED)
                .source(source_id),
        );
    }

    parent
}

// =========================================================================
// Helpers
// =========================================================================

/// Format ETA in human-readable form.
fn format_eta(seconds: f64) -> String {
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
}
