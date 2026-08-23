//! WiM Geräteübernahme und Gerätewechsel — the two ORDERS/ORDRSP legs the
//! **abgebender** Messstellenbetreiber (MSBA) answers, in both Sparten.
//!
//! Models WiM Strom Teil 1 (Anlage 2a zu BK6-22-024) Kap. 3.1 and 3.2, and the
//! identical AWH WiM Gas 2.0 Kap. 4.1 and 4.2.
//!
//! # Two Use-Cases, one Messlokation
//!
//! The MSBN may run them in parallel, in either order, or only one of them
//! (WiM Teil 1 Kap. 2.3.2 Nr. 5/6). They concern the same devices at the same
//! Messlokation, so one process stream keyed on the MeLo holds both — but they
//! are **separate exchanges with separate Antwortcodes**, not phases of one.
//!
//! ```text
//! Geräteübernahme (Kap. 3.2 / AWH Gas 4.2)
//!   MSBN ──REQOTE 35001 Anforderung Geräteübernahmeangebot─────────────────▶ MSBA
//!   MSBN ◀─QUOTES 15001 Angebot──────────── 4 WT nach dem ÜT von Nr. 1 ───── MSBA
//!   MSBN ──ORDERS 17001 Bestellung───────── 3 WT nach dem ÜT von Nr. 2 ────▶ MSBA
//!   MSBN ◀─ORDRSP 19001 / 19002──────────── 2 WT nach dem ÜT von Nr. 3 ───── MSBA
//!
//! Gerätewechsel (Kap. 3.1 / AWH Gas 4.1)
//!   MSBN ──ORDERS 17009 Anzeige Gerätewechselabsicht───────────────────────▶ MSBA
//!   MSBN ◀─ORDRSP 19015 / 19016──────────── 2 WT *vor* dem Wechseltermin ─── MSBA
//! ```
//!
//! This workflow owns only what arrives as an ORDERS and leaves as an ORDRSP.
//! The REQOTE 35001 Anforderung and the QUOTES 15001 Angebot belong to
//! [`crate::preisanfrage`], so 17001 is the Bestellung and nothing else; ORDERS
//! **17002** „Weiterverpflichtung" is NB → MSBA with its own Frist and
//! Entscheidungsbaum — [`crate::weiterverpflichtung`].
//!
//! # 19016 is not a refusal
//!
//! The AHB names 19016 „Ablehnung Gerätewechselabsicht", but the codes say what
//! the pair actually decides: `ZB4` „Eigenausbau wird erfolgen" — the MSBA
//! removes its own devices — against `ZB5` „Kein Eigenausbau des MSBA", where
//! the MSBN does. Both agree that the Gerätewechsel happens; they divide the
//! labour. The genuine refusals are `E17` (Frist) and `Z07` (Berechtigung).
//!
//! # One PID, two Sparten
//!
//! ORDERS and ORDRSP are Sparte-neutral AHBs: 17001, 17009 and the answers
//! 19001/19002/19015/19016 carry the Strom **and** the Gas Use-Case. Nothing in
//! the message body says which — the Sparte is the Sparte of the interchange
//! recipient's MP-ID (BDEW Allgemeine Festlegungen §2.13) and it decides the
//! Entscheidungsbaum, hence the Codeliste `SG2 AJT` DE 1082 must name:
//!
//! | Leg | Strom tree | Strom DE 1082 | Gas tree | Gas DE 1082 |
//! |---|---|---|---|---|
//! | 17001 → 19001/19002 | `E_0247` | `S_0067` / `S_0068` | `E_2011` | `G_0061` / `G_0074` |
//! | 17009 → 19015/19016 | `E_0204` | `S_0065` / `S_0066` | `E_2007` | `G_0059` / `G_0060` |
//!
//! # Regulatory basis
//!
//! - **WiM Strom Teil 1 Kap. 3.1/3.2** (Anlage 2a zu BK6-22-024)
//! - **AWH WiM Gas 2.0 Kap. 4.1/4.2** (gültig ab 01.10.2026)
//! - **ORDRSP AHB 1.1b Kap. 4** — `SG2 AJT` DE 4465 (code) und DE 1082 (Codeliste)
//! - **EBD 4.3** Kap. 8.4/8.5 (Strom) und 14.4/14.5 (Gas)

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator, Sparte},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{HolidayCalendar, deadline_at_werktage};
use time::OffsetDateTime;

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-geraeteubernahme";

/// Inbound ORDERS PIDs routed to this workflow, in **both Sparten**.
///
/// | PID | AHB name | Fundstelle Strom | Fundstelle Gas |
/// |---|---|---|---|
/// | 17001 | Bestellung Geräteübernahme | Kap. 3.2.2 Nr. 3 | AWH 4.2.2 Nr. 3 |
/// | 17009 | Anzeige Gerätewechselabsicht | Kap. 3.1.2 Nr. 1 | AWH 4.1.2 Nr. 1 |
///
/// Neighbouring ORDERS PIDs that are deliberately absent: **17002**
/// (Weiterverpflichtung, NB → MSBA — [`crate::weiterverpflichtung`]), **17005**
/// (Rechnungsabwicklung MSB über LF — [`crate::rechnungsabwicklung`]) and
/// **17011** (Änderung der Technik — [`crate::technik_aenderung`]).
pub const GERAETEUBERNAHME_PIDS: &[u32] = &[17001, 17009];

/// ORDERS 17001 — „Bestellung Geräteübernahme".
///
/// The Bestellung against a standing QUOTES 15001 Angebot. It is the **only**
/// ORDERS in the Geräteübernahme: the Anforderung that precedes it is REQOTE
/// 35001 and belongs to [`crate::preisanfrage`].
pub const BESTELLUNG_PIDS: &[u32] = &[17001];

/// ORDERS 17009 — „Anzeige Gerätewechselabsicht".
pub const ANKUENDIGUNG_PIDS: &[u32] = &[17009];

/// The REQOTE that opens a Geräteübernahme, for cross-referencing only.
///
/// Owned by [`crate::preisanfrage`] — this workflow never sees it, and names it
/// only so the two legs of the Use-Case are legible from one place.
pub const ANFRAGE_PID: Pruefidentifikator = Pruefidentifikator::const_new(35001);

/// QUOTES 15001 — „Geräteübernahmeangebot", answered by [`crate::preisanfrage`].
pub const ANGEBOT_PID: Pruefidentifikator = Pruefidentifikator::const_new(15001);

/// ORDRSP 19001 — „Bestellbestätigung" (positive answer to 17001).
pub const BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19001);

/// ORDRSP 19002 — „Ablehnung der Bestellung" (negative answer to 17001).
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19002);

/// ORDRSP 19015 / 19016 — the MSBA's answer to a Gerätewechselabsicht.
///
/// **Neither is a refusal of the Gerätewechsel** — see the module docs.
pub const GERAETEWECHSELABSICHT_PIDS: (u32, u32) = (19015, 19016);

/// Werktage for the Geräteübernahmeangebot (Kap. 3.2.2 Nr. 2 / AWH 4.2.2 Nr. 2).
///
/// Owned by [`crate::preisanfrage`]; restated here because the Bestellfrist
/// below is counted from the ÜT of the Angebot.
pub const ANGEBOT_FRIST_WT: u32 = 4;

/// Werktage the MSBN has to order against a standing Angebot
/// (Kap. 3.2.2 Nr. 3 / AWH 4.2.2 Nr. 3).
pub const BESTELLUNG_FRIST_WT: u32 = 3;

/// Werktage for the Bestellbestätigung — „Unverzüglich, jedoch spätester ÜT ist
/// der 2. WT nach dem ÜT von Nr. 3" (Kap. 3.2.2 Nr. 4 / AWH 4.2.2 Nr. 4).
pub const BESTAETIGUNG_FRIST_WT: u32 = 2;

/// Werktage **before** the Gerätewechseltermin by which the MSBA must answer an
/// Anzeige Gerätewechselabsicht (Kap. 3.1.2 Nr. 2 / AWH 4.1.2 Nr. 2).
///
/// A Vorlauffrist, not an Antwortfrist: it is anchored on the Termin the
/// *message* carries, so it can already be in the past when the Anzeige
/// arrives. That is not an error in the arithmetic — it is a Vorlauffrist the
/// MSBN failed to observe, and `E17` is the code for it.
pub const GERAETEWECHSELABSICHT_ANTWORT_WT: u32 = 2;

/// Werktage after the Anzeige at the earliest at which the Gerätewechsel may
/// take place (Kap. 3.1.2 Nr. 1 / AWH 4.1.2 Nr. 1).
pub const GERAETEWECHSEL_TERMIN_FRUEHESTENS_WT: u32 = 4;

/// Deadline label for the ORDRSP answer window on a Bestellung (2 Werktage).
pub const ORDRSP_DEADLINE_LABEL: &str = "wim-geraeteubernahme-ordrsp-deadline";

/// Deadline label for the ORDRSP answer window on a Gerätewechselabsicht.
///
/// Distinct from [`ORDRSP_DEADLINE_LABEL`] because the two are anchored
/// differently — one on the arrival instant, one on a date in the payload — and
/// an operator queue that cannot tell them apart cannot state why an entry is
/// due when it is.
pub const GERAETEWECHSELABSICHT_DEADLINE_LABEL: &str = "wim-geraetewechselabsicht-ordrsp-deadline";

/// The Entscheidungsbaum that answers an inbound ORDERS, per Sparte.
///
/// Returns `None` for a PID this workflow does not answer.
#[must_use]
pub const fn ordrsp_ebd(orders_pid: u32, sparte: Sparte) -> Option<&'static str> {
    use mako_pruefung::codes as c;
    match (orders_pid, sparte) {
        (17001, Sparte::Strom) => Some(c::EBD_BESTELLUNG_GERAETEUEBERNAHME),
        (17001, Sparte::Gas) => Some(c::EBD_BESTELLUNG_GERAETEUEBERNAHME_GAS),
        (17009, Sparte::Strom) => Some(c::EBD_GERAETEWECHSELABSICHT),
        (17009, Sparte::Gas) => Some(c::EBD_GERAETEWECHSELABSICHT_GAS),
        _ => None,
    }
}

/// The ORDRSP answer PID for an inbound ORDERS and an answer cluster.
///
/// The **cluster picks the PID**, never a boolean the caller passes alongside
/// the code — a Zustimmungscode cannot ride an Ablehnungs-PID.
#[must_use]
pub const fn ordrsp_antwort_pid(orders_pid: u32, zustimmung: bool) -> Option<u32> {
    match (orders_pid, zustimmung) {
        (17001, true) => Some(19001),
        (17001, false) => Some(19002),
        (17009, true) => Some(GERAETEWECHSELABSICHT_PIDS.0),
        (17009, false) => Some(GERAETEWECHSELABSICHT_PIDS.1),
        _ => None,
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Geräteübernahme workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeraeteubernahmeEvent {
    /// An ORDERS 17001 Bestellung or 17009 Anzeige Gerätewechselabsicht arrived.
    OrdersEmpfangen {
        /// ORDERS PID (17001 or 17009).
        pid: Pruefidentifikator,
        /// MP-ID of the incoming Messstellenbetreiber (MSBN) — the sender.
        msbn: MarktpartnerCode,
        /// MP-ID of this party, the abgebender MSB (MSBA) — the receiver.
        msba: MarktpartnerCode,
        /// Messlokation the devices sit at.
        melo_id: MeLo,
        /// Gerätenummer, where the order names one.
        device_id: DeviceId,
        /// Document date from `DTM+137` (YYYYMMDD).
        document_date: String,
        /// The date the order asks for: the Übernahmezeitpunkt on a 17001, the
        /// Gerätewechseltermin on a 17009 (YYYYMMDD).
        termin: Option<String>,
        /// EDIFACT message reference, echoed in the answer's `RFF+ACW`.
        message_ref: MessageRef,
        /// Sparte of the interchange — it picks the Entscheidungsbaum.
        sparte: Sparte,
    },
    /// The ORDERS passed AHB validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// The ORDRSP answer went out.
    AntwortGesendet {
        /// 19001/19002 for a Bestellung, 19015/19016 for a Gerätewechselabsicht.
        pruefidentifikator: Pruefidentifikator,
        /// `SG2 AJT` DE 4465 — the Prüfschritt code.
        antwort_code: String,
        /// The Entscheidungsbaum the code was resolved against.
        antwort_ebd: String,
        /// `true` when the code sits in the tree's Zustimmungs-Cluster.
        zustimmung: bool,
    },
    /// The physical device transfer completed.
    Abgeschlossen {
        /// Gerätenummer confirmed at transfer.
        device_id: DeviceId,
    },
    /// The order was refused before it could be answered (AHB validation).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline fired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GeraeteubernahmeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::OrdersEmpfangen { .. } => "WimGeraeteubernahmeOrdersEmpfangen",
            Self::ValidationPassed { .. } => "WimGeraeteubernahmeValidationPassed",
            Self::AntwortGesendet { .. } => "WimGeraeteubernahmeAntwortGesendet",
            Self::Abgeschlossen { .. } => "WimGeraeteubernahmeAbgeschlossen",
            Self::Abgelehnt { .. } => "WimGeraeteubernahmeAbgelehnt",
            Self::DeadlineExpired { .. } => "WimGeraeteubernahmeDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the inbound ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeraeteubernahmeData {
    /// ORDERS PID (17001 or 17009).
    pub pid: Pruefidentifikator,
    /// MP-ID of the incoming Messstellenbetreiber (MSBN).
    pub msbn: MarktpartnerCode,
    /// MP-ID of this party, the abgebender MSB (MSBA).
    pub msba: MarktpartnerCode,
    /// Messlokation the devices sit at.
    pub melo_id: MeLo,
    /// Gerätenummer, where the order names one.
    pub device_id: DeviceId,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// Übernahmezeitpunkt (17001) or Gerätewechseltermin (17009).
    pub termin: Option<String>,
    /// EDIFACT message reference of the ORDERS.
    pub message_ref: MessageRef,
    /// Sparte of the interchange.
    pub sparte: Sparte,
}

/// State of a single WiM Geräteübernahme / Gerätewechsel process stream.
///
/// ```text
/// New → OrdersEmpfangen → ValidationPassed → Beantwortet → Abgeschlossen
///                       ↘ Abgelehnt (AHB validation)   ↘ Abgelehnt (Ablehnungscode)
///       any non-terminal → Abgelehnt (deadline expired)
/// ```
///
/// `Beantwortet` is reached by a Zustimmungscode and `Abgelehnt` by an
/// Ablehnungscode — the cluster decides, not the caller.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GeraeteubernahmeState {
    /// No events yet.
    #[default]
    New,
    /// ORDERS received; AHB validation result not yet applied.
    OrdersEmpfangen(GeraeteubernahmeData),
    /// Validation passed; the ORDRSP answer is owed.
    ValidationPassed(GeraeteubernahmeData),
    /// A Zustimmung went out; the physical work is outstanding.
    Beantwortet(GeraeteubernahmeData),
    /// Device transfer completed.
    Abgeschlossen(GeraeteubernahmeData),
    /// Refused — AHB validation, an Ablehnungscode, or a lapsed deadline.
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl GeraeteubernahmeState {
    /// Returns `true` if the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Abgeschlossen(_) | Self::Abgelehnt { .. })
    }

    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::OrdersEmpfangen(_) => "OrdersEmpfangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Beantwortet(_) => "Beantwortet",
            Self::Abgeschlossen(_) => "Abgeschlossen",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for GeraeteubernahmeState {
    fn occupies_business_key(&self) -> bool {
        !matches!(self, Self::New) && !self.is_terminal()
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Geräteübernahme workflow.
#[derive(Clone)]
pub enum GeraeteubernahmeCommand {
    /// An inbound ORDERS 17001 or 17009 accepted from the AS4 layer.
    ///
    /// Domain fields are extracted and AHB validation is performed at the
    /// transport boundary **before** this command is constructed.
    ReceiveOrders {
        /// ORDERS PID (17001 or 17009).
        pid: Pruefidentifikator,
        /// MP-ID of the message sender (MSBN).
        sender: MarktpartnerCode,
        /// MP-ID of the message receiver (this party, MSBA).
        receiver: MarktpartnerCode,
        /// Messlokation the devices sit at.
        melo_id: MeLo,
        /// Gerätenummer, where the order names one.
        device_id: DeviceId,
        /// Document date from `DTM+137`.
        document_date: String,
        /// Übernahmezeitpunkt (17001) or Gerätewechseltermin (17009), YYYYMMDD.
        termin: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation succeeded.
        validation_passed: bool,
        /// Validation issues, for the `Abgelehnt` event.
        validation_errors: Vec<String>,
        /// Sparte of the interchange, from the recipient MP-ID.
        sparte: Sparte,
        /// Arrival instant, for the answer deadline.
        received_at: OffsetDateTime,
    },
    /// Send the ORDRSP answer.
    ///
    /// `antwort_code` must be published by the tree [`ordrsp_ebd`] resolves for
    /// this ORDERS and Sparte. The **cluster** the code sits in picks 19001 or
    /// 19002 (resp. 19015 or 19016) — passing a boolean alongside the code lets
    /// the two disagree, which is how a Zustimmung ends up on an Ablehnungs-PID.
    DispatchAntwort {
        /// `SG2 AJT` DE 4465 — the Prüfschritt code.
        antwort_code: String,
        /// Free-text Begründung, where the code needs one.
        bemerkung: Option<String>,
    },
    /// Confirm that the physical device transfer is complete.
    ConfirmTransfer {
        /// Gerätenummer confirmed at transfer.
        device_id: DeviceId,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GeraeteubernahmeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Geräteübernahme / Gerätewechsel workflow — the **MSBA side**.
pub struct WimGeraeteubernahmeWorkflow;

impl Workflow for WimGeraeteubernahmeWorkflow {
    type State = GeraeteubernahmeState;
    type Event = GeraeteubernahmeEvent;
    type Command = GeraeteubernahmeCommand;

    /// Deadline compensation for the two ORDRSP answer windows.
    ///
    /// | Label | State guard | Frist |
    /// |---|---|---|
    /// | [`ORDRSP_DEADLINE_LABEL`] | any non-terminal | 2 WT nach dem ÜT der Bestellung |
    /// | [`GERAETEWECHSELABSICHT_DEADLINE_LABEL`] | any non-terminal | 2 WT vor dem Gerätewechseltermin |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        let ours = matches!(
            deadline.label(),
            ORDRSP_DEADLINE_LABEL | GERAETEWECHSELABSICHT_DEADLINE_LABEL
        );
        if ours && !state.is_terminal() {
            Some(GeraeteubernahmeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            })
        } else {
            None
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GeraeteubernahmeEvent::OrdersEmpfangen {
                pid,
                msbn,
                msba,
                melo_id,
                device_id,
                document_date,
                termin,
                message_ref,
                sparte,
            } => GeraeteubernahmeState::OrdersEmpfangen(GeraeteubernahmeData {
                pid: *pid,
                msbn: msbn.clone(),
                msba: msba.clone(),
                melo_id: melo_id.clone(),
                device_id: device_id.clone(),
                document_date: document_date.clone(),
                termin: termin.clone(),
                message_ref: message_ref.clone(),
                sparte: *sparte,
            }),
            GeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GeraeteubernahmeState::OrdersEmpfangen(data) = state {
                    GeraeteubernahmeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            GeraeteubernahmeEvent::AntwortGesendet {
                zustimmung,
                antwort_code,
                antwort_ebd,
                ..
            } => match state {
                GeraeteubernahmeState::ValidationPassed(data) => {
                    if *zustimmung {
                        GeraeteubernahmeState::Beantwortet(data)
                    } else {
                        GeraeteubernahmeState::Abgelehnt {
                            reason: format!("{antwort_ebd} {antwort_code}"),
                        }
                    }
                }
                other => other,
            },
            GeraeteubernahmeEvent::Abgeschlossen { device_id } => match state {
                GeraeteubernahmeState::Beantwortet(mut data) => {
                    data.device_id = device_id.clone();
                    GeraeteubernahmeState::Abgeschlossen(data)
                }
                other => other,
            },
            GeraeteubernahmeEvent::Abgelehnt { reason } => GeraeteubernahmeState::Abgelehnt {
                reason: reason.clone(),
            },
            GeraeteubernahmeEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    GeraeteubernahmeState::Abgelehnt {
                        reason: format!("deadline expired: {label}"),
                    }
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GeraeteubernahmeCommand::ReceiveOrders {
                pid,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                termin,
                message_ref,
                validation_passed,
                validation_errors,
                sparte,
                received_at,
            } => {
                if !matches!(state, GeraeteubernahmeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !GERAETEUBERNAHME_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not answered by this workflow (expected {GERAETEUBERNAHME_PIDS:?})",
                        pid.as_u32(),
                    )));
                }
                // Clones for the APERAK, which travels back the other way.
                let absender = sender.clone();
                let empfaenger = receiver.clone();

                let mut events = vec![GeraeteubernahmeEvent::OrdersEmpfangen {
                    pid,
                    msbn: sender,
                    msba: receiver,
                    melo_id,
                    device_id,
                    document_date,
                    termin: termin.clone(),
                    message_ref: message_ref.clone(),
                    sparte,
                }];

                if !validation_passed {
                    let reason = validation_errors.join("; ");
                    events.push(GeraeteubernahmeEvent::Abgelehnt {
                        reason: reason.clone(),
                    });
                    return Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![aperak(&empfaenger, &absender, sparte, Some(&reason))],
                        vec![aperak_deadline(sparte, pid.as_u32(), received_at)],
                    ));
                }

                events.push(GeraeteubernahmeEvent::ValidationPassed { message_ref });

                // Two clocks. The APERAK says the ORDERS could be processed —
                // 45 minutes in Strom, and in Gas only if it could *not*. The
                // business answer is the ORDRSP, and its window depends on
                // which Use-Case the PID opens.
                let mut deadlines = vec![aperak_deadline(sparte, pid.as_u32(), received_at)];
                deadlines.push(antwort_deadline(
                    pid.as_u32(),
                    termin.as_deref(),
                    received_at,
                )?);

                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    events,
                    vec![aperak(&empfaenger, &absender, sparte, None)],
                    deadlines,
                ))
            }

            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code,
                bemerkung,
            } => {
                let GeraeteubernahmeState::ValidationPassed(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                };
                let orders_pid = data.pid.as_u32();
                let tree = ordrsp_ebd(orders_pid, data.sparte).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "ORDERS {orders_pid} has no Entscheidungsbaum in Sparte {}",
                        data.sparte
                    ))
                })?;
                let code = mako_pruefung::codes::lookup(tree, &antwort_code).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "Antwortcode {antwort_code:?} is not published in {tree}"
                    ))
                })?;
                let zustimmung = code.ist_zustimmung().ok_or_else(|| {
                    WorkflowError::rejected(format!("{} sits off the agreement axis", code.code))
                })?;
                if code.braucht_bemerkung && bemerkung.is_none() {
                    return Err(WorkflowError::rejected(format!(
                        "{tree} {} ({}) requires a written Erläuterung",
                        code.code, code.bedeutung
                    )));
                }
                let antwort_pid = ordrsp_antwort_pid(orders_pid, zustimmung).ok_or_else(|| {
                    WorkflowError::rejected(format!("ORDERS {orders_pid} has no ORDRSP answer PID"))
                })?;
                // `SG2 AJT`: DE 4465 the code, DE 1082 the **Codeliste** it
                // comes from — `S_0067`/`G_0061` and friends. The EBD number is
                // the identity the code is resolved against and never the wire
                // value for this family (ORDRSP AHB 1.1b Kap. 4).
                let codeliste = code.wire_codeliste().ok_or_else(|| {
                    WorkflowError::rejected(format!("{tree} {} names no Codeliste", code.code))
                })?;

                let mut payload = serde_json::json!({
                    "pid":               antwort_pid,
                    "sender":            data.msba.as_str(),
                    "receiver":          data.msbn.as_str(),
                    "melo":              data.melo_id.as_str(),
                    "antwort_code":      code.code,
                    "antwort_codeliste": codeliste,
                    "antwort_ebd":       tree,
                    "orig_message_ref":  data.message_ref.as_str(),
                });
                if let Some(ref t) = data.termin {
                    payload["termin"] = serde_json::Value::String(t.clone());
                }
                if let Some(ref text) = bemerkung {
                    payload["bemerkung"] = serde_json::Value::String(text.clone());
                }

                Ok(WorkflowOutput::with_outbox(
                    vec![GeraeteubernahmeEvent::AntwortGesendet {
                        pruefidentifikator: Pruefidentifikator::new(antwort_pid)
                            .map_err(WorkflowError::rejected)?,
                        antwort_code: code.code.to_owned(),
                        antwort_ebd: tree.to_owned(),
                        zustimmung,
                    }],
                    vec![PendingOutbox::new("ORDRSP", data.msbn.as_str(), payload).caused_by(0)],
                ))
            }

            GeraeteubernahmeCommand::ConfirmTransfer { device_id } => {
                if !matches!(state, GeraeteubernahmeState::Beantwortet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "Beantwortet",
                        state.status_str(),
                    ));
                }
                Ok(vec![GeraeteubernahmeEvent::Abgeschlossen { device_id }].into())
            }

            GeraeteubernahmeCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeraeteubernahmeEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

/// The APERAK the receiver owes on an inbound ORDERS.
///
/// `reason` present → Verarbeitbarkeitsfehlermeldung `BGM+313`; absent →
/// Anerkennungsmeldung `BGM+312`, which Gas suppresses.
fn aperak(
    from: &MarktpartnerCode,
    to: &MarktpartnerCode,
    sparte: Sparte,
    reason: Option<&str>,
) -> PendingOutbox {
    let positive = reason.is_none();
    let mut payload = serde_json::json!({
        "sender":   from.as_str(),
        "receiver": to.as_str(),
        "pid":      29001_u32,
        "positive": positive,
        "sparte":   sparte,
    });
    // Gas has no Anerkennungsmeldung: a processable message is acknowledged by
    // the Frist lapsing in silence (APERAK AHB 1.1 §2.3). The decision is still
    // recorded for the ERP; `suppress_wire` keeps it off the wire.
    if positive && !mako_fristen::aperak_hat_anerkennungsmeldung(sparte == Sparte::Gas) {
        payload["suppress_wire"] = serde_json::Value::Bool(true);
    }
    if let Some(r) = reason {
        payload["error_code"] = serde_json::Value::String(mako_engine::erc::codes::Z29.to_owned());
        payload["reason"] = serde_json::Value::String(r.to_owned());
    }
    PendingOutbox::new("APERAK", to.as_str(), payload).caused_by(0)
}

/// The APERAK *sending* deadline, per Sparte — 45 minutes in Strom, and in Gas
/// the window the PID's Initial-/Folgeprozess classification selects.
fn aperak_deadline(sparte: Sparte, pid: u32, received_at: OffsetDateTime) -> PendingDeadline {
    match sparte {
        Sparte::Strom => PendingDeadline::new(
            mako_fristen::APERAK_STROM_WINDOW_LABEL,
            mako_fristen::aperak_strom_due_at(received_at),
        ),
        Sparte::Gas => {
            let (label, due) = mako_fristen::aperak_gas_due_at(pid, received_at);
            PendingDeadline::new(label, due)
        }
    }
}

/// The ORDRSP answer deadline, which is anchored differently per Use-Case.
///
/// * **17001 Bestellung** — an *Antwortfrist*: 2 Werktage after the arrival ÜT.
/// * **17009 Anzeige Gerätewechselabsicht** — a *Vorlauffrist*: the 2. Werktag
///   **before** the Gerätewechseltermin the message carries. Conflating the two
///   is how an answer that is already overdue on arrival looks on schedule.
///
/// A 17009 without a Termin cannot be scheduled at all; that is a rejected
/// order, not a default window.
fn antwort_deadline(
    orders_pid: u32,
    termin: Option<&str>,
    received_at: OffsetDateTime,
) -> Result<PendingDeadline, WorkflowError> {
    match orders_pid {
        17001 => Ok(PendingDeadline::new(
            ORDRSP_DEADLINE_LABEL,
            deadline_at_werktage(
                received_at,
                BESTAETIGUNG_FRIST_WT,
                HolidayCalendar::BdewMaKo,
            ),
        )),
        17009 => {
            let Some(datum) = termin.and_then(parse_yyyymmdd) else {
                return Err(WorkflowError::rejected(
                    "eine Anzeige der Gerätewechselabsicht muss den Gerätewechseltermin nennen — \
                     die Antwortfrist ist der 2. WT davor (WiM Teil 1 Kap. 3.1.2 Nr. 2)"
                        .to_owned(),
                ));
            };
            let due = mako_fristen::sub_werktage(
                datum,
                GERAETEWECHSELABSICHT_ANTWORT_WT,
                HolidayCalendar::BdewMaKo,
            );
            Ok(PendingDeadline::new(
                GERAETEWECHSELABSICHT_DEADLINE_LABEL,
                mako_fristen::berlin_at(
                    due,
                    time::Time::from_hms(17, 0, 0).expect("17:00 is a valid time"),
                ),
            ))
        }
        other => Err(WorkflowError::rejected(format!(
            "ORDERS {other} has no ORDRSP answer window in this workflow"
        ))),
    }
}

/// `YYYYMMDD` → a calendar date; `None` on anything else.
fn parse_yyyymmdd(raw: &str) -> Option<time::Date> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 8 {
        return None;
    }
    let year: i32 = digits[0..4].parse().ok()?;
    let month = time::Month::try_from(digits[4..6].parse::<u8>().ok()?).ok()?;
    let day: u8 = digits[6..8].parse().ok()?;
    time::Date::from_calendar_date(year, month, day).ok()
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Geräteübernahme process stream.
#[derive(Debug)]
pub enum GeraeteubernahmeRecord {
    /// No `OrdersEmpfangen` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `OrdersEmpfangen` applied; process fields available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Messlokation the devices sit at.
        melo_id: MeLo,
        /// MP-ID of the incoming Messstellenbetreiber.
        msbn: MarktpartnerCode,
        /// MP-ID of this party (the abgebender MSB).
        msba: MarktpartnerCode,
        /// Gerätenummer (updated on `Abgeschlossen`).
        device_id: DeviceId,
        /// ORDERS PID that opened the process.
        pid: Pruefidentifikator,
        /// Sparte of the interchange.
        sparte: Sparte,
        /// Total events applied.
        event_count: usize,
    },
}

impl GeraeteubernahmeRecord {
    /// Current lifecycle status label.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied to this stream.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }

    /// Domain data for this record, or `None` while it is still `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<GeraeteubernahmeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id,
                msbn,
                msba,
                device_id,
                pid,
                sparte,
                ..
            } => Some(GeraeteubernahmeRecordData {
                melo_id,
                msbn,
                msba,
                device_id,
                pid,
                sparte: *sparte,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`GeraeteubernahmeRecord`].
#[derive(Debug, Clone, Copy)]
pub struct GeraeteubernahmeRecordData<'a> {
    /// Messlokation the devices sit at.
    pub melo_id: &'a MeLo,
    /// MP-ID of the incoming Messstellenbetreiber.
    pub msbn: &'a MarktpartnerCode,
    /// MP-ID of this party (the abgebender MSB).
    pub msba: &'a MarktpartnerCode,
    /// Gerätenummer.
    pub device_id: &'a DeviceId,
    /// ORDERS PID that opened the process.
    pub pid: &'a Pruefidentifikator,
    /// Sparte of the interchange.
    pub sparte: Sparte,
}

impl Default for GeraeteubernahmeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model tracking WiM Geräteübernahme streams.
#[derive(Debug, Default)]
pub struct GeraeteubernahmeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, GeraeteubernahmeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for GeraeteubernahmeProjection {
    fn name(&self) -> &'static str {
        "GeraeteubernahmeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<GeraeteubernahmeEvent>() else {
            return;
        };

        match record {
            GeraeteubernahmeRecord::New { event_count }
            | GeraeteubernahmeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            GeraeteubernahmeEvent::OrdersEmpfangen {
                pid,
                msbn,
                msba,
                melo_id,
                device_id,
                sparte,
                ..
            } => {
                let count = record.event_count();
                *record = GeraeteubernahmeRecord::Active {
                    status: "OrdersEmpfangen",
                    pid,
                    msbn,
                    msba,
                    melo_id,
                    device_id,
                    sparte,
                    event_count: count,
                };
            }
            GeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            GeraeteubernahmeEvent::AntwortGesendet { zustimmung, .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = if zustimmung {
                        "Beantwortet"
                    } else {
                        "Abgelehnt"
                    };
                }
            }
            GeraeteubernahmeEvent::Abgeschlossen { device_id } => {
                if let GeraeteubernahmeRecord::Active {
                    status,
                    device_id: d,
                    ..
                } = record
                {
                    *status = "Abgeschlossen";
                    *d = device_id;
                }
            }
            GeraeteubernahmeEvent::Abgelehnt { .. }
            | GeraeteubernahmeEvent::DeadlineExpired { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "Abgelehnt";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MELO: &str = "DE0000000001234567890000000000001";

    fn orders(pid: u32, sparte: Sparte, termin: Option<&str>) -> GeraeteubernahmeCommand {
        GeraeteubernahmeCommand::ReceiveOrders {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            melo_id: MeLo::new(MELO),
            device_id: DeviceId::new("1ESY1161234567"),
            document_date: "20260302".to_owned(),
            termin: termin.map(str::to_owned),
            message_ref: MessageRef::new("ORD-1"),
            validation_passed: true,
            validation_errors: vec![],
            sparte,
            received_at: time::macros::datetime!(2026-03-02 09:00 UTC),
        }
    }

    fn validated(pid: u32, sparte: Sparte, termin: Option<&str>) -> GeraeteubernahmeState {
        let s = GeraeteubernahmeState::default();
        let out = WimGeraeteubernahmeWorkflow::handle(&s, orders(pid, sparte, termin))
            .expect("valid ORDERS");
        out.events
            .iter()
            .fold(s, WimGeraeteubernahmeWorkflow::apply)
    }

    /// The Bestellung is answered with an ORDRSP that carries the code **and**
    /// the Codeliste it comes from.
    #[test]
    fn a_bestellung_is_answered_with_an_ordrsp() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Strom, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "Z13".to_owned(),
                bemerkung: None,
            },
        )
        .expect("Z13");
        assert_eq!(&*out.outbox[0].message_type, "ORDRSP");
        assert_eq!(out.outbox[0].payload["pid"], 19_001);
        assert_eq!(out.outbox[0].payload["antwort_code"], "Z13");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "S_0067");
        assert_eq!(out.outbox[0].payload["antwort_ebd"], "E_0247");
    }

    /// The cluster picks the PID. `Z32` is an Ablehnung, so it rides 19002 —
    /// deriving the PID from a boolean the caller passes lets the two disagree.
    #[test]
    fn the_cluster_picks_the_answer_pid() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Strom, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "Z32".to_owned(),
                bemerkung: None,
            },
        )
        .expect("Z32");
        assert_eq!(out.outbox[0].payload["pid"], 19_002);
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "S_0068");
    }

    /// One Prüfidentifikator, two Sparten, two Codelisten — the recipient MP-ID
    /// is the only thing that says which.
    #[test]
    fn the_sparte_picks_the_tree_on_a_shared_pid() {
        for (sparte, ebd, codeliste) in [
            (Sparte::Strom, "E_0247", "S_0067"),
            (Sparte::Gas, "E_2011", "G_0061"),
        ] {
            let out = WimGeraeteubernahmeWorkflow::handle(
                &validated(17001, sparte, Some("20260401")),
                GeraeteubernahmeCommand::DispatchAntwort {
                    antwort_code: "Z13".to_owned(),
                    bemerkung: None,
                },
            )
            .expect("Z13");
            assert_eq!(out.outbox[0].payload["antwort_ebd"], ebd, "{sparte}");
            assert_eq!(out.outbox[0].payload["antwort_codeliste"], codeliste);
        }
    }

    /// `ZB4` and `ZB5` divide the labour over who removes the old devices;
    /// neither refuses the Gerätewechsel. 19016 is therefore not a rejection —
    /// but the state machine still has to record which of the two was sent.
    #[test]
    fn the_geraetewechselabsicht_answer_is_a_division_of_labour() {
        for (code, pid) in [("ZB4", 19_015), ("ZB5", 19_016)] {
            let out = WimGeraeteubernahmeWorkflow::handle(
                &validated(17009, Sparte::Strom, Some("20260401")),
                GeraeteubernahmeCommand::DispatchAntwort {
                    antwort_code: code.to_owned(),
                    bemerkung: None,
                },
            )
            .unwrap_or_else(|e| panic!("{code}: {e}"));
            assert_eq!(out.outbox[0].payload["pid"], pid);
        }
    }

    /// The answer to a Gerätewechselabsicht is a **Vorlauffrist** — the 2. WT
    /// before the Termin the message carries — not two Werktage after arrival.
    #[test]
    fn the_geraetewechselabsicht_window_is_anchored_on_the_termin() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17009, Sparte::Strom, Some("20260401")),
        )
        .expect("valid 17009");
        let dl = out
            .deadlines
            .iter()
            .find(|d| &*d.label == GERAETEWECHSELABSICHT_DEADLINE_LABEL)
            .expect("the Gerätewechselabsicht window");
        // 2026-04-01 is a Wednesday; two Werktage before is Monday 2026-03-30.
        assert_eq!(dl.due_at.date(), time::macros::date!(2026 - 03 - 30));
        // …and it is nowhere near „2 Werktage after arrival" on 2026-03-02.
        assert!(dl.due_at > time::macros::datetime!(2026-03-04 00:00 UTC));
    }

    /// A 17009 that names no Termin has no answer window at all. Defaulting to
    /// „two Werktage from now" would report a Frist the Festlegung does not
    /// state, on a message that is already malformed.
    #[test]
    fn a_geraetewechselabsicht_without_a_termin_is_refused() {
        let err = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17009, Sparte::Strom, None),
        )
        .expect_err("no Termin");
        assert!(err.to_string().contains("Gerätewechseltermin"), "{err}");
    }

    /// Gas has no Anerkennungsmeldung: a processable Gas ORDERS is acknowledged
    /// by the APERAK Frist lapsing in silence (APERAK AHB 1.1 §2.3). The
    /// decision is still recorded; `suppress_wire` keeps it off the wire.
    #[test]
    fn gas_suppresses_the_positive_aperak() {
        let strom = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17001, Sparte::Strom, Some("20260401")),
        )
        .expect("valid");
        assert_eq!(strom.outbox[0].payload["positive"], true);
        assert!(strom.outbox[0].payload.get("suppress_wire").is_none());

        let gas = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17001, Sparte::Gas, Some("20260401")),
        )
        .expect("valid");
        assert_eq!(gas.outbox[0].payload["suppress_wire"], true);
    }

    /// A code from another tree never reaches the wire. `ZB4` is `E_0204`, not
    /// `E_0247` — and `E_0247` publishes no `ZB4`.
    #[test]
    fn a_foreign_code_is_refused() {
        let err = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Strom, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "ZB4".to_owned(),
                bemerkung: None,
            },
        )
        .expect_err("ZB4 is not an E_0247 code");
        assert!(err.to_string().contains("E_0247"), "{err}");
    }
}
