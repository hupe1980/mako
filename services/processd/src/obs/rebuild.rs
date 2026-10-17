//! Rebuild `process_projections` from `marktd`'s CloudEvent replay log.
//!
//! The projection is a read model: every row is a function of the `de.mako.*`
//! events `marktd` fanned out. `marktd` keeps every envelope it fans out in
//! `event_log` and serves it, oldest first, at `GET /admin/events` — the same
//! bytes its fan-out POSTs to `/webhook`. A rebuild pages that log and hands
//! each envelope to [`project`], the function the webhook calls, so a rebuilt
//! row and a live row are one function over one input.
//!
//! **Idempotent.** The projection upserts on `process_id` and its terminal-state
//! guard holds whatever order an event arrives in, so a rebuild over a populated
//! table repairs it rather than duplicating it, and a rebuild interrupted half
//! way is finished by running it again.
//!
//! `marktd` gates `GET /admin/events` behind its `manage-fanout` action: the log
//! carries full payloads (MaLo-IDs, partner identities). The `[marktd] api_key`
//! processd uses must therefore belong to a principal holding it.

use mako_service::http::{Upstream, UpstreamError};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::obs::error::ObsError;
use crate::obs::project::{ObsState, Projected, project};

/// The largest page `marktd` serves (`limit` is clamped to 5000 there).
pub const PAGE_LIMIT: usize = 5000;

/// One row of `GET /admin/events`.
#[derive(Debug, Deserialize)]
struct EventLogRow {
    event_id: String,
    /// The CloudEvent exactly as the fan-out delivers it.
    envelope: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    received_at: OffsetDateTime,
}

/// What a rebuild did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RebuildReport {
    /// Envelopes read from the replay log.
    pub read: u64,
    /// Envelopes that wrote a projection row.
    pub projected: u64,
    /// Envelopes the read model does not carry (another namespace, no state).
    pub ignored: u64,
}

/// Why a rebuild stopped.
#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    /// `marktd` did not serve the replay log.
    #[error("replay log: {0}")]
    Upstream(#[from] UpstreamError),
    /// The projection store refused a write; the rebuild stops at that event so
    /// a re-run resumes it rather than skipping it.
    #[error("projection store: {0}")]
    Store(#[from] ObsError),
    /// At least a page of events share one `received_at` instant: paging by
    /// that instant cannot advance, and continuing would loop.
    #[error(
        "the replay log holds at least a page of events received at {at}; it cannot be \
         paged by received_at"
    )]
    Stalled { at: String },
}

/// Replay every `de.mako.*` event `marktd` holds into the projection.
///
/// Pages `GET /admin/events` from the beginning of the log up to the instant
/// the rebuild started, in `received_at` order. The window's lower bound is
/// inclusive, so each page after the first re-reads the events of its first
/// instant; those already seen are skipped by `event_id`.
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
    let page_limit = page_limit.clamp(1, PAGE_LIMIT);
    let until = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let mut from = OffsetDateTime::UNIX_EPOCH;
    // The event ids already applied at `from` — the overlap the inclusive
    // lower bound re-serves on the next page.
    let mut seen_at_from: Vec<String> = Vec::new();
    let mut report = RebuildReport::default();

    loop {
        let from_s = from.format(&Rfc3339).unwrap_or_default();
        let limit = page_limit.to_string();
        let req = marktd.get("/admin/events").query(&[
            ("from", from_s.as_str()),
            ("to", until.as_str()),
            ("limit", limit.as_str()),
        ]);
        let page: Vec<EventLogRow> = marktd.json(req).await?.unwrap_or_default();
        let full = page.len() >= page_limit;

        let mut fresh = 0usize;
        let mut last_at = from;
        let mut seen_at_last: Vec<String> = Vec::new();
        for row in page {
            if row.received_at == from && seen_at_from.contains(&row.event_id) {
                continue;
            }
            fresh += 1;
            report.read += 1;
            match project(state, &row.envelope).await? {
                Projected::Upserted => report.projected += 1,
                Projected::Ignored => report.ignored += 1,
            }
            if row.received_at != last_at {
                last_at = row.received_at;
                seen_at_last.clear();
            }
            seen_at_last.push(row.event_id);
        }

        if !full {
            break;
        }
        // A full page that brought nothing new is the overlap alone: at least
        // a page of events share the instant `from`, and paging by arrival
        // time cannot get past them.
        if fresh == 0 {
            return Err(RebuildError::Stalled { at: from_s });
        }
        if last_at == from {
            seen_at_from.extend(seen_at_last);
        } else {
            from = last_at;
            seen_at_from = seen_at_last;
        }
    }

    tracing::info!(
        read = report.read,
        projected = report.projected,
        ignored = report.ignored,
        "processd obs: process_projections rebuilt from marktd's replay log"
    );
    Ok(report)
}
