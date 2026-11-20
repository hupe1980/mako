//! The MCP runner: the tools the snapshot declares, a call without a token
//! refused, and a tool refused where its REST twin is.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::report::Runner;
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
    let r = req.send().await?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    let message = serde_json::from_str(&text).unwrap_or_else(|_| {
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
        Ok((401, _)) => ctx
            .report
            .held(Runner::Mcp, "a call without a token is refused"),
        Ok((s, _)) => ctx.report.broken(
            Runner::Mcp,
            "a call without a token is refused",
            format!("answered {s}, not 401"),
        ),
        Err(e) => ctx.report.broken(
            Runner::Mcp,
            "a call without a token is refused",
            format!("unreachable: {e}"),
        ),
    }

    let listed = match post(ctx, Some(&ctx.target.token), "tools/list", None, json!({})).await {
        Ok((200, m)) => m["result"]["tools"].as_array().map(|ts| {
            ts.iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect::<BTreeSet<_>>()
        }),
        Ok((s, m)) => {
            ctx.report
                .broken(Runner::Mcp, "tools/list", format!("answered {s}: {m}"));
            None
        }
        Err(e) => {
            ctx.report
                .broken(Runner::Mcp, "tools/list", format!("unreachable: {e}"));
            None
        }
    };
    if let Some(listed) = listed {
        let declared: BTreeSet<String> = ctx.mcp_tools.iter().cloned().collect();
        let missing: Vec<_> = declared.difference(&listed).cloned().collect();
        let extra: Vec<_> = listed.difference(&declared).cloned().collect();
        if missing.is_empty() && extra.is_empty() {
            ctx.report.held(
                Runner::Mcp,
                format!("tools/list names the {} declared tools", declared.len()),
            );
        } else {
            ctx.report.broken(
                Runner::Mcp,
                "tools/list names the declared tools",
                format!("missing {missing:?}, undeclared {extra:?}"),
            );
        }
    }

    let refusal = suite.refusal();
    let name = format!("{} is refused where GET {} is", refusal.tool, refusal.path);
    let Some(token) = ctx.target.refused_token.as_deref() else {
        ctx.report
            .skipped(Runner::Mcp, name, "no --refused-token given");
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
            Runner::Mcp,
            name,
            "answered 401 — the refused principal was not authenticated, so the policy was never asked",
        ),
        Ok((s, m)) if s == 403 || m.get("error").is_some() || m["result"]["isError"] == true => {
            ctx.report.held(Runner::Mcp, name);
        }
        Ok((s, m)) => ctx.report.broken(
            Runner::Mcp,
            name,
            format!("answered {s} with a result — the tool admits a principal its REST twin refuses: {m}"),
        ),
        Err(e) => ctx.report.broken(Runner::Mcp, name, format!("unreachable: {e}")),
    }
}
