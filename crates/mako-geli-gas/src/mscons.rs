//! GeLi Gas / WiM Gas MSCONS data delivery workflow.
//!
//! Handles inbound MSCONS messages carrying Gas metered values delivered by the
//! Netzbetreiber (NB) or Messstellenbetreiber (MSB) to the Lieferant (LF).
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description                                    | Direction   | Domain           |
//! |-------|------------------------------------------------|-------------|------------------|
//! | 13002 | Messwerte Zählerstand Gas                      | NB/MSB → LF | GeLi Gas 2.0 / WiM Gas |
//! | 13007 | Gasbeschaffenheitsdaten                        | NB → LF     | GeLi Gas 2.0 / KoV Gas |
//! | 13008 | Messwert Lastgang Gas                          | NB/MSB → LF | GeLi Gas 2.0 / KoV Gas |
//! | 13009 | Messwert Energiemenge Gas                      | NB/MSB → LF | GeLi Gas 2.0 / WiM Gas |
//!
//! PIDs **13013** and **13014** are MMM Strom/Gas (Allokationsliste) and are **not** in scope
//! for GeLi Gas 2.0. PID 13013 (Gas-only) belongs to `mako-gabi-gas` `gabi-gas-mmma`.
//! PID 13014 (Strom side) belongs to `mako-gpke` `gpke-allokationsliste`.
//!
//! ## Regulatory basis
//!
//! - **BK7-24-01-009** (GeLi Gas 3.0) — UTILMD Gas / MSCONS Gas data exchange
//! - **BDEW GeLi Gas AHB** — metered gas data formats and PID definitions
//! - **MSCONS G1.x** — EDI@Energy metered gas data format

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{
        MaLo, MarktpartnerCode, MessageRef, MeteredInterval, MeteredZaehlerstand,
        Pruefidentifikator,
    },
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// The empty MaLo, for replaying a stream recorded before the location was
/// carried.
fn no_location() -> MaLo {
    MaLo::new("")
}

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the GeLi Gas MSCONS data delivery workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-mscons";

/// MSCONS Prüfidentifikatoren for Gas data delivery (NB/MSB → LF).
///
/// | PID   | Description                               | Source | Domain |
/// |-------|-------------------------------------------|--------|--------|
/// | 13002 | Messwerte Zählerstand Gas                 | NB/MSB | GeLi Gas 2.0 / WiM Gas |
/// | 13007 | Gasbeschaffenheitsdaten                   | NB     | GeLi Gas 2.0 / KoV BK-Mgmt Gas |
/// | 13008 | Messwert Lastgang Gas                     | NB/MSB | GeLi Gas 2.0 / KoV BK-Mgmt Gas |
/// | 13009 | Messwert Energiemenge Gas                 | NB/MSB | GeLi Gas 2.0 / WiM Gas |
///
/// PIDs 13013/13014 are **MMM Strom/Gas** (Allokationsliste), NOT GeLi Gas.
/// PID 13013 (Gas-only) belongs to `mako-gabi-gas` `gabi-gas-mmma`.
/// PID 13014 (cross-commodity Strom side) belongs to `mako-gpke` `gpke-allokationsliste`.
pub const MSCONS_PIDS: &[u32] = &[13002, 13007, 13008, 13009];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a Gas MSCONS message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GasMsconsDatenData {
    /// BDEW Prüfidentifikator of the inbound MSCONS.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the NB or MSB sender.
    pub sender: MarktpartnerCode,
    /// MaLo the delivery is for.
    ///
    /// Defaulted to an empty MaLo so a stream written before the location was
    /// carried still replays — such a process genuinely recorded none.
    #[serde(default = "no_location")]
    pub location_id: MaLo,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
    /// The readings the delivery carried.
    #[serde(default)]
    pub reads: Vec<MeteredInterval>,
    /// The Zählerstände the delivery carried (PID 13002).
    #[serde(default)]
    pub zaehlerstaende: Vec<MeteredZaehlerstand>,
    /// Abrechnungsbrennwert in kWh/m³ (PID 13007 only).
    ///
    /// Extracted from `QTY+Z08` in the EDIFACT MSCONS interchange.
    /// Stored as a string to avoid floating-point precision issues.
    /// `None` for PIDs 13002, 13008, 13009 (meter readings, not gas quality).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brennwert_kwh_per_m3: Option<String>,
    /// Zustandszahl — dimensionless compressibility factor (PID 13007 only).
    ///
    /// Extracted from `QTY+Z10` in the EDIFACT MSCONS interchange.
    /// `None` for PIDs 13002, 13008, 13009.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zustandszahl: Option<String>,
    /// Gas quality type (PID 13007 only).
    ///
    /// Normalized to the BO4E `Gasqualitaet` wire value: `"H_GAS"` | `"L_GAS"`.
    /// Those are the only two the schema defines — see
    /// [`crate::gas_quality`] for why no H2 spelling is invented here.
    /// `None` until the DVGW/BNetzA EDIFACT qualifier for gas quality type in
    /// MSCONS is standardized (expected 2026–2028 AHB wave).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gasqualitaet: Option<String>,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas MSCONS data delivery workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasMsconsDatenEvent {
    /// Inbound Gas MSCONS received; AHB validation not yet performed.
    MsconsDatenErhalten {
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sending NB or MSB.
        sender: MarktpartnerCode,
        /// MaLo the delivery is for.
        #[serde(default = "no_location")]
        location_id: MaLo,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// The readings the delivery carried.
        #[serde(default)]
        reads: Vec<MeteredInterval>,
        /// The Zählerstände the delivery carried (PID 13002).
        #[serde(default)]
        zaehlerstaende: Vec<MeteredZaehlerstand>,
        /// Abrechnungsbrennwert in kWh/m³ (PID 13007 only, from `QTY+Z08`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brennwert_kwh_per_m3: Option<String>,
        /// Zustandszahl (PID 13007 only, from `QTY+Z10`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zustandszahl: Option<String>,
        /// Gas quality type (PID 13007 only).
        ///
        /// BO4E `Gasqualitaet` wire value: `"H_GAS"` | `"L_GAS"`.
        /// `None` until the DVGW/BNetzA H2-blend AHB qualifier is published.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gasqualitaet: Option<String>,
    },
    /// AHB validation passed; data available for downstream use.
    ValidationPassed {
        /// Message reference of the validated MSCONS.
        message_ref: MessageRef,
    },
    /// AHB validation failed; data unusable.
    ValidationFailed {
        /// Human-readable validation error summary.
        reason: String,
    },
}

impl EventPayload for GasMsconsDatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::MsconsDatenErhalten { .. } => "GasMsconsDatenErhalten",
            Self::ValidationPassed { .. } => "GasMsconsDatenValidationPassed",
            Self::ValidationFailed { .. } => "GasMsconsDatenValidationFailed",
        }
    }

    /// The inbound MSCONS is the one market message. `ValidationPassed`
    /// queues the `ProcessCompleted` ERP notification, not a market message,
    /// and `ValidationFailed` queues nothing.
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

/// Current state of a GeLi Gas MSCONS data delivery process stream.
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
pub enum GasMsconsDatenState {
    /// No events yet.
    #[default]
    New,
    /// MSCONS received; awaiting validation result.
    DatenErhalten(GasMsconsDatenData),
    /// Validation passed; data available (terminal success).
    ValidationPassed(GasMsconsDatenData),
    /// Validation failed (terminal failure).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

/// A delivery is settled once it is validated: each MSCONS is its own
/// process, so the next one for the same Marktlokation spawns rather than
/// being fed into a finished delivery (whose `ReceiveMscons` records nothing
/// outside `New`).
impl mako_engine::workflow::OccupiesBusinessKey for GasMsconsDatenState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Received, not yet validated — never left standing by one
            // command, so read as live.
            Self::DatenErhalten(_) => true,
            Self::New | Self::ValidationPassed(_) | Self::ValidationFailed { .. } => false,
        }
    }
}

impl GasMsconsDatenState {
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

/// Commands for the GeLi Gas MSCONS data delivery workflow.
#[derive(Clone)]
pub enum GasMsconsDatenCommand {
    /// Inbound Gas MSCONS received from NB or MSB.
    ReceiveMscons {
        /// BDEW Prüfidentifikator of the inbound MSCONS.
        pid: Pruefidentifikator,
        /// GLN of the NB or MSB sender.
        sender: MarktpartnerCode,
        /// MaLo the delivery is for, from `SG5 NAD`.
        location_id: MaLo,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// The decoded `SG9`/`SG10` readings the message carried.
        reads: Vec<MeteredInterval>,
        /// The decoded Zählerstände, each at its `SG10 DTM+7`
        /// Nutzungszeitpunkt (MSCONS MIG 2.5, S. 35).
        zaehlerstaende: Vec<MeteredZaehlerstand>,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Human-readable validation errors (if any).
        validation_errors: Vec<String>,
        /// Abrechnungsbrennwert in kWh/m³ (PID 13007 only, from `QTY+Z08`).
        ///
        /// `None` for PIDs 13002, 13008, 13009.
        brennwert_kwh_per_m3: Option<String>,
        /// Zustandszahl (PID 13007 only, from `QTY+Z10`).
        ///
        /// `None` for PIDs 13002, 13008, 13009.
        zustandszahl: Option<String>,
        /// Gas quality type (PID 13007 only).
        ///
        /// BO4E `Gasqualitaet` wire value: `"H_GAS"` | `"L_GAS"`; `None` when absent.
        gasqualitaet: Option<String>,
        /// `DTM+137` Nachrichtendatum, verbatim (format 303). It versions a
        /// 13008 Lastgang (MSCONS AHB 3.2 Kap. 5.1).
        nachrichtendatum: String,
    },
}

impl CommandPayload for GasMsconsDatenCommand {}

/// Build the `ProcessCompleted` payload `edmd` consumes.
///
/// Same key set as the Strom half in `mako_gpke::messwerte`, plus the
/// Gasbeschaffenheit factors PID 13007 carries. `malo_id` is unconditional:
/// `edmd` refuses an event without one before it reaches either the interval
/// parser or the Brennwert branch, so both depend on it.
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
    brennwert_kwh_per_m3: Option<&str>,
    zustandszahl: Option<&str>,
    gasqualitaet: Option<&str>,
    nachrichtendatum: &str,
) -> serde_json::Value {
    serde_json::json!({
        "pid": pid.as_u32(),
        "malo_id": location_id.as_str(),
        "sender": sender.as_str(),
        "message_ref": message_ref.as_str(),
        "sparte": "GAS",
        // The `DTM+137` Nachrichtendatum, for the Lastgang Gas whose
        // Versions-Tupel ends in it — what `edmd` orders deliveries by.
        "mscons_version": mako_engine::types::messwesen_version(pid.as_u32(), nachrichtendatum),
        "brennwert_kwh_per_m3": brennwert_kwh_per_m3,
        "zustandszahl": zustandszahl,
        "gasqualitaet": gasqualitaet,
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

/// GeLi Gas MSCONS data delivery workflow.
///
/// Records inbound Gas MSCONS metered-data messages from NB/MSB to LF,
/// validates them against the Gas MSCONS AHB profile, and makes the data
/// available for downstream billing and settlement.
pub struct GeliGasMsconsWorkflow;

impl Workflow for GeliGasMsconsWorkflow {
    type State = GasMsconsDatenState;
    type Event = GasMsconsDatenEvent;
    type Command = GasMsconsDatenCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasMsconsDatenEvent::MsconsDatenErhalten {
                pruefidentifikator,
                sender,
                location_id,
                reads,
                zaehlerstaende,
                message_ref,
                brennwert_kwh_per_m3,
                zustandszahl,
                gasqualitaet,
            } => {
                if matches!(state, GasMsconsDatenState::New) {
                    GasMsconsDatenState::DatenErhalten(GasMsconsDatenData {
                        pruefidentifikator: *pruefidentifikator,
                        sender: sender.clone(),
                        location_id: location_id.clone(),
                        reads: reads.clone(),
                        zaehlerstaende: zaehlerstaende.clone(),
                        message_ref: message_ref.clone(),
                        brennwert_kwh_per_m3: brennwert_kwh_per_m3.clone(),
                        zustandszahl: zustandszahl.clone(),
                        gasqualitaet: gasqualitaet.clone(),
                    })
                } else {
                    state
                }
            }
            GasMsconsDatenEvent::ValidationPassed { .. } => {
                if let GasMsconsDatenState::DatenErhalten(data) = state {
                    GasMsconsDatenState::ValidationPassed(data)
                } else {
                    state
                }
            }
            GasMsconsDatenEvent::ValidationFailed { reason } => {
                GasMsconsDatenState::ValidationFailed {
                    reason: reason.clone(),
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        cmd: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match cmd {
            GasMsconsDatenCommand::ReceiveMscons {
                pid,
                sender,
                location_id,
                message_ref,
                reads,
                zaehlerstaende,
                validation_passed,
                validation_errors,
                brennwert_kwh_per_m3,
                zustandszahl,
                gasqualitaet,
                nachrichtendatum,
            } => {
                if !MSCONS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas MSCONS PID ({MSCONS_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GasMsconsDatenState::New) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                let erp_payload = erp_payload(
                    pid,
                    &sender,
                    &location_id,
                    &message_ref,
                    &reads,
                    &zaehlerstaende,
                    brennwert_kwh_per_m3.as_deref(),
                    zustandszahl.as_deref(),
                    gasqualitaet.as_deref(),
                    &nachrichtendatum,
                );
                let erhalten_event = GasMsconsDatenEvent::MsconsDatenErhalten {
                    pruefidentifikator: pid,
                    sender,
                    location_id,
                    reads,
                    zaehlerstaende,
                    message_ref: message_ref.clone(),
                    brennwert_kwh_per_m3: brennwert_kwh_per_m3.clone(),
                    zustandszahl: zustandszahl.clone(),
                    gasqualitaet: gasqualitaet.clone(),
                };
                let mut events = vec![erhalten_event];
                let outbox = if validation_passed {
                    events.push(GasMsconsDatenEvent::ValidationPassed { message_ref });
                    // Notify `edmd` that validated Gas MSCONS data is ready.
                    // The payload carries the MaLo, the readings, and — for PID
                    // 13007 — Brennwert and Zustandszahl. The MaLo is what
                    // makes the rest reachable: `edmd` refuses an event without
                    // one before it looks at either, so the Brennwert this
                    // workflow already extracted never arrived either.
                    vec![PendingOutbox::new("ProcessCompleted", "", erp_payload).caused_by(1)]
                } else {
                    events.push(GasMsconsDatenEvent::ValidationFailed {
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

    #[test]
    fn verlauf_states_the_inbound_mscons() {
        let out = GeliGasMsconsWorkflow::handle(
            &GasMsconsDatenState::New,
            GasMsconsDatenCommand::ReceiveMscons {
                pid: Pruefidentifikator::new(13_002).unwrap(),
                sender: MarktpartnerCode::new("9800000000004"),
                location_id: MaLo::new("51238696012"),
                message_ref: MessageRef::new("MSC-1"),
                reads: vec![],
                zaehlerstaende: vec![],
                validation_passed: true,
                validation_errors: vec![],
                brennwert_kwh_per_m3: None,
                zustandszahl: None,
                gasqualitaet: None,
                nachrichtendatum: String::new(),
            },
        )
        .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(13_002),
                message_ref: Some("MSC-1".to_owned()),
            })
        );
        assert_eq!(
            out.events[1].verlauf(),
            None,
            "the ProcessCompleted notification is no market message"
        );
        assert_eq!(
            GasMsconsDatenEvent::ValidationFailed {
                reason: "x".to_owned()
            }
            .verlauf(),
            None
        );
    }

    /// The `edmd` payload of a 13008 carries the version its Versions-Tupel
    /// names and the origin each value states.
    #[test]
    fn a_lastgang_gas_payload_carries_its_version_and_origin() {
        let read = MeteredInterval {
            obis_code: Some("7-20:99.33.17".to_owned()),
            melo_id: None,
            dtm_from: "2026-07-01T04:00:00+00:00".to_owned(),
            dtm_to: "2026-07-01T05:00:00+00:00".to_owned(),
            quantity: "12.5".to_owned(),
            unit: None,
            qualifier: "220".to_owned(),
            plausibilisierung: vec!["Z83".to_owned()],
        };
        let out = GeliGasMsconsWorkflow::handle(
            &GasMsconsDatenState::New,
            GasMsconsDatenCommand::ReceiveMscons {
                pid: Pruefidentifikator::new(13_008).unwrap(),
                sender: MarktpartnerCode::new("9800000000004"),
                location_id: MaLo::new("51238696012"),
                message_ref: MessageRef::new("MSC-1"),
                reads: vec![read],
                zaehlerstaende: vec![],
                validation_passed: true,
                validation_errors: vec![],
                brennwert_kwh_per_m3: None,
                zustandszahl: None,
                gasqualitaet: None,
                nachrichtendatum: "202607020800+00".to_owned(),
            },
        )
        .unwrap();
        let payload = &out.outbox[0].payload;
        assert_eq!(payload["mscons_version"], "20260702080000");
        assert_eq!(payload["reads"][0]["herkunft"], "KUNDENSELBSTABLESUNG");
    }
}
