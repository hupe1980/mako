//! B2B framework contracts.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_service::{ApiError, ApiResult, oidc::Claims};
use serde::Deserialize;
use uuid::Uuid;

use super::{Ctx, ok, require_kunde};
use crate::{
    domain::{self, Vertragsart},
    events::build_cloud_event,
    pg,
};

/// `POST /api/v1/kunden/{id}/rahmenvertraege` — create a framework contract.
///
/// A Rahmenvertrag sets shared pricing, notice periods and billing terms for N
/// Versorgungsverträge below it — the model for portfolio customers with many
/// delivery points.
pub async fn create(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<pg::CreateRahmenvertragInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    require_kunde(&ctx, kunden_id).await?;
    let id = pg::insert_rahmenvertrag(&ctx.pool, kunden_id, ctx.tenant(), &input)
        .await
        .map_err(ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "rahmenvertrag_id": id })),
    ))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/rahmenvertraege` — operator list view.
pub async fn list(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::list_all_rahmenvertraege(
        &ctx.pool,
        ctx.tenant(),
        q.status.as_deref(),
        q.limit.unwrap_or(100).clamp(1, 500),
    )
    .await
    .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "rahmenvertraege": rows }))
}

/// `GET /api/v1/kunden/{id}/rahmenvertraege`
pub async fn list_by_kunde(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kunde(&ctx, kunden_id).await?;
    let rows = pg::list_rahmenvertraege_by_kunde(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(rows)
}

/// `GET /api/v1/rahmenvertraege/{id}` — with its child supply contracts.
pub async fn get(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let rahmenvertrag = pg::fetch_rahmenvertrag(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    let vertraege = pg::list_versorgungsvertraege_by_rahmenvertrag(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(serde_json::json!({
        "rahmenvertrag": rahmenvertrag,
        "versorgungsvertraege": vertraege,
        "vertraege_count": vertraege.len(),
    }))
}

/// `GET /api/v1/rahmenvertraege/{id}/malos` — the sites a Sammelrechnung covers.
///
/// Each row carries the component's own `product_code`, which is what billingd
/// prices; the `rechnungsempfaenger` block is the **Rahmenvertrag holder**,
/// because a Sammelrechnung bills them rather than any one site's customer.
pub async fn malos(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::list_rahmenvertrag_malos(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    // Best-effort: a missing Kunde must not fail the site enumeration a billing
    // run depends on; the invoice then falls back to its unaddressed buyer.
    let rechnungsempfaenger =
        match pg::fetch_rechnungsempfaenger_by_rahmenvertrag(&ctx.pool, id, ctx.tenant()).await {
            Ok(re) => re,
            Err(e) => {
                tracing::warn!(
                    rahmenvertrag_id = %id, error = %e,
                    "vertragd: BG-7 buyer lookup for the Rahmenvertrag failed",
                );
                None
            }
        };
    ok(serde_json::json!({
        "malos": rows,
        "rechnungsempfaenger": rechnungsempfaenger,
    }))
}

/// `POST /api/v1/rahmenvertraege/{id}/kuendigen` — cascade termination.
///
/// Terminates every live Versorgungsvertrag under the framework contract to the
/// same `lieferende`. Each child is checked against *its own* notice period —
/// the customer, the Vertragsart and the agreed period differ per site — so a
/// date that is lawful for most sites terminates those and reports the rest
/// rather than terminating none or terminating all unlawfully.
pub async fn kuendigen(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(rahmenvertrag_id): Path<Uuid>,
    Json(input): Json<pg::KuendigungInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let rahmenvertrag = pg::fetch_rahmenvertrag(&ctx.pool, rahmenvertrag_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    let vertraege =
        pg::list_versorgungsvertraege_by_rahmenvertrag(&ctx.pool, rahmenvertrag_id, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?;
    if vertraege.is_empty() {
        return Err(ApiError::conflict(
            "keine laufenden Versorgungsverträge unter diesem Rahmenvertrag",
        ));
    }

    let today = mako_fristen::heute();
    let eingang = input.eingang.unwrap_or(today);
    if eingang > today {
        return Err(ApiError::unprocessable(
            "eingang liegt in der Zukunft — eine Kündigung kann nicht vorab zugehen",
        ));
    }

    let mut gekuendigt = Vec::new();
    let mut uebersprungen = Vec::new();
    let mut dispatched = 0usize;

    for v in &vertraege {
        if !matches!(v.status.as_str(), "AKTIV" | "TEILERFUELLUNG") {
            uebersprungen.push(serde_json::json!({
                "vertrag_id": v.id,
                "vertrags_nr": v.vertrags_nr,
                "grund": format!("Status '{}' ist nicht kündbar", v.status),
            }));
            continue;
        }
        let Some(kunde) = pg::fetch_kunde(&ctx.pool, v.kunden_id, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?
        else {
            uebersprungen.push(serde_json::json!({
                "vertrag_id": v.id,
                "grund": "Kunde nicht auffindbar",
            }));
            continue;
        };
        let frist = domain::kuendigungsfrist(
            eingang,
            Vertragsart::from_db(&v.vertragsart),
            kunde.haushaltskunde,
            input.grund,
            v.kuendigungsfrist_monate,
            input.preisanpassung_wirksam_zum,
        );
        if input.lieferende < frist.fruehestens {
            uebersprungen.push(serde_json::json!({
                "vertrag_id": v.id,
                "vertrags_nr": v.vertrags_nr,
                "grund": "lieferende liegt vor dem frühestmöglichen Kündigungstermin",
                "fruehestens": frist.fruehestens.to_string(),
                "frist": frist.frist,
                "rechtsgrundlage": frist.rechtsgrundlage,
            }));
            continue;
        }

        let mut tx = ctx.pool.begin().await.map_err(sqlx_err)?;
        let result = pg::kuendige_vertrag(&mut tx, v, &input, eingang, &ctx.cfg.lf_mp_id)
            .await
            .map_err(ApiError::Internal)?;
        dispatched += result.dispatched.len();
        let ce = build_cloud_event(
            mako_events::vertrag::GEKUENDIGT,
            v.id,
            ctx.tenant(),
            serde_json::json!({
                "vertrag_id": v.id,
                "vertrags_nr": v.vertrags_nr,
                "rahmenvertrag_id": rahmenvertrag_id,
                "rahmenvertrag_nr": rahmenvertrag.rahmenvertrag_nr,
                "lieferende": input.lieferende.to_string(),
                "grund": input.grund,
                "rechtsgrundlage": frist.rechtsgrundlage,
                "kuendigungsbestaetigung": {
                    "erforderlich": true,
                    "form": "Textform",
                    "rechtsgrundlage": "§ 41 Abs. 8 Nr. 2 EnWG",
                    "vertragsende": input.lieferende.to_string(),
                },
            }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce)
            .await
            .map_err(|e| ApiError::Internal(e.into()))?;
        pg::mark_kuendigung_bestaetigt(&mut *tx, v.id, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?;
        tx.commit().await.map_err(sqlx_err)?;
        gekuendigt.push(v.id);
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "rahmenvertrag_id": rahmenvertrag_id,
            "lieferende": input.lieferende.to_string(),
            "vertraege_gesamt": vertraege.len(),
            "gekuendigt": gekuendigt.len(),
            "mako_dispatched": dispatched,
            "uebersprungen": uebersprungen.len(),
            "uebersprungen_details": uebersprungen,
        })),
    ))
}

fn sqlx_err(e: sqlx::Error) -> ApiError {
    ApiError::Internal(anyhow::Error::new(e))
}
