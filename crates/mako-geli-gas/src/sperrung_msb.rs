//! Sperrung Gas — gMSB side: the GNB's Anfrage Sperrung (ORDERS 17116).
//!
//! The GNB asks the gMSB to agree to the Sperrung and the later Entsperrung
//! (Anwendungsübersicht der Prüfidentifikatoren 4.0 lfd. Nr. 12530–12550,
//! „AWH Sperrprozesse", NB an MSB). The gMSB answers with ORDRSP 19118
//! (Bestätigung) or 19119 (Ablehnung) out of `E_1001` „Anfrage prüfen (Basiert
//! auf Strom EBD: E_0488_Anfrage prüfen)" (EBD 4.3 Kap. 18.2.2, S. 814–815):
//! `BGM+Z51`, `SG1 RFF+ON` the 17116's `BGM` DE 1004 (Hinweis `[529]`), `SG2
//! AJT` the code with „EBD Nr. E_1001" (`[493]`, recipient of the Sparte Gas),
//! and beside `A02`/`A03` the problem named in `SG2 FTX+AAP` (`[43]`) — ORDRSP
//! AHB 1.1b Kap. 4.3.1, S. 17–18.
//!
//! | PID   | Message                       | Direction  |
//! |-------|-------------------------------|------------|
//! | 17116 | Anfrage Sperrung              | GNB → gMSB |
//! | 19118 | Bestätigung Anfrage Sperrung  | gMSB → GNB |
//! | 19119 | Ablehnung Anfrage Sperrung    | gMSB → GNB |
//!
//! # No window
//!
//! The Frist of the gMSB's answer is set by the BDEW *Anwendungshilfe
//! Sperrprozesse Gas*, which is not mirrored: the process registers no
//! deadline, and nothing infers the Strom rule's „gilt als Zustimmung" lapse
//! (GPKE Teil 2 Kap. 3.5.1.2 Nr. 4) for Gas.
//!
//! # Why its own workflow
//!
//! The GNB's [`crate::sperrung_nb`] process runs on the same Marktlokation; a
//! deployment that is GNB and gMSB at once would otherwise resume its own
//! Sperrauftrag with the gMSB's copy of the Anfrage.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Workflow name for the gMSB side of the Anfrage Sperrung Gas.
pub const WORKFLOW_NAME: &str = "geli-gas-sperrung-msb";

/// ORDERS 17116 „Anfrage Sperrung", GNB → gMSB.
pub const ANFRAGE_PID: u32 = 17_116;

/// ORDRSP 19118 „Bestätigung Anfrage Sperrung".
pub const BESTAETIGUNG_PID: u32 = 19_118;

/// ORDRSP 19119 „Ablehnung Anfrage Sperrung".
pub const ABLEHNUNG_PID: u32 = 19_119;

/// `BGM` DE 1001 of the answer — `Z51` „Sperrung", the only value the 19118 /
/// 19119 columns admit (ORDRSP AHB 1.1b Kap. 4.3.1, S. 17).
pub const BGM_SPERRUNG: &str = "Z51";

/// The Anfrage as the gMSB received it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnfrageSperrungGas {
    /// The GNB asking — `NAD+MS`.
    pub netzbetreiber: MarktpartnerCode,
    /// The gMSB asked — `NAD+MR`; the answer's sender.
    pub msb: MarktpartnerCode,
    /// The Marktlokation — `SG2 LOC+172`.
    pub malo: MaLo,
    /// The ORDERS' `BGM` DE 1004, echoed in the answer's `SG1 RFF+ON`.
    pub dokumentennummer: String,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
}

/// The gMSB's answer as sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AntwortSperrungGas {
    /// 19118 or 19119 — picked by the code's Cluster.
    pub pid: Pruefidentifikator,
    /// The `E_1001` code.
    pub antwortcode: String,
    /// Reference of the outbound ORDRSP.
    pub message_ref: MessageRef,
}

/// Events of the gMSB-side Anfrage Sperrung.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SperrungMsbGasEvent {
    /// ORDERS 17116 received.
    AnfrageEingegangen {
        /// ORDERS Prüfidentifikator (17116).
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: AnfrageSperrungGas,
    },
    /// The ORDERS failed its AHB check.
    AnfrageUngueltig {
        /// The findings.
        reason: String,
    },
    /// The gMSB answered with ORDRSP 19118 / 19119.
    AntwortGesendet {
        /// The answer.
        antwort: AntwortSperrungGas,
        /// The tree the code was read from (`E_1001`).
        codeliste: String,
    },
}

impl EventPayload for SperrungMsbGasEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageEingegangen { .. } => "SperrungMsbGasAnfrageEingegangen",
            Self::AnfrageUngueltig { .. } => "SperrungMsbGasAnfrageUngueltig",
            Self::AntwortGesendet { .. } => "SperrungMsbGasAntwortGesendet",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageEingegangen { pid, anfrage } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(anfrage.message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet { antwort, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(antwort.pid.as_u32()),
                message_ref: Some(antwort.message_ref.as_str().to_owned()),
            }),
            Self::AnfrageUngueltig { .. } => None,
        }
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortGesendet { antwort, codeliste } => Some(Antwortcode {
                code: antwort.antwortcode.clone(),
                codeliste: Some(codeliste.clone()),
            }),
            _ => None,
        }
    }
}

/// State of the gMSB-side Anfrage Sperrung.
///
/// ```text
/// New → Offen → Beantwortet   (ORDRSP 19118 / 19119 sent)
///     ↘ Ungueltig             (AHB check failed)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum SperrungMsbGasState {
    /// Nothing received yet.
    #[default]
    New,
    /// The Anfrage is open; the gMSB owes its answer.
    Offen(AnfrageSperrungGas),
    /// The Anfrage failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// The gMSB answered.
    Beantwortet {
        /// The Anfrage.
        anfrage: AnfrageSperrungGas,
        /// The answer.
        antwort: AntwortSperrungGas,
    },
}

impl SperrungMsbGasState {
    /// Stable label of the variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Offen(_) => "Offen",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::Beantwortet { .. } => "Beantwortet",
        }
    }

    /// Whether the process has ended.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ungueltig { .. } | Self::Beantwortet { .. })
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for SperrungMsbGasState {
    /// An open Anfrage holds the Marktlokation; a settled one does not.
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::Offen(_))
    }
}

/// Commands of the gMSB-side Anfrage Sperrung.
#[derive(Debug, Clone)]
pub enum SperrungMsbGasCommand {
    /// Inbound ORDERS 17116.
    ReceiveAnfrage {
        /// Prüfidentifikator read from `SG1 RFF+Z13`.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: AnfrageSperrungGas,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// The gMSB answers out of `E_1001`.
    SendAntwort {
        /// The `E_1001` code; its Cluster picks 19118 or 19119.
        antwortcode: String,
        /// `SG2 FTX+AAP` — the problem named, Muss beside `A02`/`A03`.
        freitext: Option<String>,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
}

impl CommandPayload for SperrungMsbGasCommand {}

/// The answer PID for `antwortcode` out of `E_1001`.
fn antwort_pid(antwortcode: &str, freitext: Option<&str>) -> Result<u32, WorkflowError> {
    let tree = mako_pruefung::codes::EBD_ANFRAGE_SPERRUNG_MSB_GAS;
    let code = mako_pruefung::codes::lookup(tree, antwortcode).ok_or_else(|| {
        WorkflowError::validation(format!(
            "{antwortcode:?} is not published in {tree} (EBD 4.3 Kap. 18.2.2)"
        ))
    })?;
    if code.braucht_bemerkung && freitext.is_none_or(|t| t.trim().is_empty()) {
        return Err(WorkflowError::validation(format!(
            "{antwortcode:?} of {tree} requires the problem named — SG2 FTX+AAP is Muss beside \
             it (ORDRSP AHB 1.1b Kap. 4.3.1, [43])"
        )));
    }
    match code.ist_zustimmung() {
        Some(true) => Ok(BESTAETIGUNG_PID),
        Some(false) => Ok(ABLEHNUNG_PID),
        None => Err(WorkflowError::validation(format!(
            "{antwortcode:?} of {tree} is neither a Zustimmung nor an Ablehnung"
        ))),
    }
}

/// The gMSB side of the Anfrage Sperrung Gas (ORDERS 17116).
pub struct GeliGasSperrungMsbWorkflow;

impl Workflow for GeliGasSperrungMsbWorkflow {
    type State = SperrungMsbGasState;
    type Event = SperrungMsbGasEvent;
    type Command = SperrungMsbGasCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SperrungMsbGasEvent::AnfrageEingegangen { anfrage, .. } => {
                SperrungMsbGasState::Offen(anfrage.clone())
            }
            SperrungMsbGasEvent::AnfrageUngueltig { reason } => SperrungMsbGasState::Ungueltig {
                reason: reason.clone(),
            },
            SperrungMsbGasEvent::AntwortGesendet { antwort, .. } => match state {
                SperrungMsbGasState::Offen(anfrage) => SperrungMsbGasState::Beantwortet {
                    anfrage,
                    antwort: antwort.clone(),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            SperrungMsbGasCommand::ReceiveAnfrage {
                pid,
                anfrage,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, SperrungMsbGasState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != ANFRAGE_PID {
                    return Err(WorkflowError::rejected(format!(
                        "expected the Anfrage Sperrung {ANFRAGE_PID}, got {pid}"
                    )));
                }
                let mut events = vec![SperrungMsbGasEvent::AnfrageEingegangen { pid, anfrage }];
                if !validation_passed {
                    events.push(SperrungMsbGasEvent::AnfrageUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
            SperrungMsbGasCommand::SendAntwort {
                antwortcode,
                freitext,
                message_ref,
            } => {
                let SperrungMsbGasState::Offen(anfrage) = state else {
                    return Err(WorkflowError::invalid_state("Offen", state.label()));
                };
                let pid = antwort_pid(&antwortcode, freitext.as_deref())?;
                let codeliste = mako_pruefung::codes::EBD_ANFRAGE_SPERRUNG_MSB_GAS;
                let antwort = AntwortSperrungGas {
                    pid: Pruefidentifikator::new(pid)
                        .map_err(|e| WorkflowError::validation(format!("answer PID {pid}: {e}")))?,
                    antwortcode,
                    message_ref,
                };
                let outbox = vec![
                    PendingOutbox::typed(
                        anfrage.netzbetreiber.as_str(),
                        &mako_engine::payload::OrdrspPayload {
                            pid: Some(pid),
                            sender: Some(anfrage.msb.as_str().to_owned()),
                            receiver: Some(anfrage.netzbetreiber.as_str().to_owned()),
                            message_ref: Some(antwort.message_ref.as_str().to_owned()),
                            document_code: Some(BGM_SPERRUNG.to_owned()),
                            document_id: Some(antwort.message_ref.as_str().to_owned()),
                            order_reference: Some(anfrage.dokumentennummer.clone()),
                            antwort_code: Some(antwort.antwortcode.clone()),
                            antwort_codeliste: Some(codeliste.to_owned()),
                            antwort_freitext: freitext,
                            ..Default::default()
                        },
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![SperrungMsbGasEvent::AntwortGesendet {
                        antwort,
                        codeliste: codeliste.to_owned(),
                    }],
                    outbox,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offen() -> SperrungMsbGasState {
        let out = GeliGasSperrungMsbWorkflow::handle(
            &SperrungMsbGasState::New,
            SperrungMsbGasCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::new(ANFRAGE_PID).expect("pid"),
                anfrage: AnfrageSperrungGas {
                    netzbetreiber: MarktpartnerCode::new("9870012345678"),
                    msb: MarktpartnerCode::new("9807317000007"),
                    malo: MaLo::new("51238696012"),
                    dokumentennummer: "SPERR-GAS-1".to_owned(),
                    message_ref: MessageRef::new("UNH-17116"),
                },
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect("received");
        out.events
            .iter()
            .fold(SperrungMsbGasState::New, GeliGasSperrungMsbWorkflow::apply)
    }

    fn antwort(code: &str, freitext: Option<&str>) -> SperrungMsbGasCommand {
        SperrungMsbGasCommand::SendAntwort {
            antwortcode: code.to_owned(),
            freitext: freitext.map(str::to_owned),
            message_ref: MessageRef::new("ORDRSP-1"),
        }
    }

    /// The Cluster of the `E_1001` code picks the PID, and the answer ends the
    /// process.
    #[test]
    fn the_e_1001_cluster_picks_the_answer_pid() {
        let out =
            GeliGasSperrungMsbWorkflow::handle(&offen(), antwort("A01", None)).expect("Zustimmung");
        assert_eq!(out.outbox[0].payload["pid"], BESTAETIGUNG_PID);
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_1001");
        assert_eq!(out.outbox[0].payload["order_reference"], "SPERR-GAS-1");
        let typed = <mako_engine::payload::OrdrspPayload as mako_engine::payload::OutboundPayload>::from_payload(&out.outbox[0].payload)
            .expect("the ORDRSP reads back as its typed payload");
        assert_eq!(typed.antwort_code.as_deref(), Some("A01"));
        let ende = out
            .events
            .iter()
            .fold(offen(), GeliGasSperrungMsbWorkflow::apply);
        assert!(ende.is_terminal());

        let out = GeliGasSperrungMsbWorkflow::handle(
            &offen(),
            antwort("A02", Some("Verhinderungsgrund angezeigt")),
        )
        .expect("Ablehnung");
        assert_eq!(out.outbox[0].payload["pid"], ABLEHNUNG_PID);
    }

    /// `A02`/`A03` need the problem named; a code `E_1001` does not publish is
    /// refused.
    #[test]
    fn an_unnamed_problem_or_a_foreign_code_is_refused() {
        assert!(GeliGasSperrungMsbWorkflow::handle(&offen(), antwort("A03", None)).is_err());
        assert!(GeliGasSperrungMsbWorkflow::handle(&offen(), antwort("A09", None)).is_err());
    }

    /// The gMSB registers no window: the Gas Frist is not sourced here.
    #[test]
    fn no_window_is_registered() {
        assert!(GeliGasSperrungMsbWorkflow::DEADLINE_LABELS.is_empty());
    }
}
