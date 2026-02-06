# Strata

A GPU-accelerated UI engine for building high-performance terminal-like applications in Rust.

## Overview

Strata provides a declarative layout system with GPU-accelerated rendering, designed for applications that need:

- Cross-widget text selection
- High-performance scrolling with virtualization
- Accurate hit-testing for interactive elements
- Cached text shaping via cosmic-text

## Architecture

Strata follows the Elm architecture pattern:

```
init() → State
update(State, Message) → (State, Command)
view(State) → LayoutSnapshot
```

Each frame, `view()` builds a declarative layout tree that computes positions once and flushes primitives to a `LayoutSnapshot`. The GPU pipeline then renders from this snapshot.

### Lifetime-Based State Sync

Layout containers use Rust lifetimes to enable zero-cost state synchronization:

```rust
// ScrollColumn<'a> holds &'a ScrollState
// During layout, it updates scroll limits via Cell (interior mutability)
ScrollColumn::from_state(&state.scroll)
    .push(children)
```

This eliminates manual `sync_from_snapshot()` calls while maintaining zero runtime overhead. The borrow checker ensures the state reference is valid for the lifetime of the layout tree.

## Quick Start

```rust
use strata::{
    StrataApp, LayoutSnapshot, Command, MouseResponse,
    Column, Row, TextElement, Length, Element,
};

struct MyApp {
    counter: i32,
}

#[derive(Clone, Debug)]
enum Message {
    Increment,
}

impl StrataApp for MyApp {
    type State = Self;
    type Message = Message;
    type SharedState = ();

    fn init(_shared: &(), _images: &mut ImageStore) -> (Self, Command<Message>) {
        (MyApp { counter: 0 }, Command::none())
    }

    fn update(state: &mut Self, msg: Message, _images: &mut ImageStore) -> Command<Message> {
        match msg {
            Message::Increment => state.counter += 1,
        }
        Command::none()
    }

    fn view(state: &Self, snapshot: &mut LayoutSnapshot) {
        let (vw, vh) = snapshot.viewport_size();

        Column::new()
            .width(Length::Fixed(vw))
            .height(Length::Fixed(vh))
            .push(TextElement::new(format!("Count: {}", state.counter)))
            .layout_with_constraints(
                &mut LayoutContext::new(snapshot),
                LayoutConstraints::tight(vw, vh),
                Point::ORIGIN,
            );
    }

    fn selection(_state: &Self) -> Option<&Selection> {
        None
    }
}

fn main() {
    strata::shell::run::<MyApp>().unwrap();
}
```

## Layout System

### Containers

Strata provides flexbox-inspired layout containers:

#### Column

Vertical stack of children. Children flow top-to-bottom.

```rust
Column::new()
    .width(Length::Fill)           // Expand to fill parent width
    .height(Length::Shrink)        // Shrink to fit content
    .spacing(8.0)                  // Gap between children
    .padding(16.0)                 // Padding around all children
    .alignment(Alignment::Center)  // Main axis alignment
    .cross_alignment(CrossAxisAlignment::Start)
    .push(TextElement::new("First"))
    .push(TextElement::new("Second"))
```

#### Row

Horizontal stack of children. Children flow left-to-right.

```rust
Row::new()
    .width(Length::Fill)
    .spacing(12.0)
    .push(TextElement::new("Left"))
    .spacer(1.0)                   // Flexible spacer (pushes "Right" to end)
    .push(TextElement::new("Right"))
```

#### ScrollColumn

Virtualized vertical scroll container. Only visible children are rendered.

```rust
// In your state:
struct MyState {
    scroll: ScrollState,  // Manages scroll offset, thumb drag, etc.
}

// In view():
ScrollColumn::from_state(&state.scroll)  // Auto-syncs scroll limits during layout
    .width(Length::Fill)
    .height(Length::Fixed(400.0))
    .spacing(4.0)
    .push(/* ... many children ... */)
```

When using `from_state()`, the scroll state is automatically synchronized during layout - no manual `sync_from_snapshot()` call needed.

#### FlowContainer

Inline flow layout with automatic line wrapping (like HTML inline elements).

```rust
FlowContainer::new()
    .spacing(4.0)
    .text(TextElement::new("This"))
    .text(TextElement::new("text").color(Color::BLUE))
    .text(TextElement::new("wraps automatically"))
```

### Length

Controls how containers size themselves on each axis:

| Value | Behavior |
|-------|----------|
| `Length::Shrink` | Fit content (default) |
| `Length::Fill` | Expand to fill available space |
| `Length::FillPortion(n)` | Expand proportionally (flex: n) |
| `Length::Fixed(px)` | Exact pixel size |

### Custom Widgets

Create reusable UI components with the `Widget` trait:

```rust
use strata::{Widget, Element, Column, TextElement, Length};

struct Card<'a> {
    title: &'a str,
    content: &'a str,
}

impl<'a> Widget<'a> for Card<'a> {
    fn build(self) -> Element<'a> {
        Column::new()
            .padding(16.0)
            .spacing(8.0)
            .background(Color::rgb(0.15, 0.15, 0.18))
            .corner_radius(8.0)
            .push(TextElement::new(self.title).bold())
            .push(TextElement::new(self.content))
            .into()
    }
}

// Use in view():
column.push(Card { title: "Hello", content: "World" })
```

The `'a` lifetime allows widgets to borrow from application state. Widgets that don't borrow can use `'static`:

```rust
impl Widget<'static> for StaticBadge {
    fn build(self) -> Element<'static> { /* ... */ }
}
```

### Elements

Leaf nodes that render actual content:

#### TextElement

```rust
TextElement::new("Hello, world!")
    .color(Color::rgb(0.8, 0.9, 1.0))
    .size(16.0)           // Font size in pixels
    .bold()
    .italic()
    .source(source_id)    // Enable text selection
    .widget_id(click_id)  // Enable click detection
    .cursor_hint(CursorIcon::Pointer)
```

#### ButtonElement

```rust
ButtonElement::new(button_id, "Click me")
    .padding(Padding::symmetric(16.0, 8.0))
    .background(Color::rgb(0.2, 0.4, 0.8))
    .corner_radius(4.0)
```

#### ImageElement

```rust
ImageElement::new(image_handle, 200.0, 150.0)
    .corner_radius(8.0)
```

#### TextInputElement

```rust
// In state:
struct MyState {
    input: TextInputState,
}

// In view:
TextInputElement::from_state(&state.input)
    .width(Length::Fill)
    .multiline(true)
    .placeholder("Type here...")
```

## Event Handling

### Mouse Events

Implement `on_mouse` to handle mouse interactions:

```rust
fn on_mouse(
    state: &Self::State,
    event: MouseEvent,
    hit: Option<HitResult>,
    capture: &CaptureState,
) -> MouseResponse<Self::Message> {
    // Route to scroll containers first
    if let Some(r) = state.scroll.handle_mouse(&event, &hit, capture) {
        return r.map(Message::Scroll);
    }

    // Handle button clicks
    if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = event {
        if let Some(HitResult::Widget(id)) = hit {
            if id == state.button_id {
                return MouseResponse::message(Message::ButtonClicked);
            }
        }
    }

    MouseResponse::none()
}
```

### Keyboard Events

```rust
fn on_key(state: &Self::State, event: KeyEvent) -> Option<Self::Message> {
    if let KeyEvent::Pressed { key, modifiers, .. } = event {
        // Handle Cmd+S
        if modifiers.command() && key == Key::Character("s".into()) {
            return Some(Message::Save);
        }
    }
    None
}
```

### Pointer Capture

For drag operations, capture the pointer to receive events even when the cursor leaves the widget:

```rust
// On mouse down: start drag and capture
MouseResponse::message_and_capture(Message::DragStart, widget_id)

// On mouse move while captured: continue drag
MouseResponse::message(Message::DragMove(position))

// On mouse up: end drag and release capture
MouseResponse::message_and_release(Message::DragEnd)
```

## State Helpers

### ScrollState

Encapsulates scroll offset, thumb drag state, and bounds tracking:

```rust
struct MyState {
    scroll: ScrollState,
}

// In update():
fn update(state: &mut Self, msg: Message, _images: &mut ImageStore) -> Command<Message> {
    match msg {
        Message::Scroll(action) => state.scroll.apply(action),
    }
    Command::none()
}

// In view():
fn view(state: &Self, snapshot: &mut LayoutSnapshot) {
    ScrollColumn::from_state(&state.scroll)  // Auto-syncs during layout
        .push(/* children */)
        .layout_with_constraints(/* ... */);

    // No sync_from_snapshot needed - from_state handles it automatically
}
```

The `from_state()` constructor creates a `ScrollColumn<'a>` that holds a reference to the scroll state. During layout, it automatically updates the scroll limits, track geometry, and bounds via interior mutability (`Cell`). This is zero-cost at runtime.

### TextInputState

Full text editing with cursor movement, selection, clipboard, and undo:

```rust
struct MyState {
    input: TextInputState,
}

// Handle keyboard in on_key():
if let Some(action) = state.input.handle_key(&event) {
    return Some(Message::InputAction(action));
}

// Apply actions in update():
match msg {
    Message::InputAction(action) => {
        state.input.apply(action);
    }
}
```

## Content Addressing

Strata uses `SourceId` and `ContentAddress` for stable content identification across widget boundaries.

### SourceId

Identifies a logical data source:

```rust
// Auto-generated unique ID
let id = SourceId::new();

// Stable ID from a name (deterministic)
let id = SourceId::named("main-terminal");

// From an existing value (e.g., block ID)
let id = SourceId::from_raw(block.id);
```

### Selection

Cross-widget text selection:

```rust
let selection = Selection {
    anchor: ContentAddress {
        source_id: source1,
        item_index: 0,
        content_offset: 10,
    },
    focus: ContentAddress {
        source_id: source2,
        item_index: 5,
        content_offset: 25,
    },
};
```

## Hit Testing

The layout snapshot provides hit-testing after layout:

```rust
// In on_mouse():
match hit {
    Some(HitResult::Widget(id)) => {
        // Clicked on a registered widget
    }
    Some(HitResult::Content(address)) => {
        // Clicked on selectable content at this address
    }
    None => {
        // Clicked on empty space
    }
}
```

## Async Commands

Return `Command` from `update()` to perform async work:

```rust
fn update(state: &mut Self, msg: Message, _images: &mut ImageStore) -> Command<Message> {
    match msg {
        Message::FetchData => {
            Command::perform(
                async { fetch_from_api().await },
                |result| Message::DataLoaded(result),
            )
        }
        Message::DataLoaded(data) => {
            state.data = data;
            Command::none()
        }
    }
}
```

## Subscriptions

Subscribe to external event streams:

```rust
fn subscription(state: &Self::State) -> Subscription<Self::Message> {
    // Timer subscription via iced
    Subscription::from_iced(
        iced::time::every(Duration::from_secs(1))
            .map(|_| Message::Tick)
    )
}
```

## Performance

### Layout Caching

Containers compute `content_hash()` for memoization. Static UI sections get cache hits:

```rust
// Enable caching by providing a LayoutCache
let mut cache = LayoutCache::new();
let mut ctx = LayoutContext::with_cache(snapshot, &mut cache);
```

### Virtualization

`ScrollColumn` only renders visible children. For thousands of items:

```rust
ScrollColumn::from_state(&state.scroll)
    .push(/* 10,000 items - only ~50 visible are rendered */)
```

### Text Shaping Cache

TextElement automatically caches shaped text by content hash. Static text is shaped once:

```rust
// Same text = cache hit on text shaping
TextElement::new("Static label")  // Shaped once, cached forever
```

## Multi-Window

For apps with multiple windows, implement `SharedState` and `create_window`:

```rust
impl StrataApp for MyApp {
    type SharedState = Arc<SharedKernel>;

    fn create_window(
        shared: &Self::SharedState,
        images: &mut ImageStore,
    ) -> Option<(Self::State, Command<Self::Message>)> {
        Some((WindowState::new(shared.clone()), Command::none()))
    }

    fn is_new_window_request(msg: &Self::Message) -> bool {
        matches!(msg, Message::NewWindow)
    }
}
```

## Color

```rust
// Named constants
Color::WHITE
Color::BLACK
Color::TRANSPARENT

// RGB (0.0-1.0)
Color::rgb(0.2, 0.4, 0.8)

// RGBA
Color::rgba(0.2, 0.4, 0.8, 0.5)

// From 8-bit values
Color::rgb8(51, 102, 204)
Color::rgba8(51, 102, 204, 128)
```

## Debugging

In debug builds, enable layout tracing:

```rust
let mut ctx = LayoutContext::new(snapshot).with_debug(true);
// Logs constraint flow to stderr
```

## Platform Integration

### File Drops

```rust
fn on_file_drop(
    state: &Self::State,
    event: FileDropEvent,
    hit: Option<HitResult>,
) -> Option<Self::Message> {
    match event {
        FileDropEvent::Dropped { paths, .. } => {
            Some(Message::FilesDropped(paths))
        }
        _ => None,
    }
}
```

### Native Drag

Start OS-level drags from your app:

```rust
// Drag a file
DragSource::File(path)

// Drag text
DragSource::Text(string)

// Drag TSV (for spreadsheet paste)
DragSource::Tsv(data)
```
