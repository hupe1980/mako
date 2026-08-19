//! Kunden, portal identities, and the DSGVO data-subject rights.

use std::{collections::HashSet, sync::Arc};

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_service::{ApiError, ApiResult, oidc::Claims};
use serde::Deserialize;
use uuid::Uuid;

use super::{Ctx, ok, require_kunde};
use crate::pg;

// ── Kunde ─────────────────────────────────────────────────────────────────────

/// `POST /api/v1/kunden` — create or update a customer profile (B2C or B2B).
///
/// Idempotent on `erp_kunde_id`. When `oidc_sub` is supplied the primary portal
/// identity is created in the same transaction.
pub async fn create_kunde(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Json(input): Json<pg::CreateKundeInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let id = pg::upsert_kunde(&ctx.pool, ctx.tenant(), &input)
        .await
        .map_err(ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "kunden_id": id })),
    ))
}

/// `GET /api/v1/kunden/{id}` — customer profile, active MaLos and portal users.
pub async fn get_kunde(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let kunde = require_kunde(&ctx, id).await?;
    let (malo_ids, identitaeten) = tokio::try_join!(
        pg::list_aktive_malo_ids(&ctx.pool, id, ctx.tenant()),
        pg::list_identitaeten(&ctx.pool, id, ctx.tenant()),
    )
    .map_err(ApiError::Internal)?;
    ok(serde_json::json!({
        "kunde": kunde,
        "active_malo_ids": malo_ids,
        "identitaeten": identitaeten,
    }))
}

/// `PUT /api/v1/kunden/{id}` — partial update; absent fields keep their value.
pub async fn update_kunde(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(input): Json<pg::UpdateKundeInput>,
) -> ApiResult<StatusCode> {
    if pg::update_kunde(&ctx.pool, id, ctx.tenant(), &input)
        .await
        .map_err(ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[derive(Deserialize)]
pub struct KundenListQuery {
    pub kundentyp: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/kunden` — operator list view.
pub async fn list_kunden(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<KundenListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::list_kunden(
        &ctx.pool,
        ctx.tenant(),
        q.kundentyp.as_deref(),
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await
    .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "kunden": rows }))
}

/// `GET /api/v1/kunden/by-sub/{sub}` — resolve an OIDC subject to a customer.
///
/// Used by `portald` for resource-level authorization: the response carries
/// exactly the MaLos this identity may see, already narrowed by its
/// `standort_filter`.
pub async fn get_kunde_by_sub(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(oidc_sub): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let kunde = pg::fetch_kunde_by_sub(&ctx.pool, &oidc_sub, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    let identity = pg::fetch_identitaet_by_sub(&ctx.pool, &oidc_sub, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    let vertraege = pg::list_vertraege_by_kunde(&ctx.pool, kunde.id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    let mut malo_ids = pg::list_aktive_malo_ids(&ctx.pool, kunde.id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    if let Some(filter) = identity.as_ref().and_then(|i| i.standort_filter.as_deref()) {
        let scoped = scoped_malos(&ctx, &vertraege, filter).await?;
        malo_ids.retain(|m| scoped.contains(m));
    }
    ok(serde_json::json!({
        "kunde": kunde,
        "active_malo_ids": malo_ids,
        "vertraege_count": vertraege.len(),
        "rolle": identity.as_ref().map(|i| &i.rolle),
        "standort_filter": identity.as_ref().and_then(|i| i.standort_filter.as_deref()),
    }))
}

/// The MaLos reachable from the contracts matching a site scope.
async fn scoped_malos(
    ctx: &Ctx,
    vertraege: &[pg::VersorgungsvertragRow],
    filter: &str,
) -> ApiResult<HashSet<String>> {
    let mut out = HashSet::new();
    for v in vertraege
        .iter()
        .filter(|v| v.standort_bezeichnung.as_deref() == Some(filter))
    {
        for komp in pg::list_komponenten(&ctx.pool, v.id)
            .await
            .map_err(ApiError::Internal)?
        {
            if let Some(m) = komp.malo_id {
                out.insert(m);
            }
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct AuthenticateQuery {
    pub malo_id: String,
}

/// `GET /api/v1/kunden/authenticate?malo_id=…` — the portal authorization check.
///
/// The `sub` comes from the **verified** token, then this answers whether that
/// customer owns `malo_id` within their site scope.
///
/// ## Anti-enumeration
///
/// Every "not authorized" outcome — unknown sub, sub with no customer, customer
/// that does not own the MaLo, MaLo outside the identity's `standort_filter` —
/// returns the **same 403**. A distinct 404 for "no such customer" would let a
/// holder of any valid token probe which subjects and MaLo IDs exist
/// (DSGVO Art. 32). Only a genuine server fault returns 500.
pub async fn authenticate(
    claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<AuthenticateQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let sub = claims.sub().to_owned();
    let kunde = pg::fetch_kunde_by_sub(&ctx.pool, &sub, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::Forbidden)?;

    let malo_ids = pg::list_aktive_malo_ids(&ctx.pool, kunde.id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    if !malo_ids.iter().any(|id| id == &q.malo_id) {
        return Err(ApiError::Forbidden);
    }

    let identity = pg::fetch_identitaet_by_sub(&ctx.pool, &sub, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    if let Some(filter) = identity.as_ref().and_then(|i| i.standort_filter.as_deref()) {
        let vertraege = pg::list_vertraege_by_kunde(&ctx.pool, kunde.id, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?;
        if !scoped_malos(&ctx, &vertraege, filter)
            .await?
            .contains(&q.malo_id)
        {
            return Err(ApiError::Forbidden);
        }
    }

    // Best-effort audit trail: a failed stamp must not deny a valid request.
    if let Err(e) = pg::update_letzter_login(&ctx.pool, &sub, ctx.tenant()).await {
        tracing::warn!(error = %e, "vertragd: letzter_login update failed");
    }

    ok(serde_json::json!({
        "kunden_id": kunde.id,
        "kundentyp": kunde.kundentyp,
        "rolle": identity.as_ref().map(|i| &i.rolle),
        "malo_id": q.malo_id,
    }))
}

// ── Portal identities ─────────────────────────────────────────────────────────

/// `POST /api/v1/kunden/{id}/identitaeten` — add or update a portal user.
pub async fn upsert_identitaet(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<pg::UpsertIdentitaetInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    require_kunde(&ctx, kunden_id).await?;
    // Only a genuinely new sub counts against the cap; re-POSTing an existing
    // one is an update, not a new seat.
    let existing = pg::fetch_identitaet_by_sub(&ctx.pool, &input.oidc_sub, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    if existing.is_none() {
        let count = pg::count_active_identitaeten(&ctx.pool, kunden_id, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?;
        if count >= i64::from(ctx.cfg.max_identitaeten_per_kunde) {
            return Err(ApiError::unprocessable(format!(
                "maximum {} active identities per customer — deactivate one first",
                ctx.cfg.max_identitaeten_per_kunde
            )));
        }
    }
    let id = pg::upsert_identitaet(&ctx.pool, kunden_id, ctx.tenant(), &input)
        .await
        .map_err(ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "kunden_id": kunden_id,
            "oidc_sub": input.oidc_sub,
            "rolle": input.rolle.as_deref().unwrap_or("VOLLZUGRIFF"),
        })),
    ))
}

/// `GET /api/v1/kunden/{id}/identitaeten` — active portal users.
pub async fn list_identitaeten(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kunde(&ctx, kunden_id).await?;
    let rows = pg::list_identitaeten(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(rows)
}

/// `DELETE /api/v1/kunden/{id}/identitaeten/{sub}` — revoke portal access.
pub async fn delete_identitaet(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((kunden_id, oidc_sub)): Path<(Uuid, String)>,
) -> ApiResult<StatusCode> {
    require_kunde(&ctx, kunden_id).await?;
    if pg::deactivate_identitaet_by_sub(&ctx.pool, kunden_id, ctx.tenant(), &oidc_sub)
        .await
        .map_err(ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ── BO4E sub-objects ──────────────────────────────────────────────────────────

/// `PUT /api/v1/kunden/{id}/person` — store the BO4E `Person` of a B2C customer.
///
/// The payload is validated by round-tripping through `rubo4e`, so `anrede` and
/// `titel` are real enum members and the stored JSON is canonical camelCase —
/// which is what makes a DSGVO Art. 15 disclosure structured rather than a
/// free-text blob.
pub async fn put_person(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(mut body): Json<serde_json::Value>,
) -> ApiResult<StatusCode> {
    use rubo4e::current::Person;
    match body.get("_typ").and_then(serde_json::Value::as_str) {
        None => body["_typ"] = serde_json::json!("PERSON"),
        Some("PERSON") => {}
        Some(other) => {
            return Err(ApiError::unprocessable(format!(
                "expected _typ=PERSON, got {other:?}"
            )));
        }
    }
    let typed: Person = serde_json::from_value(body)
        .map_err(|e| ApiError::unprocessable(format!("invalid Person payload: {e}")))?;
    let canonical =
        serde_json::to_value(&typed).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    if pg::upsert_person(&ctx.pool, id, ctx.tenant(), canonical)
        .await
        .map_err(ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `GET /api/v1/kunden/{id}/person`
pub async fn get_person(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    pg::fetch_person(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// `PUT /api/v1/kunden/{id}/zahlungsinformation` — IBAN/BIC/SEPA.
///
/// The IBAN is checked against ISO 13616 mod-97 before storage, so a typo is a
/// 422 here rather than a returned direct debit weeks later.
pub async fn put_zahlungsinformation(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    use rubo4e::current::Zahlungsinformation;
    require_kunde(&ctx, kunden_id).await?;
    let typed: Zahlungsinformation = serde_json::from_value(body)
        .map_err(|e| ApiError::unprocessable(format!("invalid Zahlungsinformation: {e}")))?;
    if let Some(ref iban) = typed.iban
        && let Err(msg) = sepa::validate_iban(iban)
    {
        return Err(ApiError::unprocessable(format!("invalid IBAN: {msg}")));
    }
    let canonical =
        serde_json::to_value(&typed).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    if !pg::upsert_zahlungsinformation(&ctx.pool, kunden_id, ctx.tenant(), &canonical)
        .await
        .map_err(ApiError::Internal)?
    {
        return Err(ApiError::NotFound);
    }
    ok(serde_json::json!({
        "kunden_id": kunden_id,
        "zahlungsinformation": canonical,
    }))
}

/// `GET /api/v1/kunden/{id}/zahlungsinformation`
pub async fn get_zahlungsinformation(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    pg::fetch_zahlungsinformation(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

// ── DSGVO ─────────────────────────────────────────────────────────────────────

/// `GET /api/v1/kunden/{id}/export` — DSGVO Art. 15 / Art. 20.
///
/// The complete record `vertragd` holds about one data subject: Kunde, Person,
/// Zahlungsinformation, portal identities, contracts and components.
pub async fn gdpr_export(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let export = pg::gdpr_export(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    ok(serde_json::json!({
        "rechtsgrundlage": "DSGVO Art. 15 (Auskunft) / Art. 20 (Datenübertragbarkeit)",
        "exported_at": time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "export": export,
    }))
}

/// Body of the erasure request.
#[derive(Debug, Deserialize, Default)]
pub struct AnonymizeRequest {
    /// Who asked. Falls back to the operator's token subject.
    pub requested_by: Option<String>,
    /// Why — recorded in the audit log, and required with `force`.
    pub request_reason: Option<String>,
    /// Erase although contracts are still running. Only for the cases where
    /// Art. 17 applies regardless (e.g. Abs. 1 lit. d, unlawful processing).
    #[serde(default)]
    pub force: bool,
}

/// `POST /api/v1/kunden/{id}/anonymize` — DSGVO Art. 17 right to erasure.
///
/// Pseudonymises every personal datum — including the supply address — and
/// keeps the commercial record for § 147 Abs. 3 AO. Irreversible; the operator
/// is responsible for having identified the data subject first.
///
/// Answers **409** while contracts are still running: the data is needed to
/// perform them (Art. 6 Abs. 1 lit. b DSGVO), so Art. 17 Abs. 1 lit. a does not
/// apply yet. The response names the contracts, so the answer to the data
/// subject writes itself.
pub async fn anonymize(
    claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    body: Option<Json<AnonymizeRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    if req.force && req.request_reason.is_none() {
        return Err(ApiError::unprocessable(
            "force requires request_reason — an override of the Art. 17 Abs. 3 lit. b \
             retention must be justifiable",
        ));
    }
    let requested_by = req.requested_by.as_deref().unwrap_or_else(|| claims.sub());
    let outcome = pg::anonymize_kunde(
        &ctx.pool,
        id,
        ctx.tenant(),
        requested_by,
        req.request_reason.as_deref(),
        req.force,
    )
    .await
    .map_err(ApiError::Internal)?;

    match outcome {
        pg::gdpr::ErasureOutcome::NotFound => Err(ApiError::NotFound),
        pg::gdpr::ErasureOutcome::Refused {
            grund,
            laufende_vertraege,
        } => {
            tracing::info!(
                kunden_id = %id, contracts = laufende_vertraege.len(),
                "vertragd: Art. 17 erasure refused — contracts still running"
            );
            Err(ApiError::conflict(format!(
                "{grund} Laufende Verträge: {}",
                laufende_vertraege.join(", ")
            )))
        }
        pg::gdpr::ErasureOutcome::Anonymized { fields } => {
            tracing::info!(kunden_id = %id, %requested_by, "vertragd: Art. 17 erasure executed");
            ok(serde_json::json!({
                "kunden_id": id,
                "anonymized": true,
                "anonymized_fields": fields,
                "rechtsgrundlage": "DSGVO Art. 17 — Recht auf Löschung",
                "aufbewahrung": "§ 147 Abs. 3 AO: Handelsbriefe 6 Jahre, Buchungsbelege 8 Jahre \
                                 — Vertragsdaten bleiben ohne Personenbezug erhalten",
                "audit_log": "anonymization_log Eintrag erstellt",
            }))
        }
    }
}

// ── Portfolio ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/kunden/{id}/portfolio` — one row per active MaLo/Sparte.
pub async fn portfolio(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kunde(&ctx, kunden_id).await?;
    let rows = pg::list_portfolio_by_kunde(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    let total_malos = rows.iter().filter(|r| r.malo_id.is_some()).count();
    ok(serde_json::json!({
        "kunden_id": kunden_id,
        "total_active_komponenten": rows.len(),
        "total_malos": total_malos,
        "komponenten": rows,
    }))
}
