//! Nexus integration test harness.
//!
//! Provides infrastructure for testing the agent + transport layer against
//! a real SSH server in Docker. Simulates network failures, disconnects,
//! and reconnects programmatically.

pub mod assertions;
pub mod client;
pub mod container;
pub mod network;
