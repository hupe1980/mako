//! NIS sync logic — compares incoming NIS entries against `marktd` and pushes updates.

use std::sync::Arc;

use mako_markt::{domain::Sparte, marktd_client::MarktdClient};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{RwLock, Semaphore};
use tokio::task::JoinSet;
use tracing::warn;
use uuid::Uuid;

// ── Public types ──────────────────────────────────────────────────────────────

/// One MaLo entry from a NIS/GIS export.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NisEntry {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Bilanzierungsgebiet-EIC (e.g. `"10YDE-VE-------2"`).
    ///
    /// The key field for `processd` NB check 4.  `None` if the NIS does not carry
    /// this value for the given MaLo (uncommon; set when known).
    pub bilanzierungsgebiet: Option<String>,
    /// NB-internal Netzgebiet code (optional).
    pub netzgebiet: Option<String>,
    /// Energy commodity — `STROM` or `GAS`.
    ///
    /// Uses `mako_markt::domain::Sparte`, which is validated on deserialisation:
    /// any value other than `"STROM"` or `"GAS"` is rejected with a 422 error.
    pub sparte: Sparte,
}

impl NisEntry {
    /// Validate that fields conform to the expected format.
    ///
    /// Returns `Err(String)` with a description of the first validation failure.
    ///
    /// # Rules
    /// - `malo_id` must be exactly 11 ASCII digits.
    /// - `bilanzierungsgebiet`, when `Some`, must be non-empty.
    pub fn validate(&self) -> Result<(), String> {
        if self.malo_id.len() != 11 || !self.malo_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "malo_id '{}' must be an 11-digit numeric string",
                self.malo_id
            ));
        }
        if self.bilanzierungsgebiet.as_deref() == Some("") {
            return Err(
                "bilanzierungsgebiet must not be an empty string when present; use null to omit"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Summary of one sync pass.
#[derive(Debug, Default, Serialize, Clone)]
pub struct SyncReport {
    /// Number of records pushed to `marktd` (upsert returned success).
    pub updated: usize,
    /// Number of records already matching — skipped (dry-run counts all as skipped).
    pub skipped: usize,
    /// Records that failed to push (network error or rejected by `marktd`).
    pub errors: Vec<String>,
    /// `true` when at least one record differed from the current `marktd` state.
    pub drift_detected: bool,
    /// Number of MaLo entries that differed from `marktd` (drift detail).
    pub drift_count: usize,
}

/// Shared cache of the most recent sync report.
///
/// `None` until the first sync completes.  Both the HTTP handler and the MCP
/// server hold a clone of the same `Arc` so LLM tools can introspect the last
/// sync result without triggering a new sync.
pub type LastSyncReport = Arc<RwLock<Option<SyncReport>>>;

// ── Internal per-entry result ─────────────────────────────────────────────────

struct EntryResult {
    drifted: bool,
    outcome: Outcome,
}

enum Outcome {
    Updated,
    Skipped,
    Error(String),
}

// ── Core sync logic ───────────────────────────────────────────────────────────

/// Execute a sync pass with bounded concurrency.
///
/// Dispatches up to `concurrency` concurrent `GET`+`PUT` request pairs to
/// `marktd`.  For a typical NIS export of ~5 000 MaLos this reduces wall-clock
/// time from minutes (sequential) to seconds.
///
/// In `dry_run` mode the function compares incoming data against `marktd` but
/// never calls `PUT /api/v1/malo/{id}/grid`.  This is useful for validating
/// a NIS export before committing it.
///
/// When `drift_webhook_url` is `Some` and `drift_detected == true`, a
/// `de.markt.grid.drift.detected` CloudEvent 1.0 is POSTed to that URL so
/// downstream consumers (e.g. `agentd grid-anomaly-agent`) can react.
pub async fn run_sync(
    client: Arc<MarktdClient>,
    nb_mp_id: &str,
    entries: &[NisEntry],
    dry_run: bool,
    drift_webhook_url: Option<&str>,
    concurrency: usize,
) -> SyncReport {
    if entries.is_empty() {
        return SyncReport::default();
    }

    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let nb_mp_id_owned = nb_mp_id.to_owned();
    let mut join_set: JoinSet<EntryResult> = JoinSet::new();

    for entry in entries {
        let client = Arc::clone(&client);
        let nb = nb_mp_id_owned.clone();
        let entry = entry.clone();
        let sem = Arc::clone(&semaphore);

        join_set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            sync_entry(&client, &nb, &entry, dry_run).await
        });
    }

    let mut report = SyncReport::default();
    while let Some(task_result) = join_set.join_next().await {
        match task_result {
            Ok(r) => {
                if r.drifted {
                    report.drift_detected = true;
                    report.drift_count += 1;
                }
                match r.outcome {
                    Outcome::Updated => report.updated += 1,
                    Outcome::Skipped => report.skipped += 1,
                    Outcome::Error(e) => report.errors.push(e),
                }
            }
            Err(e) => {
                report.errors.push(format!("sync task panicked: {e}"));
            }
        }
    }

    if report.drift_detected
        && !dry_run
        && let Some(url) = drift_webhook_url
    {
        emit_drift_event(url, nb_mp_id, &report).await;
    }

    report
}

async fn sync_entry(
    client: &MarktdClient,
    nb_mp_id: &str,
    entry: &NisEntry,
    dry_run: bool,
) -> EntryResult {
    let current = match client.get_malo_grid(&entry.malo_id).await {
        Ok(v) => v,
        Err(e) => {
            return EntryResult {
                drifted: false,
                outcome: Outcome::Error(format!("GET malo_grid for {} failed: {e}", entry.malo_id)),
            };
        }
    };

    let needs_update = match &current {
        None => true,
        Some(rec) => {
            rec.bilanzierungsgebiet != entry.bilanzierungsgebiet
                || rec.netzgebiet != entry.netzgebiet
                || rec.sparte != entry.sparte
                || rec.nb_mp_id != nb_mp_id
        }
    };

    if dry_run || !needs_update {
        return EntryResult {
            drifted: needs_update,
            outcome: Outcome::Skipped,
        };
    }

    match client
        .put_malo_grid(
            &entry.malo_id,
            nb_mp_id,
            entry.bilanzierungsgebiet.as_deref(),
            entry.netzgebiet.as_deref(),
            &entry.sparte.to_string(),
            "nis",
        )
        .await
    {
        Ok(()) => EntryResult {
            drifted: true,
            outcome: Outcome::Updated,
        },
        Err(e) => EntryResult {
            drifted: true,
            outcome: Outcome::Error(format!("PUT malo_grid for {} failed: {e}", entry.malo_id)),
        },
    }
}

/// Emit a `de.markt.grid.drift.detected` CloudEvent 1.0 to the configured webhook.
///
/// Fire-and-forget — a delivery failure is logged as a warning but does
/// not affect the sync result or HTTP response.
async fn emit_drift_event(webhook_url: &str, nb_mp_id: &str, report: &SyncReport) {
    let event = serde_json::json!({
        "specversion":     "1.0",
        "id":              Uuid::new_v4().to_string(),
        "source":          format!("urn:nis-syncd:nb:{nb_mp_id}"),
        "type":            "de.markt.grid.drift.detected",
        "time":            OffsetDateTime::now_utc()
                               .format(&Rfc3339)
                               .unwrap_or_default(),
        "datacontenttype": "application/json",
        "data": {
            "nb_mp_id":    nb_mp_id,
            "drift_count": report.drift_count,
            "updated":     report.updated,
            "error_count": report.errors.len(),
        }
    });

    let client = mako_service::http::default_client();
    if let Err(e) = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .json(&event)
        .send()
        .await
    {
        warn!(
            webhook_url,
            drift_count = report.drift_count,
            error = %e,
            "nis-syncd: failed to deliver de.markt.grid.drift.detected CloudEvent (non-fatal)"
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::domain::Sparte;

    fn strom_entry(malo_id: &str) -> NisEntry {
        NisEntry {
            malo_id: malo_id.to_owned(),
            bilanzierungsgebiet: Some("10YDE-VE-------2".to_owned()),
            netzgebiet: None,
            sparte: Sparte::Strom,
        }
    }

    // ── NisEntry::validate ────────────────────────────────────────────────────

    #[test]
    fn validate_valid_strom_entry() {
        assert!(strom_entry("51238696780").validate().is_ok());
    }

    #[test]
    fn validate_valid_gas_entry_no_bilanzierung() {
        let e = NisEntry {
            malo_id: "51238696781".to_owned(),
            bilanzierungsgebiet: None,
            netzgebiet: Some("MUSTERSTADT".to_owned()),
            sparte: Sparte::Gas,
        };
        assert!(e.validate().is_ok());
    }

    #[test]
    fn validate_malo_id_too_short_fails() {
        let mut e = strom_entry("5123869678"); // 10 digits
        e.malo_id = "5123869678".to_owned();
        assert!(e.validate().is_err());
    }

    #[test]
    fn validate_malo_id_too_long_fails() {
        let mut e = strom_entry("512386967801"); // 12 digits
        e.malo_id = "512386967801".to_owned();
        assert!(e.validate().is_err());
    }

    #[test]
    fn validate_malo_id_non_numeric_fails() {
        let mut e = strom_entry("5123869678A");
        e.malo_id = "5123869678A".to_owned();
        assert!(e.validate().is_err());
    }

    #[test]
    fn validate_empty_bilanzierungsgebiet_fails() {
        let e = NisEntry {
            malo_id: "51238696780".to_owned(),
            bilanzierungsgebiet: Some(String::new()),
            netzgebiet: None,
            sparte: Sparte::Strom,
        };
        assert!(e.validate().is_err());
    }

    #[test]
    fn validate_none_bilanzierungsgebiet_ok() {
        let e = NisEntry {
            malo_id: "51238696780".to_owned(),
            bilanzierungsgebiet: None,
            netzgebiet: None,
            sparte: Sparte::Strom,
        };
        assert!(e.validate().is_ok());
    }

    // ── Sparte serde ─────────────────────────────────────────────────────────

    #[test]
    fn sparte_strom_roundtrip() {
        let json = r#"{"malo_id":"51238696780","bilanzierungsgebiet":null,"netzgebiet":null,"sparte":"STROM"}"#;
        let entry: NisEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.sparte, Sparte::Strom);
        assert!(serde_json::to_string(&entry).unwrap().contains("\"STROM\""));
    }

    #[test]
    fn sparte_gas_roundtrip() {
        let json = r#"{"malo_id":"51238696781","bilanzierungsgebiet":null,"netzgebiet":null,"sparte":"GAS"}"#;
        let entry: NisEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.sparte, Sparte::Gas);
        assert!(serde_json::to_string(&entry).unwrap().contains("\"GAS\""));
    }

    #[test]
    fn sparte_invalid_string_rejected() {
        let json = r#"{"malo_id":"51238696782","bilanzierungsgebiet":null,"netzgebiet":null,"sparte":"WAERME"}"#;
        assert!(serde_json::from_str::<NisEntry>(json).is_err());
    }

    // ── SyncReport ────────────────────────────────────────────────────────────

    #[test]
    fn sync_report_default_is_clean() {
        let r = SyncReport::default();
        assert_eq!(r.updated, 0);
        assert_eq!(r.skipped, 0);
        assert!(r.errors.is_empty());
        assert!(!r.drift_detected);
        assert_eq!(r.drift_count, 0);
    }
}
