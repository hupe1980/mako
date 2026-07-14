//! Upstream service clients for `portald`.
//!
//! Each client is a thin `reqwest`-based wrapper that proxies data from one
//! upstream service to the portal layer.  All clients are stateless — they
//! carry only base URL + optional bearer token.

use anyhow::{Context as _, Result};

/// Generic upstream client.
pub struct UpstreamClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    service_name: &'static str,
}

impl UpstreamClient {
    /// Create a new client for `service_name` at `base_url`.
    pub fn new(service_name: &'static str, base_url: &str, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client: mako_service::http::default_client(),
            service_name,
        }
    }

    /// GET `{base_url}{path}` and return the response body as JSON.
    /// Returns `None` on 404.
    pub async fn get_json(&self, path: &str) -> Result<Option<serde_json::Value>> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{} GET {path}", self.service_name))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("{} error: {e}", self.service_name))?;
        let body = resp
            .json()
            .await
            .with_context(|| format!("{} parse {path}", self.service_name))?;
        Ok(Some(body))
    }

    /// GET with a forwarded Authorization header (for customer-authenticated calls).
    #[allow(dead_code)]
    pub async fn get_json_with_auth(
        &self,
        path: &str,
        bearer: &str,
    ) -> Result<Option<serde_json::Value>> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, bearer)
            .send()
            .await
            .with_context(|| format!("{} GET {path}", self.service_name))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "{} {}: {}",
                self.service_name,
                status,
                path
            ));
        }
        let body = resp
            .json()
            .await
            .with_context(|| format!("{} parse {path}", self.service_name))?;
        Ok(Some(body))
    }

    /// POST `{base_url}{path}` with a JSON body.
    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, serde_json::Value)> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.post(&url).json(body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{} POST {path}", self.service_name))?;
        let status = resp.status().as_u16();
        let json: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, json))
    }

    /// PUT `{base_url}{path}` with a JSON body.
    pub async fn put_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, serde_json::Value)> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.put(&url).json(body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{} PUT {path}", self.service_name))?;
        let status = resp.status().as_u16();
        let json: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, json))
    }

    /// GET with query parameters forwarded as-is.
    #[allow(dead_code)]
    pub async fn get_json_query(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Option<serde_json::Value>> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.get(&url).query(params);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{} GET {path}", self.service_name))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("{} error: {e}", self.service_name))?;
        Ok(Some(resp.json().await?))
    }

    // ── Accessors for direct reqwest use ─────────────────────────────────────

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }
}
