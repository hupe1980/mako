//! Phase 2 ingest execution: EDIFACT message → typed domain command → Process.
//!
//! [`EdifactIngestDispatcher`] bridges parsed [`AnyMessage`] values from the
//! EDIFACT ingest layer to running domain workflow processes.  Used by:
//!
//! - `edifact_api` — REST `POST /edifact` path
//! - `as4_ingest`  — AS4 inbound delivery path
//! - `as4_sender`  — in-process loopback for self-addressed messages
//!   (combined-role deployments: NB+LF, NB+MSB, GNB+gMSB sharing one MP-ID)
//!
//! ## Routing strategy
//!
//! The caller supplies the pre-computed `workflow_name` (from
//! [`mako_engine::pid_router::PidRouter`]) and raw PID so the dispatcher can
//! choose the correct adapter and spawn-vs-resume strategy without re-detecting
//! them.
//!
//! - **Spawn** (new process): the tenant receives an initiating message, e.g.
//!   NB receives ORDERS 17115 Sperrauftrag from LF.  Uses `lookup_correlated`
//!   by business key; if no *live* process is found, spawns a fresh one and
//!   registers it.  The index is append-only, so "live" means the matched
//!   process was replayed and still occupies the key — an entry left by a
//!   finished process is retired, not resumed.
//! - **Resume** (continue existing): e.g. NB receives ORDRSP 19118 from MSB.
//!   Uses `lookup_correlated` by business key; returns [`IngestOutcome::Skipped`]
//!   if no process is found (not an error — peer may have sent an orphan message).
//!
//! The correlation key is untyped by design: a MaLo for most GPKE flows, a MeLo
//! for WiM, an invoice reference for INVOIC/REMADV, and the echoed order
//! reference for the LOC-less ORDRSP/ORDCHG answers.
//!
//! ## Combined-role loopback
//!
//! When `BdewAs4Sender` detects `recipient == own_mp_id`, it
//! renders the domain payload to EDIFACT wire bytes, re-parses them, and calls
//! `dispatch` here instead of transmitting over AS4.  This enables zero-latency
//! in-process delivery for Stadtwerke deployments (NB+LF, GNB+gMSB).

use mako_engine::process::RunOptions;
use std::any::Any;
use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _, ReleaseRegistry};
use mako_engine::{
    deadline::Deadline,
    error::EngineError,
    event_store::CorrelationEntry,
    ids::{ProcessId, ProcessIdentity, TenantId},
    process::Process,
    registry::ProcessRegistry as _,
    store_slatedb::{SlateDbSnapshotStore, SlateDbStore},
    types::{MaLo, MarktpartnerCode},
    version::{FormatVersion, WorkflowId},
    workflow::{CommandPayload, Workflow},
};
use mako_fristen::{self as fristen, HolidayCalendar};
use mako_gabi_gas::{
    AllocationCommand, GaBiGasAllocationWorkflow, GaBiGasInvoicWorkflow,
    GaBiGasMehrMindermengenWorkflow, GaBiGasNominationWorkflow, NominationCommand,
};
use mako_geli_gas::{
    GeliGasDatenabrufWorkflow, GeliGasLfAnmeldungWorkflow, GeliGasLfStornierungWorkflow,
    GeliGasMsconsWorkflow, GeliGasPartinWorkflow, GeliGasSperrprozesseInvoicWorkflow,
    GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow, GeliGasStornierungWorkflow,
    GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    GpkeAbrechnungWorkflow, GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeBeendigungZuordnungWorkflow,
    GpkeDatenabrufMsbWorkflow, GpkeDatenabrufWorkflow, GpkeGeschaeftsdatenanfrageWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeLfAbmeldungWorkflow, GpkeLfAnmeldungWorkflow,
    GpkeMesswerteLieferungWorkflow, GpkeNeuanlageWorkflow, GpkePartinWorkflow,
    GpkeSperrungLfWorkflow, GpkeSperrungMsbWorkflow, GpkeSperrungWorkflow, GpkeStornierungWorkflow,
    GpkeSupplierChangeWorkflow, GpkeUtiltsWorkflow,
};
use mako_mabis::{MabisBillingWorkflow, MabisClearinglisteWorkflow};
use mako_wim::{
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimInsrptWorkflow, WimInvoicWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimTechnikAenderungWorkflow,
    WimWeiterverpflichtungWorkflow,
};
use time::OffsetDateTime;

use crate::adapters;

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Outcome of a successful ingest dispatch attempt.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "fields are read via Debug formatting in tracing events"
)]
pub enum IngestOutcome {
    /// A new process was spawned and the initiating command executed.
    Spawned {
        /// Workflow family name (e.g. `"gpke-sperrung"`).
        workflow_name: &'static str,
        /// Newly created process identifier.
        process_id: ProcessId,
    },
    /// An existing process received the continuation command.
    Dispatched {
        /// Workflow family name.
        workflow_name: &'static str,
        /// Identifier of the resumed process.
        process_id: ProcessId,
    },
    /// Dispatch was deliberately skipped — this PID/workflow is not handled
    /// at this role or the process simply does not exist yet (orphan response).
    Skipped {
        /// Workflow family name (best-effort; may be `"unregistered"`).
        workflow_name: &'static str,
        /// Machine-readable skip reason for observability.
        reason: &'static str,
    },
}

/// `Skipped { reason }` value of a message the door answered with its
/// Verarbeitbarkeitsfehlermeldung instead of handing it to its workflow — an
/// AHB-Fehler of a class in [`ABGEWIESENE_KLASSEN`].
pub const SKIP_REFUSED_AT_DOOR: &str = "refused_at_door";

/// The rule-id family of an AHB-Prüfung finding — the unit the door's
/// refusal is switched on in.
///
/// A class is counted first (`makod_inbound_findings_total{class}`) and
/// refused only once its inbound count is explained: no real counterparty
/// message has been through these checks, and a check that misreads a valid
/// message refuses a counterparty falsely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fehlerklasse {
    /// `MIG-…` — the Nachrichtenstruktur and Segmentlayout.
    Mig,
    /// `AHB-…` — the Anwendungsfall's Prüfschablone.
    Ahb,
    /// `FMT-…` — a Formatbedingung `[901]`–`[999]` on a value.
    Fmt,
}

impl Fehlerklasse {
    /// The class of a rule id, `None` for one outside the three families.
    #[must_use]
    pub fn of(rule_id: &str) -> Option<Self> {
        if rule_id.starts_with(edi_energy::profile::validate::FMT_PREFIX) {
            Some(Self::Fmt)
        } else if rule_id.starts_with("AHB-") {
            Some(Self::Ahb)
        } else if rule_id.starts_with("MIG-") {
            Some(Self::Mig)
        } else {
            None
        }
    }

    /// The metric label: `MIG`, `AHB`, `FMT`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mig => "MIG",
            Self::Ahb => "AHB",
            Self::Fmt => "FMT",
        }
    }

    /// The Formatbedingung number of a `FMT-<pid>-<nr>-<tag>-<de>-<n>` rule
    /// id — its last field.
    #[must_use]
    pub fn formatbedingung(rule_id: &str) -> Option<&str> {
        rule_id
            .strip_prefix(edi_energy::profile::validate::FMT_PREFIX)?
            .rsplit('-')
            .next()
            .filter(|n| !n.is_empty())
    }
}

/// The classes the door refuses: none.
///
/// Every class stays counted, not refused, until a release has counted real
/// counterparty traffic and every non-zero count is explained — `FMT` with
/// the severity change of its findings, `MIG` and `AHB` by the same rule.
/// Thirty of the 66 adapter registries never validate, so switching a class
/// on switches their messages to refusals at once. It is a constant rather
/// than an operator setting: APERAK AHB 1.1 settles that an AHB-Fehler is
/// refused — a Geschäftsvorfall short of its Prüfschablone „so ist er
/// abzulehnen" (Kap. 2.1.2, S. 11), and Kap. 2.1.2.1 (S. 12) counts `Z35`
/// „Format nicht eingehalten" among the AHB-Fehler — so the only open question
/// is whether mako's own checks are right, and that is answered once for every
/// deployment.
pub const ABGEWIESENE_KLASSEN: &[Fehlerklasse] = &[];

/// `Skipped { reason }` value used when a workflow name reaches the family
/// router with no arm in its family module.
pub const SKIP_WORKFLOW_NOT_DISPATCHED: &str = "workflow_not_in_dispatch_table";

/// Prefix of every `Skipped { reason }` meaning "the PID reached its workflow
/// but the `match pid` inside that arm has no branch for it".
pub const SKIP_PID_NOT_DISPATCHED_PREFIX: &str = "pid_not_in_";

impl IngestOutcome {
    /// The skip reason when this outcome means **mako** dropped the message,
    /// as opposed to the peer having sent an orphan.
    ///
    /// Two very different things share the `Skipped` variant. `process_not_found`
    /// and `no_correlation_key` are normal traffic: a counterparty answered a
    /// process that has already closed, or sent a message carrying nothing to
    /// correlate on. Recording those would bury the audit trail in noise.
    ///
    /// A `pid_not_in_*` or [`SKIP_WORKFLOW_NOT_DISPATCHED`] reason is the
    /// opposite: the `PidRouter` resolved the PID, so mako told the transport it
    /// handles this message, acknowledged it, and then dropped it. That is a
    /// coverage bug on this side and an acknowledged-but-unprocessed inbound
    /// message under § 147 AO / GoBD — the callers dead-letter it.
    #[must_use]
    pub fn coverage_gap(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Skipped {
                workflow_name,
                reason,
            } if *reason == SKIP_WORKFLOW_NOT_DISPATCHED
                || reason.starts_with(SKIP_PID_NOT_DISPATCHED_PREFIX) =>
            {
                Some((workflow_name, reason))
            }
            _ => None,
        }
    }
}

/// Verdict on whether a process in the given state still occupies its business
/// key — the ingest-side equivalent of
/// [`mako_engine::workflow::OccupiesBusinessKey::occupies_business_key`].
///
/// A parameter rather than a `W::State: OccupiesBusinessKey` bound because only
/// 56 of the 65 workflow names dispatched here supply a verdict at all — 40
/// through that trait, 16 through an inherent terminal-state closure — and the
/// verdict is domain knowledge that belongs in the workflow's own crate — makod
/// only *consumes* what a crate has already published (that trait, or an
/// inherent `is_terminal()`). The four INVOIC families and the Redispatch
/// Aktivierung pass [`occupied_by_own_reference`]. An arm without a verdict would decide on
/// index presence alone; `cargo xtask check-occupancy` counts those arms and
/// refuses any.
type Occupancy<W> = fn(&<W as Workflow>::State) -> bool;

/// What the door knows about one inbound message beyond its content: when it
/// arrived, and the APERAK delivery window that arrival opened.
///
/// Built once per message in [`EdifactIngestDispatcher::dispatch`] and handed
/// to every spawn and resume, so the window is registered whichever of the two
/// the message drives, and every outbox entry the run enqueues reports the
/// receipt instant as its occurrence ([`RunOptions::received_at`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Inbound<'a> {
    /// The instant the door received the message — for AS4 the instant of its
    /// Zustellquittung.
    pub(crate) received_at: OffsetDateTime,
    aperak: Option<AperakWindow>,
    /// The APERAK the door owes the message ([`door_aperak`]) — its
    /// Anerkennungs- or Verarbeitbarkeitsfehlermeldung — enqueued with the run
    /// unless the workflow answers with its own.
    anerkennung: Option<&'a mako_engine::outbox::PendingOutbox>,
}

impl<'a> Inbound<'a> {
    /// A message that APERAK AHB 1.1 does not reach — a DVGW message, a
    /// Redispatch XML document.
    pub(crate) const fn without_aperak(received_at: OffsetDateTime) -> Self {
        Self {
            received_at,
            aperak: None,
            anerkennung: None,
        }
    }

    /// The Anerkennungsmeldung to enqueue with a run of `W` — none when `W`
    /// decides the message's APERAK on a later command.
    fn anerkennung<W: Workflow>(&self) -> Option<&'a mako_engine::outbox::PendingOutbox> {
        self.anerkennung.filter(|_| !W::APERAK_AFTER_RECEIPT)
    }

    /// The APERAK window as deadlines of `process`: those registered with the
    /// run, and those registered only if the run enqueues an APERAK.
    fn aperak_deadlines<W: Workflow, S: mako_engine::event_store::EventStore>(
        &self,
        process: &Process<W, S>,
    ) -> (Vec<Deadline>, Vec<Deadline>) {
        let Some(window) = self.aperak else {
            return (Vec::new(), Vec::new());
        };
        let deadline = Deadline::new(
            process.stream_id().clone(),
            process.process_id(),
            process.identity().tenant_id,
            process.identity().workflow_id.clone(),
            window.label,
            window.due_at,
        );
        if window.only_with_aperak {
            (Vec::new(), vec![deadline])
        } else {
            (vec![deadline], Vec::new())
        }
    }
}

/// The APERAK the door owes a message its workflow does not answer with one
/// of its own.
///
/// An AHB-Fehler (Kap. 2.1.1, S. 10) is rejected with its Verarbeitbarkeitsfehler-
/// meldung in both Sparten — in Gas it is the one result reported (Kap. 2.3,
/// S. 21); a processable Strom message gets its Anerkennungsmeldung; a
/// processable Gas message and a finding the APERAK has no code for get none.
/// Formatbedingung findings count only where `formatbedingungen` says their
/// class is refused.
fn door_aperak(
    msg: &AnyMessage,
    gas: bool,
    formatbedingungen: bool,
) -> Option<mako_engine::outbox::PendingOutbox> {
    use crate::adapters::AhbBefund;
    match crate::adapters::ahb_befund(msg, formatbedingungen) {
        AhbBefund::Fehler(fehler) => {
            let (payload, sender) = fehlermeldung_of(msg, fehler)?;
            Some(mako_engine::outbox::PendingOutbox::typed(sender, &payload))
        }
        AhbBefund::Verarbeitbar if !gas => anerkennung_of(msg),
        AhbBefund::Verarbeitbar | AhbBefund::Ungeklaert => None,
    }
}

/// The Verarbeitbarkeitsfehlermeldung naming `fehler`, and who it goes to:
/// one `SG4 ERC` group per AHB-Fehler, each with its Ortsangabe
/// (APERAK AHB 1.1 Kap. 2.1.1, S. 10: „Es sind alle AHB-Fehler anzugeben";
/// Kap. 2.1.2.2, S. 12). `None` when `fehler` is empty or `msg` names no
/// `NAD+MS` / `NAD+MR` to answer.
fn fehlermeldung_of(
    msg: &AnyMessage,
    fehler: Vec<crate::adapters::AhbFehler>,
) -> Option<(mako_engine::payload::AperakPayload, &str)> {
    let (own, sender) = (msg.nad_receiver()?, msg.nad_sender()?);
    let mut fehler = fehler.into_iter();
    let first = fehler.next()?;
    let mut payload = mako_engine::payload::AperakPayload::fehler(
        own,
        sender,
        msg.message_ref(),
        first.code,
        ftx_text(&first.reason),
    );
    payload.ortsangabe = first.ortsangabe.as_deref().map(ftx_text);
    payload.weitere_fehler = fehler
        .map(|f| mako_engine::payload::AperakFehler {
            error_code: f.code.to_owned(),
            reason: Some(ftx_text(&f.reason)),
            ortsangabe: f.ortsangabe.as_deref().map(ftx_text),
        })
        .collect();
    Some((payload, sender))
}

/// A finding's text as an APERAK `FTX` DE 4440 carries it: `an..512` (APERAK
/// MIG), in the UNOC repertoire the interchange declares — ISO 8859-1, which
/// holds the umlauts but not the typographic quotes and dashes the
/// validator's messages use. Those become their ASCII forms and anything else
/// outside the repertoire `?`, so the Fehlermeldung renders instead of being
/// refused by the writer.
fn ftx_text(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '„' | '“' | '”' | '‚' | '‘' | '’' | '«' | '»' => '"',
            '—' | '–' | '−' => '-',
            '…' => '.',
            c if u32::from(c) < 0x100 && !c.is_control() => c,
            _ => '?',
        })
        .take(512)
        .collect()
}

/// The Anerkennungsmeldung (`BGM+312`, APERAK 29002) a processable Strom
/// message is owed.
///
/// APERAK AHB 1.1 Kap. 2.4, S. 23: „Ist der Geschäftsvorfall verarbeitbar, so wird
/// diese Information über eine Anerkennungsmeldung mitgeteilt", within the
/// window of Kap. 2.4.1. It references the message by `UNH` DE 0062
/// (`SG2 RFF+ACE`), `BGM` DE 1004 (`SG2 RFF+AGO`) and `DTM+137` (`SG2 DTM+171`)
/// — Kap. 3, S. 26. It goes from the message's `NAD+MR` back to its `NAD+MS`;
/// a message naming neither cannot be answered and gets none.
fn anerkennung_of(msg: &AnyMessage) -> Option<mako_engine::outbox::PendingOutbox> {
    let (own, sender) = (msg.nad_receiver()?, msg.nad_sender()?);
    let segs = msg.segments();
    let document_number = segs
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(1, 0))
        .filter(|v| !v.is_empty());
    let document_date = segs
        .iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some("137"))
        .and_then(|s| {
            crate::adapters::parse_dtm_datetime(s.component_str(0, 1)?, s.component_str(0, 2))
        })
        .and_then(|at| {
            at.format(time::macros::format_description!(
                "[year][month][day][hour][minute]"
            ))
            .ok()
        });
    let mut payload =
        mako_engine::payload::AperakPayload::anerkennung(own, sender, msg.message_ref());
    if let Some(number) = document_number {
        payload = payload.referencing(number, document_date);
    }
    Some(mako_engine::outbox::PendingOutbox::typed(sender, &payload))
}

/// Whether a follow-up with Prüfidentifikator `pid` that no process takes is
/// answered `Z33`.
///
/// The PID-Übersicht names, per Prozessschritt, the tuple that assigns a
/// Geschäftsvorfall to its predecessor („Zuordnung zu einem Geschäftsvorfall")
/// and where that assignment is checked („Stelle der Zuordnungsprüfung":
/// `V` the Verarbeitbarkeitsprüfung, `E` the Entscheidungsbaum). APERAK AHB 1.1
/// Kap. 2.1.3 (S. 14): only a `V` assignment „kann und muss" be refused with a
/// Zuordnungsfehler. So `Z33` is owed when the PID names a Geschäftsvorfall
/// tuple and every row that names one marks `V`. Read from the tracked
/// `pid-overview.json` (`cargo xtask import-pid-overview`).
#[must_use]
pub fn z33_owed(pid: u32) -> bool {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::OnceLock;
    #[derive(serde::Deserialize)]
    struct Zuordnung {
        geschaeftsvorfall: bool,
        stelle: BTreeSet<String>,
    }
    #[derive(serde::Deserialize)]
    struct Overview {
        zuordnung: BTreeMap<String, Zuordnung>,
    }
    static OWED: OnceLock<BTreeSet<u32>> = OnceLock::new();
    OWED.get_or_init(|| {
        let overview: Overview = serde_json::from_str(include_str!(
            "../../../../../crates/edi-energy/profiles/pid-overview.json"
        ))
        .expect("the tracked pid-overview.json parses");
        overview
            .zuordnung
            .into_iter()
            .filter(|(_, z)| {
                z.geschaeftsvorfall && z.stelle.iter().all(|s| s == "V") && !z.stelle.is_empty()
            })
            .filter_map(|(pid, _)| pid.parse().ok())
            .collect()
    })
    .contains(&pid)
}

/// The APERAK delivery window one inbound message opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AperakWindow {
    /// An `aperak-` label from `mako_fristen`.
    pub(crate) label: &'static str,
    /// When the APERAK must have gone out.
    pub(crate) due_at: OffsetDateTime,
    /// Owed only when the message is answered with an APERAK at all — a Gas
    /// APERAK reports a Verarbeitbarkeitsfehler and nothing else.
    pub(crate) only_with_aperak: bool,
}

/// The APERAK delivery window an inbound message opens, from its message type,
/// Sparte and Prüfidentifikator.
///
/// * **Strom** (APERAK AHB 1.1 Kap. 2.4.1, S. 24): the result of the
///   Verarbeitbarkeitsprüfung of every Geschäftsvorfall is owed — an
///   Anerkennungsmeldung or a Verarbeitbarkeitsfehlermeldung —
///   „spätestens bis zum nächsten Werktag 12 Uhr"; a UTILMD or an ORDERS
///   „spätestens 45 Minuten nach Eingang", on a Samstag „bis zum Sonntag,
///   12 Uhr". Owed for every message, so the window opens unconditionally.
/// * **Gas** (Kap. 2.3, S. 21; Kap. 2.3.1, S. 22): only a Verarbeitbarkeitsfehler is
///   reported, by the next Werktag 12 Uhr in a Folgeprozess and within
///   3 Werktage in an Initialprozess, chosen by the PID
///   ([`fristen::aperak_gas_due_at`]). The window opens only beside an APERAK
///   the run enqueues: a processable Gas message is answered by silence.
///
/// An APERAK or a CONTRL is never answered with an APERAK, and a message whose
/// type cannot be read opens no window.
pub(crate) fn aperak_window(
    message_type: Option<edi_energy::MessageType>,
    sparte: mako_engine::types::Sparte,
    pid: u32,
    received_at: OffsetDateTime,
) -> Option<AperakWindow> {
    use edi_energy::MessageType;
    use mako_engine::types::Sparte;
    let message_type = message_type?;
    if matches!(message_type, MessageType::Aperak | MessageType::Contrl) {
        return None;
    }
    Some(match sparte {
        Sparte::Strom if matches!(message_type, MessageType::Utilmd | MessageType::Orders) => {
            AperakWindow {
                label: fristen::APERAK_STROM_WINDOW_LABEL,
                due_at: fristen::aperak_strom_due_at(received_at),
                only_with_aperak: false,
            }
        }
        Sparte::Strom => AperakWindow {
            label: fristen::APERAK_STROM_ALLGEMEIN_LABEL,
            due_at: fristen::aperak_strom_allgemein_due_at(received_at),
            only_with_aperak: false,
        },
        Sparte::Gas => {
            let (label, due_at) = fristen::aperak_gas_due_at(pid, received_at);
            AperakWindow {
                label,
                due_at,
                only_with_aperak: true,
            }
        }
    })
}

/// The verdict for a family keyed on the message's own reference — an INVOIC
/// on its `UNH` DE 0062.
///
/// Every process on such a key *is* that message, so a second arrival with the
/// same reference is a duplicate of it: it resumes the existing process, which
/// refuses it, and never opens a second settlement of one invoice.
fn occupied_by_own_reference<S>(_: &S) -> bool {
    true
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

/// Phase 2 ingest execution dispatcher.
///
/// Translates parsed [`AnyMessage`] objects to typed domain commands and
/// executes them on the correct workflow process.  Share across threads via
/// [`Arc`] — all fields are `Clone + Send + Sync`.
#[derive(Clone)]
pub struct EdifactIngestDispatcher {
    store: Arc<SlateDbStore>,
    snap_store: SlateDbSnapshotStore,
    snapshot_interval: u64,
    tenant_id: TenantId,
    /// MP-IDs of counterparties acting as an Energieserviceanbieter.
    ///
    /// marktd client used to gate inbound ESA messages against the consent
    /// registry. `None` disables the gate (dev mode / marktd not configured).
    ///
    /// See [`EdifactIngestDispatcher::with_marktd_client`].
    marktd_client: Option<Arc<mako_markt::marktd_client::MarktdClient>>,
    /// Serialises the lookup→spawn window per business key — see
    /// [`BusinessKeyLocks`].
    ///
    /// `Arc`, not a plain field: the dispatcher is `Clone`, and a clone with its
    /// own lock map would serialise nothing between the two copies.
    key_locks: Arc<BusinessKeyLocks>,
    /// Own MP-IDs and their Sparte — the authoritative Sparte signal for the
    /// AHBs that are Sparte-neutral.
    ///
    /// ORDERS/ORDRSP/REQOTE/QUOTES/IFTSTA/INSRPT carry the same
    /// Prüfidentifikator in both Sparten, and the message body says nothing
    /// about which. What decides it is the **recipient MP-ID** — one of our own
    /// parties, and each covers exactly one Sparte (BDEW Allgemeine
    /// Festlegungen Kap. 2.13). The MP-ID's issuing agency is *not* the
    /// discriminator: a BDEW `99…` code and a DVGW `98…` code both appear under
    /// NAD DE 3055 = 9 or 332 depending on the party, so the registry is the
    /// only sound source.
    ///
    /// `None` leaves the Sparte at [`Sparte::Strom`], which is what a
    /// Strom-only deployment and every existing test are.
    mp_id_registry: Option<Arc<crate::core::party_registry::MpIdRegistry>>,
    /// `[policy.ingest] unbekannter_vorgang` — what an orphan follow-up is
    /// answered with ([`Self::answer_orphan`]).
    unbekannter_vorgang: crate::core::config::UnbekannterVorgang,
    /// The AHB-Fehler classes answered at the door instead of reaching their
    /// workflow — [`ABGEWIESENE_KLASSEN`] unless a test says otherwise.
    abgewiesene_klassen: Vec<Fehlerklasse>,
}

/// Per-business-key mutexes guarding the lookup→spawn critical section.
///
/// # Why this exists
///
/// Spawning is a check-then-act: `lookup_correlated` finds no live process, so
/// one is created. Two initiating messages for the same business key arriving
/// concurrently both pass the check and both spawn. The damage surfaces later —
/// every follow-up resolves the key to two processes and fails with
/// `AmbiguousProcess`, and the duplicate runs its own Fristen to expiry.
///
/// AS4 inbox deduplication does not cover this: it suppresses identical
/// retransmits of one message, whereas this needs two *distinct* messages
/// (an ORDERS and an ORDCHG for one MaLo, say) landing together.
///
/// An in-process lock is sufficient because `makod` is a single writer by
/// construction — the exclusive data-directory lock refuses a second instance.
#[derive(Default)]
pub(crate) struct BusinessKeyLocks {
    inner: std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl BusinessKeyLocks {
    /// Acquire the lock for `key`, waiting for any concurrent spawn to finish.
    ///
    /// Entries for keys nobody is holding are dropped on each acquisition, so
    /// the map tracks in-flight spawns rather than every key ever seen.
    pub(crate) async fn acquire(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut map = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.retain(|_, l| Arc::strong_count(l) > 1);
            Arc::clone(
                map.entry(key.to_owned())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        lock.lock_owned().await
    }
}

/// The process's one [`BusinessKeyLocks`], shared by every door that spawns.
///
/// A spawn on the ingest path and one on the ERP command door check the same
/// correlation index for the same business key, so they have to serialise on
/// the same lock: two maps would each let one spawn through, and the business
/// key would end up with two processes.
pub(crate) fn business_key_locks() -> Arc<BusinessKeyLocks> {
    static LOCKS: std::sync::OnceLock<Arc<BusinessKeyLocks>> = std::sync::OnceLock::new();
    Arc::clone(LOCKS.get_or_init(|| Arc::new(BusinessKeyLocks::default())))
}

impl EdifactIngestDispatcher {
    /// All workflow names that have a dispatch arm in [`Self::dispatch`].
    ///
    /// Used by `startup::validate_dispatch_completeness` to verify at startup
    /// that every workflow name registered in the `PidRouter` has a matching
    /// arm here. When a new workflow is added to a domain crate's
    /// `register_pids`, add its name here AND add the corresponding `match`
    /// arm in `dispatch` below.
    pub const KNOWN_WORKFLOW_NAMES: &'static [&'static str] = &[
        mako_emob::EmobAbmeldungWorkflow::WORKFLOW_NAME,
        mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME,
        mako_emob::EmobZuordnungsendeWorkflow::WORKFLOW_NAME,
        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        "gabi-gas-allocation",
        "gabi-gas-invoic",
        "gabi-gas-mehr-mindermengen",
        "gabi-gas-mmma",
        "gabi-gas-nomination",
        "geli-gas-datenabruf",
        mako_geli_gas::geschaeftsdatenanfrage::WORKFLOW_NAME,
        "geli-gas-lf-anmeldung",
        "geli-gas-mscons",
        "geli-gas-partin",
        "geli-gas-sperrprozesse-invoic",
        "geli-gas-sperrung-lf",
        "geli-gas-sperrung-msb",
        "geli-gas-sperrung-nb",
        "geli-gas-stammdatenaenderung",
        "geli-gas-stornierung",
        "geli-gas-stornierung-lf",
        "geli-gas-supplier-change",
        "geli-gas-zuordnungsmeldung",
        "gpke-abrechnung",
        "gpke-abrechnungsdaten",
        "gpke-allokationsliste",
        "gpke-anfrage-bestellung",
        "gpke-ankuendigung-zuordnung-lf",
        "gpke-beendigung-zuordnung",
        "gpke-datenabruf",
        "gpke-datenabruf-msb",
        "gpke-eog",
        "gpke-geschaeftsdatenanfrage",
        "gpke-konfiguration",
        "gpke-konfiguration-aenderung",
        "gpke-kuendigung",
        "gpke-lf-abmeldung",
        "gpke-lf-anmeldung",
        "gpke-messwerte",
        "gpke-nb-lieferende",
        "gpke-neuanlage",
        "gpke-partin",
        "gpke-sperrung",
        "gpke-sperrung-lf",
        "gpke-sperrung-msb",
        "gpke-stammdatenaenderung",
        "gpke-stornierung",
        "gpke-supplier-change",
        "gpke-utilts",
        "gpke-zuordnungsmeldung",
        "mabis-anforderung",
        "mabis-billing",
        "mabis-clearingliste",
        "mabis-listenabgleich",
        "mabis-profile",
        "mabis-zp-lifecycle",
        "redispatch-aktivierung",
        "wim-device-change",
        "wim-ersteinbau",
        "wim-geraeteubernahme",
        "wim-geraeteubernahme-msbn",
        "wim-insrpt",
        "wim-invoic",
        "wim-preisanfrage",
        "wim-preisliste",
        "wim-rechnungsabwicklung",
        "wim-technik-aenderung",
        mako_wim::weiterverpflichtung::WORKFLOW_NAME,
        mako_wim::wertebestellung::WORKFLOW_NAME,
    ];

    /// Wire the marktd consent-registry gate for inbound ESA messages.
    ///
    /// With a client set, an ESA Werteanfrage (REQOTE 35003) or Bestellung
    /// (ORDERS 17007) is checked against the registry before the workflow runs.
    /// A revoked consent or an unestablished framework agreement is answered
    /// with an Ablehnung (the clearing case) rather than being processed.
    /// Without a client the gate is off and every ESA message proceeds.
    #[must_use]
    pub fn with_marktd_client(
        mut self,
        client: Option<Arc<mako_markt::marktd_client::MarktdClient>>,
    ) -> Self {
        self.marktd_client = client;
        self
    }

    /// Refuse AHB-Fehler of `klassen` at the door, in place of
    /// [`ABGEWIESENE_KLASSEN`] — how a class's refusal is exercised before
    /// its inbound count lets it into the constant.
    #[must_use]
    pub fn with_abgewiesene_klassen(mut self, klassen: &[Fehlerklasse]) -> Self {
        self.abgewiesene_klassen = klassen.to_vec();
        self
    }

    fn weist_ab(&self, klasse: Fehlerklasse) -> bool {
        self.abgewiesene_klassen.contains(&klasse)
    }

    /// The Verarbeitbarkeitsfehlermeldung for `msg` when it carries an
    /// AHB-Fehler of a refused class — the message is then answered at the
    /// door and handed to no workflow, whether or not its adapter would have
    /// validated it. Every AHB-Fehler of the message is named, refused class
    /// or not (Kap. 2.1.1, S. 10). `None` for an APERAK or a CONTRL, which no
    /// APERAK answers (Kap. 2, S. 9), and for a message with nothing refused.
    fn door_refusal(&self, msg: &AnyMessage) -> Option<mako_engine::outbox::OutboxMessage> {
        use crate::adapters::AhbBefund;
        if self.abgewiesene_klassen.is_empty()
            || matches!(
                msg.try_message_type(),
                Some(edi_energy::MessageType::Aperak | edi_energy::MessageType::Contrl) | None
            )
        {
            return None;
        }
        let AhbBefund::Fehler(fehler) =
            crate::adapters::ahb_befund(msg, self.weist_ab(Fehlerklasse::Fmt))
        else {
            return None;
        };
        if !fehler.iter().any(|f| self.weist_ab(f.klasse)) {
            return None;
        }
        let (payload, sender) = fehlermeldung_of(msg, fehler)?;
        Some(crate::edifact_api::aperak_ohne_prozess(
            &payload,
            sender,
            self.tenant_id,
        ))
    }

    /// Answer an orphan follow-up as `[policy.ingest] unbekannter_vorgang`
    /// says — `Z33` unless the operator holds them for a person.
    #[must_use]
    pub fn with_unbekannter_vorgang(
        mut self,
        choice: crate::core::config::UnbekannterVorgang,
    ) -> Self {
        self.unbekannter_vorgang = choice;
        self
    }

    /// Wire the own-party registry that resolves an interchange's Sparte.
    ///
    /// Without it every interchange is treated as Strom — correct for a
    /// Strom-only deployment, and wrong for a Gas one, which would answer a
    /// WiM Gas ORDERS out of the Strom Entscheidungsbaum and name an `S_00xx`
    /// Codeliste the Gas market does not publish.
    #[must_use]
    pub fn with_mp_id_registry(
        mut self,
        registry: Arc<crate::core::party_registry::MpIdRegistry>,
    ) -> Self {
        self.mp_id_registry = Some(registry);
        self
    }

    /// The Sparte of an inbound interchange, from the recipient MP-ID.
    ///
    /// `NAD+MR` is guaranteed equal to `UNB` DE 0010 — `edi-energy` enforces
    /// the Kap. 2.13 party-identity rule at parse — so reading the recipient off
    /// the message is reading the interchange's own addressee.
    pub(super) fn sparte_of(&self, msg: &AnyMessage) -> mako_engine::types::Sparte {
        use crate::core::party_registry::RoleSparte;
        use mako_engine::types::Sparte;
        let Some(reg) = self.mp_id_registry.as_ref() else {
            return Sparte::Strom;
        };
        match msg.nad_receiver().and_then(|mp| reg.sparte_of(mp)) {
            Some(RoleSparte::Gas) => Sparte::Gas,
            // `Both` is a Sparte-neutral own party (a Marktpartner registered
            // for both commodities). Strom is the safe reading: it is the
            // stricter Frist on every window the two share.
            Some(RoleSparte::Strom | RoleSparte::Both) | None => Sparte::Strom,
        }
    }

    /// The Sparte whose APERAK rules a message falls under.
    ///
    /// The GeLi Gas and GaBi Gas families are Gas by Festlegung; WiM and the
    /// ESA Wertebestellung serve both Sparten, so their recipient decides
    /// ([`Self::sparte_of`]); every other family is Strom.
    fn aperak_sparte(&self, msg: &AnyMessage, workflow_name: &str) -> mako_engine::types::Sparte {
        use mako_engine::types::Sparte;
        if workflow_name.starts_with("geli-gas-") || workflow_name.starts_with("gabi-gas-") {
            Sparte::Gas
        } else if workflow_name.starts_with("wim-")
            || workflow_name == mako_wim::esa_wertebestellung::WORKFLOW_NAME
        {
            self.sparte_of(msg)
        } else {
            Sparte::Strom
        }
    }

    /// Which Marktrolle of ours a message is addressed to, for the WiM windows
    /// that branch on it.
    ///
    /// PID 31009 carries **three** Use-Cases with three different answer
    /// windows, and the message body says which one nowhere: the NB answers by
    /// the 4. WT before the Zahlungsziel (WiM Teil 1 Kap. 6.2 Nr. 2), the
    /// **ESA** by the 4. WT before it too (WiM Teil 2 Kap. 4.5.2 Nr. 2), and
    /// the LF *zum* Zahlungsziel (Kap. 3.6.3.8.2). BDEW Allgemeine
    /// Festlegungen Kap. 2.13 gives every Marktrolle its own MP-ID, so the
    /// interchange recipient is what identifies it.
    ///
    /// Anything that matches none of our role codes falls to the LF/MSB arm,
    /// which is the longest window — a misread there answers late, never early.
    /// The ESA arm is *not* that fallback: it is the shortest of the three, and
    /// reaching it by guesswork would report a breach that has not happened.
    pub(super) fn rechnung_empfaenger(
        &self,
        msg: &AnyMessage,
    ) -> mako_fristen::vorlauf::RechnungEmpfaenger {
        use mako_fristen::vorlauf::RechnungEmpfaenger;
        let Some(reg) = self.mp_id_registry.as_ref() else {
            return RechnungEmpfaenger::LieferantOderMsb;
        };
        let Some(empfaenger) = msg.nad_receiver() else {
            return RechnungEmpfaenger::LieferantOderMsb;
        };
        // `IMD+7081` = `KON` („Abrechnung von Konfigurationen
        // (Universalbestellprozess)") is the ESA Use-Case stated on the wire —
        // INVOIC AHB 1.0b, and the strongest evidence there is. It wins over
        // the MP-ID because a combined deployment may hold several roles while
        // the message says which one this invoice is for.
        if invoic_rechnungstyp(msg).as_deref() == Some("KON") {
            return RechnungEmpfaenger::Esa;
        }
        if ["NB", "GNB"]
            .iter()
            .any(|role| reg.mp_id_for_role(role) == Some(empfaenger))
        {
            RechnungEmpfaenger::Netzbetreiber
        } else if reg.mp_id_for_role("ESA") == Some(empfaenger) {
            RechnungEmpfaenger::Esa
        } else {
            RechnungEmpfaenger::LieferantOderMsb
        }
    }

    /// Gate an inbound ESA `WertebestellungCommand` against the consent registry.
    ///
    /// Thin boundary wrapper: extracts the ESA/MSB/location identifiers from the
    /// wire message and delegates the fail-open policy to
    /// [`mako_wim::consent::gate_inbound`]. With no marktd client configured the
    /// command passes through unchanged (the gate is defence-in-depth; the
    /// durable stop signal remains the 17008 Abbestellung fired on revocation).
    /// Gate an inbound ESA order against the consent registry.
    ///
    /// `location` must be supplied for a Bestellung: only the REQOTE carries a
    /// `LOC`, so extracting one from an ORDERS finds nothing and the check
    /// would pass on an empty key. The caller reads it from the running
    /// process ([`Self::esa_location_of`]).
    async fn gate_esa_consent(
        &self,
        msg: &AnyMessage,
        cmd: mako_wim::wertebestellung::WertebestellungCommand,
        location: Option<&str>,
    ) -> mako_wim::wertebestellung::WertebestellungCommand {
        let Some(marktd) = &self.marktd_client else {
            return cmd;
        };
        let esa = extract_sender_mp_id(msg);
        let msb = extract_receiver_mp_id(msg);
        let location =
            location.map_or_else(|| extract_malo_from_msg(msg), mako_engine::types::MaLo::new);
        mako_wim::consent::gate_inbound(
            cmd,
            &esa,
            &msb,
            &location,
            &MarktdConsentGate { client: marktd },
        )
        .await
    }

    /// The location an ESA-Wertebestellung process is running for, found by
    /// one of its correlation keys.
    ///
    /// The consent registry is keyed on locations, and every message after the
    /// opening REQOTE carries only a Belegnummer — so a re-check between the
    /// Angebot and the Bestellung has to read the location back off the
    /// process. `None` when no process matches, which is the orphan case the
    /// caller skips anyway.
    async fn esa_location_of(&self, key: &str) -> Option<String> {
        if key.is_empty() {
            return None;
        }
        let identity = self
            .store
            .as_process_registry()
            .lookup_correlated(self.tenant_id, key)
            .await
            .ok()?
            .into_iter()
            .find(|id| id.workflow_id.name.as_ref() == mako_wim::wertebestellung::WORKFLOW_NAME)?;
        let process = Process::<
            mako_wim::wertebestellung::WimWertebestellungWorkflow,
            Arc<SlateDbStore>,
        >::from_identity(Arc::clone(&self.store), identity);
        let state = process.state().await.ok()?;
        state.data().map(|d| d.lokations_id.clone())
    }

    /// File the prices of a just-accepted Angebot where `invoicd` looks.
    ///
    /// An ORDRSP 19011 answering a Bestellung is the moment the MSB's offer
    /// becomes the **price agreement**, and it is the only price basis an ESA
    /// has: `PreisblattMessung` is what an MSB publishes toward the NB and the
    /// LF, and there is no published sheet for a Kapitel-4.6 Messprodukt
    /// because §35 MsbG leaves the Entgelt for a Zusatzleistung to be agreed
    /// per request. Without this, an ESA's INVOIC 31009 is checked for
    /// arithmetic and totals and nothing else.
    ///
    /// **Best-effort.** A marktd outage must not fail a confirmed
    /// subscription: the values are authorised either way, and the missing
    /// basis is a warning on the later invoice check rather than a dispute.
    async fn record_accepted_angebot(&self, key: &str) {
        let Some(marktd) = self.marktd_client.as_ref() else {
            return;
        };
        if key.is_empty() {
            return;
        }
        let Ok(identities) = self
            .store
            .as_process_registry()
            .lookup_correlated(self.tenant_id, key)
            .await
        else {
            return;
        };
        let Some(identity) = identities.into_iter().find(|id| {
            id.workflow_id.name.as_ref() == mako_wim::esa_wertebestellung::WORKFLOW_NAME
        }) else {
            return;
        };
        let process = Process::<
            mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow,
            Arc<SlateDbStore>,
        >::from_identity(Arc::clone(&self.store), identity);
        let Ok(state) = process.state().await else {
            return;
        };
        let Some(data) = state.data() else { return };
        if data.angebot.preise.is_empty() {
            // A confirmed subscription whose offer priced nothing is either a
            // counterparty that omitted a Muss (`SG31 PRI`) or an offer mako
            // read before it modelled prices. Say so once rather than filing an
            // empty agreement, which would read as "we agreed to nothing".
            tracing::warn!(
                esa = %data.esa.as_str(), msb = %data.msb.as_str(),
                location = %data.lokations_id,
                "makod: Bestellung confirmed but the Angebot priced nothing — the ESA has no \
                 basis to check the MSB's INVOIC 31009 against"
            );
            return;
        }
        let body = serde_json::json!({
            "lokations_id": data.lokations_id,
            "messprodukt": data.gegenstand.messprodukt,
            "bestellung_ref": data.bestellung_ref,
            "waehrung": data.angebot.waehrung.as_deref().unwrap_or("EUR"),
            "valid_from": data.gegenstand.wunschtermin.to_string(),
            "valid_to": data.gegenstand.zeitraum_bis.map(|d| d.to_string()),
            "preise": data
                .angebot
                .preise
                .iter()
                .map(|p| serde_json::json!({
                    "artikel_id": p.artikel_id,
                    "preistyp": p.preistyp.pri_code(),
                    "betrag": p.betrag,
                    "einheit": p.einheit,
                }))
                .collect::<Vec<_>>(),
        });
        if let Err(e) = marktd
            .put_esa_preise(data.msb.as_str(), data.esa.as_str(), &body)
            .await
        {
            tracing::warn!(
                error = %e, esa = %data.esa.as_str(), msb = %data.msb.as_str(),
                "makod: could not file the accepted Angebot at marktd — the INVOIC 31009 check \
                 will report a missing price basis"
            );
        }
    }

    /// Construct a new dispatcher backed by the given stores.
    #[must_use]
    pub fn new(
        store: Arc<SlateDbStore>,
        snap_store: SlateDbSnapshotStore,
        snapshot_interval: u64,
        tenant_id: TenantId,
    ) -> Self {
        Self {
            store,
            snap_store,
            snapshot_interval,
            tenant_id,
            marktd_client: None,
            key_locks: business_key_locks(),
            mp_id_registry: None,
            unbekannter_vorgang: crate::core::config::UnbekannterVorgang::Z33,
            abgewiesene_klassen: ABGEWIESENE_KLASSEN.to_vec(),
        }
    }

    /// Dispatch `msg` to the appropriate workflow process.
    ///
    /// `workflow_name` must be pre-computed by the caller via `PidRouter::route`.
    /// `pid` is the raw Prüfidentifikator value already extracted from the UNH.
    ///
    /// Returns [`IngestOutcome::Skipped`] (not `Err`) when this PID/workflow
    /// combination is not in the current dispatch table, or when no process is
    /// found for a response message.  Returns `Err` only on storage or adapter
    /// failures.
    ///
    /// `received_at` is the instant the door received the message — for AS4
    /// the instant it answered with its Zustellquittung. Every arrival-anchored
    /// window (the Antwortfrist and the APERAK window) is resolved against it,
    /// never against the dispatch instant: the Frist runs from the ÜT, the
    /// „Tag des Empfangs der Übertragungsdatei" (GPKE Teil 1 Kap. 7
    /// „Fristenberechnung", S. 27), and a message dispatched after midnight
    /// was still received the day before.
    pub async fn dispatch(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
        received_at: OffsetDateTime,
    ) -> Result<IngestOutcome, EngineError> {
        // Correlation-routing override (conversation-ID routing). A REMADV/COMDIS
        // reply PID (33001–33004 / 29001–29002) is legitimately claimed by several
        // *same-Sparte* billing families — GPKE and WiM both register them for Strom
        // — so `resolve_workflow`'s MP-ID→Sparte narrowing cannot separate them and
        // the static PID router falls back to a last-write-wins guess. Re-resolve to
        // the family that actually holds an open process for the referenced invoice
        // (RFF+Z13 → original INVOIC message-ref, the same key the family's
        // `resume_by_key` uses downstream). If none is found, the static route
        // stands — behaviour is unchanged for correctly-routed and orphan messages.
        let corrected = self.correlation_route(msg, workflow_name).await;
        let effective_name = corrected.as_deref().unwrap_or(workflow_name);
        let outcome = if let Some(answer) = self.door_refusal(msg) {
            self.refuse_at_door(msg, effective_name, &answer).await
        } else {
            let aperak = aperak_window(
                msg.try_message_type(),
                self.aperak_sparte(msg, effective_name),
                pid,
                received_at,
            );
            let anerkennung = aperak.and_then(|w| {
                door_aperak(msg, w.only_with_aperak, self.weist_ab(Fehlerklasse::Fmt))
            });
            let inbound = Inbound {
                received_at,
                aperak,
                anerkennung: anerkennung.as_ref(),
            };
            self.dispatch_inner(msg, effective_name, pid, inbound).await
        };
        let outcome = match outcome {
            Ok(
                skipped @ IngestOutcome::Skipped {
                    reason: "process_not_found",
                    ..
                },
            ) => self
                .answer_orphan(msg, effective_name, pid)
                .await
                .map(|()| skipped),
            other => other,
        };
        let result = match &outcome {
            Ok(IngestOutcome::Spawned { .. } | IngestOutcome::Dispatched { .. }) => "dispatched",
            Ok(IngestOutcome::Skipped { .. }) => "skipped",
            Err(_) => "error",
        };
        mako_engine::metrics::EngineMetrics::global().inbound_received(pid, result);
        // A `pid_not_in_*` skip means a PID reached its workflow (it is registered
        // in the router) but the ingest `match pid` arm has no branch for it — the
        // message is silently dropped. That is always a coverage bug, so make it
        // LOUD (distinct from expected orphans like `process_not_found` /
        // `no_correlation_key`, which stay quiet). This is the runtime safety net for the
        // registered-but-not-dispatched class.
        if let Ok(o) = &outcome
            && let Some((workflow_name, reason)) = o.coverage_gap()
        {
            tracing::warn!(
                pid,
                workflow = %workflow_name,
                reason,
                "ingest: PID is registered to this workflow but has no dispatch arm — \
                 inbound message dropped (coverage bug); the caller dead-letters it"
            );
        }
        outcome
    }

    /// Answer `msg` with its Verarbeitbarkeitsfehlermeldung and nothing else:
    /// the one write the door makes for a message it refuses, as for an
    /// orphan's `Z33`.
    async fn refuse_at_door(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        answer: &mako_engine::outbox::OutboxMessage,
    ) -> Result<IngestOutcome, EngineError> {
        use mako_engine::outbox::OutboxStore as _;
        self.store.enqueue(std::slice::from_ref(answer)).await?;
        tracing::warn!(
            workflow = %workflow_name,
            message_ref = %msg.message_ref(),
            error_code = answer.payload["error_code"].as_str().unwrap_or("-"),
            "ingest: AHB-Fehler of a refused class — answered at the door, not dispatched",
        );
        Ok(IngestOutcome::Skipped {
            workflow_name: Self::KNOWN_WORKFLOW_NAMES
                .iter()
                .copied()
                .find(|n| *n == workflow_name)
                .unwrap_or("unregistered"),
            reason: SKIP_REFUSED_AT_DOOR,
        })
    }

    /// Answer a follow-up no process takes with `Z33` „Referenziertes
    /// Geschäftsvorfall-Tupel nicht vorhanden" (APERAK AHB 1.1 Kap. 4.3, S. 43)
    /// — where the PID-Übersicht puts its assignment into the
    /// Verarbeitbarkeitsprüfung ([`z33_owed`]). In both Sparten: a
    /// Verarbeitbarkeitsfehler is the one result Gas reports too (Kap. 2.3).
    ///
    /// No process exists to carry it, so the answer is the one write the
    /// door makes for the message.
    ///
    /// Under `[policy.ingest] unbekannter_vorgang = "zur_pruefung"` — a
    /// cutover, or a second system the initiating messages leave through —
    /// the message is recorded as a dead letter for a person instead, and no
    /// APERAK goes out.
    async fn answer_orphan(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<(), EngineError> {
        use mako_engine::dead_letter::DeadLetterSink as _;
        use mako_engine::outbox::OutboxStore as _;
        if !z33_owed(pid) {
            return Ok(());
        }
        if self.unbekannter_vorgang == crate::core::config::UnbekannterVorgang::ZurPruefung {
            let Some(p) = mako_engine::ids::Pid::from_u32(pid) else {
                return Ok(());
            };
            return self
                .store
                .as_dead_letter_sink(self.tenant_id)
                .reject(
                    &mako_engine::dead_letter::DeadLetterReason::NotDispatchable {
                        workflow_name: workflow_name.to_owned(),
                        pid: p,
                        reason: "process_not_found — held for review \
                             ([policy.ingest] unbekannter_vorgang = zur_pruefung)"
                            .to_owned(),
                        context: mako_engine::dead_letter::AuditContext::now()
                            .with_pid(p)
                            .with_message_ref(msg.message_ref())
                            .with_tenant_id(self.tenant_id.to_string()),
                    },
                )
                .await;
        }
        let Some(answer) = crate::edifact_api::fehlermeldung_ohne_prozess(
            msg,
            mako_engine::erc::codes::Z33,
            "der referenzierte Geschäftsvorfall liegt beim Empfänger nicht vor".to_owned(),
            self.tenant_id,
        ) else {
            return Ok(());
        };
        self.store.enqueue(std::slice::from_ref(&answer)).await
    }

    /// Execute a **DVGW** message against its workflow.
    ///
    /// The counterpart of [`dispatch`](Self::dispatch) for the gas transport
    /// family. A DVGW message is not an [`AnyMessage`] and never can be — the
    /// two families share only the `PidRouter`, because DVGW allocates
    /// 70000–79999 and BDEW does not — so the caller sniffs the interchange
    /// (`dvgw_edi::sniff`), parses with the right library, and calls the
    /// matching entry point here.
    ///
    /// There is no correlation-routing override: that exists to separate BDEW
    /// reply PIDs claimed by several same-Sparte billing families, and DVGW has
    /// no such collision — every code maps to exactly one workflow.
    pub async fn dispatch_dvgw(
        &self,
        msg: &dvgw_edi::DvgwMessage,
        workflow_name: &str,
        pid: u32,
        received_at: OffsetDateTime,
    ) -> Result<IngestOutcome, EngineError> {
        // A DVGW message is not a BDEW EDIFACT message: APERAK AHB 1.1 does not
        // reach it, so no APERAK window opens.
        let outcome = self
            .dispatch_gabi_gas_dvgw(
                msg,
                workflow_name,
                pid,
                Inbound::without_aperak(received_at),
            )
            .await;
        let result = match &outcome {
            Ok(IngestOutcome::Spawned { .. } | IngestOutcome::Dispatched { .. }) => "dispatched",
            Ok(IngestOutcome::Skipped { .. }) => "skipped",
            Err(_) => "error",
        };
        mako_engine::metrics::EngineMetrics::global().inbound_received(pid, result);
        if let Ok(o) = &outcome
            && let Some((workflow_name, reason)) = o.coverage_gap()
        {
            tracing::warn!(
                pid,
                workflow = %workflow_name,
                reason,
                "ingest: PID is registered to this workflow but has no dispatch arm — \
                 inbound message dropped (coverage bug); the caller dead-letters it"
            );
        }
        outcome
    }

    /// Conversation-ID routing for reply messages whose PID is shared across
    /// same-Sparte billing families (REMADV / COMDIS).
    ///
    /// Returns the workflow name of the open process registered under the reply's
    /// referenced invoice (RFF+Z13), when that differs from the statically resolved
    /// `static_name`. Returns `None` — leaving the static route in force — when the
    /// message is not a correlation-routed reply, carries no reference, or has no
    /// correlated open process (an orphan reply is still `Skipped` downstream, as
    /// before). This never mis-books: it only redirects a reply to the family that
    /// already owns the invoice it answers.
    ///
    /// **INVOIC 31004 needs no override yet.** The Stornorechnung is
    /// Sparte-neutral (INVOIC AHB Kap. 3.1.2) and cancels an invoice from any
    /// billing family, so it looks like the same collision — but today it is
    /// single-owner: only `mako_wim::invoic` registers it (`WIM_INVOIC_PIDS`),
    /// and `wim-invoic` is the one arm that dispatches it. An override becomes
    /// necessary the moment a second domain crate owns its own invoices' Storno,
    /// because the static route would then send half the Stornos to the wrong
    /// family.
    async fn correlation_route(&self, msg: &AnyMessage, static_name: &str) -> Option<String> {
        let key = match msg {
            AnyMessage::Remadv(_) => extract_invoice_ref_from_remadv(msg),
            AnyMessage::Comdis(_) => extract_invoice_ref_from_comdis(msg),
            // Not a shared reply PID — the static route is authoritative.
            _ => return None,
        };
        if key.is_empty() {
            return None;
        }
        let registry = self.store.as_process_registry();
        let identities = registry
            .lookup_correlated(self.tenant_id, &key)
            .await
            .ok()?;
        // The invoice ref keys exactly one billing process (the invoicer that sent
        // it); route the reply to that family.
        let owner = identities.first()?;
        let name = owner.workflow_id.name.as_ref();
        (name != static_name).then(|| name.to_owned())
    }

    /// Family router — pure routing by workflow-family name prefix.
    ///
    /// Each family's dispatch arms live in the correspondingly named submodule
    /// (`gpke`, `geli_gas`, `wim`, `gabi_gas`, `mabis`, `emob`,
    /// `redispatch`); the per-family method re-matches on `workflow_name`.
    async fn dispatch_inner(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
        inbound: Inbound<'_>,
    ) -> Result<IngestOutcome, EngineError> {
        match workflow_name {
            n if n.starts_with("geli-gas-") => self.dispatch_geli_gas(msg, n, pid, inbound).await,
            n if n.starts_with("gabi-gas-") => self.dispatch_gabi_gas(msg, n, pid, inbound).await,
            n if n.starts_with("gpke-") => self.dispatch_gpke(msg, n, pid, inbound).await,
            // ESA-side Wertebestellung is WiM Strom Teil 2 — its arm lives in
            // the `wim` submodule next to the MSB-side half of the handshake.
            n if n == mako_wim::esa_wertebestellung::WORKFLOW_NAME => {
                self.dispatch_wim(msg, n, pid, inbound).await
            }
            n if n.starts_with("wim-") => self.dispatch_wim(msg, n, pid, inbound).await,
            n if n.starts_with("mabis-") => self.dispatch_mabis(msg, n, pid, inbound).await,
            n if n.starts_with("emob-") => self.dispatch_emob(msg, n, pid, inbound).await,
            n if n.starts_with("redispatch-") => {
                self.dispatch_redispatch(msg, n, pid, inbound).await
            }
            // ── Any other workflow has no ingest arm ──────────────────────────
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }

    // ── Redispatch XML entry points ───────────────────────────────────────────

    /// Spawn or resume a Redispatch workflow from the AS4 **XML** leg.
    ///
    /// Same machinery as the EDIFACT path, but the version key is the BDEW
    /// Redispatch XSD release (not a MIG `FormatVersion` detected from the
    /// interchange — XML documents carry their version in the namespace).
    /// `spawn_deadlines` are registered atomically with the first events.
    pub(crate) async fn spawn_or_resume_redispatch<W>(
        &self,
        received_at: OffsetDateTime,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        spawn_deadlines: &[(&'static str, time::OffsetDateTime)],
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
        Arc<SlateDbStore>: mako_engine::event_store::AtomicAppend,
    {
        // BDEW Redispatch 2.0 XSD release 1.1f (Fehlerkorrektur 2026-02-19).
        let fv = FormatVersion::new("FV2026-02-19");
        // An XML document is acknowledged by an AcknowledgementDocument, not
        // an APERAK, so no APERAK window opens.
        let inbound = Inbound::without_aperak(received_at);
        // Keyed on the document's own mRID: every process on the key is that
        // document, so a repeat resumes it rather than opening a second one.
        self.spawn_or_resume_guarded::<W>(
            inbound,
            key,
            workflow_name_static,
            cmd,
            &fv,
            spawn_deadlines,
            occupied_by_own_reference,
        )
        .await
    }

    /// Resume an existing Redispatch process by business key (no spawn).
    ///
    /// Used for correlation-routed documents (`AcknowledgementDocument`):
    /// the key is the MRID of the document being acknowledged, under which
    /// the target process was registered at spawn.
    pub(crate) async fn resume_redispatch<W>(
        &self,
        received_at: OffsetDateTime,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        self.resume_by_key::<W>(
            Inbound::without_aperak(received_at),
            key,
            workflow_name_static,
            cmd,
        )
        .await
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// [`spawn_or_resume_keyed`](Self::spawn_or_resume_keyed) with an occupancy verdict.
    ///
    /// Pass this wherever the message is *initiating* and the workflow's own
    /// crate publishes a terminal-state verdict — either
    /// [`mako_engine::workflow::OccupiesBusinessKey`] or an inherent
    /// `is_terminal()`. See [`Occupancy`] for why it is a parameter rather than
    /// a trait bound.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
    )]
    async fn spawn_or_resume_guarded<W>(
        &self,
        inbound: Inbound<'_>,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        fv: &FormatVersion,
        spawn_deadlines: &[(&'static str, time::OffsetDateTime)],
        occupies: Occupancy<W>,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
        Arc<SlateDbStore>: mako_engine::event_store::AtomicAppend,
    {
        self.spawn_or_resume_keyed::<W>(
            inbound,
            key,
            workflow_name_static,
            cmd,
            fv,
            spawn_deadlines,
            &[],
            Some(occupies),
        )
        .await
    }

    /// Look up an existing process by business key and workflow name.
    ///
    /// If a matching process exists, execute `cmd` on it and return
    /// [`IngestOutcome::Dispatched`].  Otherwise spawn a new process, execute
    /// `cmd`, register the process under `key`, and return
    /// [`IngestOutcome::Spawned`].
    ///
    /// `key` is whatever business key identifies the process for this message
    /// family — a MaLo for most GPKE flows, but a MeLo for WiM, an invoice
    /// reference for INVOIC/REMADV, or an order reference for the LOC-less
    /// ORDRSP/ORDCHG answers. The correlation index is untyped by design.
    ///
    /// `spawn_deadlines`: zero or more business `(label, due_at)` pairs to
    /// register atomically with the events in a single `WriteBatch` via
    /// [`Process::run`] with [`RunOptions::deadlines`].  Pass `&[]` for
    /// workflows that have no deadlines (e.g. pure continuation handlers).
    /// They are only registered for freshly-spawned processes — resuming an
    /// existing process must not re-register (they were set at spawn).
    ///
    /// The APERAK delivery window is not one of them: `inbound` carries it
    /// ([`aperak_window`]), and it is registered on a spawn and on a resume
    /// alike, because the obligation belongs to each message.
    ///
    /// It also indexes the process under `extra_keys` (e.g. an inbound ORDERS'
    /// Belegnummer) so a later LOC-less ORDRSP/ORDCHG can resume it by the
    /// echoed order reference. `occupies` is the workflow's verdict on whether
    /// a found process still holds the key ([`Occupancy`]).
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
    )]
    async fn spawn_or_resume_keyed<W>(
        &self,
        inbound: Inbound<'_>,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        fv: &FormatVersion,
        spawn_deadlines: &[(&'static str, time::OffsetDateTime)],
        extra_keys: &[&str],
        occupies: Option<Occupancy<W>>,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
        // `Process::run` requires `AtomicAppend`:
        Arc<SlateDbStore>: mako_engine::event_store::AtomicAppend,
    {
        if key.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no correlation key in message — cannot register; skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_correlation_key",
            });
        }

        // Held across the whole check-then-act below. Releasing it before the
        // spawn commits would reopen the window it exists to close.
        let _key_guard = self.key_locks.acquire(key).await;

        let registry = self.store.as_process_registry();
        let identities = registry.lookup_correlated(self.tenant_id, key).await?;

        // Filter for this workflow family specifically — there can be multiple
        // concurrent processes per key (e.g. active Lieferbeginn + Sperrung).
        let matching: Vec<ProcessIdentity> = identities
            .into_iter()
            .filter(|id| id.workflow_id.name.as_ref() == workflow_name_static)
            .collect();

        let resume = match occupies {
            Some(occupies) => {
                self.find_occupying_process::<W>(&registry, key, matching, occupies)
                    .await?
            }
            // No published verdict for this family — presence still means resume.
            None => matching.into_iter().next(),
        };

        if let Some(identity) = resume {
            // Existing process — idempotent continuation. Any further key (the
            // Belegnummer of this inbound ORDERS) lands in the same write as the
            // events, so a later LOC-less message can resume this process by
            // reference whenever the continuation itself is durable.
            let process =
                Process::<W, Arc<SlateDbStore>>::from_identity(Arc::clone(&self.store), identity);
            let process_id = process.process_id();
            let keys = correlation_entries(self.tenant_id, &process, extra_keys.iter().copied());
            let (windows, owed_with_aperak) = inbound.aperak_deadlines(&process);
            process
                .run(
                    cmd,
                    RunOptions::new()
                        .received_at(inbound.received_at)
                        .acknowledgement_opt(inbound.anerkennung::<W>())
                        .deadlines(&windows)
                        .delivery_windows(&owed_with_aperak)
                        .correlations(&keys)
                        .snapshot(&self.snap_store, self.snapshot_interval),
                )
                .await?;
            return Ok(IngestOutcome::Dispatched {
                workflow_name: workflow_name_static,
                process_id,
            });
        }

        // No matching process — spawn a fresh one.
        let workflow_id = WorkflowId::new(workflow_name_static, fv.as_str());
        let process = Process::<W, Arc<SlateDbStore>>::new(
            Arc::clone(&self.store),
            self.tenant_id,
            workflow_id.clone(),
        );
        let process_id = process.process_id();

        // Persist events, the APERAK/process Frist deadlines, *and* the
        // correlation-index entries in one write: all three are what a spawn
        // consists of. A process whose events are durable but whose business key
        // is not registered cannot be found by the counterparty's reply — every
        // reply resolves to `Skipped(process_not_found)` and the Frist expires
        // as a false timeout, while the key stays blocked against a fresh spawn.
        let correlations = correlation_entries(
            self.tenant_id,
            &process,
            std::iter::once(key).chain(extra_keys.iter().copied()),
        );

        let (windows, owed_with_aperak) = inbound.aperak_deadlines(&process);
        let deadlines: Vec<Deadline> = spawn_deadlines
            .iter()
            .map(|&(label, due_at)| {
                Deadline::new(
                    process.stream_id().clone(),
                    process_id,
                    self.tenant_id,
                    workflow_id.clone(),
                    label,
                    due_at,
                )
            })
            .chain(windows)
            .collect();
        process
            .run(
                cmd,
                RunOptions::new()
                    .received_at(inbound.received_at)
                    .acknowledgement_opt(inbound.anerkennung::<W>())
                    .deadlines(&deadlines)
                    .delivery_windows(&owed_with_aperak)
                    .correlations(&correlations),
            )
            .await?;

        // Snapshot separately: it is a read-path accelerator, not part of the
        // spawn's durability contract, and a failed snapshot must not undo it.
        if let Err(e) = process
            .take_snapshot(&self.snap_store, self.snapshot_interval)
            .await
        {
            tracing::warn!(
                process_id = %process_id,
                error      = %e,
                "ingest dispatcher: post-spawn snapshot failed (non-fatal — replay is slower)",
            );
        }

        Ok(IngestOutcome::Spawned {
            workflow_name: workflow_name_static,
            process_id,
        })
    }

    /// Return the matched process that still occupies `key`, pruning the
    /// correlation entries of those that have finished.
    ///
    /// The correlation index is **append-only** — `register_correlated` writes
    /// on spawn and nothing removes the entry when the process ends — so an
    /// entry proves a process once existed, never that one is still running.
    /// Resuming on mere presence fed an *initiating* message into a settled
    /// process, which every initiating command rejects outside its initial
    /// state; the business key was then blocked forever, since no spawn could
    /// ever happen again.
    ///
    /// This is the ingest-side twin of the commands-API duplicate guard, and
    /// the prune is load-bearing for the same reason: `resume_by_key` and
    /// `dispatch_to_process_keyed` resolve a key to a single process, so the
    /// finished one must be gone before the replacement is registered.
    /// A failed prune is logged, not propagated — the verdict is already known,
    /// and the next message retries it.
    async fn find_occupying_process<W>(
        &self,
        registry: &impl mako_engine::registry::ProcessRegistry,
        key: &str,
        candidates: Vec<ProcessIdentity>,
        occupies: Occupancy<W>,
    ) -> Result<Option<ProcessIdentity>, EngineError>
    where
        W: Workflow + 'static,
    {
        for identity in candidates {
            let process = Process::<W, Arc<SlateDbStore>>::from_identity(
                Arc::clone(&self.store),
                identity.clone(),
            );
            let process_id = process.process_id();
            if occupies(&process.state().await?) {
                return Ok(Some(identity));
            }
            if let Err(e) = registry
                .remove_correlated(self.tenant_id, key, process_id)
                .await
            {
                tracing::warn!(
                    key           = %key,
                    workflow_name = %identity.workflow_id.name.as_ref(),
                    process_id    = %process_id,
                    error         = %e,
                    "ingest dispatcher: could not retire the finished process's correlation entry",
                );
            }
        }
        Ok(None)
    }

    /// Look up an existing process by business key and execute the continuation
    /// command.
    ///
    /// `key` is whatever the answering message carries: a MaLo for most GPKE
    /// flows, a MeLo for WiM, an invoice reference for REMADV, or the echoed
    /// order reference for a LOC-less ORDRSP/ORDCHG.
    ///
    /// Returns [`IngestOutcome::Skipped`] (not `Err`) when no process is found —
    /// this is expected when the initiating command was handled by the peer role
    /// and no local LF-side process was ever spawned.
    async fn resume_by_key<W>(
        &self,
        inbound: Inbound<'_>,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        if key.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no correlation key in response message — skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_correlation_key",
            });
        }

        let identities = self
            .store
            .as_process_registry()
            .lookup_correlated(self.tenant_id, key)
            .await?;

        let matching: Vec<&ProcessIdentity> = identities
            .iter()
            .filter(|id| id.workflow_id.name.as_ref() == workflow_name_static)
            .collect();

        let identity = match matching.first() {
            Some(id) => (*id).clone(),
            None => {
                tracing::warn!(
                    workflow_name = %workflow_name_static,
                    key           = %key,
                    "ingest dispatcher: no active process for this correlation key — response \
                     dropped; ensure the initiating command was executed first",
                );
                return Ok(IngestOutcome::Skipped {
                    workflow_name: workflow_name_static,
                    reason: "process_not_found",
                });
            }
        };

        let process =
            Process::<W, Arc<SlateDbStore>>::from_identity(Arc::clone(&self.store), identity);
        let process_id = process.process_id();
        let (windows, owed_with_aperak) = inbound.aperak_deadlines(&process);
        process
            .run(
                cmd,
                RunOptions::new()
                    .received_at(inbound.received_at)
                    .acknowledgement_opt(inbound.anerkennung::<W>())
                    .deadlines(&windows)
                    .delivery_windows(&owed_with_aperak)
                    .snapshot(&self.snap_store, self.snapshot_interval),
            )
            .await?;

        Ok(IngestOutcome::Dispatched {
            workflow_name: workflow_name_static,
            process_id,
        })
    }

    /// Resume a process by correlation key, arm deadlines, and index the
    /// process under further keys so later messages can find it.
    ///
    /// The ESA Wertebestellung needs all three at once: an inbound ORDERS
    /// correlates by the reference it echoes, owes an answer within 2 WT, and
    /// must itself become a key because the ORDRSP that answers it and any
    /// later ORDCHG both reference *its* Belegnummer. Resume-only by design —
    /// an order for which no Anfrage was ever seen is an orphan, not a new
    /// process.
    async fn resume_by_key_indexing<W>(
        &self,
        inbound: Inbound<'_>,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        deadlines: &[(&'static str, time::OffsetDateTime)],
        extra_keys: &[&str],
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        if key.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no correlation key in message — skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_correlation_key",
            });
        }
        let identities = self
            .store
            .as_process_registry()
            .lookup_correlated(self.tenant_id, key)
            .await?;
        let Some(identity) = identities
            .iter()
            .find(|id| id.workflow_id.name.as_ref() == workflow_name_static)
            .cloned()
        else {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                key           = %key,
                "ingest dispatcher: no active process for this correlation key — message \
                 dropped; ensure the initiating message was processed first",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "process_not_found",
            });
        };

        let process =
            Process::<W, Arc<SlateDbStore>>::from_identity(Arc::clone(&self.store), identity);
        let process_id = process.process_id();
        let (windows, owed_with_aperak) = inbound.aperak_deadlines(&process);
        let pending: Vec<Deadline> = deadlines
            .iter()
            .map(|(label, due)| {
                Deadline::new(
                    process.stream_id().clone(),
                    process_id,
                    self.tenant_id,
                    process.identity().workflow_id.clone(),
                    *label,
                    *due,
                )
            })
            .chain(windows)
            .collect();
        let keys = correlation_entries(self.tenant_id, &process, extra_keys.iter().copied());
        process
            .run(
                cmd,
                RunOptions::new()
                    .received_at(inbound.received_at)
                    .acknowledgement_opt(inbound.anerkennung::<W>())
                    .deadlines(&pending)
                    .delivery_windows(&owed_with_aperak)
                    .correlations(&keys),
            )
            .await?;

        Ok(IngestOutcome::Dispatched {
            workflow_name: workflow_name_static,
            process_id,
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The correlation key of an ESA-Wertebestellung message, taken from the
/// Zuordnungsschlüssel the BDEW *Anwendungsübersicht der Prüfidentifikatoren*
/// 4.0 publishes for its PID.
///
/// Only the opening REQOTE 35003 is keyed on a location (`ZO-T17`,
/// `LOC+172`). Every later step is keyed on a **Belegnummer** the message
/// echoes, and a conformant ORDERS, ORDCHG, ORDRSP or IFTSTA of Kapitel 4
/// carries no `LOC` at all — so extracting a MaLo from them yields nothing.
///
/// The IFTSTA puts its reference in `SG15 RFF+AGI`; the rest use `SG1 RFF`.
/// Both are plain `RFF` segments on the wire, so one scan covers them.
pub fn esa_korrelation_key(msg: &AnyMessage, pid: u32) -> String {
    let Some(korrelation) = mako_wim::esa::korrelation(pid) else {
        return String::new();
    };
    let Some(qualifier) = korrelation.rff_qualifier() else {
        // `ZO-T17` — the Meldepunkt of the opening REQOTE.
        return String::from(extract_malo_from_msg(msg));
    };
    msg.segments()
        .iter()
        .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .filter(|v| !v.is_empty())
        .map_or_else(
            || {
                // A QUOTES also carries `LOC+172` (Muss), so a partner that
                // omits the `RFF+AAV` is still routable. No other step has a
                // location to fall back on.
                if pid == 15003 {
                    String::from(extract_malo_from_msg(msg))
                } else {
                    String::new()
                }
            },
            str::to_owned,
        )
}

/// Extract the Messlokations-ID from an ORDERS/ORDRSP/ORDCHG message's IDE segment.
///
/// WiM ORDERS messages (Geräteübernahme, Stornierung) identify the
/// Messlokation in the `IDE` segment (element 1, component 0 = object ID).
/// Returns an empty string when the message is not an ORDERS/ORDRSP/ORDCHG or
/// when the IDE segment is absent.
pub fn extract_melo_from_orders(msg: &AnyMessage) -> String {
    let segs = match msg {
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        AnyMessage::Ordchg(o) => o.segments(),
        _ => return String::new(),
    };
    segs.iter()
        .find(|s| s.tag == "IDE")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned()
}

/// The Rechnungsnummer of an inbound INVOIC — `BGM` DE1004 (INVOIC MIG,
/// place 00004) — the key its process is registered under, so the REMADV
/// and COMDIS that name it in `DOC` DE1004 find it. Not the UNH message
/// reference: that numbers the message inside its interchange and is reused
/// by every sender.
///
/// Returns an empty string when the message is not an INVOIC.
pub fn extract_invoice_ref_from_invoic(msg: &AnyMessage) -> String {
    match msg {
        AnyMessage::Invoic(i) => i
            .segments()
            .iter()
            .find(|s| s.tag == "BGM")
            .and_then(|s| s.component_str(1, 0))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// The business answer Frist for an inbound PID.
///
/// Single-sourced from [`mako_fristen::antwort`], the one table `processd` sizes
/// its operator queue by and raises breach alerts against — so the
/// deadline makod registers on the process, the queue entry that expires
/// against it and the alert that fires are three readings of one number.
///
/// The windows are **not** flat durations: GPKE Teil 2 states each one as a
/// wall-clock instant on the first Werktag after the ÜT (11:00 Anmeldung, 06:00
/// Abmeldung, 05:00 NB-seitiges Lieferende, 09:00 Anfrage zur Beendigung), and
/// GeLi Gas as „Ablauf des n. Werktags nach Eingang" (4 WT Anmeldung, 3 WT
/// Abmeldung). A 24-hour approximation expires a Friday arrival on Saturday and
/// — the quiet failure — still calls a Tuesday-evening arrival healthy nine
/// hours after its Frist has lapsed.
///
/// A PID no table quantifies still gets a real instant — a process with no
/// registered deadline never transitions out of `Initiated` and is invisible to
/// the deadline scheduler. But that instant is an **operating convention**, not
/// a Frist, and the difference has to be visible: `mako_fristen::antwort::
/// operator_window` marks it `is_regulatory: false` and carries a `source`
/// saying so, which this logs at `warn` the moment it is used.
///
/// The convention lives here rather than at the call sites. A per-call-site
/// `fallback` invites a copy of `add_hours(received, 24)` — the very number
/// [`GPKE_IS_NOT_TWENTY_FOUR_HOURS`] exists to refute — so registering a PID
/// without adding its row to the table would silently produce a fabricated
/// 24-hour regulatory deadline. One convention, in one place, announcing
/// itself.
///
/// [`GPKE_IS_NOT_TWENTY_FOUR_HOURS`]: mako_fristen::antwort::GPKE_IS_NOT_TWENTY_FOUR_HOURS
pub(crate) fn antwort_due_at(pid: u32, received: OffsetDateTime) -> OffsetDateTime {
    let window = mako_fristen::antwort::operator_window(pid, received);
    if !window.is_regulatory {
        tracing::warn!(
            pid,
            due_at = %window.deadline,
            source = window.source,
            "no published Antwortfrist for this Prüfidentifikator — the process \
             deadline is an operating convention, not a regulatory Frist",
        );
    }
    window.deadline
}

/// Extract the **Fälligkeitsdatum** (Zahlungsziel) from an INVOIC — `SG8 DTM+265`
/// per INVOIC AHB 1.0b. Returns the latest such date when several are present.
///
/// This is the regulatory settlement-response deadline for an INVOIC: the receiver
/// must answer *"zum Zahlungsziel"* (MMM / GeLi Gas / WiM AWH process tables). It is
/// **Sparte-neutral** — the Gas "10 Werktage" is only a sender-side *floor* on the
/// Zahlungsziel that is already reflected in this date — so it is the correct
/// deadline for a universal Stornorechnung (PID 31004) regardless of commodity.
/// Returns `None` when the invoice omits DTM+265 (callers fall back to +10 Werktage).
/// The `IMD+7081` Rechnungstyp of an inbound INVOIC.
///
/// **Muss** on every Anwendungsfall of the AHB, and on PID 31009 it is what
/// says which of three Use-Cases the invoice belongs to: `KON` the ESA billing
/// of WiM Teil 2 Kap. 4.5, `MSB` the Messstellenbetrieb toward NB or LF, `TEC`
/// the Änderung der Technik. They answer under different trees, on different
/// windows.
fn invoic_rechnungstyp(msg: &AnyMessage) -> Option<String> {
    let AnyMessage::Invoic(m) = msg else {
        return None;
    };
    // `IMD+7077+C272+C273`: DE 7081 is the first component of `C272`, so a
    // conformant segment reads `IMD++KON`.
    m.segments()
        .iter()
        .find(|s| s.tag == "IMD")
        .and_then(|s| s.component_str(1, 0))
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

pub(crate) fn faelligkeitsdatum_from_invoic(msg: &AnyMessage) -> Option<time::OffsetDateTime> {
    let AnyMessage::Invoic(m) = msg else {
        return None;
    };
    m.segments()
        .iter()
        .filter(|s| s.tag == "DTM" && s.component_str(0, 0) == Some("265"))
        .filter_map(|s| {
            // DE 2379 is `303` on every INVOIC Anwendungsfall (AHB 1.0b), so
            // the format element decides how the value is read; assuming
            // `CCYYMMDD` here found nothing in a conformant invoice and every
            // Zahlungsziel silently became the +10-Werktage fallback.
            crate::orchestrator::adapters::parse_dtm_datetime(
                s.component_str(0, 1)?,
                s.component_str(0, 2),
            )
        })
        .max()
}

/// The settlement window of an inbound INVOIC whose Anwendungsfall makes the
/// Fälligkeitsdatum `SG8 DTM+265` Muss: the Zahlungsziel the invoice states.
///
/// INVOIC AHB 1.0b places `DTM+265` as Muss on the MMM-Rechnungen 31005–31008
/// (Kap. 3.1.3, S. 39), the Kapazitätsrechnung 31010 (Kap. 3.1.4, S. 45) and the
/// Rechnung sonstige Leistung 31011 (Kap. 3.1.5, S. 53). The payer answers by
/// that date, so it is the deadline; an invoice without one is refused rather
/// than given a window nobody stated.
pub(crate) fn zahlungsziel_due(
    msg: &AnyMessage,
    pid: u32,
) -> Result<time::OffsetDateTime, EngineError> {
    faelligkeitsdatum_from_invoic(msg).ok_or_else(|| {
        EngineError::Deserialization(format!(
            "INVOIC {pid} carries no readable SG8 DTM+265 (Fälligkeitsdatum, INVOIC AHB 1.0b \
             Kap. 3.1) — its settlement window is the Zahlungsziel and cannot be set without it"
        ))
    })
}

/// Extract the Marktlokations-ID from the first LOC segment (component 1, index 0).
///
/// BDEW convention: `LOC+<qualifier>+<malo_id>::<code_list>:Z13`.
/// Applies to ORDERS, ORDRSP, and UTILMD messages.
/// Returns an empty string when the LOC segment is absent (INVOIC, IFTSTA, …).
/// Envelope facts (sender NAD+MS, receiver NAD+MR, BGM document number) for
/// Redispatch EDIFACT messages (IFTSTA/MSCONS/ORDERS/ORDRSP).
fn redispatch_envelope(msg: &AnyMessage) -> (String, String, String) {
    let segs = match msg {
        AnyMessage::Iftsta(m) => m.segments(),
        AnyMessage::Mscons(m) => m.segments(),
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        _ => return (String::new(), String::new(), String::new()),
    };
    let party = |qualifier: &str| {
        segs.iter()
            .find(|s| s.tag == "NAD" && s.component_str(0, 0) == Some(qualifier))
            .and_then(|s| s.component_str(1, 0))
            .unwrap_or("")
            .to_owned()
    };
    let message_ref = segs
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned();
    (party("MS"), party("MR"), message_ref)
}

/// Correlation key for the Redispatch activation process: the MaLo when the
/// message carries one, else the BGM reference, else sender+PID (a stable
/// last-resort bucket so the audit record still lands somewhere queryable).
fn redispatch_process_key(msg: &AnyMessage, message_ref: &str, sender: &str, pid: u32) -> String {
    let malo = String::from(extract_malo_from_msg(msg));
    if !malo.is_empty() {
        malo
    } else if !message_ref.is_empty() {
        message_ref.to_owned()
    } else {
        format!("{sender}-{pid}")
    }
}

pub fn extract_malo_from_msg(msg: &AnyMessage) -> MaLo {
    let segs = match msg {
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        AnyMessage::Utilmd(u) => u.segments(),
        // MSCONS carries the MaLo in LOC (same convention as ORDERS/ORDRSP).
        // Used by gpke-allokationsliste to correlate MSCONS 13013/13014 with the
        // process that was spawned when the LF sent ORDERS 17110/17114.
        AnyMessage::Mscons(m) => m.segments(),
        // REQOTE/QUOTES carry the addressed location in LOC — the ESA
        // Wertebestellung handshake correlates on it.
        AnyMessage::Reqote(r) => r.segments(),
        AnyMessage::Quotes(q) => q.segments(),
        // IFTSTA status/Vollzugsmeldung carries the single addressed location in
        // LOC (MaLo for GPKE Sperrung/SupplierChange, MeLo for WiM device-change) —
        // per the IFTSTA AHB profile, which has one LOC segment. The status reply
        // resumes the process that was opened for that location.
        AnyMessage::Iftsta(i) => i.segments(),
        // UTILTS names the location its Konfigurationsdaten describe in
        // `LOC+172`.
        AnyMessage::Utilts(u) => u.segments(),
        _ => return MaLo::new(""),
    };
    // The location travels in LOC where the profile has one; the GPKE/GeLi
    // Lieferbeginn family (55001/44001, per the official Beispiele) carries
    // the MaLo as the IDE object id instead — fall back to IDE so both the
    // conformant inbound shape and our own loopback renderings correlate.
    MaLo::new(
        segs.iter()
            .find(|s| s.tag == "LOC")
            .and_then(|s| s.component_str(1, 0))
            .filter(|v| !v.is_empty())
            .or_else(|| {
                segs.iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
            })
            .unwrap_or(""),
    )
}

/// The business key of a message that describes its sender rather than a
/// location — PARTIN Kommunikationsdaten, a PRICAT Preisliste: the sending
/// Marktpartner's `NAD+MS` MP-ID. Empty when the message names none.
pub fn extract_sender_key(msg: &AnyMessage) -> String {
    msg.nad_sender().unwrap_or_default().to_owned()
}

// ── marktd consent-gate adapter ───────────────────────────────────────────────

/// [`mako_wim::consent::ConsentGate`] implementation over the marktd HTTP client.
///
/// Maps the wire-level `mako_markt` decision types onto the domain-owned
/// `mako_wim::consent` types and logs every negative outcome here — the pure
/// gating policy in `mako-wim` stays log-free.
pub(crate) struct MarktdConsentGate<'a> {
    /// Borrowed marktd client (present only when consent gating is configured).
    pub client: &'a mako_markt::marktd_client::MarktdClient,
}

impl mako_wim::consent::ConsentGate for MarktdConsentGate<'_> {
    async fn check(
        &self,
        esa_mp_id: &MarktpartnerCode,
        msb_mp_id: &MarktpartnerCode,
        location_id: &str,
        perspective: mako_wim::consent::ConsentPerspective,
    ) -> Result<mako_wim::consent::ConsentDecision, mako_wim::consent::ConsentGateError> {
        use mako_markt::repository as wire;
        use mako_wim::consent as domain;

        let wire_perspective = match perspective {
            domain::ConsentPerspective::MsbInbound => wire::ConsentPerspective::MsbInbound,
            domain::ConsentPerspective::EsaOutbound => wire::ConsentPerspective::EsaOutbound,
        };
        match self
            .client
            .esa_consent_check(
                esa_mp_id.as_str(),
                msb_mp_id.as_str(),
                location_id,
                wire_perspective,
            )
            .await
        {
            Ok(d) => {
                let code = match d.code {
                    wire::ConsentCode::Active => domain::ConsentCode::Active,
                    wire::ConsentCode::SelfAssertion => domain::ConsentCode::SelfAssertion,
                    wire::ConsentCode::NoConsent => domain::ConsentCode::NoConsent,
                    wire::ConsentCode::Revoked => domain::ConsentCode::Revoked,
                    wire::ConsentCode::FrameworkRejected => domain::ConsentCode::FrameworkRejected,
                };
                if !d.allowed {
                    tracing::info!(
                        esa = %esa_mp_id, location = %location_id, code = ?code, ?perspective,
                        "ESA consent gate: blocked — {}", d.reason
                    );
                }
                Ok(domain::ConsentDecision {
                    allowed: d.allowed,
                    code,
                    reason: d.reason,
                })
            }
            Err(e) => {
                tracing::warn!(
                    error = %e, esa = %esa_mp_id, location = %location_id, ?perspective,
                    "ESA consent gate: marktd check failed"
                );
                Err(domain::ConsentGateError(e.to_string()))
            }
        }
    }
}

/// Extract the first echoed order reference from an ORDRSP / ORDCHG, under
/// either `RFF+ACW` or `RFF+ON`.
///
/// A **shape-agnostic** fallback for the non-ESA processes that use these
/// messages. The ESA Wertebestellung has a per-PID Zuordnungsschlüssel — an
/// ORDRSP 19011 keys on `ON` and a 19013 on `ACW`, which point at *different*
/// messages — so it uses [`esa_korrelation_key`] instead, and never this.
///
/// Empty when the message carries neither.
pub fn extract_order_ref_from_msg(msg: &AnyMessage) -> String {
    let segs = match msg {
        AnyMessage::Ordrsp(o) => o.segments(),
        AnyMessage::Ordchg(o) => o.segments(),
        _ => return String::new(),
    };
    segs.iter()
        .find(|s| s.tag == "RFF" && matches!(s.component_str(0, 0), Some("ACW" | "ON")))
        .and_then(|s| s.component_str(0, 1))
        .filter(|v| !v.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Extract the sender MP-ID from the message's `NAD+MS` segment.
///
/// Empty when the message carries no sender NAD.
pub fn extract_sender_mp_id(msg: &AnyMessage) -> MarktpartnerCode {
    let segs = match msg {
        AnyMessage::Reqote(r) => r.segments(),
        AnyMessage::Quotes(q) => q.segments(),
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        _ => return MarktpartnerCode::new(""),
    };
    MarktpartnerCode::new(
        segs.iter()
            .find(|s| s.tag == "NAD" && s.component_str(0, 0) == Some("MS"))
            .and_then(|s| s.component_str(1, 0))
            .unwrap_or(""),
    )
}

/// Extract the receiver MP-ID from the message's `NAD+MR` segment.
///
/// Empty when the message carries no receiver NAD.
pub fn extract_receiver_mp_id(msg: &AnyMessage) -> MarktpartnerCode {
    let segs = match msg {
        AnyMessage::Reqote(r) => r.segments(),
        AnyMessage::Quotes(q) => q.segments(),
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        _ => return MarktpartnerCode::new(""),
    };
    MarktpartnerCode::new(
        segs.iter()
            .find(|s| s.tag == "NAD" && s.component_str(0, 0) == Some("MR"))
            .and_then(|s| s.component_str(1, 0))
            .unwrap_or(""),
    )
}

/// Extract the Lokations-ID the first UTILMD transaction names.
///
/// `SG5 LOC+Z17` for a Messlokation, falling back to `LOC+Z16` for a
/// Marktlokation — the WiM MSB processes (55039, 55042, 55051, 55168) name a
/// MeLo, the GPKE and GeLi Gas ones a MaLo. It is never read from `IDE`, whose
/// DE 7402 is the sender's **Vorgangsnummer** (UTILMD AHB Strom 2.2 marks
/// `SG4 IDE 7495 = 24` and `7402 = Vorgangsnummer` on all three 55039 columns,
/// with the MeLo in `SG5 LOC 3227 = Z17`).
///
/// Returns an empty string when the message is not a UTILMD or names no Lokation.
pub fn extract_melo_from_utilmd(msg: &AnyMessage) -> String {
    let AnyMessage::Utilmd(u) = msg else {
        return String::new();
    };
    u.transactions()
        .first()
        .and_then(|t| t.messlokation().or_else(|| t.marktlokation()))
        .unwrap_or("")
        .to_owned()
}

/// Extract the Messlokation an INSRPT concerns, from its mandatory `LOC+172`.
///
/// The INSRPT is not a UTILMD and has no transactions: reading it with the
/// UTILMD extractor returned the empty string, which keyed every
/// Störungsmeldung of every Messlokation onto one business key.
pub fn extract_melo_from_insrpt(msg: &AnyMessage) -> String {
    let AnyMessage::Insrpt(i) = msg else {
        return String::new();
    };
    i.segments()
        .iter()
        .find(|s| s.tag == "LOC" && s.component_str(0, 0) == Some("172"))
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or_default()
        .to_owned()
}

/// The Rechnungsnummer a REMADV answers, for process correlation.
///
/// It is `DOC` DE1004 „Dokumentennummer" (REMADV MIG, place 00012) — the
/// number the INVOIC stated in `BGM` DE1004 and the key the invoice's process
/// is registered under. `RFF+Z13` is the REMADV's own Prüfidentifikator and
/// names no invoice. The first `DOC` is read; empty when there is none, which
/// the caller skips as uncorrelatable.
pub fn extract_invoice_ref_from_remadv(msg: &AnyMessage) -> String {
    match msg {
        AnyMessage::Remadv(r) => dokumentennummer(r.segments()),
        _ => String::new(),
    }
}

/// The Rechnungsnummer a COMDIS answers — `DOC` DE1004 (COMDIS MIG, place
/// 00010), as for [`extract_invoice_ref_from_remadv`].
pub fn extract_invoice_ref_from_comdis(msg: &AnyMessage) -> String {
    match msg {
        AnyMessage::Comdis(c) => dokumentennummer(c.segments()),
        _ => String::new(),
    }
}

fn dokumentennummer(segments: &[edifact_rs::Segment]) -> String {
    segments
        .iter()
        .find(|s| s.tag == "DOC")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or_default()
        .to_owned()
}

/// Detect the BDEW format version to dispatch a message under.
///
/// The FV selects which `MessageAdapter` handles the message
/// (`accepts_format_version`) and names the spawned `WorkflowId`. It does **not**
/// choose the AHB profile used for validation — that comes from the message's
/// own release in the AS4 ingest path.
///
/// Falls back to the **newest registered FV** when the version cannot be derived:
///
/// - the message is an `AnyMessage::Unknown` variant (no message type);
/// - the UNH association code is absent or unparseable;
/// - no profile is registered for the `(message_type, release)` pair on today's
///   date, i.e. a release this binary predates or one already archived;
/// - the profile carries no `valid_from` date.
///
/// Falling back rather than rejecting is deliberate: during an annual cutover a
/// counterparty may send a release this binary has not been updated for, and
/// refusing the message outright is worse than dispatching it under the closest
/// registered version. Adapters accept every registered FV and a running process
/// keeps its original `WorkflowId`, so the substitution is usually invisible in
/// behaviour.
///
/// It is logged all the same. A substituted FV means mako could not read the
/// release the counterparty stated, and during the transition window that is a
/// signal a profile is missing from the deployed binary. The spawned process also
/// carries the substituted FV in its `WorkflowId`, so the audit trail names a
/// release the message did not claim.
fn detect_format_version(msg: &AnyMessage) -> FormatVersion {
    // Derive the fallback dynamically from the registry so it stays current
    // across annual format-version cutovers without a code change.
    let fallback = |reason: &'static str| {
        let fv = adapters::known_fvs().into_iter().max().unwrap_or_else(|| {
            // Last-resort: if the registry is empty (pathological), use
            // the current production FV. This branch should never fire.
            FormatVersion::parse("FV2025-10-01")
                .expect("FV2025-10-01 is always a valid FormatVersion literal")
        });
        tracing::warn!(
            reason,
            substituted_fv = %fv.as_str(),
            message_type = ?msg.try_message_type(),
            "format version could not be derived from the message — validating \
             against the newest known release instead; the AHB rules applied are \
             not necessarily those of the release the message claims",
        );
        fv
    };

    let Some(message_type) = msg.try_message_type() else {
        return fallback("unknown message type");
    };
    let Ok(release) = msg.detect_release() else {
        return fallback("release not present or unparseable in UNH");
    };

    let today = mako_fristen::heute();
    let Ok(profile) = ReleaseRegistry::global().profile_on(message_type, release, today) else {
        // The interesting case: a release mako has no profile for on this date —
        // a future FV it has not been updated for, or one already archived.
        return fallback("no profile registered for this release on today's date");
    };
    let Some(valid_from) = profile.valid_from() else {
        return fallback("profile carries no valid_from date");
    };

    let fv_str = format!(
        "FV{:04}-{:02}-{:02}",
        valid_from.year(),
        valid_from.month() as u8,
        valid_from.day(),
    );
    FormatVersion::parse(&fv_str)
        .unwrap_or_else(|_| fallback("profile valid_from did not render a valid FormatVersion"))
}

// ── Unknown-workflow fallback ─────────────────────────────────────────────────
//
// WARNING: messages routed here are dead-lettered. Any workflow name landing
// here should be investigated and given an explicit dispatch arm in the
// matching family submodule.
fn unknown_workflow_skip(wf_name: &str, pid: u32) -> Result<IngestOutcome, EngineError> {
    tracing::warn!(
        workflow_name = %wf_name,
        pid,
        "ingest dispatcher: no Phase 2 handler — add a dispatch arm in \
         ingest_dispatcher to handle this workflow",
    );
    Ok(IngestOutcome::Skipped {
        workflow_name: "unregistered",
        reason: SKIP_WORKFLOW_NOT_DISPATCHED,
    })
}

// ── Per-family dispatch submodules ────────────────────────────────────────────

mod emob;
mod gabi_gas;
mod geli_gas;
mod gpke;
mod mabis;
mod redispatch;
mod wim;

/// The correlation-index entries that make `process` findable under `keys`,
/// skipping empty ones. Passed to [`RunOptions::correlations`] so a key lands
/// in the same write as the events that justify it.
/// The correlation index entries a spawn writes: one per business key, and one
/// under the process's own id.
///
/// The id is how a caller names a process that has no business key yet — the
/// answer to an Anmeldung neue Marktlokation that named no Marktlokation
/// (Bedingung `[165]` „Wenn bekannt") carries the `process_id` its
/// `de.mako.process.initiated` event published. Without the entry that answer
/// finds no process.
fn correlation_entries<'k, W: Workflow>(
    tenant_id: TenantId,
    process: &Process<W, Arc<SlateDbStore>>,
    keys: impl IntoIterator<Item = &'k str>,
) -> Vec<CorrelationEntry> {
    let identity = process.identity();
    let own = process.process_id().to_string();
    keys.into_iter()
        .filter(|k| !k.is_empty())
        .map(ToOwned::to_owned)
        .chain(std::iter::once(own))
        .map(|tag| CorrelationEntry {
            tenant_id,
            tag,
            process_id: process.process_id(),
            identity: identity.clone(),
        })
        .collect()
}

#[cfg(test)]
mod faelligkeitsdatum_tests {
    use super::faelligkeitsdatum_from_invoic;

    fn parse_invoic(dtm_segments: &str) -> edi_energy::AnyMessage {
        let raw = format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+260101:0000+1'\
             UNH+1+INVOIC:D:06A:UN:2.8e'\
             BGM+457+00031004'\
             DTM+137:202601010000?+00:303'{dtm_segments}\
             NAD+MS+4012345000023::293'\
             NAD+MR+9900357000004::293'\
             UNT+7+1'UNZ+1+1'"
        );
        edi_energy::parse(raw.as_bytes()).expect("valid INVOIC parses")
    }

    #[test]
    fn extracts_faelligkeitsdatum_from_dtm_265() {
        let msg = parse_invoic("DTM+265:20260215:102'");
        let due = faelligkeitsdatum_from_invoic(&msg).expect("DTM+265 present");
        assert_eq!(due.date(), time::macros::date!(2026 - 02 - 15));
    }

    #[test]
    fn no_dtm_265_yields_none_so_caller_falls_back() {
        // Only the invoice date (DTM+137) is present — no Zahlungsziel.
        let msg = parse_invoic("");
        assert!(faelligkeitsdatum_from_invoic(&msg).is_none());
    }

    /// INVOIC AHB 1.0b gives `SG8 DTM+265` as DE 2379 `303` in every
    /// Anwendungsfall, so this is the shape a conformant invoice actually
    /// carries. Reading it as `CCYYMMDD` returned `None`, and every Zahlungsziel
    /// silently became the +10-Werktage fallback.
    #[test]
    fn extracts_faelligkeitsdatum_from_a_conformant_303_value() {
        let msg = parse_invoic("DTM+265:202602151200?+00:303'");
        let due = faelligkeitsdatum_from_invoic(&msg).expect("DTM+265 present");
        assert_eq!(due.date(), time::macros::date!(2026 - 02 - 15));
        assert_eq!(due.hour(), 12);
    }

    /// DE 2379 `273` is a *duration* (REQOTE Bindungsfrist), not a point in
    /// time. A reader that ignores the format element would turn a count of
    /// days into a year.
    #[test]
    fn a_non_date_format_yields_none_rather_than_a_guess() {
        let msg = parse_invoic("DTM+265:5:273'");
        assert!(faelligkeitsdatum_from_invoic(&msg).is_none());
    }

    #[test]
    fn multiple_dtm_265_takes_the_latest() {
        let msg = parse_invoic("DTM+265:20260215:102'DTM+265:20260320:102'");
        let due = faelligkeitsdatum_from_invoic(&msg).expect("DTM+265 present");
        assert_eq!(due.date(), time::macros::date!(2026 - 03 - 20));
    }
}

#[cfg(test)]
mod business_key_lock_tests {
    use super::BusinessKeyLocks;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Two holders of the same key must never be inside the guarded section at
    /// once.
    ///
    /// This is the mechanism behind the one-key-one-process invariant. The
    /// integration test cannot prove it: the window between `lookup_correlated`
    /// returning empty and the spawn committing is narrow, and whether two
    /// ingests land inside it depends on store latency. Here the overlap is
    /// forced with an await point in the critical section, so mutual exclusion
    /// is asserted rather than hoped for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_same_key_is_never_held_twice() {
        let locks = Arc::new(BusinessKeyLocks::default());
        let occupied = Arc::new(AtomicBool::new(false));
        let overlaps = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let locks = Arc::clone(&locks);
            let occupied = Arc::clone(&occupied);
            let overlaps = Arc::clone(&overlaps);
            handles.push(tokio::spawn(async move {
                let _guard = locks.acquire("51238696012").await;
                if occupied.swap(true, Ordering::SeqCst) {
                    overlaps.fetch_add(1, Ordering::SeqCst);
                }
                // Yield inside the section: without the lock this is where a
                // second task would observe the same "no process yet" state.
                tokio::task::yield_now().await;
                occupied.store(false, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("task joins");
        }

        assert_eq!(
            overlaps.load(Ordering::SeqCst),
            0,
            "two ingests were inside the lookup→spawn section for one business \
             key at the same time; both would have found no process and spawned \
             one, leaving the key resolving to two",
        );
    }

    /// Different keys must not block each other — the lock is per key, not a
    /// global ingest bottleneck.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn different_keys_do_not_serialise() {
        let locks = Arc::new(BusinessKeyLocks::default());
        let both_inside = Arc::new(tokio::sync::Barrier::new(2));

        let a = {
            let locks = Arc::clone(&locks);
            let barrier = Arc::clone(&both_inside);
            tokio::spawn(async move {
                let _g = locks.acquire("51238696012").await;
                barrier.wait().await;
            })
        };
        let b = {
            let locks = Arc::clone(&locks);
            let barrier = Arc::clone(&both_inside);
            tokio::spawn(async move {
                let _g = locks.acquire("51238696782").await;
                barrier.wait().await;
            })
        };

        // The barrier only releases when both are holding their locks at once,
        // so completing at all is the assertion.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            a.await.expect("task a");
            b.await.expect("task b");
        })
        .await
        .expect("two different keys must be holdable concurrently");
    }

    /// The map must not accumulate an entry per business key ever seen.
    #[tokio::test]
    async fn released_keys_are_pruned() {
        let locks = BusinessKeyLocks::default();
        for i in 0..100 {
            let _g = locks.acquire(&format!("key-{i}")).await;
        }
        let held = locks
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        assert!(
            held <= 1,
            "the lock map must track in-flight spawns, not every key ever seen; \
             it holds {held} entries after 100 sequential acquisitions",
        );
    }
}

#[cfg(test)]
mod aperak_window_tests {
    use super::{AperakWindow, aperak_window};
    use edi_energy::MessageType;
    use mako_engine::types::Sparte;
    use mako_fristen as fristen;

    /// Tue 2026-07-07 12:00 CEST.
    const AT: time::OffsetDateTime = time::macros::datetime!(2026-07-07 10:00 UTC);

    /// APERAK AHB 1.1 Kap. 2.4.1 names UTILMD and ORDERS for the 45 minutes — an
    /// ORDCHG is neither and falls to the next Werktag 12 Uhr.
    #[test]
    fn strom_utilmd_and_orders_take_45_minutes_and_the_rest_the_next_werktag() {
        for mt in [MessageType::Utilmd, MessageType::Orders] {
            assert_eq!(
                aperak_window(Some(mt), Sparte::Strom, 55_001, AT),
                Some(AperakWindow {
                    label: fristen::APERAK_STROM_WINDOW_LABEL,
                    due_at: fristen::aperak_strom_due_at(AT),
                    only_with_aperak: false,
                })
            );
        }
        for mt in [
            MessageType::Mscons,
            MessageType::Ordchg,
            MessageType::Invoic,
        ] {
            assert_eq!(
                aperak_window(Some(mt), Sparte::Strom, 13_018, AT),
                Some(AperakWindow {
                    label: fristen::APERAK_STROM_ALLGEMEIN_LABEL,
                    due_at: fristen::aperak_strom_allgemein_due_at(AT),
                    only_with_aperak: false,
                })
            );
        }
    }

    /// Gas: the PID decides Initial- vs. Folgeprozess (Kap. 2.3.1), and the window
    /// exists only beside an APERAK (Kap. 2.3).
    #[test]
    fn gas_follows_the_pid_and_only_rides_an_aperak() {
        let initial = aperak_window(Some(MessageType::Utilmd), Sparte::Gas, 44_001, AT)
            .expect("a Gas UTILMD has a window");
        assert_eq!(initial.label, fristen::APERAK_GAS_INITIALPROZESS_LABEL);
        assert_eq!(
            initial.due_at,
            fristen::aperak_gas_initialprozess_due_at(AT)
        );
        assert!(initial.only_with_aperak);
        let folge = aperak_window(Some(MessageType::Utilmd), Sparte::Gas, 44_002, AT)
            .expect("a Gas UTILMD has a window");
        assert_eq!(folge.label, fristen::APERAK_GAS_FOLGEPROZESS_LABEL);
        assert!(folge.only_with_aperak);
    }

    /// An APERAK or a CONTRL is never answered with an APERAK.
    #[test]
    fn acknowledgements_open_no_window() {
        for mt in [MessageType::Aperak, MessageType::Contrl] {
            assert_eq!(aperak_window(Some(mt), Sparte::Strom, 0, AT), None);
        }
        assert_eq!(aperak_window(None, Sparte::Strom, 0, AT), None);
    }
}

#[cfg(test)]
mod fehlerklasse_tests {
    use super::{Fehlerklasse, ftx_text};
    use crate::adapters::erc_of;

    #[test]
    fn a_rule_id_names_its_class() {
        assert_eq!(
            Fehlerklasse::of("FMT-55001-00047-LOC-3225-950"),
            Some(Fehlerklasse::Fmt)
        );
        assert_eq!(
            Fehlerklasse::of("AHB-13018-00004-DTM-MISSING"),
            Some(Fehlerklasse::Ahb)
        );
        assert_eq!(
            Fehlerklasse::of("MIG-00004-DTM-REQUIRED"),
            Some(Fehlerklasse::Mig)
        );
        assert_eq!(Fehlerklasse::of("SEM-DTM-VALUE"), None);
    }

    /// The counter is keyed by the number, never by the unbounded rule id.
    #[test]
    fn a_format_rule_id_names_its_formatbedingung() {
        assert_eq!(
            Fehlerklasse::formatbedingung("FMT-55001-00020-DTM-2380-932"),
            Some("932")
        );
        assert_eq!(
            Fehlerklasse::formatbedingung("AHB-55001-00020-DTM-MISSING"),
            None
        );
    }

    /// APERAK AHB 1.1 Kap. 4.3: `Z35` for a Formatbedingung (S. 45), `Z29` for
    /// a missing Angabe, `Z39` for a code outside the Wertebereich; a MIG
    /// finding has no code here.
    #[test]
    fn each_coded_finding_has_its_erc() {
        assert_eq!(erc_of("FMT-55001-00047-LOC-3225-950"), Some("Z35"));
        assert_eq!(erc_of("AHB-13018-00004-DTM-MISSING"), Some("Z29"));
        assert_eq!(erc_of("AHB-55001-00020-DTM-2005-CODE"), Some("Z39"));
        assert_eq!(erc_of("AHB-55001-00020-DTM-NOT-PERMITTED"), None);
        assert_eq!(erc_of("MIG-00004-DTM-REQUIRED"), None);
    }

    /// A validator message is not UNOC: its quotes and dashes are mapped, the
    /// umlauts kept, and the text bounded by DE 4440 `an..512`.
    #[test]
    fn a_finding_text_fits_ftx() {
        assert_eq!(
            ftx_text("DE 2380 „Datum“ — Prüfschablone"),
            "DE 2380 \"Datum\" - Prüfschablone"
        );
        assert_eq!(ftx_text("⊻"), "?");
        assert_eq!(ftx_text(&"x".repeat(600)).chars().count(), 512);
    }
}
