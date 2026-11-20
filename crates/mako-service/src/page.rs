//! Keyset pagination: the one page shape every list answers with.
//!
//! A list takes `limit` (default [`DEFAULT_LIMIT`], at most [`MAX_LIMIT`]) and
//! `cursor`, and answers `{items, next_cursor, total_estimate?}`. The query
//! behind it is a keyset — `WHERE (sort key) > (cursor key) ORDER BY … LIMIT
//! limit + 1` on an index — never `OFFSET`, which reads and discards every
//! skipped row and drifts when rows arrive between two pages.
//!
//! Nothing is clamped and nothing is guessed: a `limit` above the cap is
//! refused with `limit_too_large`, and a cursor is bound to the filter it was
//! issued under, so one carried to another filter or another route is refused
//! with `cursor_invalid` instead of being read as a position in a different
//! list.
//!
//! ```rust,ignore
//! let limit = q.page.limit()?;
//! let after: Option<(OffsetDateTime, Uuid)> = q.page.after(&filter)?;
//! let rows = fetch(pool, &filter, after, limit + 1).await?;   // one more than the page
//! Ok(Json(Page::from_rows(rows, limit, &filter, |r| (r.created_at, r.id))))
//! ```

use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::ApiError;

/// Rows on a page when the caller names no `limit`.
pub const DEFAULT_LIMIT: u32 = 50;
/// The largest `limit` a list accepts.
pub const MAX_LIMIT: u32 = 200;
/// Above this a count is reported as capped rather than counted.
pub const COUNT_CAP: i64 = 10_000;

/// `limit` and `cursor`, as a list route reads them from its query string.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    schemars::JsonSchema,
    utoipa::IntoParams,
)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// Rows per page, 1–200; 50 when absent.
    #[serde(default)]
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page; absent for the first.
    #[serde(default)]
    pub cursor: Option<String>,
}

impl PageQuery {
    /// The requested page size.
    ///
    /// # Errors
    ///
    /// `400 limit_too_large` above [`MAX_LIMIT`], naming the cap;
    /// `400 limit_invalid` for `0`.
    pub fn limit(&self) -> Result<u32, ApiError> {
        match self.limit {
            None => Ok(DEFAULT_LIMIT),
            Some(0) => {
                Err(ApiError::bad_request("limit must be at least 1").with_code("limit_invalid"))
            }
            Some(n) if n > MAX_LIMIT => Err(ApiError::bad_request(format!(
                "limit {n} is above the maximum of {MAX_LIMIT}"
            ))
            .with_fields(serde_json::json!({ "code": "limit_too_large", "max": MAX_LIMIT }))),
            Some(n) => Ok(n),
        }
    }

    /// The sort key the page starts after, or `None` for the first page.
    ///
    /// `filter` is everything else that selects the list — the route and its
    /// filter parameters; the cursor must have been issued under the same.
    ///
    /// # Errors
    ///
    /// `400 cursor_invalid` when the cursor does not decode, was issued under
    /// another filter, or carries a key of another shape.
    pub fn after<K: DeserializeOwned>(
        &self,
        filter: &impl Serialize,
    ) -> Result<Option<K>, ApiError> {
        let Some(raw) = self.cursor.as_deref() else {
            return Ok(None);
        };
        let invalid = |why: &str| {
            ApiError::bad_request(format!("cursor is not valid here: {why}"))
                .with_code("cursor_invalid")
        };
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| invalid("it does not decode"))?;
        let cursor: Cursor =
            serde_json::from_slice(&bytes).map_err(|_| invalid("it does not decode"))?;
        if cursor.f != digest(filter) {
            return Err(invalid("it was issued for another list or filter"));
        }
        serde_json::from_value(cursor.k)
            .map(Some)
            .map_err(|_| invalid("its position has another shape"))
    }
}

/// What a cursor carries: the last row's sort key and the filter digest.
#[derive(Serialize, Deserialize)]
struct Cursor {
    k: serde_json::Value,
    f: String,
}

/// The opaque cursor positioned after `key` under `filter`.
#[must_use]
pub fn encode_cursor(key: &impl Serialize, filter: &impl Serialize) -> String {
    let cursor = Cursor {
        k: serde_json::to_value(key).unwrap_or(serde_json::Value::Null),
        f: digest(filter),
    };
    let bytes = serde_json::to_vec(&cursor).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The first 16 hex digits of SHA-256 over the filter's JSON.
fn digest(filter: &impl Serialize) -> String {
    let json = serde_json::to_vec(filter).unwrap_or_default();
    let hash = sha2::Sha256::digest(&json);
    hash.iter()
        .take(8)
        .fold(String::with_capacity(16), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// A count bounded at [`COUNT_CAP`]: a list over a million rows reports
/// `{count: 10000, capped: true}` rather than counting them.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    utoipa::ToSchema,
)]
pub struct TotalEstimate {
    /// The rows counted, at most 10 000.
    pub count: i64,
    /// `true` when there are more rows than `count`.
    pub capped: bool,
}

/// `n` rows counted by a query limited to `COUNT_CAP + 1`, as an estimate.
///
/// Count with `SELECT count(*) FROM (SELECT 1 FROM … LIMIT 10001) t`, so the
/// database stops at the cap.
#[must_use]
pub const fn capped_count(n: i64) -> TotalEstimate {
    if n > COUNT_CAP {
        TotalEstimate {
            count: COUNT_CAP,
            capped: true,
        }
    } else {
        TotalEstimate {
            count: n,
            capped: false,
        }
    }
}

/// One page of a list.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, utoipa::ToSchema,
)]
pub struct Page<T> {
    /// The rows of this page, in the list's order.
    pub items: Vec<T>,
    /// The cursor of the next page; absent on the last.
    pub next_cursor: Option<String>,
    /// How many rows the whole list holds, where the list counts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_estimate: Option<TotalEstimate>,
}

impl<T> Page<T> {
    /// The page from `rows` fetched with `LIMIT limit + 1`: a row beyond
    /// `limit` means there is a next page, positioned after the last row kept.
    #[must_use]
    pub fn from_rows<K: Serialize>(
        mut rows: Vec<T>,
        limit: u32,
        filter: &impl Serialize,
        key: impl Fn(&T) -> K,
    ) -> Self {
        let limit = limit as usize;
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|last| encode_cursor(&key(last), filter))
        } else {
            None
        };
        Self {
            items: rows,
            next_cursor,
            total_estimate: None,
        }
    }

    /// This page with `total_estimate` set.
    #[must_use]
    pub fn with_total(mut self, total: TotalEstimate) -> Self {
        self.total_estimate = Some(total);
        self
    }

    /// The rows mapped, the cursor and estimate kept.
    #[must_use]
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Page<U> {
        Page {
            items: self.items.into_iter().map(f).collect(),
            next_cursor: self.next_cursor,
            total_estimate: self.total_estimate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(limit: Option<u32>, cursor: Option<String>) -> PageQuery {
        PageQuery { limit, cursor }
    }

    #[test]
    fn a_limit_above_the_cap_is_refused() {
        assert_eq!(q(None, None).limit().unwrap(), 50);
        assert_eq!(q(Some(200), None).limit().unwrap(), 200);
        let refused = q(Some(201), None).limit().unwrap_err();
        assert_eq!(refused.code(), Some("limit_too_large"));
        assert_eq!(refused.status(), axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            q(Some(0), None).limit().unwrap_err().code(),
            Some("limit_invalid")
        );
    }

    #[test]
    fn a_cursor_from_another_filter_is_refused() {
        let rows: Vec<i64> = (1..=6).collect();
        let page = Page::from_rows(rows, 5, &("events", "de.mako.vertrag.*"), |r| *r);
        assert_eq!(page.items, [1, 2, 3, 4, 5]);
        let cursor = page.next_cursor.expect("a sixth row means a next page");
        let same = q(None, Some(cursor.clone()));
        assert_eq!(
            same.after::<i64>(&("events", "de.mako.vertrag.*")).unwrap(),
            Some(5)
        );
        let other = same
            .after::<i64>(&("events", "de.mako.markt.*"))
            .unwrap_err();
        assert_eq!(other.code(), Some("cursor_invalid"));
        let shape = same
            .after::<(String, String)>(&("events", "de.mako.vertrag.*"))
            .unwrap_err();
        assert_eq!(shape.code(), Some("cursor_invalid"));
        let garbage = q(None, Some("%%%".into())).after::<i64>(&()).unwrap_err();
        assert_eq!(garbage.code(), Some("cursor_invalid"));
    }

    #[test]
    fn the_last_page_has_no_cursor() {
        let page = Page::from_rows(vec![1, 2, 3], 5, &(), |r| *r);
        assert_eq!(page.items, [1, 2, 3]);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn the_count_is_capped_at_ten_thousand() {
        assert_eq!(
            capped_count(9_999),
            TotalEstimate {
                count: 9_999,
                capped: false
            }
        );
        assert_eq!(
            capped_count(10_001),
            TotalEstimate {
                count: 10_000,
                capped: true
            }
        );
        let page = Page::from_rows(vec![1], 5, &(), |r| *r).with_total(capped_count(1_000_000));
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(
            json["total_estimate"],
            serde_json::json!({"count": 10000, "capped": true})
        );
    }
}
