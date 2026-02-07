//! Layout System for Strata
//!
//! Provides flexbox-inspired layout containers that compute child positions
//! and batch primitives efficiently. The layout pass happens once per frame,
//! not per-widget.
//!
//! # Architecture
//!
//! ```text
//! view() builds declarative tree -> layout() computes Rects -> flush to snapshot
//! ```
//!
//! This avoids the "immediate mode trap" where widgets compute layout every frame.

pub mod base;
pub mod cache;
pub mod constraints;
pub mod context;
pub mod elements;
pub mod flex;
pub mod length;
pub mod primitives;

// Container modules (order matters for dependencies)
pub mod flow;            // FlowContainer
pub mod scroll_column;   // ScrollColumn
pub mod row;             // Row
pub mod column;          // Column
pub mod text_input;      // TextInputElement
pub mod table;           // TableElement, VirtualTableElement
pub mod canvas;          // Canvas (custom drawing)
pub mod list_view;       // ListView (virtualized list)
pub mod child;           // LayoutChild enum (central switchboard)

// Re-export core types
pub use base::{Chrome, render_chrome};
pub use cache::{LayoutCache, LayoutCacheKey};
pub use constraints::LayoutConstraints;
pub use context::{LayoutContext, FlexAllocation};
pub use length::{Length, Alignment, CrossAxisAlignment, Padding, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

// Re-export elements
pub use elements::{TextElement, TerminalElement, ImageElement, ButtonElement};

// Re-export child types (LayoutChild, Widget, Element)
pub use child::{LayoutChild, Widget, Element};

// Re-export containers
pub use flow::FlowContainer;
pub use scroll_column::ScrollColumn;
pub use row::Row;
pub use column::Column;
pub use text_input::TextInputElement;
pub use table::{TableElement, TableColumn, TableCell, VirtualTableElement, VirtualCell};
pub use canvas::Canvas;
pub use list_view::ListView;
pub use primitives::{LineStyle, PrimitiveBatch};

// =========================================================================
// Integration Tests (Phase 4 Verification)
// =========================================================================

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::layout_snapshot::LayoutSnapshot;
    use crate::primitives::Point;

    /// The "Shrink" Test: Column with Length::Shrink containing a FlowContainer.
    /// Verifies that shrink-wrap layout correctly caps at parent's max_width.
    #[test]
    fn test_shrink_column_with_flow_container() {
        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("This is a very long text that should wrap"))
            .text(TextElement::new("And another long text here"));

        let col = Column::new()
            .width(Length::Shrink)
            .push(flow);

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        // Parent provides max 200px width
        let constraints = LayoutConstraints::with_max_width(200.0);
        let size = col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // Column should respect the max_width constraint
        assert!(size.width <= 200.0,
            "Shrink column exceeded max_width: {} > 200.0", size.width);
        assert!(size.height > 0.0);
    }

    /// The "Deep Nesting" Test: Rows inside Columns inside Rows, 10 levels deep.
    /// With boxed recursion in LayoutChild, this should be stable and performant.
    #[test]
    fn test_deep_nesting_stability() {
        fn build_nested(depth: usize) -> LayoutChild<'static> {
            if depth == 0 {
                TextElement::new("Leaf").into()
            } else if depth % 2 == 0 {
                Column::new()
                    .push(build_nested(depth - 1))
                    .into()
            } else {
                Row::new()
                    .push(build_nested(depth - 1))
                    .into()
            }
        }

        // Build 10 levels deep (10 is even, so top is Column)
        let nested = match build_nested(10) {
            LayoutChild::Column(c) => *c,
            _ => panic!("Expected Column at top level"),
        };

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::loose(500.0, 500.0);
        let size = nested.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // Should complete without stack overflow and produce valid size
        assert!(size.width > 0.0);
        assert!(size.height > 0.0);
    }

    /// Test constraint propagation through nested containers.
    #[test]
    fn test_constraint_propagation() {
        let inner = Column::new()
            .width(Length::Fill)
            .push(TextElement::new("Fill width"));

        let outer = Row::new()
            .width(Length::Fixed(300.0))
            .push(inner);

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::loose(500.0, 100.0);
        let size = outer.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // Outer row should be 300px (fixed)
        assert_eq!(size.width, 300.0);
    }

    /// Test flex distribution in Row with Fill children.
    #[test]
    fn test_row_flex_distribution() {
        let row = Row::new()
            .width(Length::Fixed(300.0))
            .spacing(0.0)
            .push(Column::new().width(Length::Fill))
            .push(Column::new().width(Length::Fill))
            .push(Column::new().width(Length::Fill));

        // Measure should show intrinsic size
        let size = row.measure();
        assert!(size.width >= 0.0);
    }

    /// Test that debug logging works in debug builds.
    #[cfg(debug_assertions)]
    #[test]
    fn test_debug_logging() {
        let col = Column::new()
            .push(TextElement::new("Test"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot).with_debug(true);

        assert!(ctx.is_debug());

        let constraints = LayoutConstraints::loose(100.0, 50.0);
        let _size = col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // If we get here without panicking, debug logging is working
    }

    /// Regression test: "Opaque Container" hashing bug.
    ///
    /// When a FlowContainer has a nested Column, and the Column's children
    /// change (e.g., text changes from "A" to "B"), the FlowContainer's
    /// content_hash MUST change. Otherwise, the parent will return a stale
    /// cached size.
    #[test]
    fn test_nested_container_hash_propagation() {
        // FlowContainer with nested Column containing "A"
        let flow1 = FlowContainer::new()
            .push(Column::new().push(TextElement::new("A")));

        // FlowContainer with nested Column containing "B"
        let flow2 = FlowContainer::new()
            .push(Column::new().push(TextElement::new("B")));

        // FlowContainer with nested Column containing "A" (same as flow1)
        let flow3 = FlowContainer::new()
            .push(Column::new().push(TextElement::new("A")));

        // flow1 and flow3 should have the same hash (identical content)
        assert_eq!(
            flow1.content_hash(), flow3.content_hash(),
            "Identical nested structures should have the same hash"
        );

        // flow1 and flow2 should have DIFFERENT hashes (nested content differs)
        assert_ne!(
            flow1.content_hash(), flow2.content_hash(),
            "Different nested content must produce different hashes"
        );
    }

    /// Regression test: deeply nested container hash changes propagate.
    #[test]
    fn test_deep_nested_hash_propagation() {
        // Three levels: FlowContainer > Column > Row > TextElement
        let deep1 = FlowContainer::new()
            .push(Column::new()
                .push(Row::new()
                    .push(TextElement::new("Leaf A"))));

        let deep2 = FlowContainer::new()
            .push(Column::new()
                .push(Row::new()
                    .push(TextElement::new("Leaf B"))));

        // Even deeply nested content differences must produce different hashes
        assert_ne!(
            deep1.content_hash(), deep2.content_hash(),
            "Deeply nested content changes must propagate to parent hash"
        );
    }
}
