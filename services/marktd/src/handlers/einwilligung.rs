//! ESA consent-registry REST handlers (§49 Abs. 2 Nr. 9 MsbG).
//!
//! Routes:
//!   POST   /api/v1/esa/einwilligungen                 — grant consent
//!   GET    /api/v1/esa/einwilligungen?esa_mp_id=…      — list active consents
//!   GET    /api/v1/esa/einwilligungen/:id             — fetch one
//!   DELETE /api/v1/esa/einwilligungen/:id             — revoke (Art. 7(3) GDPR)
//!   PUT    /api/v1/esa/framework/:msb_mp_id/:esa_mp_id — upsert framework agreement
//!   GET    /api/v1/esa/framework/:msb_mp_id/:esa_mp_id — fetch framework agreement
//!
//! **Evidence-agnostic** (BNetzA forbids rejecting consent for deviating from
//! the BDEW template): `evidence_uri`/`evidence_hash` are stored verbatim and
//! never validated for form.
//!
//! Revocation emits `de.markt.einwilligung.widerrufen` and fires the **17008
//! Abbestellung** at makod — GDPR Art. 7(3) obliges the ESA to stop, and the
//! only way to stop is the Abbestellung.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    makod_client::{ForwardCommand, MakodClient},
    repository::{
        ConsentPerspective, EinwilligungRecord, EinwilligungRepository, EsaFrameworkAgreement,
        EsaMessproduktPreis,
    },
};
use serde::Deserialize;
use uuid::Uuid;

use mako_service::cedar::CedarEnforcer;

use super::{Claims, IntoMdmResponse as _, TenantGln};

/// Deny response for the consent registry.
///
/// Both actions are gated on the MSB or ESA role: the registry records the
/// relationship between exactly those two parties, and a revocation fires the
/// ORDERS 17008 Abbestellung at makod. An LF has no lawful interest in it.
fn forbidden(action: &'static str) -> axum::response::Response {
    mako_markt::error::MdmError::Forbidden { reason: action }.into_response()
}

/// Injected `Arc<PgEinwilligungRepository>`.
pub type EinwilligungRepoExt = Arc<crate::pg::PgEinwilligungRepository>;
async fn emit(
    pool: &sqlx::PgPool,
    notify: &tokio::sync::Notify,
    tenant: &str,
    ce_type: &str,
    subject: String,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let evt = MarktEvent::new(tenant, ce_type, subject, data);
    crate::outbox::enqueue(pool, &evt, notify).await
}

#[derive(Debug, Deserialize)]
pub struct GrantBody {
    pub anschlussnutzer_ref: String,
    pub esa_mp_id: String,
    pub location_ids: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    #[serde(default)]
    pub valid_to: Option<time::Date>,
    /// Opaque evidence — stored verbatim, never validated for form.
    #[serde(default)]
    pub evidence_uri: Option<String>,
    #[serde(default)]
    pub evidence_hash: Option<String>,
}

/// `POST /api/v1/esa/einwilligungen` — grant an ESA consent.
pub async fn grant_einwilligung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Json(body): Json<GrantBody>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    if body.location_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "location_ids must not be empty" })),
        )
            .into_response();
    }
    let rec = EinwilligungRecord {
        id: Uuid::nil(),
        tenant: tenant.clone(),
        anschlussnutzer_ref: body.anschlussnutzer_ref.clone(),
        esa_mp_id: body.esa_mp_id.clone(),
        location_ids: body.location_ids.clone(),
        scope: body.scope.unwrap_or_else(|| "werte".to_owned()),
        granted_at: time::OffsetDateTime::now_utc(),
        valid_from: body
            .valid_from
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date()),
        valid_to: body.valid_to,
        revoked_at: None,
        evidence_uri: body.evidence_uri,
        evidence_hash: body.evidence_hash,
    };
    match repo.grant(rec).await {
        Ok(id) => {
            if let Err(e) = emit(
                &pool,
                &notify,
                &tenant,
                mako_events::markt::EINWILLIGUNG_ERTEILT,
                id.to_string(),
                serde_json::json!({
                    "einwilligung_id": id,
                    "esa_mp_id": body.esa_mp_id,
                    "anschlussnutzer_ref": body.anschlussnutzer_ref,
                    "location_ids": body.location_ids,
                }),
            )
            .await
            {
                tracing::error!(error = %e, "einwilligung: durable enqueue failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub esa_mp_id: String,
}

/// `GET /api/v1/esa/einwilligungen?esa_mp_id=…` — list active consents.
pub async fn list_einwilligungen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo.list_for_esa(&tenant, &q.esa_mp_id).await {
        Ok(rows) => Json(serde_json::json!({
            "esa_mp_id": q.esa_mp_id,
            "count": rows.len(),
            "einwilligungen": rows,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/esa/einwilligungen/:id`.
pub async fn get_einwilligung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo.get(&tenant, id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// `DELETE /api/v1/esa/einwilligungen/:id` — revoke consent (Art. 7(3) GDPR).
///
/// Emits `de.markt.einwilligung.widerrufen` and fires the **17008 Abbestellung**
/// at makod for the covered locations. The Abbestellung dispatch is best-effort
/// (logged on failure) — the revocation itself always succeeds and is the
/// durable signal a consumer can act on.
#[allow(clippy::too_many_arguments)] // axum extractors, not a parameter list
pub async fn revoke_einwilligung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    let revoked = match repo.revoke(&tenant, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return e.into_response(),
    };

    if let Err(e) = emit(
        &pool,
        &notify,
        &tenant,
        mako_events::markt::EINWILLIGUNG_WIDERRUFEN,
        id.to_string(),
        serde_json::json!({
            "einwilligung_id": id,
            "esa_mp_id": revoked.esa_mp_id,
            "anschlussnutzer_ref": revoked.anschlussnutzer_ref,
            "location_ids": revoked.location_ids,
        }),
    )
    .await
    {
        tracing::error!(error = %e, "einwilligung: widerrufen durable enqueue failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // GDPR Art. 7(3): stopping value delivery requires an Abbestellung (17008)
    // per covered location. Fire it at makod — best-effort so a makod outage
    // never blocks the customer's revocation right.
    //
    // **No `messprodukt`, deliberately.** An ESA subscription is the
    // (Meldepunkt, Messprodukt) pair and one location may carry several — the
    // Codeliste offers `9991 00000 305 6` and `9991 00000 314 7` for the same
    // Marktlokation. A Widerruf withdraws the lawful basis for all of them, and
    // marktd has no idea how many are running: makod resolves an omitted
    // `messprodukt` to *every* live subscription at the location and sends one
    // 17008 each. Naming one here would stop one and leave the rest delivering.
    for location_id in &revoked.location_ids {
        let cmd = ForwardCommand {
            command: "esa.abbestellung.beauftragen".to_owned(),
            marktrolle: Some("ESA".to_owned()),
            malo_id: Some(location_id.clone()),
            melo_id: None,
            payload: serde_json::json!({
                "malo_id": location_id,
                "esa_mp_id": revoked.esa_mp_id,
                "grund": "einwilligung_widerrufen",
                "einwilligung_id": id,
            }),
        };
        let idem = format!("esa-abbestellung:{id}:{location_id}");
        if let Err(e) = makod.post_command(&idem, &cmd).await {
            tracing::warn!(
                error = %e,
                location_id,
                "marktd: Abbestellung dispatch to makod failed after Widerruf — \
                 de.markt.einwilligung.widerrufen was emitted; retry via that event"
            );
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
pub struct ConsentCheckQuery {
    pub esa_mp_id: String,
    pub msb_mp_id: String,
    pub location_id: String,
    /// `msb_inbound` (default, lenient) or `esa_outbound` (strict).
    #[serde(default)]
    pub perspective: ConsentPerspective,
}

/// `GET /api/v1/esa/consent-check?esa_mp_id=…&msb_mp_id=…&location_id=…&perspective=…`.
///
/// Gates an ESA message against the registry. Always answers `200` with a
/// [`ConsentDecision`](mako_markt::repository::ConsentDecision) — a revoked
/// consent or an unestablished framework agreement yields `allowed: false`, the
/// clearing case the caller answers with an Ablehnung.
///
/// `perspective` sets how a *missing* record is read: `msb_inbound` (default)
/// treats it as self-assertion and allows (`BNetzA` forbids form-based
/// rejection); `esa_outbound` treats it as no lawful basis and blocks — the ESA
/// is the consent holder and must not originate a request it has no consent for.
pub async fn consent_check(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Query(q): Query<ConsentCheckQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo
        .consent_check(
            &tenant,
            &q.esa_mp_id,
            &q.msb_mp_id,
            &q.location_id,
            q.perspective,
        )
        .await
    {
        Ok(decision) => Json(decision).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct FrameworkBody {
    #[serde(default)]
    pub signed_at: Option<time::OffsetDateTime>,
    #[serde(default)]
    pub edi_agreement: bool,
    #[serde(default)]
    pub cert_state: Option<String>,
}

/// `PUT /api/v1/esa/framework/:msb_mp_id/:esa_mp_id`.
pub async fn put_framework(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    Json(body): Json<FrameworkBody>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    let rec = EsaFrameworkAgreement {
        tenant,
        msb_mp_id,
        esa_mp_id,
        signed_at: body.signed_at,
        edi_agreement: body.edi_agreement,
        cert_state: body.cert_state.unwrap_or_else(|| "pending".to_owned()),
    };
    match repo.upsert_framework(rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// Body of `PUT /api/v1/esa/preise/:msb_mp_id/:esa_mp_id`.
#[derive(Debug, Deserialize)]
pub struct EsaPreiseBody {
    /// Which subscription these prices were agreed for — a subscription is the
    /// (Meldepunkt, Messprodukt) pair.
    pub lokations_id: String,
    pub messprodukt: String,
    /// Belegnummer of the ORDERS 17007 the offer was accepted with.
    #[serde(default)]
    pub bestellung_ref: Option<String>,
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    #[serde(default)]
    pub valid_to: Option<time::Date>,
    /// `SG4 CUX` DE 6345.
    #[serde(default)]
    pub waehrung: Option<String>,
    /// The priced Artikel-IDs of the accepted offer.
    pub preise: Vec<EsaPreisPosition>,
}

/// One `SG27 PIA+Z02` / `SG31 PRI+CAL` pair of an accepted Angebot.
#[derive(Debug, Deserialize)]
pub struct EsaPreisPosition {
    pub artikel_id: String,
    /// `SG31 PRI` DE 5387 — `Z01` / `Z02` / `Z03`.
    pub preistyp: String,
    /// `SG31 PRI` DE 5118, up to six decimals.
    pub betrag: rust_decimal::Decimal,
    /// `SG31 PRI` DE 6411 — `H87` Stück or `DAY` Tag.
    pub einheit: String,
}

/// `PUT /api/v1/esa/preise/:msb_mp_id/:esa_mp_id` — record the prices of an
/// accepted QUOTES 15003 Angebot.
///
/// **The ESA price basis.** `PreisblattMessung` is what an MSB publishes toward
/// the NB and the LF; there is none for the Kapitel-4.6 Messprodukte, because
/// §35 MsbG leaves the Entgelt for a Zusatzleistung to be agreed per request.
/// So the offer the ESA ordered against is the agreement, and `invoicd` checks
/// the MSB's INVOIC 31009 positions against exactly these rows — the invoice
/// names the same Artikel-IDs back (`SG26 LIN` DE 7143 `Z09`).
///
/// Written by `makod` when the MSB confirms the Bestellung (ORDRSP 19011),
/// which is the moment the offer becomes binding.
pub async fn put_esa_preise(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    Json(body): Json<EsaPreiseBody>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    if body.preise.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "preise must not be empty — SG31 PRI is Muss inside the SG27 LIN                           position block, and an offer that prices nothing is the Ablehnung",
            })),
        )
            .into_response();
    }
    let waehrung = body.waehrung.unwrap_or_else(|| "EUR".to_owned());
    let rows: Vec<EsaMessproduktPreis> = body
        .preise
        .into_iter()
        .map(|p| EsaMessproduktPreis {
            tenant: tenant.clone(),
            esa_mp_id: esa_mp_id.clone(),
            msb_mp_id: msb_mp_id.clone(),
            lokations_id: body.lokations_id.clone(),
            messprodukt: body.messprodukt.clone(),
            artikel_id: p.artikel_id,
            preistyp: p.preistyp,
            betrag: p.betrag,
            einheit: p.einheit,
            waehrung: waehrung.clone(),
            bestellung_ref: body.bestellung_ref.clone(),
            valid_from: body.valid_from,
            valid_to: body.valid_to,
        })
        .collect();
    match repo.upsert_esa_preise(&rows).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/esa/preise/:msb_mp_id/:esa_mp_id?at=YYYY-MM-DD` — the prices in
/// force between the pair on `at`.
///
/// Across **all** of the pair's subscriptions, deliberately: an INVOIC 31009
/// bills a Rahmenvertrag rather than a single Meldepunkt, and its positions name
/// Artikel-IDs without saying which subscription each belongs to.
pub async fn get_esa_preise(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<EsaPreiseQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    let at =
        q.at.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    match repo
        .esa_preise_at(&tenant, &esa_mp_id, &msb_mp_id, at)
        .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Query of `GET /api/v1/esa/preise/…`.
#[derive(Debug, Deserialize)]
pub struct EsaPreiseQuery {
    /// The day the prices must be in force on. Defaults to today.
    #[serde(default)]
    pub at: Option<time::Date>,
}

/// `GET /api/v1/esa/framework/:msb_mp_id/:esa_mp_id`.
pub async fn get_framework(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo.get_framework(&tenant, &msb_mp_id, &esa_mp_id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}
