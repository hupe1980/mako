//! Typed HTTP client for the `marktd` data hub API.
//!
//! # Endpoints
//!
//! | Method | Path | Returns |
//! |--------|------|---------|
//! | `GET` | `/api/v1/versorgung/{malo_id}` | `Option<VersorgungsStatusRecord>` |
//! | `GET` | `/api/v1/malo/{malo_id}` | `Option<MaloTypedFields>` |
//! | `GET` | `/api/v1/malo/{malo_id}/grid` | `Option<MaloGridRecord>` |
//! | `GET` | `/api/v1/partners/{mp_id}` | `bool` (partner known) |
//! | `GET` | `/api/v1/preisblaetter/{nb_mp_id}?date=…` | `Option<PreisblattNetznutzung>` |
//! | `GET` | `/api/v1/preisblaetter-messung/{msb_mp_id}?date=…` | `Option<PreisblattMessung>` |
//! | `PUT` | `/api/v1/subscriptions/{id}` | `()` (idempotent registration) |
//!
//! # Resilience
//!
//! The preisblatt endpoint includes a **circuit breaker** (3 failures → 30-second open)
//! and a **1-hour TTL cache** to prevent thundering-herd on `marktd` under load.
//!
//! All other endpoints use the standard 30-second request timeout via the
//! shared `reqwest::Client`.
//!
//! # Feature gate
//!
//! This module is only compiled with `features = ["marktd-client"]`.

use std::collections::HashMap;

use rubo4e::current::{PreisblattMessung, PreisblattNetznutzung};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::repository::{
    MaloGridRecord, MaloTypedFields, PreisblattDienstleistungRecord, PreisblattHardwareRecord,
    PreisblattKaRecord, VersorgungsStatusRecord,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// How long a successfully fetched Preisblatt is kept in the cache (1 hour).
const CACHE_TTL_SECS: i64 = 3_600;

/// Number of consecutive `marktd` failures before the circuit opens.
const CB_FAILURE_THRESHOLD: u32 = 3;

/// How long the circuit stays open before a probe is allowed through (30 s).
const CB_COOLDOWN_SECS: i64 = 30;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors returned by [`MarktdClient`] methods.
#[derive(Debug, thiserror::Error)]
pub enum MarktdClientError {
    /// Network or HTTP error (non-404 status code).
    #[error("marktd request failed: {0}")]
    Http(String),

    /// Response body could not be deserialized.
    #[error("marktd response deserialization failed: {0}")]
    Deserialization(String),
}

impl From<reqwest::Error> for MarktdClientError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e.to_string())
    }
}

// ── Subscription request body ─────────────────────────────────────────────────

/// Request body for `PUT /api/v1/subscriptions/{subscriber_id}`.
#[derive(Debug, Serialize)]
pub struct SubscriptionRequest<'a> {
    /// Public webhook URL that `marktd` will POST events to.
    pub webhook_url: &'a str,
    /// Optional HMAC-SHA256 secret `marktd` signs outbound payloads with.
    ///
    /// `None` disables signature verification for this subscription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<&'a str>,
    /// `CloudEvent` type filter (empty = wildcard, receive all events).
    pub event_types: &'a [&'a str],
    /// Optional PID filter (empty = all PIDs).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub makopid_filter: &'a [u32],
    /// Whether the subscription is active.
    pub active: bool,
}

// ── Circuit-breaker inner state ───────────────────────────────────────────────

struct CbInner {
    cache: HashMap<(String, time::Date), CacheEntry>,
    cb_failures: u32,
    cb_open_until: Option<OffsetDateTime>,
}

struct CacheEntry {
    sheet: Option<PreisblattNetznutzung>,
    expires_at: OffsetDateTime,
}

impl CbInner {
    fn is_cb_open(&self, now: OffsetDateTime) -> bool {
        self.cb_open_until.is_some_and(|t| now < t)
    }

    fn record_success(&mut self) {
        self.cb_failures = 0;
        self.cb_open_until = None;
    }

    fn record_failure(&mut self, now: OffsetDateTime) {
        self.cb_failures += 1;
        if self.cb_failures >= CB_FAILURE_THRESHOLD {
            self.cb_open_until = Some(now + time::Duration::seconds(CB_COOLDOWN_SECS));
        }
    }

    fn get_cached(&self, nb_mp_id: &str, date: time::Date) -> Option<PreisblattNetznutzung> {
        let entry = self.cache.get(&(nb_mp_id.to_owned(), date))?;
        if OffsetDateTime::now_utc() < entry.expires_at {
            entry.sheet.clone()
        } else {
            None
        }
    }

    fn set_cached(
        &mut self,
        nb_mp_id: &str,
        date: time::Date,
        sheet: Option<PreisblattNetznutzung>,
    ) {
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(CACHE_TTL_SECS);
        self.cache.insert(
            (nb_mp_id.to_owned(), date),
            CacheEntry { sheet, expires_at },
        );
    }
}

// ── MarktdClient ──────────────────────────────────────────────────────────────

/// Typed HTTP client for the `marktd` data hub APIs.
///
/// Clone is cheap — the underlying `reqwest::Client` is `Arc`-backed and the
/// circuit-breaker state is shared via `Arc<Mutex<…>>`.
#[derive(Clone)]
pub struct MarktdClient {
    client: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    /// Circuit-breaker + TTL cache for the preisblatt endpoint.
    cb: std::sync::Arc<Mutex<CbInner>>,
}

impl std::fmt::Debug for MarktdClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarktdClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl MarktdClient {
    /// Construct a new client.
    ///
    /// `base_url` — cluster-internal URL, e.g. `http://marktd:8180`.
    /// `api_key`  — Bearer token for machine-to-machine auth.
    ///
    /// The provided `reqwest::Client` should be built with the standard
    /// `mako_service::http::default_client()` timeouts (30 s request, 5 s connect).
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: SecretString,
        client: reqwest::Client,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
            cb: std::sync::Arc::new(Mutex::new(CbInner {
                cache: HashMap::new(),
                cb_failures: 0,
                cb_open_until: None,
            })),
        }
    }

    // ── Core endpoints ────────────────────────────────────────────────────────

    /// `GET /api/v1/versorgung/{malo_id}` — current `VersorgungsStatus`.
    ///
    /// Returns `None` on 404 (`MaLo` not found in `marktd`).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or non-404 HTTP errors.
    pub async fn get_versorgung(
        &self,
        malo_id: &str,
    ) -> Result<Option<VersorgungsStatusRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/versorgung/{}", self.base_url, malo_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/malo/{malo_id}` — typed Marktlokation fields.
    ///
    /// Returns the key typed fields extracted from `Marktlokation` JSONB
    /// (`netzebene`, `bilanzierungsgebiet`, `gasqualitaet`).
    ///
    /// `processd` NB check 4 uses `bilanzierungsgebiet` as primary source;
    /// falls back to `get_malo_grid` only when this returns `None`.
    ///
    /// Returns `None` on 404 (`MaLo` not registered in `marktd`).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or non-404 HTTP errors.
    pub async fn get_malo(
        &self,
        malo_id: &str,
    ) -> Result<Option<MaloTypedFields>, MarktdClientError> {
        let url = format!("{}/api/v1/malo/{}", self.base_url, malo_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json::<MaloTypedFields>()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/malo/{malo_id}/grid` — NB grid topology record.    ///
    /// Returns `None` on 404 (no grid record for this `MaLo`).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or non-404 HTTP errors.
    pub async fn get_malo_grid(
        &self,
        malo_id: &str,
    ) -> Result<Option<MaloGridRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/malo/{}/grid", self.base_url, malo_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `PUT /api/v1/malo/{malo_id}/grid` — upsert the NB grid topology record for a `MaLo`.
    ///
    /// Called by `nis-syncd` to push NIS/GIS data into `marktd`.  Idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or non-2xx HTTP errors.
    pub async fn put_malo_grid(
        &self,
        malo_id: &str,
        nb_mp_id: &str,
        bilanzierungsgebiet: Option<&str>,
        netzgebiet: Option<&str>,
        sparte: &str,
        source: &str,
    ) -> Result<(), MarktdClientError> {
        let url = format!("{}/api/v1/malo/{}/grid", self.base_url, malo_id);
        let body = serde_json::json!({
            "nb_mp_id": nb_mp_id,
            "bilanzierungsgebiet": bilanzierungsgebiet,
            "netzgebiet": netzgebiet,
            "sparte": sparte,
            "source": source,
        });
        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            warn!(malo_id, status = %resp.status(), "put_malo_grid: HTTP error");
            return Err(MarktdClientError::Http(format!(
                "HTTP {}",
                resp.status().as_u16()
            )));
        }
        Ok(())
    }

    /// `GET /api/v1/partners/{mp_id}` — returns `true` if the partner is registered.
    ///
    /// A 200 response means the partner exists; 404 means unknown.
    /// Any other HTTP status is treated as a network error.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network errors or unexpected status codes.
    pub async fn partner_known(&self, mp_id: &str) -> Result<bool, MarktdClientError> {
        let url = format!("{}/api/v1/partners/{}", self.base_url, mp_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        Ok(resp.status().is_success())
    }

    /// `GET /api/v1/preisblaetter/{nb_mp_id}?date={billing_date}` — Preisblatt.
    ///
    /// Returns `None` when:
    /// - 404 (no Preisblatt registered for this NB + date), **or**
    /// - the circuit breaker is open (degrades gracefully — structural checks proceed)
    ///
    /// Responses are cached for `CACHE_TTL_SECS` (1 hour). After three consecutive
    /// failures the circuit opens for `CB_COOLDOWN_SECS` (30 seconds).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on non-404 HTTP errors when the circuit is closed.
    pub async fn get_preisblatt(
        &self,
        nb_mp_id: &str,
        billing_date: time::Date,
    ) -> Result<Option<PreisblattNetznutzung>, MarktdClientError> {
        let now = OffsetDateTime::now_utc();
        let inner = self.cb.lock().await;

        // Serve from cache if available.
        if inner
            .cache
            .contains_key(&(nb_mp_id.to_owned(), billing_date))
        {
            let cached = inner.get_cached(nb_mp_id, billing_date);
            return Ok(cached);
        }

        // Check circuit.
        if inner.is_cb_open(now) {
            warn!(
                nb_mp_id,
                %billing_date,
                "MarktdClient: circuit open — degrading to structural checks only"
            );
            return Ok(None);
        }
        drop(inner); // Release mutex before async HTTP call.

        let date_str = billing_date.to_string(); // "YYYY-MM-DD"
        let url = format!("{}/api/v1/preisblaetter/{}", self.base_url, nb_mp_id);
        let result = self
            .client
            .get(&url)
            .query(&[("date", &date_str)])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await;

        let mut inner = self.cb.lock().await;
        match result {
            Err(e) => {
                inner.record_failure(now);
                warn!(%e, nb_mp_id, "MarktdClient: preisblatt fetch failed");
                Err(MarktdClientError::Http(e.to_string()))
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                inner.record_success();
                inner.set_cached(nb_mp_id, billing_date, None);
                Ok(None)
            }
            Ok(resp) if !resp.status().is_success() => {
                inner.record_failure(now);
                let status = resp.status().as_u16();
                warn!(
                    nb_mp_id,
                    status, "MarktdClient: preisblatt returned non-2xx"
                );
                Err(MarktdClientError::Http(format!("HTTP {status}")))
            }
            Ok(resp) => match resp.json::<PreisblattNetznutzung>().await {
                Ok(sheet) => {
                    inner.record_success();
                    inner.set_cached(nb_mp_id, billing_date, Some(sheet.clone()));
                    Ok(Some(sheet))
                }
                Err(e) => {
                    inner.record_failure(now);
                    Err(MarktdClientError::Deserialization(e.to_string()))
                }
            },
        }
    }

    /// `GET /api/v1/preisblaetter-messung/{msb_mp_id}?date={billing_date}` — MSB Preisblatt.
    ///
    /// Returns the `PreisblattMessung` for the MSB valid on `billing_date`, or `None` on 404.
    /// Used by `invoicd` for PID 31009 tariff checks (positions 4+5).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on non-404 HTTP errors.
    pub async fn get_preisblatt_messung(
        &self,
        msb_mp_id: &str,
        billing_date: time::Date,
    ) -> Result<Option<PreisblattMessung>, MarktdClientError> {
        let date_str = billing_date.to_string();
        let url = format!(
            "{}/api/v1/preisblaetter-messung/{}",
            self.base_url, msb_mp_id
        );
        let resp = self
            .client
            .get(&url)
            .query(&[("date", &date_str)])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            warn!(
                msb_mp_id,
                status, "MarktdClient: preisblatt-messung returned non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {status}")));
        }
        let sheet = resp
            .json::<PreisblattMessung>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(Some(sheet))
    }

    /// `GET /api/v1/preisblaetter-ka/{nb_mp_id}?date=…&sparte=STROM&kundengruppe=Tarifkunden`
    ///
    /// Returns the `PreisblattKonzessionsabgabe` valid on `billing_date`.
    /// Used by `netzbilanzd` for KA tariff positions in INVOIC 31001/31002 (§17 `StromNZV`).
    ///
    /// Returns `None` on 404.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on non-404 HTTP errors.
    pub async fn get_preisblatt_ka(
        &self,
        nb_mp_id: &str,
        billing_date: time::Date,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
    ) -> Result<Option<PreisblattKaRecord>, MarktdClientError> {
        let date_str = billing_date.to_string();
        let mut query = vec![("date", date_str.as_str()), ("sparte", sparte)];
        let kg;
        if let Some(kg_str) = kundengruppe_ka {
            kg = kg_str.to_owned();
            query.push(("kundengruppe", kg.as_str()));
        }
        let url = format!("{}/api/v1/preisblaetter-ka/{}", self.base_url, nb_mp_id);
        let resp = self
            .client
            .get(&url)
            .query(&query)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                nb_mp_id,
                status = s,
                "MarktdClient: preisblatt-ka returned non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        resp.json::<PreisblattKaRecord>()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/preisblaetter-dienstleistung/{msb_mp_id}?date=…` — MSB service price sheet.
    pub async fn get_preisblatt_dienstleistung(
        &self,
        msb_mp_id: &str,
        billing_date: time::Date,
    ) -> Result<Option<PreisblattDienstleistungRecord>, MarktdClientError> {
        let date_str = billing_date.to_string();
        let url = format!(
            "{}/api/v1/preisblaetter-dienstleistung/{}",
            self.base_url, msb_mp_id
        );
        let resp = self
            .client
            .get(&url)
            .query(&[("date", &date_str)])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                msb_mp_id,
                status = s,
                "MarktdClient: preisblatt-dienstleistung non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        resp.json::<PreisblattDienstleistungRecord>()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/preisblaetter-hardware/{msb_mp_id}?date=…` — MSB hardware rental price sheet.
    pub async fn get_preisblatt_hardware(
        &self,
        msb_mp_id: &str,
        billing_date: time::Date,
    ) -> Result<Option<PreisblattHardwareRecord>, MarktdClientError> {
        let date_str = billing_date.to_string();
        let url = format!(
            "{}/api/v1/preisblaetter-hardware/{}",
            self.base_url, msb_mp_id
        );
        let resp = self
            .client
            .get(&url)
            .query(&[("date", &date_str)])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                msb_mp_id,
                status = s,
                "MarktdClient: preisblatt-hardware non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        resp.json::<PreisblattHardwareRecord>()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/partners/{mp_id}/as4-address` — AS4 endpoint list (B2 `Marktteilnehmer.makoadresse`).
    pub async fn get_as4_address(
        &self,
        mp_id: &str,
    ) -> Result<Option<Vec<String>>, MarktdClientError> {
        let url = format!("{}/api/v1/partners/{}/as4-address", self.base_url, mp_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(MarktdClientError::Http(format!(
                "HTTP {}",
                resp.status().as_u16()
            )));
        }
        let body = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        let addrs = body["makoadresse"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(addrs))
    }

    /// Fetch the full `Lokationszuordnung` graph reachable from `root_id`.
    ///
    /// Pass `at_date` as `"YYYY-MM-DD"` for point-in-time queries; `None`
    /// returns all edges regardless of validity.
    pub async fn get_lokationen(
        &self,
        root_id: &str,
        root_typ: &str,
        at_date: Option<&str>,
    ) -> Result<Vec<crate::repository::LokationszuordnungEdge>, MarktdClientError> {
        let path = match root_typ {
            "melo" => format!("{}/api/v1/melos/{}/lokationen", self.base_url, root_id),
            _ => format!("{}/api/v1/malo/{}/lokationen", self.base_url, root_id),
        };
        let mut req = self
            .client
            .get(&path)
            .bearer_auth(self.api_key.expose_secret());
        if let Some(d) = at_date {
            req = req.query(&[("at", d)]);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            warn!(root_id, root_typ, status = %resp.status(), "get_lokationen: HTTP error");
            return Err(MarktdClientError::Http(format!(
                "HTTP {}",
                resp.status().as_u16()
            )));
        }
        resp.json::<Vec<crate::repository::LokationszuordnungEdge>>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// Fetch a `TechnischeRessource` record by `TrId` from `marktd`.
    ///
    /// Returns `None` if the resource is not registered yet.
    pub async fn get_technische_ressource(
        &self,
        tr_id: &str,
    ) -> Result<Option<crate::repository::TechnischeRessourceRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/technische-ressourcen/{}", self.base_url, tr_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            warn!(tr_id, status = %resp.status(), "get_technische_ressource: HTTP error");
            return Err(MarktdClientError::Http(format!(
                "HTTP {}",
                resp.status().as_u16()
            )));
        }
        let record = resp
            .json::<crate::repository::TechnischeRessourceRecord>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(Some(record))
    }

    /// `GET /api/v1/steuerbare-ressourcen/{sr_id}`
    ///
    /// Returns the full JSONB payload for a `SteuerbareRessource`.
    /// Used by `processd` N5 (§14a `Steuerungsauftrag` auto-ORDRSP) to check
    /// `istFernschaltbar` before auto-confirming a control command.
    pub async fn get_steuerbare_ressource(
        &self,
        sr_id: &str,
    ) -> Result<Option<serde_json::Value>, MarktdClientError> {
        let url = format!("{}/api/v1/steuerbare-ressourcen/{}", self.base_url, sr_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            warn!(sr_id, status = %resp.status(), "get_steuerbare_ressource: HTTP error");
            return Err(MarktdClientError::Http(format!(
                "HTTP {}",
                resp.status().as_u16()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(Some(body))
    }

    ///
    /// This is an idempotent upsert — safe to call on every service restart.
    /// Non-2xx responses are logged as warnings but do **not** return an error
    /// so that startup proceeds even when `marktd` is temporarily unavailable.
    pub async fn put_subscription(&self, subscriber_id: &str, req: &SubscriptionRequest<'_>) {
        let url = format!("{}/api/v1/subscriptions/{}", self.base_url, subscriber_id);
        let body = serde_json::json!({
            "webhook_url":    req.webhook_url,
            "webhook_secret": req.webhook_secret,
            "roles":          [],
            "event_types":    req.event_types,
            "makopid_filter": req.makopid_filter,
            "active":         req.active,
        });

        match self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!(subscriber_id, "MarktdClient: subscription registered");
            }
            Ok(resp) => {
                warn!(
                    subscriber_id,
                    status = resp.status().as_u16(),
                    "MarktdClient: subscription registration returned non-2xx"
                );
            }
            Err(e) => {
                warn!(%e, subscriber_id, "MarktdClient: subscription registration failed");
            }
        }
    }
    /// Fetch Gas MMM Abrechnungspreise for a billing month from `marktd`.
    ///
    /// Returns `None` if no prices have been imported yet for that month.
    /// `netzbilanzd` calls this before each Gas MMM billing run (INVOIC 31007/31008)
    /// to avoid requiring manual ERP input per run.
    pub async fn get_mmma_gas(
        &self,
        year: i32,
        month: u8,
        marktgebiet: &str,
    ) -> Result<Option<crate::repository::MmmaPreisGasRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/mmma-preise/gas/{year}/{month}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[("marktgebiet", marktgebiet)])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                year,
                month,
                status = s,
                "MarktdClient: mmma-gas returned non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        resp.json::<crate::repository::MmmaPreisGasRecord>()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// Fetch Strom MMM Ausgleichsenergie prices for a billing month + ÜNB.
    ///
    /// Returns `None` if no prices have been imported for that month/ÜNB.
    pub async fn get_mmm_strom(
        &self,
        year: i32,
        month: u8,
        unb_mp_id: &str,
    ) -> Result<Option<crate::repository::MmmPreisStromRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/mmm-preise/strom/{year}/{month}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[("unb_mp_id", unb_mp_id)])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                year,
                month,
                unb_mp_id,
                status = s,
                "MarktdClient: mmm-strom returned non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        resp.json::<crate::repository::MmmPreisStromRecord>()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte`
    ///
    /// Returns the contracted `Konfigurationsprodukte` for a `SteuerbareRessource`.
    /// Used by `makod` M1 guard to verify that the requested `produkt_code` is
    /// in the list before dispatching a positive ORDRSP `bestaetigen`.
    ///
    /// Returns `None` on 404 (SR not found in `marktd`).
    /// Returns an empty `Vec` when the SR has no contracted products.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or non-404 HTTP errors.
    pub async fn get_konfigurationsprodukte(
        &self,
        sr_id: &str,
    ) -> Result<Option<Vec<serde_json::Value>>, MarktdClientError> {
        let url = format!(
            "{}/api/v1/steuerbare-ressourcen/{}/konfigurationsprodukte",
            self.base_url, sr_id
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        // Response: { "sr_id": "...", "konfigurationsprodukte": [...] }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        let products = body
            .get("konfigurationsprodukte")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(Some(products))
    }

    /// `PATCH /api/v1/melos/{melo_id}/standorteigenschaften`
    ///
    /// Auto-populates `Standorteigenschaften` on a `Messlokation` from `WiM` Stammdaten
    /// (PIDs 17102–17133).  Called by `makod` when a `StammdatenUebermittelt` event
    /// is received.
    ///
    /// Fields accepted: `regelzone` (EIC code → ÜNB for Redispatch 2.0 routing),
    /// `bilanzierungsgebiet`, `netzgebiet`, `gasqualitaet`, `druckstufe` (Gas),
    /// plus the full `eigenschaftenStrom` / `eigenschaftenGas` BO4E arrays.
    ///
    /// Idempotent — safe to call on every Stammdaten receipt.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on non-2xx HTTP errors.
    pub async fn patch_melo_standorteigenschaften(
        &self,
        melo_id: &str,
        standorteigenschaften: &serde_json::Value,
    ) -> Result<(), MarktdClientError> {
        let url = format!(
            "{}/api/v1/melos/{}/standorteigenschaften",
            self.base_url, melo_id
        );
        let resp = self
            .client
            .patch(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(standorteigenschaften)
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                melo_id,
                status = s,
                "MarktdClient: patch standorteigenschaften non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        info!(
            melo_id,
            "MarktdClient: standorteigenschaften updated from WiM Stammdaten"
        );
        Ok(())
    }

    /// `GET /api/v1/melos/{melo_id}/standorteigenschaften`
    ///
    /// Returns the typed `Standorteigenschaften` for a `Messlokation`, or `None` on 404.
    pub async fn get_melo_standorteigenschaften(
        &self,
        melo_id: &str,
    ) -> Result<Option<serde_json::Value>, MarktdClientError> {
        let url = format!(
            "{}/api/v1/melos/{}/standorteigenschaften",
            self.base_url, melo_id
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        let body = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(Some(body))
    }
}
