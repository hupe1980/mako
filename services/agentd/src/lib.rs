#![deny(unsafe_code)]
//! `agentd` — Multi-agent LLM orchestration daemon.
//!
//! Port: `:9580`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! //! | `builtin` | Compiled-in specialist definitions (shipped in container) |
//! | `config` | Configuration (`agentd.toml`) |
//! | `handlers` | HTTP handlers + `AppState` |
//! //! //!
pub mod builtin;
pub mod config;
pub mod handlers;
pub mod plane;
