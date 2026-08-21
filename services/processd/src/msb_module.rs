//! MSB process decision module — the Messstellenbetreiber's own answers.
//!
//! | Inbound PID | Process | Direction | Answered by | Frist |
//! |---|---|---|---|---|
//! | **55042** | WiM Anmeldung MSB | MSBN → NB | the **NB** | 5 WT |
//! | **55051** | WiM Ende MSB | MSBA → NB | the **NB** | 7 WT |
//! | **55039** | WiM Kündigung MSB | MSBN → MSBA | the **MSB** | 3 WT |
//! | **55168** | WiM Verpflichtungsanfrage | NB → gMSB | the **MSB** | 1 WT |
//! | **35001/35002/35004/35005** | REQOTE Preisanfrage | nMSB → aMSB | the **MSB** | 5 WT |
//!
//! The directions are not uniform, which is why the PID sets are two constants
//! gated by separate Cargo features: a Kündigung MSB never reaches the NB at
//! all, so an NB-role handler that answered it would be answering a message the
//! NB cannot receive.
//!
//! ```text
//! Event arrives → parse MsbWechselPayload
//!   → GET /api/v1/melos/{melo_id}                 ← marktd (does the MeLo exist?)
//!   → GET /api/v1/melos/{melo_id}/zaehler         ← marktd (meters + Zählertyp)
//!   → GET /api/v1/partners/{nmsb_mp_id}           ← marktd (nMSB registered?)
//!   → GET /api/v1/technische-ressourcen/{sr_id}   ← marktd (§14a SR linked?)
//!   → evaluate_msb_anmeldung / evaluate_msb_kuendigung
//!       Accept   → wim.geraetewechsel.bestaetigen [if auto_accept]
//!                  else approval_queue with the WiM Frist
//!       Reject   → wim.geraetewechsel.ablehnen (ERC)
//!       Escalate → approval_queue with the WiM Frist
//! ```
//!
//! Two rules the checks depend on:
//!
//! - **A transport error is not evidence of absence.** Every `marktd` lookup
//!   failure propagates so the caller answers 5xx and the fan-out redelivers;
//!   only a genuine 404 may become an `A02`. The existence check reads the
//!   **MeLo** rather than inferring it from the MaLo, because a rejection that
//!   names the wrong object is still a rejection.
//! - **§ 14a iMSys eligibility is decided from the `Zaehlertyp`**
//!   (`INTELLIGENTES_MESSSYSTEM` — three `s`; `Geraetetyp` spells the same
//!   concept with two). A MeLo that already carries one escalates: only the
//!   grundzuständige MSB may be displaced there.
//!
//! # Regulatory basis
//!
//! - **BK6-24-174** (WiM Strom) — per-PID Antwortfristen, REQOTE/QUOTES
//! - **§ 21 MsbG** — the nMSB has a right to register; the NB may reject only
//!   on enumerated grounds
//! - **§ 14a EnWG** — controllable loads require an MSB eligibility check

use tracing::{info, warn};

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the MSB module.
///
/// Carries no `marktd` connection details: the webhook path is handed an
/// already-connected client.
#[derive(Debug, Clone)]
pub struct MsbModuleConfig {
    pub own_mp_id: String,
    pub tenant: String,
    /// `[msb] auto_accept`. When `true`, an `Accept` verdict dispatches the
    /// Bestätigung itself; when `false`, it goes to the approval queue with its
    /// Frist attached.
    pub auto_accept: bool,
    /// When `true`, an inbound REQOTE is answered with a QUOTES built from the
    /// current `PreisblattMessung`. `[msb] auto_preisanfrage` in TOML.
    pub auto_preisanfrage: bool,
}

// ── Decision types ────────────────────────────────────────────────────────────

/// The WiM MSB-Wechsel PIDs **this deployment's NB role** answers.
///
/// Per `mako_wim::geraetewechsel`, directions are not uniform: 55042
/// (Anmeldung) is MSBN → NB and 55051 (Ende MSB) is MSBA → NB, so the NB owes
/// both answers. 55039 and 55168 never reach the NB.
pub const NB_ANSWERED_PIDS: &[u32] = &[55_042, 55_051];

/// The WiM MSB-Wechsel PIDs **this deployment's MSB role** answers.
///
/// 55039 (Kündigung MSB) is MSBN → MSBA — it never reaches the NB at all, so
/// routing it into an NB-role handler answers a message the NB cannot receive.
/// 55168 (Verpflichtungsanfrage) is NB → gMSB.
pub const MSB_ANSWERED_PIDS: &[u32] = &[55_039, 55_168];

/// Fields extracted from `de.mako.process.initiated` for a WiM MSB-Wechsel PID.
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
    /// Parse from a `de.mako.process.initiated` CloudEvent for any WiM
    /// MSB-Wechsel PID this deployment could be asked to answer.
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !NB_ANSWERED_PIDS.contains(&pid) && !MSB_ANSWERED_PIDS.contains(&pid) {
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

impl MsbModuleConfig {
    /// The config the webhook path needs, from the shared handler state.
    #[must_use]
    pub fn for_state(
        own_mp_id: &str,
        tenant: &str,
        auto_accept: bool,
        auto_preisanfrage: bool,
    ) -> Self {
        Self {
            own_mp_id: own_mp_id.to_owned(),
            tenant: tenant.to_owned(),
            auto_accept,
            auto_preisanfrage,
        }
    }
}

/// Outcome of an MSB-Wechsel STP evaluation.
#[derive(Debug, Clone)]
pub enum MsbDecisionOutcome {
    /// Auto-accept — nMSB eligible; NB has no valid grounds to reject.
    Accept,
    /// Auto-reject — specific ground enumerated in ERC code.
    Reject { antwortcode: String, reason: String },
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
            antwortcode: ERC_MELO_NOT_FOUND.to_owned(),
            reason: format!("MeLo {} not found in grid registry", payload.melo_id),
        };
    }

    // Check 2: nMSB must be registered in partner directory.
    if !nmsb_registered {
        return MsbDecisionOutcome::Reject {
            antwortcode: ERC_NMSB_NOT_REGISTERED.to_owned(),
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
            antwortcode: ERC_MELO_NOT_FOUND.to_owned(),
            reason: format!("MeLo {} not found in grid registry", payload.melo_id),
        };
    }
    if !nmsb_registered {
        return MsbDecisionOutcome::Reject {
            antwortcode: ERC_NMSB_NOT_REGISTERED.to_owned(),
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
    let marktrolle = if NB_ANSWERED_PIDS.contains(&pid) {
        "NB"
    } else {
        "MSB"
    };
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
/// of absence: treating it as one dispatches a wrongful A02 "MeLo not found"
/// rejection into the market against a valid §21 MsbG registration.
pub async fn handle_msb_wechsel(
    cfg: &MsbModuleConfig,
    payload: MsbWechselPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<()> {
    // ── Query marktd in parallel ──────────────────────────────────────────────
    let (melo_result, nmsb_known, zaehler_result, sr_result) = tokio::join!(
        async {
            if payload.melo_id.is_empty() {
                // No MeLo on the order: fall back to the MaLo, which is the only
                // location this message names. `Ok(None)` is the 404.
                marktd
                    .get_versorgung(&payload.malo_id)
                    .await
                    .map(|v| v.is_some())
            } else {
                marktd.melo_known(&payload.melo_id).await
            }
        },
        marktd.partner_known(&payload.nmsb_mp_id),
        async {
            if payload.melo_id.is_empty() {
                Ok(Vec::new())
            } else {
                marktd.list_zaehler(&payload.melo_id).await
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

    // Every `?` here propagates a *transport* failure so the caller answers 5xx
    // and the fan-out redelivers. A transport error is not evidence of absence:
    // treating it as one dispatches a wrongful A02 into the market against a
    // valid § 21 MsbG registration.
    let melo_exists = melo_result?;
    let nmsb_registered = nmsb_known?;
    let zaehler = zaehler_result?;
    let zaehler_count = u32::try_from(zaehler.len()).unwrap_or(u32::MAX);
    let sr_linked = sr_result?.is_some();

    // § 14a / § 21 MsbG turns on whether the MeLo already carries an iMSys, so
    // the Zählertyp decides it. `None` only when the registry lists no meter at
    // all — check 3 stops there anyway — so the eligibility question is now
    // answered from data instead of escalating unconditionally.
    let is_ima_device = if zaehler.is_empty() {
        None
    } else {
        Some(
            zaehler
                .iter()
                .any(mako_markt::marktd_client::ZaehlerSummary::ist_imsys),
        )
    };

    // ── Evaluate ──────────────────────────────────────────────────────────────
    //
    // Only the Anmeldung (55042) and the Kündigung (55039) have STP rules that
    // this codebase can state from the AHB. The Ende MSB (55051) and the
    // Verpflichtungsanfrage (55168) are escalated to the operator with their
    // own answer window rather than auto-decided: inventing a rule for them
    // would put an unfounded Bestätigung or Ablehnung on the market, and doing
    // nothing at all — which is what happened before, since neither PID was
    // routed anywhere — leaves the message unanswered past its Frist.
    let outcome = match payload.pid {
        55_042 => evaluate_msb_anmeldung(
            &payload,
            melo_exists,
            nmsb_registered,
            zaehler_count,
            is_ima_device,
            sr_linked,
        ),
        55_039 => evaluate_msb_kuendigung(&payload, melo_exists, nmsb_registered),
        pid => MsbDecisionOutcome::Escalate {
            reason: format!(
                "PID {pid} ({}) has no automatable decision rule — operator must answer \
                 within {} Werktage (WiM Strom Teil 1)",
                msb_wechsel_process_name(pid),
                mako_wim::antwort_frist_werktage(pid).unwrap_or(0),
            ),
        },
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
            if !cfg.auto_accept {
                // auto_accept off is "an operator decides", not "nobody
                // answers": without a queue row the order goes unanswered and
                // unseen past its WiM Antwortfrist.
                let (approve, reject) = (
                    geraetewechsel_answer_command(payload.pid, true),
                    geraetewechsel_answer_command(payload.pid, false),
                );
                let window = crate::fristen::operator_window(payload.pid, payload.received_at);
                let entry = ApprovalQueueEntry::pending(
                    payload.process_id,
                    payload.pid as i32,
                    Some(payload.malo_id.clone()),
                    format!(
                        "auto_accept disabled — STP says Accept for {} (Antwortfrist {})",
                        msb_wechsel_process_name(payload.pid),
                        window.deadline
                    ),
                    window.expires_at,
                    cfg.tenant.clone(),
                )
                .with_commands(approve.0, reject.0, Some(approve.1));
                queue.enqueue(&entry).await?;
            } else {
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
        MsbDecisionOutcome::Reject {
            antwortcode,
            reason,
        } => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                antwortcode,
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
                    "antwortcode": antwortcode,
                    // makod's APERAK dispatch forwards `reason`; carry the ERC
                    // code inside it so the rejection ground survives the hop.
                    "reason": format!("{antwortcode}: {reason}"),
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
                antwortcode,
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

/// Human-readable name for a WiM MSB-Wechsel PID, for operator-facing reasons.
fn msb_wechsel_process_name(pid: u32) -> &'static str {
    match pid {
        55_039 => "Kündigung MSB",
        55_042 => "Anmeldung MSB",
        55_051 => "Ende MSB",
        55_168 => "Verpflichtungsanfrage",
        _ => "unknown MSB-Wechsel process",
    }
}

/// Operator deadline for an escalated MSB-Wechsel: the per-PID WiM Antwortfrist
/// (3 / 5 / 7 / 1 Werktage, BK6-24-174 Teil 1), less an hour of headroom.
///
/// A `warn!`-only escalation let this Frist lapse unseen.
fn msb_wechsel_expires_at(pid: u32, received_at: time::OffsetDateTime) -> time::OffsetDateTime {
    crate::fristen::operator_window(pid, received_at).expires_at
}

// ── M3: Preisanfrage REQOTE auto-response ──────────────────────────────────────

/// PIDs for which the MSB must auto-respond with a QUOTES message.
///
/// Single-sourced from `mako-wim`. A local copy here also listed 35003, which
/// is the ESA Werteanfrage (answered by 15003 in `esa-wertebestellung`) — so a
/// request for measurement values was answered with a PreisblattMessung quote.
use mako_wim::preisanfrage::REQOTE_PIDS;

/// Answer an inbound REQOTE Preisanfrage (PIDs 35001/35002/35004/35005,
/// nMSB → aMSB) with a QUOTES built from the current `PreisblattMessung`.
///
/// ## Every branch ends somewhere
///
/// Answering HTTP 200 on a path that automated nothing lets the fan-out mark
/// the event delivered while the five-Werktage window runs out with no queue
/// row and no operator surface. Each outcome therefore has a distinct ending:
///
/// | Situation | Ending |
/// |---|---|
/// | `auto_preisanfrage = false` | approval-queue entry with the WiM Frist |
/// | No active `PreisblattMessung` | approval-queue entry — an operator must quote |
/// | `marktd` unreachable | `Err` → the caller answers 5xx and the fan-out redelivers |
/// | `makod` dispatch failed | `Err` → same; the QUOTES has not gone out |
/// | Quote dispatched | `Ok(true)` |
///
/// ## Returns
///
/// `Ok(true)` when the event was handled, `Ok(false)` when the PID is not a
/// REQOTE Preisanfrage. `Err` means *retry*, never *decided*.
///
/// ## Regulatory basis
///
/// BK6-24-174 (WiM Strom), REQOTE/QUOTES AHB 1.2 — the aMSB answers within
/// [`mako_wim::PREISANFRAGE_ANTWORT_FRIST_WT`] Werktage.
pub async fn handle_preisanfrage_reqote(
    event: &serde_json::Value,
    cfg: &MsbModuleConfig,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<bool> {
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| event["data"]["pid"].as_u64())
        .unwrap_or(0) as u32;

    if !REQOTE_PIDS.contains(&pid) {
        return Ok(false);
    }

    // The subject is the makod process UUID. A non-UUID is a broken producer
    // contract: acking it would drop the answer obligation silently.
    let Ok(process_id) = event["subject"]
        .as_str()
        .unwrap_or("")
        .parse::<uuid::Uuid>()
    else {
        anyhow::bail!(
            "REQOTE (PID {pid}) CloudEvent subject {:?} is not a process UUID",
            event["subject"]
        );
    };

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

    let received_at = event["time"]
        .as_str()
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(time::OffsetDateTime::now_utc);

    // Escalation is an approval-queue row carrying the WiM answer Frist and the
    // command that sends the quote, so an operator can dispatch it from the
    // queue rather than reconstructing it in the ERP.
    let escalate = async |reason: String| -> anyhow::Result<bool> {
        let window = crate::fristen::operator_window(pid, received_at);
        let entry = ApprovalQueueEntry::pending(
            process_id,
            pid as i32,
            None,
            format!(
                "{reason} (Antwortfrist {}: {})",
                window.deadline, window.source
            ),
            window.expires_at,
            cfg.tenant.clone(),
        )
        .with_approve_command(
            mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN,
            Some("MSB"),
        );
        queue.enqueue(&entry).await?;
        warn!(
            %process_id, pid, %melo_id, %nmsb_mp_id,
            deadline = %window.deadline,
            "processd MSB: REQOTE escalated to the approval queue"
        );
        Ok(true)
    };

    if !cfg.auto_preisanfrage {
        return escalate(
            "auto_preisanfrage disabled — the QUOTES is dispatched on operator approval".to_owned(),
        )
        .await;
    }

    // A marktd outage is not a business finding: only a genuine *absence* of a
    // PreisblattMessung may escalate. A transport error propagates so the
    // fan-out redelivers.
    let today = time::OffsetDateTime::now_utc().date();
    let preisblatt = marktd
        .get_preisblatt_messung(&cfg.own_mp_id, today)
        .await
        .map_err(|e| {
            warn!(error = %e, own_mp_id = %cfg.own_mp_id, %process_id,
                  "processd MSB: PreisblattMessung lookup failed — fan-out will redeliver");
            anyhow::anyhow!("marktd PreisblattMessung lookup failed: {e}")
        })?;

    let Some(preisblatt) = preisblatt else {
        return escalate(format!(
            "no PreisblattMessung is in force for aMSB {} on {today} — a QUOTES with no \
             prices must not go out automatically",
            cfg.own_mp_id
        ))
        .await;
    };

    let cmd = mako_markt::makod_client::ForwardCommand {
        command: mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN.to_owned(),
        marktrolle: Some("MSB".to_owned()),
        malo_id: None,
        melo_id: (!melo_id.is_empty()).then(|| melo_id.clone()),
        payload: serde_json::json!({
            "process_id": process_id,
            "auto_response": true,
            "source_pid": pid,
            // Forward the Gueltigkeit so makod can build the QUOTES.
            "preisblatt_gueltigkeit": preisblatt
                .gueltigkeit
                .as_ref()
                .map(|g| serde_json::to_value(g).unwrap_or_default()),
        }),
    };
    let resp = makod
        .post_command(&format!("preisanfrage-angebot-{process_id}"), &cmd)
        .await
        .map_err(|e| {
            warn!(error = %e, %process_id, pid,
                  "processd MSB: QUOTES dispatch failed — fan-out will redeliver");
            anyhow::anyhow!("makod QUOTES dispatch failed: {e}")
        })?;

    info!(
        %process_id, pid, %melo_id, %nmsb_mp_id,
        response_process_id = %resp.process_id,
        "processd MSB: auto-dispatched QUOTES (wim.preisanfrage.angebot-senden)"
    );
    Ok(true)
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
        assert!(
            matches!(result, MsbDecisionOutcome::Reject { antwortcode, .. } if antwortcode == "A02")
        );
    }

    #[test]
    fn anmeldung_reject_nmsb_not_registered() {
        let result =
            evaluate_msb_anmeldung(&payload(55042, None), true, false, 1, Some(false), false);
        assert!(
            matches!(result, MsbDecisionOutcome::Reject { antwortcode, .. } if antwortcode == "A05")
        );
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
    // The posted names must come from the shared `mako_markt::commands` list;
    // makod's registry test asserts every name in that list is registered. The
    // pair of tests is what keeps processd from posting a name makod rejects
    // with 422.

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
