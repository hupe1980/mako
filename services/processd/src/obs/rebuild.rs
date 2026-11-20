//! Rebuild `process_projections` from `marktd`'s event feed.
//!
//! The projection is a read model: every row is a function of the `de.mako.engine.*`
//! events `marktd` fanned out. `marktd` serves its outbox, in commit order, at
//! `GET /api/v1/events` — the same envelopes its fan-out POSTs to `/webhook`.
//! A rebuild pages that feed by its cursor and hands each envelope to
//! [`project`], the function the webhook calls, so a rebuilt row and a live
//! row are one function over one input.
//!
//! **Bounded by marktd's retention.** The feed reaches back
//! `[subscriptions] retention_days` on marktd; a process whose events are older
//! is not rebuilt. The report names the oldest event read, so an operator sees
//! how far back the rebuild reached.
//!
//! **Idempotent.** The projection upserts on `process_id` and its terminal-state
//! guard holds whatever order an event arrives in, so a rebuild over a populated
//! table repairs it rather than duplicating it, and a rebuild interrupted half
//! way is finished by running it again.
//!
//! `marktd` gates the feed behind its `read-events` action: it carries full
//! payloads (MaLo-IDs, partner identities). The `[marktd] api_key` processd
//! uses must therefore belong to a principal holding it.

use mako_service::http::{Upstream, UpstreamError};
use serde::Deserialize;

use crate::obs::error::ObsError;
use crate::obs::project::{ObsState, Projected, project};

/// The largest page the feed serves.
pub const PAGE_LIMIT: usize = 200;

/// One page of `GET /api/v1/events`.
#[derive(Debug, Deserialize)]
struct FeedPage {
    /// Each CloudEvent exactly as the fan-out delivers it.
    items: Vec<serde_json::Value>,
    next_cursor: Option<String>,
}

/// What a rebuild did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RebuildReport {
    /// Envelopes read from the feed.
    pub read: u64,
    /// Envelopes that wrote a projection row.
    pub projected: u64,
    /// Envelopes the read model does not carry (no process state).
    pub ignored: u64,
    /// The CloudEvent `time` of the oldest event read — how far back marktd's
    /// retention let the rebuild reach. `None` when the feed was empty.
    pub oldest_event: Option<String>,
}

/// Why a rebuild stopped.
#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    /// `marktd` did not serve the feed.
    #[error("event feed: {0}")]
    Upstream(#[from] UpstreamError),
    /// The projection store refused a write; the rebuild stops at that event so
    /// a re-run resumes it rather than skipping it.
    #[error("projection store: {0}")]
    Store(#[from] ObsError),
}

/// Replay every `de.mako.engine.*` event `marktd`'s feed holds into the projection,
/// oldest first, page by page until the feed names no next cursor.
///
/// # Errors
///
/// [`RebuildError`] — the rebuild stops at the first failure, and because the
/// projection is idempotent the remedy is to run it again.
pub async fn rebuild(
    state: &ObsState,
    marktd: &Upstream,
    page_limit: usize,
) -> Result<RebuildReport, RebuildError> {
    let limit = page_limit.clamp(1, PAGE_LIMIT).to_string();
    let mut cursor: Option<String> = None;
    let mut report = RebuildReport::default();
    loop {
        let mut query = vec![("type", "de.mako.engine.*"), ("limit", limit.as_str())];
        if let Some(c) = cursor.as_deref() {
            query.push(("cursor", c));
        }
        let req = marktd.get("/api/v1/events").query(&query);
        let Some(page) = marktd.json::<FeedPage>(req).await? else {
            break;
        };
        for envelope in &page.items {
            if report.oldest_event.is_none() {
                report.oldest_event = envelope["time"].as_str().map(str::to_owned);
            }
            report.read += 1;
            match project(state, envelope).await? {
                Projected::Upserted | Projected::Beendet { .. } => report.projected += 1,
                Projected::Ignored => report.ignored += 1,
            }
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    tracing::info!(
        read = report.read,
        projected = report.projected,
        ignored = report.ignored,
        oldest_event = report.oldest_event.as_deref().unwrap_or("-"),
        "processd obs: process_projections rebuilt from marktd's event feed"
    );
    Ok(report)
}
