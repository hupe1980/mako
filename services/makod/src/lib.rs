//! `makod` library surface — exposed for integration tests.
//!
//! This module re-exports the two subsystems that integration tests need to
//! exercise the full render → wire → parse → adapt pipeline without depending
//! on the binary entry point.
//!
//! # For integration tests
//!
//! ```rust,ignore
//! use makod::adapters::gpke_registry;
//! use makod::edifact_renderer::render_to_wire_bytes;
//! use makod::deadline_dispatch;
//! ```
//!
//! All other modules (`main`, `config`, `commands_api`, …) are compiled only
//! as part of the binary target and are not accessible from the library.

pub mod adapters;
pub mod api_bridge;
pub mod deadline_dispatch;
pub mod edifact_renderer;
