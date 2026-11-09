//! GPKE Neuanlage — new Marktlokation registration process.
//!
//! Covers the GPKE Neuanlage process per BNetzA BK6-24-174 Anlage 1b (GPKE
//! Teil 2 §2.2): registration of a **new** Marktlokation not previously in the
//! grid topology. This is distinct from Lieferbeginn (PID 55001) which assumes
//! the location already exists.
//!
//! This module implements the **receiving-party perspective** (Netzbetreiber / NB):
//! the system receives an inbound Anmeldung Neuanlage from the Lieferant/Einspeiser
//! and responds with Bestätigung or Ablehnung.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2)
//!
//! ## Inbound ANFRAGE — routed to `gpke-neuanlage`
//!
//! | PID   | Process name (AHB)                               | Direction |
//! |-------|--------------------------------------------------|-----------|
//! | 55600 | Anmeldung neue verb. MaLo (LF → NB)             | LF → NB   |
//! | 55601 | Anmeldung neue erz. MaLo (LF → NB)              | LF → NB   |
//!
//! ## Outbound ANTWORT — derived by this workflow, NOT routed as inbound
//!
//! | PID   | Process name (AHB)                               | Derived from  |
//! |-------|--------------------------------------------------|---------------|
//! | 55602 | Bestätigung Anmeldung neue verb. MaLo (NB → LF) | 55600 accepted|
//! | 55603 | Bestätigung Anmeldung neue erz. MaLo (NB → LF)  | 55601 accepted|
//! | 55604 | Ablehnung Anmeldung neue verb. MaLo (NB → LF)   | 55600 rejected|
//! | 55605 | Ablehnung Anmeldung neue erz. MaLo (NB → LF)    | 55601 rejected|
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 1b** — GPKE Teil 2 Kap. 2.2 Neuanlage
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format
//! - **APERAK AHB 1.1 § 2.4.1** — technical acknowledgement, **45 Minuten** on a
//!   weekday for a UTILMD. A separate clock from the business answer window,
//!   which GPKE Teil 2 states as a wall-clock instant on the 1. Werktag nach
//!   dem ÜT.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-neuanlage";

/// Inbound ANFRAGE PIDs for Neuanlage handled by [`GpkeNeuanlageWorkflow`].
///
/// | PID   | Process (AHB name)                                  | AHB profile  |
/// |-------|-----------------------------------------------------|--------------|
/// | 55600 | Anmeldung neue verb. MaLo (LF → NB)                | S2.1, S2.2   |
/// | 55601 | Anmeldung neue erz. MaLo (LF → NB)                 | S2.1, S2.2   |
///
/// Response PIDs (55602–55605) are derived internally and never routed.
pub const NEUANLAGE_PIDS: &[u32] = &[55600, 55601];

/// Deadline label for the **business** answer window — „spätester ÜZ ist 00:00
/// Uhr des 61. WT nach dem ÜT" (GPKE Teil 2 Kap. 2.2.2), resolved by
/// `mako_fristen::antwort`: `E_0608` Prüfschritte 110/590 make identifying a
/// newly commissioned Marktlokation a daily re-check for up to 60 Werktage.
///
/// Not the APERAK clock: that is 45 Minuten for a UTILMD (APERAK AHB 1.1
/// § 2.4.1) and rides `mako_fristen::APERAK_STROM_WINDOW_LABEL`.
///
/// Register immediately after `ValidationPassed`:
/// ```rust,ignore
/// let due = mako_fristen::antwort::antwort_deadline(pid, received_at)
///     .expect("a PID with a published Antwortfrist");
/// let dl = Deadline::new(stream_id, ..., NEUANLAGE_ANTWORT_WINDOW_LABEL, due);
/// deadline_store.register(&dl).await?;
/// ```
pub const NEUANLAGE_ANTWORT_WINDOW_LABEL: &str = "gpke-neuanlage-antwortfrist";

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound UTILMD response PID for a Neuanlage Anfrage.
///
/// | Anfrage | accepted | Response PID |
/// |---------|----------|--------------|
/// | 55600   | true     | 55602        |
/// | 55600   | false    | 55604        |
/// | 55601   | true     | 55603        |
/// | 55601   | false    | 55605        |
fn neuanlage_response_pid(anfrage_pid: u32, accepted: bool) -> Option<Pruefidentifikator> {
    let code: u32 = match anfrage_pid {
        55600 => {
            if accepted {
                55602
            } else {
                55604
            }
        }
        55601 => {
            if accepted {
                55603
            } else {
                55605
            }
        }
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Neuanlage workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NeuanlageEvent {
    /// UTILMD Neuanlage Anfrage received.
    AnmeldungErhalten {
        /// Marktlokation EIC code (to be assigned).
        location_id: MaLo,
        /// GLN of the Lieferant / Einspeiser.
        sender: MarktpartnerCode,
        /// GLN of the Netzbetreiber.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Requested connection date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55600 or 55601).
        pruefidentifikator: Pruefidentifikator,
        /// `SG4 IDE+24` DE 7402 of the Anmeldung — the answer's `SG6 RFF+TN`.
        #[serde(default)]
        vorgangsnummer: Option<String>,
        /// `SG4 STS+7` Transaktionsgrund of the Anmeldung, which the answer
        /// restates.
        #[serde(default)]
        transaktionsgrund: Option<String>,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound UTILMD response (55602–55605) was sent.
    AntwortGesendet {
        /// Derived outbound response PID.
        response_pid: Option<Pruefidentifikator>,
        /// `true` = accepted (Bestätigung), `false` = rejected (Ablehnung).
        accepted: bool,
        /// Rejection reason (set only when `accepted = false`).
        reason: Option<String>,
    },
    /// Neuanlage completed — MaLo is active in the grid.
    Aktiviert,
    /// Process closed due to validation failure, rejection, or deadline expiry.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired before the process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for NeuanlageEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnmeldungErhalten { .. } => "NeuanlageAnmeldungErhalten",
            Self::ValidationPassed { .. } => "NeuanlageValidationPassed",
            Self::AntwortGesendet { .. } => "NeuanlageAntwortGesendet",
            Self::Aktiviert => "NeuanlageAktiviert",
            Self::Rejected { .. } => "NeuanlageRejected",
            Self::DeadlineExpired { .. } => "NeuanlageDeadlineExpired",
        }
    }

    /// An `AntwortGesendet` without a `response_pid` rendered no UTILMD —
    /// `handle` queues the answer only for a resolved PID — so it states none.
    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnmeldungErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet { response_pid, .. } => response_pid.map(|p| Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(p.as_u32()),
                message_ref: None,
            }),
            Self::Aktiviert | Self::DeadlineExpired { .. } => None,
            // `ValidationPassed` is emitted only beside the APERAK 312
            // Anerkennungsmeldung `handle` queues for it, `Rejected` only beside
            // the APERAK 313 Verarbeitbarkeitsfehlermeldung; an APERAK carries
            // no Prüfidentifikator.
            Self::ValidationPassed { .. } | Self::Rejected { .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnmeldungErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NeuanlageData {
    /// EIC/MaLo code of the new location.
    pub location_id: MaLo,
    /// GLN of the requesting Lieferant / Einspeiser.
    pub sender: MarktpartnerCode,
    /// GLN of the responsible Netzbetreiber.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Requested connection date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55600 or 55601).
    pub pruefidentifikator: Pruefidentifikator,
    /// `SG4 IDE+24` DE 7402 of the Anmeldung. Every answer references it in
    /// `SG6 RFF+TN` (UTILMD AHB Strom 2.2, Muss on 55602–55605).
    #[serde(default)]
    pub vorgangsnummer: Option<String>,
    /// `SG4 STS+7` Transaktionsgrund of the Anmeldung (`E02`), which every
    /// answer restates; the Ergänzung is the answer's own (`ZW3`/`ZW4` on an
    /// Ablehnung, the column fixes it).
    #[serde(default)]
    pub transaktionsgrund: Option<String>,
}

/// State of a GPKE Neuanlage process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Aktiviert
///                                      ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum NeuanlageState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received; AHB validation pending.
    Eingegangen(NeuanlageData),
    /// Validation passed; response not yet sent.
    ValidationPassed(NeuanlageData),
    /// Response sent; awaiting MaLo activation.
    AntwortGesendet {
        /// Data from the Anfrage.
        data: NeuanlageData,
        /// Derived outbound response PID.
        response_pid: Option<Pruefidentifikator>,
    },
    /// MaLo is active in the grid.
    Aktiviert(NeuanlageData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GPKE Teil 2 Kap. 2.2.2 gives the NB its answer window on the Anmeldung einer Neuanlage and no consequence of missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<NeuanlageState>,
    },
}

/// Whether a received Neuanlage still holds its Marktlokation against a new one.
///
/// Live while the answer is owed or the accepted Vorgang awaits its
/// activation; settled once refused or activated. A settled process must not
/// take the next Neuanlage for the same Marktlokation — its receive command
/// refuses anything but `New`.
impl mako_engine::workflow::OccupiesBusinessKey for NeuanlageState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            Self::Eingegangen(_) | Self::ValidationPassed(_) | Self::AntwortGesendet { .. } => true,
            Self::New | Self::Aktiviert(_) | Self::Rejected { .. } => false,
        }
    }
}

impl NeuanlageState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Aktiviert(_) => "Aktiviert",
            Self::Rejected { .. } => "Rejected",
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

    /// Return `Some(&NeuanlageData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&NeuanlageData> {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.data(),
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Aktiviert(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Neuanlage workflow.
#[derive(Clone)]
pub enum NeuanlageCommand {
    /// Inbound UTILMD Neuanlage Anfrage received from the AS4 layer.
    ReceiveAnmeldung {
        /// BDEW Prüfidentifikator (55600 or 55601).
        pid: Pruefidentifikator,
        /// GLN of the sender (LF).
        sender: MarktpartnerCode,
        /// GLN of the receiver (NB).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Requested connection date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings (for the `Rejected` event).
        validation_errors: Vec<String>,
        /// The Anmeldung's `SG4` facts — its Vorgangsnummer and Transaktionsgrund.
        vorgang: Box<crate::LfVorgangsdaten>,
    },
    /// Send the outbound UTILMD response to the Lieferant.
    ///
    /// Derive the response PID from the Anfrage PID:
    /// - 55600 → 55602 (accepted) / 55604 (rejected)
    /// - 55601 → 55603 (accepted) / 55605 (rejected)
    ///
    /// The Neuanlage answer window is **00:00 Uhr des 61. Werktags nach dem
    /// ÜT** (BK6-24-174 GPKE Teil 2 Kap. 2.2.2): `E_0608` Prüfschritte 110 und 590
    /// give the NB 60 Werktage of daily re-identification before it may refuse a
    /// newly commissioned Marktlokation. A 24-hour deadline on this process
    /// manufactures a rejection roughly three months early.
    SendAntwort {
        /// The NB's answer: the resolved `E_0608` Antwortcode and its cluster.
        ///
        /// `SG4 STS+E01` is Muss on every Antwortnachricht, and the published
        /// **Cluster** selects the response PID — `A09` / `A18` ride 55602 /
        /// 55603, every other code rides 55604 / 55605.
        antwort: crate::LfAntwort,
    },
    /// Mark the new Marktlokation as active in the grid.
    Aktivieren,
    /// A registered deadline fired; record expiry and close the process.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for NeuanlageCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Neuanlage workflow (PIDs 55600/55601).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeNeuanlageWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-neuanlage", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeNeuanlageWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the NB still owes its answer
/// (`Eingegangen`, `ValidationPassed`).
fn window_open(label: &str, state: &NeuanlageState) -> bool {
    label == NEUANLAGE_ANTWORT_WINDOW_LABEL
        && matches!(
            state,
            NeuanlageState::Eingegangen(_) | NeuanlageState::ValidationPassed(_)
        )
}

impl Workflow for GpkeNeuanlageWorkflow {
    type State = NeuanlageState;
    type Event = NeuanlageEvent;
    type Command = NeuanlageCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[NEUANLAGE_ANTWORT_WINDOW_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| NeuanlageCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            NeuanlageState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            NeuanlageEvent::AnmeldungErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                vorgangsnummer,
                transaktionsgrund,
                ..
            } => NeuanlageState::Eingegangen(NeuanlageData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                vorgangsnummer: vorgangsnummer.clone(),
                transaktionsgrund: transaktionsgrund.clone(),
            }),
            NeuanlageEvent::ValidationPassed { .. } => match state {
                NeuanlageState::Eingegangen(data) => NeuanlageState::ValidationPassed(data),
                other => other,
            },
            NeuanlageEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        NeuanlageState::ValidationPassed(data) => NeuanlageState::AntwortGesendet {
                            response_pid: *response_pid,
                            data,
                        },
                        other => other,
                    }
                } else {
                    NeuanlageState::Rejected {
                        reason: "Neuanlage abgelehnt".to_owned(),
                    }
                }
            }
            NeuanlageEvent::Aktiviert => match state {
                NeuanlageState::AntwortGesendet { data, .. } => NeuanlageState::Aktiviert(data),
                other => other,
            },
            NeuanlageEvent::Rejected { reason } => NeuanlageState::Rejected {
                reason: reason.clone(),
            },
            NeuanlageEvent::DeadlineExpired { label, .. } => match state {
                NeuanlageState::Eingegangen(_) | NeuanlageState::ValidationPassed(_) => {
                    NeuanlageState::FristAbgelaufen {
                        label: label.to_string(),
                        offen: Box::new(state),
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
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                validation_passed,
                validation_errors,
                vorgang,
            } => {
                if !matches!(state, NeuanlageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !NEUANLAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Neuanlage PID (55600 or 55601), got {pid}",
                    )));
                }
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![NeuanlageEvent::AnmeldungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                    vorgangsnummer: vorgang.vorgangsnummer.clone(),
                    transaktionsgrund: vorgang.transaktionsgrund.clone(),
                }];
                if validation_passed {
                    events.push(NeuanlageEvent::ValidationPassed {
                        message_ref: message_ref.clone(),
                    });
                    // APERAK BGM+312 (Anerkennungsmeldung) — mandatory per APERAK AHB 1.1 §2.4.
                    // Strom UTILMD: 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.1 §2.4.1).
                    let outbox = vec![
                        PendingOutbox::aperak_anerkennung(
                            receiver_gln.as_str(),
                            sender_mp_id.as_str(),
                            message_ref.as_str(),
                        )
                        .caused_by(1),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(NeuanlageEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // APERAK BGM+313 — mandatory per APERAK AHB 1.1 §2.1.1.
                    // Strom UTILMD: 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.1 §2.4.1).
                    let outbox = vec![
                        PendingOutbox::aperak_fehler(
                            receiver_gln.as_str(),
                            sender_mp_id.as_str(),
                            message_ref.as_str(),
                            mako_engine::erc::codes::Z29,
                            reason,
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
            }

            NeuanlageCommand::SendAntwort { antwort } => {
                let data = match state {
                    NeuanlageState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let accepted = antwort.zustimmung;
                let response_pid =
                    neuanlage_response_pid(data.pruefidentifikator.as_u32(), accepted);
                let events = vec![NeuanlageEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason: antwort.bemerkung.clone(),
                }];

                // The answer is a message, not just an event: an
                // `AntwortGesendet` with no outbox entry records the process as
                // answered while the Lieferant watches its 61-Werktage window
                // expire.
                let mut outbox = Vec::new();
                // `SG5 LOC+Z16` names a Marktlokation the NB has. On the
                // Ablehnung of a neue erzeugende Marktlokation 55605 it is Muss
                // only beside `A12`/`A13` and admitted nowhere else (Bedingung
                // `[43]`, UTILMD AHB Strom 2.2) — any other refusal names none.
                let nennt_marktlokation = !data.location_id.as_str().is_empty()
                    && (response_pid.map(Pruefidentifikator::as_u32) != Some(55_605)
                        || matches!(antwort.antwort_code.as_str(), "A12" | "A13"));
                if let Some(rpid) = response_pid {
                    outbox.push(PendingOutbox::new(
                        "UTILMD",
                        data.sender.as_str(),
                        serde_json::json!({
                            "pid":          rpid.as_u32(),
                            "sender":       data.receiver.as_str(),
                            "receiver":     data.sender.as_str(),
                            "malo":         nennt_marktlokation.then(|| data.location_id.as_str()),
                            "process_date": data.process_date,
                            // `SG4 STS+E01++<code>:E_0608` — Muss on every
                            // Antwortnachricht.
                            "antwort_code": antwort.antwort_code,
                            "antwort_codeliste":  antwort.ebd,
                            "bemerkung":    antwort.bemerkung,
                            // `SG4 STS+7` and `SG6 RFF+TN` — the Anmeldung's
                            // Grund and Vorgangsnummer, Muss on every answer.
                            "transaktionsgrund":       data.transaktionsgrund,
                            "referenz_vorgangsnummer": data.vorgangsnummer,
                        }),
                    ));
                }
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            NeuanlageCommand::Aktivieren => {
                if !matches!(state, NeuanlageState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![NeuanlageEvent::Aktiviert].into())
            }

            NeuanlageCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(vec![].into());
                }
                Ok(vec![NeuanlageEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{ids::DeadlineId, workflow::Workflow};

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn anmeldung_cmd(pid_code: u32, ok: bool) -> NeuanlageCommand {
        NeuanlageCommand::ReceiveAnmeldung {
            pid: pid(pid_code),
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("NEUA-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            vorgang: Box::default(),
        }
    }

    #[test]
    fn verlauf_states_the_inbound_anmeldung_and_the_outbound_answer() {
        assert_eq!(
            NeuanlageEvent::Rejected {
                reason: "x".to_owned(),
            }
            .verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
            "the APERAK 313 queued beside the rejection"
        );
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, true))
            .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(55_600),
                message_ref: Some("NEUA-001".to_owned()),
            })
        );
        assert_eq!(
            out.events[1].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
            "the APERAK 312 queued beside ValidationPassed"
        );
        assert!(
            out.outbox
                .iter()
                .any(|o| &*o.message_type == "APERAK" && o.caused_by_event_index == 1),
            "the APERAK 312 is caused by ValidationPassed"
        );
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                antwort: crate::LfAntwort::zustimmung("A09", "E_0608"),
            },
        )
        .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(55_602),
                message_ref: None,
            })
        );
        let unrendered = NeuanlageEvent::AntwortGesendet {
            response_pid: None,
            accepted: true,
            reason: None,
        };
        assert_eq!(unrendered.verlauf(), None, "no PID, no UTILMD rendered");
        assert_eq!(NeuanlageEvent::Aktiviert.verlauf(), None);
    }

    #[test]
    fn neuanlage_55600_happy_path() {
        // Step 1: receive 55600 Anmeldung
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, true))
            .unwrap();
        assert_eq!(out.events.len(), 2); // AnmeldungErhalten + ValidationPassed
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        assert!(matches!(state, NeuanlageState::ValidationPassed(_)));

        // Step 2: send Bestätigung
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                antwort: crate::LfAntwort::zustimmung("A09", "E_0608"),
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        if let NeuanlageEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(accepted);
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(55602));
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = out.events.iter().fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::AntwortGesendet { .. }));

        // Step 3: activate
        let out = GpkeNeuanlageWorkflow::handle(&state, NeuanlageCommand::Aktivieren).unwrap();
        let state = out.events.iter().fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::Aktiviert(_)));
    }

    #[test]
    fn neuanlage_55601_rejected() {
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55601, true))
            .unwrap();
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                antwort: crate::LfAntwort::ablehnung("A03", "E_0608"),
            },
        )
        .unwrap();
        if let NeuanlageEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(!accepted);
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(55605));
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = out.events.iter().fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::Rejected { .. }));
    }

    #[test]
    fn neuanlage_validation_failure_rejects() {
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, false))
            .unwrap();
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        assert!(matches!(state, NeuanlageState::Rejected { .. }));
    }

    #[test]
    fn neuanlage_wrong_pid_rejected() {
        let result = GpkeNeuanlageWorkflow::handle(
            &NeuanlageState::New,
            NeuanlageCommand::ReceiveAnmeldung {
                pid: pid(55001), // wrong PID
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                location_id: malo("51238696781"),
                document_date: "20251001".to_owned(),
                process_date: "20260101".to_owned(),
                message_ref: mref("NEUA-X"),
                validation_passed: true,
                validation_errors: vec![],
                vorgang: Box::default(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn timeout_in_terminal_state_is_noop() {
        let state = NeuanlageState::Aktiviert(NeuanlageData {
            location_id: malo("51238696781"),
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            pruefidentifikator: pid(55600),
            vorgangsnummer: None,
            transaktionsgrund: None,
        });
        let dl_id = DeadlineId::new();
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::TimeoutExpired {
                deadline_id: dl_id,
                label: NEUANLAGE_ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }
}
