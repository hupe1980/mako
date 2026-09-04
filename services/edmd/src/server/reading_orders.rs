//! Ablesesteuerung — reading order API.

#[allow(unused_imports)]
use super::*;

use rust_decimal::Decimal;

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
    /// `STROM` (default) · `GAS` · `WAERME` · `WASSER`.
    ///
    /// Decides the unit of the Zählerstand this order will report, so it is part
    /// of the order rather than of the completion: the person reading the meter
    /// should not be the one deciding what commodity it measures.
    #[serde(default)]
    pub sparte: Option<String>,
    /// OBIS register to be read, when it is known.
    #[serde(default)]
    pub obis_code: Option<String>,
}

/// Register readings and gas factors reported when a reading order completes.
///
/// Every quantity is a `Decimal`, matching the `NUMERIC` columns that hold it.
/// They were `f64`: on the way in that silently rounded a five-decimal
/// Zählerstand, and on the way out sqlx has no `NUMERIC → f64` decode at all, so
/// listing or fetching any *completed* order failed with a type-mismatch error
/// the moment a register reading was present.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CompleteReadingOrderRequest {
    pub zaehlerstand_kwh: Option<Decimal>,
    pub zaehlerstand_qm3: Option<Decimal>,
    pub brennwert: Option<Decimal>,
    pub zustandszahl: Option<Decimal>,
    pub mscons_ref: Option<String>,
    /// When the reading was taken, if not now.
    ///
    /// A Jahresablesung is frequently entered days after the meter was read, and
    /// the § 40 Abs. 2 Nr. 6 EnWG opening/closing Zählerstand is selected by the
    /// instant it *held*, not the instant it was typed in.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub abgelesen_am: Option<time::OffsetDateTime>,
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
    pub zaehlerstand_kwh: Option<Decimal>,
    pub zaehlerstand_qm3: Option<Decimal>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ausgefuehrt_am: Option<time::OffsetDateTime>,
    pub mscons_ref: Option<String>,
    pub auftrag_position_id: Option<uuid::Uuid>,
    pub insrpt_process_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
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

    // Refused, not defaulted: the Sparte decides the unit of the Zählerstand
    // this order will report, and a gas reading filed as electricity is a
    // register value in the wrong dimension.
    let sparte = match req.sparte.as_deref() {
        None => crate::domain::Sparte::Strom,
        Some(raw) => match crate::domain::parse_sparte(raw) {
            Some(s) => s,
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!("unknown sparte `{raw}`"),
                        "expected": crate::domain::Sparte::CODES,
                    })),
                )
                    .into_response();
            }
        },
    };

    let id = uuid::Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO ablese_auftraege
         (id,malo_id,melo_id,tenant,anlass,auftraggeber_rolle,
          ausfuehrender_msb,geplant_am,ausfuehrt_bis,
          auftrag_position_id,insrpt_process_id,sparte,obis_code)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
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
    .bind(sparte.as_str())
    .bind(&req.obis_code)
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
///
/// Records the reading and **files the Zählerstand into the reading store**.
///
/// The second half is the point. A register value written onto the order row
/// and nowhere else is unreachable from billing: it cannot answer § 40 Abs. 2
/// Nr. 6 EnWG (the invoice's opening and closing Zählerstand), and it cannot be
/// differenced against the previous year's reading — which for an **SLP**
/// delivery point, with no interval metering at all, is the entire billing path
/// (`metering::reading::consumption_between`).
///
/// The order row keeps its copy: it is the record of what this Auftrag returned,
/// which is a different fact from what the register held at an instant.
pub(crate) async fn complete_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<CompleteReadingOrderRequest>,
) -> impl IntoResponse {
    use sqlx::Row as _;

    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // The instant the register *held* the value, not the instant it was typed
    // in: a Jahresablesung is routinely entered days later, and the period
    // bounds select on the reading's own timestamp.
    let abgelesen_am = req.abgelesen_am.unwrap_or_else(OffsetDateTime::now_utc);

    let res = sqlx::query(
        "UPDATE ablese_auftraege
         SET status='AUSGEFUEHRT',
             zaehlerstand_kwh=$1,
             zaehlerstand_qm3=$2,
             brennwert=$3,
             zustandszahl=$4,
             ausgefuehrt_am=$8,
             mscons_ref=COALESCE($5,mscons_ref)
         WHERE id=$6 AND tenant=$7 AND status IN ('OFFEN','BEAUFTRAGT')
         RETURNING malo_id, melo_id, sparte, obis_code",
    )
    .bind(req.zaehlerstand_kwh)
    .bind(req.zaehlerstand_qm3)
    .bind(req.brennwert)
    .bind(req.zustandszahl)
    .bind(&req.mscons_ref)
    .bind(id)
    .bind(&state.tenant)
    .bind(abgelesen_am)
    .fetch_optional(state.repo.pool())
    .await;

    let row = match res {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let malo_id: String = row.try_get("malo_id").unwrap_or_default();
    let sparte = row
        .try_get::<String, _>("sparte")
        .ok()
        .as_deref()
        .and_then(crate::domain::parse_sparte)
        .unwrap_or(crate::domain::Sparte::Strom);
    let obis_code: Option<String> = row.try_get("obis_code").unwrap_or(None);

    // The register value in the unit the register counts — the same rule the
    // Zählerstandsgang follows. `zaehlerstand_qm3` is a volume and belongs to a
    // gas or water meter; `zaehlerstand_kwh` is an energy register. Reporting
    // the one that does not match the order's Sparte is a decode fault, not a
    // reading, so it is refused rather than filed in the wrong dimension.
    let value = match sparte.measured_unit() {
        metering::MeasurementUnit::CubicMetre => req.zaehlerstand_qm3,
        metering::MeasurementUnit::KiloWattHour => req.zaehlerstand_kwh,
    };
    let mismatched = match sparte.measured_unit() {
        metering::MeasurementUnit::CubicMetre => req.zaehlerstand_kwh.is_some(),
        metering::MeasurementUnit::KiloWattHour => req.zaehlerstand_qm3.is_some(),
    };
    if mismatched {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "the order is for {}, whose register counts {} — the other \
                     Zählerstand field belongs to a different commodity",
                    sparte.as_str(),
                    sparte.measured_unit().as_str(),
                ),
            })),
        )
            .into_response();
    }

    // A Zählerstand is filed against a register of a meter, and the
    // Zählerstandsgang store keys on both: a Marktlokation may be measured by
    // several Messlokationen, and two meters carry the same OBIS register at the
    // same instants. Neither is derivable from an order that does not name it,
    // and inventing a canonical register per commodity would file the reading
    // against a channel nobody read.
    let melo_id: Option<String> = row.try_get("melo_id").unwrap_or(None);
    if value.is_some() {
        let missing: Vec<&str> = [
            obis_code.is_none().then_some("obis_code"),
            melo_id.is_none().then_some("melo_id"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!(
                        "the order names no {} , so its Zählerstand cannot be filed: a reading \
                         belongs to one register of one meter, and stored without them a second \
                         meter's reading would overwrite the first",
                        missing.join(" and no ")
                    ),
                    "missing": missing,
                })),
            )
                .into_response();
        }
    }

    // A completion without a reading is legitimate — an order can be closed
    // administratively — so it records the status and files nothing.
    if let Some(zaehlerstand) = value {
        let reading = crate::domain::MeterReading {
            malo_id: malo_id.clone(),
            read_at: abgelesen_am,
            zaehlerstand,
            quality: QualityFlag::Measured,
            sparte,
            obis_code,
            // The meter the register belongs to, as the order named it.
            melo_id: melo_id.clone(),
            tenant: state.tenant.clone(),
            // An Ablesung is an operator entry, whoever physically took it.
            source: IngestionSource::Manual,
            sender_mp_id: None,
            push_session: None,
        };
        if let Err(e) = state
            .repo
            .store_readings(std::slice::from_ref(&reading))
            .await
        {
            // The order is already AUSGEFUEHRT and the value is on its row, so
            // nothing is lost — but the billing path cannot see it, which is the
            // whole reason for filing it, so it must be visible.
            tracing::error!(
                order = %id, malo_id = %malo_id, error = %e,
                "edmd: reading order completed but its Zählerstand could not be filed"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e.to_string(),
                    "status": "AUSGEFUEHRT",
                    "zaehlerstand_filed": false,
                })),
            )
                .into_response();
        }
    }

    StatusCode::NO_CONTENT.into_response()
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
/// § 40b Abs. 1 EnWG gap, so it keeps appearing in `list_overdue_reading_orders`
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
        let ce = mako_service::CloudEvent::new(
            mako_service::source("edmd", &state.tenant),
            mako_events::messwert::READING_ORDER_FAILED,
            malo_id.clone(),
            serde_json::json!({
                "order_id":          id.to_string(),
                "malo_id":           malo_id,
                "anlass":            anlass,
                "grund":             req.grund,
                "notiz":             req.notiz,
                "ausfuehrt_bis":     ausfuehrt_bis.map(|d| d.to_string()),
                "ausfuehrender_msb": ausfuehrender_msb,
                "recommended_action":
                    "Re-dispatch the reading, or estimate under §40a EnWG and document the basis",
            }),
        )
        .extension("tenantid", state.tenant.clone());
        if let Err(e) = mako_service::post_ce_with_retry(
            &client,
            webhook_url,
            &ce,
            state.webhook_secret_bytes(),
        )
        .await
        {
            tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
        }
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
