//! Operator extension point for the mako event bus.
//!
//! A deployment can enrich or annotate every CloudEvent before it is delivered
//! — adding an operator identifier, tagging events for a downstream ERP,
//! dropping a field an internal policy forbids — without forking mako. That is
//! the whole of this crate: one trait, one registry, one host call-site.
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
//!         Ok(())
//!     }
//! }
//!
//! let mut registry = PluginRegistry::default();
//! registry.register_cloud_event(Box::new(MyEnricher));
//! ```
//!
//! Hand the registry to the bus with `WebhookBus::with_plugins` (in
//! `mako-service`); every `EventBus::publish` then runs the chain in
//! registration order. A plugin that returns `Err` is logged and skipped — the
//! event is still delivered, because an operator customisation must not be able
//! to stop a regulated market notification.
//!
//! # Scope
//!
//! Plugins are compiled into the daemon. There is deliberately no dynamic
//! loading tier: mako daemons ship as distroless images built per deployment,
//! so "rebuild with your plugin" is already the delivery model, and a sandboxed
//! runtime would add an attack surface and a JIT dependency for a capability
//! the build step already provides.
//!
//! # Unbundling
//!
//! A plugin registered in an NB-role service must not copy LF customer data
//! into an enriched event. §6a EnWG informatorisches Unbundling applies to
//! operator extensions exactly as it applies to mako's own code.

pub mod error;
pub mod registry;
pub mod traits;

pub use error::PluginError;
pub use registry::PluginRegistry;
pub use traits::CloudEventPlugin;

/// Context passed to every plugin call — read-only operator metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginContext {
    /// Operator tenant identifier (the BDEW Marktpartner code).
    pub tenant: String,
    /// Plugin-specific configuration, supplied by the registering daemon.
    pub config: serde_json::Value,
}
