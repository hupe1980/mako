//! Partner REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/partners/:gln
//!   GET  /api/v1/partners/:gln
//!   GET  /api/v1/partners
//!
//! All PUT requests are validated as `rubo4e::current::Geschaeftspartner` (L6).
//! The `_typ` discriminator is auto-injected when absent and the canonical
//! camelCase form is stored in the `partners.channels` JSONB column.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
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
use rubo4e::current::Geschaeftspartner;
use serde::Deserialize;

use super::preisblatt::PriCatRepoExt;
use super::{Claims, IntoMdmResponse as _};

#[derive(Debug, Deserialize)]
pub struct PartnerQuery {
    pub marktrolle: Option<String>,
    pub sparte: Option<String>,
}

/// Validate and normalise a partner `data` payload as `rubo4e::current::Geschaeftspartner`.
///
/// 1. Auto-injects `"_typ": "GESCHAEFTSPARTNER"` when absent.
/// 2. Rejects 422 when `_typ` is present but wrong.
/// 3. Validates all enum fields (`marktrolle`, `rollencodetyp`, `marktteilnehmerstatus`).
/// 4. Re-serialises to canonical camelCase BO4E form.
///
/// The `data` field in `PartnerRecord.channels` is the partner's BO4E payload.
/// Returns the normalised JSON on success, or a 422 error body on failure.
fn normalize_geschaeftspartner(
    mut data: serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, serde_json::Value)> {
    if let Some(obj) = data.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::json!("GESCHAEFTSPARTNER"));
    }
    if data
        .get("_typ")
        .and_then(|v| v.as_str())
        .map(|t| t.to_uppercase() != "GESCHAEFTSPARTNER")
        .unwrap_or(false)
    {
        let typ = data.get("_typ").and_then(|v| v.as_str()).unwrap_or("");
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({
                "error": format!("expected _typ GESCHAEFTSPARTNER, got '{typ}'")
            }),
        ));
    }
    let partner: Geschaeftspartner = serde_json::from_value(data).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid Geschaeftspartner payload: {e}") }),
        )
    })?;
    Ok(serde_json::to_value(&partner).unwrap_or_default())
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

    // L6: Validate and normalise the partner's BO4E payload as Geschaeftspartner.
    // Injects _typ, validates enum fields, canonicalises camelCase.
    // Only applied when the `channels` field contains a non-null object.
    if record.channels.is_object() && !record.channels.is_null() {
        match normalize_geschaeftspartner(record.channels.clone()) {
            Ok(normalised) => record.channels = normalised,
            Err((status, body)) => return (status, Json(body)).into_response(),
        }
    }

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
        Ok(Some(p)) => {
            // L5: deserialise `channels` JSONB as `rubo4e::current::Geschaeftspartner`
            // for a fully typed GET response. Falls back to raw JSON on parse failure
            // (e.g. legacy records written before L6 PUT validation was enforced).
            let geschaeftspartner: Option<serde_json::Value> =
                if p.channels.is_object() && !p.channels.is_null() {
                    match serde_json::from_value::<Geschaeftspartner>(p.channels.clone()) {
                        Ok(gp) => serde_json::to_value(&gp).ok(),
                        Err(_) => Some(p.channels.clone()),
                    }
                } else {
                    None
                };
            let resp = serde_json::json!({
                "mp_id":              p.mp_id.to_string(),
                "display_name":       p.display_name,
                "marktrolle":         p.marktrolle,
                "rollencodetyp":      p.rollencodetyp,
                "makoadresse":        p.makoadresse,
                "geschaeftspartner":  geschaeftspartner,
                "version":            p.version,
                "updated_at":         p.updated_at.to_string(),
            });
            Json(resp).into_response()
        }
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

// ── B2: AS4 address lookup ────────────────────────────────────────────────────

/// `GET /api/v1/partners/{mp_id}/as4-address`
///
/// Returns the list of AS4 endpoint URLs (`Marktteilnehmer.makoadresse`) for the
/// given MP-ID. Used by `makod` for dynamic AS4 destination routing instead of
/// static config.
///
/// Returns 404 when the partner is not registered or has no AS4 addresses.
pub async fn get_as4_address<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(mp_id_str): Path<String>,
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
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let mp_id = match mp_id_str.parse::<MarktpartnerId>() {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    match state.partner_repo.find(&mp_id).await {
        Ok(Some(p)) if !p.makoadresse.is_empty() => axum::Json(serde_json::json!({
            "mp_id":          mp_id_str,
            "rollencodetyp":  p.rollencodetyp,
            "makoadresse":    p.makoadresse,
        }))
        .into_response(),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            format!("No AS4 address registered for {mp_id_str}"),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}
