//! GPKE LF-side Anmeldung workflow — Lieferbeginn, Lieferende, Kündigung.
//!
//! When `makod` operates as a **Lieferant (LF)**, the ERP instructs the engine
//! to initiate a GPKE process outbound to the Netzbetreiber (NB) or the old
//! Lieferant (LFA for Kündigung). This module implements that LF-side
//! outbound-first workflow.
//!
//! ## Process flow
//!
//! ```text
//! ERP → POST /api/v1/commands          (gpke.lieferbeginn.anmelden, role LF)
//!       ↓
//!   [InitiateAnmeldung]
//!       ↓ emits Initiated + UTILMD outbox entry
//! AS4 sender → UTILMD 55001 to NB
//!       ↓
//! AS4 inbound ← UTILMD 55002/55003 from NB
//!       ↓
//!   [HandleAntwort]
//!       ↓ emits AntwortReceived
//!       ↓ (on acceptance) emits Activated
//! ERP webhook ← ErpEvent::AperakAccepted / AperakRejected (via outbox)
//! ```
//!
//! ## Prüfidentifikatoren
//!
//! | Outbound (LF → NB)               | PID   | Inbound response (NB → LF) | PID   |
//! |----------------------------------|-------|-------------------------------|-------|
//! | Anmeldung verb. MaLo             | 55001 | Bestätigung Anmeldung         | 55002 |
//! |                                  |       | Ablehnung Anmeldung           | 55003 |
//! | Abmeldung                        | 55004 | Bestätigung Abmeldung         | 55005 |
//! |                                  |       | Ablehnung Abmeldung           | 55006 |
//! | Kündigung Lieferbeginn           | 55016 | Bestätigung Kündigung        | 55017 |
//! |                                  |       | Ablehnung Kündigung          | 55018 |
//! | Anmeldung Lieferbeginn erz. MaLo | 55077 | Bestätigung erz. MaLo         | 55078 |
//! |                                  |       | Ablehnung erz. MaLo           | 55080 |
//!
//! ## Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **BK6-22-024** — BNetzA ruling; NB must respond within **24 wall-clock hours**
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE LF-side Anmeldung workflow.
///
/// Used as the `workflow_name` in [`WorkflowId`] when spawning a new process
/// and as the registration key in the PID router (see [`GpkeModule`]).
///
/// [`WorkflowId`]: mako_engine::version::WorkflowId
/// [`GpkeModule`]: crate::GpkeModule
pub const WORKFLOW_NAME: &str = "gpke-lf-anmeldung";

/// Outbound request PIDs that trigger a new `GpkeLfAnmeldungWorkflow` process
/// when the ERP calls `POST /api/v1/commands`.
///
/// These are LF→NB/LFA direction only; the corresponding NB→LF response PIDs
/// ([`ANTWORT_PIDS_LF`]) complete the conversation.
pub const ANFRAGE_PIDS_LF: &[u32] = &[
    55001, // Anmeldung verb. MaLo (LF → NB)
    55004, // Abmeldung            (LF → NB)
    55016, // Kündigung Lieferbeginn (LFN → LFA)
    55077, // Anmeldung Lieferbeginn erz. MaLo (LFN → NB, BK6-24-174)
];

/// Inbound response PIDs (NB → LF or LFA → LF) routed back to this workflow.
///
/// These must be registered in the PID router so the AS4 inbound layer can
/// route them by conversation ID to the correct `GpkeLfAnmeldungWorkflow`
/// instance.
pub const ANTWORT_PIDS_LF: &[u32] = &[
    55002, // Bestätigung Anmeldung verb. MaLo (NB → LF)
    55003, // Ablehnung Anmeldung verb. MaLo   (NB → LF)
    55005, // Bestätigung Abmeldung            (NB → LF)
    55006, // Ablehnung Abmeldung              (NB → LF)
    55017, // Bestätigung Kündigung                (LFA → LFN)
    55018, // Ablehnung Kündigung                  (LFA → LFN)
    55078, // Bestätigung Anmeldung erz. MaLo       (NB → LFN)
    55080, // Ablehnung Anmeldung erz. MaLo         (NB → LFN); 55079 unassigned
];

/// Deadline label for the NB/LFA response window.
///
/// Sized by `mako_fristen::antwort` — a clock time on the 1. Werktag
/// after the ÜT, per PID. Never a flat 24 hours: that approximation expires a
/// Friday arrival on Saturday and calls a Tuesday-evening one healthy nine
/// hours after its Frist lapsed.
///
/// After sending the outbound ANFRAGE, the LF registers a deadline with this
/// label. If no ANTWORT arrives within 24 wall-clock hours, the scheduler
/// fires `on_deadline` → `TimeoutExpired` to transition the process to
/// `Rejected`.
///
/// ```rust,ignore
/// let due = mako_fristen::antwort::antwort_deadline(pid, sent_at)
///     .expect("a PID with a published Antwortfrist");
/// let deadline = Deadline::new(process.stream_id().clone(), ..., NB_RESPONSE_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const NB_RESPONSE_WINDOW_LABEL: &str = "nb-response-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE LF-side Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum LfAnmeldungEvent {
    /// LF-side Anmeldung initiated — outbound UTILMD queued for AS4 delivery.
    Initiated {
        /// PID of the outbound Anfrage (55001, 55004, 55016, or 55077).
        pruefidentifikator: Pruefidentifikator,
        /// MaLo / supply point identifier.
        location_id: MaLo,
        /// Our own market-partner ID (the Lieferant).
        sender: MarktpartnerCode,
        /// Counterparty market-partner ID (NB or LFA).
        receiver: MarktpartnerCode,
        /// Requested supply start / end / cancellation date.
        process_date: String,
    },
    /// Counterparty (NB or LFA) responded — accepted or rejected.
    AntwortReceived {
        /// PID of the inbound response (55002/55003, 55005/55006, 55017, 55018, 55078, 55080).
        response_pid: Pruefidentifikator,
        /// `true` if the request was accepted.
        accepted: bool,
        /// Rejection reason (only set when `accepted = false`).
        reason: Option<String>,
        /// Message reference from the inbound response UTILMD.
        response_ref: MessageRef,
    },
    /// Accepted Lieferbeginn Anmeldung activated (supply now live).
    Activated,
    /// A registered deadline expired before the NB responded.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for LfAnmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "LfAnmeldungInitiated",
            Self::AntwortReceived { .. } => "LfAnmeldungAntwortReceived",
            Self::Activated => "LfAnmeldungActivated",
            Self::DeadlineExpired { .. } => "LfAnmeldungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `Initiated` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfAnmeldungData {
    /// PID of the outbound Anfrage (55001, 55004, 55016, or 55077).
    pub pruefidentifikator: Pruefidentifikator,
    /// MaLo / supply point identifier.
    pub location_id: MaLo,
    /// Our own market-partner ID (the Lieferant).
    pub sender: MarktpartnerCode,
    /// Counterparty market-partner ID (NB or LFA).
    pub receiver: MarktpartnerCode,
    /// Requested supply start / end / cancellation date.
    pub process_date: String,
}

/// Process state for the GPKE LF-side Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub enum LfAnmeldungState {
    /// Initial state before `InitiateAnmeldung` is received.
    #[default]
    New,
    /// UTILMD Anfrage sent; awaiting NB/LFA response.
    Pending(LfAnmeldungData),
    /// NB or LFA accepted the Anmeldung; supply active (Lieferbeginn only).
    Active(LfAnmeldungData),
    /// NB rejected or deadline expired.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for LfAnmeldungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Awaiting the NB's answer, or supply is running — a second
            // Anmeldung for the same MaLo would put two live processes on one
            // supply point.
            Self::Pending(_) | Self::Active(_) => true,
            // `New` never started. `Rejected` is terminal: the NB refused, and
            // the LF's corrected Anmeldung is a normal GPKE flow that must be
            // allowed rather than answered with `duplicate_process`.
            Self::New | Self::Rejected { .. } => false,
        }
    }
}

impl LfAnmeldungState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Pending(_) => "Pending",
            Self::Active(_) => "Active",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE LF-side Anmeldung workflow.
#[derive(Clone)]
pub enum LfAnmeldungCommand {
    /// ERP instructs the engine to initiate a Lieferbeginn, Lieferende, or
    /// Kündigung Anmeldung as the Lieferant.
    ///
    /// The engine:
    /// 1. Records the `Initiated` event.
    /// 2. Enqueues a `PendingOutbox` entry with `message_type = "UTILMD"` and
    ///    the structured domain payload. The AS4 sender serialises this to
    ///    wire-format EDIFACT and delivers it to the NB via AS4.
    InitiateAnmeldung {
        /// Outbound request PID (55001, 55004, 55016, or 55077).
        pid: Pruefidentifikator,
        /// Our own market-partner ID (the Lieferant, from `--tenant-id`).
        sender: MarktpartnerCode,
        /// Counterparty market-partner ID (NB or LFA), resolved from the MaLo cache.
        receiver: MarktpartnerCode,
        /// Supply point identifier.
        location_id: MaLo,
        /// Requested process date (Lieferbeginn-/Lieferende-/Kündigungs-Datum).
        process_date: String,
        /// SG4 STS Transaktionsgrund (DE9013) — `E01` Ein-/Auszug,
        /// `E03` Wechsel. Rendered as the outbound STS segment.
        transaktionsgrund: Option<String>,
    },
    /// Inbound NB/LFA response (55002/55003, 55005/55006, 55017, 55018, 55078, 55080) received via AS4.
    ///
    /// Dispatched by the AS4 inbound layer after extracting the domain fields
    /// from the UTILMD response message.
    HandleAntwort {
        /// PID of the inbound response.
        response_pid: Pruefidentifikator,
        /// `true` if the NB accepted; `false` if rejected.
        accepted: bool,
        /// Optional rejection reason from the UTILMD text segment.
        reason: Option<String>,
        /// Message reference from the inbound response.
        response_ref: MessageRef,
    },
    /// Mark the accepted Lieferbeginn supply relationship as active.
    ///
    /// Typically dispatched after the ERP confirms supply activation downstream.
    Activate,
    /// A registered deadline fired (NB did not respond within 24h wall-clock).
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for LfAnmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE LF-side outbound Anmeldung workflow.
///
/// Handles the Lieferant's perspective of initiating a supplier-change process:
/// sends the UTILMD Anfrage to the NB and tracks the NB's response.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeLfAnmeldungWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeLfAnmeldungWorkflow;

impl Workflow for GpkeLfAnmeldungWorkflow {
    type State = LfAnmeldungState;
    type Event = LfAnmeldungEvent;
    type Command = LfAnmeldungCommand;

    /// Deadline compensation for the NB/LFA response window (24h, BK6-22-024).
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"nb-response-window"` | `Pending` | `TimeoutExpired` | BK6-22-024 — 24h wall-clock Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (NB_RESPONSE_WINDOW_LABEL, LfAnmeldungState::Pending(_)) => {
                Some(LfAnmeldungCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            LfAnmeldungEvent::Initiated {
                pruefidentifikator,
                location_id,
                sender,
                receiver,
                process_date,
            } => LfAnmeldungState::Pending(LfAnmeldungData {
                pruefidentifikator: *pruefidentifikator,
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
            }),
            LfAnmeldungEvent::AntwortReceived {
                accepted, reason, ..
            } => {
                if *accepted {
                    match state {
                        LfAnmeldungState::Pending(data) => LfAnmeldungState::Active(data),
                        other => other,
                    }
                } else {
                    LfAnmeldungState::Rejected {
                        reason: reason.clone().unwrap_or_else(|| "Ablehnung".to_owned()),
                    }
                }
            }
            LfAnmeldungEvent::Activated => match state {
                LfAnmeldungState::Active(data) => LfAnmeldungState::Active(data),
                other => other,
            },
            LfAnmeldungEvent::DeadlineExpired { label, .. } => match state {
                LfAnmeldungState::Active(_) | LfAnmeldungState::Rejected { .. } => state,
                _ => LfAnmeldungState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            LfAnmeldungCommand::InitiateAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
            } => {
                if !matches!(state, LfAnmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ANFRAGE_PIDS_LF.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an LF Anfrage PID (55001, 55004, 55016, 55077), got {pid}",
                    )));
                }

                let event = LfAnmeldungEvent::Initiated {
                    pruefidentifikator: pid,
                    location_id: location_id.clone(),
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    process_date: process_date.clone(),
                };

                // Enqueue the outbound UTILMD as a PendingOutbox entry.
                //
                // The AS4 sender in `makod` picks up entries with
                // `message_type = "UTILMD"` and serialises the payload
                // to wire-format EDIFACT before handing it to the AS4
                // transport layer.
                // `document_date` and `message_ref` are intentionally omitted:
                // the renderer derives them at dispatch time (today / causation_event_id).
                let outbox = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":         "outbound",
                        "pid":               pid.as_u32(),
                        "sender":            sender.as_str(),
                        "receiver":          receiver.as_str(),
                        "malo":              location_id.as_str(),
                        "process_date":      process_date,
                        // SG4 STS Transaktionsgrund (E01 Ein-/Auszug, E03
                        // Wechsel) — rendered as the outbound STS segment.
                        "transaktionsgrund": transaktionsgrund,
                    }),
                );

                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            LfAnmeldungCommand::HandleAntwort {
                response_pid,
                accepted,
                reason,
                response_ref,
            } => {
                if !matches!(state, LfAnmeldungState::Pending(_)) {
                    return Err(WorkflowError::invalid_state("Pending", state.label()));
                }
                if !ANTWORT_PIDS_LF.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an LF Antwort PID (55002/55003, 55005/55006, 55017, 55018, 55078, 55080), got {response_pid}",
                    )));
                }

                // Carry malo_id so marktd's event_ingest can derive
                // VersorgungsStatus without relying on the CE subject (a process
                // UUID). Mirrors the GeLi Gas LFN workflow — without this outbox
                // entry no `de.mako.process.completed` is ever emitted for a
                // Strom supplier change, so `lf_mp_id_next` stays announced and
                // never gets promoted to the active Lieferant.
                let malo_id_str = match state {
                    LfAnmeldungState::Pending(data) => data.location_id.as_str().to_owned(),
                    _ => String::new(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ProcessCompleted",
                    "",
                    serde_json::json!({
                        "pid":      response_pid.as_u32(),
                        "malo_id":  malo_id_str,
                        "accepted": accepted,
                        "outcome":  if accepted { "accepted" } else { "rejected" },
                    }),
                )];

                Ok(WorkflowOutput::with_outbox(
                    vec![LfAnmeldungEvent::AntwortReceived {
                        response_pid,
                        accepted,
                        reason,
                        response_ref,
                    }],
                    outbox,
                ))
            }

            LfAnmeldungCommand::Activate => {
                if !matches!(state, LfAnmeldungState::Active(_)) {
                    return Err(WorkflowError::invalid_state("Active", state.label()));
                }
                // Already active — emit a no-op or idempotent event.
                // In practice Activate is sent after ERP confirms supply.
                Ok(vec![LfAnmeldungEvent::Activated].into())
            }

            LfAnmeldungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    LfAnmeldungState::Active(_) | LfAnmeldungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![LfAnmeldungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{
        types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
        workflow::Workflow,
    };

    use super::*;

    fn make_initiate(pid: u32) -> LfAnmeldungCommand {
        LfAnmeldungCommand::InitiateAnmeldung {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            location_id: MaLo::new("10001234558"),
            process_date: "2026-10-01".to_owned(),
            transaktionsgrund: None,
        }
    }

    #[test]
    fn initiate_lieferbeginn_transitions_to_pending() {
        let state = LfAnmeldungState::New;
        let out = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55001)).unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1, "must enqueue UTILMD outbox entry");

        let new_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, LfAnmeldungState::Pending(_)));
    }

    #[test]
    fn initiate_abmeldung_transitions_to_pending() {
        let state = LfAnmeldungState::New;
        let out = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55004)).unwrap();
        let new_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, LfAnmeldungState::Pending(_)));
    }

    #[test]
    fn initiate_kuendigung_transitions_to_pending() {
        let state = LfAnmeldungState::New;
        let out = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55016)).unwrap();
        let new_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, LfAnmeldungState::Pending(_)));
    }

    #[test]
    fn nb_acceptance_transitions_to_active() {
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        let cmd = LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55002).unwrap(), // Bestätigung Anmeldung
            accepted: true,
            reason: None,
            response_ref: MessageRef::new("NB-RESP-001"),
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        let final_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(final_state, LfAnmeldungState::Active(_)));
    }

    /// `HandleAntwort` must enqueue the `ProcessCompleted` outbox entry that
    /// drives marktd's `VersorgungsStatus` transition.
    ///
    /// Without it no `de.mako.process.completed` is emitted for a Strom
    /// supplier change, so `lf_mp_id_next` is announced and never promoted —
    /// the MaLo stays `Unbeliefert` forever. The Gas twin
    /// (`geli-gas-lf-anmeldung`) always emitted this; GPKE did not.
    ///
    /// marktd keys on the PID: 55002 confirms, 55003 clears the announcement.
    #[test]
    fn handle_antwort_emits_process_completed_for_marktd() {
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        for (response_pid, accepted, outcome) in
            [(55002u32, true, "accepted"), (55003, false, "rejected")]
        {
            let out = GpkeLfAnmeldungWorkflow::handle(
                &state,
                LfAnmeldungCommand::HandleAntwort {
                    response_pid: Pruefidentifikator::new(response_pid).unwrap(),
                    accepted,
                    reason: None,
                    response_ref: MessageRef::new("NB-RESP-001"),
                },
            )
            .unwrap();

            assert_eq!(out.outbox.len(), 1, "{response_pid}: one outbox entry");
            let entry = &out.outbox[0];
            assert_eq!(entry.message_type.as_ref(), "ProcessCompleted");
            assert_eq!(
                entry.payload["pid"].as_u64().unwrap(),
                u64::from(response_pid)
            );
            assert_eq!(
                entry.payload["malo_id"].as_str().unwrap(),
                "10001234558",
                "{response_pid}: marktd resolves the MaLo from the payload, not the CE subject",
            );
            assert_eq!(entry.payload["outcome"].as_str().unwrap(), outcome);
        }
    }

    #[test]
    fn nb_rejection_transitions_to_rejected() {
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        let cmd = LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55003).unwrap(), // Ablehnung Anmeldung
            accepted: false,
            reason: Some("MaLo nicht in Netzgebiet".to_owned()),
            response_ref: MessageRef::new("NB-RESP-002"),
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        let final_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(final_state, LfAnmeldungState::Rejected { .. }));
    }

    #[test]
    fn invalid_pid_is_rejected() {
        let state = LfAnmeldungState::New;
        // 55003 is a response PID, not an Anfrage PID
        let err = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55003));
        assert!(err.is_err());
    }

    #[test]
    fn timeout_on_pending_transitions_to_rejected() {
        use mako_engine::ids::DeadlineId;
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        let cmd = LfAnmeldungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: "nb-response-window".into(),
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        let final_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(final_state, LfAnmeldungState::Rejected { .. }));
    }

    #[test]
    fn timeout_on_active_is_noop() {
        use mako_engine::ids::DeadlineId;
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);
        let accepted_event = LfAnmeldungEvent::AntwortReceived {
            response_pid: Pruefidentifikator::new(55003).unwrap(),
            accepted: true,
            reason: None,
            response_ref: MessageRef::new("REF-002"),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(state, &accepted_event);
        assert!(matches!(state, LfAnmeldungState::Active(_)));

        let cmd = LfAnmeldungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: "nb-response-window".into(),
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 0, "timeout is no-op on Active");
    }
}
