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

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

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
#[serde(deny_unknown_fields)]
pub struct GasMsconsDatenData {
    /// BDEW Prüfidentifikator of the inbound MSCONS.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the NB or MSB sender.
    pub sender: MarktpartnerCode,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
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
    /// Normalized to canonical form: `"H_GAS"` | `"L_GAS"` | `"H2_BLEND"` | `"BIOGAS"`.
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
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Abrechnungsbrennwert in kWh/m³ (PID 13007 only, from `QTY+Z08`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brennwert_kwh_per_m3: Option<String>,
        /// Zustandszahl (PID 13007 only, from `QTY+Z10`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zustandszahl: Option<String>,
        /// Gas quality type (PID 13007 only).
        ///
        /// Canonical: `"H_GAS"` | `"L_GAS"` | `"H2_BLEND"` | `"BIOGAS"`.
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
        /// EDIFACT message reference.
        message_ref: MessageRef,
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
        /// Canonical: `"H_GAS"` | `"L_GAS"` | `"H2_BLEND"`. `None` until AHB is published.
        gasqualitaet: Option<String>,
    },
}

impl CommandPayload for GasMsconsDatenCommand {}

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
                message_ref,
                brennwert_kwh_per_m3,
                zustandszahl,
                gasqualitaet,
            } => {
                if matches!(state, GasMsconsDatenState::New) {
                    GasMsconsDatenState::DatenErhalten(GasMsconsDatenData {
                        pruefidentifikator: *pruefidentifikator,
                        sender: sender.clone(),
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
                message_ref,
                validation_passed,
                validation_errors,
                brennwert_kwh_per_m3,
                zustandszahl,
                gasqualitaet,
            } => {
                if !MSCONS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas MSCONS PID ({MSCONS_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GasMsconsDatenState::New) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                let erhalten_event = GasMsconsDatenEvent::MsconsDatenErhalten {
                    pruefidentifikator: pid,
                    sender,
                    message_ref: message_ref.clone(),
                    brennwert_kwh_per_m3: brennwert_kwh_per_m3.clone(),
                    zustandszahl: zustandszahl.clone(),
                    gasqualitaet: gasqualitaet.clone(),
                };
                let mut events = vec![erhalten_event];
                let outbox = if validation_passed {
                    events.push(GasMsconsDatenEvent::ValidationPassed { message_ref });
                    // Notify `edmd` that validated Gas MSCONS data is ready.
                    // For PID 13007 the payload carries Brennwert and Zustandszahl
                    // so `edmd` can update `meter_billing_periods` directly.
                    vec![
                        PendingOutbox::new(
                            "ProcessCompleted",
                            "",
                            serde_json::json!({
                                "pid": pid.as_u32(),
                                "brennwert_kwh_per_m3": brennwert_kwh_per_m3,
                                "zustandszahl": zustandszahl,
                                "gasqualitaet": gasqualitaet,
                            }),
                        )
                        .caused_by(1),
                    ]
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
