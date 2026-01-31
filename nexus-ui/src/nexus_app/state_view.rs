//! View helpers — named render sections for NexusState.

use strata::{Column, LayoutSnapshot, ScrollColumn};

use super::NexusState;
use crate::nexus_widgets::WelcomeScreen;

impl NexusState {
    pub(super) fn layout_blocks(&self, mut scroll: ScrollColumn) -> ScrollColumn {
        if !self.has_blocks() {
            scroll = scroll.push(WelcomeScreen { cwd: &self.cwd });
        } else {
            // Merge-walk shell + agent blocks in BlockId order without allocation
            let mut si = 0;
            let mut ai = 0;
            while si < self.shell.blocks.len() || ai < self.agent.blocks.len() {
                let take_shell = match (self.shell.blocks.get(si), self.agent.blocks.get(ai)) {
                    (Some(s), Some(a)) => s.id.0 <= a.id.0,
                    (Some(_), None) => true,
                    (None, Some(_)) => false,
                    (None, None) => unreachable!(),
                };
                if take_shell {
                    let block = &self.shell.blocks[si];
                    si += 1;
                    scroll = self.shell.push_block(scroll, block, &self.focus);
                } else {
                    let block = &self.agent.blocks[ai];
                    ai += 1;
                    scroll = self.agent.push_block(scroll, block);
                }
            }
        }
        scroll
    }

    pub(super) fn layout_overlays_and_input(
        &self,
        mut col: Column,
        cursor_visible: bool,
    ) -> Column {
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
