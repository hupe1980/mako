//! HTTP client for fetching `PreisblattNetznutzung` from `marktd`.
//!
//! # Architecture
//!
//! `invoicd` never stores price sheets itself — it fetches them from `marktd` at
//! check time via `GET /api/v1/preisblaetter/{nb_mp_id}?date={billing_date}`.
//!
//! Two reliability mechanisms protect against `marktd` unavailability:
//!
//! ## 1 — 1-hour TTL cache
//! A `HashMap`-backed cache keyed by `(nb_mp_id, billing_date)` holds the last
//! successful fetch for 1 hour.  Subsequent checks for the same NB + date within
//! the hour are served from the cache without hitting `marktd`.
//!
//! ## 2 — Circuit breaker (CLOSED → OPEN → HALF-OPEN)
//! After 3 consecutive `marktd` failures the circuit opens for 30 seconds.
//! While open, `fetch()` returns `None` immediately (degrade to structural checks
//! only — never reject a valid invoice solely due to a missing price sheet).
//! After the cooldown, the first request is allowed through (HALF-OPEN); a
//! successful response resets the circuit; a failure reopens it.
//!
//! # Thread safety
//! Both the cache and the circuit-breaker state are protected by a single
//! `tokio::sync::Mutex`; the mutex is only held for in-memory operations, never
//! across the async HTTP await point.

use std::collections::HashMap;

use invoic_checker::tariff::InMemoryPreisblattStore;
use rubo4e::v202501::PreisblattNetznutzung;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tracing::{info, warn};

// ── Constants ─────────────────────────────────────────────────────────────────

/// How long a successfully fetched price sheet is kept in the cache (1 hour).
const CACHE_TTL_SECS: i64 = 3_600;

/// Number of consecutive `marktd` failures before the circuit opens.
const CB_FAILURE_THRESHOLD: u32 = 3;

/// How long the circuit stays open before the first probe is allowed through (30 s).
const CB_COOLDOWN_SECS: i64 = 30;

// ── Inner state (behind Mutex) ────────────────────────────────────────────────

struct Inner {
    cache: HashMap<(String, time::Date), CacheEntry>,
    cb_failures: u32,
    cb_open_until: Option<OffsetDateTime>,
}

struct CacheEntry {
    sheet: Option<PreisblattNetznutzung>,
    expires_at: OffsetDateTime,
}

impl Inner {
    fn is_cb_open(&self, now: OffsetDateTime) -> bool {
        self.cb_open_until.map(|t| now < t).unwrap_or(false)
    }

    fn record_success(&mut self) {
        self.cb_failures = 0;
        self.cb_open_until = None;
    }

    fn record_failure(&mut self, now: OffsetDateTime) {
        self.cb_failures += 1;
        if self.cb_failures >= CB_FAILURE_THRESHOLD {
            let cooldown = time::Duration::seconds(CB_COOLDOWN_SECS);
            self.cb_open_until = Some(now + cooldown);
        }
    }

    fn get_cached(
        &self,
        nb_mp_id: &str,
        billing_date: time::Date,
        now: OffsetDateTime,
    ) -> Option<&Option<PreisblattNetznutzung>> {
        let key = (nb_mp_id.to_owned(), billing_date);
        self.cache
            .get(&key)
            .filter(|e| now < e.expires_at)
            .map(|e| &e.sheet)
    }

    fn insert_cache(
        &mut self,
        nb_mp_id: &str,
        billing_date: time::Date,
        sheet: Option<PreisblattNetznutzung>,
        now: OffsetDateTime,
    ) {
        let ttl = time::Duration::seconds(CACHE_TTL_SECS);
        self.cache.insert(
            (nb_mp_id.to_owned(), billing_date),
            CacheEntry {
                sheet,
                expires_at: now + ttl,
            },
        );
    }
}

// ── Public client ─────────────────────────────────────────────────────────────

/// Async HTTP client that fetches `PreisblattNetznutzung` from `marktd`
/// with a 1-hour TTL cache and a circuit breaker.
#[derive(Clone)]
pub struct MarktdPreisblattClient {
    marktd_url: String,
    http: reqwest::Client,
    inner: std::sync::Arc<Mutex<Inner>>,
}

impl MarktdPreisblattClient {
    /// Create a new client pointing at `marktd_url`
    /// (e.g. `"http://localhost:8180"`).
    #[must_use]
    pub fn new(marktd_url: String) -> Self {
        Self {
            marktd_url,
            http: mako_service::http::default_client(),
            inner: std::sync::Arc::new(Mutex::new(Inner {
                cache: HashMap::new(),
                cb_failures: 0,
                cb_open_until: None,
            })),
        }
    }

    /// Fetch the `PreisblattNetznutzung` for `nb_mp_id` valid on `billing_date`.
    ///
    /// Returns:
    /// - `Some(sheet)` — price sheet found and valid.
    /// - `None` — no price sheet available (not an error; check engine degrades
    ///   to structural checks only).
    ///
    /// Never panics.  Network or `marktd` errors are logged as warnings and
    /// cause `None` to be returned.
    pub async fn fetch(
        &self,
        nb_mp_id: &str,
        billing_date: time::Date,
    ) -> Option<PreisblattNetznutzung> {
        let now = OffsetDateTime::now_utc();

        // ── 1. Check cache ────────────────────────────────────────────────────
        {
            let guard = self.inner.lock().await;
            if let Some(cached) = guard.get_cached(nb_mp_id, billing_date, now) {
                return cached.clone();
            }
            // ── 2. Check circuit breaker ──────────────────────────────────────
            if guard.is_cb_open(now) {
                warn!(
                    nb_mp_id,
                    %billing_date,
                    "invoicd: Preisblatt circuit breaker OPEN — degrading to structural checks"
                );
                return None;
            }
        }

        // ── 3. Fetch from marktd ────────────────────────────────────────────
        // time::Date Display outputs "YYYY-MM-DD" — the format marktd's API expects.
        let url = format!(
            "{}/api/v1/preisblaetter/{}?date={}",
            self.marktd_url, nb_mp_id, billing_date
        );
        let result = self.http.get(&url).send().await.map(|r| {
            // 404 is a valid "no data" response, not an error
            if r.status() == reqwest::StatusCode::NOT_FOUND {
                None
            } else {
                Some(r)
            }
        });

        match result {
            Err(e) => {
                warn!(nb_mp_id, %billing_date, error = %e, "invoicd: failed to reach marktd for Preisblatt");
                let mut guard = self.inner.lock().await;
                guard.record_failure(now);
                None
            }
            Ok(None) => {
                // 404 — no price sheet; cache the miss so we don't hammer marktd
                let mut guard = self.inner.lock().await;
                guard.record_success();
                guard.insert_cache(nb_mp_id, billing_date, None, now);
                None
            }
            Ok(Some(resp)) => {
                let status = resp.status();
                if !status.is_success() {
                    warn!(
                        nb_mp_id,
                        %billing_date,
                        status = status.as_u16(),
                        "invoicd: marktd returned non-2xx for Preisblatt"
                    );
                    let mut guard = self.inner.lock().await;
                    guard.record_failure(now);
                    return None;
                }
                match resp.json::<PreisblattNetznutzung>().await {
                    Ok(sheet) => {
                        info!(
                            nb_mp_id,
                            %billing_date, "invoicd: fetched Preisblatt from marktd"
                        );
                        let mut guard = self.inner.lock().await;
                        guard.record_success();
                        guard.insert_cache(nb_mp_id, billing_date, Some(sheet.clone()), now);
                        Some(sheet)
                    }
                    Err(e) => {
                        warn!(
                            nb_mp_id,
                            %billing_date,
                            error = %e,
                            "invoicd: failed to deserialize Preisblatt from marktd"
                        );
                        let mut guard = self.inner.lock().await;
                        guard.record_failure(now);
                        None
                    }
                }
            }
        }
    }

    /// Build a single-entry [`InMemoryPreisblattStore`] from a fetched sheet.
    ///
    /// This is the bridge between the async `MarktdPreisblattClient` and the
    /// synchronous `PreisblattStore` trait consumed by [`invoic_checker`].
    #[must_use]
    pub fn into_store(
        nb_mp_id: &str,
        sheet: Option<PreisblattNetznutzung>,
    ) -> InMemoryPreisblattStore {
        let mut store = InMemoryPreisblattStore::new();
        if let Some(s) = sheet {
            store.insert(nb_mp_id.to_owned(), s);
        }
        store
    }
}
