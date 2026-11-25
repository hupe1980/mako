//! The estate as the seeder reaches it: one base URL per service and the
//! requests it makes, each answer checked for its status.

use std::time::Duration;

use anyhow::{Context as _, bail};
use reqwest::{Method, StatusCode};
use serde_json::Value;

/// Every service of the estate and its port (deploy/ports.toml).
pub const PORTS: &[(&str, u16)] = &[
    ("makod", 8080),
    ("marktd", 8180),
    ("invoicd", 8280),
    ("edmd", 8380),
    ("processd", 8580),
    ("netzbilanzd", 8680),
    ("productd", 9080),
    ("einsd", 9180),
    ("billingd", 9280),
    ("accountingd", 9380),
    ("portald", 9480),
    ("agentd", 9580),
    ("vertragd", 9780),
    ("outputd", 9880),
];

/// makod's door: its `[http] auth_keys` entry `dev=…` (deploy/demo/makod.toml).
const MAKOD_KEY: &str = "dev-makod-key";

/// The running estate.
pub struct Estate {
    pub(crate) http: reqwest::Client,
    compose: bool,
}

/// One answer: its status and its body, JSON where it parsed.
pub struct Answer {
    pub status: StatusCode,
    pub body: Value,
}

impl Answer {
    /// The body, when the status is a success; else an error naming the call.
    pub fn ok(self, what: &str) -> anyhow::Result<Value> {
        if self.status.is_success() {
            Ok(self.body)
        } else {
            bail!("{what} answered {}: {}", self.status, self.body)
        }
    }
}

impl Estate {
    /// The estate on loopback, or by its compose host names.
    pub fn new(compose: bool) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("the HTTP client")?;
        Ok(Self { http, compose })
    }

    /// `http://127.0.0.1:<port>`, or `http://<service>:<port>` under compose.
    pub fn base(&self, service: &str) -> String {
        let port = PORTS
            .iter()
            .find(|(s, _)| *s == service)
            .map_or(0, |(_, p)| *p);
        if self.compose {
            format!("http://{service}:{port}")
        } else {
            format!("http://127.0.0.1:{port}")
        }
    }

    async fn send(
        &self,
        method: Method,
        service: &str,
        path: &str,
        body: Option<Body<'_>>,
        idempotency_key: Option<&str>,
    ) -> anyhow::Result<Answer> {
        let url = format!("{}{path}", self.base(service));
        let mut req = self.http.request(method.clone(), &url);
        if service == "makod" {
            req = req.bearer_auth(MAKOD_KEY);
        }
        if let Some(key) = idempotency_key {
            req = req.header("Idempotency-Key", key);
        }
        req = match body {
            Some(Body::Json(v)) => req.json(v),
            Some(Body::Text(text, content_type)) => req
                .header("content-type", content_type)
                .body(text.to_owned()),
            None => req,
        };
        let response = req
            .send()
            .await
            .with_context(|| format!("{method} {url}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Ok(Answer { status, body })
    }

    /// `GET`.
    pub async fn get(&self, service: &str, path: &str) -> anyhow::Result<Answer> {
        self.send(Method::GET, service, path, None, None).await
    }

    /// `PUT` a JSON body.
    pub async fn put(&self, service: &str, path: &str, body: &Value) -> anyhow::Result<Answer> {
        self.send(Method::PUT, service, path, Some(Body::Json(body)), None)
            .await
    }

    /// `POST` a JSON body.
    pub async fn post(&self, service: &str, path: &str, body: &Value) -> anyhow::Result<Answer> {
        self.send(Method::POST, service, path, Some(Body::Json(body)), None)
            .await
    }

    /// `POST` a JSON body under an `Idempotency-Key`, so a repeated run is
    /// answered with the first run's result instead of acting twice.
    pub async fn post_once(
        &self,
        service: &str,
        path: &str,
        body: &Value,
        key: &str,
    ) -> anyhow::Result<Answer> {
        self.send(
            Method::POST,
            service,
            path,
            Some(Body::Json(body)),
            Some(key),
        )
        .await
    }

    /// `POST` an EDIFACT interchange to makod's door.
    pub async fn edifact(&self, interchange: &str) -> anyhow::Result<Answer> {
        self.post_text(
            "makod",
            "/edifact",
            interchange,
            "text/plain; charset=utf-8",
        )
        .await
    }

    /// `POST` a text body (a file a publisher's import reads).
    pub async fn post_text(
        &self,
        service: &str,
        path: &str,
        text: &str,
        content_type: &str,
    ) -> anyhow::Result<Answer> {
        self.send(
            Method::POST,
            service,
            path,
            Some(Body::Text(text, content_type)),
            None,
        )
        .await
    }

    /// A tool of a service's MCP server, as the Desk calls it; its structured
    /// answer.
    pub async fn tool(&self, service: &str, name: &str, args: Value) -> anyhow::Result<Value> {
        let call = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });
        let url = format!("{}/mcp", self.base(service));
        let mut req = self
            .http
            .post(&url)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-11-25")
            .json(&call);
        if service == "makod" {
            req = req.bearer_auth(MAKOD_KEY);
        }
        let response = req.send().await.with_context(|| format!("POST {url}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        // A streamable-HTTP server may answer as one SSE `data:` line.
        let json = text
            .lines()
            .find_map(|l| l.strip_prefix("data:"))
            .unwrap_or(&text)
            .trim()
            .to_owned();
        let answer: Value = serde_json::from_str(&json)
            .with_context(|| format!("{service} {name} answered {status}: {text}"))?;
        if let Some(error) = answer.get("error") {
            bail!("{service} {name}: {error}");
        }
        let result = &answer["result"];
        if result["isError"].as_bool() == Some(true) {
            bail!("{service} {name} refused: {}", result["content"]);
        }
        if let Some(structured) = result.get("structuredContent") {
            return Ok(structured.clone());
        }
        let text = result["content"][0]["text"].as_str().unwrap_or("null");
        Ok(serde_json::from_str(text).unwrap_or(Value::String(text.to_owned())))
    }

    /// Ask `probe` until it answers `Some`, up to `secs` seconds — for what
    /// arrives through an outbox or a fan-out a moment after the write.
    pub async fn until<T, F, Fut>(&self, secs: u64, what: &str, mut probe: F) -> anyhow::Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Option<T>>>,
    {
        let mut last = None;
        for _ in 0..secs * 2 {
            match probe().await {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        match last {
            Some(e) => Err(e.context(format!("{what} within {secs} s"))),
            None => bail!("{what}: not within {secs} s"),
        }
    }
}

enum Body<'a> {
    Json(&'a Value),
    Text(&'a str, &'a str),
}
