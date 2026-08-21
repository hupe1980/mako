//! Deadline scheduler dispatch table.
//!
//! Maps workflow names to their `TimeoutExpired` command dispatch logic.
//! Adding a new workflow family requires adding one entry to the `match` in
//! [`dispatch_deadline`] — there is no other location to update.
//!
//! ## Compensation and atomicity
//!
//! All deadline commands are dispatched via
//! [`Process::execute_and_enqueue_with_retry`], which atomically persists
//! both events *and* any outbox entries (e.g. APERAK Ablehnung) produced by
//! the `TimeoutExpired` handler in a single `WriteBatch`.  This ensures
//! there is no window where the `DeadlineExpired` event is stored but the
//! outbound APERAK message is lost.
//!
//! Alternatively, [`Process::execute_timeout_with_retry`] may be used to
//! delegate the dispatch to `Workflow::on_deadline`, but this path is
//! equivalent when every workflow registers a `TimeoutExpired` command in its
//! `on_deadline` hook.
//!
//! ## Coverage check
//!
//! [`build_scheduler`] panics at startup if any workflow name registered by
//! an [`EngineModule`] via [`workflow_names`] is absent from the dispatch
//! table below.  This converts the silent regulatory miss (deadline fires,
//! drops into `unknown` branch, emits WARN) into an immediate startup failure
//! that blocks deployment.
//!
//! [`workflow_names`]: mako_engine::builder::EngineModule::workflow_names
//! [`EngineModule`]: mako_engine::builder::EngineModule

use std::sync::Arc;
use std::time::Duration;

use mako_engine::{
    builder::DeadlineScheduler,
    deadline::{Deadline, DeadlineStore},
    error::EngineError,
    ids::ProcessIdentity,
    process::Process,
};
use mako_gabi_gas::{
    DeliveryOrderCommand, GaBiGasAllocationWorkflow, GaBiGasDeliveryOrderWorkflow,
    GaBiGasInvoicCommand, GaBiGasInvoicWorkflow, GaBiGasNominationWorkflow, NominationCommand,
};
use mako_geli_gas::{
    GasSperrungLfCommand, GasSperrungNbCommand, GasSupplierChangeCommand, GeliGasDatanabrufCommand,
    GeliGasDatanabrufWorkflow, GeliGasLfStornierungWorkflow, GeliGasSperrprozesseInvoicCommand,
    GeliGasSperrprozesseInvoicWorkflow, GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow,
    GeliGasStornierungCommand, GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
    LfStornierungCommand,
};
use mako_gpke::{
    AbrechnungCommand, AbrechnungsdatenCommand as GpkeAbrechnungsdatenCommand,
    AllokationslisteCommand, AnfrageBestellungCommand, AnkuendigungZuordnungLfCommand,
    DatanabrufCommand, GpkeAbrechnungWorkflow, GpkeAbrechnungsdatenWorkflow,
    GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeBeendigungZuordnungWorkflow, GpkeDatanabrufWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeKonfigurationWorkflow, GpkeKuendigungWorkflow,
    GpkeLfAbmeldungWorkflow, GpkeLfAnmeldungWorkflow, GpkeNeuanlageWorkflow,
    GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, GpkeStornierungCommand, GpkeStornierungWorkflow,
    GpkeSupplierChangeWorkflow, KonfigurationAenderungCommand, KonfigurationCommand,
    LfAbmeldungCommand, LfAnmeldungCommand, NeuanlageCommand, SperrungCommand, SperrungLfCommand,
    SupplierChangeCommand, anfrage_bestellung::WORKFLOW_NAME as ANFRAGE_BESTELLUNG_WORKFLOW,
    ankuendigung_zuordnung_lf::WORKFLOW_NAME as ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW,
    lf_anmeldung::WORKFLOW_NAME as LF_ANMELDUNG_WORKFLOW,
    sperrung_lf::WORKFLOW_NAME as SPERRUNG_LF_WORKFLOW,
};
use mako_mabis::{BillingCommand, MabisBillingWorkflow};
use mako_redispatch::{
    ack_forward::{
        AckForwardCommand, KaskadeWorkflow, KostenblattWorkflow, NetzengpassWorkflow,
        PlanungsdatenWorkflow, StatusanfrageWorkflow, VerfuegbarkeitWorkflow,
        names::{KASKADE, KOSTENBLATT, NETZENGPASS, PLANUNGSDATEN, STATUSANFRAGE, VERFUEGBARKEIT},
    },
    aktivierung::{AktivierungCommand, AktivierungWorkflow, WORKFLOW_NAME as AKTIVIERUNG_WORKFLOW},
    stammdaten::{
        StammdatenCommand as RedispatchStammdatenCommand,
        StammdatenWorkflow as RedispatchStammdatenWorkflow, WORKFLOW_NAME as STAMMDATEN_WORKFLOW,
    },
};
use mako_wim::{
    DeviceChangeCommand, GeraeteubernahmeCommand, INSRPT_WORKFLOW_NAME as WIM_INSRPT_WORKFLOW,
    PreisanfrageCommand, PreislisteCommand, StammdatenCommand, SteuerungsauftragCommand,
    StorungsmeldungCommand, TechnikAenderungCommand, WimDeviceChangeWorkflow,
    WimGeraeteubernahmeWorkflow, WimInsrptWorkflow, WimInvoicCommand, WimInvoicWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimStammdatenWorkflow,
    WimSteuerungsauftragWorkflow, WimTechnikAenderungWorkflow,
};
use mako_wim_gas::{
    GasGeraeteubernahmeCommand, WimGasAnmeldungCommand, WimGasAnmeldungWorkflow,
    WimGasGeraeteubernahmeWorkflow, WimGasInsrptWorkflow, WimGasInvoicCommand,
    WimGasInvoicWorkflow, WimGasKuendigungCommand, WimGasKuendigungWorkflow,
    WimGasStornierungCommand, WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageWorkflow, insrpt::GasStorungsmeldungCommand,
};

use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::{SlateDbSnapshotStore, SlateDbStore};

// Command enums referenced by the dispatch table below. The table names each
// as `Enum::Variant` with bare identifiers so the generated struct-variant
// literal parses; these bring the module-qualified ones into scope.
use mako_geli_gas::GasStammdatenCommand;
use mako_gpke::{
    BeendigungZuordnungCommand, EogCommand, KuendigungCommand as GpkeKuendigungCommand,
    StammdatenCommand as GpkeStammdatenCommand,
};
use mako_wim::RechnungsabwicklungCommand;
use mako_wim::esa_wertebestellung::EsaWertebestellungCommand;
use mako_wim::wertebestellung::WertebestellungCommand;

/// Generate the deadline dispatch table and its `match` arms from one list.
///
/// Every entry names a workflow, its `Workflow` type, and the command variant
/// that carries an expired deadline. The macro emits both
/// [`DISPATCH_TABLE`] — which `assert_dispatch_coverage` checks at startup —
/// and the arms that actually run, so the two cannot disagree.
///
/// That was a real hazard: the table and the arms were maintained separately,
/// and a name present in the table but missing an arm passed startup validation
/// while its deadlines fell through to the catch-all, were logged, and were then
/// cancelled by the scheduler. A regulatory Frist would have expired with
/// nothing but a log line, on a service that reported full coverage.
///
/// Three groups, because "this workflow has no arm" needs to be a statement
/// rather than an omission:
///
/// - `timeout` — the standard shape: load the process, execute the command,
///   snapshot.
/// - `no_deadline` — workflows that register no deadline at all (pure
///   receive-and-record). Listed so their absence from the arms is deliberate.
/// - `custom` — handled by hand below, because their command carries extra
///   fields or the arm consults state before alerting.
macro_rules! deadline_dispatch {
    (
        timeout: { $( $name:expr => $Workflow:ty : $Enum:ident :: $Variant:ident ),+ $(,)? }
        no_deadline: [ $( $recv:expr ),* $(,)? ]
        custom: [ $( $custom:expr ),* $(,)? ]
    ) => {
        /// Every workflow name the deadline dispatcher handles.
        ///
        /// Generated from the `deadline_dispatch!` invocation, so it lists
        /// exactly the workflows that have an arm.
        /// [`assert_dispatch_coverage`] checks every registered workflow
        /// against it at startup.
        pub const DISPATCH_TABLE: &[&str] = &[ $( $name, )+ $( $recv, )* $( $custom, )* ];

        /// Run the standard timeout arm for `wf_name`, if it has one.
        ///
        /// `None` means the name is not in the `timeout` group; the caller then
        /// tries the `no_deadline` and `custom` groups.
        async fn dispatch_timeout(
            wf_name: &str,
            identity: ProcessIdentity,
            event_store: &Arc<SlateDbStore>,
            snap_store: &SlateDbSnapshotStore,
            snapshot_interval: u64,
            deadline_id: mako_engine::ids::DeadlineId,
            label: Box<str>,
        ) -> Option<Result<(), EngineError>> {
            $(
                if wf_name == $name {
                    let p = Process::<$Workflow, _>::from_identity(
                        Arc::clone(event_store),
                        identity,
                    );
                    return Some(async move {
                        p.execute_and_enqueue_with_retry(
                            $Enum::$Variant { deadline_id, label },
                            3,
                        )
                        .await?;
                        p.take_snapshot(snap_store, snapshot_interval).await.map(|_| ())
                    }.await);
                }
            )+
            let _ = (identity, event_store, snap_store, snapshot_interval, deadline_id, label);
            None
        }

        /// `true` when `wf_name` registers no deadline and therefore needs no arm.
        fn is_receipt_only(wf_name: &str) -> bool {
            [ $( $recv, )* ].contains(&wf_name)
        }
    };
}

deadline_dispatch! {
    timeout: {
    "gpke-supplier-change" => GpkeSupplierChangeWorkflow : SupplierChangeCommand::TimeoutExpired,
    "gpke-sperrung" => GpkeSperrungWorkflow : SperrungCommand::TimeoutExpired,
    "gpke-stornierung" => GpkeStornierungWorkflow : GpkeStornierungCommand::TimeoutExpired,
    LF_ANMELDUNG_WORKFLOW => GpkeLfAnmeldungWorkflow : LfAnmeldungCommand::TimeoutExpired,
    "gpke-konfiguration" => GpkeKonfigurationWorkflow : KonfigurationCommand::TimeoutExpired,
    "gpke-abrechnung" => GpkeAbrechnungWorkflow : AbrechnungCommand::TimeoutExpired,
    "wim-device-change" => WimDeviceChangeWorkflow : DeviceChangeCommand::TimeoutExpired,
    "wim-geraeteubernahme" => WimGeraeteubernahmeWorkflow : GeraeteubernahmeCommand::TimeoutExpired,
    "wim-stammdaten" => WimStammdatenWorkflow : StammdatenCommand::TimeoutExpired,
    mako_wim::wertebestellung::WORKFLOW_NAME => mako_wim::wertebestellung::WimWertebestellungWorkflow : WertebestellungCommand::TimeoutExpired,
    mako_wim::esa_wertebestellung::WORKFLOW_NAME => mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow : EsaWertebestellungCommand::TimeoutExpired,
    "wim-steuerungsauftrag" => WimSteuerungsauftragWorkflow : SteuerungsauftragCommand::TimeoutExpired,
    "geli-gas-supplier-change" => GeliGasSupplierChangeWorkflow : GasSupplierChangeCommand::TimeoutExpired,
    "geli-gas-stornierung" => GeliGasStornierungWorkflow : GeliGasStornierungCommand::TimeoutExpired,
    "geli-gas-stornierung-lf" => GeliGasLfStornierungWorkflow : LfStornierungCommand::TimeoutExpired,
    "geli-gas-datenabruf" => GeliGasDatanabrufWorkflow : GeliGasDatanabrufCommand::TimeoutExpired,
    "geli-gas-sperrung-lf" => GeliGasSperrungLfWorkflow : GasSperrungLfCommand::TimeoutExpired,
    "geli-gas-sperrung-nb" => GeliGasSperrungNbWorkflow : GasSperrungNbCommand::TimeoutExpired,
    "mabis-billing" => MabisBillingWorkflow : BillingCommand::PruefmitteilungDeadlineExpired,
    "gpke-neuanlage" => GpkeNeuanlageWorkflow : NeuanlageCommand::TimeoutExpired,
    "gpke-abrechnungsdaten" => GpkeAbrechnungsdatenWorkflow : GpkeAbrechnungsdatenCommand::TimeoutExpired,
    "gpke-lf-abmeldung" => GpkeLfAbmeldungWorkflow : LfAbmeldungCommand::TimeoutExpired,
    "gpke-beendigung-zuordnung" => GpkeBeendigungZuordnungWorkflow : BeendigungZuordnungCommand::TimeoutExpired,
    "gpke-kuendigung" => GpkeKuendigungWorkflow : GpkeKuendigungCommand::TimeoutExpired,
    "gpke-eog" => mako_gpke::GpkeEogWorkflow : EogCommand::TimeoutExpired,
    "gpke-stammdatenaenderung" => mako_gpke::GpkeStammdatenaenderungWorkflow : GpkeStammdatenCommand::TimeoutExpired,
    "geli-gas-stammdatenaenderung" => mako_geli_gas::GeliGasStammdatenaenderungWorkflow : GasStammdatenCommand::TimeoutExpired,
    ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW => GpkeAnkuendigungZuordnungLfWorkflow : AnkuendigungZuordnungLfCommand::TimeoutExpired,
    ANFRAGE_BESTELLUNG_WORKFLOW => GpkeAnfrageBestellungWorkflow : AnfrageBestellungCommand::TimeoutExpired,
    "wim-rechnungsabwicklung" => mako_wim::WimRechnungsabwicklungWorkflow : RechnungsabwicklungCommand::TimeoutExpired,
    "wim-preisanfrage" => WimPreisanfrageWorkflow : PreisanfrageCommand::TimeoutExpired,
    "wim-preisliste" => WimPreislisteWorkflow : PreislisteCommand::TimeoutExpired,
    "wim-invoic" => WimInvoicWorkflow : WimInvoicCommand::TimeoutExpired,
    "wim-gas-geraeteubernahme" => WimGasGeraeteubernahmeWorkflow : GasGeraeteubernahmeCommand::TimeoutExpired,
    "wim-gas-anmeldung" => WimGasAnmeldungWorkflow : WimGasAnmeldungCommand::TimeoutExpired,
    "wim-gas-kuendigung" => WimGasKuendigungWorkflow : WimGasKuendigungCommand::TimeoutExpired,
    "wim-gas-verpflichtungsanfrage" => WimGasVerpflichtungsanfrageWorkflow : WimGasVerpflichtungsanfrageCommand::TimeoutExpired,
    "wim-gas-invoic" => WimGasInvoicWorkflow : WimGasInvoicCommand::TimeoutExpired,
    "wim-gas-stornierung" => WimGasStornierungWorkflow : WimGasStornierungCommand::TimeoutExpired,
    "gabi-gas-invoic" => GaBiGasInvoicWorkflow : GaBiGasInvoicCommand::TimeoutExpired,
    "geli-gas-sperrprozesse-invoic" => GeliGasSperrprozesseInvoicWorkflow : GeliGasSperrprozesseInvoicCommand::TimeoutExpired,
    STAMMDATEN_WORKFLOW => RedispatchStammdatenWorkflow : RedispatchStammdatenCommand::TimeoutExpired,
    AKTIVIERUNG_WORKFLOW => AktivierungWorkflow : AktivierungCommand::TimeoutExpired,
    VERFUEGBARKEIT => VerfuegbarkeitWorkflow : AckForwardCommand::TimeoutExpired,
    NETZENGPASS => NetzengpassWorkflow : AckForwardCommand::TimeoutExpired,
    KASKADE => KaskadeWorkflow : AckForwardCommand::TimeoutExpired,
    PLANUNGSDATEN => PlanungsdatenWorkflow : AckForwardCommand::TimeoutExpired,
    STATUSANFRAGE => StatusanfrageWorkflow : AckForwardCommand::TimeoutExpired,
    KOSTENBLATT => KostenblattWorkflow : AckForwardCommand::TimeoutExpired,
    SPERRUNG_LF_WORKFLOW => GpkeSperrungLfWorkflow : SperrungLfCommand::TimeoutExpired,
    WIM_INSRPT_WORKFLOW => WimInsrptWorkflow : StorungsmeldungCommand::TimeoutExpired,
    "gpke-konfiguration-aenderung" => GpkeKonfigurationAenderungWorkflow : KonfigurationAenderungCommand::TimeoutExpired,
    "gpke-datenabruf" => GpkeDatanabrufWorkflow : DatanabrufCommand::TimeoutExpired,
    "gpke-allokationsliste" => GpkeAllokationslisteWorkflow : AllokationslisteCommand::TimeoutExpired,
    "wim-technik-aenderung" => WimTechnikAenderungWorkflow : TechnikAenderungCommand::TimeoutExpired,
    }
    no_deadline: [
        // Pure receive-and-record workflows: they record an inbound message and
        // register no Frist, so there is nothing for a timer to do.
        "geli-gas-partin",
        "mabis-clearingliste",
        "mabis-listenabgleich",
        "mabis-anforderung",
        "mabis-zp-lifecycle",
        // MMMA delegates delivery to gpke-allokationsliste; SCHEDL, IMBNOT and
        // TRANOT are DVGW notifications with no response obligation.
        "gabi-gas-mmma",
        "gabi-gas-schedl",
        "gabi-gas-imbnot",
        "gabi-gas-tranot",
        // Registered by an EngineModule but carrying no deadline of their own.
        mako_geli_gas::GAS_MSCONS_WORKFLOW_NAME,
        mako_gpke::messwerte::WORKFLOW_NAME,
        mako_gpke::partin::WORKFLOW_NAME,
        mako_gpke::utilts::WORKFLOW_NAME,
    ]
    custom: [
        // Commands with extra fields, or arms that consult state before
        // alerting. Written out in `dispatch_deadline` below.
        "wim-gas-insrpt",
        "gabi-gas-nomination",
        "gabi-gas-delivery-order",
        "gabi-gas-allocation",
        // Delivery-window markers: no workflow, only a regulatory alert.
        "contrl-ack-obligation",
    ]
}

/// Dispatch a fired `deadline` to the correct workflow's `TimeoutExpired` command.
///
/// After a successful execute, takes a snapshot if the stream has accumulated
/// a multiple of `snapshot_interval` events (auto-snapshot.
///
/// Returns `Ok(())` on success or non-conflict failure (the scheduler will
/// cancel the deadline after this call). Returns `Err(EngineError::VersionConflict)`
/// if the process was concurrently modified; the scheduler retries.
pub async fn dispatch_deadline(
    deadline: Deadline,
    event_store: Arc<SlateDbStore>,
    snap_store: SlateDbSnapshotStore,
    snapshot_interval: u64,
) -> Result<(), EngineError> {
    let wf_name = deadline.workflow_id().name.as_ref();
    let identity = ProcessIdentity::new(
        deadline.process_id(),
        deadline.tenant_id(),
        deadline.workflow_id().clone(),
    );
    let deadline_id = deadline.deadline_id();
    let label: Box<str> = deadline.label().into();

    // Derive the process family label from the workflow name prefix for metrics.
    // "gpke-supplier-change" → "gpke", "wim-device-change" → "wim", etc.
    let family = wf_name.split('-').next().unwrap_or(wf_name);

    // ── APERAK Strom 45-minute sending-window obligation (APERAK AHB 1.0 §2.4.1) ──
    //
    // Reaching this point means the window was still registered when it came
    // due: `OutboxWorker::discharge_delivery_window` retires it the moment the
    // APERAK is delivered, so an undischarged window is an undelivered APERAK.
    // That is a BNetzA regulatory compliance violation.
    //
    // The alert is unconditional *because* of that discharge — without it the
    // window would outlive every obligation it monitors and fire on every Strom
    // process, answered or not.
    //
    // This label is purely a monitoring marker: it does NOT carry a workflow
    // command.  Log the alert and return early — do NOT dispatch TimeoutExpired
    // to the process (the process is waiting for an AS4 delivery, not a timer).
    if label.as_ref() == mako_fristen::APERAK_STROM_WINDOW_LABEL {
        tracing::error!(
            deadline_id = %deadline_id,
            workflow    = %wf_name,
            "REGULATORY ALERT: APERAK 45-minute Strom sending-window expired \
             (APERAK AHB 1.0 §2.4.1). The outbound APERAK was not delivered \
             within 45 minutes of receipt (or by Sunday 12:00 on Saturday). \
             Check the OutboxWorker health and AS4 transport immediately.",
        );
        return Ok(());
    }

    // Standard timeout arms come from the `deadline_dispatch!` table above.
    let result = if let Some(r) = dispatch_timeout(
        wf_name,
        identity,
        &event_store,
        &snap_store,
        snapshot_interval,
        deadline_id,
        label.clone(),
    )
    .await
    {
        r
    } else if is_receipt_only(wf_name) {
        // Declared in the `no_deadline` group: the workflow records inbound
        // messages and registers no Frist, so a fired deadline here can only be
        // a leftover from an earlier registration. Nothing to do.
        tracing::debug!(
            deadline_id = %deadline_id,
            workflow    = %wf_name,
            "deadline fired for a receipt-only workflow — no action",
        );
        Ok(())
    } else {
        // The `custom` group: commands carrying extra fields, and arms that
        // consult process state before raising a regulatory alert.
        let identity = ProcessIdentity::new(
            deadline.process_id(),
            deadline.tenant_id(),
            deadline.workflow_id().clone(),
        );
        match wf_name {
            "wim-gas-insrpt" => {
                let p = Process::<WimGasInsrptWorkflow, _>::from_identity(
                    Arc::clone(&event_store),
                    identity,
                );
                p.execute_and_enqueue_with_retry(
                    GasStorungsmeldungCommand::TimeoutExpired {
                        deadline_id,
                        label,
                        outbox: None,
                    },
                    3,
                )
                .await?;
                p.take_snapshot(&snap_store, snapshot_interval)
                    .await
                    .map(|_| ())
            }
            "gabi-gas-nomination" => {
                // NOMRES response deadline — no response from FNB/MGV before D-1 15:00.
                let p = Process::<GaBiGasNominationWorkflow, _>::from_identity(
                    Arc::clone(&event_store),
                    identity,
                );
                p.execute_and_enqueue_with_retry(
                    NominationCommand::NomresDeadlineExpired {
                        deadline_id,
                        label: label.into(),
                    },
                    3,
                )
                .await?;
                p.take_snapshot(&snap_store, snapshot_interval)
                    .await
                    .map(|_| ())
            }
            "gabi-gas-delivery-order" => {
                // DELRES response deadline — no DELRES received from FNB/MGV before deadline.
                let p = Process::<GaBiGasDeliveryOrderWorkflow, _>::from_identity(
                    Arc::clone(&event_store),
                    identity,
                );
                p.execute_and_enqueue_with_retry(
                    DeliveryOrderCommand::DelresDeadlineExpired {
                        deadline_id,
                        label: label.into(),
                    },
                    3,
                )
                .await?;
                p.take_snapshot(&snap_store, snapshot_interval)
                    .await
                    .map(|_| ())
            }
            "gabi-gas-allocation" => {
                // KoV §6.4 final-allocation window (end of month M+2, 12:00 CET).
                //
                // The window is registered when the *first* ALOCAT for a gas day
                // arrives and is never cancelled, so this deadline fires for every
                // gas day — including the ones that settled normally. Whether the
                // obligation was actually missed is a question only the state can
                // answer, so go through `execute_timeout_with_retry`: it consults
                // `on_deadline`, which returns `None` for a settled or
                // already-overdue stream. Alerting before that check would page the
                // operator on the healthiest path there is.
                let p = Process::<GaBiGasAllocationWorkflow, _>::from_identity(
                    Arc::clone(&event_store),
                    identity,
                );
                let fired = p.execute_timeout_with_retry(&deadline, 3).await?;
                if fired.is_some_and(|events| !events.is_empty()) {
                    tracing::error!(
                        deadline_id = %deadline_id,
                        label       = %label,
                        "REGULATORY ALERT: GaBi Gas final-allocation window expired \
                         (KoV §6.4) — no binding final ALOCAT was received for this gas \
                         day by the end of month M+2. The imbalance cannot be settled; \
                         raise a Clearingfall with the FNB/MGV.",
                    );
                } else {
                    tracing::debug!(
                        deadline_id = %deadline_id,
                        "gabi-gas-allocation: final-allocation window closed on a \
                         settled gas day — no action",
                    );
                }
                p.take_snapshot(&snap_store, snapshot_interval)
                    .await
                    .map(|_| ())
            }
            "contrl-ack-obligation" => {
                tracing::error!(
                    deadline_id = %deadline_id,
                    label       = %label,
                    "REGULATORY ALERT: CONTRL 6h delivery window expired \
                     (CONTRL AHB 1.0 §2.3.1) — the Gas CONTRL Empfangsbestätigung \
                     was NOT delivered within 6 hours of receipt. \
                     Inspect the outbox for stuck messages and trigger manual re-delivery.",
                );
                Ok(())
            }
            unknown => {
                tracing::error!(
                    deadline_id  = %deadline_id,
                    workflow     = %unknown,
                    label        = %label,
                    "deadline scheduler: no dispatch arm for this workflow — \
                     deadline dropped. Add it to the deadline_dispatch! table.",
                );
                Ok(())
            }
        }
    };

    // Increment the per-family deadline-fired counter on successful dispatch.
    if result.is_ok() {
        EngineMetrics::global().deadline_fired(family);
    }

    result
}

/// Assert that every workflow in `registered` has a dispatch-table entry.
///
/// # Panics
///
/// Panics with an actionable message when a workflow name declared by an
/// `EngineModule` is absent from [`DISPATCH_TABLE`].  Call this at startup
/// before spawning the scheduler so missing entries are caught immediately.
pub fn assert_dispatch_coverage(registered: &[&str]) {
    for &wf in registered {
        if !DISPATCH_TABLE.contains(&wf) {
            panic!(
                "deadline_dispatch: workflow '{wf}' is registered by an EngineModule but has \
                 no entry in the dispatch table (deadline_dispatch::DISPATCH_TABLE). \
                 Add a match arm to dispatch_deadline() and add the name to DISPATCH_TABLE.",
            );
        }
    }
}

/// Build the deadline scheduler and verify dispatch coverage at startup.
///
/// # Panics
///
/// Panics when a workflow name declared by a registered `EngineModule` via
/// [`workflow_names`] is not covered by [`dispatch_deadline`]. This converts
/// a silent regulatory miss into an immediate startup failure.
///
/// [`workflow_names`]: mako_engine::builder::EngineModule::workflow_names
pub fn build_scheduler<SS, OS, DS, PR>(
    ctx: &mako_engine::builder::EngineContext<SlateDbStore, SS, OS, DS, PR>,
    event_store: Arc<SlateDbStore>,
    snapshot_interval: u64,
    poll_interval: Duration,
) -> DeadlineScheduler<DS>
where
    DS: DeadlineStore + Clone,
    SS: mako_engine::snapshot::SnapshotStore,
    OS: mako_engine::outbox::OutboxStore,
    PR: mako_engine::registry::ProcessRegistry,
{
    assert_dispatch_coverage(ctx.registered_workflows());

    ctx.run_deadline_scheduler(
        move |deadline| {
            let es = Arc::clone(&event_store);
            let ss = event_store.as_snapshot_store();
            Box::pin(async move { dispatch_deadline(deadline, es, ss, snapshot_interval).await })
        },
        100,
        poll_interval,
    )
}

#[cfg(test)]
mod dispatch_table_tests {
    /// A workflow may appear in exactly one group of the `deadline_dispatch!`
    /// invocation.
    ///
    /// The three groups are tried in order — `timeout`, then `no_deadline`,
    /// then `custom` — so a name listed twice silently takes the earlier
    /// behaviour. A workflow that grows a real Frist but keeps its old
    /// `no_deadline` entry would look dispatched and do nothing.
    #[test]
    fn no_workflow_appears_in_two_groups() {
        let mut seen = std::collections::HashSet::new();
        let dupes: Vec<&str> = super::DISPATCH_TABLE
            .iter()
            .filter(|n| !seen.insert(**n))
            .copied()
            .collect();
        assert!(
            dupes.is_empty(),
            "these workflows are listed in more than one deadline_dispatch! group: {dupes:?}"
        );
    }

    /// Every name `is_receipt_only` accepts must be in the table, and every
    /// name it rejects must be handled elsewhere.
    ///
    /// `is_receipt_only` is generated from the `no_deadline` group while the
    /// table is generated from all three, so a name drifting out of the group
    /// without leaving the table would silently move from "no action" to the
    /// `custom` catch-all, which logs a regulatory alert. The classification is
    /// what decides between those, so it is asserted directly rather than
    /// inferred from the table.
    #[test]
    fn the_receipt_only_group_is_a_subset_of_the_table() {
        let receipt_only: Vec<&str> = super::DISPATCH_TABLE
            .iter()
            .copied()
            .filter(|n| super::is_receipt_only(n))
            .collect();
        assert!(
            !receipt_only.is_empty(),
            "the no_deadline group is non-empty, so is_receipt_only must accept \
             at least one table entry — an always-false predicate would send \
             every receive-and-record workflow to the custom catch-all",
        );
        for name in receipt_only {
            assert!(
                super::DISPATCH_TABLE.contains(&name),
                "{name} is classified receipt-only but is absent from DISPATCH_TABLE, \
                 so assert_dispatch_coverage would not catch its removal",
            );
        }
    }

    /// A name in no group at all must not be silently accepted.
    #[test]
    fn an_unknown_workflow_is_not_receipt_only() {
        assert!(
            !super::is_receipt_only("definitely-not-a-workflow"),
            "is_receipt_only must reject unknown names; accepting them would turn \
             a missing dispatch arm into a silent no-op instead of an alert",
        );
    }
}
