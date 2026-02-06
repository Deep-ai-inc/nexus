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

pub mod constraints;
pub mod containers;
pub mod context;
pub mod elements;
pub mod length;
pub mod primitives;

// Re-export core types
pub use constraints::LayoutConstraints;
pub use context::{LayoutContext, FlexAllocation};
pub use length::{Length, Alignment, CrossAxisAlignment, Padding, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

// Re-export elements
pub use elements::{TextElement, TerminalElement, ImageElement, ButtonElement};

// Re-export containers
pub use containers::{Column, Row, ScrollColumn, LayoutChild, Widget, TextInputElement, TableElement, TableColumn, TableCell};
pub use primitives::{LineStyle, PrimitiveBatch};
