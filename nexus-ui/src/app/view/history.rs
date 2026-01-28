//! History view - renders the block list or welcome screen.

use iced::widget::Column;
use iced::Element;

use nexus_api::BlockId;

use crate::agent_widgets::view_agent_block;
use crate::blocks::UnifiedBlockRef;
use crate::msg::{AgentMessage, Message};
use crate::state::Nexus;
use crate::ui::view_block;

use super::welcome;

/// Render the history content (blocks or welcome screen).
pub fn view(state: &Nexus) -> Element<'_, Message> {
    let font_size = state.window.font_size;

    // Collect unified blocks with their IDs for sorting
    let mut unified: Vec<(BlockId, UnifiedBlockRef)> =
        Vec::with_capacity(state.terminal.blocks.len() + state.agent.blocks.len());

    for block in &state.terminal.blocks {
        unified.push((block.id, UnifiedBlockRef::Shell(block)));
    }
    for block in &state.agent.blocks {
        unified.push((block.id, UnifiedBlockRef::Agent(block)));
    }

    // Sort by BlockId (ascending) for chronological order
    unified.sort_by_key(|(id, _)| id.0);

    // Render in order
    let content_elements: Vec<Element<Message>> = unified
        .into_iter()
        .map(|(_, block_ref)| match block_ref {
            UnifiedBlockRef::Shell(block) => view_block(block, font_size),
            UnifiedBlockRef::Agent(block) => view_agent_block(block, font_size)
                .map(|msg| Message::Agent(AgentMessage::Widget(msg))),
        })
        .collect();

    // Show welcome screen when empty, otherwise show command history
    if content_elements.is_empty() {
        welcome::view(font_size, &state.terminal.cwd)
    } else {
        Column::with_children(content_elements)
            .spacing(4)
            .padding([4, 8])
            .into()
    }
}
