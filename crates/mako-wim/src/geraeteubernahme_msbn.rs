//! WiM Gerätewechselabsicht and Geräteübernahme — the MSBN's side.
//!
//! The MSBN announces the Gerätewechsel with ORDERS 17009 „Anzeige
//! Gerätewechselabsicht" (ORDERS AHB 1.1b Kap. 4.10, S. 78–80) and orders the
//! offered devices with ORDERS 17001 „Bestellung Geräteübernahmeangebot" (Kap.
//! 4.9.2, S. 74–77); the MSBA answers 19015 / 19016 resp. 19001 / 19002
//! (ORDRSP AHB 1.1b Kap. 4.10, 4.9.2). The MSBA's side is
//! [`crate::geraeteubernahme`].
//!
//! | PID | Message | Direction |
//! |---|---|---|
//! | 17009 | Anzeige Gerätewechselabsicht | MSBN → MSBA |
//! | 19015 / 19016 | Bestätigung / Ablehnung Gerätewechselabsicht | MSBA → MSBN |
//! | 17001 | Bestellung Geräteübernahmeangebot | MSBN → MSBA |
//! | 19001 / 19002 | Bestätigung / Ablehnung der Bestellung | MSBA → MSBN |
//!
//! # Key
//!
//! The ORDRSP carries no Messlokation; it names the ORDERS by its `BGM` DE
//! 1004 in `SG1 RFF+ON` (Hinweise `[518]`, `[535]`). The process runs under
//! that Dokumentennummer.
//!
//! # Outcome
//!
//! A 19016 ends the process Abgelehnt with its code and, beside `Z07`, the
//! Eigentümer the 19016 names in `SG3 NAD+VY` (ORDRSP AHB 1.1b Kap. 4.10, S.
//! 47, `[42]`). A 19015 ends it Bestaetigt; its code says who removes the
//! devices — `ZB5` the MSBN, to the Lieferadresse in `SG3 NAD+Z22` (`[41]`).

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Workflow name for the MSBN's side.
pub const WORKFLOW_NAME: &str = "wim-geraeteubernahme-msbn";

/// ORDERS 17009 „Anzeige Gerätewechselabsicht".
pub const ABSICHT_PID: u32 = 17_009;
/// ORDERS 17001 „Bestellung Geräteübernahmeangebot".
pub const BESTELLUNG_PID: u32 = 17_001;

/// The ORDRSP the MSBN receives: 19001/19002 answer 17001, 19015/19016
/// answer 17009.
pub const ANTWORT_PIDS: &[u32] = &[19_001, 19_002, 19_015, 19_016];

/// The ORDERS a received ORDRSP answers, and whether it agrees.
#[must_use]
pub const fn beantwortet(ordrsp: u32) -> Option<(u32, bool)> {
    match ordrsp {
        19_001 => Some((BESTELLUNG_PID, true)),
        19_002 => Some((BESTELLUNG_PID, false)),
        19_015 => Some((ABSICHT_PID, true)),
        19_016 => Some((ABSICHT_PID, false)),
        _ => None,
    }
}

/// The ORDERS as the MSBN sent it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MsbnAuftrag {
    /// 17009 or 17001.
    pub pid: Pruefidentifikator,
    /// The MSBN — `NAD+MS`.
    pub msbn: MarktpartnerCode,
    /// The MSBA — `NAD+MR`.
    pub msba: MarktpartnerCode,
    /// `BGM` DE 1004 — the key the answer echoes.
    pub dokumentennummer: String,
}

/// The MSBA's answer as received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MsbaAntwort {
    /// 19001, 19002, 19015 or 19016.
    pub pid: Pruefidentifikator,
    /// `SG2 AJT` DE 4465.
    pub antwortcode: Option<String>,
    /// `SG2 AJT` DE 1082.
    pub codeliste: Option<String>,
    /// `SG3 NAD+VY` — the Eigentümer named beside `Z07`.
    pub eigentuemer: Option<MarktpartnerCode>,
    /// Interchange reference of the inbound ORDRSP.
    pub message_ref: MessageRef,
}

/// Events of the MSBN's side.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MsbnEvent {
    /// The MSBN sent its ORDERS.
    AuftragGesendet {
        /// The ORDERS.
        auftrag: MsbnAuftrag,
    },
    /// The MSBA's ORDRSP arrived.
    AntwortErhalten {
        /// The answer.
        antwort: MsbaAntwort,
    },
}

impl EventPayload for MsbnEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragGesendet { .. } => "GeraeteubernahmeMsbnAuftragGesendet",
            Self::AntwortErhalten { .. } => "GeraeteubernahmeMsbnAntwortErhalten",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        Some(match self {
            Self::AuftragGesendet { auftrag } => Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(auftrag.pid.as_u32()),
                message_ref: Some(auftrag.dokumentennummer.clone()),
            },
            Self::AntwortErhalten { antwort } => Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(antwort.pid.as_u32()),
                message_ref: Some(antwort.message_ref.as_str().to_owned()),
            },
        })
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortErhalten { antwort } => {
                antwort.antwortcode.as_ref().map(|code| Antwortcode {
                    code: code.clone(),
                    codeliste: antwort.codeliste.clone(),
                })
            }
            Self::AuftragGesendet { .. } => None,
        }
    }
}

/// State of the MSBN's side.
///
/// ```text
/// New → Gesendet → Bestaetigt   (19015 / 19001)
///               ↘ Abgelehnt    (19016 / 19002)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum MsbnState {
    /// Nothing sent yet.
    #[default]
    New,
    /// The ORDERS is out; the MSBA's answer is awaited.
    Gesendet(MsbnAuftrag),
    /// The MSBA agreed.
    Bestaetigt {
        /// The ORDERS.
        auftrag: MsbnAuftrag,
        /// The answer.
        antwort: MsbaAntwort,
    },
    /// The MSBA refused.
    Abgelehnt {
        /// The ORDERS.
        auftrag: MsbnAuftrag,
        /// The answer.
        antwort: MsbaAntwort,
    },
}

impl MsbnState {
    /// Stable label of the variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Gesendet(_) => "Gesendet",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }

    /// Whether the process has ended.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Bestaetigt { .. } | Self::Abgelehnt { .. })
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for MsbnState {
    /// The Dokumentennummer is held while the answer is awaited.
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::Gesendet(_))
    }
}

/// Commands of the MSBN's side.
#[derive(Debug, Clone)]
pub enum MsbnCommand {
    /// Send the ORDERS 17009 or 17001; `payload` carries the column's fields
    /// (`location`, `ausfuehrungsdatum`, `sparte`, `umfang`, `geraete` resp.
    /// `angebotsnummer`, `ansprechpartner`, `positionen`).
    SendAuftrag {
        /// The ORDERS.
        auftrag: MsbnAuftrag,
        /// The column's fields.
        payload: serde_json::Value,
    },
    /// The MSBA's ORDRSP arrived.
    ReceiveAntwort {
        /// The answer.
        antwort: MsbaAntwort,
    },
}

impl CommandPayload for MsbnCommand {}

/// The MSBN's side of the Gerätewechselabsicht and the Geräteübernahme.
pub struct WimGeraeteubernahmeMsbnWorkflow;

impl Workflow for WimGeraeteubernahmeMsbnWorkflow {
    type State = MsbnState;
    type Event = MsbnEvent;
    type Command = MsbnCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            MsbnEvent::AuftragGesendet { auftrag } => MsbnState::Gesendet(auftrag.clone()),
            MsbnEvent::AntwortErhalten { antwort } => match state {
                MsbnState::Gesendet(auftrag) => {
                    if beantwortet(antwort.pid.as_u32()).is_some_and(|(_, ja)| ja) {
                        MsbnState::Bestaetigt {
                            auftrag,
                            antwort: antwort.clone(),
                        }
                    } else {
                        MsbnState::Abgelehnt {
                            auftrag,
                            antwort: antwort.clone(),
                        }
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
        match command {
            MsbnCommand::SendAuftrag { auftrag, payload } => {
                if !matches!(state, MsbnState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let pid = auftrag.pid.as_u32();
                if pid != ABSICHT_PID && pid != BESTELLUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "the MSBN sends 17009 or 17001 here, not {pid}"
                    )));
                }
                let mut body = serde_json::json!({
                    "pid": pid,
                    "sender": auftrag.msbn.as_str(),
                    "document_id": auftrag.dokumentennummer,
                    "orders_ref": auftrag.dokumentennummer,
                });
                if let (Some(target), serde_json::Value::Object(extra)) =
                    (body.as_object_mut(), payload)
                {
                    target.extend(extra);
                }
                let outbox =
                    vec![PendingOutbox::new("ORDERS", auftrag.msba.as_str(), body).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![MsbnEvent::AuftragGesendet { auftrag }],
                    outbox,
                ))
            }
            MsbnCommand::ReceiveAntwort { antwort } => {
                let MsbnState::Gesendet(auftrag) = state else {
                    return Err(WorkflowError::invalid_state("Gesendet", state.label()));
                };
                let Some((orders, _)) = beantwortet(antwort.pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "{} is no answer to a Gerätewechselabsicht or Bestellung",
                        antwort.pid
                    )));
                };
                if orders != auftrag.pid.as_u32() {
                    return Err(WorkflowError::rejected(format!(
                        "{} answers ORDERS {orders}, but this process sent {}",
                        antwort.pid, auftrag.pid
                    )));
                }
                Ok(vec![MsbnEvent::AntwortErhalten { antwort }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesendet(pid: u32) -> MsbnState {
        let out = WimGeraeteubernahmeMsbnWorkflow::handle(
            &MsbnState::New,
            MsbnCommand::SendAuftrag {
                auftrag: MsbnAuftrag {
                    pid: Pruefidentifikator::new(pid).expect("pid"),
                    msbn: MarktpartnerCode::new("9900357000004"),
                    msba: MarktpartnerCode::new("9900555000005"),
                    dokumentennummer: "GWA-1".to_owned(),
                },
                payload: serde_json::json!({ "umfang": "Z58" }),
            },
        )
        .expect("sent");
        assert_eq!(out.outbox[0].payload["document_id"], "GWA-1");
        assert_eq!(out.outbox[0].payload["umfang"], "Z58");
        out.events
            .iter()
            .fold(MsbnState::New, WimGeraeteubernahmeMsbnWorkflow::apply)
    }

    fn antwort(pid: u32, code: &str, eigentuemer: Option<&str>) -> MsbnCommand {
        MsbnCommand::ReceiveAntwort {
            antwort: MsbaAntwort {
                pid: Pruefidentifikator::new(pid).expect("pid"),
                antwortcode: Some(code.to_owned()),
                codeliste: Some("S_0066".to_owned()),
                eigentuemer: eigentuemer.map(MarktpartnerCode::new),
                message_ref: MessageRef::new("UNH-19016"),
            },
        }
    }

    /// A 19016 `Z07` ends the Gerätewechselabsicht Abgelehnt naming the
    /// Eigentümer; a 19001 does not answer a 17009.
    #[test]
    fn a_19016_ends_the_absicht_abgelehnt_naming_the_eigentuemer() {
        let state = gesendet(ABSICHT_PID);
        assert!(
            WimGeraeteubernahmeMsbnWorkflow::handle(&state, antwort(19_001, "Z13", None)).is_err()
        );
        let out = WimGeraeteubernahmeMsbnWorkflow::handle(
            &state,
            antwort(19_016, "Z07", Some("9900777000002")),
        )
        .expect("received");
        assert_eq!(
            out.events[0].antwort().map(|a| a.code),
            Some("Z07".to_owned())
        );
        let ende = out
            .events
            .iter()
            .fold(state, WimGeraeteubernahmeMsbnWorkflow::apply);
        assert!(
            matches!(&ende, MsbnState::Abgelehnt { antwort, .. }
                if antwort.eigentuemer.as_ref().map(MarktpartnerCode::as_str) == Some("9900777000002")),
            "{ende:?}"
        );
    }

    /// A 19001 confirms the Bestellung.
    #[test]
    fn a_19001_confirms_the_bestellung() {
        let state = gesendet(BESTELLUNG_PID);
        let out = WimGeraeteubernahmeMsbnWorkflow::handle(&state, antwort(19_001, "Z13", None))
            .expect("received");
        let ende = out
            .events
            .iter()
            .fold(state, WimGeraeteubernahmeMsbnWorkflow::apply);
        assert!(matches!(ende, MsbnState::Bestaetigt { .. }));
    }
}
