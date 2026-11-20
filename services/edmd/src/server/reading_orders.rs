//! Ablesesteuerung — reading order API.

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
#[serde(deny_unknown_fields)]
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
    /// `STROM` · `GAS` · `WAERME` · `WASSER` — required.
    ///
    /// Decides the unit of the Zählerstand this order will report, so it is part
    /// of the order rather than of the completion: the person reading the meter
    /// should not be the one deciding what commodity it measures. Kept optional
    /// in the type so an order without one is answered with a refusal naming
    /// the field rather than a deserialisation error.
    #[serde(default)]
    pub sparte: Option<String>,
    /// OBIS register to be read, when it is known.
    #[serde(default)]
    pub obis_code: Option<String>,
    /// The executing system's own reference for this order — a workforce
    /// tool's job id, an ERP order number. Opaque and unique per tenant;
    /// echoed on every `de.mako.messwert.reading.order.*` event.
    #[serde(default)]
    pub external_ref: Option<String>,
}

/// An `external_ref` is 1–200 characters with no control character.
fn validate_external_ref(external_ref: &str) -> Result<(), ApiError> {
    let chars = external_ref.chars().count();
    if !(1..=200).contains(&chars) || external_ref.trim().is_empty() {
        return Err(ApiError::unprocessable(
            "external_ref must be 1–200 characters and not blank",
        ));
    }
    if external_ref.chars().any(char::is_control) {
        return Err(ApiError::unprocessable(
            "external_ref must not contain a control character",
        ));
    }
    Ok(())
}

/// Whether `e` is the unique index on `(tenant, external_ref)` refusing a write.
fn external_ref_taken(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(sqlx::error::DatabaseError::constraint)
        == Some("ablese_external_ref")
}

fn external_ref_conflict() -> ApiError {
    ApiError::conflict("another reading order already carries this external_ref")
        .with_code("external_ref_taken")
}

/// Register readings and gas factors reported when a reading order completes.
///
/// Every quantity is a `Decimal`, matching the `NUMERIC` columns that hold it.
/// They were `f64`: on the way in that silently rounded a five-decimal
/// Zählerstand, and on the way out sqlx has no `NUMERIC → f64` decode at all, so
/// listing or fetching any *completed* order failed with a type-mismatch error
/// the moment a register reading was present.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListReadingOrdersQuery {
    pub malo_id: Option<String>,
    pub status: Option<String>,
    pub anlass: Option<String>,
    /// The order that carries this reference of the executing system.
    pub external_ref: Option<String>,
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
    pub sparte: String,
    pub obis_code: Option<String>,
    pub fehlschlag_grund: Option<String>,
    pub external_ref: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

/// The columns of a [`ReadingOrderRow`], as a literal a query `concat!`s.
macro_rules! row_columns {
    () => {
        "id,malo_id,melo_id,anlass,auftraggeber_rolle,\
         ausfuehrender_msb,geplant_am,ausfuehrt_bis,status,\
         zaehlerstand_kwh,zaehlerstand_qm3,ausgefuehrt_am,\
         mscons_ref,auftrag_position_id,insrpt_process_id,sparte,obis_code,\
         fehlschlag_grund,external_ref,created_at"
    };
}

/// `POST /api/v1/reading-orders` — schedule a meter reading.
#[utoipa::path(
    post,
    path = "/api/v1/reading-orders",
    tag = "reading-orders",
    request_body(content = serde_json::Value, description = "A `CreateReadingOrderRequest`"),
    responses(
        (status = 201, description = "Created", body = serde_json::Value),
        (status = 400, description = "Malformed request", body = mako_service::ApiError),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 409, description = "`external_ref_taken`: another order carries it", body = mako_service::ApiError),
        (status = 422, description = "Refused on its content", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn create_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<CreateReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    // Refused, not defaulted: the Sparte decides the unit of the Zählerstand
    // this order will report, and a gas reading filed as electricity is a
    // register value in the wrong dimension.
    let sparte = match req.sparte.as_deref() {
        None => {
            return ApiError::unprocessable_with(
                "sparte is required: it decides the unit of the Zählerstand",
                serde_json::json!({ "expected": crate::domain::Sparte::CODES }),
            )
            .into_response();
        }
        Some(raw) => match crate::domain::parse_sparte(raw) {
            Some(s) => s,
            None => {
                return ApiError::unprocessable_with(
                    format!("unknown sparte `{raw}`"),
                    serde_json::json!({ "expected": crate::domain::Sparte::CODES }),
                )
                .into_response();
            }
        },
    };

    if let Some(external_ref) = req.external_ref.as_deref()
        && let Err(e) = validate_external_ref(external_ref)
    {
        return e.into_response();
    }

    let id = uuid::Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO ablese_auftraege
         (id,malo_id,melo_id,tenant,anlass,auftraggeber_rolle,
          ausfuehrender_msb,geplant_am,ausfuehrt_bis,
          auftrag_position_id,insrpt_process_id,sparte,obis_code,external_ref)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
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
    .bind(&req.external_ref)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": id, "status": "OFFEN" })),
        )
            .into_response(),
        Err(e) if external_ref_taken(&e) => external_ref_conflict().into_response(),
        Err(e) => ApiError::internal(e).into_response(),
    }
}

/// `GET /api/v1/reading-orders?malo_id=&status=&anlass=&external_ref=&limit=`
#[utoipa::path(
    get,
    path = "/api/v1/reading-orders",
    tag = "reading-orders",
    params(ListReadingOrdersQuery),
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn list_reading_orders(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(q): Query<ListReadingOrdersQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    let rows = sqlx::query_as::<_, ReadingOrderRow>(concat!(
        "SELECT ",
        row_columns!(),
        " FROM ablese_auftraege
         WHERE tenant=$1
           AND ($2::text IS NULL OR malo_id=$2)
           AND ($3::text IS NULL OR status=$3)
           AND ($4::text IS NULL OR anlass=$4)
           AND ($6::text IS NULL OR external_ref=$6)
         ORDER BY geplant_am DESC
         LIMIT $5"
    ))
    .bind(&state.tenant)
    .bind(&q.malo_id)
    .bind(&q.status)
    .bind(&q.anlass)
    .bind(q.limit.unwrap_or(100).min(1000))
    .bind(&q.external_ref)
    .fetch_all(state.repo.pool())
    .await;

    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => ApiError::internal(e).into_response(),
    }
}

/// `GET /api/v1/reading-orders/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/reading-orders/{id}",
    tag = "reading-orders",
    params(("id" = uuid::Uuid, Path, description = "Reading order id")),
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 404, description = "Not found", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn get_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    let row = sqlx::query_as::<_, ReadingOrderRow>(concat!(
        "SELECT ",
        row_columns!(),
        " FROM ablese_auftraege WHERE id=$1 AND tenant=$2"
    ))
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await;

    match row {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => ApiError::not_found(format!("reading order {id} not found")).into_response(),
        Err(e) => ApiError::internal(e).into_response(),
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
/// (`metering::series::reading::consumption_between`).
///
/// The order row keeps its copy: it is the record of what this Auftrag returned,
/// which is a different fact from what the register held at an instant.
#[utoipa::path(
    put,
    path = "/api/v1/reading-orders/{id}/complete",
    tag = "reading-orders",
    params(("id" = uuid::Uuid, Path, description = "Reading order id")),
    request_body(content = serde_json::Value, description = "A `CompleteReadingOrderRequest`"),
    responses(
        (status = 204, description = "Done, no body"),
        (status = 400, description = "Malformed request", body = mako_service::ApiError),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 404, description = "Not found", body = mako_service::ApiError),
        (status = 422, description = "Refused on its content", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn complete_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<CompleteReadingOrderRequest>,
) -> impl IntoResponse {
    use sqlx::Row as _;

    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    // The instant the register *held* the value, not the instant it was typed
    // in: a Jahresablesung is routinely entered days later, and the period
    // bounds select on the reading's own timestamp.
    let abgelesen_am = req.abgelesen_am.unwrap_or_else(OffsetDateTime::now_utc);

    // The status change and the filed Zählerstand commit together: the order
    // row is updated inside a transaction that commits only once the reading
    // store has accepted the value. Every early return below — a refusal or a
    // failed reading write — drops the transaction and leaves the order open,
    // so it is never `AUSGEFUEHRT` without the reading billing reads. The
    // reading store spans the hot and the cold tier and cannot join this
    // transaction, so the order of the two writes is what decides the failure
    // direction: a commit that fails after the reading was filed leaves the
    // order open, and completing it again re-files the same register value.
    let mut tx = match state.repo.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => return ApiError::internal(e).into_response(),
    };

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
         RETURNING malo_id, melo_id, sparte, obis_code, anlass, external_ref, mscons_ref",
    )
    .bind(req.zaehlerstand_kwh)
    .bind(req.zaehlerstand_qm3)
    .bind(req.brennwert)
    .bind(req.zustandszahl)
    .bind(&req.mscons_ref)
    .bind(id)
    .bind(&state.tenant)
    .bind(abgelesen_am)
    .fetch_optional(&mut *tx)
    .await;

    let row = match res {
        Ok(Some(row)) => row,
        Ok(None) => {
            return ApiError::not_found(format!("no open reading order {id}")).into_response();
        }
        Err(e) => return ApiError::internal(e).into_response(),
    };

    let malo_id: String = row.try_get("malo_id").unwrap_or_default();
    // The column is `NOT NULL` under a `CHECK`, so a row without a Sparte
    // this service reads is a broken row, not an electricity order.
    let Some(sparte) = row
        .try_get::<String, _>("sparte")
        .ok()
        .as_deref()
        .and_then(crate::domain::parse_sparte)
    else {
        return ApiError::internal(anyhow::anyhow!(
            "reading order {id} carries no readable sparte"
        ))
        .into_response();
    };
    let obis_code: Option<String> = row.try_get("obis_code").unwrap_or(None);

    // The register value in the unit the register counts — the same rule the
    // Zählerstandsgang follows. `zaehlerstand_qm3` is a volume and belongs to a
    // gas or water meter; `zaehlerstand_kwh` is an energy register. Reporting
    // the one that does not match the order's Sparte is a decode fault, not a
    // reading, so it is refused rather than filed in the wrong dimension.
    let (value, mismatched) = match sparte.measured_unit() {
        metering::Unit::CubicMetre => (req.zaehlerstand_qm3, req.zaehlerstand_kwh.is_some()),
        // Energy registers — and any unit a register does not count, which no
        // Zählerstand field can carry.
        _ => (req.zaehlerstand_kwh, req.zaehlerstand_qm3.is_some()),
    };
    if mismatched {
        return ApiError::unprocessable(format!(
            "the order is for {}, whose register counts {} — the other \
                     Zählerstand field belongs to a different commodity",
            sparte.as_str(),
            sparte.measured_unit().as_str(),
        ))
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
            return ApiError::unprocessable_with(
                format!(
                    "the order names no {} , so its Zählerstand cannot be filed: a reading \
                         belongs to one register of one meter, and stored without them a second \
                         meter's reading would overwrite the first",
                    missing.join(" and no ")
                ),
                serde_json::json!({ "missing": missing }),
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
            ablesung_herkunft: None,
        };
        if let Err(e) = state
            .repo
            .store_readings(std::slice::from_ref(&reading))
            .await
        {
            // Returning drops the transaction: the order stays open.
            tracing::error!(
                order = %id, malo_id = %malo_id, error = %e,
                "edmd: the Zählerstand of a reading order could not be filed; the order stays open"
            );
            return ApiError::internal(e).into_response();
        }
    }

    // The completion and its announcement commit together.
    let completed = ReadingOrderCompleted {
        order_id: id.to_string(),
        external_ref: row.try_get("external_ref").unwrap_or(None),
        malo_id: malo_id.clone(),
        melo_id,
        anlass: row.try_get("anlass").unwrap_or_default(),
        sparte: sparte.as_str().to_owned(),
        obis_code: row.try_get("obis_code").unwrap_or(None),
        abgelesen_am: abgelesen_am
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        zaehlerstand: value,
        einheit: value.map(|_| sparte.measured_unit().as_str().to_owned()),
        mscons_ref: row.try_get("mscons_ref").unwrap_or(None),
    };
    if let Err(e) = emit_typed_tx(&mut tx, &state.tenant, &malo_id, &completed).await {
        return ApiError::internal(e).into_response();
    }
    if let Err(e) = tx.commit().await {
        return ApiError::internal(e).into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `PUT /api/v1/reading-orders/{id}/cancel`
#[utoipa::path(
    put,
    path = "/api/v1/reading-orders/{id}/cancel",
    tag = "reading-orders",
    params(("id" = uuid::Uuid, Path, description = "Reading order id")),
    responses(
        (status = 204, description = "Done, no body"),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 404, description = "Not found", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn cancel_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
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
        Ok(r) if r.rows_affected() == 0 => {
            ApiError::not_found(format!("no open reading order {id}")).into_response()
        }
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::internal(e).into_response(),
    }
}

/// Body for `PUT /api/v1/reading-orders/{id}/fail`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FailReadingOrderRequest {
    /// Ablesehindernis — why no reading could be taken.
    grund: String,
    /// Free-text detail for the field report.
    #[serde(default)]
    notiz: Option<String>,
}

/// Why a reading could not be taken — the closed vocabulary of
/// `PUT /api/v1/reading-orders/{id}/fail`, of `fehlschlag_grund` and of
/// `de.mako.messwert.reading.order.failed`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    utoipa::ToSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Ablesehindernis {
    /// Kein Zutritt zum Gebäude oder Grundstück.
    KeinZutritt,
    /// Zähler nicht zugänglich (verbaut, verschlossen).
    ZaehlerUnzugaenglich,
    /// Zähler defekt oder nicht ablesbar.
    ZaehlerDefekt,
    /// Zähler nicht auffindbar.
    ZaehlerNichtAuffindbar,
    /// Ablesung von der Kundin oder dem Kunden verweigert.
    KundeVerweigert,
    /// Abgelesener Wert unplausibel.
    AblesungUnplausibel,
    /// Sonstiges Hindernis; die Notiz sagt, welches.
    Sonstiges,
}

impl Ablesehindernis {
    /// Every Ablesehindernis, in the order the vocabulary is published.
    pub(crate) const ALL: [Self; 7] = [
        Self::KeinZutritt,
        Self::ZaehlerUnzugaenglich,
        Self::ZaehlerDefekt,
        Self::ZaehlerNichtAuffindbar,
        Self::KundeVerweigert,
        Self::AblesungUnplausibel,
        Self::Sonstiges,
    ];

    /// The wire and database value.
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::KeinZutritt => "KEIN_ZUTRITT",
            Self::ZaehlerUnzugaenglich => "ZAEHLER_UNZUGAENGLICH",
            Self::ZaehlerDefekt => "ZAEHLER_DEFEKT",
            Self::ZaehlerNichtAuffindbar => "ZAEHLER_NICHT_AUFFINDBAR",
            Self::KundeVerweigert => "KUNDE_VERWEIGERT",
            Self::AblesungUnplausibel => "ABLESUNG_UNPLAUSIBEL",
            Self::Sonstiges => "SONSTIGES",
        }
    }

    /// The German label a person reads.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::KeinZutritt => "Kein Zutritt",
            Self::ZaehlerUnzugaenglich => "Zähler nicht zugänglich",
            Self::ZaehlerDefekt => "Zähler defekt",
            Self::ZaehlerNichtAuffindbar => "Zähler nicht auffindbar",
            Self::KundeVerweigert => "Ablesung verweigert",
            Self::AblesungUnplausibel => "Ablesung unplausibel",
            Self::Sonstiges => "Sonstiges",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.code() == code)
    }
}

/// One entry of the published Ablesehindernis vocabulary.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub(crate) struct AblesehindernisEintrag {
    /// The value `grund` takes.
    code: Ablesehindernis,
    /// The German label.
    label: &'static str,
}

/// `GET /api/v1/reading-orders/fail-reasons`
///
/// The closed list `PUT …/fail` accepts as `grund`, with its German labels —
/// what a workforce tool maps its own reasons onto.
#[utoipa::path(
    get,
    path = "/api/v1/reading-orders/fail-reasons",
    tag = "reading-orders",
    responses(
        (status = 200, description = "Every Ablesehindernis", body = Vec<AblesehindernisEintrag>),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
    )
)]
pub(crate) async fn list_fail_reasons(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }
    Json(Ablesehindernis::ALL.map(|code| AblesehindernisEintrag {
        code,
        label: code.label(),
    }))
    .into_response()
}

/// `PUT /api/v1/reading-orders/{id}/fail`
///
/// Records that a dispatched reading could not be taken, with the
/// Ablesehindernis that prevented it.
///
/// Distinct from `/cancel`: a cancelled order is no longer owed, whereas a
/// failed one still is. A failed JAHRESABLESUNG past its deadline remains a
/// § 40b Abs. 1 EnWG gap, so it keeps appearing in `list_overdue_reading_orders`
/// until it is re-dispatched or the quantity is estimated under §40a EnWG.
#[utoipa::path(
    put,
    path = "/api/v1/reading-orders/{id}/fail",
    tag = "reading-orders",
    params(("id" = uuid::Uuid, Path, description = "Reading order id")),
    request_body(content = serde_json::Value, description = "A `FailReadingOrderRequest`"),
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 400, description = "Malformed request", body = mako_service::ApiError),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 404, description = "Not found", body = mako_service::ApiError),
        (status = 422, description = "Refused on its content", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn fail_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<FailReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    let Some(grund) = Ablesehindernis::parse(&req.grund) else {
        return ApiError::unprocessable_with(
            format!("unknown Ablesehindernis `{}`", req.grund),
            serde_json::json!({ "expected": Ablesehindernis::ALL.map(Ablesehindernis::code) }),
        )
        .into_response();
    };

    // The failure and its announcement commit together: the order is terminal
    // but the reading is still owed, and an unannounced failure is one nobody
    // re-dispatches.
    let mut tx = match state.repo.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => return ApiError::internal(e).into_response(),
    };
    let row = sqlx::query(
        "UPDATE ablese_auftraege
            SET status            = 'FEHLGESCHLAGEN',
                fehlschlag_grund  = $1,
                fehlschlag_notiz  = $2,
                fehlgeschlagen_am = now()
          WHERE id = $3 AND tenant = $4 AND status IN ('OFFEN','BEAUFTRAGT')
          RETURNING malo_id, anlass, ausfuehrt_bis, ausfuehrender_msb, external_ref",
    )
    .bind(grund.code())
    .bind(&req.notiz)
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(&mut *tx)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            return ApiError::not_found(format!("no open reading order {id}")).into_response();
        }
        Err(e) => return ApiError::internal(e).into_response(),
    };

    use sqlx::Row as _;
    let malo_id: String = row.try_get("malo_id").unwrap_or_default();
    let anlass: String = row.try_get("anlass").unwrap_or_default();
    let ausfuehrt_bis: Option<time::Date> = row.try_get("ausfuehrt_bis").ok().flatten();
    let ausfuehrender_msb: Option<String> = row.try_get("ausfuehrender_msb").ok().flatten();

    let failed = ReadingOrderFailed {
        order_id: id.to_string(),
        external_ref: row.try_get("external_ref").ok().flatten(),
        malo_id: malo_id.clone(),
        anlass,
        grund,
        notiz: req.notiz.clone(),
        ausfuehrt_bis: ausfuehrt_bis.map(|d| d.to_string()),
        ausfuehrender_msb,
        recommended_action:
            "Re-dispatch the reading, or estimate under §40a EnWG and document the basis".to_owned(),
    };
    if let Err(e) = emit_typed_tx(&mut tx, &state.tenant, &malo_id, &failed).await {
        return ApiError::internal(e).into_response();
    }
    if let Err(e) = tx.commit().await {
        return ApiError::internal(e).into_response();
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": id.to_string(),
            "status": "FEHLGESCHLAGEN",
            "grund": grund.code(),
            "still_owed": true,
        })),
    )
        .into_response()
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalRefRequest {
    /// The executing system's own reference: 1–200 characters, opaque, unique
    /// per tenant.
    external_ref: String,
}

/// `PUT /api/v1/reading-orders/{id}/external-ref`
///
/// Attach the executing system's reference to an order — the way an order a
/// market process or the Jahresablesung campaign created gets one. Every later
/// event of the order echoes it, and `GET /api/v1/reading-orders?external_ref=`
/// finds it.
#[utoipa::path(
    put,
    path = "/api/v1/reading-orders/{id}/external-ref",
    tag = "reading-orders",
    params(("id" = uuid::Uuid, Path, description = "Reading order id")),
    request_body = ExternalRefRequest,
    responses(
        (status = 204, description = "Attached"),
        (status = 401, description = "No or unverifiable token", body = mako_service::ApiError),
        (status = 403, description = "Cedar refused the action", body = mako_service::ApiError),
        (status = 404, description = "Not found", body = mako_service::ApiError),
        (status = 409, description = "`external_ref_taken`: another order carries it", body = mako_service::ApiError),
        (status = 422, description = "Blank, longer than 200 characters, or with a control character", body = mako_service::ApiError),
        (status = 500, description = "Internal error, with an `error_id` the log carries", body = mako_service::ApiError),
    )
)]
pub(crate) async fn put_reading_order_external_ref(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<ExternalRefRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }
    if let Err(e) = validate_external_ref(&req.external_ref) {
        return e.into_response();
    }
    let res = sqlx::query("UPDATE ablese_auftraege SET external_ref=$3 WHERE id=$1 AND tenant=$2")
        .bind(id)
        .bind(&state.tenant)
        .bind(&req.external_ref)
        .execute(state.repo.pool())
        .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => {
            ApiError::not_found(format!("reading order {id} not found")).into_response()
        }
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) if external_ref_taken(&e) => external_ref_conflict().into_response(),
        Err(e) => ApiError::internal(e).into_response(),
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// The `data` of `de.mako.messwert.reading.order.created`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct ReadingOrderCreated {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub melo_id: Option<String>,
    /// Why the reading is wanted: `JAHRESABLESUNG`, `LIEFERBEGINN`, …
    pub anlass: String,
    /// The Marktrolle that ordered it.
    pub auftraggeber_rolle: String,
    pub ausfuehrender_msb: Option<String>,
    /// `STROM`, `GAS`, `WAERME` or `WASSER` — it fixes the register's unit.
    pub sparte: String,
    pub obis_code: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub geplant_am: String,
    #[schemars(extend("format" = "date"))]
    pub ausfuehrt_bis: Option<String>,
}

impl mako_service::EventPayload for ReadingOrderCreated {
    const TYPE: &'static str = mako_events::messwert::READING_ORDER_CREATED;
}

/// The `data` of `de.mako.messwert.reading.order.completed`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct ReadingOrderCompleted {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub anlass: String,
    pub sparte: String,
    pub obis_code: Option<String>,
    /// The instant the register held the value, RFC 3339.
    #[schemars(extend("format" = "date-time"))]
    pub abgelesen_am: String,
    /// The register value; absent for an order closed without a reading.
    pub zaehlerstand: Option<Decimal>,
    /// The unit `zaehlerstand` counts in.
    pub einheit: Option<String>,
    pub mscons_ref: Option<String>,
}

impl mako_service::EventPayload for ReadingOrderCompleted {
    const TYPE: &'static str = mako_events::messwert::READING_ORDER_COMPLETED;
}

/// The `data` of `de.mako.messwert.reading.order.failed`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct ReadingOrderFailed {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub anlass: String,
    pub grund: Ablesehindernis,
    pub notiz: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub ausfuehrt_bis: Option<String>,
    pub ausfuehrender_msb: Option<String>,
    pub recommended_action: String,
}

impl mako_service::EventPayload for ReadingOrderFailed {
    const TYPE: &'static str = mako_events::messwert::READING_ORDER_FAILED;
}

/// Enqueue `payload` on the caller's transaction, subject the Marktlokation.
async fn emit_typed_tx<P: mako_service::EventPayload>(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    malo_id: &str,
    payload: &P,
) -> anyhow::Result<()> {
    let ce =
        mako_service::CloudEvent::typed(mako_service::source("edmd", tenant), malo_id, payload)?
            .extension("tenantid", tenant.to_owned());
    crate::outbox::emit_tx(conn, &ce).await?;
    Ok(())
}

/// The schema of every reading-order payload.
#[cfg(test)]
pub(crate) fn event_schemas() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        mako_service::cloudevent::event_schema::<ReadingOrderCreated>(),
        mako_service::cloudevent::event_schema::<ReadingOrderCompleted>(),
        mako_service::cloudevent::event_schema::<ReadingOrderFailed>(),
    ]
}

/// Announce reading orders not announced yet, oldest first, at most 500 a
/// pass. The stamp and the events are one transaction, so an order is
/// announced once and by one replica.
///
/// # Errors
///
/// Propagates database errors.
pub async fn announce_created(pool: &sqlx::PgPool, tenant: &str) -> anyhow::Result<usize> {
    use sqlx::Row as _;
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        "UPDATE ablese_auftraege a SET created_announced_at = now()
          WHERE a.id IN (
              SELECT id FROM ablese_auftraege
               WHERE tenant = $1 AND created_announced_at IS NULL
               ORDER BY created_at
               LIMIT 500
               FOR UPDATE SKIP LOCKED)
          RETURNING a.id, a.external_ref, a.malo_id, a.melo_id, a.anlass,
                    a.auftraggeber_rolle, a.ausfuehrender_msb, a.sparte, a.obis_code,
                    a.geplant_am, a.ausfuehrt_bis",
    )
    .bind(tenant)
    .fetch_all(&mut *tx)
    .await?;
    for r in &rows {
        let malo_id: String = r.try_get("malo_id")?;
        let created = ReadingOrderCreated {
            order_id: r.try_get::<uuid::Uuid, _>("id")?.to_string(),
            external_ref: r.try_get("external_ref")?,
            malo_id: malo_id.clone(),
            melo_id: r.try_get("melo_id")?,
            anlass: r.try_get("anlass")?,
            auftraggeber_rolle: r.try_get("auftraggeber_rolle")?,
            ausfuehrender_msb: r.try_get("ausfuehrender_msb")?,
            sparte: r.try_get("sparte")?,
            obis_code: r.try_get("obis_code")?,
            geplant_am: r.try_get::<time::Date, _>("geplant_am")?.to_string(),
            ausfuehrt_bis: r
                .try_get::<Option<time::Date>, _>("ausfuehrt_bis")?
                .map(|d| d.to_string()),
        };
        emit_typed_tx(&mut tx, tenant, &malo_id, &created).await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

/// Spawn the announcer of newly created reading orders.
pub fn spawn_announcer(
    pool: Arc<sqlx::PgPool>,
    tenant: String,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = shutdown.cancelled() => break,
            }
            // A campaign writes tens of thousands of orders: drain in passes
            // rather than wait a tick per 500.
            loop {
                match announce_created(&pool, &tenant).await {
                    Ok(500) => {}
                    Ok(_) => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "edmd: reading-order announcer: pass failed");
                        break;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `events/` holds the schema of every payload this service emits through
    /// a binding (`BLESS_MCP_SNAPSHOT=1` rewrites it).
    #[test]
    fn the_committed_event_schemas_are_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("events");
        if let Err(e) = mako_service::cloudevent::compare_event_schemas(&dir, &event_schemas()) {
            panic!("{e}");
        }
    }

    /// The SQL `CHECK` on `fehlschlag_grund` and the enum name the same values,
    /// both ways: a value only the enum has is refused by the database on the
    /// request that reports it, and one only the `CHECK` has can never be sent.
    #[test]
    fn the_ablesehindernis_vocabulary_is_the_schemas_check() {
        let schema = include_str!("../../migrations/0001_schema.sql");
        let from = schema.find("CHECK (fehlschlag_grund IN (").unwrap();
        let list = &schema[from..from + schema[from..].find("))").unwrap()];
        let mut in_sql: Vec<&str> = list.split('\'').skip(1).step_by(2).collect();
        in_sql.sort_unstable();
        let mut in_code: Vec<&str> = Ablesehindernis::ALL.map(Ablesehindernis::code).to_vec();
        in_code.sort_unstable();
        assert_eq!(in_sql, in_code);
        for g in Ablesehindernis::ALL {
            assert_eq!(serde_json::to_value(g).unwrap(), g.code(), "wire value");
            assert_eq!(Ablesehindernis::parse(g.code()), Some(g));
            assert!(!g.label().is_empty());
        }
    }
}
