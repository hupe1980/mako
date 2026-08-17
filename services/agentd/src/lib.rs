#![deny(unsafe_code)]
//! `agentd` — the multi-agent plane for mako.
//!
//! Port: `:9580`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`builtin`] | Compiled-in specialist definitions: name, specialty, triggers |
//! | [`config`] | Configuration (`agentd.toml`) |
//! | [`handlers`] | HTTP handlers + `AppState` |
//! | [`plane`] | The agentplane runtime: manifests, labelling, policy, oversight |
//! | [`skills`] | Specialists whose work is computation, written as code |
//!
//! The procedures themselves are not in this crate's code. Each specialist is a
//! manifest under `agents/`, embedded at compile time and covered by a digest:
//! its prompt, model pair, tool grants, ceilings, approval policy and result
//! schema are declared there, so editing what an agent does is a file a reviewer
//! sees in a diff.
pub mod builtin;
pub mod config;
pub mod handlers;
pub mod plane;
pub mod skills;
