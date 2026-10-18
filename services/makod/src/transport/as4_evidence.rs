//! The AS4 evidence makod keeps: one `asx::AuditRecord` per exchange leg.
//!
//! An outbound delivery stores the partner's verified receipt evidence under
//! its outbox id; an inbound message stores its verified evidence (signer,
//! signed references) under the local MP-ID and the record id. The records are
//! the non-repudiation trail of the market communication and are kept with the
//! rest of the § 147 AO record in the same store; nothing purges them.

use mako_engine::error::EngineError;
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};

/// The key space of AS4 evidence records.
const AS4_EVIDENCE: KvNamespace = KvNamespace::new("as4ev/");

/// Durable AS4 evidence, in the deployment's SlateDB.
#[derive(Clone)]
pub struct As4Evidence {
    store: SlateDbStore,
}

impl As4Evidence {
    /// Evidence kept in `store`.
    #[must_use]
    pub fn new(store: SlateDbStore) -> Self {
        Self { store }
    }

    /// Keep the receipt evidence of the delivery of outbox entry `outbox_id`.
    ///
    /// # Errors
    ///
    /// A storage failure; the caller keeps the outbox entry so the resend —
    /// answered as a duplicate with a fresh receipt — records it again.
    pub async fn record_outbound(
        &self,
        outbox_id: &str,
        record: &asx::AuditRecord,
    ) -> Result<(), EngineError> {
        self.put(&format!("out/{outbox_id}"), record).await
    }

    /// Keep the evidence of an inbound message received by `local_mp_id`.
    ///
    /// # Errors
    ///
    /// A storage failure.
    pub async fn record_inbound(
        &self,
        local_mp_id: &str,
        record: &asx::AuditRecord,
    ) -> Result<(), EngineError> {
        self.put(&format!("in/{local_mp_id}/{}", record.record_id()), record)
            .await
    }

    async fn put(&self, suffix: &str, record: &asx::AuditRecord) -> Result<(), EngineError> {
        let bytes = serde_json::to_vec(record)
            .map_err(|e| EngineError::Serialization(format!("AS4 evidence: {e}")))?;
        self.store.kv_put(AS4_EVIDENCE, suffix, &bytes).await
    }
}

/// Reading the evidence back — the integration tests and an operator's export
/// do, through the library target; the daemon only writes it.
#[allow(
    dead_code,
    reason = "read through the library target; the binary target only writes evidence"
)]
impl As4Evidence {
    /// The stored outbound evidence of `outbox_id`.
    ///
    /// # Errors
    ///
    /// A storage failure, or a record that no longer decodes.
    pub async fn outbound(&self, outbox_id: &str) -> Result<Option<asx::AuditRecord>, EngineError> {
        self.get(&format!("out/{outbox_id}")).await
    }

    /// Every stored inbound record of `local_mp_id`.
    ///
    /// # Errors
    ///
    /// A storage failure, or a record that no longer decodes.
    pub async fn inbound(&self, local_mp_id: &str) -> Result<Vec<asx::AuditRecord>, EngineError> {
        let prefix = format!("in/{local_mp_id}/");
        self.store
            .kv_scan_prefix(AS4_EVIDENCE)
            .await?
            .into_iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| {
                serde_json::from_slice(&v)
                    .map_err(|e| EngineError::Serialization(format!("AS4 evidence: {e}")))
            })
            .collect()
    }

    async fn get(&self, suffix: &str) -> Result<Option<asx::AuditRecord>, EngineError> {
        self.store
            .kv_get(AS4_EVIDENCE, suffix)
            .await?
            .map(|v| {
                serde_json::from_slice(&v)
                    .map_err(|e| EngineError::Serialization(format!("AS4 evidence: {e}")))
            })
            .transpose()
    }
}
