//! GPKE / WiM MSCONS data delivery workflow — inbound metered data (NB/MSB → LF).
//!
//! Handles inbound MSCONS messages containing metered energy data, load profiles,
//! meter readings, and time-series delivered by the Netzbetreiber (NB) or
//! Messstellenbetreiber (MSB) to the Lieferant (LF).
//!
//! MSCONS is a data-delivery message format used throughout the German energy market
//! for transmitting metered values. When `makod` operates as a **Lieferant (LF)**,
//! inbound MSCONS messages from NB/MSB must be received, validated, and acknowledged.
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description                                         | Direction     | Domain          |
//! |-------|-----------------------------------------------------|---------------|-----------------|
//! | 13005 | EEG-Überführungszeitreihe                           | NB → LF       | GPKE Teil 2     |
//! | 13006 | Stornierung von Messwerten                          | NB/MSB → LF   | GPKE Teil 2     |
//! | 13015 | Arbeit Leistungsmax. Kalenderj. vor Lieferbeginn    | NB → LF       | GPKE Teil 2     |
//! | 13016 | Energiemenge u. Leistungsmax. (Strom)               | NB/MSB → LF   | GPKE Teil 2/4   |
//! | 13017 | Zählerstand (Strom)                                 | MSB → LF      | GPKE Teil 4     |
//! | 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation| MSB → LF      | GPKE Teil 4     |
//! | 13019 | Energiemenge (Strom)                                | NB/MSB → LF   | GPKE Teil 2/4   |
//! | 13025 | Lastgang Marktlokation, Tranche                     | MSB → LF      | GPKE Teil 4     |
//! | 13027 | Werte nach Typ 2                                    | MSB → LF      | WiM Strom Teil 2|
//!
//! ## Regulatory basis
//!
//! - **BDEW GPKE Teil 2** — Energiedaten nach Lieferbeginn (BK6-24-174 Anlage 1b)
//! - **BDEW GPKE Teil 4** — Stamm- und Bewegungsdaten Strom
//! - **MSCONS S1.x** — EDI@Energy metered data format
//!
//! ## Note on CONTRL acknowledgement
//!
//! CONTRL (functional acknowledgement) is sent by the transport layer automatically.
//! The application workflow records receipt and makes the data available for
//! downstream billing and reconciliation. No explicit application-level APERAK is
//! required for MSCONS in GPKE Teil 2/4.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, MeteredInterval, MeteredZaehlerstand},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the MSCONS data delivery workflow.
pub const WORKFLOW_NAME: &str = "gpke-messwerte";

/// MSCONS Prüfidentifikatoren for GPKE/WiM data delivery to LF.
///
/// These PIDs represent energy data, load profiles, and meter readings
/// that the NB or MSB delivers to the LF (inbound from LF's perspective).
///
/// | PID   | Description                                      | Source  |
/// |-------|--------------------------------------------------|---------|
/// | 13015 | Arbeit Leistungsmax. vor Lieferbeginn            | NB      |
/// | 13016 | Energiemenge u. Leistungsmax. Strom              | NB/MSB  |
/// | 13017 | Zählerstand (Strom)                              | MSB     |
/// | 13018 | Lastgang Messlokation                            | MSB     |
/// | 13019 | Energiemenge (Strom)                             | NB/MSB  |
/// | 13025 | Lastgang Marktlokation, Tranche                  | MSB     |
/// | 13027 | Werte nach Typ 2                                 | MSB     |
/// | 13005 | EEG-Überführungszeitreihe                        | NB      |
/// | 13006 | Stornierung von Messwerten                       | NB/MSB  |
pub const MSCONS_PIDS: &[u32] = &[
    13005, 13006, 13015, 13016, 13017, 13018, 13019, 13025, 13027,
];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when an MSCONS message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MesswerteLieferungData {
    /// BDEW Prüfidentifikator of the inbound MSCONS.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the NB or MSB sender.
    pub sender: MarktpartnerCode,
    /// EIC/MaLo of the supply location for which data was delivered.
    pub location_id: MaLo,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
    /// The readings the delivery carried.
    ///
    /// Defaulted so a stream written before the values were carried still
    /// replays; such a process has no readings to hand on, which is the
    /// truthful reading of what was recorded.
    #[serde(default)]
    pub reads: Vec<MeteredInterval>,
    /// The Zählerstände the delivery carried, each at its `DTM+7`
    /// Nutzungszeitpunkt.
    #[serde(default)]
    pub zaehlerstaende: Vec<MeteredZaehlerstand>,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MSCONS data delivery workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MesswerteLieferungEvent {
    /// Inbound MSCONS message received; AHB validation not yet performed.
    MsconsDatenErhalten {
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sending NB or MSB.
        sender: MarktpartnerCode,
        /// EIC/MaLo of the supply location.
        location_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// The readings the delivery carried.
        #[serde(default)]
        reads: Vec<MeteredInterval>,
        /// The Zählerstände the delivery carried.
        #[serde(default)]
        zaehlerstaende: Vec<MeteredZaehlerstand>,
    },
    /// AHB validation passed; data is available for downstream use.
    ValidationPassed {
        /// Message reference of the validated MSCONS.
        message_ref: MessageRef,
    },
    /// AHB validation failed; data is unusable.
    ValidationFailed {
        /// Human-readable validation error summary.
        reason: String,
    },
}

impl EventPayload for MesswerteLieferungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::MsconsDatenErhalten { .. } => "MesswerteLieferungErhalten",
            Self::ValidationPassed { .. } => "MesswerteLieferungValidationPassed",
            Self::ValidationFailed { .. } => "MesswerteLieferungValidationFailed",
        }
    }

    /// The inbound MSCONS is the one market message. `ValidationPassed` queues
    /// the `ProcessCompleted` ERP notification, not a market message, and
    /// `ValidationFailed` queues nothing.
    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::MsconsDatenErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::ValidationPassed { .. } | Self::ValidationFailed { .. } => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of an MSCONS data delivery process stream.
///
/// # Lifecycle
///
/// ```text
/// New → DatenErhalten → ValidationPassed (terminal)
///                     → ValidationFailed (terminal)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum MesswerteLieferungState {
    /// No events yet.
    #[default]
    New,
    /// MSCONS received; awaiting validation result.
    DatenErhalten(MesswerteLieferungData),
    /// Validation passed; data available (terminal success).
    ValidationPassed(MesswerteLieferungData),
    /// Validation failed (terminal failure).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

/// A delivery is settled once it is validated: each MSCONS is its own
/// process, so the next one for the same Marktlokation spawns rather than
/// being fed into a finished delivery (whose `ReceiveMscons` refuses anything
/// but `New`).
impl mako_engine::workflow::OccupiesBusinessKey for MesswerteLieferungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Received, not yet validated — never left standing by one
            // command, so read as live.
            Self::DatenErhalten(_) => true,
            Self::New | Self::ValidationPassed(_) | Self::ValidationFailed { .. } => false,
        }
    }
}

impl MesswerteLieferungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::DatenErhalten(_) => "DatenErhalten",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MSCONS data delivery workflow.
#[derive(Clone)]
pub enum MesswerteLieferungCommand {
    /// Inbound MSCONS received from NB or MSB.
    ReceiveMscons {
        /// BDEW Prüfidentifikator of the inbound MSCONS.
        pid: Pruefidentifikator,
        /// GLN of the NB or MSB sender.
        sender: MarktpartnerCode,
        /// EIC/MaLo of the delivery location.
        location_id: MaLo,
        /// EDIFACT document date.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// The decoded `SG9`/`SG10` readings the message carried.
        ///
        /// Empty when the message carries none, or when none could be dated —
        /// the process still records the delivery, because an MSCONS that
        /// arrived and could not be decoded is a fact an operator has to see.
        reads: Vec<MeteredInterval>,
        /// The decoded Zählerstände, each at its `SG10 DTM+7`
        /// Nutzungszeitpunkt (MSCONS MIG 2.5, S. 35).
        zaehlerstaende: Vec<MeteredZaehlerstand>,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Human-readable validation errors (if any).
        validation_errors: Vec<String>,
        /// `SG1 RFF+AGI` — the Belegnummer of the ORDERS that ordered these
        /// values, on a **PID 13027** „Werte nach Typ 2" delivery.
        ///
        /// MSCONS AHB 3.2 §11.2 hint `[574]`: „Wert aus BGM DE1004 der ORDERS
        /// mit der die Bestellung der Werte nach Typ 2 erfolgt ist". It is the
        /// first hop of the PID overview's `EZ-03` routing and the only thing
        /// on a value delivery that names the subscription it belongs to — a
        /// Meldepunkt may carry several, since a subscription is the
        /// (Meldepunkt, Messprodukt) pair.
        ///
        /// Carried through to `edmd` so a Typ-2 gap can be reported against the
        /// subscription that is actually silent instead of being inferred from
        /// whichever OBIS registers happened to stop.
        bestellung_ref: Option<String>,
    },
}

impl CommandPayload for MesswerteLieferungCommand {}

/// Build the `ProcessCompleted` payload `edmd` consumes.
///
/// The key set mirrors the outbound MSB delivery in
/// `mako_wim::wertebestellung` — `malo_id`, `sender`, `message_ref`, `sparte`
/// and a `reads` array of `{dtm_from, dtm_to, quantity_kwh, quality,
/// obis_code, melo_id}` — so the two directions of the same values are the
/// same shape.
///
/// `malo_id` is present whether or not there are readings: `edmd` refuses an
/// event without one before it looks at anything else, and a delivery whose
/// intervals could not be decoded must still leave a receipt rather than
/// vanish.
///
/// `sparte` is stated rather than left to `edmd`'s default because this crate
/// is the Strom half of MSCONS ingest; the Gas PIDs are `mako-geli-gas`.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
)]
fn erp_payload(
    pid: Pruefidentifikator,
    sender: &MarktpartnerCode,
    location_id: &MaLo,
    message_ref: &MessageRef,
    reads: &[MeteredInterval],
    zaehlerstaende: &[MeteredZaehlerstand],
    bestellung_ref: Option<&str>,
    document_date: &str,
) -> serde_json::Value {
    serde_json::json!({
        "pid": pid.as_u32(),
        "malo_id": location_id.as_str(),
        "sender": sender.as_str(),
        "message_ref": message_ref.as_str(),
        "sparte": "STROM",
        // The `DTM+137` Nachrichtendatum, for the Lastgang PIDs whose
        // Versions-Tupel ends in it — what `edmd` orders deliveries by.
        "mscons_version": mako_engine::types::messwesen_version(pid.as_u32(), document_date),
        // `SG1 RFF+AGI` on a 13027 — which ESA subscription these values are.
        "bestellung_ref": bestellung_ref,
        "reads": reads
            .iter()
            .map(|r| serde_json::json!({
                "dtm_from": r.dtm_from,
                "dtm_to": r.dtm_to,
                "quantity_kwh": r.quantity,
                "quality": r.quality(),
                "obis_code": r.obis_code,
                "melo_id": r.melo_id,
                "herkunft": r.herkunft(),
            }))
            .collect::<Vec<_>>(),
        "zaehlerstaende": zaehlerstaende
            .iter()
            .map(MeteredZaehlerstand::payload)
            .collect::<Vec<_>>(),
    })
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MSCONS data delivery workflow — handles inbound MSCONS for LF/NB role.
///
/// Records inbound MSCONS metered-data messages from NB/MSB to LF, validates
/// them against the appropriate AHB profile, and makes the data available for
/// downstream billing and settlement reconciliation.
pub struct GpkeMesswerteLieferungWorkflow;

impl Workflow for GpkeMesswerteLieferungWorkflow {
    type State = MesswerteLieferungState;
    type Event = MesswerteLieferungEvent;
    type Command = MesswerteLieferungCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            MesswerteLieferungEvent::MsconsDatenErhalten {
                pruefidentifikator,
                sender,
                location_id,
                document_date,
                message_ref,
                reads,
                zaehlerstaende,
            } => MesswerteLieferungState::DatenErhalten(MesswerteLieferungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                location_id: location_id.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
                reads: reads.clone(),
                zaehlerstaende: zaehlerstaende.clone(),
            }),
            MesswerteLieferungEvent::ValidationPassed { .. } => match state {
                MesswerteLieferungState::DatenErhalten(data) => {
                    MesswerteLieferungState::ValidationPassed(data)
                }
                other => other,
            },
            MesswerteLieferungEvent::ValidationFailed { reason } => {
                MesswerteLieferungState::ValidationFailed {
                    reason: reason.clone(),
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            MesswerteLieferungCommand::ReceiveMscons {
                pid,
                sender,
                location_id,
                document_date,
                message_ref,
                reads,
                zaehlerstaende,
                validation_passed,
                validation_errors,
                bestellung_ref,
            } => {
                if !matches!(state, MesswerteLieferungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !MSCONS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a handled MSCONS PID",
                    )));
                }
                let erp_payload = erp_payload(
                    pid,
                    &sender,
                    &location_id,
                    &message_ref,
                    &reads,
                    &zaehlerstaende,
                    bestellung_ref.as_deref(),
                    &document_date,
                );
                let mut events = vec![MesswerteLieferungEvent::MsconsDatenErhalten {
                    pruefidentifikator: pid,
                    sender,
                    location_id,
                    document_date,
                    message_ref: message_ref.clone(),
                    reads,
                    zaehlerstaende,
                }];
                let outbox = if validation_passed {
                    events.push(MesswerteLieferungEvent::ValidationPassed { message_ref });
                    // Hand `edmd` the delivery *and its readings*. `edmd` drops
                    // an event with no `malo_id` before it reaches the interval
                    // parser, so a payload carrying only the PID would complete
                    // the process and store nothing.
                    vec![PendingOutbox::new("ProcessCompleted", "", erp_payload).caused_by(1)]
                } else {
                    events.push(MesswerteLieferungEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                    vec![]
                };
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `edmd` payload of a 13025 carries the version its Versions-Tupel
    /// names (the `DTM+137` Nachrichtendatum) and the origin each value
    /// states; a 13017 Zählerstand is not versioned and carries none.
    #[test]
    fn a_lastgang_payload_carries_its_version_and_origin() {
        let read = MeteredInterval {
            obis_code: Some("1-1:1.29.0".to_owned()),
            melo_id: None,
            dtm_from: "2026-07-01T00:00:00+00:00".to_owned(),
            dtm_to: "2026-07-01T00:15:00+00:00".to_owned(),
            quantity: "0.25".to_owned(),
            unit: None,
            qualifier: "220".to_owned(),
            plausibilisierung: vec!["Z83".to_owned()],
        };
        let payload = |pid: u32| {
            let out = GpkeMesswerteLieferungWorkflow::handle(
                &MesswerteLieferungState::New,
                MesswerteLieferungCommand::ReceiveMscons {
                    pid: Pruefidentifikator::new(pid).unwrap(),
                    sender: MarktpartnerCode::new("9900357000004"),
                    location_id: MaLo::new("51238696012"),
                    document_date: "202607020800+00".to_owned(),
                    message_ref: MessageRef::new("MSC-1"),
                    reads: vec![read.clone()],
                    zaehlerstaende: vec![],
                    validation_passed: true,
                    validation_errors: vec![],
                    bestellung_ref: None,
                },
            )
            .unwrap();
            out.outbox[0].payload.clone()
        };
        let lastgang = payload(13_025);
        assert_eq!(lastgang["mscons_version"], "20260702080000");
        assert_eq!(lastgang["reads"][0]["herkunft"], "KUNDENSELBSTABLESUNG");
        assert!(payload(13_017)["mscons_version"].is_null());
    }

    /// A 13017 carries its Zählerstände to `edmd`, each with the origin it
    /// states.
    #[test]
    fn a_zaehlerstand_reaches_the_payload_with_its_origin() {
        let stand = MeteredZaehlerstand {
            obis_code: Some("1-1:1.8.0".to_owned()),
            melo_id: Some("DE0001234567890123456789012345678".to_owned()),
            at: "2026-06-30T22:00:00+00:00".to_owned(),
            value: "14230.5".to_owned(),
            unit: None,
            qualifier: "220".to_owned(),
            plausibilisierung: vec!["Z83".to_owned()],
        };
        let out = GpkeMesswerteLieferungWorkflow::handle(
            &MesswerteLieferungState::New,
            MesswerteLieferungCommand::ReceiveMscons {
                pid: Pruefidentifikator::new(13_017).unwrap(),
                sender: MarktpartnerCode::new("9900357000004"),
                location_id: MaLo::new("DE0001234567890123456789012345678"),
                document_date: "202607020800+00".to_owned(),
                message_ref: MessageRef::new("MSC-1"),
                reads: vec![],
                zaehlerstaende: vec![stand],
                validation_passed: true,
                validation_errors: vec![],
                bestellung_ref: None,
            },
        )
        .unwrap();
        let z = &out.outbox[0].payload["zaehlerstaende"][0];
        assert_eq!(z["at"], "2026-06-30T22:00:00+00:00");
        assert_eq!(z["value"], "14230.5");
        assert_eq!(z["quality"], "MEASURED");
        assert_eq!(z["herkunft"], "KUNDENSELBSTABLESUNG");
    }

    #[test]
    fn verlauf_states_the_inbound_mscons() {
        let erhalten = MesswerteLieferungEvent::MsconsDatenErhalten {
            pruefidentifikator: Pruefidentifikator::new(13_025).unwrap(),
            sender: MarktpartnerCode::new("9900357000004"),
            location_id: MaLo::new("51238696012"),
            document_date: "20260101".to_owned(),
            reads: vec![],
            zaehlerstaende: vec![],
            message_ref: MessageRef::new("MSC-1"),
        };
        assert_eq!(
            erhalten.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(13_025),
                message_ref: Some("MSC-1".to_owned()),
            })
        );
        assert_eq!(
            MesswerteLieferungEvent::ValidationPassed {
                message_ref: MessageRef::new("MSC-1"),
            }
            .verlauf(),
            None
        );
        assert_eq!(
            MesswerteLieferungEvent::ValidationFailed {
                reason: "x".to_owned(),
            }
            .verlauf(),
            None,
            "a failed validation queues no APERAK here"
        );
    }
}
