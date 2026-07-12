//! [`PluginRegistry`] — central store of all loaded plugins.

use tracing::{info, warn};

use crate::{
    BillingPlugin, BillingPosition, CloudEventPlugin, McpPluginTool, McpToolPlugin, PluginContext,
    PluginError, ValidationIssue, ValidatorPlugin, WebhookPlugin,
};

/// Central registry of all loaded mako plugins.
///
/// Held in an `Arc<PluginRegistry>` inside each service's application state.
/// Extension points call `registry.run_*` methods to invoke all registered plugins
/// in sequence.
///
/// ## Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use mako_plugin::{PluginRegistry, PluginContext};
///
/// let registry = Arc::new(PluginRegistry::default());
///
/// // In event_bus::publish():
/// let mut payload = serde_json::json!({"type": "de.mako.process.initiated"});
/// let ctx = PluginContext { tenant: "9910000000001".into(), config: Default::default() };
/// registry.run_cloud_event_plugins("de.mako.process.initiated", &mut payload, &ctx);
/// ```
#[derive(Default)]
pub struct PluginRegistry {
    cloud_event: Vec<Box<dyn CloudEventPlugin>>,
    mcp_tools: Vec<Box<dyn McpToolPlugin>>,
    billing: Vec<Box<dyn BillingPlugin>>,
    validators: Vec<Box<dyn ValidatorPlugin>>,
    webhooks: Vec<Box<dyn WebhookPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Registration ─────────────────────────────────────────────────────────

    pub fn register_cloud_event(&mut self, plugin: Box<dyn CloudEventPlugin>) -> &mut Self {
        info!(
            plugin = plugin.name(),
            "mako-plugin: registered CloudEventPlugin"
        );
        self.cloud_event.push(plugin);
        self
    }

    pub fn register_mcp_tool(&mut self, plugin: Box<dyn McpToolPlugin>) -> &mut Self {
        info!(
            plugin = plugin.name(),
            "mako-plugin: registered McpToolPlugin"
        );
        self.mcp_tools.push(plugin);
        self
    }

    pub fn register_billing(&mut self, plugin: Box<dyn BillingPlugin>) -> &mut Self {
        info!(
            plugin = plugin.name(),
            "mako-plugin: registered BillingPlugin"
        );
        self.billing.push(plugin);
        self
    }

    pub fn register_validator(&mut self, plugin: Box<dyn ValidatorPlugin>) -> &mut Self {
        info!(
            plugin = plugin.name(),
            "mako-plugin: registered ValidatorPlugin"
        );
        self.validators.push(plugin);
        self
    }

    pub fn register_webhook(&mut self, plugin: Box<dyn WebhookPlugin>) -> &mut Self {
        info!(
            plugin = plugin.name(),
            "mako-plugin: registered WebhookPlugin"
        );
        self.webhooks.push(plugin);
        self
    }

    // ── Invocation ───────────────────────────────────────────────────────────

    /// Run all `CloudEventPlugin`s.  Errors are logged but do not block delivery.
    pub fn run_cloud_event_plugins(
        &self,
        ce_type: &str,
        payload: &mut serde_json::Value,
        ctx: &PluginContext,
    ) {
        for plugin in &self.cloud_event {
            if let Err(e) = plugin.on_event(ce_type, payload, ctx) {
                warn!(
                    plugin = plugin.name(),
                    ce_type,
                    error = %e,
                    "mako-plugin: CloudEventPlugin failed (event still delivered)"
                );
            }
        }
    }

    /// Collect all MCP tool definitions from registered `McpToolPlugin`s.
    pub fn list_mcp_tools(&self) -> Vec<McpPluginTool> {
        self.mcp_tools.iter().flat_map(|p| p.list_tools()).collect()
    }

    /// Dispatch an MCP tool call to the owning plugin.
    ///
    /// Returns `None` if no plugin owns the tool.
    pub async fn call_mcp_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        ctx: &PluginContext,
    ) -> Option<Result<serde_json::Value, PluginError>> {
        for plugin in &self.mcp_tools {
            let owned: Vec<String> = plugin.list_tools().into_iter().map(|t| t.name).collect();
            if owned.iter().any(|n| n == tool_name) {
                return Some(plugin.call_tool(tool_name, args, ctx).await);
            }
        }
        None
    }

    /// Run all `BillingPlugin`s.  Protected positions (prefix `NNE_`, `KA_`) are
    /// restored after plugin execution to prevent regulatory bypass.
    pub fn run_billing_plugins(
        &self,
        malo_id: &str,
        positions: &mut Vec<BillingPosition>,
        ctx: &PluginContext,
    ) {
        for plugin in &self.billing {
            // Snapshot protected positions
            let protected: Vec<BillingPosition> = positions
                .iter()
                .filter(|p| p.name.starts_with("NNE_") || p.name.starts_with("KA_"))
                .cloned()
                .collect();

            if let Err(e) = plugin.adjust_positions(malo_id, positions, ctx) {
                warn!(
                    plugin = plugin.name(),
                    malo_id,
                    error = %e,
                    "mako-plugin: BillingPlugin failed"
                );
                continue;
            }

            // Restore protected positions if plugin removed or modified them
            for prot in &protected {
                if !positions
                    .iter()
                    .any(|p| p.name == prot.name && p.amount_ct == prot.amount_ct)
                {
                    warn!(
                        plugin = plugin.name(),
                        position = %prot.name,
                        "mako-plugin: BillingPlugin modified protected position — restoring"
                    );
                    positions.retain(|p| p.name != prot.name);
                    positions.push(prot.clone());
                }
            }
        }
    }

    /// Run all `ValidatorPlugin`s. Aggregates issues from all plugins.
    pub fn run_validators(
        &self,
        rechnung: &serde_json::Value,
        ctx: &PluginContext,
    ) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        for plugin in &self.validators {
            match plugin.validate(rechnung, ctx) {
                Ok(found) => issues.extend(found),
                Err(e) => warn!(
                    plugin = plugin.name(),
                    error = %e,
                    "mako-plugin: ValidatorPlugin failed"
                ),
            }
        }
        issues
    }

    /// Run all `WebhookPlugin`s to enrich request headers.
    pub fn run_webhook_plugins(
        &self,
        url: &str,
        ce_type: &str,
        headers: &mut std::collections::HashMap<String, String>,
        ctx: &PluginContext,
    ) {
        for plugin in &self.webhooks {
            if let Err(e) = plugin.enrich_request(url, ce_type, headers, ctx) {
                warn!(
                    plugin = plugin.name(),
                    ce_type,
                    error = %e,
                    "mako-plugin: WebhookPlugin failed"
                );
            }
        }
    }

    /// Returns `true` if no plugins are registered (fast-path skip for zero-plugin deployments).
    pub fn is_empty(&self) -> bool {
        self.cloud_event.is_empty()
            && self.mcp_tools.is_empty()
            && self.billing.is_empty()
            && self.validators.is_empty()
            && self.webhooks.is_empty()
    }

    /// Count of registered plugins across all extension points.
    pub fn plugin_count(&self) -> usize {
        self.cloud_event.len()
            + self.mcp_tools.len()
            + self.billing.len()
            + self.validators.len()
            + self.webhooks.len()
    }
}
