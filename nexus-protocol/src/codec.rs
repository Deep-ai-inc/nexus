//! Length-prefixed binary frame codec for async byte streams.
//!
//! Frame format:
//! ```text
//! [len: u32 LE] [priority: u8] [flags: u8] [payload: rmp-serde encoded Request/Response]
//! ```
//!
//! The codec maintains a priority queue for outbound frames, draining
//! priority=0 (control) before priority=1 (interactive) before priority=2 (bulk).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::MAX_FRAME_PAYLOAD;

/// Maximum total frame size (header + payload). 32 MB safety limit.
const MAX_FRAME_SIZE: u32 = 32 * 1024 * 1024;

/// Frame header size: 4 bytes length + 1 byte priority + 1 byte flags.
const HEADER_SIZE: usize = 6;

/// Flag indicating the frame contains a `Response::Event`.
/// Used by relay readers to skip msgpack decode for non-event frames.
pub const FLAG_EVENT: u8 = 1;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("frame too large: {size} bytes (max {MAX_FRAME_SIZE})")]
    FrameTooLarge { size: u32 },

    #[error("serialization error: {0}")]
    Serialize(String),

    #[error("deserialization error: {0}")]
    Deserialize(String),

    #[error("connection closed")]
    ConnectionClosed,
}

/// Async frame codec for reading/writing length-prefixed messages.
///
/// Maintains a priority queue on the write side to interleave high-priority
/// frames (control messages) between bulk data frames.
pub struct FrameCodec<R, W> {
    reader: R,
    writer: W,
    /// Priority queues: index 0 = control, 1 = interactive, 2 = bulk.
    write_queues: [VecDeque<Vec<u8>>; 3],
}

impl<R, W> FrameCodec<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// Create a new codec wrapping async reader/writer streams.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            write_queues: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
        }
    }

    /// Read and deserialize the next message from the stream.
    pub async fn read<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T, CodecError> {
        // Read header
        let mut header = [0u8; HEADER_SIZE];
        match self.reader.read_exact(&mut header).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(CodecError::ConnectionClosed);
            }
            Err(e) => return Err(CodecError::Io(e)),
        }

        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let _priority = header[4];
        let _flags = header[5];

        if len > MAX_FRAME_SIZE {
            return Err(CodecError::FrameTooLarge { size: len });
        }

        // Read payload
        let mut payload = vec![0u8; len as usize];
        self.reader
            .read_exact(&mut payload)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => CodecError::ConnectionClosed,
                _ => CodecError::Io(e),
            })?;

        // Deserialize
        rmp_serde::from_slice(&payload).map_err(|e| CodecError::Deserialize(e.to_string()))
    }

    /// Serialize and write a message to the stream immediately.
    ///
    /// For priority-aware sending, use [`enqueue`] + [`flush_queues`] instead.
    pub async fn write<T: Serialize>(&mut self, msg: &T, priority: u8) -> Result<(), CodecError> {
        let frame = encode_frame(msg, priority, 0)?;
        self.writer.write_all(&frame).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Enqueue a message for priority-aware sending.
    pub fn enqueue<T: Serialize>(&mut self, msg: &T, priority: u8) -> Result<(), CodecError> {
        let frame = encode_frame(msg, priority, 0)?;
        let idx = (priority as usize).min(2);
        self.write_queues[idx].push_back(frame);
        Ok(())
    }

    /// Flush all queued frames in priority order (0 before 1 before 2).
    pub async fn flush_queues(&mut self) -> Result<(), CodecError> {
        for queue in &mut self.write_queues {
            while let Some(frame) = queue.pop_front() {
                self.writer.write_all(&frame).await?;
            }
        }
        self.writer.flush().await?;
        Ok(())
    }

    /// Check if there are any queued frames waiting to be sent.
    pub fn has_queued(&self) -> bool {
        self.write_queues.iter().any(|q| !q.is_empty())
    }

    /// Split into separate reader and writer halves.
    pub fn into_parts(self) -> (FrameReader<R>, FrameWriter<W>) {
        (
            FrameReader {
                reader: self.reader,
            },
            FrameWriter {
                writer: self.writer,
                write_queues: self.write_queues,
            },
        )
    }
}

/// Read half of a frame codec.
pub struct FrameReader<R> {
    reader: R,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Read and deserialize the next message.
    pub async fn read<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T, CodecError> {
        let mut header = [0u8; HEADER_SIZE];
        match self.reader.read_exact(&mut header).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(CodecError::ConnectionClosed);
            }
            Err(e) => return Err(CodecError::Io(e)),
        }

        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let _priority = header[4];
        let _flags = header[5];

        if len > MAX_FRAME_SIZE {
            return Err(CodecError::FrameTooLarge { size: len });
        }

        let mut payload = vec![0u8; len as usize];
        self.reader
            .read_exact(&mut payload)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => CodecError::ConnectionClosed,
                _ => CodecError::Io(e),
            })?;

        rmp_serde::from_slice(&payload).map_err(|e| CodecError::Deserialize(e.to_string()))
    }

    /// Read the next raw frame without deserializing.
    ///
    /// Writes the payload into `buf` (clearing it first) and returns `(priority, flags)`.
    /// Used by relay readers to avoid decoding non-event frames.
    pub async fn read_raw(&mut self, buf: &mut Vec<u8>) -> Result<(u8, u8), CodecError> {
        let mut header = [0u8; HEADER_SIZE];
        match self.reader.read_exact(&mut header).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(CodecError::ConnectionClosed);
            }
            Err(e) => return Err(CodecError::Io(e)),
        }

        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let priority = header[4];
        let flags = header[5];

        if len > MAX_FRAME_SIZE {
            return Err(CodecError::FrameTooLarge { size: len });
        }

        buf.clear();
        buf.resize(len as usize, 0);
        self.reader
            .read_exact(buf)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => CodecError::ConnectionClosed,
                _ => CodecError::Io(e),
            })?;

        Ok((priority, flags))
    }
}

/// Write half of a frame codec.
pub struct FrameWriter<W> {
    writer: W,
    write_queues: [VecDeque<Vec<u8>>; 3],
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    /// Serialize and write a message immediately.
    pub async fn write<T: Serialize>(&mut self, msg: &T, priority: u8) -> Result<(), CodecError> {
        let frame = encode_frame(msg, priority, 0)?;
        self.writer.write_all(&frame).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Write a pre-serialized payload (from `encode_payload`) with framing header.
    ///
    /// Used for replaying ring buffer contents without re-serialization.
    /// Flags are set to 0.
    pub async fn write_raw(&mut self, payload: &[u8], priority: u8) -> Result<(), CodecError> {
        self.write_raw_flagged(payload, priority, 0).await
    }

    /// Write a pre-serialized payload with explicit flags.
    ///
    /// Used by event writers to set `FLAG_EVENT` for relay optimization.
    pub async fn write_raw_flagged(
        &mut self,
        payload: &[u8],
        priority: u8,
        flags: u8,
    ) -> Result<(), CodecError> {
        let len = payload.len() as u32;
        if len > MAX_FRAME_SIZE {
            return Err(CodecError::FrameTooLarge { size: len });
        }
        let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
        frame.extend_from_slice(&len.to_le_bytes());
        frame.push(priority);
        frame.push(flags);
        frame.extend_from_slice(payload);
        self.writer.write_all(&frame).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Enqueue a message for priority-aware sending.
    pub fn enqueue<T: Serialize>(&mut self, msg: &T, priority: u8) -> Result<(), CodecError> {
        let frame = encode_frame(msg, priority, 0)?;
        let idx = (priority as usize).min(2);
        self.write_queues[idx].push_back(frame);
        Ok(())
    }

    /// Flush all queued frames in priority order.
    pub async fn flush_queues(&mut self) -> Result<(), CodecError> {
        for queue in &mut self.write_queues {
            while let Some(frame) = queue.pop_front() {
                self.writer.write_all(&frame).await?;
            }
        }
        self.writer.flush().await?;
        Ok(())
    }

    /// Check if there are any queued frames waiting to be sent.
    pub fn has_queued(&self) -> bool {
        self.write_queues.iter().any(|q| !q.is_empty())
    }
}

/// Encode a message into a complete frame: [len: u32 LE][priority: u8][flags: u8][payload].
fn encode_frame<T: Serialize>(msg: &T, priority: u8, flags: u8) -> Result<Vec<u8>, CodecError> {
    let payload = rmp_serde::to_vec(msg).map_err(|e| CodecError::Serialize(e.to_string()))?;
    let len = payload.len() as u32;

    if len > MAX_FRAME_SIZE {
        return Err(CodecError::FrameTooLarge { size: len });
    }

    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(priority);
    frame.push(flags);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Encode a message to raw bytes (just the payload, no framing).
/// Useful for measuring serialized size (e.g., ring buffer byte accounting).
pub fn encode_payload<T: Serialize>(msg: &T) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec(msg).map_err(|e| CodecError::Serialize(e.to_string()))
}

/// Decode a message from raw payload bytes (no framing).
pub fn decode_payload<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, CodecError> {
    rmp_serde::from_slice(data).map_err(|e| CodecError::Deserialize(e.to_string()))
}

/// Split a large payload into chunks of at most `MAX_FRAME_PAYLOAD` bytes.
pub fn chunk_data(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    data.chunks(MAX_FRAME_PAYLOAD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{EnvInfo, Request, Response};
    use nexus_api::{BlockId, ShellEvent};
    use std::path::PathBuf;

    /// Round-trip test: encode → decode for Request.
    #[tokio::test]
    async fn round_trip_request() {
        let req = Request::Execute {
            id: 42,
            command: "ls -la".to_string(),
            block_id: BlockId(7),
        };

        let (client, server) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(client);
        let (sr, sw) = tokio::io::split(server);

        let mut client_codec = FrameCodec::new(cr, cw);
        let mut server_codec = FrameCodec::new(sr, sw);

        client_codec
            .write(&req, req.priority())
            .await
            .unwrap();
        let decoded: Request = server_codec.read().await.unwrap();

        match decoded {
            Request::Execute { id, command, block_id } => {
                assert_eq!(id, 42);
                assert_eq!(command, "ls -la");
                assert_eq!(block_id, BlockId(7));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Round-trip test: encode → decode for Response.
    #[tokio::test]
    async fn round_trip_response() {
        let env = EnvInfo {
            instance_id: String::new(),
            user: "alice".into(),
            hostname: "devbox".into(),
            cwd: PathBuf::from("/home/alice"),
            os: "linux".into(),
            arch: "x86_64".into(),
        };
        let resp = Response::HelloOk {
            agent_version: "0.2.0".into(),
            env,
            capabilities: crate::AgentCaps::default(),
            session_token: [1u8; 16],
        };

        let (client, server) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(client);
        let (sr, sw) = tokio::io::split(server);

        let mut client_codec = FrameCodec::new(cr, cw);
        let mut server_codec = FrameCodec::new(sr, sw);

        server_codec
            .write(&resp, resp.priority())
            .await
            .unwrap();
        let decoded: Response = client_codec.read().await.unwrap();

        match decoded {
            Response::HelloOk {
                agent_version,
                env,
                session_token,
                ..
            } => {
                assert_eq!(agent_version, "0.2.0");
                assert_eq!(env.user, "alice");
                assert_eq!(env.hostname, "devbox");
                assert_eq!(session_token, [1u8; 16]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Test ShellEvent wrapping in Response::Event.
    #[tokio::test]
    async fn round_trip_shell_event() {
        use nexus_api::ShellEvent;

        let event = ShellEvent::CommandStarted {
            block_id: BlockId(1),
            command: "echo hello".into(),
            cwd: PathBuf::from("/tmp"),
        };
        let resp = Response::Event { seq: 1, event };

        let (client, server) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(client);
        let (sr, sw) = tokio::io::split(server);

        let mut writer = FrameCodec::new(sr, sw);
        let mut reader = FrameCodec::new(cr, cw);

        writer.write(&resp, resp.priority()).await.unwrap();
        let decoded: Response = reader.read().await.unwrap();

        match decoded {
            Response::Event { seq, event } => {
                assert_eq!(seq, 1);
                match event {
                    ShellEvent::CommandStarted { block_id, command, .. } => {
                        assert_eq!(block_id, BlockId(1));
                        assert_eq!(command, "echo hello");
                    }
                    other => panic!("unexpected event: {other:?}"),
                }
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Test priority queue ordering.
    #[tokio::test]
    async fn priority_queue_ordering() {
        let (client, server) = tokio::io::duplex(65536);
        let (cr, cw) = tokio::io::split(client);
        let (sr, sw) = tokio::io::split(server);

        let mut writer = FrameCodec::new(sr, sw);
        let mut reader = FrameCodec::new(cr, cw);

        // Enqueue in reverse priority order
        let bulk = Request::FileWrite {
            id: 1,
            path: "/tmp/test".into(),
            offset: 0,
            data: vec![1, 2, 3],
        };
        let interactive = Request::Execute {
            id: 2,
            command: "ls".into(),
            block_id: BlockId(1),
        };
        let control = Request::Ping { seq: 1 };

        writer.enqueue(&bulk, bulk.priority()).unwrap();
        writer.enqueue(&interactive, interactive.priority()).unwrap();
        writer.enqueue(&control, control.priority()).unwrap();
        writer.flush_queues().await.unwrap();

        // Should read in priority order: control, interactive, bulk
        let first: Request = reader.read().await.unwrap();
        assert!(matches!(first, Request::Ping { seq: 1 }));

        let second: Request = reader.read().await.unwrap();
        assert!(matches!(second, Request::Execute { id: 2, .. }));

        let third: Request = reader.read().await.unwrap();
        assert!(matches!(third, Request::FileWrite { id: 1, .. }));
    }

    /// Test split reader/writer.
    #[tokio::test]
    async fn split_codec() {
        let (client, server) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(client);
        let (sr, sw) = tokio::io::split(server);

        let codec = FrameCodec::new(cr, cw);
        let (mut reader, _writer) = codec.into_parts();

        let server_codec_write = FrameCodec::new(sr, sw);
        let (_, mut server_writer) = server_codec_write.into_parts();

        let req = Request::Ping { seq: 42 };
        server_writer
            .write(&req, req.priority())
            .await
            .unwrap();

        let decoded: Request = reader.read().await.unwrap();
        assert!(matches!(decoded, Request::Ping { seq: 42 }));
    }

    /// Test encode_payload / decode_payload helpers.
    #[test]
    fn payload_encode_decode() {
        let req = Request::Execute {
            id: 1,
            command: "echo test".into(),
            block_id: BlockId(5),
        };

        let bytes = encode_payload(&req).unwrap();
        let decoded: Request = decode_payload(&bytes).unwrap();

        match decoded {
            Request::Execute { id, command, block_id } => {
                assert_eq!(id, 1);
                assert_eq!(command, "echo test");
                assert_eq!(block_id, BlockId(5));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Test chunk_data splits correctly.
    #[test]
    fn chunk_data_splits() {
        let data = vec![0u8; MAX_FRAME_PAYLOAD * 2 + 100];
        let chunks: Vec<_> = chunk_data(&data).collect();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), MAX_FRAME_PAYLOAD);
        assert_eq!(chunks[1].len(), MAX_FRAME_PAYLOAD);
        assert_eq!(chunks[2].len(), 100);
    }

    /// Test connection closed detection.
    #[tokio::test]
    async fn connection_closed() {
        let (client, server) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(client);

        drop(server); // Close the server side

        let mut reader = FrameCodec::new(cr, cw);
        let result: Result<Request, _> = reader.read().await;
        assert!(matches!(result, Err(CodecError::ConnectionClosed)));
    }

    /// Test all Request variants round-trip correctly.
    #[tokio::test]
    async fn all_request_variants() {
        use std::collections::HashMap;

        let requests: Vec<Request> = vec![
            Request::Hello {
                protocol_version: 1,
                capabilities: crate::ClientCaps::default(),
                forwarded_env: HashMap::from([("EDITOR".into(), "vim".into())]),
            },
            Request::Execute {
                id: 1,
                command: "ls".into(),
                block_id: BlockId(1),
            },
            Request::Classify {
                id: 2,
                command: "vim".into(),
            },
            Request::Complete {
                id: 3,
                input: "ls -".into(),
                cursor: 4,
            },
            Request::CancelBlock {
                id: 4,
                block_id: BlockId(1),
            },
            Request::SearchHistory {
                id: 5,
                query: "git".into(),
                limit: 10,
            },
            Request::PtySpawn {
                id: 6,
                command: "vim".into(),
                block_id: BlockId(2),
                cols: 80,
                rows: 24,
                term: "xterm-256color".into(),
                cwd: "/home/user".into(),
            },
            Request::PtyInput {
                block_id: BlockId(2),
                data: b"hello".to_vec(),
                echo_epoch: 42,
            },
            Request::PtyResize {
                block_id: BlockId(2),
                cols: 120,
                rows: 40,
            },
            Request::PtyKill {
                block_id: BlockId(2),
                signal: 9,
            },
            Request::PtyClose {
                block_id: BlockId(2),
            },
            Request::TerminalResize { cols: 120, rows: 40 },
            Request::FileRead {
                id: 7,
                path: "/etc/hosts".into(),
                offset: 0,
                len: Some(1024),
            },
            Request::FileWrite {
                id: 8,
                path: "/tmp/test".into(),
                offset: 0,
                data: vec![1, 2, 3],
            },
            Request::Nest {
                id: 9,
                transport: crate::Transport::Ssh {
                    destination: "user@host".into(),
                    port: Some(22),
                    identity: None,
                    extra_args: vec![],
                },
                force_redeploy: false,
            },
            Request::Unnest { id: 10 },
            Request::GrantCredits { bytes: 1024 * 1024 },
            Request::Ping { seq: 1 },
            Request::Resume {
                session_token: [0u8; 16],
                last_seen_seq: 42,
                cols: 120,
                rows: 24,
            },
            Request::CancelFileRead { id: 42 },
            Request::Shutdown,
        ];

        for req in &requests {
            let bytes = encode_payload(req).unwrap();
            let decoded: Request = decode_payload(&bytes).unwrap();
            // Just verify it doesn't panic — detailed matching already covered above
            let _ = format!("{decoded:?}");
        }
    }

    /// Test all Response variants round-trip correctly.
    #[tokio::test]
    async fn all_response_variants() {
        let env = EnvInfo {
            instance_id: String::new(),
            user: "test".into(),
            hostname: "host".into(),
            cwd: PathBuf::from("/home/test"),
            os: "linux".into(),
            arch: "x86_64".into(),
        };

        let responses: Vec<Response> = vec![
            Response::HelloOk {
                agent_version: "0.2.0".into(),
                env: env.clone(),
                capabilities: crate::AgentCaps::default(),
                session_token: [1u8; 16],
            },
            Response::Event {
                seq: 1,
                event: ShellEvent::CommandFinished {
                    block_id: BlockId(1),
                    exit_code: 0,
                    duration_ms: 42,
                },
            },
            Response::ClassifyResult {
                id: 1,
                classification: crate::messages::CommandClassification::Kernel,
            },
            Response::CompleteResult {
                id: 2,
                completions: vec![crate::messages::CompletionItem {
                    text: "ls".into(),
                    display: "ls".into(),
                    kind: crate::messages::CompletionKind::NativeCommand,
                    score: 100,
                }],
                start: 0,
            },
            Response::HistoryResult {
                id: 3,
                entries: vec![crate::messages::HistoryEntry {
                    command: "git status".into(),
                    timestamp: Some(1234567890),
                }],
            },
            Response::FileData {
                id: 4,
                data: vec![1, 2, 3],
                eof: true,
            },
            Response::FileWriteOk {
                id: 5,
                bytes_written: 3,
            },
            Response::NestOk {
                id: 6,
                env: env.clone(),
            },
            Response::UnnestOk {
                id: 7,
                env: env.clone(),
            },
            Response::ChildLost {
                reason: "connection reset".into(),
                surviving_env: env.clone(),
            },
            Response::GrantCredits { bytes: 65536 },
            Response::Pong { seq: 1 },
            Response::SessionState {
                token: [2u8; 16],
                env: env.clone(),
                active_blocks: vec![BlockId(1), BlockId(2)],
                events_lost: false,
            },
            Response::Error {
                id: 99,
                message: "not found".into(),
            },
        ];

        for resp in &responses {
            let bytes = encode_payload(resp).unwrap();
            let decoded: Response = decode_payload(&bytes).unwrap();
            let _ = format!("{decoded:?}");
        }
    }

    /// Test Transport variants serialize/deserialize correctly.
    #[test]
    fn transport_variants() {
        let transports = vec![
            crate::Transport::Ssh {
                destination: "user@host".into(),
                port: Some(2222),
                identity: Some("/home/user/.ssh/id_ed25519".into()),
                extra_args: vec!["-o".into(), "StrictHostKeyChecking=no".into()],
            },
            crate::Transport::Docker {
                container: "my-app".into(),
                user: Some("root".into()),
            },
            crate::Transport::Kubectl {
                pod: "web-abc123".into(),
                namespace: Some("production".into()),
                container: Some("app".into()),
            },
            crate::Transport::Command {
                argv: vec!["my-transport".into(), "--flag".into()],
            },
        ];

        for t in &transports {
            let bytes = encode_payload(t).unwrap();
            let decoded: crate::Transport = decode_payload(&bytes).unwrap();
            let _ = format!("{decoded:?}");
        }
    }
}
