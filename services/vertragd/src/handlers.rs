//! HTTP handlers for `vertragd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use mako_service::oidc::Claims;

use crate::{
    config::VertragdConfig,
    events::{build_cloud_event, parse_mako_outcome},
    pg::{
        CreateKundeInput, CreateRahmenvertragInput, CreateVersorgungsvertragInput, KuendigungInput,
        TarifwechselInput, UpdateKundeInput, UpsertIdentitaetInput, count_active_identitaeten,
        deactivate_identitaet_by_sub, derive_vertrag_status, earliest_kuendigungsdatum,
        fetch_identitaet_by_sub, fetch_komponente, fetch_kunde, fetch_kunde_by_sub, fetch_person,
        fetch_preisgarantie, fetch_vertrag, fetch_vertrag_by_malo, find_expiring_vertraege,
        gdpr_export, idempotent_event, insert_rahmenvertrag, insert_versorgungsvertrag,
        list_aktive_malo_ids, list_all_rahmenvertraege, list_identitaeten, list_komponenten,
        list_kunden, list_offene_vertraege, list_portfolio_by_kunde, list_rahmenvertraege_by_kunde,
        list_rahmenvertrag_malos, list_versorgungsvertraege_by_rahmenvertrag,
        list_vertraege_by_kunde, store_pending_tarifwechsel, storniere_vertrag,
        update_komponente_product, update_komponente_status, update_kunde, update_letzter_login,
        update_vertrag_status, upsert_identitaet, upsert_kunde, upsert_person,
        upsert_preisgarantie, widerruf_kuendigung,
    },
};

// ── Role enforcement helpers ──────────────────────────────────────────────────

/// Return 403 if the claims have no recognized energy-market write role.
///
/// LF (Lieferant), NB (Netzbetreiber) and MSB (Messstellenbetreiber) are allowed.
/// Dev mode (empty roles from OidcVerifier::disabled) passes through.
///
/// This is a lightweight guard against misconfigured tokens; Cedar ABAC (if
/// enabled) enforces fine-grained resource-level policies on top of this check.
#[allow(dead_code)]
fn require_operator_role(claims: &Claims) -> Option<(StatusCode, axum::Json<serde_json::Value>)> {
    let roles = &claims.0.mako_roles;
    if roles.is_empty() {
        // Dev mode (OidcVerifier::disabled): permit all.
        return None;
    }
    let has_write_role = roles
        .iter()
        .any(|r| matches!(r.to_uppercase().as_str(), "LF" | "NB" | "MSB"));
    if !has_write_role {
        Some((
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "insufficient role — operator endpoints require LF energy-market role",
                "required": "LF",
                "provided": roles,
            })),
        ))
    } else {
        None
    }
}

// ── Kunde ─────────────────────────────────────────────────────────────────────

/// `POST /api/v1/kunden` — create or update a customer profile (B2C or B2B).
///
/// Idempotent on `erp_kunde_id`.  Sets `oidc_sub` for portal authentication.
pub async fn post_create_kunde(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Json(input): Json<CreateKundeInput>,
) -> impl IntoResponse {
    match upsert_kunde(&pool, &cfg.tenant, &input).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "kunden_id": id })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/{id}` — get customer profile + active MaLo IDs.
pub async fn get_kunde(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_kunde(&pool, id, &cfg.tenant).await {
        Ok(Some(k)) => {
            let (malo_ids, identitaeten) = tokio::join!(
                list_aktive_malo_ids(&pool, id, &cfg.tenant),
                list_identitaeten(&pool, id, &cfg.tenant),
            );
            Json(serde_json::json!({
                "kunde": k,
                "active_malo_ids": malo_ids.unwrap_or_default(),
                "identitaeten": identitaeten.unwrap_or_default(),
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/by-sub/{oidc_sub}` — resolve OIDC subject to customer + MaLo IDs.
///
/// Used by `portald` to authorize customer requests.  Returns the full customer
/// profile and all currently active MaLo IDs for resource-level authorization.
pub async fn get_kunde_by_sub(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(oidc_sub): Path<String>,
) -> impl IntoResponse {
    match fetch_kunde_by_sub(&pool, &oidc_sub, &cfg.tenant).await {
        Ok(Some(k)) => {
            let identity = fetch_identitaet_by_sub(&pool, &oidc_sub, &cfg.tenant)
                .await
                .ok()
                .flatten();
            let mut malo_ids = list_aktive_malo_ids(&pool, k.id, &cfg.tenant)
                .await
                .unwrap_or_default();
            // Apply site scope if the identity has a standort_filter
            if let Some(ref ident) = identity
                && let Some(ref filter) = ident.standort_filter
            {
                let vertraege = list_vertraege_by_kunde(&pool, k.id, &cfg.tenant)
                    .await
                    .unwrap_or_default();
                let scoped_vertrag_ids: std::collections::HashSet<Uuid> = vertraege
                    .iter()
                    .filter(|v| v.standort_bezeichnung.as_deref() == Some(filter.as_str()))
                    .map(|v| v.id)
                    .collect();
                let mut scoped_malos = Vec::new();
                for vid in &scoped_vertrag_ids {
                    if let Ok(komps) = list_komponenten(&pool, *vid).await {
                        for komp in komps {
                            if let Some(m) = komp.malo_id {
                                scoped_malos.push(m);
                            }
                        }
                    }
                }
                malo_ids.retain(|m| scoped_malos.contains(m));
            }
            let vertraege_count = list_vertraege_by_kunde(&pool, k.id, &cfg.tenant)
                .await
                .map(|v| v.len())
                .unwrap_or(0);
            Json(serde_json::json!({
                "kunde": k,
                "active_malo_ids": malo_ids,
                "vertraege_count": vertraege_count,
                "rolle": identity.as_ref().map(|i| &i.rolle),
                "standort_filter": identity.as_ref().and_then(|i| i.standort_filter.as_deref()),
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Rahmenvertrag (B2B Framework Contract) ────────────────────────────────────

/// `POST /api/v1/kunden/{id}/rahmenvertraege` — create a B2B framework contract.
///
/// A Rahmenvertrag is a master agreement that sets shared pricing, notice periods,
/// and billing terms for N individual Versorgungsverträge (supply contracts) under it.
/// Primarily used for B2B_RLM and B2B_HV portfolio customers with multiple delivery points.
pub async fn post_create_rahmenvertrag(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<CreateRahmenvertragInput>,
) -> impl IntoResponse {
    // Verify customer belongs to tenant
    match fetch_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(_)) => {}
    }
    match insert_rahmenvertrag(&pool, kunden_id, &cfg.tenant, &input).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "rahmenvertrag_id": id })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Versorgungsvertrag (Supply Contract) ──────────────────────────────────────

/// `POST /api/v1/kunden/{id}/vertraege` — create a supply contract for a customer.
///
/// Supports both B2C (no `rahmenvertrag_id`) and B2B (with `rahmenvertrag_id`).
///
/// For each commodity `Vertragskomponente`:
/// - STROM/GAS/WAERME/SOLAR/EEG/WAERMEPUMPE/WALLBOX → `processd /start-supply`
/// - HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG → direct fulfillment
///
/// Idempotent on `erp_contract_id`.
pub async fn post_create_vertrag(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<CreateVersorgungsvertragInput>,
) -> impl IntoResponse {
    match fetch_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(_)) => {}
    }
    let inserted =
        match insert_versorgungsvertrag(&pool, kunden_id, &cfg.tenant, &cfg.lf_mp_id, &input).await
        {
            Ok(v) => v,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    // Dispatch MaKo Lieferbeginn over the ROWS ACTUALLY INSERTED — never the
    // request body. On an idempotent replay of the same `erp_contract_id`,
    // `inserted.komponenten` is empty, so a second POST fires no second
    // UTILMD. Each dispatch carries the real component id, so the
    // confirmation updates the right row.
    let mut dispatched = 0u32;
    for komp in &inserted.komponenten {
        if requires_mako_workflow(&komp.sparte)
            && let Some(malo_id) = &komp.malo_id
            && let Some(nb_mp_id) = &komp.nb_mp_id
        {
            tokio::spawn(dispatch_lieferbeginn(
                Arc::clone(&cfg),
                komp.id,
                pool.clone(),
                malo_id.clone(),
                komp.melo_id.clone(),
                nb_mp_id.clone(),
                komp.sparte.clone(),
                komp.lieferbeginn,
            ));
            dispatched += 1;
        }
    }

    if dispatched > 0 && inserted.is_new {
        let _ = update_vertrag_status(&pool, inserted.id, &cfg.tenant, "IN_BEARBEITUNG").await;
    }

    let status_code = if inserted.is_new {
        StatusCode::CREATED
    } else {
        // Idempotent replay: the contract already exists; report 200 so the
        // caller can distinguish a re-POST from a first create.
        StatusCode::OK
    };
    (
        status_code,
        Json(serde_json::json!({
            "vertrag_id":  inserted.id,
            "status":      if dispatched > 0 { "IN_BEARBEITUNG" } else { "ANGELEGT" },
            "komponenten": inserted.komponenten.len(),
            "mako_dispatched": dispatched,
            "idempotent_replay": !inserted.is_new,
        })),
    )
        .into_response()
}

/// `GET /api/v1/vertraege/{id}` — get Versorgungsvertrag + all components.
pub async fn get_vertrag(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_vertrag(&pool, id, &cfg.tenant).await {
        Ok(Some(v)) => {
            let komp = list_komponenten(&pool, id).await.unwrap_or_default();
            Json(serde_json::json!({ "vertrag": v, "komponenten": komp })).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/vertraege/by-malo/{malo_id}` — the active contract behind a MaLo.
///
/// The lookup `billingd` uses to state §40 Abs. 1 EnWG contract facts on the
/// invoice. Besides the raw contract row, the response computes the next
/// possible Kündigungstermin as of today:
///
/// - unbefristet: today plus the Kündigungsfrist;
/// - befristet, notice still possible: the Vertragsende;
/// - befristet with auto-renewal, notice window passed: today plus one month —
///   after an automatic renewal, §309 Nr. 9 lit. b BGB caps the notice period
///   at one month;
/// - befristet without renewal, notice window passed: the Vertragsende (the
///   contract ends then regardless).
pub async fn get_vertrag_by_malo(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    match fetch_vertrag_by_malo(&pool, &malo_id, &cfg.tenant).await {
        Ok(Some((vertrag, komponente))) => {
            let today = time::OffsetDateTime::now_utc().date();
            let mit_frist = earliest_kuendigungsdatum(today, vertrag.kuendigungsfrist_monate);
            let naechstmoeglicher_kuendigungstermin = match vertrag.vertragsende {
                Some(ende) if mit_frist <= ende => ende,
                Some(_) if vertrag.auto_renewal => earliest_kuendigungsdatum(today, 1),
                Some(ende) => ende,
                None => mit_frist,
            };
            Json(serde_json::json!({
                "vertrag": vertrag,
                "komponente": komponente,
                "naechstmoeglicher_kuendigungstermin":
                    naechstmoeglicher_kuendigungstermin.to_string(),
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/vertraege/billing-candidates` — active supply components with
/// their §40b EnWG billing cadence.
///
/// Consumed by billingd's billing-run worker: one entry per active
/// Vertragskomponente with a MaLo, carrying the contract's
/// `abrechnungszyklus` (MONATLICH/VIERTELJAEHRLICH/HALBJAEHRLICH/JAEHRLICH)
/// and the supply window for period clipping.
pub async fn list_billing_candidates_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
) -> impl IntoResponse {
    match crate::pg::list_billing_candidates(&pool, &cfg.tenant).await {
        Ok(rows) => Json(serde_json::json!({
            "count": rows.len(),
            "candidates": rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/vertraege` — list open contracts.
pub async fn list_vertraege(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    match list_offene_vertraege(&pool, &cfg.tenant, q.limit.unwrap_or(100).min(500)).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
}

/// `POST /api/v1/vertraege/{id}/kuendigen` — initiate Lieferende for all commodity components.
///
/// **Regulatory validation (§14 StromGVV / §13 GasGVV):**
/// - `lieferende` must be ≥ today (cannot cancel retroactively)
/// - `lieferende` must respect the contract's `kuendigungsfrist_monate`
pub async fn kuendige_vertrag(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
    Json(input): Json<KuendigungInput>,
) -> impl IntoResponse {
    let vertrag = match fetch_vertrag(&pool, id, &cfg.tenant).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if !matches!(vertrag.status.as_str(), "AKTIV" | "TEILERFUELLUNG") {
        return (
            StatusCode::CONFLICT,
            format!(
                "Vertrag status '{}' cannot be cancelled — must be AKTIV",
                vertrag.status
            ),
        )
            .into_response();
    }
    // Notice period validation — §14 StromGVV / §13 GasGVV.
    let today = time::OffsetDateTime::now_utc().date();
    if input.lieferende < today {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "lieferende cannot be in the past",
                "lieferende": input.lieferende.to_string(),
                "today": today.to_string(),
            })),
        )
            .into_response();
    }
    let earliest = earliest_kuendigungsdatum(today, vertrag.kuendigungsfrist_monate);
    if input.lieferende < earliest {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "lieferende too early — Kündigungsfrist not respected",
                "lieferende": input.lieferende.to_string(),
                "earliest_valid": earliest.to_string(),
                "kuendigungsfrist_monate": vertrag.kuendigungsfrist_monate,
                "regulatory_basis": "§14 StromGVV / §13 GasGVV",
            })),
        )
            .into_response();
    }
    let komponenten = list_komponenten(&pool, id).await.unwrap_or_default();
    let mut dispatched = 0u32;
    for k in komponenten
        .iter()
        .filter(|k| matches!(k.status.as_str(), "AKTIV" | "BESTAETIGT"))
    {
        if requires_mako_workflow(&k.sparte)
            && let Some(malo_id) = &k.malo_id
            && let Some(nb_mp_id) = &k.nb_mp_id
        {
            tokio::spawn(dispatch_lieferende(
                Arc::clone(&cfg),
                k.id,
                pool.clone(),
                malo_id.clone(),
                nb_mp_id.clone(),
                k.sparte.clone(),
                input.lieferende,
            ));
            dispatched += 1;
        } else {
            let _ = update_komponente_status(&pool, k.id, "BEENDET", None, None, None, None).await;
        }
    }
    let _ = update_vertrag_status(
        &pool,
        id,
        &cfg.tenant,
        if dispatched > 0 {
            "GEKÜNDIGT"
        } else {
            "ABGELAUFEN"
        },
    )
    .await;
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "vertrag_id": id,
            "lieferende": input.lieferende.to_string(),
            "mako_dispatched": dispatched,
        })),
    )
        .into_response()
}

// ── Identity management (portal users) ────────────────────────────────────────

/// `POST /api/v1/kunden/{id}/identitaeten`
///
/// Add a portal user to a Kunde.  Idempotent on `oidc_sub`.
/// B2B customers call this endpoint for each employee who needs portal access.
/// B2C customers typically have exactly one identity (created automatically at Kunde creation).
pub async fn post_upsert_identitaet(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<UpsertIdentitaetInput>,
) -> impl IntoResponse {
    match fetch_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(_)) => {}
    }
    // Enforce maximum identities per Kunde (resource exhaustion guard).
    // Check only when the sub is new (not an update to an existing identity).
    let existing = fetch_identitaet_by_sub(&pool, &input.oidc_sub, &cfg.tenant)
        .await
        .unwrap_or(None);
    if existing.is_none() {
        let count = count_active_identitaeten(&pool, kunden_id, &cfg.tenant)
            .await
            .unwrap_or(0);
        if count >= cfg.max_identitaeten_per_kunde as i64 {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!(
                        "maximum {} active identities per customer — deactivate an existing identity first",
                        cfg.max_identitaeten_per_kunde
                    )
                })),
            ).into_response();
        }
    }
    match upsert_identitaet(&pool, kunden_id, &cfg.tenant, &input).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id,
                "kunden_id": kunden_id,
                "oidc_sub": input.oidc_sub,
                "rolle": input.rolle.as_deref().unwrap_or("VOLLZUGRIFF"),
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/{id}/identitaeten` — list all active portal users.
pub async fn list_kunde_identitaeten(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
) -> impl IntoResponse {
    match list_identitaeten(&pool, kunden_id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Additional customer / contract endpoints ──────────────────────────────────

/// `PUT /api/v1/kunden/{id}` — update customer profile (especially oidc_sub for portal auth).
pub async fn put_update_kunde(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateKundeInput>,
) -> impl IntoResponse {
    match update_kunde(&pool, id, &cfg.tenant, &input).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/rahmenvertraege`
///
/// List all Rahmenverträge for this tenant.
/// Operator / CRM endpoint — not exposed to portal users.
/// `?status=AKTIV|ENTWURF|GEKÜNDIGT|ABGELAUFEN` for filtering.
pub async fn list_rahmenvertraege_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Query(q): Query<RahmenvertragListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    match list_all_rahmenvertraege(&pool, &cfg.tenant, q.status.as_deref(), limit).await {
        Ok(rows) => Json(serde_json::json!({
            "count": rows.len(),
            "rahmenvertraege": rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct RahmenvertragListQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/rahmenvertraege/{id}` — get a single Rahmenvertrag with child Versorgungsverträge.
pub async fn get_rahmenvertrag_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::pg::fetch_rahmenvertrag;
    match fetch_rahmenvertrag(&pool, id, &cfg.tenant).await {
        Ok(Some(r)) => {
            let vertraege = list_versorgungsvertraege_by_rahmenvertrag(&pool, id, &cfg.tenant)
                .await
                .unwrap_or_default();
            Json(serde_json::json!({
                "rahmenvertrag": r,
                "versorgungsvertraege": vertraege,
                "vertraege_count": vertraege.len(),
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Rahmenvertrag {id} not found") })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/{id}/rahmenvertraege` — list B2B framework contracts for a customer.
pub async fn list_kunde_rahmenvertraege(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
) -> impl IntoResponse {
    match list_rahmenvertraege_by_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/{id}/vertraege` — list all Versorgungsverträge for a customer.
pub async fn list_kunde_vertraege(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
) -> impl IntoResponse {
    match list_vertraege_by_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/authenticate?malo_id={malo_id}`
///
/// portald authorization endpoint. The `sub` comes from the **verified** OIDC
/// token (the `Claims` extractor runs the same signature/issuer/audience check
/// as every other protected route), then this checks whether the customer owns
/// `malo_id` within their site scope.
///
/// ## Anti-enumeration
///
/// Every "not authorized" outcome — unknown sub, sub with no customer, customer
/// that does not own the MaLo, MaLo outside the identity's `standort_filter` —
/// returns the **same `403 Forbidden`**. A distinct `404` for "no such customer"
/// would let a holder of any valid token probe which subjects and MaLo IDs
/// exist (GDPR Art. 32). Only a genuine server fault returns `500`.
pub async fn get_authenticate(
    claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Query(q): Query<AuthenticateQuery>,
) -> impl IntoResponse {
    let sub = claims.sub().to_owned();

    let kunde = match fetch_kunde_by_sub(&pool, &sub, &cfg.tenant).await {
        Ok(Some(k)) => k,
        // Unknown sub is reported as Forbidden, not Not-Found — see the
        // anti-enumeration note above.
        Ok(None) => return StatusCode::FORBIDDEN.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Check MaLo ownership with standort_filter scope
    let identity = fetch_identitaet_by_sub(&pool, &sub, &cfg.tenant)
        .await
        .ok()
        .flatten();
    let malo_ids = list_aktive_malo_ids(&pool, kunde.id, &cfg.tenant)
        .await
        .unwrap_or_default();
    let owns_malo = malo_ids.iter().any(|id| id == &q.malo_id);
    if !owns_malo {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Enforce standort_filter if set: identity can only access MaLos for their site
    if let Some(ref ident) = identity
        && let Some(ref filter) = ident.standort_filter
    {
        let vertraege = list_vertraege_by_kunde(&pool, kunde.id, &cfg.tenant)
            .await
            .unwrap_or_default();
        let scoped_malos: Vec<String> = {
            let mut out = Vec::new();
            for v in vertraege
                .iter()
                .filter(|v| v.standort_bezeichnung.as_deref() == Some(filter.as_str()))
            {
                if let Ok(komps) = list_komponenten(&pool, v.id).await {
                    for komp in komps {
                        if let Some(m) = komp.malo_id {
                            out.push(m);
                        }
                    }
                }
            }
            out
        };
        if !scoped_malos.contains(&q.malo_id) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    // Update letzter_login timestamp for audit trail (detects dormant accounts).
    // Best-effort: failure does not block the authentication response.
    let _ = update_letzter_login(&pool, &sub, &cfg.tenant).await;

    // portald needs the kunden_id to resolve write operations.
    // Return it in the body so portald avoids a second roundtrip.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "kunden_id": kunde.id,
            "kundentyp": kunde.kundentyp,
            "malo_id": q.malo_id,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct AuthenticateQuery {
    pub malo_id: String,
}

/// `POST /api/v1/vertraege/{id}/tarifwechsel` — change tariff for a component.
///
/// Tarifwechsel changes the product/pricing for an existing Vertragskomponente
/// without triggering a new UTILMD Lieferbeginn.  Only `product_code` in `tarifbd`
/// is updated; the MaKo supply status (UTILMD) remains unchanged.
///
/// ## Future vs. immediate Tarifwechsel
///
/// - `wirksamkeit > today`: stores as **pending** Tarifwechsel.  The background worker
///   applies it on `wirksamkeit` and emits `de.vertrag.preisaenderung.ankuendigung`
///   42 days before (§41 Abs. 3 EnWG: ≥ 6 weeks advance notice).
/// - `wirksamkeit ≤ today`: applied immediately (urgent correction / retroactive change).
pub async fn tarifwechsel_vertrag(
    claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(vertrag_id): Path<Uuid>,
    Json(input): Json<TarifwechselInput>,
) -> impl IntoResponse {
    // Verify component belongs to this contract
    let komp = match fetch_komponente(&pool, input.komp_id).await {
        Ok(Some(k)) if k.vertrag_id == vertrag_id => k,
        Ok(Some(_)) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "component does not belong to this Vertrag"
                })),
            )
                .into_response();
        }
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Always fetch the Versorgungsvertrag — needed for tenant validation AND
    // to get the preisgarantie_bis DATE for the override audit log.
    let vertrag = match fetch_vertrag(&pool, vertrag_id, &cfg.tenant).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let today = time::OffsetDateTime::now_utc().date();

    // ── Preisgarantie guard (§41 EnWG contractual price-lock) ────────────────
    // Reject Tarifwechsel whose wirksamkeit falls within the guarantee window
    // unless the caller explicitly bypasses with override_preisgarantie=true.
    // The guard uses `wirksamkeit <= garantie_bis` (inclusive boundary).
    if !input.override_preisgarantie
        && let Some(garantie_bis) = vertrag.preisgarantie_bis
        && input.wirksamkeit <= garantie_bis
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "Tarifwechsel blocked by Preisgarantie",
                "preisgarantie_bis": garantie_bis.to_string(),
                "wirksamkeit": input.wirksamkeit.to_string(),
                "hint": "Set override_preisgarantie=true to bypass (operator ADMIN role required)"
            })),
        )
            .into_response();
    }

    // ── Preisgarantie override audit log ─────────────────────────────────────
    // Every bypass is logged immutably with the operator's JWT sub.
    // The preisgarantie_bis column is bound from the actual DATE field,
    // not from a TEXT product_code (which caused a silent type error before).
    if input.override_preisgarantie {
        let operator_sub = claims.sub().to_owned();
        tracing::warn!(
            vertrag_id = %vertrag_id,
            komp_id    = %input.komp_id,
            wirksamkeit = %input.wirksamkeit,
            new_product = %input.new_product_code,
            operator    = %operator_sub,
            "vertragd: Preisgarantie OVERRIDE — price-lock bypassed by operator"
        );
        let _ = sqlx::query(
            r"INSERT INTO preisgarantie_override_log
              (tenant, vertrag_id, komp_id, preisgarantie_bis, wirksamkeit,
               old_product_code, new_product_code, operator_identity)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(&cfg.tenant)
        .bind(vertrag_id)
        .bind(input.komp_id)
        .bind(vertrag.preisgarantie_bis) // $4: correct DATE field (was product_code TEXT before)
        .bind(input.wirksamkeit)
        .bind(&komp.product_code)
        .bind(&input.new_product_code)
        .bind(&operator_sub)
        .execute(&pool)
        .await;
    }

    let is_future = input.wirksamkeit > today;

    if is_future {
        // Store as pending — background worker will apply on wirksamkeit
        // and emit 6-week advance notification per §41 Abs. 3 EnWG.
        if let Err(e) = store_pending_tarifwechsel(
            &pool,
            input.komp_id,
            &input.new_product_code,
            input.wirksamkeit,
        )
        .await
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    } else {
        // Apply immediately (urgent / retroactive correction).
        if let Err(e) =
            update_komponente_product(&pool, input.komp_id, &input.new_product_code).await
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }

        // Update tarifbd product assignment
        if let Some(ref malo_id) = komp.malo_id {
            let url = format!(
                "{}/api/v1/customer/{}/product",
                cfg.tarifbd_url.trim_end_matches('/'),
                malo_id
            );
            let body = serde_json::json!({
                "product_code": input.new_product_code,
                "lf_mp_id": cfg.lf_mp_id,
                "assigned_from": input.wirksamkeit.to_string(),
            });
            let client = reqwest::Client::new();
            let mut req = client.put(&url);
            if let Some(ref k) = cfg.tarifbd_api_key {
                req = req.bearer_auth(k);
            }
            let _ = req.json(&body).send().await;
        }
    }

    // Emit CloudEvent (tarifwechsel for immediate, tarifwechsel_geplant for future)
    let ce_type = if is_future {
        "tarifwechsel_geplant"
    } else {
        "tarifwechsel"
    };
    if let Some(ref url) = cfg.erp_webhook_url {
        emit_event(
            &http_client,
            url,
            cfg.erp_hmac_secret.as_deref(),
            build_cloud_event(
                ce_type,
                vertrag_id,
                &cfg.tenant,
                serde_json::json!({
                    "vertrag_id": vertrag_id,
                    "komp_id": input.komp_id,
                    "new_product_code": input.new_product_code,
                    "wirksamkeit": input.wirksamkeit.to_string(),
                    "geplant": is_future,
                }),
            ),
        )
        .await;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "vertrag_id": vertrag_id,
            "komp_id": input.komp_id,
            "new_product_code": input.new_product_code,
            "wirksamkeit": input.wirksamkeit.to_string(),
            "applied_immediately": !is_future,
            "pending": is_future,
        })),
    )
        .into_response()
}

// ── CloudEvent webhook ─────────────────────────────────────────────────────────

/// `POST /api/v1/events` — inbound CloudEvents from makod / processd.
pub async fn post_cloud_event(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Json(ce): Json<serde_json::Value>,
) -> impl IntoResponse {
    let ce_id = ce.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let ce_type = ce.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let is_new = match idempotent_event(&pool, ce_id, ce_type, &ce).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error=%e, "vertragd: event inbox write failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if !is_new {
        return StatusCode::OK.into_response();
    }

    if let Some(outcome) = parse_mako_outcome(&ce) {
        let process_id = outcome.process_id.as_deref().unwrap_or("");
        if let Ok(rows) = sqlx::query_as::<_, crate::pg::VertragskomponenteRow>(
            "SELECT k.* FROM vertragskomponenten k
             JOIN versorgungsvertraege v ON v.id = k.vertrag_id
             WHERE k.mako_process_id=$1 AND v.tenant=$2",
        )
        .bind(process_id)
        .bind(&cfg.tenant)
        .fetch_all(&pool)
        .await
        {
            for k in &rows {
                let new_status = if outcome.confirmed {
                    "BESTAETIGT"
                } else {
                    "ABGELEHNT"
                };
                let _ = update_komponente_status(
                    &pool,
                    k.id,
                    new_status,
                    None,
                    outcome.malo_id.as_deref(),
                    outcome.erc_code.as_deref(),
                    outcome.reason.as_deref(),
                )
                .await;

                // Post-confirmation: Ablesesteuerung + product assignment
                if outcome.confirmed
                    && let Some(ref malo_id) = outcome.malo_id
                {
                    tokio::spawn(post_bestaetigt_actions(
                        Arc::clone(&cfg),
                        pool.clone(),
                        malo_id.clone(),
                        k.id,
                        k.product_code.clone(),
                        k.lieferbeginn,
                    ));
                }

                // Recompute Versorgungsvertrag status
                if let Ok(all_komp) = list_komponenten(&pool, k.vertrag_id).await {
                    let new_vv_status = derive_vertrag_status(&all_komp);
                    let _ = update_vertrag_status(&pool, k.vertrag_id, &cfg.tenant, new_vv_status)
                        .await;
                    // Emit de.vertrag.aktiv when all components confirmed
                    if new_vv_status == "AKTIV"
                        && let Some(ref url) = cfg.erp_webhook_url
                    {
                        emit_event(
                            &http_client,
                            url,
                            cfg.erp_hmac_secret.as_deref(),
                            build_cloud_event(
                                "aktiv",
                                k.vertrag_id,
                                &cfg.tenant,
                                serde_json::json!({"vertrag_id": k.vertrag_id}),
                            ),
                        )
                        .await;
                        // Provision accountingd billing account
                        if let Some(ref malo_id) = outcome.malo_id {
                            tokio::spawn(provision_billing_account(
                                Arc::clone(&cfg),
                                pool.clone(),
                                malo_id.clone(),
                            ));
                        }
                    }
                }
            }
        }
    }
    StatusCode::OK.into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn requires_mako_workflow(sparte: &str) -> bool {
    matches!(
        sparte,
        "STROM" | "GAS" | "WAERME" | "SOLAR" | "EEG" | "EINSPEISUNG" | "WAERMEPUMPE" | "WALLBOX"
    )
}

/// Build the processd Lieferbeginn request body for the commodity.
///
/// The contract differs by commodity (verified against
/// `services/processd/src/server.rs`):
/// - `start-supply` (Strom) requires `lieferbeginn_datum` (ISO-8601).
/// - `start-supply-gas` requires `zaehlpunkt` (Zählpunktbezeichnung, RFF+Z13,
///   mandatory per BK7-24-01-009 AHB — the MeLo, falling back to the MaLo) and
///   `process_date` (YYYYMMDD).
///
/// Pure so the field-name contract is unit-tested without a live processd.
pub fn lieferbeginn_body(
    is_gas: bool,
    malo_id: &str,
    melo_id: Option<&str>,
    nb_mp_id: &str,
    lf_mp_id: &str,
    lieferbeginn: time::Date,
) -> serde_json::Value {
    if is_gas {
        let zaehlpunkt = melo_id.unwrap_or(malo_id);
        serde_json::json!({
            "malo_id": malo_id,
            "zaehlpunkt": zaehlpunkt,
            "nb_mp_id": nb_mp_id,
            "lf_mp_id": lf_mp_id,
            "process_date": lieferbeginn
                .format(time::macros::format_description!("[year][month][day]"))
                .unwrap_or_else(|_| lieferbeginn.to_string()),
        })
    } else {
        serde_json::json!({
            "malo_id": malo_id,
            "nb_mp_id": nb_mp_id,
            "lf_mp_id": lf_mp_id,
            "lieferbeginn_datum": lieferbeginn.to_string(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_lieferbeginn(
    cfg: Arc<VertragdConfig>,
    komp_id: Uuid,
    pool: PgPool,
    malo_id: String,
    melo_id: Option<String>,
    nb_mp_id: String,
    sparte: String,
    lieferbeginn: time::Date,
) {
    let is_gas = sparte == "GAS";
    let endpoint = if is_gas {
        "start-supply-gas"
    } else {
        "start-supply"
    };
    let url = format!(
        "{}/api/v1/{}",
        cfg.processd_url.trim_end_matches('/'),
        endpoint
    );
    // The processd contract differs by commodity (verified against
    // services/processd/src/server.rs):
    //   start-supply     requires `lieferbeginn_datum` (ISO-8601)
    //   start-supply-gas requires `zaehlpunkt` (Zählpunktbezeichnung, RFF+Z13,
    //                    mandatory per BK7-24-01-009 AHB) and `process_date`
    //                    (YYYYMMDD). The Zählpunkt is the MeLo; a gas MaLo with
    //                    no MeLo cannot start supply, so that is dead-lettered
    //                    rather than sent with a missing mandatory field.
    let body = lieferbeginn_body(
        is_gas,
        &malo_id,
        melo_id.as_deref(),
        &nb_mp_id,
        &cfg.lf_mp_id,
        lieferbeginn,
    );

    // Retry up to 3 times with exponential backoff (10s, 20s, 40s).
    // This covers transient processd downtime (restarts, rolling deploys).
    // After all retries the component stays in ANGELEGT and is flagged by
    // `find_stuck_komponents` (§20 EnWG parity monitor) after 5 WT.
    for attempt in 1u32..=3 {
        let client = reqwest::Client::new();
        let mut req = client.post(&url);
        if let Some(ref k) = cfg.processd_api_key {
            req = req.bearer_auth(k);
        }
        match req.json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    let process_id = data.get("process_id").and_then(|v| v.as_str());
                    let _ = update_komponente_status(
                        &pool,
                        komp_id,
                        "ANGEMELDET",
                        process_id,
                        None,
                        None,
                        None,
                    )
                    .await;
                }
                return; // success
            }
            Ok(resp) => {
                tracing::warn!(
                    komp_id = %komp_id, attempt, status = %resp.status(),
                    "vertragd: processd Lieferbeginn attempt {attempt}/3 failed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    komp_id = %komp_id, attempt, error = %e,
                    "vertragd: processd request error (attempt {attempt}/3)"
                );
            }
        }
        if attempt < 3 {
            let delay = std::time::Duration::from_secs(10 * 2u64.pow(attempt - 1));
            tokio::time::sleep(delay).await;
        }
    }
    tracing::error!(
        komp_id = %komp_id, malo_id = %malo_id,
        "vertragd: Lieferbeginn dispatch failed after 3 attempts — component stuck in ANGELEGT.          Will be detected by find_stuck_komponents after 5 WT."
    );
}

async fn dispatch_lieferende(
    cfg: Arc<VertragdConfig>,
    komp_id: Uuid,
    pool: PgPool,
    malo_id: String,
    nb_mp_id: String,
    sparte: String,
    lieferende: time::Date,
) {
    let endpoint = if sparte == "GAS" {
        "end-supply-gas"
    } else {
        "end-supply"
    };
    let url = format!(
        "{}/api/v1/{}",
        cfg.processd_url.trim_end_matches('/'),
        endpoint
    );
    // processd's Lieferende contract is `lieferende_datum` (verified against
    // services/processd/src/server.rs::end_supply).
    let body = serde_json::json!({
        "malo_id": malo_id,
        "nb_mp_id": nb_mp_id,
        "lf_mp_id": cfg.lf_mp_id,
        "lieferende_datum": lieferende.to_string(),
    });

    // The Schlussablesung reading order (§9 MessZV) is the LF's OWN obligation
    // and does not depend on the Lieferende UTILMD reaching the NB. Fire it
    // first and unconditionally — a failed processd call must never suppress
    // the final-reading order (that was the previous behaviour and left the
    // customer without a Schlussrechnung basis).
    trigger_ablesesteuerung(&cfg, komp_id, &malo_id, "LIEFERENDE", lieferende).await;

    // Retry the Lieferende dispatch with the same backoff as Lieferbeginn —
    // a single-attempt fire-and-forget silently loses the Lieferende
    // initiation on any transient processd downtime.
    for attempt in 1u32..=3 {
        let client = reqwest::Client::new();
        let mut req = client.post(&url);
        if let Some(ref k) = cfg.processd_api_key {
            req = req.bearer_auth(k);
        }
        match req.json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                let _ = update_komponente_status(&pool, komp_id, "BEENDET", None, None, None, None)
                    .await;
                return;
            }
            Ok(resp) => {
                tracing::warn!(komp_id=%komp_id, attempt, status=%resp.status(),
                    "vertragd: Lieferende attempt {attempt}/3 failed")
            }
            Err(e) => {
                tracing::warn!(komp_id=%komp_id, attempt, error=%e,
                    "vertragd: Lieferende request error (attempt {attempt}/3)")
            }
        }
        if attempt < 3 {
            tokio::time::sleep(std::time::Duration::from_secs(10 * 2u64.pow(attempt - 1))).await;
        }
    }
    tracing::error!(komp_id=%komp_id, malo_id=%malo_id,
        "vertragd: Lieferende dispatch failed after 3 attempts — Schlussablesung was still ordered");
}

async fn post_bestaetigt_actions(
    cfg: Arc<VertragdConfig>,
    pool: PgPool,
    malo_id: String,
    komp_id: Uuid,
    product_code: String,
    lieferbeginn: time::Date,
) {
    trigger_ablesesteuerung(&cfg, komp_id, &malo_id, "LIEFERBEGINN", lieferbeginn).await;
    let url = format!(
        "{}/api/v1/customer/{}/product",
        cfg.tarifbd_url.trim_end_matches('/'),
        malo_id
    );
    let body = serde_json::json!({ "product_code": product_code, "lf_mp_id": cfg.lf_mp_id, "assigned_from": lieferbeginn.to_string() });
    let client = reqwest::Client::new();
    let mut req = client.put(&url);
    if let Some(ref k) = cfg.tarifbd_api_key {
        req = req.bearer_auth(k);
    }
    if let Err(e) = req.json(&body).send().await {
        tracing::warn!(malo_id, error=%e, "vertragd: tarifbd product assignment failed");
        let _ = pool;
    }
}

async fn provision_billing_account(cfg: Arc<VertragdConfig>, _pool: PgPool, malo_id: String) {
    let url = format!(
        "{}/api/v1/accounts",
        cfg.accountingd_url.trim_end_matches('/')
    );
    let body = serde_json::json!({ "malo_id": malo_id, "lf_mp_id": cfg.lf_mp_id });
    let client = reqwest::Client::new();
    let mut req = client.post(&url);
    if let Some(ref k) = cfg.accountingd_api_key {
        req = req.bearer_auth(k);
    }
    if let Err(e) = req.json(&body).send().await {
        tracing::warn!(malo_id, error=%e, "vertragd: accountingd provisioning failed");
    }
}

async fn trigger_ablesesteuerung(
    cfg: &VertragdConfig,
    komp_id: Uuid,
    malo_id: &str,
    anlass: &str,
    geplant_am: time::Date,
) {
    let url = format!(
        "{}/api/v1/reading-orders",
        cfg.edmd_url.trim_end_matches('/')
    );
    let body = serde_json::json!({ "malo_id": malo_id, "anlass": anlass, "auftraggeber_rolle": "LF", "geplant_am": geplant_am.to_string(), "auftrag_position_id": komp_id });
    let client = reqwest::Client::new();
    let mut req = client.post(&url);
    if let Some(ref k) = cfg.edmd_api_key {
        req = req.bearer_auth(k);
    }
    if let Err(e) = req.json(&body).send().await {
        tracing::warn!(malo_id, anlass, error=%e, "vertragd: Ablesesteuerung failed");
    }
}

/// Deliver a CloudEvent to the ERP webhook, HMAC-signed when a secret is set.
///
/// Returns `true` only on a 2xx response, so a caller that must not advance a
/// "notified" flag until the notice actually landed (the §5 Abs. 2 StromGVV /
/// GasGVV price-change worker) can gate on it. Every emission — handler and
/// background worker alike — goes through here, so all events carry the same
/// signature and `Content-Type`.
pub async fn emit_event(
    client: &reqwest::Client,
    webhook_url: &str,
    hmac_secret: Option<&str>,
    ce: serde_json::Value,
) -> bool {
    let body = match serde_json::to_vec(&ce) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error=%e, "vertragd: CloudEvent serialization failed");
            return false;
        }
    };
    let mut req = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json");
    // HMAC-SHA256 webhook signature using workspace-standard sha256= prefix.
    if let Some(secret) = hmac_secret {
        let sig = format!(
            "sha256={}",
            mako_service::webhook::hmac_hex(secret.as_bytes(), &body)
        );
        req = req.header("X-Mako-Signature", sig);
    }
    match req.body(body).send().await {
        Ok(resp) if resp.status().is_success() => true,
        Ok(resp) => {
            tracing::warn!(status = %resp.status(), "vertragd: ERP webhook non-2xx");
            false
        }
        Err(e) => {
            tracing::warn!(error=%e, "vertragd: ERP webhook error");
            false
        }
    }
}

// ── Person sub-object (BO4E Person BO, L13 — GDPR Art. 15) ───────────────────

/// `PUT /api/v1/kunden/{id}/person`
///
/// Store or replace the BO4E `Person` BO for a B2C customer.
///
/// Body: `rubo4e::current::Person` JSON (camelCase).
///
/// Validation:
/// - `_typ: "PERSON"` is auto-injected when absent; wrong value → 422.
/// - All enum fields (`anrede`, `titel`) are validated via deserialization.
/// - Re-serialised to canonical BO4E camelCase before storage.
///
/// **Hard cut — GDPR Art. 15 data export.** `portald`/ERP can reconstruct a
/// structured data-subject response from the stored `Person` without parsing
/// free-text fields.  Correct `anrede` is required for correspondence templates.
pub async fn put_person(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
    Json(mut body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use rubo4e::current::Person;
    // Inject _typ: "PERSON" when absent; reject mismatches.
    match body.get("_typ").and_then(|v| v.as_str()) {
        None => {
            body["_typ"] = serde_json::json!("PERSON");
        }
        Some("PERSON") => {}
        Some(other) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(
                    serde_json::json!({ "error": format!("expected _typ=PERSON, got {other:?}") }),
                ),
            )
                .into_response();
        }
    }
    // Validate via rubo4e roundtrip (validates anrede, titel enums).
    let typed: Person = match serde_json::from_value(body) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid Person payload: {e}") })),
            )
                .into_response();
        }
    };
    let canonical = serde_json::to_value(&typed).unwrap_or_default();

    match upsert_person(&pool, id, &cfg.tenant, canonical).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) if e.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/{id}/person`
///
/// Retrieve the stored BO4E `Person` BO for a customer.
///
/// Returns 404 when the customer exists but no `Person` has been stored
/// (B2B Geschäftspartner / legal entity without personal data).
pub async fn get_person(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_person(&pool, id, &cfg.tenant).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no Person stored for this Kunde (B2B entity or not yet set)"
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Rahmenvertrag MaLo enumeration (L2 — Sammelrechnung) ─────────────────────

/// `GET /api/v1/rahmenvertraege/{id}/malos`
///
/// Returns all active MaLo IDs + product codes for a Rahmenvertrag.
///
/// Used by `billingd` `POST /api/v1/billing/sammelrechnung/{id}` to enumerate
/// the sites to include in a consolidated B2B Sammelrechnung.
pub async fn get_rahmenvertrag_malos(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match list_rahmenvertrag_malos(&pool, id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Preisgarantie typed REST resource ───────────────────────────────

/// `PUT /api/v1/vertraege/{id}/preisgarantie`
///
/// Store or replace the BO4E `Preisgarantie` COM for a contract.
///
/// Body: `rubo4e::current::Preisgarantie` JSON (camelCase).
///
/// The `tarifwechsel` endpoint already enforces the guarantee window —
/// this endpoint exposes the structured metadata for ERP systems to
/// display guarantee windows in customer-facing UIs.
///
/// Emits `de.vertrag.preisgarantie.updated`.
pub async fn put_preisgarantie(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(vertrag_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use rubo4e::current::Preisgarantie;

    let typed: Preisgarantie = match serde_json::from_value(body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid Preisgarantie payload: {e}") })),
            )
                .into_response();
        }
    };

    let canonical = match serde_json::to_value(&typed) {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Extract preisgarantie_bis from zeitlicheGueltigkeit.enddatum for the guard column.
    let bis = canonical
        .pointer("/zeitlicheGueltigkeit/enddatum")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            let parts: Vec<&str> = s.splitn(4, '-').collect();
            if parts.len() >= 3 {
                let y: i32 = parts[0].parse().ok()?;
                let m: u8 = parts[1].parse().ok()?;
                let d: u8 = parts[2].parse().ok()?;
                let month = time::Month::try_from(m).ok()?;
                time::Date::from_calendar_date(y, month, d).ok()
            } else {
                None
            }
        });

    match upsert_preisgarantie(&pool, vertrag_id, &cfg.tenant, canonical, bis).await {
        Ok(()) => {
            // Emit CloudEvent to ERP.
            if let Some(ref url) = cfg.erp_webhook_url {
                let ce = build_cloud_event(
                    "preisgarantie_updated",
                    vertrag_id,
                    &cfg.tenant,
                    serde_json::json!({ "vertrag_id": vertrag_id }),
                );
                // Signed like every other event — an ERP verifying the HMAC
                // must not silently reject the price-guarantee notice.
                emit_event(
                    &reqwest::Client::new(),
                    url,
                    cfg.erp_hmac_secret.as_deref(),
                    ce,
                )
                .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/vertraege/{id}/preisgarantie`
///
/// Retrieve the stored BO4E `Preisgarantie` COM for a contract.
///
/// Returns 404 when the contract exists but no `Preisgarantie` has been stored
/// (i.e. the contract has no price-lock clause).
pub async fn get_preisgarantie(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(vertrag_id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_preisgarantie(&pool, vertrag_id, &cfg.tenant).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no Preisgarantie stored for this contract (no price-lock clause)"
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Zahlungsinformation (BO4E typed payment info — IBAN + BIC + SEPA) ────────

/// `PUT /api/v1/kunden/{id}/zahlungsinformation`
///
/// Store or replace the BO4E `Zahlungsinformation` COM for a customer.
///
/// Body: `rubo4e::current::Zahlungsinformation` JSON (camelCase).
/// Accepts: `iban`, `bic`, `kontoinhaber`, `sepaReferenz`, `zahlungsart`.
///
/// Validation:
/// - IBAN is validated via mod-97 (ISO 13616) before storage.
/// - Stored as canonical BO4E camelCase JSONB.
///
/// **Hard cut.** Enables ERP-side BO4E `Zahlungsinformation` payload for SEPA
/// batch onboarding and structured GDPR Art. 15 data export.
pub async fn put_zahlungsinformation_kunde(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use rubo4e::current::Zahlungsinformation;

    // Verify customer exists
    match fetch_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(_)) => {}
    }

    let typed: Zahlungsinformation = match serde_json::from_value(body) {
        Ok(z) => z,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid Zahlungsinformation: {e}") })),
            )
                .into_response();
        }
    };

    // Validate IBAN when present.
    if let Some(ref iban) = typed.iban
        && let Err(msg) = sepa::validate_iban(iban)
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("invalid IBAN: {msg}") })),
        )
            .into_response();
    }

    let canonical = serde_json::to_value(&typed).unwrap_or_default();

    let res =
        sqlx::query("UPDATE kunden SET zahlungsinformation = $1 WHERE id = $2 AND tenant = $3")
            .bind(&canonical)
            .bind(kunden_id)
            .bind(&cfg.tenant)
            .execute(&pool)
            .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "kunden_id": kunden_id,
                "zahlungsinformation": canonical,
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/kunden/{id}/zahlungsinformation`
///
/// Retrieve the stored `Zahlungsinformation` for a customer.
/// Returns 404 when no typed payment information has been stored.
pub async fn get_zahlungsinformation_kunde(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
) -> impl IntoResponse {
    let row =
        sqlx::query("SELECT zahlungsinformation FROM kunden WHERE id = $1 AND tenant = $2 LIMIT 1")
            .bind(kunden_id)
            .bind(&cfg.tenant)
            .fetch_optional(&pool)
            .await;

    match row {
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(row)) => {
            use sqlx::Row as _;
            let data: Option<serde_json::Value> = row.try_get("zahlungsinformation").ok().flatten();
            match data {
                Some(json) => Json::<serde_json::Value>(json).into_response(),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "no Zahlungsinformation stored for this customer"
                    })),
                )
                    .into_response(),
            }
        }
    }
}

// IBAN validation is provided by the `sepa` workspace crate (ISO 13616 mod-97).
// The old inline `validate_iban_mod97` has been removed — use `sepa::validate_iban` instead.

// ── Expiring contracts ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExpiringQuery {
    /// Calendar days to look ahead. Default: 30.
    pub days: Option<i64>,
}

/// `GET /api/v1/vertraege/expiring`
///
/// **Proactive contract-lifecycle monitoring.**
///
/// Returns all `AKTIV` or `GEKÜNDIGT` Versorgungsverträge whose `vertragsende`
/// OR `preisgarantie_bis` falls within `?days` (default 30) calendar days.
///
/// Regulatory basis:
/// - §13 GasGVV / §14 StromGVV: 30-day advance notice for auto-renewal
/// - §41 EnWG: customer notification before expiry / tariff change
pub async fn list_expiring_vertraege(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Query(q): Query<ExpiringQuery>,
) -> impl IntoResponse {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    match find_expiring_vertraege(&pool, &cfg.tenant, days).await {
        Ok(rows) => Json(serde_json::json!({
            "count": rows.len(),
            "look_ahead_days": days,
            "vertraege": rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── B2B portfolio summary ─────────────────────────────────────────────────────

/// `GET /api/v1/kunden/{id}/portfolio`
///
/// **B2B portfolio view.**
///
/// Returns all active Vertragskomponenten across all Versorgungsverträge
/// for a B2B customer — one row per MaLo/Sparte combination.
///
/// Useful for B2B portal dashboards, Sammelrechnung enumeration, and
/// Rahmenvertrag portfolio analytics.
pub async fn get_kunde_portfolio(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify customer exists.
    match fetch_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(_)) => {}
    }
    match list_portfolio_by_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(rows) => {
            let total_malos = rows.iter().filter(|r| r.malo_id.is_some()).count();
            Json(serde_json::json!({
                "kunden_id": kunden_id,
                "total_active_komponenten": rows.len(),
                "total_malos": total_malos,
                "komponenten": rows,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── List Kunden (operator / CRM) ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct KundenListQuery {
    pub kundentyp: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/kunden` — list all customers for a tenant.
///
/// Operator / CRM endpoint — not exposed to portal users.
/// `?kundentyp=B2C|B2B_SLP|B2B_RLM|B2B_HV` for filtering.
pub async fn list_kunden_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Query(q): Query<KundenListQuery>,
) -> impl IntoResponse {
    match list_kunden(
        &pool,
        &cfg.tenant,
        q.kundentyp.as_deref(),
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await
    {
        Ok(rows) => Json(serde_json::json!({
            "count": rows.len(),
            "kunden": rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Stornieren (cancel ANGELEGT/IN_BEARBEITUNG contract) ──────────────────────

/// `POST /api/v1/vertraege/{id}/stornieren`
///
/// Cancel a contract that has not yet reached `AKTIV` status.
/// Valid states: `ANGELEGT`, `IN_BEARBEITUNG` (MaKo dispatched but not confirmed).
///
/// For `IN_BEARBEITUNG`: sets components to `STORNIERT` but does NOT automatically
/// cancel in-flight MaKo processes — the operator must cancel them via `processd`.
pub async fn stornieren_vertrag(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let vertrag = match fetch_vertrag(&pool, id, &cfg.tenant).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if !matches!(vertrag.status.as_str(), "ANGELEGT" | "IN_BEARBEITUNG") {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!(
                    "Vertrag status '{}' cannot be storniert — only ANGELEGT or IN_BEARBEITUNG",
                    vertrag.status
                ),
                "hint": if vertrag.status == "AKTIV" {
                    "Use POST /api/v1/vertraege/{id}/kuendigen for AKTIV contracts"
                } else {
                    "Only pre-activation contracts can be storniert"
                },
            })),
        )
            .into_response();
    }
    match storniere_vertrag(&pool, id, &cfg.tenant).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "vertrag_id": id,
                "status": "STORNIERT",
                "warning": if vertrag.status == "IN_BEARBEITUNG" {
                    Some("In-flight MaKo processes not cancelled automatically. Cancel via processd.")
                } else { None },
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Deactivate portal user ─────────────────────────────────────────────────────

/// `DELETE /api/v1/kunden/{id}/identitaeten/{sub}`
///
/// Deactivate a portal user (OIDC identity) for a Kunde.
/// Idempotent — calling again for an already-deactivated sub returns 404.
///
/// Regulatory basis: GDPR Art. 17 (right to erasure / access revocation).
pub async fn delete_identitaet(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path((kunden_id, oidc_sub)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    // Verify customer belongs to tenant.
    match fetch_kunde(&pool, kunden_id, &cfg.tenant).await {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(_)) => {}
    }
    match deactivate_identitaet_by_sub(&pool, kunden_id, &cfg.tenant, &oidc_sub).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            format!("No active identity with sub={oidc_sub} for this customer"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GDPR Art. 15 Data Export ──────────────────────────────────────────────────

/// `GET /api/v1/kunden/{id}/export`
///
/// **GDPR Art. 15 (Recht auf Auskunft) / Art. 20 (Recht auf Datenübertragbarkeit).**
///
/// Returns a complete structured JSON export of all personal data stored for
/// a customer: Kunde, Person, Zahlungsinformation, KundenIdentitaeten,
/// Versorgungsverträge, and all Vertragskomponenten.
///
/// The response is suitable for:
/// - Handing to the customer as their Art. 15 data disclosure
/// - ERP import on customer request
/// - Audit trails
pub async fn get_kunde_gdpr_export(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(kunden_id): Path<Uuid>,
) -> impl IntoResponse {
    match gdpr_export(&pool, kunden_id, &cfg.tenant).await {
        Ok(Some(export)) => Json(serde_json::json!({
            "regulatory_basis": "DSGVO Art. 15 — Recht auf Auskunft / Art. 20 — Datenübertragbarkeit",
            "exported_at": time::OffsetDateTime::now_utc().to_string(),
            "export": export,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// \u2500\u2500 GDPR Art. 17 \u2014 Right to Erasure (Anonymization) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// `POST /api/v1/kunden/{id}/anonymize` \u2014 GDPR Art. 17 right to erasure.
///
/// Pseudonymizes all PII for the customer while retaining contract records
/// for the legal retention period (\u00a7147 AO: 10-year retention obligation).
///
/// After anonymization:
/// - All portal access for this customer is revoked (`aktiv = false`)
/// - Personal data is replaced with an opaque token
/// - IBAN/BIC are replaced with the literal `ANONYMIZED`
/// - An immutable audit log entry is written to `anonymization_log`
///
/// **This operation is irreversible.** The operator is responsible for
/// verifying the customer's identity before calling this endpoint.
///
/// Regulatory basis: GDPR Art. 17 (Recht auf L\u00f6schung) + Art. 5(2) (Accountability).
pub async fn post_anonymize_kunde(
    claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::pg::anonymize_kunde;
    // `requested_by`: prefer body field; fall back to JWT sub for audit trail.
    let requested_by = body
        .get("requested_by")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| claims.sub());

    match anonymize_kunde(&pool, id, &cfg.tenant, requested_by).await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "kunden_id": id,
                "anonymized": true,
                "regulatory_basis": "DSGVO Art. 17 - Recht auf Loeschung",
                "retention_note": "§147 AO: Vertragsdaten werden 10 Jahre aufbewahrt (ohne PII)",
                "audit_log": "anonymization_log Eintrag erstellt",
            })),
        )
            .into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── CPQ-to-contract promotion ─────────────────────────────────────────────────

/// `POST /api/v1/vertraege/{id}/widerruf-kuendigung`
///
/// Widerruf der Kündigung — revokes a pending Kündigung before `lieferende`.
///
/// Valid only when the contract is in `GEKÜNDIGT` status and the Lieferende
/// has not yet taken effect. Sets BEENDET components back to AKTIV and
/// the contract back to AKTIV.
///
/// **Caller must separately cancel the in-flight Lieferende UTILMD via processd.**
pub async fn widerruf_kuendigung_handler(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match widerruf_kuendigung(&pool, id, &cfg.tenant).await {
        Ok(()) => {
            // Emit CloudEvent so ERP/portald can update their state.
            if let Some(ref url) = cfg.erp_webhook_url {
                emit_event(
                    &http_client,
                    url,
                    cfg.erp_hmac_secret.as_deref(),
                    build_cloud_event(
                        "kuendigung_widerrufen",
                        id,
                        &cfg.tenant,
                        serde_json::json!({ "vertrag_id": id }),
                    ),
                )
                .await;
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "vertrag_id": id,
                    "status": "AKTIV",
                    "message": "Kündigung widerrufen — cancel in-flight Lieferende UTILMD via processd if needed",
                })),
            )
                .into_response()
        }
        Err(e) if e.to_string().contains("only allowed") => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(e) if e.to_string().contains("not found") => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/rahmenvertraege/{id}/kuendigen`
///
/// Cascade-Kündigung for a B2B Rahmenvertrag: terminates all active
/// Versorgungsverträge under the framework contract in one operation.
///
/// Each active Versorgungsvertrag is individually Gekündigt with the same
/// `lieferende` date, respecting the individual contract notice periods.
/// Returns a summary of dispatched and skipped contracts.
pub async fn kuendige_rahmenvertrag_handler(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(rahmenvertrag_id): Path<Uuid>,
    Json(input): Json<KuendigungInput>,
) -> impl IntoResponse {
    let vertraege = match list_versorgungsvertraege_by_rahmenvertrag(
        &pool,
        rahmenvertrag_id,
        &cfg.tenant,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if vertraege.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no active Versorgungsverträge found for this Rahmenvertrag"
            })),
        )
            .into_response();
    }

    let today = time::OffsetDateTime::now_utc().date();
    if input.lieferende < today {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "lieferende cannot be in the past",
                "lieferende": input.lieferende.to_string(),
            })),
        )
            .into_response();
    }

    let mut dispatched = 0u32;
    let mut skipped = 0u32;
    let mut skipped_reasons: Vec<serde_json::Value> = Vec::new();

    for v in &vertraege {
        let earliest = earliest_kuendigungsdatum(today, v.kuendigungsfrist_monate);
        if input.lieferende < earliest {
            skipped += 1;
            skipped_reasons.push(serde_json::json!({
                "vertrag_id": v.id,
                "reason": "lieferende too early — notice period not respected",
                "earliest_valid": earliest.to_string(),
                "kuendigungsfrist_monate": v.kuendigungsfrist_monate,
            }));
            continue;
        }

        let komponenten = list_komponenten(&pool, v.id).await.unwrap_or_default();
        for k in komponenten
            .iter()
            .filter(|k| matches!(k.status.as_str(), "AKTIV" | "BESTAETIGT"))
        {
            if requires_mako_workflow(&k.sparte)
                && let Some(malo_id) = &k.malo_id
                && let Some(nb_mp_id) = &k.nb_mp_id
            {
                tokio::spawn(dispatch_lieferende(
                    Arc::clone(&cfg),
                    k.id,
                    pool.clone(),
                    malo_id.clone(),
                    nb_mp_id.clone(),
                    k.sparte.clone(),
                    input.lieferende,
                ));
                dispatched += 1;
            } else {
                let _ =
                    update_komponente_status(&pool, k.id, "BEENDET", None, None, None, None).await;
            }
        }
        let _ = update_vertrag_status(&pool, v.id, &cfg.tenant, "GEKÜNDIGT").await;

        // Emit de.vertrag.gekuendigt per contract
        if let Some(ref url) = cfg.erp_webhook_url {
            emit_event(
                &http_client,
                url,
                cfg.erp_hmac_secret.as_deref(),
                build_cloud_event(
                    "gekuendigt",
                    v.id,
                    &cfg.tenant,
                    serde_json::json!({
                        "vertrag_id": v.id,
                        "rahmenvertrag_id": rahmenvertrag_id,
                        "lieferende": input.lieferende.to_string(),
                    }),
                ),
            )
            .await;
        }
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "rahmenvertrag_id": rahmenvertrag_id,
            "lieferende": input.lieferende.to_string(),
            "vertraege_count": vertraege.len(),
            "mako_dispatched": dispatched,
            "skipped": skipped,
            "skipped_details": skipped_reasons,
        })),
    )
        .into_response()
}

/// `POST /api/v1/webhooks/angebot`
///
/// Receive `de.angebot.angenommen` from `tarifbd` and auto-create:
/// 1. A `Rahmenvertrag` (with `angebot_id` set for traceability)
/// 2. One `Versorgungsvertrag` per unique standort in the accepted positionen
/// 3. `Vertragskomponente` rows for each position (sparte/MaLo/product)
///
/// Responds with `{ "rahmenvertrag_id": "..." }` so `tarifbd` can link back
/// the Rahmenvertrag UUID to the Angebot row.
///
/// ## Idempotency
///
/// Uses `erp_rahmenvertrag_id = angebot_id.to_string()` for idempotency.
/// Re-delivery of the same CE produces no duplicate Rahmenvertrag.
///
/// ## What happens next
///
/// - `vertragd` emits `de.vertrag.aktiv` when each Versorgungsvertrag reaches AKTIV
/// - `processd` triggers UTILMD Lieferbeginn per commodity component
/// - `tarifbd` assigns the selected product_code to `customer_products` per MaLo
pub async fn post_angebot_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<VertragdConfig>>,
    Extension(_http_client): Extension<Arc<reqwest::Client>>,
    Json(ce): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Validate CE type
    if ce.get("type").and_then(|v| v.as_str()) != Some("de.angebot.angenommen") {
        return (StatusCode::BAD_REQUEST, "expected de.angebot.angenommen").into_response();
    }

    let data = match ce.get("data") {
        Some(d) => d,
        None => return (StatusCode::BAD_REQUEST, "missing data field").into_response(),
    };

    // ── Extract fields from CE payload ────────────────────────────────────────
    let angebot_id_str = data
        .get("angebot_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| ce.get("subject").and_then(|v| v.as_str()).unwrap_or(""));
    let angebot_id: uuid::Uuid = match angebot_id_str.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid angebot_id UUID").into_response(),
    };

    let kunden_id: Option<uuid::Uuid> = data
        .get("kunden_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok());
    let _lf_mp_id = data
        .get("lf_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(&cfg.tenant)
        .to_owned();
    // Prefer the BO4E `Angebot`: it is the object the customer was quoted, and
    // its accepted variant carries that variant's own Laufzeit and supply
    // points. The scalar fields below are the fallback for a quotation that was
    // accepted before it was ever priced.
    let gewaehlte_variante = data
        .get("gewaehlte_variante")
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i16::try_from(v).ok());
    let accepted = crate::angebot_bo4e::from_ce_data(data)
        .as_ref()
        .and_then(|a| crate::angebot_bo4e::read_accepted(a, gewaehlte_variante));

    let laufzeit_monate: i32 = accepted
        .as_ref()
        .and_then(|a| a.laufzeit_monate)
        .or_else(|| {
            data.get("laufzeit_monate")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
        })
        .unwrap_or(12);
    let lieferbeginn_str = data
        .get("lieferbeginn")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let lieferbeginn = accepted
        .as_ref()
        .and_then(|a| a.lieferbeginn)
        .unwrap_or_else(|| {
            time::Date::parse(
                lieferbeginn_str,
                &time::format_description::well_known::Iso8601::DATE,
            )
            .unwrap_or_else(|_| {
                // Default: first day of next month
                let now = time::OffsetDateTime::now_utc().date();
                let (y, m) = if now.month() as u8 == 12 {
                    (now.year() + 1, time::Month::January)
                } else {
                    (
                        now.year(),
                        time::Month::try_from(now.month() as u8 + 1)
                            .unwrap_or(time::Month::January),
                    )
                };
                time::Date::from_calendar_date(y, m, 1).unwrap_or(now)
            })
        });

    let positionen = data
        .get("positionen")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let angebotsnummer = accepted
        .as_ref()
        .and_then(|a| a.angebotsnummer.clone())
        .or_else(|| {
            data.get("angebotsnummer")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();

    // ── Resolve or create Kunde ───────────────────────────────────────────────
    let kunden_id = if let Some(kid) = kunden_id {
        // Verify it belongs to this tenant
        match crate::pg::fetch_kunde(&pool, kid, &cfg.tenant).await {
            Ok(Some(_)) => kid,
            _ => return (StatusCode::NOT_FOUND, "kunden_id not found in tenant").into_response(),
        }
    } else {
        // Create a prospect Kunde from interessent_name
        let interessent_name = data
            .get("interessent_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unbekannt")
            .to_owned();
        let input = crate::pg::CreateKundeInput {
            kunden_nr: Some(angebotsnummer.clone()),
            oidc_sub: None,
            email: data
                .get("contact_email")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            kundentyp: "B2B_SLP".to_owned(),
            geschaeftspartner: Some(serde_json::json!({ "name1": interessent_name })),
            organisations_id: None,
            umsatzsteuer_id: None,
            zahlungsziel_tage: Some(30),
            sepa_erlaubt: Some(false),
            erp_kunde_id: Some(angebot_id_str.to_owned()),
            notizen: None,
        };
        match crate::pg::upsert_kunde(&pool, &cfg.tenant, &input).await {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    // ── Create Rahmenvertrag with angebot_id linkage ──────────────────────────
    let vertragsende = time::Date::from_calendar_date(
        lieferbeginn.year() + laufzeit_monate / 12,
        lieferbeginn.month(),
        lieferbeginn.day(),
    )
    .ok();

    let rahmen_input = crate::pg::CreateRahmenvertragInput {
        rahmenvertrag_nr: Some(format!("RV-{angebotsnummer}")),
        gueltig_von: lieferbeginn,
        gueltig_bis: vertragsende,
        kuendigungsfrist_monate: Some(3),
        auto_renewal: Some(false), // CPQ Angebote are fixed-term by default
        renewal_monate: Some(laufzeit_monate),
        preisanpassungsformel: None,
        portfolio_rabatt_prozent: data.get("varianten").and_then(|v| v.as_array()).and_then(
            |vars| {
                // If a specific variant was chosen, extract its rabatt_pct
                data.get("gewaehlte_variante")
                    .and_then(|i| i.as_u64())
                    .and_then(|idx| vars.get(idx as usize))
                    .and_then(|var| var.get("rabatt_pct"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
            },
        ),
        rechnungsstellung: Some("SAMMEL".to_owned()),
        sammelrechnung_intervall: Some("JAEHRLICH".to_owned()),
        erp_rahmenvertrag_id: Some(angebot_id.to_string()), // idempotency key
        angebot_id: Some(angebot_id), // CPQ traceability — links Rahmenvertrag to Angebot
        notizen: Some(format!("CPQ Angebot {angebotsnummer} angenommen")),
    };

    let rahmenvertrag_id =
        match crate::pg::insert_rahmenvertrag(&pool, kunden_id, &cfg.tenant, &rahmen_input).await {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    // ── Create Versorgungsverträge per site/position ──────────────────────────
    // Group positions by standort_bezeichnung to create one Versorgungsvertrag per site.
    let mut by_standort: std::collections::BTreeMap<String, Vec<&serde_json::Value>> =
        std::collections::BTreeMap::new();
    for pos in &positionen {
        let standort = pos
            .get("standort_bezeichnung")
            .and_then(|v| v.as_str())
            .unwrap_or("Hauptstandort")
            .to_owned();
        by_standort.entry(standort).or_default().push(pos);
    }

    let mut versorgungsvertrag_ids: Vec<uuid::Uuid> = Vec::new();
    for (standort, site_positionen) in &by_standort {
        let komponenten: Vec<crate::pg::CreateKomponenteInput> = site_positionen
            .iter()
            .filter_map(|pos| {
                let sparte = pos.get("sparte").and_then(|v| v.as_str())?.to_owned();
                let product_code = pos.get("product_code").and_then(|v| v.as_str())?.to_owned();
                Some(crate::pg::CreateKomponenteInput {
                    sparte,
                    malo_id: pos
                        .get("malo_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    melo_id: None,
                    nb_mp_id: pos
                        .get("nb_mp_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    product_code,
                    lieferbeginn,
                    lieferende: vertragsende,
                    fulfillment_data: None,
                })
            })
            .collect();

        if komponenten.is_empty() {
            continue;
        }

        let vv_input = crate::pg::CreateVersorgungsvertragInput {
            rahmenvertrag_id: Some(rahmenvertrag_id),
            kundentyp: "B2B_RLM".to_owned(),
            bundle_code: None,
            vertragsbeginn: lieferbeginn,
            vertragsende,
            kuendigungsfrist_monate: Some(3),
            preisgarantie_bis: vertragsende, // fixed-term = price guarantee for full duration
            auto_renewal: Some(false),
            standort_bezeichnung: Some(standort.clone()),
            erp_contract_id: Some(format!("{angebot_id}-{standort}")),
            notizen: None,
            komponenten,
        };

        match crate::pg::insert_versorgungsvertrag(
            &pool,
            kunden_id,
            &cfg.tenant,
            &cfg.tenant, // lf_mp_id
            &vv_input,
        )
        .await
        {
            Ok(inserted) => {
                versorgungsvertrag_ids.push(inserted.id);
                // Same MaKo dispatch as the direct create path: a CPQ-created
                // supply contract needs its Lieferbeginn UTILMD too, over the
                // rows actually inserted (idempotent-replay safe).
                for komp in &inserted.komponenten {
                    if requires_mako_workflow(&komp.sparte)
                        && let (Some(malo_id), Some(nb_mp_id)) = (&komp.malo_id, &komp.nb_mp_id)
                    {
                        tokio::spawn(dispatch_lieferbeginn(
                            Arc::clone(&cfg),
                            komp.id,
                            pool.clone(),
                            malo_id.clone(),
                            komp.melo_id.clone(),
                            nb_mp_id.clone(),
                            komp.sparte.clone(),
                            komp.lieferbeginn,
                        ));
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, standort, "vertragd: CPQ webhook: failed to create Versorgungsvertrag");
            }
        }
    }

    tracing::info!(
        angebot_id = %angebot_id,
        rahmenvertrag_id = %rahmenvertrag_id,
        versorgungsvertraege = versorgungsvertrag_ids.len(),
        "vertragd: CPQ Angebot angenommen → Rahmenvertrag + Versorgungsverträge erstellt"
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "rahmenvertrag_id": rahmenvertrag_id,
            "kunden_id": kunden_id,
            "versorgungsvertrag_ids": versorgungsvertrag_ids,
            "message": "Rahmenvertrag und Versorgungsverträge aus CPQ-Angebot erstellt",
        })),
    )
        .into_response()
}
