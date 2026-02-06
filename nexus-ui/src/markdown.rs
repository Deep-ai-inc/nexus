//! Markdown rendering for agent mode responses.
//!
//! Uses pulldown-cmark for proper CommonMark + GFM parsing.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, HeadingLevel};
use strata::layout::containers::{Column, Length, Padding, Row, TextElement};
use strata::content_address::SourceId;

use crate::nexus_app::colors;

/// Render markdown text to a strata Column layout.
pub fn render(text: &str, source_id: SourceId) -> Column {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;

    let parser = Parser::new_ext(text, options);
    let mut renderer = MarkdownRenderer::new(source_id);
    renderer.render(parser);
    renderer.finish()
}

/// State tracking for markdown rendering
struct MarkdownRenderer {
    source_id: SourceId,
    /// Main column for block-level elements
    column: Column,
    /// Stack of active styles (bold, italic, etc.)
    style_stack: Vec<Style>,
    /// Current list nesting with (ordered, start_number) for each level
    list_stack: Vec<ListInfo>,
    /// Current blockquote nesting level
    blockquote_level: usize,
    /// Content for current code block (single string with newlines)
    code_block_content: String,
    /// Whether we're in a code block
    in_code_block: bool,
    /// Current table data
    table_rows: Vec<Vec<String>>,
    /// Current row cells
    current_row: Vec<String>,
    /// Current cell text
    current_cell: String,
    /// Whether we're in a table
    in_table: bool,
    /// Current heading level (0 = not in heading)
    heading_level: u8,
    /// Accumulated inline content (for paragraphs, list items, headings)
    inline_content: Vec<InlineSpan>,
    /// Whether we're currently in a list item
    in_list_item: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Bold,
    Italic,
    Strikethrough,
    Link,
}

struct ListInfo {
    ordered: bool,
    next_number: u64,
}

#[derive(Clone)]
struct InlineSpan {
    text: String,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    is_code: bool,
    is_link: bool,
}

impl MarkdownRenderer {
    fn new(source_id: SourceId) -> Self {
        Self {
            source_id,
            column: Column::new().spacing(2.0),
            style_stack: Vec::new(),
            list_stack: Vec::new(),
            blockquote_level: 0,
            code_block_content: String::new(),
            in_code_block: false,
            table_rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            in_table: false,
            heading_level: 0,
            inline_content: Vec::new(),
            in_list_item: false,
        }
    }

    fn render(&mut self, parser: Parser) {
        for event in parser {
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => self.text(&text),
                Event::Code(code) => self.inline_code(&code),
                Event::SoftBreak => self.soft_break(),
                Event::HardBreak => self.hard_break(),
                Event::Rule => self.horizontal_rule(),
                Event::TaskListMarker(checked) => self.task_list_marker(checked),
                _ => {}
            }
        }
    }

    fn finish(self) -> Column {
        self.column
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.heading_level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
            }
            Tag::BlockQuote(_) => {
                self.blockquote_level += 1;
            }
            Tag::CodeBlock(_) => {
                self.in_code_block = true;
                self.code_block_content.clear();
            }
            Tag::List(start) => {
                self.list_stack.push(ListInfo {
                    ordered: start.is_some(),
                    next_number: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                self.in_list_item = true;
                self.inline_content.clear();
            }
            Tag::Table(_) => {
                self.in_table = true;
                self.table_rows.clear();
            }
            Tag::TableHead => {}
            Tag::TableRow => {
                self.current_row.clear();
            }
            Tag::TableCell => {
                self.current_cell.clear();
            }
            Tag::Emphasis => {
                self.style_stack.push(Style::Italic);
            }
            Tag::Strong => {
                self.style_stack.push(Style::Bold);
            }
            Tag::Strikethrough => {
                self.style_stack.push(Style::Strikethrough);
            }
            Tag::Link { .. } => {
                self.style_stack.push(Style::Link);
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if self.in_list_item {
                    // Don't flush - content goes to list item
                } else if self.heading_level > 0 {
                    // Don't flush - will be handled by heading end
                } else {
                    self.flush_paragraph();
                }
            }
            TagEnd::Heading(_) => {
                self.flush_heading();
                self.heading_level = 0;
            }
            TagEnd::BlockQuote(_) => {
                self.blockquote_level = self.blockquote_level.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.flush_code_block();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item => {
                self.flush_list_item();
                self.in_list_item = false;
            }
            TagEnd::Table => {
                self.flush_table();
                self.in_table = false;
            }
            TagEnd::TableHead => {}
            TagEnd::TableRow => {
                self.table_rows.push(std::mem::take(&mut self.current_row));
            }
            TagEnd::TableCell => {
                self.current_row.push(std::mem::take(&mut self.current_cell));
            }
            TagEnd::Emphasis => {
                self.style_stack.retain(|s| *s != Style::Italic);
            }
            TagEnd::Strong => {
                self.style_stack.retain(|s| *s != Style::Bold);
            }
            TagEnd::Strikethrough => {
                self.style_stack.retain(|s| *s != Style::Strikethrough);
            }
            TagEnd::Link => {
                self.style_stack.retain(|s| *s != Style::Link);
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_block_content.push_str(text);
            return;
        }

        if self.in_table {
            self.current_cell.push_str(text);
            return;
        }

        // Create span with current styles
        let span = InlineSpan {
            text: text.to_string(),
            bold: self.style_stack.contains(&Style::Bold),
            italic: self.style_stack.contains(&Style::Italic),
            strikethrough: self.style_stack.contains(&Style::Strikethrough),
            is_code: false,
            is_link: self.style_stack.contains(&Style::Link),
        };

        self.inline_content.push(span);
    }

    fn inline_code(&mut self, code: &str) {
        if self.in_table {
            // Just add the code text without backticks for tables
            self.current_cell.push_str(code);
            return;
        }

        let span = InlineSpan {
            text: code.to_string(),
            bold: false,
            italic: false,
            strikethrough: false,
            is_code: true,
            is_link: false,
        };

        self.inline_content.push(span);
    }

    fn soft_break(&mut self) {
        if !self.in_code_block && !self.in_table {
            self.inline_content.push(InlineSpan {
                text: " ".to_string(),
                bold: false,
                italic: false,
                strikethrough: false,
                is_code: false,
                is_link: false,
            });
        }
    }

    fn hard_break(&mut self) {
        // For simplicity, treat as soft break
        self.soft_break();
    }

    fn horizontal_rule(&mut self) {
        self.column = std::mem::take(&mut self.column).push(
            Row::new()
                .padding_custom(Padding::new(4.0, 0.0, 4.0, 0.0))
                .push(Column::new().height(Length::Fixed(1.0)).width(Length::Fill).background(colors::TEXT_MUTED))
        );
    }

    fn task_list_marker(&mut self, checked: bool) {
        let marker_text = if checked { "\u{2713} " } else { "\u{25CB} " };
        let span = InlineSpan {
            text: marker_text.to_string(),
            bold: false,
            italic: false,
            strikethrough: false,
            is_code: false,
            is_link: checked, // Use link color for checkmark (green via SUCCESS)
        };
        self.inline_content.insert(0, span);
    }

    fn flush_paragraph(&mut self) {
        let content = std::mem::take(&mut self.inline_content);
        if content.is_empty() {
            return;
        }

        let row = self.build_inline_row(content);

        if self.blockquote_level > 0 {
            let bq_row = self.wrap_in_blockquote(row);
            self.column = std::mem::take(&mut self.column).push(bq_row);
        } else {
            self.column = std::mem::take(&mut self.column).push(row);
        }
    }

    fn flush_heading(&mut self) {
        let content = std::mem::take(&mut self.inline_content);
        if content.is_empty() {
            return;
        }

        // All headings are bold; larger headings get bigger sizes
        // Body text is 14.0, so H4-H6 stay at body size but use bold
        let size = match self.heading_level {
            1 => 18.0,
            2 => 16.0,
            3 => 15.0,
            _ => 14.0, // H4, H5, H6 same size as body but bold
        };

        // For headings, combine all content into one text element
        let text: String = content.iter().map(|s| s.text.as_str()).collect();

        let elem = TextElement::new(&text)
            .color(colors::TEXT_PRIMARY)
            .source(self.source_id)
            .size(size)
            .bold();

        self.column = std::mem::take(&mut self.column).push(elem);
    }

    fn flush_list_item(&mut self) {
        let content = std::mem::take(&mut self.inline_content);
        if content.is_empty() {
            return;
        }

        let indent_level = self.list_stack.len();

        // Build the marker
        let marker = if let Some(list_info) = self.list_stack.last_mut() {
            let indent = "  ".repeat(indent_level);
            if list_info.ordered {
                let num = list_info.next_number;
                list_info.next_number += 1;
                format!("{}{}. ", indent, num)
            } else {
                format!("{}\u{00B7} ", indent)
            }
        } else {
            "  \u{00B7} ".to_string()
        };

        // Build row with marker and content
        let mut row = Row::new().spacing(0.0);
        row = row.push(TextElement::new(&marker).color(colors::TEXT_MUTED).source(self.source_id));

        // Add content spans
        row = self.add_inline_spans_to_row(row, content);

        if self.blockquote_level > 0 {
            let bq_row = self.wrap_in_blockquote(row);
            self.column = std::mem::take(&mut self.column).push(bq_row);
        } else {
            self.column = std::mem::take(&mut self.column).push(row);
        }
    }

    /// Build a row from inline content spans
    fn build_inline_row(&self, content: Vec<InlineSpan>) -> Row {
        let row = Row::new().spacing(0.0);
        self.add_inline_spans_to_row(row, content)
    }

    /// Add inline spans to an existing row
    fn add_inline_spans_to_row(&self, mut row: Row, content: Vec<InlineSpan>) -> Row {
        for span in content {
            let mut elem = TextElement::new(&span.text).source(self.source_id);

            if span.is_code {
                elem = elem.color(colors::TOOL_ACTION);
            } else if span.is_link {
                elem = elem.color(colors::TEXT_PATH);
            } else if span.strikethrough {
                elem = elem.color(colors::TEXT_MUTED);
            } else {
                elem = elem.color(colors::TEXT_PRIMARY);
            }

            if span.bold {
                elem = elem.bold();
            }
            if span.italic {
                elem = elem.italic();
            }

            row = row.push(elem);
        }
        row
    }

    fn wrap_in_blockquote(&self, inner: Row) -> Row {
        let mut bq_row = Row::new();
        for _ in 0..self.blockquote_level {
            bq_row = bq_row
                .push(Column::new().width(Length::Fixed(3.0)).height(Length::Fixed(16.0)).background(colors::TEXT_MUTED))
                .fixed_spacer(8.0);
        }
        bq_row.push(inner)
    }

    fn flush_code_block(&mut self) {
        let content = std::mem::take(&mut self.code_block_content);
        if content.is_empty() {
            return;
        }

        let mut code_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(colors::CODE_BG)
            .corner_radius(4.0)
            .width(Length::Fill);

        // Split content by lines and add each as a text element
        for line in content.lines() {
            // Use non-breaking space for empty lines to preserve height
            let display_line = if line.is_empty() { " " } else { line };
            code_col = code_col.push(
                TextElement::new(display_line)
                    .color(colors::CODE_TEXT)
                    .source(self.source_id)
            );
        }

        self.column = std::mem::take(&mut self.column).push(code_col);
    }

    fn flush_table(&mut self) {
        let rows = std::mem::take(&mut self.table_rows);
        if rows.is_empty() {
            return;
        }

        let mut table = Column::new()
            .spacing(0.0)
            .padding(4.0)
            .background(colors::CODE_BG)
            .corner_radius(4.0);

        // Calculate column widths
        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut col_widths = vec![0usize; num_cols];
        for row in &rows {
            for (idx, cell) in row.iter().enumerate() {
                if idx < col_widths.len() {
                    col_widths[idx] = col_widths[idx].max(cell.chars().count());
                }
            }
        }

        let char_width = 8.4;
        let padding_h = 24.0;

        for (i, row_data) in rows.iter().enumerate() {
            let mut row_widget = Row::new().spacing(0.0);

            for (col_idx, cell) in row_data.iter().enumerate() {
                let col_width = col_widths.get(col_idx).copied().unwrap_or(10).max(5);
                let cell_width = (col_width as f32 * char_width) + padding_h;

                let text_elem = if i == 0 {
                    TextElement::new(cell)
                        .color(colors::TEXT_PRIMARY)
                        .source(self.source_id)
                        .bold()
                } else {
                    TextElement::new(cell)
                        .color(colors::TEXT_PRIMARY)
                        .source(self.source_id)
                };

                let cell_col = Column::new()
                    .width(Length::Fixed(cell_width))
                    .padding_custom(Padding::new(4.0, 12.0, 4.0, 12.0))
                    .border(colors::TOOL_BORDER, 1.0)
                    .push(text_elem);

                row_widget = row_widget.push(cell_col);
            }

            table = table.push(row_widget);
        }

        self.column = std::mem::take(&mut self.column).push(table);
    }
}
