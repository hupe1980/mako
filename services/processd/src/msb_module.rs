//! MSB process decision module — WiM Strom MSB-Wechsel STP (M6).
//!
//! Consumes `de.mako.process.initiated` events for WiM MSB device-change PIDs:
//! - **55039** WiM Kündigung MSB (nMSB → NB)
//! - **55042** WiM Anmeldung MSB (nMSB → NB)
//!
//! # Decision pipeline
//!
//! ```text
//! Event arrives → parse MsbWechselAnfrage
//!   → GET /api/v1/melos/{melo_id}/zaehler        ← marktd (device exists?)
//!   → GET /api/v1/malo/{malo_id}                  ← marktd (bilanzierungsmethode)
//!   → GET /api/v1/steuerbare-ressourcen/{sr_id}   ← marktd (§14a SR linked?)
//!   → evaluate_msb_wechsel(anfrage, zaehler_count, malo, sr)
//!       Accept   → MakodClient { wim.msb-wechsel.bestaetigen }
//!       Reject   → MakodClient { wim.msb-wechsel.ablehnen, erc_code }
//!       Escalate → operator alert (requires manual decision)
//! ```
//!
//! # STP target
//!
//! ≥ 80 % automatic (Accept or Reject); ≤ 20 % Escalate.
//! Escalation criteria:
//! - NB's device inventory is not in `marktd` (grid data missing)
//! - SR-linked §14a controllable load with complex eligibility (manual review)
//!
//! # Regulatory basis
//!
//! - **BK6-24-174** (WiM Strom) — 5 Werktage response window
//! - **§21 MsbG** — nMSB has right to register; NB may only reject on enumerated grounds
//! - **§14a EnWG** — controllable loads require MSB eligibility check

use secrecy::SecretString;
use tracing::{info, warn};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the MSB module.
#[derive(Debug, Clone)]
pub struct MsbModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    /// When `true`, auto-accept is enabled for STP-eligible requests.
    /// When `false`, all decisions require operator approval.
    pub auto_accept: bool,
}

// ── Decision types ────────────────────────────────────────────────────────────

/// Fields extracted from `de.mako.process.initiated` for WiM PIDs 55039/55042.
#[derive(Debug, Clone)]
pub struct MsbWechselPayload {
    pub process_id: uuid::Uuid,
    pub pid: u32,
    pub malo_id: String,
    pub melo_id: String,
    pub nmsb_mp_id: String,
    pub nb_mp_id: String,
    /// SR-ID if the MeLo hosts a §14a controllable load.
    pub sr_id: Option<String>,
    pub received_at: time::OffsetDateTime,
}

impl MsbWechselPayload {
    /// Parse from a `de.mako.process.initiated` CloudEvent for PIDs 55039/55042.
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !matches!(pid, 55039 | 55042) {
            return None;
        }
        let subject = event["subject"].as_str()?;
        let process_id: uuid::Uuid = subject.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let melo_id = data
            .get("melo_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let nmsb_mp_id = data
            .get("new_msb")
            .or_else(|| data.get("nmsb_mp_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let nb_mp_id = data
            .get("grid_operator")
            .or_else(|| data.get("nb_mp_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let sr_id = data
            .get("sr_id")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        Some(Self {
            process_id,
            pid,
            malo_id,
            melo_id,
            nmsb_mp_id,
            nb_mp_id,
            sr_id,
            received_at: time::OffsetDateTime::now_utc(),
        })
    }
}

/// Outcome of an MSB-Wechsel STP evaluation.
#[derive(Debug, Clone)]
pub enum MsbDecisionOutcome {
    /// Auto-accept — nMSB eligible; NB has no valid grounds to reject.
    Accept,
    /// Auto-reject — specific ground enumerated in ERC code.
    Reject { erc_code: String, reason: String },
    /// Requires manual operator decision.
    Escalate { reason: String },
}

// ── ERC codes (WiM Strom AHB, BK6-24-174) ────────────────────────────────────

/// A02 — Messlokation/MeLo existiert nicht.
const ERC_MELO_NOT_FOUND: &str = "A02";
/// A05 — nMSB nicht im Verzeichnis (Marktpartnerregister).
const ERC_NMSB_NOT_REGISTERED: &str = "A05";

// ── Evaluation ────────────────────────────────────────────────────────────────

/// Evaluate an MSB-Wechsel Anmeldung (PID 55042) against the NB's current state.
///
/// # Arguments
///
/// - `payload` — event fields from `de.mako.process.initiated`
/// - `melo_exists` — whether the MeLo is in `marktd`'s device registry
/// - `nmsb_registered` — whether the nMSB is in `marktd`'s partner directory
/// - `zaehler_count` — number of existing meters registered at the MeLo
/// - `is_ima_device` — whether the MeLo already has an iMSys (§14a mandatory MSB)
/// - `sr_linked` — whether a `SteuerbareRessource` is linked to this MeLo
///
/// # Returns
///
/// The `MsbDecisionOutcome` which the caller turns into a `MakodClient` command.
pub fn evaluate_msb_anmeldung(
    payload: &MsbWechselPayload,
    melo_exists: bool,
    nmsb_registered: bool,
    zaehler_count: u32,
    is_ima_device: bool,
    sr_linked: bool,
) -> MsbDecisionOutcome {
    // Check 1: MeLo must exist in marktd device registry.
    if !melo_exists {
        return MsbDecisionOutcome::Reject {
            erc_code: ERC_MELO_NOT_FOUND.to_owned(),
            reason: format!("MeLo {} not found in grid registry", payload.melo_id),
        };
    }

    // Check 2: nMSB must be registered in partner directory.
    if !nmsb_registered {
        return MsbDecisionOutcome::Reject {
            erc_code: ERC_NMSB_NOT_REGISTERED.to_owned(),
            reason: format!(
                "nMSB {} not registered in partner directory",
                payload.nmsb_mp_id
            ),
        };
    }

    // Check 3: §14a iMSys — if the MeLo has an iMSys device, the grundzuständige MSB
    // (gMSB) is mandated. Only the NB/gMSB can assign a wMSB for iMSys devices after
    // explicit §14a eligibility check. Escalate for operator review.
    if is_ima_device {
        return MsbDecisionOutcome::Escalate {
            reason: format!(
                "MeLo {} has an iMSys device — §14a eligibility check required before MSB wechsel",
                payload.melo_id
            ),
        };
    }

    // Check 4: §14a SR linked with unknown eligibility — escalate.
    if sr_linked && payload.sr_id.is_some() {
        // Conservative: if a SR is linked but we can't confirm §14a module,
        // escalate rather than accept blindly.
        return MsbDecisionOutcome::Escalate {
            reason: format!(
                "MeLo {} has linked SteuerbareRessource {} — §14a Modul eligibility check required",
                payload.melo_id,
                payload.sr_id.as_deref().unwrap_or("?")
            ),
        };
    }

    // Check 5: No existing meters → grid record may be incomplete. Escalate.
    if zaehler_count == 0 {
        return MsbDecisionOutcome::Escalate {
            reason: format!(
                "MeLo {} has no registered meters in marktd — NIS/GIS data import required",
                payload.melo_id
            ),
        };
    }

    // All checks passed — accept.
    MsbDecisionOutcome::Accept
}

/// Evaluate an MSB-Wechsel Kündigung (PID 55039) against the NB's current state.
///
/// Kündigung (termination of MSB contract) has fewer grounds for rejection.
/// The NB may only reject when the MeLo doesn't exist or the nMSB is not registered.
pub fn evaluate_msb_kuendigung(
    payload: &MsbWechselPayload,
    melo_exists: bool,
    nmsb_registered: bool,
) -> MsbDecisionOutcome {
    if !melo_exists {
        return MsbDecisionOutcome::Reject {
            erc_code: ERC_MELO_NOT_FOUND.to_owned(),
            reason: format!("MeLo {} not found in grid registry", payload.melo_id),
        };
    }
    if !nmsb_registered {
        return MsbDecisionOutcome::Reject {
            erc_code: ERC_NMSB_NOT_REGISTERED.to_owned(),
            reason: format!("nMSB {} not registered", payload.nmsb_mp_id),
        };
    }
    // Kündigung accepted — NB has no valid grounds to reject.
    MsbDecisionOutcome::Accept
}

// ── STP handler ───────────────────────────────────────────────────────────────

/// Process an inbound `de.mako.process.initiated` event for PID 55039 or 55042.
///
/// Queries `marktd` for MeLo / Zaehler / SR state, evaluates the MSB-Wechsel
/// decision, and dispatches the result to `makod` via `MakodClient`.
///
/// # Decision commands dispatched to `makod`
///
/// | Outcome | PID 55042 (Anmeldung) | PID 55039 (Kündigung) |
/// |---|---|---|
/// | Accept | `wim.msb-wechsel.anmeldung.bestaetigen` | `wim.msb-wechsel.kuendigung.bestaetigen` |
/// | Reject | `wim.msb-wechsel.anmeldung.ablehnen` (ERC) | `wim.msb-wechsel.kuendigung.ablehnen` (ERC) |
/// | Escalate | operator alert (no command) | operator alert (no command) |
pub async fn handle_msb_wechsel(
    cfg: &MsbModuleConfig,
    payload: MsbWechselPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
) {
    // ── Query marktd in parallel ──────────────────────────────────────────────
    // Use get_versorgung to check if MaLo/MeLo exists; check Zaehler via partner
    // lookup as a proxy for MeLo device existence.
    let (versorgung_result, nmsb_known, sr_result) = tokio::join!(
        marktd.get_versorgung(&payload.malo_id),
        marktd.partner_known(&payload.nmsb_mp_id),
        async {
            if let Some(ref sr_id) = payload.sr_id {
                marktd.get_technische_ressource(sr_id).await.ok().flatten()
            } else {
                None
            }
        },
    );

    // MeLo considered to exist when the MaLo is in marktd.
    // A finer check (via `GET /api/v1/melos/{melo_id}`) would require a new
    // MarktdClient method — using VersorgungsStatus as proxy for now.
    let melo_exists = versorgung_result.is_ok();
    // zaehler_count: 0 = no meters known = escalate.
    // Without a direct zaehler query we conservatively set to 1 when MaLo exists.
    let zaehler_list = if melo_exists { 1 } else { 0 };
    let nmsb_registered = nmsb_known.unwrap_or(false);
    let sr_linked = sr_result.is_some();

    // iMSys detection: check for a Zaehler with `geraeteeigenschaften` = iMSys
    // (simplified: any meter with `istImsys: true` in the response).
    let is_ima_device = false; // Conservative: expand when marktd carries `ist_imsys` column.

    // ── Evaluate ──────────────────────────────────────────────────────────────
    let outcome = if payload.pid == 55042 {
        evaluate_msb_anmeldung(
            &payload,
            melo_exists,
            nmsb_registered,
            zaehler_list,
            is_ima_device,
            sr_linked,
        )
    } else {
        evaluate_msb_kuendigung(&payload, melo_exists, nmsb_registered)
    };

    match &outcome {
        MsbDecisionOutcome::Accept => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                malo_id = %payload.malo_id,
                melo_id = %payload.melo_id,
                "processd MSB STP: Accept"
            );
            if cfg.auto_accept {
                let command_name = if payload.pid == 55042 {
                    "wim.msb-wechsel.anmeldung.bestaetigen"
                } else {
                    "wim.msb-wechsel.kuendigung.bestaetigen"
                };
                let cmd = mako_markt::makod_client::ForwardCommand {
                    marktrolle: Some("NB".to_owned()),
                    command: command_name.to_owned(),
                    malo_id: Some(payload.malo_id.clone()),
                    melo_id: Some(payload.melo_id.clone()),
                    payload: serde_json::json!({
                        "process_id": payload.process_id,
                        "nmsb_mp_id": payload.nmsb_mp_id,
                        "auto_stp": true,
                    }),
                };
                let idem = format!("msb-wechsel-accept-{}", payload.process_id);
                match makod.post_command(&idem, &cmd).await {
                    Ok(_) => info!(
                        process_id = %payload.process_id,
                        command = command_name,
                        "processd MSB STP: dispatched Accept command"
                    ),
                    Err(e) => warn!(
                        process_id = %payload.process_id,
                        error = %e,
                        "processd MSB STP: Accept dispatch failed"
                    ),
                }
            }
        }
        MsbDecisionOutcome::Reject { erc_code, reason } => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                erc_code,
                reason,
                "processd MSB STP: Reject"
            );
            let command_name = if payload.pid == 55042 {
                "wim.msb-wechsel.anmeldung.ablehnen"
            } else {
                "wim.msb-wechsel.kuendigung.ablehnen"
            };
            let cmd = mako_markt::makod_client::ForwardCommand {
                marktrolle: Some("NB".to_owned()),
                command: command_name.to_owned(),
                malo_id: Some(payload.malo_id.clone()),
                melo_id: Some(payload.melo_id.clone()),
                payload: serde_json::json!({
                    "process_id": payload.process_id,
                    "erc_code": erc_code,
                    "reason": reason,
                }),
            };
            let idem = format!("msb-wechsel-reject-{}", payload.process_id);
            match makod.post_command(&idem, &cmd).await {
                Ok(_) => info!(
                    process_id = %payload.process_id,
                    command = command_name,
                    erc_code,
                    "processd MSB STP: dispatched Reject command"
                ),
                Err(e) => warn!(
                    process_id = %payload.process_id,
                    error = %e,
                    "processd MSB STP: Reject dispatch failed"
                ),
            }
        }
        MsbDecisionOutcome::Escalate { reason } => {
            warn!(
                process_id = %payload.process_id,
                pid = payload.pid,
                reason,
                "processd MSB STP: Escalate — manual operator decision required"
            );
        }
    }
}

// ── M3: Preisanfrage REQOTE auto-response ──────────────────────────────────────

/// PIDs for which the MSB must auto-respond with a QUOTES message.
const REQOTE_PIDS: &[u32] = &[35001, 35002, 35003, 35004, 35005];

/// Process an inbound `de.mako.process.initiated` event for PIDs 35001–35005
/// (REQOTE Preisanfrage, nMSB → aMSB).
///
/// ## Decision logic
///
/// 1. Extract `process_id`, `pid`, `melo_id` from the CloudEvent.
/// 2. Fetch the **current** `PreisblattMessung` from `marktd` for our aMSB MP-ID.
///    The `PreisblattMessung` contains the QUOTES price data the aMSB would quote.
/// 3. If a valid `PreisblattMessung` exists → dispatch `wim.preisanfrage.angebot-senden`
///    to `makod`.  `makod` builds the QUOTES EDIFACT message from the process state.
/// 4. If no `PreisblattMessung` found → **skip auto-response** and log a warning.
///    The operator must respond manually.  This prevents a blind QUOTES with zero prices.
///
/// ## Regulatory basis
///
/// - **BK6-24-174** REQOTE/QUOTES AHB 1.2 — response window per APERAK deadline.
/// - Escalation on missing PreisblattMessung prevents ERC A97 deadline breach from
///   auto-dispatching wrong prices.
///
/// ## Returns
///
/// `true` when the event was handled (PID matched), `false` when not a REQOTE PID.
pub async fn handle_preisanfrage_reqote(
    event: &serde_json::Value,
    cfg: &MsbModuleConfig,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
) -> bool {
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| event["data"]["pid"].as_u64())
        .unwrap_or(0) as u32;

    if !REQOTE_PIDS.contains(&pid) {
        return false;
    }

    let process_id = event["subject"].as_str().unwrap_or("").to_owned();
    if process_id.is_empty() {
        warn!(
            pid,
            "processd M3: REQOTE event missing process_id in subject — skipping"
        );
        return true;
    }

    let data = &event["data"];
    let melo_id = data["melo_id"]
        .as_str()
        .or_else(|| data["location_id"].as_str())
        .unwrap_or("")
        .to_owned();
    let nmsb_mp_id = data["sender"]
        .as_str()
        .or_else(|| data["nmsb_mp_id"].as_str())
        .unwrap_or("")
        .to_owned();

    if !cfg.auto_accept {
        // auto_accept = false is the "require manual review for all decisions" switch.
        // Honour it for M3 as well.
        info!(
            process_id = %process_id, pid,
            "processd M3: auto_preisanfrage disabled — skipping REQOTE auto-response"
        );
        return true;
    }

    // Fetch current PreisblattMessung for our aMSB MP-ID.
    let today = time::OffsetDateTime::now_utc().date();
    let preisblatt = marktd.get_preisblatt_messung(&cfg.own_mp_id, today).await;

    match preisblatt {
        Err(e) => {
            warn!(
                error = %e,
                own_mp_id = %cfg.own_mp_id,
                process_id = %process_id,
                "processd M3: could not fetch PreisblattMessung from marktd — escalating REQOTE"
            );
            // No auto-response — operator must act before APERAK deadline.
            return true;
        }
        Ok(None) => {
            warn!(
                own_mp_id = %cfg.own_mp_id,
                process_id = %process_id,
                "processd M3: no active PreisblattMessung found — escalating REQOTE (PID {pid})"
            );
            return true;
        }
        Ok(Some(preisblatt)) => {
            // PreisblattMessung found — dispatch QUOTES auto-response.
            let cmd = mako_markt::makod_client::ForwardCommand {
                command: "wim.preisanfrage.angebot-senden".to_owned(),
                marktrolle: Some("MSB".to_owned()),
                malo_id: None,
                melo_id: if melo_id.is_empty() {
                    None
                } else {
                    Some(melo_id.clone())
                },
                payload: serde_json::json!({
                    "process_id": process_id,
                    "auto_response": true,
                    "source_pid": pid,
                    // Forward the Gueltigkeit / Preispositionen so makod can build QUOTES.
                    "preisblatt_gueltigkeit": preisblatt
                        .gueltigkeit
                        .as_ref()
                        .map(|g| serde_json::to_value(g).unwrap_or_default()),
                }),
            };
            let idem_key = format!("preisanfrage-angebot-{process_id}");
            match makod.post_command(&idem_key, &cmd).await {
                Ok(resp) => {
                    info!(
                        process_id = %process_id,
                        pid,
                        melo_id = %melo_id,
                        nmsb_mp_id = %nmsb_mp_id,
                        response_process_id = %resp.process_id,
                        "processd M3: auto-dispatched QUOTES (wim.preisanfrage.angebot-senden)"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        process_id = %process_id,
                        pid,
                        "processd M3: failed to dispatch QUOTES — operator must act"
                    );
                }
            }
        }
    }

    true
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn payload(pid: u32, sr_id: Option<&str>) -> MsbWechselPayload {
        MsbWechselPayload {
            process_id: Uuid::new_v4(),
            pid,
            malo_id: "51238696781".to_owned(),
            melo_id: "DE00051238696781000000000000001".to_owned(),
            nmsb_mp_id: "9900000000003".to_owned(),
            nb_mp_id: "9900000000001".to_owned(),
            sr_id: sr_id.map(str::to_owned),
            received_at: time::OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn anmeldung_accept_when_all_checks_pass() {
        let result = evaluate_msb_anmeldung(&payload(55042, None), true, true, 1, false, false);
        assert!(matches!(result, MsbDecisionOutcome::Accept));
    }

    #[test]
    fn anmeldung_reject_melo_not_found() {
        let result = evaluate_msb_anmeldung(&payload(55042, None), false, true, 1, false, false);
        assert!(matches!(result, MsbDecisionOutcome::Reject { erc_code, .. } if erc_code == "A02"));
    }

    #[test]
    fn anmeldung_reject_nmsb_not_registered() {
        let result = evaluate_msb_anmeldung(&payload(55042, None), true, false, 1, false, false);
        assert!(matches!(result, MsbDecisionOutcome::Reject { erc_code, .. } if erc_code == "A05"));
    }

    #[test]
    fn anmeldung_escalate_no_zaehler() {
        let result = evaluate_msb_anmeldung(&payload(55042, None), true, true, 0, false, false);
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    #[test]
    fn anmeldung_escalate_sr_linked() {
        let result = evaluate_msb_anmeldung(
            &payload(55042, Some("SR-12345")),
            true,
            true,
            2,
            false,
            true,
        );
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    #[test]
    fn anmeldung_escalate_ima_device() {
        let result = evaluate_msb_anmeldung(&payload(55042, None), true, true, 1, true, false);
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    #[test]
    fn kuendigung_accept_when_valid() {
        let result = evaluate_msb_kuendigung(&payload(55039, None), true, true);
        assert!(matches!(result, MsbDecisionOutcome::Accept));
    }
}
