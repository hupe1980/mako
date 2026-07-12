//! GPKE COMDIS 29001/29002 — Kaufmännisch-Bilanzielle Ausgleichsprozesse (KBA).
//!
//! COMDIS is the BDEW-regulated formal commercial dispute channel.
//! Without COMDIS, disputes remain in informal REMADV-only territory;
//! BNetzA may interpret NB silence on REMADV as implicit acceptance.
//!
//! ## Process flow (NB role — process owner)
//!
//! ```text
//! LF:  REMADV 33002 (Zahlungsabzug / informal payment reduction)
//! NB:  Receives REMADV 33002 via gpke-abrechnung workflow
//! NB:  Decides to formally reject → dispatches COMDIS 29001 (NB → LF, Ablehnung REMADV)
//! LF:  Receives COMDIS 29001 → formal KBA process begins
//! NB:  Receives LF counter (COMDIS 29002 NB → LF, Ablehnung IFTSTA, or negotiation)
//! Both: Resolve (settled / withdrawn / escalated to BNetzA)
//! ```
//!
//! ## Prüfidentifikatoren
//!
//! | PID   | Sender | Direction  | Description                                  |
//! |-------|--------|------------|----------------------------------------------|
//! | 29001 | NB     | NB → LF    | Ablehnung REMADV (formal rejection of REMADV dispute) |
//! | 29002 | NB     | NB → LF    | Ablehnung IFTSTA (formal rejection of IFTSTA challenge) |
//!
//! Both PIDs live in `mako-gpke` (GPKE Teil 2/3, BK6-22-024).
//!
//! ## APERAK Frist
//!
//! APERAK AHB 1.0 §2.4.1 Strom "all other" rule: **nächster Werktag 12 Uhr**.
//!
//! ## Governing ruling
//!
//! **BK6-22-024** (GPKE, Beschluss 28.10.2022).

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// COMDIS Prüfidentifikatoren in `mako-gpke`.
///
/// - 29001: LF → NB — Einleitung KBA
/// - 29002: NB → LF — Antwort auf KBA
pub const COMDIS_PIDS: &[u32] = &[29001, 29002];

/// Stable workflow name used in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "gpke-comdis";

/// APERAK deadline label — nächster Werktag 12 Uhr per APERAK AHB 1.0 §2.4.1.
pub const COMDIS_APERAK_WINDOW_LABEL: &str = "gpke-comdis-aperak-next-workday";

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GPKE COMDIS workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GpkeComdisEvent {
    /// PID 29001 (LF formal KBA Einleitung) received and accepted.
    Comdis29001Received {
        /// EDIFACT message reference from UNH.
        message_ref: MessageRef,
        /// Rechnungsnummer of the disputed invoice.
        rechnungsnummer: String,
        /// LF Marktpartnercode (initiating party).
        lf_code: MarktpartnerCode,
        /// NB Marktpartnercode (responding party).
        nb_code: MarktpartnerCode,
        /// Disputed amount in 1/100 EUR (integer avoids float rounding).
        disputed_amount_ct: i64,
        /// UTC timestamp of receipt.
        received_at: time::OffsetDateTime,
    },
    /// AHB profile validation passed on inbound 29001.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// AHB profile validation failed — message rejected.
    ValidationFailed {
        /// Human-readable validation error strings.
        errors: Vec<String>,
    },
    /// NB dispatched COMDIS 29002 (counter-response).
    Comdis29002Dispatched {
        /// NB COMDIS 29002 message reference.
        counter_ref: MessageRef,
        /// `true` = NB accepts LF position, `false` = NB rejects.
        accepted: bool,
        /// Rejection reason code (EDIFACT ERC), if applicable.
        reason_code: Option<String>,
    },
    /// Dispute resolved (settled / withdrawn / escalated to BNetzA).
    ComdisResolved {
        /// Final outcome of the KBA process.
        outcome: ComdisOutcome,
    },
    /// APERAK deadline expired — operator escalation required.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl EventPayload for GpkeComdisEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Comdis29001Received { .. } => "GpkeComdis29001Received",
            Self::ValidationPassed { .. } => "GpkeComdisValidationPassed",
            Self::ValidationFailed { .. } => "GpkeComdisValidationFailed",
            Self::Comdis29002Dispatched { .. } => "GpkeComdis29002Dispatched",
            Self::ComdisResolved { .. } => "GpkeComdisResolved",
            Self::DeadlineExpired { .. } => "GpkeComdisDeadlineExpired",
        }
    }
}

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Possible final outcomes of a KBA (COMDIS) dispute process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComdisOutcome {
    /// NB accepts LF position — credit note or correction issued.
    Settled,
    /// LF withdraws dispute — original invoice stands.
    Withdrawn,
    /// Escalated to BNetzA formal arbitration.
    EscalatedBnetza,
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data recorded at `Comdis29001Received` time and carried throughout the lifecycle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpkeComdisData {
    /// EDIFACT message reference from UNH of the inbound 29001.
    pub message_ref: MessageRef,
    /// Rechnungsnummer of the disputed invoice.
    pub rechnungsnummer: String,
    /// LF Marktpartnercode (initiating party).
    pub lf_code: MarktpartnerCode,
    /// NB Marktpartnercode (responding party).
    pub nb_code: MarktpartnerCode,
    /// Disputed amount in 1/100 EUR (integer arithmetic avoids float rounding).
    pub disputed_amount_ct: i64,
    /// UTC timestamp of COMDIS 29001 receipt.
    pub received_at: time::OffsetDateTime,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Lifecycle state of a GPKE COMDIS process stream.
///
/// ```text
/// New → Initiated → ValidationPassed → Answered → Resolved
///                 ↘ ValidationFailed → Rejected
///                                    ↘ DeadlineExpired → Rejected
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GpkeComdisState {
    /// No events yet.
    #[default]
    New,
    /// 29001 received; awaiting AHB validation outcome.
    Initiated(GpkeComdisData),
    /// AHB validation passed; NB must respond (nächster Werktag 12 Uhr).
    ValidationPassed(GpkeComdisData),
    /// NB dispatched COMDIS 29002.
    Answered(GpkeComdisData),
    /// Process resolved.
    Resolved(GpkeComdisData),
    /// Process rejected due to validation failure or deadline expiry.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands handled by the GPKE COMDIS workflow.
#[derive(Debug, Clone)]
pub enum GpkeComdisCommand {
    /// Inbound COMDIS 29001 from the LF at the AS4 boundary.
    Receive29001 {
        /// EDIFACT message reference from UNH.
        message_ref: MessageRef,
        /// Rechnungsnummer of the disputed invoice.
        rechnungsnummer: String,
        /// LF Marktpartnercode (initiating party).
        lf_code: MarktpartnerCode,
        /// NB Marktpartnercode (responding party).
        nb_code: MarktpartnerCode,
        /// Disputed amount in 1/100 EUR.
        disputed_amount_ct: i64,
        /// UTC timestamp of receipt.
        received_at: time::OffsetDateTime,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Human-readable validation error strings (empty when `validation_passed = true`).
        validation_errors: Vec<String>,
    },
    /// NB operator dispatches COMDIS 29002 counter-response.
    Dispatch29002 {
        /// NB COMDIS 29002 message reference.
        counter_ref: MessageRef,
        /// `true` if NB accepts LF position.
        accepted: bool,
        /// Rejection reason code (EDIFACT ERC), if applicable.
        reason_code: Option<String>,
    },
    /// Mark the dispute as resolved (operator action after 29002 exchange).
    Resolve {
        /// Final outcome of the KBA process.
        outcome: ComdisOutcome,
    },
    /// APERAK deadline expired (engine deadline callback).
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for GpkeComdisCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE COMDIS workflow — KBA formal dispute lifecycle for PIDs 29001/29002.
pub struct GpkeComdisWorkflow;

impl Workflow for GpkeComdisWorkflow {
    type State = GpkeComdisState;
    type Event = GpkeComdisEvent;
    type Command = GpkeComdisCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                COMDIS_APERAK_WINDOW_LABEL,
                GpkeComdisState::Initiated(_) | GpkeComdisState::ValidationPassed(_),
            ) => Some(GpkeComdisCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GpkeComdisEvent::Comdis29001Received {
                message_ref,
                rechnungsnummer,
                lf_code,
                nb_code,
                disputed_amount_ct,
                received_at,
            } => GpkeComdisState::Initiated(GpkeComdisData {
                message_ref: message_ref.clone(),
                rechnungsnummer: rechnungsnummer.clone(),
                lf_code: lf_code.clone(),
                nb_code: nb_code.clone(),
                disputed_amount_ct: *disputed_amount_ct,
                received_at: *received_at,
            }),

            GpkeComdisEvent::ValidationPassed { .. } => match state {
                GpkeComdisState::Initiated(data) => GpkeComdisState::ValidationPassed(data),
                other => other,
            },

            GpkeComdisEvent::ValidationFailed { errors } => GpkeComdisState::Rejected {
                reason: errors.join("; "),
            },

            GpkeComdisEvent::Comdis29002Dispatched { .. } => match state {
                GpkeComdisState::ValidationPassed(data) => GpkeComdisState::Answered(data),
                other => other,
            },

            GpkeComdisEvent::ComdisResolved { .. } => match state {
                GpkeComdisState::Answered(data) => GpkeComdisState::Resolved(data),
                other => other,
            },

            GpkeComdisEvent::DeadlineExpired { label, .. } => GpkeComdisState::Rejected {
                reason: format!("APERAK deadline expired: {label}"),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GpkeComdisCommand::Receive29001 {
                message_ref,
                rechnungsnummer,
                lf_code,
                nb_code,
                disputed_amount_ct,
                received_at,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GpkeComdisState::New) {
                    return Err(WorkflowError::CommandRejected {
                        reason: "Receive29001 requires New state".into(),
                    });
                }
                let received_event = GpkeComdisEvent::Comdis29001Received {
                    message_ref: message_ref.clone(),
                    rechnungsnummer,
                    lf_code,
                    nb_code,
                    disputed_amount_ct,
                    received_at,
                };
                let validation_event = if validation_passed {
                    GpkeComdisEvent::ValidationPassed { message_ref }
                } else {
                    GpkeComdisEvent::ValidationFailed {
                        errors: validation_errors,
                    }
                };
                Ok(WorkflowOutput::from(vec![received_event, validation_event]))
            }

            GpkeComdisCommand::Dispatch29002 {
                counter_ref,
                accepted,
                reason_code,
            } => {
                if !matches!(state, GpkeComdisState::ValidationPassed(_)) {
                    return Err(WorkflowError::CommandRejected {
                        reason: "Dispatch29002 requires ValidationPassed state".into(),
                    });
                }
                Ok(WorkflowOutput::from(vec![
                    GpkeComdisEvent::Comdis29002Dispatched {
                        counter_ref,
                        accepted,
                        reason_code,
                    },
                ]))
            }

            GpkeComdisCommand::Resolve { outcome } => {
                if !matches!(state, GpkeComdisState::Answered(_)) {
                    return Err(WorkflowError::CommandRejected {
                        reason: "Resolve requires Answered state".into(),
                    });
                }
                Ok(WorkflowOutput::from(vec![
                    GpkeComdisEvent::ComdisResolved { outcome },
                ]))
            }

            GpkeComdisCommand::TimeoutExpired { deadline_id, label } => {
                Ok(WorkflowOutput::from(vec![
                    GpkeComdisEvent::DeadlineExpired { deadline_id, label },
                ]))
            }
        }
    }
}

// ── DB schema ─────────────────────────────────────────────────────────────────

/// DDL for the `comdis_records` business table.
///
/// Deploy in `invoicd` (LF role) and `netzbilanzd` (NB role).
/// The engine workflow tracks COMDIS state; this table mirrors the outcome for
/// operator reporting and BNetzA compliance documentation.
pub const COMDIS_RECORDS_DDL: &str = r"
CREATE TABLE IF NOT EXISTS comdis_records (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    rechnungsnummer     TEXT        NOT NULL,
    comdis_ref          TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    nb_mp_id            TEXT        NOT NULL,
    disputed_amount_ct  BIGINT      NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'open'
                        CHECK (status IN ('open','answered','resolved','escalated')),
    counter_ref         TEXT,
    outcome             TEXT
                        CHECK (outcome IN ('settled','withdrawn','escalated_bnetza') OR outcome IS NULL),
    received_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    answered_at         TIMESTAMPTZ,
    resolved_at         TIMESTAMPTZ,
    UNIQUE (tenant, rechnungsnummer, comdis_ref)
);
CREATE INDEX IF NOT EXISTS comdis_records_status   ON comdis_records (tenant, status);
CREATE INDEX IF NOT EXISTS comdis_records_lf_mp_id ON comdis_records (tenant, lf_mp_id);
";
