//! MaBiS-Zählpunkt lifecycle — activation and deactivation of MaBiS-ZP,
//! Zuordnungsermächtigung, and the Ausfallarbeitsüberführungszeitreihen (AAÜZ)
//! series.
//!
//! # Process overview
//!
//! Every process in this family has the same shape: one party sends an
//! **Anfrage** that activates or deactivates a MaBiS-Zählpunkt for a given
//! series, and — depending on the family — the counterparty returns an
//! **Antwort**, after which the receiving party may forward a
//! **Weiterleitung** to a third party.
//!
//! ```text
//! Anfrage ──→ (Antwort) ──→ (Weiterleitung)
//!  step 1        step 2          step 4
//! ```
//!
//! Only three of the six families carry an Antwort PID, and only two carry a
//! Weiterleitung. A family without an Antwort is **record-only**: the message
//! is validated and stored, and the process is terminal on arrival. Modelling
//! those as request/response would manufacture a deadline the AHB never
//! defines.
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt* — the Prozessschritt column is
//! what distinguishes an Anfrage (1) from an Antwort (2) and a Weiterleitung
//! (4).
//!
//! | Anfrage | Vorgang       | Antwort | Weiterleitung | Serie                              |
//! |--------:|---------------|--------:|--------------:|------------------------------------|
//! | 55062   | Aktivierung   | 55064   | —             | MaBiS-Zählpunkt                    |
//! | 55063   | Deaktivierung | 55064   | —             | MaBiS-Zählpunkt                    |
//! | 55071   | Aktivierung   | —       | —             | Zuordnungsermächtigung             |
//! | 55072   | Deaktivierung | —       | —             | Zuordnungsermächtigung             |
//! | 55197   | Aktivierung   | —       | —             | tägliche AAÜZ                      |
//! | 55198   | Deaktivierung | —       | —             | tägliche AAÜZ                      |
//! | 55199   | Aktivierung   | —       | —             | LF-AASZR                           |
//! | 55200   | Deaktivierung | —       | —             | LF-AASZR                           |
//! | 55203   | Aktivierung   | 55204   | 55205         | monatliche AAÜZ (BKV des LF)       |
//! | 55206   | Deaktivierung | 55207   | 55208         | monatliche AAÜZ (BKV des LF)       |
//! | 55209   | Aktivierung   | 55210   | 55211         | monatliche AAÜZ (BKV des anf. NB)  |
//! | 55212   | Deaktivierung | 55213   | 55214         | monatliche AAÜZ (BKV des anf. NB)  |
//!
//! The two monatliche-AAÜZ families are otherwise identical and differ **only**
//! in who receives the Weiterleitung: the BKV of the Lieferant (55205/55208)
//! versus the BKV of the anfordernder Netzbetreiber (55211/55214). They are
//! kept as separate families because collapsing them would lose the recipient
//! distinction that is the entire reason BDEW assigned separate codes.
//!
//! PID **55064** is shared: it is the Antwort to both 55062 and 55063. The
//! answering PID therefore cannot be derived from the request by arithmetic —
//! it comes from the table below.
//!
//! # Not in this family
//!
//! 55218 and 55220 (Abr.-Daten NNA) sit in the same numeric neighbourhood but
//! belong to **GPKE Teil 2**, not MaBiS. 55215–55217, 55219, 55221 and 55222
//! are unassigned. Neither group is routed here.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 (MaBiS)** — Bilanzkreisabrechnung, ZP
//!   activation and the AAÜZ series
//! - **UTILMD AHB Strom S2.1 / S2.2** — message format
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ AnfrageErhalten ─┬─ (validation failed) ─→ ValidationFailed  (terminal)
//!                      ├─ (no Antwort PID)    ─→ Erfasst           (terminal)
//!                      └─ AntwortGesendet ────┬─ (abgelehnt) ──────→ Abgelehnt (terminal)
//!                                             └─ (bestätigt) ──────→ Bestaetigt
//!                                                  └─ WeiterleitungGesendet → Weitergeleitet (terminal)
//! ```

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Family table ──────────────────────────────────────────────────────────────

/// Whether the Anfrage activates or deactivates the series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZpVorgang {
    /// Aktivierung — the MaBiS-ZP starts contributing to the series.
    Aktivierung,
    /// Deaktivierung — the MaBiS-ZP stops contributing.
    Deaktivierung,
}

/// Which MaBiS series the Anfrage activates or deactivates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZpSerie {
    /// MaBiS-Zählpunkt itself (55062/55063).
    MabisZaehlpunkt,
    /// Zuordnungsermächtigung of a BKV toward an NB (55071/55072).
    Zuordnungsermaechtigung,
    /// Tägliche Ausfallarbeitsüberführungszeitreihe (55197/55198).
    TaeglicheAauez,
    /// LF-Ausfallarbeitssummenzeitreihe (55199/55200).
    LfAaszr,
    /// Monatliche AAÜZ forwarded to the BKV of the Lieferant (55203–55208).
    MonatlicheAauezBkvLf,
    /// Monatliche AAÜZ forwarded to the BKV of the anfordernder NB (55209–55214).
    MonatlicheAauezBkvAnfNb,
}

impl ZpSerie {
    /// Canonical BDEW name of the series.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::MabisZaehlpunkt => "MaBiS-Zählpunkt",
            Self::Zuordnungsermaechtigung => "Zuordnungsermächtigung",
            Self::TaeglicheAauez => "tägliche AAÜZ",
            Self::LfAaszr => "LF-AASZR",
            Self::MonatlicheAauezBkvLf => "monatliche AAÜZ (BKV des LF)",
            Self::MonatlicheAauezBkvAnfNb => "monatliche AAÜZ (BKV des anfordernden NB)",
        }
    }
}

/// One row of the Anfrage → Antwort → Weiterleitung table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZpFamilie {
    /// Inbound Anfrage Prüfidentifikator (Prozessschritt 1).
    pub anfrage: u32,
    /// Whether this row activates or deactivates.
    pub vorgang: ZpVorgang,
    /// Series being activated or deactivated.
    pub serie: ZpSerie,
    /// Outbound Antwort PID (Prozessschritt 2), when the AHB defines one.
    pub antwort: Option<u32>,
    /// Outbound Weiterleitung PID (Prozessschritt 4), when the AHB defines one.
    pub weiterleitung: Option<u32>,
}

/// Every Anfrage this workflow accepts, with its answer and forwarding PIDs.
///
/// This table is the single source of truth for the family: the workflow never
/// computes an answer PID from the request. BDEW does not number these `+1/+2`
/// — 55062 and 55063 share the Antwort 55064.
pub const ZP_FAMILIEN: &[ZpFamilie] = &[
    ZpFamilie {
        anfrage: 55062,
        vorgang: ZpVorgang::Aktivierung,
        serie: ZpSerie::MabisZaehlpunkt,
        antwort: Some(55064),
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55063,
        vorgang: ZpVorgang::Deaktivierung,
        serie: ZpSerie::MabisZaehlpunkt,
        antwort: Some(55064),
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55071,
        vorgang: ZpVorgang::Aktivierung,
        serie: ZpSerie::Zuordnungsermaechtigung,
        antwort: None,
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55072,
        vorgang: ZpVorgang::Deaktivierung,
        serie: ZpSerie::Zuordnungsermaechtigung,
        antwort: None,
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55197,
        vorgang: ZpVorgang::Aktivierung,
        serie: ZpSerie::TaeglicheAauez,
        antwort: None,
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55198,
        vorgang: ZpVorgang::Deaktivierung,
        serie: ZpSerie::TaeglicheAauez,
        antwort: None,
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55199,
        vorgang: ZpVorgang::Aktivierung,
        serie: ZpSerie::LfAaszr,
        antwort: None,
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55200,
        vorgang: ZpVorgang::Deaktivierung,
        serie: ZpSerie::LfAaszr,
        antwort: None,
        weiterleitung: None,
    },
    ZpFamilie {
        anfrage: 55203,
        vorgang: ZpVorgang::Aktivierung,
        serie: ZpSerie::MonatlicheAauezBkvLf,
        antwort: Some(55204),
        weiterleitung: Some(55205),
    },
    ZpFamilie {
        anfrage: 55206,
        vorgang: ZpVorgang::Deaktivierung,
        serie: ZpSerie::MonatlicheAauezBkvLf,
        antwort: Some(55207),
        weiterleitung: Some(55208),
    },
    ZpFamilie {
        anfrage: 55209,
        vorgang: ZpVorgang::Aktivierung,
        serie: ZpSerie::MonatlicheAauezBkvAnfNb,
        antwort: Some(55210),
        weiterleitung: Some(55211),
    },
    ZpFamilie {
        anfrage: 55212,
        vorgang: ZpVorgang::Deaktivierung,
        serie: ZpSerie::MonatlicheAauezBkvAnfNb,
        antwort: Some(55213),
        weiterleitung: Some(55214),
    },
];

/// Look up the family for an inbound Anfrage PID.
#[must_use]
pub fn familie_for(anfrage: u32) -> Option<&'static ZpFamilie> {
    ZP_FAMILIEN.iter().find(|f| f.anfrage == anfrage)
}

/// Every PID this workflow is registered for — Anfragen, Antworten and
/// Weiterleitungen alike.
///
/// The Antwort and Weiterleitung PIDs are registered because mako may sit on
/// either side: as the answering party it *emits* them, and as the requesting
/// party it *receives* them.
#[must_use]
pub fn all_pids() -> Vec<u32> {
    let mut v: Vec<u32> = ZP_FAMILIEN
        .iter()
        .flat_map(|f| [Some(f.anfrage), f.antwort, f.weiterleitung])
        .flatten()
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-zp-lifecycle";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a lifecycle Anfrage is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZpLifecycleData {
    /// Prüfidentifikator of the inbound Anfrage.
    pub pruefidentifikator: Pruefidentifikator,
    /// Activation or deactivation.
    pub vorgang: ZpVorgang,
    /// Series affected.
    pub serie: ZpSerie,
    /// MaBiS-Zählpunkt the Anfrage refers to.
    pub mabis_zp_id: String,
    /// GLN of the requesting party.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving party.
    pub receiver: MarktpartnerCode,
    /// Billing period the activation takes effect in.
    pub billing_period: BillingPeriod,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// EDIFACT message reference of the Anfrage.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MaBiS-ZP lifecycle workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ZpLifecycleEvent {
    /// Inbound Anfrage received and recorded.
    AnfrageErhalten {
        /// Prüfidentifikator of the Anfrage.
        pruefidentifikator: Pruefidentifikator,
        /// Activation or deactivation.
        vorgang: ZpVorgang,
        /// Series affected.
        serie: ZpSerie,
        /// MaBiS-Zählpunkt the Anfrage refers to.
        mabis_zp_id: String,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        receiver: MarktpartnerCode,
        /// Billing period the activation takes effect in.
        billing_period: BillingPeriod,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Anfrage recorded with no Antwort obligation (terminal for that family).
    Erfasst {
        /// Reference of the recorded message.
        message_ref: MessageRef,
    },
    /// Outbound Antwort dispatched.
    AntwortGesendet {
        /// Antwort Prüfidentifikator actually sent.
        antwort_pid: Pruefidentifikator,
        /// `true` when the Anfrage was confirmed.
        bestaetigt: bool,
        /// Rejection reason, when `bestaetigt` is `false`.
        grund: Option<String>,
    },
    /// Outbound Weiterleitung dispatched to the downstream BKV.
    WeiterleitungGesendet {
        /// Weiterleitung Prüfidentifikator actually sent.
        weiterleitung_pid: Pruefidentifikator,
        /// GLN of the BKV the Weiterleitung was addressed to.
        empfaenger: MarktpartnerCode,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Human-readable summary of validation errors.
        reason: String,
    },
}

impl EventPayload for ZpLifecycleEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "MabisZpAnfrageErhalten",
            Self::Erfasst { .. } => "MabisZpErfasst",
            Self::AntwortGesendet { .. } => "MabisZpAntwortGesendet",
            Self::WeiterleitungGesendet { .. } => "MabisZpWeiterleitungGesendet",
            Self::ValidationFailed { .. } => "MabisZpValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a MaBiS-ZP lifecycle process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum ZpLifecycleState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received; an Antwort is owed.
    AnfrageErhalten(Box<ZpLifecycleData>),
    /// Anfrage recorded; the family defines no Antwort (terminal).
    Erfasst(Box<ZpLifecycleData>),
    /// Antwort sent confirming the Anfrage.
    Bestaetigt(Box<ZpLifecycleData>),
    /// Antwort sent rejecting the Anfrage (terminal).
    Abgelehnt {
        /// Rejection reason.
        grund: String,
    },
    /// Weiterleitung dispatched to the downstream BKV (terminal).
    Weitergeleitet(Box<ZpLifecycleData>),
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Validation error summary.
        reason: String,
    },
}

impl ZpLifecycleState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageErhalten(_) => "AnfrageErhalten",
            Self::Erfasst(_) => "Erfasst",
            Self::Bestaetigt(_) => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Weitergeleitet(_) => "Weitergeleitet",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }

    /// The recorded Anfrage data, when the state carries any.
    #[must_use]
    pub fn data(&self) -> Option<&ZpLifecycleData> {
        match self {
            Self::AnfrageErhalten(d)
            | Self::Erfasst(d)
            | Self::Bestaetigt(d)
            | Self::Weitergeleitet(d) => Some(d),
            Self::New | Self::Abgelehnt { .. } | Self::ValidationFailed { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MaBiS-ZP lifecycle workflow.
///
/// `Workflow::handle()` is pure — no I/O, no EDIFACT parsing, no store access.
#[derive(Clone)]
pub enum ZpLifecycleCommand {
    /// Inbound Anfrage received from the AS4 layer.
    ReceiveAnfrage {
        /// Prüfidentifikator of the inbound UTILMD.
        pid: Pruefidentifikator,
        /// MaBiS-Zählpunkt the Anfrage refers to, as it arrived.
        ///
        /// Deliberately a `String` and not
        /// [`MabisZaehlpunktId`](crate::MabisZaehlpunktId): this is a
        /// counterparty's value. Requiring the validated type would make a
        /// malformed Meldepunkt unconstructible, and the workflow could then
        /// neither record what arrived nor answer it with a proper Ablehnung.
        /// The outbound side — [`crate::Summenzeitreihe`] — uses the type,
        /// because that value is ours to get right.
        mabis_zp_id: String,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        receiver: MarktpartnerCode,
        /// Billing period the activation takes effect in.
        billing_period: BillingPeriod,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
    /// Send the Antwort for a received Anfrage.
    SendAntwort {
        /// `true` to confirm, `false` to reject.
        bestaetigt: bool,
        /// Rejection reason — required when `bestaetigt` is `false`.
        grund: Option<String>,
    },
    /// Forward the confirmed activation to the downstream BKV.
    SendWeiterleitung {
        /// GLN of the BKV to forward to.
        empfaenger: MarktpartnerCode,
    },
}

impl CommandPayload for ZpLifecycleCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MaBiS-ZP lifecycle workflow.
///
/// Handles activation and deactivation of the MaBiS-Zählpunkt, the
/// Zuordnungsermächtigung, and the AAÜZ/LF-AASZR series. See the module
/// documentation for the PID table and the state machine.
pub struct MabisZpLifecycleWorkflow;

impl Workflow for MabisZpLifecycleWorkflow {
    type State = ZpLifecycleState;
    type Event = ZpLifecycleEvent;
    type Command = ZpLifecycleCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ZpLifecycleEvent::AnfrageErhalten {
                pruefidentifikator,
                vorgang,
                serie,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
            } => ZpLifecycleState::AnfrageErhalten(Box::new(ZpLifecycleData {
                pruefidentifikator: *pruefidentifikator,
                vorgang: *vorgang,
                serie: *serie,
                mabis_zp_id: mabis_zp_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                billing_period: billing_period.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            })),

            ZpLifecycleEvent::Erfasst { .. } => match state {
                ZpLifecycleState::AnfrageErhalten(d) => ZpLifecycleState::Erfasst(d),
                other => other,
            },

            ZpLifecycleEvent::AntwortGesendet {
                bestaetigt, grund, ..
            } => match state {
                ZpLifecycleState::AnfrageErhalten(d) => {
                    if *bestaetigt {
                        ZpLifecycleState::Bestaetigt(d)
                    } else {
                        ZpLifecycleState::Abgelehnt {
                            grund: grund.clone().unwrap_or_default(),
                        }
                    }
                }
                other => other,
            },

            ZpLifecycleEvent::WeiterleitungGesendet { .. } => match state {
                ZpLifecycleState::Bestaetigt(d) => ZpLifecycleState::Weitergeleitet(d),
                other => other,
            },

            ZpLifecycleEvent::ValidationFailed { reason } => ZpLifecycleState::ValidationFailed {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ZpLifecycleState::New) {
                    // Idempotent: a redelivered Anfrage is a no-op.
                    return Ok(vec![].into());
                }

                let Some(familie) = familie_for(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a MaBiS-ZP lifecycle Anfrage; expected one of {:?}",
                        ZP_FAMILIEN.iter().map(|f| f.anfrage).collect::<Vec<_>>()
                    )));
                };

                if !validation_passed {
                    return Ok(vec![ZpLifecycleEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }

                let erhalten = ZpLifecycleEvent::AnfrageErhalten {
                    pruefidentifikator: pid,
                    vorgang: familie.vorgang,
                    serie: familie.serie,
                    mabis_zp_id,
                    sender,
                    receiver,
                    billing_period,
                    document_date,
                    message_ref: message_ref.clone(),
                };

                // A family with no Antwort PID is terminal on arrival. Leaving
                // it in `AnfrageErhalten` would model an obligation the AHB
                // does not define.
                if familie.antwort.is_none() {
                    return Ok(vec![erhalten, ZpLifecycleEvent::Erfasst { message_ref }].into());
                }

                Ok(vec![erhalten].into())
            }

            ZpLifecycleCommand::SendAntwort { bestaetigt, grund } => {
                let ZpLifecycleState::AnfrageErhalten(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendAntwort requires state AnfrageErhalten, got {}",
                        state.label()
                    )));
                };

                let familie = familie_for(data.pruefidentifikator.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "no family for recorded Anfrage {}",
                        data.pruefidentifikator
                    ))
                })?;

                let Some(antwort) = familie.antwort else {
                    return Err(WorkflowError::rejected(format!(
                        "the {} family (Anfrage {}) defines no Antwort PID",
                        familie.serie.label(),
                        familie.anfrage
                    )));
                };

                if !bestaetigt && grund.as_ref().is_none_or(|g| g.trim().is_empty()) {
                    return Err(WorkflowError::rejected(
                        "a rejecting Antwort requires a reason".to_owned(),
                    ));
                }

                let antwort_pid = Pruefidentifikator::new(antwort).map_err(|e| {
                    WorkflowError::rejected(format!("invalid Antwort PID {antwort}: {e}"))
                })?;

                let outbox = PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": antwort,
                        "mabis_zp_id": data.mabis_zp_id,
                        "process_date": data.document_date,
                        "bestaetigt": bestaetigt,
                        "grund": grund,
                    }),
                );

                Ok(WorkflowOutput {
                    events: vec![ZpLifecycleEvent::AntwortGesendet {
                        antwort_pid,
                        bestaetigt,
                        grund,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            ZpLifecycleCommand::SendWeiterleitung { empfaenger } => {
                let ZpLifecycleState::Bestaetigt(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendWeiterleitung requires state Bestaetigt, got {}",
                        state.label()
                    )));
                };

                let familie = familie_for(data.pruefidentifikator.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "no family for recorded Anfrage {}",
                        data.pruefidentifikator
                    ))
                })?;

                let Some(weiterleitung) = familie.weiterleitung else {
                    return Err(WorkflowError::rejected(format!(
                        "the {} family (Anfrage {}) defines no Weiterleitung PID",
                        familie.serie.label(),
                        familie.anfrage
                    )));
                };

                let weiterleitung_pid = Pruefidentifikator::new(weiterleitung).map_err(|e| {
                    WorkflowError::rejected(format!(
                        "invalid Weiterleitung PID {weiterleitung}: {e}"
                    ))
                })?;

                let outbox = PendingOutbox::new(
                    "UTILMD",
                    empfaenger.as_str(),
                    serde_json::json!({
                        "pid": weiterleitung,
                        "mabis_zp_id": data.mabis_zp_id,
                        "process_date": data.document_date,
                    }),
                );

                Ok(WorkflowOutput {
                    events: vec![ZpLifecycleEvent::WeiterleitungGesendet {
                        weiterleitung_pid,
                        empfaenger,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn receive(pid: u32) -> ZpLifecycleCommand {
        ZpLifecycleCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            mabis_zp_id: "DE0001112223334445556667778889990".to_owned(),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            billing_period: BillingPeriod::new("2026-07"),
            document_date: "20260701".to_owned(),
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn fold(events: &[ZpLifecycleEvent]) -> ZpLifecycleState {
        events.iter().fold(ZpLifecycleState::default(), |s, e| {
            MabisZpLifecycleWorkflow::apply(s, e)
        })
    }

    #[test]
    fn every_family_pid_is_distinct_and_no_pid_is_both_anfrage_and_answer() {
        let anfragen: Vec<u32> = ZP_FAMILIEN.iter().map(|f| f.anfrage).collect();
        let mut sorted = anfragen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), anfragen.len(), "duplicate Anfrage PID");

        for f in ZP_FAMILIEN {
            for answer in [f.antwort, f.weiterleitung].into_iter().flatten() {
                assert!(
                    familie_for(answer).is_none(),
                    "{answer} is an answer PID but is also registered as an Anfrage — \
                     receiving it would spawn a process that answers an answer"
                );
            }
        }
    }

    #[test]
    fn the_shared_antwort_pid_is_not_derived_arithmetically() {
        // 55062 and 55063 both answer with 55064 — the reason this is a table.
        assert_eq!(familie_for(55062).unwrap().antwort, Some(55064));
        assert_eq!(familie_for(55063).unwrap().antwort, Some(55064));
    }

    #[test]
    fn all_pids_covers_anfragen_answers_and_weiterleitungen() {
        let pids = all_pids();
        for f in ZP_FAMILIEN {
            assert!(pids.contains(&f.anfrage));
            for p in [f.antwort, f.weiterleitung].into_iter().flatten() {
                assert!(pids.contains(&p), "{p} missing from all_pids()");
            }
        }
        // 12 Anfragen + 55064 + (55204,55205,55207,55208,55210,55211,55213,55214)
        assert_eq!(pids.len(), 21, "unexpected PID count: {pids:?}");
    }

    #[test]
    fn a_family_without_an_antwort_is_terminal_on_arrival() {
        // 55071 Zuordnungsermächtigung has no Antwort PID.
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, receive(55071))
            .expect("accepted");
        let state = fold(&out.events);
        assert_eq!(state.label(), "Erfasst");
        assert!(out.outbox.is_empty(), "record-only family must not emit");

        // Asking it to answer is an error, not a silently wrong PID.
        let err = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: true,
                grund: None,
            },
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("Antwort"), "got: {err}");
    }

    #[test]
    fn anfrage_antwort_weiterleitung_happy_path() {
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, receive(55203))
            .expect("accepted");
        let state = fold(&out.events);
        assert_eq!(state.label(), "AnfrageErhalten");

        let antwort = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: true,
                grund: None,
            },
        )
        .expect("pruefung");
        assert_eq!(antwort.outbox.len(), 1);
        assert_eq!(antwort.outbox[0].payload["pid"], 55204);
        assert_eq!(
            antwort.outbox[0].recipient.as_ref(),
            "9900123456789",
            "the Antwort goes back to the requesting party"
        );

        // Continue folding onto the state the Anfrage produced.
        let state = antwort
            .events
            .iter()
            .fold(state, MabisZpLifecycleWorkflow::apply);
        assert_eq!(state.label(), "Bestaetigt");

        let out = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendWeiterleitung {
                empfaenger: mp("9900555555555"),
            },
        )
        .expect("weiterleitung");
        assert_eq!(out.outbox[0].payload["pid"], 55205);
    }

    #[test]
    fn the_two_monatliche_families_forward_to_different_recipients() {
        // Identical process, different Weiterleitung code — the only thing
        // separating them, and the reason they are not merged.
        assert_eq!(familie_for(55203).unwrap().weiterleitung, Some(55205));
        assert_eq!(familie_for(55209).unwrap().weiterleitung, Some(55211));
        assert_ne!(
            familie_for(55203).unwrap().serie,
            familie_for(55209).unwrap().serie
        );
    }

    #[test]
    fn a_rejecting_antwort_requires_a_reason() {
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, receive(55062))
            .expect("accepted");
        let state = fold(&out.events);
        let err = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: false,
                grund: None,
            },
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("reason"), "got: {err}");
    }

    #[test]
    fn validation_failure_is_terminal_and_emits_nothing() {
        let cmd = match receive(55062) {
            ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                ..
            } => ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                validation_passed: false,
                validation_errors: vec!["SG6 LOC missing".to_owned()],
            },
            other => other,
        };
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd).expect("accepted");
        assert!(out.outbox.is_empty());
        assert_eq!(fold(&out.events).label(), "ValidationFailed");
    }

    #[test]
    fn an_unknown_pid_is_rejected_rather_than_silently_recorded() {
        // 55218 is GPKE Teil 2 (Abr.-Daten NNA), not MaBiS.
        let err = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, receive(55218))
            .expect_err("must reject");
        assert!(
            format!("{err}").contains("not a MaBiS-ZP lifecycle"),
            "{err}"
        );
    }

    #[test]
    fn a_redelivered_anfrage_is_a_no_op() {
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, receive(55062))
            .expect("accepted");
        let state = fold(&out.events);
        let again = MabisZpLifecycleWorkflow::handle(&state, receive(55062)).expect("idempotent");
        assert!(again.events.is_empty());
    }
}
