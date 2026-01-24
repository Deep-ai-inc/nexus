# Terminal Resize Implementation Plan

This document analyzes the current terminal resizing issues and outlines a plan for excellent resize support comparable to iTerm2 and macOS Terminal.

## Current State Analysis

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│ Nexus UI (app.rs)                                               │
│   ├─ terminal_size: (cols, rows)  ← UI dimension target         │
│   ├─ pty_handles: Vec<PtyHandle>  ← active PTYs                 │
│   └─ blocks: Vec<Block>                                         │
│         └─ parser: TerminalParser                               │
│               └─ term: alacritty_terminal::Term  ← source of    │
│                                                    truth        │
└─────────────────────────────────────────────────────────────────┘
```

### Data Flow on Resize (Current)

```
Window Resized Event
        │
        ▼
Calculate new column count
        │
        ▼
Update state.terminal_size
        │
        ▼
Resize PTY handles (PtyHandle::resize)
        │
        ▼
... nothing else ...

MISSING:
  ✗ block.parser.resize() never called
  ✗ alacritty_terminal state stays at old dimensions
  ✗ Content written before resize displayed at old width
```

### The Core Problem

1. **TerminalParser instances are never resized** - Only PTY handles are resized
2. **Size mismatch** - PTY thinks terminal is 80 cols, parser thinks it's 120 cols
3. **No reflow** - Content that wrapped at old width stays wrapped wrong
4. **Lost columns** - Shrinking loses content beyond new width permanently

### What Happens Today

```
Initial: 120 columns
Command output: "Very long text that spans the full 120 column width here"

User resizes to 80 columns:
  ├─ PTY notified via SIGWINCH ✓
  ├─ Shell now outputs at 80 cols ✓
  └─ But parser still at 120 cols ✗

User resizes back to 120 columns:
  ├─ Content already truncated/corrupted
  └─ Lines that wrapped at 80 stay wrapped
```

## What Good Terminals Do

### iTerm2 / macOS Terminal Approach

1. **Logical Lines vs Display Lines**
   - Store content as logical lines (text ending in hard newline)
   - Soft wraps are just display artifacts based on current width
   - On resize, re-wrap all logical lines to new width

2. **Wrap Marker Tracking**
   ```
   Logical line: "This is a very long line that would wrap"

   At 20 cols:              At 40 cols:
   "This is a very long "   "This is a very long line that would "
   "line that would wrap"   "wrap"

   Soft wrap ↑              Soft wrap ↑
   ```

3. **Scrollback Preservation**
   - Content that scrolls off screen goes to scrollback buffer
   - Scrollback is also reflowed on resize
   - User can scroll back to see all historical output

4. **Resize Algorithm (Reflow)**
   ```
   for each logical_line in history + visible:
       calculate display_rows = ceil(line_length / new_cols)
       wrap at new_cols boundaries

   adjust cursor position to stay on same logical character
   adjust scroll position to keep same content visible
   ```

### alacritty_terminal Capabilities

alacritty_terminal (which we wrap) has built-in reflow support:

```rust
// In alacritty_terminal
pub struct Term<T> {
    grid: Grid<Cell>,  // Has scrollback + reflow support
    // ...
}

impl Term<T> {
    pub fn resize(&mut self, size: TermSize) {
        // This DOES handle reflow if configured properly
    }
}
```

Key: We're already using alacritty_terminal which handles reflow - we just need to:
1. Actually call `parser.resize()` on window resize
2. Ensure scrollback is enabled in the Term config

## Implementation Plan

### Phase 1: Fix the Immediate Bug (Critical)

**Problem:** `TerminalParser::resize()` is never called.

**Solution:** In `Message::WindowResized` handler, also resize all block parsers.

```rust
Message::WindowResized(width, _height) => {
    let char_width = 8.4_f32;
    let padding = 30.0;
    let cols = ((width as f32 - padding) / char_width) as u16;
    let cols = cols.max(80).min(300);

    if cols != state.terminal_size.0 {
        state.terminal_size = (cols, state.terminal_size.1);

        // Resize all block parsers (NEW!)
        for block in &mut state.blocks {
            block.parser.resize(cols, state.terminal_size.1);
        }

        // Resize all running PTYs
        for handle in &state.pty_handles {
            let _ = handle.resize(cols, state.terminal_size.1);
        }
    }
}
```

**Files to modify:**
- `nexus-ui/src/app.rs` - Add parser resize in WindowResized handler

### Phase 2: Enable Scrollback in alacritty_terminal

**Problem:** We use default Term config which may not have scrollback enabled.

**Solution:** Configure alacritty_terminal with scrollback history.

```rust
// In nexus-term/src/parser.rs
impl TerminalParser {
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize::new(cols as usize, rows as usize);

        // Configure with scrollback
        let mut config = Config::default();
        config.scrolling.history = 10000;  // 10k lines of scrollback

        let term = Term::new(config, &size, EventProxy);
        let processor = Processor::new();

        Self { term, processor }
    }
}
```

**Files to modify:**
- `nexus-term/src/parser.rs` - Configure scrollback in Term creation

### Phase 3: Expose Scrollback in Grid API

**Problem:** `TerminalGrid` only captures visible cells, not scrollback.

**Solution:** Add methods to access scrollback content.

```rust
// In nexus-term/src/parser.rs
impl TerminalParser {
    /// Get the number of lines in scrollback history.
    pub fn scrollback_lines(&self) -> usize {
        self.term.history_size()
    }

    /// Extract grid with scrollback content.
    pub fn grid_with_scrollback(&self, scroll_offset: usize) -> TerminalGrid {
        // Include scrollback in grid extraction
        // ...
    }
}
```

**Files to modify:**
- `nexus-term/src/parser.rs` - Add scrollback access methods
- `nexus-term/src/grid.rs` - May need to support variable height

### Phase 4: UI Scrolling Integration

**Problem:** UI doesn't handle scrollback, each block is fixed height.

**Solution:** Allow blocks to be scrollable with scrollback content.

```rust
// In view function, for completed blocks with scrollback:
if block.has_scrollback() {
    scrollable(
        column![
            terminal_view_with_scrollback(block),
        ]
    )
    .height(Length::Fixed(400.0))  // Or dynamic based on content
} else {
    terminal_view(block)
}
```

**Files to modify:**
- `nexus-ui/src/app.rs` - Add scroll support for blocks with history
- `nexus-ui/src/widgets/terminal_view.rs` - Support rendering with scroll offset

### Phase 5: Row Count Calculation

**Problem:** Currently rows are hardcoded at 24, never recalculated on resize.

**Solution:** Calculate rows based on available height.

```rust
Message::WindowResized(width, height) => {
    let char_width = 8.4_f32;
    let line_height = 19.6;  // 14px * 1.4
    let h_padding = 30.0;
    let v_padding = 60.0;  // Top/bottom padding

    let cols = ((width as f32 - h_padding) / char_width) as u16;
    let rows = ((height as f32 - v_padding) / line_height) as u16;

    let cols = cols.max(80).min(300);
    let rows = rows.max(10).min(100);

    // ... resize with both dimensions
}
```

**Note:** For Nexus's block-based model, fixed rows may actually be correct since each command is a separate block, not a traditional scrolling terminal. This needs design consideration.

### Phase 6: Reflow Quality Verification

**Verify alacritty_terminal reflow works correctly:**

```rust
#[test]
fn test_resize_reflow() {
    let mut parser = TerminalParser::new(80, 24);

    // Write a long line
    parser.feed(b"A".repeat(120).as_slice());
    parser.feed(b"\n");

    // Should wrap to 2 lines
    let grid = parser.grid();
    assert!(grid.rows_iter().take(2).all(|row|
        row.iter().any(|c| c.c == 'A')
    ));

    // Resize smaller
    parser.resize(40, 24);

    // Should reflow to 3 lines
    let grid = parser.grid();
    assert!(grid.rows_iter().take(3).all(|row|
        row.iter().any(|c| c.c == 'A')
    ));

    // Resize larger - should reflow back
    parser.resize(120, 24);

    let grid = parser.grid();
    // Line should fit in 1 row now
}
```

## Implementation Priority

| Priority | Phase | Effort | Impact | Status |
|----------|-------|--------|--------|--------|
| P0 | Phase 1: Call parser.resize() | Small | Fixes immediate bug | ✅ Done |
| P1 | Phase 2: Enable scrollback | Small | Preserves history | ✅ Done |
| P2 | Phase 5: Row calculation | Small | Better dimension handling | ✅ Done |
| P3 | Phase 3: Scrollback API | Medium | Enables history access | Pending |
| P4 | Phase 4: UI scrolling | Medium | User can see history | Pending |
| P5 | Phase 6: Verification | Small | Confidence in behavior | ✅ Done |

## What Was Implemented

### Phase 1: Parser Resize (✅ Complete)
- Added `block.parser.resize()` calls in `WindowResized` handler
- Split logic for running vs finished blocks:
  - **Running blocks**: Get full window height (for TUIs like vim)
  - **Finished blocks**: Compact to content height via `content_height()` method

### Debouncing (✅ Complete)
- Added 15ms debounce to prevent resize storms during window drag
- `WindowResized` stores pending dimensions, `DebouncedResize` applies them

### Phase 2: Scrollback (✅ Complete)
- Configured alacritty_terminal with explicit 10k line scrollback history
- Default config already had this, but now it's explicit and documented

### Phase 5: Row Calculation (✅ Complete)
- Now calculates both columns AND rows from window dimensions
- Uses `line_height = 19.6px` (14px * 1.4)
- Proper padding accounting for horizontal and vertical space

### Content Height (✅ Complete)
- Added `TerminalParser::content_height()` method
- Scans grid to find last row with actual content
- Used for compacting finished blocks

### Tests (✅ Complete)
- Added unit tests for resize reflow, width-only resize, content height calculation
- Verified content preservation through resize cycles

## Design Considerations

### Block-Based vs Traditional Terminal

Nexus uses a **block-based** model where each command is a separate block, unlike traditional terminals with one continuous scrolling buffer. This means:

1. **Per-block scrollback** - Each block has its own history
2. **Completed blocks are static** - No new content added
3. **Only running blocks need live resize** - Completed blocks just need reflow

### Row Count Philosophy

Two options:

**Option A: Dynamic rows per block**
- Each block has rows = content_lines
- Growing blocks expand
- Traditional terminal feel

**Option B: Fixed viewport rows (current)**
- Running commands have fixed 24-row viewport
- Content scrolls within viewport
- Block-based feel preserved

Recommendation: Keep Option B for running commands, but allow completed blocks to show all content without artificial row limit.

### Memory Considerations

With scrollback enabled:
- 10,000 lines * 300 cols * ~20 bytes/cell = ~60MB per block maximum
- Need to consider memory pressure with many blocks
- Could implement scrollback limits per block based on total memory

## Testing Plan

1. **Reflow test**: Write long lines, resize smaller/larger, verify content preserved
2. **Multi-block test**: Run several commands, resize, verify all blocks correct
3. **Running command test**: Resize while command running, verify output correct
4. **Edge cases**:
   - Resize to very small (80x10)
   - Resize to very large (300x100)
   - Rapid resize events
   - Resize during heavy output

## Summary

The fix is straightforward:

1. **Immediate fix**: Call `block.parser.resize()` in WindowResized handler
2. **Enable scrollback**: Configure alacritty_terminal with history
3. **Verify reflow**: Test that alacritty_terminal reflows correctly

alacritty_terminal already has sophisticated reflow support - we're just not using it. The main work is plumbing the resize call through and configuring scrollback properly.
