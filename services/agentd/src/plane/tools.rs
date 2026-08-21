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

/// Every tool server the **compiled** specialists' manifests actually grant.
///
/// Derived from the declarations rather than from `[mcp_servers]`, so the check
/// below compares what the agents need against what the deployment wired —
/// which is the direction that catches a missing server. The reverse direction
/// (a wired server nothing grants) is merely unused, and is reported as such.
///
/// Filtered to the specialists in this build's subscription table, because the
/// `manifests![]` embedding itself is not role-gated: a `role-lf` binary still
/// carries the NB manifests as data. Without the filter, an LF deployment would
/// be required to wire `sperrd`'s MCP endpoint — a server only the NB Sperrung
/// specialist grants — which is exactly the cross-arm configuration § 9 EnWG
/// role scoping exists to make impossible.
#[must_use]
pub fn servers_named_in_grants() -> BTreeSet<String> {
    let mut servers = BTreeSet::new();
    for (name, manifest) in super::manifests() {
        if crate::builtin::find(name).is_none() {
            continue;
        }
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
    http: &reqwest::Client,
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
        let client = connect_one(name, uri, &token, http)
            .await
            .with_context(|| format!("connect to MCP server '{name}' at {uri}"))?;
        clients.push((name.clone(), client));
    }

    tracing::info!(servers = clients.len(), "MCP tool transports connected");
    Ok(clients)
}

/// One streamable-HTTP MCP connection, authenticated with the shared bearer.
///
/// `http` is the runner's client, not a fresh one per server. Building one per
/// MCP endpoint gave each its own connection pool, its own DNS cache and — the
/// part that matters — none of the timeouts and TLS settings `mako_service`
/// configures centrally, so a hung tool server had no client-side deadline at
/// all. `reqwest::Client` is an `Arc` internally; cloning it shares the pool.
async fn connect_one(
    name: &str,
    uri: &str,
    token: &str,
    http: &reqwest::Client,
) -> Result<Arc<dyn ToolClient>> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(uri);
    if !token.is_empty() {
        config = config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::with_client(http.clone(), config);

    // `host_info` advertises exactly what agentplane implements. Elicitation,
    // sampling and roots are deliberately not advertised: a server offered one
    // would open an interaction that has no governed runtime path.
    let host_info = McpClient::host_info();
    // What we offered, read off the very value we send rather than restated —
    // agentplane bumping its revision must not leave a stale constant here
    // reporting drift that is not there.
    let offered = host_info.protocol_version.as_str().to_owned();
    let service = host_info
        .serve(transport)
        .await
        .with_context(|| format!("MCP handshake with '{name}'"))?;

    // Fallible since 0.21: rmcp deserializes any string into a protocol
    // version, so a server answering a revision nobody implements would
    // otherwise be talked to in a dialect that does not exist. Refusing here is
    // a startup failure naming the server, which is the same class as a granted
    // server missing from `[mcp_servers]`.
    let client = McpClient::new(name, Arc::new(service))
        .with_context(|| format!("MCP protocol negotiation with '{name}'"))?;

    // MCP negotiates *down* by design, and a downgrade is silent: the tasks
    // extension and structured tool responses defined by the offered revision
    // are simply absent, so a long-running tool behaves synchronously and the
    // governed suspension never happens with nothing saying why. agentd
    // connects at startup, so the one moment an operator can see it is here.
    match client.negotiated_version() {
        Some(version) if version != offered => tracing::warn!(
            server = %name,
            negotiated = %version,
            %offered,
            "MCP server negotiated down — the tasks extension and structured tool responses \
             are absent on this transport, so a long-running tool answers synchronously and \
             never suspends the run"
        ),
        _ => tracing::info!(server = %name, version = %offered, "MCP transport connected"),
    }

    Ok(Arc::new(client) as Arc<dyn ToolClient>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifests grant tools on servers, and we can name them.
    ///
    /// The list is what `[mcp_servers]` must cover, so an empty one would mean
    /// the startup check below passes vacuously. The count follows the
    /// *compiled* specialists: a role-scoped build reaches fewer services, and
    /// that narrowing is the point rather than a shortfall.
    #[test]
    fn the_manifests_name_the_servers_a_deployment_must_wire() {
        let servers = servers_named_in_grants();
        #[cfg(not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")))]
        assert!(
            servers.len() >= 10,
            "28 specialists reach mako's services; got {servers:?}"
        );
        assert!(
            servers.contains("makod"),
            "the cross-cutting protocol specialist grants makod in every build"
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
        let err = connect(
            &endpoints,
            &secrecy::SecretString::from(String::new()),
            &reqwest::Client::new(),
        )
        .await
        .expect_err("makod is granted but unconfigured");
        let msg = err.to_string();
        assert!(
            msg.contains("makod"),
            "the error names the missing server: {msg}"
        );
    }
}
