//! GeLi Gas Lieferantenwechsel — full gas supplier-change workflow.
//!
//! Covers all eight GeLi Gas process variants (PIDs 44001–44021):
//!
//! | Process | Anfrage PID | Antwort OK | Antwort NG | Initiator |
//! |---|---|---|---|---|
//! | Lieferbeginn Gas | 44001 | 44003 | 44004 | LFN → GNB |
//! | Lieferende Gas | 44002 | 44005 | 44006 | LFN → GNB |
//! | Abmeldung NN | 44007 | 44008 | 44009 | GNB → LFN |
//! | Abmeldungsanfrage | 44010 | 44011 | 44012 | GNB → LFA |
//! | EoG Anmeldung | 44013 | 44014 | 44015 | GNB → LF |
//! | Kündigung beim alten LF | 44016 | 44017 | 44018 | LFN → LFA |
//! | Bestandsliste | 44019 | — | — | GNB → LF |
//! | Änderungsmeldung Bestandsliste | 44020 | 44021 | — | LF → GNB |
//!
//! # Roles
//!
//! The same workflow code runs in GNB and LFN/LFA deployments. The role is
//! determined at the process-creation point:
//!
//! - **Responder role** (GNB/LFA receives the Anfrage): `ReceiveUtilmd` → `SendAntwort`.
//! - **Initiator role** (GNB sends the Anfrage, awaits response): `InitiateGnbProcess` → `ReceiveGnbAntwort`.
//!
//! # APERAK Frist
//!
//! **10 Werktage** (BNetzA BK7-24-01-009). Saturday counts as a Werktag;
//! Sunday and public holidays do not.
//!
//! # Regulatory basis
//!
//! - **BDEW GeLi Gas 3.0** — BK7-24-01-009 (Beschluss 12.09.2025)
//! - **UTILMD G AHB 1.1 / 1.2** — EDI@Energy UTILMD Gas profiles
//! - **APERAK AHB** — 10-Werktage Frist for all GeLi Gas processes

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    envelope::EventEnvelope,
    error::WorkflowError,
    fristen::{
        APERAK_GAS_FOLGEPROZESS_LABEL, APERAK_GAS_INITIALPROZESS_LABEL, HolidayCalendar,
        aperak_gas_folgeprozess_due_at, aperak_gas_initialprozess_due_at, deadline_at_werktage,
    },
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use time::OffsetDateTime;

// ── Stable names ──────────────────────────────────────────────────────────────

/// Stable workflow name used as the `WorkflowId.name` and in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "geli-gas-supplier-change";

/// Deadline label for the 10-Werktage response window (GeLi Gas 3.0 (BK7-24-01-009)).
pub const RESPONSE_WINDOW_LABEL: &str = "geli-gas-response-10-werktage";

// ── PID sets ──────────────────────────────────────────────────────────────────

/// All inbound **Anfrage** PIDs that start a new GeLi Gas process stream.
pub const ANFRAGE_PIDS: &[u32] = &[44001, 44002, 44007, 44010, 44013, 44016, 44019, 44020];

/// All **Antwort** PIDs that update an existing GeLi Gas process stream.
pub const ANTWORT_PIDS: &[u32] = &[
    44003, 44004, // Bestätigung/Ablehnung Lieferbeginn
    44005, 44006, // Bestätigung/Ablehnung Lieferende
    44008, 44009, // Bestätigung/Ablehnung Abmeldung NN
    44011, 44012, // Bestätigung/Ablehnung Abmeldungsanfrage
    44014, 44015, // Bestätigung/Ablehnung EoG Anmeldung
    44017, 44018, // Bestätigung/Ablehnung Kündigung
    44021, // Antwort auf Änderungsmeldung
];

/// All UTILMD G PIDs handled by this workflow (union of ANFRAGE and ANTWORT).
pub const UTILMD_PIDS: &[u32] = &[
    44001, 44002, 44003, 44004, 44005, 44006, 44007, 44008, 44009, 44010, 44011, 44012, 44013,
    44014, 44015, 44016, 44017, 44018, 44019, 44020, 44021,
];

// ── Process variant ───────────────────────────────────────────────────────────

/// Classification of the GeLi Gas process type, derived from the Anfrage PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GasProcessVariant {
    /// PID 44001 — Lieferbeginn Gas, LFN → GNB. Response: 44003/44004.
    LieferbeginnGas,
    /// PID 44002 — Lieferende Gas, LFN → GNB. Response: 44005/44006.
    LieferendeGas,
    /// PID 44007 — Abmeldung NN vom GNB, GNB → LFN. Response: 44008/44009.
    AbmeldungNn,
    /// PID 44010 — Abmeldungsanfrage des GNB, GNB → LFA. Response: 44011/44012.
    Abmeldungsanfrage,
    /// PID 44013 — EoG Anmeldung, GNB → LF. Response: 44014/44015.
    EogAnmeldung,
    /// PID 44016 — Kündigung beim alten Lieferanten, LFN → LFA. Response: 44017/44018.
    KuendigungLfa,
    /// PID 44019 — Bestandsliste zugeordnete Marktlokationen, GNB → LF. No response.
    Bestandsliste,
    /// PID 44020 — Änderungsmeldung zur Bestandsliste, LF → GNB. Response: 44021.
    Aenderungsmeldung,
}

impl GasProcessVariant {
    /// Derive the process variant from the inbound Anfrage PID.
    #[must_use]
    pub fn from_anfrage_pid(pid: u32) -> Option<Self> {
        Some(match pid {
            44001 => Self::LieferbeginnGas,
            44002 => Self::LieferendeGas,
            44007 => Self::AbmeldungNn,
            44010 => Self::Abmeldungsanfrage,
            44013 => Self::EogAnmeldung,
            44016 => Self::KuendigungLfa,
            44019 => Self::Bestandsliste,
            44020 => Self::Aenderungsmeldung,
            _ => return None,
        })
    }

    /// `true` if no Antwort is expected.
    #[must_use]
    pub fn is_one_way(&self) -> bool {
        matches!(self, Self::Bestandsliste)
    }

    /// `true` if the GNB is the initiator (sends the Anfrage via outbox).
    #[must_use]
    pub fn gnb_is_initiator(&self) -> bool {
        matches!(
            self,
            Self::AbmeldungNn | Self::Abmeldungsanfrage | Self::EogAnmeldung | Self::Bestandsliste
        )
    }
}

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound UTILMD G **Antwort** PID from the Anfrage PID.
///
/// | Anfrage | accepted=true | accepted=false |
/// |---------|---------------|----------------|
/// | 44001   | 44003         | 44004          |
/// | 44002   | 44005         | 44006          |
/// | 44007   | 44008         | 44009          |
/// | 44010   | 44011         | 44012          |
/// | 44013   | 44014         | 44015          |
/// | 44016   | 44017         | 44018          |
/// | 44019   | (none)        | (none)         |
/// | 44020   | 44021         | (none)         |
#[must_use]
pub fn response_pid_for(anfrage_pid: u32, accepted: bool) -> Option<Pruefidentifikator> {
    let code: u32 = match anfrage_pid {
        44001 => {
            if accepted {
                44003
            } else {
                44004
            }
        }
        44002 => {
            if accepted {
                44005
            } else {
                44006
            }
        }
        44007 => {
            if accepted {
                44008
            } else {
                44009
            }
        }
        44010 => {
            if accepted {
                44011
            } else {
                44012
            }
        }
        44013 => {
            if accepted {
                44014
            } else {
                44015
            }
        }
        44016 => {
            if accepted {
                44017
            } else {
                44018
            }
        }
        44019 => return None,
        44020 => {
            if accepted {
                44021
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data recorded at process initiation and carried through every state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasSupplierChangeData {
    /// Process variant (derived from the Anfrage PID).
    pub variant: GasProcessVariant,
    /// Marktlokation EIC code.
    pub malo_id: MaLo,
    /// GLN of the message sender.
    pub sender: MarktpartnerCode,
    /// GLN of the message receiver.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (DTM+137, `YYYYMMDD`).
    pub document_date: String,
    /// Process-specific date (e.g. Lieferbeginn-Datum, `YYYYMMDD`).
    #[serde(default)]
    pub process_date: String,
    /// BDEW Prüfidentifikator of the initial Anfrage message.
    pub pruefidentifikator: Pruefidentifikator,
    /// EDIFACT message reference of the initial Anfrage.
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas supplier-change workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasSupplierChangeEvent {
    /// Process initiated by a valid inbound UTILMD G Anfrage (responder role).
    Initiated {
        /// Classified process variant.
        variant: GasProcessVariant,
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Process-specific date (`YYYYMMDD`).
        #[serde(default)]
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator of the Anfrage message.
        pruefidentifikator: Pruefidentifikator,
    },
    /// AHB profile validation passed.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Local party sent the outbound UTILMD G Antwort.
    AntwortGesendet {
        /// Derived Antwort PID (None for one-way processes).
        response_pid: Option<Pruefidentifikator>,
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Rejection reason (only set when `accepted = false`).
        reason: Option<String>,
    },
    /// GNB initiated a process by sending the Anfrage via outbox.
    GnbProcessInitiated {
        /// Classified process variant.
        variant: GasProcessVariant,
        /// GLN of the GNB (sender).
        sender: MarktpartnerCode,
        /// GLN of the counterparty (LFN, LFA, or LF).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date.
        document_date: String,
        /// Process-specific date.
        #[serde(default)]
        process_date: String,
        /// EDIFACT message reference of the outbound Anfrage.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator of the outbound Anfrage.
        pruefidentifikator: Pruefidentifikator,
    },
    /// GNB received the Antwort to its initiated Anfrage.
    GnbAntwortErhalten {
        /// Prüfidentifikator of the inbound Antwort message.
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Rejection reason (only when `accepted = false`).
        reason: Option<String>,
    },
    /// Gas supply became active (Lieferbeginn Gas 44001 only).
    Activated,
    /// Process rejected.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Deadline expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GasSupplierChangeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "GasSupplierChangeInitiated",
            Self::ValidationPassed { .. } => "GasSupplierChangeValidationPassed",
            Self::AntwortGesendet { .. } => "GasSupplierChangeAntwortGesendet",
            Self::GnbProcessInitiated { .. } => "GasSupplierChangeGnbProcessInitiated",
            Self::GnbAntwortErhalten { .. } => "GasSupplierChangeGnbAntwortErhalten",
            Self::Activated => "GasSupplierChangeActivated",
            Self::Rejected { .. } => "GasSupplierChangeRejected",
            Self::DeadlineExpired { .. } => "GasSupplierChangeDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GeLi Gas supplier-change process stream.
///
/// # Lifecycle (responder role — GNB/LFA receives the Anfrage)
///
/// ```text
/// New → Initiated → ValidationPassed → AntwortGesendet → Active (44001)
///                                                       → Completed (others)
///                                    ↘ Rejected
///     ↘ Rejected (validation failure)
/// ```
///
/// # Lifecycle (initiator role — GNB sends the Anfrage)
///
/// ```text
/// New → GnbPending → Completed | Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GasSupplierChangeState {
    /// No events yet; stream exists but process has not started.
    #[default]
    New,
    /// Inbound Anfrage received; AHB validation in progress.
    Initiated(GasSupplierChangeData),
    /// AHB validation passed; outbound Antwort not yet dispatched.
    ValidationPassed(GasSupplierChangeData),
    /// Local party sent the outbound UTILMD G Antwort.
    AntwortGesendet {
        /// Process data.
        data: GasSupplierChangeData,
        /// Derived Antwort PID (None for 44019 Bestandsliste).
        response_pid: Option<Pruefidentifikator>,
    },
    /// GNB sent the Anfrage; awaiting counterpart response within 10 Werktage.
    GnbPending(GasSupplierChangeData),
    /// Gas supply relationship is active (Lieferbeginn Gas 44001 only).
    Active(GasSupplierChangeData),
    /// Process completed successfully (all variants except Lieferbeginn Active).
    Completed(GasSupplierChangeData),
    /// Process rejected — validation failure, negative Antwort, or timeout.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl GasSupplierChangeState {
    /// Stable status label.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::GnbPending(_) => "GnbPending",
            Self::Active(_) => "Active",
            Self::Completed(_) => "Completed",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return process data if the process has been initiated; `None` otherwise.
    #[must_use]
    pub fn data(&self) -> Option<&GasSupplierChangeData> {
        match self {
            Self::Initiated(d)
            | Self::ValidationPassed(d)
            | Self::GnbPending(d)
            | Self::Active(d)
            | Self::Completed(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }

    /// `true` if the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Active(_) | Self::Completed(_) | Self::Rejected { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas supplier-change workflow.
#[derive(Clone)]
pub enum GasSupplierChangeCommand {
    /// Inbound UTILMD G Anfrage received (responder role — GNB or LFA).
    ///
    /// Valid Anfrage PIDs: 44001, 44002, 44007, 44010, 44013, 44016, 44019, 44020.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator (must be in `ANFRAGE_PIDS`).
        pid: Pruefidentifikator,
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD` from DTM+137).
        document_date: String,
        /// Process-specific date (`YYYYMMDD` from DTM+92 or DTM+163).
        process_date: String,
        /// EDIFACT message reference (UNH 0062).
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed with no errors.
        validation_passed: bool,
        /// Human-readable validation error strings (empty when `validation_passed = true`).
        validation_errors: Vec<String>,
        /// UTC wall-clock time when the inbound UTILMD G was received.
        ///
        /// Used to compute the APERAK Gas *sending* deadline (APERAK AHB 1.0 §2.3.1)
        /// and the 10-Werktage process deadline (BK7-24-01-009) that are
        /// registered atomically with the `Initiated` event.
        received_at: OffsetDateTime,
        /// Bilanzierungsmethode from UTILMD G `TM+EM` segment (L1/N1).
        /// `"SLP"` | `"RLM"` | `"IMS"` — propagated to `ProcessInitiated` outbox
        /// so `marktd` can update `malo.bilanzierungsmethode`.
        bilanzierungsmethode: Option<String>,
        /// Gas GaBi RLM Fallgruppe from UTILMD G `TM+Z10` segment (L1/N1).
        /// Only set for Gas RLM MaLos. Propagated to `ProcessInitiated` outbox
        /// so `marktd` can update `malo.fallgruppe`.
        fallgruppe: Option<String>,
        /// Gas quality type from UTILMD G `STS` segment (L1/N1).
        ///
        /// Normalized to canonical BO4E / BNetzA MaStR form before storage:
        /// `"H_GAS"` | `"L_GAS"` | `"H2_BLEND"` | `"BIOGAS"` | `"FLUESSIGGAS"`.
        ///
        /// Propagated to `ProcessInitiated` outbox so `marktd` can update
        /// `malo.gasqualitaet` atomically with the supplier-change event.
        ///
        /// `None` when the UTILMD G message does not carry a gas quality qualifier
        /// (e.g. before the DVGW/BNetzA H2-blend AHBs are published).
        /// In that case `marktd` preserves the existing `gasqualitaet` value.
        gasqualitaet: Option<String>,
    },
    /// Send the outbound UTILMD G Antwort (responder role).
    ///
    /// Derives the Antwort PID via `response_pid_for()`. The Antwort UTILMD G
    /// is placed in the outbox atomically with `AntwortGesendet`.
    ///
    /// **APERAK Frist:** 10 Werktage (BK7-24-01-009).
    SendAntwort {
        /// `true` = Bestätigung (accept), `false` = Ablehnung (reject).
        accepted: bool,
        /// Rejection reason (required when `accepted = false`).
        reason: Option<String>,
        /// Post-acceptance downstream obligations (co-persisted atomically).
        obligations: Vec<PendingOutbox>,
    },
    /// GNB initiates a GNB-side process (initiator role).
    ///
    /// Valid for: 44007 (Abmeldung NN), 44010 (Abmeldungsanfrage),
    /// 44013 (EoG Anmeldung), 44019 (Bestandsliste).
    InitiateGnbProcess {
        /// Anfrage PID (must be 44007, 44010, 44013, or 44019).
        pid: Pruefidentifikator,
        /// GLN of the GNB (sender).
        sender: MarktpartnerCode,
        /// GLN of the counterparty (LFN, LFA, or LF).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date.
        document_date: String,
        /// Process-specific date.
        process_date: String,
        /// EDIFACT message reference of the outbound Anfrage.
        message_ref: MessageRef,
    },
    /// GNB received the Antwort to its initiated Anfrage.
    ReceiveGnbAntwort {
        /// Prüfidentifikator of the inbound Antwort message.
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Rejection reason (only when `accepted = false`).
        reason: Option<String>,
    },
    /// Mark supply as active (Lieferbeginn Gas 44001 only).
    Activate,
    /// A registered 10-Werktage deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GasSupplierChangeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas supplier-change workflow (PIDs 44001–44021).
pub struct GeliGasSupplierChangeWorkflow;

const GNB_INITIATOR_PIDS: &[u32] = &[44007, 44010, 44013, 44019];

impl Workflow for GeliGasSupplierChangeWorkflow {
    type State = GasSupplierChangeState;
    type Event = GasSupplierChangeEvent;
    type Command = GasSupplierChangeCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                RESPONSE_WINDOW_LABEL,
                GasSupplierChangeState::Initiated(_)
                | GasSupplierChangeState::ValidationPassed(_)
                | GasSupplierChangeState::AntwortGesendet { .. }
                | GasSupplierChangeState::GnbPending(_),
            ) => Some(GasSupplierChangeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasSupplierChangeEvent::Initiated {
                variant,
                sender,
                receiver,
                malo_id,
                document_date,
                process_date,
                message_ref,
                pruefidentifikator,
            } => GasSupplierChangeState::Initiated(GasSupplierChangeData {
                variant: *variant,
                malo_id: malo_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
            }),

            GasSupplierChangeEvent::ValidationPassed { .. } => {
                if let GasSupplierChangeState::Initiated(data) = state {
                    if data.variant.is_one_way() {
                        GasSupplierChangeState::Completed(data)
                    } else {
                        GasSupplierChangeState::ValidationPassed(data)
                    }
                } else {
                    state
                }
            }

            GasSupplierChangeEvent::AntwortGesendet {
                response_pid,
                accepted,
                reason,
            } => match state {
                GasSupplierChangeState::ValidationPassed(data) => {
                    if *accepted {
                        GasSupplierChangeState::AntwortGesendet {
                            data,
                            response_pid: *response_pid,
                        }
                    } else {
                        GasSupplierChangeState::Rejected {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "negative Antwort".to_owned()),
                        }
                    }
                }
                _ => state,
            },

            GasSupplierChangeEvent::GnbProcessInitiated {
                variant,
                sender,
                receiver,
                malo_id,
                document_date,
                process_date,
                message_ref,
                pruefidentifikator,
            } => {
                let data = GasSupplierChangeData {
                    variant: *variant,
                    malo_id: malo_id.clone(),
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    document_date: document_date.clone(),
                    process_date: process_date.clone(),
                    pruefidentifikator: *pruefidentifikator,
                    message_ref: Some(message_ref.clone()),
                };
                if variant.is_one_way() {
                    GasSupplierChangeState::Completed(data)
                } else {
                    GasSupplierChangeState::GnbPending(data)
                }
            }

            GasSupplierChangeEvent::GnbAntwortErhalten {
                accepted, reason, ..
            } => match state {
                GasSupplierChangeState::GnbPending(data) => {
                    if *accepted {
                        GasSupplierChangeState::Completed(data)
                    } else {
                        GasSupplierChangeState::Rejected {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "negative Antwort".to_owned()),
                        }
                    }
                }
                _ => state,
            },

            GasSupplierChangeEvent::Activated => {
                if let GasSupplierChangeState::AntwortGesendet { data, .. } = state {
                    GasSupplierChangeState::Active(data)
                } else {
                    state
                }
            }

            GasSupplierChangeEvent::Rejected { reason } => GasSupplierChangeState::Rejected {
                reason: reason.clone(),
            },

            GasSupplierChangeEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    GasSupplierChangeState::Rejected {
                        reason: format!("deadline expired: {label}"),
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GasSupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                process_date,
                message_ref,
                validation_passed,
                validation_errors,
                received_at,
                bilanzierungsmethode,
                fallgruppe,
                gasqualitaet,
            } => {
                if !matches!(state, GasSupplierChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                let variant =
                    GasProcessVariant::from_anfrage_pid(pid.as_u32()).ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "PID {pid} is not a valid GeLi Gas Anfrage PID \
                             (expected one of: {ANFRAGE_PIDS:?})"
                        ))
                    })?;

                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();
                // 44001 = Lieferbeginn Anfrage (Initialprozess); all others are Folgeprozesse.
                let is_initialprozess = pid.as_u32() == 44_001;
                // Clone before move into Initiated event for ProcessInitiated outbox (L1/N1).
                let malo_id_str = malo_id.as_str().to_owned();
                let process_date_clone = process_date.clone();

                let mut events = vec![GasSupplierChangeEvent::Initiated {
                    variant,
                    sender,
                    receiver,
                    malo_id,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(GasSupplierChangeEvent::ValidationPassed { message_ref });
                    // Register two deadlines atomically with the events:
                    //   1. APERAK Gas *sending* deadline (APERAK AHB 1.0 \u00a72.3.1):
                    //      Initialprozess (44001): 3 Werktage; Folgeprozess: n\u00e4chster Werktag 12:00.
                    //   2. GeLi Gas 10-Werktage *process response* deadline (BK7-24-01-009):
                    //      The responder must issue the Antwort within 10 WT.
                    let aperak_send_dl = if is_initialprozess {
                        PendingDeadline::new(
                            APERAK_GAS_INITIALPROZESS_LABEL,
                            aperak_gas_initialprozess_due_at(received_at),
                        )
                    } else {
                        PendingDeadline::new(
                            APERAK_GAS_FOLGEPROZESS_LABEL,
                            aperak_gas_folgeprozess_due_at(received_at),
                        )
                    };
                    let process_dl = PendingDeadline::new(
                        RESPONSE_WINDOW_LABEL,
                        deadline_at_werktage(received_at, 10, HolidayCalendar::BdewMaKo),
                    );
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        // Notify the GNB ERP via de.mako.process.initiated so it can
                        // decide to Bestätigen or Ablehnen the Anmeldung.
                        // Also carries bilanzierungsmethode + fallgruppe so marktd
                        // can update malo.bilanzierungsmethode / malo.fallgruppe (L1/N1).
                        vec![
                            PendingOutbox::new(
                                "ProcessInitiated",
                                receiver_gln.as_str(),
                                serde_json::json!({
                                    "pid":                  pid.as_u32(),
                                    "malo_id":              malo_id_str,
                                    "new_supplier":         sender_mp_id.as_str(),
                                    "grid_operator":        receiver_gln.as_str(),
                                    "process_date":         process_date_clone,
                                    "bilanzierungsmethode": bilanzierungsmethode,
                                    "fallgruppe":           fallgruppe,
                                    "gasqualitaet":         gasqualitaet,
                                }),
                            )
                            .caused_by(1),
                        ],
                        vec![aperak_send_dl, process_dl],
                    ))
                } else {
                    let reason = if validation_errors.is_empty() {
                        "AHB validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    events.push(GasSupplierChangeEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 (Verarbeitbarkeitsfehlermeldung) \u2014 mandatory
                    // per APERAK AHB 1.0 \u00a72.1.1 when AHB validation fails.
                    // Validation failure \u2192 APERAK sent immediately: register the sending
                    // deadline so the OutboxWorker delivery is monitored.
                    let aperak_send_dl = if is_initialprozess {
                        PendingDeadline::new(
                            APERAK_GAS_INITIALPROZESS_LABEL,
                            aperak_gas_initialprozess_due_at(received_at),
                        )
                    } else {
                        PendingDeadline::new(
                            APERAK_GAS_FOLGEPROZESS_LABEL,
                            aperak_gas_folgeprozess_due_at(received_at),
                        )
                    };
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_mp_id.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        outbox,
                        vec![aperak_send_dl],
                    ))
                }
            }

            GasSupplierChangeCommand::SendAntwort {
                accepted,
                reason,
                obligations,
            } => {
                let data = match state {
                    GasSupplierChangeState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                let anfrage_pid = data.pruefidentifikator.as_u32();
                let response_pid = response_pid_for(anfrage_pid, accepted);

                let mut outbox_payload = serde_json::json!({
                    "anfrage_pid": anfrage_pid,
                    "accepted":    accepted,
                    "malo_id":     data.malo_id.as_str(),
                    "sender":      data.sender.as_str(),
                    "receiver":    data.receiver.as_str(),
                    "variant":     format!("{:?}", data.variant),
                });
                if let Some(ref r) = reason {
                    outbox_payload["reason"] = serde_json::Value::String(r.clone());
                }
                if let Some(rpid) = response_pid {
                    outbox_payload["response_pid"] =
                        serde_json::Value::Number(rpid.as_u32().into());
                }
                if let Some(ref mr) = data.message_ref {
                    outbox_payload["orig_message_ref"] =
                        serde_json::Value::String(mr.as_str().to_owned());
                }

                let mut all_outbox = vec![PendingOutbox::new(
                    "UtilmdAntwort",
                    data.sender.as_str(),
                    outbox_payload,
                )];
                if accepted {
                    all_outbox.extend(obligations);
                }

                let event = GasSupplierChangeEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason,
                };
                Ok(WorkflowOutput::with_outbox(vec![event], all_outbox))
            }

            GasSupplierChangeCommand::InitiateGnbProcess {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                process_date,
                message_ref,
            } => {
                if !matches!(state, GasSupplierChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !GNB_INITIATOR_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a valid GNB-initiator PID \
                         (expected one of: {GNB_INITIATOR_PIDS:?})"
                    )));
                }
                let variant = GasProcessVariant::from_anfrage_pid(pid.as_u32())
                    .expect("GNB_INITIATOR_PIDS are all valid Anfrage PIDs");

                let outbox_payload = serde_json::json!({
                    "anfrage_pid":   pid.as_u32(),
                    "variant":       format!("{variant:?}"),
                    "malo_id":       malo_id.as_str(),
                    "sender":        sender.as_str(),
                    "receiver":      receiver.as_str(),
                    "document_date": document_date,
                    "process_date":  process_date,
                    "message_ref":   message_ref.as_str(),
                });
                let outbox = vec![PendingOutbox::new(
                    "UtilmdAnfrage",
                    receiver.as_str(),
                    outbox_payload,
                )];

                let event = GasSupplierChangeEvent::GnbProcessInitiated {
                    variant,
                    sender,
                    receiver,
                    malo_id,
                    document_date,
                    process_date,
                    message_ref,
                    pruefidentifikator: pid,
                };
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid,
                accepted,
                reason,
            } => {
                let data = match state {
                    GasSupplierChangeState::GnbPending(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "GnbPending",
                            state.status_str(),
                        ));
                    }
                };
                let anfrage_pid = data.pruefidentifikator.as_u32();
                let expected_ok = response_pid_for(anfrage_pid, true);
                let expected_ng = response_pid_for(anfrage_pid, false);
                let valid = expected_ok.map(|p| p == response_pid).unwrap_or(false)
                    || expected_ng.map(|p| p == response_pid).unwrap_or(false);
                if !valid {
                    return Err(WorkflowError::rejected(format!(
                        "response PID {response_pid} does not match anfrage PID {anfrage_pid}"
                    )));
                }
                Ok(WorkflowOutput::events(vec![
                    GasSupplierChangeEvent::GnbAntwortErhalten {
                        response_pid,
                        accepted,
                        reason,
                    },
                ]))
            }

            GasSupplierChangeCommand::Activate => match state {
                GasSupplierChangeState::AntwortGesendet { data, .. }
                    if data.variant == GasProcessVariant::LieferbeginnGas =>
                {
                    Ok(WorkflowOutput::events(vec![
                        GasSupplierChangeEvent::Activated,
                    ]))
                }
                GasSupplierChangeState::AntwortGesendet { data, .. } => {
                    Err(WorkflowError::rejected(format!(
                        "Activate is only valid for LieferbeginnGas (44001); \
                         this process is {:?} (PID {})",
                        data.variant, data.pruefidentifikator
                    )))
                }
                _ => Err(WorkflowError::invalid_state(
                    "AntwortGesendet",
                    state.status_str(),
                )),
            },

            GasSupplierChangeCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                let mut outbox: Vec<PendingOutbox> = vec![];
                if let Some(data) = state.data() {
                    outbox.push(PendingOutbox::new(
                        "AperakTimeout",
                        data.sender.as_str(),
                        serde_json::json!({
                            "pid":            data.pruefidentifikator.as_u32(),
                            "variant":        format!("{:?}", data.variant),
                            "malo_id":        data.malo_id.as_str(),
                            "sender":         data.sender.as_str(),
                            "receiver":       data.receiver.as_str(),
                            "deadline_label": label.as_ref(),
                            "deadline_id":    deadline_id,
                        }),
                    ));
                }
                let event = GasSupplierChangeEvent::DeadlineExpired { deadline_id, label };
                if outbox.is_empty() {
                    Ok(WorkflowOutput::events(vec![event]))
                } else {
                    Ok(WorkflowOutput::with_outbox(vec![event], outbox))
                }
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single GeLi Gas process stream.
#[derive(Debug)]
pub enum GasSupplierChangeRecord {
    /// No `Initiated` event applied yet.
    New {
        /// Total events applied so far.
        event_count: usize,
    },
    /// `Initiated` event applied; process fields now available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Process variant.
        variant: GasProcessVariant,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// GLN of the sender.
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl GasSupplierChangeRecord {
    /// Current lifecycle status label.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }

    /// Return domain data if active.
    #[must_use]
    pub fn active_data(&self) -> Option<GasSupplierChangeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                variant,
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => Some(GasSupplierChangeRecordData {
                variant: *variant,
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
            }),
        }
    }
}

impl Default for GasSupplierChangeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// Borrowed view of `Active` record fields.
#[derive(Debug, Clone, Copy)]
pub struct GasSupplierChangeRecordData<'a> {
    /// Process variant.
    pub variant: GasProcessVariant,
    /// Marktlokation EIC code.
    pub malo_id: &'a MaLo,
    /// GLN of the sender.
    pub sender: &'a MarktpartnerCode,
    /// GLN of the receiver.
    pub receiver: &'a MarktpartnerCode,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

/// In-process read model for all GeLi Gas process streams.
#[derive(Debug, Default)]
pub struct GasSupplierChangeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, GasSupplierChangeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for GasSupplierChangeProjection {
    fn name(&self) -> &'static str {
        "GasSupplierChangeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        let Ok(event) = envelope.decode::<GasSupplierChangeEvent>() else {
            return;
        };

        match record {
            GasSupplierChangeRecord::New { event_count }
            | GasSupplierChangeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            GasSupplierChangeEvent::Initiated {
                variant,
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
                ..
            }
            | GasSupplierChangeEvent::GnbProcessInitiated {
                variant,
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = GasSupplierChangeRecord::Active {
                    status: "Initiated",
                    variant,
                    malo_id,
                    sender,
                    receiver,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            GasSupplierChangeEvent::ValidationPassed { .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            GasSupplierChangeEvent::AntwortGesendet { accepted, .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = if accepted {
                        "AntwortGesendet"
                    } else {
                        "Rejected"
                    };
                }
            }
            GasSupplierChangeEvent::GnbAntwortErhalten { accepted, .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = if accepted { "Completed" } else { "Rejected" };
                }
            }
            GasSupplierChangeEvent::Activated => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "Active";
                }
            }
            GasSupplierChangeEvent::Rejected { .. }
            | GasSupplierChangeEvent::DeadlineExpired { .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::ids::DeadlineId;

    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("test PID")
    }
    fn malo() -> MaLo {
        MaLo::new("DE0000000001234567890000000000001")
    }
    fn sender() -> MarktpartnerCode {
        MarktpartnerCode::new("4012345000023")
    }
    fn receiver() -> MarktpartnerCode {
        MarktpartnerCode::new("9900357000004")
    }

    fn receive_cmd(p: u32, valid: bool) -> GasSupplierChangeCommand {
        GasSupplierChangeCommand::ReceiveUtilmd {
            pid: pid(p),
            sender: sender(),
            receiver: receiver(),
            malo_id: malo(),
            document_date: "20250115".to_owned(),
            process_date: "20250301".to_owned(),
            message_ref: MessageRef::new("MSG-GELI-001"),
            validation_passed: valid,
            validation_errors: if valid {
                vec![]
            } else {
                vec!["AHB violation".to_owned()]
            },
            received_at: time::OffsetDateTime::now_utc(),
            bilanzierungsmethode: None,
            fallgruppe: None,
            gasqualitaet: None,
        }
    }

    fn gnb_cmd(p: u32) -> GasSupplierChangeCommand {
        GasSupplierChangeCommand::InitiateGnbProcess {
            pid: pid(p),
            sender: sender(),
            receiver: receiver(),
            malo_id: malo(),
            document_date: "20250115".to_owned(),
            process_date: "20250301".to_owned(),
            message_ref: MessageRef::new("MSG-GNB-001"),
        }
    }

    fn apply_all(
        state: GasSupplierChangeState,
        events: &[GasSupplierChangeEvent],
    ) -> GasSupplierChangeState {
        events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply)
    }

    // ── response_pid_for coverage ─────────────────────────────────────────────

    #[test]
    fn response_pid_table_is_correct() {
        let pairs: &[(u32, u32, u32)] = &[
            (44001, 44003, 44004),
            (44002, 44005, 44006),
            (44007, 44008, 44009),
            (44010, 44011, 44012),
            (44013, 44014, 44015),
            (44016, 44017, 44018),
        ];
        for &(anfrage, ok, ng) in pairs {
            assert_eq!(
                response_pid_for(anfrage, true).map(|p| p.as_u32()),
                Some(ok),
                "anfrage {anfrage} -> ok {ok}"
            );
            assert_eq!(
                response_pid_for(anfrage, false).map(|p| p.as_u32()),
                Some(ng),
                "anfrage {anfrage} -> ng {ng}"
            );
        }
        assert_eq!(response_pid_for(44019, true), None);
        assert_eq!(response_pid_for(44019, false), None);
        assert_eq!(
            response_pid_for(44020, true).map(|p| p.as_u32()),
            Some(44021)
        );
        assert_eq!(response_pid_for(44020, false), None);
    }

    // ── Lieferbeginn Gas (44001) ──────────────────────────────────────────────

    #[test]
    fn lieferbeginn_happy_path_to_active() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44001, true)).unwrap();
        assert_eq!(out.events.len(), 2);
        assert!(matches!(
            out.events[0],
            GasSupplierChangeEvent::Initiated {
                variant: GasProcessVariant::LieferbeginnGas,
                ..
            }
        ));
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::ValidationPassed(_)));

        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        assert!(
            !out.outbox.is_empty(),
            "must include UtilmdAntwort outbox entry"
        );
        assert!(matches!(
            &out.events[0],
            GasSupplierChangeEvent::AntwortGesendet { response_pid: Some(p), accepted: true, .. }
            if p.as_u32() == 44003
        ));
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            GasSupplierChangeState::AntwortGesendet { .. }
        ));

        let out = GeliGasSupplierChangeWorkflow::handle(&state, GasSupplierChangeCommand::Activate)
            .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Active(d) if d.malo_id == malo()));
    }

    #[test]
    fn lieferbeginn_negative_antwort_rejects() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44001, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: false,
                reason: Some("MaLo nicht bekannt".to_owned()),
                obligations: vec![],
            },
        )
        .unwrap();
        assert!(matches!(
            &out.events[0],
            GasSupplierChangeEvent::AntwortGesendet { response_pid: Some(p), accepted: false, .. }
            if p.as_u32() == 44004
        ));
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Rejected { .. }));
    }

    // ── Lieferende Gas (44002) ────────────────────────────────────────────────

    #[test]
    fn lieferende_positive_antwort_is_pid_44005() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44002, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        assert!(matches!(
            &out.events[0],
            GasSupplierChangeEvent::AntwortGesendet { response_pid: Some(p), accepted: true, .. }
            if p.as_u32() == 44005
        ));
    }

    // ── Abmeldung NN (44007) — responder (LFN) ───────────────────────────────

    #[test]
    fn abmeldung_nn_responder_positive_antwort_is_pid_44008() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44007, true)).unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(
            &state,
            GasSupplierChangeState::ValidationPassed(d)
            if d.variant == GasProcessVariant::AbmeldungNn
        ));
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        assert!(matches!(
            &out.events[0],
            GasSupplierChangeEvent::AntwortGesendet { response_pid: Some(p), accepted: true, .. }
            if p.as_u32() == 44008
        ));
    }

    // ── Abmeldung NN (44007) — initiator (GNB) ───────────────────────────────

    #[test]
    fn abmeldung_nn_gnb_initiator_positive_completes() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44007)).unwrap();
        assert!(!out.outbox.is_empty());
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::GnbPending(_)));

        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid: pid(44008),
                accepted: true,
                reason: None,
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Completed(_)));
    }

    #[test]
    fn abmeldung_nn_gnb_initiator_negative_rejects() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44007)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid: pid(44009),
                accepted: false,
                reason: Some("abgelehnt".to_owned()),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Rejected { .. }));
    }

    #[test]
    fn gnb_antwort_wrong_pid_is_rejected() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44007)).unwrap();
        let state = apply_all(state, &out.events);
        let err = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid: pid(44011),
                accepted: true,
                reason: None,
            },
        )
        .expect_err("PID mismatch must be rejected");
        assert!(
            err.to_string().contains("44011") || err.to_string().contains("44007"),
            "{err}"
        );
    }

    // ── Abmeldungsanfrage (44010) and EoG Anmeldung (44013) ──────────────────

    #[test]
    fn abmeldungsanfrage_gnb_initiator_completes() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44010)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid: pid(44011),
                accepted: true,
                reason: None,
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Completed(_)));
    }

    #[test]
    fn eog_anmeldung_gnb_initiator_completes() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44013)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid: pid(44014),
                accepted: true,
                reason: None,
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Completed(_)));
    }

    // ── Kündigung beim alten LF (44016) ──────────────────────────────────────

    #[test]
    fn kuendigung_lfa_positive_antwort_is_pid_44017() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44016, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        assert!(matches!(
            &out.events[0],
            GasSupplierChangeEvent::AntwortGesendet { response_pid: Some(p), accepted: true, .. }
            if p.as_u32() == 44017
        ));
    }

    // ── Bestandsliste (44019) — one-way ──────────────────────────────────────

    #[test]
    fn bestandsliste_initiator_completes_immediately() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44019)).unwrap();
        assert!(!out.outbox.is_empty());
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, GasSupplierChangeState::Completed(_)),
            "Bestandsliste (one-way) must complete immediately"
        );
    }

    #[test]
    fn bestandsliste_responder_completes_after_validation() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44019, true)).unwrap();
        assert_eq!(out.events.len(), 2, "Initiated + ValidationPassed");
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, GasSupplierChangeState::Completed(_)),
            "Bestandsliste receipt must auto-complete; got: {state:?}"
        );
    }

    // ── Änderungsmeldung (44020) ──────────────────────────────────────────────

    #[test]
    fn aenderungsmeldung_positive_antwort_is_pid_44021() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44020, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        assert!(matches!(
            &out.events[0],
            GasSupplierChangeEvent::AntwortGesendet { response_pid: Some(p), accepted: true, .. }
            if p.as_u32() == 44021
        ));
    }

    // ── Validation failure across all Anfrage PIDs ────────────────────────────

    #[test]
    fn validation_failure_rejects_for_all_anfrage_pids() {
        for &anfrage_pid in ANFRAGE_PIDS {
            let state = GasSupplierChangeState::default();
            let out =
                GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(anfrage_pid, false))
                    .unwrap_or_else(|e| panic!("PID {anfrage_pid}: {e}"));
            let state = apply_all(state, &out.events);
            assert!(
                matches!(state, GasSupplierChangeState::Rejected { .. }),
                "PID {anfrage_pid}: expected Rejected after validation failure"
            );
        }
    }

    // ── PID guards ────────────────────────────────────────────────────────────

    #[test]
    fn antwort_pids_cannot_start_a_process_via_receive_utilmd() {
        for &antwort_pid in ANTWORT_PIDS {
            let state = GasSupplierChangeState::default();
            GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(antwort_pid, true))
                .expect_err(&format!("Antwort PID {antwort_pid} must be rejected"));
        }
    }

    #[test]
    fn non_gnb_pid_for_initiate_gnb_is_rejected() {
        let state = GasSupplierChangeState::default();
        let err = GeliGasSupplierChangeWorkflow::handle(&state, gnb_cmd(44001))
            .expect_err("44001 is not a GNB-initiator PID");
        assert!(err.to_string().contains("44001"), "{err}");
    }

    // ── Activate guard ────────────────────────────────────────────────────────

    #[test]
    fn activate_rejected_for_non_lieferbeginn_variant() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44002, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        let err = GeliGasSupplierChangeWorkflow::handle(&state, GasSupplierChangeCommand::Activate)
            .expect_err("Activate must fail for LieferendeGas (44002)");
        assert!(
            err.to_string().contains("LieferbeginnGas") || err.to_string().contains("44002"),
            "{err}"
        );
    }

    // ── Deadline handling ─────────────────────────────────────────────────────

    #[test]
    fn deadline_rejects_validation_passed_state() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44001, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Rejected { .. }));
    }

    #[test]
    fn deadline_is_noop_for_active_state() {
        let state = GasSupplierChangeState::default();
        let out = GeliGasSupplierChangeWorkflow::handle(&state, receive_cmd(44001, true)).unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        let out = GeliGasSupplierChangeWorkflow::handle(&state, GasSupplierChangeCommand::Activate)
            .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSupplierChangeState::Active(_)));

        let out = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(
            out.events.is_empty(),
            "deadline must be no-op for Active state"
        );
    }

    // ── PID set coverage ──────────────────────────────────────────────────────

    #[test]
    fn anfrage_and_antwort_pids_are_subsets_of_utilmd_pids() {
        for &p in ANFRAGE_PIDS {
            assert!(
                UTILMD_PIDS.contains(&p),
                "ANFRAGE_PID {p} missing from UTILMD_PIDS"
            );
        }
        for &p in ANTWORT_PIDS {
            assert!(
                UTILMD_PIDS.contains(&p),
                "ANTWORT_PID {p} missing from UTILMD_PIDS"
            );
        }
    }

    #[test]
    fn utilmd_pids_covers_44001_to_44021() {
        for p in 44001..=44021u32 {
            assert!(UTILMD_PIDS.contains(&p), "PID {p} missing from UTILMD_PIDS");
        }
    }
}
