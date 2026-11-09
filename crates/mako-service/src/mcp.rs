//! The MCP surface every mako server shares: the protocol revisions it speaks
//! and the stateless Streamable HTTP service it is served by.
//!
//! **The revisions are mako's, not the SDK's.** rmcp's default
//! `supported_protocol_versions()` is every revision the SDK knows, so a
//! revision a later rmcp learns would be agreed to by every server without
//! anyone deciding it. [`REVISIONS`] is the decision: `2026-07-28`, the
//! revision agentd's client offers, and `2025-11-25`, the newest one with an
//! `initialize` handshake — exactly the two agentplane's
//! `McpClient::SPOKEN_REVISIONS` holds its client to (agentd pins the
//! equality). Each `ServerHandler` returns [`supported_revisions`] from
//! `supported_protocol_versions`, which `server/discover` advertises,
//! `initialize` negotiates within and a per-request `_meta` version is checked
//! against. `cargo xtask check-mcp-surface` refuses a server that does not.
//!
//! **No server holds a session.** `2026-07-28` removed sessions from the
//! protocol, and rmcp serves a request on that revision statelessly whatever
//! the configuration. [`service`] serves `2025-11-25` the same way, so a tool
//! that needs state across calls has to pass a server-minted handle as an
//! argument on both revisions, and a replica behind a load balancer answers
//! any request it receives.

use std::borrow::Cow;
use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::model::ProtocolVersion;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};

/// The MCP revisions every mako server speaks, newest first.
pub const REVISIONS: &[ProtocolVersion] =
    &[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25];

/// [`REVISIONS`] in the shape `ServerHandler::supported_protocol_versions`
/// returns.
#[must_use]
pub fn supported_revisions() -> Cow<'static, [ProtocolVersion]> {
    Cow::Borrowed(REVISIONS)
}

/// The Streamable HTTP service for one MCP server, served without sessions.
///
/// `config` carries what differs per server (allowed hosts, keep-alive); the
/// session mode is overridden here, so no caller can turn sessions back on for
/// `2025-11-25`. Give it no shutdown token: without sessions a request ends
/// with its answer and the server's graceful shutdown drains the ones in
/// flight, while rmcp's token would cut them off with an empty 500.
pub fn service<S>(
    factory: impl Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<S, NeverSessionManager>
where
    S: ServerHandler + Send + 'static,
{
    StreamableHttpService::new(
        factory,
        Arc::new(NeverSessionManager::default()),
        config.with_legacy_session_mode(false),
    )
}

/// Round trips a test drives against a served `/mcp` endpoint.
#[cfg(feature = "testing")]
pub mod conformance {
    use serde_json::{Value, json};

    use rmcp::model::ProtocolVersion;

    use super::REVISIONS;

    /// A served MCP endpoint and the client the round trips use.
    pub struct Endpoint {
        url: String,
        http: reqwest::Client,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Endpoint {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// One answer: the HTTP status, the session header and the JSON-RPC
    /// message (the last `data:` line when the body is an event stream).
    #[derive(Debug)]
    pub struct Answer {
        /// The HTTP status.
        pub status: u16,
        /// The `Mcp-Session-Id` header, if the server minted one.
        pub session: Option<String>,
        /// The JSON-RPC message, `Null` when the body held none.
        pub message: Value,
        /// The body as received.
        pub body: String,
    }

    impl Answer {
        /// The JSON-RPC error code, if the answer is an error.
        #[must_use]
        pub fn error_code(&self) -> Option<i64> {
            self.message["error"]["code"].as_i64()
        }
    }

    impl Endpoint {
        /// Serve `app` on a loopback port and address its `path`.
        ///
        /// # Panics
        ///
        /// When no loopback port can be bound.
        pub async fn serve(app: axum::Router, path: &str) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a loopback port binds");
            let addr = listener.local_addr().expect("the listener has an address");
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self {
                url: format!("http://{addr}{path}"),
                http: reqwest::Client::new(),
                task,
            }
        }

        /// POST one JSON-RPC message with the given headers.
        ///
        /// # Panics
        ///
        /// When the request cannot be sent or the body cannot be read.
        pub async fn post(&self, headers: &[(&str, &str)], body: &Value) -> Answer {
            let mut request = self
                .http
                .post(&self.url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream");
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            let response = request
                .json(body)
                .send()
                .await
                .expect("the request is sent");
            let status = response.status().as_u16();
            let session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(ToOwned::to_owned);
            let text = response.text().await.expect("the body is read");
            let message = serde_json::from_str(&text).unwrap_or_else(|_| {
                text.lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
                    .rfind(|m| m.get("id").is_some())
                    .unwrap_or(Value::Null)
            });
            Answer {
                status,
                session,
                message,
                body: text,
            }
        }

        /// A request on `2026-07-28`: version and capabilities in `_meta`,
        /// `Mcp-Method` (and `Mcp-Name` when given) as headers, no session.
        pub async fn request_2026(
            &self,
            method: &str,
            name: Option<&str>,
            mut params: Value,
        ) -> Answer {
            meta_2026(&mut params);
            let mut headers = vec![
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", method),
            ];
            if let Some(name) = name {
                headers.push(("Mcp-Name", name));
            }
            self.post(&headers, &rpc(method, params)).await
        }

        /// A request on `2025-11-25`: the version in the header only, no
        /// session.
        pub async fn request_2025(&self, method: &str, params: Value) -> Answer {
            self.post(
                &[("MCP-Protocol-Version", "2025-11-25")],
                &rpc(method, params),
            )
            .await
        }

        /// An `initialize` naming `version`.
        pub async fn initialize(&self, version: &str) -> Answer {
            self.post(
                &[],
                &rpc(
                    "initialize",
                    json!({
                        "protocolVersion": version,
                        "capabilities": {},
                        "clientInfo": { "name": "conformance", "version": "0" }
                    }),
                ),
            )
            .await
        }
    }

    fn rpc(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
    }

    fn meta_2026(params: &mut Value) {
        if !params.is_object() {
            *params = json!({});
        }
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": { "name": "conformance", "version": "0" }
        });
    }

    /// Drive the round trips every mako MCP server must pass against `app`
    /// served at `path`, calling `tool` (one that needs no database) on both
    /// revisions.
    ///
    /// - `server/discover` on `2026-07-28` advertises exactly [`REVISIONS`];
    /// - `tools/list` and `tools/call` on `2026-07-28` are answered without a
    ///   session, `Mcp-Name` naming the tool, and a request whose `Mcp-Name`
    ///   names another tool is refused;
    /// - `initialize` on `2025-11-25` is answered with that revision and no
    ///   `Mcp-Session-Id`, and `tools/list` and `tools/call` follow without
    ///   one;
    /// - `initialize` naming `2025-06-18` settles on `2025-11-25`, and a
    ///   request naming `2025-06-18` in its `_meta` is refused.
    ///
    /// # Panics
    ///
    /// On the first round trip that does not hold, naming it.
    pub async fn round_trips(app: axum::Router, path: &str, tool: &str) {
        let mcp = Endpoint::serve(app, path).await;
        on_2026_07_28(&mcp, tool).await;
        on_2025_11_25(&mcp, tool).await;
        outside_the_declared_set(&mcp).await;
    }

    /// `server/discover`, list and call without a session; `Mcp-Name` read;
    /// `GET` opens no stream.
    async fn on_2026_07_28(mcp: &Endpoint, tool: &str) {
        let discover = mcp.request_2026("server/discover", None, json!({})).await;
        let advertised: Vec<&str> = discover.message["result"]["supportedVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("server/discover answers on 2026-07-28: {discover:?}"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let declared: Vec<&str> = REVISIONS.iter().map(ProtocolVersion::as_str).collect();
        assert_eq!(
            advertised, declared,
            "server/discover advertises the declared revisions, not the SDK's"
        );

        let list = mcp.request_2026("tools/list", None, json!({})).await;
        assert!(
            list.message["result"]["tools"]
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t["name"] == tool)),
            "tools/list on 2026-07-28 without a session lists {tool}: {list:?}"
        );
        let call = mcp
            .request_2026(
                "tools/call",
                Some(tool),
                json!({ "name": tool, "arguments": {} }),
            )
            .await;
        assert!(
            call.message["result"].is_object() && call.message["result"]["isError"] != true,
            "tools/call {tool} on 2026-07-28, Mcp-Name naming it, is answered: {call:?}"
        );
        let mismatched = mcp
            .request_2026(
                "tools/call",
                Some("not-this-tool"),
                json!({ "name": tool, "arguments": {} }),
            )
            .await;
        assert!(
            mismatched.message["result"].is_null(),
            "a 2026-07-28 request whose Mcp-Name names another tool is refused: {mismatched:?}"
        );
        assert!(
            [&discover, &list, &call]
                .iter()
                .all(|a| a.session.is_none()),
            "no 2026-07-28 answer carries an Mcp-Session-Id"
        );

        let get = mcp
            .http
            .get(&mcp.url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .expect("the request is sent");
        assert_eq!(
            get.status().as_u16(),
            405,
            "GET opens no stream: without a session there is nothing to stream to"
        );
    }

    /// The handshake, then requests without a session.
    async fn on_2025_11_25(mcp: &Endpoint, tool: &str) {
        let init = mcp.initialize("2025-11-25").await;
        assert_eq!(
            init.message["result"]["protocolVersion"], "2025-11-25",
            "initialize on 2025-11-25 settles on it: {init:?}"
        );
        assert_eq!(
            init.session, None,
            "the server is stateless on 2025-11-25 too: it mints no Mcp-Session-Id"
        );
        let list = mcp.request_2025("tools/list", json!({})).await;
        assert!(
            list.message["result"]["tools"].is_array(),
            "tools/list on 2025-11-25 is answered without a session: {list:?}"
        );
        let call = mcp
            .request_2025("tools/call", json!({ "name": tool, "arguments": {} }))
            .await;
        assert!(
            call.message["result"].is_object() && call.message["result"]["isError"] != true,
            "tools/call {tool} on 2025-11-25 is answered without a session: {call:?}"
        );
    }

    /// An older revision settles down on `initialize` and is refused inline.
    async fn outside_the_declared_set(mcp: &Endpoint) {
        let old = mcp.initialize("2025-06-18").await;
        assert_eq!(
            old.message["result"]["protocolVersion"], "2025-11-25",
            "initialize naming 2025-06-18 settles on the declared handshake revision: {old:?}"
        );
        let mut params = json!({});
        meta_2026(&mut params);
        params["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2025-06-18");
        let inline = mcp
            .post(
                &[
                    ("MCP-Protocol-Version", "2025-06-18"),
                    ("Mcp-Method", "tools/list"),
                ],
                &rpc("tools/list", params),
            )
            .await;
        assert!(
            inline.message["result"].is_null() && inline.error_code().is_some(),
            "a request naming 2025-06-18 in its _meta is refused: {inline:?}"
        );
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use rmcp::model::{
        ErrorData, Implementation, ReadResourceRequestParams, ReadResourceResponse,
        ServerCapabilities, ServerConfig,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use serde_json::json;

    use super::conformance::{Endpoint, round_trips};
    use super::*;

    /// A server with one tool and resources that are never found; `DECLARED`
    /// says whether it returns the shared revision set or the SDK's default.
    #[derive(Clone)]
    struct Fixture<const DECLARED: bool>;

    impl<const DECLARED: bool> ServerHandler for Fixture<DECLARED> {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_resources()
                    .build(),
            )
            .with_server_info(Implementation::new("fixture", "0"))
        }

        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            if DECLARED {
                supported_revisions()
            } else {
                Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
            }
        }

        async fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
            let schema = json!({ "type": "object" });
            let schema = schema.as_object().cloned().unwrap_or_default();
            Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                rmcp::model::Tool::new("ping_tool", "answers", Arc::new(schema)),
            ]))
        }

        async fn call_tool(
            &self,
            _request: rmcp::model::CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
            Ok(
                rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text("ok")])
                    .into(),
            )
        }

        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            Err(ErrorData::resource_not_found(
                format!("{} is not a resource", request.uri),
                None,
            ))
        }
    }

    fn app() -> axum::Router {
        axum::Router::new().route_service(
            "/mcp",
            service(
                || Ok(Fixture::<true>),
                StreamableHttpServerConfig::default(),
            ),
        )
    }

    /// Whether [`round_trips`] refuses `app`.
    async fn refused(app: axum::Router) -> bool {
        tokio::spawn(round_trips(app, "/mcp", "ping_tool"))
            .await
            .is_err_and(|e| e.is_panic())
    }

    /// The round trips fail a server that keeps the SDK's revision set, and
    /// one that keeps sessions on 2025-11-25 — so a server passing them
    /// declares the set and holds no session.
    #[tokio::test]
    async fn the_round_trips_refuse_the_sdk_defaults() {
        let undeclared = axum::Router::new().route_service(
            "/mcp",
            service(
                || Ok(Fixture::<false>),
                StreamableHttpServerConfig::default(),
            ),
        );
        assert!(refused(undeclared).await, "the SDK's revision set passed");

        let sessions = axum::Router::new().route_service(
            "/mcp",
            StreamableHttpService::new(
                || Ok(Fixture::<true>),
                Arc::new(
                    rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
                ),
                StreamableHttpServerConfig::default(),
            ),
        );
        assert!(refused(sessions).await, "a session-holding server passed");
    }

    #[test]
    fn the_declared_revisions_are_2026_07_28_and_2025_11_25() {
        let declared: Vec<&str> = REVISIONS.iter().map(ProtocolVersion::as_str).collect();
        assert_eq!(declared, ["2026-07-28", "2025-11-25"]);
    }

    /// The shared service passes every round trip a mako server is held to.
    #[tokio::test]
    async fn the_shared_service_passes_the_round_trips() {
        round_trips(app(), "/mcp", "ping_tool").await;
    }

    /// Resource-not-found is `-32602` on 2026-07-28 and `-32002` on 2025-11-25.
    #[tokio::test]
    async fn resource_not_found_follows_the_revision() {
        let mcp = Endpoint::serve(app(), "/mcp").await;
        let new = mcp
            .request_2026(
                "resources/read",
                Some("missing://x"),
                json!({ "uri": "missing://x" }),
            )
            .await;
        assert_eq!(new.error_code(), Some(-32602), "{new:?}");
        let old = mcp
            .request_2025("resources/read", json!({ "uri": "missing://x" }))
            .await;
        assert_eq!(old.error_code(), Some(-32002), "{old:?}");
    }
}
