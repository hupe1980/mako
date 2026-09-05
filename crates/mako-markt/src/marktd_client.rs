//! Typed HTTP client for the `marktd` data hub API.
//!
//! # Endpoints
//!
//! | Method | Path | Returns |
//! |--------|------|---------|
//! | `GET` | `/api/v1/versorgung/{malo_id}` | `Option<VersorgungsStatusRecord>` |
//! | `GET` | `/api/v1/malos/{malo_id}` | `Option<MaloTypedFields>` |
//! | `GET` | `/api/v1/malos/{malo_id}/grid` | `Option<MaloGridRecord>` |
//! | `GET` | `/api/v1/partners/{mp_id}` | `bool` (partner known) |
//! | `GET` | `/api/v1/preisblaetter/{nb_mp_id}?date=…` | `Option<PreisblattNetznutzung>` |
//! | `GET` | `/api/v1/preisblaetter-messung/{msb_mp_id}?date=…` | `Option<PreisblattMessung>` |
//! | `GET` | `/api/v1/energiemix/{nb_mp_id}?year=…` | `Option<NbEnergiemixRecord>` |
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
    ConsentDecision, ConsentPerspective, EsaMessproduktPreis, GrundversorgerRecord, MaloGridRecord,
    MaloTypedFields, PreisblattDienstleistungRecord, PreisblattHardwareRecord, PreisblattKaRecord,
    VersorgungsStatusRecord,
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

    /// The circuit breaker is open, so no request was made.
    ///
    /// Distinct from [`Self::Http`] because nothing was attempted, and — the
    /// reason this variant exists — distinct from `Ok(None)`, which means
    /// marktd answered **404: this Marktpartner has no such sheet on record**.
    /// Collapsing the two let a caller price an invoice as though no Preisblatt
    /// were published, and report a plausibility verdict on it, whenever marktd
    /// had merely been unreachable often enough to trip the breaker.
    #[error("marktd circuit open for {endpoint} ({mp_id}) — no request attempted")]
    CircuitOpen {
        /// The endpoint that would have been called.
        endpoint: String,
        /// The Marktpartner whose sheet was wanted.
        mp_id: String,
    },
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
    cache: HashMap<(String, time::Date), CacheEntry<PreisblattNetznutzung>>,
    cache_messung: HashMap<(String, time::Date), CacheEntry<PreisblattMessung>>,
    cb_failures: u32,
    cb_open_until: Option<OffsetDateTime>,
}

struct CacheEntry<T> {
    sheet: Option<T>,
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

    /// Look up a fresh cache entry. `None` means "no usable entry — fetch";
    /// `Some(None)` is a cached 404 miss served without hitting `marktd`.
    #[allow(clippy::option_option)] // outer = cache hit/miss, inner = cached 404
    fn cache_lookup<T: Clone>(
        map: &HashMap<(String, time::Date), CacheEntry<T>>,
        mp_id: &str,
        date: time::Date,
    ) -> Option<Option<T>> {
        map.get(&(mp_id.to_owned(), date))
            .filter(|e| OffsetDateTime::now_utc() < e.expires_at)
            .map(|e| e.sheet.clone())
    }

    fn cache_store<T>(
        map: &mut HashMap<(String, time::Date), CacheEntry<T>>,
        mp_id: &str,
        date: time::Date,
        sheet: Option<T>,
    ) {
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(CACHE_TTL_SECS);
        map.insert((mp_id.to_owned(), date), CacheEntry { sheet, expires_at });
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

/// One Zähler at a Messlokation, as much of it as a decision needs.
///
/// [`MarktdClient::list_zaehler_ids`] returns identifiers only, which is enough
/// to answer "does this `MeLo` have a meter" but not "is it an iMSys" — and the
/// § 14a / § 21 `MsbG` eligibility of an MSB-Wechsel turns on exactly that.
/// Deciding it from an identifier list means never deciding it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ZaehlerSummary {
    /// Zählernummer.
    pub zaehler_id: String,
    /// BO4E `Zaehlertyp` wire value, e.g. `INTELLIGENTES_MESSSYSTEM`,
    /// `MODERNE_MESSEINRICHTUNG`, `DREHSTROMZAEHLER`. `None` when the registry
    /// holds a meter without a classified type.
    #[serde(default)]
    pub zaehler_typ: Option<String>,
    /// The meter's registers, projected out of the BO4E `Zaehler` the response
    /// already carries.
    ///
    /// Which registers a meter has decides which Kapitel-4.6 Messprodukte it
    /// can serve — `E_0252` Prüfschritt 6 / `E_0256` Prüfschritt 9 — and that
    /// is a fact about the device, not a judgement about it. A narrow
    /// projection rather than the whole `Zaehler`: the caller needs the
    /// direction and the OBIS group, and pulling in the full BO would make
    /// every consumer of this summary depend on the BO4E schema version.
    #[serde(default, rename = "data")]
    pub daten: Option<ZaehlerDaten>,
}

/// The slice of a BO4E `Zaehler` this client reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZaehlerDaten {
    /// `Zaehler.zaehlwerke`.
    #[serde(default)]
    pub zaehlwerke: Vec<ZaehlwerkDaten>,
}

/// One register of a meter — its direction and its OBIS-Kennzahl.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZaehlwerkDaten {
    /// BO4E `Energierichtung`: `AUSSP` (Ausspeisung aus dem Netz — the
    /// customer's Verbrauch) or `EINSP` (Einspeisung — Erzeugung).
    #[serde(default)]
    pub richtung: Option<String>,
    /// `obisKennzahl`. The OBIS **C group** says what is metered: `1`/`2` are
    /// Wirkarbeit (Bezug / Lieferung), `3`–`8` the Blindarbeit registers and
    /// their quadrants.
    #[serde(default, rename = "obisKennzahl")]
    pub obis_kennzahl: Option<String>,
}

impl ZaehlwerkDaten {
    /// `true` when this register meters Ausspeisung aus dem Netz — Verbrauch.
    #[must_use]
    pub fn ist_verbrauch(&self) -> bool {
        self.richtung.as_deref() == Some("AUSSP")
    }

    /// `true` when this register meters Einspeisung ins Netz — Erzeugung.
    #[must_use]
    pub fn ist_erzeugung(&self) -> bool {
        self.richtung.as_deref() == Some("EINSP")
    }

    /// `true` when this is a Blindarbeit register.
    ///
    /// OBIS `A-B:C.D.E` groups by **C**: `1` Wirkarbeit Bezug, `2` Wirkarbeit
    /// Lieferung, `3` Blindarbeit Bezug, `4` Blindarbeit Lieferung, `5`–`8` the
    /// four quadrants. So `C >= 3` is the Blindarbeit family, and a meter
    /// without one cannot serve a Blindarbeit Messprodukt whatever its
    /// Energieflussrichtung.
    #[must_use]
    pub fn ist_blindarbeit(&self) -> bool {
        let Some(obis) = self.obis_kennzahl.as_deref() else {
            return false;
        };
        // `1-1:3.29.0` → take the field after the last `:`, then its first part.
        obis.rsplit(':')
            .next()
            .and_then(|rest| rest.split('.').next())
            .and_then(|c| c.parse::<u8>().ok())
            .is_some_and(|c| (3..=8).contains(&c))
    }
}

impl ZaehlerSummary {
    /// The BO4E wire spelling of an intelligentes Messsystem in `Zaehlertyp`.
    ///
    /// Three `s`. `Geraetetyp` spells the same concept with two
    /// (`INTELLIGENTES_MESSYSTEM`); the divergence is upstream in BO4E, so a
    /// comparison written against the wrong BO silently never matches.
    pub const IMSYS: &'static str = "INTELLIGENTES_MESSSYSTEM";

    /// `true` when this meter is an intelligentes Messsystem.
    #[must_use]
    pub fn ist_imsys(&self) -> bool {
        self.zaehler_typ.as_deref() == Some(Self::IMSYS)
    }
}

/// Whether an MSB serves a Kapitel-4.6 Messprodukt, and in which Abo mode.
///
/// The answer already folds in the dated **Pflicht** rule: a Pflichtprodukt is
/// served whatever the catalogue holds — `BNetzA` *Mitteilung Nr. 3* and
/// §34 Abs. 2 S. 2 Nr. 10 `MsbG` — so `als_abo` / `als_einmalig` come back
/// `Some(true)` for one even when nothing is on file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct EsaProduktAngebot {
    /// `true` when the product is Pflicht **on the requested date** — the
    /// Codeliste dates it, and a Vergangenheitswerte-Bestellung may reach back
    /// before the cut-over.
    pub pflicht: bool,
    /// `E_0256` Prüfschritt 4. `None` means „nothing recorded", which escalates
    /// rather than refusing a §34-mandated Zusatzleistung on a guess.
    pub als_abo: Option<bool>,
    /// `E_0256` Prüfschritt 5.
    pub als_einmalig: Option<bool>,
    /// Whether a catalogue row exists at all — distinct from the two flags,
    /// which a Pflichtprodukt answers without one.
    pub im_katalog: bool,
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
                cache_messung: HashMap::new(),
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

    /// `GET /api/v1/nb-contracts/by-malo/{malo_id}?on=…` — the Netznutzungsvertrag
    /// in force for a `MaLo` on a date.
    ///
    /// Read by the `processd` NB module for the **Netznutzer** and its type. A
    /// Selbstzahler („Netznutzer ohne All-Inklusiv-Vertrag") steps into the LF
    /// role in GPKE and is an ordinary LF on the wire; the one exception is the
    /// LF's Lieferantenwechsel-Meldungen (GPKE Teil 1, Vorbemerkung).
    ///
    /// Returns `None` when no contract is in force on that date.
    ///
    /// # Errors
    /// Transport, non-404 HTTP status, or deserialisation failure.
    pub async fn get_nb_contract_for_malo(
        &self,
        malo_id: &str,
        on: time::Date,
    ) -> Result<Option<crate::repository::NbContractView>, MarktdClientError> {
        let fmt = time::macros::format_description!("[year]-[month]-[day]");
        let url = format!(
            "{}/api/v1/nb-contracts/by-malo/{}?on={}",
            self.base_url,
            malo_id,
            on.format(fmt).unwrap_or_default(),
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
        resp.json()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/grundversorger/{nb_mp_id}?sparte=…` — the §36 Abs. 2 `EnWG`
    /// Grundversorger Feststellung for a Netzbetreiber and Sparte.
    ///
    /// Used by the `processd` `EoG` gap-closure automation to address the
    /// UTILMD 55013/44013 Zuordnung. Returns `None` on 404 (no Feststellung
    /// recorded — the gap must then be escalated to the operator).
    pub async fn get_grundversorger(
        &self,
        nb_mp_id: &str,
        sparte: crate::domain::Sparte,
    ) -> Result<Option<GrundversorgerRecord>, MarktdClientError> {
        let url = format!(
            "{}/api/v1/grundversorger/{}?sparte={}",
            self.base_url, nb_mp_id, sparte
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
        resp.json()
            .await
            .map(Some)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/malos/{malo_id}` — typed Marktlokation fields.
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
        let url = format!("{}/api/v1/malos/{}", self.base_url, malo_id);
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

    /// `GET /api/v1/melos/{melo_id}/msb?at=YYYY-MM-DD` — resolve the
    /// Messstellenbetreiber responsible for a Messlokation on a given date.
    ///
    /// Backs the `WiM` Teil 2 UC 4.1.1 historical Werteanfrage: a value request
    /// for a past period must reach the MSB that operated the Messlokation
    /// *then*, which the per-Messlokation dated MSB timeline
    /// (`melo_msb_zuordnungen`) records.
    ///
    /// Returns `Ok(None)` when no MSB assignment covers the date (marktd 404).
    ///
    /// # Errors
    /// [`MarktdClientError::Http`] on network/HTTP failure,
    /// [`MarktdClientError::Deserialization`] on a malformed response body.
    pub async fn get_melo_msb_at(
        &self,
        melo_id: &str,
        at: time::Date,
    ) -> Result<Option<String>, MarktdClientError> {
        let url = format!("{}/api/v1/melos/{}/msb?at={}", self.base_url, melo_id, at);
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
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(body
            .get("msb_mp_id")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned))
    }

    /// `GET /api/v1/esa/consent-check` — gate an ESA message.
    ///
    /// Returns a [`ConsentDecision`] for `(esa_mp_id, msb_mp_id, location_id)`.
    /// `allowed: false` is the clearing case (revoked consent or an
    /// unestablished framework agreement) the caller answers with an Ablehnung.
    ///
    /// `perspective` sets how a *missing* record reads:
    /// [`ConsentPerspective::MsbInbound`] allows it (self-assertion; `BNetzA`
    /// forbids form-based rejection); [`ConsentPerspective::EsaOutbound`] blocks
    /// it (the ESA has no lawful basis to originate the request).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or HTTP errors — the
    /// caller decides whether to fail open (the gate is defence-in-depth; the
    /// durable stop signal is the 17008 Abbestellung fired on revocation).
    pub async fn esa_consent_check(
        &self,
        esa_mp_id: &str,
        msb_mp_id: &str,
        location_id: &str,
        perspective: ConsentPerspective,
    ) -> Result<ConsentDecision, MarktdClientError> {
        let perspective = match perspective {
            ConsentPerspective::MsbInbound => "msb_inbound",
            ConsentPerspective::EsaOutbound => "esa_outbound",
        };
        let url = format!("{}/api/v1/esa/consent-check", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[
                ("esa_mp_id", esa_mp_id),
                ("msb_mp_id", msb_mp_id),
                ("location_id", location_id),
                ("perspective", perspective),
            ])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json::<ConsentDecision>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// Whether the ESA's Einwilligung is **valid** for this location, seen
    /// from the MSB.
    ///
    /// Three-valued on purpose, because `E_0256` Prüfschritt 8 is:
    ///
    /// - `Some(true)` — an active consent inside its validity window;
    /// - `Some(false)` — a record exists but is revoked or expired, which is
    ///   the `A08` Ablehnung;
    /// - `None` — no record at all. That is the ESA's self-assertion, and
    ///   `BNetzA` *Mitteilung Nr. 3* forbids rejecting on it, so it must not
    ///   collapse into `Some(false)`.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or HTTP errors — a
    /// transport failure is not evidence of absence and must not become `None`.
    pub async fn esa_consent_valid(
        &self,
        esa_mp_id: &str,
        msb_mp_id: &str,
        location_id: &str,
    ) -> Result<Option<bool>, MarktdClientError> {
        use crate::repository::ConsentCode;
        let decision = self
            .esa_consent_check(
                esa_mp_id,
                msb_mp_id,
                location_id,
                ConsentPerspective::MsbInbound,
            )
            .await?;
        Ok(match decision.code {
            ConsentCode::Active => Some(true),
            ConsentCode::Revoked => Some(false),
            // `SelfAssertion`/`NoConsent` are the missing record;
            // `FrameworkRejected` answers a *different* Prüfschritt (6) and
            // says nothing about the consent. All three leave Prüfschritt 8
            // unanswered rather than answering it `false`.
            ConsentCode::SelfAssertion
            | ConsentCode::NoConsent
            | ConsentCode::FrameworkRejected => None,
        })
    }

    /// `GET /api/v1/esa/framework/{msb}/{esa}` — whether the bilateral
    /// ESA-Rahmenvertrag is established (`E_0256` Prüfschritt 6).
    ///
    /// `true` requires both an EDI agreement on file and a certificate state
    /// that is not negative. A `404` means no agreement was ever recorded,
    /// which is `false`: UC 4.1.1 lists the vertragliche Grundlage among the
    /// Vorbedingungen the MSB must hold.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or HTTP errors.
    /// The prices an ESA accepted from an MSB, in force on `at`.
    ///
    /// **The ESA price basis for an INVOIC 31009.** `get_preisblatt_messung`
    /// returns what an MSB *publishes* toward the NB and the LF; there is no
    /// such sheet for the Kapitel-4.6 Messprodukte, because `§35 MsbG` leaves the
    /// Entgelt for a Zusatzleistung to be agreed per request. The accepted
    /// QUOTES 15003 Angebot is the agreement, and the invoice names the same
    /// Artikel-IDs back.
    ///
    /// Empty when nothing is on record — which `invoic-checker` reports as a
    /// warning and never a dispute, since a gap in mako's own records says
    /// nothing about whether the MSB billed correctly.
    ///
    /// # Errors
    ///
    /// [`MarktdClientError::Http`] on a non-404 HTTP failure.
    /// Ask an MSB's ESA Messprodukt-Katalog about one product on one date.
    ///
    /// `E_0252` Prüfschritt 2 and `E_0256` Prüfschritte 4/5 ask the one
    /// commercial question in those walks that the Codeliste cannot answer:
    /// which of the *optional* products this MSB carries. Without it every
    /// optional order escalates to an operator.
    ///
    /// `Ok(None)` when the code is not in Codeliste Kapitel 4.6 at all — a
    /// product this Marktrolle cannot order, which the ordering workflow
    /// refuses before any of this runs.
    ///
    /// # Errors
    ///
    /// Transport or decode failures. A `404` is a fact, not an error.
    pub async fn esa_messprodukt_angebot(
        &self,
        msb_mp_id: &str,
        messprodukt: &str,
        at: time::Date,
    ) -> Result<Option<EsaProduktAngebot>, MarktdClientError> {
        let url = format!(
            "{}/api/v1/esa/messprodukte/{msb_mp_id}/{messprodukt}",
            self.base_url
        );
        let resp = self
            .client
            .get(&url)
            .query(&[("at", at.to_string())])
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

    /// Which **Messprodukt** an ORDERS 17007 Belegnummer subscribed to.
    ///
    /// `edmd`'s Typ-2 delivery surveillance calls this to size its silence
    /// threshold: the *Codeliste der Konfigurationen* Kap. 4.6 publishes a
    /// delivery cadence **per product**, and an inbound MSCONS 13027 names only
    /// the Belegnummer of the ORDERS it belongs to (`SG1 RFF+AGI`).
    ///
    /// `Ok(None)` on `404` — no accepted Angebot names that Belegnummer, which
    /// is the ordinary state for a delivery whose sender omitted the Muss. The
    /// caller falls back to its configured threshold rather than inventing a
    /// cadence for a product it cannot identify.
    ///
    /// # Errors
    ///
    /// Transport or decode failures. A `404` is a fact, not an error.
    pub async fn esa_messprodukt_of_bestellung(
        &self,
        bestellung_ref: &str,
    ) -> Result<Option<String>, MarktdClientError> {
        let url = format!(
            "{}/api/v1/esa/subscriptions/{bestellung_ref}",
            self.base_url
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
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(body
            .get("messprodukt")
            .and_then(|v| v.as_str())
            .map(str::to_owned))
    }

    pub async fn esa_preise(
        &self,
        msb_mp_id: &str,
        esa_mp_id: &str,
        at: time::Date,
    ) -> Result<Vec<EsaMessproduktPreis>, MarktdClientError> {
        let url = format!(
            "{}/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}",
            self.base_url
        );
        let resp = self
            .client
            .get(&url)
            .query(&[("at", at.to_string())])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// Record the prices of an accepted Angebot.
    ///
    /// Called by `makod` when the MSB confirms the Bestellung (ORDRSP 19011) —
    /// the moment the offer becomes the agreement. Best-effort at the call
    /// site: a marktd outage must not fail a confirmed subscription, it only
    /// leaves the invoice check without its basis, which the checker reports
    /// as a warning.
    ///
    /// # Errors
    ///
    /// [`MarktdClientError::Http`] on an HTTP failure.
    pub async fn put_esa_preise(
        &self,
        msb_mp_id: &str,
        esa_mp_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), MarktdClientError> {
        let url = format!(
            "{}/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}",
            self.base_url
        );
        self.client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(body)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        Ok(())
    }

    pub async fn esa_framework_established(
        &self,
        msb_mp_id: &str,
        esa_mp_id: &str,
    ) -> Result<bool, MarktdClientError> {
        #[derive(serde::Deserialize)]
        struct Framework {
            #[serde(default)]
            edi_agreement: bool,
            #[serde(default)]
            cert_state: String,
        }
        let url = format!(
            "{}/api/v1/esa/framework/{msb_mp_id}/{esa_mp_id}",
            self.base_url
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        let f: Framework = resp
            .json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        let cert_negative = matches!(f.cert_state.as_str(), "rejected" | "revoked" | "suspended");
        Ok(f.edi_agreement && !cert_negative)
    }

    /// Whether this MSB operates **every** Messlokation of the Lokationsbündel
    /// rooted at `malo_id`, on `at` (`E_0256` Prüfschritt 11).
    ///
    /// UC 4.1.1's Vorbedingung for a `MaLo`-, Tranchen- or `NeLo`-level order is
    /// that „der Messstellenbetrieb wird an allen Messlokationen … von demselben
    /// MSB durchgeführt". `A10` refuses the order when it does not hold.
    ///
    /// `None` when the bundle cannot be answered — it carries no Messlokation,
    /// or an assignment is missing for one of them. Both mean „not established",
    /// which escalates rather than refusing: `A10` is a statement about the
    /// market, not about a gap in this deployment's records.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or HTTP errors — a
    /// transport failure must not read as a split bundle.
    pub async fn msb_serves_whole_buendel(
        &self,
        malo_id: &str,
        msb_mp_id: &str,
        at: time::Date,
    ) -> Result<Option<bool>, MarktdClientError> {
        #[derive(serde::Deserialize)]
        struct Buendel {
            #[serde(default)]
            messlokationen: Vec<String>,
        }
        let url = format!("{}/api/v1/malos/{malo_id}/buendel", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[("at", at.to_string())])
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        let buendel: Buendel = resp
            .json()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        // An empty bundle answers nothing: a MaLo-level order presupposes at
        // least one Messlokation behind it.
        if buendel.messlokationen.is_empty() {
            return Ok(None);
        }
        for melo in &buendel.messlokationen {
            match self.get_melo_msb_at(melo, at).await? {
                // One Messlokation on another MSB settles it — `A10`.
                Some(other) if other != msb_mp_id => return Ok(Some(false)),
                Some(_) => {}
                // A gap in the timeline is not evidence of a split bundle.
                None => return Ok(None),
            }
        }
        Ok(Some(true))
    }

    /// `PUT /api/v1/netzzugang/antraege` — upsert a §20b
    /// Netzzugangsplattform request in the marktd registry.
    ///
    /// Used by the makod `netzzugang` adapter to project a request when its
    /// command is accepted (`erfasst`) and after outbox delivery
    /// (`uebermittelt` / `fehlgeschlagen`).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or HTTP errors.
    pub async fn upsert_netzzugang_antrag(
        &self,
        antrag: &crate::repository::NetzzugangAntrag,
    ) -> Result<uuid::Uuid, MarktdClientError> {
        #[derive(serde::Deserialize)]
        struct IdBody {
            id: uuid::Uuid,
        }
        let url = format!("{}/api/v1/netzzugang/antraege", self.base_url);
        let resp = self
            .client
            .put(&url)
            .json(antrag)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json::<IdBody>()
            .await
            .map(|b| b.id)
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `PATCH /api/v1/netzzugang/antraege/{id}/status` — advance a §20b
    /// request's lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or HTTP errors.
    pub async fn set_netzzugang_status(
        &self,
        id: uuid::Uuid,
        status: crate::repository::NetzzugangStatus,
        platform_ref: Option<&str>,
    ) -> Result<(), MarktdClientError> {
        let url = format!("{}/api/v1/netzzugang/antraege/{id}/status", self.base_url);
        let resp = self
            .client
            .patch(&url)
            .json(&serde_json::json!({
                "status": status,
                "platform_ref": platform_ref,
            }))
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?;
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        Ok(())
    }

    /// `GET /api/v1/malos/{malo_id}/grid` — NB grid topology record.    ///
    /// Returns `None` on 404 (no grid record for this `MaLo`).
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on network or non-404 HTTP errors.
    pub async fn get_malo_grid(
        &self,
        malo_id: &str,
    ) -> Result<Option<MaloGridRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/malos/{}/grid", self.base_url, malo_id);
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

    /// `PUT /api/v1/malos/{malo_id}/grid` — upsert the NB grid topology record for a `MaLo`.
    ///
    /// NB-role provisioning of NIS/GIS grid data (manual or ERP integration).  Idempotent.
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
        let url = format!("{}/api/v1/malos/{}/grid", self.base_url, malo_id);
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
        let status = resp.status();
        if status.is_success() {
            return Ok(true);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        // Anything else is marktd failing, not the partner being absent. Callers
        // reject registrations on `false`, so collapsing the two would turn an
        // outage into a wrongful market rejection.
        warn!(mp_id, %status, "partner_known: HTTP error");
        Err(MarktdClientError::Http(format!("HTTP {}", status.as_u16())))
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
        self.get_sheet_with_cb(
            "preisblaetter",
            nb_mp_id,
            billing_date,
            |inner| CbInner::cache_lookup(&inner.cache, nb_mp_id, billing_date),
            |inner, sheet| CbInner::cache_store(&mut inner.cache, nb_mp_id, billing_date, sheet),
        )
        .await
    }

    /// `GET /api/v1/preisblaetter-messung/{msb_mp_id}?date={billing_date}` — MSB Preisblatt.
    ///
    /// Returns the `PreisblattMessung` for the MSB valid on `billing_date`, or `None` on 404.
    /// Used by `invoicd` for PID 31009 tariff checks (positions 4+5).
    ///
    /// Hardened like [`Self::get_preisblatt`]: responses (including 404 misses)
    /// are cached for `CACHE_TTL_SECS` (1 hour) and the shared circuit breaker
    /// degrades to `Ok(None)` while open.
    ///
    /// # Errors
    ///
    /// Returns [`MarktdClientError::Http`] on non-404 HTTP errors when the circuit is closed.
    pub async fn get_preisblatt_messung(
        &self,
        msb_mp_id: &str,
        billing_date: time::Date,
    ) -> Result<Option<PreisblattMessung>, MarktdClientError> {
        self.get_sheet_with_cb(
            "preisblaetter-messung",
            msb_mp_id,
            billing_date,
            |inner| CbInner::cache_lookup(&inner.cache_messung, msb_mp_id, billing_date),
            |inner, sheet| {
                CbInner::cache_store(&mut inner.cache_messung, msb_mp_id, billing_date, sheet);
            },
        )
        .await
    }

    /// Shared fetch path for the date-scoped Preisblatt endpoints: 1-hour TTL
    /// cache + circuit breaker.
    ///
    /// The breaker state is shared across sheet kinds — three consecutive
    /// `marktd` failures on either endpoint open the circuit for both, because
    /// the failure mode being guarded is "`marktd` is down", not a
    /// per-endpoint condition. While open, returns `Ok(None)` so callers
    /// degrade to structural checks instead of erroring.
    async fn get_sheet_with_cb<T, L, S>(
        &self,
        endpoint: &str,
        mp_id: &str,
        billing_date: time::Date,
        cache_lookup: L,
        cache_store: S,
    ) -> Result<Option<T>, MarktdClientError>
    where
        T: serde::de::DeserializeOwned + Clone,
        L: FnOnce(&CbInner) -> Option<Option<T>>,
        S: FnOnce(&mut CbInner, Option<T>),
    {
        let now = OffsetDateTime::now_utc();
        {
            let inner = self.cb.lock().await;

            // Serve from cache if fresh (a cached 404 miss is also served).
            if let Some(cached) = cache_lookup(&inner) {
                return Ok(cached);
            }

            // Check circuit.
            if inner.is_cb_open(now) {
                warn!(
                    mp_id,
                    %billing_date,
                    endpoint,
                    "MarktdClient: circuit open — refusing rather than reporting the sheet as absent"
                );
                // Not `Ok(None)`: that is marktd's 404, and it means the sheet
                // is genuinely not on record. A caller that cannot tell the two
                // apart checks an invoice without the Preisblatt and still
                // reports a verdict on it.
                return Err(MarktdClientError::CircuitOpen {
                    endpoint: endpoint.to_owned(),
                    mp_id: mp_id.to_owned(),
                });
            }
        } // Release mutex before the async HTTP call.

        let date_str = billing_date.to_string(); // "YYYY-MM-DD"
        let url = format!("{}/api/v1/{}/{}", self.base_url, endpoint, mp_id);
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
                warn!(%e, mp_id, endpoint, "MarktdClient: preisblatt fetch failed");
                Err(MarktdClientError::Http(e.to_string()))
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                inner.record_success();
                cache_store(&mut inner, None);
                Ok(None)
            }
            Ok(resp) if !resp.status().is_success() => {
                inner.record_failure(now);
                let status = resp.status().as_u16();
                warn!(
                    mp_id,
                    status, endpoint, "MarktdClient: preisblatt returned non-2xx"
                );
                Err(MarktdClientError::Http(format!("HTTP {status}")))
            }
            Ok(resp) => match resp.json::<T>().await {
                Ok(sheet) => {
                    inner.record_success();
                    cache_store(&mut inner, Some(sheet.clone()));
                    Ok(Some(sheet))
                }
                Err(e) => {
                    inner.record_failure(now);
                    Err(MarktdClientError::Deserialization(e.to_string()))
                }
            },
        }
    }

    /// `GET /api/v1/preisblaetter-ka/{nb_mp_id}?date=…&sparte=STROM&kundengruppe=Tarifkunden`
    ///
    /// Returns the `PreisblattKonzessionsabgabe` valid on `billing_date`.
    /// Used by `netzbilanzd` for KA tariff positions in INVOIC 31001/31002 (KAV §2).
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
            _ => format!("{}/api/v1/malos/{}/lokationen", self.base_url, root_id),
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

    /// Fetch the nationwide Strom Mehr-/Mindermengenpreise for an application
    /// month.
    ///
    /// § 13 Abs. 3 `StromNZV` makes these *einheitlich* and the BDEW publishes one
    /// series for the whole market, so the month is the whole key. Returns
    /// `None` if that month has not been imported.
    pub async fn get_mmm_strom(
        &self,
        year: i32,
        month: u8,
    ) -> Result<Option<crate::repository::MmmPreisStromRecord>, MarktdClientError> {
        let url = format!("{}/api/v1/mmm-preise/strom/{year}/{month}", self.base_url);
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
            let s = resp.status().as_u16();
            warn!(
                year,
                month,
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

    /// `GET /api/v1/melos/{melo_id}/zaehler` — Zähler registered at a `MeLo`.
    ///
    /// Returns the `zaehler_id`s (Zählernummern) only — the device registry's
    /// answer to "which meter serves this metering location". Used by billingd
    /// to put the §40 Abs. 2 Nr. 6 `EnWG` meter identity on the bill.
    pub async fn list_zaehler_ids(&self, melo_id: &str) -> Result<Vec<String>, MarktdClientError> {
        let url = format!("{}/api/v1/melos/{}/zaehler", self.base_url, melo_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        let body = resp
            .json::<Vec<serde_json::Value>>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(body
            .iter()
            .filter_map(|z| z.get("zaehler_id").and_then(|v| v.as_str()))
            .map(str::to_owned)
            .collect())
    }

    /// `GET /api/v1/melos/{melo_id}/zaehler` — the meters at a Messlokation,
    /// with their BO4E `Zaehlertyp`.
    ///
    /// Returns an empty vector when the `MeLo` has no meters *or* does not exist;
    /// use [`Self::melo_known`] when the two need to be told apart.
    ///
    /// # Errors
    ///
    /// [`MarktdClientError::Http`] on network/HTTP failure — never conflate one
    /// with an empty inventory: a transport error is not evidence of absence,
    /// and treating it as one rejects a valid § 21 `MsbG` registration.
    pub async fn list_zaehler(
        &self,
        melo_id: &str,
    ) -> Result<Vec<ZaehlerSummary>, MarktdClientError> {
        let url = format!("{}/api/v1/melos/{}/zaehler", self.base_url, melo_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        resp.json::<Vec<ZaehlerSummary>>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))
    }

    /// `GET /api/v1/melos/{melo_id}` — is this Messlokation in the registry?
    ///
    /// The `A02` „Messlokation existiert nicht" rejection ground needs the `MeLo`
    /// itself, not the `MaLo` it hangs off. Deriving it from a `MaLo` lookup puts a
    /// rejection on the market that names the wrong object.
    ///
    /// # Errors
    ///
    /// [`MarktdClientError::Http`] on network/HTTP failure. A `404` is
    /// `Ok(false)`; anything else propagates, because only a genuine absence
    /// may become an `A02`.
    pub async fn melo_known(&self, melo_id: &str) -> Result<bool, MarktdClientError> {
        let url = format!("{}/api/v1/melos/{}", self.base_url, melo_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        resp.error_for_status_ref()
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        Ok(true)
    }

    /// `PUT /api/v1/zaehler/{zaehler_id}/register`
    ///
    /// Upsert a `ZaehlzeitRegisterRecord` for a given Zähler.  Idempotent: the
    /// server uses `ON CONFLICT (zaehler_id, tenant, bezeichnung, valid_from) DO
    /// UPDATE` so repeated calls with the same business key are safe.
    ///
    /// Called by `makod` after processing an inbound ORDERS 17102–17133 that
    /// carries ZAK+ZE register definitions.
    pub async fn put_zaehler_register(
        &self,
        zaehler_id: &str,
        rec: &crate::repository::ZaehlzeitRegisterRecord,
    ) -> Result<(), MarktdClientError> {
        let url = format!("{}/api/v1/zaehler/{}/register", self.base_url, zaehler_id);
        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(rec)
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                zaehler_id,
                status = s,
                "MarktdClient: put_zaehler_register non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        info!(
            zaehler_id,
            bezeichnung = %rec.bezeichnung,
            "MarktdClient: ZaehlzeitRegister upserted from WiM Stammdaten"
        );
        Ok(())
    }

    /// `PUT /api/v1/zaehler-register/{register_id}/saisons`
    ///
    /// Upsert a `ZaehlzeitSaisonRecord` for a given register.
    pub async fn put_zaehler_saison(
        &self,
        register_id: uuid::Uuid,
        rec: &crate::repository::ZaehlzeitSaisonRecord,
    ) -> Result<(), MarktdClientError> {
        let url = format!(
            "{}/api/v1/zaehler-register/{}/saisons",
            self.base_url, register_id
        );
        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(rec)
            .send()
            .await
            .map_err(|e| MarktdClientError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            warn!(
                %register_id,
                status = s,
                "MarktdClient: put_zaehler_saison non-2xx"
            );
            return Err(MarktdClientError::Http(format!("HTTP {s}")));
        }
        Ok(())
    }

    /// `GET /api/v1/energiemix/{nb_mp_id}?year={year}`
    ///
    /// Returns the §42 EnWG annual grid-area `Energiemix` for the given NB.
    ///
    /// Used by:
    /// - `billingd` to compute Reststrommix disclosure on customer bills
    /// - `einsd` for EEG plant context and §42 Abs. 5 EnWG compliance
    /// - `productd` for Ökostrom/green-tariff labelling
    ///
    /// Returns `None` when no Energiemix has been published for this NB yet.
    /// When `year` is `None`, returns the most recent available year.
    #[allow(clippy::doc_markdown)]
    pub async fn get_nb_energiemix(
        &self,
        nb_mp_id: &str,
        year: Option<i16>,
    ) -> Result<Option<crate::repository::NbEnergiemixRecord>, MarktdClientError> {
        let mut url = format!("{}/api/v1/energiemix/{}", self.base_url, nb_mp_id);
        if let Some(y) = year {
            use std::fmt::Write as _;
            let _ = write!(url, "?year={y}");
        }
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
            .json::<crate::repository::NbEnergiemixRecord>()
            .await
            .map_err(|e| MarktdClientError::Deserialization(e.to_string()))?;
        Ok(Some(body))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn unreachable_client() -> MarktdClient {
        // TEST-NET-1 style refused endpoint: nothing listens on the discard
        // port locally, so every request fails fast with a connect error.
        MarktdClient::new(
            "http://127.0.0.1:9",
            SecretString::from("test-key"),
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_millis(200))
                .timeout(std::time::Duration::from_millis(400))
                .build()
                .expect("client"),
        )
    }

    /// An open circuit refuses; it does not answer "no sheet on record".
    ///
    /// `get_preisblatt_messung` shares the TTL/circuit-breaker hardening with
    /// `get_preisblatt`: after `CB_FAILURE_THRESHOLD` consecutive failures the
    /// circuit opens and no request is made. What it returns then matters.
    ///
    /// It used to return `Ok(None)` — the same value marktd's **404** produces,
    /// which means *this Marktpartner has no such Preisblatt published*. A
    /// caller cannot tell those apart, so `invoicd` priced an invoice as though
    /// no Preisblatt existed and still reported a plausibility verdict on it,
    /// for as long as the breaker stayed open. `invoicd` had its own copy of
    /// this bug (`.await.ok().flatten()`) and fixing it there was not enough:
    /// the breaker re-opened the hole one layer down, and every other
    /// price-sheet consumer had it too.
    #[tokio::test]
    async fn an_open_circuit_is_not_a_missing_preisblatt() {
        let client = unreachable_client();
        let date = time::Date::from_calendar_date(2026, time::Month::July, 1).expect("date");

        for _ in 0..CB_FAILURE_THRESHOLD {
            let r = client.get_preisblatt_messung("9900000000001", date).await;
            assert!(r.is_err(), "closed circuit surfaces the network error");
        }

        // Circuit is now open: refuse without a network call, and say why.
        let r = client.get_preisblatt_messung("9900000000001", date).await;
        assert!(
            matches!(&r, Err(MarktdClientError::CircuitOpen { .. })),
            "an open circuit must be distinguishable from a 404, got {r:?}"
        );

        // The breaker is shared across sheet kinds: Netznutzung refuses too.
        let r = client.get_preisblatt("9900000000001", date).await;
        assert!(
            matches!(&r, Err(MarktdClientError::CircuitOpen { .. })),
            "breaker is shared with get_preisblatt, got {r:?}"
        );
    }

    /// Serve one request with `status`, then return the bound base URL.
    async fn one_shot_server(status: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt as _;
                let _ = sock
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    fn client_for(base: &str) -> MarktdClient {
        MarktdClient::new(base, SecretString::from("test-key"), reqwest::Client::new())
    }

    /// A 404 means the partner is genuinely unregistered.
    #[tokio::test]
    async fn partner_known_reports_absent_on_404() {
        let base = one_shot_server("404 Not Found").await;
        let r = client_for(&base).partner_known("9900000000001").await;
        assert!(matches!(r, Ok(false)));
    }

    /// A 5xx means marktd is failing, not that the partner is missing. Callers
    /// reject market registrations on `Ok(false)`, so collapsing the two would
    /// turn an outage into a wrongful rejection on the wire.
    #[tokio::test]
    async fn partner_known_errors_on_server_failure() {
        let base = one_shot_server("500 Internal Server Error").await;
        let r = client_for(&base).partner_known("9900000000001").await;
        assert!(
            matches!(r, Err(MarktdClientError::Http(_))),
            "5xx must not read as an unknown partner"
        );
    }

    /// **The OBIS C group says what a register meters.** `A-B:C.D.E` — `1`
    /// Wirkarbeit Bezug, `2` Wirkarbeit Lieferung, `3`/`4` Blindarbeit, `5`–`8`
    /// the four quadrants. A meter without a `C >= 3` register cannot serve a
    /// Blindarbeit Messprodukt whatever its Energieflussrichtung, which is
    /// `E_0252` Prüfschritt 6 / `E_0256` Prüfschritt 9.
    #[test]
    fn the_obis_c_group_tells_wirkarbeit_from_blindarbeit() {
        let werk = |obis: &str, richtung: &str| ZaehlwerkDaten {
            richtung: Some(richtung.to_owned()),
            obis_kennzahl: Some(obis.to_owned()),
        };

        // Wirkarbeit — Bezug and Lieferung.
        assert!(!werk("1-1:1.29.0", "AUSSP").ist_blindarbeit());
        assert!(!werk("1-1:2.29.0", "EINSP").ist_blindarbeit());
        // Blindarbeit and its quadrants.
        for obis in ["1-1:3.29.0", "1-1:4.29.0", "1-1:5.8.0", "1-1:8.8.0"] {
            assert!(werk(obis, "AUSSP").ist_blindarbeit(), "{obis}");
        }
        // Beyond the Blindarbeit family again.
        assert!(!werk("1-1:9.8.0", "AUSSP").ist_blindarbeit());

        // Direction is its own axis: AUSSP is Ausspeisung aus dem Netz, which
        // is the customer's Verbrauch; EINSP is Erzeugung. Reading them the
        // other way round refuses every Erzeugung product at a solar site.
        assert!(werk("1-1:1.29.0", "AUSSP").ist_verbrauch());
        assert!(!werk("1-1:1.29.0", "AUSSP").ist_erzeugung());
        assert!(werk("1-1:2.29.0", "EINSP").ist_erzeugung());

        // A register with no OBIS says nothing rather than claiming Wirkarbeit.
        let ohne = ZaehlwerkDaten {
            richtung: Some("AUSSP".to_owned()),
            obis_kennzahl: None,
        };
        assert!(!ohne.ist_blindarbeit());
    }

    /// The summary reads the registers straight out of the `Zaehler` the
    /// endpoint already returns — no second round trip, and no dependency on
    /// the BO4E schema version beyond the two fields it names.
    #[test]
    fn the_summary_projects_the_registers_off_the_wire() {
        let wire = serde_json::json!({
            "zaehler_id": "1ESA0000000001",
            "zaehler_typ": "INTELLIGENTES_MESSSYSTEM",
            "data": {
                "zaehlwerke": [
                    { "obisKennzahl": "1-1:1.29.0", "richtung": "AUSSP" },
                    { "obisKennzahl": "1-1:2.29.0", "richtung": "EINSP" }
                ]
            }
        });
        let s: ZaehlerSummary = serde_json::from_value(wire).expect("summary");
        assert!(s.ist_imsys());
        let werke = &s.daten.expect("registers").zaehlwerke;
        assert_eq!(werke.len(), 2);
        assert!(werke.iter().any(ZaehlwerkDaten::ist_verbrauch));
        assert!(werke.iter().any(ZaehlwerkDaten::ist_erzeugung));

        // A response without the BO — an older projection, or a meter with no
        // registers on file — deserialises rather than failing.
        let bare: ZaehlerSummary =
            serde_json::from_value(serde_json::json!({ "zaehler_id": "X" })).expect("summary");
        assert!(bare.daten.is_none());
    }
}
