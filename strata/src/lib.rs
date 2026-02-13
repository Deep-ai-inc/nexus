//! Strata: High-Performance GPU UI Engine
//!
//! Strata provides a unified content addressing and layout system that enables:
//! - Cross-widget text selection
//! - Accurate hit-testing via LayoutSnapshot
//! - Global pointer capture for drag operations
//! - GPU-accelerated rendering with cached text shaping
//!
//! # Architecture
//!
//! The core primitive is `ContentAddress`, which provides stable addressing
//! for any content regardless of which widget renders it. The `LayoutSnapshot`
//! captures layout information once per frame and serves both rendering and queries.
//!
//! # Usage
//!
//! Applications implement `StrataApp` and run via `shell::run()`:
//!
//! ```ignore
//! use strata::{StrataApp, LayoutSnapshot, Selection};
//!
//! struct MyApp { /* state */ }
//!
//! impl StrataApp for MyApp {
//!     // ...
//! }
//!
//! fn main() {
//!     strata::shell::run::<MyApp>().unwrap();
//! }
//! ```

// Core primitives
pub mod primitives;
pub mod content_address;
pub mod layout_snapshot;
pub mod event_context;

// Layout system (flexbox-inspired containers)
pub mod layout;

// Text engine (cosmic-text integration)
pub mod text_engine;

// State helpers
pub mod text_input_state;
pub mod scroll_state;

// Application trait
pub mod app;

// Shell integration (native macOS backend)
pub mod shell;

// Demo application
pub mod demo;
pub mod demo_widgets;

// Component system
pub mod component;

// Widget system
pub mod widget;
pub mod widgets;

// Platform-specific (native drag, etc.)
pub mod platform;

// GPU pipeline
pub mod gpu;

// Performance instrumentation
pub mod frame_timing;

// Re-export core types
pub use primitives::{Color, Rect, Size, Constraints, Point};
pub use content_address::{ContentAddress, SourceId, Selection, SourceOrdering};
pub use layout_snapshot::{Anchor, CursorIcon, Decoration, HitResult, LayoutSnapshot, ScrollTrackInfo, SourceLayout, ItemLayout, TextLayout, GridLayout, GridRow, TextRun, RunStyle, UnderlineStyle};
pub use event_context::{
    CaptureState, Event, EventContext, FileDropEvent, Key, KeyEvent, Modifiers, MouseButton,
    MouseEvent, NamedKey, ScrollDelta,
};
pub use app::{StrataApp, Command, Subscription, AppConfig, MouseResponse, CaptureRequest, DragSource};
pub use widget::{StrataWidget, StrataWidgetExt, EventResult, BoxedWidget};
pub use text_engine::{TextEngine, TextAttrs, ShapedText, FontFamily};
pub use widgets::{TextWidget, TerminalWidget};

// Layout system exports
pub use layout::{
    Column, Row, ScrollColumn, FlowContainer, Canvas, ListView,
    LayoutChild, Widget, Element, Padding, Alignment, CrossAxisAlignment, Length, LineStyle, PrimitiveBatch,
};
pub use layout::{TextElement, TerminalElement, ImageElement, ButtonElement, TextInputElement, TableElement, TableColumn, TableCell, VirtualTableElement, VirtualCell};
pub use gpu::{ImageHandle, ImageStore};
pub use text_input_state::{TextInputState, TextInputAction, TextInputMouseAction};
pub use scroll_state::{ScrollState, ScrollAction};
