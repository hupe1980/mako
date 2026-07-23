//! Online in-flight process migration API.
//!
//! Exposes `POST /admin/migrations` — a bearer-token-protected endpoint that
//! migrates all in-flight process streams from one BDEW format version to
//! another **while the daemon is live**. No downtime is required.
//!
//! # Why online migration (not a CLI subcommand)?
//!
//! `makod` holds an **exclusive lock** on its `--data-dir` via SlateDB's
//! embedded-database lock protocol. A separate `makod migrate` binary would
//! fail to open the same store path while the daemon is running.
//!
//! Running migration as an in-process HTTP handler avoids the lock entirely:
//! the handler uses the daemon's already-open store handles. SlateDB's
//! Serializable Snapshot Isolation ensures that concurrent `execute_and_enqueue`
//! calls on unrelated streams do not conflict with the migration scan. The
//! migration only writes **snapshots** (not events), so there is no
//! version-conflict window.
//!
//! # Deployment sequence
//!
//! ```text
//! 1. Deploy new binary (both FVs registered in adapter registry)   ← daemon stays live
//! 2. POST /admin/migrations {"from":"FV2025-10-01","to":"FV2026-10-01"}
//! 3. Assert MigrateResponse.errors == []                           ← zero errors required
//! 4. Remove old FV from adapter config, redeploy                   ← normal rolling restart
//! ```
//!
//! # Authentication
//!
//! All `/admin/migrations` endpoints are protected by Cedar ABAC authorization.
//! The caller's principal must be permitted the relevant action in the active
//! Cedar policy set. Never mount this router on the public API-Webdienste port.
//!
//! # Supported FV transitions
//!
//! The endpoint dispatches a compile-time table of [`IdentityMigration`] runners
//! (one per registered workflow family) for each known `(from, to)` FV pair.
//! For transitions where a workflow state schema changed, replace the
//! `IdentityMigration` entry with a bespoke `StateMigration` implementation in
//! the relevant domain crate and update the dispatch table here.
//!
//! Add a new `(from, to)` arm each October release cycle.
//!
//! [`IdentityMigration`]: mako_engine::migration::IdentityMigration

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::post,
};
use mako_engine::store_slatedb::SlateDbStore;
use mako_engine::{
    migration::{IdentityMigration, MigrationReport, MigrationRunner},
    version::WorkflowId,
};
use mako_gabi_gas::GaBiGasInvoicWorkflow;
use mako_geli_gas::{GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow};
use mako_gpke::{
    GpkeAbrechnungWorkflow, GpkeAnfrageBestellungWorkflow, GpkeKonfigurationWorkflow,
    GpkeLfAbmeldungWorkflow, GpkeLfAnmeldungWorkflow, GpkeNeuanlageWorkflow, GpkeSperrungWorkflow,
    GpkeStornierungWorkflow, GpkeSupplierChangeWorkflow,
};
use mako_mabis::MabisBillingWorkflow;
use mako_redispatch::{
    ack_forward::{
        KaskadeWorkflow, KostenblattWorkflow, NetzengpassWorkflow, PlanungsdatenWorkflow,
        StatusanfrageWorkflow, VerfuegbarkeitWorkflow,
        names::{KASKADE, KOSTENBLATT, NETZENGPASS, PLANUNGSDATEN, STATUSANFRAGE, VERFUEGBARKEIT},
    },
    aktivierung::{AktivierungWorkflow, WORKFLOW_NAME as AKTIVIERUNG_WORKFLOW},
    stammdaten::{
        StammdatenWorkflow as RedispatchStammdatenWorkflow, WORKFLOW_NAME as STAMMDATEN_WORKFLOW,
    },
};
use mako_wim::{
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimPreisanfrageWorkflow,
    WimPreislisteWorkflow, WimRechnungWorkflow, WimStammdatenWorkflow,
    WimSteuerungsauftragWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungWorkflow, WimGasInvoicWorkflow, WimGasKuendigungWorkflow,
    WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageWorkflow,
};
use serde::{Deserialize, Serialize};

use crate::cedar_authz::CedarAuthorizer;

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the migration admin API.
pub struct MigrationApiState {
    pub store: Arc<SlateDbStore>,
    /// Cedar-based authorization engine.
    pub cedar: Arc<CedarAuthorizer>,
    /// Operator tenant (GLN) — the Cedar resource scope.
    pub tenant: String,
}

/// Every `(from, to)` FV pair that is registered in [`dispatch_migrations`].
///
/// **Maintenance rule:** whenever a new `match` arm is added to
/// `dispatch_migrations`, the corresponding `(from, to)` pair must also be
/// added here.  The `migration_dispatch_table_covers_active_fv_transitions`
/// integration test panics if any active transition is absent from this list.
///
/// Add a new entry each October release cycle.
#[allow(dead_code)] // used by integration tests via the lib target
pub const KNOWN_FV_TRANSITIONS: &[(&str, &str)] = &[("FV2025-10-01", "FV2026-10-01")];

// ── Request / response types ──────────────────────────────────────────────────

/// `POST /admin/migrations` request body.
#[derive(Debug, Deserialize)]
pub struct MigrateRequest {
    /// Source BDEW format version (e.g. `"FV2025-10-01"`).
    pub from: String,
    /// Target BDEW format version (e.g. `"FV2026-10-01"`).
    pub to: String,
}

/// `POST /admin/migrations` response.
#[derive(Debug, Serialize)]
pub struct MigrateResponse {
    /// Source format version that was migrated from.
    pub from: String,
    /// Target format version that was migrated to.
    pub to: String,
    /// Total streams successfully migrated (snapshotted under new `workflow_id`).
    pub migrated: usize,
    /// Total streams skipped (wrong `workflow_id`, empty, or already migrated).
    pub skipped: usize,
    /// Streams that failed migration. Non-empty means action is required.
    pub errors: Vec<String>,
    /// Number of workflow-family runners executed.
    pub runners_executed: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ── Auth helper ───────────────────────────────────────────────────────────────

// (Auth is handled via CedarAuthorizer in the handler directly.)

// ── Migration dispatch ────────────────────────────────────────────────────────

/// Run all workflow-family migrations for the given `(from, to)` FV pair.
///
/// Returns `None` when the pair is not recognised (unknown FV transition).
/// Returns `Some((report, runners_executed))` on success.
///
/// # Adding a new annual release
///
/// 1. Add a new `("FV20XX-10-01", "FV20YY-10-01") =>` arm.
/// 2. For each workflow whose state schema **did not change**, the
///    existing `identity!` entry is correct.
/// 3. For workflows where the state type changed, replace the `identity!`
///    call with a dedicated `StateMigration` implementation in the domain
///    crate and construct a `MigrationRunner` with it here.
pub async fn dispatch_migrations(
    from: &str,
    to: &str,
    store: &SlateDbStore,
) -> Option<(MigrationReport, usize)> {
    /// Construct and run an identity migration for one workflow, merging the
    /// result into `$report` and incrementing `$count`.
    macro_rules! identity {
        ($report:expr, $count:expr, $store:expr, $wf:ty, $name:expr, $from:expr, $to:expr) => {{
            let snap = $store.as_snapshot_store();
            let registry = $store.as_process_registry();
            let runner = MigrationRunner::new(
                IdentityMigration::<$wf>::new(
                    WorkflowId::new($name, $from),
                    WorkflowId::new($name, $to),
                ),
                $store.clone(),
                snap,
            );
            let r = runner.run_and_update_registry(&registry).await;
            tracing::info!(
                workflow = $name,
                migrated = r.migrated,
                skipped = r.skipped,
                errors = r.errors.len(),
                "migration runner complete",
            );
            $report.merge(r);
            $count += 1;
        }};
    }

    match (from, to) {
        // ── FV2025-10-01 → FV2026-10-01 ──────────────────────────────────────
        //
        // No workflow state schemas changed for this annual release.
        // All migrations are identity: the snapshot is repointed to the new
        // workflow_id while the state value is preserved unchanged.
        //
        // If a workflow's state type changes in a future release, replace the
        // `identity!` call with a custom `StateMigration` impl from the domain crate.
        ("FV2025-10-01", "FV2026-10-01") => {
            let mut report = MigrationReport::default();
            let mut count = 0usize;

            // ── GPKE (Strom) ──────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                GpkeSupplierChangeWorkflow,
                "gpke-supplier-change",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeLfAnmeldungWorkflow,
                "gpke-lf-anmeldung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeSperrungWorkflow,
                "gpke-sperrung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeStornierungWorkflow,
                "gpke-stornierung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeAbrechnungWorkflow,
                "gpke-abrechnung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeKonfigurationWorkflow,
                "gpke-konfiguration",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeNeuanlageWorkflow,
                "gpke-neuanlage",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeLfAbmeldungWorkflow,
                "gpke-lf-abmeldung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GpkeAnfrageBestellungWorkflow,
                "gpke-anfrage-bestellung",
                from,
                to
            );

            // ── WiM Strom ────────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                WimDeviceChangeWorkflow,
                "wim-device-change",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimGeraeteubernahmeWorkflow,
                "wim-geraeteubernahme",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimStammdatenWorkflow,
                "wim-stammdaten",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimSteuerungsauftragWorkflow,
                "wim-steuerungsauftrag",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimPreisanfrageWorkflow,
                "wim-preisanfrage",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimPreislisteWorkflow,
                "wim-preisliste",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimRechnungWorkflow,
                "wim-rechnung",
                from,
                to
            );

            // ── GeLi Gas ──────────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                GeliGasSupplierChangeWorkflow,
                "geli-gas-supplier-change",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                GeliGasStornierungWorkflow,
                "geli-gas-stornierung",
                from,
                to
            );

            // ── MABIS ─────────────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                MabisBillingWorkflow,
                "mabis-billing",
                from,
                to
            );

            // ── WiM Gas ───────────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                WimGasAnmeldungWorkflow,
                "wim-gas-anmeldung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimGasKuendigungWorkflow,
                "wim-gas-kuendigung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimGasVerpflichtungsanfrageWorkflow,
                "wim-gas-verpflichtungsanfrage",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimGasInvoicWorkflow,
                "wim-gas-invoic",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimGasStornierungWorkflow,
                "wim-gas-stornierung",
                from,
                to
            );

            // ── GaBi Gas ──────────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                GaBiGasInvoicWorkflow,
                "gabi-gas-invoic",
                from,
                to
            );

            // ── Redispatch 2.0 ────────────────────────────────────────────────
            identity!(
                report,
                count,
                store,
                RedispatchStammdatenWorkflow,
                STAMMDATEN_WORKFLOW,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                AktivierungWorkflow,
                AKTIVIERUNG_WORKFLOW,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                VerfuegbarkeitWorkflow,
                VERFUEGBARKEIT,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                NetzengpassWorkflow,
                NETZENGPASS,
                from,
                to
            );
            identity!(report, count, store, KaskadeWorkflow, KASKADE, from, to);
            identity!(
                report,
                count,
                store,
                PlanungsdatenWorkflow,
                PLANUNGSDATEN,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                StatusanfrageWorkflow,
                STATUSANFRAGE,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                KostenblattWorkflow,
                KOSTENBLATT,
                from,
                to
            );

            Some((report, count))
        }

        // ── Unknown FV pair ──────────────────────────────────────────────────
        _ => None,
    }
}

// ── HTTP handler ──────────────────────────────────────────────────────────────

async fn handle_migrate(
    State(state): State<Arc<MigrationApiState>>,
    headers: HeaderMap,
    Json(req): Json<MigrateRequest>,
) -> Response {
    let Some(identity) = state.cedar.authenticate(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "unauthorized".to_owned(),
            }),
        )
            .into_response();
    };
    // A migration mutates every in-flight process — authentication alone is
    // not authorization. Cedar action: AdminMigrations.
    if !state.cedar.authorize_migrations(
        &identity,
        &crate::cedar_authz::MigrationResource {
            tenant: &state.tenant,
        },
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "AdminMigrations permission denied".to_owned(),
            }),
        )
            .into_response();
    }

    tracing::info!(
        from = req.from,
        to = req.to,
        "admin: starting in-flight process migration",
    );

    match dispatch_migrations(&req.from, &req.to, &state.store).await {
        None => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse {
                error: format!(
                    "no migration registered for FV pair ({} → {}); \
                     known pair: FV2025-10-01 → FV2026-10-01",
                    req.from, req.to,
                ),
            }),
        )
            .into_response(),

        Some((report, runners)) => {
            let has_errors = !report.errors.is_empty();
            let status = if has_errors {
                StatusCode::MULTI_STATUS
            } else {
                StatusCode::OK
            };
            if has_errors {
                tracing::error!(
                    from = req.from,
                    to = req.to,
                    migrated = report.migrated,
                    skipped = report.skipped,
                    error_count = report.errors.len(),
                    runners_executed = runners,
                    "admin: migration completed WITH ERRORS — manual intervention may be required",
                );
            } else {
                tracing::info!(
                    from = req.from,
                    to = req.to,
                    migrated = report.migrated,
                    skipped = report.skipped,
                    runners_executed = runners,
                    "admin: migration completed successfully",
                );
            }
            let errors: Vec<String> = report.errors.iter().map(|e| e.to_string()).collect();
            (
                status,
                Json(MigrateResponse {
                    from: req.from,
                    to: req.to,
                    migrated: report.migrated,
                    skipped: report.skipped,
                    errors,
                    runners_executed: runners,
                }),
            )
                .into_response()
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the migration admin router.
///
/// Mount at the admin port only (`--http-addr`). **Never** on the public
/// API-Webdienste port.
pub fn router(state: Arc<MigrationApiState>) -> Router {
    Router::new()
        .route("/admin/migrations", post(handle_migrate))
        .with_state(state)
}
