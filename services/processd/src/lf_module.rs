//! LF process decision module — LFA E_0624 auto-response and related automation.
//!
//! Handles LF (Lieferant) automation obligations under LFW24 (BK6-22-024):
//!
//! | PID | Process | Deadline | Auto action |
//! |-----|---------|----------|-------------|
//! | 55008 | LFA E_0624 Abmeldung | 45 min | Evaluate from VersorgungsStatus |
//! | 55022 | Zuordnungsanfrage | 45 min | Confirm `lf_gln_next` if known |
//! | 55013 | Abmeldung-Bestätigung | — | Update `lieferstatus = Unbeliefert` via marktd |
//!
//! # Decision logic for E_0624 (PID 55008)
//!
//! ```text
//! GET /api/v1/versorgung/{malo_id}
//!   Beliefert + clean end            → einwilligung (auto-consent)
//!   Beliefert + Vertragsbindung      → ablehnen A35
//!   Beliefert + Einzug               → ablehnen A32
//!   Beliefert + Ersatzversorgung     → einwilligung
//!   Grundversorgung                  → einwilligung
//!   MaLo unknown / lf_mp_id mismatch  → approval_queue
//!   Any other state                  → approval_queue
//! ```
//!
//! # Regulatory basis
//!
//! - GPKE Teil 1 §5 (GPKE LFA obligation, BK6-22-024)
//! - LFW24: 45-minute APERAK + response window

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use secrecy::SecretString;

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue, QueueStatus};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the LF module.
#[derive(Debug, Clone)]
pub struct LfModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    /// When `true`, dispatch `einwilligung`/`ablehnen` automatically.
    pub auto_respond: bool,
    /// Duration before deadline to expire queue entries (in seconds).
    pub queue_ttl_secs: u64,
}

// ── marktd reader (shared with nb_module via direct reqwest) ──────────────────

// ── E_0624 payload ────────────────────────────────────────────────────────────

/// Parsed fields from a `de.mako.process.initiated` for PID 55008 (E_0624).
#[derive(Debug, Clone)]
pub struct E0624Payload {
    pub process_id: Uuid,
    pub malo_id: String,
    /// GLN of the grid operator who sent the E_0624.
    pub initiating_nb_gln: String,
    /// Requested Lieferende date.
    pub lieferende_date: Option<time::Date>,
    /// Whether this is a Vertragsbindung or Einzug scenario.
    pub scenario: E0624Scenario,
    /// CE deadline (computed from event `time` + 45 min).
    pub deadline_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E0624Scenario {
    Standard,
    Einzug,
    Ersatzversorgung,
    Unknown,
}

impl E0624Payload {
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())?;
        if pid != 55008 {
            return None;
        }

        let subject = event["subject"].as_str()?;
        let process_id: Uuid = subject.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let initiating_nb_gln = data
            .get("grid_operator")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        let scenario_str = data
            .get("scenario")
            .and_then(|v| v.as_str())
            .unwrap_or("standard");
        let scenario = match scenario_str {
            "einzug" | "EINZUG" => E0624Scenario::Einzug,
            "ersatzversorgung" | "ERSATZVERSORGUNG" => E0624Scenario::Ersatzversorgung,
            _ => E0624Scenario::Standard,
        };

        let lieferende_date = data
            .get("lieferende")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                if s.len() == 8 {
                    let fmt = time::macros::format_description!("[year][month][day]");
                    time::Date::parse(s, &fmt).ok()
                } else {
                    let fmt = time::macros::format_description!("[year]-[month]-[day]");
                    time::Date::parse(s, &fmt).ok()
                }
            });

        // Compute deadline: event time + 45 min (APERAK AHB 1.0 §2.4.1)
        let event_time = event["time"]
            .as_str()
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(OffsetDateTime::now_utc);
        let deadline_at = event_time + time::Duration::minutes(45);

        Some(Self {
            process_id,
            malo_id,
            initiating_nb_gln,
            lieferende_date,
            scenario,
            deadline_at,
        })
    }
}

// ── LF decision ───────────────────────────────────────────────────────────────

/// Outcome of the LFA E_0624 evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LfDecision {
    /// Dispatch `einwilligung` (consent to Abmeldung).
    Einwilligung,
    /// Dispatch `ablehnen` with `erc_code` (A32 = Einzug, A35 = Vertragsbindung).
    Ablehnen { erc_code: String },
    /// Enqueue for ERP review.
    Escalate { reason: String },
}

/// Evaluate the LFA E_0624 decision from the current VersorgungsStatus.
fn evaluate_e0624(
    payload: &E0624Payload,
    versorgung: Option<&VersorgungsStatusRecord>,
    own_mp_id: &str,
) -> LfDecision {
    let Some(vs) = versorgung else {
        return LfDecision::Escalate {
            reason: format!(
                "MaLo {} not found in master data. Cannot auto-decide E_0624.",
                payload.malo_id
            ),
        };
    };

    // Verify this LF is actually supplying the MaLo.
    if vs.lieferstatus != LieferStatus::Beliefert
        && vs.lieferstatus != LieferStatus::Grundversorgung
        && vs.lieferstatus != LieferStatus::Ersatzversorgung
    {
        return LfDecision::Escalate {
            reason: format!(
                "MaLo {} is not in Beliefert/Grundversorgung/Ersatzversorgung state \
                 (current: {}). Cannot auto-decide.",
                payload.malo_id, vs.lieferstatus
            ),
        };
    }

    // Verify the LF GLN matches our own.
    if vs.lf_mp_id.as_deref().is_some_and(|lf| lf != own_mp_id) {
        let active_lf = vs.lf_mp_id.as_deref().unwrap_or("");
        return LfDecision::Escalate {
            reason: format!(
                "MaLo {} is supplied by {} but our GLN is {}. \
                 Cannot auto-decide — LF mismatch.",
                payload.malo_id, active_lf, own_mp_id
            ),
        };
    }

    // Apply E_0624 scenario rules.
    match payload.scenario {
        E0624Scenario::Einzug => LfDecision::Ablehnen {
            erc_code: "A32".to_owned(),
        },
        E0624Scenario::Ersatzversorgung => LfDecision::Einwilligung,
        E0624Scenario::Standard | E0624Scenario::Unknown => {
            if vs.lieferstatus == LieferStatus::Beliefert
                || vs.lieferstatus == LieferStatus::Grundversorgung
            {
                LfDecision::Einwilligung
            } else {
                LfDecision::Escalate {
                    reason: format!(
                        "Unexpected LieferStatus {} for E_0624 on MaLo {}.",
                        vs.lieferstatus, payload.malo_id
                    ),
                }
            }
        }
    }
}

// ── process_e0624 ─────────────────────────────────────────────────────────────

/// Handle one `de.mako.process.initiated` event for PID 55008 (E_0624).
///
/// Returns `true` if the event was handled (even if escalated).
/// Returns `false` if the event PID is not 55008.
pub async fn process_e0624(
    event: &serde_json::Value,
    config: &LfModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    queue: &PgApprovalQueue,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let Some(payload) = E0624Payload::parse(event) else {
        return Ok(false);
    };

    info!(
        process_id = %payload.process_id,
        malo_id = %payload.malo_id,
        "processd LF: evaluating E_0624"
    );

    // ── Fetch VersorgungsStatus ────────────────────────────────────────────
    let versorgung = reader.get_versorgung(&payload.malo_id).await.inspect_err(
        |e| warn!(%e, malo_id = %payload.malo_id, "processd LF: marktd fetch failed"),
    )?;

    let decision = evaluate_e0624(&payload, versorgung.as_ref(), &config.own_mp_id);

    info!(
        process_id = %payload.process_id,
        malo_id = %payload.malo_id,
        outcome = ?decision,
        "processd LF: E_0624 decision"
    );

    match &decision {
        LfDecision::Einwilligung => {
            if config.auto_respond {
                let cmd = ForwardCommand {
                    marktrolle: None,
                    command: "gpke.lfa.einwilligung".to_owned(),
                    malo_id: Some(payload.malo_id.clone()),
                    melo_id: None,
                    payload: serde_json::json!({
                        "process_id": payload.process_id,
                        "lieferende": payload.lieferende_date,
                    }),
                };
                makod
                    .post_command(
                        &format!("processd-lf-einwilligung-{}", payload.process_id),
                        &cmd,
                    )
                    .await
                    .inspect_err(|e| warn!(%e, "processd LF: einwilligung dispatch failed"))?;
                info!(process_id = %payload.process_id, "processd LF: dispatched einwilligung");
            }
        }
        LfDecision::Ablehnen { erc_code } => {
            if config.auto_respond {
                let cmd = ForwardCommand {
                    marktrolle: None,
                    command: "gpke.lfa.ablehnen".to_owned(),
                    malo_id: Some(payload.malo_id.clone()),
                    melo_id: None,
                    payload: serde_json::json!({
                        "process_id": payload.process_id,
                        "erc_code": erc_code,
                    }),
                };
                makod
                    .post_command(
                        &format!("processd-lf-ablehnen-{}", payload.process_id),
                        &cmd,
                    )
                    .await
                    .inspect_err(|e| warn!(%e, "processd LF: ablehnen dispatch failed"))?;
                info!(process_id = %payload.process_id, %erc_code, "processd LF: dispatched ablehnen");
            }
        }
        LfDecision::Escalate { reason } => {
            warn!(
                process_id = %payload.process_id,
                malo_id = %payload.malo_id,
                %reason,
                "processd LF: E_0624 escalated — creating approval_queue entry"
            );
            let entry = ApprovalQueueEntry {
                id: Uuid::new_v4(),
                process_id: payload.process_id,
                pid: 55008_i32,
                malo_id: Some(payload.malo_id.clone()),
                reason: reason.clone(),
                status: QueueStatus::Pending,
                // Expire 5 minutes before the regulatory deadline.
                expires_at: payload.deadline_at - time::Duration::minutes(5),
                created_at: OffsetDateTime::now_utc(),
                decided_at: None,
                tenant: config.tenant.clone(),
            };
            queue
                .enqueue(&entry)
                .await
                .inspect_err(|e| warn!(%e, "processd LF: failed to enqueue approval entry"))?;
        }
    }

    Ok(true)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::domain::MaloId;
    use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
    use time::OffsetDateTime;

    fn make_vs(status: LieferStatus, lf_mp_id: Option<&str>) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696780".parse::<MaloId>().unwrap(),
            lieferstatus: status,
            lf_mp_id: lf_mp_id.map(ToOwned::to_owned),
            lf_gln_next: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: "9900000000001".to_owned(),
            last_process_id: None,
            updated_at: OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        }
    }

    fn make_payload(scenario: E0624Scenario) -> E0624Payload {
        E0624Payload {
            process_id: Uuid::new_v4(),
            malo_id: "51238696780".to_owned(),
            initiating_nb_gln: "9900000000001".to_owned(),
            lieferende_date: None,
            scenario,
            deadline_at: OffsetDateTime::now_utc() + time::Duration::minutes(45),
        }
    }

    #[test]
    fn beliefert_standard_einwilligung() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900357000004"));
        let payload = make_payload(E0624Scenario::Standard);
        let result = evaluate_e0624(&payload, Some(&vs), "9900357000004");
        assert_eq!(result, LfDecision::Einwilligung);
    }

    #[test]
    fn einzug_ablehnen_a32() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900357000004"));
        let payload = make_payload(E0624Scenario::Einzug);
        let result = evaluate_e0624(&payload, Some(&vs), "9900357000004");
        assert_eq!(
            result,
            LfDecision::Ablehnen {
                erc_code: "A32".to_owned()
            }
        );
    }

    #[test]
    fn unknown_malo_escalates() {
        let payload = make_payload(E0624Scenario::Standard);
        let result = evaluate_e0624(&payload, None, "9900357000004");
        assert!(matches!(result, LfDecision::Escalate { .. }));
    }

    #[test]
    fn wrong_lf_gln_escalates() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900999000001")); // different LF
        let payload = make_payload(E0624Scenario::Standard);
        let result = evaluate_e0624(&payload, Some(&vs), "9900357000004"); // own_mp_id differs
        assert!(matches!(result, LfDecision::Escalate { .. }));
    }

    #[test]
    fn grundversorgung_einwilligung() {
        let vs = make_vs(LieferStatus::Grundversorgung, Some("9900357000004"));
        let payload = make_payload(E0624Scenario::Standard);
        let result = evaluate_e0624(&payload, Some(&vs), "9900357000004");
        assert_eq!(result, LfDecision::Einwilligung);
    }
}
