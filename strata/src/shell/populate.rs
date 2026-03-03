//! Shared pipeline population logic.
//!
//! Walks a `LayoutSnapshot` and calls drawing methods on `StrataPipeline`.
//! Used by both the macOS native backend and the Linux winit backend.

use crate::content_address::Selection;
use crate::gpu::StrataPipeline;
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Color, Rect};

/// Base font size for terminal grid text (before scaling).
pub(crate) const BASE_FONT_SIZE: f32 = 14.0;

/// Populate the pipeline with all primitives from the layout snapshot.
pub(crate) fn populate_pipeline(
    pipeline: &mut StrataPipeline,
    snapshot: &LayoutSnapshot,
    selection: Option<&Selection>,
    scale: f32,
    font_system: &mut cosmic_text::FontSystem,
) {
    let primitives = snapshot.primitives();

    #[inline]
    fn clip_to_gpu(clip: &Option<Rect>, scale: f32) -> Option<[f32; 4]> {
        clip.map(|c| [c.x * scale, c.y * scale, c.width * scale, c.height * scale])
    }
    #[inline]
    fn maybe_clip(pipeline: &mut StrataPipeline, start: usize, clip: &Option<Rect>, scale: f32) {
        if let Some(gpu_clip) = clip_to_gpu(clip, scale) {
            pipeline.apply_clip_since(start, gpu_clip);
        }
    }
    #[inline]
    fn hash_grid_row(row: &crate::layout_snapshot::GridRow) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for run in &row.runs {
            run.text.hash(&mut hasher);
            run.fg.hash(&mut hasher);
            run.bg.hash(&mut hasher);
            run.col_offset.hash(&mut hasher);
            run.cell_len.hash(&mut hasher);
            use crate::layout_snapshot::UnderlineStyle;
            let ul_bits: u8 = match run.style.underline {
                UnderlineStyle::None => 0, UnderlineStyle::Single => 1,
                UnderlineStyle::Double => 2, UnderlineStyle::Curly => 3,
                UnderlineStyle::Dotted => 4, UnderlineStyle::Dashed => 5,
            };
            let style_bits: u16 = (run.style.bold as u16)
                | ((run.style.italic as u16) << 1)
                | ((run.style.strikethrough as u16) << 2)
                | ((run.style.dim as u16) << 3)
                | ((ul_bits as u16) << 4);
            style_bits.hash(&mut hasher);
        }
        hasher.finish()
    }

    for decoration in snapshot.background_decorations() { render_decoration(pipeline, decoration, scale); }

    for prim in &primitives.shadows {
        let start = pipeline.instance_count();
        pipeline.add_shadow(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.blur_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.rounded_rects {
        let start = pipeline.instance_count();
        pipeline.add_rounded_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.circles {
        let start = pipeline.instance_count();
        pipeline.add_circle(prim.center.x * scale, prim.center.y * scale, prim.radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.solid_rects {
        let start = pipeline.instance_count();
        pipeline.add_solid_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.borders {
        let start = pipeline.instance_count();
        pipeline.add_border(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.border_width * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.lines {
        let start = pipeline.instance_count();
        pipeline.add_line_styled(prim.p1.x * scale, prim.p1.y * scale, prim.p2.x * scale, prim.p2.y * scale, prim.thickness * scale, prim.color, convert_line_style(prim.style));
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.polylines {
        let start = pipeline.instance_count();
        let scaled_points: Vec<[f32; 2]> = prim.points.iter().map(|p| [p.x * scale, p.y * scale]).collect();
        pipeline.add_polyline_styled(&scaled_points, prim.thickness * scale, prim.color, convert_line_style(prim.style));
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &primitives.images {
        let start = pipeline.instance_count();
        pipeline.add_image(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.handle, prim.corner_radius * scale, prim.tint);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }

    for (source_id, source_layout) in snapshot.sources_in_order() {
        for (phys_idx, item) in source_layout.items.iter().enumerate() {
            let item_index = source_layout.logical_index(phys_idx);
            if let crate::layout_snapshot::ItemLayout::Grid(grid_layout) = item {
                let grid_clip = &grid_layout.clip_rect;
                let cell_w = grid_layout.cell_width * scale;
                let cell_h = grid_layout.cell_height * scale;
                pipeline.ensure_grid_cache(grid_layout.cols, grid_layout.rows_content.len(), grid_layout.bounds.x);

                let num_rows = grid_layout.rows_content.len();
                let (first_vis, last_vis) = if let Some(ref clip) = *grid_clip {
                    let first = ((clip.y - grid_layout.bounds.y) / grid_layout.cell_height)
                        .floor().max(0.0) as usize;
                    let last = ((clip.y + clip.height - grid_layout.bounds.y) / grid_layout.cell_height)
                        .ceil().max(0.0) as usize;
                    (first.min(num_rows), last.min(num_rows))
                } else {
                    (0, num_rows)
                };

                for row_idx in first_vis..last_vis {
                    let row = &grid_layout.rows_content[row_idx];
                    if row.runs.is_empty() { continue; }
                    let signature = hash_grid_row(row);
                    let Some(build_start) = pipeline.begin_grid_row(row_idx, signature) else { continue; };
                    let row_y = (grid_layout.bounds.y + row_idx as f32 * grid_layout.cell_height) * scale;
                    let base_x = grid_layout.bounds.x * scale;

                    for run in &row.runs {
                        let run_x = base_x + run.col_offset as f32 * cell_w;
                        let run_w = run.cell_len as f32 * cell_w;

                        if run.bg != 0 {
                            pipeline.add_solid_rect(run_x, row_y, run_w, cell_h, Color::unpack(run.bg));
                        }

                        let mut fg_color = Color::unpack(run.fg);
                        if run.style.dim { fg_color.a *= 0.5; }

                        render_run_foreground(pipeline, run, base_x, row_y, fg_color, cell_w, cell_h, scale, font_system);
                    }
                    pipeline.end_grid_row(row_idx, signature, build_start, row_y);
                }
                let grid_base_y = grid_layout.bounds.y * scale;
                pipeline.gather_grid_rows(grid_base_y, cell_h, grid_layout.rows_content.len(), clip_to_gpu(grid_clip, scale));

                // Draw terminal cursor
                if let Some(ref cursor) = grid_layout.cursor {
                    let cursor_start = pipeline.instance_count();
                    use crate::layout_snapshot::GridCursorShape;
                    let cx = (grid_layout.bounds.x + cursor.col as f32 * grid_layout.cell_width) * scale;
                    let cy = (grid_layout.bounds.y + cursor.row as f32 * grid_layout.cell_height) * scale;
                    let cursor_fg = if cursor.fg != 0 {
                        Color::unpack(cursor.fg)
                    } else {
                        Color::rgb(0.9, 0.9, 0.9)
                    };
                    let cursor_bg = if cursor.bg != 0 {
                        Color::unpack(cursor.bg)
                    } else {
                        Color::rgb(0.12, 0.12, 0.12)
                    };

                    match cursor.shape {
                        GridCursorShape::Block => {
                            pipeline.add_solid_rect(cx, cy, cell_w, cell_h, cursor_fg);
                            if cursor.ch != ' ' && cursor.ch != '\0' {
                                let mut ch_buf = [0u8; 4];
                                let ch_str = cursor.ch.encode_utf8(&mut ch_buf);
                                pipeline.add_text_grid(ch_str, cx, cy, cursor_bg, BASE_FONT_SIZE * scale, false, false, font_system);
                            }
                        }
                        GridCursorShape::HollowBlock => {
                            let t = scale.max(1.0);
                            pipeline.add_solid_rect(cx, cy, cell_w, t, cursor_fg);
                            pipeline.add_solid_rect(cx, cy + cell_h - t, cell_w, t, cursor_fg);
                            pipeline.add_solid_rect(cx, cy, t, cell_h, cursor_fg);
                            pipeline.add_solid_rect(cx + cell_w - t, cy, t, cell_h, cursor_fg);
                        }
                        GridCursorShape::Beam => {
                            pipeline.add_solid_rect(cx, cy, (2.0 * scale).max(1.0), cell_h, cursor_fg);
                        }
                        GridCursorShape::Underline => {
                            pipeline.add_solid_rect(cx, cy + cell_h - (2.0 * scale).max(1.0), cell_w, (2.0 * scale).max(1.0), cursor_fg);
                        }
                    }
                    maybe_clip(pipeline, cursor_start, grid_clip, scale);
                }

                // Grid selection overlay
                if let Some(sel) = selection {
                    render_grid_selection(
                        pipeline, snapshot, &source_id, item_index, grid_layout,
                        sel, scale, font_system, grid_clip,
                    );
                }
            }
        }
    }

    let viewport_bottom = snapshot.viewport().height;
    for prim in &primitives.text_runs {
        if prim.position.y > viewport_bottom || prim.position.y + prim.font_size * 1.5 < 0.0 { continue; }
        if let Some(clip) = &prim.clip_rect {
            if clip.y > viewport_bottom || (clip.y + clip.height) < 0.0 { continue; }
        }
        let start = pipeline.instance_count();
        pipeline.add_text(&prim.text, prim.position.x * scale, prim.position.y * scale, prim.color, prim.font_size * scale, font_system);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }

    for decoration in snapshot.foreground_decorations() { render_decoration(pipeline, decoration, scale); }

    // Text selection overlay (non-grid content)
    if let Some(sel) = selection {
        if !sel.is_collapsed() {
            for (r, clip) in &snapshot.text_selection_bounds(sel) {
                let start = pipeline.instance_count();
                let scaled = Rect { x: r.x * scale, y: r.y * scale, width: r.width * scale, height: r.height * scale };
                pipeline.add_solid_rects(&[scaled], crate::gpu::SELECTION_COLOR);
                maybe_clip(pipeline, start, clip, scale);
            }
            for (r, clip) in &snapshot.selection_gap_rects(sel) {
                let start = pipeline.instance_count();
                let scaled = Rect { x: r.x * scale, y: r.y * scale, width: r.width * scale, height: r.height * scale };
                pipeline.add_solid_rects(&[scaled], crate::gpu::SELECTION_COLOR);
                maybe_clip(pipeline, start, clip, scale);
            }
        }
    }

    let overlays = snapshot.overlay_primitives();
    for prim in &overlays.shadows {
        let start = pipeline.instance_count();
        pipeline.add_shadow(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.blur_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.rounded_rects {
        let start = pipeline.instance_count();
        pipeline.add_rounded_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.solid_rects {
        let start = pipeline.instance_count();
        pipeline.add_solid_rect(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.borders {
        let start = pipeline.instance_count();
        pipeline.add_border(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.corner_radius * scale, prim.border_width * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.circles {
        let start = pipeline.instance_count();
        pipeline.add_circle(prim.center.x * scale, prim.center.y * scale, prim.radius * scale, prim.color);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.lines {
        let start = pipeline.instance_count();
        pipeline.add_line_styled(prim.p1.x * scale, prim.p1.y * scale, prim.p2.x * scale, prim.p2.y * scale, prim.thickness * scale, prim.color, convert_line_style(prim.style));
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.polylines {
        let start = pipeline.instance_count();
        let scaled_points: Vec<[f32; 2]> = prim.points.iter().map(|p| [p.x * scale, p.y * scale]).collect();
        pipeline.add_polyline_styled(&scaled_points, prim.thickness * scale, prim.color, convert_line_style(prim.style));
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.images {
        let start = pipeline.instance_count();
        pipeline.add_image(prim.rect.x * scale, prim.rect.y * scale, prim.rect.width * scale, prim.rect.height * scale, prim.handle, prim.corner_radius * scale, prim.tint);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
    for prim in &overlays.text_runs {
        let start = pipeline.instance_count();
        pipeline.add_text_styled(&prim.text, prim.position.x * scale, prim.position.y * scale, prim.color, prim.font_size * scale, prim.bold, prim.italic, font_system);
        maybe_clip(pipeline, start, &prim.clip_rect, scale);
    }
}

fn render_run_foreground(
    pipeline: &mut StrataPipeline,
    run: &crate::layout_snapshot::TextRun,
    base_x: f32,
    row_y: f32,
    fg_color: Color,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    font_system: &mut cosmic_text::FontSystem,
) {
    let run_x = base_x + run.col_offset as f32 * cell_w;
    let run_w = run.cell_len as f32 * cell_w;
    let is_whitespace = run.text.trim().is_empty();

    if !is_whitespace {
        let has_custom = run.text.chars().any(crate::gpu::is_custom_drawn);
        if has_custom {
            use unicode_width::UnicodeWidthChar;
            let mut col = 0usize;
            let mut text_buf = String::new();
            let mut text_col_start = 0usize;
            for ch in run.text.chars() {
                let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                if ch_width == 0 { text_buf.push(ch); continue; }
                if crate::gpu::is_custom_drawn(ch) {
                    if !text_buf.is_empty() {
                        pipeline.add_text_grid(&text_buf, run_x + text_col_start as f32 * cell_w, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, font_system);
                        text_buf.clear();
                    }
                    let cx = run_x + col as f32 * cell_w;
                    if !pipeline.draw_box_char(ch, cx, row_y, cell_w, cell_h, fg_color)
                        && !pipeline.draw_block_char(ch, cx, row_y, cell_w, cell_h, fg_color) {
                        if text_buf.is_empty() { text_col_start = col; }
                        text_buf.push(ch);
                    }
                    col += 1;
                } else {
                    if text_buf.is_empty() { text_col_start = col; }
                    text_buf.push(ch);
                    col += ch_width;
                }
            }
            if !text_buf.is_empty() {
                pipeline.add_text_grid(&text_buf, run_x + text_col_start as f32 * cell_w, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, font_system);
            }
        } else {
            pipeline.add_text_grid(&run.text, run_x, row_y, fg_color, BASE_FONT_SIZE * scale, run.style.bold, run.style.italic, font_system);
        }
    }

    {
        use crate::layout_snapshot::UnderlineStyle;
        let ul_thickness = scale.max(1.0);
        match run.style.underline {
            UnderlineStyle::None => {}
            UnderlineStyle::Single | UnderlineStyle::Curly | UnderlineStyle::Dotted | UnderlineStyle::Dashed => {
                pipeline.add_solid_rect(run_x, row_y + cell_h * 0.85, run_w, ul_thickness, fg_color);
            }
            UnderlineStyle::Double => {
                let gap = (2.0 * scale).max(2.0);
                pipeline.add_solid_rect(run_x, row_y + cell_h * 0.82, run_w, ul_thickness, fg_color);
                pipeline.add_solid_rect(run_x, row_y + cell_h * 0.82 + gap, run_w, ul_thickness, fg_color);
            }
        }
    }
    if run.style.strikethrough {
        pipeline.add_solid_rect(run_x, row_y + cell_h * 0.5, run_w, 1.0 * scale, fg_color);
    }
}

fn render_grid_selection(
    pipeline: &mut StrataPipeline,
    snapshot: &LayoutSnapshot,
    source_id: &crate::content_address::SourceId,
    item_index: usize,
    grid_layout: &crate::layout_snapshot::GridLayout,
    selection: &Selection,
    scale: f32,
    font_system: &mut cosmic_text::FontSystem,
    grid_clip: &Option<Rect>,
) {
    let cell_count = grid_layout.cell_count();
    let Some((sel_start, sel_end)) = snapshot.grid_selection_offsets(
        source_id, item_index, cell_count, selection,
    ) else {
        return;
    };

    let cols = grid_layout.cols as usize;
    let cell_w = grid_layout.cell_width * scale;
    let cell_h = grid_layout.cell_height * scale;
    let base_x = grid_layout.bounds.x * scale;
    let base_y = grid_layout.bounds.y * scale;
    let sel_fg = crate::gpu::GRID_SELECTION_FG;
    let sel_bg = crate::gpu::GRID_SELECTION_BG;

    let gpu_grid_clip = grid_clip.map(|c| [c.x * scale, c.y * scale, c.width * scale, c.height * scale]);

    let (start_col, start_row) = grid_layout.offset_to_grid(sel_start);
    let (end_col, end_row) = grid_layout.offset_to_grid(sel_end.saturating_sub(1));
    let last_row = (end_row as usize).min(grid_layout.rows_content.len().saturating_sub(1));

    let rect_col_range = if let crate::content_address::SelectionShape::Rectangular { x_min, x_max } = selection.shape {
        let col_start = ((x_min - grid_layout.bounds.x) / grid_layout.cell_width).floor().max(0.0) as usize;
        let col_end = ((x_max - grid_layout.bounds.x) / grid_layout.cell_width).ceil().min(cols as f32) as usize;
        Some((col_start, col_end))
    } else {
        None
    };

    for row_idx in (start_row as usize)..=last_row {
        let row = &grid_layout.rows_content[row_idx];
        let row_y = base_y + row_idx as f32 * grid_layout.cell_height * scale;

        let (row_sel_start, row_sel_end) = if let Some((cs, ce)) = rect_col_range {
            (cs, ce)
        } else {
            let rs = if row_idx == start_row as usize { start_col as usize } else { 0 };
            let re = if row_idx == end_row as usize { end_col as usize + 1 } else { cols };
            (rs, re)
        };

        let bg_inst = pipeline.instance_count();
        let bg_x = base_x + row_sel_start as f32 * cell_w;
        let bg_w = (row_sel_end - row_sel_start) as f32 * cell_w;
        pipeline.add_solid_rect(bg_x, row_y, bg_w, cell_h, sel_bg);
        if let Some(gc) = gpu_grid_clip {
            pipeline.apply_clip_since(bg_inst, gc);
        }

        for run in &row.runs {
            let run_start = run.col_offset as usize;
            let run_end = run_start + run.cell_len as usize;

            if run_end <= row_sel_start || run_start >= row_sel_end {
                continue;
            }

            let run_inst = pipeline.instance_count();
            render_run_foreground(pipeline, run, base_x, row_y, sel_fg, cell_w, cell_h, scale, font_system);

            if run_start < row_sel_start || run_end > row_sel_end {
                let clip_x = base_x + row_sel_start.max(run_start) as f32 * cell_w;
                let clip_r = base_x + row_sel_end.min(run_end) as f32 * cell_w;
                let mut clip = [clip_x, row_y, clip_r - clip_x, cell_h];
                if let Some(gc) = gpu_grid_clip {
                    clip = intersect_clips(clip, gc);
                }
                pipeline.apply_clip_since(run_inst, clip);
            } else if let Some(gc) = gpu_grid_clip {
                pipeline.apply_clip_since(run_inst, gc);
            }
        }
    }
}

#[inline]
fn intersect_clips(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let r = (a[0] + a[2]).min(b[0] + b[2]);
    let bot = (a[1] + a[3]).min(b[1] + b[3]);
    [x, y, (r - x).max(0.0), (bot - y).max(0.0)]
}

fn render_decoration(pipeline: &mut StrataPipeline, decoration: &crate::layout_snapshot::Decoration, scale: f32) {
    use crate::layout_snapshot::Decoration;
    match decoration {
        Decoration::SolidRect { rect, color } => {
            pipeline.add_solid_rect(rect.x * scale, rect.y * scale, rect.width * scale, rect.height * scale, *color);
        }
        Decoration::RoundedRect { rect, corner_radius, color } => {
            pipeline.add_rounded_rect(rect.x * scale, rect.y * scale, rect.width * scale, rect.height * scale, corner_radius * scale, *color);
        }
        Decoration::Circle { center, radius, color } => {
            pipeline.add_circle(center.x * scale, center.y * scale, radius * scale, *color);
        }
    }
}

pub(crate) fn convert_line_style(style: crate::layout::primitives::LineStyle) -> crate::gpu::LineStyle {
    match style {
        crate::layout::primitives::LineStyle::Solid => crate::gpu::LineStyle::Solid,
        crate::layout::primitives::LineStyle::Dashed => crate::gpu::LineStyle::Dashed,
        crate::layout::primitives::LineStyle::Dotted => crate::gpu::LineStyle::Dotted,
    }
}
