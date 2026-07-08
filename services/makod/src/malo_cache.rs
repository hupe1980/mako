//! SlateDB-backed MaLo master-data cache.
//!
//! ## Role in the architecture
//!
//! `makod` is a communication-protocol processor, not a master-data system.
//! The **ERP** (SAP IS-U, Powercloud, …) holds authoritative MaLo records.
//! This cache is a **read-side snapshot** that the ERP fills via the admin
//! API (`PUT /admin/malo/{malo_id}`) or via the `ErpCommandSource` channel.
//!
//! `makod` serves every `POST /maloId/request/v1` from its local cache
//! without calling the ERP. If the cache is stale, a negative response is
//! returned — the ERP is responsible for keeping the cache fresh.
//!
//! ## Key schema (prefix `mc/`)
//!
//! ```text
//! mc/{tenant_id}/{malo_id}                         →  JSON(MaloIdentResultPositive)
//! mc_addr/{tenant_id}/{zip}/{city}/{street}/{house} →  malo_id string
//! mc_stat/{tenant_id}/count                        →  u64 LE
//! mc_stat/{tenant_id}/last_upsert_nanos            →  i128 LE (unix timestamp)
//! mc_txres/{tenant_id}/{tx_id}                     →  JSON(MaloIdentResolved)
//! ```
//!
//! The `mc_addr/` secondary index enables address-based lookups when the
//! Lieferant does not supply a MaLo-ID.
//!
//! The `mc_txres/` index holds the resolved `(malo_id, nb_mp_id)` for each
//! completed MaLo-ID identification request, keyed by the LF-supplied `tx_id`.
//! This is the correlation bridge used by `maloid.lieferbeginn.fortsetzen`
//!: the ERP receives `MaloIdentified` via the ERP webhook, then
//! calls the command endpoint with only the `tx_id` and `lieferbeginn_datum` —
//! `makod` resolves the `malo_id` and `nb_mp_id` from this index.

use energy_api::models::electricity::{IdentificationParameter, MaloIdentResultPositive};
use energy_api::server::malo_ident::MaloRegistry;
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};
use time::OffsetDateTime;

// ── KV namespace constants ────────────────────────────────────────────────────

/// Primary MaLo record namespace: `mc/{tenant_id}/{malo_id}`.
const MC: KvNamespace = KvNamespace::new("mc/");
/// Address secondary-index namespace: `mc_addr/{tenant_id}/{zip}/{city}/{street}/{house}`.
const MC_ADDR: KvNamespace = KvNamespace::new("mc_addr/");
/// Per-tenant statistics namespace: `mc_stat/{tenant_id}/{field}`.
const MC_STAT: KvNamespace = KvNamespace::new("mc_stat/");
/// MaLo-ID identification result namespace: `mc_txres/{tenant_id}/{tx_id}`.
const MC_TXRES: KvNamespace = KvNamespace::new("mc_txres/");

// ── SlateDbMaloCache ──────────────────────────────────────────────────────────

/// MaLo master-data cache backed by the shared `SlateDbStore`.
///
/// All reads and writes use the `mc/` key prefix, which is reserved for this
/// cache and does not overlap with any `mako-engine` key space.
///
/// `Clone` is cheap — all clones share the underlying database handle.
#[derive(Clone)]
pub struct SlateDbMaloCache {
    store: SlateDbStore,
}

impl SlateDbMaloCache {
    /// Create a new cache using an existing shared store.
    #[must_use]
    pub fn new(store: SlateDbStore) -> Self {
        Self { store }
    }

    // ── Write path (admin API + ErpCommandSource) ─────────────────────────────

    /// Insert or overwrite a MaLo record.
    ///
    /// Also updates the secondary address index and the per-tenant statistics
    /// counters.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub async fn upsert(
        &self,
        tenant_id: &str,
        result: &MaloIdentResultPositive,
    ) -> Result<(), anyhow::Error> {
        let malo_id = result.data_market_location.malo_id.0.as_str();
        let suffix = malo_suffix(tenant_id, malo_id);
        let value = serde_json::to_vec(result)?;
        self.store.kv_put(MC, &suffix, &value).await?;

        // Secondary address index — only written when full address is present.
        if let Some(postal) = result
            .data_market_location
            .data_market_location_address
            .as_ref()
        {
            let addr_suffix = addr_index_suffix(
                tenant_id,
                postal.zip_code.as_deref().unwrap_or(""),
                postal.city.as_deref().unwrap_or(""),
                postal.street.as_deref().unwrap_or(""),
                postal.house_number,
            );
            self.store
                .kv_put(MC_ADDR, &addr_suffix, malo_id.as_bytes())
                .await?;
        }

        // Stats: increment count and record last-upsert timestamp.
        self.increment_count(tenant_id).await?;
        let now_nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
        self.store
            .kv_put(
                MC_STAT,
                &format!("{tenant_id}/last_upsert_nanos"),
                &now_nanos.to_le_bytes(),
            )
            .await?;

        Ok(())
    }

    /// Remove a MaLo record. Returns `true` if the record existed.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub async fn remove(&self, tenant_id: &str, malo_id: &str) -> Result<bool, anyhow::Error> {
        let suffix = malo_suffix(tenant_id, malo_id);
        let existed = self.store.kv_get(MC, &suffix).await?.is_some();
        if existed {
            self.store.kv_delete(MC, &suffix).await?;
            self.decrement_count(tenant_id).await?;
        }
        Ok(existed)
    }

    /// Retrieve a single MaLo record by its 11-digit ID.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure or JSON deserialization failure.
    pub async fn get(
        &self,
        tenant_id: &str,
        malo_id: &str,
    ) -> Result<Option<MaloIdentResultPositive>, anyhow::Error> {
        let suffix = malo_suffix(tenant_id, malo_id);
        match self.store.kv_get(MC, &suffix).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Return cache statistics for a tenant.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub async fn stats(&self, tenant_id: &str) -> Result<MaloCacheStats, anyhow::Error> {
        let count = self.read_count(tenant_id).await?;
        let last_upsert = self.read_last_upsert(tenant_id).await?;
        Ok(MaloCacheStats {
            tenant_id: tenant_id.to_owned(),
            count,
            last_upsert,
        })
    }

    /// List all tenants that have at least one MaLo record in the cache.
    ///
    /// Scans the `mc_stat/` prefix. Returns an empty vec when the cache is
    /// completely empty.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub async fn list_tenants(&self) -> Result<Vec<String>, anyhow::Error> {
        // kv_scan_prefix returns suffix-only keys (namespace prefix stripped).
        // Suffix shape: `{tenant_id}/{field}`
        let pairs = self.store.kv_scan_prefix(MC_STAT).await?;
        let mut tenants: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (suffix, _) in pairs {
            if let Some(tenant) = suffix.split('/').next() {
                tenants.insert(tenant.to_owned());
            }
        }
        Ok(tenants.into_iter().collect())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    async fn read_count(&self, tenant_id: &str) -> Result<u64, anyhow::Error> {
        Ok(
            match self
                .store
                .kv_get(MC_STAT, &format!("{tenant_id}/count"))
                .await?
            {
                Some(b) if b.len() >= 8 => u64::from_le_bytes(b[..8].try_into().unwrap()),
                _ => 0,
            },
        )
    }

    async fn increment_count(&self, tenant_id: &str) -> Result<(), anyhow::Error> {
        let count = self.read_count(tenant_id).await?;
        self.store
            .kv_put(
                MC_STAT,
                &format!("{tenant_id}/count"),
                &(count + 1).to_le_bytes(),
            )
            .await?;
        Ok(())
    }

    async fn decrement_count(&self, tenant_id: &str) -> Result<(), anyhow::Error> {
        let count = self.read_count(tenant_id).await?;
        let new_count = count.saturating_sub(1);
        self.store
            .kv_put(
                MC_STAT,
                &format!("{tenant_id}/count"),
                &new_count.to_le_bytes(),
            )
            .await?;
        Ok(())
    }

    async fn read_last_upsert(
        &self,
        tenant_id: &str,
    ) -> Result<Option<OffsetDateTime>, anyhow::Error> {
        Ok(
            match self
                .store
                .kv_get(MC_STAT, &format!("{tenant_id}/last_upsert_nanos"))
                .await?
            {
                Some(b) if b.len() >= 16 => {
                    let nanos = i128::from_le_bytes(b[..16].try_into().unwrap());
                    OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()
                }
                _ => None,
            },
        )
    }
}

// ── MaloRegistry impl ─────────────────────────────────────────────────────────

impl MaloRegistry for SlateDbMaloCache {
    async fn lookup(
        &self,
        tenant_id: &str,
        params: &IdentificationParameter,
    ) -> Result<Option<MaloIdentResultPositive>, energy_api::Error> {
        // 1. Try direct lookup by MaLo-ID (most common path).
        if let Some(id_params) = &params.identification_parameter_id
            && let Some(malo_id) = &id_params.malo_id
        {
            let suffix = malo_suffix(tenant_id, &malo_id.0);
            match self.store.kv_get(MC, &suffix).await {
                Ok(Some(bytes)) => {
                    return serde_json::from_slice(&bytes).map(Some).map_err(|e| {
                        energy_api::Error::Http {
                            status: 500,
                            body: format!("MaLo cache deserialization error: {e}"),
                        }
                    });
                }
                Ok(None) => return Ok(None),
                Err(e) => {
                    return Err(energy_api::Error::Http {
                        status: 500,
                        body: format!("MaLo cache storage error: {e}"),
                    });
                }
            }
        }

        // 2. Try address-based lookup via secondary index.
        let postal = params.identification_parameter_address.address.as_ref();
        let Some(addr_suffix) = postal.map(|p| {
            addr_index_suffix(
                tenant_id,
                p.zip_code.as_deref().unwrap_or(""),
                p.city.as_deref().unwrap_or(""),
                p.street.as_deref().unwrap_or(""),
                p.house_number,
            )
        }) else {
            return Ok(None);
        };
        match self.store.kv_get(MC_ADDR, &addr_suffix).await {
            Ok(Some(malo_id_bytes)) => {
                let malo_id =
                    std::str::from_utf8(&malo_id_bytes).map_err(|e| energy_api::Error::Http {
                        status: 500,
                        body: format!("MaLo cache index corruption: {e}"),
                    })?;
                let data_suffix = malo_suffix(tenant_id, malo_id);
                match self.store.kv_get(MC, &data_suffix).await {
                    Ok(Some(bytes)) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
                        energy_api::Error::Http {
                            status: 500,
                            body: format!("MaLo cache deserialization error: {e}"),
                        }
                    }),
                    Ok(None) => Ok(None),
                    Err(e) => Err(energy_api::Error::Http {
                        status: 500,
                        body: format!("MaLo cache storage error: {e}"),
                    }),
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(energy_api::Error::Http {
                status: 500,
                body: format!("MaLo cache storage error: {e}"),
            }),
        }
    }
}

// ── Key helpers ───────────────────────────────────────────────────────────────

/// Build the suffix (without `mc/` prefix) for a MaLo record key.
fn malo_suffix(tenant_id: &str, malo_id: &str) -> String {
    // Sanitise malo_id: replace path-unsafe chars with `_` so the suffix cannot
    // escape the `mc/` namespace via e.g. `../../e/stream`.
    let safe_id = malo_id.replace(['/', '\\', '\0'], "_");
    format!("{tenant_id}/{safe_id}")
}

/// Build the suffix (without `mc_addr/` prefix) for the address secondary-index key.
fn addr_index_suffix(
    tenant_id: &str,
    zip: &str,
    city: &str,
    street: &str,
    house_number: Option<i32>,
) -> String {
    let house = house_number.map(|h| h.to_string()).unwrap_or_default();
    // Sanitise components: replace `/` with `_` so the prefix scan is unambiguous.
    let city_s = city.replace('/', "_");
    let street_s = street.replace('/', "_");
    format!("{tenant_id}/{zip}/{city_s}/{street_s}/{house}")
}

// ── Stats ─────────────────────────────────────────────────────────────────────

/// Per-tenant statistics returned by `GET /admin/malo/stats`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MaloCacheStats {
    pub tenant_id: String,
    pub count: u64,
    pub last_upsert: Option<OffsetDateTime>,
}

// ── MaloIdentResolved / MaloIdentResultCache ──────────────────────────────────

/// Resolved MaLo-ID identification result — the correlation bridge for
/// `maloid.lieferbeginn.fortsetzen`.
///
/// Stored under key `mc_txres/{tenant_id}/{tx_id}` after a successful
/// positive MaLo-ID callback delivery. The ERP uses the `tx_id` it received
/// when calling `POST /maloId/request/v1` to look up the resolved `malo_id`
/// and `nb_mp_id` for the follow-on PID 55001 Lieferbeginn dispatch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaloIdentResolved {
    /// The `tx_id` from the original `POST /maloId/request/v1` request.
    pub tx_id: String,
    /// The resolved Marktlokations-ID (11-digit).
    pub malo_id: String,
    /// The NB's GLN (13-digit), resolved from the MaLo record at callback
    /// delivery time.
    pub nb_mp_id: String,
    /// When the positive callback was successfully delivered.
    #[serde(with = "time::serde::rfc3339")]
    pub resolved_at: OffsetDateTime,
}

/// Short-lived key-value store for `tx_id → MaloIdentResolved` mappings.
///
/// Uses the `mc_txres/` prefix in the shared SlateDB instance.
/// Entries are written by [`MaloIdentSender`] after a positive callback
/// delivery and read by `maloid.lieferbeginn.fortsetzen` in `commands_api.rs`.
///
/// There is no TTL enforcement at the storage level — entries persist until
/// explicitly removed or until the database is pruned. In practice, entries
/// are small (< 256 bytes) and the volume equals the number of MaLo-ID
/// requests, which is bounded by the number of supply-point changes.
///
/// [`MaloIdentSender`]: crate::malo_ident_sender::MaloIdentSender
#[derive(Clone)]
pub struct MaloIdentResultCache {
    store: SlateDbStore,
}

impl MaloIdentResultCache {
    /// Create a new result cache backed by the shared store.
    #[must_use]
    pub fn new(store: SlateDbStore) -> Self {
        Self { store }
    }

    /// Persist the resolved MaLo-ID identification result.
    ///
    /// Idempotent — overwrites any previous entry for the same `tx_id`.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub async fn store_result(
        &self,
        tenant_id: &str,
        result: &MaloIdentResolved,
    ) -> Result<(), anyhow::Error> {
        let suffix = txres_suffix(tenant_id, &result.tx_id);
        let data = serde_json::to_vec(result)?;
        self.store.kv_put(MC_TXRES, &suffix, &data).await?;
        Ok(())
    }

    /// Look up the resolved result for a given `tx_id`.
    ///
    /// Returns `None` if the identification request has not yet completed
    /// (callback not yet delivered) or if the `tx_id` is unknown.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub async fn get_result(
        &self,
        tenant_id: &str,
        tx_id: &str,
    ) -> Result<Option<MaloIdentResolved>, anyhow::Error> {
        let suffix = txres_suffix(tenant_id, tx_id);
        match self.store.kv_get(MC_TXRES, &suffix).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }
}

/// Build the suffix (without `mc_txres/` prefix) for a tx-result key.
fn txres_suffix(tenant_id: &str, tx_id: &str) -> String {
    // Sanitise tx_id — replace path-unsafe chars with `_` so the suffix
    // does not escape the `mc_txres/` namespace.
    let safe_tx = tx_id.replace(['/', '\\', '\0'], "_");
    format!("{tenant_id}/{safe_tx}")
}
