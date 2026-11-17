//! `Idempotency-Key` on the decision routes: a decision resent under the same
//! key gets the first answer, and dispatches nothing again.
//!
//! The comparison is the IETF `Idempotency-Key` header draft's
//! (draft-ietf-httpapi-idempotency-key-header): the same key with the same
//! request is the first request again and is answered with the first
//! response; the same key with another request is `422
//! idempotency_key_reuse`; a key whose first request is still being decided is
//! `409 idempotency_request_in_flight`. The request is the route, the entry it
//! names and the body, so one key cannot decide two entries.
//!
//! The header is optional: without it a resend meets the Pending claim and is
//! `409 already_decided`. A first request that fails with a `5xx` leaves no
//! row, so its retry runs again.

use std::future::Future;

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use mako_service::ApiError;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;

/// The header the Desk sends with every decision.
pub const HEADER: &str = "Idempotency-Key";

/// The longest key accepted.
const MAX_KEY: usize = 255;

fn fingerprint(scope: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(scope.as_bytes());
    h.update([0]);
    h.update(body);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The single `Idempotency-Key`, trimmed; `None` without one.
fn key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(key) = mako_service::headers::single_str(headers, HEADER)
        .map_err(|e| ApiError::bad_request(e.to_string()).with_code("idempotency_key_duplicate"))?
        .map(str::trim)
        .filter(|k| !k.is_empty())
    else {
        return Ok(None);
    };
    if key.len() > MAX_KEY {
        return Err(ApiError::bad_request(format!(
            "Idempotency-Key is longer than {MAX_KEY} characters"
        ))
        .with_code("idempotency_key_invalid"));
    }
    Ok(Some(key.to_owned()))
}

/// Run `decide` once per `Idempotency-Key`: `scope` names the route and the
/// entry, `body` is the request body as sent.
pub async fn once<F, Fut>(
    pool: &PgPool,
    tenant: &str,
    headers: &HeaderMap,
    scope: &str,
    body: &[u8],
    decide: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Response>,
{
    let key = match key(headers) {
        Ok(Some(k)) => k,
        Ok(None) => return decide().await,
        Err(e) => return e.into_response(),
    };
    let print = fingerprint(scope, body);
    match claim(pool, tenant, &key, &print).await {
        Ok(Claim::First) => {}
        Ok(Claim::Seen(seen)) => return seen.answer(&print),
        Err(e) => return ApiError::internal(e).into_response(),
    }
    let response = decide().await;
    let status = response.status();
    if status.is_server_error() {
        // A failure of this service: the retry decides again.
        if let Err(e) = release(pool, tenant, &key).await {
            tracing::error!(error = %e, "processd: could not release an Idempotency-Key");
        }
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            return ApiError::internal(anyhow::anyhow!("read the answer: {e}")).into_response();
        }
    };
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Err(e) = record(pool, tenant, &key, status, content_type.as_deref(), &bytes).await {
        // The decision is made; only its replay is lost, and a resend then
        // meets the Pending claim.
        tracing::error!(error = %e, "processd: could not record the answer of an Idempotency-Key");
    }
    Response::from_parts(parts, axum::body::Body::from(bytes))
}

enum Claim {
    First,
    Seen(Seen),
}

struct Seen {
    fingerprint: String,
    status: Option<i16>,
    content_type: Option<String>,
    response: Option<Vec<u8>>,
}

impl Seen {
    fn answer(self, print: &str) -> Response {
        if self.fingerprint != print {
            return ApiError::unprocessable(
                "this Idempotency-Key was already used for another decision or body",
            )
            .with_code("idempotency_key_reuse")
            .into_response();
        }
        let Some(status) = self
            .status
            .and_then(|s| u16::try_from(s).ok())
            .and_then(|s| StatusCode::from_u16(s).ok())
        else {
            return ApiError::conflict(
                "the first request with this Idempotency-Key is still being decided",
            )
            .with_code("idempotency_request_in_flight")
            .into_response();
        };
        let mut response = Response::new(axum::body::Body::from(Bytes::from(
            self.response.unwrap_or_default(),
        )));
        *response.status_mut() = status;
        if let Some(ct) = self
            .content_type
            .and_then(|c| HeaderValue::from_str(&c).ok())
        {
            response.headers_mut().insert(header::CONTENT_TYPE, ct);
        }
        response
    }
}

async fn claim(pool: &PgPool, tenant: &str, key: &str, print: &str) -> Result<Claim, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM decision_idempotency WHERE created_at < now() - interval '1 day'")
        .execute(&mut *tx)
        .await?;
    let inserted = sqlx::query(
        "INSERT INTO decision_idempotency (tenant, key, fingerprint) VALUES ($1, $2, $3) \
         ON CONFLICT (tenant, key) DO NOTHING",
    )
    .bind(tenant)
    .bind(key)
    .bind(print)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let claim = if inserted == 1 {
        Claim::First
    } else {
        let (fingerprint, status, content_type, response): (
            String,
            Option<i16>,
            Option<String>,
            Option<Vec<u8>>,
        ) = sqlx::query_as(
            "SELECT fingerprint, status, content_type, response FROM decision_idempotency \
             WHERE tenant = $1 AND key = $2",
        )
        .bind(tenant)
        .bind(key)
        .fetch_one(&mut *tx)
        .await?;
        Claim::Seen(Seen {
            fingerprint,
            status,
            content_type,
            response,
        })
    };
    tx.commit().await?;
    Ok(claim)
}

async fn record(
    pool: &PgPool,
    tenant: &str,
    key: &str,
    status: StatusCode,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE decision_idempotency SET status = $3, content_type = $4, response = $5 \
         WHERE tenant = $1 AND key = $2",
    )
    .bind(tenant)
    .bind(key)
    .bind(i16::try_from(status.as_u16()).unwrap_or(i16::MAX))
    .bind(content_type)
    .bind(body)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn release(pool: &PgPool, tenant: &str, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM decision_idempotency WHERE tenant = $1 AND key = $2")
        .bind(tenant)
        .bind(key)
        .execute(pool)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One key cannot decide two entries or two bodies: the fingerprint
    /// binds the route and entry as well as the body.
    #[test]
    fn the_fingerprint_binds_scope_and_body() {
        let a = fingerprint("approve/1", b"{}");
        assert_eq!(a, fingerprint("approve/1", b"{}"));
        assert_ne!(a, fingerprint("approve/2", b"{}"));
        assert_ne!(a, fingerprint("reject/1", b"{}"));
        assert_ne!(a, fingerprint("approve/1", b"{\"bemerkung\":\"x\"}"));
    }

    /// A repeated header is refused, not read first-wins.
    #[test]
    fn a_repeated_key_is_refused() {
        let mut h = HeaderMap::new();
        h.append(HEADER, HeaderValue::from_static("a"));
        h.append(HEADER, HeaderValue::from_static("b"));
        assert!(key(&h).is_err());
        assert_eq!(key(&HeaderMap::new()).ok().flatten(), None);
    }
}
