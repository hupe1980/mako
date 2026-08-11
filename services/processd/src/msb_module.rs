//! MSB process decision module — WiM Strom MSB-Wechsel STP (M6).
//!
//! Consumes `de.mako.process.initiated` events for WiM MSB device-change PIDs:
//! - **55039** WiM Kündigung MSB (nMSB → aMSB, answered by the incumbent MSB)
//! - **55042** WiM Anmeldung MSB (nMSB → NB, answered by the NB)
//!
//! # Decision pipeline
//!
//! ```text
//! Event arrives → parse MsbWechselAnfrage
//!   → GET /api/v1/melos/{melo_id}/zaehler        ← marktd (device exists?)
//!   → GET /api/v1/malo/{malo_id}                  ← marktd (bilanzierungsmethode)
//!   → GET /api/v1/steuerbare-ressourcen/{sr_id}   ← marktd (§14a SR linked?)
//!   → evaluate_msb_wechsel(anfrage, zaehler_count, malo, sr)
//!       Accept   → MakodClient { wim.geraetewechsel.bestaetigen }
//!       Reject   → MakodClient { wim.geraetewechsel.ablehnen, erc_code }
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

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

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
/// - `is_ima_device` — whether the MeLo already has an iMSys (§14a mandatory
///   MSB); `None` when the device registry cannot answer, which escalates —
///   an unknown iMSys status must never resolve to an auto-Accept
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
    is_ima_device: Option<bool>,
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

    // Check 3: No existing meters → grid record incomplete. Escalate.
    if zaehler_count == 0 {
        return MsbDecisionOutcome::Escalate {
            reason: format!(
                "MeLo {} has no registered meters in marktd — NIS/GIS data import required",
                payload.melo_id
            ),
        };
    }

    // Check 4: §14a iMSys — if the MeLo has an iMSys device, the grundzuständige MSB
    // (gMSB) is mandated. Only the NB/gMSB can assign a wMSB for iMSys devices after
    // explicit §14a eligibility check. Escalate for operator review.
    match is_ima_device {
        Some(true) => {
            return MsbDecisionOutcome::Escalate {
                reason: format!(
                    "MeLo {} has an iMSys device — §14a eligibility check required before MSB wechsel",
                    payload.melo_id
                ),
            };
        }
        None => {
            return MsbDecisionOutcome::Escalate {
                reason: format!(
                    "MeLo {} — Zählertyp not available from marktd, iMSys status unknown; \
                     §14a eligibility cannot be decided automatically",
                    payload.melo_id
                ),
            };
        }
        Some(false) => {}
    }

    // Check 5: §14a SR linked with unknown eligibility — escalate.
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

// ── Command name mapping ──────────────────────────────────────────────────────

/// Registered `makod` command + Marktrolle for answering an inbound
/// MSB-Wechsel order (PID 55039/55042).
///
/// Both PIDs resolve to the same command pair — `makod` routes the answer into
/// the `wim-geraetewechsel` process it spawned for the inbound order, which
/// already knows whether it is an Anmeldung or a Kündigung. The Marktrolle
/// differs: PID 55042 (Anmeldung, MSBN → NB) is answered by the NB, PID 55039
/// (Kündigung, MSBN → MSBA) by the incumbent MSB.
fn geraetewechsel_answer_command(pid: u32, accept: bool) -> (&'static str, &'static str) {
    let command = if accept {
        mako_markt::commands::WIM_GERAETEWECHSEL_BESTAETIGEN
    } else {
        mako_markt::commands::WIM_GERAETEWECHSEL_ABLEHNEN
    };
    let marktrolle = if pid == 55042 { "NB" } else { "MSB" };
    (command, marktrolle)
}

// ── STP handler ───────────────────────────────────────────────────────────────

/// Process an inbound `de.mako.process.initiated` event for PID 55039 or 55042.
///
/// Queries `marktd` for MeLo / Zaehler / SR state, evaluates the MSB-Wechsel
/// decision, and dispatches the result to `makod` via `MakodClient`.
///
/// # Decision commands dispatched to `makod`
///
/// Both PIDs answer through the same registered command pair — the
/// Anmeldung/Kündigung distinction lives in the `wim-geraetewechsel` process
/// that `makod` spawned for the inbound order (keyed by MeLo). The Marktrolle
/// depends on the PID: 55042 is answered by the NB, 55039 by the incumbent MSB.
///
/// | Outcome | PID 55042 (Anmeldung, as NB) | PID 55039 (Kündigung, as MSB) |
/// |---|---|---|
/// | Accept | `wim.geraetewechsel.bestaetigen` | `wim.geraetewechsel.bestaetigen` |
/// | Reject | `wim.geraetewechsel.ablehnen` (ERC in reason) | `wim.geraetewechsel.ablehnen` (ERC in reason) |
/// | Escalate | approval-queue entry | approval-queue entry |
///
/// # Errors
///
/// Every `marktd` lookup failure is propagated so the caller answers 5xx and
/// `marktd`'s durable fan-out redelivers. A transport error is **not** evidence
/// of absence: treating it as one used to dispatch a wrongful A02 "MeLo not
/// found" rejection into the market against a valid §21 MsbG registration.
pub async fn handle_msb_wechsel(
    cfg: &MsbModuleConfig,
    payload: MsbWechselPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<()> {
    // ── Query marktd in parallel ──────────────────────────────────────────────
    let (versorgung_result, nmsb_known, zaehler_result, sr_result) = tokio::join!(
        marktd.get_versorgung(&payload.malo_id),
        marktd.partner_known(&payload.nmsb_mp_id),
        async {
            if payload.melo_id.is_empty() {
                Ok(Vec::new())
            } else {
                marktd.list_zaehler_ids(&payload.melo_id).await
            }
        },
        async {
            if let Some(ref sr_id) = payload.sr_id {
                marktd.get_technische_ressource(sr_id).await
            } else {
                Ok(None)
            }
        },
    );

    // `Ok(None)` is the 404 — a genuinely absent MaLo, and the A02 ground.
    // MeLo considered to exist when the MaLo is in marktd; a finer check (via
    // `GET /api/v1/melos/{melo_id}`) would require a new MarktdClient method.
    let melo_exists = versorgung_result?.is_some();
    let nmsb_registered = nmsb_known?;
    let zaehler_count = u32::try_from(zaehler_result?.len()).unwrap_or(u32::MAX);
    let sr_linked = sr_result?.is_some();

    // iMSys detection needs the Zählertyp, which `MarktdClient` does not expose
    // (`list_zaehler_ids` returns identifiers only). Unknown, so check 4
    // escalates rather than fabricating a value that auto-accepts.
    let is_ima_device = None;

    // ── Evaluate ──────────────────────────────────────────────────────────────
    let outcome = if payload.pid == 55042 {
        evaluate_msb_anmeldung(
            &payload,
            melo_exists,
            nmsb_registered,
            zaehler_count,
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
                let (command_name, marktrolle) = geraetewechsel_answer_command(payload.pid, true);
                let cmd = mako_markt::makod_client::ForwardCommand {
                    marktrolle: Some(marktrolle.to_owned()),
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
                makod.post_command(&idem, &cmd).await.inspect_err(|e| {
                    warn!(
                        process_id = %payload.process_id,
                        error = %e,
                        "processd MSB STP: Accept dispatch failed"
                    );
                })?;
                info!(
                    process_id = %payload.process_id,
                    command = command_name,
                    "processd MSB STP: dispatched Accept command"
                );
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
            let (command_name, marktrolle) = geraetewechsel_answer_command(payload.pid, false);
            let cmd = mako_markt::makod_client::ForwardCommand {
                marktrolle: Some(marktrolle.to_owned()),
                command: command_name.to_owned(),
                malo_id: Some(payload.malo_id.clone()),
                melo_id: Some(payload.melo_id.clone()),
                payload: serde_json::json!({
                    "process_id": payload.process_id,
                    "erc_code": erc_code,
                    // makod's APERAK dispatch forwards `reason`; carry the ERC
                    // code inside it so the rejection ground survives the hop.
                    "reason": format!("{erc_code}: {reason}"),
                }),
            };
            let idem = format!("msb-wechsel-reject-{}", payload.process_id);
            makod.post_command(&idem, &cmd).await.inspect_err(|e| {
                warn!(
                    process_id = %payload.process_id,
                    error = %e,
                    "processd MSB STP: Reject dispatch failed"
                );
            })?;
            info!(
                process_id = %payload.process_id,
                command = command_name,
                erc_code,
                "processd MSB STP: dispatched Reject command"
            );
        }
        MsbDecisionOutcome::Escalate { reason } => {
            warn!(
                process_id = %payload.process_id,
                pid = payload.pid,
                reason,
                "processd MSB STP: Escalate — enqueued for operator decision"
            );
            let (approve, reject) = (
                geraetewechsel_answer_command(payload.pid, true),
                geraetewechsel_answer_command(payload.pid, false),
            );
            let entry = ApprovalQueueEntry::pending(
                payload.process_id,
                payload.pid as i32,
                Some(payload.malo_id.clone()),
                reason.clone(),
                msb_wechsel_expires_at(payload.pid, payload.received_at),
                cfg.tenant.clone(),
            )
            .with_commands(approve.0, reject.0, Some(approve.1));
            queue.enqueue(&entry).await?;
        }
    }
    Ok(())
}

/// Operator deadline for an escalated MSB-Wechsel: the per-PID WiM Antwortfrist
/// (3 / 5 / 7 / 1 Werktage, BK6-24-174 Teil 1), minus an hour of headroom.
///
/// A warn!-only escalation let this Frist lapse unseen.
fn msb_wechsel_expires_at(pid: u32, received_at: time::OffsetDateTime) -> time::OffsetDateTime {
    use mako_engine::fristen::{HolidayCalendar, deadline_at_werktage};
    let werktage = mako_wim::antwort_frist_werktage(pid).unwrap_or(3);
    deadline_at_werktage(received_at, werktage, HolidayCalendar::BdewMaKo)
        - time::Duration::hours(1)
}

// ── M3: Preisanfrage REQOTE auto-response ──────────────────────────────────────

/// PIDs for which the MSB must auto-respond with a QUOTES message.
///
/// Single-sourced from `mako-wim`. A local copy here also listed 35003, which
/// is the ESA Werteanfrage (answered by 15003 in `esa-wertebestellung`) — so a
/// request for measurement values was answered with a PreisblattMessung quote.
use mako_wim::preisanfrage::REQOTE_PIDS;

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
/// - Escalation on missing PreisblattMessung prevents an APERAK-Frist breach from
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
                command: mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN.to_owned(),
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
        let result =
            evaluate_msb_anmeldung(&payload(55042, None), true, true, 1, Some(false), false);
        assert!(matches!(result, MsbDecisionOutcome::Accept));
    }

    #[test]
    fn anmeldung_reject_melo_not_found() {
        let result =
            evaluate_msb_anmeldung(&payload(55042, None), false, true, 1, Some(false), false);
        assert!(matches!(result, MsbDecisionOutcome::Reject { erc_code, .. } if erc_code == "A02"));
    }

    #[test]
    fn anmeldung_reject_nmsb_not_registered() {
        let result =
            evaluate_msb_anmeldung(&payload(55042, None), true, false, 1, Some(false), false);
        assert!(matches!(result, MsbDecisionOutcome::Reject { erc_code, .. } if erc_code == "A05"));
    }

    #[test]
    fn anmeldung_escalate_no_zaehler() {
        let result =
            evaluate_msb_anmeldung(&payload(55042, None), true, true, 0, Some(false), false);
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    #[test]
    fn anmeldung_escalate_sr_linked() {
        let result = evaluate_msb_anmeldung(
            &payload(55042, Some("SR-12345")),
            true,
            true,
            2,
            Some(false),
            true,
        );
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    #[test]
    fn anmeldung_escalate_ima_device() {
        let result =
            evaluate_msb_anmeldung(&payload(55042, None), true, true, 1, Some(true), false);
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    /// Unknown iMSys status must never resolve to an auto-Accept — §21 MsbG /
    /// §14a policy is a decision, not a default.
    #[test]
    fn anmeldung_escalate_when_ima_status_unknown() {
        let result = evaluate_msb_anmeldung(&payload(55042, None), true, true, 1, None, false);
        assert!(matches!(result, MsbDecisionOutcome::Escalate { .. }));
    }

    #[test]
    fn kuendigung_accept_when_valid() {
        let result = evaluate_msb_kuendigung(&payload(55039, None), true, true);
        assert!(matches!(result, MsbDecisionOutcome::Accept));
    }

    // ── Command name mapping ───────────────────────────────────────────────────
    //
    // The posted names must come from the shared `mako_markt::commands` list —
    // makod's registry test asserts every name in that list is registered, so
    // the pair of tests closes the processd → makod drift gap that previously
    // let `wim.msb-wechsel.*` (an unregistered name) fail every answer with 422.

    #[test]
    fn answer_command_anmeldung_is_nb() {
        assert_eq!(
            geraetewechsel_answer_command(55042, true),
            ("wim.geraetewechsel.bestaetigen", "NB")
        );
        assert_eq!(
            geraetewechsel_answer_command(55042, false),
            ("wim.geraetewechsel.ablehnen", "NB")
        );
    }

    #[test]
    fn answer_command_kuendigung_is_msb() {
        assert_eq!(
            geraetewechsel_answer_command(55039, true),
            ("wim.geraetewechsel.bestaetigen", "MSB")
        );
        assert_eq!(
            geraetewechsel_answer_command(55039, false),
            ("wim.geraetewechsel.ablehnen", "MSB")
        );
    }

    /// 35003 is the ESA Werteanfrage (answered by 15003 in `esa-wertebestellung`),
    /// not a Preisanfrage. A local copy of the list here drifted into answering
    /// it with a PreisblattMessung quote.
    #[test]
    fn reqote_pids_are_the_canonical_preisanfrage_set() {
        assert_eq!(REQOTE_PIDS, mako_wim::preisanfrage::REQOTE_PIDS);
        assert!(!REQOTE_PIDS.contains(&35003));
    }

    #[test]
    fn posted_commands_are_in_shared_registry_list() {
        for name in [
            geraetewechsel_answer_command(55042, true).0,
            geraetewechsel_answer_command(55039, false).0,
            mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN,
        ] {
            assert!(
                mako_markt::commands::DISPATCHED_BY_SERVICES.contains(&name),
                "{name:?} missing from mako_markt::commands::DISPATCHED_BY_SERVICES — \
                 makod's registry cross-check would not cover it"
            );
        }
    }
}
