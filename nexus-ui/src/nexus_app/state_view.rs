//! View helpers — named render sections for NexusState.

use strata::{Column, LayoutSnapshot, ScrollColumn};

use super::NexusState;
use crate::widgets::WelcomeScreen;

impl NexusState {
    pub(super) fn layout_blocks<'a>(&'a self, mut scroll: ScrollColumn<'a>) -> ScrollColumn<'a> {
        if !self.has_blocks() {
            scroll = scroll.push(WelcomeScreen { cwd: &self.cwd });
        } else {
            // Use shared ordered block list (same order as navigation helpers)
            for id in self.all_block_ids_ordered() {
                if let Some(block) = self.shell.block_by_id(id) {
                    scroll = self.shell.push_block(scroll, block, &self.focus);
                } else if let Some(&idx) = self.agent.block_index.get(&id) {
                    if let Some(block) = self.agent.blocks.get(idx) {
                        scroll = self.agent.push_block(scroll, block);
                    }
                }
            }
        }
        scroll
    }

    pub(super) fn layout_overlays_and_input<'a>(
        &'a self,
        mut col: Column<'a>,
        cursor_visible: bool,
    ) -> Column<'a> {
        // Job bar (shell-owned data, placed in overlay area)
        if let Some(job_bar) = self.shell.view_job_bar() {
            col = col.push(job_bar);
        }

        // Input-owned sections: completion popup, history search, attachments, input bar
        col = self.input.layout_overlays(col);
        col = self.input.layout_attachments(col);
        col = self.input.layout_input_bar(col, &self.cwd, self.shell.last_exit_code, cursor_visible);
        col
    }

    pub(super) fn sync_scroll_states(&self, snapshot: &mut LayoutSnapshot) {
        self.scroll.sync_from_snapshot(snapshot);
        self.input.sync_scroll_states(snapshot);
    }
}
