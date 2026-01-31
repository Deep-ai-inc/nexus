//! View helpers — named render sections for NexusState.

use strata::{
    ButtonElement, Column, CrossAxisAlignment, ImageElement, LayoutSnapshot, Length, Padding, Row,
    ScrollColumn,
};

use super::colors;
use super::source_ids;
use super::NexusState;
use crate::nexus_widgets::{
    AgentBlockWidget, CompletionPopup, HistorySearchBar, JobBar, NexusInputBar,
    ShellBlockWidget, WelcomeScreen,
};

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
                    let kill_id = source_ids::kill(block.id);
                    let image_info = self.shell.image_handles.get(&block.id).copied();
                    let is_focused =
                        matches!(self.focus, crate::blocks::Focus::Block(id) if id == block.id);
                    scroll = scroll.push(ShellBlockWidget {
                        block,
                        kill_id,
                        image_info,
                        is_focused,
                    });
                } else {
                    let block = &self.agent.blocks[ai];
                    ai += 1;
                    scroll = scroll.push(AgentBlockWidget {
                        block,
                        thinking_toggle_id: source_ids::agent_thinking_toggle(block.id),
                        stop_id: source_ids::agent_stop(block.id),
                    });
                }
            }
        }
        scroll
    }

    pub(super) fn layout_overlays(&self, mut col: Column) -> Column {
        if !self.shell.jobs.is_empty() {
            col = col.push(JobBar {
                jobs: &self.shell.jobs,
            });
        }

        if self.input.completion.is_active() {
            col = col.push(CompletionPopup {
                completions: &self.input.completion.completions,
                selected_index: self.input.completion.index,
                hovered_index: self.input.completion.hovered.get(),
                scroll: &self.input.completion.scroll,
            });
        }

        if self.input.history_search.is_active() {
            col = col.push(HistorySearchBar {
                query: &self.input.history_search.query,
                results: &self.input.history_search.results,
                result_index: self.input.history_search.index,
                hovered_index: self.input.history_search.hovered.get(),
                scroll: &self.input.history_search.scroll,
            });
        }

        col
    }

    pub(super) fn layout_attachments(&self, mut col: Column) -> Column {
        if self.input.attachments.is_empty() {
            return col;
        }

        let mut attach_row = Row::new().spacing(8.0).padding(4.0);
        for (i, attachment) in self.input.attachments.iter().enumerate() {
            let scale = (60.0_f32 / attachment.width as f32)
                .min(60.0 / attachment.height as f32)
                .min(1.0);
            let w = attachment.width as f32 * scale;
            let h = attachment.height as f32 * scale;
            let remove_id = source_ids::remove_attachment(i);
            attach_row = attach_row.push(
                Column::new()
                    .spacing(2.0)
                    .cross_align(CrossAxisAlignment::Center)
                    .image(ImageElement::new(attachment.image_handle, w, h).corner_radius(4.0))
                    .push(
                        ButtonElement::new(remove_id, "\u{2715}")
                            .background(colors::BTN_DENY)
                            .corner_radius(4.0),
                    ),
            );
        }
        col = col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 0.0, 4.0))
                .width(Length::Fill)
                .push(attach_row),
        );
        col
    }

    pub(super) fn layout_input_bar(&self, mut col: Column, cursor_visible: bool) -> Column {
        let line_count = {
            let count = self.input.text_input.text.lines().count()
                + if self.input.text_input.text.ends_with('\n') {
                    1
                } else {
                    0
                };
            count.max(1).min(6)
        };

        col = col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 4.0, 4.0))
                .width(Length::Fill)
                .push(NexusInputBar {
                    input: &self.input.text_input,
                    mode: self.input.mode,
                    cwd: &self.cwd,
                    last_exit_code: self.shell.last_exit_code,
                    cursor_visible,
                    mode_toggle_id: source_ids::mode_toggle(),
                    line_count,
                }),
        );
        col
    }

    pub(super) fn sync_scroll_states(&self, snapshot: &mut LayoutSnapshot) {
        self.scroll.sync_from_snapshot(snapshot);
        self.input.completion.scroll.sync_from_snapshot(snapshot);
        self.input.history_search.scroll.sync_from_snapshot(snapshot);
        self.input.text_input.sync_from_snapshot(snapshot);
    }
}
