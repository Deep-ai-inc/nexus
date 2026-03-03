//! Offline command queue: buffers commands while disconnected from the
//! remote agent, then flushes them on reconnect.
//!
//! When the connection drops:
//! - The prompt still works (it's local)
//! - Commands are queued in `RemoteBackend.pending_queue`
//! - Displayed in UI as muted/pending
//! - On reconnect: flush queue as sequential `Request::Execute`
//!
//! This module is a placeholder for the queue rendering logic.
//! The actual queue is managed in `RemoteBackend` (mod.rs).
