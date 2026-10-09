//! GeLi Gas LF-side Gas Sperrung / Entsperrung workflow (ORDERS 17115/17117, ORDRSP 19116/19117).
//!
//! When `makod` operates as a **Lieferant (LF)** in the Gas market, the ERP instructs
//! the engine to issue a Gas-Sperrauftrag (ORDERS 17115) or Gas-Entsperrauftrag
//! (ORDERS 17117) to the Gasnetzbetreiber (GNB). This workflow tracks the outbound
//! Sperrauftrag and all inbound responses.
//!
//! ## Process flow
//!
//! ```text
//! ERP → InitiateSperrung command
//!        ↓ emits AuftragInitiiert + ORDERS 17115/17117 outbox entry
//! AS4 sender → ORDERS 17115/17117 to GNB
//!        ↓
//! AS4 inbound ← ORDRSP 19116 (Bestätigung) or 19117 (Ablehnung) from GNB
//! ```
//!
//! For **Stornierung** (cancellation before execution):
//!
//! ```text
//! ERP → SendStornierung command
//!        ↓ emits StornierungGesendet + ORDCHG 39000 outbox entry
//! AS4 sender → ORDCHG 39000 to GNB
//!        ↓
//! AS4 inbound ← ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung) from GNB
//! ```
//!
//! ## Prüfidentifikatoren
//!
//! | Direction   | PID   | Description                                         |
//! |-------------|-------|-----------------------------------------------------|
//! | Outbound LF | 17115 | Gas-Sperrauftrag (LF → GNB)                         |
//! | Outbound LF | 17117 | Gas-Entsperrauftrag (LF → GNB)                      |
//! | Inbound GNB | 19116 | Bestätigung Sperr-/Entsperrauftrag (GNB → LF)       |
//! | Inbound GNB | 19117 | Ablehnung Sperr-/Entsperrauftrag (GNB → LF)         |
//! | Inbound GNB | 19128 | Bestätigung Stornierung Gas-Sperrauftrag (GNB → LF)  |
//! | Inbound GNB | 19129 | Ablehnung Stornierung Gas-Sperrauftrag (GNB → LF)   |
//!
//! ## Regulatory basis
//!
//! - **BDEW AWH „Unterbrechung / Wiederherstellung der Anschlussnutzung"** —
//!   the Gas Sperrprozesse live in an Anwendungshilfe, with the Gas
//!   Entscheidungsbäume `E_1000` / `E_1004`. **GeLi Gas 3.0 (BK7-24-01-009)
//!   contains no Sperrprozess.**
//! - **Frist**: the GNB answers the Auftrag on the Sparte-neutral row in
//!   [`mako_fristen::antwort`] — 1 Werktag, sourced from BK6-24-174 GPKE Teil 2
//!   § 3.5, the only text on hand that quantifies 17115 / 17117.
//! - Saturdays, Sundays and gesetzliche Feiertage are not Werktage
//!   (GeLi Gas 3.0 Kap. 2.6)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the LF-side Gas Sperrung workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-sperrung-lf";

/// Outbound PIDs that start a new `GeliGasSperrungLfWorkflow` (LF sends to GNB).
pub const SPERRUNG_ANFRAGE_PIDS: &[u32] = &[
    17115, // Gas-Sperrauftrag (LF → GNB)
    17117, // Gas-Entsperrauftrag (LF → GNB)
];

/// ORDRSP inbound response PIDs the LF receives from the GNB after Sperrauftrag.
///
/// These PIDs apply to both the electricity (GPKE) and gas (GeLi Gas) markets;
/// the process context (Strom vs. Gas) is resolved by correlation ID at runtime.
///
/// - 19116: Bestätigung Sperr-/Entsperrauftrag (GNB → LF)
/// - 19117: Ablehnung Sperr-/Entsperrauftrag (GNB → LF)
pub const ORDRSP_SPERRUNG_PIDS: &[u32] = &[19116, 19117];

/// ORDRSP inbound response PIDs the LF receives from the GNB after Stornierung.
///
/// - 19128: Bestätigung Stornierung Gas-Sperrauftrag (GNB → LF)
/// - 19129: Ablehnung Stornierung Gas-Sperrauftrag (GNB → LF)
pub const ORDRSP_STORNO_PIDS: &[u32] = &[19128, 19129];

/// Deadline label for the GNB's ORDRSP answer window on our Gas-Sperrauftrag.
///
/// **1 Werktag**, resolved per Prüfidentifikator through
/// [`mako_fristen::antwort::antwort_deadline`] — the same Sparte-neutral row
/// the Strom side uses, because 17115 / 17117 are one ORDERS Anwendungsfall in
/// both Sparten — see the module header for the Fundstelle.
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-sperrung-lf-antwort";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data captured when the LF initiates a Gas-Sperrauftrag.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GasSperrungAuftragData {
    /// Marktlokations-ID (EIC/MaLo) of the gas supply point.
    pub location_id: MaLo,
    /// GLN of the GNB that will execute the Gas-Sperrung.
    pub gnb_gln: MarktpartnerCode,
    /// BDEW Prüfidentifikator of the outbound ORDERS (17115 or 17117).
    pub pruefidentifikator: Pruefidentifikator,
    /// EDIFACT message reference of the outbound ORDERS.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas LF-side Gas Sperrung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasSperrungLfEvent {
    /// LF initiated a Gas-Sperrauftrag (ORDERS 17115) or Gas-Entsperrauftrag (ORDERS 17117).
    ///
    /// The ORDERS message is dispatched via the outbox at the same time.
    AuftragInitiiert {
        /// Marktlokations-ID of the gas supply point.
        location_id: MaLo,
        /// GLN of the GNB.
        gnb_gln: MarktpartnerCode,
        /// Prüfidentifikator (17115 = Sperrauftrag, 17117 = Entsperrauftrag).
        pruefidentifikator: Pruefidentifikator,
        /// EDIFACT message reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19116 (Bestätigung) or 19117 (Ablehnung) received from GNB.
    OrdrspEmpfangen {
        /// 19116 = Bestätigung, 19117 = Ablehnung.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for 19116 (GNB accepts and will execute), `false` for 19117 (GNB rejects).
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the sending GNB.
        sender: MarktpartnerCode,
    },
    /// LF sent a Stornierung (ORDCHG 39000) to cancel the pending Gas-Sperrauftrag.
    StornierungGesendet {
        /// EDIFACT message reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung Stornierung) received from GNB.
    StornoOrdrspEmpfangen {
        /// 19128 = Bestätigung, 19129 = Ablehnung.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for 19128 (GNB accepted cancellation), `false` for 19129.
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the sending GNB.
        sender: MarktpartnerCode,
    },
    /// A registered deadline expired before the GNB responded.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GasSperrungLfEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragInitiiert { .. } => "GasSperrungLfAuftragInitiiert",
            Self::OrdrspEmpfangen { .. } => "GasSperrungLfOrdrspEmpfangen",
            Self::StornierungGesendet { .. } => "GasSperrungLfStornierungGesendet",
            Self::StornoOrdrspEmpfangen { .. } => "GasSperrungLfStornoOrdrspEmpfangen",
            Self::DeadlineExpired { .. } => "GasSperrungLfDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GeLi Gas LF-side Gas Sperrung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → AuftragGesendet → OrdrspBestaetigt (GNB confirms execution)  [terminal]
///                       → OrdrspAbgelehnt  (GNB rejects)              [terminal]
///             ↘ StornierungGesendet → StornoBestaetigt                [terminal]
///                                   → StornoAbgelehnt                 [terminal]
///             ↘ DeadlineExpired (answer window expired)               [terminal]
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GasSperrungLfState {
    /// No events yet.
    #[default]
    New,
    /// Gas-Sperrauftrag sent outbound; awaiting GNB's ORDRSP.
    AuftragGesendet(GasSperrungAuftragData),
    /// GNB confirmed the Gas-Sperrauftrag (ORDRSP 19116, terminal success).
    OrdrspBestaetigt(GasSperrungAuftragData),
    /// GNB rejected the Gas-Sperrauftrag (ORDRSP 19117, terminal failure).
    OrdrspAbgelehnt {
        /// Optional rejection reason from ORDRSP.
        reason: Option<String>,
    },
    /// Stornierung sent; awaiting GNB's Storno-ORDRSP (19128/19129).
    StornierungGesendet(GasSperrungAuftragData),
    /// GNB accepted the Stornierung (ORDRSP 19128, terminal).
    StornoBestaetigt(GasSperrungAuftragData),
    /// GNB rejected the Stornierung (ORDRSP 19129, terminal — must proceed with Sperrung).
    StornoAbgelehnt(GasSperrungAuftragData),
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the GNB leaving the Gas-Sperrauftrag unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GasSperrungLfState>,
    },
}

impl GasSperrungLfState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragGesendet(_) => "AuftragGesendet",
            Self::OrdrspBestaetigt(_) => "OrdrspBestaetigt",
            Self::OrdrspAbgelehnt { .. } => "OrdrspAbgelehnt",
            Self::StornierungGesendet(_) => "StornierungGesendet",
            Self::StornoBestaetigt(_) => "StornoBestaetigt",
            Self::StornoAbgelehnt(_) => "StornoAbgelehnt",
            Self::FristAbgelaufen { .. } => "FristAbgelaufen",
        }
    }

    /// The state the process is waiting in — itself, unless a window lapsed.
    #[must_use]
    pub fn offen(&self) -> &Self {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.offen(),
            other => other,
        }
    }

    /// Returns `true` if this is a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::OrdrspBestaetigt(_)
                | Self::OrdrspAbgelehnt { .. }
                | Self::StornoBestaetigt(_)
                | Self::StornoAbgelehnt(_)
        )
    }

    /// Return `Some(&GasSperrungAuftragData)` if the process has domain data.
    #[must_use]
    pub fn auftrag_data(&self) -> Option<&GasSperrungAuftragData> {
        match self {
            Self::AuftragGesendet(d)
            | Self::OrdrspBestaetigt(d)
            | Self::StornierungGesendet(d)
            | Self::StornoBestaetigt(d)
            | Self::StornoAbgelehnt(d) => Some(d),
            Self::FristAbgelaufen { offen, .. } => offen.auftrag_data(),
            Self::New | Self::OrdrspAbgelehnt { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas LF-side Gas Sperrung workflow.
#[derive(Clone)]
pub enum GasSperrungLfCommand {
    /// ERP instructs the LF to issue a Gas-Sperrauftrag (17115) or Gas-Entsperrauftrag (17117).
    ///
    /// Emits `AuftragInitiiert` + an outbox entry for the ORDERS message to the GNB.
    InitiateSperrung {
        /// Prüfidentifikator (17115 = Sperrauftrag, 17117 = Entsperrauftrag).
        pid: Pruefidentifikator,
        /// GLN of the GNB to send the Gas-Sperrauftrag to.
        gnb_gln: MarktpartnerCode,
        /// Marktlokations-ID of the gas supply point.
        location_id: MaLo,
        /// EDIFACT message reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// When the ORDERS leaves — the Übertragungstag the counterparty's
        /// Antwortfrist counts from ([`mako_fristen::antwort::antwort_deadline`]).
        gesendet_am: time::OffsetDateTime,
    },
    /// Inbound ORDRSP 19116/19117 from GNB received by the AS4 layer.
    ReceiveOrdrsp {
        /// 19116 = Bestätigung, 19117 = Ablehnung.
        pid: Pruefidentifikator,
        /// `true` for 19116 (confirmed), `false` for 19117 (rejected).
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the GNB sender.
        sender: MarktpartnerCode,
        /// Optional rejection reason from the ORDRSP (19117 only).
        reason: Option<String>,
    },
    /// ERP instructs the LF to cancel the pending Gas-Sperrauftrag.
    ///
    /// Emits `StornierungGesendet` + outbox entry for ORDCHG 39000.
    SendStornierung {
        /// EDIFACT message reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19128/19129 from GNB received after Stornierung.
    ReceiveStornoOrdrsp {
        /// 19128 = Bestätigung, 19129 = Ablehnung.
        pid: Pruefidentifikator,
        /// `true` for 19128 (accepted), `false` for 19129 (rejected).
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the GNB sender.
        sender: MarktpartnerCode,
    },
    /// A registered deadline (the GNB's answer window) fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for GasSperrungLfCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas LF-side Gas Sperrung / Entsperrung workflow (ORDERS 17115/17117).
///
/// Handles the Lieferant's outbound Gas-Sperrauftrag lifecycle and all inbound
/// responses from the Gasnetzbetreiber (ORDRSP 19116/19117) and Stornierung
/// responses (ORDRSP 19128/19129).
///
/// Deadline: **1 Werktag** on the Sparte-neutral 17115 / 17117 row. Compute with
/// `mako_fristen::add_werktage(date, 10, BdewMaKo)`.
pub struct GeliGasSperrungLfWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The GNB's answer window is open while the Gas-Sperrauftrag waits for the
/// ORDRSP (`AuftragGesendet`).
fn window_open(label: &str, state: &GasSperrungLfState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, GasSperrungLfState::AuftragGesendet(_))
}

impl Workflow for GeliGasSperrungLfWorkflow {
    type State = GasSperrungLfState;
    type Event = GasSperrungLfEvent;
    type Command = GasSperrungLfCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GasSperrungLfCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GasSperrungLfState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GasSperrungLfEvent::AuftragInitiiert {
                location_id,
                gnb_gln,
                pruefidentifikator,
                message_ref,
            } => GasSperrungLfState::AuftragGesendet(GasSperrungAuftragData {
                location_id: location_id.clone(),
                gnb_gln: gnb_gln.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: message_ref.clone(),
            }),

            GasSperrungLfEvent::OrdrspEmpfangen {
                is_confirmed,
                pruefidentifikator: _,
                message_ref: _,
                sender: _,
            } => match state {
                GasSperrungLfState::AuftragGesendet(data) => {
                    if *is_confirmed {
                        GasSperrungLfState::OrdrspBestaetigt(data)
                    } else {
                        GasSperrungLfState::OrdrspAbgelehnt { reason: None }
                    }
                }
                // Terminal or unexpected state: ignore (idempotent replay).
                other => other,
            },

            GasSperrungLfEvent::StornierungGesendet { .. } => match state {
                GasSperrungLfState::AuftragGesendet(data) => {
                    GasSperrungLfState::StornierungGesendet(data)
                }
                // Already terminal or confirmed: ignore.
                other => other,
            },

            GasSperrungLfEvent::StornoOrdrspEmpfangen {
                is_confirmed,
                pruefidentifikator: _,
                message_ref: _,
                sender: _,
            } => match state {
                GasSperrungLfState::StornierungGesendet(data) => {
                    if *is_confirmed {
                        GasSperrungLfState::StornoBestaetigt(data)
                    } else {
                        GasSperrungLfState::StornoAbgelehnt(data)
                    }
                }
                other => other,
            },

            GasSperrungLfEvent::DeadlineExpired { label, .. } => match state {
                GasSperrungLfState::AuftragGesendet(_) => GasSperrungLfState::FristAbgelaufen {
                    label: label.to_string(),
                    offen: Box::new(state),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            GasSperrungLfCommand::InitiateSperrung {
                pid,
                gnb_gln,
                location_id,
                message_ref,
                gesendet_am,
            } => {
                if !matches!(state, GasSperrungLfState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !SPERRUNG_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas-Sperrung PID (17115 or 17117), got {pid}",
                    )));
                }
                let outbox = PendingOutbox::new(
                    "ORDERS",
                    gnb_gln.as_str(),
                    serde_json::json!({
                        "type":        "GasSperrungAuftrag",
                        // See the Strom counterpart: 17115/17117 name the same
                        // Marktrolle in both Sparten, so the sender MP-ID can
                        // only be resolved with the Sparte stated here.
                        "sparte":      "Gas",
                        "pid":         pid.as_u32(),
                        "location_id": location_id.as_str(),
                        "message_ref": message_ref.as_str(),
                    }),
                );
                let event = GasSperrungLfEvent::AuftragInitiiert {
                    location_id,
                    gnb_gln,
                    pruefidentifikator: pid,
                    message_ref,
                };
                // The counterparty's Antwortfrist, from the published row for
                // this ORDERS; a PID no Festlegung quantifies registers nothing.
                let frist: Vec<PendingDeadline> =
                    mako_fristen::antwort::antwort_deadline(pid.as_u32(), gesendet_am)
                        .map(|due| PendingDeadline::new(ANTWORT_WINDOW_LABEL, due))
                        .into_iter()
                        .collect();
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![event],
                    vec![outbox],
                    frist,
                ))
            }

            GasSperrungLfCommand::ReceiveOrdrsp {
                pid,
                is_confirmed,
                message_ref,
                sender,
                reason: _,
            } => {
                if !ORDRSP_SPERRUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas-Sperrung ORDRSP PID (19116 or 19117), got {pid}",
                    )));
                }
                // Idempotent: ignore if already in terminal state.
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(WorkflowOutput::events(vec![
                    GasSperrungLfEvent::OrdrspEmpfangen {
                        pruefidentifikator: pid,
                        is_confirmed,
                        message_ref,
                        sender,
                    },
                ]))
            }

            GasSperrungLfCommand::SendStornierung { message_ref } => match state {
                GasSperrungLfState::AuftragGesendet(data) => {
                    let outbox = PendingOutbox::new(
                        "ORDCHG",
                        data.gnb_gln.as_str(),
                        serde_json::json!({
                            "type":        "GasSperrungStornierung",
                            "pid":         39000,
                            "location_id": data.location_id.as_str(),
                            "message_ref": message_ref.as_str(),
                        }),
                    );
                    let event = GasSperrungLfEvent::StornierungGesendet { message_ref };
                    Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
                }
                other => Err(WorkflowError::invalid_state(
                    "AuftragGesendet",
                    other.label(),
                )),
            },

            GasSperrungLfCommand::ReceiveStornoOrdrsp {
                pid,
                is_confirmed,
                message_ref,
                sender,
            } => {
                if !ORDRSP_STORNO_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas-Sperrung Storno-ORDRSP PID (19128 or 19129), got {pid}",
                    )));
                }
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(WorkflowOutput::events(vec![
                    GasSperrungLfEvent::StornoOrdrspEmpfangen {
                        pruefidentifikator: pid,
                        is_confirmed,
                        message_ref,
                        sender,
                    },
                ]))
            }

            GasSperrungLfCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(WorkflowOutput::events(vec![
                    GasSperrungLfEvent::DeadlineExpired { deadline_id, label },
                ]))
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::{
        ids::DeadlineId,
        types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
        workflow::Workflow,
    };

    fn gnb() -> MarktpartnerCode {
        MarktpartnerCode::from("4012345678901")
    }

    fn malo() -> MaLo {
        MaLo::from("50MALO000000001G")
    }

    fn mref(s: &str) -> MessageRef {
        MessageRef::from(s)
    }

    fn pid_sperrauftrag() -> Pruefidentifikator {
        Pruefidentifikator::new(17115).expect("valid PID")
    }

    fn pid_entsperrauftrag() -> Pruefidentifikator {
        Pruefidentifikator::new(17117).expect("valid PID")
    }

    fn pid_bestaetigung() -> Pruefidentifikator {
        Pruefidentifikator::new(19116).expect("valid PID")
    }

    fn pid_ablehnung() -> Pruefidentifikator {
        Pruefidentifikator::new(19117).expect("valid PID")
    }

    /// The Auftrag starts the GNB's answer window — the Sparte-neutral 1-WT row
    /// for 17115 — under the label the workflow decides.
    #[test]
    fn an_auftrag_registers_the_gnb_answer_window() {
        let sent = time::macros::datetime!(2026-10-07 10:00 UTC);
        let out = GeliGasSperrungLfWorkflow::handle(
            &GasSperrungLfState::default(),
            GasSperrungLfCommand::InitiateSperrung {
                pid: pid_sperrauftrag(),
                gnb_gln: gnb(),
                location_id: malo(),
                message_ref: mref("MSG-001"),
                gesendet_am: sent,
            },
        )
        .unwrap();
        assert_eq!(out.deadlines.len(), 1);
        assert_eq!(out.deadlines[0].label, ANTWORT_WINDOW_LABEL);
        assert_eq!(
            Some(out.deadlines[0].due_at),
            mako_fristen::antwort::antwort_deadline(17_115, sent)
        );
    }

    #[test]
    fn initiate_sperrauftrag_emits_event_and_outbox() {
        let state = GasSperrungLfState::default();
        let cmd = GasSperrungLfCommand::InitiateSperrung {
            pid: pid_sperrauftrag(),
            gnb_gln: gnb(),
            location_id: malo(),
            message_ref: mref("MSG-001"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1);
        assert!(matches!(
            out.events[0],
            GasSperrungLfEvent::AuftragInitiiert { .. }
        ));
    }

    #[test]
    fn initiate_entsperrauftrag_emits_event_and_outbox() {
        let state = GasSperrungLfState::default();
        let cmd = GasSperrungLfCommand::InitiateSperrung {
            pid: pid_entsperrauftrag(),
            gnb_gln: gnb(),
            location_id: malo(),
            message_ref: mref("MSG-002"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1);
    }

    #[test]
    fn invalid_pid_rejected() {
        let state = GasSperrungLfState::default();
        let cmd = GasSperrungLfCommand::InitiateSperrung {
            pid: Pruefidentifikator::new(99999).expect("valid PID"),
            gnb_gln: gnb(),
            location_id: malo(),
            message_ref: mref("MSG-003"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        };
        assert!(GeliGasSperrungLfWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn apply_initiate_transitions_to_auftrag_gesendet() {
        let state = GasSperrungLfState::default();
        let event = GasSperrungLfEvent::AuftragInitiiert {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-004"),
        };
        let new_state = GeliGasSperrungLfWorkflow::apply(state, &event);
        assert!(matches!(new_state, GasSperrungLfState::AuftragGesendet(_)));
    }

    #[test]
    fn ordrsp_bestaetigung_terminates_process() {
        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-005"),
        };
        let state = GasSperrungLfState::AuftragGesendet(data);
        let cmd = GasSperrungLfCommand::ReceiveOrdrsp {
            pid: pid_bestaetigung(),
            is_confirmed: true,
            message_ref: mref("ORDRSP-001"),
            sender: gnb(),
            reason: None,
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            GasSperrungLfEvent::OrdrspEmpfangen {
                is_confirmed: true,
                ..
            }
        ));
        let final_state = GeliGasSperrungLfWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            final_state,
            GasSperrungLfState::OrdrspBestaetigt(_)
        ));
        assert!(final_state.is_terminal());
    }

    #[test]
    fn ordrsp_ablehnung_terminates_process() {
        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-006"),
        };
        let state = GasSperrungLfState::AuftragGesendet(data);
        let cmd = GasSperrungLfCommand::ReceiveOrdrsp {
            pid: pid_ablehnung(),
            is_confirmed: false,
            message_ref: mref("ORDRSP-002"),
            sender: gnb(),
            reason: Some("Zähler nicht erreichbar".to_owned()),
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        let final_state = GeliGasSperrungLfWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            final_state,
            GasSperrungLfState::OrdrspAbgelehnt { .. }
        ));
        assert!(final_state.is_terminal());
    }

    #[test]
    fn stornierung_happy_path() {
        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-007"),
        };
        let state = GasSperrungLfState::AuftragGesendet(data);
        let cmd = GasSperrungLfCommand::SendStornierung {
            message_ref: mref("STORNO-001"),
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1);
        assert!(matches!(
            out.events[0],
            GasSperrungLfEvent::StornierungGesendet { .. }
        ));

        let storno_state = GeliGasSperrungLfWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            storno_state,
            GasSperrungLfState::StornierungGesendet(_)
        ));

        let storno_cmd = GasSperrungLfCommand::ReceiveStornoOrdrsp {
            pid: Pruefidentifikator::new(19128).expect("valid PID"),
            is_confirmed: true,
            message_ref: mref("STORNO-ORDRSP-001"),
            sender: gnb(),
        };
        let out2 = GeliGasSperrungLfWorkflow::handle(&storno_state, storno_cmd).unwrap();
        let final_state = GeliGasSperrungLfWorkflow::apply(storno_state, &out2.events[0]);
        assert!(matches!(
            final_state,
            GasSperrungLfState::StornoBestaetigt(_)
        ));
        assert!(final_state.is_terminal());
    }

    /// The GNB's silence is a Frist breach that leaves the Auftrag waiting.
    #[test]
    fn timeout_marks_the_waiting_process_lapsed() {
        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-008"),
        };
        let state = GasSperrungLfState::AuftragGesendet(data);
        let cmd = GasSperrungLfCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANTWORT_WINDOW_LABEL.into(),
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        let final_state = GeliGasSperrungLfWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            final_state,
            GasSperrungLfState::FristAbgelaufen { .. }
        ));
        assert!(!final_state.is_terminal());
    }

    #[test]
    fn duplicate_receipt_is_idempotent() {
        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-009"),
        };
        // Process already confirmed — should accept additional ORDRSP silently.
        let state = GasSperrungLfState::OrdrspBestaetigt(data);
        let cmd = GasSperrungLfCommand::ReceiveOrdrsp {
            pid: pid_bestaetigung(),
            is_confirmed: true,
            message_ref: mref("ORDRSP-DUP"),
            sender: gnb(),
            reason: None,
        };
        let out = GeliGasSperrungLfWorkflow::handle(&state, cmd).unwrap();
        assert!(out.events.is_empty(), "duplicate ORDRSP must be a no-op");
    }

    #[test]
    fn on_deadline_fires_timeout_command_when_awaiting() {
        use mako_engine::deadline::Deadline;
        use mako_engine::ids::{ProcessId, StreamId, TenantId};
        use mako_engine::version::WorkflowId;

        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-010"),
        };
        let state = GasSperrungLfState::AuftragGesendet(data);
        let deadline = Deadline::new(
            StreamId::new("process/test-010"),
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new(WORKFLOW_NAME, "FV2025-10-01"),
            ANTWORT_WINDOW_LABEL,
            time::macros::datetime!(2025-01-01 12:00:00 UTC),
        );
        let cmd = GeliGasSperrungLfWorkflow::on_deadline(&deadline, &state);
        assert!(
            cmd.is_some(),
            "on_deadline should fire when AuftragGesendet"
        );
    }

    #[test]
    fn on_deadline_does_not_fire_when_terminal() {
        use mako_engine::deadline::Deadline;
        use mako_engine::ids::{ProcessId, StreamId, TenantId};
        use mako_engine::version::WorkflowId;

        let state = GasSperrungLfState::OrdrspBestaetigt(GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-011"),
        });
        let deadline = Deadline::new(
            StreamId::new("process/test-011"),
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new(WORKFLOW_NAME, "FV2025-10-01"),
            ANTWORT_WINDOW_LABEL,
            time::macros::datetime!(2025-01-01 12:00:00 UTC),
        );
        let cmd = GeliGasSperrungLfWorkflow::on_deadline(&deadline, &state);
        assert!(
            cmd.is_none(),
            "on_deadline must not fire for terminal state"
        );
    }

    #[test]
    fn workflow_name_constant_is_correct() {
        assert_eq!(WORKFLOW_NAME, "geli-gas-sperrung-lf");
    }

    #[test]
    fn sperrauftrag_cannot_be_initiated_twice() {
        let data = GasSperrungAuftragData {
            location_id: malo(),
            gnb_gln: gnb(),
            pruefidentifikator: pid_sperrauftrag(),
            message_ref: mref("MSG-012"),
        };
        let state = GasSperrungLfState::AuftragGesendet(data);
        let cmd = GasSperrungLfCommand::InitiateSperrung {
            pid: pid_sperrauftrag(),
            gnb_gln: gnb(),
            location_id: malo(),
            message_ref: mref("MSG-013"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        };
        assert!(
            GeliGasSperrungLfWorkflow::handle(&state, cmd).is_err(),
            "must reject double-initiation"
        );
    }

    #[test]
    fn stornierung_from_non_auftrag_state_is_rejected() {
        let state = GasSperrungLfState::New;
        let cmd = GasSperrungLfCommand::SendStornierung {
            message_ref: mref("STORNO-INVALID"),
        };
        assert!(
            GeliGasSperrungLfWorkflow::handle(&state, cmd).is_err(),
            "Stornierung from New state must fail"
        );
    }
}
