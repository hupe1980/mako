//! WiM INSRPT Störungsbehebung — the fault report and everything that answers it.
//!
//! One workflow hosts **both sides**. The deployment's Marktrolle decides which
//! commands it issues:
//!
//! - **Störungsmelder (LF/NB):** [`StoerungsmeldungCommand::SendStoerungsmeldung`]
//!   records the outbound 23001 and awaits the MSB's answer.
//! - **MSB:** [`StoerungsmeldungCommand::ReceiveStoerungsmeldung`] ingests the
//!   inbound 23001, then [`DispatchAntwort`] returns 23003/23004 and
//!   [`DispatchErgebnisbericht`] the 23008 that closes the Use-Case.
//!
//! [`DispatchAntwort`]: StoerungsmeldungCommand::DispatchAntwort
//! [`DispatchErgebnisbericht`]: StoerungsmeldungCommand::DispatchErgebnisbericht
//!
//! ## Prüfidentifikatoren
//!
//! The INSRPT AHB is Sparte-neutral: the same Prüfidentifikatoren carry the
//! Strom and the Gas Use-Case, except for two informational messages Gas adds.
//!
//! | PID   | Description | Direction | Sparte |
//! |-------|-------------|-----------|--------|
//! | 23001 | Störungsmeldung | Störungsmelder (LF/NB) → MSB | beide |
//! | 23003 | Ablehnung der Störungsmeldung | MSB → Melder | beide |
//! | 23004 | Bestätigung der Störungsmeldung | MSB → Melder | beide |
//! | 23005 | Informationsmeldung über die Störung | MSB → NB | **Gas** |
//! | 23008 | Ergebnisbericht (Mitteilung Ergebnis) | MSB → Melder | beide |
//! | 23009 | Informationsmeldung über die Behebung | MSB → NB | **Gas** |
//! | 23011 | Information über Störung an betroffener Marktlokation | MSB → LF/NB | Strom |
//! | 23012 | Information über Ergebnis an betroffener Marktlokation | MSB → LF/NB | Strom |
//!
//! ## Drei Fristen, und zwei davon hängen an der Messtechnik
//!
//! Neither window is stated per PID, and the message carries nothing that
//! decides them — the MSB's own device registry does.
//!
//! | Prozessschritt | Messtechnik | Strom | Gas |
//! |---|---|---|---|
//! | Antwort (23003/23004) | kME ohne RLM, mME | **3 WT** | **3 WT** |
//! | | kME mit RLM, iMS | **1 WT** | 3 WT ¹ |
//! | Mitteilung Ergebnis (23008) | kME ohne RLM, mME (NS), iMS ohne ¼-h (NS) | **7 WT** | **7 WT** |
//! | | kME mit RLM (NS), iMS mit ¼-h (NS) | **4 WT** | 7 WT ¹ |
//! | | kME mit RLM (MS/HS), iMS (MS/HS) | **2 WT** | 7 WT ¹ |
//! | Weiterleitung an betroffene MaLo (23011/23012) | — | **1 WT** | — |
//!
//! ¹ Gas states one flat number per Prozessschritt: it has no iMS rollout
//! obligation, so no Messtechnik branch. AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 2/4.
//!
//! The Ergebnisbericht window runs from the ÜT of the **Bestätigung**, not of
//! the Störungsmeldung, so the workflow arms it when it confirms — see
//! [`ERGEBNIS_WINDOW_LABEL`].
//!
//! The Weiterleitung an die betroffenen Marktlokationen (23011 nach der
//! Bestätigung, 23012 nach dem Ergebnisbericht, je 1 WT) is the one window a
//! terminal state does not close: the 23012 falls due *after* the Use-Case
//! ends. `weiterleitung_offen` tracks what is still owed, and only sending it
//! discharges the window.
//!
//! ## Regulatory basis
//!
//! - **BK6-22-024 Anlage 2b** — WiM Strom Teil 2 Kap. 1.2
//! - **AWH WiM Gas 2.0 Kap. 4.3** (gültig ab 01.10.2026)
//! - **INSRPT AHB 1.1g / MIG 1.1a** — EDI@Energy inspection report format

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MeLo, MessageRef, Sparte},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{HolidayCalendar, antwort::Messtechnik};
use time::OffsetDateTime;

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the INSRPT Störungsbehebung workflow.
pub const WORKFLOW_NAME: &str = "wim-insrpt";

/// The Störungsmeldung that opens the Use-Case (Melder → MSB), in both Sparten.
pub const INSRPT_ANFRAGE_PIDS: &[u32] = &[23001];

/// PIDs the MSB sends back, in **both Sparten**.
///
/// | PID | Meaning | Sparte |
/// |---|---|---|
/// | 23003 | Ablehnung der Störungsmeldung | beide |
/// | 23004 | Bestätigung der Störungsmeldung | beide |
/// | 23005 | Informationsmeldung über die Störung an den NB | Gas |
/// | 23008 | Ergebnisbericht — „Mitteilung Ergebnis" | beide |
/// | 23009 | Informationsmeldung über die Behebung an den NB | Gas |
/// | 23011 | Information über die Störung an betroffener Marktlokation | Strom |
/// | 23012 | Information über das Ergebnis an betroffener Marktlokation | Strom |
///
/// One workflow owns all of them: the INSRPT AHB is Sparte-neutral, and what
/// differs between the Sparten is the Frist, which
/// [`antwort_werktage`]/[`ergebnis_werktage`] take as an argument.
pub const INSRPT_ANTWORT_PIDS: &[u32] = &[23003, 23004, 23005, 23008, 23009, 23011, 23012];

/// The two PIDs that decide the Störungsmeldung — Ablehnung and Bestätigung.
pub const INSRPT_ENTSCHEIDUNGS_PIDS: &[u32] = &[23003, 23004];

/// The Ergebnisbericht that closes the Use-Case.
pub const INSRPT_ERGEBNIS_PID: u32 = 23008;

/// The accompanying messages that decide nothing: the Gas Informationsmeldungen
/// an den NB (23005/23009) and the Strom Weiterleitung an die betroffenen
/// Marktlokationen (23011/23012).
pub const INSRPT_INFORMATIONS_PIDS: &[u32] = &[23005, 23009, 23011, 23012];

/// Deadline label for the MSB's **Antwort** window (23003/23004).
///
/// Sized per Messtechnik and Sparte — see [`antwort_werktage`]. Never flat.
pub const ANTWORT_WINDOW_LABEL: &str = "wim-insrpt-antwort";

/// Deadline label for the MSB's **Ergebnisbericht** window (23008).
///
/// Counted from the ÜT of the MSB's own Bestätigung, **not** from the
/// Störungsmeldung (WiM Strom Teil 2 Kap. 1.2 Nr. 7: „spätester ÜT ist der n.
/// WT nach dem ÜT von Nr. 2"). Anchoring it on the Störungsmeldung shortens
/// every window by the answer time that has already elapsed.
pub const ERGEBNIS_WINDOW_LABEL: &str = "wim-insrpt-ergebnis";

/// Deadline label for the Weiterleitung an die betroffenen Marktlokationen
/// (23011/23012), one Werktag after the Information an die Messlokation.
pub const WEITERLEITUNG_WINDOW_LABEL: &str = "wim-insrpt-weiterleitung";

/// The Antwortfrist on a Störungsmeldung, in Werktagen.
///
/// Strom branches on the Messtechnik (WiM Strom Teil 2 Kap. 1.2 Nr. 2); Gas
/// states one number for every Messlokation (AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 2),
/// because it has no iMS rollout obligation to branch on.
#[must_use]
pub fn antwort_werktage(sparte: Sparte, messtechnik: Messtechnik) -> u32 {
    match sparte {
        Sparte::Strom => messtechnik.stoerungsmeldung_werktage(),
        Sparte::Gas => mako_fristen::antwort::STOERUNGSMELDUNG_KME_WERKTAGE,
    }
}

/// The Frist for the Ergebnisbericht, in Werktagen from the ÜT of the
/// Bestätigung.
///
/// Strom branches three ways on Messtechnik **and Spannungsebene** — the one
/// WiM window whose branch is the voltage level. Gas states a flat 7 WT.
#[must_use]
pub fn ergebnis_werktage(sparte: Sparte, messtechnik: Messtechnik) -> u32 {
    match sparte {
        Sparte::Strom => messtechnik.ergebnisbericht_werktage(),
        Sparte::Gas => mako_fristen::antwort::ERGEBNISBERICHT_GAS_WERKTAGE,
    }
}

/// A pending deadline `n` Werktage after `from`, at the 17:00 Europe/Berlin
/// MaKo cut-off.
fn window(label: &'static str, from: OffsetDateTime, werktage: u32) -> PendingDeadline {
    PendingDeadline::new(
        label,
        mako_fristen::deadline_at_werktage(from, werktage, HolidayCalendar::BdewMaKo),
    )
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Which side of the exchange this process stream is.
///
/// Both sides reach `Bestaetigt` — the MSB by sending the 23004, the Melder by
/// receiving it — and only the MSB owes the Ergebnisbericht that follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Seite {
    /// The LF or NB that reported the Störung.
    Melder,
    /// The MSB that owes the answer and the Ergebnisbericht.
    Msb,
}

/// Data captured when a Störungsmeldung opens the process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoerungsmeldungData {
    /// Which side of the exchange this deployment is on.
    pub seite: Seite,
    /// BDEW Prüfidentifikator of the opening INSRPT (23001).
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the Störungsmelder (LF or NB).
    pub melder_mp_id: MarktpartnerCode,
    /// GLN of the MSB that owes the answer.
    pub msb_mp_id: MarktpartnerCode,
    /// Messlokation the Störung concerns (`LOC+172`).
    pub melo_id: MeLo,
    /// Sparte the process runs in — it selects both Fristen.
    pub sparte: Sparte,
    /// EDIFACT document date.
    pub document_date: String,
    /// EDIFACT message reference of the Störungsmeldung.
    pub message_ref: MessageRef,
    /// The Weiterleitungen an die betroffenen Marktlokationen that are still
    /// owed — 23011 after the Bestätigung, 23012 after dem Ergebnisbericht,
    /// each within einem Werktag (WiM Teil 2 Kap. 1.2 Nr. 4–6 / 9–11).
    ///
    /// Strom only: Gas has no Weiterleitung, so the set stays empty there and
    /// no window is armed.
    #[serde(default)]
    pub weiterleitung_offen: Vec<u32>,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the INSRPT Störungsbehebung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StoerungsmeldungEvent {
    /// Melder side: INSRPT 23001 dispatched to the MSB.
    StoerungsmeldungGesendet(Box<StoerungsmeldungData>),
    /// MSB side: INSRPT 23001 received; the Antwortfrist runs.
    StoerungsmeldungEmpfangen(Box<StoerungsmeldungData>),
    /// MSB side: INSRPT 23003/23004 dispatched.
    AntwortGesendet {
        /// 23003 (Ablehnung) or 23004 (Bestätigung).
        pruefidentifikator: Pruefidentifikator,
        /// Outbound message reference.
        message_ref: MessageRef,
    },
    /// Melder side: INSRPT 23003/23004/23008 received.
    AntwortErhalten {
        /// Response PID.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the responding MSB.
        sender: MarktpartnerCode,
        /// `true` for Bestätigung (23004) and Ergebnisbericht (23008).
        is_confirmation: bool,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSB side: INSRPT 23008 dispatched — the Use-Case is closed.
    ErgebnisberichtGesendet {
        /// Outbound message reference.
        message_ref: MessageRef,
    },
    /// An accompanying INSRPT (23005/23009/23011/23012) crossed the wire.
    Informationsmeldung {
        /// PID.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the counterparty.
        counterparty: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
        /// `true` when mako sent it, `false` when it arrived.
        outbound: bool,
    },
    /// A window closed before the obligation it tracks was met.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for StoerungsmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StoerungsmeldungGesendet(_) => "InsrptStoerungsmeldungGesendet",
            Self::StoerungsmeldungEmpfangen(_) => "InsrptStoerungsmeldungEmpfangen",
            Self::AntwortGesendet { .. } => "InsrptAntwortGesendet",
            Self::AntwortErhalten { .. } => "InsrptAntwortErhalten",
            Self::ErgebnisberichtGesendet { .. } => "InsrptErgebnisberichtGesendet",
            Self::Informationsmeldung { .. } => "InsrptInformationsmeldung",
            Self::DeadlineExpired { .. } => "InsrptDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of an INSRPT Störungsbehebung process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum StoerungsmeldungState {
    /// No events yet.
    #[default]
    New,
    /// Melder side: 23001 sent, awaiting the MSB's decision.
    StoerungsmeldungGesendet(Box<StoerungsmeldungData>),
    /// MSB side: 23001 received, the Antwort is owed.
    StoerungsmeldungEmpfangen(Box<StoerungsmeldungData>),
    /// Confirmed (23004). **Not terminal** — the Ergebnisbericht follows, and
    /// closing here dropped every 23008 the MSB sent afterwards.
    Bestaetigt(Box<StoerungsmeldungData>),
    /// Rejected (23003) — terminal.
    Abgelehnt(Box<StoerungsmeldungData>),
    /// Ergebnisbericht exchanged (23008) — terminal.
    Ergebnisbericht(Box<StoerungsmeldungData>),
    /// A window expired before its obligation was met — terminal.
    DeadlineExpired {
        /// Label of the expired deadline.
        label: String,
    },
}

impl StoerungsmeldungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::StoerungsmeldungGesendet(_) => "StoerungsmeldungGesendet",
            Self::StoerungsmeldungEmpfangen(_) => "StoerungsmeldungEmpfangen",
            Self::Bestaetigt(_) => "Bestaetigt",
            Self::Abgelehnt(_) => "Abgelehnt",
            Self::Ergebnisbericht(_) => "Ergebnisbericht",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Returns `true` if this is a terminal state.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Abgelehnt(_) | Self::Ergebnisbericht(_) | Self::DeadlineExpired { .. }
        )
    }

    /// The process data, where the state carries any.
    #[must_use]
    pub const fn data(&self) -> Option<&StoerungsmeldungData> {
        match self {
            Self::StoerungsmeldungGesendet(d)
            | Self::StoerungsmeldungEmpfangen(d)
            | Self::Bestaetigt(d)
            | Self::Abgelehnt(d)
            | Self::Ergebnisbericht(d) => Some(d),
            Self::New | Self::DeadlineExpired { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the INSRPT Störungsbehebung workflow.
#[derive(Clone)]
pub enum StoerungsmeldungCommand {
    /// Melder side: dispatch a Störungsmeldung (23001) to the MSB.
    SendStoerungsmeldung {
        /// Prüfidentifikator (always 23001).
        pid: Pruefidentifikator,
        /// GLN of the receiving MSB.
        msb_mp_id: MarktpartnerCode,
        /// Messlokation the Störung concerns.
        ///
        /// The INSRPT AHB marks `LOC` mandatory, so a Störungsmeldung without
        /// a Messlokation cannot be rendered into a valid interchange.
        melo_id: MeLo,
        /// Sparte, which selects both Fristen.
        sparte: Sparte,
        /// Document date.
        document_date: String,
        /// Message reference of the outbound INSRPT.
        message_ref: MessageRef,
    },
    /// MSB side: an inbound Störungsmeldung (23001) opens the process.
    ReceiveStoerungsmeldung {
        /// Prüfidentifikator (always 23001).
        pid: Pruefidentifikator,
        /// GLN of the Störungsmelder.
        melder_mp_id: MarktpartnerCode,
        /// GLN this deployment received it on.
        msb_mp_id: MarktpartnerCode,
        /// Messlokation the Störung concerns.
        melo_id: MeLo,
        /// Sparte, resolved from the recipient MP-ID.
        sparte: Sparte,
        /// Document date.
        document_date: String,
        /// Message reference of the inbound INSRPT.
        message_ref: MessageRef,
        /// Arrival instant, from which the Antwortfrist runs.
        received_at: OffsetDateTime,
        /// Messtechnik at the Messlokation — the MSB's own registry decides it,
        /// and it is what sizes both windows.
        messtechnik: Messtechnik,
    },
    /// MSB side: answer the Störungsmeldung with 23003 or 23004.
    DispatchAntwort {
        /// 23003 (Ablehnung) or 23004 (Bestätigung).
        pid: Pruefidentifikator,
        /// `STS` code carried by the answer, where the ERP supplies one.
        status_code: Option<String>,
        /// Outbound message reference.
        message_ref: MessageRef,
        /// Dispatch instant, from which the Ergebnisfrist runs on a Bestätigung.
        sent_at: OffsetDateTime,
        /// Messtechnik at the Messlokation, sizing the Ergebnisfrist.
        messtechnik: Messtechnik,
    },
    /// MSB side: close the Use-Case with the Ergebnisbericht (23008).
    DispatchErgebnisbericht {
        /// Outbound message reference.
        message_ref: MessageRef,
        /// `STS` code carried by the report, where the ERP supplies one.
        status_code: Option<String>,
    },
    /// MSB side: send an accompanying Informationsmeldung
    /// (23005/23009 Gas, 23011/23012 Strom).
    DispatchInformationsmeldung {
        /// PID.
        pid: Pruefidentifikator,
        /// GLN of the receiving NB or Marktlokations-LF.
        receiver: MarktpartnerCode,
        /// Outbound message reference.
        message_ref: MessageRef,
        /// `STS` code carried by the message, where the ERP supplies one.
        status_code: Option<String>,
    },
    /// Melder side: an inbound INSRPT answer (23003/23004/23008).
    ReceiveAntwort {
        /// Response PID.
        pid: Pruefidentifikator,
        /// GLN of the MSB.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// An inbound accompanying INSRPT (23005/23009/23011/23012).
    ReceiveInformationsmeldung {
        /// PID.
        pid: Pruefidentifikator,
        /// GLN of the sending MSB.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// A window expired.
    TimeoutExpired {
        /// Unique ID.
        deadline_id: DeadlineId,
        /// Label.
        label: Box<str>,
    },
}

impl CommandPayload for StoerungsmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// INSRPT Störungsbehebung workflow — both the Melder and the MSB side.
pub struct WimInsrptWorkflow;

impl WimInsrptWorkflow {
    /// The outbox payload for an outbound INSRPT of this process.
    ///
    /// Keys follow the `render_insrpt` contract: `melo` feeds the mandatory
    /// `LOC+172`, without which the interchange parses but fails AHB validation.
    fn outbox(
        data: &StoerungsmeldungData,
        pid: u32,
        receiver: &MarktpartnerCode,
        message_ref: &MessageRef,
        status_code: Option<&str>,
    ) -> PendingOutbox {
        let mut payload = serde_json::json!({
            "type":          "Stoerungsmeldung",
            "pid":           pid,
            "melo":          data.melo_id.as_str(),
            "receiver":      receiver.as_str(),
            "document_date": data.document_date,
            "message_ref":   message_ref.as_str(),
        });
        if let Some(code) = status_code {
            payload["status_code"] = serde_json::Value::String(code.to_owned());
        }
        PendingOutbox::new("INSRPT", receiver.as_str(), payload)
    }
}

impl Workflow for WimInsrptWorkflow {
    type State = StoerungsmeldungState;
    type Event = StoerungsmeldungEvent;
    type Command = StoerungsmeldungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        // The Weiterleitung is the one window a *terminal* state does not close:
        // the Ergebnisbericht ends the Use-Case and still owes the 23012. What
        // closes it is the Weiterleitung going out.
        let open = if deadline.label() == WEITERLEITUNG_WINDOW_LABEL {
            state
                .data()
                .is_some_and(|d| !d.weiterleitung_offen.is_empty())
        } else {
            matches!(
                deadline.label(),
                ANTWORT_WINDOW_LABEL | ERGEBNIS_WINDOW_LABEL
            ) && !state.is_terminal()
        };
        open.then(|| StoerungsmeldungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            StoerungsmeldungEvent::StoerungsmeldungGesendet(d) => {
                StoerungsmeldungState::StoerungsmeldungGesendet(d.clone())
            }
            StoerungsmeldungEvent::StoerungsmeldungEmpfangen(d) => {
                StoerungsmeldungState::StoerungsmeldungEmpfangen(d.clone())
            }
            StoerungsmeldungEvent::AntwortGesendet {
                pruefidentifikator, ..
            } => match state {
                StoerungsmeldungState::StoerungsmeldungEmpfangen(mut d) => {
                    if pruefidentifikator.as_u32() == 23_004 {
                        if d.sparte == Sparte::Strom {
                            d.weiterleitung_offen.push(23_011);
                        }
                        StoerungsmeldungState::Bestaetigt(d)
                    } else {
                        StoerungsmeldungState::Abgelehnt(d)
                    }
                }
                other => other,
            },
            StoerungsmeldungEvent::AntwortErhalten {
                pruefidentifikator,
                is_confirmation,
                ..
            } => match state {
                StoerungsmeldungState::StoerungsmeldungGesendet(d)
                | StoerungsmeldungState::Bestaetigt(d) => {
                    if pruefidentifikator.as_u32() == INSRPT_ERGEBNIS_PID {
                        StoerungsmeldungState::Ergebnisbericht(d)
                    } else if *is_confirmation {
                        StoerungsmeldungState::Bestaetigt(d)
                    } else {
                        StoerungsmeldungState::Abgelehnt(d)
                    }
                }
                other => other,
            },
            StoerungsmeldungEvent::ErgebnisberichtGesendet { .. } => match state {
                StoerungsmeldungState::Bestaetigt(mut d)
                | StoerungsmeldungState::StoerungsmeldungEmpfangen(mut d) => {
                    if d.sparte == Sparte::Strom {
                        d.weiterleitung_offen.push(23_012);
                    }
                    StoerungsmeldungState::Ergebnisbericht(d)
                }
                other => other,
            },
            // Accompanying messages decide nothing, but an outbound one
            // discharges the Weiterleitung the window tracks.
            StoerungsmeldungEvent::Informationsmeldung {
                pruefidentifikator,
                outbound: true,
                ..
            } => {
                let pid = pruefidentifikator.as_u32();
                match state {
                    StoerungsmeldungState::Bestaetigt(mut d) => {
                        d.weiterleitung_offen.retain(|p| *p != pid);
                        StoerungsmeldungState::Bestaetigt(d)
                    }
                    StoerungsmeldungState::Ergebnisbericht(mut d) => {
                        d.weiterleitung_offen.retain(|p| *p != pid);
                        StoerungsmeldungState::Ergebnisbericht(d)
                    }
                    other => other,
                }
            }
            StoerungsmeldungEvent::Informationsmeldung { .. } => state,
            StoerungsmeldungEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    StoerungsmeldungState::DeadlineExpired {
                        label: label.to_string(),
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
            StoerungsmeldungCommand::SendStoerungsmeldung {
                pid,
                msb_mp_id,
                melo_id,
                sparte,
                document_date,
                message_ref,
            } => {
                if !matches!(state, StoerungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !INSRPT_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected INSRPT PID 23001 for Störungsmeldung, got {pid}",
                    )));
                }
                let data = StoerungsmeldungData {
                    seite: Seite::Melder,
                    pruefidentifikator: pid,
                    // The Melder is this deployment; the renderer fills its own
                    // MP-ID from the party registry.
                    melder_mp_id: MarktpartnerCode::new(""),
                    msb_mp_id: msb_mp_id.clone(),
                    melo_id,
                    sparte,
                    document_date,
                    message_ref: message_ref.clone(),
                    weiterleitung_offen: Vec::new(),
                };
                let outbox = Self::outbox(&data, pid.as_u32(), &msb_mp_id, &message_ref, None);
                Ok(WorkflowOutput::with_outbox(
                    vec![StoerungsmeldungEvent::StoerungsmeldungGesendet(Box::new(
                        data,
                    ))],
                    vec![outbox],
                ))
            }

            StoerungsmeldungCommand::ReceiveStoerungsmeldung {
                pid,
                melder_mp_id,
                msb_mp_id,
                melo_id,
                sparte,
                document_date,
                message_ref,
                received_at,
                messtechnik,
            } => {
                if !matches!(state, StoerungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !INSRPT_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected INSRPT PID 23001 for Störungsmeldung, got {pid}",
                    )));
                }
                let data = StoerungsmeldungData {
                    seite: Seite::Msb,
                    pruefidentifikator: pid,
                    melder_mp_id,
                    msb_mp_id,
                    melo_id,
                    sparte,
                    document_date,
                    message_ref,
                    weiterleitung_offen: Vec::new(),
                };
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![StoerungsmeldungEvent::StoerungsmeldungEmpfangen(Box::new(
                        data,
                    ))],
                    vec![],
                    vec![window(
                        ANTWORT_WINDOW_LABEL,
                        received_at,
                        antwort_werktage(sparte, messtechnik),
                    )],
                ))
            }

            StoerungsmeldungCommand::DispatchAntwort {
                pid,
                status_code,
                message_ref,
                sent_at,
                messtechnik,
            } => {
                let StoerungsmeldungState::StoerungsmeldungEmpfangen(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "StoerungsmeldungEmpfangen",
                        state.label(),
                    ));
                };
                if !INSRPT_ENTSCHEIDUNGS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "the Störungsmeldung is answered with INSRPT 23003 or 23004, got {pid}",
                    )));
                }
                let outbox = Self::outbox(
                    data,
                    pid.as_u32(),
                    &data.melder_mp_id,
                    &message_ref,
                    status_code.as_deref(),
                );
                let events = vec![StoerungsmeldungEvent::AntwortGesendet {
                    pruefidentifikator: pid,
                    message_ref,
                }];
                // Only the Bestätigung opens the Ergebnisfrist: an Ablehnung
                // ends the Use-Case and owes no report.
                if pid.as_u32() == 23_004 {
                    let mut deadlines = vec![window(
                        ERGEBNIS_WINDOW_LABEL,
                        sent_at,
                        ergebnis_werktage(data.sparte, messtechnik),
                    )];
                    // Strom forwards the Störung to every affected Marktlokation
                    // within einem Werktag; Gas publishes no such step.
                    if data.sparte == Sparte::Strom {
                        deadlines.push(window(
                            WEITERLEITUNG_WINDOW_LABEL,
                            sent_at,
                            mako_fristen::antwort::STOERUNG_WEITERLEITUNG_WERKTAGE,
                        ));
                    }
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![outbox],
                        deadlines,
                    ))
                } else {
                    Ok(WorkflowOutput::with_outbox(events, vec![outbox]))
                }
            }

            StoerungsmeldungCommand::DispatchErgebnisbericht {
                message_ref,
                status_code,
            } => {
                let StoerungsmeldungState::Bestaetigt(data) = state else {
                    return Err(WorkflowError::invalid_state("Bestaetigt", state.label()));
                };
                if data.seite != Seite::Msb {
                    return Err(WorkflowError::rejected(
                        "only the MSB sends the Ergebnisbericht; this stream is the Melder's",
                    ));
                }
                let outbox = Self::outbox(
                    data,
                    INSRPT_ERGEBNIS_PID,
                    &data.melder_mp_id,
                    &message_ref,
                    status_code.as_deref(),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![StoerungsmeldungEvent::ErgebnisberichtGesendet { message_ref }],
                    vec![outbox],
                ))
            }

            StoerungsmeldungCommand::DispatchInformationsmeldung {
                pid,
                receiver,
                message_ref,
                status_code,
            } => {
                let Some(data) = state.data() else {
                    return Err(WorkflowError::invalid_state(
                        "an open Störungsmeldung",
                        state.label(),
                    ));
                };
                if !INSRPT_INFORMATIONS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not an INSRPT Informationsmeldung",
                    )));
                }
                if data.seite != Seite::Msb {
                    return Err(WorkflowError::rejected(
                        "only the MSB sends an Informationsmeldung; this stream is the Melder's",
                    ));
                }
                let outbox = Self::outbox(
                    data,
                    pid.as_u32(),
                    &receiver,
                    &message_ref,
                    status_code.as_deref(),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![StoerungsmeldungEvent::Informationsmeldung {
                        pruefidentifikator: pid,
                        counterparty: receiver,
                        message_ref,
                        outbound: true,
                    }],
                    vec![outbox],
                ))
            }

            StoerungsmeldungCommand::ReceiveAntwort {
                pid,
                sender,
                message_ref,
            } => {
                if !INSRPT_ANTWORT_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a handled INSRPT response PID",
                    )));
                }
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                // 23004 is the Bestätigung and 23008 the Ergebnisbericht that
                // follows it. 23005/23009 are the Gas Informationsmeldungen the
                // MSB sends the NB alongside — they carry no decision at all,
                // and reading them as a confirmation closed the process before
                // the Störung was answered.
                if INSRPT_INFORMATIONS_PIDS.contains(&pid.as_u32()) {
                    return Ok(vec![StoerungsmeldungEvent::Informationsmeldung {
                        pruefidentifikator: pid,
                        counterparty: sender,
                        message_ref,
                        outbound: false,
                    }]
                    .into());
                }
                let is_confirmation = matches!(pid.as_u32(), 23_004 | INSRPT_ERGEBNIS_PID);
                Ok(vec![StoerungsmeldungEvent::AntwortErhalten {
                    pruefidentifikator: pid,
                    sender,
                    is_confirmation,
                    message_ref,
                }]
                .into())
            }

            StoerungsmeldungCommand::ReceiveInformationsmeldung {
                pid,
                sender,
                message_ref,
            } => Ok(vec![StoerungsmeldungEvent::Informationsmeldung {
                pruefidentifikator: pid,
                counterparty: sender,
                message_ref,
                outbound: false,
            }]
            .into()),

            StoerungsmeldungCommand::TimeoutExpired { deadline_id, label } => {
                let weiterleitung_offen = &*label == WEITERLEITUNG_WINDOW_LABEL
                    && state
                        .data()
                        .is_some_and(|d| !d.weiterleitung_offen.is_empty());
                if state.is_terminal() && !weiterleitung_offen {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![StoerungsmeldungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }

    fn now() -> OffsetDateTime {
        time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, time::Month::June, 15).expect("valid date"),
            time::Time::from_hms(9, 0, 0).expect("valid time"),
        )
        .assume_utc()
    }

    fn receive(sparte: Sparte, messtechnik: Messtechnik) -> StoerungsmeldungCommand {
        StoerungsmeldungCommand::ReceiveStoerungsmeldung {
            pid: pid(23_001),
            melder_mp_id: MarktpartnerCode::new("9900001000001"),
            msb_mp_id: MarktpartnerCode::new("9900001000003"),
            melo_id: MeLo::new("DE0001112223334445556667778889"),
            sparte,
            document_date: "20260615".into(),
            message_ref: MessageRef::new("INS-1"),
            received_at: now(),
            messtechnik,
        }
    }

    fn run(
        state: StoerungsmeldungState,
        cmd: StoerungsmeldungCommand,
    ) -> (StoerungsmeldungState, WorkflowOutput<StoerungsmeldungEvent>) {
        let out = WimInsrptWorkflow::handle(&state, cmd).expect("command accepted");
        let next = out.events.iter().fold(state, WimInsrptWorkflow::apply);
        (next, out)
    }

    /// An inbound 23001 reaches the MSB side and arms the Antwortfrist there.
    #[test]
    fn the_msb_receives_the_stoerungsmeldung_and_owes_an_answer() {
        let (state, out) = run(
            StoerungsmeldungState::default(),
            receive(Sparte::Strom, Messtechnik::RlmOderImsMsHs),
        );
        assert_eq!(state.label(), "StoerungsmeldungEmpfangen");
        assert!(!state.is_terminal());
        let labels: Vec<_> = out.deadlines.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec![ANTWORT_WINDOW_LABEL]);
    }

    /// Only the Bestätigung opens the Ergebnisfrist; the Ablehnung ends the
    /// Use-Case and owes no report.
    #[test]
    fn only_the_bestaetigung_arms_the_ergebnisfrist() {
        let (empfangen, _) = run(
            StoerungsmeldungState::default(),
            receive(Sparte::Strom, Messtechnik::KmeOhneRlm),
        );
        let antwort = |p: u32| StoerungsmeldungCommand::DispatchAntwort {
            pid: pid(p),
            status_code: None,
            message_ref: MessageRef::new("INS-2"),
            sent_at: now(),
            messtechnik: Messtechnik::KmeOhneRlm,
        };

        let (bestaetigt, out) = run(empfangen.clone(), antwort(23_004));
        assert_eq!(bestaetigt.label(), "Bestaetigt");
        assert!(
            !bestaetigt.is_terminal(),
            "the Ergebnisbericht still follows"
        );
        assert_eq!(
            out.deadlines
                .iter()
                .map(|d| d.label.as_str())
                .collect::<Vec<_>>(),
            vec![ERGEBNIS_WINDOW_LABEL, WEITERLEITUNG_WINDOW_LABEL]
        );

        let (abgelehnt, out) = run(empfangen, antwort(23_003));
        assert_eq!(abgelehnt.label(), "Abgelehnt");
        assert!(abgelehnt.is_terminal());
        assert!(out.deadlines.is_empty());
    }

    /// A Melder-side stream that reached `Bestaetigt` by *receiving* the 23004
    /// must not go on to send the Ergebnisbericht — it has no MSB code to
    /// address it to.
    #[test]
    fn only_the_msb_side_sends_the_ergebnisbericht() {
        let (gesendet, _) = run(
            StoerungsmeldungState::default(),
            StoerungsmeldungCommand::SendStoerungsmeldung {
                pid: pid(23_001),
                msb_mp_id: MarktpartnerCode::new("9900001000003"),
                melo_id: MeLo::new("DE0001112223334445556667778889"),
                sparte: Sparte::Strom,
                document_date: "20260615".into(),
                message_ref: MessageRef::new("INS-1"),
            },
        );
        let (bestaetigt, _) = run(
            gesendet,
            StoerungsmeldungCommand::ReceiveAntwort {
                pid: pid(23_004),
                sender: MarktpartnerCode::new("9900001000003"),
                message_ref: MessageRef::new("INS-3"),
            },
        );
        assert_eq!(bestaetigt.label(), "Bestaetigt");
        assert!(
            WimInsrptWorkflow::handle(
                &bestaetigt,
                StoerungsmeldungCommand::DispatchErgebnisbericht {
                    message_ref: MessageRef::new("INS-5"),
                    status_code: None,
                },
            )
            .is_err()
        );
    }

    /// The Bestätigung must not close the process: the 23008 that follows it
    /// is the one that does.
    #[test]
    fn the_ergebnisbericht_closes_the_use_case_not_the_bestaetigung() {
        let (state, _) = run(
            StoerungsmeldungState::default(),
            StoerungsmeldungCommand::SendStoerungsmeldung {
                pid: pid(23_001),
                msb_mp_id: MarktpartnerCode::new("9900001000003"),
                melo_id: MeLo::new("DE0001112223334445556667778889"),
                sparte: Sparte::Strom,
                document_date: "20260615".into(),
                message_ref: MessageRef::new("INS-1"),
            },
        );
        let recv = |p: u32| StoerungsmeldungCommand::ReceiveAntwort {
            pid: pid(p),
            sender: MarktpartnerCode::new("9900001000003"),
            message_ref: MessageRef::new("INS-3"),
        };
        let (bestaetigt, _) = run(state, recv(23_004));
        assert_eq!(bestaetigt.label(), "Bestaetigt");
        assert!(!bestaetigt.is_terminal());
        let (ergebnis, out) = run(bestaetigt, recv(23_008));
        assert_eq!(ergebnis.label(), "Ergebnisbericht");
        assert!(ergebnis.is_terminal());
        assert_eq!(out.events.len(), 1);
    }

    /// The Weiterleitung an die betroffenen Marktlokationen outlives the
    /// Ergebnisbericht: the 23012 is owed *after* the Use-Case closes, so a
    /// terminal state must not silence its window.
    #[test]
    fn the_weiterleitung_window_survives_the_ergebnisbericht() {
        let (empfangen, _) = run(
            StoerungsmeldungState::default(),
            receive(Sparte::Strom, Messtechnik::KmeOhneRlm),
        );
        let (bestaetigt, out) = run(
            empfangen,
            StoerungsmeldungCommand::DispatchAntwort {
                pid: pid(23_004),
                status_code: None,
                message_ref: MessageRef::new("INS-2"),
                sent_at: now(),
                messtechnik: Messtechnik::KmeOhneRlm,
            },
        );
        assert!(
            out.deadlines
                .iter()
                .any(|d| d.label == WEITERLEITUNG_WINDOW_LABEL)
        );
        assert_eq!(
            bestaetigt.data().map(|d| d.weiterleitung_offen.as_slice()),
            Some([23_011].as_slice())
        );

        // Sending the 23011 discharges it; the window then goes quiet.
        let (nach_23011, _) = run(
            bestaetigt.clone(),
            StoerungsmeldungCommand::DispatchInformationsmeldung {
                pid: pid(23_011),
                receiver: MarktpartnerCode::new("9900001000001"),
                message_ref: MessageRef::new("INS-6"),
                status_code: None,
            },
        );
        assert_eq!(
            nach_23011.data().map(|d| d.weiterleitung_offen.len()),
            Some(0)
        );

        // The Ergebnisbericht closes the Use-Case and opens the 23012 debt,
        // which a terminal state must not hide.
        let (ergebnis, _) = run(
            nach_23011,
            StoerungsmeldungCommand::DispatchErgebnisbericht {
                message_ref: MessageRef::new("INS-7"),
                status_code: None,
            },
        );
        assert!(ergebnis.is_terminal());
        assert_eq!(
            ergebnis.data().map(|d| d.weiterleitung_offen.as_slice()),
            Some([23_012].as_slice())
        );
    }

    /// Gas publishes no Weiterleitung, so no window is armed there.
    #[test]
    fn gas_arms_no_weiterleitung_window() {
        let (empfangen, _) = run(
            StoerungsmeldungState::default(),
            receive(Sparte::Gas, Messtechnik::KmeOhneRlm),
        );
        let (_, out) = run(
            empfangen,
            StoerungsmeldungCommand::DispatchAntwort {
                pid: pid(23_004),
                status_code: None,
                message_ref: MessageRef::new("INS-2"),
                sent_at: now(),
                messtechnik: Messtechnik::KmeOhneRlm,
            },
        );
        assert!(
            !out.deadlines
                .iter()
                .any(|d| d.label == WEITERLEITUNG_WINDOW_LABEL)
        );
    }

    /// The Gas Informationsmeldungen an den NB and the Strom Weiterleitung
    /// decide nothing and must never move the state.
    #[test]
    fn accompanying_messages_do_not_decide_the_process() {
        let (empfangen, _) = run(
            StoerungsmeldungState::default(),
            receive(Sparte::Gas, Messtechnik::KmeOhneRlm),
        );
        for p in INSRPT_INFORMATIONS_PIDS {
            let (after, _) = run(
                empfangen.clone(),
                StoerungsmeldungCommand::ReceiveAntwort {
                    pid: pid(*p),
                    sender: MarktpartnerCode::new("9900001000003"),
                    message_ref: MessageRef::new("INS-4"),
                },
            );
            assert_eq!(after.label(), "StoerungsmeldungEmpfangen", "PID {p}");
        }
    }

    /// Gas states one flat number per Prozessschritt; Strom branches.
    #[test]
    fn gas_does_not_branch_on_messtechnik() {
        for mt in [
            Messtechnik::KmeOhneRlm,
            Messtechnik::RlmOderImsNs,
            Messtechnik::RlmOderImsMsHs,
        ] {
            assert_eq!(antwort_werktage(Sparte::Gas, mt), 3);
            assert_eq!(ergebnis_werktage(Sparte::Gas, mt), 7);
        }
        assert_eq!(antwort_werktage(Sparte::Strom, Messtechnik::KmeOhneRlm), 3);
        assert_eq!(
            antwort_werktage(Sparte::Strom, Messtechnik::RlmOderImsMsHs),
            1
        );
        assert_eq!(
            ergebnis_werktage(Sparte::Strom, Messtechnik::RlmOderImsMsHs),
            2
        );
    }
}
