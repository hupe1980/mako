//! The transport that reaches mako's MCP servers.
//!
//! A manifest grants `tool://makod/submit_command`. That names *which* tool, and
//! deliberately not how to reach it: an agent's declaration — and therefore its
//! digest — must not change when it moves between a laptop and a cluster. Grants
//! are reviewed; wiring is deployed. This module is the wiring.
//!
//! ## One client per server
//!
//! Each `[mcp_servers]` entry becomes its own [`McpClient`], registered under
//! the same name the grants use. `agentplane` routes on the server component of
//! a [`ToolId`](agentplane::tools::ToolId), so a call to `tool://marktd/get_malo`
//! can only ever reach the transport wired for `marktd`. Two mako services both
//! offering `list_deadlines` are two different tools and stay that way.
//!
//! A server named in a grant but absent from `[mcp_servers]` is a startup
//! failure, not a run that discovers at its first tool call that nothing is
//! listening.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use agentplane::tools::{McpClient, ToolClient};
use anyhow::{Context as _, Result};
use rmcp::ServiceExt as _;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use secrecy::ExposeSecret as _;

/// Every tool server the embedded manifests actually grant.
///
/// Derived from the declarations rather than from `[mcp_servers]`, so the check
/// below compares what the agents need against what the deployment wired —
/// which is the direction that catches a missing server. The reverse direction
/// (a wired server nothing grants) is merely unused, and is reported as such.
#[must_use]
pub fn servers_named_in_grants() -> BTreeSet<String> {
    let mut servers = BTreeSet::new();
    for (_, src) in super::MANIFESTS {
        let Ok(manifest) = super::parse_manifest(src) else {
            continue;
        };
        for grant in &manifest.spec.tools {
            if let Some(id) = agentplane::tools::ToolId::parse(&grant.reference) {
                servers.insert(id.server);
            }
        }
    }
    servers
}

/// Connect to every configured MCP server.
///
/// Returns one named client per entry, ready for
/// [`RuntimeBuilder::tool_server`](agentplane::runtime::RuntimeBuilder::tool_server).
///
/// # Errors
///
/// Returns an error when a server a manifest grants is not configured, or when a
/// configured server cannot be reached. Both are deployment faults, and both are
/// worth refusing to start over: an agent whose tools are unreachable produces
/// confident answers with no evidence behind them, which is worse than an
/// agent that does not run.
pub async fn connect(
    endpoints: &HashMap<String, String>,
    api_key: &secrecy::SecretString,
) -> Result<Vec<(String, Arc<dyn ToolClient>)>> {
    let granted = servers_named_in_grants();

    let missing: Vec<&str> = granted
        .iter()
        .map(String::as_str)
        .filter(|s| !endpoints.contains_key(*s))
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "these MCP servers are granted by a specialist manifest but absent from \
         [mcp_servers]: {missing:?} — every run of those specialists would fail \
         identically at its first tool call"
    );

    // A configured server nothing grants is harmless but almost always a typo in
    // the name, so it is said out loud rather than ignored.
    for name in endpoints.keys() {
        if !granted.contains(name) {
            tracing::warn!(
                server = %name,
                "configured in [mcp_servers] but no specialist manifest grants a tool on it"
            );
        }
    }

    let token = api_key.expose_secret().to_owned();
    let mut clients: Vec<(String, Arc<dyn ToolClient>)> = Vec::with_capacity(granted.len());

    for name in &granted {
        let uri = &endpoints[name];
        let client = connect_one(name, uri, &token)
            .await
            .with_context(|| format!("connect to MCP server '{name}' at {uri}"))?;
        clients.push((name.clone(), client));
    }

    tracing::info!(servers = clients.len(), "MCP tool transports connected");
    Ok(clients)
}

/// One streamable-HTTP MCP connection, authenticated with the shared bearer.
async fn connect_one(name: &str, uri: &str, token: &str) -> Result<Arc<dyn ToolClient>> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(uri);
    if !token.is_empty() {
        config = config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::with_client(reqwest::Client::new(), config);

    // `host_info` advertises exactly what agentplane implements. Elicitation,
    // sampling and roots are deliberately not advertised: a server offered one
    // would open an interaction that has no governed runtime path.
    let service = McpClient::host_info()
        .serve(transport)
        .await
        .with_context(|| format!("MCP handshake with '{name}'"))?;

    Ok(Arc::new(McpClient::new(name, Arc::new(service))) as Arc<dyn ToolClient>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifests grant tools on servers, and we can name them.
    ///
    /// The list is what `[mcp_servers]` must cover, so an empty one would mean
    /// the startup check below passes vacuously.
    #[test]
    fn the_manifests_name_the_servers_a_deployment_must_wire() {
        let servers = servers_named_in_grants();
        assert!(
            servers.len() >= 10,
            "28 specialists reach mako's services; got {servers:?}"
        );
        assert!(
            servers.contains("makod"),
            "the protocol daemon is granted by several specialists"
        );
    }

    /// A granted server that is not configured refuses to start.
    ///
    /// The failure this prevents is a deployment that boots clean and then fails
    /// every run of the affected specialists at its first tool call — the same
    /// wiring mistake reported once per request instead of once.
    #[tokio::test]
    async fn a_granted_server_missing_from_the_config_is_a_startup_failure() {
        let endpoints = HashMap::from([("marktd".to_owned(), "http://127.0.0.1:1/mcp".to_owned())]);
        let err = connect(&endpoints, &secrecy::SecretString::from(String::new()))
            .await
            .expect_err("makod is granted but unconfigured");
        let msg = err.to_string();
        assert!(
            msg.contains("makod"),
            "the error names the missing server: {msg}"
        );
    }
}
