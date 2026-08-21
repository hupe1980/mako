//! GPKE Wechselprozesse Strom — UTILMD-based connection processes.
//!
//! Covers all GPKE processes that use the UTILMD S2.x message format per the
//! LFW24 specification (BK6-22-024, effective 2025-06-06). The authoritative PID
//! list is the UTILMD AHB S2.1/S2.2 (EDI@Energy).
//!
//! This module implements the **receiving-party perspective** (Netzbetreiber / NB):
//! the system receives inbound ANFRAGE messages and emits outbound ANTWORT messages.
//!
//! # Prüfidentifikatoren (LFW24 UTILMD AHB S2.1/S2.2)
//!
//! ## Inbound ANFRAGE — routed to this workflow
//!
//! | PID   | Process name (AHB)                              | Direction  |
//! |-------|-------------------------------------------------|------------|
//! | 55001 | Anmeldung verb. MaLo                            | LF → NB   |
//! | 55004 | Abmeldung                                       | LF → NB   |
//!
//! ## Outbound ANTWORT — derived by this workflow, NOT routed as inbound
//!
//! The AHB lays each Anwendungsfall out as a triple of adjacent PID columns
//! `(Anfrage, Bestätigung, Ablehnung)`, so the response is `anfrage + 1` to
//! accept and `anfrage + 2` to reject.
//!
//! | PID   | Process name (AHB)                              | Derived from |
//! |-------|-------------------------------------------------|--------------|
//! | 55002 | Bestätigung Anmeldung verb. MaLo (NB → LF)      | 55001 accept |
//! | 55003 | Ablehnung Anmeldung verb. MaLo (NB → LF)        | 55001 reject |
//! | 55005 | Bestätigung Abmeldung (NB → LF)                 | 55004 accept |
//! | 55006 | Ablehnung Abmeldung (NB → LF)                   | 55004 reject |
//!
//!
//! ORDERS Sperrung (PIDs 17115/17116/17117) is handled by `GpkeSperrungWorkflow` — see
//! the [`sperrung`][crate::sperrung] module.
//!
//! **PID 55555** is "Anfrage Daten der individuellen Bestellung" (GPKE Teil 4) —
//! a separate UTILMD data-request process, not a Sperrung PID.
//!
//!
//! **PIDs 55007–55009 — NB-initiated Lieferende:** These PIDs are **present**
//! in UTILMD AHB Strom 2.1 (FV2025-10-01) and are handled by the separate
//! [`super::lf_abmeldung::GpkeLfAbmeldungWorkflow`] (LF-side). They are NOT
//! registered here. PIDs 55013–55015 (Ersatz-/Grundversorgung) are handled by
//! [`super::eog::GpkeEogWorkflow`]; 55010–55012 (Anfrage zur Beendigung der
//! Zuordnung — the NB Abmeldeanfrage) by
//! [`super::beendigung_zuordnung::GpkeBeendigungZuordnungWorkflow`].
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **BK6-22-024** — BNetzA ruling governing the GPKE timeline obligations
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format for grid connection processes
//! - **APERAK 2.x** — Application error acknowledgement (**24h** wall-clock Frist)

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{APERAK_STROM_WINDOW_LABEL, aperak_strom_due_at};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-supplier-change";

/// Inbound ANFRAGE PIDs handled by this workflow as a receiving NB/LFA.
///
/// Only these PIDs are routed to `GpkeSupplierChangeWorkflow` by the engine.
/// The corresponding outbound ANTWORT PIDs (55002/55003, 55005/55006, 55078, 55080)
/// are derived internally by `response_pid_for` and stored in the `AntwortGesendet` event.
///
/// | PID   | Process (LFW24 AHB name)                          | AHB profile  |
/// |-------|---------------------------------------------------|--------------|
/// | 55001 | Anfrage Lieferbeginn verb. MaLo (LFN → NB)        | S2.1–S2.2 ✅ |
/// | 55004 | Abmeldung / Lieferende verb. MaLo (LFN → NB)      | S2.1–S2.2 ✅ |
/// | 55077 | Anmeldung Lieferbeginn erz. MaLo (LFN → NB)       | S2.1–S2.2 ✅ |
///
/// ORDERS Sperrung (PIDs 17115/17116/17117) is handled by `GpkeSperrungWorkflow`
/// (see `sperrung` module). **PID 55555** is the GPKE Teil 4 data request
/// („Anfrage Daten der individuellen Bestellung", `gpke-anfrage-bestellung`) and
/// **PID 55557** the Änderung MSB-Abrechnungsdaten der MaLo, which is an
/// ordinary Teil-4 Stammdatenänderung answered 55559 by
/// `GpkeStammdatenaenderungWorkflow`. **PIDs 55007–55015** are NB-initiated;
/// 55007–55009 route to `GpkeLfAbmeldungWorkflow`.
pub const UTILMD_PIDS: &[u32] = &[
    55001, // Anmeldung verb. MaLo (LF → NB); answers 55002/55003
    55004, // Abmeldung (LF → NB); answers 55005/55006
    55077, // Anmeldung Lieferbeginn erz. MaLo (LFN → NB, BK6-24-174)
];

/// The subset of [`UTILMD_PIDS`] this workflow can carry to completion.
///
/// `SendAntwort` derives its outbound PID from `response_pid_for`, which knows
/// only these four. Spawning a process for a PID it cannot answer produces one
/// that sits until its registered deadline and then transitions to
/// `Rejected` without anything having gone wrong — worse than not accepting the
/// message, because it manufactures a false rejection.
///
/// `55557` (Änderung MSB-Abrechnungsdaten, GPKE Teil 4) stays registered so the
/// router still resolves it, but has no Antwort mapping and therefore no
/// receiving implementation yet.
pub const UTILMD_ANFRAGE_PIDS: &[u32] = &[55001, 55004, 55077];

/// IFTSTA GPKE Prüfidentifikatoren — PIDs 21024–21028, 21033, 21035.
///
/// These are the Vollzugsmeldung and Statusmeldung messages exchanged by LF,
/// NB/LFA in the GPKE supplier-change (Wechsel) process (BK6-22-024 LFW24).
/// All are routed to `"gpke-supplier-change"` for correlation via conversation
/// ID (CI tag).
///
/// GPKE IFTSTA messages are informational: they confirm that a physical switch
/// has been executed (Vollzugsmeldung) or report on process status.
/// They do not drive state transitions in the supplier-change state machine
/// but are recorded in the event log for audit purposes.
///
/// **AHB-authoritative routing (IFTSTA AHB fv20251001/fv20261001):**
/// PIDs 21024–21028 are labeled "GPKE / Vollzugsmeldung" in the AHB.
/// Earlier pid-reference.md entries attributing them to WiM Gas / GeLi Gas
/// were incorrect. The AHB profile is the single source of truth.
///
/// | PID   | AHB-Name | Richtung |
/// |-------|---|---|
/// | 21024 | GPKE / Vollzugsmeldung Lieferantenwechsel | NB → LF |
/// | 21025 | GPKE / Vollzugsmeldung Einzug | NB → LF |
/// | 21026 | GPKE / Vollzugsmeldung Auszug | NB → LF |
/// | 21027 | GPKE / Vollzugsmeldung Netznutzung | NB → LF |
/// | 21028 | GPKE / Vollzugsmeldung | NB → LF |
/// | 21033 | GPKE / Statusmeldung Kündigung (Ablehnung GPKE Teil 3) | MSB → NB/LF |
/// | 21035 | GPKE Teil 2 / Rückmeldung an Lieferstelle | MSB → LF |
/// | 21045 | EnFG Informationen (GPKE Teil 4) | LF → NB |
/// | 21047 | Bearbeitungsstandsmeldung (GPKE Teil 2/4) | NB → LF · NB → ÜNB · MSB → NB |
pub const IFTSTA_PIDS: &[u32] = &[
    21_024, 21_025, 21_026, 21_027, 21_028, 21_033,
    21_035, // Rückmeldung an Lieferstelle (GPKE Teil 2, MSB → LF) — pid-reference.md
    21_045, // EnFG Informationen (GPKE Teil 4, LF → NB)
    21_047, // Bearbeitungsstandsmeldung (GPKE Teil 2/4) — pid-reference.md
];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE supplier-change workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SupplierChangeEvent {
    /// Process initiated by a valid UTILMD Lieferbeginn.
    Initiated {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// GLN of the new supplier (nLF).
        new_supplier: MarktpartnerCode,
        /// GLN of the grid operator (NB).
        grid_operator: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Process-specific date (e.g. Lieferbeginn-Datum, DTM 163), `YYYYMMDD`.
        #[serde(default)]
        process_date: String,
        /// SG4 STS Transaktionsgrund (DE9013), when transmitted.
        #[serde(default)]
        transaktionsgrund: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound UTILMD business response (55002/55003/55005/55006/55078/55080) was sent
    /// to the counterparty. The response PID is derived from the anfrage PID and
    /// the `accepted` flag by `response_pid_for`.
    AntwortGesendet {
        /// Derived outbound UTILMD response PID.
        response_pid: Option<Pruefidentifikator>,
        /// `true` if the request was accepted, `false` if rejected.
        accepted: bool,
        /// Rejection reason (only set when `accepted = false`).
        reason: Option<String>,
    },
    /// Supply relationship became active.
    Activated,
    /// APERAK 29001 (Verarbeitbarkeitsfehler) was sent to the counterparty.
    ///
    /// Emitted when the NB cannot process the inbound UTILMD for technical
    /// reasons (e.g., duplicate MsgId, system error, inconsistent state).
    /// This is distinct from a business rejection (`AntwortGesendet { accepted: false }`):
    /// the APERAK replaces the UTILMD 55002/55003 response channel entirely.
    ///
    /// BDEW GPKE / BK6-22-024: Must be dispatched within **24 wall-clock hours**.
    AperakFehlerDispatched {
        /// APERAK Prüfidentifikator sent (29001 = Verarbeitbarkeitsfehler).
        aperak_pid: Pruefidentifikator,
        /// Error reason included in the APERAK.
        reason: String,
        /// Reference ID of the outbound APERAK message.
        outbound_ref: MessageRef,
    },
    /// Process was rejected and closed.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline expired before the process completed.
    ///
    /// Stored in the event log for audit and read-model purposes.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received a GPKE IFTSTA Vollzugsmeldung or Statusmeldung (PIDs 21024–21028, 21033).
    ///
    /// These PIDs carry GPKE-domain completion confirmations (Vollzugsmeldungen)
    /// and status notifications. Informational — does not drive state transitions;
    /// recorded for audit purposes.
    VollzugsmeldungReceived {
        /// IFTSTA Prüfidentifikator (21024–21028 or 21033).
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
}

impl EventPayload for SupplierChangeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "SupplierChangeInitiated",
            Self::ValidationPassed { .. } => "SupplierChangeValidationPassed",
            Self::AntwortGesendet { .. } => "SupplierChangeAntwortGesendet",
            Self::Activated => "SupplierChangeActivated",
            Self::AperakFehlerDispatched { .. } => "SupplierChangeAperakFehlerDispatched",
            Self::Rejected { .. } => "SupplierChangeRejected",
            Self::DeadlineExpired { .. } => "SupplierChangeDeadlineExpired",
            Self::VollzugsmeldungReceived { .. } => "SupplierChangeVollzugsmeldungReceived",
        }
    }
    // schema_version defaults to 1; increment and add an upcast arm on next
    // backward-incompatible payload layout change.
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set at `Initiated` time and carried through every later state.
///
/// All fields are structurally guaranteed to be present once the process moves
/// past `New` — no `unwrap()` required downstream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitiatedData {
    /// EIC/MaLo code for the supply location.
    pub location_id: MaLo,
    /// Market partner code (GLN) of the new supplier.
    pub new_supplier: MarktpartnerCode,
    /// Market partner code (GLN) of the responsible grid operator.
    pub grid_operator: MarktpartnerCode,
    /// EDIFACT document date string from the UTILMD (`YYYYMMDD`).
    pub document_date: String,
    /// Process-specific date (e.g. Lieferbeginn-Datum, DTM 163) from the UTILMD (`YYYYMMDD`).
    #[serde(default)]
    pub process_date: String,
    /// SG4 STS Transaktionsgrund (DE9013), when transmitted.
    #[serde(default)]
    pub transaktionsgrund: Option<String>,
    /// BDEW Prüfidentifikator — identifies the process family and step.
    pub pruefidentifikator: Pruefidentifikator,
}

/// Current state of a GPKE supplier-change process stream.
///
/// Modelled as an enum-per-variant to eliminate all `Option`-unwraps:
/// each variant carries exactly the data that is structurally available at
/// that stage. Invalid states are unrepresentable.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → AntwortGesendet → Active
///                                    ↘ Rejected (negative Antwort or deadline)
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum SupplierChangeState {
    /// No events yet. Stream exists but process has not started.
    #[default]
    New,
    /// UTILMD received and `Initiated` event applied.
    Initiated(InitiatedData),
    /// EDIFACT validation passed; UTILMD business response not yet sent.
    ValidationPassed(InitiatedData),
    /// Outbound UTILMD business response sent; awaiting supply activation.
    AntwortGesendet {
        /// Process data from the Anfrage.
        data: InitiatedData,
        /// Derived outbound response PID (e.g. 55002 for an accepted 55001).
        response_pid: Option<Pruefidentifikator>,
    },
    /// Supply relationship is active (or Kündigung is complete).
    Active(InitiatedData),
    /// Process rejected (validation failure or negative Antwort).
    Rejected {
        /// Human-readable rejection reason for read models and auditing.
        reason: String,
    },
}

impl SupplierChangeState {
    /// Stable string label for the current variant.
    ///
    /// Used in read models and structured log output without deserializing
    /// the full enum.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Active(_) => "Active",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return `Some(&InitiatedData)` when the process has been initiated,
    /// or `None` when it is still `New` or `Rejected`.
    #[must_use]
    pub fn initiated_data(&self) -> Option<&InitiatedData> {
        match self {
            Self::Initiated(d) | Self::ValidationPassed(d) | Self::Active(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE supplier-change workflow.
///
/// **All domain values must be pre-extracted by the transport layer** before
/// constructing a command. `Workflow::handle()` is pure — no I/O, no EDIFACT
/// parsing, no external calls. See the crate-level doc for a construction
/// example.
#[derive(Clone)]
pub enum SupplierChangeCommand {
    /// Inbound UTILMD accepted from the AS4 layer. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator.
        pid: Pruefidentifikator,
        /// GLN of the message sender (nLF).
        sender: MarktpartnerCode,
        /// GLN of the message receiver (NB).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Process-specific date (e.g. Lieferbeginn-Datum, DTM 163), `YYYYMMDD`.
        process_date: String,
        /// Bilanzierungsgebiet EIC code extracted from the UTILMD (NAD+Z09 / LOC+237).
        ///
        /// Used by `processd` NB check 4 (bilanzierungsgebiet consistency).
        /// The adapter in `makod/src/adapters.rs` should populate this when the
        /// UTILMD carries a `LOC+237` segment; `None` when absent (rare in practice).
        ///
        /// Propagated verbatim into the `ProcessInitiated` outbox message so that
        /// `processd` can use it directly without a `marktd` fallback round-trip.
        bilanzierungsgebiet: Option<String>,
        /// Bilanzierungsmethode extracted from UTILMD `TM+EM` segment.
        ///
        /// `"SLP"` | `"RLM"` | `"IMS"` — maps to BO4E `Bilanzierungsmethode`.
        /// Populated from TM+EM qualifier: Z01=SLP, Z02=RLM, Z04=IMS.
        /// Used by `marktd` to update `malo.bilanzierungsmethode` on receipt of
        /// `de.mako.process.initiated`.
        bilanzierungsmethode: Option<String>,
        /// Gas GaBi RLM Fallgruppe from UTILMD `TM+Z10` segment.
        ///
        /// Only set for Gas RLM MaLos. Populated from TM qualifier Z10 + category code.
        /// Stored in `marktd` `malo.fallgruppe` for Gas GaBi MMM billing.
        fallgruppe: Option<String>,
        /// SG4 STS Transaktionsgrund (DE9013, category 7) — e.g. `E01`
        /// Ein-/Auszug (Umzug), `E03` Lieferantenwechsel, `Z33`, `E06`.
        ///
        /// Drives the `mako-pruefung` date-plausibility rules: GPKE permits a
        /// retroactive Lieferbeginn for Ein-/Auszug but not for a regular
        /// Wechsel. Propagated into the `ProcessInitiated` outbox payload.
        transaktionsgrund: Option<String>,
        /// SG4 `STS+7` DE 9013 **element 3** — the Transaktionsgrundergänzung
        /// (`ZW4` verbrauchende, `ZW3` erzeugende, `ZAP` ruhende Marktlokation).
        ///
        /// It is a *different composite* from the Anmeldegrund in element 2, not
        /// a second `STS+7` segment: `STS+7++E03:ZW3` carries both. Scanning the
        /// parsed `Sts` list for a `reason_code == "ZW3"` therefore never
        /// matched, which is why this is the raw Ergänzung and not a boolean.
        ///
        /// `processd` maps it onto `mako_pruefung::Marktlokationsart`, which
        /// decides which of `E_0622`'s two disjoint code spaces answers.
        transaktionsgrund_ergaenzung: Option<String>,
        /// SG10 `CCI+Z22` DE 7037 — the Veräußerungsform of an erzeugende
        /// Marktlokation (`Z90` Einspeise-/Ausfallvergütung, `Z91` Marktprämie,
        /// `Z92` sonstige Direktvermarktung, `Z94` KWKG-Vergütung).
        ///
        /// One of the two facts `E_0622` Prüfschritte 400–830 choose the
        /// Vorlauffrist from; the other (the *bestehende* Veräußerungsform) is
        /// the NB's own register.
        veraeusserungsform: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// UTC timestamp at which the inbound UTILMD was received at the transport layer.
        ///
        /// Used to compute the APERAK 45-minute sending deadline per APERAK AHB 1.0 §2.4.1.
        received_at: time::OffsetDateTime,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD business response to the counterparty.
    ///
    /// The workflow derives the correct response PID from the anfrage PID and
    /// the `accepted` flag:
    /// - 55001 (Anmeldung / Lieferbeginn) → 55002 (accepted) / 55003 (rejected)
    /// - 55004 (Abmeldung / Lieferende)   → 55005 (accepted) / 55006 (rejected)
    /// - 55077 (Anmeldung erz. MaLo)      → 55078 (accepted) / 55080 (rejected)
    ///
    /// BDEW GPKE / BK6-22-024: Response must be sent within **24 wall-clock
    /// hours** of receiving the UTILMD Anfrage (not Werktage).
    ///
    /// # Post-acceptance obligations
    ///
    /// The workflow is a pure state machine: it does not know which downstream
    /// processes (MSCONS, ORDERS, …) must be triggered after a PID-55001
    /// acceptance. Callers are responsible for computing the `obligations` slice
    /// using [`crate::post_acceptance::lieferbeginn_obligations`] and passing it
    /// here. When `accepted = true`, all entries in `obligations` are
    /// co-persisted atomically with the `AntwortGesendet` event via the outbox.
    /// When `accepted = false`, `obligations` is ignored.
    ///
    /// This design keeps cross-process PID knowledge (MSCONS 13015, ORDERS 17134)
    /// outside the supplier-change state machine while preserving the atomicity
    /// guarantee required by BK6-22-024.
    SendAntwort {
        /// The NB's answer: the resolved EBD Antwortcode and its cluster.
        ///
        /// `SG4 STS+E01` is Muss on every Antwortnachricht and the AHB
        /// restricts the code to the named tree's cluster, so the answer is a
        /// code — not a boolean with a free-text reason. The Bestätigung of a
        /// Lieferbeginn is `A51` / `A58` out of `E_0623`, the Ablehnung one of
        /// `E_0622`'s; Gas answers out of `G_0011` / `G_0012`.
        antwort: crate::LfAntwort,
        /// Pre-computed post-acceptance outbox obligations to co-persist
        /// atomically with the `AntwortGesendet` event.
        ///
        /// Build this with [`crate::post_acceptance::lieferbeginn_obligations`]
        /// for PID 55001 (Lieferbeginn). Pass an empty `Vec` for all other PIDs
        /// or when no downstream processes must be triggered.
        ///
        /// Ignored when the answer is an Ablehnung.
        obligations: Vec<PendingOutbox>,
    },
    /// Mark the supply relationship as active after all checks pass.
    Activate,
    /// Dispatch APERAK 29001 (Verarbeitbarkeitsfehler) to the counterparty.
    ///
    /// Use this when the NB system cannot process the inbound UTILMD for
    /// technical reasons (duplicate MsgId, system error, inconsistent state).
    /// The APERAK replaces the UTILMD 55002/55003 response channel.
    ///
    /// May be sent from `Initiated` or `ValidationPassed` state. Transitions
    /// the process to `Rejected`.
    ///
    /// BDEW GPKE / BK6-22-024: Must be dispatched within **24 wall-clock hours**.
    DispatchAperakFehler {
        /// Error reason included in the APERAK.
        reason: String,
        /// Message reference assigned to the outbound APERAK.
        outbound_ref: MessageRef,
    },
    /// A registered deadline fired and was dispatched by the scheduler.
    ///
    /// The workflow records a `DeadlineExpired` event and transitions to
    /// `Rejected` unless the process has already reached a terminal state
    /// (`Active` or `Rejected`), in which case this command is a no-op.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received a GPKE IFTSTA Vollzugsmeldung or Statusmeldung (PIDs 21024–21028, 21033).
    ///
    /// Constructed by the IFTSTA adapter in `makod` when an inbound AS4
    /// IFTSTA message with a GPKE-domain PID arrives, or via the
    /// `"gpke.vollzugsmeldung.empfangen"` REST command.
    ReceiveVollzugsmeldung {
        /// IFTSTA Prüfidentifikator (21024–21028 or 21033).
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Whether the IFTSTA message passed AHB validation.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for SupplierChangeCommand {}

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound UTILMD response PID from the inbound ANFRAGE PID.
///
/// | Anfrage | Anwendungsfall | accepted=true | accepted=false |
/// |---------|----------------|---------------|----------------|
/// | 55001   | Anmeldung verb. MaLo | 55002   | 55003          |
/// | 55004   | Abmeldung            | 55005   | 55006          |
/// | 55077   | Anmeldung erz. MaLo  | 55078   | 55080          |
///
/// The UTILMD AHB Strom lays each Anwendungsfall out as a **triple** of adjacent
/// PID columns — `(Anfrage, Bestätigung, Ablehnung)` with the header reading
/// `LF an NB / NB an LF / NB an LF` — so the response is `anfrage + 1` to accept
/// and `anfrage + 2` to reject. Verified against AHB Strom 2.1 §55001/§55004 and
/// 2.2; `pid-reference.md` carries the same mapping.
///
/// Note: PID 55079 does not exist in BDEW UTILMD AHB Strom (no such PID
/// assigned), which is why 55077 rejects with 55080.
pub(crate) fn response_pid_for(anfrage_pid: u32, accepted: bool) -> Option<Pruefidentifikator> {
    let code: u32 = match anfrage_pid {
        55001 => {
            if accepted {
                55002 // Bestätigung Anmeldung verb. MaLo (NB an LF)
            } else {
                55003 // Ablehnung Anmeldung verb. MaLo (NB an LF)
            }
        }
        55004 => {
            if accepted {
                55005 // Bestätigung Abmeldung (NB an LF)
            } else {
                55006 // Ablehnung Abmeldung (NB an LF)
            }
        }
        55016 => {
            if accepted {
                55017 // Bestätigung Kündigung (LFA → LFN)
            } else {
                55018 // Ablehnung Kündigung (LFA → LFN)
            }
        }
        55077 => {
            if accepted {
                55078 // Bestätigung Anmeldung erz. MaLo (NB → LFN)
            } else {
                55080 // Ablehnung Anmeldung erz. MaLo (NB → LFN); 55079 is unassigned
            }
        }
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Deadline label constants ──────────────────────────────────────────────────

/// Deadline label for the **24-hour process response window** per BK6-22-024 §4(3).
///
/// This is the maximum time within which the NB must send a UTILMD
/// Bestätigung or Ablehnung after receiving the LF's Lieferbeginn UTILMD.
/// It is **not** the APERAK *sending* deadline (45 min weekday per
/// APERAK AHB 1.0 §2.4.1 — see `mako_fristen::APERAK_STROM_WINDOW_LABEL`).
///
/// Register the deadline immediately after `ReceiveUtilmd` is processed:
/// ```rust,ignore
/// let due = mako_fristen::antwort::antwort_deadline(pid, received).expect("published");
/// let deadline = Deadline::new(process.stream_id().clone(), ..., GPKE_PROCESS_RESPONSE_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
///
/// The scheduler fires `on_deadline` → `SupplierChangeCommand::TimeoutExpired`
/// when the window lapses.
pub const GPKE_PROCESS_RESPONSE_LABEL: &str = "gpke-response-24h-window";

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Lieferbeginn Strom (PID 55001) workflow.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-supplier-change", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeSupplierChangeWorkflow;

impl Workflow for GpkeSupplierChangeWorkflow {
    type State = SupplierChangeState;
    type Event = SupplierChangeEvent;
    type Command = SupplierChangeCommand;

    /// Deadline compensation for GPKE regulatory timeouts.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"aperak-window"` | `Initiated` or `ValidationPassed` | `TimeoutExpired` | BK6-22-024 §4(3) — 24h Frist |
    ///
    /// Processes in `Active` or `Rejected` state absorb any late-firing
    /// deadline as a no-op via `TimeoutExpired`'s idempotent handler.
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            // Process response window expired before the NB sent a UTILMD
            // Bestätigung or Ablehnung → close as Rejected (BK6-22-024 §4(3)).
            (GPKE_PROCESS_RESPONSE_LABEL, SupplierChangeState::Initiated(_))
            | (GPKE_PROCESS_RESPONSE_LABEL, SupplierChangeState::ValidationPassed(_)) => {
                Some(SupplierChangeCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            // All other deadline labels, or terminal states — no-op.
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SupplierChangeEvent::Initiated {
                location_id,
                new_supplier,
                grid_operator,
                document_date,
                process_date,
                transaktionsgrund,
                pruefidentifikator,
                ..
            } => SupplierChangeState::Initiated(InitiatedData {
                location_id: location_id.clone(),
                new_supplier: new_supplier.clone(),
                grid_operator: grid_operator.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                transaktionsgrund: transaktionsgrund.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            SupplierChangeEvent::ValidationPassed { .. } => {
                // Transition to ValidationPassed, carrying forward InitiatedData.
                match state {
                    SupplierChangeState::Initiated(data) => {
                        SupplierChangeState::ValidationPassed(data)
                    }
                    other => other, // defensive: no-op on unexpected state
                }
            }
            SupplierChangeEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        SupplierChangeState::ValidationPassed(data) => {
                            SupplierChangeState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    SupplierChangeState::Rejected {
                        reason: "Anfrage abgelehnt".to_owned(),
                    }
                }
            }
            SupplierChangeEvent::Activated => match state {
                SupplierChangeState::AntwortGesendet { data, .. } => {
                    SupplierChangeState::Active(data)
                }
                other => other,
            },
            SupplierChangeEvent::AperakFehlerDispatched { reason, .. } => {
                SupplierChangeState::Rejected {
                    reason: format!("APERAK 29001: {reason}"),
                }
            }
            SupplierChangeEvent::Rejected { reason } => SupplierChangeState::Rejected {
                reason: reason.clone(),
            },
            SupplierChangeEvent::DeadlineExpired { label, .. } => {
                // Treat any deadline expiry as a rejection unless the process
                // has already completed (Active) or is already Rejected.
                match state {
                    SupplierChangeState::Active(_) | SupplierChangeState::Rejected { .. } => state,
                    _ => SupplierChangeState::Rejected {
                        reason: format!("deadline expired: {label}"),
                    },
                }
            }

            // Informational GPKE Vollzugsmeldung/Statusmeldung — no state change.
            SupplierChangeEvent::VollzugsmeldungReceived { .. } => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            SupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                bilanzierungsgebiet,
                bilanzierungsmethode,
                fallgruppe,
                transaktionsgrund,
                transaktionsgrund_ergaenzung,
                veraeusserungsform,
                message_ref,
                received_at,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, SupplierChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                // PID guard — accepts only inbound ANFRAGE PIDs per UTILMD AHB S2.1/S2.2.
                // Response PIDs 55002/55003, 55005/55006 are outbound; they are derived
                // internally by response_pid_for() and stored in AntwortGesendet events.
                // ORDERS Sperrung (17115/17116/17117) is routed to GpkeSperrungWorkflow.
                // PID 55555 is "Anfrage Daten der individuellen Bestellung" (GPKE Teil 4).
                //
                // PIDs 55007–55009 (NB-seitiges Lieferende, NB→LF) are routed to
                // GpkeLfAbmeldungWorkflow (lf_abmeldung module) — NOT this workflow.
                // DO NOT add 55007–55009 to UTILMD_PIDS here.
                if !UTILMD_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an inbound ANFRAGE PID (55001, 55004 or 55077), \
                         got {pid}. Response PIDs (55002/55003, 55005/55006) \
                         are outbound only. ORDERS Sperrung (17115/17116/17117) \
                         routes to GpkeSperrungWorkflow.",
                    )));
                }
                let mut events = vec![SupplierChangeEvent::Initiated {
                    location_id: location_id.clone(),
                    new_supplier: sender.clone(),
                    grid_operator: receiver.clone(),
                    document_date,
                    process_date: process_date.clone(),
                    transaktionsgrund: transaktionsgrund.clone(),
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(SupplierChangeEvent::ValidationPassed { message_ref });
                } else {
                    events.push(SupplierChangeEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }

                // Notify the NB ERP that a new Lieferbeginn/Lieferende/Kündigung
                // Anfrage has arrived and is awaiting the NB decision.
                // The OutboxErpWorker delivers this as a CloudEvent
                // (`de.mako.process.initiated`) to the configured ERP webhook URL
                // so the ERP can review and then call
                // `gpke.lieferbeginn.bestaetigen` / `gpke.lieferbeginn.ablehnen`
                // via the command API.
                //
                // Only emitted when validation passed — rejected interchanges do
                // not require any NB action.
                let outbox = if validation_passed {
                    vec![
                        PendingOutbox::new(
                            "ProcessInitiated",
                            receiver.as_str(),
                            serde_json::json!({
                                "pid":                   pid.as_u32(),
                                "malo_id":               location_id.as_str(),
                                "new_supplier":          sender.as_str(),
                                "grid_operator":         receiver.as_str(),
                                "process_date":          process_date,
                                // bilanzierungsgebiet is consumed by processd NB check 4.
                                // Populated by makod adapter from UTILMD LOC+237/NAD+Z09.
                                "bilanzierungsgebiet":   bilanzierungsgebiet,
                                // bilanzierungsmethode and fallgruppe are consumed by
                                // marktd to update malo columns (L1/N1).
                                "bilanzierungsmethode":  bilanzierungsmethode,
                                "fallgruppe":            fallgruppe,
                                // SG4 STS Transaktionsgrund — consumed by processd
                                // `mako-pruefung` (date-plausibility rules).
                                "transaktionsgrund":     transaktionsgrund,
                                // SG4 STS+7 DE 9013 element 3 and SG10 CCI+Z22
                                // DE 7037 — `processd` derives the Marktlokationsart
                                // and the angemeldete Veräußerungsform from them,
                                // which together pick one of E_0622's six published
                                // Vorlauffristen.
                                "transaktionsgrund_ergaenzung": transaktionsgrund_ergaenzung,
                                "veraeusserungsform":    veraeusserungsform,
                            }),
                        )
                        // Caused by ValidationPassed (index 1), not Initiated (index 0),
                        // so the ERP event correlates to the event that confirms the
                        // message was parseable and process-able.
                        .caused_by(1),
                        // F-038: APERAK BGM+312 (Anerkennungsmeldung) — mandatory per
                        // APERAK AHB 1.0 §2.4: both Anerkennung and rejection must always
                        // be sent for Strom (unlike Gas, where silence = acceptance).
                        // Frist (UTILMD/ORDERS weekday): 45 Min (APERAK AHB 1.0 §2.4.1).
                        // Saturday UTILMD/ORDERS: Sonntag 12 Uhr; other: nächster Werktag 12 Uhr.
                        PendingOutbox::new(
                            "APERAK",
                            sender.as_str(),
                            serde_json::json!({
                                "sender":        receiver.as_str(),
                                "receiver":      sender.as_str(),
                                "pid":           29001_u32,
                                "document_code": "312",
                            }),
                        )
                        .caused_by(1),
                    ]
                } else {
                    // F-035: AHB validation failed — emit APERAK BGM+313
                    // (Verarbeitbarkeitsfehlermeldung) back to the new supplier.
                    // APERAK AHB 1.0 §2.1.1: mandatory rejection APERAK on validation failure.
                    // Caused by Initiated (index 0) — no ValidationPassed event in this path.
                    vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender.as_str(),
                            serde_json::json!({
                                "sender":     receiver.as_str(),
                                "receiver":   sender.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     validation_errors.join("; "),
                            }),
                        )
                        .caused_by(0),
                    ]
                };

                // APERAK AHB 1.0 §2.4.1: register the 45-minute sending deadline.
                // The engine persists this atomically alongside the events.
                let aperak_deadline = PendingDeadline::new(
                    APERAK_STROM_WINDOW_LABEL,
                    aperak_strom_due_at(received_at),
                );
                Ok(WorkflowOutput::with_outbox_and_deadline(
                    events,
                    outbox,
                    aperak_deadline,
                ))
            }

            SupplierChangeCommand::SendAntwort {
                antwort,
                obligations,
            } => {
                let accepted = antwort.zustimmung;
                let data = match state {
                    SupplierChangeState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let response_pid = response_pid_for(data.pruefidentifikator.as_u32(), accepted);
                let events = vec![SupplierChangeEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason: antwort.bemerkung.clone(),
                }];

                // Always enqueue the UTILMD response back to the new supplier
                // (55002/55003/55005/55006/55078/55080).
                let mut outbox: Vec<PendingOutbox> = vec![];
                if let Some(rpid) = response_pid {
                    outbox.push(PendingOutbox::new(
                        "UTILMD",
                        data.new_supplier.as_str(),
                        serde_json::json!({
                            "pid":          rpid.as_u32(),
                            "sender":       data.grid_operator.as_str(),
                            "receiver":     data.new_supplier.as_str(),
                            "malo":         data.location_id.as_str(),
                            "process_date": data.process_date,
                            // `SG4 STS+E01++<code>:<ebd>` — Muss on every
                            // Antwortnachricht. Without it the renderer emits a
                            // well-formed UTILMD that states no Grund at all.
                            "antwort_code": antwort.antwort_code,
                            "antwort_ebd":  antwort.ebd,
                            // `FTX+ACB` — the Erläuterung the catch-all codes
                            // require and every Ablehnung benefits from.
                            "bemerkung":    antwort.bemerkung,
                        }),
                    ));
                }
                // Co-persist caller-provided post-acceptance obligations
                // atomically with the AntwortGesendet event.  Obligations are
                // ignored when the process is rejected — the caller may safely
                // pass a pre-computed slice regardless of acceptance outcome.
                if accepted {
                    outbox.extend(obligations);
                }

                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            SupplierChangeCommand::Activate => {
                if !matches!(state, SupplierChangeState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![SupplierChangeEvent::Activated].into())
            }

            SupplierChangeCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    SupplierChangeState::New => {
                        return Err(WorkflowError::invalid_state(
                            "Initiated or ValidationPassed",
                            state.label(),
                        ));
                    }
                    SupplierChangeState::Active(_) | SupplierChangeState::Rejected { .. } => {
                        return Err(WorkflowError::invalid_state(
                            "Initiated or ValidationPassed",
                            state.label(),
                        ));
                    }
                    _ => {}
                }
                let aperak_pid = Pruefidentifikator::new(29001)
                    .map_err(|_| WorkflowError::other("invalid APERAK PID 29001"))?;
                let events = vec![SupplierChangeEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason: reason.clone(),
                    outbound_ref,
                }];

                // F-035: emit the APERAK outbox entry atomically with the event.
                // APERAK AHB 1.0 §2.1.1: mandatory within 24h (BK6-22-024 §4).
                let outbox = if let Some(data) = state.initiated_data() {
                    vec![
                        PendingOutbox::new(
                            "APERAK",
                            data.new_supplier.as_str(),
                            serde_json::json!({
                                "sender":     data.grid_operator.as_str(),
                                "receiver":   data.new_supplier.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ]
                } else {
                    vec![]
                };

                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            SupplierChangeCommand::TimeoutExpired { deadline_id, label } => {
                // Idempotent: terminal states (Active, Rejected) absorb the
                // event without error so a late-firing deadline does not
                // corrupt an already-completed process.
                if matches!(
                    state,
                    SupplierChangeState::Active(_) | SupplierChangeState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }

                // Compensation: enqueue an AperakTimeout outbox entry so the
                // OutboxErpWorker notifies the ERP that no APERAK was received
                // within the 24h regulatory window (BK6-22-024 §4(3)).
                // The outbox entry is persisted atomically with DeadlineExpired
                // via execute_and_enqueue_with_retry → WriteBatch.
                let mut outbox: Vec<PendingOutbox> = vec![];
                if let Some(data) = state.initiated_data() {
                    outbox.push(PendingOutbox::new(
                        "AperakTimeout",
                        data.new_supplier.as_str(),
                        serde_json::json!({
                            "pid":          data.pruefidentifikator.as_u32(),
                            "malo":         data.location_id.as_str(),
                            "new_supplier": data.new_supplier.as_str(),
                            "grid_operator": data.grid_operator.as_str(),
                            "deadline_label": label.as_ref(),
                            "deadline_id":  deadline_id,
                        }),
                    ));
                }

                let event = SupplierChangeEvent::DeadlineExpired { deadline_id, label };
                if outbox.is_empty() {
                    Ok(vec![event].into())
                } else {
                    Ok(WorkflowOutput::with_outbox(vec![event], outbox))
                }
            }

            SupplierChangeCommand::ReceiveVollzugsmeldung {
                pid,
                sender,
                receiver,
                message_ref,
                ..
            } => {
                // GPKE Vollzugsmeldungen and Statusmeldungen are informational.
                // Accept in any state (the process may be Active or Rejected
                // when a late Vollzugsmeldung arrives) and record for audit.
                Ok(vec![SupplierChangeEvent::VollzugsmeldungReceived {
                    pid,
                    sender,
                    receiver,
                    message_ref,
                }]
                .into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Business data available once the process has been initiated.
///
/// All fields are guaranteed present once the `Initiated` event has been applied.
/// Use [`SupplierChangeRecord::details`] to access this.
#[derive(Debug, Clone)]
pub struct InitiatedDetails {
    /// EIC/MaLo supply location code.
    pub location_id: MaLo,
    /// New supplier GLN.
    pub new_supplier: MarktpartnerCode,
    /// Grid operator GLN.
    pub grid_operator: MarktpartnerCode,
    /// BDEW Prüfidentifikator (e.g. 55001).
    pub pruefidentifikator: Pruefidentifikator,
}

/// Read-model record for a single supplier-change process stream.
///
/// Starts in the `New` state (no business data yet). Once the `Initiated` event
/// is applied, [`details`][Self::details] carries the full `InitiatedDetails`.
/// Callers downstream of `Initiated` can call `details().expect("…")` or match on it.
#[derive(Debug)]
pub struct SupplierChangeRecord {
    /// Current lifecycle status label (e.g. `"Initiated"`, `"Active"`).
    pub status: &'static str,
    /// Business data populated on `Initiated`; `None` while still in `New` state.
    pub details: Option<InitiatedDetails>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for SupplierChangeRecord {
    fn default() -> Self {
        Self {
            status: "New",
            details: None,
            event_count: 0,
        }
    }
}

/// In-process read model that tracks status across all GPKE supplier-change
/// streams. Feed via [`mako_engine::projection::ProjectionRunner`].
#[derive(Debug, Default)]
pub struct SupplierChangeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, SupplierChangeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for SupplierChangeProjection {
    fn name(&self) -> &'static str {
        "SupplierChangeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<SupplierChangeEvent>() else {
            return;
        };

        match event {
            SupplierChangeEvent::Initiated {
                location_id,
                new_supplier,
                grid_operator,
                pruefidentifikator,
                ..
            } => {
                record.status = "Initiated";
                record.details = Some(InitiatedDetails {
                    location_id,
                    new_supplier,
                    grid_operator,
                    pruefidentifikator,
                });
            }
            SupplierChangeEvent::ValidationPassed { .. } => {
                record.status = "ValidationPassed";
            }
            SupplierChangeEvent::AntwortGesendet { accepted, .. } => {
                record.status = if accepted {
                    "AntwortGesendet"
                } else {
                    "Rejected"
                };
            }
            SupplierChangeEvent::Activated => {
                record.status = "Active";
            }
            SupplierChangeEvent::AperakFehlerDispatched { .. } => {
                record.status = "Rejected";
            }
            SupplierChangeEvent::Rejected { .. } => {
                record.status = "Rejected";
            }
            SupplierChangeEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
            }
            SupplierChangeEvent::VollzugsmeldungReceived { .. } => {
                // Informational — does not change the status label.
            }
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        if self.last_seq == 0 {
            None
        } else {
            Some(self.last_seq)
        }
    }
}
