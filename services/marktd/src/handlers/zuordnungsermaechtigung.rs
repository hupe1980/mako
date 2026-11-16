//! `GET /api/v1/zuordnungsermaechtigungen/{lf_mp_id}` — the
//! Zuordnungsermächtigungen this NB holds for a Lieferant.
//!
//! A BKV gives them by UTILMD 55071 and withdraws them by 55072 („Die
//! Ermächtigung erfolgt je ZRT, BG, BK und LF ab einem bestimmten Zeitpunkt",
//! BK6-24-174 Anlage 3 Kap. 10.2.1, S. 119); the event ingest records both.
//! processd reads this list for `E_0622` Prüfschritt 60 and refuses an
//! Anmeldung whose Bilanzkreis none covers on its Zuordnungsbeginn.
//!
//! There is no write route: the record is what the BKV sent, and an
//! Ermächtigung typed in by hand would authorise Zuordnungen no BKV agreed to.
//!
//! ## Access control
//!
//! `GET` — `read-zuordnungsermaechtigung`, any authenticated caller in the
//! same tenant.

use std::sync::Arc;

use axum::{Extension, extract::Path, response::IntoResponse};
use mako_service::cedar::CedarEnforcer;
use mako_service::{ApiError, Json};

use crate::handlers::{Claims, Tenant, mdm_error};
use crate::pg::PgZuordnungsermaechtigungRepository;

/// `GET /api/v1/zuordnungsermaechtigungen/{lf_mp_id}` — every Ermächtigung,
/// active or ended, naming the Lieferant. An empty list is an answer: no BKV
/// has authorised it.
pub async fn list_zuordnungsermaechtigungen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<Arc<PgZuordnungsermaechtigungRepository>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(lf_mp_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-zuordnungsermaechtigung", &tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }
    match repo.fuer_lieferant(&tenant, &lf_mp_id).await {
        Ok(recs) => Json(recs).into_response(),
        Err(e) => mdm_error(e).into_response(),
    }
}
