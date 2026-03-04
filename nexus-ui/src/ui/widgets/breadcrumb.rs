//! Breadcrumb bar widget — shows the connection stack when remote.
//!
//! ```text
//! local > devbox > webapp > db-server    23ms
//! ```
//!
//! Each segment is clickable (sends Unnest chain to pop back to that level).
//! Shows a connection health indicator (color dot) and RTT.

use strata::layout::{LayoutChild, Length, Padding, Row, TextElement, Widget};
use strata::primitives::Color;

use crate::features::shell::remote::{BackendEntry, ConnectionState};
use crate::utils::ids as source_ids;
use nexus_protocol::messages::EnvInfo;

/// Data needed to render the breadcrumb bar.
pub(crate) struct BreadcrumbBar<'a> {
    /// The local hostname.
    pub local_host: &'a str,
    /// The remote connection stack.
    pub stack: &'a [BackendEntry],
    /// The current (active) remote environment, shown as the final segment.
    pub current_env: Option<&'a EnvInfo>,
    /// Current connection state.
    pub state: ConnectionState,
    /// Current round-trip time in milliseconds (if available).
    pub rtt_ms: Option<u64>,
    /// If set, segments beyond this depth are greyed out (unnesting in progress).
    pub unnesting_to: Option<usize>,
    /// Whether the disconnect confirmation is active (first click happened).
    pub confirm_active: bool,
}

impl<'a> Widget<'a> for BreadcrumbBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        let mut row = Row::new().spacing(4.0);

        // Connection health dot
        let dot_color = match self.state {
            ConnectionState::Connected => Color::rgb(0.3, 0.85, 0.3), // Green
            ConnectionState::Reconnecting => Color::rgb(0.9, 0.5, 0.2), // Orange
            ConnectionState::Disconnected => Color::rgb(0.8, 0.2, 0.2), // Red
        };
        row = row.push(
            TextElement::new("\u{25CF}".to_string())
                .color(dot_color)
                .size(10.0),
        );

        // "local" segment — clickable (depth 0 = disconnect entirely)
        let local_alpha = self.segment_alpha(0);
        let (local_label, local_color) = if self.confirm_active {
            ("disconnect?".to_string(), Color::rgba(0.9, 0.5, 0.2, 0.9))
        } else {
            (self.local_host.to_string(), Color::rgba(0.7, 0.8, 1.0, local_alpha))
        };
        row = row.push(
            Row::new()
                .id(source_ids::breadcrumb_segment(0))
                .push(
                    TextElement::new(local_label)
                        .color(local_color)
                        .size(12.0),
                ),
        );

        // Remote segments (from backend_stack — previously visited hops)
        for (i, entry) in self.stack.iter().enumerate() {
            let depth = i + 1;
            let alpha = self.segment_alpha(depth);

            // Separator
            row = row.push(
                TextElement::new("\u{203A}".to_string()) // ›
                    .color(Color::rgba(0.5, 0.5, 0.5, alpha * 0.8))
                    .size(12.0),
            );

            // Hostname segment — clickable
            let label = format!("{}@{}", entry.env.user, entry.env.hostname);
            row = row.push(
                Row::new()
                    .id(source_ids::breadcrumb_segment(depth))
                    .push(
                        TextElement::new(label)
                            .color(Color::rgba(0.7, 0.8, 1.0, alpha))
                            .size(12.0),
                    ),
            );
        }

        // Current (active) remote environment — final segment
        if let Some(env) = self.current_env {
            let depth = self.stack.len() + 1;

            // Separator
            row = row.push(
                TextElement::new("\u{203A}".to_string()) // ›
                    .color(Color::rgba(0.5, 0.5, 0.5, 0.8))
                    .size(12.0),
            );

            // Active segment — full brightness
            let label = format!("{}@{}", env.user, env.hostname);
            row = row.push(
                Row::new()
                    .id(source_ids::breadcrumb_segment(depth))
                    .push(
                        TextElement::new(label)
                            .color(Color::rgba(0.7, 0.8, 1.0, 1.0))
                            .size(12.0),
                    ),
            );
        }

        // RTT display (right-aligned)
        if let Some(rtt) = self.rtt_ms {
            row = row.push(Row::new().spacer(1.0)); // Push RTT to the right
            let rtt_color = if rtt < 50 {
                Color::rgba(0.5, 0.7, 0.5, 0.7) // Low latency — green-ish
            } else if rtt < 200 {
                Color::rgba(0.7, 0.7, 0.4, 0.7) // Medium
            } else {
                Color::rgba(0.7, 0.4, 0.4, 0.7) // High latency — red-ish
            };
            row = row.push(
                TextElement::new(format!("{}ms", rtt))
                    .color(rtt_color)
                    .size(11.0),
            );
        }

        Row::new()
            .padding_custom(Padding::new(2.0, 8.0, 2.0, 8.0))
            .width(Length::Fill)
            .background(Color::rgba(0.15, 0.15, 0.2, 0.8))
            .corner_radius(4.0)
            .push(row)
            .into()
    }
}

impl<'a> BreadcrumbBar<'a> {
    /// Compute alpha for a segment. Segments beyond `unnesting_to` are greyed out.
    fn segment_alpha(&self, depth: usize) -> f32 {
        match self.unnesting_to {
            Some(target) if depth > target => 0.3,
            _ => 0.9,
        }
    }
}
