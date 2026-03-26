//! Local echo prediction engine.
//!
//! Tracks predicted keystrokes and overlays them on the terminal grid
//! until the server confirms (via echo epoch) that the PTY has processed
//! the input. Predictions render with a visual hint (dashed underline)
//! so the user perceives them as tentative.
//!
//! For TUI apps that toggle cursor visibility (DECTCEM), the caller
//! should provide the last-known visible cursor position rather than
//! the raw grid cursor — this gives the true input position.

use std::collections::VecDeque;
use std::time::Instant;

use nexus_term::TerminalGrid;

/// A single predicted keystroke.
#[derive(Debug, Clone)]
struct Prediction {
    /// The echo epoch this prediction belongs to.
    epoch: u64,
    /// Grid column where the character should appear.
    col: u16,
    /// Grid row where the character should appear.
    row: u16,
    /// The predicted character.
    ch: char,
    /// When this prediction was created.
    #[allow(dead_code)]
    created: Instant,
}

/// Result of looking up a prediction at a grid position.
#[derive(Debug, Clone, Copy)]
pub struct PredictedCell {
    /// The predicted character.
    pub ch: char,
}

/// The prediction engine. Lives alongside a Block's terminal parser
/// and overlays unconfirmed keystrokes on the authoritative grid.
#[derive(Debug)]
pub struct PredictionEngine {
    /// Pending predictions, ordered by epoch (oldest first).
    pending: VecDeque<Prediction>,
    /// The predicted cursor column offset from the anchor.
    cursor_col_offset: u16,
    /// The anchor position for predictions.
    anchor_col: u16,
    anchor_row: u16,
    /// Grid column count (for line wrapping predictions).
    cols: u16,
    /// Whether prediction is enabled for this block.
    enabled: bool,
    /// Epochs confirmed but not yet matched on the grid.
    grace_pending: VecDeque<GraceEntry>,
    /// Pre-feed snapshot of characters at predicted positions.
    pre_feed_snapshot: Vec<(u16, u16, char)>,
}

/// A prediction awaiting grid confirmation after epoch was acknowledged.
#[derive(Debug, Clone)]
struct GraceEntry {
    col: u16,
    row: u16,
    ch: char,
    confirmed_at: Instant,
}

impl PredictionEngine {
    fn grace_period_ms(rtt_ms: u64) -> u64 {
        (rtt_ms * 3).clamp(200, 5000)
    }

    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            cursor_col_offset: 0,
            anchor_col: 0,
            anchor_row: 0,
            cols: 80,
            enabled: true,
            grace_pending: VecDeque::new(),
            pre_feed_snapshot: Vec::new(),
        }
    }

    pub fn set_cols(&mut self, cols: u16) {
        self.cols = cols;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.reset();
        }
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Placeholder for diagnostics compatibility.
    pub fn learned_offset(&self) -> (i32, i32) {
        (0, 0)
    }

    /// Add a predicted keystroke at the given cursor position.
    /// The caller should pass the best-known cursor position
    /// (e.g., last visible cursor for TUI apps).
    pub fn predict(&mut self, epoch: u64, ch: char, cursor_col: u16, cursor_row: u16) {
        if !self.enabled {
            return;
        }

        if self.pending.is_empty() {
            self.anchor_col = cursor_col;
            self.anchor_row = cursor_row;
            self.cursor_col_offset = 0;
        }

        let predicted_col = self.anchor_col + self.cursor_col_offset;
        let (col, row) = if predicted_col >= self.cols {
            (predicted_col - self.cols, self.anchor_row + 1)
        } else {
            (predicted_col, self.anchor_row)
        };

        self.pending.push_back(Prediction {
            epoch,
            col,
            row,
            ch,
            created: Instant::now(),
        });

        self.cursor_col_offset += 1;
    }

    /// Look up a predicted character at the given grid position.
    pub fn get(&self, col: u16, row: u16) -> Option<PredictedCell> {
        for pred in &self.pending {
            if pred.col == col && pred.row == row {
                return Some(PredictedCell { ch: pred.ch });
            }
        }
        None
    }

    /// Get the predicted cursor position, if predictions are pending.
    pub fn predicted_cursor(&self) -> Option<(u16, u16)> {
        if self.pending.is_empty() {
            return None;
        }
        let col = self.anchor_col + self.cursor_col_offset;
        if col >= self.cols {
            Some((col - self.cols, self.anchor_row + 1))
        } else {
            Some((col, self.anchor_row))
        }
    }

    /// Snapshot grid at predicted positions BEFORE feed().
    pub fn snapshot_before_feed(&mut self, grid: &TerminalGrid) {
        self.pre_feed_snapshot.clear();
        for pred in &self.pending {
            let ch = grid.get(pred.col, pred.row).map_or('\0', |c| c.c);
            self.pre_feed_snapshot.push((pred.col, pred.row, ch));
        }
    }

    /// Reconcile predictions against the authoritative grid after feed().
    /// Returns `true` if a rollback occurred.
    pub fn reconcile(&mut self, confirmed_epoch: u64, grid: &TerminalGrid, rtt_ms: u64) -> bool {
        let now = Instant::now();
        let grace_ms = Self::grace_period_ms(rtt_ms);

        // Re-check grace entries
        self.grace_pending.retain(|entry| {
            grid.get(entry.col, entry.row)
                .map_or(true, |cell| cell.c != entry.ch)
        });

        // Process confirmed predictions
        while let Some(front) = self.pending.front() {
            if front.epoch <= confirmed_epoch {
                let pred = self.pending.pop_front().unwrap();

                let matched_at_pos = grid.get(pred.col, pred.row)
                    .map_or(false, |cell| cell.c == pred.ch);

                // Check for false positive: char was already there before feed
                let was_already_there = self.pre_feed_snapshot.iter()
                    .any(|&(c, r, ch)| c == pred.col && r == pred.row && ch == pred.ch);

                if matched_at_pos && !was_already_there {
                    // True match — confirmed
                } else {
                    // Mismatch or false positive — grace period
                    self.grace_pending.push_back(GraceEntry {
                        col: pred.col,
                        row: pred.row,
                        ch: pred.ch,
                        confirmed_at: now,
                    });
                }
            } else {
                break;
            }
        }

        self.pre_feed_snapshot.clear();

        // Check grace period timeouts
        let expired = self.grace_pending.iter().any(|entry| {
            now.duration_since(entry.confirmed_at).as_millis() as u64 >= grace_ms
        });

        if expired {
            self.reset();
            return true;
        }

        if self.pending.is_empty() && self.grace_pending.is_empty() {
            self.cursor_col_offset = 0;
        }

        false
    }

    /// Clear all predictions and reset state.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.grace_pending.clear();
        self.cursor_col_offset = 0;
        self.pre_feed_snapshot.clear();
    }

    /// Number of pending predictions (for diagnostics).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grid(cols: u16, rows: u16, content: &[(u16, u16, char)]) -> TerminalGrid {
        let mut grid = TerminalGrid::new(cols, rows);
        for &(col, row, ch) in content {
            let mut cell = nexus_term::Cell::default();
            cell.c = ch;
            grid.set(col, row, cell);
        }
        grid
    }

    #[test]
    fn predict_and_lookup() {
        let mut engine = PredictionEngine::new();
        engine.set_cols(80);
        engine.predict(1, 'h', 0, 0);
        engine.predict(2, 'i', 0, 0);

        assert_eq!(engine.get(0, 0).unwrap().ch, 'h');
        assert_eq!(engine.get(1, 0).unwrap().ch, 'i');
        assert!(engine.get(2, 0).is_none());
    }

    #[test]
    fn predicted_cursor_advances() {
        let mut engine = PredictionEngine::new();
        engine.set_cols(80);
        assert!(engine.predicted_cursor().is_none());

        engine.predict(1, 'a', 5, 3);
        assert_eq!(engine.predicted_cursor(), Some((6, 3)));

        engine.predict(2, 'b', 5, 3);
        assert_eq!(engine.predicted_cursor(), Some((7, 3)));
    }

    #[test]
    fn reconcile_match_clears_predictions() {
        let mut engine = PredictionEngine::new();
        engine.set_cols(80);
        engine.predict(1, 'a', 0, 0);
        engine.predict(2, 'b', 0, 0);

        let empty = make_grid(80, 24, &[]);
        engine.snapshot_before_feed(&empty);

        let grid = make_grid(80, 24, &[(0, 0, 'a'), (1, 0, 'b')]);
        let rollback = engine.reconcile(2, &grid, 0);

        assert!(!rollback);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn reconcile_mismatch_triggers_rollback_after_grace() {
        let mut engine = PredictionEngine::new();
        engine.set_cols(80);
        engine.predict(1, 'a', 0, 0);

        let empty = make_grid(80, 24, &[]);
        engine.snapshot_before_feed(&empty);

        let grid = make_grid(80, 24, &[(0, 0, '*')]);
        let rollback = engine.reconcile(1, &grid, 0);
        assert!(!rollback);

        std::thread::sleep(std::time::Duration::from_millis(250));
        let rollback = engine.reconcile(1, &grid, 0);
        assert!(rollback);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn disabled_engine_ignores_predictions() {
        let mut engine = PredictionEngine::new();
        engine.set_enabled(false);
        engine.predict(1, 'a', 0, 0);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn reset_clears_everything() {
        let mut engine = PredictionEngine::new();
        engine.set_cols(80);
        engine.predict(1, 'a', 0, 0);
        engine.predict(2, 'b', 0, 0);
        assert_eq!(engine.pending_count(), 2);

        engine.reset();
        assert_eq!(engine.pending_count(), 0);
        assert!(engine.predicted_cursor().is_none());
    }
}
