//! The INVOIC settle/dispute state machine, shared by every billing family.
//!
//! Every billing process in German market communication is the same
//! conversation. An invoice is issued; the recipient validates it against the
//! AHB and either settles or disputes it; where this deployment is the
//! *issuer*, a REMADV comes back confirming or refusing payment, and a COMDIS
//! may refuse that REMADV in turn.
//!
//! Nothing in it is commodity-specific: the process is keyed on the invoice
//! reference, and the Sparte only decides which price sheet `invoic-checker`
//! fetches — `invoicd`'s decision, not the workflow's. GPKE, WiM, GaBi Gas and
//! GeLi Gas therefore register an [`InvoicFamily`] here rather than
//! implementing the process.
//!
//! # What a family chooses
//!
//! [`InvoicFamily`] is the whole of the variation — the PID sets, the two role
//! capabilities, the deadline label and the workflow name. Everything else is
//! shared.
//!
//! ```text
//! ── Recipient (payer) ────────────────────────────────────────────────
//! New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed
//!                                        ╰─[invalid]──► Rejected
//! ValidationPassed ──SettleInvoice──► Settled
//!                  ╰─DisputeInvoice──► Disputed
//!
//! ── Issuer ───────────────────────────────────────────────────────────
//! New ──SendInvoic──► InvoicSent ──ReceiveRemadv 33001──► PaymentConfirmed
//!                                ╰─ReceiveRemadv 33002/3/4──► PaymentDisputed
//!
//! ── Payer, after its REMADV was refused ──────────────────────────────
//! any settled/sent state ──ReceiveComdis 29001──► ComdisRejected
//!
//! Any non-terminal state ──TimeoutExpired──► Rejected
//! ```
//!
//! # Regulatory basis
//!
//! - **INVOIC AHB 1.0** (FV2025-10-01 onwards; AHB 2.8e before) — the invoice
//!   message and its Prüfidentifikatoren.
//! - **REMADV AHB 1.0a § 3** — the payment advice. Settlement is „ganz oder gar
//!   nicht": there are no Teilzahlungen, so 33002/33003/33004 are all
//!   Abweisungen and only 33001 confirms.
//! - **COMDIS AHB 1.0** — the invoicer's refusal of a payer's REMADV (29001).
//! - **APERAK AHB 1.0 § 2.4.1** — the technical acknowledgement, 45 Minuten on a
//!   weekday. A different clock from the business answer this workflow runs.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::marker::PhantomData;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use rubo4e::current::Rechnung;

// ── Shared PID sets ───────────────────────────────────────────────────────────

/// The REMADV Prüfidentifikatoren a payer can answer an invoice with.
///
/// Received by the **invoicer** after sending an INVOIC. Per REMADV AHB 1.0a § 3
/// settlement is „ganz oder gar nicht" — there are no Teilzahlungen, so only
/// 33001 confirms payment and the other three are all Abweisungen.
///
/// | PID   | Name                                                          |
/// |-------|---------------------------------------------------------------|
/// | 33001 | Bestätigung (Zahlungsavis — vollständige Zahlung bestätigt)   |
/// | 33002 | Abweisung (nicht positionsscharf)                             |
/// | 33003 | Strom Abweisung Kopf und Summe (positionsscharf)              |
/// | 33004 | Strom Abweisung Position (positionsscharf)                    |
pub const REMADV_PIDS: &[u32] = &[33001, 33002, 33003, 33004];

/// The single REMADV PID that confirms payment. Everything else disputes it.
pub const REMADV_CONFIRMATION_PID: u32 = 33001;

/// The REMADV a payer **sends** to confirm — the Zahlungsavis. It carries no
/// `AJT` at all (REMADV AHB 1.0a § 3.1.1): agreement needs no Antwortcode.
pub const ZAHLUNGSAVIS_PID: u32 = REMADV_CONFIRMATION_PID;

/// The REMADV a payer sends to refuse an invoice whose tree answers with **one**
/// code — the plain „Abweisung" of REMADV AHB 1.0a § 3.1.1.
///
/// Not the default for every refusal: § 3.1.2's 33003 / 33004 pair is what
/// carries a *set* of codes, and DE 1082 admits a different list of trees on
/// each. [`RemadvAntwort::remadv_pid`] is what picks between them.
pub const ABWEISUNG_PID: u32 = 33002;

/// COMDIS Prüfidentifikator for an inbound Ablehnung of a REMADV (payer side).
///
/// Sent by the invoicer when it refuses the payer's REMADV — e.g. because the
/// stated payment amount is wrong. Source: COMDIS AHB 1.0.
pub const COMDIS_ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(29001);

/// `true` when `pid` is a REMADV that confirms payment rather than disputing it.
#[must_use]
pub fn remadv_confirms(pid: Pruefidentifikator) -> bool {
    pid.as_u32() == REMADV_CONFIRMATION_PID
}

// ── Family ────────────────────────────────────────────────────────────────────

/// What one billing family chooses. Everything else about the process is shared.
///
/// A family is a zero-sized marker type; [`InvoicWorkflow`] is generic over it.
pub trait InvoicFamily: Send + Sync + 'static {
    /// Canonical workflow name registered in the process engine.
    ///
    /// Used as the `workflow_name` parameter in `spawn_or_resume` /
    /// `dispatch_to_process` calls, and stored on every stream.
    const WORKFLOW_NAME: &'static str;

    /// Deadline label for the settlement response window.
    ///
    /// Register a `Deadline` with this label once the invoice validates; the
    /// recipient must settle or dispute before it fires.
    const DEADLINE_LABEL: &'static str;

    /// INVOIC Prüfidentifikatoren this family accepts, inbound and outbound.
    const INVOIC_PIDS: &'static [u32];

    /// Whether this deployment can play the **issuer** role for the family —
    /// recording an outbound INVOIC and correlating the payer's REMADV back to
    /// it.
    ///
    /// A family that only ever receives invoices refuses `SendInvoic` and
    /// `ReceiveRemadv` rather than opening a state it cannot reach honestly.
    const SENDS_INVOIC: bool;

    /// Whether the family exchanges COMDIS 29001 — the invoicer's refusal of a
    /// payer's REMADV.
    const ANSWERS_COMDIS: bool;

    /// Human-readable PID list, for the rejection message when an unexpected
    /// PID arrives. Defaults to the debug rendering of [`Self::INVOIC_PIDS`].
    #[must_use]
    fn pid_hint() -> String {
        Self::INVOIC_PIDS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("/")
    }
}

// ── Data carried through the process ──────────────────────────────────────────

/// The invoice facts a billing stream carries from receipt to settlement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InvoicData {
    /// BDEW Prüfidentifikator of the invoice.
    pub pruefidentifikator: Pruefidentifikator,
    /// MP-ID of the invoice sender (the issuer).
    pub sender: MarktpartnerCode,
    /// MP-ID of the invoice recipient (the payer).
    pub recipient: MarktpartnerCode,
    /// EDIFACT document date from BGM/DTM (`YYYYMMDD`).
    pub document_date: String,
    /// Invoice reference from UNH/BGM — the REMADV correlation key.
    pub invoice_ref: MessageRef,
    /// BO4E invoice object, translated from EDIFACT by the `makod` adapter.
    ///
    /// `invoicd` reads this from the event store to run `invoic-checker`
    /// without going back to the EDIFACT archive. Absent on the issuer side,
    /// where the document was rendered here rather than parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rechnung: Option<Box<Rechnung>>,
    /// `SG1 RFF+ACE` — the **order this invoice answers**.
    ///
    /// Muss on the WiM- and MSB-Rechnung (INVOIC AHB 1.0b segment 00020). What
    /// it names follows the Rechnungstyp: the ORDERS for `KON`/`TEC`, the
    /// QUOTES for `MSB`.
    ///
    /// A process fact rather than a BO4E field, because BO4E's `Rechnung`
    /// models the document and not the order behind it. `E_0264` Prüfschritt 40
    /// („Basiert die Rechnung auf einer Bestellung?") is what reads it — WiM
    /// Teil 2 UC 4.5.1: „Eine Rechnung referenziert auf die zugrundeliegende
    /// Bestellung."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bestellung_ref: Option<String>,
    /// `IMD+7081` — the Rechnungstyp, and on PID 31009 the **Use-Case**.
    ///
    /// `KON` „Abrechnung von Konfigurationen (Universalbestellprozess)" is the
    /// ESA billing of WiM Teil 2 Kap. 4.5 stated on the wire; `MSB` is the
    /// Messstellenbetrieb billed toward NB or LF, `TEC` the Änderung der
    /// Technik. One PID, three Use-Cases, three Entscheidungsbäume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rechnungstyp: Option<String>,
}

/// One `AJT` of a Nicht-Zahlungsavis — a published Antwortcode, the Ebene it
/// came from and, on the Positionsebene, the Positionsnummer it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemadvBefund {
    /// `AJT` DE 4465 — the Code des Prüfschritts.
    pub code: String,
    /// `"kopf"`, `"position"` or `"summe"`. The Kopf- and Summenebene ride
    /// REMADV 33003, the Positionsebene 33004.
    pub ebene: String,
    /// `SG26 LIN` Positionsnummer, on a position-level code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positionsnummer: Option<u16>,
    /// The written Erläuterung, where the code's own Hinweis requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The market answer a Nicht-Zahlungsavis carries.
///
/// Not a bare code: `AJT` DE 1082 names the **Entscheidungsbaum**, and the same
/// letter means different things across trees — `A70` is the Netznutzungs-
/// Summenprüfung of `E_0406` and is undefined in the ESA tree `E_0264`, whose
/// own total check is `A24`. The tree therefore travels with the codes, and
/// `mako_pruefung::codes::rechnungspruefung` is what picks it from the PID
/// and the recipient's Marktrolle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemadvAntwort {
    /// `AJT` DE 1082 — the EBD that publishes the codes (`E_0264`, `E_0406`).
    pub ebd: String,
    /// The refusals. Empty is not a refusal and must not reach this type.
    pub befunde: Vec<RemadvBefund>,
    /// The REMADV Prüfidentifikator this answer must ride — **33002** for a
    /// tree that answers with one code, **33003** („Abweisung Kopf und Summe")
    /// or **33004** („Abweisung Position") for one that answers with a set.
    pub remadv_pid: u32,
}

impl RemadvAntwort {
    /// The head code — what a single-`AJT` rendering states.
    #[must_use]
    pub fn erster_code(&self) -> Option<&str> {
        self.befunde.first().map(|b| b.code.as_str())
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of one billing process stream.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum InvoicState {
    /// No events yet.
    #[default]
    New,
    /// INVOIC received; AHB validation pending.
    InvoicReceived(InvoicData),
    /// INVOIC passed AHB validation; awaiting settlement or dispute.
    ValidationPassed(InvoicData),
    /// Invoice settled.
    Settled(InvoicData),
    /// Invoice disputed.
    Disputed {
        /// Invoice facts captured at the time of the dispute.
        data: InvoicData,
        /// Human-readable dispute reason.
        reason: String,
    },
    /// Process rejected — AHB validation failure or an expired deadline.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Outbound INVOIC recorded (issuer role); awaiting the payer's REMADV.
    InvoicSent(InvoicData),
    /// REMADV 33001 received — payment confirmed.
    PaymentConfirmed(InvoicData),
    /// REMADV 33002/33003/33004 received — payment refused.
    PaymentDisputed {
        /// Invoice facts.
        data: InvoicData,
        /// The REMADV PID that refused it.
        remadv_pid: Pruefidentifikator,
    },
    /// COMDIS 29001 received — the invoicer refused our REMADV (payer role).
    ComdisRejected(InvoicData),
}

impl InvoicState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::InvoicReceived(_) => "InvoicReceived",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Settled(_) => "Settled",
            Self::Disputed { .. } => "Disputed",
            Self::Rejected { .. } => "Rejected",
            Self::InvoicSent(_) => "InvoicSent",
            Self::PaymentConfirmed(_) => "PaymentConfirmed",
            Self::PaymentDisputed { .. } => "PaymentDisputed",
            Self::ComdisRejected(_) => "ComdisRejected",
        }
    }

    /// The invoice facts, once an invoice has been received or sent.
    #[must_use]
    pub fn data(&self) -> Option<&InvoicData> {
        match self {
            Self::InvoicReceived(d)
            | Self::ValidationPassed(d)
            | Self::Settled(d)
            | Self::InvoicSent(d)
            | Self::PaymentConfirmed(d)
            | Self::ComdisRejected(d) => Some(d),
            Self::Disputed { data, .. } | Self::PaymentDisputed { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }

    /// `true` when the process has reached an outcome and a late deadline must
    /// no longer overwrite it.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Settled(_)
                | Self::Disputed { .. }
                | Self::Rejected { .. }
                | Self::PaymentConfirmed(_)
                | Self::PaymentDisputed { .. }
                | Self::ComdisRejected(_)
        )
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum InvoicEvent {
    /// An inbound INVOIC was received and its domain fields extracted.
    InvoicReceived {
        /// Invoice reference from UNH/BGM.
        invoice_ref: MessageRef,
        /// MP-ID of the issuer.
        sender: MarktpartnerCode,
        /// MP-ID of the payer.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// BO4E invoice object for downstream plausibility checking.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rechnung: Option<Box<Rechnung>>,
        /// `SG1 RFF+ACE` — the order this invoice answers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bestellung_ref: Option<String>,
        /// `IMD+7081` — the Rechnungstyp, and on 31009 the Use-Case.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rechnungstyp: Option<String>,
    },
    /// AHB validation succeeded; the settlement window opens.
    ValidationPassed {
        /// Invoice reference, so a consumer need not re-read the receive event.
        invoice_ref: MessageRef,
    },
    /// The invoice was accepted and settled.
    InvoiceSettled,
    /// The invoice was disputed.
    InvoiceDisputed {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// The process was rejected — validation failure or a hard refusal.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// The settlement deadline fired before an answer was given.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
    /// An outbound INVOIC was recorded (issuer role).
    InvoicSent {
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// MP-ID of the issuer.
        sender: MarktpartnerCode,
        /// MP-ID of the payer.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Invoice reference — the REMADV correlation key.
        invoice_ref: MessageRef,
    },
    /// A REMADV answered the outbound invoice.
    RemadvReceived {
        /// The REMADV Prüfidentifikator.
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// MP-ID of the payer that sent it.
        sender: MarktpartnerCode,
        /// `true` only for PID 33001 — see [`REMADV_PIDS`].
        is_confirmed: bool,
    },
    /// A COMDIS 29001 refused our REMADV (payer role).
    ComdisAbLehnungReceived {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl EventPayload for InvoicEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "InvoicReceived",
            Self::ValidationPassed { .. } => "InvoicValidationPassed",
            Self::InvoiceSettled => "InvoiceSettled",
            Self::InvoiceDisputed { .. } => "InvoiceDisputed",
            Self::Rejected { .. } => "InvoicRejected",
            Self::DeadlineExpired { .. } => "InvoicDeadlineExpired",
            Self::InvoicSent { .. } => "InvoicSent",
            Self::RemadvReceived { .. } => "RemadvReceived",
            Self::ComdisAbLehnungReceived { .. } => "ComdisAblehnungReceived",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands accepted by the billing workflow.
#[derive(Clone)]
pub enum InvoicCommand {
    /// An inbound INVOIC arrived from the transport layer.
    ///
    /// The adapter parses the EDIFACT and runs AHB validation *before*
    /// constructing this command: pass `validation_passed: false` with
    /// `validation_errors` populated and the workflow rejects the process.
    ReceiveInvoic {
        /// BDEW Prüfidentifikator — must be one of [`InvoicFamily::INVOIC_PIDS`].
        pid: Pruefidentifikator,
        /// MP-ID of the issuer.
        sender: MarktpartnerCode,
        /// MP-ID of the payer.
        recipient: MarktpartnerCode,
        /// Invoice reference from UNH/BGM.
        invoice_ref: MessageRef,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// `true` when AHB profile validation found no errors.
        validation_passed: bool,
        /// Validation issues, empty when `validation_passed`.
        validation_errors: Vec<String>,
        /// BO4E invoice object, when the adapter translated one.
        rechnung: Option<Box<Rechnung>>,
        /// `SG1 RFF+ACE` — the order this invoice answers (INVOIC AHB 1.0b
        /// segment 00020, Muss on the WiM- and MSB-Rechnung).
        bestellung_ref: Option<String>,
        /// `IMD+7081` — the Rechnungstyp; `KON` is the ESA Use-Case.
        rechnungstyp: Option<String>,
    },
    /// **Issuer role:** record an outbound INVOIC so the payer's REMADV
    /// correlates back to it.
    SendInvoic {
        /// BDEW Prüfidentifikator of the outbound invoice.
        pid: Pruefidentifikator,
        /// MP-ID of the issuer.
        sender: MarktpartnerCode,
        /// MP-ID of the payer.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Invoice reference — the REMADV correlation key.
        invoice_ref: MessageRef,
    },
    /// **Issuer role:** an inbound REMADV answered the outbound invoice.
    ReceiveRemadv {
        /// The REMADV Prüfidentifikator — 33001 confirms, the rest dispute.
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// MP-ID of the payer that sent it.
        sender: MarktpartnerCode,
    },
    /// **Payer role:** an inbound COMDIS 29001 refused our REMADV.
    ReceiveComdis {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
    /// Settle the invoice — REMADV **33001** Zahlungsavis to the issuer.
    SettleInvoice {
        /// Belegnummer of the outbound REMADV. The issuer correlates its
        /// invoice by the `RFF` this message echoes, so it must equal the wire
        /// UNH reference the renderer emits.
        message_ref: MessageRef,
    },
    /// Dispute the invoice — a Nicht-Zahlungsavis to the issuer.
    ///
    /// Settlement is „ganz oder gar nicht" (REMADV AHB 1.0a § 3): there is no
    /// Teilzahlung, so this refuses the whole invoice.
    DisputeInvoice {
        /// Belegnummer of the outbound REMADV.
        message_ref: MessageRef,
        /// Human-readable dispute reason — `SG7 FTX+ABO`.
        reason: String,
        /// The published Antwortcode(s) the refusal states, and the tree that
        /// publishes them.
        ///
        /// `SG7 AJT` is **Muss** on every Nicht-Zahlungsavis (REMADV AHB 1.0a
        /// § 3.1.1 / § 3.1.2), so a refusal without one is a message the issuer
        /// cannot act on. `None` is accepted only from a caller that could not
        /// resolve a tree at all, and the renderer then refuses to put an
        /// incomplete answer on the wire.
        #[allow(clippy::struct_field_names)]
        antwort: Option<RemadvAntwort>,
    },
    /// The settlement deadline fired before an answer was given.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for InvoicCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The INVOIC settle/dispute workflow for one [`InvoicFamily`].
pub struct InvoicWorkflow<F: InvoicFamily>(PhantomData<fn() -> F>);

impl<F: InvoicFamily> Workflow for InvoicWorkflow<F> {
    type State = InvoicState;
    type Event = InvoicEvent;
    type Command = InvoicCommand;

    /// Deadline compensation for the settlement window.
    ///
    /// The window runs from receipt to answer, so it only compensates a stream
    /// that has an invoice in hand and has not yet answered.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (label, InvoicState::InvoicReceived(_) | InvoicState::ValidationPassed(_))
                if label == F::DEADLINE_LABEL =>
            {
                Some(InvoicCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            InvoicEvent::InvoicReceived {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
                rechnung,
                bestellung_ref,
                rechnungstyp,
            } => InvoicState::InvoicReceived(InvoicData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
                rechnung: rechnung.clone(),
                bestellung_ref: bestellung_ref.clone(),
                rechnungstyp: rechnungstyp.clone(),
            }),

            InvoicEvent::ValidationPassed { .. } => match state {
                InvoicState::InvoicReceived(data) => InvoicState::ValidationPassed(data),
                other => other,
            },

            InvoicEvent::InvoiceSettled => match state {
                // The **second round**. WiM Teil 2 Kap. 4.5.2 Nr. 4 has the
                // payer answer again after the issuer's COMDIS, and this time
                // conceding: the invoice stands and is paid.
                InvoicState::ComdisRejected(data) => InvoicState::Settled(data),
                InvoicState::ValidationPassed(data) => InvoicState::Settled(data),
                other => other,
            },

            InvoicEvent::InvoiceDisputed { reason } => match state {
                // The second round, refusing again — `E_0266` `A25`, „der MSB
                // konnte nicht alle Einwände entkräften". A third round is not
                // published: „kommt es zu einer erneuten Ablehnung durch den
                // MSB, ist eine bilaterale Klärung notwendig".
                InvoicState::ComdisRejected(data) => InvoicState::Disputed {
                    data,
                    reason: reason.clone(),
                },
                InvoicState::ValidationPassed(data) => InvoicState::Disputed {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            InvoicEvent::Rejected { reason } => InvoicState::Rejected {
                reason: reason.clone(),
            },

            // A deadline that fires after the process already reached an
            // outcome changes nothing — the answer was given in time.
            InvoicEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    InvoicState::Rejected {
                        reason: format!("settlement deadline expired: {label}"),
                    }
                }
            }

            InvoicEvent::InvoicSent {
                pruefidentifikator,
                sender,
                recipient,
                document_date,
                invoice_ref,
            } => InvoicState::InvoicSent(InvoicData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
                rechnung: None,
                // The issuer rendered the document here; the reference it put
                // on the wire is the sender's own and is not read back.
                bestellung_ref: None,
                rechnungstyp: None,
            }),

            InvoicEvent::RemadvReceived {
                pid, is_confirmed, ..
            } => match state {
                InvoicState::InvoicSent(data) => {
                    if *is_confirmed {
                        InvoicState::PaymentConfirmed(data)
                    } else {
                        InvoicState::PaymentDisputed {
                            remadv_pid: *pid,
                            data,
                        }
                    }
                }
                other => other,
            },

            // Accepted in exactly the states `handle` admits — see the guard
            // there for why those and no others.
            InvoicEvent::ComdisAbLehnungReceived { .. } => match state {
                InvoicState::ValidationPassed(data)
                | InvoicState::Settled(data)
                | InvoicState::Disputed { data, .. } => InvoicState::ComdisRejected(data),
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            InvoicCommand::ReceiveInvoic {
                pid,
                sender,
                recipient,
                invoice_ref,
                document_date,
                validation_passed,
                validation_errors,
                rechnung,
                bestellung_ref,
                rechnungstyp,
            } => {
                if !matches!(state, InvoicState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !F::INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an INVOIC PID for {} ({}), got {pid}",
                        F::WORKFLOW_NAME,
                        F::pid_hint(),
                    )));
                }
                let mut events = vec![InvoicEvent::InvoicReceived {
                    invoice_ref: invoice_ref.clone(),
                    sender: sender.clone(),
                    recipient: recipient.clone(),
                    document_date,
                    pruefidentifikator: pid,
                    rechnung: rechnung.clone(),
                    bestellung_ref: bestellung_ref.clone(),
                    rechnungstyp: rechnungstyp.clone(),
                }];
                let mut outbox: Vec<PendingOutbox> = Vec::new();
                if validation_passed {
                    events.push(InvoicEvent::ValidationPassed {
                        invoice_ref: invoice_ref.clone(),
                    });
                    // Tell `invoicd` a validated invoice is ready for
                    // plausibility checking. The BO4E `Rechnung` rides along so
                    // it can run `InvoicCheckEngine::check` straight off the
                    // webhook payload without re-reading the EDIFACT archive.
                    outbox.push(
                        PendingOutbox::new(
                            "ProcessInitiated",
                            recipient.as_str(),
                            serde_json::json!({
                                "pid":          pid.as_u32(),
                                "invoice_ref":  invoice_ref.as_str(),
                                "sender_mp_id": sender.as_str(),
                                "workflow":     F::WORKFLOW_NAME,
                                "rechnung":     serde_json::to_value(rechnung.as_deref())
                                    .unwrap_or(serde_json::Value::Null),
                                // The two EDIFACT facts BO4E has no field for.
                                // `E_0264` Prüfschritt 40 needs the first, and
                                // the second states the Use-Case on the wire —
                                // one PID, three of them.
                                "bestellung_ref": bestellung_ref,
                                "rechnungstyp":   rechnungstyp,
                            }),
                        )
                        // Caused by ValidationPassed (index 1).
                        .caused_by(1),
                    );
                } else {
                    events.push(InvoicEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            InvoicCommand::SettleInvoice { message_ref } => {
                if !answerable(state) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed|ComdisRejected",
                        state.label(),
                    ));
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![InvoicEvent::InvoiceSettled],
                    vec![
                        remadv_outbox(state, ZAHLUNGSAVIS_PID, &message_ref, None, None),
                        completion_outbox::<F>(state, "settled", None),
                    ],
                ))
            }

            InvoicCommand::DisputeInvoice {
                message_ref,
                reason,
                antwort,
            } => {
                if !answerable(state) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed|ComdisRejected",
                        state.label(),
                    ));
                }
                // REMADV AHB 1.0a § 3.1.1/§ 3.1.2 make `SG7 AJT` Muss on every
                // Nicht-Zahlungsavis, and the Prüfidentifikator follows the
                // shape of the answer: 33002 for a tree that states one code,
                // 33003/33004 for one that states a set. Defaulting to 33002
                // here would put an `E_0264` code on a Prüfidentifikator whose
                // DE 1082 does not admit that tree.
                let pid = antwort.as_ref().map_or(ABWEISUNG_PID, |a| a.remadv_pid);
                let outbox = vec![
                    remadv_outbox(state, pid, &message_ref, Some(&reason), antwort.as_ref()),
                    completion_outbox::<F>(state, "disputed", Some(&reason)),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![InvoicEvent::InvoiceDisputed { reason }],
                    outbox,
                ))
            }

            InvoicCommand::TimeoutExpired { deadline_id, label } => {
                // A deadline that fires after the answer was already given is a
                // no-op, not a rejection.
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![InvoicEvent::DeadlineExpired { deadline_id, label }].into())
            }

            InvoicCommand::SendInvoic {
                pid,
                sender,
                recipient,
                document_date,
                invoice_ref,
            } => {
                if !F::SENDS_INVOIC {
                    return Err(WorkflowError::rejected(format!(
                        "{} does not play the issuer role — it receives invoices only",
                        F::WORKFLOW_NAME,
                    )));
                }
                if !matches!(state, InvoicState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !F::INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an INVOIC PID for {} ({}), got {pid}",
                        F::WORKFLOW_NAME,
                        F::pid_hint(),
                    )));
                }
                Ok(vec![InvoicEvent::InvoicSent {
                    pruefidentifikator: pid,
                    sender,
                    recipient,
                    document_date,
                    invoice_ref,
                }]
                .into())
            }

            InvoicCommand::ReceiveRemadv {
                pid,
                remadv_ref,
                sender,
            } => {
                if !F::SENDS_INVOIC {
                    return Err(WorkflowError::rejected(format!(
                        "{} never issues an invoice, so no REMADV can answer one",
                        F::WORKFLOW_NAME,
                    )));
                }
                if !matches!(state, InvoicState::InvoicSent(_)) {
                    return Err(WorkflowError::invalid_state("InvoicSent", state.label()));
                }
                if !REMADV_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a REMADV PID (33001–33004), got {pid}",
                    )));
                }
                // REMADV AHB 1.0a § 3 — settlement is „ganz oder gar nicht".
                // Only 33001 confirms; 33002/33003/33004 are all Abweisungen.
                let is_confirmed = remadv_confirms(pid);
                Ok(vec![InvoicEvent::RemadvReceived {
                    pid,
                    remadv_ref,
                    sender,
                    is_confirmed,
                }]
                .into())
            }

            InvoicCommand::ReceiveComdis { comdis_ref } => {
                if !F::ANSWERS_COMDIS {
                    return Err(WorkflowError::rejected(format!(
                        "{} does not exchange COMDIS 29001",
                        F::WORKFLOW_NAME,
                    )));
                }
                // A COMDIS refuses a REMADV **we** sent, and we send one as the
                // *payer*: after validating an invoice we settle or dispute it,
                // and that answer is the REMADV. So the only states in which one
                // can arrive are the payer's answered states, plus
                // `ValidationPassed` for a COMDIS that races our own answer.
                //
                // `InvoicSent`, `PaymentConfirmed` and `PaymentDisputed` are the
                // *issuer's* states. There we are the one who would send a
                // COMDIS, never receive it, so an inbound one is a routing
                // error and saying so beats recording an event that `apply`
                // would then ignore — which is what the four copies did, each
                // with a slightly different set.
                if !matches!(
                    state,
                    InvoicState::ValidationPassed(_)
                        | InvoicState::Settled(_)
                        | InvoicState::Disputed { .. }
                ) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed|Settled|Disputed",
                        state.label(),
                    ));
                }
                Ok(vec![InvoicEvent::ComdisAbLehnungReceived { comdis_ref }].into())
            }
        }
    }
}

/// The `ProcessCompleted` outbox entry for a settled or disputed invoice.
/// The states a payer may answer an invoice from.
///
/// `ValidationPassed` is the first round. **`ComdisRejected` is the second**:
/// the issuer answered the payer's Nicht-Zahlungsavis with a COMDIS 29001
/// claiming its invoice was correct, and the payer owes another answer — WiM
/// Teil 2 Kap. 4.5.2 Nr. 4 for an ESA, by the Zahlungsziel, and the tree is
/// `E_0266` rather than `E_0264` (its Prüfschritt 1 asks whether the COMDIS
/// actually rebutted the objections, which `E_0264` does not publish a code
/// for).
///
/// Without this arm `ComdisRejected` is a dead end: the process records the
/// COMDIS and can never answer it, so the round that either releases the
/// payment or ends in bilateral clearing is unreachable.
const fn answerable(state: &InvoicState) -> bool {
    matches!(
        state,
        InvoicState::ValidationPassed(_) | InvoicState::ComdisRejected(_)
    )
}

/// Build the **outbound REMADV** — the answer the invoice issuer is waiting on.
///
/// The market answer and the ERP notification are two different messages with
/// two different audiences: `ProcessCompleted` tells this operator's own ERP
/// what happened, and only this reaches the counterparty. Both go out, because
/// an invoice recorded as answered in the § 147 AO trail and unanswered on the
/// wire is the same invoice.
///
/// The recipient is the invoice's **issuer**: a REMADV travels back up the
/// invoice, so sender and receiver are the mirror of the INVOIC's.
fn remadv_outbox(
    state: &InvoicState,
    pid: u32,
    message_ref: &MessageRef,
    reason: Option<&str>,
    antwort: Option<&RemadvAntwort>,
) -> PendingOutbox {
    let data = state.data();
    let issuer = data.map(|d| d.sender.as_str()).unwrap_or_default();
    let mut payload = serde_json::json!({
        "pid":         pid,
        "sender":      data.map(|d| d.recipient.as_str()).unwrap_or_default(),
        "receiver":    issuer,
        "message_ref": message_ref.as_str(),
        // BGM DE 1001: `481` Zahlungsavis, `239` Abgelehnte Forderung
        // (Nicht-Zahlungsavis) — REMADV AHB 1.0a § 3.1.1.
        "document_code": if pid == ZAHLUNGSAVIS_PID { "481" } else { "239" },
        // `SG5 RFF` — the invoice this answers. The issuer correlates on it.
        "invoice_ref": data.map(|d| d.invoice_ref.to_string()).unwrap_or_default(),
        "document_date": data.map(|d| d.document_date.clone()).unwrap_or_default(),
    });
    let Some(obj) = payload.as_object_mut() else {
        return PendingOutbox::new("REMADV", issuer, payload);
    };
    // `SG5` — the invoice being answered, its fälliger Betrag and its
    // Rechnungsdatum, all **Muss** (REMADV AHB 1.0a § 3.1.1 segments
    // 00012–00015). Read off the stored BO4E `Rechnung`: the payer side keeps
    // it precisely so the answer need not go back to the EDIFACT archive.
    //
    // The **Überweisungsbetrag** is not a copy of the fällige Betrag: condition
    // `[926]` fixes it to `0` on an Abweisung, because refusing an invoice
    // transfers nothing, and conditions `[3]`/`[4]` negate it on a Gutschrift.
    if let Some(r) = data.and_then(|d| d.rechnung.as_deref()) {
        let faellig = r
            .zu_zahlen
            .as_ref()
            .or(r.gesamtbrutto.as_ref())
            .and_then(|b| b.wert)
            .unwrap_or_default();
        let gutschrift = r.ist_storno == Some(true);
        let ueberweisung = if pid == ZAHLUNGSAVIS_PID {
            if gutschrift { -faellig } else { faellig }
        } else {
            rust_decimal::Decimal::ZERO
        };
        obj.insert(
            "rechnungsbezug".to_owned(),
            serde_json::json!({
                // `SG5 DOC` DE 1001. A Storno of a self-billed invoice is `Z25`,
                // of an ordinary one `457`; otherwise `389` self-billed and
                // `380` Handelsrechnung.
                "dokumentenart": match (gutschrift, r.ist_original == Some(false)) {
                    (true, true) => "Z25",
                    (true, false) => "457",
                    (false, true) => "389",
                    (false, false) => "380",
                },
                "rechnungsnummer": r.rechnungsnummer.clone().unwrap_or_default(),
                "faelliger_betrag": faellig.round_dp(2).to_string(),
                "ueberweisungsbetrag": ueberweisung.round_dp(2).to_string(),
                "rechnungsdatum": r
                    .rechnungsdatum
                    .map(|d| d.date().to_string())
                    .unwrap_or_default(),
            }),
        );
    }
    if let Some(reason) = reason {
        obj.insert("ablehnungsgrund".to_owned(), serde_json::json!(reason));
    }
    if let Some(a) = antwort {
        // `SG7 AJT` DE 4465 / DE 1082. The head code renders as the single
        // `AJT` every REMADV carries; the full set travels alongside it for the
        // itemised 33003/33004 rendering and for the audit trail, which has to
        // show every Prüfschritt that refused.
        obj.insert(
            "antwort_code".to_owned(),
            serde_json::json!(a.erster_code()),
        );
        obj.insert("antwort_ebd".to_owned(), serde_json::json!(a.ebd));
        obj.insert("antwort_befunde".to_owned(), serde_json::json!(a.befunde));
    }
    PendingOutbox::new("REMADV", issuer, payload)
}

fn completion_outbox<F: InvoicFamily>(
    state: &InvoicState,
    outcome: &str,
    reason: Option<&str>,
) -> PendingOutbox {
    let data = state.data();
    let mut payload = serde_json::json!({
        "pid":         data.map_or(0, |d| d.pruefidentifikator.as_u32()),
        "invoice_ref": data.map(|d| d.invoice_ref.to_string()).unwrap_or_default(),
        "workflow":    F::WORKFLOW_NAME,
        "outcome":     outcome,
    });
    if let Some(reason) = reason
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("reason".to_owned(), serde_json::json!(reason));
    }
    PendingOutbox::new("ProcessCompleted", "", payload)
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single billing process stream.
#[derive(Debug)]
pub struct InvoicRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// BDEW Prüfidentifikator, once an invoice has been received or sent.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for InvoicRecord {
    fn default() -> Self {
        Self {
            status: "New",
            pruefidentifikator: None,
            event_count: 0,
        }
    }
}

/// In-process read model tracking billing process streams.
#[derive(Debug, Default)]
pub struct InvoicProjection {
    /// All known billing process records keyed by stream ID.
    pub records: HashMap<String, InvoicRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for InvoicProjection {
    fn name(&self) -> &'static str {
        "InvoicProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<InvoicEvent>() else {
            return;
        };

        match event {
            InvoicEvent::InvoicReceived {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicReceived";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            InvoicEvent::ValidationPassed { .. } => record.status = "ValidationPassed",
            InvoicEvent::InvoiceSettled => record.status = "Settled",
            InvoicEvent::InvoiceDisputed { .. } => record.status = "Disputed",
            InvoicEvent::Rejected { .. } | InvoicEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
            }
            InvoicEvent::InvoicSent {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicSent";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            InvoicEvent::RemadvReceived { is_confirmed, .. } => {
                record.status = if is_confirmed {
                    "PaymentConfirmed"
                } else {
                    "PaymentDisputed"
                };
            }
            InvoicEvent::ComdisAbLehnungReceived { .. } => record.status = "ComdisRejected",
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        if self.last_seq == 0 {
            None
        } else {
            Some(self.last_seq)
        }
    }
}
