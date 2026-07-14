//! NB process decision module — GPKE and GeLi Gas Anmeldung STP.
//!
//! Consumes `de.mako.process.initiated` events for Lieferbeginn PIDs:
//! - **55001** GPKE Lieferbeginn Standard (Strom) — 24h deadline
//! - **55016** GPKE Lieferbeginn Netzentnahme (Strom) — 24h deadline
//! - **44001** GeLi Gas Lieferbeginn — 10 Werktage deadline
//!
//! # Decision pipeline
//!
//! ```text
//! Event arrives → parse AnmeldungAnfrage
//!   → GET /api/v1/versorgung/{malo_id}          ← marktd (VersorgungsStatus)
//!   → GET /api/v1/malo/{malo_id}/grid            ← marktd (MaloGridRecord)
//!   → GET /api/v1/partners/{lf_mp_id}              ← marktd (partner_known)
//!   → netz_checker::evaluate(anfrage, vs, grid, partner_known, now)
//!       Accept   → write anmeldung_decisions(Accept)
//!                  → MakodClient::post_command(bestaetigen)   [if auto_accept]
//!       Reject   → write anmeldung_decisions(Reject, erc_code)
//!                  → MakodClient::post_command(ablehnen, erc_code)
//!       Escalate → write anmeldung_decisions(Escalate)
//!                  → alert operator
//! ```
//!
//! # Regulatory basis
//!
//! - GPKE: BK6-22-024 §5 + UTILMD Strom AHB
//! - GeLi Gas: BK7-24-01-009 §3 + UTILMD Gas AHB

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use netz_checker::types::RejectReason;
use netz_checker::{AnmeldungAnfrage, Messtyp, NetzCheckResult};
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use mako_markt::domain::Sparte;
use secrecy::SecretString;

use crate::pg::anmeldung::{AnmeldungDecision, AnmeldungDecisionRecord, PgAnmeldungRepository};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the NB module.
#[derive(Debug, Clone)]
pub struct NbModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    pub auto_accept: bool,
}

// MarktdReader replaced by mako_markt::marktd_client::MarktdClient

// ── NB module payload ─────────────────────────────────────────────────────────

/// Fields extracted from a `de.mako.process.initiated` CloudEvent payload
/// for a Lieferbeginn PID.
#[derive(Debug, Clone)]
pub struct AnmeldungPayload {
    pub pid: u32,
    pub process_id: Uuid,
    pub malo_id: String,
    pub new_supplier_gln: String,
    pub grid_operator_gln: String,
    pub bilanzierungsgebiet: Option<String>,
    pub process_date: time::Date,
}

impl AnmeldungPayload {
    /// Parse from the `data` field of a `de.mako.process.initiated` CloudEvent.
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;

        // Only handle Lieferbeginn PIDs.
        if !matches!(pid, 55001 | 55016 | 44001) {
            return None;
        }

        let subject = event["subject"].as_str()?;
        let process_id: Uuid = subject.parse().ok()?;

        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let new_supplier_gln = data.get("new_supplier")?.as_str()?.to_owned();
        let grid_operator_gln = data.get("grid_operator")?.as_str()?.to_owned();
        let bilanzierungsgebiet = data
            .get("bilanzierungsgebiet")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        let date_str = data.get("process_date")?.as_str()?;
        let process_date = if date_str.len() == 8 {
            let fmt = time::macros::format_description!("[year][month][day]");
            time::Date::parse(date_str, &fmt).ok()?
        } else {
            let fmt = time::macros::format_description!("[year]-[month]-[day]");
            time::Date::parse(date_str, &fmt).ok()?
        };

        Some(Self {
            pid,
            process_id,
            malo_id,
            new_supplier_gln,
            grid_operator_gln,
            bilanzierungsgebiet,
            process_date,
        })
    }

    /// Derive `AnmeldungAnfrage` for passing to `netz-checker`.
    pub fn into_anfrage(self) -> AnmeldungAnfrage {
        let sparte = if self.pid == 44001 {
            Sparte::Gas
        } else {
            Sparte::Strom
        };
        // Gas is always SLP for GeLi Gas.  Strom defaults to SLP unless
        // the UTILMD carries an RLM marker (TODO: extract from payload when available).
        let messtyp = Messtyp::Slp;
        AnmeldungAnfrage {
            pid: self.pid,
            process_id: self.process_id,
            malo_id: self.malo_id,
            new_supplier_gln: self.new_supplier_gln,
            grid_operator_gln: self.grid_operator_gln,
            bilanzierungsgebiet: self.bilanzierungsgebiet,
            process_date: self.process_date,
            sparte,
            messtyp,
        }
    }
}

// ── evaluate_and_decide ───────────────────────────────────────────────────────

/// Orchestrate the full NB STP decision for one `de.mako.process.initiated` event.
///
/// # Steps
///
/// 1. Parse `AnmeldungPayload` from the event.
/// 2. Fetch `VersorgungsStatus`, `MaloGridRecord`, and partner presence from `marktd`.
/// 3. Call `netz_checker::evaluate`.
/// 4. Write `anmeldung_decisions` row.
/// 5. If `auto_accept` and result is `Accept`, call `MakodClient::post_command`.
///    If result is `Reject`, always call `MakodClient::post_command` with ERC code.
///
/// Returns `true` if the event was handled (even if the decision was Escalate).
/// Returns `false` if the event is not a Lieferbeginn PID.
pub async fn evaluate_and_decide(
    event: &serde_json::Value,
    config: &NbModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    repo: &PgAnmeldungRepository,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // ── 1. Parse payload ──────────────────────────────────────────────────
    let Some(payload) = AnmeldungPayload::parse(event) else {
        return Ok(false);
    };

    // ── 2. Misdirection check ─────────────────────────────────────────────
    // Fast pre-check: if the event is not for our GLN, skip silently.
    if !payload.grid_operator_gln.is_empty() && payload.grid_operator_gln != config.own_mp_id {
        return Ok(false);
    }

    let initiator_is_affiliate = payload.new_supplier_gln == config.own_mp_id;
    let pid = payload.pid;
    let process_id = payload.process_id;
    let malo_id = payload.malo_id.clone();
    let lf_mp_id = payload.new_supplier_gln.clone();

    info!(
        %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
        "processd NB: evaluating Anmeldung"
    );

    // ── 3. Fetch marktd data ──────────────────────────────────────────────
    let versorgung = reader
        .get_versorgung(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd versorgung fetch failed"))?;

    let malo = reader
        .get_malo(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd malo fetch failed"))
        .unwrap_or(None);

    let grid = reader
        .get_malo_grid(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd grid fetch failed"))?;

    let partner_known = reader.partner_known(&lf_mp_id).await.inspect_err(
        |e| warn!(%e, lf_mp_id = %lf_mp_id, "processd NB: marktd partner check failed"),
    )?;

    let anfrage = payload.into_anfrage();
    let now = OffsetDateTime::now_utc();

    // ── 4. Evaluate ───────────────────────────────────────────────────────
    // Build a grid record for netz-checker from the best available source:
    //  1. `malo_grid` side table (populated by nis-syncd) — most authoritative
    //  2. `malo.bilanzierungsgebiet` (B1 typed extraction) — fallback when
    //     nis-syncd has not yet run; raises STP from ~60% to ~80% for SLP MaLos
    let vs_ref = versorgung.as_ref();
    let grid_nc: Option<netz_checker::MaloGridRecord> = if grid.is_some() {
        grid.as_ref().map(Into::into)
    } else if let Some(ref m) = malo {
        if m.bilanzierungsgebiet.is_some() || m.netzebene.is_some() {
            Some(netz_checker::MaloGridRecord {
                malo_id: malo_id.clone(),
                nb_mp_id: anfrage.grid_operator_gln.clone(),
                bilanzierungsgebiet: m.bilanzierungsgebiet.clone(),
                netzgebiet: None,
            })
        } else {
            None
        }
    } else {
        None
    };
    let grid_ref = grid_nc.as_ref();

    let result = netz_checker::evaluate(&anfrage, vs_ref, grid_ref, partner_known, now);

    info!(
        %process_id, pid, %malo_id,
        grid_source = if grid.is_some() { "malo_grid" } else if grid_nc.is_some() { "malo_typed" } else { "none" },
        outcome = ?result,
        "processd NB: netz-checker result"
    );

    // ── 5. Persist decision ───────────────────────────────────────────────
    let (decision, erc_code, detail) = match &result {
        NetzCheckResult::Accept => (AnmeldungDecision::Accept, None, None),
        NetzCheckResult::Reject(RejectReason {
            erc_code, detail, ..
        }) => (
            AnmeldungDecision::Reject,
            Some(erc_code.clone()),
            Some(detail.clone()),
        ),
        NetzCheckResult::Escalate { reason } => {
            (AnmeldungDecision::Escalate, None, Some(reason.clone()))
        }
    };

    let rec = AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: pid as i32,
        malo_id: malo_id.clone(),
        lf_mp_id: lf_mp_id.clone(),
        decision,
        erc_code: erc_code.clone(),
        detail: detail.clone(),
        initiator_is_affiliate,
        decided_at: now,
        tenant: config.tenant.clone(),
    };

    repo.insert(&rec).await?;

    // ── 6. Dispatch command to makod ──────────────────────────────────────
    match &result {
        NetzCheckResult::Accept => {
            // §20 EnWG Diskriminierungsfreiheitspflicht:
            // When the initiating LF shares the same MP-ID as our operator
            // (vertically integrated utility — §6b EnWG deployment), automatic
            // acceptance is forbidden.  The operator must review manually.
            // Bypassing this check exposes the NB to BNetzA sanctions.
            if initiator_is_affiliate {
                warn!(
                    %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
                    "processd NB: §20 EnWG — affiliate Anmeldung detected; \
                     auto_accept overridden to false — operator must review"
                );
            } else if config.auto_accept {
                let cmd_body = ForwardCommand {
                    marktrolle: None,
                    command: lieferbeginn_accept_command(pid, &malo_id),
                    malo_id: Some(malo_id.clone()),
                    melo_id: None,
                    payload: serde_json::json!({ "process_id": process_id }),
                };
                makod
                    .post_command(&format!("processd-nb-accept-{process_id}"), &cmd_body)
                    .await
                    .inspect_err(
                        |e| warn!(%e, %process_id, "processd NB: bestaetigen dispatch failed"),
                    )?;
                info!(%process_id, pid, %malo_id, "processd NB: dispatched bestaetigen");
            } else {
                info!(%process_id, pid, %malo_id, "processd NB: Accept (auto_accept=false or §20 EnWG affiliate — operator must confirm)");
            }
        }
        NetzCheckResult::Reject(reason) => {
            let cmd_body = ForwardCommand {
                marktrolle: None,
                command: lieferbeginn_reject_command(pid, &malo_id),
                malo_id: Some(malo_id.clone()),
                melo_id: None,
                payload: serde_json::json!({
                    "process_id": process_id,
                    "erc_code": reason.erc_code,
                    "detail": reason.detail,
                }),
            };
            makod
                .post_command(&format!("processd-nb-reject-{process_id}"), &cmd_body)
                .await
                .inspect_err(|e| warn!(%e, %process_id, "processd NB: ablehnen dispatch failed"))?;
            info!(%process_id, pid, %malo_id, erc = %reason.erc_code, "processd NB: dispatched ablehnen");
        }
        NetzCheckResult::Escalate { reason } => {
            warn!(%process_id, pid, %malo_id, %reason, "processd NB: Escalate — operator action required");
        }
    }

    Ok(true)
}

// ── Command name helpers ───────────────────────────────────────────────────────

fn lieferbeginn_accept_command(pid: u32, _malo_id: &str) -> String {
    match pid {
        44001 => "geli.gas.lieferbeginn.bestaetigen".to_owned(),
        _ => "gpke.lieferbeginn.bestaetigen".to_owned(),
    }
}

fn lieferbeginn_reject_command(pid: u32, _malo_id: &str) -> String {
    match pid {
        44001 => "geli.gas.lieferbeginn.ablehnen".to_owned(),
        _ => "gpke.lieferbeginn.ablehnen".to_owned(),
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AnmeldungPayload parsing ───────────────────────────────────────────────

    #[test]
    fn parse_strom_lieferbeginn_event() {
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696780",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "bilanzierungsgebiet": "11YF-VATTENFALL-2",
                "process_date": "20261001"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse");
        assert_eq!(payload.pid, 55001);
        assert_eq!(payload.malo_id, "51238696780");
        assert_eq!(payload.new_supplier_gln, "9900357000004");
        assert_eq!(payload.grid_operator_gln, "9900000000001");
        assert_eq!(
            payload.bilanzierungsgebiet.as_deref(),
            Some("11YF-VATTENFALL-2")
        );
    }

    #[test]
    fn parse_gas_lieferbeginn_event() {
        let event = serde_json::json!({
            "makopid": 44001,
            "subject": "550e8400-e29b-41d4-a716-446655440001",
            "data": {
                "malo_id": "51238696781",
                "new_supplier": "9800357000004",
                "grid_operator": "9800000000001",
                "process_date": "2026-10-01"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse gas event");
        assert_eq!(payload.pid, 44001);
        let anfrage = payload.into_anfrage();
        assert!(matches!(anfrage.sparte, mako_markt::domain::Sparte::Gas));
    }

    #[test]
    fn parse_ignores_unknown_pids() {
        let event = serde_json::json!({
            "makopid": 55008, // E_0624 — LF PID, not NB
            "subject": "550e8400-e29b-41d4-a716-446655440002",
            "data": { "malo_id": "51238696780", "new_supplier": "99x", "grid_operator": "99y", "process_date": "20261001" }
        });
        assert!(AnmeldungPayload::parse(&event).is_none());
    }

    // ── Command name mapping ───────────────────────────────────────────────────

    #[test]
    fn accept_command_strom() {
        assert_eq!(
            lieferbeginn_accept_command(55001, "51238696780"),
            "gpke.lieferbeginn.bestaetigen"
        );
        assert_eq!(
            lieferbeginn_accept_command(55016, "51238696780"),
            "gpke.lieferbeginn.bestaetigen"
        );
    }

    #[test]
    fn accept_command_gas() {
        assert_eq!(
            lieferbeginn_accept_command(44001, "51238696780"),
            "geli.gas.lieferbeginn.bestaetigen"
        );
    }

    #[test]
    fn reject_command_strom() {
        assert_eq!(
            lieferbeginn_reject_command(55001, "51238696780"),
            "gpke.lieferbeginn.ablehnen"
        );
    }

    #[test]
    fn reject_command_gas() {
        assert_eq!(
            lieferbeginn_reject_command(44001, "51238696780"),
            "geli.gas.lieferbeginn.ablehnen"
        );
    }

    // ── Misdirection check ─────────────────────────────────────────────────────

    #[test]
    fn affiliate_detection() {
        // When new_supplier == own_mp_id, initiator_is_affiliate must be true.
        let own_mp_id = "9900357000004";
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440003",
            "data": {
                "malo_id": "51238696780",
                "new_supplier": own_mp_id, // affiliate!
                "grid_operator": "9900000000001",
                "process_date": "20261001"
            }
        });
        let payload = AnmeldungPayload::parse(&event).unwrap();
        let initiator_is_affiliate = payload.new_supplier_gln == own_mp_id;
        assert!(
            initiator_is_affiliate,
            "affiliate must be detected when new_supplier == own_mp_id"
        );
    }
}
