//! The MCP runner: the tools the snapshot declares, a call without a token
//! refused, and a tool refused where its REST twin is.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::profile::Probe;
use crate::report::Reason;
use crate::suite::{Ctx, Suite};

/// A JSON-RPC request on MCP revision 2026-07-28: stateless, its revision and
/// client in `_meta`.
fn request(method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "mako-conformance", "version": "0" }
    });
    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
}

/// Post one request; the HTTP status and the JSON-RPC message.
async fn post(
    ctx: &Ctx,
    token: Option<&str>,
    method: &str,
    name: Option<&str>,
    params: Value,
) -> anyhow::Result<(u16, Value)> {
    let mut req = ctx
        .http
        .post(format!("{}/mcp", ctx.target.base))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", method)
        .json(&request(method, params));
    if let Some(n) = name {
        req = req.header("Mcp-Name", n);
    }
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let r = crate::transport::send(req).await?;
    let status = r.status.as_u16();
    let text = std::str::from_utf8(&r.body)?;
    let message = serde_json::from_str(text).unwrap_or_else(|_| {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .rfind(|m| m.get("id").is_some())
            .unwrap_or(Value::Null)
    });
    Ok((status, message))
}

/// Run every MCP check.
pub async fn run(ctx: &Ctx, suite: &dyn Suite) {
    match post(ctx, None, "tools/list", None, json!({})).await {
        Ok((401, _)) => ctx.report.held(Probe::McpAuth),
        Ok((status, _)) => ctx
            .report
            .broken(Probe::McpAuth, format!("expected 401, received {status}")),
        Err(error) => ctx.report.broken(Probe::McpAuth, error.to_string()),
    }

    match post(ctx, Some(&ctx.target.token), "tools/list", None, json!({})).await {
        Ok((200, message))
            if message["jsonrpc"] == "2.0"
                && message["id"] == 1
                && message.get("error").is_none()
                && message["result"]["tools"].is_array() =>
        {
            let tools = message["result"]["tools"]
                .as_array()
                .expect("verified array");
            let declared: BTreeSet<_> = ctx.mcp_tools.iter().map(String::as_str).collect();
            let listed: BTreeSet<_> = tools
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect();
            if listed == declared && listed.len() == tools.len() {
                ctx.report.held(Probe::Tools);
            } else {
                ctx.report.broken(
                    Probe::Tools,
                    "missing, duplicate, malformed or undeclared MCP tool",
                );
            }
            for tool in &ctx.mcp_tools {
                let probe = Probe::Tool(tool.clone());
                if listed.contains(tool.as_str()) {
                    ctx.report.held(probe);
                } else {
                    ctx.report.broken(probe, "declared tool missing");
                }
            }
        }
        Ok((status, _)) => ctx
            .report
            .broken(Probe::Tools, format!("invalid listing (HTTP {status})")),
        Err(error) => ctx.report.broken(Probe::Tools, error.to_string()),
    }
    let refusal = suite.refusal();
    let name = Probe::McpRefusal(refusal.tool.into());
    let Some(token) = ctx.target.refused_token.as_deref() else {
        ctx.report.skipped(name, Reason::MissingRefusedCredential);
        return;
    };
    match post(
        ctx,
        Some(token),
        "tools/call",
        Some(refusal.tool),
        json!({ "name": refusal.tool, "arguments": refusal.arguments }),
    )
    .await
    {
        // Authenticated and then denied: a 401 would only say the key is
        // unknown, not that the policy refuses the principal.
        Ok((401, _)) => ctx.report.broken(
            name,
            "answered 401 — the refused principal was not authenticated, so the policy was never asked",
        ),
        Ok((403, _)) => {
            ctx.report.held(name);
        }
        Ok((s, m)) => ctx.report.broken(
            name,
            format!("answered {s} with a result — the tool admits a principal its REST twin refuses: {m}"),
        ),
        Err(e) => ctx.report.broken(name, format!("unreachable: {e}")),
    }
}
