//! GeLi Gas Geschäftsdatenanfrage — Anfrage Stammdaten Gas (ORDERS 17101 /
//! 17126), answered by UTILMD 44035 / 44060 or refused by ORDRSP 19101.
//!
//! BK7-24-01-009 Anlage (GeLi Gas 3.0) Kap. 4.4 (S. 14): a Marktbeteiligter
//! asks the Netzbetreiber for the Stammdaten of a Markt- or Messlokation, and
//! on a berechtigte Anfrage the Netzbetreiber „übermittelt … dem Anfragenden
//! die angefragten Informationen unverzüglich, jedoch spätestens bis zum Ablauf
//! des 10. Werktags nach Eingang der Geschäftsdatenanfrage".
//!
//! ```text
//! LF  ──── ORDERS 17101 Anfrage Stammdaten Marktlokation ────► NB (Gas)
//!     ◄─── UTILMD 44035 Antwort auf die Geschäftsdatenanfrage ─┤  10 WT
//! MSB ──── ORDERS 17126 Anfrage Stammdaten Messlokation ─────► NB (Gas)
//!     ◄─── UTILMD 44060 Antwort auf die Geschäftsdatenanfrage ─┤  10 WT
//!     ◄─── ORDRSP 19101 Ablehnung der Anfrage Stammdaten ──────┘
//! ```
//!
//! | PID   | Message | Direction | Anwendungsübersicht 4.0 |
//! |-------|---------|-----------|-------------------------|
//! | 17101 | ORDERS  | LF → NB   | lfd. Nr. 38480 |
//! | 44035 | UTILMD  | NB → LF   | lfd. Nr. 38500 |
//! | 17126 | ORDERS  | MSB (Strom / Gas) → NB | lfd. Nr. 38560 |
//! | 44060 | UTILMD  | NB → MSB  | lfd. Nr. 38580 |
//! | 19101 | ORDRSP  | NB → LF · MSB | lfd. Nr. 38510 (`G_0049`), 38570 (`E_3004`) |
//!
//! # 19101 is shared with Strom
//!
//! GPKE's `gpke-geschaeftsdatenanfrage` refuses a 17132 with the same ORDRSP
//! 19101. This crate registers it **Sparte-qualified** (Gas) only, so the
//! router sends a 19101 addressed to a Gas party here and one addressed to a
//! Strom party to GPKE, whatever the module order.
//!
//! # One workflow, both sides
//!
//! - **Anfragender** (LF / MSB): [`GdaGasCommand::SendAnfrage`] records the
//!   ORDERS it sent; the NB's UTILMD resumes it
//!   ([`GdaGasCommand::ReceiveAntwort`]) or the 19101 does
//!   ([`GdaGasCommand::ReceiveAblehnung`]).
//! - **NB**: an inbound 17101 / 17126 opens it
//!   ([`GdaGasCommand::ReceiveAnfrage`]) with the 10-WT window;
//!   [`GdaGasCommand::SendAntwort`] or [`GdaGasCommand::SendAblehnung`]
//!   records the answer.
//!
//! Every message names the Anfrage by the ORDERS' `BGM` DE 1004: the UTILMD
//! answer in `SG6 RFF+AAV` „Nummer der Anfrage" (UTILMD AHB Gas 1.2,
//! Anwendungsübersicht Geschäftsdatenanfrage durch den LF, S. 223, and durch
//! den MSB, S. 238), the ORDRSP in `SG1 RFF+ON` (ORDRSP AHB 1.1b, S. 7). The
//! two sides are keyed apart exactly as on the Strom side: [`anfragender_key`]
//! holds the number this party issued, [`netzbetreiber_key`] the number with
//! the Anfragender's MP-ID.
//!
//! # The outbound legs
//!
//! - ORDERS 17101 `BGM+Z61` / 17126 `BGM+Z62`, the Anfrage-Nummer in DE 1004,
//!   `SG2 NAD+DP` / `LOC+172` the Lokation (ORDERS AHB 1.1b Kap. 4.1.1.2, S. 10).
//! - UTILMD 44035 / 44060 `BGM+Z14`, `SG4 STS+7` Transaktionsgrund `Z40`
//!   „Geschäftsdatenanfrage", `SG6 RFF+AAV` the Anfrage-Nummer, the Stammdaten
//!   carried whole (UTILMD AHB Gas 1.2, S. 223 / S. 238).
//! - ORDRSP 19101 `BGM+Z14`, `SG1 RFF+ON` the Anfrage-Nummer, `SG2 AJT` the
//!   code: `G_0049` to a LF (`[4] ∧ [493]`), `E_3004` to a MSB (`[14]`)
//!   (ORDRSP AHB 1.1b, S. 7).

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
pub use mako_engine::payload::Stammdaten;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    payload::UtilmdPayload,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "geli-gas-geschaeftsdatenanfrage";

/// ORDERS 17101 — Anfrage Stammdaten Marktlokation (Gas), LF → NB.
pub const ANFRAGE_LF_PID: u32 = 17_101;

/// ORDERS 17126 — Anfrage Stammdaten Messlokation (Gas), MSB → NB.
pub const ANFRAGE_MSB_PID: u32 = 17_126;

/// Both ORDERS that open a Gas Geschäftsdatenanfrage.
pub const ANFRAGE_PIDS: &[u32] = &[ANFRAGE_LF_PID, ANFRAGE_MSB_PID];

/// UTILMD answers: 44035 to the LF, 44060 to the MSB.
pub const ANTWORT_PIDS: &[u32] = &[44_035, 44_060];

/// ORDRSP 19101 — Ablehnung der Anfrage Stammdaten, NB → LF · MSB.
pub const ABLEHNUNG_PID: u32 = 19_101;

/// The PIDs registered Sparte-agnostic: both Anfragen and both answers. 19101
/// is registered for the Sparte Gas only — GPKE owns the Strom one.
pub const GDA_PIDS: &[u32] = &[17_101, 17_126, 44_035, 44_060];

/// Deadline label for the NB's 10-WT answer window on an inbound Anfrage.
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-gda-antwort";

/// `BGM` DE 1001 of the ORDRSP 19101 (ORDRSP AHB 1.1b, S. 7) and of the UTILMD
/// answers (UTILMD AHB Gas 1.2, S. 223 / S. 238).
pub const BGM_ANTWORT: &str = "Z14";

/// `SG4 STS+7` Transaktionsgrund of the UTILMD answers — `Z40`
/// „Geschäftsdatenanfrage".
pub const TRANSAKTIONSGRUND_ANTWORT: &str = "Z40";

/// Correlation key of the Anfragender's process: the Anfrage-Nummer it issued.
#[must_use]
pub fn anfragender_key(anfrage_nummer: &str) -> String {
    format!("gda-gas-anfrage:{anfrage_nummer}")
}

/// Correlation key of the NB's process: the Anfragender's MP-ID and its
/// Anfrage-Nummer, which is unique only among that sender's Anfragen.
#[must_use]
pub fn netzbetreiber_key(anfragender: &str, anfrage_nummer: &str) -> String {
    format!("gda-gas-eingang:{anfragender}:{anfrage_nummer}")
}

/// Who sent the Anfrage — read off its ORDERS PID — which decides the answer
/// PID, the ORDERS `BGM` and the Codeliste of an Ablehnung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Anfragender {
    /// A Lieferant — ORDERS 17101, answered 44035, refused from `G_0049`.
    Lf,
    /// A Messstellenbetreiber (Strom or Gas) — ORDERS 17126, answered 44060,
    /// refused from `E_3004`.
    Msb,
}

impl Anfragender {
    /// The Anfragender of an ORDERS PID.
    #[must_use]
    pub fn of_anfrage(pid: u32) -> Option<Self> {
        match pid {
            ANFRAGE_LF_PID => Some(Self::Lf),
            ANFRAGE_MSB_PID => Some(Self::Msb),
            _ => None,
        }
    }

    /// The ORDERS PID this Anfragender sends.
    #[must_use]
    pub fn anfrage_pid(self) -> u32 {
        match self {
            Self::Lf => ANFRAGE_LF_PID,
            Self::Msb => ANFRAGE_MSB_PID,
        }
    }

    /// The UTILMD answer PID — 44035 to a LF, 44060 to a MSB.
    #[must_use]
    pub fn antwort_pid(self) -> u32 {
        match self {
            Self::Lf => 44_035,
            Self::Msb => 44_060,
        }
    }

    /// `BGM` DE 1001 of the ORDERS — `Z61` (17101) / `Z62` (17126).
    #[must_use]
    pub fn bgm(self) -> &'static str {
        match self {
            Self::Lf => "Z61",
            Self::Msb => "Z62",
        }
    }

    /// `SG2 AJT` DE 1082 of the Ablehnung — `G_0049` to a LF, `E_3004` to a
    /// MSB (ORDRSP AHB 1.1b, S. 7).
    #[must_use]
    pub fn codeliste(self) -> &'static str {
        match self {
            Self::Lf => mako_pruefung::codes::CODELISTE_GDA_STAMMDATEN_LF_GAS,
            Self::Msb => mako_pruefung::codes::EBD_GDA_STAMMDATEN_MSB_GAS,
        }
    }

    /// Whether that Codeliste publishes `code` as an Ablehnung.
    #[must_use]
    pub fn admits(self, code: &str) -> bool {
        mako_pruefung::codes::lookup(self.codeliste(), code)
            .is_some_and(|c| c.ist_zustimmung() == Some(false))
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// The Anfrage both sides hold.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GdaGasAnfrage {
    /// Who asked — decides the ORDERS, the answer and the Codeliste.
    pub anfragender_rolle: Anfragender,
    /// MP-ID of the Anfragender — `NAD+MS` of the ORDERS.
    pub anfragender: MarktpartnerCode,
    /// MP-ID of the NB — `NAD+MR` of the ORDERS.
    pub netzbetreiber: MarktpartnerCode,
    /// ID of the Markt- or Messlokation (`SG2 LOC+172`).
    pub lokation_id: String,
    /// The Anfrage-Nummer — the ORDERS' `BGM` DE 1004.
    pub anfrage_nummer: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events of the Gas Geschäftsdatenanfrage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GdaGasEvent {
    /// This party (LF / MSB) sent the ORDERS.
    AnfrageGesendet {
        /// The Anfrage.
        anfrage: GdaGasAnfrage,
    },
    /// The NB received the ORDERS.
    AnfrageEingegangen {
        /// The Anfrage.
        anfrage: GdaGasAnfrage,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
    },
    /// The inbound ORDERS failed AHB validation; the APERAK answers it.
    AnfrageUngueltig {
        /// Validation errors.
        reason: String,
    },
    /// The NB sent the UTILMD answer.
    AntwortGesendet {
        /// UTILMD PID — 44035 or 44060.
        antwort_pid: Pruefidentifikator,
        /// Reference of the outbound UTILMD.
        message_ref: MessageRef,
    },
    /// The NB sent the ORDRSP 19101.
    AblehnungGesendet {
        /// `SG2 AJT` DE 4465.
        antwortcode: String,
        /// `SG2 AJT` DE 1082 — `G_0049` or `E_3004`.
        codeliste: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// The Anfragender received the NB's UTILMD answer.
    AntwortErhalten {
        /// UTILMD PID — 44035 or 44060.
        antwort_pid: Pruefidentifikator,
        /// Reference of the inbound UTILMD.
        message_ref: MessageRef,
    },
    /// The Anfragender received the NB's ORDRSP 19101.
    AblehnungErhalten {
        /// `SG2 AJT` code, when the message carried one.
        antwortcode: Option<String>,
        /// `SG2 AJT` DE 1082, as it arrived.
        codeliste: Option<String>,
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// The NB's answer window lapsed while the answer was owed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GdaGasEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "GdaGasAnfrageGesendet",
            Self::AnfrageEingegangen { .. } => "GdaGasAnfrageEingegangen",
            Self::AnfrageUngueltig { .. } => "GdaGasAnfrageUngueltig",
            Self::AntwortGesendet { .. } => "GdaGasAntwortGesendet",
            Self::AblehnungGesendet { .. } => "GdaGasAblehnungGesendet",
            Self::AntwortErhalten { .. } => "GdaGasAntwortErhalten",
            Self::AblehnungErhalten { .. } => "GdaGasAblehnungErhalten",
            Self::DeadlineExpired { .. } => "GdaGasDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        let stated = |richtung, pid: u32, message_ref: &MessageRef| {
            Some(Verlauf {
                richtung,
                pid: Some(pid),
                message_ref: Some(message_ref.as_str().to_owned()),
            })
        };
        match self {
            Self::AnfrageGesendet { anfrage } => stated(
                Richtung::Ausgang,
                anfrage.anfragender_rolle.anfrage_pid(),
                &anfrage.anfrage_nummer,
            ),
            Self::AnfrageEingegangen {
                anfrage,
                message_ref,
            } => stated(
                Richtung::Eingang,
                anfrage.anfragender_rolle.anfrage_pid(),
                message_ref,
            ),
            Self::AntwortGesendet {
                antwort_pid,
                message_ref,
            } => stated(Richtung::Ausgang, antwort_pid.as_u32(), message_ref),
            Self::AblehnungGesendet { message_ref, .. } => {
                stated(Richtung::Ausgang, ABLEHNUNG_PID, message_ref)
            }
            Self::AntwortErhalten {
                antwort_pid,
                message_ref,
            } => stated(Richtung::Eingang, antwort_pid.as_u32(), message_ref),
            Self::AblehnungErhalten { message_ref, .. } => {
                stated(Richtung::Eingang, ABLEHNUNG_PID, message_ref)
            }
            Self::AnfrageUngueltig { .. } | Self::DeadlineExpired { .. } => None,
        }
    }

    /// The ORDRSP 19101's `SG2 AJT` code and its DE 1082 Codeliste.
    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AblehnungGesendet {
                antwortcode,
                codeliste,
                ..
            } => Some(Antwortcode::new(
                antwortcode.clone(),
                Some(codeliste.clone()),
            )),
            Self::AblehnungErhalten {
                antwortcode,
                codeliste,
                ..
            } => antwortcode
                .as_ref()
                .map(|c| Antwortcode::new(c.clone(), codeliste.clone())),
            _ => None,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// State of one Gas Geschäftsdatenanfrage.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GdaGasState {
    /// No events yet.
    #[default]
    New,
    /// Anfragender side: the ORDERS is out, the NB's answer is awaited.
    Angefragt(GdaGasAnfrage),
    /// NB side: the ORDERS arrived and passed validation; the answer is owed.
    Offen(GdaGasAnfrage),
    /// The NB answered with the Stammdaten.
    Beantwortet {
        /// The Anfrage.
        anfrage: GdaGasAnfrage,
        /// UTILMD PID of the answer.
        antwort_pid: Pruefidentifikator,
    },
    /// The NB refused the Anfrage (ORDRSP 19101).
    Abgelehnt {
        /// The Anfrage.
        anfrage: GdaGasAnfrage,
        /// `SG2 AJT` code, when known.
        antwortcode: Option<String>,
    },
    /// The inbound ORDERS failed AHB validation.
    Ungueltig {
        /// Validation errors.
        reason: String,
    },
    /// The NB's 10-WT window lapsed while the answer was owed. Kap. 4.4 gives
    /// the window and no consequence of missing it: a late answer is still
    /// accepted.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GdaGasState>,
    },
}

impl GdaGasState {
    /// `true` once the Anfrage is answered, refused or invalid.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. } | Self::Abgelehnt { .. } | Self::Ungueltig { .. }
        )
    }

    /// Stable label of the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Angefragt(_) => "Angefragt",
            Self::Offen(_) => "Offen",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Ungueltig { .. } => "Ungueltig",
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
}

/// An Anfrage holds its key while it waits for its answer, on either side.
impl mako_engine::workflow::OccupiesBusinessKey for GdaGasState {
    fn occupies_business_key(&self) -> bool {
        matches!(self.offen(), Self::Angefragt(_) | Self::Offen(_))
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands of the Gas Geschäftsdatenanfrage.
#[derive(Debug, Clone)]
pub enum GdaGasCommand {
    /// Anfragender: send the ORDERS 17101 / 17126 and record it.
    SendAnfrage {
        /// The Anfrage.
        anfrage: GdaGasAnfrage,
    },
    /// NB: an inbound ORDERS 17101 / 17126, adapted at the transport boundary.
    ReceiveAnfrage {
        /// ORDERS PID read from `RFF+Z13`.
        pid: Pruefidentifikator,
        /// MP-ID of the Anfragender (`NAD+MS`).
        anfragender: MarktpartnerCode,
        /// MP-ID of the NB (`NAD+MR`).
        netzbetreiber: MarktpartnerCode,
        /// `SG2 LOC+172`.
        lokation_id: String,
        /// `BGM` DE 1004.
        anfrage_nummer: MessageRef,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Validation errors when it did not.
        validation_errors: Vec<String>,
    },
    /// NB: send the UTILMD answer (44035 / 44060, by the Anfragender) and
    /// record it.
    SendAntwort {
        /// Reference of the outbound UTILMD.
        message_ref: MessageRef,
        /// The Stammdaten the answer transmits, as the NB holds them.
        stammdaten: Stammdaten,
    },
    /// NB: send the ORDRSP 19101 and record it.
    SendAblehnung {
        /// `SG2 AJT` code from `G_0049` (to a LF) / `E_3004` (to a MSB).
        antwortcode: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Anfragender: the NB's UTILMD answer arrived.
    ReceiveAntwort {
        /// UTILMD PID read from the message.
        antwort_pid: Pruefidentifikator,
        /// Reference of the inbound UTILMD.
        message_ref: MessageRef,
    },
    /// Anfragender: the NB's ORDRSP 19101 arrived.
    ReceiveAblehnung {
        /// `SG2 AJT` code, when present.
        antwortcode: Option<String>,
        /// `SG2 AJT` DE 1082, when present.
        codeliste: Option<String>,
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GdaGasCommand {}

// ── Outbound messages ─────────────────────────────────────────────────────────

fn anfrage_outbox(anfrage: &GdaGasAnfrage) -> PendingOutbox {
    PendingOutbox::typed(
        anfrage.netzbetreiber.as_str(),
        &mako_engine::payload::OrdersPayload {
            pid: Some(anfrage.anfragender_rolle.anfrage_pid()),
            sender: Some(anfrage.anfragender.as_str().to_owned()),
            document_code: Some(anfrage.anfragender_rolle.bgm().to_owned()),
            document_id: Some(anfrage.anfrage_nummer.as_str().to_owned()),
            location: Some(anfrage.lokation_id.clone()),
            ..Default::default()
        },
    )
}

fn antwort_outbox(
    anfrage: &GdaGasAnfrage,
    message_ref: &MessageRef,
    stammdaten: Stammdaten,
) -> PendingOutbox {
    PendingOutbox::typed(
        anfrage.anfragender.as_str(),
        &UtilmdPayload {
            pid: anfrage.anfragender_rolle.antwort_pid(),
            sender: anfrage.netzbetreiber.as_str().to_owned(),
            receiver: Some(anfrage.anfragender.as_str().to_owned()),
            message_ref: Some(message_ref.as_str().to_owned()),
            document_code: Some(BGM_ANTWORT.to_owned()),
            transaktionsgrund: Some(TRANSAKTIONSGRUND_ANTWORT.to_owned()),
            anfrage_referenz: Some(anfrage.anfrage_nummer.as_str().to_owned()),
            stammdaten,
            ..Default::default()
        },
    )
}

fn ablehnung_outbox(
    anfrage: &GdaGasAnfrage,
    antwortcode: &str,
    message_ref: &MessageRef,
) -> PendingOutbox {
    PendingOutbox::typed(
        anfrage.anfragender.as_str(),
        &mako_engine::payload::OrdrspPayload {
            pid: Some(ABLEHNUNG_PID),
            sender: Some(anfrage.netzbetreiber.as_str().to_owned()),
            receiver: Some(anfrage.anfragender.as_str().to_owned()),
            message_ref: Some(message_ref.as_str().to_owned()),
            document_code: Some(BGM_ANTWORT.to_owned()),
            document_id: Some(message_ref.as_str().to_owned()),
            order_reference: Some(anfrage.anfrage_nummer.as_str().to_owned()),
            antwort_code: Some(antwortcode.to_owned()),
            antwort_codeliste: Some(anfrage.anfragender_rolle.codeliste().to_owned()),
            ..Default::default()
        },
    )
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The Geschäftsdatenanfrage Gas (ORDERS 17101 / 17126 → UTILMD 44035 / 44060
/// or ORDRSP 19101).
pub struct GeliGasGeschaeftsdatenanfrageWorkflow;

fn window_open(label: &str, state: &GdaGasState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, GdaGasState::Offen(_))
}

impl Workflow for GeliGasGeschaeftsdatenanfrageWorkflow {
    type State = GdaGasState;
    type Event = GdaGasEvent;
    type Command = GdaGasCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    /// | Label | State guard | Command | Rule |
    /// |---|---|---|---|
    /// | `"geli-gas-gda-antwort"` | `Offen` | `TimeoutExpired` | GeLi Gas 3.0 Kap. 4.4 — Ablauf des 10. WT |
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GdaGasCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        let state = match state {
            GdaGasState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GdaGasEvent::AnfrageGesendet { anfrage } => GdaGasState::Angefragt(anfrage.clone()),
            GdaGasEvent::AnfrageEingegangen { anfrage, .. } => GdaGasState::Offen(anfrage.clone()),
            GdaGasEvent::AnfrageUngueltig { reason } => GdaGasState::Ungueltig {
                reason: reason.clone(),
            },
            GdaGasEvent::AntwortGesendet { antwort_pid, .. }
            | GdaGasEvent::AntwortErhalten { antwort_pid, .. } => match state {
                GdaGasState::Offen(anfrage) | GdaGasState::Angefragt(anfrage) => {
                    GdaGasState::Beantwortet {
                        anfrage,
                        antwort_pid: *antwort_pid,
                    }
                }
                other => other,
            },
            GdaGasEvent::AblehnungGesendet { antwortcode, .. } => match state {
                GdaGasState::Offen(anfrage) => GdaGasState::Abgelehnt {
                    anfrage,
                    antwortcode: Some(antwortcode.clone()),
                },
                other => other,
            },
            GdaGasEvent::AblehnungErhalten { antwortcode, .. } => match state {
                GdaGasState::Angefragt(anfrage) => GdaGasState::Abgelehnt {
                    anfrage,
                    antwortcode: antwortcode.clone(),
                },
                other => other,
            },
            GdaGasEvent::DeadlineExpired { label, .. } => match state {
                GdaGasState::Offen(_) => GdaGasState::FristAbgelaufen {
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
        let lapsed = state;
        let state = state.offen();
        match command {
            GdaGasCommand::SendAnfrage { anfrage } => {
                if !matches!(state, GdaGasState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                let outbox = vec![anfrage_outbox(&anfrage).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![GdaGasEvent::AnfrageGesendet { anfrage }],
                    outbox,
                ))
            }
            GdaGasCommand::ReceiveAnfrage {
                pid,
                anfragender,
                netzbetreiber,
                lokation_id,
                anfrage_nummer,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GdaGasState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                let rolle = Anfragender::of_anfrage(pid.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {pid} is no Geschäftsdatenanfrage Gas (expected {ANFRAGE_PIDS:?})"
                    ))
                })?;
                let mut events = vec![GdaGasEvent::AnfrageEingegangen {
                    anfrage: GdaGasAnfrage {
                        anfragender_rolle: rolle,
                        anfragender,
                        netzbetreiber,
                        lokation_id,
                        anfrage_nummer,
                    },
                    message_ref,
                }];
                if !validation_passed {
                    events.push(GdaGasEvent::AnfrageUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
            GdaGasCommand::SendAntwort {
                message_ref,
                stammdaten,
            } => {
                let GdaGasState::Offen(anfrage) = state else {
                    return Err(WorkflowError::invalid_state("Offen", state.status_str()));
                };
                if stammdaten.lokationen.is_empty() || stammdaten.bloecke.is_empty() {
                    return Err(WorkflowError::validation(
                        "a Stammdaten-Antwort names its Lokationen (SG5 LOC) and carries its \
                         Stammdaten (SG8) — UTILMD AHB Gas 1.2, S. 223 / S. 238",
                    ));
                }
                let antwort_pid = Pruefidentifikator::new(anfrage.anfragender_rolle.antwort_pid())
                    .map_err(WorkflowError::validation)?;
                let outbox = vec![antwort_outbox(anfrage, &message_ref, stammdaten).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![GdaGasEvent::AntwortGesendet {
                        antwort_pid,
                        message_ref,
                    }],
                    outbox,
                ))
            }
            GdaGasCommand::SendAblehnung {
                antwortcode,
                message_ref,
            } => {
                let GdaGasState::Offen(anfrage) = state else {
                    return Err(WorkflowError::invalid_state("Offen", state.status_str()));
                };
                let rolle = anfrage.anfragender_rolle;
                if !rolle.admits(&antwortcode) {
                    return Err(WorkflowError::validation(format!(
                        "{antwortcode:?} is not an Ablehnungscode of {}",
                        rolle.codeliste()
                    )));
                }
                let outbox =
                    vec![ablehnung_outbox(anfrage, &antwortcode, &message_ref).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![GdaGasEvent::AblehnungGesendet {
                        antwortcode,
                        codeliste: rolle.codeliste().to_owned(),
                        message_ref,
                    }],
                    outbox,
                ))
            }
            GdaGasCommand::ReceiveAntwort {
                antwort_pid,
                message_ref,
            } => {
                let GdaGasState::Angefragt(anfrage) = state else {
                    return Err(WorkflowError::invalid_state(
                        "Angefragt",
                        state.status_str(),
                    ));
                };
                if antwort_pid.as_u32() != anfrage.anfragender_rolle.antwort_pid() {
                    return Err(WorkflowError::rejected(format!(
                        "PID {antwort_pid} does not answer a {} (expected {})",
                        anfrage.anfragender_rolle.anfrage_pid(),
                        anfrage.anfragender_rolle.antwort_pid()
                    )));
                }
                Ok(vec![GdaGasEvent::AntwortErhalten {
                    antwort_pid,
                    message_ref,
                }]
                .into())
            }
            GdaGasCommand::ReceiveAblehnung {
                antwortcode,
                codeliste,
                message_ref,
            } => {
                if !matches!(state, GdaGasState::Angefragt(_)) {
                    return Err(WorkflowError::invalid_state(
                        "Angefragt",
                        state.status_str(),
                    ));
                }
                Ok(vec![GdaGasEvent::AblehnungErhalten {
                    antwortcode,
                    codeliste,
                    message_ref,
                }]
                .into())
            }
            GdaGasCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GdaGasEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(p: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(p).expect("valid PID")
    }

    fn anfrage(rolle: Anfragender) -> GdaGasAnfrage {
        GdaGasAnfrage {
            anfragender_rolle: rolle,
            anfragender: MarktpartnerCode::new("9870012345678"),
            netzbetreiber: MarktpartnerCode::new("9807317000007"),
            lokation_id: "51238696012".into(),
            anfrage_nummer: MessageRef::new("GDA-GAS-1"),
        }
    }

    fn run(state: GdaGasState, cmd: GdaGasCommand) -> Result<GdaGasState, WorkflowError> {
        let out = GeliGasGeschaeftsdatenanfrageWorkflow::handle(&state, cmd)?;
        Ok(out
            .iter()
            .fold(state, GeliGasGeschaeftsdatenanfrageWorkflow::apply))
    }

    fn eingegangen(p: u32) -> GdaGasCommand {
        let a = anfrage(Anfragender::of_anfrage(p).unwrap_or(Anfragender::Lf));
        GdaGasCommand::ReceiveAnfrage {
            pid: pid(p),
            anfragender: a.anfragender,
            netzbetreiber: a.netzbetreiber,
            lokation_id: a.lokation_id,
            anfrage_nummer: a.anfrage_nummer,
            message_ref: MessageRef::new("UNH-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn stammdaten() -> Stammdaten {
        use mako_engine::payload::{StammdatenBlock, StammdatenLokation, StammdatenSegment};
        Stammdaten {
            lokationen: vec![StammdatenLokation {
                qualifier: "172".into(),
                id: "51238696012".into(),
            }],
            bloecke: vec![StammdatenBlock {
                seq: "Z01".into(),
                segmente: vec![StammdatenSegment {
                    tag: "RFF".into(),
                    elemente: vec![vec!["Z18".into(), "51238696012".into()]],
                }],
            }],
            ..Stammdaten::default()
        }
    }

    /// The LF's 17101 is an ORDERS `BGM+Z61`, the MSB's 17126 `BGM+Z62`, each
    /// with the Anfrage-Nummer in DE 1004.
    #[test]
    fn each_anfragender_sends_its_own_orders() {
        for (rolle, p, bgm) in [
            (Anfragender::Lf, 17_101, "Z61"),
            (Anfragender::Msb, 17_126, "Z62"),
        ] {
            let out = GeliGasGeschaeftsdatenanfrageWorkflow::handle(
                &GdaGasState::New,
                GdaGasCommand::SendAnfrage {
                    anfrage: anfrage(rolle),
                },
            )
            .unwrap();
            let e = &out.outbox[0];
            assert_eq!(e.message_type.as_ref(), "ORDERS");
            assert_eq!(e.recipient.as_ref(), "9807317000007");
            assert_eq!(e.payload["pid"], p);
            assert_eq!(e.payload["document_code"], bgm);
            assert_eq!(e.payload["document_id"], "GDA-GAS-1");
        }
    }

    /// A 17101 is answered by 44035 and a 17126 by 44060 — the other answer
    /// does not close it.
    #[test]
    fn the_anfragender_resumes_on_its_own_answer_only() {
        for (rolle, own, other) in [
            (Anfragender::Lf, 44_035, 44_060),
            (Anfragender::Msb, 44_060, 44_035),
        ] {
            let state = run(
                GdaGasState::New,
                GdaGasCommand::SendAnfrage {
                    anfrage: anfrage(rolle),
                },
            )
            .unwrap();
            assert!(
                run(
                    state.clone(),
                    GdaGasCommand::ReceiveAntwort {
                        antwort_pid: pid(other),
                        message_ref: MessageRef::new("U-1"),
                    },
                )
                .is_err()
            );
            let done = run(
                state,
                GdaGasCommand::ReceiveAntwort {
                    antwort_pid: pid(own),
                    message_ref: MessageRef::new("U-1"),
                },
            )
            .unwrap();
            assert!(matches!(done, GdaGasState::Beantwortet { .. }));
        }
    }

    #[test]
    fn the_anfragender_resumes_on_19101() {
        let state = run(
            GdaGasState::New,
            GdaGasCommand::SendAnfrage {
                anfrage: anfrage(Anfragender::Lf),
            },
        )
        .unwrap();
        let state = run(
            state,
            GdaGasCommand::ReceiveAblehnung {
                antwortcode: Some("Z15".into()),
                codeliste: Some("G_0049".into()),
                message_ref: MessageRef::new("O-1"),
            },
        )
        .unwrap();
        assert!(matches!(state, GdaGasState::Abgelehnt { .. }));
    }

    /// The NB answers with the UTILMD of the Anfragender: `BGM+Z14`,
    /// `STS+7` `Z40`, the Anfrage-Nummer in `RFF+AAV`.
    #[test]
    fn the_nb_answers_with_the_utilmd_of_the_anfragender() {
        for (p, antwort) in [(17_101, 44_035), (17_126, 44_060)] {
            let state = run(GdaGasState::New, eingegangen(p)).unwrap();
            assert!(matches!(state, GdaGasState::Offen(_)));
            let out = GeliGasGeschaeftsdatenanfrageWorkflow::handle(
                &state,
                GdaGasCommand::SendAntwort {
                    message_ref: MessageRef::new("U-1"),
                    stammdaten: stammdaten(),
                },
            )
            .unwrap();
            let pl = &out.outbox[0].payload;
            assert_eq!(pl["pid"], antwort);
            assert_eq!(pl["document_code"], "Z14");
            assert_eq!(pl["transaktionsgrund"], "Z40");
            assert_eq!(pl["anfrage_referenz"], "GDA-GAS-1");
        }
    }

    /// The Ablehnung is drawn from `G_0049` to a LF and from `E_3004` to a
    /// MSB, and a code of the other list is refused.
    #[test]
    fn the_ablehnung_names_the_codeliste_of_its_recipient() {
        for (p, code, liste, foreign) in [
            (17_101, "Z15", "G_0049", "A01"),
            (17_126, "A03", "E_3004", "Z15"),
        ] {
            let state = run(GdaGasState::New, eingegangen(p)).unwrap();
            let out = GeliGasGeschaeftsdatenanfrageWorkflow::handle(
                &state,
                GdaGasCommand::SendAblehnung {
                    antwortcode: code.into(),
                    message_ref: MessageRef::new("O-1"),
                },
            )
            .unwrap();
            assert_eq!(out.outbox[0].payload["pid"], 19_101);
            assert_eq!(out.outbox[0].payload["antwort_codeliste"], liste);
            let a = out.events[0].antwort().expect("states its code");
            assert!(
                mako_pruefung::codes::lies(liste, &a.code).is_some(),
                "{liste} {code} has its wording"
            );
            assert!(
                GeliGasGeschaeftsdatenanfrageWorkflow::handle(
                    &state,
                    GdaGasCommand::SendAblehnung {
                        antwortcode: foreign.into(),
                        message_ref: MessageRef::new("O-1"),
                    },
                )
                .is_err(),
                "{foreign} is not {liste}'s"
            );
        }
    }

    #[test]
    fn the_strom_anfrage_does_not_open_it() {
        assert!(run(GdaGasState::New, eingegangen(17_132)).is_err());
    }

    #[test]
    fn a_lapse_is_a_frist_breach_and_a_late_answer_still_closes() {
        let state = run(GdaGasState::New, eingegangen(17_101)).unwrap();
        let state = run(
            state,
            GdaGasCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(matches!(state, GdaGasState::FristAbgelaufen { .. }));
        let state = run(
            state,
            GdaGasCommand::SendAntwort {
                message_ref: MessageRef::new("U-1"),
                stammdaten: stammdaten(),
            },
        )
        .unwrap();
        assert!(state.is_terminal());
    }

    #[test]
    fn the_two_sides_are_keyed_apart() {
        assert_ne!(
            anfragender_key("X"),
            netzbetreiber_key("9870012345678", "X")
        );
    }
}
