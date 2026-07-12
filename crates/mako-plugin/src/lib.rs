//! mako Plugin Infrastructure.
//!
//! Provides a three-tier plugin system for mako daemons:
//!
//! ## Tier 1 — Native Rust plugins
//!
//! Implement the extension-point traits directly in Rust and register them
//! at startup.  Zero overhead — trait dispatch is a single virtual call.
//! Suitable for operator-customised binary builds.
//!
//! ```rust,no_run
//! use mako_plugin::{PluginRegistry, CloudEventPlugin, PluginContext, PluginError};
//! use serde_json::Value;
//!
//! struct MyEnricher;
//!
//! impl CloudEventPlugin for MyEnricher {
//!     fn name(&self) -> &str { "my-enricher" }
//!
//!     fn on_event(&self, ce_type: &str, payload: &mut Value, _ctx: &PluginContext)
//!         -> Result<(), PluginError>
//!     {
//!         payload["x-operator-id"] = "my-company".into();
//!         tracing::debug!(plugin = "my-enricher", ce_type, "enriched event");
//!         Ok(())
//!     }
//! }
//!
//! let mut registry = PluginRegistry::default();
//! registry.register_cloud_event(Box::new(MyEnricher));
//! ```
//!
//! ## Tier 2 — WASM plugins (feature `wasm`)
//!
//! Drop any `.wasm` file into the `plugins_dir` configured in the daemon's
//! TOML.  Plugins may be written in Rust, Go, TypeScript, Python, C, or any
//! other WASM-targeting language.  The sandbox is enforced by Wasmtime —
//! plugins cannot read host memory, open sockets, or access the filesystem
//! unless the host explicitly grants those capabilities.
//!
//! ```toml
//! # makod.toml
//! [plugins]
//! dir = "/etc/mako/plugins"
//! wasm_allowed_paths = []     # filesystem paths exposed to WASM plugins
//! ```
//!
//! ## Tier 3 — Process plugins (via agentd)
//!
//! The `agentd` service is itself a process-level plugin host: it calls other
//! services' MCP tool servers and exposes them to the LLM agent loop.
//! Custom MCP tools can be added by registering a `McpToolPlugin` in agentd.
//!
//! ## Extension points
//!
//! | Trait | Called by | Purpose |
//! |---|---|---|
//! | [`CloudEventPlugin`] | `event_bus` | Enrich/filter events before delivery |
//! | [`McpToolPlugin`] | `agentd` | Add custom LLM-callable tools |
//! | [`BillingPlugin`] | `billingd` | Custom `Rechnungsposition` adjustments |
//! | [`ValidatorPlugin`] | `invoic-checker` | Operator-specific plausibility rules |
//! | [`WebhookPlugin`] | `mako-service` webhook | Add headers/metadata to outbound webhooks |

pub mod error;
pub mod manifest;
pub mod registry;
pub mod traits;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use error::PluginError;
pub use manifest::PluginManifest;
pub use registry::PluginRegistry;
pub use traits::{
    BillingPlugin, BillingPosition, CloudEventPlugin, McpPluginTool, McpToolPlugin,
    ValidationIssue, ValidatorPlugin, WebhookPlugin,
};

/// Context passed to every plugin call — read-only access to operator metadata.
///
/// Plugins receive this context but cannot modify it.  They must not call back
/// into the host outside of the explicitly exposed host functions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginContext {
    /// Operator tenant identifier.
    pub tenant: String,
    /// Plugin-specific configuration extracted from TOML `[[plugins]]` entry.
    pub config: serde_json::Value,
}
