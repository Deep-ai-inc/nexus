//! Claude Code process management using the claude-codes crate.
//!
//! Provides a typed interface for communicating with Claude CLI.

use std::path::PathBuf;

use claude_codes::{AsyncClient, ClaudeInput, ClaudeOutput, ControlRequestPayload};
use tokio::sync::mpsc;

/// Error types for Claude process operations.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Failed to create Claude client: {0}")]
    ClientError(String),

    #[error("Failed to send message: {0}")]
    SendError(String),

    #[error("Stream error: {0}")]
    StreamError(String),
}

/// Message type for the async event reader.
#[derive(Debug)]
pub enum ReaderMessage {
    /// A Claude output message.
    Output(ClaudeOutput),
    /// Error from the stream.
    Error(String),
    /// Stream closed.
    Closed,
}

/// Handle for a Claude Code session.
pub struct ClaudeSession {
    /// The async client.
    client: AsyncClient,
    /// Working directory.
    cwd: PathBuf,
    /// Channel sender for outputs.
    tx: mpsc::UnboundedSender<ReaderMessage>,
}

impl ClaudeSession {
    /// Create a new Claude session.
    pub async fn new(cwd: PathBuf) -> Result<(Self, mpsc::UnboundedReceiver<ReaderMessage>), ProcessError> {
        let client = AsyncClient::with_defaults()
            .await
            .map_err(|e| ProcessError::ClientError(e.to_string()))?;

        let (tx, rx) = mpsc::unbounded_channel();

        Ok((
            Self { client, cwd, tx },
            rx,
        ))
    }

    /// Send a user message and stream the response.
    ///
    /// This method processes responses and handles permission requests automatically.
    pub async fn send_message(&mut self, content: &str) -> Result<(), ProcessError> {
        tracing::info!("Sending message to Claude: {}", content);

        // Send the user message
        let input = ClaudeInput::user_message(content, uuid::Uuid::new_v4());
        self.client
            .send(&input)
            .await
            .map_err(|e| ProcessError::SendError(e.to_string()))?;

        // Receive responses one at a time (avoids borrow conflicts with stream)
        loop {
            let output = match self.client.receive().await {
                Ok(o) => o,
                Err(e) => {
                    // Connection closed is expected at end
                    tracing::debug!("Receive ended: {}", e);
                    break;
                }
            };

            tracing::debug!("Claude output: {:?}", output.message_type());
            let is_result = matches!(output, ClaudeOutput::Result(_));

            // Check for permission requests and auto-approve
            if let ClaudeOutput::ControlRequest(ref ctrl_req) = output {
                if let ControlRequestPayload::CanUseTool(ref tool_req) = ctrl_req.request {
                    tracing::info!(
                        "Auto-approving tool permission: {} (request_id: {})",
                        tool_req.tool_name,
                        ctrl_req.request_id
                    );

                    // Create approval response
                    let response = tool_req.allow(&ctrl_req.request_id);
                    let response_input = ClaudeInput::ControlResponse(response);

                    // Send approval back to Claude
                    if let Err(e) = self.client.send(&response_input).await {
                        tracing::error!("Failed to send permission response: {}", e);
                        let _ = self.tx.send(ReaderMessage::Error(e.to_string()));
                    }
                }
            }

            // Forward output to UI
            if self.tx.send(ReaderMessage::Output(output)).is_err() {
                break;
            }

            // Result message means the turn is complete
            if is_result {
                break;
            }
        }

        let _ = self.tx.send(ReaderMessage::Closed);
        Ok(())
    }

    /// Get the working directory.
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }
}

/// Buffer streaming deltas before sending to UI.
///
/// Prevents high-frequency re-renders by batching tiny text chunks.
#[derive(Debug, Default)]
pub struct BlockAccumulator {
    /// Accumulated text content.
    current_text: String,

    /// Accumulated thinking content.
    current_thinking: String,

    /// Pending tool input (JSON may arrive incrementally).
    pending_tool_input: String,

    /// Current tool ID being accumulated.
    current_tool_id: Option<String>,

    /// Character threshold before flushing to UI.
    flush_threshold: usize,
}

impl BlockAccumulator {
    /// Create a new accumulator with the given flush threshold.
    pub fn new(flush_threshold: usize) -> Self {
        Self {
            flush_threshold,
            ..Default::default()
        }
    }

    /// Push a text delta. Returns Some if threshold reached.
    pub fn push_text_delta(&mut self, delta: &str) -> Option<String> {
        self.current_text.push_str(delta);
        if self.current_text.len() >= self.flush_threshold {
            Some(std::mem::take(&mut self.current_text))
        } else {
            None
        }
    }

    /// Push a thinking delta. Returns Some if threshold reached.
    pub fn push_thinking_delta(&mut self, delta: &str) -> Option<String> {
        self.current_thinking.push_str(delta);
        if self.current_thinking.len() >= self.flush_threshold {
            Some(std::mem::take(&mut self.current_thinking))
        } else {
            None
        }
    }

    /// Flush remaining text content.
    pub fn flush_text(&mut self) -> Option<String> {
        if self.current_text.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.current_text))
        }
    }

    /// Flush remaining thinking content.
    pub fn flush_thinking(&mut self) -> Option<String> {
        if self.current_thinking.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.current_thinking))
        }
    }

    /// Start accumulating tool input for a tool.
    pub fn start_tool_input(&mut self, tool_id: String) {
        self.pending_tool_input.clear();
        self.current_tool_id = Some(tool_id);
    }

    /// Push partial tool input JSON.
    pub fn push_tool_input(&mut self, partial: &str) {
        self.pending_tool_input.push_str(partial);
    }

    /// Try to parse accumulated tool input as JSON.
    pub fn try_parse_tool_input(&self) -> Option<(String, serde_json::Value)> {
        let tool_id = self.current_tool_id.as_ref()?;
        let parsed: serde_json::Value = serde_json::from_str(&self.pending_tool_input).ok()?;
        Some((tool_id.clone(), parsed))
    }

    /// Reset tool input accumulator.
    pub fn reset_tool_input(&mut self) {
        self.pending_tool_input.clear();
        self.current_tool_id = None;
    }

    /// Reset all accumulated content.
    pub fn reset(&mut self) {
        self.current_text.clear();
        self.current_thinking.clear();
        self.pending_tool_input.clear();
        self.current_tool_id = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_accumulator_text() {
        let mut acc = BlockAccumulator::new(10);

        // Small chunks don't trigger flush
        assert!(acc.push_text_delta("abc").is_none());
        assert!(acc.push_text_delta("def").is_none());

        // Threshold reached triggers flush
        let flushed = acc.push_text_delta("ghij");
        assert!(flushed.is_some());
        assert_eq!(flushed.unwrap(), "abcdefghij");

        // Buffer is now empty
        assert!(acc.flush_text().is_none());
    }

    #[test]
    fn test_block_accumulator_thinking() {
        let mut acc = BlockAccumulator::new(5);

        acc.push_thinking_delta("abc");
        let flushed = acc.push_thinking_delta("de");
        assert!(flushed.is_some());
        assert_eq!(flushed.unwrap(), "abcde");
    }

    #[test]
    fn test_block_accumulator_flush() {
        let mut acc = BlockAccumulator::new(100);

        acc.push_text_delta("partial");
        let flushed = acc.flush_text();
        assert_eq!(flushed, Some("partial".to_string()));

        // Second flush returns None
        assert!(acc.flush_text().is_none());
    }

    #[test]
    fn test_block_accumulator_tool_input() {
        let mut acc = BlockAccumulator::new(10);

        acc.start_tool_input("tool_123".to_string());
        acc.push_tool_input(r#"{"path":"#);
        assert!(acc.try_parse_tool_input().is_none()); // Incomplete

        acc.push_tool_input(r#""/foo"}"#);
        let (tool_id, value) = acc.try_parse_tool_input().unwrap();
        assert_eq!(tool_id, "tool_123");
        assert_eq!(value["path"], "/foo");
    }
}
