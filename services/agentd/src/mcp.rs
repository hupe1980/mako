//! MCP client pool — JSON-RPC tool discovery + execution.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::llm::ToolDef;

static ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct McpEndpoint {
    pub name: String,
    base_url: String,
    api_key: SecretString,
    client: Client,
}

impl McpEndpoint {
    pub fn new(name: String, base_url: String, api_key: SecretString) -> Self {
        Self {
            name,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client: mako_service::http::default_client(),
        }
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = ID.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let resp = self
            .client
            .post(format!("{}/mcp", self.base_url))
            .bearer_auth(self.api_key.expose_secret())
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("MCP {} {}", self.name, method))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("MCP {} {}: {status} {text}", self.name, method);
        }
        let data: Value = serde_json::from_str(&text)?;
        if let Some(err) = data.get("error") {
            anyhow::bail!("MCP {}: {err}", self.name);
        }
        Ok(data["result"].clone())
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolDef>> {
        let result = self.rpc("tools/list", serde_json::json!({})).await?;
        Ok(result["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                Some(ToolDef {
                    name: format!("{}_{}", self.name, t["name"].as_str()?),
                    description: t["description"].as_str().unwrap_or("").to_owned(),
                    input_schema: t["inputSchema"].clone(),
                })
            })
            .collect())
    }

    pub async fn call_tool(&self, bare_name: &str, args: Value) -> Result<Value> {
        let result = self
            .rpc(
                "tools/call",
                serde_json::json!({"name": bare_name, "arguments": args}),
            )
            .await?;
        let content = result["content"].as_array().cloned().unwrap_or_default();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text_parts: Vec<String> = content
            .iter()
            .filter_map(|c| match c["type"].as_str() {
                Some("text") => c["text"].as_str().map(|s| s.to_owned()),
                Some("json") => serde_json::to_string_pretty(&c["json"]).ok(),
                _ => None,
            })
            .collect();
        let text = text_parts.join("\n");
        if is_error {
            Ok(serde_json::json!({"error": text}))
        } else {
            Ok(serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)))
        }
    }
}

pub struct McpPool {
    endpoints: Vec<McpEndpoint>,
    all_tools: Vec<ToolDef>,
}

impl McpPool {
    /// An empty pool — no endpoints, no tools. For tests and tool-less runs.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            endpoints: Vec::new(),
            all_tools: Vec::new(),
        }
    }

    pub async fn connect(
        servers: &std::collections::HashMap<String, String>,
        api_key: &SecretString,
    ) -> Self {
        let mut endpoints = Vec::new();
        let mut all_tools = Vec::new();
        for (name, url) in servers {
            // Clone the SecretString (cheap — Arc-backed internally in secrecy 0.10)
            let key = SecretString::new(api_key.expose_secret().to_string().into());
            let ep = McpEndpoint::new(name.clone(), url.clone(), key);
            match ep.list_tools().await {
                Ok(ts) => {
                    tracing::info!(server = %name, count = ts.len(), "MCP tools");
                    all_tools.extend(ts);
                    endpoints.push(ep);
                }
                Err(e) => tracing::warn!(server = %name, error = %e, "MCP discovery failed"),
            }
        }
        Self {
            endpoints,
            all_tools,
        }
    }

    /// All tools visible to this pool.
    pub fn all_tools(&self) -> &[ToolDef] {
        &self.all_tools
    }

    /// Tools filtered to a set of server names (empty = all).
    pub fn tools_for_servers(&self, servers: &[String]) -> Vec<ToolDef> {
        if servers.is_empty() {
            return self.all_tools.clone();
        }
        self.all_tools
            .iter()
            .filter(|t| servers.iter().any(|s| t.name.starts_with(&format!("{s}_"))))
            .cloned()
            .collect()
    }

    pub async fn call_tool(&self, prefixed: &str, args: Value) -> Result<Value> {
        for ep in &self.endpoints {
            let prefix = format!("{}_", ep.name);
            if let Some(bare) = prefixed.strip_prefix(&prefix) {
                return ep.call_tool(bare, args).await;
            }
        }
        anyhow::bail!("no MCP endpoint for tool '{prefixed}'")
    }
}
