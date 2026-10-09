//! Background worker that syncs API-Webdienste Strom endpoint URLs from the
//! BDEW Verzeichnisdienst into the local [`SlateDbPartnerStore`].
//!
//! ## Overview
//!
//! The BDEW Verzeichnisdienst is the central registry where market partners
//! publish the base URLs of their API-Webdienste Strom servers. Instead of
//! relying on static operator-managed partner-URL maps, `makod` can discover
//! endpoint URLs automatically by querying the Verzeichnisdienst.
//!
//! This worker:
//!
//! 1. Accepts a list of partner MP-IDs to track (from the `PartnerStore` or from
//!    static `--maloid-partner` seeds).
//! 2. At startup and then on a configurable refresh interval, queries
//!    [`DirectoryServiceClient::get_record`] for each MP-ID with
//!    `api_id = "maloIdV1"` and `major_version = 1`.
//! 3. On success, upserts the discovered base URL into the [`SlateDbPartnerStore`]
//!    as a `CommunicationChannel` with qualifier `"AW"` (API-Webdienste).
//! 4. On `Error::NotFound` the entry is left untouched (partner may not yet be
//!    registered). On `Error::Redirect` the redirect URL is followed once.
//!
//! ## Discovery on demand
//!
//! In addition to the background refresh, [`VerzeichnisdienstLookup`] provides
//! an on-demand `lookup_malo_ident_url` method used by `MaloIdentSender` to
//! resolve an LF's callback URL at delivery time. Results are written back to
//! the partner store for future cache hits.
//!
//! ## Feature gate
//!
//! This module depends on [`energy_api::directory::DirectoryServiceClient`],
//! which requires the `client` feature of the `energy-api` crate.  That
//! feature is already enabled in `makod/Cargo.toml`.

use std::time::Duration;

use energy_api::directory::DirectoryServiceClient;
use energy_api::error::Error as ApiError;
use mako_engine::ids::TenantId;
use mako_engine::partner::{CommunicationChannel, PartnerRecord, PartnerStore as _};
use mako_engine::store_slatedb::SlateDbPartnerStore;
use mako_engine::types::MarktpartnerCode;
use reqwest::Url;
use tracing::{debug, info, warn};

use crate::preflight::is_secure_endpoint;

/// The BDEW API-Identifier for the MaLo-ID callback service.
///
/// Used as `api_id` in every Verzeichnisdienst lookup for MaLo-ID URLs.
pub const MALO_IDENT_API_ID: &str = "maloIdV1";

/// Major version of the MaLo-ID API.
pub const MALO_IDENT_MAJOR_VERSION: i32 = 1;

/// On-demand Verzeichnisdienst lookup helper.
///
/// Wraps a [`DirectoryServiceClient`] and a [`SlateDbPartnerStore`] to provide
/// cached endpoint URL resolution for `MaloIdentSender`.
///
/// Resolution order:
///
/// 1. Check the partner store for an existing `"AW"` channel (previously
///    discovered or configured via static seed).
/// 2. On cache miss, query the Verzeichnisdienst.
/// 3. On successful lookup, persist the URL to the partner store and return it.
#[derive(Clone)]
pub struct VerzeichnisdienstLookup {
    client: DirectoryServiceClient,
    partner_store: SlateDbPartnerStore,
    tenant_id: TenantId,
}

impl VerzeichnisdienstLookup {
    /// Create a new lookup helper.
    #[must_use]
    pub fn new(
        client: DirectoryServiceClient,
        partner_store: SlateDbPartnerStore,
        tenant_id: TenantId,
    ) -> Self {
        Self {
            client,
            partner_store,
            tenant_id,
        }
    }

    /// Resolve the MaLo-ID base URL for the given LF MP-ID.
    ///
    /// Checks the partner store first.  On a miss, queries the Verzeichnisdienst
    /// and caches the result.  Returns `None` when the partner is not registered
    /// in the Verzeichnisdienst or the lookup fails.
    ///
    /// # Errors
    ///
    /// Returns [`mako_engine::error::EngineError`] on storage errors.
    pub async fn lookup_malo_ident_url(
        &self,
        mp_id: &str,
    ) -> Result<Option<Url>, mako_engine::error::EngineError> {
        let mp_id_code = MarktpartnerCode::new(mp_id);

        // 1. Check the partner store for a cached AW channel.
        if let Some(record) = self.partner_store.get(self.tenant_id, &mp_id_code).await?
            && let Some(url_str) = record.api_webdienste_endpoint()
        {
            match Url::parse(url_str) {
                Ok(url) if is_secure_endpoint(&url) => {
                    debug!(mp_id, url = %url, "Verzeichnisdienst: cache hit in partner store");
                    return Ok(Some(url));
                }
                // A record written before the scheme guard existed, or seeded
                // by an operator through `PUT /admin/partners/{mp_id}`, is held
                // to the same rule as a freshly discovered one.
                Ok(url) => {
                    warn!(
                        mp_id,
                        url = %url,
                        "Verzeichnisdienst: stored API-Webdienste endpoint is not HTTPS — \
                         ignoring it and re-fetching",
                    );
                }
                Err(e) => {
                    warn!(mp_id, url = url_str, error = %e, "Verzeichnisdienst: stored URL is invalid — re-fetching");
                }
            }
        }

        // 2. Query the Verzeichnisdienst.
        self.fetch_from_directory(mp_id, &mp_id_code).await
    }

    /// Re-resolve `mp_id` from the Verzeichnisdienst, bypassing the partner
    /// store, and store what it answers.
    ///
    /// The refresh path: the on-demand lookup answers from the store, so only
    /// this notices that a partner republished its endpoint.
    ///
    /// # Errors
    ///
    /// As [`lookup_malo_ident_url`](Self::lookup_malo_ident_url).
    pub async fn refresh_malo_ident_url(
        &self,
        mp_id: &str,
    ) -> Result<Option<Url>, mako_engine::error::EngineError> {
        self.fetch_from_directory(mp_id, &MarktpartnerCode::new(mp_id))
            .await
    }

    /// Ask the Verzeichnisdienst and persist its answer to the partner store.
    async fn fetch_from_directory(
        &self,
        mp_id: &str,
        mp_id_code: &MarktpartnerCode,
    ) -> Result<Option<Url>, mako_engine::error::EngineError> {
        match self
            .client
            .get_record(mp_id, MALO_IDENT_API_ID, MALO_IDENT_MAJOR_VERSION)
            .await
        {
            Ok((record, _cert, _sig)) => {
                let api_url = record.url.clone();
                info!(
                    mp_id,
                    url = %api_url,
                    "Verzeichnisdienst: discovered MaLo-ID endpoint — persisting to partner store"
                );

                // 3. Persist to partner store so future lookups are instant.
                self.persist_api_webdienste_url(mp_id, mp_id_code, api_url.as_str())
                    .await;

                Ok(Some(api_url))
            }
            Err(ApiError::NotFound) => {
                debug!(
                    mp_id,
                    "Verzeichnisdienst: partner not registered (NotFound)"
                );
                Ok(None)
            }
            Err(ApiError::Redirect { url }) => {
                // Follow a single redirect level: re-issue using the redirect URL.
                info!(mp_id, redirect_to = %url, "Verzeichnisdienst: received 307 redirect — following");
                match Url::parse(url.as_ref()) {
                    Ok(redirected_url) => {
                        // Store the redirect target URL as the partner's API-Webdienste endpoint.
                        self.persist_api_webdienste_url(mp_id, mp_id_code, redirected_url.as_str())
                            .await;
                        Ok(Some(redirected_url))
                    }
                    Err(e) => {
                        warn!(mp_id, redirect = %url, error = %e, "Verzeichnisdienst: redirect URL is invalid");
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                warn!(mp_id, error = %e, "Verzeichnisdienst: lookup failed — using no URL");
                Ok(None)
            }
        }
    }

    /// Upsert the `"AW"` channel into the partner store for the given MP-ID.
    async fn persist_api_webdienste_url(
        &self,
        mp_id: &str,
        mp_id_code: &MarktpartnerCode,
        url: &str,
    ) {
        // Both the direct answer and the followed redirect land here, so this
        // is the one place the transport requirement has to hold.
        match Url::parse(url) {
            Ok(parsed) if is_secure_endpoint(&parsed) => {}
            Ok(parsed) => {
                warn!(
                    mp_id,
                    url = %parsed,
                    scheme = parsed.scheme(),
                    "Verzeichnisdienst: refusing a non-HTTPS API-Webdienste endpoint. \
                     A MaLo-ID callback carries the Marktlokation's postal address and \
                     its NB/MSB assignment; it is not sent in the clear. Ask the \
                     partner to publish an https:// URL.",
                );
                return;
            }
            Err(e) => {
                warn!(mp_id, url, error = %e, "Verzeichnisdienst: discovered URL is unparseable");
                return;
            }
        }
        let record = match self.partner_store.get(self.tenant_id, mp_id_code).await {
            Ok(Some(mut existing)) => {
                // Update or insert the AW channel.
                if let Some(ch) = existing
                    .channels
                    .iter_mut()
                    .find(|c| c.qualifier.as_ref() == "AW")
                {
                    ch.address = url.into();
                } else {
                    existing
                        .channels
                        .push(CommunicationChannel::api_webdienste(url));
                }
                existing.updated_at = time::OffsetDateTime::now_utc();
                existing
            }
            Ok(None) => {
                // Create a minimal record if this partner is not yet in the store.
                PartnerRecord {
                    mp_id: mp_id_code.clone(),
                    display_name: None,
                    channels: vec![CommunicationChannel::api_webdienste(url)],
                    roles: Vec::new(),
                    valid_from: None,
                    contacts: Vec::new(),
                    country_code: None,
                    updated_at: time::OffsetDateTime::now_utc(),
                }
            }
            Err(e) => {
                warn!(mp_id, error = %e, "Verzeichnisdienst: failed to read partner record before upsert");
                return;
            }
        };

        if let Err(e) = self.partner_store.upsert(self.tenant_id, &record).await {
            warn!(mp_id, error = %e, "Verzeichnisdienst: failed to persist discovered URL to partner store");
        }
    }
}

/// Periodic background refresh task.
///
/// Iterates all partner MP-IDs registered in the store and re-fetches their
/// Verzeichnisdienst entries on a configurable interval, past the cache, so an
/// endpoint a partner republishes replaces the stored one.
///
/// Spawn with [`tokio::spawn`]:
///
/// ```rust,ignore
/// let lookup = VerzeichnisdienstLookup::new(client, partner_store, tenant_id);
/// tokio::spawn(verzeichnisdienst_refresh_task(
///     lookup,
///     Duration::from_secs(300),
///     shutdown_token,
/// ));
/// ```
pub async fn verzeichnisdienst_refresh_task(
    lookup: VerzeichnisdienstLookup,
    interval: Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = shutdown.cancelled() => {
                debug!("Verzeichnisdienst refresh: shutdown signalled; stopping");
                return;
            }
        }

        refresh_cycle(&lookup).await;
    }
}

/// One refresh cycle over every partner in the store.
///
/// Each partner is re-resolved from the directory rather than through the
/// on-demand lookup's cache, so a republished endpoint is in use within one
/// cycle; the directory is asked once per partner per cycle, not per request.
async fn refresh_cycle(lookup: &VerzeichnisdienstLookup) {
    let partners = match lookup.partner_store.list(lookup.tenant_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Verzeichnisdienst refresh: failed to list partners");
            return;
        }
    };

    let count = partners.len();
    debug!(
        count,
        "Verzeichnisdienst refresh: refreshing {} partner(s)", count
    );

    for partner in partners {
        let mp_id = partner.mp_id.as_str().to_owned();
        if let Err(e) = lookup.refresh_malo_ident_url(&mp_id).await {
            warn!(mp_id, error = %e, "Verzeichnisdienst refresh: lookup error");
        }
    }

    debug!("Verzeichnisdienst refresh: cycle complete");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod endpoint_scheme_tests {
    use crate::preflight::is_secure_endpoint;
    use reqwest::Url;

    fn ok(u: &str) -> bool {
        is_secure_endpoint(&Url::parse(u).expect("test URL parses"))
    }

    /// A MaLo-ID callback carries the Marktlokation's postal address and its
    /// NB/MSB assignment. `preflight` already refuses a configured partner on
    /// plain `http://`; a URL discovered from the Verzeichnisdienst is held to
    /// the same rule, because the directory says who a partner is, not how
    /// their data may travel.
    #[test]
    fn a_plaintext_endpoint_is_refused() {
        assert!(!ok("http://lf.example/maloId/"));
        assert!(!ok("http://10.0.0.5:8080/maloId/"));
    }

    #[test]
    fn an_https_endpoint_is_accepted() {
        assert!(ok("https://lf.example/maloId/"));
        assert!(ok("https://lf.example:8443/maloId/v1/"));
    }

    /// The same development exemption `preflight` makes for the OIDC issuer, so
    /// a local stub is still reachable without an operator disabling the rule.
    #[test]
    fn localhost_is_exempt() {
        assert!(ok("http://localhost:8090/maloId/"));
    }

    /// Anything that is neither — a `file://` or a directory entry pointing at
    /// something exotic — is refused rather than quietly persisted.
    #[test]
    fn a_non_http_scheme_is_refused() {
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("ftp://lf.example/maloId/"));
    }
}

#[cfg(test)]
mod refresh_tests {
    //! A republished partner URL is picked up by the next refresh cycle.
    //!
    //! The on-demand lookup answers from the partner store first, which is
    //! what makes it cheap; the refresh exists to notice that the directory's
    //! answer changed. A refresh that went through the same cache would only
    //! ever re-read what it stored, and `makod` would keep calling the URL a
    //! partner abandoned until an operator noticed.

    use super::*;

    const MP_ID: &str = "9900001000001";

    /// A Verzeichnisdienst answering every record lookup with `url`.
    async fn directory(url: &'static str) -> Url {
        let record = serde_json::json!({
            "providerId": MP_ID,
            "apiId": MALO_IDENT_API_ID,
            "majorVersion": MALO_IDENT_MAJOR_VERSION,
            "url": url,
            "lastUpdated": "2026-10-01T00:00:00Z",
            "revision": 2,
            "status": "Online",
        });
        let app = axum::Router::new().fallback(move || {
            let record = record.clone();
            async move { axum::Json(record) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        Url::parse(&format!("http://{addr}/")).expect("url")
    }

    #[tokio::test]
    async fn a_refresh_cycle_replaces_a_republished_url() {
        let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("store");
        let partners = store.as_partner_store();
        let tenant = TenantId::from_party_id("9900357000004");
        partners
            .upsert(
                tenant,
                &PartnerRecord {
                    mp_id: MarktpartnerCode::new(MP_ID),
                    display_name: None,
                    channels: vec![CommunicationChannel::api_webdienste(
                        "https://old.partner.example/maloId/",
                    )],
                    roles: Vec::new(),
                    valid_from: None,
                    contacts: Vec::new(),
                    country_code: None,
                    updated_at: time::OffsetDateTime::now_utc(),
                },
            )
            .await
            .expect("seed");

        let base = directory("https://new.partner.example/maloId/").await;
        let client = DirectoryServiceClient::new(base, reqwest::Client::new());
        let lookup = VerzeichnisdienstLookup::new(client, partners.clone(), tenant);

        refresh_cycle(&lookup).await;

        let stored = partners
            .get(tenant, &MarktpartnerCode::new(MP_ID))
            .await
            .expect("read")
            .expect("record");
        assert_eq!(
            stored.api_webdienste_endpoint(),
            Some("https://new.partner.example/maloId/"),
            "one refresh cycle must replace the URL the directory no longer publishes"
        );
    }
}
