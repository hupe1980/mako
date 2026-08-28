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
//! and the AWH Aktivitätsdiagramme WiM V1.3 §§2.8–2.11:
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
//! - **17006 flows both ways.** The MSB may end the arrangement (AD §2.9, EBD
//!   `E_0206`) and so may the LF (AD §2.11, EBD `E_0209`); whichever side
//!   *receives* the Beendigung answers with ORDRSP 19009/19010. The answer is
//!   a decision, not an echo — it arrives through the ERP command API, exactly
//!   like a Sperrung confirmation.

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
    },
    /// APERAK 29001 dispatched.
    AperakFehlerDispatched {
        /// Error reason.
        reason: String,
        /// Outbound message reference.
        outbound_ref: MessageRef,
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
            Self::AperakFehlerDispatched { .. } => "RechnungsabwicklungAperakFehlerDispatched",
            Self::Rejected { .. } => "RechnungsabwicklungRejected",
            Self::DeadlineExpired { .. } => "RechnungsabwicklungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Parties of the exchange, captured at first receipt/send.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RechnungsabwicklungData {
    /// GLN of the initiating side.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving side.
    pub receiver: MarktpartnerCode,
    /// Message reference of the message that opened the process — for a
    /// received Beendigung this is what the ORDRSP echoes in `RFF+ON`, so the
    /// counterparty can correlate the answer.
    pub message_ref: MessageRef,
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
}

impl mako_engine::workflow::OccupiesBusinessKey for RechnungsabwicklungState {
    /// A MaLo is occupied only while a Beendigung is unanswered — a recorded
    /// Bestellung, a settled Beendigung and a rejection are all terminal and
    /// must not block the next process on the same MaLo.
    fn occupies_business_key(&self) -> bool {
        matches!(
            self,
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
        /// GLN of the counterparty (the other side of the arrangement).
        counterparty: MarktpartnerCode,
        /// MaLo the arrangement concerns — the ORDERS carries it in `LOC`.
        location_id: String,
        /// EDIFACT message reference of the outbound ORDERS 17006.
        message_ref: MessageRef,
    },
    /// Answer a received Beendigung (decision via the ERP command API —
    /// EBD `E_0206`/`E_0209` is the counterparty's check, the decision here
    /// is the operator's).
    SendAntwort {
        /// `true` → ORDRSP 19009 (Bestätigung), `false` → 19010 (Ablehnung).
        zustimmung: bool,
        /// EDIFACT message reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19009/19010 answering mako's Beendigung.
    ReceiveAntwort {
        /// 19009 or 19010.
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Dispatch APERAK 29001.
    DispatchAperakFehler {
        /// Error reason.
        reason: String,
        /// Outbound APERAK message reference.
        outbound_ref: MessageRef,
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

impl Workflow for WimRechnungsabwicklungWorkflow {
    type State = RechnungsabwicklungState;
    type Event = RechnungsabwicklungEvent;
    type Command = RechnungsabwicklungCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                RECHNUNGSABWICKLUNG_DEADLINE_LABEL,
                RechnungsabwicklungState::BeendigungEingegangen(_)
                | RechnungsabwicklungState::BeendigungAngefragt,
            ) => Some(RechnungsabwicklungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            RechnungsabwicklungEvent::BestellungErhalten {
                sender,
                receiver,
                message_ref,
            } => RechnungsabwicklungState::Bestellt(RechnungsabwicklungData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                message_ref: message_ref.clone(),
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
            } => RechnungsabwicklungState::BeendigungEingegangen(RechnungsabwicklungData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                message_ref: message_ref.clone(),
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
            RechnungsabwicklungEvent::AperakFehlerDispatched { reason, .. } => {
                RechnungsabwicklungState::Rejected {
                    reason: format!("APERAK 29001: {reason}"),
                }
            }
            RechnungsabwicklungEvent::Rejected { reason } => RechnungsabwicklungState::Rejected {
                reason: reason.clone(),
            },
            RechnungsabwicklungEvent::DeadlineExpired { label, .. } => match state {
                RechnungsabwicklungState::Bestellt(_)
                | RechnungsabwicklungState::Beendet { .. }
                | RechnungsabwicklungState::Rejected { .. } => state,
                _ => RechnungsabwicklungState::Rejected {
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
            RechnungsabwicklungCommand::ReceiveOrders {
                pid,
                sender,
                receiver,
                message_ref,
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
                counterparty,
                location_id,
                message_ref,
            } => {
                if !matches!(state, RechnungsabwicklungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                // The renderer supplies mako's own GLN as sender when the
                // payload names none — the sending role differs per deployment
                // (an MSB ends it as readily as an LF).
                let outbox = PendingOutbox::new(
                    "ORDERS",
                    counterparty.as_str(),
                    serde_json::json!({
                        "pid": 17006,
                        "receiver": counterparty.as_str(),
                        "location": location_id,
                        "message_ref": message_ref.as_str(),
                    }),
                );
                Ok(WorkflowOutput {
                    events: vec![RechnungsabwicklungEvent::BeendigungGesendet { message_ref }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            RechnungsabwicklungCommand::SendAntwort {
                zustimmung,
                message_ref,
            } => {
                let RechnungsabwicklungState::BeendigungEingegangen(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "BeendigungEingegangen",
                        state.label(),
                    ));
                };
                let response_pid = Pruefidentifikator::new(antwort_pid(zustimmung))
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                // The ORDRSP goes back to whoever sent the Beendigung and
                // echoes its message reference in `RFF+ON` so the initiator
                // can correlate the LOC-less answer.
                let outbox = PendingOutbox::new(
                    "ORDRSP",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": response_pid.as_u32(),
                        "receiver": data.sender.as_str(),
                        "order_reference": data.message_ref.as_str(),
                        "message_ref": message_ref.as_str(),
                    }),
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

            RechnungsabwicklungCommand::ReceiveAntwort { pid, message_ref } => {
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
                }]
                .into())
            }

            RechnungsabwicklungCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                if !matches!(
                    state,
                    RechnungsabwicklungState::New
                        | RechnungsabwicklungState::BeendigungEingegangen(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "New or BeendigungEingegangen",
                        state.label(),
                    ));
                }
                Ok(vec![RechnungsabwicklungEvent::AperakFehlerDispatched {
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            RechnungsabwicklungCommand::TimeoutExpired { deadline_id, label } => match state {
                RechnungsabwicklungState::Bestellt(_)
                | RechnungsabwicklungState::Beendet { .. }
                | RechnungsabwicklungState::Rejected { .. } => Ok(vec![].into()),
                _ => Ok(
                    vec![RechnungsabwicklungEvent::DeadlineExpired { deadline_id, label }].into(),
                ),
            },
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
        for (zustimmung, want_pid, want_ended) in [(true, 19009, true), (false, 19010, false)] {
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
                    message_ref: mref("ORDRSP-1"),
                },
            )
            .unwrap();
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
                counterparty: mcod("9900000000004"),
                location_id: "51238696012".to_owned(),
                message_ref: mref("ORDERS-OUT-1"),
            },
        )
        .unwrap();
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
            },
        )
        .unwrap();
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
