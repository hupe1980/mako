//! What a service declares, and what one run knows.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::Value;

use crate::document::Document;
use crate::report::Report;

/// A boxed future — the seeds are async and the suites are trait objects.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A route and an MCP tool the same principal is refused on.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// A `GET` path the refused principal must get `403` on.
    pub path: &'static str,
    /// The MCP tool, the twin of that route.
    pub tool: &'static str,
    /// Its arguments.
    pub arguments: Value,
}

/// An inbound route the service receives signed events on, with an event it
/// must accept and that changes nothing it holds.
#[derive(Debug, Clone)]
pub struct Inbound {
    /// The route.
    pub path: &'static str,
    /// The event delivered to it.
    pub event: Value,
}

/// One replaceable service's contract beyond its documents.
pub trait Suite: Sync {
    /// The service's name — the directory under `services/`.
    fn service(&self) -> &'static str;

    /// The event types it emits.
    fn emits(&self) -> Vec<&'static str>;

    /// Where it receives signed events.
    fn inbound(&self) -> Vec<Inbound>;

    /// The refusal pair.
    fn refusal(&self) -> Refusal;

    /// Documented calls that make the service emit events the replay then
    /// carries.
    fn seed<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// How one run reaches the service under test.
#[derive(Debug, Clone)]
pub struct Target {
    /// The service's base URL, no trailing slash.
    pub base: String,
    /// A token the service admits for everything the suite does.
    pub token: String,
    /// A token of a principal the service refuses on the refusal pair.
    pub refused_token: Option<String>,
    /// The secret the service verifies inbound events with.
    pub inbound_secret: Option<String>,
    /// Where the kit listens for deliveries.
    pub listen: String,
    /// The URL the service delivers to; `http://{listen}/` when absent.
    pub callback: Option<String>,
    /// How long to wait for deliveries.
    pub wait: Duration,
}

/// One run: the target, its documents and the report.
pub struct Ctx {
    /// The service under test.
    pub target: Target,
    /// Its reference OpenAPI document.
    pub document: Document,
    /// The tools its reference snapshot declares.
    pub mcp_tools: Vec<String>,
    /// The client every call goes through.
    pub http: reqwest::Client,
    /// What the run found.
    pub report: Report,
}

/// An answer: status and the body as JSON (`Null` when it was not).
#[derive(Debug, Clone)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The body as JSON, `Null` when it was not.
    pub body: Value,
    /// The `ETag`, where one was sent.
    pub etag: Option<String>,
}

impl Ctx {
    /// Call `method path` on the target with `token`.
    ///
    /// # Errors
    ///
    /// When the service cannot be reached.
    pub async fn call(
        &self,
        token: Option<&str>,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> anyhow::Result<Answer> {
        let url = format!("{}{path}", self.target.base);
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut req = self.http.request(method, url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let r = req.send().await?;
        let status = r.status().as_u16();
        let etag = r
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let text = r.text().await.unwrap_or_default();
        Ok(Answer {
            status,
            body: serde_json::from_str(&text).unwrap_or(Value::Null),
            etag,
        })
    }

    /// The same, with the operator token.
    ///
    /// # Errors
    ///
    /// When the service cannot be reached.
    pub async fn operator(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> anyhow::Result<Answer> {
        self.call(Some(&self.target.token), method, path, body)
            .await
    }
}
