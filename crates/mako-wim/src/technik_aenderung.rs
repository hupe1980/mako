//! WiM Strom/Gas Technikänderung — device/measurement-point change requests.
//!
//! This module handles ORDERS-based requests for technical changes at a
//! Messlokation:
//!
//! | PID   | Direction  | Description |
//! |-------|------------|-------------|
//! | 17011 | NB / LF → MSB | Beauftragung resp. Bestellung zur Änderung der Technik |
//! | 17118 | MSB → MSB  | Bestellung einer Konfigurationsänderung (see below) |
//! | 19005 | MSB → NB/LF | Bestätigung Auftrag Änderung Technik |
//! | 19006 | MSB → NB/LF | Ablehnung Auftrag Änderung Technik |
//!
//! # Both sides
//!
//! mako sends the Auftrag as NB or LF ([`TechnikAenderungCommand::SendAuftrag`])
//! and receives it as the MSB ([`TechnikAenderungCommand::ReceiveAuftrag`]).
//! The received Auftrag runs under [`auftrag_key`] — the sender and the
//! ORDERS' Dokumentennummer — so a deployment that is NB and MSB at once holds
//! its own sent Auftrag and the received copy as two processes. Only 17011 is
//! received here: the Anwendungsübersicht 4.0 (lfd. Nr. 22600–23180) files
//! 17118 under GPKE Teil 3 „Bestellung einer Konfiguration" / „Bestellung
//! Beendigung einer Konfiguration", answered by ORDRSP 19127 out of
//! `E_0527` / `E_0539`, not by 19005/19006.
//!
//! **17121 is not here.** „Bestellung Änderung (NB an MSB)" belongs to GPKE
//! Teil 3 and is answered with ORDRSP 19120 out of `E_0526`; `mako-gpke` owns
//! it.
//!
//! The ESA Ab-/Bestellung PIDs (ORDERS 17007, ORDRSP 19011/19012/19013/19014)
//! belong to [`crate::wertebestellung`], which models their own lifecycle.
//!
//! # Two documents, one message pair
//!
//! WiM Strom Teil 1 Kap. 3.3 has the NB or the LF order the change outright;
//! the BDEW *AWH Prozesse zur Änderung der Technik an Lokationen* V1.1 puts a
//! REQOTE 35005 / QUOTES 15005 round in front of the same order. Both end in
//! ORDERS 17011 → ORDRSP 19005/19006, so **four** Entscheidungsbäume share one
//! answer PID pair and the sender's Marktrolle resolves only half of it — see
//! [`mako_pruefung::msb::technik`], which is where that resolution lives.
//!
//! The leg after the answer is the **Durchführung**: the MSB goes to the
//! Lokation and reports back only if the visit failed, on IFTSTA 21027 (to the
//! NB) resp. 21025 (to the LF) out of `E_0286`.
//!
//! # Regulatory basis
//!
//! - **BK6-24-174** — WiM Strom Teil 1 Kap. 3.3 (Messlokationsänderung)
//! - **BDEW AWH Prozesse zur Änderung der Technik an Lokationen** V1.1 (31.03.2025)
//! - **Entscheidungsbaum-Diagramme und Codelisten 4.3** Kap. 8.6, 8.7, 9.1, 9.2
//! - Antwortfrist: **10 Werktage** (WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2
//!   Nr. 2), against a Mindestvorlauffrist of **20 Werktagen** on the direct
//!   Beauftragung (Nr. 1). Both are anchored differently: the answer window runs
//!   forward from the ÜT, the Vorlauffrist backward from the gewünschter
//!   Änderungstermin — see [`mako_fristen::vorlauf`]. The AWH Bestellung has no
//!   Vorlauffrist Prüfschritt at all; its Umsetzungszeitraum was agreed in the
//!   Angebot.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the WiM Technikänderung process.
pub const WORKFLOW_NAME: &str = "wim-technik-aenderung";

/// ORDERS PIDs for technical change requests (all directions).
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17011 | Beauftragung zur Änderung der Technik (Messlokationsänderung Strom) |
/// | 17118 | Bestellung Konfigurationsänderung (MSB → MSB) |
pub const ORDERS_PIDS: &[u32] = &[17011, 17118];

/// ORDRSP PIDs received in response to technical change requests.
///
/// | PID   | Description | EBD |
/// |-------|-------------|-----|
/// | 19005 | Bestätigung Auftrag Änderung Technik | `E_0249` / `E_0250` |
/// | 19006 | Ablehnung Auftrag Änderung Technik | `E_0249` / `E_0250` |
///
/// Three neighbouring ORDRSP PIDs answer other processes (ORDRSP AHB 1.1b
/// §§ 4.11–4.13) and are not part of this set:
///
/// - **19003 / 19004** „Fortführungsbestätigung / Ablehnung Fortführung"
///   answer the Weiterverpflichtung ORDERS 17002 out of `E_0203` —
///   [`crate::weiterverpflichtung`].
/// - **19007** „Ablehnung Anforderung von Werten" answers a Werteanforderung.
///
/// # Four trees, one PID pair
///
/// 19005/19006 are shared by `E_0249`/`E_0250` (WiM Teil 1, direct
/// Beauftragung) and `E_0279`/`E_0283` (AWH, Bestellung nach Angebot). The
/// sender's Marktrolle separates the columns; the ORDERS' Zuordnung zu einem
/// Objekt (`ZO-T15` against `ZG-T24`) separates the rows. Resolve with
/// [`mako_pruefung::codes::aenderung_der_technik_baum`] — never against the
/// answer PID, and never on the Marktrolle alone: `A02` is the Zustimmung of
/// `E_0249` and an Ablehnung of `E_0279`.
pub const ORDRSP_PIDS: &[u32] = &[19005, 19006];

/// Positive ORDRSP PIDs (confirmation).
const ORDRSP_BESTAETIGUNG_PIDS: &[u32] = &[19005];

/// The ORDERS the MSB receives here: the Beauftragung Änderung der Technik
/// 17011, „NB, LF an MSB" (ORDERS AHB 1.1b Kap. 4.12, S. 83).
pub const RECEIVE_ORDERS_PIDS: &[u32] = &[17_011];

/// Deadline label for the response window (10 Werktage, WiM Strom).
pub const ANTWORT_WINDOW_LABEL: &str = "wim-technik-aenderung-antwort";

/// Deadline label for the MSB's own answer window on a received Auftrag —
/// „Unverzüglich, jedoch spätester ÜT ist der 10. WT nach dem ÜT von Nr. 1"
/// (BK6-24-174 WiM Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 2, S. 53), the 17011 row
/// of [`mako_fristen::antwort`].
pub const EINGANG_WINDOW_LABEL: &str = "wim-technik-aenderung-eingang";

/// The key a received Auftrag runs under: the sender's MP-ID and the ORDERS'
/// Dokumentennummer (`BGM` DE 1004).
///
/// Not the Messlokation: the answer leg is not built, so a process keyed on
/// the Lokation would stay open and refuse the next Auftrag for it. The number
/// is unique only per sender, hence the MP-ID.
#[must_use]
pub fn auftrag_key(sender: &str, dokumentennummer: &str) -> String {
    format!("technik-auftrag:{sender}:{dokumentennummer}")
}

/// When the MSB's answer to the received ORDERS `pid` is due, from its row in
/// [`mako_fristen::antwort`].
#[must_use]
pub fn eingang_antwortfrist(
    pid: u32,
    eingang: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    RECEIVE_ORDERS_PIDS
        .contains(&pid)
        .then(|| mako_fristen::antwort::antwort_deadline(pid, eingang))
        .flatten()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Technikänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TechnikAenderungEvent {
    /// ORDERS technical change request sent.
    AuftragGesendet {
        /// ORDERS Prüfidentifikator.
        orders_pid: Pruefidentifikator,
        /// Recipient GLN (MSB or NB).
        recipient: MarktpartnerCode,
        /// Affected MeLo/MaLo.
        location_id: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSB side: an ORDERS Auftrag arrived.
    AuftragEingegangen {
        /// ORDERS Prüfidentifikator.
        orders_pid: Pruefidentifikator,
        /// The NB or LF that sent it — `NAD+MS`.
        sender: MarktpartnerCode,
        /// The Messlokation — `SG2 LOC+172`.
        location_id: Option<String>,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
    },
    /// MSB side: the received ORDERS failed its AHB check.
    AuftragUngueltig {
        /// The findings.
        reason: String,
    },
    /// ORDRSP received — confirmation.
    AuftragBestaetigt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// ORDRSP received — rejection.
    AuftragAbgelehnt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline expired before ORDRSP arrived.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for TechnikAenderungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragGesendet { .. } => "TechnikAenderungAuftragGesendet",
            Self::AuftragEingegangen { .. } => "TechnikAenderungAuftragEingegangen",
            Self::AuftragUngueltig { .. } => "TechnikAenderungAuftragUngueltig",
            Self::AuftragBestaetigt { .. } => "TechnikAenderungAuftragBestaetigt",
            Self::AuftragAbgelehnt { .. } => "TechnikAenderungAuftragAbgelehnt",
            Self::DeadlineExpired { .. } => "TechnikAenderungDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        let (richtung, pid, message_ref) = match self {
            Self::AuftragGesendet {
                orders_pid,
                message_ref,
                ..
            } => (Richtung::Ausgang, orders_pid, message_ref),
            Self::AuftragEingegangen {
                orders_pid,
                message_ref,
                ..
            } => (Richtung::Eingang, orders_pid, message_ref),
            Self::AuftragBestaetigt {
                ordrsp_pid,
                message_ref,
            }
            | Self::AuftragAbgelehnt {
                ordrsp_pid,
                message_ref,
                ..
            } => (Richtung::Eingang, ordrsp_pid, message_ref),
            Self::DeadlineExpired { .. } | Self::AuftragUngueltig { .. } => return None,
        };
        Some(Verlauf {
            richtung,
            pid: Some(pid.as_u32()),
            message_ref: Some(message_ref.as_str().to_owned()),
        })
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Request data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuftragData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// Recipient GLN.
    pub recipient: MarktpartnerCode,
    /// Location identifier.
    pub location_id: Option<String>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// A received Auftrag (MSB side).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EingangData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// The NB or LF that sent it.
    pub sender: MarktpartnerCode,
    /// The Messlokation.
    pub location_id: Option<String>,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
}

/// Current state of a WiM Technikänderung process.
///
/// ```text
/// New → AuftragGesendet → Bestaetigt
///                      ↘ Abgelehnt
///                      ↘ FristAbgelaufen
/// New → AuftragEingegangen → FristAbgelaufen   (MSB side)
///     ↘ Ungueltig
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum TechnikAenderungState {
    /// No ORDERS sent yet.
    #[default]
    New,
    /// ORDERS sent; awaiting ORDRSP.
    AuftragGesendet(AuftragData),
    /// MSB side: an Auftrag arrived; the MSB owes the ORDRSP 19005/19006.
    AuftragEingegangen(EingangData),
    /// MSB side: the received Auftrag failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// MSB/NB confirmed the change.
    Bestaetigt {
        /// Original request data.
        auftrag: AuftragData,
        /// Confirmation ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
    },
    /// MSB/NB rejected the change.
    Abgelehnt {
        /// Original request data.
        auftrag: AuftragData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the Änderung der Technik going unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<TechnikAenderungState>,
    },
}

impl TechnikAenderungState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragGesendet(_) => "AuftragGesendet",
            Self::AuftragEingegangen(_) => "AuftragEingegangen",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
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

    /// Whether terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Bestaetigt { .. } | Self::Abgelehnt { .. } | Self::Ungueltig { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Technikänderung workflow.
#[derive(Clone)]
pub enum TechnikAenderungCommand {
    /// Send an ORDERS technical change request.
    SendAuftrag {
        /// ORDERS PID.
        orders_pid: Pruefidentifikator,
        /// Recipient GLN.
        recipient: MarktpartnerCode,
        /// Location ID (optional).
        location_id: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
        /// ORDERS body payload.
        payload: serde_json::Value,
        /// When the ORDERS leaves — the Übertragungstag the counterparty's
        /// Antwortfrist counts from ([`mako_fristen::antwort::antwort_deadline`]).
        gesendet_am: time::OffsetDateTime,
    },
    /// MSB side: an inbound ORDERS Auftrag (17011).
    ReceiveAuftrag {
        /// ORDERS PID read from `SG1 RFF+Z13`.
        orders_pid: Pruefidentifikator,
        /// The NB or LF that sent it — `NAD+MS`.
        sender: MarktpartnerCode,
        /// The Messlokation — `SG2 LOC+172`.
        location_id: Option<String>,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// Inbound ORDRSP received.
    ReceiveOrdrsp {
        /// ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason (for negative responses).
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline fired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Label.
        label: Box<str>,
    },
}

impl CommandPayload for TechnikAenderungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Technikänderung workflow — device/config change requests.
pub struct WimTechnikAenderungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The counterparty's answer window is open while the sent Auftrag waits for
/// its ORDRSP (`AuftragGesendet`); the MSB's own while a received one waits
/// for the MSB's (`AuftragEingegangen`).
fn window_open(label: &str, state: &TechnikAenderungState) -> bool {
    match label {
        ANTWORT_WINDOW_LABEL => matches!(state, TechnikAenderungState::AuftragGesendet(_)),
        EINGANG_WINDOW_LABEL => matches!(state, TechnikAenderungState::AuftragEingegangen(_)),
        _ => false,
    }
}

impl Workflow for WimTechnikAenderungWorkflow {
    type State = TechnikAenderungState;
    type Event = TechnikAenderungEvent;
    type Command = TechnikAenderungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL, EINGANG_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| TechnikAenderungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            TechnikAenderungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            TechnikAenderungEvent::AuftragGesendet {
                orders_pid,
                recipient,
                location_id,
                message_ref,
            } => TechnikAenderungState::AuftragGesendet(AuftragData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                location_id: location_id.clone(),
                message_ref: message_ref.clone(),
            }),
            TechnikAenderungEvent::AuftragEingegangen {
                orders_pid,
                sender,
                location_id,
                message_ref,
            } => TechnikAenderungState::AuftragEingegangen(EingangData {
                orders_pid: *orders_pid,
                sender: sender.clone(),
                location_id: location_id.clone(),
                message_ref: message_ref.clone(),
            }),
            TechnikAenderungEvent::AuftragUngueltig { reason } => {
                TechnikAenderungState::Ungueltig {
                    reason: reason.clone(),
                }
            }
            TechnikAenderungEvent::AuftragBestaetigt { ordrsp_pid, .. } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::Bestaetigt {
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::AuftragAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::Abgelehnt {
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                        reason: reason.clone(),
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::DeadlineExpired { label, .. } => match state {
                TechnikAenderungState::AuftragGesendet(_)
                | TechnikAenderungState::AuftragEingegangen(_) => {
                    TechnikAenderungState::FristAbgelaufen {
                        label: label.to_string(),
                        offen: Box::new(state),
                    }
                }
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
            TechnikAenderungCommand::SendAuftrag {
                orders_pid,
                recipient,
                location_id,
                message_ref,
                payload,
                gesendet_am,
            } => {
                if !matches!(state, TechnikAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid WiM Technikänderung ORDERS PID: {orders_pid}",
                    )));
                }
                let event = TechnikAenderungEvent::AuftragGesendet {
                    orders_pid,
                    recipient: recipient.clone(),
                    location_id: location_id.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ORDERS",
                    recipient.as_str(),
                    serde_json::json!({
                        "pid":        orders_pid.as_u32(),
                        "location":   location_id,
                        "orders_ref": message_ref.as_str(),
                        "payload":    payload,
                    }),
                )];
                // The counterparty's Antwortfrist, from the published row for
                // this ORDERS; a PID no Festlegung quantifies registers nothing.
                let frist: Vec<PendingDeadline> =
                    mako_fristen::antwort::antwort_deadline(orders_pid.as_u32(), gesendet_am)
                        .map(|due| PendingDeadline::new(ANTWORT_WINDOW_LABEL, due))
                        .into_iter()
                        .collect();
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![event],
                    outbox,
                    frist,
                ))
            }

            TechnikAenderungCommand::ReceiveAuftrag {
                orders_pid,
                sender,
                location_id,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, TechnikAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !RECEIVE_ORDERS_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not an ORDERS the MSB receives in the Technikänderung: {orders_pid}",
                    )));
                }
                let mut events = vec![TechnikAenderungEvent::AuftragEingegangen {
                    orders_pid,
                    sender,
                    location_id,
                    message_ref,
                }];
                if !validation_passed {
                    events.push(TechnikAenderungEvent::AuftragUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            TechnikAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                reason,
                message_ref,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, TechnikAenderungState::AuftragGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AuftragGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid WiM Technikänderung ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                let event = if ORDRSP_BESTAETIGUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    TechnikAenderungEvent::AuftragBestaetigt {
                        ordrsp_pid,
                        message_ref,
                    }
                } else {
                    TechnikAenderungEvent::AuftragAbgelehnt {
                        ordrsp_pid,
                        reason,
                        message_ref,
                    }
                };
                Ok(vec![event].into())
            }

            TechnikAenderungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![TechnikAenderungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auftrag(pid: u32, gesendet_am: time::OffsetDateTime) -> TechnikAenderungCommand {
        TechnikAenderungCommand::SendAuftrag {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            recipient: MarktpartnerCode::new("9900000000003"),
            location_id: None,
            message_ref: MessageRef::new("ORDERS-1"),
            payload: serde_json::json!({}),
            gesendet_am,
        }
    }

    /// A 17011 starts the MSB's 10-WT answer window (WiM Teil 1 Kap. 3.3);
    /// a 17118, which no Festlegung on hand quantifies, starts none.
    #[test]
    fn the_auftrag_registers_the_published_answer_window() {
        let sent = time::macros::datetime!(2026-10-07 10:00 UTC);
        let out = WimTechnikAenderungWorkflow::handle(
            &TechnikAenderungState::default(),
            auftrag(17_011, sent),
        )
        .expect("accepted");
        assert_eq!(out.deadlines.len(), 1);
        assert_eq!(out.deadlines[0].label, ANTWORT_WINDOW_LABEL);
        assert_eq!(
            Some(out.deadlines[0].due_at),
            mako_fristen::antwort::antwort_deadline(17_011, sent)
        );

        let out = WimTechnikAenderungWorkflow::handle(
            &TechnikAenderungState::default(),
            auftrag(17_118, sent),
        )
        .expect("accepted");
        assert!(out.deadlines.is_empty());
    }
}

#[cfg(test)]
mod eingang_tests {
    use super::*;

    fn receive(pid: u32) -> TechnikAenderungCommand {
        TechnikAenderungCommand::ReceiveAuftrag {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender: MarktpartnerCode::new("9900000000003"),
            location_id: Some("DE00056266802AO6G56M11SN51G21M24S".to_owned()),
            message_ref: MessageRef::new("ORD-17011"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn fold(
        state: TechnikAenderungState,
        out: &WorkflowOutput<TechnikAenderungEvent>,
    ) -> TechnikAenderungState {
        out.events
            .iter()
            .fold(state, WimTechnikAenderungWorkflow::apply)
    }

    /// The MSB's 10-WT window runs from the received Auftrag, and a lapse
    /// leaves the answer owed — no Festlegung gives it an outcome.
    #[test]
    fn a_received_auftrag_owes_its_answer_within_ten_werktage() {
        let eingang = time::macros::datetime!(2026-10-07 10:00 UTC);
        assert_eq!(
            eingang_antwortfrist(17_011, eingang),
            mako_fristen::antwort::antwort_deadline(17_011, eingang)
        );
        assert!(eingang_antwortfrist(17_011, eingang).is_some());
        assert_eq!(eingang_antwortfrist(17_118, eingang), None);

        let out = WimTechnikAenderungWorkflow::handle(&TechnikAenderungState::New, receive(17_011))
            .expect("accepted");
        assert_eq!(
            out.events[0].verlauf().map(|v| (v.richtung, v.pid)),
            Some((Richtung::Eingang, Some(17_011)))
        );
        let offen = fold(TechnikAenderungState::New, &out);
        assert!(matches!(
            offen,
            TechnikAenderungState::AuftragEingegangen(_)
        ));

        // The requester's label does not touch the MSB's process.
        let out = WimTechnikAenderungWorkflow::handle(
            &offen,
            TechnikAenderungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        assert!(out.events.is_empty());

        let out = WimTechnikAenderungWorkflow::handle(
            &offen,
            TechnikAenderungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: EINGANG_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        let lapsed = fold(offen, &out);
        assert!(
            matches!(&lapsed, TechnikAenderungState::FristAbgelaufen { offen, .. }
                if matches!(**offen, TechnikAenderungState::AuftragEingegangen(_))),
            "{lapsed:?}"
        );
        assert!(!lapsed.is_terminal());
    }

    /// 17118 is GPKE Teil 3's, answered by ORDRSP 19127: the MSB does not
    /// receive it as a Technikänderung.
    #[test]
    fn only_17011_is_received() {
        assert!(
            WimTechnikAenderungWorkflow::handle(&TechnikAenderungState::New, receive(17_118))
                .is_err()
        );
    }
}

#[cfg(test)]
mod verlauf_tests {
    use super::*;
    use mako_engine::types::{MessageRef, Pruefidentifikator};

    fn pid(p: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(p).expect("valid PID")
    }

    fn stated(richtung: Richtung, pid: Option<u32>, message_ref: Option<&str>) -> Verlauf {
        Verlauf {
            richtung,
            pid,
            message_ref: message_ref.map(str::to_owned),
        }
    }

    #[test]
    fn verlauf_states_the_messages_and_only_the_messages() {
        let auftrag = TechnikAenderungEvent::AuftragGesendet {
            orders_pid: pid(17_011),
            recipient: MarktpartnerCode::new("9900000000001"),
            location_id: None,
            message_ref: MessageRef::new("ORD-1"),
        };
        assert_eq!(
            auftrag.verlauf(),
            Some(stated(Richtung::Ausgang, Some(17_011), Some("ORD-1")))
        );
        let abgelehnt = TechnikAenderungEvent::AuftragAbgelehnt {
            ordrsp_pid: pid(19_006),
            reason: None,
            message_ref: MessageRef::new("RSP-1"),
        };
        assert_eq!(
            abgelehnt.verlauf(),
            Some(stated(Richtung::Eingang, Some(19_006), Some("RSP-1")))
        );
        let abgelaufen = TechnikAenderungEvent::DeadlineExpired {
            deadline_id: DeadlineId::new(),
            label: "x".into(),
        };
        assert_eq!(abgelaufen.verlauf(), None);
    }
}
