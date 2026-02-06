//! Layout Child Enum - Central Switchboard
//!
//! This module contains the `LayoutChild` enum which represents any element
//! that can be placed in a layout container. It acts as a dispatch layer
//! between containers (Column, Row, etc.) and the concrete element types.
//!
//! The recursive container types (Column, Row, ScrollColumn) are boxed to
//! break the size recursion that would otherwise make the enum infinitely sized.

use crate::primitives::Size;

// Import element types from elements module
use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};

// Import length types
use super::length::{Length, CHAR_WIDTH, LINE_HEIGHT};

// Forward declare container types - these are still in containers.rs for now
// Will be updated as we extract them to separate modules
use super::containers::{
    Column, Row, ScrollColumn, FlowContainer,
    TextInputElement, TableElement, VirtualTableElement,
};

// =========================================================================
// LayoutChild Enum
// =========================================================================

/// A child element in a layout container.
///
/// This enum is the central switchboard for the layout system. Containers
/// don't need to know the concrete type of their children - they just work
/// with `LayoutChild` and call its methods for measurement and flex calculation.
///
/// ## Boxing Strategy
///
/// Recursive container types (Column, Row, ScrollColumn) are boxed to:
/// 1. Break the infinite size recursion (enum size would depend on itself)
/// 2. Keep the enum size small (~40 bytes instead of hundreds)
/// 3. Improve cache performance when iterating Vec<LayoutChild>
///
/// The one pointer indirection cost is negligible compared to layout math.
pub enum LayoutChild {
    /// A text element.
    Text(TextElement),

    /// A terminal/grid element.
    Terminal(TerminalElement),

    /// An image element.
    Image(ImageElement),

    /// A nested column (boxed to break size recursion).
    Column(Box<Column>),

    /// A nested row (boxed to break size recursion).
    Row(Box<Row>),

    /// A scroll column (boxed to break size recursion).
    ScrollColumn(Box<ScrollColumn>),

    /// A spacer that expands to fill available space.
    Spacer { flex: f32 },

    /// A button element (text label with background, registers as widget hit target).
    Button(ButtonElement),

    /// A text input element (editable text field, registers as widget hit target).
    TextInput(TextInputElement),

    /// A table element (headers + rows with sortable columns).
    Table(TableElement),

    /// A virtual table (only renders visible rows — O(visible) not O(total)).
    VirtualTable(VirtualTableElement),

    /// A fixed-size spacer.
    FixedSpacer { size: f32 },

    /// A flow container (wrapping inline layout, like CSS flex-wrap).
    Flow(FlowContainer),
}

// =========================================================================
// LayoutChild Methods
// =========================================================================

impl LayoutChild {
    /// Measure this child's main axis size (height for Column parent, width for Row parent).
    pub(crate) fn measure_main(&self, is_column: bool) -> f32 {
        let size = match self {
            LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT),
            LayoutChild::Terminal(t) => t.size(),
            LayoutChild::Image(img) => img.size(),
            LayoutChild::Button(b) => b.estimate_size(),
            LayoutChild::TextInput(t) => t.estimate_size(),
            LayoutChild::Table(t) => t.estimate_size(),
            LayoutChild::VirtualTable(t) => t.estimate_size(),
            LayoutChild::Column(c) => c.measure(),
            LayoutChild::Row(r) => r.measure(),
            LayoutChild::ScrollColumn(s) => s.measure(),
            LayoutChild::Flow(f) => f.measure(),
            LayoutChild::Spacer { .. } => return 0.0,
            LayoutChild::FixedSpacer { size } => return *size,
        };
        if is_column { size.height } else { size.width }
    }

    /// Measure this child's cross axis size (width for Column parent, height for Row parent).
    pub(crate) fn measure_cross(&self, is_column: bool) -> f32 {
        let size = match self {
            LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT),
            LayoutChild::Terminal(t) => t.size(),
            LayoutChild::Image(img) => img.size(),
            LayoutChild::Button(b) => b.estimate_size(),
            LayoutChild::TextInput(t) => t.estimate_size(),
            LayoutChild::Table(t) => t.estimate_size(),
            LayoutChild::VirtualTable(t) => t.estimate_size(),
            LayoutChild::Column(c) => c.measure(),
            LayoutChild::Row(r) => r.measure(),
            LayoutChild::ScrollColumn(s) => s.measure(),
            LayoutChild::Flow(f) => f.measure(),
            LayoutChild::Spacer { .. } => return 0.0,
            LayoutChild::FixedSpacer { .. } => return 0.0,
        };
        if is_column { size.width } else { size.height }
    }

    /// Get the flex factor on the parent's main axis.
    ///
    /// `is_column`: true if the parent is a Column (main axis = height),
    ///              false if the parent is a Row (main axis = width).
    pub(crate) fn flex_factor(&self, is_column: bool) -> f32 {
        match self {
            LayoutChild::Spacer { flex } => *flex,
            LayoutChild::Column(c) => {
                if is_column { c.height.flex() } else { c.width.flex() }
            }
            LayoutChild::Row(r) => {
                if is_column { r.height.flex() } else { r.width.flex() }
            }
            LayoutChild::ScrollColumn(s) => {
                if is_column { s.height.flex() } else { s.width.flex() }
            }
            LayoutChild::TextInput(t) => {
                if is_column { 0.0 } else { t.width.flex() }
            }
            LayoutChild::Flow(f) => {
                if is_column { 0.0 } else { f.width.flex() }
            }
            _ => 0.0,
        }
    }

    /// Get the intrinsic size of this child.
    pub(crate) fn size(&self) -> Size {
        match self {
            LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT),
            LayoutChild::Terminal(t) => t.size(),
            LayoutChild::Image(img) => img.size(),
            LayoutChild::Button(b) => b.estimate_size(),
            LayoutChild::TextInput(t) => t.estimate_size(),
            LayoutChild::Table(t) => t.estimate_size(),
            LayoutChild::VirtualTable(t) => t.estimate_size(),
            LayoutChild::Column(c) => c.measure(),
            LayoutChild::Row(r) => r.measure(),
            LayoutChild::ScrollColumn(s) => s.measure(),
            LayoutChild::Flow(f) => f.measure(),
            LayoutChild::Spacer { .. } => Size::new(0.0, 0.0),
            LayoutChild::FixedSpacer { size } => Size::new(*size, *size),
        }
    }

    /// Get the Length for the main axis dimension.
    pub(crate) fn main_length(&self, is_column: bool) -> Length {
        match self {
            LayoutChild::Column(c) => if is_column { c.height } else { c.width },
            LayoutChild::Row(r) => if is_column { r.height } else { r.width },
            LayoutChild::ScrollColumn(s) => if is_column { s.height } else { s.width },
            LayoutChild::TextInput(t) => if is_column { t.height } else { t.width },
            LayoutChild::Flow(f) => if is_column { Length::Shrink } else { f.width },
            LayoutChild::Spacer { flex } => Length::FillPortion((*flex * 100.0) as u16),
            LayoutChild::FixedSpacer { size } => Length::Fixed(*size),
            _ => Length::Shrink,
        }
    }
}

// =========================================================================
// From impls for LayoutChild — enables generic `.push()` on containers
// =========================================================================

impl From<TextElement> for LayoutChild {
    fn from(v: TextElement) -> Self { Self::Text(v) }
}

impl From<TerminalElement> for LayoutChild {
    fn from(v: TerminalElement) -> Self { Self::Terminal(v) }
}

impl From<ImageElement> for LayoutChild {
    fn from(v: ImageElement) -> Self { Self::Image(v) }
}

impl From<Column> for LayoutChild {
    fn from(v: Column) -> Self { Self::Column(Box::new(v)) }
}

impl From<Row> for LayoutChild {
    fn from(v: Row) -> Self { Self::Row(Box::new(v)) }
}

impl From<ScrollColumn> for LayoutChild {
    fn from(v: ScrollColumn) -> Self { Self::ScrollColumn(Box::new(v)) }
}

impl From<ButtonElement> for LayoutChild {
    fn from(v: ButtonElement) -> Self { Self::Button(v) }
}

impl From<TextInputElement> for LayoutChild {
    fn from(v: TextInputElement) -> Self { Self::TextInput(v) }
}

impl From<TableElement> for LayoutChild {
    fn from(v: TableElement) -> Self { Self::Table(v) }
}

impl From<VirtualTableElement> for LayoutChild {
    fn from(v: VirtualTableElement) -> Self { Self::VirtualTable(v) }
}

impl From<FlowContainer> for LayoutChild {
    fn from(v: FlowContainer) -> Self { Self::Flow(v) }
}

// =========================================================================
// Widget trait — zero-cost reusable components
// =========================================================================

/// Trait for reusable, composable UI components.
///
/// Implementors produce a `LayoutChild` from existing primitives (Column, Row, etc.).
/// The blanket `From` impl means any `Widget` works with `.push()` on all containers.
///
/// # Zero-cost
///
/// `build()` consumes `self` by value. The widget struct lives on the stack,
/// and the returned `LayoutChild` is an enum variant — no heap allocation
/// (except for the boxed containers, which is necessary for recursion).
///
/// # Example
///
/// ```ignore
/// struct Card { inner: Column }
///
/// impl Card {
///     fn new(title: &str) -> Self {
///         Card {
///             inner: Column::new()
///                 .padding(10.0)
///                 .background(Color::rgb(0.1, 0.1, 0.1))
///                 .push(TextElement::new(title))
///         }
///     }
///
///     fn push(mut self, child: impl Into<LayoutChild>) -> Self {
///         self.inner = self.inner.push(child);
///         self
///     }
/// }
///
/// impl Widget for Card {
///     fn build(self) -> LayoutChild { self.inner.into() }
/// }
///
/// // Works with .push() on any container:
/// scroll_col.push(Card::new("Settings").push(some_input))
/// ```
pub trait Widget {
    /// Consume this widget and produce a layout node.
    fn build(self) -> LayoutChild;
}

/// Blanket impl: any `Widget` can be used with `.push()` on containers.
///
/// This does NOT conflict with the explicit `From` impls above because
/// built-in types (Column, Row, TextElement, etc.) do not implement `Widget`.
impl<W: Widget> From<W> for LayoutChild {
    #[inline(always)]
    fn from(w: W) -> LayoutChild {
        w.build()
    }
}
