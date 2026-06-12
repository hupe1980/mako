//! EDI-Energy Directory Service v1 (Verzeichnisdienst).
//!
//! The directory is the central registry for all EDI-Energy API-Webdienste.
//! API providers register their endpoint URLs here; consumers look them up
//! before making calls.
//!
//! This module covers both transport protocols:
//! - **REST** (`directoryServiceV1.yaml`) — [`DirectoryServiceClient`] and
//!   the [`server`] router factory.
//! - **WebSocket** (`webSocketV1.yaml`) — [`DirectoryWsClient`] real-time
//!   subscription client.
//!
//! ## Feature flags
//!
//! | Feature     | What it enables                                      |
//! |-------------|------------------------------------------------------|
//! | `client`    | [`DirectoryServiceClient`] — REST client             |
//! | `websocket` | [`DirectoryWsClient`] — WebSocket subscription client|
//! | `server`    | [`server`] module — axum router + handler trait      |

// Re-export all directory types so callers can `use energy_api::directory::*`.
pub use crate::models::directory::*;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::DirectoryServiceClient;

#[cfg(feature = "websocket")]
mod ws;
#[cfg(feature = "websocket")]
pub use ws::{DirectoryWsClient, SubscriptionSender};

#[cfg(feature = "server")]
pub mod server;
