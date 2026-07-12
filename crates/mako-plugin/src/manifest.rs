//! Plugin manifest — describes which extension points a plugin implements.

use serde::{Deserialize, Serialize};

/// Describes a single plugin loaded by a mako daemon.
///
/// In TOML:
/// ```toml
/// [[plugins]]
/// name   = "my-enricher"
/// kind   = "wasm"           # "native" for Rust blanket plugins
/// path   = "/etc/mako/plugins/my_enricher.wasm"
/// capabilities = ["cloud_event", "webhook"]
/// config = { promo_code_api = "https://promo.example.com" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier.
    pub name: String,
    /// Plugin kind: `"native"` (Rust blanket impl) or `"wasm"` (WASM binary).
    #[serde(default = "default_kind")]
    pub kind: PluginKind,
    /// Path to the `.wasm` file (required for `kind = "wasm"`).
    pub path: Option<String>,
    /// Extension points this plugin implements.
    pub capabilities: Vec<PluginCapability>,
    /// Plugin-specific configuration passed as `PluginContext.config`.
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    /// Implements [`CloudEventPlugin`](crate::CloudEventPlugin).
    CloudEvent,
    /// Implements [`McpToolPlugin`](crate::McpToolPlugin).
    McpTool,
    /// Implements [`BillingPlugin`](crate::BillingPlugin).
    Billing,
    /// Implements [`ValidatorPlugin`](crate::ValidatorPlugin).
    Validator,
    /// Implements [`WebhookPlugin`](crate::WebhookPlugin).
    Webhook,
}

fn default_kind() -> PluginKind {
    PluginKind::Native
}
