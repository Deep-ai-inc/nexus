//! Content Addressing System
//!
//! This module provides stable, global addressing for content across widget boundaries.
//! Unlike widget-local positions, `ContentAddress` remains valid regardless of which
//! widget renders the content or how the UI is laid out.
//!
//! # Key Types
//!
//! - `SourceId`: Identifies a data source (e.g., a terminal buffer, agent block)
//! - `ContentAddress`: Global address with source + item_index + content_offset
//! - `Selection`: A range defined by two content addresses
//! - `SourceOrdering`: Maintains document order for cross-source comparisons

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Counter for generating unique source IDs.
static SOURCE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Unique identifier for a data source.
///
/// A source represents a logical container of content, such as:
/// - A terminal buffer
/// - An agent response block
/// - A table
/// - A text document
///
/// Sources are assigned unique IDs and can be compared for equality.
/// The ordering of sources in the document is determined by `SourceOrdering`,
/// not by the numeric value of the ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId(pub u64);

impl SourceId {
    /// Create a new unique source ID.
    ///
    /// Each call returns a different ID.
    pub fn new() -> Self {
        Self(SOURCE_ID_COUNTER.fetch_add(1, AtomicOrdering::Relaxed))
    }

    /// Create a stable source ID from a name.
    ///
    /// Deterministic: same name always produces the same ID.
    /// Uses high bit to avoid collision with the atomic counter.
    pub fn named(name: &str) -> Self {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        Self(hasher.finish() | (1 << 63))
    }

    /// Create a source ID from an existing value.
    ///
    /// Use this for deterministic IDs (e.g., derived from block IDs).
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw numeric value.
    pub const fn raw(&self) -> u64 {
        self.0
    }

    /// Create a deterministic child ID from this parent.
    ///
    /// Uses entropy-preserving mixing (rotate + XOR with golden ratio constant)
    /// to derive unique child IDs without allocation or heavy hashing.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Define discriminators as constants
    /// const HEADER: u64 = 1;
    /// const BODY: u64 = 2;
    ///
    /// // In view: create scoped widget IDs
    /// .widget_id(block_id.child(HEADER))
    ///
    /// // In update: recreate to compare
    /// if clicked_id == block_id.child(HEADER) {
    ///     // handle header click
    /// }
    /// ```
    ///
    /// Note: This is a one-way operation. You cannot recover the parent ID
    /// from the child. The update function must have access to the same
    /// parent ID that generated the view.
    pub const fn child(&self, discriminator: u64) -> Self {
        // Golden ratio fractional bits - same constant used in FxHash/SplitMix64
        const PHI: u64 = 0x9E3779B97F4A7C15;
        let mixed = self.0.rotate_left(21) ^ discriminator.wrapping_mul(PHI);
        Self(mixed)
    }
}

impl Default for SourceId {
    fn default() -> Self {
        Self::new()
    }
}

/// A global content address that works across widget boundaries.
///
/// This is the core primitive for cross-widget selection. Unlike widget-local
/// positions (e.g., "row 5, column 10 of terminal widget X"), a ContentAddress
/// identifies content by its logical location in the data model.
///
/// # Fields
///
/// - `source_id`: Which data source this content belongs to
/// - `item_index`: Index of the item within the source (e.g., row number, paragraph index)
/// - `content_offset`: Character offset within the item
///
/// # Examples
///
/// Terminal content:
/// ```ignore
/// ContentAddress {
///     source_id: terminal_source,
///     item_index: 0,  // Terminal is a single item
///     content_offset: row * cols + col,  // Linear offset in grid
/// }
/// ```
///
/// Agent block text:
/// ```ignore
/// ContentAddress {
///     source_id: agent_source,
///     item_index: 2,  // Third paragraph
///     content_offset: 15,  // 15th character in paragraph
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentAddress {
    /// The data source this content belongs to.
    pub source_id: SourceId,

    /// Index of the item within the source.
    ///
    /// For terminal grids, this is typically 0 (single item).
    /// For lists or documents, this is the row/paragraph index.
    pub item_index: usize,

    /// Character offset within the item.
    ///
    /// For text: character index (0-based).
    /// For grids: linear cell index (row * cols + col).
    pub content_offset: usize,
}

impl ContentAddress {
    /// Create a new content address.
    #[inline]
    pub const fn new(source_id: SourceId, item_index: usize, content_offset: usize) -> Self {
        Self {
            source_id,
            item_index,
            content_offset,
        }
    }

    /// Create an address at the start of a source.
    #[inline]
    pub const fn start_of(source_id: SourceId) -> Self {
        Self {
            source_id,
            item_index: 0,
            content_offset: 0,
        }
    }

    /// Check if this address is in the same source as another.
    #[inline]
    pub fn same_source(&self, other: &Self) -> bool {
        self.source_id == other.source_id
    }

    /// Compare this address to another within the same source.
    ///
    /// Returns `None` if the addresses are in different sources.
    pub fn compare_within_source(&self, other: &Self) -> Option<Ordering> {
        if self.source_id != other.source_id {
            return None;
        }

        Some(
            self.item_index
                .cmp(&other.item_index)
                .then_with(|| self.content_offset.cmp(&other.content_offset)),
        )
    }
}

impl Hash for ContentAddress {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.source_id.hash(state);
        self.item_index.hash(state);
        self.content_offset.hash(state);
    }
}

/// Selection shape determines how the anchor-focus range is interpreted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectionShape {
    /// Standard linear selection from anchor to focus in document order.
    Linear,
    /// Rectangular/column selection. Visual x-coordinates define the column range,
    /// while anchor/focus define the row range.
    Rectangular { x_min: f32, x_max: f32 },
}

impl Default for SelectionShape {
    fn default() -> Self {
        Self::Linear
    }
}

/// A selection defined by two content addresses.
///
/// The selection spans from `anchor` (where the selection started) to `focus`
/// (the current cursor position). These may be in any order - use `normalized()`
/// to get them in document order.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// The starting point of the selection (where the user clicked).
    pub anchor: ContentAddress,

    /// The current endpoint of the selection (where the cursor is now).
    pub focus: ContentAddress,

    /// Shape of the selection (linear or rectangular).
    pub shape: SelectionShape,
}

impl Selection {
    /// Create a new linear selection.
    #[inline]
    pub fn new(anchor: ContentAddress, focus: ContentAddress) -> Self {
        Self { anchor, focus, shape: SelectionShape::Linear }
    }

    /// Create a new selection with a specific shape.
    #[inline]
    pub fn with_shape(anchor: ContentAddress, focus: ContentAddress, shape: SelectionShape) -> Self {
        Self { anchor, focus, shape }
    }

    /// Create a collapsed selection (cursor position, no actual selection).
    #[inline]
    pub fn collapsed(position: ContentAddress) -> Self {
        Self {
            anchor: position.clone(),
            focus: position,
            shape: SelectionShape::Linear,
        }
    }

    /// Check if the selection is collapsed (anchor == focus).
    #[inline]
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }

    /// Check if anchor and focus are in the same source.
    #[inline]
    pub fn is_within_source(&self) -> bool {
        self.anchor.source_id == self.focus.source_id
    }

    /// Normalize the selection so anchor comes before focus in document order.
    ///
    /// Returns `(start, end)` where start <= end according to the ordering.
    pub fn normalized(&self, ordering: &SourceOrdering) -> (ContentAddress, ContentAddress) {
        match ordering.compare(&self.anchor, &self.focus) {
            Ordering::Greater => (self.focus.clone(), self.anchor.clone()),
            _ => (self.anchor.clone(), self.focus.clone()),
        }
    }

    /// Check if a content address is within this selection.
    ///
    /// Returns true if `addr` is between anchor and focus (inclusive).
    pub fn contains(&self, addr: &ContentAddress, ordering: &SourceOrdering) -> bool {
        let (start, end) = self.normalized(ordering);

        let after_start = ordering.compare(addr, &start) != Ordering::Less;
        let before_end = ordering.compare(addr, &end) != Ordering::Greater;

        after_start && before_end
    }

    /// Get the set of source IDs that this selection spans.
    pub fn sources(&self, ordering: &SourceOrdering) -> Vec<SourceId> {
        let (start, end) = self.normalized(ordering);
        ordering.sources_between(&start.source_id, &end.source_id)
    }
}

/// Maintains the document order of sources for cross-source comparisons.
///
/// Since `SourceId` is just a unique identifier without inherent ordering,
/// we need a separate structure to track how sources are ordered in the
/// document. This ordering is determined by the layout pass and updated
/// whenever sources are added or reordered.
#[derive(Debug, Clone, Default)]
pub struct SourceOrdering {
    /// Maps source ID to its position in the document (0 = first).
    order: HashMap<SourceId, usize>,

    /// Sources in document order (for iteration).
    sources: Vec<SourceId>,
}

impl SourceOrdering {
    /// Create an empty source ordering.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a source and assign it the next position in document order.
    ///
    /// Returns the assigned position index.
    pub fn register(&mut self, source_id: SourceId) -> usize {
        if let Some(&existing) = self.order.get(&source_id) {
            return existing;
        }

        let position = self.sources.len();
        self.order.insert(source_id, position);
        self.sources.push(source_id);
        position
    }

    /// Clear all sources (call at the start of each frame's layout pass).
    pub fn clear(&mut self) {
        self.order.clear();
        self.sources.clear();
    }

    /// Get the position of a source in document order.
    pub fn position(&self, source_id: &SourceId) -> Option<usize> {
        self.order.get(source_id).copied()
    }

    /// Get the source at a given position.
    pub fn source_at(&self, position: usize) -> Option<SourceId> {
        self.sources.get(position).copied()
    }

    /// Get all sources in document order.
    pub fn sources_in_order(&self) -> &[SourceId] {
        &self.sources
    }

    /// Get sources between two sources (inclusive), in document order.
    pub fn sources_between(&self, start: &SourceId, end: &SourceId) -> Vec<SourceId> {
        let start_pos = self.order.get(start).copied().unwrap_or(usize::MAX);
        let end_pos = self.order.get(end).copied().unwrap_or(0);

        let (min_pos, max_pos) = if start_pos <= end_pos {
            (start_pos, end_pos)
        } else {
            (end_pos, start_pos)
        };

        self.sources
            .iter()
            .enumerate()
            .filter(|(i, _)| *i >= min_pos && *i <= max_pos)
            .map(|(_, id)| *id)
            .collect()
    }

    /// Compare two content addresses in document order.
    ///
    /// This is the core comparison function for selection operations.
    pub fn compare(&self, a: &ContentAddress, b: &ContentAddress) -> Ordering {
        if a.source_id == b.source_id {
            // Same source: compare by item_index, then content_offset
            a.item_index
                .cmp(&b.item_index)
                .then_with(|| a.content_offset.cmp(&b.content_offset))
        } else {
            // Different sources: compare by source position
            let a_pos = self.order.get(&a.source_id).copied().unwrap_or(usize::MAX);
            let b_pos = self.order.get(&b.source_id).copied().unwrap_or(usize::MAX);
            a_pos.cmp(&b_pos)
        }
    }

    /// Check if a source is registered.
    pub fn contains(&self, source_id: &SourceId) -> bool {
        self.order.contains_key(source_id)
    }

    /// Get the number of registered sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Check if no sources are registered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_uniqueness() {
        let id1 = SourceId::new();
        let id2 = SourceId::new();
        let id3 = SourceId::new();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn content_address_same_source_comparison() {
        let source = SourceId::new();

        let a = ContentAddress::new(source, 0, 10);
        let b = ContentAddress::new(source, 0, 20);
        let c = ContentAddress::new(source, 1, 5);

        assert_eq!(a.compare_within_source(&b), Some(Ordering::Less));
        assert_eq!(b.compare_within_source(&a), Some(Ordering::Greater));
        assert_eq!(a.compare_within_source(&c), Some(Ordering::Less));
        assert_eq!(c.compare_within_source(&b), Some(Ordering::Greater));

        let other_source = SourceId::new();
        let d = ContentAddress::new(other_source, 0, 10);
        assert_eq!(a.compare_within_source(&d), None);
    }

    #[test]
    fn source_ordering_registration() {
        let mut ordering = SourceOrdering::new();

        let s1 = SourceId::new();
        let s2 = SourceId::new();
        let s3 = SourceId::new();

        assert_eq!(ordering.register(s1), 0);
        assert_eq!(ordering.register(s2), 1);
        assert_eq!(ordering.register(s3), 2);

        // Re-registering returns same position
        assert_eq!(ordering.register(s1), 0);

        assert_eq!(ordering.sources_in_order(), &[s1, s2, s3]);
    }

    #[test]
    fn source_ordering_compare() {
        let mut ordering = SourceOrdering::new();

        let s1 = SourceId::new();
        let s2 = SourceId::new();

        ordering.register(s1);
        ordering.register(s2);

        // Same source, different offsets
        let a = ContentAddress::new(s1, 0, 10);
        let b = ContentAddress::new(s1, 0, 20);
        assert_eq!(ordering.compare(&a, &b), Ordering::Less);

        // Same source, different items
        let c = ContentAddress::new(s1, 1, 0);
        assert_eq!(ordering.compare(&b, &c), Ordering::Less);

        // Different sources
        let d = ContentAddress::new(s2, 0, 0);
        assert_eq!(ordering.compare(&a, &d), Ordering::Less);
        assert_eq!(ordering.compare(&c, &d), Ordering::Less);
    }

    #[test]
    fn selection_normalization() {
        let mut ordering = SourceOrdering::new();

        let s1 = SourceId::new();
        ordering.register(s1);

        let start = ContentAddress::new(s1, 0, 10);
        let end = ContentAddress::new(s1, 0, 50);

        // Anchor before focus
        let sel1 = Selection::new(start.clone(), end.clone());
        let (norm_start, norm_end) = sel1.normalized(&ordering);
        assert_eq!(norm_start, start);
        assert_eq!(norm_end, end);

        // Anchor after focus (reversed)
        let sel2 = Selection::new(end.clone(), start.clone());
        let (norm_start2, norm_end2) = sel2.normalized(&ordering);
        assert_eq!(norm_start2, start);
        assert_eq!(norm_end2, end);
    }

    #[test]
    fn selection_contains() {
        let mut ordering = SourceOrdering::new();

        let s1 = SourceId::new();
        ordering.register(s1);

        let sel = Selection::new(
            ContentAddress::new(s1, 0, 10),
            ContentAddress::new(s1, 0, 50),
        );

        // Inside selection
        assert!(sel.contains(&ContentAddress::new(s1, 0, 30), &ordering));

        // At boundaries
        assert!(sel.contains(&ContentAddress::new(s1, 0, 10), &ordering));
        assert!(sel.contains(&ContentAddress::new(s1, 0, 50), &ordering));

        // Outside selection
        assert!(!sel.contains(&ContentAddress::new(s1, 0, 5), &ordering));
        assert!(!sel.contains(&ContentAddress::new(s1, 0, 55), &ordering));
    }

    #[test]
    fn selection_across_sources() {
        let mut ordering = SourceOrdering::new();

        let s1 = SourceId::new();
        let s2 = SourceId::new();
        let s3 = SourceId::new();

        ordering.register(s1);
        ordering.register(s2);
        ordering.register(s3);

        let sel = Selection::new(
            ContentAddress::new(s1, 0, 50),
            ContentAddress::new(s3, 0, 10),
        );

        let sources = sel.sources(&ordering);
        assert_eq!(sources, vec![s1, s2, s3]);

        // Point in middle source is selected
        assert!(sel.contains(&ContentAddress::new(s2, 0, 0), &ordering));
    }

    #[test]
    fn source_ordering_clear() {
        let mut ordering = SourceOrdering::new();

        let s1 = SourceId::new();
        let s2 = SourceId::new();

        ordering.register(s1);
        ordering.register(s2);
        assert_eq!(ordering.len(), 2);

        ordering.clear();
        assert!(ordering.is_empty());

        // Can register again with new positions
        ordering.register(s2);
        ordering.register(s1);
        assert_eq!(ordering.position(&s2), Some(0));
        assert_eq!(ordering.position(&s1), Some(1));
    }
}
