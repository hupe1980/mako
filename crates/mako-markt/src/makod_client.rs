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
    /// BO4E market role — serialises as the BDEW code (e.g. `"LF"`), which is
    /// exactly the string makod's admin API expects.
    pub marktrolle: Option<rubo4e::current::Marktrolle>,
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
            // `makod` returns 409 for two unrelated reasons, and only one of them
            // is an idempotent success. Note that `makod` does **not** dedupe on
            // the `Idempotency-Key` header — it requires the header but never
            // compares it. Both 409s come from business-level guards, so the
            // discriminator has to be the `error` field, not the status code.
            //
            //   duplicate_process — an active process already exists for this
            //     business key. This is the at-least-once redelivery case: the
            //     command took effect on the first delivery and the body carries
            //     that process's id. Safe to report as success.
            //
            //   invalid_state — the command is not legal in the process's current
            //     state (e.g. `bestaetigen` on an already-accepted process). The
            //     body carries no `process_id`. This is a genuine error and must
            //     reach the caller.
            //
            // Treating every 409 as success and substituting `Uuid::nil()` for the
            // missing id turned `invalid_state` into a silent success whose
            // process_id pointed at no process — callers persisted the nil UUID as
            // a correlation handle and every later lookup missed it.
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let outcome = classify_conflict(&body, &cmd.command, idempotency_key);
            if let Ok(ref accepted) = outcome {
                debug!(
                    idempotency_key,
                    process_id = %accepted.process_id,
                    "makod returned 409 duplicate_process — adopting the existing process"
                );
            }
            outcome
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            warn!(status, %body, "makod POST /api/v1/commands failed");
            Err(MdmError::MakodSync(format!(
                "POST /api/v1/commands returned HTTP {status}: {body}"
            )))
        }
    }

    /// Fetch the BO4E `Rechnung` for a WiM billing process from makod.
    ///
    /// Calls `GET /api/v1/invoic/{process_id}/rechnung`.
    ///
    /// Returns `Ok(Some(value))` on success, `Ok(None)` on 404,
    /// and `Err` on network/HTTP errors.
    ///
    /// # Errors
    ///
    /// Returns [`MdmError::MakodSync`] on HTTP error or network failure.
    pub async fn get_invoic_rechnung(
        &self,
        process_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, MdmError> {
        let url = format!("{}/api/v1/invoic/{process_id}/rechnung", self.base_url);
        debug!(%process_id, "fetching WiM rechnung from makod");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MdmError::MakodSync(e.to_string()))?;

        if resp.status().is_success() {
            let value: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| MdmError::MakodSync(e.to_string()))?;
            Ok(Some(value))
        } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            warn!(status, %process_id, %body, "makod GET invoic/rechnung failed");
            Err(MdmError::MakodSync(format!(
                "GET /api/v1/invoic/{process_id}/rechnung returned HTTP {status}: {body}"
            )))
        }
    }
}

// ── 409 classification ────────────────────────────────────────────────────────

/// Decide whether a `makod` 409 is an idempotent replay or a real rejection.
///
/// Split out from [`MakodClient::post_command`] so the decision can be tested
/// without an HTTP server — the classification is the part that was wrong, not
/// the transport.
fn classify_conflict(
    body: &serde_json::Value,
    command: &str,
    idempotency_key: &str,
) -> Result<CommandAccepted, MdmError> {
    let kind = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
    let detail = body
        .get("detail")
        .and_then(|v| v.as_str())
        .unwrap_or("(no detail)")
        .to_owned();
    let process_id = body
        .get("process_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<uuid::Uuid>().ok());

    match (kind, process_id) {
        ("duplicate_process", Some(process_id)) => Ok(CommandAccepted {
            process_id,
            command: command.to_owned(),
            idempotency_key: Some(idempotency_key.to_owned()),
        }),
        // A `duplicate_process` we cannot correlate is not a success: reporting
        // one without a usable id is what produced the nil-UUID correlations.
        ("duplicate_process", None) => Err(MdmError::MakodConflict {
            kind: "duplicate_process_without_id".to_owned(),
            detail: format!(
                "makod reported duplicate_process but the body carried no parseable \
                 process_id, so the existing process cannot be correlated: {detail}"
            ),
        }),
        _ => Err(MdmError::MakodConflict {
            kind: if kind.is_empty() {
                "unknown".to_owned()
            } else {
                kind.to_owned()
            },
            detail,
        }),
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::{MdmError, classify_conflict};
    use serde_json::json;

    /// The at-least-once redelivery case: the command took effect on the first
    /// delivery, and the body names the process it created.
    #[test]
    fn duplicate_process_with_an_id_is_an_idempotent_success() {
        let id = "018f3c1e-0000-7000-8000-0000000000aa";
        let accepted = classify_conflict(
            &json!({ "error": "duplicate_process", "process_id": id }),
            "gpke.lieferbeginn.anmelden",
            "k-1",
        )
        .expect("duplicate_process with an id is a success");
        assert_eq!(accepted.process_id.to_string(), id);
        assert_eq!(accepted.idempotency_key.as_deref(), Some("k-1"));
    }

    /// The defect this function exists to close.
    ///
    /// `invalid_state` (e.g. `bestaetigen` on an already-accepted process) is a
    /// 409 that carries no `process_id`. It used to be reported as a success
    /// whose `process_id` was `Uuid::nil()`; callers persisted that nil UUID as
    /// a correlation handle, so the real error vanished and every later lookup
    /// against the stored id missed.
    #[test]
    fn invalid_state_is_an_error_not_a_nil_uuid_success() {
        let err = classify_conflict(
            &json!({
                "error":  "invalid_state",
                "detail": "cannot bestaetigen a process in state Abgeschlossen",
            }),
            "gpke.lieferbeginn.bestaetigen",
            "k-2",
        )
        .expect_err("invalid_state must not be reported as success");
        match err {
            MdmError::MakodConflict { kind, detail } => {
                assert_eq!(kind, "invalid_state");
                assert!(detail.contains("Abgeschlossen"), "detail lost: {detail}");
            }
            other => panic!("expected MakodConflict, got {other:?}"),
        }
    }

    /// A `duplicate_process` whose id is absent or unparseable cannot be
    /// correlated, so it is an error rather than a success with a fabricated id.
    #[test]
    fn duplicate_process_without_a_usable_id_is_an_error() {
        for body in [
            json!({ "error": "duplicate_process" }),
            json!({ "error": "duplicate_process", "process_id": "not-a-uuid" }),
        ] {
            let err = classify_conflict(&body, "cmd", "k")
                .expect_err("an uncorrelatable duplicate is not a success");
            assert!(
                matches!(err, MdmError::MakodConflict { ref kind, .. }
                         if kind == "duplicate_process_without_id"),
                "unexpected: {err:?}"
            );
        }
    }

    /// An empty or unrecognised body must not fall through to success either.
    #[test]
    fn an_unrecognised_conflict_body_is_an_error() {
        let err = classify_conflict(&json!({}), "cmd", "k").expect_err("unknown 409 is an error");
        assert!(
            matches!(err, MdmError::MakodConflict { ref kind, .. } if kind == "unknown"),
            "unexpected: {err:?}"
        );
    }

    /// `MakodConflict` maps to 409, not 500: retrying will not help.
    #[test]
    fn a_command_conflict_is_reported_as_409() {
        let err = MdmError::MakodConflict {
            kind: "invalid_state".to_owned(),
            detail: "x".to_owned(),
        };
        assert_eq!(err.status_u16(), 409);
        assert_eq!(err.error_code(), "makod_conflict");
    }
}
