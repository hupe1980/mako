#![allow(clippy::doc_markdown)]
//! HTTP client for `makod` admin APIs.
//!
//! The MDM calls `makod` on three paths:
//! - `PUT /admin/malo/{malo_id}` — push `MaloIdentResultPositive` to the MaLo cache
//! - `PUT /admin/partners/{gln}` — upsert a trading-partner record
//! - `POST /api/v1/commands` — forward an ERP command with enriched context
//!
//! All calls carry a named API key in `Authorization: Bearer <key>`.

use reqwest::Client;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::MdmError;

// ── MaloIdentResultPositive ───────────────────────────────────────────────────

/// Subset of the BDEW API-Webdienste Strom `MaloIdentResultPositive` that the
/// MDM needs to push to `makod PUT /admin/malo/{malo_id}`.
///
/// The full schema is defined in `energy-api`; we use a compatible subset here
/// to avoid a cross-crate dependency on `energy-api` from `mako-mdm`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaloIdentResultPositive {
    pub malo_id: String,
    pub nb_gln: String,
    pub msb_gln: Option<String>,
    pub sender_market_partner_id: String,
    pub bilanzierungsgebiet: Option<String>,
    pub netzgebiet: Option<String>,
    pub sparte: String,
}

// ── ForwardCommand ────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/commands` on `makod`.
#[derive(Debug, Serialize)]
pub struct ForwardCommand {
    pub command: String,
    pub malo_id: Option<String>,
    pub melo_id: Option<String>,
    pub payload: serde_json::Value,
}

/// `202 Accepted` response from `POST /api/v1/commands`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandAccepted {
    pub process_id: uuid::Uuid,
    pub command: String,
    pub idempotency_key: Option<String>,
}

// ── PartnerRecord (makod wire format) ─────────────────────────────────────────

/// Trading-partner record in `makod`'s admin API format.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MakodPartner {
    pub gln: String,
    pub display_name: Option<String>,
    pub marktrolle: Option<String>,
    pub channels: serde_json::Value,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Typed HTTP client for `makod` admin and command APIs.
///
/// Clone is cheap — the underlying `reqwest::Client` is `Arc`-backed.
#[derive(Clone)]
pub struct MakodClient {
    client: Client,
    base_url: String,
    api_key: SecretString,
}

impl std::fmt::Debug for MakodClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MakodClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl MakodClient {
    /// Construct a new client.
    ///
    /// `base_url` should be the cluster-internal URL, e.g. `http://makod:8080`.
    /// `api_key` is the named API key provisioned on `makod` with `--auth-key mdm=<token>`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying TLS/connection configuration is invalid, which
    /// cannot happen with the default `reqwest` settings.
    pub fn new(base_url: impl Into<String>, api_key: SecretString) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client construction is infallible"),
            base_url: base_url.into(),
            api_key,
        }
    }

    /// Push a `MaloIdentResultPositive` to `makod`'s MaLo cache.
    ///
    /// `PUT /admin/malo/{malo_id}`
    ///
    /// # Errors
    ///
    /// Returns [`MdmError::MakodSync`] on HTTP error or network failure.
    pub async fn put_malo(
        &self,
        malo_id: &str,
        record: &MaloIdentResultPositive,
    ) -> Result<(), MdmError> {
        let url = format!("{}/admin/malo/{malo_id}", self.base_url);
        debug!(malo_id, "pushing MaLo to makod admin cache");
        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(record)
            .send()
            .await
            .map_err(|e| MdmError::MakodSync(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            warn!(malo_id, status, %body, "makod PUT /admin/malo failed");
            Err(MdmError::MakodSync(format!(
                "PUT /admin/malo/{malo_id} returned HTTP {status}: {body}"
            )))
        }
    }

    /// Upsert a trading partner in `makod`'s partner directory.
    ///
    /// `PUT /admin/partners/{gln}`
    ///
    /// # Errors
    ///
    /// Returns [`MdmError::MakodSync`] on HTTP error or network failure.
    pub async fn put_partner(&self, gln: &str, partner: &MakodPartner) -> Result<(), MdmError> {
        let url = format!("{}/admin/partners/{gln}", self.base_url);
        debug!(gln, "pushing partner to makod admin directory");
        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(partner)
            .send()
            .await
            .map_err(|e| MdmError::MakodSync(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            warn!(gln, status, %body, "makod PUT /admin/partners failed");
            Err(MdmError::MakodSync(format!(
                "PUT /admin/partners/{gln} returned HTTP {status}: {body}"
            )))
        }
    }

    /// Forward an ERP command to `makod`.
    ///
    /// `POST /api/v1/commands`
    ///
    /// # Errors
    ///
    /// Returns [`MdmError::MakodSync`] on HTTP error or network failure.
    pub async fn post_command(
        &self,
        idempotency_key: &str,
        cmd: &ForwardCommand,
    ) -> Result<CommandAccepted, MdmError> {
        let url = format!("{}/api/v1/commands", self.base_url);
        debug!(command = %cmd.command, idempotency_key, "forwarding command to makod");
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.api_key.expose_secret())
            .header("Idempotency-Key", idempotency_key)
            .json(cmd)
            .send()
            .await
            .map_err(|e| MdmError::MakodSync(e.to_string()))?;

        if resp.status().is_success() {
            resp.json::<CommandAccepted>()
                .await
                .map_err(|e| MdmError::MakodSync(e.to_string()))
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            warn!(status, %body, "makod POST /api/v1/commands failed");
            Err(MdmError::MakodSync(format!(
                "POST /api/v1/commands returned HTTP {status}: {body}"
            )))
        }
    }
}
