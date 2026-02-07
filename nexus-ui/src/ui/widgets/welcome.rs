//! Welcome screen widget — shown when no blocks exist.

use strata::layout::{Column, LayoutChild, Length, Row, TextElement, Widget};

use crate::ui::theme;

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
            logo_col = logo_col.push(TextElement::new(line).color(theme::WELCOME_TITLE));
        }

        let left = Column::new()
            .spacing(4.0)
            .width(Length::Fill)
            .push(logo_col)
            .fixed_spacer(8.0)
            .push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new("Welcome to Nexus Shell").color(theme::WELCOME_TITLE).size(16.0))
                    .push(TextElement::new("v0.1.0").color(theme::TEXT_MUTED)),
            )
            .fixed_spacer(4.0)
            .push(TextElement::new(format!("  {}", display_cwd)).color(theme::TEXT_PATH));

        // Tips card
        let tips = Column::new()
            .padding(8.0)
            .spacing(2.0)
            .background(theme::CARD_BG)
            .corner_radius(4.0)
            .border(theme::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Getting Started").color(theme::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Type any command and press Enter").color(theme::TEXT_SECONDARY))
            .push(TextElement::new("\u{2022} Use Tab for completions").color(theme::TEXT_SECONDARY))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Click [SH] to switch to AI mode").color(theme::TEXT_PURPLE))
            .push(TextElement::new("\u{2022} Prefix with \"? \" for one-shot AI queries").color(theme::TEXT_PURPLE))
            .fixed_spacer(8.0)
            .push(TextElement::new("Try: ? what files are in this directory?").color(theme::TEXT_PURPLE));

        // Shortcuts card
        let shortcuts = Column::new()
            .padding(8.0)
            .spacing(2.0)
            .background(theme::CARD_BG)
            .corner_radius(4.0)
            .border(theme::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Shortcuts").color(theme::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("Cmd+K     Clear screen").color(theme::TEXT_SECONDARY))
            .push(TextElement::new("Cmd++/-   Zoom in/out").color(theme::TEXT_SECONDARY))
            .push(TextElement::new("Ctrl+R    Search history").color(theme::TEXT_SECONDARY))
            .push(TextElement::new("Up/Down   Navigate history").color(theme::TEXT_SECONDARY));

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
