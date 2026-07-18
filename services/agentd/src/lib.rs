#![deny(unsafe_code)]
//! `agentd` — Multi-agent LLM orchestration daemon.
//!
//! Port: `:9580`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `agent` | Orchestrator + specialist agent mesh |
//! | `builtin` | Compiled-in specialist definitions (shipped in container) |
//! | `config` | Configuration (`agentd.toml`) |
//! | `handlers` | HTTP handlers + `AppState` |
//! | `llm` | LLM provider abstraction (OpenAI, Anthropic, Bedrock) |
//! | `mcp` | MCP tool pool across all services |
//! | `rag` | LanceDB RAG engine |

pub mod agent;
pub mod builtin;
pub mod config;
pub mod dlq;
pub mod handlers;
pub mod llm;
pub mod mcp;
pub mod rag;
