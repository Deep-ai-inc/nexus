//! Nexus UI - GPU-accelerated terminal interface.
//!
//! This crate provides the graphical user interface for the Nexus terminal,
//! built on the Strata GPU rendering engine.
//!
//! # Module Organization
//!
//! - `app`: Application core (state machine, message routing, views)
//! - `data`: Shared data models (blocks, agent blocks, jobs, context)
//! - `features`: Business logic slices (shell, agent, input, selection)
//! - `ui`: Shared visuals (widgets, theme, markdown, menus)
//! - `infra`: Low-level system integrations (PTY, kernel, agent systems)
//! - `utils`: Shared utilities

pub mod app;
pub mod data;
pub mod features;
pub mod ui;
pub mod infra;
pub mod utils;
