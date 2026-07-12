//! WASM plugin loader — requires feature `wasm`.
//!
//! Loads `.wasm` files using Extism (Wasmtime-backed sandbox) and wraps them
//! as trait-object implementations of the mako extension-point traits.
//!
//! ## WASM plugin interface
//!
//! The WASM plugin must export functions matching the capabilities listed in
//! its `PluginManifest`:
//!
//! | Capability | Required WASM export | Input | Output |
//! |---|---|---|---|
//! | `cloud_event` | `on_event(ce_type, payload_json) -> payload_json` | JSON string | JSON string |
//! | `mcp_tool` | `list_tools() -> tools_json` | — | JSON array of `McpPluginTool` |
//! | `mcp_tool` | `call_tool(name, args_json) -> result_json` | JSON string | JSON string |
//! | `billing` | `adjust_positions(malo_id, positions_json) -> positions_json` | JSON string | JSON string |
//! | `validator` | `validate(rechnung_json) -> issues_json` | JSON string | JSON array of `ValidationIssue` |
//! | `webhook` | `enrich_request(url, ce_type, headers_json) -> headers_json` | JSON string | JSON string |
//!
//! ## Security
//!
//! - Plugins run in a Wasmtime sandbox: no file system, no network, no OS access
//!   unless explicitly granted via `wasm_allowed_paths` config.
//! - CPU time is not bounded by default; add a Wasmtime fuel limit if needed.
//! - Memory is limited to the Wasmtime default (currently 4 GiB addressable, 64 KiB default heap).
//!
//! ## Writing a WASM plugin
//!
//! The easiest way is the Extism Rust PDK:
//!
//! ```toml
//! # Cargo.toml for your plugin
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! extism-pdk = "1"
//! serde_json = "1"
//! ```
//!
//! ```rust,ignore
//! use extism_pdk::*;
//! use serde_json::{json, Value};
//!
//! #[plugin_fn]
//! pub fn on_event(input: String) -> FnResult<String> {
//!     let mut payload: Value = serde_json::from_str(&input)?;
//!     payload["x-mako-plugin"] = json!("my-wasm-enricher");
//!     Ok(serde_json::to_string(&payload)?)
//! }
//! ```
//!
//! Compile with: `cargo build --target wasm32-wasip1 --release`

use std::sync::Mutex;

use extism::{Manifest, Plugin as ExtismPlugin, PluginBuilder, Wasm};
use serde_json::Value;

use crate::{
    BillingPlugin, BillingPosition, CloudEventPlugin, McpPluginTool, McpToolPlugin, PluginContext,
    PluginError, ValidationIssue, ValidatorPlugin, WebhookPlugin,
    manifest::{PluginCapability, PluginManifest},
};

/// A WASM plugin wrapping an Extism `Plugin`.
///
/// The inner `Mutex` is required because `extism::Plugin::call` takes `&mut self`.
/// This is safe: plugin calls are serialised per-instance.  For high-concurrency
/// workloads, create multiple instances (Extism is designed for this).
pub struct WasmPlugin {
    name: String,
    capabilities: Vec<PluginCapability>,
    #[allow(dead_code)]
    ctx: PluginContext,
    inner: Mutex<ExtismPlugin>,
}

impl WasmPlugin {
    /// Load a WASM plugin from the path specified in the manifest.
    pub fn load(manifest: &PluginManifest) -> Result<Self, PluginError> {
        let path = manifest
            .path
            .as_deref()
            .ok_or_else(|| PluginError::Config {
                name: manifest.name.clone(),
                message: "WASM plugin requires `path` field in manifest".into(),
            })?;

        let wasm = Wasm::file(path);
        let extism_manifest = Manifest::new([wasm]);

        let plugin = PluginBuilder::new(extism_manifest)
            .with_wasi(true) // enable WASI for stdout (plugin logs)
            .build()
            .map_err(|e| PluginError::Config {
                name: manifest.name.clone(),
                message: format!("failed to load WASM: {e}"),
            })?;

        tracing::info!(
            name = %manifest.name,
            path,
            capabilities = ?manifest.capabilities,
            "mako-plugin: loaded WASM plugin"
        );

        Ok(Self {
            name: manifest.name.clone(),
            capabilities: manifest.capabilities.clone(),
            ctx: PluginContext {
                tenant: String::new(), // filled in at call time
                config: manifest.config.clone(),
            },
            inner: Mutex::new(plugin),
        })
    }

    fn has_capability(&self, cap: &PluginCapability) -> bool {
        self.capabilities.contains(cap)
    }

    fn call_json(&self, func: &str, input: &str) -> Result<String, PluginError> {
        let mut p = self.inner.lock().unwrap();
        let result: Vec<u8> = p.call(func, input).map_err(|e| PluginError::WasmTrap {
            name: self.name.clone(),
            message: e.to_string(),
        })?;
        String::from_utf8(result).map_err(|e| PluginError::Business {
            name: self.name.clone(),
            message: format!("WASM output is not valid UTF-8: {e}"),
        })
    }
}

impl CloudEventPlugin for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_event(
        &self,
        ce_type: &str,
        payload: &mut Value,
        ctx: &PluginContext,
    ) -> Result<(), PluginError> {
        if !self.has_capability(&PluginCapability::CloudEvent) {
            return Ok(());
        }
        let input = serde_json::json!({
            "ce_type": ce_type,
            "payload": payload,
            "tenant": ctx.tenant,
            "config": ctx.config,
        });
        let out = self.call_json("on_event", &input.to_string())?;
        let new_payload: Value =
            serde_json::from_str(&out).map_err(|e| PluginError::Serialise {
                name: self.name.clone(),
                source: e,
            })?;
        *payload = new_payload;
        Ok(())
    }
}

impl BillingPlugin for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn adjust_positions(
        &self,
        malo_id: &str,
        positions: &mut Vec<BillingPosition>,
        ctx: &PluginContext,
    ) -> Result<(), PluginError> {
        if !self.has_capability(&PluginCapability::Billing) {
            return Ok(());
        }
        let input = serde_json::json!({
            "malo_id": malo_id,
            "positions": positions,
            "tenant": ctx.tenant,
            "config": ctx.config,
        });
        let out = self.call_json("adjust_positions", &input.to_string())?;
        let new_pos: Vec<BillingPosition> =
            serde_json::from_str(&out).map_err(|e| PluginError::Serialise {
                name: self.name.clone(),
                source: e,
            })?;
        *positions = new_pos;
        Ok(())
    }
}

impl ValidatorPlugin for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn validate(
        &self,
        rechnung: &Value,
        ctx: &PluginContext,
    ) -> Result<Vec<ValidationIssue>, PluginError> {
        if !self.has_capability(&PluginCapability::Validator) {
            return Ok(vec![]);
        }
        let input = serde_json::json!({
            "rechnung": rechnung,
            "tenant": ctx.tenant,
            "config": ctx.config,
        });
        let out = self.call_json("validate", &input.to_string())?;
        serde_json::from_str(&out).map_err(|e| PluginError::Serialise {
            name: self.name.clone(),
            source: e,
        })
    }
}

impl WebhookPlugin for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn enrich_request(
        &self,
        url: &str,
        ce_type: &str,
        headers: &mut std::collections::HashMap<String, String>,
        ctx: &PluginContext,
    ) -> Result<(), PluginError> {
        if !self.has_capability(&PluginCapability::Webhook) {
            return Ok(());
        }
        let input = serde_json::json!({
            "url": url, "ce_type": ce_type,
            "headers": headers,
            "tenant": ctx.tenant,
            "config": ctx.config,
        });
        let out = self.call_json("enrich_request", &input.to_string())?;
        let new_headers: std::collections::HashMap<String, String> = serde_json::from_str(&out)
            .map_err(|e| PluginError::Serialise {
                name: self.name.clone(),
                source: e,
            })?;
        *headers = new_headers;
        Ok(())
    }
}

// McpToolPlugin is async — run Extism call in spawn_blocking
impl McpToolPlugin for WasmPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn list_tools(&self) -> Vec<McpPluginTool> {
        if !self.has_capability(&PluginCapability::McpTool) {
            return vec![];
        }
        match self.call_json("list_tools", "") {
            Ok(out) => serde_json::from_str(&out).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(plugin = %self.name, error = %e, "WASM list_tools failed");
                vec![]
            }
        }
    }

    fn call_tool(
        &self,
        tool_name: &str,
        args: Value,
        ctx: &PluginContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, PluginError>> + Send + '_>>
    {
        let input = serde_json::json!({
            "tool": tool_name,
            "args": args,
            "tenant": ctx.tenant,
            "config": ctx.config,
        });
        let input_str = input.to_string();
        // Extism Plugin is not Send — run in spawn_blocking on the blocking thread pool
        let name = self.name.clone();
        let mut inner = self.inner.lock().unwrap();
        let result: Result<Vec<u8>, _> = inner.call("call_tool", &input_str);
        let result = result
            .map_err(|e| PluginError::WasmTrap {
                name: name.clone(),
                message: e.to_string(),
            })
            .and_then(|bytes| {
                let s = String::from_utf8(bytes).map_err(|_| PluginError::Business {
                    name: name.clone(),
                    message: "non-UTF-8 output".into(),
                })?;
                serde_json::from_str(&s).map_err(|e| PluginError::Serialise { name, source: e })
            });
        Box::pin(async move { result })
    }
}

/// Load all WASM plugins from a directory of manifests.
///
/// Returns only successfully loaded plugins; failures are logged and skipped.
pub fn load_wasm_plugins(manifests: &[PluginManifest]) -> Vec<Box<WasmPlugin>> {
    manifests
        .iter()
        .filter(|m| m.kind == crate::manifest::PluginKind::Wasm)
        .filter_map(|m| match WasmPlugin::load(m) {
            Ok(p) => Some(Box::new(p)),
            Err(e) => {
                tracing::error!(
                    plugin = %m.name,
                    error = %e,
                    "mako-plugin: failed to load WASM plugin — skipping"
                );
                None
            }
        })
        .collect()
}
