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
    },
};
use serde::Deserialize;
use uuid::Uuid;

use super::{Claims, IntoMdmResponse as _, TenantGln};

/// Injected `Arc<PgEinwilligungRepository>`.
pub type EinwilligungRepoExt = Arc<crate::pg::PgEinwilligungRepository>;
type EventTx = tokio::sync::mpsc::UnboundedSender<serde_json::Value>;

fn emit(event_tx: &EventTx, tenant: &str, ce_type: &str, subject: String, data: serde_json::Value) {
    let evt = MarktEvent::new(tenant, ce_type, subject, data);
    if let Ok(payload) = serde_json::to_value(&evt) {
        let _ = event_tx.send(payload);
    }
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
    _claims: Claims,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Extension(event_tx): Extension<EventTx>,
    Json(body): Json<GrantBody>,
) -> impl IntoResponse {
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
            emit(
                &event_tx,
                &tenant,
                "de.markt.einwilligung.erteilt",
                id.to_string(),
                serde_json::json!({
                    "einwilligung_id": id,
                    "esa_mp_id": body.esa_mp_id,
                    "anschlussnutzer_ref": body.anschlussnutzer_ref,
                    "location_ids": body.location_ids,
                }),
            );
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
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
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
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
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
pub async fn revoke_einwilligung(
    _claims: Claims,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Extension(event_tx): Extension<EventTx>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let revoked = match repo.revoke(&tenant, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return e.into_response(),
    };

    emit(
        &event_tx,
        &tenant,
        "de.markt.einwilligung.widerrufen",
        id.to_string(),
        serde_json::json!({
            "einwilligung_id": id,
            "esa_mp_id": revoked.esa_mp_id,
            "anschlussnutzer_ref": revoked.anschlussnutzer_ref,
            "location_ids": revoked.location_ids,
        }),
    );

    // GDPR Art. 7(3): stopping value delivery requires an Abbestellung (17008,
    // NBA 1) per covered location. Fire it at makod — best-effort so a makod
    // outage never blocks the customer's revocation right.
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
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Query(q): Query<ConsentCheckQuery>,
) -> impl IntoResponse {
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
    _claims: Claims,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    Json(body): Json<FrameworkBody>,
) -> impl IntoResponse {
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

/// `GET /api/v1/esa/framework/:msb_mp_id/:esa_mp_id`.
pub async fn get_framework(
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match repo.get_framework(&tenant, &msb_mp_id, &esa_mp_id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}
