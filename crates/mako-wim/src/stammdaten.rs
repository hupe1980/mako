//! WiM Stammdaten — master data request and transmission (PIDs 17132, 17102–17133).
//!
//! Models the BDEW WiM process for requesting and transmitting metering-point
//! master data between MSB and NB in the **Strom** domain.
//!
//! # Process flow
//!
//! ```text
//! NB → ORDERS 17132 (Anfrage zur Übermittlung von Stammdaten Strom) ── MSB
//!                                                                         │
//! NB ← ORDERS 17102–17133 (Stammdatenübermittlung) ←── 5 Werktage ───────┘
//!    or ORDRSP (rejection if data unavailable)
//! ```
//!
//! Unlike the Geräteübernahme process, the response to a Stammdaten request is
//! itself an ORDERS message (not an ORDRSP). The AHB assigns specific PIDs for
//! different categories of master data being transmitted.
//!
//! # PID assignments (ORDERS AHB fv20251001)
//!
//! | PID   | AHB description                                         | Direction |
//! |-------|---------------------------------------------------------|-----------|
//! | 17132 | Anfrage zur Übermittlung von Stammdaten **Strom**        | NB → MSB  |
//! | 17102–17133 | Stammdatenübermittlung (various data categories)  | MSB → NB  |
//!
//! **Note on 17101**: PID 17101 is "Anfrage zur Übermittlung von Stammdaten **Gas**"
//! per the ORDERS AHB — it belongs to the GeLi Gas / WiM Gas domain, **not** WiM Strom.
//! See [`ANFORDERUNG_PID_GAS`] and `mako-wim-gas`.
//!
//! **Note on 17134/17135**: These PIDs are GPKE Konfiguration processes
//! ("Einrichtung Konfiguration aufgrund Zuordnung LF") and are explicitly excluded
//! from the WiM Stammdaten response range even though they fall between 17132 and 17135.
//!
//! # Regulatory basis
//!
//! - **BDEW WiM AHB** — Stammdaten Anfrage/Übermittlung
//! - **BNetzA BK6-18-032** — 5 Werktage Frist

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    projection::Projection,
    types::{MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-stammdaten";

/// ORDERS PID for Stammdaten Anforderung **Strom** (NB → MSB).
///
/// Per ORDERS AHB fv20251001: 17132 = "Anfrage zur Übermittlung von Stammdaten Strom".
///
/// This triggers a new [`WimStammdatenWorkflow`] process on the MSB side,
/// and is sent as an outbox entry by the NB side.
///
/// # WiM Strom only
///
/// This crate (`mako-wim`) handles **Strom** stammdaten only. The Gas counterpart
/// (PID 17101) belongs to `mako-wim-gas`; see [`ANFORDERUNG_PID_GAS`].
pub const ANFORDERUNG_PID: u32 = 17132;

/// ORDERS PID for Stammdaten Anforderung **Gas** (NB → MSB, Gas domain).
///
/// Per ORDERS AHB fv20251001: 17101 = "Anfrage zur Übermittlung von Stammdaten Gas".
///
/// This constant is provided here for documentation and cross-reference only.
/// The Gas workflow is implemented in `mako-wim-gas`, not in this crate.
pub const ANFORDERUNG_PID_GAS: u32 = 17101;

/// ORDERS PIDs for Stammdatenübermittlung responses (17102–17133, MSB → NB).
///
/// These are dispatched as outbox entries by the MSB (responding party);
/// the NB (requesting party) receives them as inbound responses.
///
/// # Exclusions
///
/// PIDs 17134 and 17135 are explicitly excluded:
/// - 17134: "Einrichtung Konfiguration aufgrund Zuordnung LF (NB an MSB)" — **GPKE Konfiguration**
/// - 17135: "Einrichtung Konfiguration aufgrund Zuordnung LF (MSB an MSB)" — **GPKE Konfiguration**
///
/// Both are GPKE-owned and registered by `mako-gpke`. Including them here would
/// cause a PID routing conflict on any instance running both WiM and GPKE modules.
pub const UEBERMITTLUNG_PIDS: std::ops::RangeInclusive<u32> = 17102..=17133;

/// Deadline label for the 5-Werktage data-transmittal window (WiM BK6-18-032).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 5, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., STAMMDATEN_DEADLINE_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const STAMMDATEN_DEADLINE_LABEL: &str = "wim-stammdaten-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Stammdaten workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StammdatenEvent {
    /// ORDERS 17132 (Anfrage zur Übermittlung von Stammdaten Strom) received.
    AnforderungReceived {
        /// ORDERS PID (17132 for Strom).
        pid: Pruefidentifikator,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the responding party.
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Document date from DTM segment.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// EDIFACT ORDERS passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Responding party transmitted master data (ORDERS 17102–17133).
    StammdatenUebermittelt {
        /// ORDERS PID of the response (17102–17133).
        response_pid: Pruefidentifikator,
        /// Message reference of the response ORDERS.
        response_ref: MessageRef,
        /// Extracted `Standorteigenschaften` JSONB for `marktd` auto-update.
        ///
        /// Contains `regelzone` (EIC), `bilanzierungsgebiet`, `netzgebiet`,
        /// `gasqualitaet`, and `eigenschaftenStrom` / `eigenschaftenGas` arrays.
        /// `None` when the EDIFACT payload did not carry LOC/QTY/MEA Stammdaten.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        standorteigenschaften: Option<serde_json::Value>,
        /// Extracted Zaehlwerk records (from ZAK+ZE segments) for device registry.
        ///
        /// Each entry is a JSON object matching `rubo4e::current::Zaehlwerk`.
        /// Used by `makod` to auto-populate `marktd` `zaehler/zaehlwerke` on receipt.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        zaehlwerke: Vec<serde_json::Value>,
    },
    /// Request rejected (data unavailable or validation failure).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline fired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for StammdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnforderungReceived { .. } => "WimStammdatenAnforderungReceived",
            Self::ValidationPassed { .. } => "WimStammdatenValidationPassed",
            Self::StammdatenUebermittelt { .. } => "WimStammdatenUebermittelt",
            Self::Abgelehnt { .. } => "WimStammdatenAbgelehnt",
            Self::DeadlineExpired { .. } => "WimStammdatenDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the Anforderung ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StammdatenData {
    /// ORDERS PID (17132 for Strom).
    pub pid: Pruefidentifikator,
    /// GLN of the requesting party.
    pub sender: MarktpartnerCode,
    /// GLN of the responding party.
    pub receiver: MarktpartnerCode,
    /// Messlokation EIC code.
    pub melo_id: MeLo,
    /// Document date from DTM segment.
    pub document_date: String,
}

/// State of a single WiM Stammdaten process stream.
///
/// # Lifecycle
///
/// ```text
/// New → AnforderungReceived → ValidationPassed → Uebermittelt (data transmitted)
///                           ↘ Abgelehnt (validation failed)
///       ValidationPassed → Abgelehnt (data unavailable)
///       ValidationPassed → Abgelehnt (deadline expired)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum StammdatenState {
    /// No events yet.
    #[default]
    New,
    /// Anforderung received; awaiting validation.
    AnforderungReceived(StammdatenData),
    /// Validation passed; data retrieval in progress.
    ValidationPassed(StammdatenData),
    /// Master data transmitted successfully.
    Uebermittelt {
        /// Captured process data.
        data: StammdatenData,
        /// ORDERS PID of the transmitted response.
        response_pid: Pruefidentifikator,
    },
    /// Request rejected (validation failure, data unavailable, or deadline).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl StammdatenState {
    /// Returns `true` if the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Uebermittelt { .. } | Self::Abgelehnt { .. })
    }

    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnforderungReceived(_) => "AnforderungReceived",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Uebermittelt { .. } => "Uebermittelt",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Stammdaten workflow.
#[derive(Clone)]
pub enum StammdatenCommand {
    /// Inbound ORDERS 17132 — Anfrage zur Übermittlung von Stammdaten Strom (NB → MSB).
    ///
    /// Domain fields extracted and EDIFACT validation performed by the
    /// transport boundary **before** constructing this command.
    ReceiveAnforderung {
        /// ORDERS PID (17132 for Strom).
        pid: Pruefidentifikator,
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Document date from DTM segment.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if EDIFACT profile validation succeeded.
        validation_passed: bool,
        /// Validation error messages when `validation_passed = false`.
        validation_errors: Vec<String>,
    },
    /// Transmit master data as an ORDERS response (PIDs 17102–17133).
    ///
    /// **BNetzA BK6-18-032**: Data must be transmitted within **5 Werktage**
    /// of receiving the Anforderung.
    TransmitStammdaten {
        /// ORDERS PID of the response (17102–17133).
        response_pid: Pruefidentifikator,
        /// Message reference assigned to the outbound ORDERS.
        response_ref: MessageRef,
        /// Extracted `Standorteigenschaften` JSONB for `marktd` auto-update.
        ///
        /// Derived from LOC/QTY/MEA segments of the ORDERS payload.
        /// Pass `None` when the EDIFACT does not carry location attributes.
        standorteigenschaften: Option<serde_json::Value>,
        /// Extracted `Zaehlwerk` records from ZAK+ZE segments.
        ///
        /// Used by `makod` to auto-populate device registers in `marktd`.
        zaehlwerke: Vec<serde_json::Value>,
    },
    /// Reject the data request (data unavailable, permission denied, etc.).
    RejectAnforderung {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for StammdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Stammdaten workflow — Strom (ORDERS 17132 inbound, responses 17102–17133).
///
/// Implements the BDEW WiM master data request/response process from the
/// **responding party's perspective** (MSB receiving NB's Anforderung) and
/// from the **requesting party's perspective** (NB receiving MSB's response).
pub struct WimStammdatenWorkflow;

impl Workflow for WimStammdatenWorkflow {
    type State = StammdatenState;
    type Event = StammdatenEvent;
    type Command = StammdatenCommand;

    /// Deadline compensation for the WiM Stammdaten 5-Werktage response window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"wim-stammdaten-deadline"` | `AnforderungReceived` or `ValidationPassed` | `TimeoutExpired` | BK6-18-032 — 5 Werktage Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                STAMMDATEN_DEADLINE_LABEL,
                StammdatenState::AnforderungReceived(_) | StammdatenState::ValidationPassed(_),
            ) => Some(StammdatenCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            StammdatenEvent::AnforderungReceived {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                ..
            } => StammdatenState::AnforderungReceived(StammdatenData {
                pid: *pid,
                sender: sender.clone(),
                receiver: receiver.clone(),
                melo_id: melo_id.clone(),
                document_date: document_date.clone(),
            }),
            StammdatenEvent::ValidationPassed { .. } => {
                if let StammdatenState::AnforderungReceived(data) = state {
                    StammdatenState::ValidationPassed(data)
                } else {
                    state
                }
            }
            StammdatenEvent::StammdatenUebermittelt { response_pid, .. } => {
                if let StammdatenState::ValidationPassed(data) = state {
                    StammdatenState::Uebermittelt {
                        data,
                        response_pid: *response_pid,
                    }
                } else {
                    state
                }
            }
            StammdatenEvent::Abgelehnt { reason } => StammdatenState::Abgelehnt {
                reason: reason.clone(),
            },
            StammdatenEvent::DeadlineExpired { label, .. } => match state {
                s if s.is_terminal() => s,
                _ => StammdatenState::Abgelehnt {
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
            StammdatenCommand::ReceiveAnforderung {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, StammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if pid.as_u32() != ANFORDERUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Stammdaten-Anforderung PID (expected {ANFORDERUNG_PID})",
                        pid.as_u32()
                    )));
                }
                let mut events = vec![StammdatenEvent::AnforderungReceived {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(StammdatenEvent::ValidationPassed { message_ref });
                } else {
                    events.push(StammdatenEvent::Abgelehnt {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            StammdatenCommand::TransmitStammdaten {
                response_pid,
                response_ref,
                standorteigenschaften,
                zaehlwerke,
            } => {
                let StammdatenState::ValidationPassed(data) = &state else {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                };
                if !UEBERMITTLUNG_PIDS.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Stammdaten-Übermittlung PID (expected 17102–17133)",
                        response_pid.as_u32()
                    )));
                }
                let melo_id = data.melo_id.as_str().to_owned();
                let event = StammdatenEvent::StammdatenUebermittelt {
                    response_pid,
                    response_ref,
                    standorteigenschaften: standorteigenschaften.clone(),
                    zaehlwerke: zaehlwerke.clone(),
                };

                // If the response carries Stammdaten (ZAK+ZE registers or Standorteigenschaften),
                // emit a ProcessCompleted outbox entry so that `marktd` can auto-update the
                // ZaehlzeitRegister and Standorteigenschaften columns.
                // `recipient` is empty — this entry is consumed by the ERP/marktd webhook,
                // not by an EDIFACT AS4 recipient.
                if !zaehlwerke.is_empty() || standorteigenschaften.is_some() {
                    let mut payload = serde_json::json!({
                        "melo_id": melo_id,
                        "pid":     response_pid.as_u32(),
                    });
                    if !zaehlwerke.is_empty() {
                        payload["zaehlwerke"] = serde_json::Value::Array(zaehlwerke);
                    }
                    if let Some(se) = standorteigenschaften {
                        payload["standorteigenschaften"] = se;
                    }
                    let outbox =
                        mako_engine::outbox::PendingOutbox::new("ProcessCompleted", "", payload);
                    Ok(mako_engine::workflow::WorkflowOutput::with_outbox(
                        vec![event],
                        vec![outbox],
                    ))
                } else {
                    Ok(vec![event].into())
                }
            }

            StammdatenCommand::RejectAnforderung { reason } => {
                if !matches!(
                    state,
                    StammdatenState::AnforderungReceived(_) | StammdatenState::ValidationPassed(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "AnforderungReceived or ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(vec![StammdatenEvent::Abgelehnt { reason }].into())
            }

            StammdatenCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![StammdatenEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Stammdaten process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`
/// for the primary domain fields (`melo_id`, `sender`). The `response_pid`
/// remains `Option` as it is only populated by the `StammdatenUebermittelt` event.
#[derive(Debug)]
pub enum StammdatenRecord {
    /// No `AnforderungReceived` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `AnforderungReceived` event applied; primary process fields now available.
    Active {
        /// Current lifecycle stage (e.g. "AnforderungReceived", "Uebermittelt", "Abgelehnt").
        status: &'static str,
        /// Messlokation EIC code from the Anforderung.
        melo_id: MeLo,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// ORDERS PID used for the data transmission response (set after `StammdatenUebermittelt`).
        response_pid: Option<Pruefidentifikator>,
        /// Total events applied.
        event_count: usize,
    },
}

impl StammdatenRecord {
    /// Current lifecycle status label, suitable for logging and serialisation.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied to this stream.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }

    /// Domain data for this record if it has been initiated, or `None` if `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<StammdatenRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id,
                sender,
                response_pid,
                ..
            } => Some(StammdatenRecordData {
                melo_id,
                sender,
                response_pid: response_pid.as_ref(),
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`StammdatenRecord`].
#[derive(Debug, Clone, Copy)]
pub struct StammdatenRecordData<'a> {
    /// Messlokation EIC code from the Anforderung.
    pub melo_id: &'a MeLo,
    /// GLN of the requesting party.
    pub sender: &'a MarktpartnerCode,
    /// ORDERS PID used for the data transmission response (if already transmitted).
    pub response_pid: Option<&'a Pruefidentifikator>,
}

impl Default for StammdatenRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model tracking WiM Stammdaten streams.
#[derive(Debug, Default)]
pub struct StammdatenProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, StammdatenRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for StammdatenProjection {
    fn name(&self) -> &'static str {
        "StammdatenProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<StammdatenEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            StammdatenRecord::New { event_count }
            | StammdatenRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            StammdatenEvent::AnforderungReceived {
                sender, melo_id, ..
            } => {
                let count = record.event_count();
                *record = StammdatenRecord::Active {
                    status: "AnforderungReceived",
                    melo_id,
                    sender,
                    response_pid: None,
                    event_count: count,
                };
            }
            StammdatenEvent::ValidationPassed { .. } => {
                if let StammdatenRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            StammdatenEvent::StammdatenUebermittelt { response_pid, .. } => {
                if let StammdatenRecord::Active {
                    status,
                    response_pid: rp,
                    ..
                } = record
                {
                    *status = "Uebermittelt";
                    *rp = Some(response_pid);
                }
            }
            StammdatenEvent::Abgelehnt { .. } | StammdatenEvent::DeadlineExpired { .. } => {
                if let StammdatenRecord::Active { status, .. } = record {
                    *status = "Abgelehnt";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn anforderung_cmd(pid: u32, valid: bool) -> StammdatenCommand {
        StammdatenCommand::ReceiveAnforderung {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("9900123456789"),
            receiver: MarktpartnerCode::new("4012345000023"),
            melo_id: MeLo::new("DE00056789012"),
            document_date: "20260101".to_owned(),
            message_ref: MessageRef::new("MSG-17132-001"),
            validation_passed: valid,
            validation_errors: if valid {
                vec![]
            } else {
                vec!["error".to_owned()]
            },
        }
    }

    #[test]
    fn happy_path_anforderung_to_uebermittlung() {
        let state = StammdatenState::default();
        let events = WimStammdatenWorkflow::handle(&state, anforderung_cmd(17132, true)).unwrap();
        let state = events.iter().fold(state, WimStammdatenWorkflow::apply);
        assert!(matches!(state, StammdatenState::ValidationPassed(_)));

        let events = WimStammdatenWorkflow::handle(
            &state,
            StammdatenCommand::TransmitStammdaten {
                response_pid: Pruefidentifikator::new(17102).unwrap(),
                response_ref: MessageRef::new("MSG-17102-001"),
                standorteigenschaften: None,
                zaehlwerke: vec![],
            },
        )
        .unwrap();
        let state = events.iter().fold(state, WimStammdatenWorkflow::apply);
        assert!(matches!(state, StammdatenState::Uebermittelt { .. }));
    }

    #[test]
    fn validation_failure_rejects() {
        let state = StammdatenState::default();
        let events = WimStammdatenWorkflow::handle(&state, anforderung_cmd(17132, false)).unwrap();
        let state = events.iter().fold(state, WimStammdatenWorkflow::apply);
        assert!(matches!(state, StammdatenState::Abgelehnt { .. }));
    }

    #[test]
    fn wrong_anforderung_pid_is_rejected() {
        let state = StammdatenState::default();
        let result = WimStammdatenWorkflow::handle(&state, anforderung_cmd(17102, true));
        assert!(
            result.is_err(),
            "PID 17102 is Übermittlung, not Anforderung"
        );
    }

    #[test]
    fn deadline_on_active_rejects() {
        let state = StammdatenState::default();
        let events = WimStammdatenWorkflow::handle(&state, anforderung_cmd(17132, true)).unwrap();
        let state = events.iter().fold(state, WimStammdatenWorkflow::apply);
        let events = WimStammdatenWorkflow::handle(
            &state,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "wim-stammdaten-deadline".into(),
            },
        )
        .unwrap();
        let state = events.iter().fold(state, WimStammdatenWorkflow::apply);
        assert!(matches!(state, StammdatenState::Abgelehnt { .. }));
    }

    #[test]
    fn deadline_on_terminal_is_noop() {
        let terminal = StammdatenState::Abgelehnt {
            reason: "test".to_owned(),
        };
        let events = WimStammdatenWorkflow::handle(
            &terminal,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "late".into(),
            },
        )
        .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn all_uebermittlung_pids_accepted() {
        for pid in UEBERMITTLUNG_PIDS {
            let state = StammdatenState::default();
            let events =
                WimStammdatenWorkflow::handle(&state, anforderung_cmd(17132, true)).unwrap();
            let state = events.iter().fold(state, WimStammdatenWorkflow::apply);
            let result = WimStammdatenWorkflow::handle(
                &state,
                StammdatenCommand::TransmitStammdaten {
                    response_pid: Pruefidentifikator::new(pid).unwrap(),
                    response_ref: MessageRef::new("MSG-RESP"),
                    standorteigenschaften: None,
                    zaehlwerke: vec![],
                },
            );
            assert!(result.is_ok(), "PID {pid} must be accepted as Übermittlung");
        }
    }
}
