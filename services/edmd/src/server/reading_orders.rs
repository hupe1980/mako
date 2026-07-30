//! Ablesesteuerung — reading order API.

#[allow(unused_imports)]
use super::*;

// ── Ablesesteuerung — Reading Order API ──────────────────────────────────────
//
// All three market roles schedule meter readings through the same API:
//   LF  → LIEFERBEGINN / LIEFERENDE / ZWISCHENABLESUNG / JAHRESABLESUNG
//   NB  → JAHRESABLESUNG / EINZUG / AUSZUG / SPERRUNG / ENTSPERRUNG
//   MSB → SONDERABLESUNG / INSRPT_STOERUNG / ISMS_AUSLESUNG
//
// DB: `ablese_auftraege` (migration 0003_ablese_auftraege.sql)

#[derive(Debug, serde::Deserialize)]
pub(crate) struct CreateReadingOrderRequest {
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub anlass: String,
    pub auftraggeber_rolle: String,
    pub ausfuehrender_msb: Option<String>,
    pub geplant_am: time::Date,
    pub ausfuehrt_bis: Option<time::Date>,
    pub auftrag_position_id: Option<uuid::Uuid>,
    pub insrpt_process_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct CompleteReadingOrderRequest {
    pub zaehlerstand_kwh: Option<f64>,
    pub zaehlerstand_qm3: Option<f64>,
    pub brennwert: Option<f64>,
    pub zustandszahl: Option<f64>,
    pub mscons_ref: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ListReadingOrdersQuery {
    pub malo_id: Option<String>,
    pub status: Option<String>,
    pub anlass: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub(crate) struct ReadingOrderRow {
    pub id: uuid::Uuid,
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub anlass: String,
    pub auftraggeber_rolle: String,
    pub ausfuehrender_msb: Option<String>,
    pub geplant_am: time::Date,
    pub ausfuehrt_bis: Option<time::Date>,
    pub status: String,
    pub zaehlerstand_kwh: Option<f64>,
    pub zaehlerstand_qm3: Option<f64>,
    pub ausgefuehrt_am: Option<time::OffsetDateTime>,
    pub mscons_ref: Option<String>,
    pub auftrag_position_id: Option<uuid::Uuid>,
    pub insrpt_process_id: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// `POST /api/v1/reading-orders` — schedule a meter reading.
pub(crate) async fn create_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<CreateReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let id = uuid::Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO ablese_auftraege
         (id,malo_id,melo_id,tenant,anlass,auftraggeber_rolle,
          ausfuehrender_msb,geplant_am,ausfuehrt_bis,
          auftrag_position_id,insrpt_process_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(&req.malo_id)
    .bind(&req.melo_id)
    .bind(&state.tenant)
    .bind(&req.anlass)
    .bind(&req.auftraggeber_rolle)
    .bind(&req.ausfuehrender_msb)
    .bind(req.geplant_am)
    .bind(req.ausfuehrt_bis)
    .bind(req.auftrag_position_id)
    .bind(&req.insrpt_process_id)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": id, "status": "OFFEN" })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/reading-orders?malo_id=&status=&anlass=&limit=`
pub(crate) async fn list_reading_orders(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(q): Query<ListReadingOrdersQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let rows = sqlx::query_as::<_, ReadingOrderRow>(
        "SELECT id,malo_id,melo_id,anlass,auftraggeber_rolle,
                ausfuehrender_msb,geplant_am,ausfuehrt_bis,status,
                zaehlerstand_kwh,zaehlerstand_qm3,ausgefuehrt_am,
                mscons_ref,auftrag_position_id,insrpt_process_id,created_at
         FROM ablese_auftraege
         WHERE tenant=$1
           AND ($2::text IS NULL OR malo_id=$2)
           AND ($3::text IS NULL OR status=$3)
           AND ($4::text IS NULL OR anlass=$4)
         ORDER BY geplant_am DESC
         LIMIT $5",
    )
    .bind(&state.tenant)
    .bind(&q.malo_id)
    .bind(&q.status)
    .bind(&q.anlass)
    .bind(q.limit.unwrap_or(100).min(1000))
    .fetch_all(state.repo.pool())
    .await;

    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/reading-orders/{id}`
pub(crate) async fn get_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let row = sqlx::query_as::<_, ReadingOrderRow>(
        "SELECT id,malo_id,melo_id,anlass,auftraggeber_rolle,
                ausfuehrender_msb,geplant_am,ausfuehrt_bis,status,
                zaehlerstand_kwh,zaehlerstand_qm3,ausgefuehrt_am,
                mscons_ref,auftrag_position_id,insrpt_process_id,created_at
         FROM ablese_auftraege WHERE id=$1 AND tenant=$2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await;

    match row {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/reading-orders/{id}/complete`
pub(crate) async fn complete_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<CompleteReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let res = sqlx::query(
        "UPDATE ablese_auftraege
         SET status='AUSGEFUEHRT',
             zaehlerstand_kwh=$1::numeric,
             zaehlerstand_qm3=$2::numeric,
             brennwert=$3::numeric,
             zustandszahl=$4::numeric,
             ausgefuehrt_am=now(),
             mscons_ref=COALESCE($5,mscons_ref)
         WHERE id=$6 AND tenant=$7 AND status IN ('OFFEN','BEAUFTRAGT')",
    )
    .bind(req.zaehlerstand_kwh)
    .bind(req.zaehlerstand_qm3)
    .bind(req.brennwert)
    .bind(req.zustandszahl)
    .bind(&req.mscons_ref)
    .bind(id)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/reading-orders/{id}/cancel`
pub(crate) async fn cancel_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let res = sqlx::query(
        "UPDATE ablese_auftraege SET status='STORNIERT'
         WHERE id=$1 AND tenant=$2 AND status IN ('OFFEN','BEAUFTRAGT')",
    )
    .bind(id)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Body for `PUT /api/v1/reading-orders/{id}/fail`.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct FailReadingOrderRequest {
    /// Ablesehindernis — why no reading could be taken.
    grund: String,
    /// Free-text detail for the field report.
    #[serde(default)]
    notiz: Option<String>,
}

/// Ablesehindernis codes a reading order may be failed with.
pub(crate) const ABLESEHINDERNIS_GRUENDE: [&str; 7] = [
    "KEIN_ZUTRITT",
    "ZAEHLER_UNZUGAENGLICH",
    "ZAEHLER_DEFEKT",
    "ZAEHLER_NICHT_AUFFINDBAR",
    "KUNDE_VERWEIGERT",
    "ABLESUNG_UNPLAUSIBEL",
    "SONSTIGES",
];

/// `PUT /api/v1/reading-orders/{id}/fail`
///
/// Records that a dispatched reading could not be taken, with the
/// Ablesehindernis that prevented it.
///
/// Distinct from `/cancel`: a cancelled order is no longer owed, whereas a
/// failed one still is. A failed JAHRESABLESUNG past its deadline remains a
/// §40 Abs. 2 EnWG gap, so it keeps appearing in `list_overdue_reading_orders`
/// until it is re-dispatched or the quantity is estimated under §40a EnWG.
pub(crate) async fn fail_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<FailReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if !ABLESEHINDERNIS_GRUENDE.contains(&req.grund.as_str()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("unknown Ablesehindernis `{}`", req.grund),
                "expected": ABLESEHINDERNIS_GRUENDE,
            })),
        )
            .into_response();
    }

    let row = sqlx::query(
        "UPDATE ablese_auftraege
            SET status            = 'FEHLGESCHLAGEN',
                fehlschlag_grund  = $1,
                fehlschlag_notiz  = $2,
                fehlgeschlagen_am = now()
          WHERE id = $3 AND tenant = $4 AND status IN ('OFFEN','BEAUFTRAGT')
          RETURNING malo_id, anlass, ausfuehrt_bis, ausfuehrender_msb",
    )
    .bind(&req.grund)
    .bind(&req.notiz)
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    use sqlx::Row as _;
    let malo_id: String = row.try_get("malo_id").unwrap_or_default();
    let anlass: String = row.try_get("anlass").unwrap_or_default();
    let ausfuehrt_bis: Option<time::Date> = row.try_get("ausfuehrt_bis").ok().flatten();
    let ausfuehrender_msb: Option<String> = row.try_get("ausfuehrender_msb").ok().flatten();

    // The order is terminal but the reading is still owed, so the failure is
    // announced rather than just recorded.
    if let Some(ref webhook_url) = state.erp_webhook_url {
        let client = mako_service::http::default_client();
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": mako_events::messwert::READING_ORDER_FAILED,
            "source": format!("urn:edmd:tenant:{}:{}", state.tenant, malo_id),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": OffsetDateTime::now_utc().to_string(),
            "subject": malo_id,
            "tenantid": state.tenant,
            "datacontenttype": "application/json",
            "data": {
                "order_id":          id.to_string(),
                "malo_id":           malo_id,
                "anlass":            anlass,
                "grund":             req.grund,
                "notiz":             req.notiz,
                "ausfuehrt_bis":     ausfuehrt_bis.map(|d| d.to_string()),
                "ausfuehrender_msb": ausfuehrender_msb,
                "recommended_action":
                    "Re-dispatch the reading, or estimate under §40a EnWG and document the basis",
            }
        });
        post_ce_with_retry(&client, webhook_url, &ce, state.webhook_secret_bytes()).await;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": id.to_string(),
            "status": "FEHLGESCHLAGEN",
            "grund": req.grund,
            "still_owed": true,
        })),
    )
        .into_response()
}
