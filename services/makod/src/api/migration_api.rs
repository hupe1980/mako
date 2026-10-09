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
//! Serializable Snapshot Isolation ensures that concurrent `Process::run`
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
//! For each known `(from, to)` FV pair the endpoint runs an
//! [`IdentityMigration`] for every workflow this binary registers, through the
//! workflow's [`WorkflowDriver`] — there is no list of workflows here to keep
//! beside the modules. For a transition where a workflow's state schema
//! changed, that workflow needs a bespoke `StateMigration` in its domain crate.
//!
//! Add a new `(from, to)` pair to [`KNOWN_FV_TRANSITIONS`] each release cycle.
//!
//! [`IdentityMigration`]: mako_engine::migration::IdentityMigration
//! [`WorkflowDriver`]: mako_engine::driver::WorkflowDriver

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use mako_engine::migration::MigrationReport;
use mako_engine::store_slatedb::SlateDbStore;
use mako_service::Json;
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
/// [`dispatch_migrations`] runs a migration only for a pair listed here. The
/// `migration_dispatch_table_covers_active_fv_transitions` integration test
/// panics if any active transition is absent.
///
/// Add a new entry each release cycle.
pub const KNOWN_FV_TRANSITIONS: &[(&str, &str)] = &[("FV2025-10-01", "FV2026-10-01")];

// ── Request / response types ──────────────────────────────────────────────────

/// `POST /admin/migrations` request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ── Auth helper ───────────────────────────────────────────────────────────────

// (Auth is handled via CedarAuthorizer in the handler directly.)

// ── Migration dispatch ────────────────────────────────────────────────────────

/// Run every registered workflow's migration for the given `(from, to)` FV
/// pair.
///
/// Returns `None` when the pair is not in [`KNOWN_FV_TRANSITIONS`]. Returns
/// `Some((report, covered_workflows))` otherwise — the names, not just a count,
/// so callers can report exactly what was migrated.
///
/// Every transition so far is an identity repoint: no workflow state schema
/// changed, so each snapshot moves to the new `workflow_id` unchanged.
pub async fn dispatch_migrations(
    from: &str,
    to: &str,
    store: &SlateDbStore,
) -> Option<(MigrationReport, Vec<&'static str>)> {
    if !KNOWN_FV_TRANSITIONS.contains(&(from, to)) {
        return None;
    }
    let mut drivers: Vec<_> = crate::startup::production_modules()
        .iter()
        .flat_map(|m| m.drivers())
        .collect();
    drivers.sort_unstable_by_key(|d| d.name());
    let mut report = MigrationReport::default();
    let mut covered = Vec::with_capacity(drivers.len());
    for driver in drivers {
        let r = driver.migrate_identity(store, from, to).await;
        tracing::info!(
            workflow = driver.name(),
            migrated = r.migrated,
            skipped = r.skipped,
            errors = r.errors.len(),
            "migration runner complete",
        );
        report.merge(r);
        covered.push(driver.name());
    }
    Some((report, covered))
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
                    "no migration registered for FV pair ({} → {}); known pairs: {:?}",
                    req.from, req.to, KNOWN_FV_TRANSITIONS,
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
