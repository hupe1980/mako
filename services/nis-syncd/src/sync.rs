//! NIS sync logic — compares incoming NIS entries against `marktd` and pushes updates.

use mako_markt::marktd_client::MarktdClient;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

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
    /// Energy commodity: `"STROM"` or `"GAS"`.
    pub sparte: String,
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
    /// Number of MaLo entries that differed (drift detail).
    pub drift_count: usize,
}

/// Execute a sync pass.
///
/// In `dry_run` mode the function compares incoming data against `marktd` but
/// never calls `PUT /api/v1/malo/{id}/grid`.  This is useful for validating
/// a NIS export before committing it.
///
/// When `drift_webhook_url` is `Some` and `drift_detected == true`, a
/// `de.markt.grid.drift.detected` CloudEvent 1.0 is POSTed to that URL so
/// downstream consumers (e.g. `obsd` alerting) can react.
pub async fn run_sync(
    client: &MarktdClient,
    nb_mp_id: &str,
    entries: &[NisEntry],
    dry_run: bool,
    drift_webhook_url: Option<&str>,
) -> SyncReport {
    let mut report = SyncReport::default();

    for entry in entries {
        // Check current state to detect drift (skip if identical, avoid spurious writes).
        let current = match client.get_malo_grid(&entry.malo_id).await {
            Ok(v) => v,
            Err(e) => {
                report
                    .errors
                    .push(format!("GET malo_grid for {} failed: {}", entry.malo_id, e));
                None
            }
        };

        let needs_update = match &current {
            None => true,
            Some(rec) => {
                rec.bilanzierungsgebiet != entry.bilanzierungsgebiet
                    || rec.netzgebiet != entry.netzgebiet
                    || rec.sparte.to_string() != entry.sparte
                    || rec.nb_mp_id != nb_mp_id
            }
        };

        if needs_update {
            report.drift_detected = true;
            report.drift_count += 1;
        }

        if dry_run || !needs_update {
            report.skipped += 1;
            continue;
        }

        match client
            .put_malo_grid(
                &entry.malo_id,
                nb_mp_id,
                entry.bilanzierungsgebiet.as_deref(),
                entry.netzgebiet.as_deref(),
                &entry.sparte,
                "nis",
            )
            .await
        {
            Ok(()) => report.updated += 1,
            Err(e) => {
                report
                    .errors
                    .push(format!("PUT malo_grid for {} failed: {}", entry.malo_id, e));
            }
        }
    }

    // Emit de.markt.grid.drift.detected CloudEvent when drift found and configured.
    if report.drift_detected
        && !dry_run
        && let Some(webhook_url) = drift_webhook_url
    {
        emit_drift_event(webhook_url, nb_mp_id, &report).await;
    }

    report
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
        "time":            OffsetDateTime::now_utc().to_string(),
        "datacontenttype": "application/json",
        "data": {
            "nb_mp_id":    nb_mp_id,
            "drift_count": report.drift_count,
            "updated":     report.updated,
            "error_count": report.errors.len(),
        }
    });

    let client = reqwest::Client::new();
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
