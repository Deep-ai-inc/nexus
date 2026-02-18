//! PTY backend — terminal key encoding, PTY handle management, and terminal sizing.
//!
//! This module encapsulates all "legacy terminal" concerns: converting GUI
//! key events to VT byte sequences, managing PTY subprocess handles, and
//! debouncing terminal resize signals.  The rest of the shell widget treats
//! it as an opaque backend.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexus_api::BlockId;
use nexus_term::TerminalParser;
use tokio::sync::{mpsc, Mutex};

use crate::data::{Block, PtyEvent};
use crate::infra::pty_driver::PtyHandle;
use strata::event_context::{Key, KeyEvent, NamedKey};

// =========================================================================
// Terminal key encoding — converts GUI key events to PTY byte sequences
// =========================================================================

/// Flags from the terminal that affect how keys are encoded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TermKeyFlags {
    /// DECCKM: Application Cursor Keys mode.  When true, arrow keys use
    /// SS3 (`\x1bO`) instead of CSI (`\x1b[`).
    pub app_cursor: bool,

    /// macOS "Option as Meta" toggle.  When true, Option+key sends
    /// `\x1b` + key (Meta/Alt behaviour for shells and Emacs).  When false,
    /// the OS-composed character is sent (e.g., Option+a → å).
    pub option_as_meta: bool,
}

impl Default for TermKeyFlags {
    fn default() -> Self {
        Self {
            app_cursor: false,
            // Default to true — terminal users on macOS almost always want
            // Option to behave as Meta for readline/Emacs keybindings.
            option_as_meta: true,
        }
    }
}

/// Encode a key event into the byte sequence a real terminal would send.
///
/// `flags` carries live terminal state (DECCKM, etc.) that affects encoding.
pub(crate) fn strata_key_to_bytes(
    event: &KeyEvent,
    flags: TermKeyFlags,
) -> Option<Vec<u8>> {
    let (key, modifiers, text) = match event {
        KeyEvent::Pressed {
            key,
            modifiers,
            text,
        } => (key, modifiers, text.as_deref()),
        KeyEvent::Released { .. } => return None,
    };

    match key {
        Key::Character(c) => encode_character(c, modifiers, text, flags),
        Key::Named(named) => encode_named(*named, modifiers, flags),
    }
}

// -- Character keys ---------------------------------------------------------

fn encode_character(
    c: &str,
    modifiers: &strata::event_context::Modifiers,
    text: Option<&str>,
    flags: TermKeyFlags,
) -> Option<Vec<u8>> {
    // Ctrl+letter → ASCII control code (0x01–0x1a)
    if modifiers.ctrl && c.len() == 1 {
        let ch = c.chars().next()?;
        if ch.is_ascii_alphabetic() {
            let ctrl_code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
            return Some(vec![ctrl_code]);
        }
        // Ctrl+special punctuation
        match ch {
            ' ' | '2' | '@' => return Some(vec![0x00]), // Ctrl+Space / Ctrl+@
            '[' => return Some(vec![0x1b]),               // Ctrl+[ = Escape
            '\\' => return Some(vec![0x1c]),              // Ctrl+\ = FS (SIGQUIT)
            ']' => return Some(vec![0x1d]),               // Ctrl+] = GS
            '/' => return Some(vec![0x1f]),               // Ctrl+/ = US
            '_' => return Some(vec![0x1f]),               // Ctrl+_ = US
            _ => {}
        }
    }

    // Alt/Option+character handling.
    //
    // When `option_as_meta` is true (default for terminal users):
    //   Option+b → \x1b b  (Meta-b = backward-word in readline/Emacs)
    //   Ignore the OS-composed text (å, ∫, etc.)
    //
    // When `option_as_meta` is false:
    //   Option+a → å  (fall through to normal text path, using OS-composed text)
    if modifiers.alt && flags.option_as_meta {
        let raw = c.as_bytes();
        let mut bytes = Vec::with_capacity(1 + raw.len());
        bytes.push(0x1b);
        bytes.extend_from_slice(raw);
        return Some(bytes);
    }

    // Normal character — prefer OS-composed text (handles Shift, dead keys, IME)
    if let Some(t) = text {
        if !t.is_empty() {
            return Some(t.as_bytes().to_vec());
        }
    }
    Some(c.as_bytes().to_vec())
}

// -- Named keys -------------------------------------------------------------

/// Compute the xterm modifier parameter: shift=2, alt=3, shift+alt=4, ctrl=5,
/// ctrl+shift=6, ctrl+alt=7, ctrl+shift+alt=8.  Returns 0 when no modifiers.
fn modifier_param(m: &strata::event_context::Modifiers) -> u8 {
    let mut p: u8 = 0;
    if m.shift { p |= 1; }
    if m.alt { p |= 2; }
    if m.ctrl { p |= 4; }
    if p == 0 { 0 } else { p + 1 }
}

/// Build a CSI sequence with an optional modifier parameter.
///
/// *Letter-terminated* keys (arrows, Home, End):
///   unmodified  → `\x1b[ <suffix>`
///   modified    → `\x1b[1;<mod> <suffix>`
///
/// *Tilde-terminated* keys (Insert, Delete, PgUp, PgDn, F5+):
///   unmodified  → `\x1b[ <code> ~`
///   modified    → `\x1b[ <code>;<mod> ~`
fn csi_modified_letter(suffix: u8, m: &strata::event_context::Modifiers) -> Vec<u8> {
    let p = modifier_param(m);
    if p == 0 {
        vec![0x1b, b'[', suffix]
    } else {
        vec![0x1b, b'[', b'1', b';', b'0' + p, suffix]
    }
}

fn csi_modified_tilde(code: &[u8], m: &strata::event_context::Modifiers) -> Vec<u8> {
    let p = modifier_param(m);
    let mut v = vec![0x1b, b'['];
    v.extend_from_slice(code);
    if p != 0 {
        v.push(b';');
        v.push(b'0' + p);
    }
    v.push(b'~');
    v
}

/// CSI u sequence for modified keys without a legacy encoding.
///
/// Format: `\x1b[ <unicode_codepoint> ; <modifier> u`
///
/// Part of the "fixterms" / Kitty keyboard protocol, widely supported
/// by iTerm2, Kitty, WezTerm, Ghostty, and modern CLI tools.
/// The `u` terminator avoids conflicts with legacy tilde/letter sequences.
fn csi_u(codepoint: u32, m: &strata::event_context::Modifiers) -> Vec<u8> {
    let p = modifier_param(m);
    let cp = codepoint.to_string();
    let mut v = Vec::with_capacity(cp.len() + 6);
    v.extend_from_slice(b"\x1b[");
    v.extend_from_slice(cp.as_bytes());
    if p != 0 {
        v.push(b';');
        v.push(b'0' + p);
    }
    v.push(b'u');
    v
}

/// SS3 sequence (used for F1-F4 unmodified, and application-mode arrows).
fn ss3(suffix: u8) -> Vec<u8> {
    vec![0x1b, b'O', suffix]
}

fn encode_named(
    named: NamedKey,
    modifiers: &strata::event_context::Modifiers,
    flags: TermKeyFlags,
) -> Option<Vec<u8>> {
    let m = modifiers;
    let has_mods = m.shift || m.alt || m.ctrl;

    match named {
        // -- Simple keys (no CSI) ------------------------------------------
        NamedKey::Enter => {
            if has_mods {
                Some(csi_u(13, m)) // CR codepoint
            } else {
                Some(vec![b'\r'])
            }
        }
        NamedKey::Escape => {
            if has_mods {
                Some(csi_u(27, m)) // ESC codepoint
            } else {
                Some(vec![0x1b])
            }
        }
        NamedKey::Space => {
            if m.ctrl && !m.shift && !m.alt {
                Some(vec![0x00]) // Ctrl+Space = NUL (legacy)
            } else if has_mods {
                Some(csi_u(b' ' as u32, m))
            } else {
                Some(vec![b' '])
            }
        }
        NamedKey::Backspace => {
            if m.ctrl && !m.shift && !m.alt {
                Some(vec![0x08]) // Ctrl+Backspace = BS (legacy)
            } else if m.alt && !m.shift && !m.ctrl {
                Some(vec![0x1b, 0x7f]) // Alt+Backspace = ESC DEL (legacy)
            } else if has_mods {
                Some(csi_u(0x7f, m)) // Other combos via CSI u
            } else {
                Some(vec![0x7f]) // Backspace = DEL
            }
        }
        NamedKey::Tab => {
            if m.shift && !m.alt && !m.ctrl {
                Some(vec![0x1b, b'[', b'Z']) // Shift+Tab = backtab (legacy)
            } else if has_mods {
                Some(csi_u(b'\t' as u32, m))
            } else {
                Some(vec![b'\t'])
            }
        }

        // -- Arrow keys (DECCKM-aware) -------------------------------------
        NamedKey::ArrowUp | NamedKey::ArrowDown |
        NamedKey::ArrowRight | NamedKey::ArrowLeft => {
            let suffix = match named {
                NamedKey::ArrowUp => b'A',
                NamedKey::ArrowDown => b'B',
                NamedKey::ArrowRight => b'C',
                NamedKey::ArrowLeft => b'D',
                _ => unreachable!(),
            };
            if has_mods {
                Some(csi_modified_letter(suffix, m))
            } else if flags.app_cursor {
                Some(ss3(suffix))
            } else {
                Some(vec![0x1b, b'[', suffix])
            }
        }

        // -- Home / End (letter-terminated) --------------------------------
        NamedKey::Home => Some(csi_modified_letter(b'H', m)),
        NamedKey::End => Some(csi_modified_letter(b'F', m)),

        // -- Tilde-terminated keys -----------------------------------------
        NamedKey::Insert => Some(csi_modified_tilde(b"2", m)),
        NamedKey::Delete => Some(csi_modified_tilde(b"3", m)),
        NamedKey::PageUp => Some(csi_modified_tilde(b"5", m)),
        NamedKey::PageDown => Some(csi_modified_tilde(b"6", m)),

        // -- Function keys -------------------------------------------------
        // F1-F4: SS3 when unmodified, CSI with modifier when modified
        NamedKey::F1 => if has_mods { Some(csi_modified_tilde(b"11", m)) } else { Some(ss3(b'P')) },
        NamedKey::F2 => if has_mods { Some(csi_modified_tilde(b"12", m)) } else { Some(ss3(b'Q')) },
        NamedKey::F3 => if has_mods { Some(csi_modified_tilde(b"13", m)) } else { Some(ss3(b'R')) },
        NamedKey::F4 => if has_mods { Some(csi_modified_tilde(b"14", m)) } else { Some(ss3(b'S')) },
        // F5-F12: always tilde-terminated
        NamedKey::F5 => Some(csi_modified_tilde(b"15", m)),
        NamedKey::F6 => Some(csi_modified_tilde(b"17", m)),
        NamedKey::F7 => Some(csi_modified_tilde(b"18", m)),
        NamedKey::F8 => Some(csi_modified_tilde(b"19", m)),
        NamedKey::F9 => Some(csi_modified_tilde(b"20", m)),
        NamedKey::F10 => Some(csi_modified_tilde(b"21", m)),
        NamedKey::F11 => Some(csi_modified_tilde(b"23", m)),
        NamedKey::F12 => Some(csi_modified_tilde(b"24", m)),

        _ => None,
    }
}

// =========================================================================
// PTY backend — handle management, key forwarding, paste, sizing
// =========================================================================

/// Manages PTY subprocess handles and terminal sizing state.
///
/// This is the "legacy terminal" backend: it owns the PTY file descriptors,
/// translates GUI events into VT byte sequences, and debounces resize
/// signals.  The shell widget delegates all PTY operations here.
pub(crate) struct PtyBackend {
    pub(super) handles: Vec<PtyHandle>,
    pub(super) tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    pub(crate) rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,

    /// Current terminal grid size (cols, rows) — set from the view pass.
    pub(crate) terminal_size: Cell<(u16, u16)>,
    /// Last size committed to all block parsers.
    last_parser_size: Cell<(u16, u16)>,
    /// Last size sent to PTY handles (avoids redundant SIGWINCH).
    last_pty_size: Cell<(u16, u16)>,
    /// Pending column downsize: `(target_size, first_seen)`. The timer
    /// restarts whenever the target changes, so the reflow only commits
    /// once the size has been stable for the debounce window.
    pending_downsize: Cell<Option<((u16, u16), Instant)>>,
}

impl PtyBackend {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            handles: Vec::new(),
            tx,
            rx: Arc::new(Mutex::new(rx)),
            terminal_size: Cell::new((120, 24)),
            last_parser_size: Cell::new((120, 24)),
            last_pty_size: Cell::new((120, 24)),
            pending_downsize: Cell::new(None),
        }
    }

    /// Whether a PTY handle exists for this block.
    pub fn has_handle(&self, block_id: BlockId) -> bool {
        self.handles.iter().any(|h| h.block_id == block_id)
    }

    /// Send interrupt (Ctrl+C / SIGINT) to a PTY.
    pub fn send_interrupt(&self, block_id: BlockId) {
        if let Some(handle) = self.handles.iter().find(|h| h.block_id == block_id) {
            let _ = handle.send_interrupt();
        }
    }

    /// Send interrupt + kill signal to a PTY.
    pub fn kill(&self, block_id: BlockId) {
        if let Some(handle) = self.handles.iter().find(|h| h.block_id == block_id) {
            let _ = handle.send_interrupt();
            handle.kill();
        }
    }

    /// Remove the handle for a block (called on PTY exit).
    pub fn remove_handle(&mut self, block_id: BlockId) {
        self.handles.retain(|h| h.block_id != block_id);
    }

    /// Kill all PTYs and clear handles.
    pub fn kill_all(&mut self) {
        for handle in &self.handles {
            let _ = handle.send_interrupt();
            handle.kill();
        }
        self.handles.clear();
    }

    /// Forward a key event to a PTY. Returns false if no handle exists.
    pub fn forward_key(&self, block: Option<&Block>, block_id: BlockId, event: &KeyEvent) -> bool {
        if let Some(handle) = self.handles.iter().find(|h| h.block_id == block_id) {
            let flags = block
                .map(|b| TermKeyFlags { app_cursor: b.parser.app_cursor(), ..TermKeyFlags::default() })
                .unwrap_or_default();
            if let Some(bytes) = strata_key_to_bytes(event, flags) {
                let _ = handle.write(&bytes);
            }
            true
        } else {
            false
        }
    }

    /// Paste text into a PTY, respecting Bracketed Paste mode.
    ///
    /// If the terminal has enabled bracketed paste (`\x1b[?2004h`), the text
    /// is wrapped in `\x1b[200~` / `\x1b[201~` to prevent accidental command
    /// execution.
    pub fn paste_to_pty(&self, block: Option<&Block>, block_id: BlockId, text: &str) -> bool {
        if let Some(handle) = self.handles.iter().find(|h| h.block_id == block_id) {
            let bracketed = block
                .map(|b| b.parser.bracketed_paste())
                .unwrap_or(false);
            if bracketed {
                let _ = handle.write(b"\x1b[200~");
                let _ = handle.write(text.as_bytes());
                let _ = handle.write(b"\x1b[201~");
            } else {
                let _ = handle.write(text.as_bytes());
            }
            true
        } else {
            false
        }
    }

    /// Spawn a PTY subprocess. Returns `Err` with a message on failure.
    pub fn spawn(
        &mut self,
        cmd: &str,
        block_id: BlockId,
        cwd: &str,
    ) -> Result<(), String> {
        let (cols, rows) = self.terminal_size.get();
        match PtyHandle::spawn_with_size(cmd, cwd, block_id, self.tx.clone(), cols, rows) {
            Ok(handle) => {
                self.handles.push(handle);
                Ok(())
            }
            Err(e) => Err(format!("{}", e)),
        }
    }

    /// Propagate terminal size changes to block parsers.
    ///
    /// Uses an asymmetric strategy:
    ///   - **Upsizing / height-only**: resize parser immediately.
    ///   - **Column downsize**: delay the column reflow until the target
    ///     size has been stable for ~32ms.
    ///
    /// PTY handles are resized via `sync_pty_sizes()` in `view()`.
    pub fn sync_terminal_size(&self, blocks: &mut [Block]) {
        let current_size = self.terminal_size.get();
        let (target_cols, target_rows) = current_size;
        let (parser_cols, parser_rows) = self.last_parser_size.get();

        if (target_cols, target_rows) == (parser_cols, parser_rows) {
            self.pending_downsize.set(None);
            return;
        }

        // Upsizing or width unchanged: resize parser immediately.
        if target_cols >= parser_cols {
            self.last_parser_size.set(current_size);
            self.pending_downsize.set(None);
            for block in blocks.iter_mut() {
                block.parser.resize(target_cols, target_rows);
            }
            return;
        }

        // Column downsize: apply row changes immediately, delay column reflow.
        if target_rows != parser_rows {
            self.last_parser_size.set((parser_cols, target_rows));
            for block in blocks.iter_mut() {
                block.parser.resize(parser_cols, target_rows);
            }
        }

        const DEBOUNCE: Duration = Duration::from_millis(32);
        match self.pending_downsize.get() {
            Some((pending_size, started))
                if pending_size == current_size && started.elapsed() >= DEBOUNCE =>
            {
                self.last_parser_size.set(current_size);
                self.pending_downsize.set(None);
                for block in blocks.iter_mut() {
                    block.parser.resize(target_cols, target_rows);
                }
            }
            Some((pending_size, _)) if pending_size == current_size => {
                // Still waiting for debounce to expire.
            }
            _ => {
                self.pending_downsize.set(Some((current_size, Instant::now())));
            }
        }
    }

    /// Send PTY resize (SIGWINCH) to all handles when size changes.
    ///
    /// Only sends when the size actually changes (avoids redundant signals
    /// every frame).
    pub fn sync_pty_sizes(&self) {
        let current_size = self.terminal_size.get();
        if current_size != self.last_pty_size.get() {
            self.last_pty_size.set(current_size);
            let (cols, rows) = current_size;
            for handle in &self.handles {
                let _ = handle.resize(cols, rows);
            }
        }
    }

    /// Create a new parser sized to the current terminal dimensions.
    pub fn new_parser(&self) -> TerminalParser {
        let (cols, rows) = self.terminal_size.get();
        TerminalParser::new(cols, rows)
    }
}
