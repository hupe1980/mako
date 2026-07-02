//! GeLi Gas LF-side Stornierung workflow — cancellation of supply-change requests.
//!
//! When `makod` operates as a **Lieferant (LFN or LFA)**, the ERP instructs the
//! engine to initiate a Stornierung request outbound to the GNB (PID 44022).
//! This module implements that LF-side outbound-first workflow.
//!
//! ## Process flow
//!
//! ```text
//! ERP → POST /api/v1/commands          (geli-gas.stornierung.initiieren, role LF)
//!       ↓
//!   [InitiateStornierung]
//!       ↓ emits Initiated + UTILMD G outbox entry (PID 44022)
//! AS4 sender → UTILMD G 44022 to GNB
//!       ↓
//! AS4 inbound ← UTILMD G 44023/44024 from GNB  (within 10 Werktage)
//!       ↓
//!   [HandleAntwort]
//!       ↓ emits AntwortReceived
//! ERP webhook ← ErpEvent::StornierungAccepted / StornierungRejected (via outbox)
//! ```
//!
//! ## Prüfidentifikatoren
//!
//! | Direction               | PID   | Description                          |
//! |-------------------------|-------|--------------------------------------|
//! | Outbound (LF → GNB)     | 44022 | Anfrage nach Stornierung             |
//! | Inbound (GNB → LF)      | 44023 | Bestätigung Stornierung (accepted)   |
//! | Inbound (GNB → LF)      | 44024 | Ablehnung Stornierung (rejected)     |
//!
//! ## BGM qualifier semantics
//!
//! The BGM 1001 qualifier in the outbound PID 44022 message encodes the **type
//! of the original message being cancelled**:
//! - `E01` — cancelling an Anmeldung (Lieferbeginn Gas)
//! - `E02` — cancelling an Abmeldung (Lieferende Gas)
//! - `E35` — cancelling a Kündigung Lieferbeginn Gas
//!
//! ## Regulatory basis
//!
//! - **BDEW UTILMD AHB Gas 1.1 / 1.2** — AHB rules for PIDs 44022–44024
//! - **BNetzA BK7-24-01-009** — GeLi Gas 3.0; GNB must respond within **10 Werktage**
//! - **APERAK Frist: 10 Werktage** (BdewMaKo calendar, German local time)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name for the GeLi Gas LF-side Stornierung workflow.
///
/// Used as the `workflow_name` in [`WorkflowId`] when spawning a new process
/// and as the registration key in the PID router (see [`GeliGasModule`]).
///
/// [`WorkflowId`]: mako_engine::version::WorkflowId
/// [`GeliGasModule`]: crate::GeliGasModule
pub const WORKFLOW_NAME: &str = "geli-gas-stornierung-lf";

/// Outbound request PID that triggers a new `GeliGasLfStornierungWorkflow` process.
///
/// The LF (LFN or LFA) sends PID 44022 (Anfrage nach Stornierung) to the GNB
/// to cancel a previously submitted Anmeldung, Abmeldung, or Kündigung.
/// This PID is **not** registered in the PID router for inbound routing — it is
/// ERP-initiated via `POST /api/v1/commands` and queued for outbound AS4 delivery.
pub const ANFRAGE_PID_LF: u32 = 44022;

/// Inbound response PIDs (GNB → LF) routed back to this workflow.
///
/// These must be registered in the PID router so the AS4 inbound layer can
/// route them by conversation ID to the correct `GeliGasLfStornierungWorkflow`
/// instance (see `GeliGasModule::register_pids_with_roles`).
pub const ANTWORT_PIDS_LF: &[u32] = &[
    44023, // Bestätigung Stornierung (GNB accepted the cancellation)
    44024, // Ablehnung Stornierung   (GNB rejected the cancellation)
];

/// Deadline label for the GNB response window (10 Werktage, BK7-24-01-009).
///
/// After sending the outbound 44022, the LF registers a deadline with this label.
/// If no 44023/44024 arrives within 10 Werktage, the scheduler fires
/// `on_deadline` → `TimeoutExpired` to transition the process to `Rejected`.
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     sent_at, 10, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(
///     stream_id, deadline_id, GNB_RESPONSE_WINDOW_LABEL, due,
/// );
/// deadline_store.register(&deadline).await?;
/// ```
pub const GNB_RESPONSE_WINDOW_LABEL: &str = "geli-gas-stornierung-lf-response-10-werktage";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas LF-side Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum LfStornierungEvent {
    /// LF-side Stornierung initiated — outbound UTILMD G 44022 queued for AS4 delivery.
    Initiated {
        /// Prüfidentifikator of the outbound Anfrage (must be 44022).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the message sender (LFN / LFA — our own tenant GLN).
        sender: MarktpartnerCode,
        /// GLN of the GNB receiving the Stornierung request.
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from IDE+24 — identifies the original process being cancelled.
        vorgang_id: MaLo,
        /// BGM 1001 qualifier encoding the original message type (`E01`/`E02`/`E35`).
        bgm_qualifier: String,
    },
    /// GNB responded — accepted (44023) or rejected (44024).
    AntwortReceived {
        /// PID of the inbound response (44023 or 44024).
        response_pid: Pruefidentifikator,
        /// `true` if the GNB accepted the cancellation (PID 44023).
        accepted: bool,
        /// Rejection reason (only set when `accepted = false`).
        reason: Option<String>,
        /// EDIFACT message reference from the inbound response UNH.
        response_ref: MessageRef,
    },
    /// A registered deadline expired before the GNB responded.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for LfStornierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "LfStornierungInitiated",
            Self::AntwortReceived { .. } => "LfStornierungAntwortReceived",
            Self::DeadlineExpired { .. } => "LfStornierungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `Initiated` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfStornierungData {
    /// Prüfidentifikator of the outbound Anfrage (44022).
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the message sender (LFN / LFA).
    pub sender: MarktpartnerCode,
    /// GLN of the GNB.
    pub receiver: MarktpartnerCode,
    /// Vorgangsnummer from IDE+24 of the original process.
    pub vorgang_id: MaLo,
    /// BGM 1001 qualifier (`E01` / `E02` / `E35`).
    pub bgm_qualifier: String,
}

/// Process state for the GeLi Gas LF-side Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LfStornierungState {
    /// Initial state before `InitiateStornierung` is received.
    New,
    /// UTILMD G 44022 sent; awaiting GNB response within 10 Werktage.
    Pending(LfStornierungData),
    /// GNB accepted the cancellation (PID 44023 received).
    Accepted(LfStornierungData),
    /// GNB rejected or deadline expired.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl LfStornierungState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Pending(_) => "Pending",
            Self::Accepted(_) => "Accepted",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Returns `true` for terminal states where no further commands are expected.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Accepted(_) | Self::Rejected { .. })
    }
}

impl Default for LfStornierungState {
    fn default() -> Self {
        Self::New
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas LF-side Stornierung workflow.
///
/// All domain values must be pre-extracted and validated at the transport boundary.
/// `Workflow::handle()` is pure — no I/O.
#[derive(Clone)]
pub enum LfStornierungCommand {
    /// ERP instructs the engine to initiate a Stornierung request as LFN or LFA.
    ///
    /// The engine:
    /// 1. Records the `Initiated` event.
    /// 2. Enqueues a `PendingOutbox` entry with `message_type = "UTILMD"` and
    ///    the structured domain payload. The AS4 sender serialises this to
    ///    wire-format EDIFACT G and delivers it to the GNB via AS4.
    InitiateStornierung {
        /// Must be 44022 (Anfrage nach Stornierung).
        pid: Pruefidentifikator,
        /// Our own GLN (the Lieferant, from `--tenant-id`).
        sender: MarktpartnerCode,
        /// GNB GLN, resolved from the MaLo / process cache.
        receiver: MarktpartnerCode,
        /// Vorgangsnummer (IDE+24) of the original process being cancelled.
        vorgang_id: MaLo,
        /// BGM 1001 qualifier for the type of the original message.
        /// Use `"E01"` for Anmeldung, `"E02"` for Abmeldung, `"E35"` for Kündigung.
        bgm_qualifier: String,
    },
    /// Inbound GNB response (PID 44023 or 44024) received via AS4.
    ///
    /// Dispatched by the AS4 inbound layer after extracting the domain fields
    /// from the UTILMD G response message.
    HandleAntwort {
        /// PID of the inbound response (44023 or 44024).
        response_pid: Pruefidentifikator,
        /// `true` if PID 44023 (Bestätigung); `false` if PID 44024 (Ablehnung).
        accepted: bool,
        /// Optional rejection reason from UTILMD text segment (FTXT or STS).
        reason: Option<String>,
        /// EDIFACT message reference from the inbound response UNH.
        response_ref: MessageRef,
    },
    /// The 10-Werktage GNB response deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type (matches `GNB_RESPONSE_WINDOW_LABEL`).
        label: Box<str>,
    },
}

impl CommandPayload for LfStornierungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas LF-side outbound Stornierung workflow (PID 44022 outbound / 44023–44024 inbound).
///
/// Handles the Lieferant's perspective of initiating a supply-change cancellation:
/// sends UTILMD G 44022 to the GNB and tracks the 10-Werktage response.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GeliGasLfStornierungWorkflow>(
///     tenant_id,
///     WorkflowId::new("geli-gas-stornierung-lf", "FV2025-10-01"),
/// );
/// ```
pub struct GeliGasLfStornierungWorkflow;

impl Workflow for GeliGasLfStornierungWorkflow {
    type State = LfStornierungState;
    type Event = LfStornierungEvent;
    type Command = LfStornierungCommand;

    /// Deadline compensation for the GNB response window (10 Werktage, BK7-24-01-009).
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `GNB_RESPONSE_WINDOW_LABEL` | `Pending` | `TimeoutExpired` | BK7-24-01-009 — 10 Werktage Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        if deadline.label() == GNB_RESPONSE_WINDOW_LABEL
            && matches!(state, LfStornierungState::Pending(_))
        {
            Some(LfStornierungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            })
        } else {
            None
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            LfStornierungEvent::Initiated {
                pruefidentifikator,
                sender,
                receiver,
                vorgang_id,
                bgm_qualifier,
            } => LfStornierungState::Pending(LfStornierungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                receiver: receiver.clone(),
                vorgang_id: vorgang_id.clone(),
                bgm_qualifier: bgm_qualifier.clone(),
            }),
            LfStornierungEvent::AntwortReceived {
                accepted, reason, ..
            } => {
                if *accepted {
                    match state {
                        LfStornierungState::Pending(data) => LfStornierungState::Accepted(data),
                        other => other,
                    }
                } else {
                    LfStornierungState::Rejected {
                        reason: reason.clone().unwrap_or_else(|| "Ablehnung".to_owned()),
                    }
                }
            }
            LfStornierungEvent::DeadlineExpired { label, .. } => match state {
                LfStornierungState::Accepted(_) | LfStornierungState::Rejected { .. } => state,
                _ => LfStornierungState::Rejected {
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
            LfStornierungCommand::InitiateStornierung {
                pid,
                sender,
                receiver,
                vorgang_id,
                bgm_qualifier,
            } => {
                if !matches!(state, LfStornierungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != ANFRAGE_PID_LF {
                    return Err(WorkflowError::rejected(format!(
                        "expected LF Stornierung Anfrage PID {ANFRAGE_PID_LF} (44022), got {pid}",
                    )));
                }

                let event = LfStornierungEvent::Initiated {
                    pruefidentifikator: pid,
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    vorgang_id: vorgang_id.clone(),
                    bgm_qualifier: bgm_qualifier.clone(),
                };

                // Enqueue the outbound UTILMD G as a PendingOutbox entry.
                //
                // The AS4 sender in `makod` picks up entries with
                // `message_type = "UTILMD"` and serialises the payload
                // to wire-format EDIFACT G before handing it to AS4.
                // `document_date` and `message_ref` are derived at dispatch time.
                let outbox = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":     "outbound",
                        "pid":           pid.as_u32(),
                        "sender":        sender.as_str(),
                        "receiver":      receiver.as_str(),
                        "vorgang_id":    vorgang_id.as_str(),
                        "bgm_qualifier": bgm_qualifier,
                        "sparte":        "gas",
                    }),
                );

                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            LfStornierungCommand::HandleAntwort {
                response_pid,
                accepted,
                reason,
                response_ref,
            } => {
                if !matches!(state, LfStornierungState::Pending(_)) {
                    return Err(WorkflowError::invalid_state("Pending", state.label()));
                }
                if !ANTWORT_PIDS_LF.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GNB Stornierung Antwort PID (44023 or 44024), got {response_pid}",
                    )));
                }
                Ok(vec![LfStornierungEvent::AntwortReceived {
                    response_pid,
                    accepted,
                    reason,
                    response_ref,
                }]
                .into())
            }

            LfStornierungCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![LfStornierungEvent::DeadlineExpired { deadline_id, label }].into())
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

    fn lf_gln() -> MarktpartnerCode {
        MarktpartnerCode::new("9900000000001")
    }

    fn gnb_gln() -> MarktpartnerCode {
        MarktpartnerCode::new("9900000000002")
    }

    fn vorgang() -> MaLo {
        MaLo::new("DE0001000001000000000000000001234")
    }

    fn make_ref() -> MessageRef {
        MessageRef::new("MSG-0001")
    }

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }

    #[test]
    fn workflow_name_is_stable() {
        assert_eq!(WORKFLOW_NAME, "geli-gas-stornierung-lf");
    }

    #[test]
    fn anfrage_pid_is_44022() {
        assert_eq!(ANFRAGE_PID_LF, 44022);
    }

    #[test]
    fn antwort_pids_are_44023_and_44024() {
        assert!(ANTWORT_PIDS_LF.contains(&44023));
        assert!(ANTWORT_PIDS_LF.contains(&44024));
        assert_eq!(ANTWORT_PIDS_LF.len(), 2);
    }

    #[test]
    fn initiate_transitions_new_to_pending() {
        let state = LfStornierungState::New;
        let cmd = LfStornierungCommand::InitiateStornierung {
            pid: pid(44022),
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E01".to_owned(),
        };
        let output = GeliGasLfStornierungWorkflow::handle(&state, cmd).unwrap();
        assert!(
            !output.outbox.is_empty(),
            "must enqueue outbound UTILMD outbox entry"
        );
        let next = output
            .events
            .iter()
            .fold(state, GeliGasLfStornierungWorkflow::apply);
        assert!(matches!(next, LfStornierungState::Pending(_)));
    }

    #[test]
    fn handle_antwort_accepted_transitions_to_accepted() {
        let state = LfStornierungState::Pending(LfStornierungData {
            pruefidentifikator: pid(44022),
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E01".to_owned(),
        });
        let cmd = LfStornierungCommand::HandleAntwort {
            response_pid: pid(44023),
            accepted: true,
            reason: None,
            response_ref: make_ref(),
        };
        let output = GeliGasLfStornierungWorkflow::handle(&state, cmd).unwrap();
        let next = output
            .events
            .iter()
            .fold(state, GeliGasLfStornierungWorkflow::apply);
        assert!(matches!(next, LfStornierungState::Accepted(_)));
    }

    #[test]
    fn handle_antwort_rejected_transitions_to_rejected() {
        let state = LfStornierungState::Pending(LfStornierungData {
            pruefidentifikator: pid(44022),
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E02".to_owned(),
        });
        let cmd = LfStornierungCommand::HandleAntwort {
            response_pid: pid(44024),
            accepted: false,
            reason: Some("Stornierung nicht möglich".to_owned()),
            response_ref: make_ref(),
        };
        let output = GeliGasLfStornierungWorkflow::handle(&state, cmd).unwrap();
        let next = output
            .events
            .iter()
            .fold(state, GeliGasLfStornierungWorkflow::apply);
        assert!(matches!(next, LfStornierungState::Rejected { .. }));
    }

    #[test]
    fn wrong_pid_in_initiate_returns_error() {
        let state = LfStornierungState::New;
        let cmd = LfStornierungCommand::InitiateStornierung {
            pid: pid(44001), // wrong PID
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E01".to_owned(),
        };
        assert!(GeliGasLfStornierungWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn wrong_antwort_pid_returns_error() {
        let state = LfStornierungState::Pending(LfStornierungData {
            pruefidentifikator: pid(44022),
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E01".to_owned(),
        });
        let cmd = LfStornierungCommand::HandleAntwort {
            response_pid: pid(44001), // wrong PID
            accepted: true,
            reason: None,
            response_ref: make_ref(),
        };
        assert!(GeliGasLfStornierungWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn timeout_in_pending_transitions_to_rejected() {
        use mako_engine::ids::DeadlineId;
        let state = LfStornierungState::Pending(LfStornierungData {
            pruefidentifikator: pid(44022),
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E35".to_owned(),
        });
        let cmd = LfStornierungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: GNB_RESPONSE_WINDOW_LABEL.into(),
        };
        let output = GeliGasLfStornierungWorkflow::handle(&state, cmd).unwrap();
        let next = output
            .events
            .iter()
            .fold(state, GeliGasLfStornierungWorkflow::apply);
        assert!(matches!(next, LfStornierungState::Rejected { .. }));
    }

    #[test]
    fn timeout_in_terminal_state_is_noop() {
        use mako_engine::ids::DeadlineId;
        let state = LfStornierungState::Accepted(LfStornierungData {
            pruefidentifikator: pid(44022),
            sender: lf_gln(),
            receiver: gnb_gln(),
            vorgang_id: vorgang(),
            bgm_qualifier: "E01".to_owned(),
        });
        let cmd = LfStornierungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: GNB_RESPONSE_WINDOW_LABEL.into(),
        };
        let output = GeliGasLfStornierungWorkflow::handle(&state, cmd).unwrap();
        assert!(output.events.is_empty());
    }
}
