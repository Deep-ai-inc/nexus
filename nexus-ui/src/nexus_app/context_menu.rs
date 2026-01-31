//! Context menu types and rendering.

use std::cell::Cell;
use std::path::PathBuf;

use nexus_api::BlockId;
use strata::layout_snapshot::LayoutSnapshot;
use strata::primitives::{Color, Point, Rect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuItem {
    Copy,
    Paste,
    SelectAll,
    Clear,
    CopyCommand,
    CopyOutput,
    CopyAsJson,
    CopyAsTsv,
    Rerun,
    RevealInFinder(PathBuf),
}

impl ContextMenuItem {
    pub fn label(&self) -> &str {
        match self {
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::Clear => "Clear",
            Self::CopyCommand => "Copy Command",
            Self::CopyOutput => "Copy Output",
            Self::CopyAsJson => "Copy as JSON",
            Self::CopyAsTsv => "Copy as TSV",
            Self::Rerun => "Rerun",
            Self::RevealInFinder(_) => "Reveal in Finder",
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            Self::Copy => "\u{2318}C",
            Self::Paste => "\u{2318}V",
            Self::SelectAll => "\u{2318}A",
            _ => "",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ContextTarget {
    Block(BlockId),
    AgentBlock(BlockId),
    Input,
}

pub struct ContextMenuState {
    pub x: f32,
    pub y: f32,
    pub items: Vec<ContextMenuItem>,
    pub target: ContextTarget,
    pub hovered_item: Cell<Option<usize>>,
}

pub fn render_context_menu(snapshot: &mut LayoutSnapshot, menu: &ContextMenuState) {
    let w = 200.0_f32;
    let row_h = 30.0_f32;
    let padding = 6.0_f32;
    let h = menu.items.len() as f32 * row_h + padding * 2.0;

    // Clamp position to stay within viewport
    let vp = snapshot.viewport();
    let x = menu.x.min(vp.width - w - 4.0).max(0.0);
    let y = menu.y.min(vp.height - h - 4.0).max(0.0);

    let p = snapshot.overlay_primitives_mut();

    // Shadow
    p.add_shadow(
        Rect::new(x + 3.0, y + 3.0, w, h),
        8.0, 16.0,
        Color::rgba(0.0, 0.0, 0.0, 0.7),
    );
    // Background — dark opaque
    p.add_rounded_rect(Rect::new(x, y, w, h), 8.0, Color::rgb(0.08, 0.08, 0.10));
    // Border
    p.add_border(Rect::new(x, y, w, h), 8.0, 1.0, Color::rgba(1.0, 1.0, 1.0, 0.15));

    let ix = x + padding;
    let iw = w - padding * 2.0;

    let hovered = menu.hovered_item.get();

    for (i, item) in menu.items.iter().enumerate() {
        let iy = y + padding + i as f32 * row_h;
        let item_rect = Rect::new(ix, iy, iw, row_h - 2.0);

        // Register as clickable widget
        let item_id = super::source_ids::ctx_menu_item(i);
        snapshot.register_widget(item_id, item_rect);

        let p = snapshot.overlay_primitives_mut();

        // Item background — highlight on hover
        let bg = if hovered == Some(i) {
            Color::rgb(0.25, 0.35, 0.55)
        } else {
            Color::rgb(0.15, 0.15, 0.18)
        };
        p.add_rounded_rect(item_rect, 4.0, bg);

        // Label
        p.add_text(item.label(), Point::new(ix + 10.0, iy + 6.0), Color::rgb(0.92, 0.92, 0.92), 14.0);

        // Shortcut hint (right-aligned)
        let shortcut = item.shortcut();
        if !shortcut.is_empty() {
            p.add_text(shortcut, Point::new(ix + iw - 36.0, iy + 6.0), Color::rgb(0.45, 0.45, 0.5), 14.0);
        }
    }
}
