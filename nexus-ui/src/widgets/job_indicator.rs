//! Visual job indicator widget - shows background/stopped jobs as clickable pills.
//!
//! This replaces the traditional text-based job control output with visual UI elements.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, container, row, text, Row};
use iced::{Background, Border, Color, Element, Length, Theme};

/// A job displayed in the status bar.
#[derive(Debug, Clone)]
pub struct VisualJob {
    pub id: u32,
    pub command: String,
    pub state: VisualJobState,
}

/// Visual state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualJobState {
    Running,
    Stopped,
}

impl VisualJob {
    pub fn new(id: u32, command: String, state: VisualJobState) -> Self {
        Self { id, command, state }
    }

    /// Get a shortened display name for the job.
    pub fn display_name(&self) -> String {
        // Truncate long commands
        if self.command.len() > 20 {
            format!("{}...", &self.command[..17])
        } else {
            self.command.clone()
        }
    }

    /// Get the icon for this job state.
    pub fn icon(&self) -> &'static str {
        match self.state {
            VisualJobState::Running => "●", // Solid circle for running
            VisualJobState::Stopped => "⏸", // Pause symbol for stopped
        }
    }

    /// Get the color for this job state.
    pub fn color(&self) -> Color {
        match self.state {
            VisualJobState::Running => Color::from_rgb(0.3, 0.8, 0.3), // Green
            VisualJobState::Stopped => Color::from_rgb(0.9, 0.7, 0.2), // Yellow/amber
        }
    }
}

/// Actions that can be performed on a job.
#[derive(Debug, Clone)]
pub enum JobAction {
    /// Bring job to foreground.
    Foreground(u32),
    /// Resume job in background.
    Background(u32),
    /// Kill the job.
    Kill(u32),
}

/// Create a visual job indicator bar.
///
/// Returns an element showing all jobs as clickable pills, or empty if no jobs.
pub fn job_indicator<'a, Message>(
    jobs: &'a [VisualJob],
    font_size: f32,
    on_click: impl Fn(u32) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    if jobs.is_empty() {
        return row![].into();
    }

    let pills: Vec<Element<Message>> = jobs
        .iter()
        .map(|job| {
            let job_id = job.id;
            let on_click = on_click.clone();

            job_pill(job, font_size, move || on_click(job_id))
        })
        .collect();

    Row::with_children(pills)
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
}

/// Create a single job pill widget.
fn job_pill<'a, Message>(
    job: &'a VisualJob,
    font_size: f32,
    on_click: impl Fn() -> Message + 'a,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    let icon_color = job.color();
    let pill_font_size = font_size * 0.85;
    let job_state = job.state; // Copy the state to avoid borrowing job

    let content = row![
        text(job.icon())
            .size(pill_font_size)
            .color(icon_color),
        text(job.display_name())
            .size(pill_font_size)
            .color(Color::from_rgb(0.8, 0.8, 0.8))
            .font(iced::Font::MONOSPACE),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    button(content)
        .on_press(on_click())
        .padding([4, 10])
        .style(move |theme, status| pill_style(theme, status, job_state))
        .into()
}

/// Style for job pill buttons.
fn pill_style(
    _theme: &Theme,
    status: button::Status,
    state: VisualJobState,
) -> button::Style {
    let base_bg = match state {
        VisualJobState::Running => Color::from_rgba(0.2, 0.4, 0.2, 0.6),
        VisualJobState::Stopped => Color::from_rgba(0.4, 0.35, 0.1, 0.6),
    };

    let bg = match status {
        button::Status::Hovered => Color {
            a: base_bg.a + 0.2,
            ..base_bg
        },
        button::Status::Pressed => Color {
            a: base_bg.a + 0.3,
            ..base_bg
        },
        _ => base_bg,
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::from_rgb(0.9, 0.9, 0.9),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: Color::from_rgba(0.5, 0.5, 0.5, 0.3),
        },
        ..Default::default()
    }
}

/// Create a job status bar (container with jobs on the right).
pub fn job_status_bar<'a, Message>(
    jobs: &'a [VisualJob],
    font_size: f32,
    on_click: impl Fn(u32) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    if jobs.is_empty() {
        return container(row![]).height(0).into();
    }

    let indicator = job_indicator(jobs, font_size, on_click);

    container(indicator)
        .width(Length::Fill)
        .padding([4, 15])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.1, 0.1, 0.12, 0.9))),
            border: Border {
                width: 0.0,
                ..Default::default()
            },
            ..Default::default()
        })
        .align_x(Horizontal::Right)
        .align_y(Vertical::Center)
        .into()
}
