//! Partner REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/partners/:gln
//!   GET  /api/v1/partners/:gln
//!   GET  /api/v1/partners

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::{EventExtensions, MarktEvent},
    domain::MarktpartnerId,
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRecord, PartnerRepository, PriCatRepository as _, SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use serde::Deserialize;

use super::preisblatt::PriCatRepoExt;
use super::{Claims, IntoMdmResponse as _};

#[derive(Debug, Deserialize)]
pub struct PartnerQuery {
    pub marktrolle: Option<String>,
    pub sparte: Option<String>,
}

/// `PUT /api/v1/partners/:gln`
pub async fn put_partner<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pricat_repo): Extension<PriCatRepoExt>,
    claims: Claims,
    Path(gln_str): Path<String>,
    Json(mut record): Json<PartnerRecord>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "write-partner", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

    // Validate and parse the path-parameter GLN; override the body's gln field.
    let mp_id = match gln_str.parse::<MarktpartnerId>() {
        Ok(g) => g,
        Err(e) => {
            return MdmError::InvalidMpId {
                mp_id: gln_str,
                reason: e.to_string(),
            }
            .into_response();
        }
    };
    let is_lf = record
        .marktrolle
        .as_deref()
        .is_some_and(|r| r.eq_ignore_ascii_case("LF") || r.eq_ignore_ascii_case("LFG"));
    let lf_mp_id = gln_str.clone();
    record.mp_id = mp_id;

    match state.partner_repo.upsert(record).await {
        Ok(version) => {
            let evt = MarktEvent::new(
                &state.tenant_gln,
                "de.markt.partner.updated",
                gln_str,
                serde_json::json!({ "version": version }),
            )
            .with_extensions(EventExtensions {
                ..Default::default()
            });
            if let Ok(payload) = serde_json::to_value(&evt) {
                let _ = state.event_tx.send(payload);
            }

            // Phase 2: when a new LF partner is registered, auto-dispatch the
            // latest PRICAT version for every NB the operator manages.
            // The tenant GLN is the operator's own GLN (typically NB).
            // This is best-effort — dispatch failure is logged but not returned.
            if is_lf {
                let nb_gln_for_dispatch = state.tenant_gln.clone();
                let tenant_for_dispatch = state.tenant_gln.clone();
                // Inline instead of spawn — avoids silently swallowed panics.
                match pricat_repo
                    .find_latest(&nb_gln_for_dispatch, &tenant_for_dispatch)
                    .await
                {
                    Ok(Some(v))
                        if !matches!(
                            v.dispatch_state,
                            mako_markt::repository::PriCatDispatchState::Done
                        ) =>
                    {
                        // Already pending/queued/error — don't double-queue
                    }
                    Ok(Some(v)) => {
                        // Latest version is Done — re-queue it so the LF gets the sheet.
                        if let Err(e) = pricat_repo.mark_queued(v.id).await {
                            tracing::warn!(
                                lf_mp_id = %lf_mp_id,
                                nb_mp_id = %nb_gln_for_dispatch,
                                error  = %e,
                                "put_partner: PRICAT re-queue for new LF failed (non-fatal)",
                            );
                        }
                    }
                    Ok(None) => {
                        // No PRICAT published yet for this NB — nothing to dispatch.
                    }
                    Err(e) => {
                        tracing::warn!(
                            lf_mp_id = %lf_mp_id,
                            nb_mp_id = %nb_gln_for_dispatch,
                            error  = %e,
                            "put_partner: PRICAT lookup for new LF failed (non-fatal)",
                        );
                    }
                }
            }

            Json(serde_json::json!({ "version": version })).into_response()
        }
        Err(e) => e.into_response(),
    }
}
/// `GET /api/v1/partners/:gln`
pub async fn get_partner<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(gln_str): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "read-partner", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let mp_id = match gln_str.parse::<MarktpartnerId>() {
        Ok(g) => g,
        Err(e) => {
            return MdmError::InvalidMpId {
                mp_id: gln_str,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    match state.partner_repo.find(&mp_id).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => mako_markt::error::MdmError::NotFound {
            resource_type: "resource",
            id: gln_str,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/partners`
pub async fn list_partners<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Query(q): Query<PartnerQuery>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "read-partner", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    match state.partner_repo.list().await {
        Ok(partners) => {
            // Filter in Rust after fetching all (typical deployments have < 1000 partners)
            let filtered: Vec<_> = partners
                .into_iter()
                .filter(|p| {
                    let role_ok = q.marktrolle.as_deref().is_none_or(|r| {
                        p.marktrolle
                            .as_deref()
                            .is_some_and(|mr| mr.eq_ignore_ascii_case(r))
                    });
                    let sparte_ok = q.sparte.as_deref().is_none_or(|s| {
                        p.sparte
                            .is_some_and(|ps| ps.to_string().eq_ignore_ascii_case(s))
                    });
                    role_ok && sparte_ok
                })
                .collect();
            Json(filtered).into_response()
        }
        Err(e) => e.into_response(),
    }
}
