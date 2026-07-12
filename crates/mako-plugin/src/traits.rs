//! Extension-point traits for mako plugins.

use std::{future::Future, pin::Pin};

use serde_json::Value;

use crate::{PluginContext, PluginError};

// ── CloudEventPlugin ─────────────────────────────────────────────────────────

/// Enriches or filters CloudEvents before they are delivered by the EventBus.
///
/// Called once per `EventBus::publish()` for every registered plugin, in
/// registration order.  If any plugin returns `Err(_)`, the event is still
/// delivered but a warning is emitted.
///
/// **Informatorisches Unbundling:** plugins registered in NB-role services
/// must not include LF customer data in enriched events.
pub trait CloudEventPlugin: Send + Sync + 'static {
    /// Unique plugin name for logging.
    fn name(&self) -> &str;

    /// Mutate `payload` in-place to add or remove fields.
    ///
    /// The CloudEvent envelope fields (`type`, `source`, `id`, `time`) are
    /// guaranteed to be present.  Plugins must not rename or remove them.
    fn on_event(
        &self,
        ce_type: &str,
        payload: &mut Value,
        ctx: &PluginContext,
    ) -> Result<(), PluginError>;
}

// ── McpToolPlugin ─────────────────────────────────────────────────────────────

/// Definition of a single MCP tool exposed by a plugin.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpPluginTool {
    /// Tool name — must be globally unique within the agentd registry.
    /// Recommended format: `{plugin_name}_{verb}_{noun}`.
    pub name: String,
    /// Human-readable description shown to the LLM.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
}

/// Adds custom MCP tools to the `agentd` LLM agent.
///
/// Called during `agentd` MCP pool initialisation.  The returned tools are
/// merged with the built-in mako MCP tool list and presented to the LLM.
///
/// **Security:** plugins receive `PluginContext` only — they cannot directly
/// access the database, the EDIFACT engine, or other services beyond what
/// the host explicitly exposes via host functions.
pub trait McpToolPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn list_tools(&self) -> Vec<McpPluginTool>;

    /// Execute a tool call.
    ///
    /// `tool_name` is the bare tool name (without plugin prefix).
    /// Returns the tool result as a JSON value.
    fn call_tool(
        &self,
        tool_name: &str,
        args: Value,
        ctx: &PluginContext,
    ) -> Pin<Box<dyn Future<Output = Result<Value, PluginError>> + Send + '_>>;
}

// ── BillingPlugin ────────────────────────────────────────────────────────────

/// A single adjustable billing line item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BillingPosition {
    pub name: String,
    /// Amount in EUR cents.  Positive = charge; negative = credit.
    pub amount_ct: i64,
    pub unit: String,
    pub quantity: f64,
}

/// Adjusts the list of `Rechnungsposition`s calculated by `billingd`.
///
/// Called after billingd builds the standard positions but before the
/// Rechnung is persisted.  Plugins can add discount lines, promo credits,
/// §14a Steuerungsrabatt adjustments, or custom service fees.
pub trait BillingPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Mutate `positions` in-place.  May add, remove, or modify entries.
    ///
    /// Must not change positions with `name` starting with `NNE_` or `KA_`
    /// (pass-through grid costs are protected).
    fn adjust_positions(
        &self,
        malo_id: &str,
        positions: &mut Vec<BillingPosition>,
        ctx: &PluginContext,
    ) -> Result<(), PluginError>;
}

// ── ValidatorPlugin ──────────────────────────────────────────────────────────

/// A single validation finding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationIssue {
    pub code: String,
    pub message: String,
    pub severity: ValidationSeverity,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ValidationSeverity {
    Error,
    Warning,
    Info,
}

/// Adds operator-specific INVOIC plausibility rules to `invoic-checker`.
///
/// Called after the standard 6-check pipeline.  `Error`-severity issues
/// trigger a `Dispute` outcome (REMADV 33002); `Warning` produces a
/// `Warn` outcome that is auto-accepted; `Info` is logged only.
pub trait ValidatorPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Validate a `Rechnung`-shaped JSON payload.
    fn validate(
        &self,
        rechnung: &Value,
        ctx: &PluginContext,
    ) -> Result<Vec<ValidationIssue>, PluginError>;
}

// ── WebhookPlugin ─────────────────────────────────────────────────────────────

/// Enriches outbound webhook HTTP requests before they are sent.
///
/// Called by `mako_service::webhook` for every outbound CloudEvent POST.
/// Plugins can add custom headers (e.g. `X-Operator-Signature`), modify the
/// URL (e.g. for routing), or log outbound events.
pub trait WebhookPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Mutate outbound headers in place.
    fn enrich_request(
        &self,
        url: &str,
        ce_type: &str,
        headers: &mut std::collections::HashMap<String, String>,
        ctx: &PluginContext,
    ) -> Result<(), PluginError>;
}
