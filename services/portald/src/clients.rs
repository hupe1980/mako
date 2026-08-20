//! Upstream service clients for `portald`.
//!
//! [`mako_service::http::Upstream`] carries the transport. This layer adds the
//! untyped relay a gateway needs — `serde_json::Value` in, `(status, Value)`
//! out — which the SDK deliberately does not offer, because a domain service
//! relaying a peer's raw body and status is coupling itself to its wire format.
//!
//! The credentials here are **service** credentials and never stand in for the
//! customer's token: which customer a request is for was decided by
//! [`crate::auth::authorize`], and is expressed by the `malo_id` in the path.

use std::sync::Arc;

use mako_service::http::{Upstream, UpstreamError};
use secrecy::SecretString;

/// The upstreams `portald` aggregates, plus the client used for authorization.
#[derive(Clone)]
pub struct PortalClients {
    /// Meter data — Lastgang, `MeterBillingPeriod`.
    pub edmd: Option<Arc<UpstreamClient>>,
    /// Invoices — billing records, XRechnung rendering.
    pub billingd: Option<Arc<UpstreamClient>>,
    /// Customer account — balance, Kontoauszug, Vorauszahlung, SEPA mandates.
    pub accountingd: Option<Arc<UpstreamClient>>,
    /// EEG/KWKG plants and settlements.
    pub einsd: Option<Arc<UpstreamClient>>,
    /// Supply status.
    pub marktd: Option<Arc<UpstreamClient>>,
    /// Contracts — and the authorization authority for every route.
    pub vertragd: Option<Arc<UpstreamClient>>,
    /// Client for the `vertragd /kunden/authenticate` call.
    ///
    /// Separate from [`Self::vertragd`], which attaches the service credential
    /// as `Authorization`: the authorization call must carry the **customer's**
    /// token in that header.
    pub auth_client: reqwest::Client,
}

/// One upstream, relayed rather than interpreted.
pub struct UpstreamClient(Upstream);

impl UpstreamClient {
    /// Address `name` at `base_url`, sharing the daemon's HTTP client.
    #[must_use]
    pub fn new(
        name: &'static str,
        base_url: &str,
        api_key: Option<SecretString>,
        client: reqwest::Client,
    ) -> Self {
        Self(Upstream::new(name, base_url, api_key, client))
    }

    /// GET a JSON body. `None` on 404.
    ///
    /// # Errors
    ///
    /// Transport failure, a non-404 error status, or a body that is not JSON.
    pub async fn get_json(&self, path: &str) -> Result<Option<serde_json::Value>, UpstreamError> {
        self.0.json(self.0.get(path)).await
    }

    /// GET a text body — an XML rendering, which is not JSON. `None` on 404.
    ///
    /// # Errors
    ///
    /// Transport failure or a non-404 error status.
    pub async fn get_text(&self, path: &str) -> Result<Option<String>, UpstreamError> {
        self.0.text(self.0.get(path)).await
    }

    /// POST a JSON body, returning `(status, body)`.
    ///
    /// The status is returned rather than folded into an error: an upstream
    /// `422` carries the rule it applied — a notice period, a contract state —
    /// and the portal relays that to the customer unchanged.
    ///
    /// # Errors
    ///
    /// Transport failure only.
    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, serde_json::Value), UpstreamError> {
        Self::relay(self.0.post(path).json(body)).await
    }

    /// PUT a JSON body, returning `(status, body)`.
    ///
    /// # Errors
    ///
    /// Transport failure only.
    pub async fn put_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, serde_json::Value), UpstreamError> {
        Self::relay(self.0.put(path).json(body)).await
    }

    async fn relay(
        req: reqwest::RequestBuilder,
    ) -> Result<(u16, serde_json::Value), UpstreamError> {
        // Not `Upstream::send`: that maps a 404 to absence and an error status
        // to a failure, and a write's status *is* the answer being relayed.
        let resp = req
            .send()
            .await
            .map_err(|source| UpstreamError::Transport {
                service: "upstream",
                source,
            })?;
        let status = resp.status().as_u16();
        let json = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, json))
    }
}
