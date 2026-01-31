//! Source ID helpers — single source of truth for all named SourceIds.

use nexus_api::BlockId;
use strata::content_address::SourceId;

pub fn shell_header(id: BlockId) -> SourceId { SourceId::named(&format!("shell_header_{}", id.0)) }
pub fn shell_term(id: BlockId) -> SourceId { SourceId::named(&format!("shell_term_{}", id.0)) }
pub fn native(id: BlockId) -> SourceId { SourceId::named(&format!("native_{}", id.0)) }
pub fn table(id: BlockId) -> SourceId { SourceId::named(&format!("table_{}", id.0)) }
pub fn table_sort(id: BlockId, col: usize) -> SourceId { SourceId::named(&format!("sort_{}_{}", id.0, col)) }
pub fn kill(id: BlockId) -> SourceId { SourceId::named(&format!("kill_{}", id.0)) }

pub fn agent_query(id: BlockId) -> SourceId { SourceId::named(&format!("agent_query_{}", id.0)) }
pub fn agent_thinking(id: BlockId) -> SourceId { SourceId::named(&format!("agent_thinking_{}", id.0)) }
pub fn agent_response(id: BlockId) -> SourceId { SourceId::named(&format!("agent_response_{}", id.0)) }
pub fn agent_thinking_toggle(id: BlockId) -> SourceId { SourceId::named(&format!("thinking_{}", id.0)) }
pub fn agent_stop(id: BlockId) -> SourceId { SourceId::named(&format!("stop_{}", id.0)) }
pub fn agent_tool_toggle(id: BlockId, i: usize) -> SourceId { SourceId::named(&format!("tool_toggle_{}_{}", id.0, i)) }
pub fn agent_perm_deny(id: BlockId) -> SourceId { SourceId::named(&format!("perm_deny_{}", id.0)) }
pub fn agent_perm_allow(id: BlockId) -> SourceId { SourceId::named(&format!("perm_allow_{}", id.0)) }
pub fn agent_perm_always(id: BlockId) -> SourceId { SourceId::named(&format!("perm_always_{}", id.0)) }

// Global UI
pub fn mode_toggle() -> SourceId { SourceId::named("mode_toggle") }
pub fn remove_attachment(i: usize) -> SourceId { SourceId::named(&format!("remove_attach_{}", i)) }
pub fn ctx_menu_item(i: usize) -> SourceId { SourceId::named(&format!("ctx_menu_{}", i)) }
