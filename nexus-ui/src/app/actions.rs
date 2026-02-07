//! State actions — focus, cursor, clipboard, terminal sizing, block navigation.

use std::time::Instant;

use strata::ImageStore;

use super::NexusState;

impl NexusState {
    pub(super) fn next_id(&mut self) -> nexus_api::BlockId {
        let id = self.next_block_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        nexus_api::BlockId(id)
    }

    // --- Focus ---

    pub(super) fn set_focus(&mut self, focus: crate::data::Focus) {
        self.focus = focus;
        self.input.text_input.focused = matches!(focus, crate::data::Focus::Input);
        self.agent.question_input.focused = matches!(focus, crate::data::Focus::AgentInput);
    }

    // --- Output-arrived tick ---

    /// Called on tick when output has arrived. Delegates to ScrollModel.
    pub(super) fn on_output_arrived(&mut self) {
        if self.shell.needs_redraw() || self.agent.needs_redraw() {
            self.shell.terminal_dirty = false;
            self.agent.dirty = false;
            self.scroll.hint_bottom();
        }

        // Auto-scroll during active drag/selection
        if let Some(delta) = self.drag.auto_scroll.get() {
            self.scroll.apply_user_scroll(strata::ScrollAction::ScrollBy(-delta));
        }
    }

    // --- Clear ---

    pub(super) fn clear_screen(&mut self) {
        self.shell.clear();
        self.agent.clear();
        self.scroll.reset();
        self.transient.dismiss_all(&mut self.input);
        self.set_focus(crate::data::Focus::Input);
    }

    // --- Block navigation ---

    /// All block IDs (shell + agent) in display order (ascending BlockId).
    /// Single source of truth used by both navigation and layout.
    pub(super) fn all_block_ids_ordered(&self) -> Vec<nexus_api::BlockId> {
        let mut ids = Vec::with_capacity(self.shell.blocks.blocks.len() + self.agent.blocks.len());
        let mut si = 0;
        let mut ai = 0;
        while si < self.shell.blocks.blocks.len() || ai < self.agent.blocks.len() {
            let take_shell = match (self.shell.blocks.blocks.get(si), self.agent.blocks.get(ai)) {
                (Some(s), Some(a)) => s.id.0 <= a.id.0,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => unreachable!(),
            };
            if take_shell {
                ids.push(self.shell.blocks.blocks[si].id);
                si += 1;
            } else {
                ids.push(self.agent.blocks[ai].id);
                ai += 1;
            }
        }
        ids
    }

    pub(super) fn prev_block_id(&self, current: nexus_api::BlockId) -> Option<nexus_api::BlockId> {
        let ids = self.all_block_ids_ordered();
        let pos = ids.iter().position(|&id| id == current)?;
        if pos > 0 { Some(ids[pos - 1]) } else { None }
    }

    pub(super) fn next_block_id(&self, current: nexus_api::BlockId) -> Option<nexus_api::BlockId> {
        let ids = self.all_block_ids_ordered();
        let pos = ids.iter().position(|&id| id == current)?;
        ids.get(pos + 1).copied()
    }

    pub(super) fn last_block_id(&self) -> Option<nexus_api::BlockId> {
        self.all_block_ids_ordered().last().copied()
    }

    pub(super) fn block_has_active_pty(&self, id: nexus_api::BlockId) -> bool {
        self.shell.pty.has_handle(id)
    }

    // --- Cursor ---

    pub(super) fn cursor_visible(&self) -> bool {
        // Hide main input cursor when a static block has focus.
        // AgentInput keeps the original blink logic (it has its own cursor).
        if matches!(self.focus, crate::data::Focus::Block(_)) {
            return false;
        }
        let blink_elapsed = Instant::now()
            .duration_since(self.last_edit_time)
            .as_millis();
        (blink_elapsed / 500) % 2 == 0
    }

    pub(super) fn has_blocks(&self) -> bool {
        !self.shell.blocks.is_empty() || !self.agent.blocks.is_empty()
    }

    // --- Clipboard ---

    pub(super) fn set_clipboard_text(text: &str) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text);
        }
    }

    pub(super) fn paste_from_clipboard(&mut self, images: &mut ImageStore) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            // When a PTY block is focused, paste text directly into the
            // terminal (with bracketed paste wrapping if the shell requested
            // it).  If the block has no PTY (e.g. a non-terminal block type),
            // fall through to the normal input/image paste path.
            if let crate::data::Focus::Block(id) = self.focus {
                if let Ok(text) = clipboard.get_text() {
                    if !text.is_empty() && self.shell.paste_to_pty(id, &text) {
                        return;
                    }
                }
            }

            if let Ok(img) = clipboard.get_image() {
                let width = img.width as u32;
                let height = img.height as u32;
                let rgba_data = img.bytes.into_owned();

                let mut png_data = Vec::new();
                if let Some(img_buf) =
                    image::RgbaImage::from_raw(width, height, rgba_data.clone())
                {
                    let _ = img_buf.write_to(
                        &mut std::io::Cursor::new(&mut png_data),
                        image::ImageFormat::Png,
                    );
                }

                if !png_data.is_empty() {
                    let handle = images.load_rgba(width, height, rgba_data);
                    self.input.add_attachment(super::Attachment {
                        data: png_data,
                        image_handle: handle,
                        width,
                        height,
                    });
                }
            } else if let Ok(text) = clipboard.get_text() {
                if !text.is_empty() {
                    self.input.paste_text(&text);
                }
            }
        }
    }

    // --- Terminal sizing ---

    /// Pure computation of terminal grid dimensions from viewport size.
    pub(super) fn compute_terminal_size(vw: f32, vh: f32) -> (u16, u16) {
        let char_width = 8.4;
        let line_height = 18.0;
        let h_padding = 4.0 + 6.0 * 2.0;
        let v_padding = 44.0;
        let cols = ((vw - h_padding) / char_width) as u16;
        let rows = ((vh - v_padding) / line_height) as u16;
        (cols.max(40).min(500), rows.max(5).min(200))
    }
}
