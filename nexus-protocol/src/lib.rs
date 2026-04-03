//! Nexus Protocol - Wire protocol for client↔agent communication.
//!
//! Defines the message types and framing used between the Nexus UI (client)
//! and the Nexus Agent (remote headless kernel). Both sides depend on this crate.

pub mod codec;
pub mod messages;
pub mod types;

pub use codec::FrameCodec;
pub use messages::{EnvInfo, Request, Response, Transport};
pub use types::{AgentCaps, ClientCaps};

/// Protocol version. Increment on breaking changes.
/// Used to version-key deployed agent binaries.
pub const PROTOCOL_VERSION: u32 = 10;

/// Maximum payload size per frame (16 KB) to prevent head-of-line blocking.
pub const MAX_FRAME_PAYLOAD: usize = 16 * 1024;

/// Frame priority levels.
pub mod priority {
    /// Control messages (CancelBlock, PtyInput, Ping, Shutdown) — always processed first.
    pub const CONTROL: u8 = 0;
    /// Interactive messages (Execute, Complete, PtySpawn, ShellEvent output).
    pub const INTERACTIVE: u8 = 1;
    /// Bulk data (FileWrite/FileData chunks, large StdoutChunk).
    pub const BULK: u8 = 2;
}
