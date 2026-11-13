//! WiM Rechnungsabwicklung des Messstellenbetriebes über den LF — the ORDERS
//! half of the arrangement the REQOTE/QUOTES exchange only negotiates.
//!
//! Under this arrangement the LF collects the MSB's Messentgelt from the
//! Letztverbraucher **in the MSB's name and for the MSB's account** (a
//! durchlaufender Posten, § 10 Abs. 1 Satz 4 UStG — the exempt position the
//! document gate's specimen carries). The quote phase (REQOTE 35002 → QUOTES
//! 15002) is [`crate::preisanfrage`]'s; this workflow owns what happens after:
//!
//! | PID | Message | Meaning | Direction |
//! |---|---|---|---|
//! | 17005 | ORDERS | Bestellung Rechnungsabwicklung — the LF accepts the quote | LF → MSB |
//! | 21032 | IFTSTA | Antwort auf das Angebot — the LF **refuses** it (`E_0205`/`E_0208`) | LF → MSB |
//! | 17006 | ORDERS | Beendigung Rechnungsabwicklung | **either** direction |
//! | 19009 | ORDRSP | Bestätigung der Beendigung | answered by the 17006 receiver |
//! | 19010 | ORDRSP | Ablehnung der Beendigung | answered by the 17006 receiver |
//!
//! Verified against the BDEW PID overview 4.0 (sheet *Prüf-ID Prozessschritt*)
//! and the AWH Aktivitätsdiagramme WiM V1.3 Kap. 2.8–2.11:
//!
//! - **17005 has no ORDRSP answer.** It is itself the answer ("Antwort auf das
//!   Angebot", Prozessschritt 2/3 of the Angebot/Anfrage sequences) — receiving
//!   it *is* the arrangement taking effect, so the process records it and
//!   completes. Nothing further travels on the wire.
//! - **…and it is only the positive half of that Prozessschritt.** The same
//!   step is answered by ORDERS 17005 *or* by **IFTSTA 21032** „Antwort auf das
//!   Angebot", and it is 21032 that carries the refusal and its
//!   Entscheidungsbaum: `E_0205` when the MSB offered (lfd. Nr. 30920/30930),
//!   `E_0208` when the LF asked first (31010/31020). A deployment that routes
//!   only 17005 records every acceptance and can never receive a „nein" — the
//!   Angebot then sits open until the operator notices.
//! - **17006 flows both ways.** The MSB may end the arrangement (AD Kap. 2.9, EBD
//!   `E_0206`) and so may the LF (AD Kap. 2.11, EBD `E_0209`); whichever side
//!   *receives* the Beendigung answers with ORDRSP 19009/19010. The answer is
//!   a decision, not an echo — it arrives through the ERP command API, exactly
//!   like a Sperrung confirmation.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-rechnungsabwicklung";

/// Inbound ORDERS PIDs. 17005 records the arrangement; 17006 asks to end it.
pub const RECHNUNGSABWICKLUNG_ORDERS_PIDS: &[u32] = &[17005, 17006];

/// Inbound ORDRSP PIDs — the counterparty's answer to a Beendigung mako sent.
pub const RECHNUNGSABWICKLUNG_ORDRSP_PIDS: &[u32] = &[19009, 19010];

/// IFTSTA 21032 „Antwort auf das Angebot" — the LF's **refusal** of the
/// Angebot, and the only carrier its Entscheidungsbaum has.
///
/// The acceptance is ORDERS 17005 and carries no code; this is the other half
/// of the same Prozessschritt. Which tree the code belongs to follows the
/// sequence, not the PID — see [`angebot_ablehnung_ebd`].
pub const RECHNUNGSABWICKLUNG_ABLEHNUNG_PID: u32 = 21032;

/// Which sequence a Rechnungsabwicklung Angebot came out of.
///
/// It decides the Entscheidungsbaum of an inbound IFTSTA 21032, and the PID
/// cannot: both sequences end in the same message. `A02` is „ausschließlich
/// kME" in `E_0205` and „kein Preisblatt vorhanden" in `E_0208`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AngebotHerkunft {
    /// The MSB offered unprompted — WiM Teil 1 Kap. 3.6.3.2, `E_0205`.
    AngebotDurchMsb,
    /// The LF asked first with REQOTE 35002 — Kap. 3.6.3.4, `E_0208`.
    AnfrageDurchLf,
}

/// The Entscheidungsbaum an inbound IFTSTA 21032 resolves against.
///
/// **The two alphabets overlap and disagree.** `E_0205` publishes `A01`–`A06`
/// and `E_0208` publishes `A01`–`A03`, and the three they share mean different
/// things — `A01` is „Kein gültiger Vertrag zwischen MSB und LF" in the first
/// and „Vertragsverhältnis lässt das Beginndatum nicht zu" in the second. So a
/// code cannot be validated into the right tree: the wrong tree accepts it and
/// records the wrong reason. The Herkunft has to be *known*, which is why
/// [`RechnungsabwicklungCommand::ReceiveAngebotAblehnung`] takes an `Option`
/// and an unknown one records the code without claiming a meaning.
#[must_use]
pub const fn angebot_ablehnung_ebd(herkunft: AngebotHerkunft) -> &'static str {
    match herkunft {
        AngebotHerkunft::AngebotDurchMsb => mako_pruefung::codes::EBD_RECHNUNGSABWICKLUNG_ANGEBOT,
        AngebotHerkunft::AnfrageDurchLf => {
            mako_pruefung::codes::EBD_RECHNUNGSABWICKLUNG_ANFRAGE_ANTWORT
        }
    }
}

/// Which side ended the arrangement — it picks the Entscheidungsbaum the
/// receiving side answers out of: `E_0206` when the MSB ended it (the LF
/// checks), `E_0209` when the LF did (the MSB checks). ORDRSP AHB 1.1b Kap.
/// 4.14 (S. 58) ties DE 1082 to the sender's role, `[9]`/`[10]`; the ORDERS
/// 17006 names neither, so the answering side states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BeendigungDurch {
    /// The MSB ended it; the LF answers out of `E_0206`.
    Msb,
    /// The LF ended it; the MSB answers out of `E_0209`.
    Lf,
}

impl BeendigungDurch {
    /// The Entscheidungsbaum the answer's code comes from.
    #[must_use]
    pub const fn ebd(self) -> &'static str {
        match self {
            Self::Msb => mako_pruefung::codes::EBD_RECHNUNGSABWICKLUNG_BEENDIGUNG_MSB,
            Self::Lf => mako_pruefung::codes::EBD_RECHNUNGSABWICKLUNG_BEENDIGUNG_LF,
        }
    }
}

/// ORDRSP PID for a Zustimmung / an Ablehnung to a received Beendigung.
#[must_use]
pub fn antwort_pid(zustimmung: bool) -> u32 {
    if zustimmung { 19009 } else { 19010 }
}

/// Deadline label for the Beendigung answer window —
/// [`mako_fristen::antwort::RECHNUNGSABWICKLUNG_WERKTAGE`] (8 Werktage, WiM
/// Strom Teil 1 Kap. 3.6.3.5.2 / 3.6.3.7.2 Nr. 2). The decision itself is EBD
/// `E_0206` (MSB beendet) or `E_0209` (LF beendet).
///
/// 17005 gets no deadline: it is the answer to the Angebot, and nothing
/// answers it in turn.
pub const RECHNUNGSABWICKLUNG_DEADLINE_LABEL: &str = "wim-rechnungsabwicklung-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the Rechnungsabwicklung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum RechnungsabwicklungEvent {
    /// ORDERS 17005 received — the LF accepted the quote; the arrangement is
    /// in force. Terminal on the wire: nothing answers a Bestellung.
    BestellungErhalten {
        /// GLN of the ordering LF.
        sender: MarktpartnerCode,
        /// GLN of the MSB whose invoicing the LF now runs.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// IFTSTA 21032 received — the LF **refused** the Angebot.
    ///
    /// The other half of the Prozessschritt `BestellungErhalten` records. Also
    /// terminal on the wire: an Ablehnung is not answered.
    AngebotAbgelehnt {
        /// GLN of the refusing LF.
        sender: MarktpartnerCode,
        /// GLN of the MSB whose Angebot was refused.
        receiver: MarktpartnerCode,
        /// `E_0205` or `E_0208` — which one follows the sequence, not the PID.
        ///
        /// `None` when the Herkunft of the Angebot was not established. The
        /// code is still recorded; what is not recorded is a meaning for it,
        /// because the two trees overlap on `A01`–`A03` with different wording.
        ebd: Option<String>,
        /// The Antwortcode the LF stated, where the IFTSTA carried one.
        antwort_code: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// ORDERS 17006 received — the counterparty wants the arrangement ended.
    BeendigungErhalten {
        /// GLN of the initiating side (MSB *or* LF — 17006 flows both ways).
        sender: MarktpartnerCode,
        /// GLN of the receiving side, which owes the ORDRSP.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `BGM` DE 1004 of the ORDERS — the answer's `SG1 RFF+ON`.
        #[serde(default)]
        dokumentennummer: String,
        /// `DTM+203` of the ORDERS, the German day (`YYYYMMDD`) the
        /// arrangement ends — the answer restates it.
        #[serde(default)]
        ausfuehrungsdatum: Option<String>,
    },
    /// ORDERS 17006 dispatched — mako initiated the Beendigung and now awaits
    /// the counterparty's ORDRSP.
    BeendigungGesendet {
        /// EDIFACT message reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19009/19010 dispatched — mako answered a received Beendigung.
    AntwortGesendet {
        /// 19009 (Bestätigung) or 19010 (Ablehnung).
        response_pid: Pruefidentifikator,
        /// EDIFACT message reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// ORDRSP 19009/19010 received — the counterparty answered mako's
    /// Beendigung.
    AntwortErhalten {
        /// 19009 (Bestätigung) or 19010 (Ablehnung).
        response_pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `SG2 AJT` — the code and the Codeliste the answer names.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<Antwortcode>,
    },
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for RechnungsabwicklungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::BestellungErhalten { .. } => "RechnungsabwicklungBestellungErhalten",
            Self::AngebotAbgelehnt { .. } => "RechnungsabwicklungAngebotAbgelehnt",
            Self::BeendigungErhalten { .. } => "RechnungsabwicklungBeendigungErhalten",
            Self::BeendigungGesendet { .. } => "RechnungsabwicklungBeendigungGesendet",
            Self::AntwortGesendet { .. } => "RechnungsabwicklungAntwortGesendet",
            Self::AntwortErhalten { .. } => "RechnungsabwicklungAntwortErhalten",
            Self::Rejected { .. } => "RechnungsabwicklungRejected",
            Self::DeadlineExpired { .. } => "RechnungsabwicklungDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        let (richtung, pid, message_ref) = match self {
            Self::BestellungErhalten { message_ref, .. }
            | Self::AngebotAbgelehnt { message_ref, .. }
            | Self::BeendigungErhalten { message_ref, .. } => {
                (Richtung::Eingang, None, message_ref)
            }
            Self::BeendigungGesendet { message_ref } => (Richtung::Ausgang, None, message_ref),
            Self::AntwortGesendet {
                response_pid,
                message_ref,
            } => (Richtung::Ausgang, Some(response_pid), message_ref),
            Self::AntwortErhalten {
                response_pid,
                message_ref,
                ..
            } => (Richtung::Eingang, Some(response_pid), message_ref),
            Self::Rejected { .. } | Self::DeadlineExpired { .. } => return None,
        };
        Some(Verlauf {
            richtung,
            pid: pid.map(|p| p.as_u32()),
            message_ref: Some(message_ref.as_str().to_owned()),
        })
    }

    /// The LF's Ablehnung of the Angebot states its code with the tree its
    /// Herkunft names — `None` for the tree where the Herkunft was not
    /// established, because `E_0205` and `E_0208` word `A01`–`A03` differently.
    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AngebotAbgelehnt {
                antwort_code: Some(code),
                ebd,
                ..
            } => Some(Antwortcode::new(code.clone(), ebd.clone())),
            Self::AntwortErhalten { antwort, .. } => antwort.clone(),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Parties of the exchange, captured at first receipt/send.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RechnungsabwicklungData {
    /// GLN of the initiating side.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving side.
    pub receiver: MarktpartnerCode,
    /// Message reference of the message that opened the process — for a
    /// received Beendigung this is what the ORDRSP echoes in `RFF+ON`, so the
    /// counterparty can correlate the answer.
    pub message_ref: MessageRef,
    /// `BGM` DE 1004 of a received ORDERS 17006 — the answer's `SG1 RFF+ON`
    /// (ORDRSP AHB 1.1b Kap. 4.14, `[520]`).
    #[serde(default)]
    pub dokumentennummer: String,
    /// `DTM+203` of a received ORDERS 17006, `YYYYMMDD`.
    #[serde(default)]
    pub ausfuehrungsdatum: Option<String>,
}

/// State of a Rechnungsabwicklung process.
///
/// # Lifecycle
///
/// ```text
/// New → Bestellt                                       (17005: record, done)
/// New → BeendigungEingegangen → Beendet{..}            (17006 in, ORDRSP out)
/// New → BeendigungAngefragt   → Beendet{..}            (17006 out, ORDRSP in)
///     ↘ Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum RechnungsabwicklungState {
    /// No events yet.
    #[default]
    New,
    /// ORDERS 17005 recorded — the arrangement is in force. Terminal.
    Bestellt(RechnungsabwicklungData),
    /// IFTSTA 21032 recorded — the LF refused the Angebot. Terminal: no
    /// arrangement comes into force and nothing answers an Ablehnung.
    Abgelehnt {
        /// `E_0205` or `E_0208`, or `None` when the Herkunft was unknown — see
        /// [`angebot_ablehnung_ebd`].
        ebd: Option<String>,
        /// The code the LF stated, where it carried one.
        antwort_code: Option<String>,
    },
    /// ORDERS 17006 received; mako owes the ORDRSP decision.
    BeendigungEingegangen(RechnungsabwicklungData),
    /// ORDERS 17006 sent; awaiting the counterparty's ORDRSP.
    BeendigungAngefragt,
    /// The Beendigung was answered. Terminal.
    Beendet {
        /// `true` when the answer was 19009 (Bestätigung) — the arrangement
        /// ends; `false` for 19010 (Ablehnung) — it continues.
        zugestimmt: bool,
    },
    /// Process rejected.
    Rejected {
        /// Reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// WiM Teil 1 gives the answer window on the Beendigung der Rechnungsabwicklung and no consequence of missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<RechnungsabwicklungState>,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for RechnungsabwicklungState {
    /// A MaLo is occupied only while a Beendigung is unanswered — a recorded
    /// Bestellung, a settled Beendigung and a rejection are all terminal and
    /// must not block the next process on the same MaLo.
    fn occupies_business_key(&self) -> bool {
        matches!(
            self.offen(),
            Self::BeendigungEingegangen(_) | Self::BeendigungAngefragt
        )
    }
}

impl RechnungsabwicklungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Bestellt(_) => "Bestellt",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::BeendigungEingegangen(_) => "BeendigungEingegangen",
            Self::BeendigungAngefragt => "BeendigungAngefragt",
            Self::Beendet { .. } => "Beendet",
            Self::Rejected { .. } => "Rejected",
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

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the Rechnungsabwicklung workflow.
#[derive(Clone)]
pub enum RechnungsabwicklungCommand {
    /// Inbound ORDERS 17005 or 17006.
    ReceiveOrders {
        /// 17005 (Bestellung) or 17006 (Beendigung).
        pid: Pruefidentifikator,
        /// GLN of the sender.
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `BGM` DE 1004.
        dokumentennummer: String,
        /// `DTM+203` Ausführungsdatum, the German day `YYYYMMDD` — 17006 only.
        ausfuehrungsdatum: Option<String>,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Validation errors.
        validation_errors: Vec<String>,
    },
    /// Inbound IFTSTA 21032 — the LF refused the Angebot.
    ///
    /// `herkunft` is not derivable from the message: both sequences end in the
    /// same PID and only the Vorgang the Angebot opened says which tree the
    /// code belongs to.
    ReceiveAngebotAblehnung {
        /// GLN of the refusing LF.
        sender: MarktpartnerCode,
        /// GLN of this party.
        receiver: MarktpartnerCode,
        /// Which sequence the Angebot came out of — it picks `E_0205`/`E_0208`.
        ///
        /// `None` where the caller could not establish it. The refusal is then
        /// recorded verbatim and left for an operator, which is the only honest
        /// outcome: the two trees share `A01`–`A03` with different meanings, so
        /// guessing does not fail closed — it records the wrong reason.
        herkunft: Option<AngebotHerkunft>,
        /// `SG4 STS` DE 9013, where the IFTSTA carried one.
        antwort_code: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Initiate a Beendigung (mako's side, via the ERP command API).
    SendBeendigung {
        /// This party's MP-ID — `SG2 NAD+MS`.
        sender: MarktpartnerCode,
        /// GLN of the counterparty (the other side of the arrangement).
        counterparty: MarktpartnerCode,
        /// MaLo the arrangement concerns — the ORDERS carries it in `LOC`.
        location_id: String,
        /// EDIFACT message reference of the outbound ORDERS 17006; also its
        /// `BGM` DE 1004, which the answer's `SG1 RFF+ON` echoes.
        message_ref: MessageRef,
        /// Which side ends it — this one.
        beendet_durch: BeendigungDurch,
        /// `DTM+203` — the day the arrangement ends, a German calendar day.
        ausfuehrungsdatum: String,
        /// `SG29 IMD` Grund der Anforderung — `Z61` Direkter Vertrag MSB – AN,
        /// `Z62` – ANN, `Z63` Änderung im Lokationsbündel; Muss from the MSB
        /// (`[15]`), absent from the LF.
        grund: Option<String>,
    },
    /// Answer a received Beendigung (decision via the ERP command API —
    /// EBD `E_0206`/`E_0209` is the counterparty's check, the decision here
    /// is the operator's).
    SendAntwort {
        /// `true` → ORDRSP 19009 (Bestätigung), `false` → 19010 (Ablehnung).
        zustimmung: bool,
        /// `SG2 AJT` DE 4465 — a code of [`BeendigungDurch::ebd`] in the
        /// cluster `zustimmung` names.
        antwort_code: String,
        /// Which side ended the arrangement.
        beendet_durch: BeendigungDurch,
        /// EDIFACT message reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19009/19010 answering mako's Beendigung.
    ReceiveAntwort {
        /// 19009 or 19010.
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `SG2 AJT` — the code and the Codeliste the answer names.
        antwort: Option<Antwortcode>,
    },
    /// Deadline expired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for RechnungsabwicklungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Rechnungsabwicklung workflow (ORDERS 17005/17006, ORDRSP 19009/19010).
pub struct WimRechnungsabwicklungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while a Beendigung is unanswered — owed by
/// this party (`BeendigungEingegangen`) or to it (`BeendigungAngefragt`).
fn window_open(label: &str, state: &RechnungsabwicklungState) -> bool {
    label == RECHNUNGSABWICKLUNG_DEADLINE_LABEL
        && matches!(
            state,
            RechnungsabwicklungState::BeendigungEingegangen(_)
                | RechnungsabwicklungState::BeendigungAngefragt
        )
}

impl Workflow for WimRechnungsabwicklungWorkflow {
    type State = RechnungsabwicklungState;
    type Event = RechnungsabwicklungEvent;
    type Command = RechnungsabwicklungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[RECHNUNGSABWICKLUNG_DEADLINE_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| RechnungsabwicklungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            RechnungsabwicklungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            RechnungsabwicklungEvent::BestellungErhalten {
                sender,
                receiver,
                message_ref,
            } => RechnungsabwicklungState::Bestellt(RechnungsabwicklungData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                message_ref: message_ref.clone(),
                dokumentennummer: String::new(),
                ausfuehrungsdatum: None,
            }),
            RechnungsabwicklungEvent::AngebotAbgelehnt {
                ebd, antwort_code, ..
            } => RechnungsabwicklungState::Abgelehnt {
                ebd: ebd.clone(),
                antwort_code: antwort_code.clone(),
            },
            RechnungsabwicklungEvent::BeendigungErhalten {
                sender,
                receiver,
                message_ref,
                dokumentennummer,
                ausfuehrungsdatum,
            } => RechnungsabwicklungState::BeendigungEingegangen(RechnungsabwicklungData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                message_ref: message_ref.clone(),
                dokumentennummer: dokumentennummer.clone(),
                ausfuehrungsdatum: ausfuehrungsdatum.clone(),
            }),
            RechnungsabwicklungEvent::BeendigungGesendet { .. } => match state {
                RechnungsabwicklungState::New => RechnungsabwicklungState::BeendigungAngefragt,
                other => other,
            },
            RechnungsabwicklungEvent::AntwortGesendet { response_pid, .. } => match state {
                RechnungsabwicklungState::BeendigungEingegangen(_) => {
                    RechnungsabwicklungState::Beendet {
                        zugestimmt: response_pid.as_u32() == 19009,
                    }
                }
                other => other,
            },
            RechnungsabwicklungEvent::AntwortErhalten { response_pid, .. } => match state {
                RechnungsabwicklungState::BeendigungAngefragt => {
                    RechnungsabwicklungState::Beendet {
                        zugestimmt: response_pid.as_u32() == 19009,
                    }
                }
                other => other,
            },
            RechnungsabwicklungEvent::Rejected { reason } => RechnungsabwicklungState::Rejected {
                reason: reason.clone(),
            },
            RechnungsabwicklungEvent::DeadlineExpired { label, .. } => match state {
                RechnungsabwicklungState::BeendigungEingegangen(_)
                | RechnungsabwicklungState::BeendigungAngefragt => {
                    RechnungsabwicklungState::FristAbgelaufen {
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
            RechnungsabwicklungCommand::ReceiveOrders {
                pid,
                sender,
                receiver,
                message_ref,
                dokumentennummer,
                ausfuehrungsdatum,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, RechnungsabwicklungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !RECHNUNGSABWICKLUNG_ORDERS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected ORDERS 17005/17006, got {pid}",
                    )));
                }
                if !validation_passed {
                    return Ok(vec![RechnungsabwicklungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }
                let event = if pid.as_u32() == 17005 {
                    RechnungsabwicklungEvent::BestellungErhalten {
                        sender,
                        receiver,
                        message_ref,
                    }
                } else {
                    RechnungsabwicklungEvent::BeendigungErhalten {
                        sender,
                        receiver,
                        message_ref,
                        dokumentennummer,
                        ausfuehrungsdatum,
                    }
                };
                Ok(vec![event].into())
            }

            RechnungsabwicklungCommand::ReceiveAngebotAblehnung {
                sender,
                receiver,
                herkunft,
                antwort_code,
                message_ref,
            } => {
                if !matches!(state, RechnungsabwicklungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let ebd = herkunft.map(angebot_ablehnung_ebd);
                // A code from another tree is not a softer refusal — it is
                // unreadable at this end. `None` for the code is admissible
                // (the AHB makes the STS conditional); a wrong code is not.
                //
                // With no Herkunft there is no tree to check against, and
                // checking against a guessed one is worse than not checking:
                // `E_0205` and `E_0208` share `A01`–`A03`, so the guess would
                // pass and stamp the wrong Bedeutung on the refusal.
                if let (Some(t), Some(c)) = (ebd, antwort_code.as_ref())
                    && mako_pruefung::codes::lookup(t, c).is_none()
                {
                    return Err(WorkflowError::rejected(format!(
                        "Antwortcode {c:?} is not published in {t}"
                    )));
                }
                Ok(vec![RechnungsabwicklungEvent::AngebotAbgelehnt {
                    sender,
                    receiver,
                    ebd: ebd.map(ToOwned::to_owned),
                    antwort_code,
                    message_ref,
                }]
                .into())
            }

            RechnungsabwicklungCommand::SendBeendigung {
                sender,
                counterparty,
                location_id,
                message_ref,
                beendet_durch,
                ausfuehrungsdatum,
                grund,
            } => {
                if !matches!(state, RechnungsabwicklungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                // ORDERS AHB 1.1b Kap. 4.14.2 (S. 93–94): the `SG29` Grund der
                // Anforderung is Muss when the MSB sends (`[15]`) and has no
                // place when the LF does.
                let grund = match (beendet_durch, grund) {
                    (BeendigungDurch::Msb, Some(g))
                        if matches!(g.as_str(), "Z61" | "Z62" | "Z63") =>
                    {
                        Some(g)
                    }
                    (BeendigungDurch::Msb, other) => {
                        return Err(WorkflowError::rejected(format!(
                            "an MSB's Beendigung 17006 names its Grund der Anforderung — Z61, \
                             Z62 or Z63 (ORDERS AHB 1.1b Kap. 4.14.2, S. 94, [15]); got {other:?}"
                        )));
                    }
                    (BeendigungDurch::Lf, None) => None,
                    (BeendigungDurch::Lf, Some(g)) => {
                        return Err(WorkflowError::rejected(format!(
                            "an LF's Beendigung 17006 carries no Grund der Anforderung — the \
                             SG29 is the MSB's (ORDERS AHB 1.1b Kap. 4.14.2, S. 94, [15]); got {g:?}"
                        )));
                    }
                };
                if ausfuehrungsdatum.is_empty() {
                    return Err(WorkflowError::rejected(
                        "the Beendigung 17006 names the day it takes effect (DTM+203, Muss)"
                            .to_owned(),
                    ));
                }
                let outbox = PendingOutbox::typed(
                    counterparty.as_str(),
                    &mako_engine::payload::OrdersPayload {
                        pid: Some(17_006),
                        sender: Some(sender.as_str().to_owned()),
                        sparte: Some(mako_engine::types::Sparte::Strom),
                        location: Some(location_id),
                        message_ref: Some(message_ref.as_str().to_owned()),
                        document_id: Some(message_ref.as_str().to_owned()),
                        ausfuehrungsdatum: Some(ausfuehrungsdatum),
                        grund,
                        ..Default::default()
                    },
                );
                Ok(WorkflowOutput {
                    events: vec![RechnungsabwicklungEvent::BeendigungGesendet { message_ref }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            RechnungsabwicklungCommand::SendAntwort {
                zustimmung,
                antwort_code,
                beendet_durch,
                message_ref,
            } => {
                let RechnungsabwicklungState::BeendigungEingegangen(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "BeendigungEingegangen",
                        state.label(),
                    ));
                };
                // `SG2 AJT` is Muss: a code of the tree the ending side picks,
                // in the cluster of the answer (`[17]`/`[18]`).
                let tree = beendet_durch.ebd();
                let code = mako_pruefung::codes::lookup(tree, &antwort_code).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "Antwortcode {antwort_code:?} is not published in {tree}"
                    ))
                })?;
                if code.ist_zustimmung() != Some(zustimmung) {
                    return Err(WorkflowError::rejected(format!(
                        "{tree} {} sits in the {} cluster, not the one a {} answers with",
                        code.code,
                        code.cluster.label(),
                        antwort_pid(zustimmung),
                    )));
                }
                // `SG1 RFF+ON` names the ORDERS 17006 by its `BGM` DE 1004
                // (`[520]`), and `DTM+203` restates its Ausführungsdatum —
                // both Muss.
                if data.dokumentennummer.is_empty() {
                    return Err(WorkflowError::rejected(
                        "the answer names the ORDERS 17006 by its BGM Dokumentennummer \
                         (SG1 RFF+ON), and none was recorded with it",
                    ));
                }
                let Some(ausfuehrungsdatum) = data.ausfuehrungsdatum.clone() else {
                    return Err(WorkflowError::rejected(
                        "the answer restates the ORDERS 17006's Ausführungsdatum (DTM+203), \
                         and none was recorded with it",
                    ));
                };
                let response_pid = Pruefidentifikator::new(antwort_pid(zustimmung))
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                // ORDRSP AHB 1.1b Kap. 4.14 (S. 57–59): `BGM+Z29`, `DTM+203`,
                // `IMD++Z02` Ende Abo, `SG1 RFF+ON`, `SG2 AJT` with the EBD.
                // A Strom-only exchange (`[30]`), so the day begins in Strom.
                let outbox = PendingOutbox::typed(
                    data.sender.as_str(),
                    &mako_engine::payload::OrdrspPayload {
                        pid: Some(response_pid.as_u32()),
                        sender: Some(data.receiver.as_str().to_owned()),
                        receiver: Some(data.sender.as_str().to_owned()),
                        document_code: Some("Z29".to_owned()),
                        order_reference: Some(data.dokumentennummer.clone()),
                        message_ref: Some(message_ref.as_str().to_owned()),
                        ausfuehrungsdatum: Some(ausfuehrungsdatum),
                        sparte: Some(mako_engine::types::Sparte::Strom),
                        leistung: Some("Z02".to_owned()),
                        antwort_code: Some(code.code.to_owned()),
                        antwort_codeliste: Some(code.wire_codeliste().unwrap_or(tree).to_owned()),
                        ..Default::default()
                    },
                );
                Ok(WorkflowOutput {
                    events: vec![RechnungsabwicklungEvent::AntwortGesendet {
                        response_pid,
                        message_ref,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            RechnungsabwicklungCommand::ReceiveAntwort {
                pid,
                message_ref,
                antwort,
            } => {
                if !RECHNUNGSABWICKLUNG_ORDRSP_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected ORDRSP 19009/19010, got {pid}",
                    )));
                }
                if !matches!(state, RechnungsabwicklungState::BeendigungAngefragt) {
                    return Err(WorkflowError::invalid_state(
                        "BeendigungAngefragt",
                        state.label(),
                    ));
                }
                Ok(vec![RechnungsabwicklungEvent::AntwortErhalten {
                    response_pid: pid,
                    message_ref,
                    antwort,
                }]
                .into())
            }

            RechnungsabwicklungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(vec![].into());
                }
                Ok(vec![RechnungsabwicklungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::workflow::Workflow;

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn receive(pid_code: u32, ok: bool) -> RechnungsabwicklungCommand {
        RechnungsabwicklungCommand::ReceiveOrders {
            pid: pid(pid_code),
            sender: mcod("9900000000004"),
            receiver: mcod("9900357000004"),
            message_ref: mref("ORDERS-1"),
            dokumentennummer: "ORD-17006-1".to_owned(),
            ausfuehrungsdatum: Some("20270101".to_owned()),
            validation_passed: ok,
            validation_errors: if ok { vec![] } else { vec!["AHB".into()] },
        }
    }

    fn apply_all(
        init: RechnungsabwicklungState,
        events: &[RechnungsabwicklungEvent],
    ) -> RechnungsabwicklungState {
        events
            .iter()
            .fold(init, WimRechnungsabwicklungWorkflow::apply)
    }

    #[test]
    fn the_lf_ablehnung_states_its_code_and_a_bestellung_states_none() {
        let abgelehnt = |ebd: Option<&str>| RechnungsabwicklungEvent::AngebotAbgelehnt {
            sender: mcod("9900000000004"),
            receiver: mcod("9900357000004"),
            ebd: ebd.map(str::to_owned),
            antwort_code: Some("A01".into()),
            message_ref: mref("IFTSTA-1"),
        };
        assert_eq!(
            abgelehnt(Some("E_0205")).antwort(),
            Some(Antwortcode::new("A01", Some("E_0205")))
        );
        assert_eq!(
            abgelehnt(None).antwort(),
            Some(Antwortcode::new("A01", None::<String>)),
            "an unknown Herkunft names no tree"
        );
        assert_eq!(
            RechnungsabwicklungEvent::BeendigungGesendet {
                message_ref: mref("ORDERS-2")
            }
            .antwort(),
            None
        );
    }

    /// A Bestellung is terminal on the wire: nothing answers a 17005, so the
    /// process records the arrangement and is done — no ORDRSP state to hang.
    #[test]
    fn a_bestellung_records_and_completes() {
        let out = WimRechnungsabwicklungWorkflow::handle(
            &RechnungsabwicklungState::New,
            receive(17005, true),
        )
        .unwrap();
        let state = apply_all(RechnungsabwicklungState::New, &out.events);
        assert!(matches!(state, RechnungsabwicklungState::Bestellt(_)));
        // And it stays terminal: a stray answer command is refused.
        assert!(
            WimRechnungsabwicklungWorkflow::handle(
                &state,
                RechnungsabwicklungCommand::SendAntwort {
                    zustimmung: true,
                    antwort_code: "A02".to_owned(),
                    beendet_durch: BeendigungDurch::Msb,
                    message_ref: mref("X"),
                },
            )
            .is_err(),
            "a Bestellung has no answer to send",
        );
    }

    /// The receiving side of a Beendigung owes an ORDRSP — Zustimmung maps to
    /// 19009, Ablehnung to 19010, and the terminal state records which.
    #[test]
    fn a_received_beendigung_is_answered_by_decision() {
        for (zustimmung, code, want_pid, want_ended) in
            [(true, "A02", 19009, true), (false, "A01", 19010, false)]
        {
            let out = WimRechnungsabwicklungWorkflow::handle(
                &RechnungsabwicklungState::New,
                receive(17006, true),
            )
            .unwrap();
            let state = apply_all(RechnungsabwicklungState::New, &out.events);
            assert!(matches!(
                state,
                RechnungsabwicklungState::BeendigungEingegangen(_)
            ));

            let out = WimRechnungsabwicklungWorkflow::handle(
                &state,
                RechnungsabwicklungCommand::SendAntwort {
                    zustimmung,
                    antwort_code: code.to_owned(),
                    beendet_durch: BeendigungDurch::Msb,
                    message_ref: mref("ORDRSP-1"),
                },
            )
            .unwrap();
            // A code off the answer's cluster, or out of the other tree, is
            // refused.
            for (wrong, durch) in [
                (if zustimmung { "A01" } else { "A02" }, BeendigungDurch::Msb),
                ("A04", BeendigungDurch::Msb),
            ] {
                assert!(
                    WimRechnungsabwicklungWorkflow::handle(
                        &state,
                        RechnungsabwicklungCommand::SendAntwort {
                            zustimmung,
                            antwort_code: wrong.to_owned(),
                            beendet_durch: durch,
                            message_ref: mref("ORDRSP-1"),
                        },
                    )
                    .is_err(),
                    "{wrong} answering a {want_pid}"
                );
            }
            let RechnungsabwicklungEvent::AntwortGesendet { response_pid, .. } = &out.events[0]
            else {
                panic!("expected AntwortGesendet");
            };
            assert_eq!(response_pid.as_u32(), want_pid);
            let state = apply_all(state, &out.events);
            assert!(
                matches!(state, RechnungsabwicklungState::Beendet { zugestimmt } if zugestimmt == want_ended),
            );
        }
    }

    /// The requester side: mako sends the 17006 and the counterparty's ORDRSP
    /// resumes and closes the process.
    #[test]
    fn an_initiated_beendigung_completes_on_the_counterparty_answer() {
        let out = WimRechnungsabwicklungWorkflow::handle(
            &RechnungsabwicklungState::New,
            RechnungsabwicklungCommand::SendBeendigung {
                sender: mcod("9900000000011"),
                counterparty: mcod("9900000000004"),
                location_id: "51238696012".to_owned(),
                message_ref: mref("ORDERS-OUT-1"),
                beendet_durch: BeendigungDurch::Lf,
                ausfuehrungsdatum: "20270101".to_owned(),
                grund: None,
            },
        )
        .unwrap();
        // The ORDERS 17006 reads back as its typed payload.
        use mako_engine::payload::OutboundPayload as _;
        let typed = mako_engine::payload::OrdersPayload::from_payload(&out.outbox[0].payload)
            .expect("an ORDERS payload");
        assert_eq!(typed.location.as_deref(), Some("51238696012"));
        let state = apply_all(RechnungsabwicklungState::New, &out.events);
        assert!(matches!(
            state,
            RechnungsabwicklungState::BeendigungAngefragt
        ));

        let out = WimRechnungsabwicklungWorkflow::handle(
            &state,
            RechnungsabwicklungCommand::ReceiveAntwort {
                pid: pid(19010),
                message_ref: mref("ORDRSP-IN-1"),
                antwort: Some(Antwortcode::new("A01", Some("E_0209"))),
            },
        )
        .unwrap();
        // The MSB's code rides the received answer's envelope.
        assert_eq!(
            out.events[0].antwort().map(|a| a.code),
            Some("A01".to_owned())
        );
        let state = apply_all(state, &out.events);
        assert!(
            matches!(
                state,
                RechnungsabwicklungState::Beendet { zugestimmt: false }
            ),
            "an Ablehnung ends the process with the arrangement still running",
        );
    }

    /// Validation failure rejects instead of recording a broken arrangement,
    /// and off-family PIDs are refused outright.
    #[test]
    fn bad_input_is_refused() {
        let out = WimRechnungsabwicklungWorkflow::handle(
            &RechnungsabwicklungState::New,
            receive(17006, false),
        )
        .unwrap();
        let state = apply_all(RechnungsabwicklungState::New, &out.events);
        assert!(matches!(state, RechnungsabwicklungState::Rejected { .. }));

        assert!(
            WimRechnungsabwicklungWorkflow::handle(
                &RechnungsabwicklungState::New,
                receive(17001, true)
            )
            .is_err(),
            "a Geräteübernahme ORDERS does not belong here",
        );
    }

    /// A deadline on a settled process is a no-op — deadlines fire on the
    /// healthy path too, and only the unanswered states may escalate.
    #[test]
    fn timeout_on_settled_states_is_noop() {
        for state in [
            RechnungsabwicklungState::Bestellt(RechnungsabwicklungData {
                sender: mcod("9900000000004"),
                receiver: mcod("9900357000004"),
                message_ref: mref("ORDERS-1"),
                dokumentennummer: String::new(),
                ausfuehrungsdatum: None,
            }),
            RechnungsabwicklungState::Beendet { zugestimmt: true },
        ] {
            let out = WimRechnungsabwicklungWorkflow::handle(
                &state,
                RechnungsabwicklungCommand::TimeoutExpired {
                    deadline_id: DeadlineId::new(),
                    label: RECHNUNGSABWICKLUNG_DEADLINE_LABEL.into(),
                },
            )
            .unwrap();
            assert!(out.events.is_empty());
        }
    }

    use mako_engine::ids::DeadlineId;

    /// The trap this module's `Option<AngebotHerkunft>` exists for: `E_0205`
    /// and `E_0208` **share** `A01`–`A03` and disagree about what they mean, so
    /// a guessed Herkunft does not fail closed — the code check passes and the
    /// wrong Bedeutung is recorded.
    #[test]
    fn the_two_angebot_trees_overlap_and_disagree() {
        use mako_pruefung::codes::lookup;
        for c in ["A01", "A02", "A03"] {
            let a = lookup("E_0205", c).unwrap_or_else(|| panic!("E_0205 publishes {c}"));
            let b = lookup("E_0208", c).unwrap_or_else(|| panic!("E_0208 publishes {c}"));
            assert_ne!(
                a.bedeutung, b.bedeutung,
                "{c} must mean different things in the two trees — that is the trap"
            );
        }
        // …and the ones only `E_0205` has are what a wrong guess would reject
        // instead, which is the visible half of the same defect.
        for c in ["A04", "A05", "A06"] {
            assert!(lookup("E_0205", c).is_some());
            assert!(lookup("E_0208", c).is_none());
        }
    }

    /// With no Herkunft the refusal is recorded and no tree is claimed.
    #[test]
    fn an_unknown_herkunft_records_the_code_without_a_tree() {
        let out = WimRechnungsabwicklungWorkflow::handle(
            &RechnungsabwicklungState::New,
            RechnungsabwicklungCommand::ReceiveAngebotAblehnung {
                sender: MarktpartnerCode::new("9900000000002"),
                receiver: MarktpartnerCode::new("9900000000003"),
                herkunft: None,
                antwort_code: Some("A01".to_owned()),
                message_ref: MessageRef::new("MSG-1"),
            },
        )
        .expect("an unknown Herkunft is recorded, not refused");
        let state = out
            .events
            .iter()
            .fold(RechnungsabwicklungState::New, |s, e| {
                WimRechnungsabwicklungWorkflow::apply(s, e)
            });
        let RechnungsabwicklungState::Abgelehnt { ebd, antwort_code } = &state else {
            panic!("expected Abgelehnt, got {}", state.label());
        };
        assert_eq!(*ebd, None, "no tree may be claimed without the Herkunft");
        assert_eq!(antwort_code.as_deref(), Some("A01"));
    }

    /// A stated Herkunft does resolve the tree, and a code outside it is refused.
    #[test]
    fn a_stated_herkunft_resolves_the_tree_and_checks_the_code() {
        let cmd = |herkunft, code: &str| RechnungsabwicklungCommand::ReceiveAngebotAblehnung {
            sender: MarktpartnerCode::new("9900000000002"),
            receiver: MarktpartnerCode::new("9900000000003"),
            herkunft: Some(herkunft),
            antwort_code: Some(code.to_owned()),
            message_ref: MessageRef::new("MSG-1"),
        };
        let out = WimRechnungsabwicklungWorkflow::handle(
            &RechnungsabwicklungState::New,
            cmd(AngebotHerkunft::AngebotDurchMsb, "A05"),
        )
        .expect("A05 is published in E_0205");
        let RechnungsabwicklungEvent::AngebotAbgelehnt { ebd, .. } = &out.events[0] else {
            panic!("expected an AngebotAbgelehnt");
        };
        assert_eq!(ebd.as_deref(), Some("E_0205"));

        // `A05` does not exist in `E_0208` — the LF-initiated sequence.
        assert!(
            WimRechnungsabwicklungWorkflow::handle(
                &RechnungsabwicklungState::New,
                cmd(AngebotHerkunft::AnfrageDurchLf, "A05"),
            )
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
        let antwort = RechnungsabwicklungEvent::AntwortGesendet {
            response_pid: pid(19_009),
            message_ref: MessageRef::new("RSP-1"),
        };
        assert_eq!(
            antwort.verlauf(),
            Some(stated(Richtung::Ausgang, Some(19_009), Some("RSP-1")))
        );
        let beendigung = RechnungsabwicklungEvent::BeendigungErhalten {
            sender: MarktpartnerCode::new("9900000000001"),
            receiver: MarktpartnerCode::new("9900000000002"),
            message_ref: MessageRef::new("ORD-1"),
            dokumentennummer: "ORD-1".to_owned(),
            ausfuehrungsdatum: None,
        };
        assert_eq!(
            beendigung.verlauf(),
            Some(stated(Richtung::Eingang, None, Some("ORD-1")))
        );
        assert_eq!(
            RechnungsabwicklungEvent::Rejected { reason: "x".into() }.verlauf(),
            None
        );
    }
}
