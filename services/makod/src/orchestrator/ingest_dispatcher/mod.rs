//! Phase 2 ingest execution: EDIFACT message → typed domain command → Process.
//!
//! [`EdifactIngestDispatcher`] bridges parsed [`AnyMessage`] values from the
//! EDIFACT ingest layer to running domain workflow processes.  Used by:
//!
//! - `edifact_api` — REST `POST /edifact` path
//! - `as4_ingest`  — AS4 inbound delivery path
//! - `as4_sender`  — in-process loopback for self-addressed messages
//!   (combined-role deployments: NB+LF, NB+MSB, GNB+gMSB sharing one GLN)
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
//!   by MaLo; if nothing found, spawns a fresh process and registers it.
//! - **Resume** (continue existing): e.g. NB receives ORDRSP 19118 from MSB.
//!   Uses `lookup_correlated` by MaLo; returns [`IngestOutcome::Skipped`] if no
//!   process is found (not an error — peer may have sent an orphan message).
//!
//! ## Combined-role loopback
//!
//! When `BdewAs4Sender` detects `recipient == own_mp_id`, it
//! renders the domain payload to EDIFACT wire bytes, re-parses them, and calls
//! `dispatch` here instead of transmitting over AS4.  This enables zero-latency
//! in-process delivery for Stadtwerke deployments (NB+LF, GNB+gMSB).

use std::any::Any;
use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _, ReleaseRegistry};
use mako_engine::{
    deadline::Deadline,
    error::EngineError,
    fristen::{self, HolidayCalendar},
    ids::{ProcessId, ProcessIdentity, TenantId},
    process::Process,
    registry::ProcessRegistry as _,
    store_slatedb::{SlateDbSnapshotStore, SlateDbStore},
    types::{MaLo, MarktpartnerCode},
    version::{FormatVersion, WorkflowId},
    workflow::{CommandPayload, Workflow},
};
use mako_gabi_gas::{GaBiGasAllocationWorkflow, GaBiGasInvoicWorkflow, GaBiGasNominationWorkflow};
use mako_geli_gas::{
    GeliGasDatanabrufWorkflow, GeliGasLfAnmeldungWorkflow, GeliGasLfStornierungWorkflow,
    GeliGasMsconsWorkflow, GeliGasPartinWorkflow, GeliGasSperrprozesseInvoicWorkflow,
    GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow, GeliGasStornierungWorkflow,
    GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    GpkeAbrechnungWorkflow, GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeDatanabrufWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeMesswerteLieferungWorkflow, GpkeNeuanlageWorkflow,
    GpkePartinWorkflow, GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, GpkeStornierungWorkflow,
    GpkeSupplierChangeWorkflow, GpkeUtiltsWorkflow,
};
use mako_mabis::{MabisBillingWorkflow, MabisClearinglisteWorkflow};
use mako_wim::{
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimInsrptWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimRechnungWorkflow, WimStammdatenWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungWorkflow, WimGasInsrptWorkflow, WimGasInvoicWorkflow, WimGasKuendigungWorkflow,
    WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageWorkflow,
};
use time::OffsetDateTime;

use crate::adapters;

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Outcome of a successful ingest dispatch attempt.
#[derive(Debug)]
#[allow(dead_code)] // fields are read via Debug formatting in tracing events
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
    /// GLNs of counterparties acting as an Energieserviceanbieter.
    ///
    /// See [`EdifactIngestDispatcher::with_esa_partners`].
    esa_partner_mp_ids: std::collections::HashSet<String>,
    /// marktd client used to gate inbound ESA messages against the consent
    /// registry. `None` disables the gate (dev mode / marktd not configured).
    ///
    /// See [`EdifactIngestDispatcher::with_marktd_client`].
    marktd_client: Option<Arc<mako_markt::marktd_client::MarktdClient>>,
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
        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        "gabi-gas-allocation",
        "gabi-gas-delivery-order",
        "gabi-gas-imbnot",
        "gabi-gas-invoic",
        "gabi-gas-mmma",
        "gabi-gas-nomination",
        "gabi-gas-schedl",
        "gabi-gas-tranot",
        "geli-gas-datenabruf",
        "geli-gas-lf-anmeldung",
        "geli-gas-mscons",
        "geli-gas-partin",
        "geli-gas-sperrprozesse-invoic",
        "geli-gas-sperrung-lf",
        "geli-gas-sperrung-nb",
        "geli-gas-stornierung",
        "geli-gas-stornierung-lf",
        "geli-gas-supplier-change",
        "gpke-abrechnung",
        "gpke-allokationsliste",
        "gpke-anfrage-bestellung",
        "gpke-ankuendigung-zuordnung-lf",
        "gpke-datenabruf",
        "gpke-eog",
        "gpke-konfiguration",
        "gpke-konfiguration-aenderung",
        "gpke-lf-abmeldung",
        "gpke-lf-anmeldung",
        "gpke-messwerte",
        "gpke-neuanlage",
        "gpke-partin",
        "gpke-sperrung",
        "gpke-sperrung-lf",
        "gpke-stornierung",
        "gpke-supplier-change",
        "gpke-utilts",
        "mabis-billing",
        "mabis-clearingliste",
        "redispatch-aktivierung",
        "wim-device-change",
        "wim-gas-anmeldung",
        "wim-gas-insrpt",
        "wim-gas-invoic",
        "wim-gas-kuendigung",
        "wim-gas-stornierung",
        "wim-gas-verpflichtungsanfrage",
        "wim-geraeteubernahme",
        "wim-insrpt",
        "wim-preisanfrage",
        "wim-preisliste",
        "wim-rechnung",
        "wim-stammdaten",
        "wim-technik-aenderung",
        mako_wim::wertebestellung::WORKFLOW_NAME,
    ];

    /// Register the GLNs of counterparties known to act as an
    /// Energieserviceanbieter.
    ///
    /// REQOTE 35002 is shared: an ESA Werteanfrage (WiM Teil 2 Kap. 4 UC 4.1
    /// Nr. 1) and a Preisanfrage arrive under the same Prüfidentifikator,
    /// because no ESA-specific REQOTE PID exists. The sender's registered role
    /// is the decisive discriminator — an ESA is registered via PARTIN 37006 —
    /// and the message itself carries only the market-partner ID, not the role.
    /// Supplying the known ESA partner IDs here turns that signal on; without it
    /// the classifier falls back to the `PIA` Messprodukt marker alone.
    #[must_use]
    pub fn with_esa_partners(mut self, mp_ids: impl IntoIterator<Item = String>) -> Self {
        self.esa_partner_mp_ids = mp_ids.into_iter().collect();
        self
    }

    /// Wire the marktd consent-registry gate for inbound ESA messages.
    ///
    /// With a client set, an ESA Werteanfrage (REQOTE 35002) or Bestellung
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

    /// `true` when `gln` is a registered ESA counterparty.
    fn sender_is_esa(&self, mp_id: &str) -> bool {
        !mp_id.is_empty() && self.esa_partner_mp_ids.contains(mp_id)
    }

    /// Gate an inbound ESA `WertebestellungCommand` against the consent registry.
    ///
    /// Thin boundary wrapper: extracts the ESA/MSB/location identifiers from the
    /// wire message and delegates the fail-open policy to
    /// [`mako_wim::consent::gate_inbound`]. With no marktd client configured the
    /// command passes through unchanged (the gate is defence-in-depth; the
    /// durable stop signal remains the 17008 Abbestellung fired on revocation).
    async fn gate_esa_consent(
        &self,
        msg: &AnyMessage,
        cmd: mako_wim::wertebestellung::WertebestellungCommand,
    ) -> mako_wim::wertebestellung::WertebestellungCommand {
        let Some(marktd) = &self.marktd_client else {
            return cmd;
        };
        let esa = extract_sender_mp_id(msg);
        let msb = extract_receiver_mp_id(msg);
        let location = extract_malo_from_msg(msg);
        mako_wim::consent::gate_inbound(
            cmd,
            &esa,
            &msb,
            &location,
            &MarktdConsentGate { client: marktd },
        )
        .await
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
            esa_partner_mp_ids: std::collections::HashSet::new(),
            marktd_client: None,
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
    pub async fn dispatch(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let outcome = self.dispatch_inner(msg, workflow_name, pid).await;
        let result = match &outcome {
            Ok(IngestOutcome::Spawned { .. } | IngestOutcome::Dispatched { .. }) => "dispatched",
            Ok(IngestOutcome::Skipped { .. }) => "skipped",
            Err(_) => "error",
        };
        mako_engine::metrics::EngineMetrics::global().inbound_received(pid, result);
        outcome
    }

    /// Family router — pure routing by workflow-family name prefix.
    ///
    /// Each family's dispatch arms live in the correspondingly named submodule
    /// (`gpke`, `geli_gas`, `wim`, `wim_gas`, `gabi_gas`, `mabis`,
    /// `redispatch`); the per-family method re-matches on `workflow_name`.
    async fn dispatch_inner(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        match workflow_name {
            n if n.starts_with("geli-gas-") => self.dispatch_geli_gas(msg, n, pid).await,
            n if n.starts_with("wim-gas-") => self.dispatch_wim_gas(msg, n, pid).await,
            n if n.starts_with("gabi-gas-") => self.dispatch_gabi_gas(msg, n, pid).await,
            n if n.starts_with("gpke-") => self.dispatch_gpke(msg, n, pid).await,
            // ESA-side Wertebestellung is WiM Strom Teil 2 — its arm lives in
            // the `wim` submodule next to the MSB-side half of the handshake.
            n if n == mako_wim::esa_wertebestellung::WORKFLOW_NAME => {
                self.dispatch_wim(msg, n, pid).await
            }
            n if n.starts_with("wim-") => self.dispatch_wim(msg, n, pid).await,
            n if n.starts_with("mabis-") => self.dispatch_mabis(msg, n, pid).await,
            n if n.starts_with("redispatch-") => self.dispatch_redispatch(msg, n, pid).await,
            // ── All other workflows: not yet in Phase 2 dispatch table ────────
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
        self.spawn_or_resume::<W>(key, workflow_name_static, cmd, &fv, spawn_deadlines)
            .await
    }

    /// Resume an existing Redispatch process by business key (no spawn).
    ///
    /// Used for correlation-routed documents (`AcknowledgementDocument`):
    /// the key is the MRID of the document being acknowledged, under which
    /// the target process was registered at spawn.
    pub(crate) async fn resume_redispatch<W>(
        &self,
        key: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        self.resume_by_malo::<W>(key, workflow_name_static, cmd)
            .await
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Look up an existing process by MaLo business key and workflow name.
    ///
    /// If a matching process exists, execute `cmd` on it and return
    /// [`IngestOutcome::Dispatched`].  Otherwise spawn a new process, execute
    /// `cmd`, register the process under the MaLo tag, and return
    /// [`IngestOutcome::Spawned`].
    ///
    /// `spawn_deadlines`: zero or more `(label, due_at)` pairs to register
    /// atomically with the events in a single `WriteBatch` via
    /// [`Process::execute_and_enqueue_with_deadlines`].  Pass `&[]` for
    /// workflows that have no deadlines (e.g. pure continuation handlers).
    /// Deadlines are only registered for freshly-spawned processes — resuming
    /// an existing process must not re-register (deadlines were set at spawn).
    ///
    /// To satisfy APERAK AHB 1.0 §2.4.1 for Strom UTILMD/ORDERS, callers
    /// should pass **two** deadlines: the process-response window and the
    /// 45-minute APERAK sending window (`fristen::APERAK_STROM_WINDOW_LABEL`).
    async fn spawn_or_resume<W>(
        &self,
        malo_id: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        fv: &FormatVersion,
        spawn_deadlines: &[(&'static str, time::OffsetDateTime)],
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
        Arc<SlateDbStore>: mako_engine::event_store::AtomicAppend,
    {
        self.spawn_or_resume_keyed::<W>(
            malo_id,
            workflow_name_static,
            cmd,
            fv,
            spawn_deadlines,
            &[],
        )
        .await
    }

    /// [`spawn_or_resume`](Self::spawn_or_resume) that also indexes the process
    /// under `extra_keys` (e.g. an inbound ORDERS' Belegnummer) so a later
    /// LOC-less ORDRSP/ORDCHG can resume it by the echoed order reference.
    async fn spawn_or_resume_keyed<W>(
        &self,
        malo_id: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        fv: &FormatVersion,
        spawn_deadlines: &[(&'static str, time::OffsetDateTime)],
        extra_keys: &[&str],
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
        // `execute_and_enqueue_with_deadlines` requires `AtomicAppend`:
        Arc<SlateDbStore>: mako_engine::event_store::AtomicAppend,
    {
        if malo_id.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no MaLo ID in message — cannot register; skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_malo_id",
            });
        }

        let registry = self.store.as_process_registry();
        let identities = registry.lookup_correlated(self.tenant_id, malo_id).await?;

        // Filter for this workflow family specifically — there can be multiple
        // concurrent processes per MaLo (e.g. active Lieferbeginn + Sperrung).
        let matching: Vec<&ProcessIdentity> = identities
            .iter()
            .filter(|id| id.workflow_id.name.as_ref() == workflow_name_static)
            .collect();

        if let Some(first) = matching.first() {
            // Existing process — idempotent continuation.
            let identity = (*first).clone();
            let process =
                Process::<W, Arc<SlateDbStore>>::from_identity(Arc::clone(&self.store), identity);
            let process_id = process.process_id();
            process
                .execute_and_enqueue_with_snapshot_and_retry(
                    cmd,
                    3,
                    &self.snap_store,
                    self.snapshot_interval,
                )
                .await?;
            // Register any additional correlation keys (e.g. the Belegnummer of
            // this inbound ORDERS) so a later LOC-less message can resume this
            // process by reference.
            self.register_extra_keys(&registry, process_id, (*first).clone(), extra_keys)
                .await;
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

        // Atomically persist events and (when applicable) the APERAK/process Frist
        // deadlines.  Using `execute_and_enqueue_with_deadlines` ensures a crash
        // between event write and deadline registration cannot produce a process with
        // no monitoring window (dual-write atomicity requirement).
        if spawn_deadlines.is_empty() {
            process
                .execute_and_enqueue_with_snapshot_and_retry(
                    cmd,
                    3,
                    &self.snap_store,
                    self.snapshot_interval,
                )
                .await?;
        } else {
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
                .collect();
            process
                .execute_and_enqueue_with_deadlines(cmd, &deadlines)
                .await?;
        }

        // Register under MaLo business key for future correlation lookups.
        let identity = process.identity();
        if let Err(e) = registry
            .register_correlated(self.tenant_id, malo_id, process_id, identity.clone())
            .await
        {
            tracing::warn!(
                process_id = %process_id,
                malo_id    = %malo_id,
                error      = %e,
                "ingest dispatcher: MaLo registry failed (non-fatal — process was spawned)",
            );
        }
        self.register_extra_keys(&registry, process_id, identity, extra_keys)
            .await;

        Ok(IngestOutcome::Spawned {
            workflow_name: workflow_name_static,
            process_id,
        })
    }

    /// Register a process under additional correlation keys (best-effort).
    ///
    /// Used to index a Wertebestellung process by the Belegnummer of an inbound
    /// ORDERS so a later ORDRSP/ORDCHG — which carry no LOC — can resume it by
    /// the echoed order reference.
    async fn register_extra_keys<R: mako_engine::registry::ProcessRegistry>(
        &self,
        registry: &R,
        process_id: ProcessId,
        identity: ProcessIdentity,
        extra_keys: &[&str],
    ) {
        for key in extra_keys.iter().filter(|k| !k.is_empty()) {
            if let Err(e) = registry
                .register_correlated(self.tenant_id, key, process_id, identity.clone())
                .await
            {
                tracing::warn!(
                    process_id = %process_id,
                    key        = %key,
                    error      = %e,
                    "ingest dispatcher: order-reference registry failed (non-fatal)",
                );
            }
        }
    }

    /// Look up an existing process by MaLo and execute the continuation command.
    ///
    /// Returns [`IngestOutcome::Skipped`] (not `Err`) when no process is found —
    /// this is expected when the initiating command was handled by the peer role
    /// and no local LF-side process was ever spawned.
    async fn resume_by_malo<W>(
        &self,
        malo_id: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        if malo_id.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no MaLo ID in response message — skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_malo_id",
            });
        }

        let identities = self
            .store
            .as_process_registry()
            .lookup_correlated(self.tenant_id, malo_id)
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
                    malo_id       = %malo_id,
                    "ingest dispatcher: no active process for MaLo — response dropped; \
                     ensure the initiating command was executed first",
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
        process
            .execute_and_enqueue_with_snapshot_and_retry(
                cmd,
                3,
                &self.snap_store,
                self.snapshot_interval,
            )
            .await?;

        Ok(IngestOutcome::Dispatched {
            workflow_name: workflow_name_static,
            process_id,
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the Messlokations-ID from an ORDERS/ORDRSP/ORDCHG message's IDE segment.
///
/// WiM ORDERS messages (Geräteübernahme, Stammdaten, Stornierung) identify the
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

/// Extract the INVOIC sender/document reference for MaLo correlation.
///
/// INVOIC messages do not carry a LOC or IDE segment. Use the message reference
/// (UNH DE 0062) as the correlation key — workflows that receive INVOIC messages
/// as initial commands are keyed on the invoice reference, not a MaLo.
///
/// Returns an empty string when the message is not an INVOIC.
pub fn extract_malo_from_invoic(msg: &AnyMessage) -> String {
    match msg {
        AnyMessage::Invoic(_) => msg.message_ref().to_owned(),
        _ => String::new(),
    }
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

/// Extract the echoed order reference from an ORDRSP / ORDCHG.
///
/// These two messages carry **no** LOC in their conformant ESA-Wertebestellung
/// form, so they cannot be correlated by MaLo. Instead:
/// - an **ORDRSP** answer echoes the order it answers in `RFF+ACW`, and
/// - the ESA's **ORDCHG** Stornierung references the original Bestellung's
///   Belegnummer in `RFF+ON`.
///
/// The referenced process is registered under that Belegnummer (the dispatcher
/// indexes it under the inbound ORDERS' Belegnummer via `extra_keys`), so
/// returning it here lets the dispatcher resume the correct process. Empty when
/// absent.
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

/// Extract the sender GLN from the message's `NAD+MS` segment.
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

/// Extract the receiver GLN from the message's `NAD+MR` segment.
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

/// Extract every `PIA` product identifier from a message.
///
/// BDEW encodes the Messprodukt as `PIA+5+<code>::<code list>`. Used to tell an
/// ESA Werteanfrage from a Preisanfrage, which share REQOTE PID 35002.
pub fn extract_pia_codes(msg: &AnyMessage) -> Vec<&str> {
    let segs = match msg {
        AnyMessage::Reqote(r) => r.segments(),
        AnyMessage::Quotes(q) => q.segments(),
        AnyMessage::Orders(o) => o.segments(),
        _ => return Vec::new(),
    };
    segs.iter()
        .filter(|s| s.tag == "PIA")
        .filter_map(|s| s.component_str(1, 0))
        .collect()
}

/// Extract the Messlokations-ID from the first UTILMD transaction's IDE segment.
///
/// WiM UTILMD messages (55039, 55042, 55051, 55168) identify the Messlokation in
/// the transaction header via `IDE+24+<melo_id>:::Z19` rather than the LOC segment
/// convention used by GPKE messages.  The `wim_registry()` adapter extracts the
/// MeLo from the same `transactions()[0].ide.object_id` path.
///
/// Returns an empty string when the message is not a UTILMD or the IDE is absent.
pub fn extract_melo_from_utilmd(msg: &AnyMessage) -> String {
    let AnyMessage::Utilmd(u) = msg else {
        return String::new();
    };
    u.transactions()
        .first()
        .and_then(|t| t.ide.object_id.as_deref())
        .unwrap_or("")
        .to_owned()
}

/// Extract the original invoice message-reference from a REMADV for process correlation.
///
/// BDEW convention: the REMADV carries `RFF+Z13:<original_message_ref>` where the
/// reference value is the UNH message-reference (DE 0062) of the originating INVOIC.
/// This matches the key used when spawning the billing process (`extract_malo_from_invoic`).
///
/// Falls back to the REMADV's own `msg.message_ref()` when the RFF+Z13 is absent.
pub fn extract_invoice_ref_from_remadv(msg: &AnyMessage) -> String {
    let AnyMessage::Remadv(r) = msg else {
        return msg.message_ref().to_owned();
    };
    r.segments()
        .iter()
        .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some("Z13"))
        .and_then(|s| s.component_str(0, 1))
        .map(|s| s.to_owned())
        .unwrap_or_else(|| msg.message_ref().to_owned())
}

/// Extract the original invoice message-reference from a COMDIS for process correlation.
///
/// Same `RFF+Z13` convention as [`extract_invoice_ref_from_remadv`].
pub fn extract_invoice_ref_from_comdis(msg: &AnyMessage) -> String {
    let AnyMessage::Comdis(c) = msg else {
        return msg.message_ref().to_owned();
    };
    c.segments()
        .iter()
        .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some("Z13"))
        .and_then(|s| s.component_str(0, 1))
        .map(|s| s.to_owned())
        .unwrap_or_else(|| msg.message_ref().to_owned())
}

/// Detect the BDEW format version to use when spawning a new `WorkflowId`.
///
/// Reads the association-assigned release code from UNH element S009 component
/// DE 0057 (e.g. `"S2.1"`, `"G1.1"`, `"2.8e"`), then resolves the profile active
/// for that release code today via [`ReleaseRegistry::global`].  The profile's
/// `valid_from` date is used to build the `FormatVersion` key
/// (e.g. `"FV2025-10-01"`).
///
/// Falls back to `FV2025-10-01` when:
/// - The message is an `AnyMessage::Unknown` variant (no message type).
/// - The UNH association code is absent or empty.
/// - No profile is registered for the `(message_type, release)` pair.
/// - The profile exists but has no `valid_from` date (legacy undated profile).
///
/// The fallback is safe because:
/// 1. The FV only affects the `WorkflowId` name on the spawned process.
/// 2. Adapters use `is_known_fv`, which accepts all registered FVs.
/// 3. A running process keeps its original `WorkflowId` and is never re-versioned.
fn detect_format_version(msg: &AnyMessage) -> FormatVersion {
    // Derive the fallback dynamically from the registry so it stays current
    // across annual format-version cutovers without a code change.
    let fallback = || {
        adapters::known_fvs().into_iter().max().unwrap_or_else(|| {
            // Last-resort: if the registry is empty (pathological), use
            // the current production FV. This branch should never fire.
            FormatVersion::parse("FV2025-10-01")
                .expect("FV2025-10-01 is always a valid FormatVersion literal")
        })
    };

    let Some(message_type) = msg.try_message_type() else {
        return fallback();
    };
    let Ok(release) = msg.detect_release() else {
        return fallback();
    };

    let today = OffsetDateTime::now_utc().date();
    let Ok(profile) = ReleaseRegistry::global().profile_on(message_type, release, today) else {
        return fallback();
    };
    let Some(valid_from) = profile.valid_from() else {
        return fallback();
    };

    let fv_str = format!(
        "FV{:04}-{:02}-{:02}",
        valid_from.year(),
        valid_from.month() as u8,
        valid_from.day(),
    );
    FormatVersion::parse(&fv_str).unwrap_or_else(|_| fallback())
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
        "ingest dispatcher: no Phase 2 handler — message dead-lettered; \
         add a dispatch arm in ingest_dispatcher to handle this workflow",
    );
    Ok(IngestOutcome::Skipped {
        workflow_name: "unregistered",
        reason: "workflow_not_in_dispatch_table",
    })
}

// ── Per-family dispatch submodules ────────────────────────────────────────────

mod gabi_gas;
mod geli_gas;
mod gpke;
mod mabis;
mod redispatch;
mod wim;
mod wim_gas;
