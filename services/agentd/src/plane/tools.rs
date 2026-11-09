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
/// be required to wire `netzbilanzd`'s MCP endpoint — a server only the NB
/// specialists grant — which is exactly the cross-arm configuration §§ 6a and 7a EnWG
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
    // agentd's own tools (`quellen`) are compiled in, not reached over MCP.
    let granted: BTreeSet<String> = servers_named_in_grants()
        .into_iter()
        .filter(|s| s != super::quellen::SERVER)
        .collect();

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
    let client = open(name, uri, token, http).await?;
    let offered = agentplane::tools::MCP_REVISION.as_str();

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

/// The client for one server, its revision settled.
async fn open(name: &str, uri: &str, token: &str, http: &reqwest::Client) -> Result<McpClient> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(uri);
    if !token.is_empty() {
        config = config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::with_client(http.clone(), config);

    // The destination the allowlist judges has to be the host actually dialled,
    // so it is read off `uri` rather than restated — every mako MCP server is a
    // streamable-HTTP endpoint, so it is always a remote one.
    let destination = reqwest::Url::parse(uri)
        .ok()
        .and_then(|u| u.host_str().map(ToOwned::to_owned))
        .map_or(agentplane::tools::Destination::Local, |host| {
            agentplane::tools::Destination::remote(host)
        });
    // `connect` runs the lifecycle agentplane is written for: `server/discover`
    // on 2026-07-28 first, the `initialize` handshake on 2025-11-25 only when the
    // server cannot answer it. A plain `initialize` naming 2026-07-28 is
    // answered with 2025-11-25, so starting the client by hand would negotiate
    // down on every connection. It advertises exactly what agentplane
    // implements — no elicitation, sampling or roots, which would open an
    // interaction with no governed runtime path — and refuses a server settling
    // outside `SPOKEN_REVISIONS`: a startup failure naming the server, the same
    // class as a granted server missing from `[mcp_servers]`.
    McpClient::connect(name, transport, destination)
        .await
        .with_context(|| format!("MCP connection to '{name}'"))
}

/// A tool server that refuses every call of a run whose stop stands.
///
/// agentplane acts on a stop at the run's next step boundary, and a
/// tool-calling agent's whole tool loop is one step: without this, a run the
/// person stopped would go on reading services until its model answered. With
/// it, each call after the stop is refused before it leaves — answered to the
/// model as a refused call, journaled as a failed one — so no service is read
/// on behalf of a run its starter stopped, and the step ends at the model's
/// next answer.
///
/// The stop is read from the journal store (`JournalStore::cancellation`),
/// where `request_cancel` writes it durably, so it holds on every instance.
#[derive(Debug)]
pub struct StopGuard {
    inner: Arc<dyn ToolClient>,
    journal: Arc<dyn agentplane::journal::JournalStore>,
}

impl StopGuard {
    /// `inner`, refusing the calls of a run with a standing stop in `journal`.
    #[must_use]
    pub fn guarding(
        inner: Arc<dyn ToolClient>,
        journal: Arc<dyn agentplane::journal::JournalStore>,
    ) -> Arc<dyn ToolClient> {
        Arc::new(Self { inner, journal })
    }
}

#[async_trait::async_trait]
impl ToolClient for StopGuard {
    async fn call(
        &self,
        tool: &agentplane::tools::ToolId,
        arguments: &serde_json::Value,
        provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<serde_json::Value, agentplane::tools::ToolError> {
        if let Some(p) = provenance {
            // An unreadable store refuses the call: reading on behalf of a run
            // that may have been stopped is the direction not to fail in.
            let stopped = self.journal.cancellation(p.run).await.map_err(|e| {
                agentplane::tools::ToolError::Unreachable {
                    tool: tool.clone(),
                    detail: format!("whether the run was stopped is unreadable: {e}"),
                }
            })?;
            if stopped.is_some() {
                return Err(agentplane::tools::ToolError::Refused {
                    tool: tool.clone(),
                    detail: "the run was stopped; nothing more is read for it".to_owned(),
                });
            }
        }
        self.inner.call(tool, arguments, provenance).await
    }

    fn destination(&self, tool: &agentplane::tools::ToolId) -> agentplane::tools::Destination {
        self.inner.destination(tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client offers 2026-07-28 and speaks exactly it and 2025-11-25.
    ///
    /// The set every mako MCP server's declared revisions are held to. A
    /// server settling outside it is refused by `McpClient::new` at startup;
    /// one settling on 2025-11-25 is the negotiate-down `connect_one` warns
    /// about. Pinned here so an agentplane release that moves either value
    /// fails a test beside the code that reports it.
    #[test]
    fn the_client_offers_2026_07_28_and_speaks_exactly_two_revisions() {
        assert_eq!(
            McpClient::host_info().protocol_version.as_str(),
            "2026-07-28"
        );
        assert_eq!(agentplane::tools::MCP_REVISION.as_str(), "2026-07-28");
        let spoken: Vec<&str> = McpClient::SPOKEN_REVISIONS
            .iter()
            .map(rmcp::model::ProtocolVersion::as_str)
            .collect();
        assert_eq!(spoken, ["2026-07-28", "2025-11-25"]);
    }

    /// agentd's client settles on 2026-07-28 with a server built the way every
    /// mako server is (`mako_service::mcp`), so no connection negotiates down.
    #[tokio::test]
    async fn the_client_settles_on_2026_07_28_with_a_mako_server() {
        #[derive(Clone)]
        struct Server;
        impl rmcp::ServerHandler for Server {
            fn get_info(&self) -> rmcp::model::ServerConfig {
                rmcp::model::ServerConfig::new(
                    rmcp::model::ServerCapabilities::builder()
                        .enable_tools()
                        .build(),
                )
            }
            fn supported_protocol_versions(
                &self,
            ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
                mako_service::mcp::supported_revisions()
            }
        }
        let app = axum::Router::new().route_service(
            "/mcp",
            mako_service::mcp::service(
                || Ok(Server),
                rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default(),
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = open(
            "fixture",
            &format!("http://{addr}/mcp"),
            "",
            &reqwest::Client::new(),
        )
        .await
        .expect("the client connects");
        assert_eq!(client.negotiated_version().as_deref(), Some("2026-07-28"));
    }

    /// The servers' declared revisions are the client's spoken ones.
    #[test]
    fn the_servers_declare_the_revisions_the_client_speaks() {
        assert_eq!(mako_service::mcp::REVISIONS, McpClient::SPOKEN_REVISIONS);
    }

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
        assert!(
            !msg.contains(&format!("\"{}\"", super::super::quellen::SERVER)),
            "agentd's own tools need no MCP endpoint: {msg}"
        );
    }
}
