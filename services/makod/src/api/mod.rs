//! Operator-facing API layer — REST surfaces, OpenAPI, and the MCP server.
//!
//! Everything here is read/administer access for operators and adjacent
//! services; market-partner traffic goes through `transport` instead.

pub mod edifact_api;
pub mod invoic_api;
pub mod malo_admin_api;
pub mod mcp_server;
pub mod metrics_api;
pub mod migration_api;
pub mod openapi;
pub mod partner_api;
