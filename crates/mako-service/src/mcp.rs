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

/// The Streamable HTTP service for one MCP server, served without sessions
/// and reachable under any `Host`.
///
/// `config` carries what differs per server (keep-alive); the session mode and
/// the Host allowlist are overridden here, so no caller can turn either back
/// on. rmcp's default allowlist admits only loopback names — a guard against
/// DNS rebinding of an unauthenticated server on a developer's machine. A mako
/// service authenticates every MCP request and is addressed by name (Compose,
/// Kubernetes, a load balancer), where that allowlist refuses every caller. Give it no shutdown token: without sessions a request ends
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
        config
            .with_legacy_session_mode(false)
            .disable_allowed_hosts(),
    )
}

/// The MCP answer of a shared read — one function behind a REST route and an
/// MCP tool — that returned `answer`.
///
/// `Ok` is the structured content the tool's `output_schema` declares. A
/// refusal the REST route answers with a 4xx is a tool error result naming
/// its stable `code` (`not_found` and the like when the error carries none)
/// and message, which an agent reads and can act on; a 5xx is a protocol
/// error carrying no detail beyond the status, as the REST route renders an
/// `error_id` rather than the error's text.
///
/// # Errors
///
/// A 5xx, or an answer that does not serialize.
pub fn answer<T: serde::Serialize>(
    answer: Result<T, crate::ApiError>,
) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
    use rmcp::model::{CallToolResult, ContentBlock};
    match answer {
        Ok(value) => serde_json::to_value(value)
            .map(CallToolResult::structured)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None)),
        Err(e) if e.status().is_server_error() => {
            tracing::error!(error = %e, "mcp: shared read failed");
            Err(rmcp::ErrorData::internal_error(
                e.status().to_string(),
                None,
            ))
        }
        Err(e) => {
            let code = e.code().map_or_else(
                || {
                    e.status()
                        .canonical_reason()
                        .unwrap_or("refused")
                        .to_lowercase()
                        .replace(' ', "_")
                },
                ToOwned::to_owned,
            );
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{code}: {e}"
            ))]))
        }
    }
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

    /// Assert that `tool` declares an output schema and that `answer` — the
    /// structured content it returns — satisfies it.
    ///
    /// The Desk's types are generated from the declared schema, so an answer
    /// that does not satisfy it (a `Decimal` the schema calls a number, a field
    /// renamed in `serialize` only) is a wire the Desk misreads.
    ///
    /// # Panics
    ///
    /// When the tool is not among `tools`, declares no output schema, the
    /// schema does not compile, or `answer` violates it — naming each
    /// violation.
    pub fn assert_structured_output(tools: &[rmcp::model::Tool], tool: &str, answer: &Value) {
        let declared = tools
            .iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("{tool} is not served"));
        let schema = Value::Object(
            declared
                .output_schema
                .as_deref()
                .unwrap_or_else(|| panic!("{tool} declares no output schema"))
                .clone(),
        );
        let validator = jsonschema::validator_for(&schema)
            .unwrap_or_else(|e| panic!("{tool}'s output schema does not compile: {e}"));
        let violations: Vec<String> = validator
            .iter_errors(answer)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(
            violations.is_empty(),
            "{tool}'s answer violates its output schema:\n{}\nanswer: {answer:#}",
            violations.join("\n")
        );
    }

    /// Assert that `tool`'s declared output schema and the `200` response of
    /// its REST twin `GET path` in `openapi` describe the same shape — or
    /// `POST path` where the route has no `GET`: a read whose input is
    /// personal data (an IBAN) is posted as a body so no access log records
    /// it.
    ///
    /// Both are generated from one Rust type — schemars for MCP, utoipa for
    /// REST — so this holds the two derives to each other: the property names,
    /// which are required, each property's type and nullability, a string's
    /// `format` (`date`, `date-time`, `decimal` — what the Desk brands) and an
    /// enum's values, recursively through `$ref`s, array items and nullable
    /// unions. An integer's width and a description are not compared: the two
    /// generators spell them differently for the same type.
    ///
    /// # Panics
    ///
    /// When the tool or the route is missing, either declares no schema, or
    /// the two shapes differ — printing both.
    pub fn assert_rest_twin(tools: &[rmcp::model::Tool], tool: &str, openapi: &Value, path: &str) {
        let declared = tools
            .iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("{tool} is not served"));
        let mcp = Value::Object(
            declared
                .output_schema
                .as_deref()
                .unwrap_or_else(|| panic!("{tool} declares no output schema"))
                .clone(),
        );
        let route = &openapi["paths"][path];
        let method = if route["get"].is_null() {
            "post"
        } else {
            "get"
        };
        let rest = &route[method]["responses"]["200"]["content"]["application/json"]["schema"];
        assert!(
            !rest.is_null(),
            "{method} {path} documents no application/json 200 response"
        );
        let from_mcp = shape(&mcp, &mcp);
        let from_rest = shape(rest, openapi);
        assert_eq!(
            from_mcp, from_rest,
            "{tool}'s output schema and {method} {path}'s response differ\nMCP:  {from_mcp:#}\nREST: {from_rest:#}"
        );
    }

    /// The comparable shape of `schema`, `$ref`s resolved against `root`.
    fn shape(schema: &Value, root: &Value) -> Value {
        let mut nullable = false;
        let resolved = unwrap_nullable(schema, root, &mut nullable);
        let mut out = serde_json::Map::new();
        let types: Vec<&str> = match &resolved["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if types.contains(&"null") {
            nullable = true;
        }
        let ty = types.into_iter().find(|t| *t != "null");
        out.insert("nullable".into(), Value::Bool(nullable));
        if let Some(ty) = ty {
            out.insert("type".into(), Value::String(ty.into()));
            if ty == "string"
                && let Some(f) = resolved.get("format")
            {
                out.insert("format".into(), f.clone());
            }
        }
        if let Some(values) = resolved.get("enum").and_then(Value::as_array) {
            let mut values: Vec<String> = values
                .iter()
                .filter(|v| !v.is_null())
                .map(Value::to_string)
                .collect();
            values.sort();
            out.insert("enum".into(), json!(values));
        }
        if let Some(props) = resolved.get("properties").and_then(Value::as_object) {
            let props: serde_json::Map<String, Value> = props
                .iter()
                .map(|(k, v)| (k.clone(), shape(v, root)))
                .collect();
            // schemars lists a nullable property as required, utoipa does not;
            // both mean "present or null", so only a non-nullable one counts.
            let mut required: Vec<&str> = resolved["required"]
                .as_array()
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            required.retain(|k| {
                props
                    .get(*k)
                    .is_none_or(|p| p["nullable"] == Value::Bool(false))
            });
            required.sort_unstable();
            out.insert("properties".into(), Value::Object(props));
            out.insert("required".into(), json!(required));
        }
        if let Some(items) = resolved.get("items") {
            out.insert("items".into(), shape(items, root));
        }
        Value::Object(out)
    }

    /// `schema` with its `$ref` resolved and a `null` arm of an `anyOf` /
    /// `oneOf` (or a one-arm `allOf`) folded into `nullable`.
    fn unwrap_nullable(schema: &Value, root: &Value, nullable: &mut bool) -> Value {
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            let target = root
                .pointer(r.trim_start_matches('#'))
                .unwrap_or_else(|| panic!("unresolved $ref {r}"));
            return unwrap_nullable(target, root, nullable);
        }
        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(arms) = schema.get(key).and_then(Value::as_array) {
                let real: Vec<&Value> = arms
                    .iter()
                    .filter(|a| a.get("type").and_then(Value::as_str) != Some("null"))
                    .collect();
                if real.len() < arms.len() {
                    *nullable = true;
                }
                if let [only] = real.as_slice() {
                    return unwrap_nullable(only, root, nullable);
                }
                // A unit enum whose variants carry doc comments: schemars
                // writes one `const` arm per variant, utoipa one `enum` list.
                let consts: Option<Vec<Value>> =
                    real.iter().map(|a| a.get("const").cloned()).collect();
                if let Some(values) = consts.filter(|v| !v.is_empty()) {
                    return json!({ "type": "string", "enum": values });
                }
            }
        }
        schema.clone()
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

    /// A request addressed to the service by name is not refused for its
    /// `Host`: rmcp's default allowlist admits only loopback names, and a
    /// service in Compose or Kubernetes is reached as `vertragd:8480`.
    #[tokio::test]
    async fn a_service_addressed_by_name_is_not_refused_for_its_host() {
        use tower::ServiceExt as _;
        for host in ["vertragd:8480", "mako.example.org"] {
            let res = app()
                .oneshot(
                    axum::http::Request::post("/mcp")
                        .header("host", host)
                        .header("content-type", "application/json")
                        .header("accept", "application/json, text/event-stream")
                        .body(axum::body::Body::from(
                            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(res.status(), axum::http::StatusCode::FORBIDDEN, "{host}");
        }
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

    /// A `Decimal` the schema calls a string and a missing required field are
    /// both violations the structured-output check names.
    #[test]
    fn a_structured_answer_is_held_to_its_declared_schema() {
        #[derive(schemars::JsonSchema)]
        #[expect(dead_code, reason = "only its schema is read")]
        struct Betrag {
            betrag: String,
            tage: u32,
        }
        let tools = [rmcp::model::Tool::new(
            "betrag",
            "a typed answer",
            std::sync::Arc::new(serde_json::Map::new()),
        )
        .with_output_schema::<Betrag>()];
        super::conformance::assert_structured_output(
            &tools,
            "betrag",
            &json!({ "betrag": "12.50", "tage": 3 }),
        );
        let wrong = std::panic::catch_unwind(|| {
            super::conformance::assert_structured_output(
                &tools,
                "betrag",
                &json!({ "betrag": 12.5 }),
            );
        });
        assert!(
            wrong.is_err(),
            "a number for a string and a missing field are refused"
        );
    }

    mod rest_twin {
        use serde::Serialize;

        /// One read's answer, derived for both doors.
        #[derive(Serialize, schemars::JsonSchema, utoipa::ToSchema)]
        pub struct Stand {
            pub malo_id: String,
            #[schemars(extend("format" = "date"))]
            #[schema(format = Date)]
            pub stichtag: String,
            pub ende: Option<String>,
            pub tage: Vec<Tag>,
        }

        #[derive(Serialize, schemars::JsonSchema, utoipa::ToSchema)]
        pub struct Tag {
            pub zustand: Zustand,
        }

        #[derive(Serialize, schemars::JsonSchema, utoipa::ToSchema)]
        #[serde(rename_all = "snake_case")]
        #[expect(dead_code, reason = "only its schema is read")]
        pub enum Zustand {
            /// Every slot of the day measured.
            Gemessen,
            /// A slot of the day without a value.
            Fehlt,
        }

        /// The same answer with the date's format left off the REST side.
        #[derive(Serialize, schemars::JsonSchema, utoipa::ToSchema)]
        pub struct OhneFormat {
            pub malo_id: String,
            pub stichtag: String,
            pub ende: Option<String>,
            pub tage: Vec<Tag>,
        }

        #[utoipa::path(get, path = "/api/v1/stand", responses((status = 200, body = Stand)))]
        pub async fn stand() {}

        #[utoipa::path(get, path = "/api/v1/ohne", responses((status = 200, body = OhneFormat)))]
        pub async fn ohne() {}
    }

    /// The MCP output schema (schemars) and the REST response (utoipa) of one
    /// type are the same shape; a format only one side carries is a difference.
    #[test]
    fn a_rest_twin_is_held_to_the_tool_output_schema() {
        use utoipa_axum::{router::OpenApiRouter, routes};
        let (_, doc) = OpenApiRouter::<()>::new()
            .routes(routes!(rest_twin::stand))
            .routes(routes!(rest_twin::ohne))
            .split_for_parts();
        let doc = serde_json::to_value(doc).expect("the document serializes");
        let tools = [rmcp::model::Tool::new(
            "stand",
            "a typed read",
            std::sync::Arc::new(serde_json::Map::new()),
        )
        .with_output_schema::<rest_twin::Stand>()];
        super::conformance::assert_rest_twin(&tools, "stand", &doc, "/api/v1/stand");
        let differs = std::panic::catch_unwind(|| {
            super::conformance::assert_rest_twin(&tools, "stand", &doc, "/api/v1/ohne");
        });
        assert!(
            differs.is_err(),
            "a date format only MCP declares is refused"
        );
    }
}
