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
use mako_geli_gas::{
    GeliGasLfAnmeldungWorkflow, GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    GpkeAbrechnungWorkflow, GpkeAbrechnungsdatenWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow, GpkeLfAnmeldungWorkflow,
    GpkeNeuanlageWorkflow, GpkeSperrungWorkflow, GpkeStornierungWorkflow,
    GpkeSupplierChangeWorkflow,
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
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimInvoicWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimStammdatenWorkflow,
    WimSteuerungsauftragWorkflow, WimWeiterverpflichtungWorkflow,
};
use serde::{Deserialize, Serialize};

use crate::cedar_authz::CedarAuthorizer;

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the migration admin API.
pub struct MigrationApiState {
    pub store: Arc<SlateDbStore>,
    /// Cedar-based authorization engine.
    pub cedar: Arc<CedarAuthorizer>,
    /// Operator tenant (MP-ID) — the Cedar resource scope.
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
    /// Workflow families this migration covered, in name order.
    ///
    /// A count alone reads as complete whatever the migration happens to
    /// include; the names let an operator check the list against the workflows
    /// their deployment actually runs.
    pub workflows: Vec<String>,
    /// Workflow families deliberately not migrated, each with its reason.
    ///
    /// Pure receive-and-record families: they record an inbound message and
    /// finish, so no process survives a cutover for a migration to repoint.
    pub workflows_not_migrated: Vec<(String, String)>,
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
/// Returns `Some((report, covered_workflows))` on success — the names, not just
/// a count, so callers can report exactly what was migrated.
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
) -> Option<(MigrationReport, Vec<&'static str>)> {
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
            $count.push($name);
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
            let mut count: Vec<&'static str> = Vec::new();

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
                GpkeAbrechnungsdatenWorkflow,
                "gpke-abrechnungsdaten",
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
                WimInvoicWorkflow,
                "wim-invoic",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                WimWeiterverpflichtungWorkflow,
                "wim-weiterverpflichtung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_wim::ersteinbau::WimErsteinbauWorkflow,
                mako_wim::ersteinbau::WORKFLOW_NAME,
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
            // The LFN's own Anmeldung waits out the GNB's answer window, so a
            // process started before a cutover is routinely still open after
            // it — the same reason `gpke-lf-anmeldung` carries an arm.
            identity!(
                report,
                count,
                store,
                GeliGasLfAnmeldungWorkflow,
                "geli-gas-lf-anmeldung",
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

            // ── NZR-EMob / Modell 2 ───────────────────────────────────────────
            //
            // All three legs share `ModellwechselState`, so one schema change
            // would move all three at once — which is exactly why each is
            // listed rather than one standing for the family.
            identity!(
                report,
                count,
                store,
                mako_emob::EmobAnmeldungWorkflow,
                mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_emob::EmobZuordnungsendeWorkflow,
                mako_emob::EmobZuordnungsendeWorkflow::WORKFLOW_NAME,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_emob::EmobAbmeldungWorkflow,
                mako_emob::EmobAbmeldungWorkflow::WORKFLOW_NAME,
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

            // ── Workflows added after the first cutover ────────────────────
            //
            // These hold in-flight state across a release just as the families
            // above do — the GaBi Gas final-allocation window alone runs to the
            // end of month M+2, so it routinely spans the October cutover — and
            // were simply never added. The coverage guard below now refuses a
            // dispatchable workflow with neither an arm nor a stated reason.
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeBeendigungZuordnungWorkflow,
                "gpke-beendigung-zuordnung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeKuendigungWorkflow,
                "gpke-kuendigung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeAnkuendigungZuordnungLfWorkflow,
                "gpke-ankuendigung-zuordnung-lf",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeSperrungLfWorkflow,
                "gpke-sperrung-lf",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeKonfigurationAenderungWorkflow,
                "gpke-konfiguration-aenderung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeDatanabrufWorkflow,
                "gpke-datenabruf",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeAllokationslisteWorkflow,
                "gpke-allokationsliste",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeEogWorkflow,
                "gpke-eog",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gpke::GpkeStammdatenaenderungWorkflow,
                "gpke-stammdatenaenderung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_geli_gas::GeliGasStammdatenaenderungWorkflow,
                "geli-gas-stammdatenaenderung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_geli_gas::GeliGasLfStornierungWorkflow,
                "geli-gas-stornierung-lf",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_geli_gas::GeliGasDatanabrufWorkflow,
                "geli-gas-datenabruf",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_geli_gas::GeliGasSperrungLfWorkflow,
                "geli-gas-sperrung-lf",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_geli_gas::GeliGasSperrungNbWorkflow,
                "geli-gas-sperrung-nb",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_geli_gas::GeliGasSperrprozesseInvoicWorkflow,
                "geli-gas-sperrprozesse-invoic",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_wim::WimRechnungsabwicklungWorkflow,
                "wim-rechnungsabwicklung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_wim::WimTechnikAenderungWorkflow,
                "wim-technik-aenderung",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_wim::WimInsrptWorkflow,
                "wim-insrpt",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_wim::wertebestellung::WimWertebestellungWorkflow,
                mako_wim::wertebestellung::WORKFLOW_NAME,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow,
                mako_wim::esa_wertebestellung::WORKFLOW_NAME,
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gabi_gas::GaBiGasNominationWorkflow,
                "gabi-gas-nomination",
                from,
                to
            );
            identity!(
                report,
                count,
                store,
                mako_gabi_gas::GaBiGasAllocationWorkflow,
                "gabi-gas-allocation",
                from,
                to
            );

            count.sort_unstable();
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
                    runners_executed = runners.len(),
                    "admin: migration completed WITH ERRORS — manual intervention may be required",
                );
            } else {
                tracing::info!(
                    from = req.from,
                    to = req.to,
                    migrated = report.migrated,
                    skipped = report.skipped,
                    runners_executed = runners.len(),
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
                    runners_executed: runners.len(),
                    workflows: runners.iter().map(|w| (*w).to_owned()).collect(),
                    workflows_not_migrated: NO_MIGRATION_NEEDED
                        .iter()
                        .map(|(w, why)| ((*w).to_owned(), (*why).to_owned()))
                        .collect(),
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

// ── Coverage guard ────────────────────────────────────────────────────────────

/// Workflows that deliberately have no FV migration arm, with the reason.
///
/// Reported by `POST /admin/migrations` alongside the covered list, because
/// the annual-release runbook asks an operator to sign off on the migration
/// result and "which families were intentionally skipped, and why" is part of
/// that judgement.
///
/// A workflow needs one when an in-flight process must carry its snapshot
/// across an annual release. The families below never hold one long enough for
/// that to matter: they record an inbound message and finish, so no process
/// survives a cutover for the migration to repoint.
const NO_MIGRATION_NEEDED: &[(&str, &str)] = &[
    (
        "mabis-profile",
        "records one MSCONS profile delivery and either accepts it or sends the \
         ORDERS 17211 Reklamation; a stream lives for one Bilanzierungsmonat and \
         does not survive a format-version change",
    ),
    (
        "geli-gas-mscons",
        "records inbound MSCONS Messdaten and completes",
    ),
    (
        "gpke-zuordnungsmeldung",
        "a Zuordnungs-Meldung is a Meldepflicht: one command, one event, done. \
         There is no Antwortnachricht, so no process is ever left in flight for \
         an annual release to repoint",
    ),
    (
        "geli-gas-zuordnungsmeldung",
        "the Gas twin of gpke-zuordnungsmeldung — the AHB says it in as many \
         words: „eine Nachricht, für die keine Antwort vorgesehen ist\"",
    ),
    (
        "gpke-messwerte",
        "records inbound MSCONS Messwerte and completes",
    ),
    (
        "gpke-partin",
        "records inbound PARTIN Kommunikationsdaten and completes",
    ),
    (
        "geli-gas-partin",
        "records inbound PARTIN Gas Kommunikationsdaten and completes",
    ),
    (
        "gpke-utilts",
        "records inbound UTILTS Konfigurationsdaten and completes",
    ),
    (
        "gabi-gas-mmma",
        "delegates delivery to gpke-allokationsliste",
    ),
    (
        "mabis-clearingliste",
        "records the inbound Clearingliste and completes",
    ),
    (
        "mabis-listenabgleich",
        "records the inbound list and completes",
    ),
    (
        "mabis-anforderung",
        "records the inbound Anforderung and completes",
    ),
    (
        "mabis-zp-lifecycle",
        "records the inbound ZP lifecycle message and completes",
    ),
    (
        "contrl-ack-obligation",
        "a delivery-window marker, not a workflow — it has no process state",
    ),
];

#[cfg(test)]
mod coverage_tests {
    use super::NO_MIGRATION_NEEDED;
    use crate::deadline_dispatch::DISPATCH_TABLE;

    /// Every dispatchable workflow must either have a migration arm or an
    /// explicit reason for not needing one.
    ///
    /// An unlisted workflow is harmless while every migration is an identity
    /// repoint and every workflow is `ForwardCompatible`, and stops being
    /// harmless the first release a workflow's state schema changes — at which
    /// point the omission is data loss discovered under cutover time pressure.
    ///
    /// This test does not demand a migration for everything. It demands a
    /// decision for everything.
    #[tokio::test]
    async fn every_dispatchable_workflow_has_a_migration_decision() {
        let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store");
        let (_report, covered) = super::dispatch_migrations("FV2025-10-01", "FV2026-10-01", &store)
            .await
            .expect("the active FV pair is registered");

        let exempt: std::collections::HashMap<&str, &str> =
            NO_MIGRATION_NEEDED.iter().copied().collect();
        let covered: std::collections::HashSet<&str> = covered.into_iter().collect();

        let undecided: Vec<&str> = DISPATCH_TABLE
            .iter()
            .filter(|w| !covered.contains(**w) && !exempt.contains_key(**w))
            .copied()
            .collect();
        assert!(
            undecided.is_empty(),
            "these workflows have neither an FV migration arm nor an entry in \
             NO_MIGRATION_NEEDED:\n  {}\n\
             Add an `identity!` call in dispatch_migrations, or list the workflow \
             with the reason a migration is unnecessary.",
            undecided.join("\n  "),
        );

        // An exemption for a workflow that is also migrated, or for one that no
        // longer exists, hides a real gap behind a stale name.
        let contradictory: Vec<&str> = exempt
            .keys()
            .filter(|w| covered.contains(**w))
            .copied()
            .collect();
        assert!(
            contradictory.is_empty(),
            "NO_MIGRATION_NEEDED names workflows that are migrated anyway: {contradictory:?}"
        );
        let unknown: Vec<&str> = exempt
            .keys()
            .filter(|w| !DISPATCH_TABLE.contains(w))
            .copied()
            .collect();
        assert!(
            unknown.is_empty(),
            "NO_MIGRATION_NEEDED names unknown workflows: {unknown:?}"
        );
    }
}
