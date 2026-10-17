//! The MaLo-Ident cache follows `marktd` through its durable fan-out.
//!
//! `marktd` holds the MaLo master record and enqueues `de.markt.malo.updated`
//! in the transaction that changes it. makod subscribes to that event like any
//! fan-out subscriber, reads the record back (`GET /api/v1/malos/{id}`) and
//! upserts or drops its [`SlateDbMaloCache`] entry. The event carries only the
//! record's `version`, so the read is what makes the cache current.
//!
//! **Why a subscription, not a push or a read-through.** A push from `marktd`
//! after its commit cannot be retried by anything, so a makod that is down at
//! that moment keeps answering MaLo-ID identifications to counterparties from a
//! stale record. The fan-out retries a failed delivery with backoff and
//! dead-letters it after its budget (`marktd` `/admin/fanout/dlq`), so the
//! handler answers `503` whenever the read fails, and the redelivery does the
//! rest. A read-through on every lookup would instead put `marktd`'s
//! availability inside every answer makod gives a counterparty, which is what
//! the cache exists to avoid.
//!
//! **Startup backfill.** A fresh makod (or one whose fan-out deliveries ran out
//! while it was away) reads `marktd`'s whole MaLo list before `/health/ready`
//! answers `200`. The subscription is registered first, so a change during the
//! backfill is delivered afterwards; and a record the webhook has already
//! applied at a newer `version` is not overwritten by an older backfill page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use energy_api::models::electricity::MaloIdentResultPositive;
use mako_markt::repository::Rollenzuordnung;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use time::Date;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::malo_cache::SlateDbMaloCache;
use crate::party_registry::MpIdRegistry;

/// The path `marktd` posts the fan-out to.
pub const WEBHOOK_PATH: &str = "/webhooks/marktd";

/// The subscriber id makod registers under.
pub const SUBSCRIBER_ID: &str = "makod";

/// The fan-out event types makod subscribes to. `marktd` has no MaLo delete
/// endpoint; a record that disappears answers `404` to the read, and the entry
/// is dropped.
pub const EVENT_TYPES: &[&str] = &[mako_events::markt::MALO_UPDATED];

/// Backfill page size — `marktd` caps `size` at 500.
const PAGE_SIZE: u32 = 500;

/// Why a record could not be brought into the cache.
#[derive(Debug)]
pub enum SyncError {
    /// `marktd` was unreachable or answered a non-success status.
    Marktd(String),
    /// The cache write failed.
    Cache(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Marktd(e) => write!(f, "marktd: {e}"),
            Self::Cache(e) => write!(f, "cache: {e}"),
        }
    }
}

impl std::error::Error for SyncError {}

// ── marktd wire shapes ────────────────────────────────────────────────────────

/// The fields of `marktd`'s `MaloResponse` the cache record is built from.
///
/// A response body from another service: unknown fields are ignored, so a
/// field `marktd` adds does not break the read.
#[derive(Debug, Clone, Deserialize)]
pub struct MarktdMalo {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// `STROM` / `GAS`.
    pub sparte: String,
    /// Optimistic-lock version of the record.
    pub version: i64,
    /// BO4E `Energierichtung` — `EINSP` feeds the grid, `AUSSP` draws from it.
    #[serde(default)]
    pub energierichtung: Option<String>,
    /// The role assignments valid on the day `marktd` answered.
    #[serde(default)]
    pub rollenzuordnung: Vec<Rollenzuordnung>,
}

/// One page of `GET /api/v1/malos`.
#[derive(Debug, Deserialize)]
struct MarktdPage {
    items: Vec<MarktdMalo>,
    total: u64,
}

/// The cache record for `rec`, as of `today`.
///
/// NB and MSB are the role codes valid `today` — the same half-open
/// `[valid_from, valid_to)` window `marktd` reads with; the latest start wins.
/// A record without an NB assignment is attributed to `own_nb`, the operator's
/// own NB MP-ID for the record's Sparte: the cache answers identifications in
/// the operator's grid.
///
/// # Errors
///
/// A description of the gap when the record names no NB and the operator holds
/// no NB MP-ID for its Sparte, or when an MP-ID or the MaLo-ID does not parse.
pub fn cache_record(
    rec: &MarktdMalo,
    own_nb: Option<&str>,
    today: Date,
) -> Result<MaloIdentResultPositive, String> {
    let current = |typ: &str, generic: &str| {
        rec.rollenzuordnung
            .iter()
            .filter(|z| {
                (z.zuordnungstyp == typ || z.zuordnungstyp == generic)
                    && z.valid_from <= today
                    && z.valid_to.is_none_or(|to| to > today)
            })
            .max_by_key(|z| z.valid_from)
            .map(|z| z.rollencodenummer.clone())
    };
    let nb = current("NB", "GNB")
        .or_else(|| own_nb.map(str::to_owned))
        .ok_or_else(|| {
            format!(
                "MaLo {} has no current NB Rollenzuordnung and the operator holds no NB \
                 MP-ID for Sparte {}",
                rec.malo_id, rec.sparte
            )
        })?;
    let msb = current("MSB", "GMSB");

    let operator = |mp_id: &str| -> Result<serde_json::Value, String> {
        let id: rubo4e::identifiers::MarktpartnerId = mp_id
            .parse()
            .map_err(|e| format!("Marktpartner-ID {mp_id:?}: {e}"))?;
        Ok(serde_json::json!({
            "marketPartnerId": id.to_string(),
            "executionTimeFrom": "2000-01-01T00:00:00Z"
        }))
    };
    // `marktd` carries neither the Messtechnikeinstufung nor the
    // Prognosegrundlage, so those two fields are fixed below, and a record
    // without an Energierichtung is entered as `consumption`.
    let energy_direction = match rec.energierichtung.as_deref() {
        Some("EINSP") => "production",
        _ => "consumption",
    };
    let mut mpo = Vec::new();
    if let Some(msb) = msb.as_deref().filter(|m| !m.is_empty()) {
        mpo.push(operator(msb)?);
    }
    let value = serde_json::json!({
        "dataMarketLocation": {
            "maloId": rec.malo_id,
            "energyDirection": energy_direction,
            "measurementTechnologyClassification": "conventionalMeasuringSystem",
            "optionalChangeForecastBasis": "notPossible",
            "dataMarketLocationProperties": [],
            "dataMarketLocationNetworkOperators": [operator(&nb)?],
            "dataMarketLocationTransmissionSystemOperators": [],
            "dataMarketLocationMeasuringPointOperators": mpo
        }
    });
    serde_json::from_value(value).map_err(|e| format!("MaLo {}: {e}", rec.malo_id))
}

// ── MaloSync ──────────────────────────────────────────────────────────────────

/// Keeps the MaLo cache current from `marktd`.
pub struct MaloSync {
    http: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    /// makod's own webhook URL, as `marktd` reaches it.
    webhook_url: String,
    /// Standard Webhooks secret `marktd` signs deliveries with. `None`
    /// accepts unsigned deliveries (development).
    webhook_secret: Option<SecretString>,
    cache: SlateDbMaloCache,
    /// Cache partition — the primary MP-ID, as the admin API uses.
    tenant: String,
    registry: Arc<MpIdRegistry>,
    /// The newest `version` the webhook applied per MaLo, so a backfill page
    /// read before a change does not overwrite it.
    applied: Mutex<HashMap<String, i64>>,
}

impl MaloSync {
    /// A sync against `marktd` at `base_url`.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one constructor, called once at startup"
    )]
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: SecretString,
        webhook_url: impl Into<String>,
        webhook_secret: Option<SecretString>,
        cache: SlateDbMaloCache,
        tenant: impl Into<String>,
        registry: Arc<MpIdRegistry>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key,
            webhook_url: webhook_url.into(),
            webhook_secret,
            cache,
            tenant: tenant.into(),
            registry,
            applied: Mutex::new(HashMap::new()),
        }
    }

    fn own_nb(&self, sparte: &str) -> Option<&str> {
        let role = if sparte.eq_ignore_ascii_case("GAS") {
            "GNB"
        } else {
            "NB"
        };
        self.registry.mp_id_for_role(role)
    }

    async fn upsert(&self, rec: &MarktdMalo) -> Result<bool, SyncError> {
        match cache_record(rec, self.own_nb(&rec.sparte), mako_fristen::heute()) {
            Ok(result) => {
                self.cache
                    .upsert(&self.tenant, &result)
                    .await
                    .map_err(|e| SyncError::Cache(e.to_string()))?;
                Ok(true)
            }
            // A record that cannot be represented stays out of the cache rather
            // than entering it with a guessed NB; an identification for it then
            // answers negatively.
            Err(reason) => {
                warn!(malo_id = %rec.malo_id, %reason, "malo sync: record not cached");
                Ok(false)
            }
        }
    }

    /// Read `malo_id` from `marktd` and upsert or drop its cache entry.
    ///
    /// # Errors
    ///
    /// [`SyncError::Marktd`] when the read fails, [`SyncError::Cache`] when
    /// the write does — both answered `503`, so the fan-out redelivers.
    pub async fn apply(&self, malo_id: &str) -> Result<(), SyncError> {
        let url = format!("{}/api/v1/malos/{malo_id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| SyncError::Marktd(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            self.cache
                .remove(&self.tenant, malo_id)
                .await
                .map_err(|e| SyncError::Cache(e.to_string()))?;
            info!(
                malo_id,
                "malo sync: record gone from marktd, cache entry dropped"
            );
            return Ok(());
        }
        let resp = resp
            .error_for_status()
            .map_err(|e| SyncError::Marktd(e.to_string()))?;
        let rec: MarktdMalo = resp
            .json()
            .await
            .map_err(|e| SyncError::Marktd(format!("GET {url}: {e}")))?;
        self.upsert(&rec).await?;
        self.applied
            .lock()
            .expect("applied map is never poisoned")
            .insert(rec.malo_id.clone(), rec.version);
        Ok(())
    }

    /// Register makod's subscription with `marktd`.
    ///
    /// # Errors
    ///
    /// [`SyncError::Marktd`] when `marktd` refuses or is unreachable. Unlike a
    /// subscription that only logs its failure, this one is retried by the
    /// caller and keeps readiness down until it is in place.
    pub async fn subscribe(&self) -> Result<(), SyncError> {
        let url = format!("{}/api/v1/subscriptions/{SUBSCRIBER_ID}", self.base_url);
        let body = serde_json::json!({
            "webhook_url": self.webhook_url,
            "webhook_secret": self.webhook_secret.as_ref().map(|s| s.expose_secret().to_owned()),
            "event_types": EVENT_TYPES,
            "active": true,
        });
        let resp = self
            .http
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::Marktd(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(SyncError::Marktd(format!(
                "PUT {url} answered {status}: {text}"
            )));
        }
        Ok(())
    }

    /// Read every MaLo from `marktd` into the cache.
    ///
    /// Returns the number of records cached.
    ///
    /// # Errors
    ///
    /// The first page read or cache write that fails.
    pub async fn backfill(&self) -> Result<usize, SyncError> {
        let mut cached = 0_usize;
        let mut page = 0_u32;
        loop {
            let url = format!("{}/api/v1/malos", self.base_url);
            let resp = self
                .http
                .get(&url)
                .query(&[("page", page), ("size", PAGE_SIZE)])
                .bearer_auth(self.api_key.expose_secret())
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| SyncError::Marktd(e.to_string()))?;
            let body: MarktdPage = resp
                .json()
                .await
                .map_err(|e| SyncError::Marktd(format!("GET {url}: {e}")))?;
            let n = body.items.len();
            for rec in &body.items {
                let newer = self
                    .applied
                    .lock()
                    .expect("applied map is never poisoned")
                    .get(&rec.malo_id)
                    .is_some_and(|v| *v > rec.version);
                if !newer && self.upsert(rec).await? {
                    cached += 1;
                }
            }
            let seen = u64::from(page + 1) * u64::from(PAGE_SIZE);
            if n < PAGE_SIZE as usize || seen >= body.total {
                return Ok(cached);
            }
            page += 1;
        }
    }

    /// Subscribe, then backfill, retrying both until they succeed or
    /// `shutdown` fires; `ready` is opened once both have.
    pub async fn run_startup(
        self: Arc<Self>,
        ready: crate::health::ReadinessGate,
        shutdown: CancellationToken,
    ) {
        let mut delay = Duration::from_secs(2);
        loop {
            let attempt = async {
                self.subscribe().await?;
                self.backfill().await
            };
            match attempt.await {
                Ok(n) => {
                    info!(
                        cached = n,
                        "malo sync: subscribed to marktd and backfilled the MaLo cache"
                    );
                    ready.open();
                    return;
                }
                Err(e) => warn!(
                    error = %e,
                    retry_in_secs = delay.as_secs(),
                    "malo sync: marktd subscription or backfill failed; /health/ready stays 503"
                ),
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(Duration::from_secs(60));
        }
    }
}

// ── Webhook ───────────────────────────────────────────────────────────────────

/// `POST /webhooks/marktd` — the fan-out delivery endpoint.
///
/// Authenticated by the Standard Webhooks signature `marktd` sends, not by a
/// bearer token: the fan-out holds a signing secret per subscriber and no
/// makod credential.
pub fn router(sync: Arc<MaloSync>) -> Router {
    Router::new()
        .route(WEBHOOK_PATH, post(handle_webhook))
        .with_state(sync)
}

async fn handle_webhook(
    State(sync): State<Arc<MaloSync>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let secret = sync
        .webhook_secret
        .as_ref()
        .map(|s| s.expose_secret().as_bytes().to_vec());
    // Idempotent by construction — every delivery re-reads the current record
    // — so the webhook id needs no deduplication store.
    if let Err(status) =
        mako_service::webhook::verify_request_only(secret.as_deref(), &headers, &body)
    {
        warn!(%status, "malo sync: inbound webhook refused");
        return status.into_response();
    }
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "malo sync: delivery is not JSON");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let ce_type = event["type"].as_str().unwrap_or_default();
    if !EVENT_TYPES.contains(&ce_type) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let malo_id = event["marktmaloid"]
        .as_str()
        .or_else(|| event["subject"].as_str())
        .unwrap_or_default();
    if malo_id.parse::<rubo4e::identifiers::MaloId>().is_err() {
        warn!(malo_id, "malo sync: delivery names no valid MaLo-ID");
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    match sync.apply(malo_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!(malo_id, error = %e, "malo sync: record not applied; the fan-out redelivers");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rz(typ: &str, code: &str, from: Date, to: Option<Date>) -> Rollenzuordnung {
        Rollenzuordnung {
            zuordnungstyp: typ.to_owned(),
            rollencodenummer: code.to_owned(),
            valid_from: from,
            valid_to: to,
        }
    }

    fn malo(rollen: Vec<Rollenzuordnung>) -> MarktdMalo {
        MarktdMalo {
            malo_id: "51238696012".to_owned(),
            sparte: "STROM".to_owned(),
            version: 1,
            energierichtung: Some("EINSP".to_owned()),
            rollenzuordnung: rollen,
        }
    }

    #[expect(non_snake_case, reason = "reads as a date literal at the call sites")]
    fn D(y: i32, d: u8) -> Date {
        Date::from_calendar_date(y, time::Month::January, d).expect("valid date")
    }

    #[test]
    fn the_current_msb_wins_over_an_expired_one() {
        let today = D(2026, 15);
        let rec = malo(vec![
            rz("NB", "9900357000004", D(2020, 1), None),
            rz("MSB", "9900001000001", D(2020, 1), Some(D(2026, 10))),
            rz("MSB", "9900001000002", D(2026, 10), None),
        ]);
        let r = cache_record(&rec, None, today).expect("record");
        let msb = r
            .data_market_location
            .data_market_location_measuring_point_operators
            .expect("msb");
        assert_eq!(msb[0].market_partner_id.to_string(), "9900001000002");
        assert_eq!(
            serde_json::to_value(r.data_market_location.energy_direction).expect("json"),
            "production"
        );
    }

    #[test]
    fn a_record_without_an_nb_takes_the_operators_own() {
        let rec = malo(vec![]);
        let r = cache_record(&rec, Some("9900357000004"), D(2026, 15)).expect("record");
        assert_eq!(
            r.data_market_location
                .data_market_location_network_operators[0]
                .market_partner_id
                .to_string(),
            "9900357000004"
        );
        assert!(cache_record(&rec, None, D(2026, 15)).is_err());
    }
}
