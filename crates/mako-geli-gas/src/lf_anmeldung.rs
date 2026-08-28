//! GeLi Gas LFN-side Lieferbeginn/Lieferende/Kündigung workflow.
//!
//! Handles the Lieferant's perspective of initiating a GeLi Gas supplier-change:
//! the LFN sends UTILMD G 44001 outbound to the GNB and tracks the GNB's
//! response (44002 Bestätigung / 44003 Ablehnung).
//!
//! ## Covered Prüfidentifikatoren
//!
//! | PID   | Direction    | Description                                     |
//! |-------|--------------|-------------------------------------------------|
//! | 44001 | LFN → GNB    | Anmeldung NN / Lieferbeginn (spawns this process)|
//! | 44004 | LFN → GNB    | Abmeldung NN / Lieferende (spawns this process) |
//! | 44002 | GNB → LFN    | Bestätigung Anmeldung NN (inbound response)     |
//! | 44003 | GNB → LFN    | Ablehnung Anmeldung NN (inbound response)       |
//! | 44005 | GNB → LFN    | Bestätigung Abmeldung NN (inbound response)     |
//! | 44006 | GNB → LFN    | Ablehnung Abmeldung NN (inbound response)       |
//!
//! ## Regulatory basis
//!
//! **BK7-24-01-009** (GeLi Gas 3.0, effective 2026-04-01).
//! Counterparty answer deadline: the published window per PID from
//! `mako_fristen::antwort` — 4 Werktage for 44001, 3 for 44004 and 44016.
//!
//! ## Key Gas-specific requirement (R3)
//!
//! **Both `malo_id` (`SG5 LOC+Z16`) and `zaehlpunkt` are mandatory** in
//! UTILMD G 44001 per AHB rules `AHB-44001-IDE-M` and `AHB-44001-RFF-M`.
//! The LFN must supply both fields from the ERP command; there is no
//! NB-side resolution of a missing Gas-MaLo-ID.
//!
//! ## Comparison with GPKE
//!
//! | Aspect              | GPKE (`gpke-lf-anmeldung`)     | GeLi Gas (`geli-gas-lf-anmeldung`)    |
//! |---------------------|--------------------------------|----------------------------------------|
//! | Request PID         | 55001                          | 44001                                  |
//! | Response PIDs       | 55002 (✓) / 55003 (✗)          | 44002 (✓) / 44003 (✗)                 |
//! | Antwortfrist        | clock time des 1. WT (GPKE Teil 2) | 4 / 3 Werktage (BK7-24-01-009 Kap. 3.1–3.2.3) |
//! | MaLo source         | API-Webdienste Strom optional  | ERP must supply `malo_id` + `zaehlpunkt` |
//! | Activation step     | explicit `Activate` command    | explicit `Activate` command            |
//! | ProcessInitiated CE | ✅ emitted on spawn            | ✅ emitted on spawn                    |

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use time::OffsetDateTime;

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the GeLi Gas LFN-side Anmeldung workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-lf-anmeldung";

/// Outbound request PIDs initiated by the LFN (ERP-driven).
///
/// | PID   | Process variant                             | Answered by | Frist |
/// |-------|---------------------------------------------|---|---|
/// | 44001 | Anmeldung NN — Lieferbeginn (supply start)   | GNB | 4 WT |
/// | 44004 | Abmeldung NN — Lieferende (supply end)      | GNB | 3 WT |
/// | 44016 | Kündigung beim Altlieferanten (LFN → LFA)   | LFA | 3 WT |
///
/// 44016 does not go to the grid operator: GeLi Gas 3.0 § 3.1 has the
/// Neulieferant send the Kündigung straight to the Altlieferant, who must
/// answer „unverzüglich, spätestens jedoch bis zum Ablauf des 3. Werktages".
pub const ANFRAGE_PIDS_LF: &[u32] = &[44001, 44004, 44016];

/// The one Anfrage-PID whose `SG10 CCI+Z19` Bilanzkreis is **Muss**.
///
/// UTILMD AHB Gas 1.2 marks the segment Muss on the Anmeldung NN. An Abmeldung
/// (44004) and a Kündigung (44016) register nothing and carry none. GeLi Gas has
/// no Produktpaket — the Strom `SG8 SEQ+Z79` shape is not sendable here.
pub const ANMELDUNG_PID_MIT_BILANZKREIS: u32 = 44001;

/// Inbound response PIDs that resume this workflow.
///
/// | PID   | Meaning                          |
/// |-------|----------------------------------|
/// | 44002 | Bestätigung Anmeldung NN (✓)     |
/// | 44003 | Ablehnung Anmeldung NN (✗)       |
/// | 44005 | Bestätigung Abmeldung NN (✓)     |
/// | 44006 | Ablehnung Abmeldung NN (✗)       |
/// | 44017 | Bestätigung Kündigung (✓)        |
/// | 44018 | Ablehnung Kündigung (✗)          |
pub const ANTWORT_PIDS_LF: &[u32] = &[44002, 44003, 44005, 44006, 44017, 44018];

/// Deadline label for the counterparty's published answer window.
///
/// The window itself comes from `mako_fristen::antwort` — 4 / 3 / 3 Werktage by
/// PID, never a flat number. The familiar 10 Werktage is the **LFN's own
/// Vorlauffrist** before Lieferbeginn, a different clock entirely.
pub const GNB_RESPONSE_WINDOW_LABEL: &str = "geli-gas-lf-anmeldung-antwortfrist";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas LFN-side Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasLfAnmeldungEvent {
    /// LFN-side Anmeldung initiated — outbound UTILMD G queued for AS4 delivery.
    Initiated {
        /// PID of the outbound Anfrage (44001 Lieferbeginn, 44004 Lieferende,
        /// 44016 Kündigung).
        pruefidentifikator: Pruefidentifikator,
        /// Gas Marktlokations-ID — rendered into `SG5 LOC+Z16`, never `IDE`.
        malo_id: MaLo,
        /// Zählpunktbezeichnung.
        zaehlpunkt: String,
        /// Our own GLN (the Lieferant / LFN).
        sender: MarktpartnerCode,
        /// GNB GLN (receiver).
        receiver: MarktpartnerCode,
        /// Requested Lieferbeginn, Lieferende or Kündigungstermin (YYYYMMDD).
        process_date: String,
    },
    /// GNB responded — accepted or rejected.
    AntwortReceived {
        /// PID of the inbound response (44002/44003, 44005/44006, 44017/44018).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung (accepted), `false` = Ablehnung (rejected).
        accepted: bool,
        /// Optional rejection reason from FTX segment.
        reason: Option<String>,
        /// EDIFACT message reference of the response.
        response_ref: MessageRef,
    },
    /// Accepted Lieferbeginn Gas supply activated (44001 only).
    Activated,
    /// The GNB's answer window lapsed without a Bestätigung or Ablehnung.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GeliGasLfAnmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "GeliGasLfAnmeldungInitiated",
            Self::AntwortReceived { .. } => "GeliGasLfAnmeldungAntwortReceived",
            Self::Activated => "GeliGasLfAnmeldungActivated",
            Self::DeadlineExpired { .. } => "GeliGasLfAnmeldungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `Initiated` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliGasLfAnmeldungData {
    /// PID of the outbound Anfrage.
    pub pruefidentifikator: Pruefidentifikator,
    /// Gas Marktlokations-ID.
    pub malo_id: MaLo,
    /// Zählpunktbezeichnung (RFF+Z13).
    pub zaehlpunkt: String,
    /// Our own GLN (the Lieferant).
    pub sender: MarktpartnerCode,
    /// GNB GLN.
    pub receiver: MarktpartnerCode,
    /// Requested process date (YYYYMMDD).
    pub process_date: String,
}

/// Process state for the GeLi Gas LFN-side Anmeldung workflow.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum GeliGasLfAnmeldungState {
    /// No outbound Anfrage sent yet.
    #[default]
    New,
    /// UTILMD G 44001/44002 sent; awaiting the GNB's answer (4 Werktage
    /// for a 44001 Anmeldung — GeLi Gas 3.0 Kap. 3.2.3).
    Pending(GeliGasLfAnmeldungData),
    /// GNB accepted — supply confirmed (Bestätigung received).
    Active(GeliGasLfAnmeldungData),
    /// GNB rejected or deadline expired.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Supply activated (end state for 44001 Lieferbeginn).
    Completed(GeliGasLfAnmeldungData),
}

impl mako_engine::workflow::OccupiesBusinessKey for GeliGasLfAnmeldungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Awaiting the counterparty inside its published window, or supply
            // confirmed and running.
            Self::Pending(_) | Self::Active(_) => true,
            // `Rejected` (GNB refusal or deadline) and `Completed` are terminal;
            // a corrected Gas Anmeldung after a rejection is a normal flow.
            Self::New | Self::Rejected { .. } | Self::Completed(_) => false,
        }
    }
}

impl GeliGasLfAnmeldungState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Pending(_) => "Pending",
            Self::Active(_) => "Active",
            Self::Rejected { .. } => "Rejected",
            Self::Completed(_) => "Completed",
        }
    }

    /// Returns `true` for terminal states.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Rejected { .. } | Self::Completed(_))
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas LFN-side Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasLfAnmeldungCommand {
    /// ERP instructs the engine to initiate a Gas Lieferbeginn or Lieferende.
    ///
    /// **Both `malo_id` and `zaehlpunkt` are mandatory** (BK7-24-01-009 AHB rules
    /// `AHB-44001-IDE-M` and `AHB-44001-RFF-M`). The engine enqueues an outbound
    /// UTILMD G 44001/44002 as a `PendingOutbox` entry; the AS4 sender delivers
    /// it to the GNB.
    InitiateAnmeldung {
        /// Outbound request PID (44001 = Lieferbeginn, 44002 = Lieferende).
        pid: Pruefidentifikator,
        /// Our own GLN (the Lieferant / LFN).
        sender: MarktpartnerCode,
        /// GNB GLN (resolved from MaLo cache).
        receiver: MarktpartnerCode,
        /// Gas Marktlokations-ID → `SG5 LOC+Z16`.
        malo_id: MaLo,
        /// Zählpunktbezeichnung (RFF+Z13) — mandatory for Gas.
        zaehlpunkt: String,
        /// Requested Lieferbeginn or Lieferende date (YYYYMMDD, in CET/CEST).
        process_date: String,
        /// SG4 STS Transaktionsgrund (DE9013) — `E01` Ein-/Auszug (retroactive
        /// permitted within the 6-week window for SLP metering), `E02` Einzug
        /// in Neuanlage, `E03` Wechsel (future-only, ≥ 10 WT).
        transaktionsgrund: Option<String>,
        /// `SG10 CCI+Z19` DE 7037 — the Bilanzkreis the LF registers the
        /// Marktlokation into.
        ///
        /// **Muss on 44001** (UTILMD AHB Gas 1.2). GeLi Gas has no Produktpaket:
        /// where GPKE Strom puts the Bilanzkreis in `SG8 SEQ+Z79`, Gas states it
        /// in this one segment. `None` on 44004, which registers nothing.
        bilanzkreis: Option<String>,
        /// UTC wall-clock time when the ERP command was received.
        received_at: OffsetDateTime,
    },
    /// Inbound GNB response (44002/44003, 44005/44006) received via AS4.
    HandleAntwort {
        /// PID of the inbound response.
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung (accepted), `false` = Ablehnung (rejected).
        accepted: bool,
        /// Optional rejection reason from FTX segment.
        reason: Option<String>,
        /// Message reference of the inbound response.
        response_ref: MessageRef,
    },
    /// Mark the accepted Lieferbeginn Gas supply as active.
    ///
    /// Dispatched by `processd` after confirming activation downstream (e.g.
    /// MSCONS metering data received, or ERP billing system activated).
    Activate,
    /// The GNB's answer window fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for GeliGasLfAnmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas LFN-side Anmeldung workflow (44001/44004 outbound, 44002/44003 + 44005/44006 inbound).
///
/// **Initiation:** ERP calls `geli.lieferbeginn.anmelden` via `POST /api/v1/commands`.
/// **Completion:** `Activate` command after ERP confirms supply is live.
/// **Deadline:** the window the outbound Prüfidentifikator publishes — 4
/// Werktage for a 44001 Anmeldung (GeLi Gas 3.0 Kap. 3.2.3), resolved through
/// [`mako_fristen::antwort`]. The familiar 10 Werktage is the LFN's own
/// *Vorlauffrist*, not the GNB's answer window.
pub struct GeliGasLfAnmeldungWorkflow;

impl Workflow for GeliGasLfAnmeldungWorkflow {
    type State = GeliGasLfAnmeldungState;
    type Event = GeliGasLfAnmeldungEvent;
    type Command = GeliGasLfAnmeldungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (GNB_RESPONSE_WINDOW_LABEL, GeliGasLfAnmeldungState::Pending(_)) => {
                Some(GeliGasLfAnmeldungCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GeliGasLfAnmeldungEvent::Initiated {
                pruefidentifikator,
                malo_id,
                zaehlpunkt,
                sender,
                receiver,
                process_date,
            } => GeliGasLfAnmeldungState::Pending(GeliGasLfAnmeldungData {
                pruefidentifikator: *pruefidentifikator,
                malo_id: malo_id.clone(),
                zaehlpunkt: zaehlpunkt.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
            }),
            GeliGasLfAnmeldungEvent::AntwortReceived {
                accepted, reason, ..
            } => {
                if *accepted {
                    match state {
                        GeliGasLfAnmeldungState::Pending(data) => {
                            GeliGasLfAnmeldungState::Active(data)
                        }
                        other => other,
                    }
                } else {
                    GeliGasLfAnmeldungState::Rejected {
                        reason: reason.clone().unwrap_or_else(|| "Ablehnung".to_owned()),
                    }
                }
            }
            GeliGasLfAnmeldungEvent::Activated => match state {
                GeliGasLfAnmeldungState::Active(data) => GeliGasLfAnmeldungState::Completed(data),
                other => other,
            },
            GeliGasLfAnmeldungEvent::DeadlineExpired { label, .. } => match state {
                GeliGasLfAnmeldungState::Active(_)
                | GeliGasLfAnmeldungState::Rejected { .. }
                | GeliGasLfAnmeldungState::Completed(_) => state,
                _ => GeliGasLfAnmeldungState::Rejected {
                    reason: format!("GNB response deadline expired: {label}"),
                },
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GeliGasLfAnmeldungCommand::InitiateAnmeldung {
                pid,
                sender,
                receiver,
                malo_id,
                zaehlpunkt,
                process_date,
                transaktionsgrund,
                bilanzkreis,
                received_at: _,
            } => {
                if !matches!(state, GeliGasLfAnmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ANFRAGE_PIDS_LF.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas LFN Anfrage PID (44001/44002), got {pid}",
                    )));
                }
                // UTILMD AHB Gas 1.2 marks `SG10 CCI+Z19` (Bilanzkreis) Muss on
                // a 44001; without it the GNB has no balancing circle to assign
                // the Marktlokation to. Refusing here beats sending an
                // AHB-invalid Anmeldung and waiting for the APERAK.
                if pid.as_u32() == ANMELDUNG_PID_MIT_BILANZKREIS
                    && bilanzkreis.as_deref().is_none_or(str::is_empty)
                {
                    return Err(WorkflowError::rejected(
                        "Gas-Anmeldung 44001 ohne Bilanzkreis: die UTILMD AHB Gas 1.2 \
                         macht SG10 CCI+Z19 (Bilanzkreis) zur Muss-Angabe.",
                    ));
                }

                let event = GeliGasLfAnmeldungEvent::Initiated {
                    pruefidentifikator: pid,
                    malo_id: malo_id.clone(),
                    zaehlpunkt: zaehlpunkt.clone(),
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    process_date: process_date.clone(),
                };

                // Enqueue the outbound UTILMD G as a PendingOutbox entry.
                // The renderer derives document_date and message_ref at dispatch
                // time. `zaehlpunkt` carries RFF+Z13.
                let outbox = vec![
                    PendingOutbox::new(
                        "UTILMD",
                        receiver.as_str(),
                        serde_json::json!({
                            "direction":         "outbound",
                            "pid":               pid.as_u32(),
                            "sender":            sender.as_str(),
                            "receiver":          receiver.as_str(),
                            "malo":              malo_id.as_str(),
                            "zaehlpunkt":        zaehlpunkt,
                            "process_date":      process_date,
                            // SG4 STS Transaktionsgrund — E01/E02 permit the
                            // 6-week retroactive window (AWH GeLi Gas 2.0
                            // Kap. 2.2); E03 is future-only.
                            "transaktionsgrund": transaktionsgrund,
                            // `SG10 CCI+Z19` DE 7037 — the Gas shape of the
                            // Bilanzkreis; GeLi Gas has no Produktpaket.
                            "bilanzkreis":       bilanzkreis,
                        }),
                    ),
                    // ProcessInitiated CE — notifies marktd → processd/invoicd/edmd.
                    PendingOutbox::new(
                        "ProcessInitiated",
                        receiver.as_str(),
                        serde_json::json!({
                            "pid":     pid.as_u32(),
                            "malo_id": malo_id.as_str(),
                        }),
                    )
                    .caused_by(0),
                ];

                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            GeliGasLfAnmeldungCommand::HandleAntwort {
                response_pid,
                accepted,
                reason,
                response_ref,
            } => {
                if !matches!(state, GeliGasLfAnmeldungState::Pending(_)) {
                    return Err(WorkflowError::invalid_state("Pending", state.label()));
                }
                if !ANTWORT_PIDS_LF.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GNB response PID (44002/44003, 44005/44006), got {response_pid}",
                    )));
                }

                // Include malo_id so that marktd's event_ingest can derive
                // VersorgungsStatus without relying on the CE subject (process UUID).
                let malo_id_str = if let GeliGasLfAnmeldungState::Pending(data) = state {
                    data.malo_id.as_str().to_owned()
                } else {
                    String::new()
                };
                let outbox = vec![PendingOutbox::new(
                    "ProcessCompleted",
                    "",
                    serde_json::json!({
                        "pid":      response_pid.as_u32(),
                        "malo_id":  malo_id_str,
                        "accepted": accepted,
                        "outcome":  if accepted { "accepted" } else { "rejected" },
                    }),
                )];

                Ok(WorkflowOutput::with_outbox(
                    vec![GeliGasLfAnmeldungEvent::AntwortReceived {
                        response_pid,
                        accepted,
                        reason,
                        response_ref,
                    }],
                    outbox,
                ))
            }

            GeliGasLfAnmeldungCommand::Activate => {
                if !matches!(state, GeliGasLfAnmeldungState::Active(_)) {
                    return Err(WorkflowError::invalid_state("Active", state.label()));
                }
                Ok(WorkflowOutput::events(vec![
                    GeliGasLfAnmeldungEvent::Activated,
                ]))
            }

            GeliGasLfAnmeldungCommand::TimeoutExpired { deadline_id, label } => {
                if !matches!(state, GeliGasLfAnmeldungState::Pending(_)) {
                    return Err(WorkflowError::invalid_state("Pending", state.label()));
                }
                Ok(WorkflowOutput::events(vec![
                    GeliGasLfAnmeldungEvent::DeadlineExpired { deadline_id, label },
                ]))
            }
        }
    }
}

/// Build a [`WorkflowId`] for the current BDEW format version.
#[must_use]
pub fn current_workflow_id(fv: &mako_engine::version::FormatVersion) -> WorkflowId {
    WorkflowId::new(WORKFLOW_NAME, fv.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }
    fn mc(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    #[test]
    fn happy_path_44001_lieferbeginn() {
        let state = GeliGasLfAnmeldungState::New;
        let cmd = GeliGasLfAnmeldungCommand::InitiateAnmeldung {
            pid: pid(44001),
            sender: mc("9900000000001"),
            receiver: mc("9904234560001"),
            malo_id: MaLo::new("DE0001234567890"),
            zaehlpunkt: "DE00123456789012345678901234567890".to_owned(),
            process_date: "20261001".to_owned(),
            transaktionsgrund: None,
            bilanzkreis: Some("9870000000006".to_owned()),
            received_at: time::OffsetDateTime::now_utc(),
        };
        let output = GeliGasLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(output.events.len(), 1);
        assert!(matches!(
            output.events[0],
            GeliGasLfAnmeldungEvent::Initiated { .. }
        ));
        // 2 outbox entries: UTILMD + ProcessInitiated
        assert_eq!(output.outbox.len(), 2);

        let state2 = output
            .events
            .iter()
            .fold(state, GeliGasLfAnmeldungWorkflow::apply);
        assert!(matches!(state2, GeliGasLfAnmeldungState::Pending(_)));
    }

    #[test]
    fn gnb_bestaetigung_44002() {
        let data = GeliGasLfAnmeldungData {
            pruefidentifikator: pid(44001),
            malo_id: MaLo::new("DE0001234567890"),
            zaehlpunkt: "DE00123456789012345678901234567890".to_owned(),
            sender: mc("9900000000001"),
            receiver: mc("9904234560001"),
            process_date: "20261001".to_owned(),
        };
        let state = GeliGasLfAnmeldungState::Pending(data);
        let cmd = GeliGasLfAnmeldungCommand::HandleAntwort {
            response_pid: pid(44002),
            accepted: true,
            reason: None,
            response_ref: mref("BESTMSG001"),
        };
        let output = GeliGasLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        assert!(matches!(
            output.events[0],
            GeliGasLfAnmeldungEvent::AntwortReceived { accepted: true, .. }
        ));
        assert_eq!(output.outbox.len(), 1); // ProcessCompleted

        let state2 = output
            .events
            .iter()
            .fold(state, GeliGasLfAnmeldungWorkflow::apply);
        assert!(matches!(state2, GeliGasLfAnmeldungState::Active(_)));
    }

    #[test]
    fn gnb_ablehnung_44003() {
        let data = GeliGasLfAnmeldungData {
            pruefidentifikator: pid(44001),
            malo_id: MaLo::new("DE0001234567890"),
            zaehlpunkt: "DE00123456789012345678901234567890".to_owned(),
            sender: mc("9900000000001"),
            receiver: mc("9904234560001"),
            process_date: "20261001".to_owned(),
        };
        let state = GeliGasLfAnmeldungState::Pending(data);
        let cmd = GeliGasLfAnmeldungCommand::HandleAntwort {
            response_pid: pid(44003),
            accepted: false,
            reason: Some("Z29".to_owned()),
            response_ref: mref("ABLMSG001"),
        };
        let output = GeliGasLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        let state2 = output
            .events
            .iter()
            .fold(state, GeliGasLfAnmeldungWorkflow::apply);
        assert!(matches!(state2, GeliGasLfAnmeldungState::Rejected { .. }));
    }

    #[test]
    fn wrong_pid_rejected() {
        let state = GeliGasLfAnmeldungState::New;
        let cmd = GeliGasLfAnmeldungCommand::InitiateAnmeldung {
            pid: pid(55001), // Strom PID — should be rejected
            sender: mc("9900000000001"),
            receiver: mc("9904234560001"),
            malo_id: MaLo::new("DE0001234567890"),
            zaehlpunkt: "DE00123456789012345678901234567890".to_owned(),
            process_date: "20261001".to_owned(),
            transaktionsgrund: None,
            bilanzkreis: Some("9870000000006".to_owned()),
            received_at: time::OffsetDateTime::now_utc(),
        };
        assert!(GeliGasLfAnmeldungWorkflow::handle(&state, cmd).is_err());
    }
}
