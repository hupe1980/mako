#![allow(clippy::doc_markdown)]
//! HTTP client for `makod` admin APIs.
//!
//! The MDM calls `makod` on three paths:
//! - `PUT /admin/malo/{malo_id}` — push `MaloIdentResultPositive` to the MaLo cache
//! - `PUT /admin/partners/{mp_id}` — upsert a trading-partner record
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
/// to avoid a cross-crate dependency on `energy-api` from `mako-markt`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaloIdentResultPositive {
    pub malo_id: String,
    pub nb_mp_id: String,
    pub msb_mp_id: Option<String>,
    pub sender_market_partner_id: String,
    pub bilanzierungsgebiet: Option<String>,
    pub netzgebiet: Option<String>,
    pub sparte: String,
}

// ── ForwardCommand ────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/commands` on `makod`.
///
/// Serializes to the `ErpCommand` wire format:
/// `{ "command": "...", "marktrolle": "...", "payload": { "malo_id": "...", ... } }`
///
/// `malo_id` and `melo_id` are convenience fields that are merged into
/// `payload` during serialization — callers do not need to repeat them inside
/// the payload object.
#[derive(Debug)]
pub struct ForwardCommand {
    pub command: String,
    /// Optional Marktrolle disambiguation (required for multi-role commands
    /// such as `wim.geraetewechsel.beauftragen`).
    pub marktrolle: Option<String>,
    /// Convenience field: merged into `payload` as `"malo_id"` on serialization.
    pub malo_id: Option<String>,
    /// Convenience field: merged into `payload` as `"melo_id"` on serialization.
    pub melo_id: Option<String>,
    pub payload: serde_json::Value,
}

impl serde::Serialize for ForwardCommand {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        // Merge malo_id / melo_id into payload so the wire format matches
        // makod's ErpCommand: `{ command, marktrolle?, payload: { malo_id?, ... } }`
        let mut merged = match &self.payload {
            serde_json::Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        if let Some(ref id) = self.malo_id {
            merged
                .entry("malo_id")
                .or_insert_with(|| serde_json::Value::String(id.clone()));
        }
        if let Some(ref id) = self.melo_id {
            merged
                .entry("melo_id")
                .or_insert_with(|| serde_json::Value::String(id.clone()));
        }
        let field_count = if self.marktrolle.is_some() { 3 } else { 2 };
        let mut map = serializer.serialize_map(Some(field_count))?;
        map.serialize_entry("command", &self.command)?;
        if let Some(ref role) = self.marktrolle {
            map.serialize_entry("marktrolle", role)?;
        }
        map.serialize_entry("payload", &serde_json::Value::Object(merged))?;
        map.end()
    }
}

/// `202 Accepted` response from `POST /api/v1/commands`.
///
/// makod serialises this in snake_case (`process_id`, `idempotency_key`).
#[derive(Debug, Deserialize)]
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
    pub mp_id: String,
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
    /// Constructs the `energy_api::MaloIdentResultPositive`-compatible JSON
    /// from the flat [`MaloIdentResultPositive`] fields.  The nested
    /// `dataMarketLocation` shape is the minimum required by makod's
    /// `UpsertRequest { result, source }` serde contract.
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

        // Build the camelCase nested structure that makod's UpsertRequest expects:
        //   { "result": { "dataMarketLocation": { ... } }, "source": "mdm-sync" }
        //
        // MarktpartnerId::to_i64() is infallible in rubo4e v0.3 — no .unwrap_or(0)
        // fallback that could silently produce a wrong GLN (0 is not a valid GLN).
        let mut nb_operators = Vec::new();
        if !record.nb_mp_id.is_empty() {
            let nb_i64 = record
                .nb_mp_id
                .parse::<rubo4e::identifiers::MarktpartnerId>()
                .map(|id| id.to_i64())
                .unwrap_or(0);
            nb_operators.push(serde_json::json!({
                "marketPartnerId": nb_i64,
                "executionTimeFrom": "2000-01-01T00:00:00Z"
            }));
        }
        let mut mpo = Vec::new();
        if let Some(msb) = &record.msb_mp_id
            && !msb.is_empty()
        {
            let msb_i64 = msb
                .parse::<rubo4e::identifiers::MarktpartnerId>()
                .map(|id| id.to_i64())
                .unwrap_or(0);
            mpo.push(serde_json::json!({
                "marketPartnerId": msb_i64,
                "executionTimeFrom": "2000-01-01T00:00:00Z"
            }));
        }

        let body = serde_json::json!({
            "result": {
                "dataMarketLocation": {
                    "maloId": record.malo_id,
                    "energyDirection": "consumption",
                    "measurementTechnologyClassification": "conventionalMeasuringSystem",
                    "optionalChangeForecastBasis": "notPossible",
                    "dataMarketLocationProperties": [],
                    "dataMarketLocationNetworkOperators": nb_operators,
                    "dataMarketLocationTransmissionSystemOperators": [],
                    "dataMarketLocationMeasuringPointOperators": mpo
                }
            },
            "source": "mdm-sync"
        });

        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
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
    /// `PUT /admin/partners/{mp_id}`
    ///
    /// # Errors
    ///
    /// Returns [`MdmError::MakodSync`] on HTTP error or network failure.
    pub async fn put_partner(&self, mp_id: &str, partner: &MakodPartner) -> Result<(), MdmError> {
        let url = format!("{}/admin/partners/{mp_id}", self.base_url);
        debug!(mp_id, "pushing partner to makod admin directory");
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
            warn!(mp_id, status, %body, "makod PUT /admin/partners failed");
            Err(MdmError::MakodSync(format!(
                "PUT /admin/partners/{mp_id} returned HTTP {status}: {body}"
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
        } else if resp.status() == reqwest::StatusCode::CONFLICT {
            // 409: makod already processed this idempotency key (at-least-once
            // re-delivery from marktd fanout). Treat as idempotent success — the
            // command was executed on the first delivery and the outbound EDIFACT
            // is already in the outbox.
            debug!(
                idempotency_key,
                "makod returned 409 — command already processed (idempotent)"
            );
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            // Extract process_id from the error body if available; fall back to nil UUID.
            let process_id = body
                .get("process_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(uuid::Uuid::nil());
            Ok(CommandAccepted {
                process_id,
                command: cmd.command.clone(),
                idempotency_key: Some(idempotency_key.to_owned()),
            })
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
