//! Job bar widget — shows background job pills.

use strata::content_address::SourceId;
use strata::layout::{LayoutChild, Length, Padding, Row, TextElement, Widget};
use strata::primitives::Color;

use crate::data::{VisualJob, VisualJobState};
use crate::utils::ids;

// =========================================================================
// Job Bar — shows background job pills
// =========================================================================

pub struct JobBar<'a> {
    pub jobs: &'a [VisualJob],
}

impl JobBar<'_> {
    /// Generate a stable SourceId for clicking a job pill.
    /// Uses zero-allocation IdSpace pattern.
    pub fn job_pill_id(job_id: u32) -> SourceId {
        ids::job_pill(job_id)
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
