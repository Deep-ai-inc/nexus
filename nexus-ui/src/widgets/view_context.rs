//! View context — bundles commonly-passed rendering parameters.
//!
//! Instead of passing `(&state, &images, &theme, &registry, is_focused)` through
//! 5-7 levels of function calls, widgets receive a single `ViewContext` reference.
//!
//! # Migration Strategy
//!
//! Existing widgets in `nexus_widgets.rs` can be migrated incrementally:
//! 1. New widgets use `ViewContext` from the start
//! 2. Existing widgets can be updated one at a time to accept `ViewContext`
//! 3. Eventually `nexus_widgets.rs` is split into per-widget modules

use std::cell::RefCell;
use std::collections::HashMap;

use strata::content_address::SourceId;
use strata::gpu::ImageHandle;

use crate::nexus_app::shell::{AnchorEntry, ClickAction};

/// Rendering context passed to all widgets.
///
/// Contains read-only globals and the click registry for interactive elements.
pub(crate) struct ViewContext<'a> {
    /// Click registry for anchor/tree-toggle actions that need associated data.
    /// Widgets call `register_anchor()` during view to enable click handling.
    pub click_registry: &'a RefCell<HashMap<SourceId, ClickAction>>,

    /// Optional image handle + dimensions for blocks that display images.
    pub image_info: Option<(ImageHandle, u32, u32)>,

    /// Whether the current widget tree is focused.
    pub is_focused: bool,
}

impl<'a> ViewContext<'a> {
    /// Create a new view context.
    pub(crate) fn new(
        click_registry: &'a RefCell<HashMap<SourceId, ClickAction>>,
        image_info: Option<(ImageHandle, u32, u32)>,
        is_focused: bool,
    ) -> Self {
        Self {
            click_registry,
            image_info,
            is_focused,
        }
    }

    /// Register an anchor click action.
    pub(crate) fn register_anchor(&self, id: SourceId, entry: AnchorEntry) {
        self.click_registry
            .borrow_mut()
            .insert(id, ClickAction::Anchor(entry));
    }

    /// Register a tree-toggle click action.
    pub(crate) fn register_tree_toggle(
        &self,
        id: SourceId,
        block_id: nexus_api::BlockId,
        path: std::path::PathBuf,
    ) {
        self.click_registry
            .borrow_mut()
            .insert(id, ClickAction::TreeToggle { block_id, path });
    }

    /// Create a child context with different focus state.
    pub(crate) fn with_focus(&self, is_focused: bool) -> ViewContext<'a> {
        ViewContext {
            click_registry: self.click_registry,
            image_info: self.image_info,
            is_focused,
        }
    }
}
