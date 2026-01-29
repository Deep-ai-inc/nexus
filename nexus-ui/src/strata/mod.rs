//! Strata: High-Performance GUI Abstraction Layer
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

// Re-export core types
pub use primitives::{Color, Rect, Size, Constraints, Point};
pub use content_address::{ContentAddress, SourceId, Selection, SourceOrdering};
pub use layout_snapshot::{LayoutSnapshot, SourceLayout, ItemLayout, TextLayout, GridLayout};
pub use event_context::{
    CaptureState, Event, EventContext, Key, KeyEvent, Modifiers, MouseButton, MouseEvent,
    NamedKey, ScrollDelta,
};

// Widget system (Phase 3)
// mod widget;
// mod virtual_list;
// pub use widget::{StrataWidget, Event, EventResult};
// pub use virtual_list::{VirtualList, ListDelegate, ListItemDescriptor};

// Text engine (Phase 2)
// mod text_engine;
// pub use text_engine::{TextEngine, TextHandle, TextAttrs, TextLayoutData};

// GPU pipeline (Phase 4)
// pub mod gpu;

// Widgets (Phase 4)
// pub mod widgets;

// Shell integration (Phase 2)
// pub mod shell;

// App trait (Phase 2)
// mod app;
// pub use app::{StrataApp, Command, Subscription};
