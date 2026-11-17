//! The REST surface: endpoint CRUD, rotation, test delivery, the delivery log,
//! replay, and the event feed — each route registered with its document entry.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Extension;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header, request::Parts};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Subscriptions;
use super::store::{
    DELIVERY_COLS, Delivery, DeliveryRow, ENDPOINT_COLS, Endpoint, EndpointRow, new_secret, rfc3339,
};
use crate::page::{Page, PageQuery};
use crate::{ApiError, ApiResult, Json};

/// Cedar action every endpoint, delivery and replay route requires.
pub const MANAGE: &str = "manage-subscription";
/// Cedar action the event feed requires.
pub const READ_EVENTS: &str = "read-events";
/// The longest overlap a rotation keeps the previous secret signing.
pub const MAX_OVERLAP_HOURS: u32 = 168;
/// The most events one window replay re-delivers.
pub const MAX_WINDOW_EVENTS: i64 = 10_000;

/// The caller: its Cedar principal, the enforcer, and the partner it acts
/// for when the service's authentication layer marked it as one.
pub struct Caller {
    principal: crate::cedar::CedarPrincipal,
    enforcer: Arc<crate::cedar::CedarEnforcer>,
    partner: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let claims = crate::oidc::Claims::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let Extension(enforcer) =
            Extension::<Arc<crate::cedar::CedarEnforcer>>::from_request_parts(parts, state)
                .await
                .map_err(|_| {
                    ApiError::internal(anyhow::anyhow!("no CedarEnforcer is layered"))
                        .into_response()
                })?;
        let partner = parts
            .extensions
            .get::<super::Partner>()
            .map(|p| p.0.clone());
        Ok(Self {
            principal: claims.principal(),
            enforcer,
            partner,
        })
    }
}

impl Caller {
    fn allow(&self, action: &str, tenant: &str) -> ApiResult<()> {
        self.enforcer
            .check(&self.principal, action, tenant)
            .map_err(|e| ApiError::forbidden(e.to_string()))
    }
}

/// The body of `POST /api/v1/webhooks/endpoints`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EndpointCreate {
    /// Unique per service; `default` is reserved for the config URL.
    pub name: String,
    /// `https` URL deliveries are POSTed to.
    pub url: String,
    /// `mako_events::matches` patterns (`de.vertrag.*`); empty matches every type.
    #[serde(default)]
    pub event_types: Vec<String>,
    /// CloudEvent extensions an event must carry with these values.
    #[serde(default)]
    pub extensions: BTreeMap<String, String>,
}

/// The body of `PUT /api/v1/webhooks/endpoints/{id}` — the whole endpoint.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EndpointUpdate {
    /// `https` URL deliveries are POSTed to.
    pub url: String,
    /// `mako_events::matches` patterns; empty matches every type.
    #[serde(default)]
    pub event_types: Vec<String>,
    /// CloudEvent extensions an event must carry with these values.
    #[serde(default)]
    pub extensions: BTreeMap<String, String>,
    /// Whether deliveries are made.
    pub active: bool,
}

/// A created endpoint and its secret — the only answer that shows it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EndpointCreated {
    /// The endpoint.
    pub endpoint: Endpoint,
    /// The `whsec_` signing secret; shown once.
    pub secret: String,
}

/// The body of `POST /api/v1/webhooks/endpoints/{id}/rotate-secret`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RotateSecret {
    /// How long the previous secret keeps signing, at most 168 hours.
    pub overlap_hours: u32,
}

/// A rotated secret — the only answer that shows it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SecretRotated {
    /// The new `whsec_` signing secret; shown once.
    pub secret: String,
    /// Until when the previous secret also signs.
    #[schema(format = DateTime)]
    pub previous_expires_at: String,
}

/// What a test delivery came to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TestDelivery {
    /// Whether the endpoint answered 2xx.
    pub delivered: bool,
    /// The HTTP status, when it answered.
    pub status: Option<u16>,
    /// How long it took, when it answered.
    pub latency_ms: Option<i32>,
    /// Why it was not delivered.
    pub error: Option<String>,
}

/// The body of `POST /api/v1/webhooks/endpoints/{id}/replay`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayWindow {
    /// Start of the window, inclusive (RFC 3339).
    #[schema(format = DateTime)]
    pub from: String,
    /// End of the window, exclusive (RFC 3339).
    #[schema(format = DateTime)]
    pub to: String,
}

/// How many deliveries a replay queued.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Replayed {
    /// Deliveries queued again or for the first time.
    pub queued: i64,
}

/// Which deliveries the log lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFilter {
    /// Every delivery.
    All,
    /// Owed.
    Pending,
    /// Accepted.
    Delivered,
    /// Retrying after a failure, or dead-lettered.
    Failed,
}

/// The query of `GET /api/v1/webhooks/endpoints/{id}/deliveries`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct DeliveryQuery {
    /// Which deliveries; every one when absent.
    pub status: Option<DeliveryFilter>,
    /// Rows per page, 1–200; 50 when absent.
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

/// The query of `GET /api/v1/events`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct FeedQuery {
    /// A `mako_events::matches` pattern (`de.vertrag.*`); every type when absent.
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub event_type: Option<String>,
    /// Rows per page, 1–200; 50 when absent.
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

fn page_query(limit: Option<u32>, cursor: Option<String>) -> PageQuery {
    PageQuery { limit, cursor }
}

fn etag(version: i64) -> String {
    format!("\"{version}\"")
}

/// The version an `If-Match` names.
fn if_match(headers: &HeaderMap) -> ApiResult<i64> {
    let raw = crate::headers::single_str(headers, header::IF_MATCH.as_str())
        .map_err(|_| ApiError::bad_request("If-Match is repeated").with_code("if_match_invalid"))?
        .ok_or_else(|| {
            ApiError::bad_request("this write needs If-Match with the endpoint's ETag")
                .with_code("if_match_required")
        })?;
    raw.trim()
        .trim_start_matches("W/")
        .trim_matches('"')
        .parse()
        .map_err(|_| {
            ApiError::bad_request("If-Match is not an ETag of this endpoint")
                .with_code("if_match_invalid")
        })
}

fn not_found(id: &str) -> ApiError {
    ApiError::not_found(format!("no webhook endpoint {id}")).with_code("endpoint_not_found")
}

fn parse_id(raw: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(raw).map_err(|_| not_found(raw))
}

fn check_filter(event_types: &[String], extensions: &BTreeMap<String, String>) -> ApiResult<()> {
    if let Some(p) = event_types
        .iter()
        .find(|p| !p.starts_with("de.") && p.as_str() != "*")
    {
        return Err(ApiError::unprocessable(format!(
            "event type pattern {p:?} names no mako event: types start with `de.`"
        ))
        .with_code("event_type_pattern_invalid"));
    }
    if let Some(k) = extensions.keys().find(|k| {
        k.is_empty()
            || !k
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    }) {
        return Err(ApiError::unprocessable(format!(
            "extension {k:?} is not a CloudEvents extension name (lowercase letters and digits)"
        ))
        .with_code("extension_invalid"));
    }
    Ok(())
}

impl Subscriptions {
    fn check_api_url(&self, url: &str) -> ApiResult<()> {
        super::ssrf::check_url(url, self.inner.cfg.allow_loopback)
            .map(|_| ())
            .map_err(|e| {
                ApiError::unprocessable(format!("url: {e}")).with_code("endpoint_url_not_allowed")
            })
    }

    /// The endpoint `id` as `caller` may see it: a partner sees only its own.
    async fn endpoint(&self, id: Uuid, partner: Option<&str>) -> ApiResult<EndpointRow> {
        let row: Option<EndpointRow> = sqlx::query_as(crate::sql::dynamic(format!(
            "SELECT {ENDPOINT_COLS} FROM webhook_endpoints
              WHERE tenant = $1 AND id = $2 AND ($3::text IS NULL OR partner = $3)"
        )))
        .bind(&self.inner.tenant)
        .bind(id)
        .bind(partner)
        .fetch_optional(&self.inner.pool)
        .await?;
        row.ok_or_else(|| not_found(&id.to_string()))
    }

    /// The endpoints `partner` may see, a page ordered by name — the read the
    /// `list_webhook_endpoints` MCP tool answers with.
    ///
    /// # Errors
    ///
    /// A refused `limit` or cursor, or a database error.
    pub async fn list_endpoints(
        &self,
        partner: Option<&str>,
        page: &PageQuery,
    ) -> ApiResult<Page<Endpoint>> {
        let limit = page.limit()?;
        let filter = ("webhook_endpoints", partner);
        let after: Option<String> = page.after(&filter)?;
        let rows: Vec<EndpointRow> = sqlx::query_as(crate::sql::dynamic(format!(
            "SELECT {ENDPOINT_COLS} FROM webhook_endpoints
              WHERE tenant = $1 AND ($2::text IS NULL OR partner = $2)
                AND ($3::text IS NULL OR name > $3)
              ORDER BY name
              LIMIT $4"
        )))
        .bind(&self.inner.tenant)
        .bind(partner)
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.inner.pool)
        .await?;
        Ok(Page::from_rows(rows, limit, &filter, |r| r.name.clone())
            .map(EndpointRow::into_endpoint))
    }

    /// The deliveries of one endpoint `partner` may see, newest first — the
    /// read the `list_webhook_deliveries` MCP tool answers with.
    ///
    /// # Errors
    ///
    /// An unknown endpoint, a refused `limit` or cursor, or a database error.
    pub async fn list_deliveries(
        &self,
        partner: Option<&str>,
        endpoint_id: &str,
        status: DeliveryFilter,
        page: &PageQuery,
    ) -> ApiResult<Page<Delivery>> {
        let id = parse_id(endpoint_id)?;
        self.endpoint(id, partner).await?;
        let limit = page.limit()?;
        let filter = ("webhook_deliveries", id, status);
        let after: Option<(String, Uuid)> = page.after(&filter)?;
        let state = match status {
            DeliveryFilter::All => "true",
            DeliveryFilter::Pending => "delivered_at IS NULL AND dead_lettered_at IS NULL",
            DeliveryFilter::Delivered => "delivered_at IS NOT NULL",
            DeliveryFilter::Failed => {
                "delivered_at IS NULL AND (dead_lettered_at IS NOT NULL OR last_error IS NOT NULL)"
            }
        };
        let (after_at, after_id) = match after {
            Some((at, id)) => (
                Some(
                    OffsetDateTime::parse(&at, &time::format_description::well_known::Rfc3339)
                        .map_err(|_| {
                            ApiError::bad_request("cursor is not valid here")
                                .with_code("cursor_invalid")
                        })?,
                ),
                Some(id),
            ),
            None => (None, None),
        };
        let rows: Vec<DeliveryRow> = sqlx::query_as(crate::sql::dynamic(format!(
            "SELECT {DELIVERY_COLS} FROM webhook_deliveries
              WHERE tenant = $1 AND endpoint_id = $2 AND {state}
                AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4))
              ORDER BY created_at DESC, id DESC
              LIMIT $5"
        )))
        .bind(&self.inner.tenant)
        .bind(id)
        .bind(after_at)
        .bind(after_id)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.inner.pool)
        .await?;
        Ok(
            Page::from_rows(rows, limit, &filter, |r| (rfc3339(r.created_at), r.id))
                .map(DeliveryRow::into_delivery),
        )
    }
}

/// Register an endpoint; the answer shows its secret once.
#[utoipa::path(
    post, path = "/api/v1/webhooks/endpoints", tag = "webhooks",
    request_body = EndpointCreate,
    responses(
        (status = 201, body = EndpointCreated),
        (status = 409, description = "`endpoint_name_taken`", body = ApiError),
        (status = 422, description = "`endpoint_url_not_allowed`, `endpoint_name_reserved`", body = ApiError),
    )
)]
pub async fn create_endpoint(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Json(req): Json<EndpointCreate>,
) -> ApiResult<(StatusCode, Json<EndpointCreated>)> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    if req.name == super::DEFAULT_ENDPOINT {
        return Err(ApiError::unprocessable(
            "the name `default` belongs to the endpoint the service's config names",
        )
        .with_code("endpoint_name_reserved"));
    }
    if req.name.trim().is_empty() {
        return Err(
            ApiError::unprocessable("an endpoint needs a name").with_code("endpoint_name_invalid")
        );
    }
    subs.check_api_url(&req.url)?;
    check_filter(&req.event_types, &req.extensions)?;
    let secret = new_secret();
    let inserted: Option<EndpointRow> = sqlx::query_as(crate::sql::dynamic(format!(
        "INSERT INTO webhook_endpoints
             (id, tenant, name, url, event_types, extensions, owner, partner, secret)
         VALUES ($1, $2, $3, $4, $5, $6, 'api', $7, $8)
         ON CONFLICT (tenant, name) DO NOTHING
         RETURNING {ENDPOINT_COLS}"
    )))
    .bind(Uuid::new_v4())
    .bind(&subs.inner.tenant)
    .bind(&req.name)
    .bind(&req.url)
    .bind(&req.event_types)
    .bind(serde_json::to_value(&req.extensions).unwrap_or_default())
    .bind(caller.partner.as_deref())
    .bind(&secret)
    .fetch_optional(&subs.inner.pool)
    .await?;
    let row = inserted.ok_or_else(|| {
        ApiError::conflict(format!("an endpoint named {:?} exists", req.name))
            .with_code("endpoint_name_taken")
    })?;
    Ok((
        StatusCode::CREATED,
        Json(EndpointCreated {
            endpoint: row.into_endpoint(),
            secret,
        }),
    ))
}

/// The endpoints, a page ordered by name.
#[utoipa::path(
    get, path = "/api/v1/webhooks/endpoints", tag = "webhooks",
    params(PageQuery),
    responses((status = 200, body = Page<Endpoint>), (status = 400, body = ApiError))
)]
pub async fn list_endpoints(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Page<Endpoint>>> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    Ok(Json(
        subs.list_endpoints(caller.partner.as_deref(), &page)
            .await?,
    ))
}

/// One endpoint, with its version as `ETag`.
#[utoipa::path(
    get, path = "/api/v1/webhooks/endpoints/{id}", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id")),
    responses((status = 200, body = Endpoint), (status = 404, body = ApiError))
)]
pub async fn get_endpoint(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    let row = subs
        .endpoint(parse_id(&id)?, caller.partner.as_deref())
        .await?;
    let tag = etag(row.version);
    Ok(([(header::ETAG, tag)], Json(row.into_endpoint())).into_response())
}

fn refuse_config_owned(row: &EndpointRow) -> ApiResult<()> {
    if row.owner() == super::Owner::Config {
        return Err(ApiError::conflict(
            "this endpoint is the one the service's config names; change it there",
        )
        .with_code("endpoint_config_owned"));
    }
    Ok(())
}

fn version_conflict() -> ApiError {
    ApiError::precondition_failed("the endpoint changed since its ETag was read")
        .with_code("endpoint_version_mismatch")
}

/// Replace an endpoint; `If-Match` names the version read.
#[utoipa::path(
    put, path = "/api/v1/webhooks/endpoints/{id}", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id")),
    request_body = EndpointUpdate,
    responses(
        (status = 200, body = Endpoint),
        (status = 409, description = "`endpoint_config_owned`", body = ApiError),
        (status = 412, description = "`endpoint_version_mismatch`", body = ApiError),
    )
)]
pub async fn update_endpoint(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<EndpointUpdate>,
) -> ApiResult<Response> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    let version = if_match(&headers)?;
    let row = subs
        .endpoint(parse_id(&id)?, caller.partner.as_deref())
        .await?;
    refuse_config_owned(&row)?;
    subs.check_api_url(&req.url)?;
    check_filter(&req.event_types, &req.extensions)?;
    let updated: Option<EndpointRow> = sqlx::query_as(crate::sql::dynamic(format!(
        "UPDATE webhook_endpoints
            SET url = $3, event_types = $4, extensions = $5, active = $6,
                deactivated_reason = CASE WHEN $6 THEN NULL ELSE deactivated_reason END,
                version = version + 1, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND version = $7 AND owner = 'api'
          RETURNING {ENDPOINT_COLS}"
    )))
    .bind(&subs.inner.tenant)
    .bind(row.id)
    .bind(&req.url)
    .bind(&req.event_types)
    .bind(serde_json::to_value(&req.extensions).unwrap_or_default())
    .bind(req.active)
    .bind(version)
    .fetch_optional(&subs.inner.pool)
    .await?;
    let row = updated.ok_or_else(version_conflict)?;
    let tag = etag(row.version);
    Ok(([(header::ETAG, tag)], Json(row.into_endpoint())).into_response())
}

/// Delete an endpoint; its pending deliveries are cancelled with it.
#[utoipa::path(
    delete, path = "/api/v1/webhooks/endpoints/{id}", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id")),
    responses(
        (status = 204),
        (status = 409, description = "`endpoint_config_owned`", body = ApiError),
        (status = 412, description = "`endpoint_version_mismatch`", body = ApiError),
    )
)]
pub async fn delete_endpoint(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    let version = if_match(&headers)?;
    let row = subs
        .endpoint(parse_id(&id)?, caller.partner.as_deref())
        .await?;
    refuse_config_owned(&row)?;
    let deleted = sqlx::query(
        "DELETE FROM webhook_endpoints
          WHERE tenant = $1 AND id = $2 AND version = $3 AND owner = 'api'",
    )
    .bind(&subs.inner.tenant)
    .bind(row.id)
    .bind(version)
    .execute(&subs.inner.pool)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(version_conflict());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Rotate the secret: the new one signs from now, the previous one beside it
/// for `overlap_hours`. The answer shows the new secret once.
#[utoipa::path(
    post, path = "/api/v1/webhooks/endpoints/{id}/rotate-secret", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id")),
    request_body = RotateSecret,
    responses(
        (status = 200, body = SecretRotated),
        (status = 409, description = "`endpoint_config_owned`", body = ApiError),
        (status = 422, description = "`overlap_too_long`", body = ApiError),
    )
)]
pub async fn rotate_secret(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    Json(req): Json<RotateSecret>,
) -> ApiResult<Json<SecretRotated>> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    if req.overlap_hours > MAX_OVERLAP_HOURS {
        return Err(ApiError::unprocessable(format!(
            "the previous secret signs for at most {MAX_OVERLAP_HOURS} hours"
        ))
        .with_fields(serde_json::json!({ "code": "overlap_too_long", "max": MAX_OVERLAP_HOURS })));
    }
    let row = subs
        .endpoint(parse_id(&id)?, caller.partner.as_deref())
        .await?;
    refuse_config_owned(&row)?;
    let secret = new_secret();
    let expires: OffsetDateTime = sqlx::query_scalar(
        "UPDATE webhook_endpoints
            SET previous_secret = secret, secret = $3,
                previous_secret_expires_at = now() + ($4 * INTERVAL '1 hour'),
                version = version + 1, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND owner = 'api'
          RETURNING previous_secret_expires_at",
    )
    .bind(&subs.inner.tenant)
    .bind(row.id)
    .bind(&secret)
    .bind(i64::from(req.overlap_hours))
    .fetch_one(&subs.inner.pool)
    .await?;
    Ok(Json(SecretRotated {
        secret,
        previous_expires_at: rfc3339(expires),
    }))
}

/// Send `de.mako.webhook.test` to the endpoint now, signed like a delivery.
#[utoipa::path(
    post, path = "/api/v1/webhooks/endpoints/{id}/test", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id")),
    responses(
        (status = 200, body = TestDelivery),
        (status = 404, body = ApiError),
        (status = 415, description = "`json_required`", body = ApiError),
    )
)]
pub async fn test_endpoint(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<TestDelivery>> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    crate::json::nur_json(&headers)?;
    let row = subs
        .endpoint(parse_id(&id)?, caller.partner.as_deref())
        .await?;
    let secrets: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT secret,
                CASE WHEN previous_secret_expires_at > now() THEN previous_secret END
           FROM webhook_endpoints WHERE tenant = $1 AND id = $2",
    )
    .bind(&subs.inner.tenant)
    .bind(row.id)
    .fetch_one(&subs.inner.pool)
    .await?;
    let active: Vec<&str> = [secrets.0.as_deref(), secrets.1.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    Ok(Json(super::worker::send_test(&subs, &row, &active).await))
}

/// The deliveries of one endpoint, newest first.
#[utoipa::path(
    get, path = "/api/v1/webhooks/endpoints/{id}/deliveries", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id"), DeliveryQuery),
    responses((status = 200, body = Page<Delivery>), (status = 404, body = ApiError))
)]
pub async fn list_deliveries(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<DeliveryQuery>,
) -> ApiResult<Json<Page<Delivery>>> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    let page = page_query(q.limit, q.cursor);
    Ok(Json(
        subs.list_deliveries(
            caller.partner.as_deref(),
            &id,
            q.status.unwrap_or(DeliveryFilter::All),
            &page,
        )
        .await?,
    ))
}

fn outside_retention(oldest: OffsetDateTime) -> ApiError {
    ApiError::unprocessable(format!(
        "the outbox keeps events from {} on; nothing older can be replayed",
        rfc3339(oldest)
    ))
    .with_fields(serde_json::json!({
        "code": "outside_retention",
        "oldest_replayable": rfc3339(oldest),
    }))
}

impl Subscriptions {
    fn oldest_replayable(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc() - time::Duration::days(i64::from(self.inner.cfg.retention_days))
    }

    /// Queue every event of `[from, to)` again for `endpoint`: reset the
    /// deliveries it has, and create one for each matching event it has none
    /// for. Returns how many were queued.
    async fn replay_window(
        &self,
        endpoint: &EndpointRow,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> ApiResult<i64> {
        let oldest = self.oldest_replayable();
        if from < oldest {
            return Err(outside_retention(oldest));
        }
        if from >= to {
            return Err(ApiError::unprocessable("`from` must be before `to`")
                .with_code("replay_window_invalid"));
        }
        let mut tx = self.inner.pool.begin().await?;
        let events: Vec<(String, String, i64, i64, Option<String>, serde_json::Value)> =
            sqlx::query_as(crate::sql::dynamic(format!(
                "SELECT event_id, ce_type, txid, seq, ordering_key, envelope
                   FROM event_outbox
                  WHERE kind = 'cloudevent' AND upstream IS NULL
                    AND created_at >= $1 AND created_at < $2
                    AND txid < {}
                  ORDER BY txid, seq
                  LIMIT $3",
                crate::outbox::COMMITTED_BELOW
            )))
            .bind(from)
            .bind(to)
            .bind(MAX_WINDOW_EVENTS + 1)
            .fetch_all(&mut *tx)
            .await?;
        if i64::try_from(events.len()).unwrap_or(i64::MAX) > MAX_WINDOW_EVENTS {
            return Err(ApiError::unprocessable(format!(
                "the window holds more than {MAX_WINDOW_EVENTS} events; replay a shorter one"
            ))
            .with_fields(serde_json::json!({
                "code": "replay_window_too_large",
                "max": MAX_WINDOW_EVENTS,
            })));
        }
        let mut queued = 0i64;
        for (event_id, ce_type, txid, seq, ordering_key, envelope) in &events {
            if !endpoint.matches(envelope) {
                continue;
            }
            let n = sqlx::query(
                "INSERT INTO webhook_deliveries
                     (tenant, endpoint_id, event_id, ce_type, event_txid, event_seq, ordering_key)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (endpoint_id, event_id) DO UPDATE
                    SET attempts = 0, next_attempt_at = now(), delivered_at = NULL,
                        dead_lettered_at = NULL, last_error = NULL",
            )
            .bind(&self.inner.tenant)
            .bind(endpoint.id)
            .bind(event_id)
            .bind(ce_type)
            .bind(txid)
            .bind(seq)
            .bind(ordering_key)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            queued += i64::try_from(n).unwrap_or(0);
        }
        tx.commit().await?;
        Ok(queued)
    }
}

/// Queue every event of a time window again for one endpoint, including
/// events that never had a delivery to it.
#[utoipa::path(
    post, path = "/api/v1/webhooks/endpoints/{id}/replay", tag = "webhooks",
    params(("id" = String, Path, description = "The endpoint's id")),
    request_body = ReplayWindow,
    responses(
        (status = 200, body = Replayed),
        (status = 422, description = "`outside_retention` (with `oldest_replayable`), `replay_window_invalid`, `replay_window_too_large`", body = ApiError),
    )
)]
pub async fn replay_window(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    Json(req): Json<ReplayWindow>,
) -> ApiResult<Json<Replayed>> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    let parse = |s: &str| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).map_err(|_| {
            ApiError::unprocessable(format!("{s:?} is not an RFC 3339 instant"))
                .with_code("replay_window_invalid")
        })
    };
    let (from, to) = (parse(&req.from)?, parse(&req.to)?);
    let row = subs
        .endpoint(parse_id(&id)?, caller.partner.as_deref())
        .await?;
    let queued = subs.replay_window(&row, from, to).await?;
    Ok(Json(Replayed { queued }))
}

/// Queue one delivery again, whatever its state.
#[utoipa::path(
    post, path = "/api/v1/webhooks/deliveries/{id}/replay", tag = "webhooks",
    params(("id" = String, Path, description = "The delivery's id")),
    responses(
        (status = 200, body = Replayed),
        (status = 404, description = "`delivery_not_found`", body = ApiError),
        (status = 415, description = "`json_required`", body = ApiError),
        (status = 422, description = "`outside_retention`", body = ApiError),
    )
)]
pub async fn replay_delivery(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Replayed>> {
    caller.allow(MANAGE, &subs.inner.tenant)?;
    crate::json::nur_json(&headers)?;
    let missing =
        || ApiError::not_found(format!("no delivery {id}")).with_code("delivery_not_found");
    let delivery = Uuid::parse_str(&id).map_err(|_| missing())?;
    let found: Option<(bool,)> = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM event_outbox o WHERE o.event_id = d.event_id)
           FROM webhook_deliveries d
           JOIN webhook_endpoints w ON w.id = d.endpoint_id
          WHERE d.tenant = $1 AND d.id = $2 AND ($3::text IS NULL OR w.partner = $3)",
    )
    .bind(&subs.inner.tenant)
    .bind(delivery)
    .bind(caller.partner.as_deref())
    .fetch_optional(&subs.inner.pool)
    .await?;
    match found {
        None => return Err(missing()),
        Some((false,)) => return Err(outside_retention(subs.oldest_replayable())),
        Some((true,)) => {}
    }
    let n = sqlx::query(
        "UPDATE webhook_deliveries
            SET attempts = 0, next_attempt_at = now(), delivered_at = NULL,
                dead_lettered_at = NULL, last_error = NULL
          WHERE tenant = $1 AND id = $2",
    )
    .bind(&subs.inner.tenant)
    .bind(delivery)
    .execute(&subs.inner.pool)
    .await?
    .rows_affected();
    Ok(Json(Replayed {
        queued: i64::try_from(n).unwrap_or(0),
    }))
}

/// A `mako_events::matches` pattern as SQL `LIKE`, `\` escaping.
fn like_pattern(glob: &str) -> String {
    let mut out = String::with_capacity(glob.len());
    for c in glob.chars() {
        match c {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// The service's events within the retention window, in commit order — the
/// feed an integrator polls instead of, or beside, an endpoint. Each item is
/// the CloudEvent as delivered.
#[utoipa::path(
    get, path = "/api/v1/events", tag = "webhooks",
    params(FeedQuery),
    responses((status = 200, body = Page<serde_json::Value>), (status = 400, body = ApiError))
)]
pub async fn list_events(
    State(subs): State<Subscriptions>,
    caller: Caller,
    Query(q): Query<FeedQuery>,
) -> ApiResult<Json<Page<serde_json::Value>>> {
    caller.allow(READ_EVENTS, &subs.inner.tenant)?;
    let page = page_query(q.limit, q.cursor);
    let limit = page.limit()?;
    let filter = ("events", q.event_type.as_deref(), caller.partner.as_deref());
    let after: Option<(i64, i64)> = page.after(&filter)?;
    let rows: Vec<(i64, i64, serde_json::Value)> = sqlx::query_as(crate::sql::dynamic(format!(
        "SELECT txid, seq, envelope FROM event_outbox
          WHERE kind = 'cloudevent' AND upstream IS NULL
            AND txid < {}
            AND created_at >= now() - ($1 * INTERVAL '1 day')
            AND ($2::text IS NULL OR ce_type LIKE $2 ESCAPE '\\')
            AND ($3::text IS NULL OR envelope ->> '{}' = $3)
            AND ($4::bigint IS NULL OR (txid, seq) > ($4, $5))
          ORDER BY txid, seq
          LIMIT $6",
        crate::outbox::COMMITTED_BELOW,
        super::PARTNER_EXTENSION,
    )))
    .bind(i64::from(subs.inner.cfg.retention_days))
    .bind(q.event_type.as_deref().map(like_pattern))
    .bind(caller.partner.as_deref())
    .bind(after.map(|a| a.0))
    .bind(after.map(|a| a.1))
    .bind(i64::from(limit) + 1)
    .fetch_all(&subs.inner.pool)
    .await?;
    Ok(Json(
        Page::from_rows(rows, limit, &filter, |r| (r.0, r.1)).map(|r| r.2),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_glob_becomes_an_escaped_like_pattern() {
        assert_eq!(super::like_pattern("de.vertrag.*"), "de.vertrag.%");
        assert_eq!(super::like_pattern("de.a_b?"), "de.a\\_b_");
    }
}
