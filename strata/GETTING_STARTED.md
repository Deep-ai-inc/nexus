# Getting Started with Strata

Strata is a GPU-accelerated UI framework for Rust. It renders directly to Metal on macOS with a flexbox-inspired layout system, built-in text editing, scrolling, and cross-widget text selection.

> **Platform:** macOS only (Metal + native NSApplication backend).

## Setup

Add Strata as a git dependency:

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2024"

[dependencies]
strata = { git = "https://github.com/Deep-ai-inc/nexus" }
```

## Minimal App

Every Strata app implements the `StrataApp` trait — three required methods and an entry point:

```rust
use strata::*;
use strata::layout::{LayoutContext, LayoutConstraints};

struct App;

#[derive(Clone, Debug)]
enum Msg {
    Increment,
}

struct State {
    count: i32,
}

impl StrataApp for App {
    type State = State;
    type Message = Msg;
    type SharedState = ();

    fn init(_shared: &(), _images: &mut ImageStore) -> (State, Command<Msg>) {
        (State { count: 0 }, Command::none())
    }

    fn update(state: &mut State, msg: Msg, _images: &mut ImageStore) -> Command<Msg> {
        match msg {
            Msg::Increment => state.count += 1,
        }
        Command::none()
    }

    fn view(state: &State, snapshot: &mut LayoutSnapshot) {
        let vp = snapshot.viewport();

        Column::new()
            .width(Length::Fixed(vp.width))
            .height(Length::Fixed(vp.height))
            .padding(24.0)
            .spacing(12.0)
            .push(TextElement::new(format!("Count: {}", state.count)).size(24.0))
            .push(TextElement::new("Press Up to increment").color(Color::rgb(0.5, 0.5, 0.6)))
            .layout_with_constraints(
                &mut LayoutContext::new(snapshot),
                LayoutConstraints::tight(vp.width, vp.height),
                Point::ORIGIN,
            );
    }

    fn selection(_state: &State) -> Option<&Selection> {
        None
    }
}

fn main() {
    strata::shell::run::<App>().unwrap();
}
```

That gives you a window with GPU-rendered text. Now let's make it interactive.

## Layout System

Strata uses containers (Column, Row, ScrollColumn, FlowContainer) and leaf elements (TextElement, ButtonElement, ImageElement, TextInputElement, TableElement, Canvas). They compose like this:

```rust
Row::new()
    .width(Length::Fill)
    .spacing(12.0)
    .push(
        Column::new()
            .width(Length::FillPortion(2))  // 2/3 of the row
            .spacing(8.0)
            .push(TextElement::new("Left panel"))
            .push(TextElement::new("More content"))
    )
    .push(
        Column::new()
            .width(Length::FillPortion(1))  // 1/3 of the row
            .push(TextElement::new("Right panel"))
    )
```

### Sizing

| Value | Behavior |
|---|---|
| `Length::Shrink` | Fit content (default) |
| `Length::Fill` | Expand to fill available space |
| `Length::FillPortion(n)` | Proportional flex |
| `Length::Fixed(px)` | Exact pixel size |

### Styling

Containers support backgrounds, borders, corner radii, and shadows:

```rust
Column::new()
    .background(Color::rgb(0.1, 0.1, 0.14))
    .corner_radius(8.0)
    .border(Color::rgba(1.0, 1.0, 1.0, 0.1), 1.0)
    .shadow(12.0, Color::rgba(0.0, 0.0, 0.0, 0.5))
    .padding(16.0)
    .push(TextElement::new("Styled card"))
```

### Row Helpers

Rows have a `.spacer(weight)` method that inserts flexible space, useful for pushing items to opposite ends:

```rust
Row::new()
    .width(Length::Fill)
    .push(TextElement::new("Left"))
    .spacer(1.0)  // flexible gap
    .push(TextElement::new("Right"))
```

`.fixed_spacer(px)` inserts a fixed-size gap on either Row or Column.

### FlowContainer

For inline wrapping layout (like HTML inline elements):

```rust
FlowContainer::new()
    .spacing(4.0)
    .line_spacing(4.0)
    .width(Length::Fill)
    .push(TextElement::new("These"))
    .push(TextElement::new("words").color(Color::rgb(0.3, 0.7, 1.0)))
    .push(TextElement::new("wrap automatically"))
```

---

## Event Handling

Events in Strata follow the Elm pattern: events produce messages, messages drive state changes in `update()`. Event handlers (`on_key`, `on_mouse`, `on_file_drop`) take `&self`/`&State` (read-only) and return messages. All mutation happens in `update()`.

### Keyboard Events

Override `on_key` to handle key presses and releases:

```rust
fn on_key(state: &State, event: KeyEvent) -> Option<Msg> {
    // Ignore key releases
    if matches!(&event, KeyEvent::Released { .. }) {
        return None;
    }

    if let KeyEvent::Pressed { key, modifiers, .. } = &event {
        // Cmd+S
        if modifiers.meta && key == &Key::Character("s".into()) {
            return Some(Msg::Save);
        }

        // Arrow keys, Escape, etc.
        match key {
            Key::Named(NamedKey::ArrowUp) => return Some(Msg::ScrollUp),
            Key::Named(NamedKey::ArrowDown) => return Some(Msg::ScrollDown),
            Key::Named(NamedKey::Escape) => return Some(Msg::Cancel),
            Key::Named(NamedKey::PageUp) => return Some(Msg::PageUp),
            Key::Named(NamedKey::PageDown) => return Some(Msg::PageDown),
            _ => {}
        }
    }
    None
}
```

The `modifiers` struct has fields: `meta` (Cmd), `shift`, `alt` (Option), `ctrl`.

### Mouse Events

Override `on_mouse` for clicks, drags, cursor movement, and scroll wheel. The `hit` parameter tells you what's under the cursor via the layout snapshot's hit-testing:

```rust
fn on_mouse(
    state: &State,
    event: MouseEvent,
    hit: Option<HitResult>,
    capture: &CaptureState,
) -> MouseResponse<Msg> {
    match event {
        // Left click
        MouseEvent::ButtonPressed { button: MouseButton::Left, .. } => {
            if let Some(HitResult::Widget(id)) = &hit {
                // Clicked on a registered widget (button, sortable header, etc.)
                if *id == SourceId::named("my_button") {
                    return MouseResponse::message(Msg::ButtonClicked);
                }
            }
            if let Some(HitResult::Content(addr)) = hit {
                // Clicked on selectable text content
                return MouseResponse::message(Msg::SelectStart(addr));
            }
        }

        // Cursor moved
        MouseEvent::CursorMoved { position, .. } => {
            // Track hover state for button highlights
            if let Some(HitResult::Widget(id)) = &hit {
                return MouseResponse::message(Msg::Hover(Some(*id)));
            }
            return MouseResponse::message(Msg::Hover(None));
        }

        // Scroll wheel / trackpad
        MouseEvent::Scroll { delta, phase, .. } => {
            let pixels = match delta {
                ScrollDelta::Pixels(_, y) => y,
                ScrollDelta::Lines(_, y) => y * 40.0,
            };
            return MouseResponse::message(Msg::ScrollBy(pixels, phase));
        }

        // Left button released
        MouseEvent::ButtonReleased { button: MouseButton::Left, .. } => {
            // ...
        }

        _ => {}
    }
    MouseResponse::none()
}
```

`HitResult` has two variants:
- `HitResult::Widget(SourceId)` — cursor is over a registered widget (buttons, sortable headers, clickable cells)
- `HitResult::Content(ContentAddress)` — cursor is over selectable text content at a specific character position

### Pointer Capture (Drag Operations)

For drag operations (text selection, scrollbar thumb dragging, image dragging), capture the pointer so you receive mouse events even when the cursor leaves the widget:

```rust
// On mouse down — start drag and capture the pointer
MouseResponse::message_and_capture(Msg::DragStart(pos), source_id)

// On mouse move while captured — continue dragging
MouseResponse::message(Msg::DragMove(pos))

// On mouse up — end drag and release capture
MouseResponse::message_and_release(Msg::DragEnd)
```

Check capture state to know if you're in a drag:

```rust
MouseEvent::CursorMoved { .. } => {
    if let CaptureState::Captured(captured_source) = capture {
        // Currently dragging — captured_source tells you what initiated the drag
        if let Some(HitResult::Content(addr)) = hit {
            return MouseResponse::message(Msg::SelectExtend(addr));
        }
    }
}
```

### Composable Mouse Routing

When your app has multiple scroll regions, text inputs, or other stateful widgets that each handle their own mouse events, use the `route_mouse!` macro for zero-cost routing:

```rust
fn on_mouse(
    state: &State,
    event: MouseEvent,
    hit: Option<HitResult>,
    capture: &CaptureState,
) -> MouseResponse<Msg> {
    // Each target's handle_mouse checks if the event is relevant to it.
    // First match wins. Expands to flat if-let chains — no allocation.
    route_mouse!(&event, &hit, capture, [
        state.sidebar_scroll  => Msg::SidebarScroll,
        state.content_scroll  => Msg::ContentScroll,
        state.search_input    => Msg::SearchInput,
        state.editor_input    => Msg::EditorInput,
    ]);

    // Handle everything else (buttons, selection, etc.)
    // ...

    MouseResponse::none()
}
```

This replaces manual chains of:
```rust
if let Some(r) = state.sidebar_scroll.handle_mouse(&event, &hit, capture) {
    return r.map(Msg::SidebarScroll);
}
if let Some(r) = state.content_scroll.handle_mouse(&event, &hit, capture) {
    return r.map(Msg::ContentScroll);
}
// ...
```

### Hover Tracking

Use `Cell<Option<SourceId>>` for hover state so `on_mouse` (which takes `&State`) can write it and `view` can read it:

```rust
struct State {
    hovered: Cell<Option<SourceId>>,
}

// In on_mouse:
MouseEvent::CursorMoved { .. } => {
    state.hovered.set(match &hit {
        Some(HitResult::Widget(id)) => Some(*id),
        _ => None,
    });
}

// In view — conditional styling:
let is_hovered = state.hovered.get() == Some(btn_id);
let bg = if is_hovered {
    Color::rgb(0.25, 0.55, 0.35)
} else {
    Color::rgb(0.15, 0.45, 0.25)
};
column.push(ButtonElement::new(btn_id, "Click me").background(bg))
```

### Focus Management

Manage focus centrally in your state. Route key events to the focused widget:

```rust
struct State {
    focused: Option<SourceId>,
    input_a: TextInputState,
    input_b: TextInputState,
}

impl State {
    fn focus(&mut self, id: SourceId) {
        self.focused = Some(id);
        self.input_a.focused = self.input_a.id() == id;
        self.input_b.focused = self.input_b.id() == id;
    }

    fn blur_all(&mut self) {
        self.focused = None;
        self.input_a.focused = false;
        self.input_b.focused = false;
    }
}

// In on_key — route to the focused input:
fn on_key(state: &State, event: KeyEvent) -> Option<Msg> {
    if state.input_a.focused {
        return Some(Msg::InputAKey(event));
    }
    if state.input_b.focused {
        return Some(Msg::InputBKey(event));
    }
    // Global shortcuts when nothing is focused
    None
}

// In on_mouse — focus on click, blur on background click:
if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = &event {
    // TextInput handle_mouse already returns focus-claiming messages
    // For background clicks:
    if hit.is_none() {
        return MouseResponse::message(Msg::BlurAll);
    }
}
```

### File Drop Events

Handle files dragged onto the window from Finder:

```rust
fn on_file_drop(
    state: &State,
    event: FileDropEvent,
    hit: Option<HitResult>,
) -> Option<Msg> {
    match event {
        FileDropEvent::Dropped { paths, .. } => {
            Some(Msg::FilesDropped(paths))
        }
        FileDropEvent::Hovered { .. } => {
            Some(Msg::DropHover(true))
        }
        FileDropEvent::Left => {
            Some(Msg::DropHover(false))
        }
    }
}
```

### Native Drag (Outbound)

Start OS-level drags from your app. Return a `DragSource` from your message handling:

```rust
DragSource::File(path)        // Drag a file — Finder accepts it
DragSource::Text(string)      // Drag plain text
DragSource::Tsv(data)         // Drag TSV — spreadsheets accept structured paste
DragSource::Image(path)       // Drag an image file
```

### Tick (60fps Timer)

`on_tick` is called every frame (~60fps). Use it for animations, spring-back physics, and auto-scroll during drags. Return `true` if you changed state and need a re-render, `false` to skip the render pass (saves GPU work when idle):

```rust
fn on_tick(state: &mut State) -> bool {
    let mut needs_render = false;

    // Rubber-band scroll spring-back
    if state.scroll.tick_spring_back() {
        needs_render = true;
    }

    // Cursor blink timer
    if state.cursor_blink_dirty() {
        needs_render = true;
    }

    needs_render
}
```

---

## Images

Load images through the `ImageStore` in `init` or dynamically in `update`:

```rust
struct State {
    logo: ImageHandle,
    thumbnails: Vec<ImageHandle>,
}

fn init(_shared: &(), images: &mut ImageStore) -> (State, Command<Msg>) {
    // Load a PNG from disk
    let logo = images.load_png("assets/logo.png").unwrap();

    // Generate a test gradient (useful for development)
    let placeholder = images.load_test_gradient(128, 128);

    (State { logo, thumbnails: vec![] }, Command::none())
}

fn update(state: &mut State, msg: Msg, images: &mut ImageStore) -> Command<Msg> {
    match msg {
        Msg::ImageDownloaded(bytes) => {
            // Load from raw bytes at runtime
            if let Ok(handle) = images.load_png_bytes(&bytes) {
                state.thumbnails.push(handle);
            }
        }
        _ => {}
    }
    Command::none()
}
```

Display with `ImageElement`:

```rust
// Basic image
ImageElement::new(state.logo, 200.0, 150.0)

// With rounded corners
ImageElement::new(state.logo, 200.0, 150.0)
    .corner_radius(8.0)
```

`ImageHandle` is a lightweight handle (just an index) — copying is free. The GPU uploads happen once when the image is loaded.

---

## Scrollable Areas

### ScrollColumn

`ScrollColumn` is a virtualized vertical scroll container. Only children intersecting the viewport are rendered.

**State setup:**

```rust
struct State {
    scroll: ScrollState,
}
```

`ScrollState` encapsulates the scroll offset, maximum scroll bounds, scrollbar track geometry, thumb drag state, and overscroll physics. It auto-generates two `SourceId`s (one for the scroll area, one for the thumb).

**View — two ways to create:**

```rust
// Option A: from_state (recommended) — zero-cost sync via interior mutability
ScrollColumn::from_state(&state.scroll)
    .width(Length::Fill)
    .height(Length::Fill)
    .spacing(8.0)
    .push(/* children */)

// Option B: manual IDs (if you need explicit control)
ScrollColumn::new(state.scroll.id(), state.scroll.thumb_id())
    .scroll_offset(state.scroll.effective_offset())
    .width(Length::Fill)
    .height(Length::Fill)
    .push(/* children */)
```

With `from_state`, the scroll limits and track geometry are updated automatically during layout. With manual construction, call `sync_from_snapshot` after layout:

```rust
state.scroll.sync_from_snapshot(snapshot);
```

**Update — apply scroll actions:**

```rust
Msg::Scroll(action) => state.scroll.apply(action),
```

**Mouse — route scroll events:**

```rust
// In on_mouse — this handles wheel scrolling AND thumb dragging:
if let Some(r) = state.scroll.handle_mouse(&event, &hit, capture) {
    return r.map(Msg::Scroll);
}
```

**Overscroll — elastic rubber-band effect:**

```rust
fn on_tick(state: &mut State) -> bool {
    state.scroll.tick_spring_back()
}
```

### ListView (Virtualized Lists)

For large datasets (thousands of items), `ListView` handles virtualization automatically. It only builds layout elements for visible items, using spacers for off-screen items to preserve correct scrollbar proportions:

```rust
struct ChatItem {
    source: SourceId,
    text: String,
}

// Estimate each item's height (must be fast — called for every item)
fn item_height(item: &ChatItem) -> f32 {
    let lines = item.text.lines().count().max(1) as f32;
    20.0 + lines * 18.0 + 4.0  // padding + text + spacing
}

// Build the visible item's layout element
fn build_item<'a>(item: &'a ChatItem, _index: usize) -> LayoutChild<'a> {
    Column::new()
        .padding(10.0)
        .spacing(4.0)
        .background(Color::rgb(0.08, 0.08, 0.11))
        .corner_radius(6.0)
        .width(Length::Fill)
        .push(TextElement::new(&item.text).source(item.source))
        .into()
}

// In view():
let chat_list = ListView::new(
    &state.items,
    item_height,
    build_item,
)
.spacing(8.0)
.scroll_offset(state.scroll.offset)
.viewport_height(state.scroll.bounds.get().height)
.width(Length::Fill)
.id(SourceId::named("chat_list"))
.build();  // Returns a Column with spacers + visible items

scroll_column.push(chat_list)
```

---

## Tables

### TableElement

For small-to-medium tables where all rows are known upfront:

```rust
let mut table = TableElement::new(SourceId::named("file_table"))
    // Non-sortable column
    .column("TYPE", 60.0)
    // Sortable columns — SourceId is used for click handling in on_mouse
    .column_sortable("NAME", 200.0, SourceId::named("sort_name"))
    .column_sortable("SIZE", 80.0, SourceId::named("sort_size"));

// Add rows
for file in &state.files {
    table = table.row(vec![
        TableCell {
            text: file.kind.into(),
            lines: vec![file.kind.into()],
            color: Color::rgb(0.4, 0.4, 0.45),
            widget_id: None,
        },
        TableCell {
            text: file.name.clone(),
            lines: vec![file.name.clone()],
            color: Color::rgb(0.4, 0.6, 1.0),
            widget_id: Some(SourceId::named(&format!("file_{}", file.name))),
            // ^ Makes this cell clickable
        },
        TableCell {
            text: file.size.clone(),
            lines: vec![file.size.clone()],
            color: Color::rgb(0.55, 0.55, 0.6),
            widget_id: None,
        },
    ]);
}

column.push(table)
```

**Multi-line cells:** Put multiple strings in the `lines` vec. The row height expands automatically.

**Sorting:** Handle header clicks in `on_mouse`:

```rust
if let Some(HitResult::Widget(id)) = &hit {
    if *id == SourceId::named("sort_name") {
        return MouseResponse::message(Msg::SortBy(0));
    }
    if *id == SourceId::named("sort_size") {
        return MouseResponse::message(Msg::SortBy(1));
    }
}
```

Then re-sort the data in `update` and toggle a sort direction flag. Render the sort arrow in the header name: `format!("NAME {}", if asc { "\u{25B2}" } else { "\u{25BC}" })`.

**Styling:**

```rust
table
    .header_bg(Color::rgba(0.15, 0.15, 0.2, 1.0))
    .header_text_color(Color::rgba(0.6, 0.6, 0.65, 1.0))
    .row_height(22.0)
    .header_height(26.0)
    .stripe_color(Some(Color::rgba(1.0, 1.0, 1.0, 0.02)))
    .separator_color(Color::rgba(1.0, 1.0, 1.0, 0.12))
```

### VirtualTableElement

For large tables (thousands of rows), `VirtualTableElement` only renders visible rows. The API is the same, but cells use `VirtualCell` instead of `TableCell`:

```rust
let mut table = VirtualTableElement::new(SourceId::named("big_table"))
    .column_sortable("STATUS", 100.0, SourceId::named("sort_status"))
    .column("NAME", 200.0)
    .column("PREVIEW", 80.0);

for item in &state.items {
    table = table.row(vec![
        // Badge cell: colored dot + text
        VirtualCell::badge(
            item.status_color,
            item.status.clone(),
            Color::rgb(0.85, 0.85, 0.88),
        ),
        // Plain text cell
        VirtualCell::text(item.name.clone(), Color::WHITE),
        // Image cell: inline thumbnail
        VirtualCell::image(item.thumbnail, 64, 64),
    ]);
}
```

Cell types:
- `VirtualCell::text(string, color)` — plain text
- `VirtualCell::badge(dot_color, text, text_color)` — colored dot indicator + text label
- `VirtualCell::image(handle, width, height)` — inline image, auto-scaled to row height

Make a cell clickable with `.with_widget_id(source_id)`.

---

## Canvas Drawing

`Canvas` accepts a drawing closure for custom graphics. The closure receives the computed bounds and a `PrimitiveBatch`:

```rust
Canvas::new(|bounds, p| {
    // Background
    p.add_rounded_rect(bounds, 8.0, Color::rgb(0.08, 0.08, 0.11));

    // Text
    p.add_text("Chart Title", Point::new(bounds.x + 10.0, bounds.y + 6.0),
        Color::rgb(0.6, 0.6, 0.65), 14.0);

    // Styled text
    p.add_text_styled("Bold label", Point::new(bounds.x + 10.0, bounds.y + 28.0),
        Color::WHITE, 14.0, true, false);  // bold=true, italic=false

    // Lines
    p.add_line(
        Point::new(bounds.x + 10.0, bounds.y + 50.0),
        Point::new(bounds.x + bounds.width - 10.0, bounds.y + 50.0),
        1.0, Color::rgb(0.3, 0.3, 0.35),
    );

    // Dashed/dotted lines
    p.add_line_styled(
        Point::new(bounds.x + 10.0, bounds.y + 60.0),
        Point::new(bounds.x + bounds.width - 10.0, bounds.y + 60.0),
        1.5, Color::rgb(0.8, 0.5, 0.2), LineStyle::Dashed,
    );

    // Polylines (charts, graphs)
    let points: Vec<Point> = (0..50).map(|i| {
        let t = i as f32 / 49.0;
        let x = bounds.x + 10.0 + t * (bounds.width - 20.0);
        let y = bounds.y + 120.0 - (t * std::f32::consts::PI * 2.0).sin() * 30.0;
        Point::new(x, y)
    }).collect();
    p.add_polyline(points, 1.5, Color::rgb(0.3, 0.7, 1.0));

    // Circles
    p.add_circle(Point::new(bounds.x + 50.0, bounds.y + 160.0), 6.0,
        Color::rgb(0.3, 0.8, 0.5));

    // Borders (hollow rounded rect)
    p.add_border(
        Rect::new(bounds.x + 10.0, bounds.y + 180.0, 100.0, 40.0),
        4.0, 1.0, Color::rgba(1.0, 1.0, 1.0, 0.12),
    );

    // Drop shadows (draw BEFORE the content they shadow)
    let card = Rect::new(bounds.x + 130.0, bounds.y + 176.0, 100.0, 48.0);
    p.add_shadow(Rect::new(card.x + 4.0, card.y + 4.0, card.width, card.height),
        8.0, 12.0, Color::rgba(0.0, 0.0, 0.0, 0.5));
    p.add_rounded_rect(card, 8.0, Color::rgb(0.12, 0.12, 0.18));

    // Images at arbitrary positions
    p.add_image(
        Rect::new(bounds.x + 10.0, bounds.y + 230.0, 64.0, 64.0),
        state.icon_handle, 4.0, Color::WHITE,
    );
})
.width(Length::Fill)
.height(Length::Fixed(300.0))
.id(SourceId::named("my_canvas"))  // optional — makes it hit-testable
```

**PrimitiveBatch drawing methods:**

| Method | Description |
|---|---|
| `add_solid_rect(rect, color)` | Filled rectangle |
| `add_rounded_rect(rect, radius, color)` | Rounded filled rectangle |
| `add_circle(center, radius, color)` | Filled circle |
| `add_line(p1, p2, thickness, color)` | Solid line segment |
| `add_line_styled(p1, p2, thickness, color, style)` | Dashed/dotted line |
| `add_polyline(points, thickness, color)` | Connected line segments |
| `add_polyline_styled(points, thickness, color, style)` | Styled polyline |
| `add_text(text, position, color, font_size)` | Text at a position |
| `add_text_styled(text, pos, color, size, bold, italic)` | Bold/italic text |
| `add_border(rect, radius, width, color)` | Hollow rounded outline |
| `add_shadow(rect, radius, blur, color)` | Drop shadow |
| `add_image(rect, handle, radius, tint)` | Image at arbitrary position |

`LineStyle` options: `LineStyle::Solid`, `LineStyle::Dashed`, `LineStyle::Dotted`.

**Capturing state in closures:** Since the Canvas closure is `FnOnce`, you can move data into it:

```rust
let anim_time = state.elapsed;
Canvas::new(move |bounds, p| {
    let phase = anim_time * 2.0;
    // Use phase for animated drawing...
})
```

---

## Text Input

`TextInputState` provides a full text editor with cursor movement, word-wise navigation, selection, clipboard, and undo:

```rust
struct State {
    input: TextInputState,
    editor: TextInputState,
}

// Create inputs
let input = TextInputState::single_line("my-input");
let editor = TextInputState::multi_line_with_text("editor", "Initial text\nLine two");
```

**Route keyboard events to the focused input:**

```rust
fn on_key(state: &State, event: KeyEvent) -> Option<Msg> {
    if state.editor.focused {
        return Some(Msg::EditorKey(event));
    }
    if state.input.focused {
        return Some(Msg::InputKey(event));
    }
    None
}
```

**Handle in update:**

```rust
Msg::InputKey(event) => {
    match state.input.handle_key(&event, false) {
        TextInputAction::Submit(text) => {
            // Enter pressed — process the input
            return Command::message(Msg::ProcessCommand);
        }
        _ => {}
    }
}
Msg::EditorKey(event) => {
    // Second param true = allow newlines (multi-line mode)
    state.editor.handle_key(&event, true);
}
```

**Mouse events (click to place cursor, drag to select):**

```rust
// In on_mouse — TextInputState handles its own mouse routing:
route_mouse!(&event, &hit, capture, [
    state.input  => Msg::InputMouse,
    state.editor => Msg::EditorMouse,
]);

// In update:
Msg::InputMouse(action) => {
    state.focus(state.input.id());
    state.input.apply_mouse(action);
}
```

**In view:**

```rust
TextInputElement::from_state(&state.input)
    .width(Length::Fill)
    .placeholder("Type a command...")
    .background(Color::rgba(0.0, 0.0, 0.0, 0.0))
    .border_color(Color::rgba(1.0, 1.0, 1.0, 0.12))
    .focus_border_color(Color::rgb(0.3, 0.5, 0.9))
    .corner_radius(4.0)
    .padding(Padding::new(8.0, 12.0, 8.0, 12.0))
    .cursor_visible(cursor_visible)  // for blink animation

TextInputElement::from_state(&state.editor)
    .height(Length::Fixed(120.0))
    .placeholder("Multi-line editor...")
```

**Cursor blink:** Track edit time, toggle every 500ms:

```rust
// In state:
last_edit_time: Instant,

// In update (reset on any edit):
state.last_edit_time = Instant::now();

// In view:
let blink_elapsed = Instant::now().duration_since(state.last_edit_time).as_millis();
let cursor_visible = (blink_elapsed / 500) % 2 == 0;
```

**Reading input state:**

```rust
let text = &state.input.text;           // Current text content
let cursor = state.input.cursor;         // Cursor position (byte offset)
let selection = &state.input.selection;  // Option<(start, end)>
```

---

## Text Selection

Cross-widget text selection lets users select text spanning multiple blocks. Tag text elements with a `SourceId` and handle the selection lifecycle:

```rust
// In view — tag selectable text:
TextElement::new(&block.text)
    .source(block.source_id)  // registers for hit-testing

// In state:
selection: Option<Selection>,

// In on_mouse:
MouseEvent::ButtonPressed { button: MouseButton::Left, .. } => {
    if let Some(HitResult::Content(addr)) = hit {
        let capture_source = addr.source_id;
        return MouseResponse::message_and_capture(
            Msg::SelectStart(addr), capture_source,
        );
    }
}
MouseEvent::CursorMoved { .. } => {
    if let CaptureState::Captured(_) = capture {
        if let Some(HitResult::Content(addr)) = hit {
            return MouseResponse::message(Msg::SelectExtend(addr));
        }
    }
}
MouseEvent::ButtonReleased { button: MouseButton::Left, .. } => {
    if let CaptureState::Captured(_) = capture {
        return MouseResponse::message_and_release(Msg::SelectEnd);
    }
}

// In update:
Msg::SelectStart(addr) => {
    state.selection = Some(Selection::new(addr.clone(), addr));
}
Msg::SelectExtend(addr) => {
    if let Some(sel) = &mut state.selection {
        sel.focus = addr;
    }
}
Msg::SelectEnd => {}
Msg::ClearSelection => {
    state.selection = None;
}

// Expose to the renderer:
fn selection(state: &State) -> Option<&Selection> {
    state.selection.as_ref()
}
```

Strata's renderer draws selection highlights automatically based on the `Selection` you return.

---

## Buttons

`ButtonElement` registers a hit-testable region. All interaction is handled in `on_mouse`:

```rust
// In view():
let btn_id = SourceId::named("save");
column.push(
    ButtonElement::new(btn_id, "Save")
        .padding(Padding::symmetric(16.0, 8.0))
        .background(Color::rgb(0.2, 0.5, 0.3))
        .corner_radius(4.0)
);

// In on_mouse():
if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = event {
    if let Some(HitResult::Widget(id)) = hit {
        if id == SourceId::named("save") {
            return MouseResponse::message(Msg::Save);
        }
    }
}
```

For hover effects, see the [Hover Tracking](#hover-tracking) section above.

---

## Async Commands

Return `Command` from `update` to run async work:

```rust
Msg::FetchData => {
    Command::perform(async {
        let data = reqwest::get("https://api.example.com/data")
            .await.unwrap()
            .text().await.unwrap();
        Msg::DataLoaded(data)
    })
}
Msg::DataLoaded(data) => {
    state.data = data;
    Command::none()
}
```

**Batch multiple commands:**

```rust
Command::batch([
    Command::perform(fetch_users(), Msg::UsersLoaded),
    Command::perform(fetch_config(), Msg::ConfigLoaded),
])
```

**Immediate message** (useful for internal routing):

```rust
Command::message(Msg::SubmitCommand)
```

---

## Subscriptions

Subscribe to external async event streams. The shell polls these each frame:

```rust
use strata::shell::subscription;

fn subscription(state: &State) -> Subscription<Msg> {
    Subscription::batch([
        // From an mpsc receiver
        subscription::from_receiver(state.events_rx.clone())
            .map(Msg::ExternalEvent),

        // Batched — coalesces rapid events into Vec<T>
        subscription::from_receiver_batched(state.log_rx.clone(), 0, 100)
            .map(Msg::LogBatch),

        // From a broadcast channel (multi-consumer)
        subscription::from_broadcast(state.broadcast_rx.clone())
            .map(Msg::BroadcastEvent),
    ])
}
```

---

## Window Configuration

Use `run_with_config` to customize the window:

```rust
fn main() {
    strata::shell::run_with_config::<App>(AppConfig {
        title: "My App".into(),
        window_size: (1200.0, 800.0),
        antialiasing: true,
        background_color: Color::rgb(0.04, 0.04, 0.06),
    }).unwrap();
}
```

Dynamic title — override `title()` on `StrataApp`:

```rust
fn title(state: &State) -> String {
    format!("My App — {}", state.current_file)
}
```

---

## Custom Widgets

The `Widget` trait lets you build reusable view fragments. A widget is just a struct that produces layout elements:

```rust
use strata::{Widget, Element, Column, Row, TextElement, Length, Color, Padding};

struct InfoCard<'a> {
    title: &'a str,
    body: &'a str,
    accent: Color,
}

impl<'a> Widget<'a> for InfoCard<'a> {
    fn build(self) -> Element<'a> {
        Column::new()
            .padding(16.0)
            .spacing(8.0)
            .background(Color::rgb(0.08, 0.08, 0.11))
            .corner_radius(8.0)
            .border(self.accent.with_alpha(0.3), 1.0)
            .width(Length::Fill)
            .push(TextElement::new(self.title).bold().color(self.accent))
            .push(TextElement::new(self.body).color(Color::rgb(0.6, 0.6, 0.65)))
            .into()
    }
}

// Use in any container:
column.push(InfoCard { title: "Status", body: "Running", accent: Color::rgb(0.3, 0.8, 0.5) })
```

The `'a` lifetime lets widgets borrow from app state. Widgets that own their data can use `'static`:

```rust
impl Widget<'static> for StatusBadge {
    fn build(self) -> Element<'static> { /* ... */ }
}
```

### Container Widgets

Widgets can wrap containers and accept children via `.push()`:

```rust
struct Card<'a> {
    inner: Column<'a>,
}

impl<'a> Card<'a> {
    fn new(title: &str) -> Self {
        Card {
            inner: Column::new()
                .padding(12.0)
                .spacing(6.0)
                .background(Color::rgb(0.08, 0.08, 0.11))
                .corner_radius(6.0)
                .width(Length::Fill)
                .push(TextElement::new(title).color(Color::rgb(0.55, 0.55, 0.6))),
        }
    }

    fn push(mut self, child: impl Into<LayoutChild<'a>>) -> Self {
        self.inner = self.inner.push(child);
        self
    }
}

impl<'a> Widget<'a> for Card<'a> {
    fn build(self) -> Element<'a> {
        self.inner.into()
    }
}

// Usage:
Card::new("Settings")
    .push(TextElement::new("Dark mode: on"))
    .push(TextElement::new("Font size: 14px"))
```

---

## Staying Organized

### Small Apps: File-Per-Concern

For apps under ~1000 lines, split by concern:

```
my-app/
  src/
    main.rs          # fn main, StrataApp impl, Message enum
    state.rs         # State struct, initialization, helper methods
    view.rs          # view() function, custom widgets
    handlers.rs      # on_key, on_mouse, update logic
  assets/
    logo.png
```

### Medium Apps: Widget Modules

When you have 3+ distinct UI regions, give each its own module:

```
my-app/
  src/
    main.rs
    state.rs
    widgets/
      mod.rs          # re-exports
      sidebar.rs      # sidebar Widget + its builder logic
      editor.rs       # editor panel
      toolbar.rs      # toolbar with buttons
    handlers/
      mod.rs
      keyboard.rs
      mouse.rs
```

Each widget module exports a struct implementing `Widget`:

```rust
// widgets/sidebar.rs
pub struct Sidebar<'a> {
    pub items: &'a [Item],
    pub selected: usize,
    pub scroll: &'a ScrollState,
}

impl<'a> Widget<'a> for Sidebar<'a> {
    fn build(self) -> Element<'a> {
        let mut col = ScrollColumn::from_state(self.scroll)
            .width(Length::Fixed(250.0))
            .spacing(4.0);

        for (i, item) in self.items.iter().enumerate() {
            let bg = if i == self.selected {
                Color::rgb(0.15, 0.3, 0.5)
            } else {
                Color::TRANSPARENT
            };
            col = col.push(
                Row::new()
                    .padding(8.0)
                    .background(bg)
                    .corner_radius(4.0)
                    .width(Length::Fill)
                    .push(TextElement::new(&item.name).widget_id(item.id))
            );
        }

        col.into()
    }
}
```

### Large Apps: Component System

For complex apps with deeply nested state, use the `Component` trait. Components own their state as struct fields and have local message types. All dispatch is static — no trait objects, no vtables.

**The Component trait:**

```rust
use strata::component::{Component, ComponentApp, RootComponent, Ctx, IdSpace};

struct Sidebar {
    items: Vec<Item>,
    selected: usize,
    scroll: ScrollState,
}

#[derive(Debug)]
enum SidebarMsg {
    Select(usize),
    Scroll(ScrollAction),
}

impl Component for Sidebar {
    type Message = SidebarMsg;
    type Output = ();  // or a type for cross-cutting effects

    fn update(&mut self, msg: SidebarMsg, _ctx: &mut Ctx) -> (Command<SidebarMsg>, ()) {
        match msg {
            SidebarMsg::Select(i) => self.selected = i,
            SidebarMsg::Scroll(a) => self.scroll.apply(a),
        }
        (Command::none(), ())
    }

    fn view(&self, snapshot: &mut LayoutSnapshot, ids: IdSpace) {
        // Build layout using ids.id(n) for widget IDs
        // ...
    }

    fn on_key(&self, event: KeyEvent) -> Option<SidebarMsg> { None }
    fn on_mouse(&self, event: MouseEvent, hit: Option<HitResult>, capture: &CaptureState)
        -> MouseResponse<SidebarMsg> { MouseResponse::none() }
}
```

**Composing components — parent owns children as fields:**

```rust
struct App {
    sidebar: Sidebar,
    editor: Editor,
}

#[derive(Debug, Clone)]
enum AppMsg {
    Sidebar(SidebarMsg),
    Editor(EditorMsg),
}

impl Component for App {
    type Message = AppMsg;
    type Output = ();

    fn update(&mut self, msg: AppMsg, ctx: &mut Ctx) -> (Command<AppMsg>, ()) {
        match msg {
            AppMsg::Sidebar(m) => {
                let (cmd, _) = self.sidebar.update(m, ctx);
                (cmd.map_msg(AppMsg::Sidebar), ())
            }
            AppMsg::Editor(m) => {
                let (cmd, _) = self.editor.update(m, ctx);
                (cmd.map_msg(AppMsg::Editor), ())
            }
        }
    }

    fn view(&self, snapshot: &mut LayoutSnapshot, ids: IdSpace) {
        self.sidebar.view(snapshot, ids.child(1));
        self.editor.view(snapshot, ids.child(2));
    }

    fn on_key(&self, event: KeyEvent) -> Option<AppMsg> {
        if let Some(m) = self.sidebar.on_key(event.clone()) {
            return Some(AppMsg::Sidebar(m));
        }
        if let Some(m) = self.editor.on_key(event) {
            return Some(AppMsg::Editor(m));
        }
        None
    }
}
```

**IdSpace** provides zero-allocation, const-fn ID namespacing to avoid collisions between components:

```rust
const IDS: IdSpace = IdSpace::new(1);
const SIDEBAR: IdSpace = IDS.child(1);
const EDITOR: IdSpace = IDS.child(2);

// Each component uses its own IdSpace for widget IDs:
let button_id: SourceId = SIDEBAR.id(0);
let save_id: SourceId = EDITOR.id(1);
```

**Bridging to StrataApp:** Implement `RootComponent` on your top-level component and run it via `ComponentApp`:

```rust
impl RootComponent for App {
    type SharedState = ();

    fn create(_shared: &(), images: &mut ImageStore) -> (Self, Command<AppMsg>) {
        (App { sidebar: Sidebar::new(), editor: Editor::new() }, Command::none())
    }

    fn title(&self) -> String { "My App".into() }
    fn background_color(&self) -> Color { Color::rgb(0.04, 0.04, 0.06) }
}

fn main() {
    strata::shell::run::<ComponentApp<App>>().unwrap();
}
```

**Cross-cutting output:** Components can declare an `Output` type for effects that span children (focus changes, scroll-to-bottom, navigation):

```rust
impl Component for Editor {
    type Output = EditorOutput;  // instead of ()
    // ...
}

#[derive(Default)]
struct EditorOutput {
    pub file_saved: bool,
}
```

The parent reads the output after `update()` and acts on it.

---

## Running the Demo

The Strata repo includes a full demo app that exercises every feature described in this guide:

```sh
cargo run -p nexus-ui --example strata_demo
```
