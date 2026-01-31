//! UI policy helpers — focus, cursor, clipboard, terminal sizing.
//!
//! Scroll policy lives in ScrollModel. Transient overlay policy lives in TransientUi.

use std::time::Instant;

use strata::ImageStore;

use super::NexusState;

impl NexusState {
    pub(super) fn next_id(&mut self) -> nexus_api::BlockId {
        let id = nexus_api::BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    // --- Focus ---

    pub(super) fn set_focus_input(&mut self) {
        self.focus = crate::blocks::Focus::Input;
        self.input.text_input.focused = true;
    }

    pub(super) fn set_focus_block(&mut self, id: nexus_api::BlockId) {
        self.focus = crate::blocks::Focus::Block(id);
        self.input.text_input.focused = false;
    }

    // --- Output-arrived tick ---

    /// Called on tick when output has arrived. Delegates to ScrollModel.
    pub(super) fn on_output_arrived(&mut self) {
        if self.shell.needs_redraw() || self.agent.needs_redraw() {
            self.shell.terminal_dirty = false;
            self.agent.dirty = false;
            self.scroll.hint();
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
        self.set_focus_input();
    }

    // --- Cursor ---

    pub(super) fn cursor_visible(&self) -> bool {
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
        let char_width = 14.0 * 0.607;
        let line_height = 14.0 * 1.4;
        let h_padding = 4.0 + 6.0 * 2.0;
        let v_padding = 44.0;
        let cols = ((vw - h_padding) / char_width) as u16;
        let rows = ((vh - v_padding) / line_height) as u16;
        (cols.max(40).min(500), rows.max(5).min(200))
    }
}
