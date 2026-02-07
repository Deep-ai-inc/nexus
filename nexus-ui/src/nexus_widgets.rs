//! Nexus Widget Structs for Strata
//!
//! Production UI components that render real Nexus data (Block, AgentBlock, etc.)
//! using Strata's layout primitives. Each widget takes references to backend
//! data models and builds a layout tree.

use nexus_kernel::{Completion, CompletionKind};

use strata::content_address::SourceId;
use strata::layout::{
    ButtonElement, Column, CrossAxisAlignment, LayoutChild, Length, Padding, Row,
    ScrollColumn, TextElement, Widget,
};
use strata::primitives::Color;
use strata::scroll_state::ScrollState;
use crate::blocks::{VisualJob, VisualJobState};

use crate::nexus_app::colors;

// Widget re-exports (moved to widgets/ module)
pub use crate::widgets::ShellBlockWidget;
pub use crate::widgets::{AgentBlockWidget, AgentBlockMessage};

// =========================================================================
// Welcome Screen — shown when no blocks exist
// =========================================================================

pub struct WelcomeScreen<'a> {
    pub cwd: &'a str,
}

impl<'a> Widget<'a> for WelcomeScreen<'a> {
    fn build(self) -> LayoutChild<'a> {
        // Shorten home directory
        let home = std::env::var("HOME").unwrap_or_default();
        let display_cwd = if self.cwd.starts_with(&home) {
            self.cwd.replacen(&home, "~", 1)
        } else {
            self.cwd.to_string()
        };

        let logo = r#" ███╗   ██╗███████╗██╗  ██╗██╗   ██╗███████╗
 ████╗  ██║██╔════╝╚██╗██╔╝██║   ██║██╔════╝
 ██╔██╗ ██║█████╗   ╚███╔╝ ██║   ██║███████╗
 ██║╚██╗██║██╔══╝   ██╔██╗ ██║   ██║╚════██║
 ██║ ╚████║███████╗██╔╝ ██╗╚██████╔╝███████║
 ╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝"#;

        // Left column: logo + welcome
        let mut logo_col = Column::new().spacing(0.0);
        for line in logo.lines() {
            logo_col = logo_col.push(TextElement::new(line).color(colors::WELCOME_TITLE));
        }

        let left = Column::new()
            .spacing(4.0)
            .width(Length::Fill)
            .push(logo_col)
            .fixed_spacer(8.0)
            .push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new("Welcome to Nexus Shell").color(colors::WELCOME_TITLE).size(16.0))
                    .push(TextElement::new("v0.1.0").color(colors::TEXT_MUTED)),
            )
            .fixed_spacer(4.0)
            .push(TextElement::new(format!("  {}", display_cwd)).color(colors::TEXT_PATH));

        // Tips card
        let tips = Column::new()
            .padding(8.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(4.0)
            .border(colors::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Getting Started").color(colors::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Type any command and press Enter").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("\u{2022} Use Tab for completions").color(colors::TEXT_SECONDARY))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Click [SH] to switch to AI mode").color(colors::TEXT_PURPLE))
            .push(TextElement::new("\u{2022} Prefix with \"? \" for one-shot AI queries").color(colors::TEXT_PURPLE))
            .fixed_spacer(8.0)
            .push(TextElement::new("Try: ? what files are in this directory?").color(colors::TEXT_PURPLE));

        // Shortcuts card
        let shortcuts = Column::new()
            .padding(8.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(4.0)
            .border(colors::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Shortcuts").color(colors::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("Cmd+K     Clear screen").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Cmd++/-   Zoom in/out").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Ctrl+R    Search history").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Up/Down   Navigate history").color(colors::TEXT_SECONDARY));

        // Right column: tips + shortcuts
        let right = Column::new()
            .spacing(12.0)
            .width(Length::Fill)
            .push(tips)
            .push(shortcuts);

        Row::new()
            .padding(12.0)
            .spacing(20.0)
            .width(Length::Fill)
            .push(left)
            .push(right)
            .into()
    }
}

// =========================================================================
// Job Bar — shows background job pills
// =========================================================================

pub struct JobBar<'a> {
    pub jobs: &'a [VisualJob],
}

impl JobBar<'_> {
    pub fn job_pill_id(job_id: u32) -> SourceId {
        SourceId::named(&format!("job_{}", job_id))
    }
}

impl<'a> Widget<'a> for JobBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        let mut row = Row::new().spacing(8.0);

        for job in self.jobs {
            let (icon, color, bg) = match job.state {
                VisualJobState::Running => ("\u{25CF}", Color::rgb(0.3, 0.8, 0.3), Color::rgba(0.2, 0.4, 0.2, 0.6)),
                VisualJobState::Stopped => ("\u{23F8}", Color::rgb(0.9, 0.7, 0.2), Color::rgba(0.4, 0.35, 0.1, 0.6)),
            };
            let name = job.display_name();
            let click_id = Self::job_pill_id(job.id);
            row = row.push(
                Row::new()
                    .id(click_id)
                    .padding_custom(Padding::new(2.0, 6.0, 2.0, 6.0))
                    .background(bg)
                    .corner_radius(12.0)
                    .border(Color::rgba(0.5, 0.5, 0.5, 0.3), 1.0)
                    .push(TextElement::new(format!("{} {}", icon, name)).color(color)),
            );
        }

        Row::new()
            .padding_custom(Padding::new(2.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(Row::new().spacer(1.0).push(row))
            .into()
    }
}

// =========================================================================
// Input Bar — mode toggle + path + prompt + text input
// =========================================================================

pub struct NexusInputBar<'a> {
    pub input: &'a strata::TextInputState,
    pub mode: crate::blocks::InputMode,
    pub cwd: &'a str,
    pub last_exit_code: Option<i32>,
    pub cursor_visible: bool,
    pub mode_toggle_id: SourceId,
    pub line_count: usize,
}

impl<'a> Widget<'a> for NexusInputBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        use crate::blocks::InputMode;
        use strata::TextInputElement;

        // Mode button
        let (mode_label, mode_color, mode_bg, prompt_char) = match self.mode {
            InputMode::Shell => ("SH", Color::rgb(0.5, 0.9, 0.5), Color::rgb(0.2, 0.3, 0.2), "$"),
            InputMode::Agent => ("AI", Color::rgb(0.7, 0.7, 1.0), Color::rgb(0.25, 0.25, 0.4), "?"),
        };

        let mode_btn = ButtonElement::new(self.mode_toggle_id, mode_label)
            .background(mode_bg)
            .text_color(mode_color)
            .corner_radius(4.0);

        // Shorten cwd for display
        let home = std::env::var("HOME").unwrap_or_default();
        let display_cwd = if self.cwd.starts_with(&home) {
            self.cwd.replacen(&home, "~", 1)
        } else {
            self.cwd.to_string()
        };

        // Prompt color based on exit code (rgb8 values from input.rs)
        let prompt_color = match self.last_exit_code {
            // rgb8(50, 205, 50) = lime green
            Some(0) | None => Color::rgb(0.196, 0.804, 0.196),
            // rgb8(220, 50, 50) = bright red
            Some(_) => Color::rgb(0.863, 0.196, 0.196),
        };

        Row::new()
            .padding_custom(Padding::new(4.0, 6.0, 4.0, 6.0))
            .spacing(6.0)
            .background(colors::BG_INPUT)
            .corner_radius(6.0)
            .border(colors::BORDER_INPUT, 1.0)
            .width(Length::Fill)
            .cross_align(CrossAxisAlignment::Center)
            .push(mode_btn)
            .push(TextElement::new(display_cwd).color(colors::TEXT_PATH))
            .push(TextElement::new(prompt_char).color(prompt_color))
            .push({
                let mut elem = TextInputElement::from_state(self.input)
                    .placeholder("Type a command...")
                    .background(Color::TRANSPARENT)
                    .border_color(Color::TRANSPARENT)
                    .focus_border_color(Color::TRANSPARENT)
                    .corner_radius(0.0)
                    .padding(Padding::new(0.0, 4.0, 0.0, 4.0))
                    .width(Length::Fill)
                    .cursor_visible(self.cursor_visible);
                if self.line_count > 1 {
                    let line_height = 18.0_f32;
                    let input_height = self.line_count as f32 * line_height + 4.0;
                    elem = elem.multiline(true).height(Length::Fixed(input_height));
                }
                elem
            })
            .into()
    }
}

// =========================================================================
// Completion Popup — shows tab completion results
// =========================================================================

pub struct CompletionPopup<'a> {
    pub completions: &'a [Completion],
    pub selected_index: Option<usize>,
    pub hovered_index: Option<usize>,
    pub scroll: &'a ScrollState,
}

impl CompletionPopup<'_> {
    /// Generate a stable SourceId for clicking a completion item.
    pub fn item_id(index: usize) -> SourceId {
        SourceId::named(&format!("comp_item_{}", index))
    }
}

impl<'a> Widget<'a> for CompletionPopup<'a> {
    fn build(self) -> LayoutChild<'a> {
        // Scrollable list of completions, max 300px tall
        let mut scroll = ScrollColumn::from_state(self.scroll)
            .spacing(0.0)
            .width(Length::Fixed(300.0))
            .height(Length::Fixed(300.0_f32.min(self.completions.len() as f32 * 26.0 + 8.0)))
            .background(Color::rgb(0.12, 0.12, 0.15))
            .corner_radius(4.0)
            .border(Color::rgb(0.3, 0.3, 0.35), 1.0);

        for (i, comp) in self.completions.iter().enumerate() {
            let is_selected = self.selected_index == Some(i);
            let is_hovered = self.hovered_index == Some(i) && !is_selected;

            // Icon from CompletionKind (matches kernel's icon() method)
            let icon = comp.kind.icon();

            // Icon colors matched from old UI input.rs completion_icon_color
            let icon_color = match comp.kind {
                CompletionKind::Directory => Color::rgb(0.4, 0.7, 1.0),
                CompletionKind::Executable | CompletionKind::NativeCommand => Color::rgb(0.4, 0.9, 0.4),
                CompletionKind::Builtin => Color::rgb(1.0, 0.8, 0.4),
                CompletionKind::Function => Color::rgb(0.8, 0.6, 1.0),
                CompletionKind::Variable => Color::rgb(1.0, 0.6, 0.6),
                _ => Color::rgb(0.7, 0.7, 0.7),
            };

            let text_color = if is_selected { Color::WHITE } else { Color::rgb(0.8, 0.8, 0.8) };
            let bg = if is_selected {
                Color::rgb(0.2, 0.4, 0.6)
            } else if is_hovered {
                Color::rgb(0.22, 0.22, 0.28)
            } else {
                Color::rgb(0.15, 0.15, 0.18)
            };

            let click_id = Self::item_id(i);
            scroll = scroll.push(
                Row::new()
                    .id(click_id)
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .spacing(4.0)
                    .background(bg)
                    .corner_radius(3.0)
                    .cross_align(CrossAxisAlignment::Center)
                    .push(TextElement::new(format!("{} ", icon)).color(icon_color))
                    .push(TextElement::new(&comp.display).color(text_color)),
            );
        }

        Column::new()
            .padding_custom(Padding::new(0.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(scroll)
            .into()
    }
}

// =========================================================================
// History Search Bar — Ctrl+R reverse-i-search
// =========================================================================

pub struct HistorySearchBar<'a> {
    pub query: &'a str,
    pub results: &'a [String],
    pub result_index: usize,
    pub hovered_index: Option<usize>,
    pub scroll: &'a ScrollState,
}

impl HistorySearchBar<'_> {
    /// Generate a stable SourceId for clicking a history result item.
    pub fn result_id(index: usize) -> SourceId {
        SourceId::named(&format!("hist_result_{}", index))
    }
}

impl<'a> Widget<'a> for HistorySearchBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        // History search overlay matched from old UI input.rs
        let mut container = Column::new()
            .padding(10.0)
            .spacing(6.0)
            .background(Color::rgb(0.1, 0.1, 0.12))
            .corner_radius(6.0)
            .border(Color::rgb(0.3, 0.5, 0.7), 1.0)
            .width(Length::Fill);

        // Search header: label + query display
        let header = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center)
            .push(TextElement::new("(reverse-i-search)").color(Color::rgb(0.6, 0.6, 0.6)))
            .push(
                // Styled query input area
                Row::new()
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .background(Color::rgb(0.15, 0.15, 0.18))
                    .corner_radius(4.0)
                    .border(Color::rgb(0.4, 0.6, 0.8), 1.0)
                    .width(Length::Fill)
                    .push(if self.query.is_empty() {
                        TextElement::new("Type to search...").color(Color::rgb(0.4, 0.4, 0.4))
                    } else {
                        TextElement::new(self.query).color(Color::rgb(0.9, 0.9, 0.9))
                    }),
            );

        container = container.push(header);

        // Scrollable results list, max 300px tall
        if !self.results.is_empty() {
            let row_height = 30.0_f32;
            let max_height = 300.0_f32.min(self.results.len() as f32 * row_height + 4.0);

            let mut scroll = ScrollColumn::from_state(self.scroll)
                .spacing(0.0)
                .width(Length::Fill)
                .height(Length::Fixed(max_height));

            for (i, result) in self.results.iter().enumerate() {
                let is_selected = i == self.result_index;
                let is_hovered = self.hovered_index == Some(i) && !is_selected;
                let text_color = if is_selected { Color::WHITE } else { Color::rgb(0.8, 0.8, 0.8) };
                let bg = if is_selected {
                    Color::rgb(0.2, 0.4, 0.6)
                } else if is_hovered {
                    Color::rgb(0.20, 0.20, 0.25)
                } else {
                    Color::rgb(0.12, 0.12, 0.15)
                };

                let click_id = Self::result_id(i);
                scroll = scroll.push(
                    Row::new()
                        .id(click_id)
                        .padding_custom(Padding::new(6.0, 10.0, 6.0, 10.0))
                        .background(bg)
                        .corner_radius(3.0)
                        .width(Length::Fill)
                        .push(TextElement::new(result).color(text_color)),
                );
            }

            container = container.push(scroll);
        } else if !self.query.is_empty() {
            container = container.push(
                Row::new()
                    .padding_custom(Padding::new(4.0, 10.0, 4.0, 10.0))
                    .push(TextElement::new("No matches found").color(colors::TEXT_MUTED)),
            );
        }

        // Status line
        if !self.results.is_empty() {
            let status = format!("{}/{}", self.result_index + 1, self.results.len());
            container = container.push(
                Row::new()
                    .push(TextElement::new("Esc to close, Enter to select, Ctrl+R for next").color(colors::TEXT_MUTED))
                    .spacer(1.0)
                    .push(TextElement::new(status).color(colors::TEXT_MUTED)),
            );
        }

        Column::new()
            .padding_custom(Padding::new(0.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(container)
            .into()
    }
}

// Value rendering functions have been moved to widgets/value_renderer.rs
pub(crate) use crate::widgets::{render_native_value, term_color_to_strata};

// Tests for format_tokens are in widgets/agent_block.rs
// Tests for format_eta are in widgets/value_renderer.rs
