//! Axum webhook handler for inbound `MdmEvent` CloudEvents.
//!
//! ## Inbound event routing
//!
//! | `ce_type`                    | `data.pid`      | Action                       |
//! |------------------------------|-----------------|------------------------------|
//! | `de.mako.process.initiated`  | INVOIC PID set  | Run plausibility check       |
//! | *(anything else)*            | *(any)*         | 204 No Content (ignored)     |
//!
//! ### INVOIC PID set (GPKE billing only)
//!
//! Only PIDs whose `ProcessInitiated` outbox payload embeds a `Rechnung` BO4E
//! object are handled here.  PIDs 31003/31004/31009/31011 belong to different
//! billing workflows whose outbox does NOT embed `rechnung`, so they are
//! intentionally omitted.
//!
//! | PID   | Description                              | Crate         |
//! |-------|------------------------------------------|---------------|
//! | 31001 | MMM-Rechnung Strom, NB → LF              | mako-gpke     |
//! | 31002 | MMM-selbst ausgest. Rechnung Strom, LF   | mako-gpke     |
//! | 31005 | NNE-Rechnung Strom, NB → LF              | mako-gpke     |
//! | 31006 | NNE-selbst ausgest. Rechnung Strom, LF   | mako-gpke     |

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use invoic_checker::{CheckConfig, CheckOutcome, InvoicCheckEngine};
use mako_mdm::cloudevents::verify_signature;
use rubo4e::v202501::Rechnung;
use secrecy::{ExposeSecret, SecretString};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{makod_client::MakodClient, tariff_store::TariffStoreHandle};

/// GPKE INVOIC PIDs that `invoicd` handles.
///
/// Only the PIDs whose `GpkeAbrechnungWorkflow` outbox embeds a `Rechnung`
/// BO4E object in `ProcessInitiated`.  PIDs 31003/31004/31009/31011 belong
/// to other workflows; their outbox payloads do NOT contain `rechnung` so
/// they must never appear here.
///
/// Source: PID ownership table, BK6-24-174.
const INVOIC_PIDS: &[u32] = &[31001, 31002, 31005, 31006];

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    pub tariff_store: TariffStoreHandle,
    pub makod: MakodClient,
    pub check_config: Arc<CheckConfig>,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// When `total_net_invoic` (in EUR-cents) exceeds this value, a `Warn`
    /// outcome is escalated to a `Dispute` instead of automatic approval.
    /// `0` means `Warn` is always approved.
    pub auto_dispute_threshold_eur_cents: i64,
}

/// `POST /webhook` — receive a `MdmEvent` from `mdmd`.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── 1. Verify signature if configured ────────────────────────────────────
    if let Some(secret) = (*state.inbound_secret).as_ref() {
        let provided = headers
            .get("x-mdm-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret.expose_secret().as_bytes(), &body, provided) {
            warn!("invoicd: webhook signature mismatch");
            return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
        }
    }

    // ── 2. Parse JSON body ────────────────────────────────────────────────────
    // `MdmEvent` implements only `Serialize`; parse as a generic JSON value to
    // avoid coupling to an internal `Deserialize` impl.
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "invoicd: failed to parse MdmEvent");
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();
    let data = &event["data"];
    let pid = extract_pid(data);

    debug!(ce_type, pid, "invoicd: received event");

    // ── 3. Route by ce_type + pid ─────────────────────────────────────────────
    if ce_type == "de.mako.process.initiated" && INVOIC_PIDS.contains(&pid) {
        let subject = event["subject"].as_str().unwrap_or("").to_owned();
        handle_invoic_initiated(state, subject, data.clone()).await;
    } else {
        debug!(ce_type, pid, "invoicd: event ignored");
    }

    StatusCode::NO_CONTENT.into_response()
}

// ── INVOIC plausibility check ─────────────────────────────────────────────────

async fn handle_invoic_initiated(state: HandlerState, subject: String, data: serde_json::Value) {
    let process_id = match subject.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            warn!(
                subject,
                "invoicd: process.initiated event has no parseable UUID subject"
            );
            return;
        }
    };

    // Extract fields from the event data payload.
    let pid = extract_pid(&data);
    let sender_gln = data["sender_gln"].as_str().unwrap_or("").to_owned();
    // `invoice_ref` is the EDIFACT INVOIC message-reference used as the
    // business key in `dispatch_to_process`.  It must be forwarded to makod
    // so the command can be routed to the correct billing process.
    let invoice_ref = data["invoice_ref"].as_str().unwrap_or("").to_owned();
    if invoice_ref.is_empty() {
        warn!(
            pid,
            "invoicd: invoice_ref missing from process.initiated payload — cannot dispatch command"
        );
        return;
    }
    let rechnung_value = data["rechnung"].clone();

    // Deserialize the Rechnung BO4E object embedded by GpkeAbrechnungWorkflow.
    let rechnung: Rechnung = match serde_json::from_value(rechnung_value) {
        Ok(r) => r,
        Err(err) => {
            warn!(%err, pid, "invoicd: could not deserialize Rechnung from event payload — skipping check");
            return;
        }
    };

    // Run the stateless plausibility check.
    let report = {
        let store = state.tariff_store.0.read().await;
        InvoicCheckEngine::check(pid, &sender_gln, &rechnung, &*store, &state.check_config)
    };

    info!(
        process_id = %process_id,
        pid,
        outcome = ?report.outcome,
        line_items = report.line_items_checked,
        findings = report.findings.len(),
        "invoicd: INVOIC check complete"
    );

    // Decide action based on outcome.
    let should_dispute = match report.outcome {
        CheckOutcome::Ok => false,
        CheckOutcome::Warn => {
            // Escalate if the invoice total exceeds the configured threshold.
            state.auto_dispute_threshold_eur_cents > 0
                && report
                    .total_net_invoic
                    .map(|t| t.0 > state.auto_dispute_threshold_eur_cents)
                    .unwrap_or(false)
        }
        CheckOutcome::Dispute => true,
    };

    if should_dispute {
        let reason = dispute_reason(&report.findings);
        warn!(
            process_id = %process_id,
            pid,
            reason = %reason,
            "invoicd: disputing invoice"
        );
        if let Err(err) = state
            .makod
            .dispute_invoice(process_id, &invoice_ref, &reason)
            .await
        {
            warn!(%err, process_id = %process_id, "invoicd: failed to submit dispute command");
        }
    } else {
        info!(process_id = %process_id, pid, invoice_ref = %invoice_ref, "invoicd: approving invoice");
        if let Err(err) = state.makod.settle_invoice(process_id, &invoice_ref).await {
            warn!(%err, process_id = %process_id, "invoicd: failed to submit settle command");
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_pid(data: &serde_json::Value) -> u32 {
    data["pid"].as_u64().unwrap_or(0) as u32
}

/// Build a human-readable dispute reason from plausibility check findings.
///
/// Falls back to a generic message when no individual findings triggered a
/// `Dispute` outcome (e.g. the invoice was escalated solely by the
/// auto-dispute monetary threshold).
fn dispute_reason(findings: &[invoic_checker::Finding]) -> String {
    let specific: Vec<&str> = findings
        .iter()
        .filter(|f| f.is_dispute)
        .map(|f| f.message.as_str())
        .collect();
    if specific.is_empty() {
        "Automatische Ablehnung: Rechnungsbetrag überschreitet Freigabegrenze".into()
    } else {
        specific.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use invoic_checker::{Finding, FindingKind};

    use super::*;

    /// INVOIC_PIDS must be exactly the four GPKE billing PIDs that embed a
    /// `Rechnung` BO4E in the `ProcessInitiated` outbox payload.
    #[test]
    fn invoic_pids_are_gpke_only() {
        assert_eq!(INVOIC_PIDS, &[31001u32, 31002, 31005, 31006]);
        // PIDs belonging to other workflows must NOT be in the list.
        for forbidden in [31003u32, 31004, 31009, 31011] {
            assert!(
                !INVOIC_PIDS.contains(&forbidden),
                "PID {forbidden} must not be in INVOIC_PIDS (wrong workflow)"
            );
        }
    }

    #[test]
    fn extract_pid_happy() {
        let data = serde_json::json!({ "pid": 31001 });
        assert_eq!(extract_pid(&data), 31001u32);
    }

    #[test]
    fn extract_pid_missing_returns_zero() {
        let data = serde_json::json!({ "other": 99 });
        assert_eq!(extract_pid(&data), 0u32);
    }

    #[test]
    fn dispute_reason_empty_findings_returns_fallback() {
        let reason = dispute_reason(&[]);
        assert!(!reason.is_empty(), "fallback message must not be empty");
        assert!(
            reason.contains("Automatische Ablehnung"),
            "fallback must mention automatic rejection"
        );
    }

    #[test]
    fn dispute_reason_with_findings_joins_them() {
        let findings = vec![
            Finding {
                kind: FindingKind::TariffDeviation,
                is_dispute: true,
                message: "Einzelpreis weicht ab".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
            Finding {
                kind: FindingKind::PeriodInvalid,
                is_dispute: true,
                message: "Abrechnungszeitraum falsch".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
        ];
        let reason = dispute_reason(&findings);
        assert!(reason.contains("Einzelpreis weicht ab"));
        assert!(reason.contains("Abrechnungszeitraum falsch"));
    }

    #[test]
    fn dispute_reason_filters_non_dispute_findings() {
        let findings = vec![
            Finding {
                kind: FindingKind::TariffNotFound,
                is_dispute: false,
                message: "just a note".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
            Finding {
                kind: FindingKind::TariffDeviation,
                is_dispute: true,
                message: "Preis abweichend".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
        ];
        let reason = dispute_reason(&findings);
        assert!(!reason.contains("just a note"));
        assert!(reason.contains("Preis abweichend"));
    }
}
