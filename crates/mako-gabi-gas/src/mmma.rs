//! GaBi Gas MMM Allokationsliste Gas — the Abonnement and its list.
//!
//! The Lieferant subscribes to the monthly marktlokationsscharfe
//! Allokationsliste Gas at the Gasnetzbetreiber, and the GNB delivers it on
//! MSCONS 13013 (BDEW/VKU/bne/GEODE AWH Prozesse zur Ermittlung und Abrechnung von
//! Mehr-/Mindermengen Strom und Gas V2.1 Kap. 6.2–6.4, S. 16–23):
//!
//! | PID   | Direction | Description | Routed to |
//! |-------|-----------|-------------|-----------|
//! | 17110 | LF → NB   | Anforderung Allokationsliste — Start or Ende Abo (`IMD` `Z01` / `Z02`, ORDERS AHB 1.1b Kap. 4.2, S. 23–24) | this workflow, for a NB |
//! | 19110 | NB → LF   | Ablehnung der Anforderung (ORDRSP AHB 1.1b Kap. 4.2, S. 15–16) | sent from here; the LF's copy resumes `gpke-allokationsliste` |
//! | 13013 | NB → LF   | Marktlokationsscharfe Allokationsliste Gas | the LF's `gpke-allokationsliste` |
//!
//! # The GNB's side
//!
//! [`GaBiGasMmmaWorkflow`] holds one received Anforderung. The answer is
//! „Unverzüglich" and „Nur bei Ablehnung" (MMM AWH V2.1 Kap. 6.2.2 Nr. 2, S.
//! 17): an accepted Abonnement is answered by the list itself, so no window is
//! registered. The Ablehnung is read out of `E_0803` „Abonnement prüfen",
//! published as Codeliste `G_0001` „ORDRSP Abl. der Anforderung" (EBD 4.3 Kap.
//! 11.1.1, S. 696), which the 19110 names in `SG2 AJT` DE 1082. Only a Start
//! Abo can be refused: the 19110 column lists `IMD` `Z01` alone, and the
//! Beendigung names no Fehlerfall (MMM AWH V2.1 Kap. 6.4.1, S. 21–22).
//!
//! # Regulatory basis
//!
//! - **BDEW/VKU/bne/GEODE AWH Mehr-/Mindermengen V2.1** (18.03.2025) — an
//!   Anwendungshilfe, not a BNetzA Festlegung.
//! - **ORDERS AHB 1.1b** Kap. 4.2 and **ORDRSP AHB 1.1b** Kap. 4.2.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name of the GNB's Abonnement process; the router also files the
/// LF's inbound 13013 under it.
pub const WORKFLOW_NAME: &str = "gabi-gas-mmma";

/// ORDERS 17110 „Anforderung Allokationsliste", LF an NB (Gas only).
pub const ORDERS_ANFRAGE_PID: Pruefidentifikator = Pruefidentifikator::const_new(17110);

/// ORDRSP 19110 „Ablehnung Allokationsliste", NB an LF (Gas only).
pub const ORDRSP_ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19110);

/// `BGM` DE 1001 `Z24` „Allokationsliste (MMMA)" — on the 17110 and the 19110.
pub const BGM_ALLOKATIONSLISTE: &str = "Z24";

/// `IMD` DE 7081 `Z01` „Start Abo".
pub const START_ABO: &str = "Z01";
/// `IMD` DE 7081 `Z02` „Ende Abo".
pub const ENDE_ABO: &str = "Z02";

/// MSCONS Prüfidentifikatoren for Gas MMM Allokationsliste data delivery.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 13013 | Marktlokationsscharfe Allokationsliste Gas (Gas-only MMMA, NB → LF) |
///
/// `mako-gabi-gas` is the canonical source of truth for the Gas MMMA PID set.
pub const MMMA_MSCONS_PIDS: &[u32] = &[13013];

/// The key the GNB's process is filed under: the Anforderung's `BGM` DE 1004,
/// which the 19110 echoes in `SG1 RFF+ON` (`[525]`), unique per sender.
#[must_use]
pub fn anforderung_key(lf: &str, dokumentennummer: &str) -> String {
    format!("{WORKFLOW_NAME}:{lf}:{dokumentennummer}")
}

/// The Anforderung as the GNB received it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Anforderung {
    /// The Lieferant — `SG2 NAD+MS`.
    pub lf: MarktpartnerCode,
    /// The Netzbetreiber — `SG2 NAD+MR`; the Ablehnung's sender.
    pub nb: MarktpartnerCode,
    /// `BGM` DE 1004.
    pub dokumentennummer: String,
    /// `IMD` DE 7081 — [`START_ABO`] or [`ENDE_ABO`].
    pub abo: String,
    /// `DTM+273` in `610` `CCYYMM` — from when resp. until when (inclusive)
    /// the Abonnement runs (`[515]`).
    pub monat: String,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
}

/// The Ablehnung as sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ablehnung {
    /// The code out of `E_0803`.
    pub antwortcode: String,
    /// The Codeliste on the wire — `G_0001`.
    pub codeliste: String,
    /// Reference of the outbound ORDRSP.
    pub message_ref: MessageRef,
}

/// Events of the GNB's Abonnement process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MmmaEvent {
    /// ORDERS 17110 received.
    AnforderungEingegangen {
        /// The Anforderung.
        anforderung: Anforderung,
    },
    /// The ORDERS failed its AHB check.
    AnforderungUngueltig {
        /// The findings.
        reason: String,
    },
    /// The GNB refused the Start Abo on its ORDRSP 19110.
    AblehnungGesendet {
        /// The Ablehnung.
        ablehnung: Ablehnung,
    },
}

impl EventPayload for MmmaEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnforderungEingegangen { .. } => "MmmaAnforderungEingegangen",
            Self::AnforderungUngueltig { .. } => "MmmaAnforderungUngueltig",
            Self::AblehnungGesendet { .. } => "MmmaAblehnungGesendet",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnforderungEingegangen { anforderung } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(ORDERS_ANFRAGE_PID.as_u32()),
                message_ref: Some(anforderung.message_ref.as_str().to_owned()),
            }),
            Self::AblehnungGesendet { ablehnung } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(ORDRSP_ABLEHNUNG_PID.as_u32()),
                message_ref: Some(ablehnung.message_ref.as_str().to_owned()),
            }),
            Self::AnforderungUngueltig { .. } => None,
        }
    }

    fn antwort(&self) -> Option<mako_engine::envelope::Antwortcode> {
        match self {
            Self::AblehnungGesendet { ablehnung } => Some(mako_engine::envelope::Antwortcode {
                code: ablehnung.antwortcode.clone(),
                codeliste: Some(ablehnung.codeliste.clone()),
            }),
            _ => None,
        }
    }
}

/// State of the GNB's Abonnement process.
///
/// ```text
/// New → Eingegangen → Abgelehnt   (ORDRSP 19110 sent)
///     ↘ Ungueltig                  (AHB check failed)
/// ```
///
/// `Eingegangen` is the accepted Abonnement: the AWH answers only an
/// Ablehnung, so nothing further is sent from this process.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum MmmaState {
    /// Nothing received yet.
    #[default]
    New,
    /// The Anforderung is held; a Start Abo may still be refused.
    Eingegangen {
        /// The Anforderung.
        anforderung: Anforderung,
    },
    /// The Anforderung failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// The GNB refused the Start Abo.
    Abgelehnt {
        /// The Anforderung.
        anforderung: Anforderung,
        /// The Ablehnung.
        ablehnung: Ablehnung,
    },
}

impl MmmaState {
    /// Stable label of the variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen { .. } => "Eingegangen",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }

    /// Whether the process has ended.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ungueltig { .. } | Self::Abgelehnt { .. })
    }
}

/// Commands of the GNB's Abonnement process.
#[derive(Debug, Clone)]
pub enum MmmaCommand {
    /// Inbound ORDERS 17110.
    ReceiveAnforderung {
        /// The Anforderung.
        anforderung: Anforderung,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// The GNB refuses the Start Abo on ORDRSP 19110.
    SendAblehnung {
        /// The code out of `E_0803` / `G_0001`.
        antwortcode: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
}

impl CommandPayload for MmmaCommand {}

/// The GNB's side of the Abonnement der marktlokationsscharfen
/// Allokationsliste Gas (ORDERS 17110 → ORDRSP 19110).
pub struct GaBiGasMmmaWorkflow;

/// The ORDRSP 19110 refusing `anforderung` with `antwortcode`.
fn ablehnung(
    anforderung: &Anforderung,
    antwortcode: String,
    message_ref: MessageRef,
) -> Result<WorkflowOutput<MmmaEvent>, WorkflowError> {
    use mako_pruefung::codes as c;
    if anforderung.abo != START_ABO {
        return Err(WorkflowError::validation(format!(
            "only a Start Abo is refused: the 19110 column lists IMD {START_ABO} alone (ORDRSP \
             AHB 1.1b Kap. 4.2, S. 15), and the Beendigung names no Fehlerfall (MMM AWH V2.1 \
             Kap. 6.4.1, S. 22); this Anforderung is IMD {}",
            anforderung.abo
        )));
    }
    let code = c::lookup(c::EBD_ABONNEMENT_ALLOKATIONSLISTE, &antwortcode).ok_or_else(|| {
        WorkflowError::validation(format!(
            "{antwortcode:?} is not published in {} (G_0001)",
            c::EBD_ABONNEMENT_ALLOKATIONSLISTE
        ))
    })?;
    if code.ist_zustimmung() != Some(false) {
        return Err(WorkflowError::validation(format!(
            "{antwortcode:?} is no Ablehnung of {}",
            c::EBD_ABONNEMENT_ALLOKATIONSLISTE
        )));
    }
    let wire = code
        .wire_codeliste()
        .unwrap_or(c::EBD_ABONNEMENT_ALLOKATIONSLISTE)
        .to_owned();
    let payload = mako_engine::payload::OrdrspPayload {
        pid: Some(ORDRSP_ABLEHNUNG_PID.as_u32()),
        sender: Some(anforderung.nb.as_str().to_owned()),
        receiver: Some(anforderung.lf.as_str().to_owned()),
        message_ref: Some(message_ref.as_str().to_owned()),
        document_code: Some(BGM_ALLOKATIONSLISTE.to_owned()),
        document_id: Some(message_ref.as_str().to_owned()),
        // `SG1 RFF+ON` — the 17110's `BGM` DE 1004 (`[525]`).
        order_reference: Some(anforderung.dokumentennummer.clone()),
        // `IMD` — `Z01` „Start Abo", the one value the column lists.
        leistung: Some(START_ABO.to_owned()),
        antwort_code: Some(antwortcode.clone()),
        antwort_codeliste: Some(wire.clone()),
        ..Default::default()
    };
    Ok(WorkflowOutput::with_outbox(
        vec![MmmaEvent::AblehnungGesendet {
            ablehnung: Ablehnung {
                antwortcode,
                codeliste: wire,
                message_ref,
            },
        }],
        vec![PendingOutbox::typed(anforderung.lf.as_str(), &payload).caused_by(0)],
    ))
}

impl Workflow for GaBiGasMmmaWorkflow {
    type State = MmmaState;
    type Event = MmmaEvent;
    type Command = MmmaCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            MmmaEvent::AnforderungEingegangen { anforderung } => MmmaState::Eingegangen {
                anforderung: anforderung.clone(),
            },
            MmmaEvent::AnforderungUngueltig { reason } => MmmaState::Ungueltig {
                reason: reason.clone(),
            },
            MmmaEvent::AblehnungGesendet { ablehnung } => match state {
                MmmaState::Eingegangen { anforderung } => MmmaState::Abgelehnt {
                    anforderung,
                    ablehnung: ablehnung.clone(),
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
            MmmaCommand::ReceiveAnforderung {
                anforderung,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, MmmaState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let mut events = vec![MmmaEvent::AnforderungEingegangen { anforderung }];
                if !validation_passed {
                    events.push(MmmaEvent::AnforderungUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
            MmmaCommand::SendAblehnung {
                antwortcode,
                message_ref,
            } => {
                let MmmaState::Eingegangen { anforderung } = state else {
                    return Err(WorkflowError::invalid_state("Eingegangen", state.label()));
                };
                ablehnung(anforderung, antwortcode, message_ref)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anforderung(abo: &str) -> Anforderung {
        Anforderung {
            lf: MarktpartnerCode::new("9870000000005"),
            nb: MarktpartnerCode::new("9870000000012"),
            dokumentennummer: "ABO-1".to_owned(),
            abo: abo.to_owned(),
            monat: "202612".to_owned(),
            message_ref: MessageRef::new("IN-1"),
        }
    }

    fn eingegangen(abo: &str) -> MmmaState {
        let out = GaBiGasMmmaWorkflow::handle(
            &MmmaState::New,
            MmmaCommand::ReceiveAnforderung {
                anforderung: anforderung(abo),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect("received");
        out.events
            .iter()
            .fold(MmmaState::New, GaBiGasMmmaWorkflow::apply)
    }

    #[test]
    fn a_start_abo_is_refused_on_19110_out_of_g_0001() {
        let out = GaBiGasMmmaWorkflow::handle(
            &eingegangen(START_ABO),
            MmmaCommand::SendAblehnung {
                antwortcode: "Z34".to_owned(),
                message_ref: MessageRef::new("OUT-1"),
            },
        )
        .expect("Z34 is G_0001's Ablehnung");
        let [entry] = out.outbox.as_slice() else {
            panic!("one ORDRSP");
        };
        let p = &entry.payload;
        assert_eq!(p["pid"], 19_110);
        assert_eq!(p["antwort_codeliste"], "G_0001");
        assert_eq!(p["antwort_code"], "Z34");
        assert_eq!(p["order_reference"], "ABO-1");
        assert_eq!(p["document_code"], "Z24");
        assert_eq!(p["leistung"], "Z01");
        assert_eq!(entry.recipient.as_ref(), "9870000000005");
    }

    #[test]
    fn an_ende_abo_is_not_refused() {
        let err = GaBiGasMmmaWorkflow::handle(
            &eingegangen(ENDE_ABO),
            MmmaCommand::SendAblehnung {
                antwortcode: "Z34".to_owned(),
                message_ref: MessageRef::new("OUT-1"),
            },
        );
        assert!(err.is_err(), "the 19110 column lists Start Abo alone");
    }

    #[test]
    fn a_code_outside_g_0001_is_refused() {
        let err = GaBiGasMmmaWorkflow::handle(
            &eingegangen(START_ABO),
            MmmaCommand::SendAblehnung {
                antwortcode: "A01".to_owned(),
                message_ref: MessageRef::new("OUT-1"),
            },
        );
        assert!(err.is_err(), "A01 is not published in E_0803");
    }
}
