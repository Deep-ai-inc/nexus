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

pub mod containers;
pub mod primitives;

pub use containers::{Column, Row, ScrollColumn, Padding, Alignment, CrossAxisAlignment, Length, ImageElement};
pub use primitives::{LineStyle, PrimitiveBatch};
