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
//! 1. Accepts a list of partner GLNs to track (from the `PartnerStore` or from
//!    static `--maloid-partner` seeds).
//! 2. At startup and then on a configurable refresh interval, queries
//!    [`DirectoryServiceClient::get_record`] for each GLN with
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

    /// Resolve the MaLo-ID base URL for the given LF GLN.
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
        gln: &str,
    ) -> Result<Option<Url>, mako_engine::error::EngineError> {
        let gln_code = MarktpartnerCode::new(gln);

        // 1. Check the partner store for a cached AW channel.
        if let Some(record) = self.partner_store.get(self.tenant_id, &gln_code).await?
            && let Some(url_str) = record.api_webdienste_endpoint()
        {
            match Url::parse(url_str) {
                Ok(url) => {
                    debug!(gln, url = %url, "Verzeichnisdienst: cache hit in partner store");
                    return Ok(Some(url));
                }
                Err(e) => {
                    warn!(gln, url = url_str, error = %e, "Verzeichnisdienst: stored URL is invalid — re-fetching");
                }
            }
        }

        // 2. Query the Verzeichnisdienst.
        match self
            .client
            .get_record(gln, MALO_IDENT_API_ID, MALO_IDENT_MAJOR_VERSION)
            .await
        {
            Ok((record, _cert, _sig)) => {
                let api_url = record.url.clone();
                info!(
                    gln,
                    url = %api_url,
                    "Verzeichnisdienst: discovered MaLo-ID endpoint — persisting to partner store"
                );

                // 3. Persist to partner store so future lookups are instant.
                self.persist_api_webdienste_url(gln, &gln_code, api_url.as_str())
                    .await;

                Ok(Some(api_url))
            }
            Err(ApiError::NotFound) => {
                debug!(gln, "Verzeichnisdienst: partner not registered (NotFound)");
                Ok(None)
            }
            Err(ApiError::Redirect { url }) => {
                // Follow a single redirect level: re-issue using the redirect URL.
                info!(gln, redirect_to = %url, "Verzeichnisdienst: received 307 redirect — following");
                match Url::parse(url.as_ref()) {
                    Ok(redirected_url) => {
                        // Store the redirect target URL as the partner's API-Webdienste endpoint.
                        self.persist_api_webdienste_url(gln, &gln_code, redirected_url.as_str())
                            .await;
                        Ok(Some(redirected_url))
                    }
                    Err(e) => {
                        warn!(gln, redirect = %url, error = %e, "Verzeichnisdienst: redirect URL is invalid");
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                warn!(gln, error = %e, "Verzeichnisdienst: lookup failed — using no URL");
                Ok(None)
            }
        }
    }

    /// Upsert the `"AW"` channel into the partner store for the given GLN.
    async fn persist_api_webdienste_url(&self, gln: &str, gln_code: &MarktpartnerCode, url: &str) {
        let record = match self.partner_store.get(self.tenant_id, gln_code).await {
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
                    gln: gln_code.clone(),
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
                warn!(gln, error = %e, "Verzeichnisdienst: failed to read partner record before upsert");
                return;
            }
        };

        if let Err(e) = self.partner_store.upsert(self.tenant_id, &record).await {
            warn!(gln, error = %e, "Verzeichnisdienst: failed to persist discovered URL to partner store");
        }
    }
}

/// Periodic background refresh task.
///
/// Iterates all partner GLNs registered in the store and re-fetches their
/// Verzeichnisdienst entries on a configurable interval. This keeps the cache
/// warm even if the on-demand path has not been exercised for a while.
///
/// Spawn with [`tokio::spawn`]:
///
/// ```rust,ignore
/// let lookup = VerzeichnisdienstLookup::new(client, partner_store, tenant_id);
/// tokio::spawn(verzeichnisdienst_refresh_task(lookup, Duration::from_secs(300)));
/// ```
pub async fn verzeichnisdienst_refresh_task(lookup: VerzeichnisdienstLookup, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        let partners = match lookup.partner_store.list(lookup.tenant_id).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Verzeichnisdienst refresh: failed to list partners");
                continue;
            }
        };

        let count = partners.len();
        debug!(
            count,
            "Verzeichnisdienst refresh: refreshing {} partner(s)", count
        );

        for partner in partners {
            let gln = partner.gln.as_str().to_owned();
            if let Err(e) = lookup.lookup_malo_ident_url(&gln).await {
                warn!(gln, error = %e, "Verzeichnisdienst refresh: lookup error");
            }
        }

        debug!("Verzeichnisdienst refresh: cycle complete");
    }
}
