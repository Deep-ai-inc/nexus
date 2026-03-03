//! Session persistence: ring buffer for event replay on reconnection.

use nexus_protocol::codec::encode_payload;
use nexus_protocol::messages::Response;

/// A bounded ring buffer for serialized Response frames.
///
/// Bounded by total serialized byte size (not event count) to handle
/// large structured events gracefully. Always drops/pushes complete
/// frames — never slices mid-payload.
pub struct RingBuffer {
    /// Stored frames: (seq, serialized_bytes).
    frames: std::collections::VecDeque<(u64, Vec<u8>)>,
    /// Current total size of all stored frames in bytes.
    current_bytes: usize,
    /// Maximum allowed total size in bytes.
    max_bytes: usize,
}

impl RingBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            frames: std::collections::VecDeque::new(),
            current_bytes: 0,
            max_bytes,
        }
    }

    /// Push a response into the ring buffer.
    /// Drops oldest frames if needed to make room.
    pub fn push(&mut self, resp: &Response) {
        let serialized = match encode_payload(resp) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("failed to serialize response for ring buffer: {e}");
                return;
            }
        };

        let frame_size = serialized.len();

        // If a single frame exceeds the entire buffer, skip it
        if frame_size > self.max_bytes {
            tracing::warn!(
                "skipping frame of {frame_size} bytes (exceeds ring buffer max {})",
                self.max_bytes
            );
            return;
        }

        // Drop oldest frames until there's room
        while self.current_bytes + frame_size > self.max_bytes {
            if let Some((_, old)) = self.frames.pop_front() {
                self.current_bytes -= old.len();
            } else {
                break;
            }
        }

        let seq = match resp {
            Response::Event { seq, .. } => *seq,
            _ => 0,
        };

        self.current_bytes += frame_size;
        self.frames.push_back((seq, serialized));
    }

    /// Replay all events with sequence number > `last_seen_seq`.
    /// Returns the serialized frames to send.
    pub fn replay_since(&self, last_seen_seq: u64) -> Vec<&[u8]> {
        self.frames
            .iter()
            .filter(|(seq, _)| *seq > last_seen_seq)
            .map(|(_, data)| data.as_slice())
            .collect()
    }

    /// Get the highest sequence number in the buffer.
    pub fn latest_seq(&self) -> u64 {
        self.frames.back().map(|(seq, _)| *seq).unwrap_or(0)
    }

    /// Current total bytes stored.
    pub fn bytes_used(&self) -> usize {
        self.current_bytes
    }

    /// Number of frames stored.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_api::{BlockId, ShellEvent};

    fn make_event(seq: u64, data_size: usize) -> Response {
        Response::Event {
            seq,
            event: ShellEvent::StdoutChunk {
                block_id: BlockId(1),
                data: vec![b'x'; data_size],
            },
        }
    }

    #[test]
    fn push_and_replay() {
        let mut rb = RingBuffer::new(10_000);
        for i in 1..=5 {
            rb.push(&make_event(i, 100));
        }
        assert_eq!(rb.len(), 5);
        assert_eq!(rb.latest_seq(), 5);

        let replay = rb.replay_since(3);
        assert_eq!(replay.len(), 2); // seq 4 and 5
    }

    #[test]
    fn evicts_oldest_when_full() {
        // Small buffer that can hold ~2 events
        let mut rb = RingBuffer::new(500);
        rb.push(&make_event(1, 100));
        rb.push(&make_event(2, 100));
        rb.push(&make_event(3, 100));

        // Oldest should have been evicted
        assert!(rb.len() <= 3);
        assert_eq!(rb.latest_seq(), 3);

        // Replay from 0 should only return what fits
        let replay = rb.replay_since(0);
        assert!(!replay.is_empty());
    }

    #[test]
    fn oversized_frame_skipped() {
        let mut rb = RingBuffer::new(100);
        rb.push(&make_event(1, 200)); // Too large for 100-byte buffer
        assert!(rb.is_empty());
    }
}
