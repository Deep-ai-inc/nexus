//! Nexus Pump - I/O handling with ring buffers and stream sniffing.
//!
//! The Pump is the "middleman" that observes data flowing through pipelines
//! without affecting throughput.

pub mod pipe;
pub mod sniffer;

pub use pipe::{Pump, RingBuffer};
pub use sniffer::{detect_format, Format};
