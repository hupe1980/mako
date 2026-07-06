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
    AbrechnungCommand, AllokationslisteCommand, AnfrageBestellungCommand,
    AnkuendigungZuordnungLfCommand, DatanabrufCommand, GpkeAbrechnungWorkflow,
    GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeDatanabrufWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeNeuanlageWorkflow, GpkeSperrungLfWorkflow, GpkeSperrungWorkflow,
    GpkeStornierungCommand, GpkeStornierungWorkflow, GpkeSupplierChangeWorkflow,
    KonfigurationAenderungCommand, KonfigurationCommand, LfAbmeldungCommand, LfAnmeldungCommand,
    NeuanlageCommand, SperrungCommand, SperrungLfCommand, SupplierChangeCommand,
    anfrage_bestellung::WORKFLOW_NAME as ANFRAGE_BESTELLUNG_WORKFLOW,
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
    StornierungCommand, StorungsmeldungCommand, TechnikAenderungCommand, WimDeviceChangeWorkflow,
    WimGeraeteubernahmeWorkflow, WimInsrptWorkflow, WimPreisanfrageWorkflow, WimPreislisteWorkflow,
    WimRechnungCommand, WimRechnungWorkflow, WimStammdatenWorkflow, WimSteuerungsauftragWorkflow,
    WimStornierungWorkflow, WimTechnikAenderungWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungWorkflow, WimGasInsrptWorkflow, WimGasInvoicCommand,
    WimGasInvoicWorkflow, WimGasKuendigungCommand, WimGasKuendigungWorkflow,
    WimGasStornierungCommand, WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageWorkflow, insrpt::GasStorungsmeldungCommand,
};

use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::{SlateDbSnapshotStore, SlateDbStore};

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

    let result = match wf_name {
        "gpke-supplier-change" => {
            let p = Process::<GpkeSupplierChangeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SupplierChangeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-sperrung" => {
            let p = Process::<GpkeSperrungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SperrungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-stornierung" => {
            let p = Process::<GpkeStornierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GpkeStornierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        LF_ANMELDUNG_WORKFLOW => {
            let p = Process::<GpkeLfAnmeldungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                LfAnmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-konfiguration" => {
            let p = Process::<GpkeKonfigurationWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                KonfigurationCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-abrechnung" => {
            let p = Process::<GpkeAbrechnungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AbrechnungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-device-change" => {
            let p = Process::<WimDeviceChangeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                DeviceChangeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-geraeteubernahme" => {
            let p = Process::<WimGeraeteubernahmeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GeraeteubernahmeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-stammdaten" => {
            let p = Process::<WimStammdatenWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                StammdatenCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-stornierung" => {
            let p = Process::<WimStornierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                StornierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-steuerungsauftrag" => {
            let p = Process::<WimSteuerungsauftragWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SteuerungsauftragCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-supplier-change" => {
            let p = Process::<GeliGasSupplierChangeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GasSupplierChangeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-stornierung" => {
            let p = Process::<GeliGasStornierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GeliGasStornierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-stornierung-lf" => {
            let p = Process::<GeliGasLfStornierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                LfStornierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-datenabruf" => {
            let p = Process::<GeliGasDatanabrufWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GeliGasDatanabrufCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-sperrung-lf" => {
            let p = Process::<GeliGasSperrungLfWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GasSperrungLfCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-sperrung-nb" => {
            let p = Process::<GeliGasSperrungNbWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GasSperrungNbCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-partin" => {
            // Gas PARTIN processes are simple receipts with no deadline obligation.
            // This arm exists solely to satisfy assert_dispatch_coverage.
            tracing::debug!(
                deadline_id = %deadline_id,
                "geli-gas-partin: no deadline action (simple receipt workflow)",
            );
            Ok(())
        }
        "mabis-clearingliste" => {
            // MaBiS Clearingliste processes (PIDs 55065/55069/55070) are simple
            // receive-and-record workflows with no deadline obligation.
            // This arm exists solely to satisfy assert_dispatch_coverage.
            tracing::debug!(
                deadline_id = %deadline_id,
                "mabis-clearingliste: no deadline action (simple receipt workflow)",
            );
            Ok(())
        }
        "mabis-billing" => {
            let p = Process::<MabisBillingWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                BillingCommand::PruefmitteilungDeadlineExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-neuanlage" => {
            let p = Process::<GpkeNeuanlageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                NeuanlageCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-lf-abmeldung" => {
            let p = Process::<GpkeLfAbmeldungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                LfAbmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW => {
            let p = Process::<GpkeAnkuendigungZuordnungLfWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AnkuendigungZuordnungLfCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        ANFRAGE_BESTELLUNG_WORKFLOW => {
            let p = Process::<GpkeAnfrageBestellungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AnfrageBestellungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-preisanfrage" => {
            let p = Process::<WimPreisanfrageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                PreisanfrageCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-preisliste" => {
            let p = Process::<WimPreislisteWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                PreislisteCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-rechnung" => {
            let p = Process::<WimRechnungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimRechnungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-anmeldung" => {
            let p = Process::<WimGasAnmeldungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasAnmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-kuendigung" => {
            let p = Process::<WimGasKuendigungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasKuendigungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-verpflichtungsanfrage" => {
            let p = Process::<WimGasVerpflichtungsanfrageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasVerpflichtungsanfrageCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-invoic" => {
            let p = Process::<WimGasInvoicWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasInvoicCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-stornierung" => {
            let p = Process::<WimGasStornierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasStornierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
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
        "gabi-gas-invoic" => {
            let p = Process::<GaBiGasInvoicWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GaBiGasInvoicCommand::TimeoutExpired { deadline_id, label },
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
        "gabi-gas-allocation" => {
            // ALOCAT is a simple receive-and-record workflow with no deadline obligation.
            // This arm exists solely to satisfy assert_dispatch_coverage.
            tracing::debug!(
                deadline_id = %deadline_id,
                "gabi-gas-allocation: no deadline action (simple receipt workflow)",
            );
            let _ = Process::<GaBiGasAllocationWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            Ok(())
        }
        "gabi-gas-schedl" => {
            // SCHEDL is a simple receive-and-record workflow with no deadline obligation.
            // This arm exists solely to satisfy assert_dispatch_coverage.
            tracing::debug!(
                deadline_id = %deadline_id,
                "gabi-gas-schedl: no deadline action (simple receipt workflow)",
            );
            Ok(())
        }
        "gabi-gas-imbnot" => {
            // IMBNOT is a simple receive-and-record workflow with no deadline obligation.
            // This arm exists solely to satisfy assert_dispatch_coverage.
            tracing::debug!(
                deadline_id = %deadline_id,
                "gabi-gas-imbnot: no deadline action (simple receipt workflow)",
            );
            Ok(())
        }
        "gabi-gas-tranot" => {
            // TRANOT is a simple receive-and-record workflow with no deadline obligation.
            // This arm exists solely to satisfy assert_dispatch_coverage.
            tracing::debug!(
                deadline_id = %deadline_id,
                "gabi-gas-tranot: no deadline action (simple receipt workflow)",
            );
            Ok(())
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
        "geli-gas-sperrprozesse-invoic" => {
            let p = Process::<GeliGasSperrprozesseInvoicWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GeliGasSperrprozesseInvoicCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        // ── Redispatch 2.0 workflows ──────────────────────────────────────────
        // Clocks: ACK/Activation windows are UTC wall-clock hours.
        // Stammdaten-forwarding and Kostenblatt use German local time (Werktage).
        STAMMDATEN_WORKFLOW => {
            let p = Process::<RedispatchStammdatenWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                RedispatchStammdatenCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        AKTIVIERUNG_WORKFLOW => {
            let p = Process::<AktivierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AktivierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        VERFUEGBARKEIT => {
            let p = Process::<VerfuegbarkeitWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AckForwardCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        NETZENGPASS => {
            let p = Process::<NetzengpassWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AckForwardCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        KASKADE => {
            let p =
                Process::<KaskadeWorkflow, _>::from_identity(Arc::clone(&event_store), identity);
            p.execute_and_enqueue_with_retry(
                AckForwardCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        PLANUNGSDATEN => {
            let p = Process::<PlanungsdatenWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AckForwardCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        STATUSANFRAGE => {
            let p = Process::<StatusanfrageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AckForwardCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        KOSTENBLATT => {
            let p = Process::<KostenblattWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AckForwardCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        SPERRUNG_LF_WORKFLOW => {
            let p = Process::<GpkeSperrungLfWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SperrungLfCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        WIM_INSRPT_WORKFLOW => {
            let p =
                Process::<WimInsrptWorkflow, _>::from_identity(Arc::clone(&event_store), identity);
            p.execute_and_enqueue_with_retry(
                StorungsmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-konfiguration-aenderung" => {
            let p = Process::<GpkeKonfigurationAenderungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                KonfigurationAenderungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-datenabruf" => {
            let p = Process::<GpkeDatanabrufWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                DatanabrufCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-allokationsliste" => {
            let p = Process::<GpkeAllokationslisteWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AllokationslisteCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-technik-aenderung" => {
            let p = Process::<WimTechnikAenderungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                TechnikAenderungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        // CONTRL 6h delivery-window obligation (CONTRL AHB 1.0 §2.3.1).
        // Registered by ContrlAckService; fires if the OutboxWorker has not
        // delivered the CONTRL Empfangsbestätigung within 6 hours.
        // There is no domain workflow to retry — log a regulatory alert so the
        // operator can investigate and manually trigger a re-delivery.
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
                "deadline scheduler: dispatch table has no entry for this workflow — \
                 deadline dropped; add a match arm to deadline_dispatch::dispatch_deadline",
            );
            Ok(())
        }
    };

    // Increment the per-family deadline-fired counter on successful dispatch.
    if result.is_ok() {
        EngineMetrics::global().deadline_fired(family);
    }

    result
}

/// All workflow names known to the deadline dispatcher.
///
/// Every name returned by an `EngineModule::workflow_names` implementation
/// that is registered with the production engine **must** appear here.
/// [`assert_dispatch_coverage`] enforces this at startup.
pub const DISPATCH_TABLE: &[&str] = &[
    "gpke-supplier-change",
    LF_ANMELDUNG_WORKFLOW,
    "gpke-sperrung",
    SPERRUNG_LF_WORKFLOW,
    "gpke-stornierung",
    "gpke-abrechnung",
    "gpke-konfiguration",
    "gpke-neuanlage",
    "gpke-lf-abmeldung",
    ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW,
    ANFRAGE_BESTELLUNG_WORKFLOW,
    "wim-device-change",
    "wim-geraeteubernahme",
    "wim-stammdaten",
    "wim-stornierung",
    "wim-steuerungsauftrag",
    "wim-preisanfrage",
    "wim-preisliste",
    "wim-rechnung",
    WIM_INSRPT_WORKFLOW,
    "gpke-konfiguration-aenderung",
    "gpke-datenabruf",
    "gpke-allokationsliste",
    "wim-technik-aenderung",
    // Simple-receipt workflows (no deadline; in DISPATCH_TABLE to satisfy assert_dispatch_coverage)
    mako_gpke::messwerte::WORKFLOW_NAME,
    mako_gpke::partin::WORKFLOW_NAME,
    mako_gpke::utilts::WORKFLOW_NAME,
    // Note: gpke-enfg has been removed; EnFG IFTSTA PIDs 21043/21044 now route to
    // gpke-konfiguration-aenderung (already in DISPATCH_TABLE above with a deadline),
    // and 21045/21047 route to gpke-supplier-change (also has a deadline).
    mako_geli_gas::GAS_MSCONS_WORKFLOW_NAME,
    mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
    mako_geli_gas::GELI_GAS_SPERRUNG_LF_WORKFLOW_NAME,
    "geli-gas-sperrung-nb",
    "geli-gas-supplier-change",
    "geli-gas-stornierung",
    "geli-gas-stornierung-lf",
    "geli-gas-partin",
    "mabis-billing",
    "mabis-clearingliste",
    "wim-gas-anmeldung",
    "wim-gas-kuendigung",
    "wim-gas-verpflichtungsanfrage",
    "wim-gas-invoic",
    "wim-gas-stornierung",
    "wim-gas-insrpt",
    "gabi-gas-invoic",
    "gabi-gas-nomination",
    "gabi-gas-allocation",
    "gabi-gas-schedl",
    "gabi-gas-imbnot",
    "gabi-gas-tranot",
    "gabi-gas-delivery-order",
    "geli-gas-sperrprozesse-invoic",
    // ── Redispatch 2.0 ───────────────────────────────────────────────────────
    STAMMDATEN_WORKFLOW,
    AKTIVIERUNG_WORKFLOW,
    VERFUEGBARKEIT,
    NETZENGPASS,
    KASKADE,
    PLANUNGSDATEN,
    STATUSANFRAGE,
    KOSTENBLATT,
];

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
