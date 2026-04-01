//! Nexus protocol client: transport, handshakes, and event bridge.
//!
//! Shared by both the GUI (`nexus-ui`) and the integration test harness.

pub mod event_bridge;
pub mod input_buffer;
pub mod reconnect;
pub mod transport;

pub use input_buffer::RequestSender;
pub use reconnect::{ReconnectError, ReconnectOutcome, ReconnectParams};
pub use transport::TransportHandle;
