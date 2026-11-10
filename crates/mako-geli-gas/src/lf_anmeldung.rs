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
//! ## Identification
//!
//! A Gas Vorgang names its Marktlokation in `SG5 LOC+172` Meldepunkt, „genau
//! einmal je SG4 IDE" (Bedingung `[2061]`), and the 44001 identifies it by its
//! ID under `IMD++Z36+Z12` (UTILMD AHB Gas 1.2 Kap. 5.6, S. 44, 46). There is no
//! second Lokation beside it: the LFN supplies the Gas-MaLo-ID, and there is
//! no NB-side resolution of a missing one.
//!
//! ## What the 44001 carries beside it
//!
//! The Anmeldung NN states the Lieferant's contract facts — Abrechnungsintervall,
//! Gruppenzuordnung, Netznutzungsvertrag, Gemeinderabatt, the Kunde and the
//! Ablesekarte — as `SG6`/`SG8`/`SG12` segments the column makes Muss. They
//! reach the workflow as [`Stammdaten`] assembled at the command door, which
//! refuses a fact the caller does not state; the workflow carries them onto
//! the outbox entry whole.
//!
//! ## Comparison with GPKE
//!
//! | Aspect              | GPKE (`gpke-lf-anmeldung`)     | GeLi Gas (`geli-gas-lf-anmeldung`)    |
//! |---------------------|--------------------------------|----------------------------------------|
//! | Request PID         | 55001                          | 44001                                  |
//! | Response PIDs       | 55002 (✓) / 55003 (✗)          | 44002 (✓) / 44003 (✗)                 |
//! | Antwortfrist        | clock time des 1. WT (GPKE Teil 2) | 4 / 3 Werktage (BK7-24-01-009 Kap. 3.1–3.2.3) |
//! | MaLo source         | API-Webdienste Strom optional  | ERP must supply `malo_id`              |
//! | Activation step     | explicit `Activate` command    | explicit `Activate` command            |
//!
//! Both emit a `ProcessInitiated` CloudEvent on spawn.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::payload::Stammdaten;
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

/// The Abmeldung NN — the one Anfrage whose `SG4 STS+7` Transaktionsgrund is
/// Muss (UTILMD AHB Gas 1.2 Kap. 5.5, S. 40, `[2061]`).
pub const ABMELDUNG_PID: u32 = 44004;

/// The 44004 Transaktionsgründe that name the Beginn instead of the Ende.
///
/// „Aufhebung einer zukünftigen Zuordnung wegen Auszug des Kunden / … wegen
/// Stilllegung / … wegen aufgehobenem Vertragsverhältnis": under these the
/// column makes `DTM+92` „Beginn zum" Muss (Bedingung `[7]`), filled „mit dem
/// ursprünglich vom NB bestätigten Beginndatum" (`[577]`), and `DTM+93` „Ende
/// zum" only under every other code (`[11]`) — UTILMD AHB Gas 1.2 Kap. 5.5,
/// S. 39–41.
pub const ABMELDUNG_AUFHEBUNG_GRUENDE: &[&str] = &["ZG9", "ZH1", "ZH2"];

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

/// The Bestätigungen among [`ANTWORT_PIDS_LF`] — the answers that accept the
/// LFN's request. Every other answer PID is the matching Ablehnung.
pub const BESTAETIGUNG_PIDS_LF: &[u32] = &[44002, 44005, 44017];

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
        /// Gas Marktlokations-ID — rendered into `SG5 LOC+172`, never `IDE`.
        malo_id: MaLo,
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
        /// `SG4 STS+E01` — the answer's code and Codeliste, which is what the
        /// refusal means; `None` where the response carries none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<Antwortcode>,
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

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::Initiated {
                pruefidentifikator, ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: None,
            }),
            Self::AntwortReceived {
                response_pid,
                response_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(response_pid.as_u32()),
                message_ref: Some(response_ref.as_str().to_owned()),
            }),
            Self::Activated | Self::DeadlineExpired { .. } => None,
        }
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortReceived { antwort, .. } => antwort.clone(),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `Initiated` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeliGasLfAnmeldungData {
    /// PID of the outbound Anfrage.
    pub pruefidentifikator: Pruefidentifikator,
    /// Gas Marktlokations-ID.
    pub malo_id: MaLo,
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
    /// GNB rejected.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Supply activated (end state for 44001 Lieferbeginn).
    Completed(GeliGasLfAnmeldungData),
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GeLi Gas 3.0 Kap. 3.2.3 gives the GNB its answer window on the Anmeldung and no consequence of the GNB missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GeliGasLfAnmeldungState>,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for GeliGasLfAnmeldungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            // Awaiting the counterparty inside its published window, or supply
            // confirmed and running.
            Self::Pending(_) | Self::Active(_) => true,
            // `Rejected` (GNB refusal) and `Completed` are terminal;
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
            Self::FristAbgelaufen { .. } => "FristAbgelaufen",
        }
    }

    /// The state the process is waiting in — itself, unless a window lapsed.
    #[must_use]
    pub fn offen(&self) -> &Self {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.offen(),
            other => other,
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
    /// ERP instructs the engine to initiate a Gas Lieferbeginn, Lieferende or
    /// Kündigung.
    ///
    /// The engine enqueues the outbound UTILMD G 44001/44004/44016 as a
    /// `PendingOutbox` entry; the AS4 sender delivers it.
    InitiateAnmeldung {
        /// Outbound request PID (44001 Lieferbeginn, 44004 Lieferende, 44016
        /// Kündigung).
        pid: Pruefidentifikator,
        /// Our own GLN (the Lieferant / LFN).
        sender: MarktpartnerCode,
        /// GNB GLN (resolved from MaLo cache).
        receiver: MarktpartnerCode,
        /// Gas Marktlokations-ID → `SG5 LOC+172`.
        malo_id: MaLo,
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
        /// The Lieferant's contract facts a 44001 carries as `SG6`/`SG8`/`SG12`
        /// segments (UTILMD AHB Gas 1.2 Kap. 5.6) — assembled at the command
        /// door, carried onto the outbox entry whole. Empty on 44004/44016.
        #[serde(default, skip_serializing_if = "Stammdaten::is_empty")]
        stammdaten: Stammdaten,
        /// UTC wall-clock time when the ERP command was received.
        #[serde(with = "time::serde::rfc3339")]
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
        /// `SG4 STS+E01` DE 9013 and DE 1131 of the response.
        antwort: Option<Antwortcode>,
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

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The GNB's answer window is open while the Anmeldung waits for it
/// (`Pending`).
fn window_open(label: &str, state: &GeliGasLfAnmeldungState) -> bool {
    label == GNB_RESPONSE_WINDOW_LABEL && matches!(state, GeliGasLfAnmeldungState::Pending(_))
}

impl Workflow for GeliGasLfAnmeldungWorkflow {
    type State = GeliGasLfAnmeldungState;
    type Event = GeliGasLfAnmeldungEvent;
    type Command = GeliGasLfAnmeldungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[GNB_RESPONSE_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GeliGasLfAnmeldungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GeliGasLfAnmeldungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GeliGasLfAnmeldungEvent::Initiated {
                pruefidentifikator,
                malo_id,
                sender,
                receiver,
                process_date,
            } => GeliGasLfAnmeldungState::Pending(GeliGasLfAnmeldungData {
                pruefidentifikator: *pruefidentifikator,
                malo_id: malo_id.clone(),
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
                GeliGasLfAnmeldungState::Pending(_) => GeliGasLfAnmeldungState::FristAbgelaufen {
                    label: label.to_string(),
                    offen: Box::new(state),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            GeliGasLfAnmeldungCommand::InitiateAnmeldung {
                pid,
                sender,
                receiver,
                malo_id,
                process_date,
                transaktionsgrund,
                bilanzkreis,
                stammdaten,
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
                let abmeldung = pid.as_u32() == ABMELDUNG_PID;
                if abmeldung && transaktionsgrund.as_deref().is_none_or(str::is_empty) {
                    return Err(WorkflowError::rejected(
                        "Gas-Abmeldung 44004 ohne Transaktionsgrund: die UTILMD AHB Gas 1.2 \
                         macht SG4 STS+7 zur Muss-Angabe ([2061]).",
                    ));
                }
                // [7]/[11]: an Aufhebung names the confirmed Beginn, not an Ende.
                let dtm_qualifier = (abmeldung
                    && transaktionsgrund
                        .as_deref()
                        .is_some_and(|g| ABMELDUNG_AUFHEBUNG_GRUENDE.contains(&g)))
                .then_some("92");

                let event = GeliGasLfAnmeldungEvent::Initiated {
                    pruefidentifikator: pid,
                    malo_id: malo_id.clone(),
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    process_date: process_date.clone(),
                };

                // Enqueue the outbound UTILMD G as a PendingOutbox entry.
                // The renderer derives document_date and message_ref at dispatch
                // time.
                let mut utilmd = serde_json::json!({
                    "pid":               pid.as_u32(),
                    "sender":            sender.as_str(),
                    "receiver":          receiver.as_str(),
                    "malo":              malo_id.as_str(),
                    "process_date":      process_date,
                    // SG4 STS Transaktionsgrund (DE 9013).
                    "transaktionsgrund": transaktionsgrund,
                    // `SG10 CCI+Z19` DE 7037 — the Gas shape of the
                    // Bilanzkreis; GeLi Gas has no Produktpaket.
                    "bilanzkreis":       bilanzkreis,
                });
                if let Some(q) = dtm_qualifier {
                    utilmd["dtm_qualifier"] = q.into();
                }
                if !stammdaten.is_empty() {
                    utilmd["stammdaten"] = serde_json::to_value(&stammdaten)
                        .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                }
                let outbox = vec![
                    PendingOutbox::new("UTILMD", receiver.as_str(), utilmd),
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
                antwort,
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
                // The date the Anfrage asked for — the Lieferbeginn on an
                // Anmeldung, the Lieferende on an Abmeldung — and the Lieferant
                // it is about, this one: without them marktd dates an ending
                // the day the answer arrived and ends every assignment of a
                // tranchierte Marktlokation.
                let (process_date, lieferant) = match state {
                    GeliGasLfAnmeldungState::Pending(data) => (
                        Some(data.process_date.clone()),
                        Some(data.sender.as_str().to_owned()),
                    ),
                    _ => (None, None),
                };
                let lieferant_key = if matches!(response_pid.as_u32(), 44_005 | 44_006) {
                    "old_supplier"
                } else {
                    "new_supplier"
                };
                let mut completed = serde_json::json!({
                    "pid":      response_pid.as_u32(),
                    "malo_id":  malo_id_str,
                    "process_date": process_date,
                    "accepted": accepted,
                    "outcome":  if accepted { "accepted" } else { "rejected" },
                    "reason":   reason,
                    "antwortcode": antwort.as_ref().map(|a| a.code.as_str()),
                    "codeliste":   antwort.as_ref().and_then(|a| a.codeliste.as_deref()),
                });
                completed[lieferant_key] = serde_json::json!(lieferant);
                let outbox = vec![PendingOutbox::new("ProcessCompleted", "", completed)];

                Ok(WorkflowOutput::with_outbox(
                    vec![GeliGasLfAnmeldungEvent::AntwortReceived {
                        response_pid,
                        accepted,
                        reason,
                        response_ref,
                        antwort,
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
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
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
            process_date: "20261001".to_owned(),
            transaktionsgrund: None,
            bilanzkreis: Some("9870000000006".to_owned()),
            stammdaten: Stammdaten::default(),
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

    /// The contract facts the door assembled ride the UTILMD entry whole, and
    /// the entry names the Marktlokation alone — the 44001 column has no place
    /// for a second Lokation beside it (UTILMD AHB Gas 1.2 Kap. 5.6, S. 46).
    #[test]
    fn the_44001_carries_its_stammdaten_and_no_second_lokation() {
        use mako_engine::payload::{StammdatenBlock, StammdatenSegment};
        let stammdaten = Stammdaten {
            bloecke: vec![StammdatenBlock {
                seq: "Z12".to_owned(),
                segmente: vec![StammdatenSegment {
                    tag: "QTY".to_owned(),
                    elemente: vec![vec!["Z16".to_owned(), "0".to_owned(), "P1".to_owned()]],
                }],
            }],
            ..Stammdaten::default()
        };
        let cmd = GeliGasLfAnmeldungCommand::InitiateAnmeldung {
            pid: pid(44001),
            sender: mc("9800000000005"),
            receiver: mc("9800357000005"),
            malo_id: MaLo::new("51238696012"),
            process_date: "20261001".to_owned(),
            transaktionsgrund: Some("E03".to_owned()),
            bilanzkreis: Some("THE0BFH000000001".to_owned()),
            stammdaten: stammdaten.clone(),
            received_at: time::OffsetDateTime::now_utc(),
        };
        let out = GeliGasLfAnmeldungWorkflow::handle(&GeliGasLfAnmeldungState::New, cmd).unwrap();
        let utilmd = &out.outbox[0];
        assert_eq!(&*utilmd.message_type, "UTILMD");
        let carried: Stammdaten =
            serde_json::from_value(utilmd.payload["stammdaten"].clone()).unwrap();
        assert_eq!(carried, stammdaten);
        assert!(utilmd.payload.get("zaehlpunkt").is_none());
        assert_eq!(utilmd.payload["malo"], "51238696012");
    }

    #[test]
    fn gnb_bestaetigung_44002() {
        let data = GeliGasLfAnmeldungData {
            pruefidentifikator: pid(44001),
            malo_id: MaLo::new("DE0001234567890"),
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
            antwort: None,
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
            antwort: None,
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
            process_date: "20261001".to_owned(),
            transaktionsgrund: None,
            bilanzkreis: Some("9870000000006".to_owned()),
            stammdaten: Stammdaten::default(),
            received_at: time::OffsetDateTime::now_utc(),
        };
        assert!(GeliGasLfAnmeldungWorkflow::handle(&state, cmd).is_err());
    }

    fn abmeldung(transaktionsgrund: Option<&str>) -> GeliGasLfAnmeldungCommand {
        GeliGasLfAnmeldungCommand::InitiateAnmeldung {
            pid: pid(ABMELDUNG_PID),
            sender: mc("9800000000001"),
            receiver: mc("9800357000005"),
            malo_id: MaLo::new("51238696012"),
            process_date: "20261231".to_owned(),
            transaktionsgrund: transaktionsgrund.map(str::to_owned),
            bilanzkreis: None,
            stammdaten: Stammdaten::default(),
            received_at: time::OffsetDateTime::now_utc(),
        }
    }

    /// `SG4 STS+7` is Muss on the 44004 (`[2061]`): without it nothing is
    /// written.
    #[test]
    fn a_44004_without_a_transaktionsgrund_is_refused() {
        let state = GeliGasLfAnmeldungState::New;
        for grund in [None, Some("")] {
            let refused = GeliGasLfAnmeldungWorkflow::handle(&state, abmeldung(grund));
            assert!(
                refused.is_err_and(|e| e.to_string().contains("STS+7")),
                "{grund:?}"
            );
        }
    }

    /// An Aufhebung names the confirmed Beginn (`DTM+92`, `[7]`); every other
    /// Grund the Ende, the PID's own qualifier (`[11]`).
    #[test]
    fn an_aufhebung_44004_names_the_beginn() {
        let state = GeliGasLfAnmeldungState::New;
        for (grund, qualifier) in [
            ("ZG9", Some("92")),
            ("ZH1", Some("92")),
            ("ZH2", Some("92")),
            ("ZT5", None),
            ("E01", None),
        ] {
            let out = GeliGasLfAnmeldungWorkflow::handle(&state, abmeldung(Some(grund)))
                .expect("the 44004 is written");
            let utilmd = &out.outbox[0].payload;
            assert_eq!(utilmd["transaktionsgrund"], grund);
            assert_eq!(
                utilmd.get("dtm_qualifier").and_then(|v| v.as_str()),
                qualifier,
                "{grund}"
            );
        }
    }
}
