//! Delivery surveillance — noticing the measuring points that stopped.
//!
//! # Why this exists
//!
//! Every other quality mechanism in `edmd` judges **data that arrived**. The
//! V-rules run on an ingest batch; the Hampel scorer grades one; the § 60 Abs. 2
//! confirmation loop chases estimates that were already written. All of them are
//! triggered by a delivery.
//!
//! Silence triggers nothing. A head-end that breaks, a gateway that loses its
//! WAN, a Kafka producer that is redeployed with the wrong topic — none of these
//! produce an ingest, so none produce a validation, a grade, or an event. The
//! measuring point simply stops appearing, and nothing in the service is looking
//! for an absence.
//!
//! That failure surfaces at settlement: the Summenzeitreihe is short, the
//! Bilanzkreis carries the difference, and the Mehr-/Mindermengensaldo lands on
//! someone. By then the window in which the values could still have been re-read
//! or substituted under § 60 Abs. 2 MsbG has usually closed.
//!
//! So this worker asks the complementary question — *which points have not
//! delivered?* — and answers it on a cadence short enough to act on.
//!
//! # What it reports
//!
//! Two conditions — see `DeliveryState` — because "stopped" and "degraded"
//! need different responses:
//!
//! | Condition | Meaning | Typical cause |
//! |---|---|---|
//! | `SILENT` | Newest interval ends more than `silent_after_hours` ago | Gateway offline, head-end down, routing broken |
//! | `UNDER_COVERED` | Still delivering, but under `min_coverage_pct` of the window | Partial batches, dropped intervals, wrong resolution |
//!
//! A point that has *never* delivered is deliberately **not** reported: edmd
//! cannot distinguish "meter installed and broken" from "MaLo in master data
//! with no meter yet", and guessing produces an alert per unbuilt connection.
//! Commissioning coverage is `marktd`'s question, and
//! `GET /api/v1/sharing/readiness` already answers the §42c form of it.
//!
//! # Deduplication
//!
//! Like the §14a compliance register, this is a register of what is wrong now,
//! not a log of every time we looked. `delivery_surveillance` holds one row per
//! `(tenant, malo_id)` and events fire on the transitions — overdue, then
//! resumed. A point that stays dark for a month is announced once, not thirty
//! times.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Serialize;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::config::SurveillanceConfig;
use crate::store::MeterStoreTimeSeriesRepository;

/// Why a measuring point is being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryState {
    /// Nothing has arrived for longer than the silence threshold.
    Silent,
    /// Still delivering, but too little of the window to settle on.
    UnderCovered,
}

impl DeliveryState {
    /// The DB spelling, pinned by the `delivery_surveillance.state` CHECK.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Silent => "SILENT",
            Self::UnderCovered => "UNDER_COVERED",
        }
    }
}

/// One measuring point that is not delivering as expected.
#[derive(Debug, Clone, Serialize)]
pub struct DeliveryFinding {
    pub malo_id: String,
    pub state: DeliveryState,
    /// End of the newest interval seen in the window.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_interval_end: Option<OffsetDateTime>,
    /// Hours between that instant and the sweep.
    pub hours_silent: i64,
    /// Share of the window actually covered by intervals, 0–100.
    pub coverage_pct: f64,
    /// Intervals seen in the window.
    pub interval_count: i64,
}

/// What one sweep found.
#[derive(Debug, Clone, Serialize)]
pub struct SurveillanceReport {
    #[serde(with = "time::serde::rfc3339")]
    pub scanned_at: OffsetDateTime,
    /// Start of the coverage window.
    #[serde(with = "time::serde::rfc3339")]
    pub window_from: OffsetDateTime,
    /// Measuring points that delivered anything in the window.
    pub points_scanned: usize,
    /// Points currently in a reported state.
    pub findings: Vec<DeliveryFinding>,
    /// Findings that newly entered a reported state — the ones that emitted.
    pub newly_overdue: usize,
    /// Points that were overdue and are delivering again.
    pub resumed: usize,
    /// Findings suppressed by `max_events_per_sweep`. Reported rather than
    /// dropped silently: a capped list that looks complete is how a fleet-wide
    /// outage gets mistaken for a handful of broken meters.
    pub suppressed: usize,
}

/// Assess delivery for every measuring point that has data in the window.
///
/// One cross-MaLo aggregate over meterstore's version-resolved relation, which
/// spans both tiers — a point whose recent history has already settled into the
/// cold tier is still visible. Only billable qualities count: a window full of
/// `FAULTY` intervals is not a delivered window, and reporting it as covered
/// would hide exactly the case § 60 Abs. 2 MsbG exists for.
///
/// # Errors
///
/// Propagates the store's error when the scan cannot run.
pub async fn assess_delivery(
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    cfg: &SurveillanceConfig,
    now: OffsetDateTime,
) -> anyhow::Result<(Vec<DeliveryFinding>, usize, OffsetDateTime)> {
    let window_from = now - time::Duration::days(cfg.coverage_window_days);
    let store = repo.store();

    // `from`/`to` are SQL reserved words, hence quoted. The tenant and the bounds
    // travel as bound parameters so no value reaches the SQL text.
    //
    // The grouping is per **register**, not per MaLo. Coverage is a duration
    // ratio, and a measuring point reporting three registers spans the window
    // three times over: grouped by `malo_id` alone the ratio ran to 300 %, the
    // clamp to 100 % swallowed it, and no multi-register point could ever fall
    // below `min_coverage_pct`. Under-coverage was undetectable for exactly the
    // prosumer and dual-tariff meters most likely to deliver partially.
    let sql = format!(
        r#"SELECT "malo_id",
                  "obis_code",
                  MAX("to")   AS last_interval_end,
                  COUNT(*)    AS interval_count,
                  SUM(EXTRACT(EPOCH FROM ("to" - "from"))) AS covered_seconds
             FROM "{table}"
            WHERE "tenant" = $1
              AND "from" >= $2
              AND "quality" NOT IN ('FAULTY', 'UNKNOWN')
            GROUP BY "malo_id", "obis_code""#,
        table = store.resolved_table(),
    );

    let rows = store
        .query_with_params(
            &sql,
            vec![
                datafusion::scalar::ScalarValue::Utf8(Some(tenant.to_owned())),
                crate::server::ts_param(window_from),
            ],
        )
        .await
        .map_err(|e| anyhow::anyhow!("delivery surveillance scan: {e}"))?
        .to_json()
        .map_err(|e| anyhow::anyhow!("delivery surveillance decode: {e}"))?;

    let window_seconds = (now - window_from).whole_seconds().max(1) as f64;

    // Fold the per-register rows back onto the measuring point.
    //
    // Coverage is the **best-covered** register, not the sum and not the worst.
    // The question a sweep asks is whether the point is still delivering, and a
    // point whose Lastgang is complete is delivering even if some secondary
    // register reports monthly — taking the worst would raise a finding for
    // every one of them.
    struct Point {
        last_interval_end: OffsetDateTime,
        interval_count: i64,
        coverage_pct: f64,
    }
    let mut points: BTreeMap<String, Point> = BTreeMap::new();
    for row in &rows {
        let Some(malo_id) = row.get("malo_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let last_interval_end = row
            .get("last_interval_end")
            .and_then(json_timestamp)
            .unwrap_or(window_from);
        let interval_count = row
            .get("interval_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let covered_seconds = row
            .get("covered_seconds")
            .and_then(json_number)
            .unwrap_or(0.0);
        // Coverage is the share of the window actually spanned by one register's
        // intervals, so it is resolution-independent: a point that switched from
        // quarter-hours to hours has fewer intervals but the same coverage, and
        // is not a finding. An interval count alone would report every such point.
        let coverage_pct = (covered_seconds / window_seconds * 100.0).clamp(0.0, 100.0);
        points
            .entry(malo_id.to_owned())
            .and_modify(|p| {
                p.last_interval_end = p.last_interval_end.max(last_interval_end);
                p.interval_count += interval_count;
                p.coverage_pct = p.coverage_pct.max(coverage_pct);
            })
            .or_insert(Point {
                last_interval_end,
                interval_count,
                coverage_pct,
            });
    }

    let points_scanned = points.len();
    let mut findings = Vec::new();

    for (malo_id, p) in points {
        let hours_silent = (now - p.last_interval_end).whole_hours();
        let state = if hours_silent >= cfg.silent_after_hours {
            Some(DeliveryState::Silent)
        } else if p.coverage_pct < cfg.min_coverage_pct {
            Some(DeliveryState::UnderCovered)
        } else {
            None
        };
        let Some(state) = state else { continue };

        findings.push(DeliveryFinding {
            malo_id,
            state,
            last_interval_end: Some(p.last_interval_end),
            hours_silent,
            coverage_pct: p.coverage_pct,
            interval_count: p.interval_count,
        });
    }

    // Worst first: a silent point outranks an under-covered one, and among
    // equals the one that has been dark longest.
    findings.sort_by(|a, b| {
        (a.state != DeliveryState::Silent)
            .cmp(&(b.state != DeliveryState::Silent))
            .then(b.hours_silent.cmp(&a.hours_silent))
    });

    Ok((findings, points_scanned, window_from))
}

/// A DataFusion timestamp rendered into JSON, back as an instant.
fn json_timestamp(v: &serde_json::Value) -> Option<OffsetDateTime> {
    let s = v.as_str()?;
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .or_else(|| {
            // DataFusion renders a timestamp without a zone designator; it is UTC.
            let fmt = time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second][optional [.[subsecond]]]"
            );
            time::PrimitiveDateTime::parse(s, fmt)
                .ok()
                .map(time::PrimitiveDateTime::assume_utc)
        })
}

/// A JSON number that may have been rendered as a string (DataFusion renders
/// decimal aggregates that way).
fn json_number(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| {
        v.as_str()
            .and_then(|s| s.parse::<Decimal>().ok())
            .and_then(|d| {
                use rust_decimal::prelude::ToPrimitive as _;
                d.to_f64()
            })
    })
}

/// Run one surveillance sweep: assess, reconcile against the register, emit on
/// the transitions.
pub async fn run_surveillance_sweep(
    repo: &MeterStoreTimeSeriesRepository,
    cfg: &SurveillanceConfig,
    tenant: &str,
    erp_webhook_url: Option<&str>,
    erp_webhook_secret: Option<&str>,
) -> SurveillanceReport {
    let scanned_at = OffsetDateTime::now_utc();
    let pool = repo.pool();
    let client = mako_service::http::default_client();

    let (findings, points_scanned, window_from) =
        match assess_delivery(repo, tenant, cfg, scanned_at).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, tenant, "edmd: surveillance: scan failed");
                return SurveillanceReport {
                    scanned_at,
                    window_from: scanned_at,
                    points_scanned: 0,
                    findings: Vec::new(),
                    newly_overdue: 0,
                    resumed: 0,
                    suppressed: 0,
                };
            }
        };

    let mut newly_overdue = 0usize;
    let mut emitted = 0usize;
    let mut suppressed = 0usize;

    for finding in &findings {
        // Open or re-sight. `first_detected_at = last_seen_at` is true exactly
        // on the two transitions worth an event: opened now, or reopened now.
        let transition: Option<bool> = match sqlx::query_scalar(
            r"INSERT INTO delivery_surveillance
                  (tenant, malo_id, stream, obis_code, subscription_ref, state,
                   last_interval_end, hours_silent, coverage_pct, interval_count)
              VALUES ($1,$2,'TYP1','','',$3,$4,$5,$6,$7)
              -- TYP1 has no subscription and no register, so both key columns
              -- stay empty — but they are still part of the key and have to be
              -- named here, or the upsert matches no unique constraint.
              ON CONFLICT (tenant, stream, malo_id, obis_code, subscription_ref) DO UPDATE SET
                  last_seen_at      = now(),
                  state             = EXCLUDED.state,
                  last_interval_end = EXCLUDED.last_interval_end,
                  hours_silent      = EXCLUDED.hours_silent,
                  coverage_pct      = EXCLUDED.coverage_pct,
                  interval_count    = EXCLUDED.interval_count,
                  first_detected_at = CASE
                      WHEN delivery_surveillance.resolved_at IS NOT NULL THEN now()
                      ELSE delivery_surveillance.first_detected_at END,
                  resolved_at       = NULL
              RETURNING first_detected_at = last_seen_at",
        )
        .bind(tenant)
        .bind(&finding.malo_id)
        .bind(finding.state.as_str())
        .bind(finding.last_interval_end)
        .bind(finding.hours_silent)
        .bind(Decimal::try_from(finding.coverage_pct).unwrap_or_default())
        .bind(finding.interval_count)
        .fetch_one(pool)
        .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, malo_id = %finding.malo_id,
                    "edmd: surveillance: register write failed");
                continue;
            }
        };

        if transition != Some(true) {
            continue;
        }
        newly_overdue += 1;

        // One broken head-end can take a whole fleet dark at once. The cap keeps
        // that from becoming a hundred thousand CloudEvents; the register still
        // holds every row, and `suppressed` says how many are not on the wire.
        if emitted >= cfg.max_events_per_sweep {
            suppressed += 1;
            continue;
        }

        tracing::warn!(
            malo_id = %finding.malo_id,
            state = finding.state.as_str(),
            hours_silent = finding.hours_silent,
            coverage_pct = format!("{:.1}", finding.coverage_pct),
            "edmd: surveillance: measuring point is not delivering (§ 60 Abs. 2 MsbG)"
        );

        if let Some(url) = erp_webhook_url {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("edmd", tenant),
                mako_events::messwert::READING_DELIVERY_OVERDUE,
                finding.malo_id.clone(),
                serde_json::json!({
                    "malo_id":           finding.malo_id,
                    "state":             finding.state.as_str(),
                    "last_interval_end": finding.last_interval_end.map(|t| t.to_string()),
                    "hours_silent":      finding.hours_silent,
                    "coverage_pct":      finding.coverage_pct,
                    "interval_count":    finding.interval_count,
                    "window_from":       window_from.to_string(),
                    "legal_basis":       "§ 60 Abs. 2 MsbG (Plausibilisierung und Ersatzwertbildung)",
                    "recommended_action":
                        "Check the delivery path, then substitute the gap via \
                         POST /api/v1/meter-reads/{malo_id}/substitute if the values \
                         cannot be recovered",
                }),
            )
            .extension("tenantid", tenant)
            .extension("worker", "delivery-surveillance");
            if let Err(e) = mako_service::post_ce_with_retry(
                &client,
                url,
                &ce,
                erp_webhook_secret.map(str::as_bytes),
            )
            .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
        emitted += 1;
    }

    // Close what this sweep no longer finds. The scan covered every point with
    // data in the window, so anything still open and not re-sighted is
    // delivering again.
    let reported: Vec<String> = findings.iter().map(|f| f.malo_id.clone()).collect();
    let resumed_rows = sqlx::query(
        r"UPDATE delivery_surveillance
             SET resolved_at = now()
           WHERE tenant = $1 AND stream = 'TYP1' AND resolved_at IS NULL
             AND NOT (malo_id = ANY($2))
       RETURNING malo_id, state, first_detected_at",
    )
    .bind(tenant)
    .bind(&reported)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for row in &resumed_rows {
        use sqlx::Row as _;
        let malo_id: String = row.get("malo_id");
        tracing::info!(%malo_id, "edmd: surveillance: measuring point is delivering again");
        if let Some(url) = erp_webhook_url {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("edmd", tenant),
                mako_events::messwert::READING_DELIVERY_RESUMED,
                malo_id.clone(),
                serde_json::json!({
                    "malo_id":     malo_id,
                    "was_state":   row.get::<String, _>("state"),
                    "overdue_since": row
                        .get::<OffsetDateTime, _>("first_detected_at")
                        .to_string(),
                    "resumed_at":  scanned_at.to_string(),
                }),
            )
            .extension("tenantid", tenant)
            .extension("worker", "delivery-surveillance");
            if let Err(e) = mako_service::post_ce_with_retry(
                &client,
                url,
                &ce,
                erp_webhook_secret.map(str::as_bytes),
            )
            .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
    }

    if suppressed > 0 {
        tracing::warn!(
            suppressed,
            cap = cfg.max_events_per_sweep,
            "edmd: surveillance: event cap reached — findings recorded but not emitted"
        );
    }

    SurveillanceReport {
        scanned_at,
        window_from,
        points_scanned,
        findings,
        newly_overdue,
        resumed: resumed_rows.len(),
        suppressed,
    }
}

/// Spawn the delivery-surveillance worker. Runs until `shutdown` is cancelled.
#[allow(clippy::too_many_arguments)] // one argument per collaborator
pub fn spawn_surveillance_worker(
    repo: MeterStoreTimeSeriesRepository,
    typ2: Option<crate::store::MeterStoreTyp2Repository>,
    cfg: SurveillanceConfig,
    tenant: String,
    // Resolves a subscription's Messprodukt so the Typ-2 sweep can use the
    // cadence that product publishes rather than one flat setting.
    marktd: Option<mako_markt::marktd_client::MarktdClient>,
    erp_webhook_url: Option<String>,
    erp_webhook_secret: Option<String>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        // Let the store settle before the first cross-MaLo scan.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;

        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(cfg.sweep_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = shutdown.cancelled() => {
                    tracing::info!("edmd: surveillance: shutdown requested");
                    break;
                }
            }
            run_surveillance_sweep(
                &repo,
                &cfg,
                &tenant,
                erp_webhook_url.as_deref(),
                erp_webhook_secret.as_deref(),
            )
            .await;

            // The ESA Typ-2 stream rides the same cadence but its own
            // thresholds and register rows — the two never mix.
            if cfg.typ2_enabled
                && let Some(t) = typ2.as_ref()
            {
                run_typ2_surveillance_sweep(
                    t,
                    repo.pool(),
                    &cfg,
                    &tenant,
                    marktd.as_ref(),
                    erp_webhook_url.as_deref(),
                    erp_webhook_secret.as_deref(),
                )
                .await;
            }
        }
    });
}

/// The open findings, grouped by state — the shape the REST endpoint returns.
#[must_use]
pub fn group_by_state(findings: &[DeliveryFinding]) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    for f in findings {
        *out.entry(f.state.as_str()).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_outranks_under_coverage_and_the_longest_dark_comes_first() {
        let f = |malo: &str, state, hours| DeliveryFinding {
            malo_id: malo.to_owned(),
            state,
            last_interval_end: None,
            hours_silent: hours,
            coverage_pct: 0.0,
            interval_count: 0,
        };
        let mut findings = [
            f("51238696012", DeliveryState::UnderCovered, 2),
            f("51238696781", DeliveryState::Silent, 40),
            f("51238696782", DeliveryState::Silent, 400),
        ];
        findings.sort_by(|a, b| {
            (a.state != DeliveryState::Silent)
                .cmp(&(b.state != DeliveryState::Silent))
                .then(b.hours_silent.cmp(&a.hours_silent))
        });
        assert_eq!(findings[0].malo_id, "51238696782", "darkest first");
        assert_eq!(findings[1].malo_id, "51238696781");
        assert_eq!(
            findings[2].state,
            DeliveryState::UnderCovered,
            "a degraded point ranks below a silent one"
        );
    }

    /// The state vocabulary and the column CHECK must agree.
    #[test]
    fn state_strings_match_the_schema_check() {
        assert_eq!(DeliveryState::Silent.as_str(), "SILENT");
        assert_eq!(DeliveryState::UnderCovered.as_str(), "UNDER_COVERED");
    }

    #[test]
    fn datafusion_timestamps_parse_with_or_without_a_zone() {
        let with_zone = serde_json::json!("2026-07-01T10:00:00Z");
        let without = serde_json::json!("2026-07-01T10:00:00");
        assert_eq!(
            json_timestamp(&with_zone),
            json_timestamp(&without),
            "a rendered timestamp without a designator is UTC, not unparseable"
        );
        assert!(json_timestamp(&with_zone).is_some());
    }

    #[test]
    fn decimal_aggregates_render_as_strings_and_still_parse() {
        assert_eq!(json_number(&serde_json::json!("604800.0")), Some(604_800.0));
        assert_eq!(json_number(&serde_json::json!(604_800.0)), Some(604_800.0));
        assert_eq!(json_number(&serde_json::json!(null)), None);
    }
}

// ── REST surface ──────────────────────────────────────────────────────────────

/// `GET /api/v1/surveillance/delivery`
///
/// The measuring points that are not delivering, from the register — the
/// standing answer, not a fresh scan. Cheap enough for a dashboard to poll.
///
/// `?state=SILENT|UNDER_COVERED` narrows it; `?include_resolved=true` adds the
/// points that recovered, so an operator can see a flapping delivery path.
pub async fn get_delivery_surveillance(
    claims: mako_service::oidc::Claims,
    axum::Extension(enforcer): axum::Extension<std::sync::Arc<mako_service::cedar::CedarEnforcer>>,
    axum::extract::State(state): axum::extract::State<crate::handler::HandlerState>,
    axum::extract::Query(params): axum::extract::Query<SurveillanceQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    use sqlx::Row as _;

    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-timeseries",
        state.tenant.as_str(),
    ) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if let Some(s) = params.state.as_deref()
        && !["SILENT", "UNDER_COVERED"].contains(&s.to_uppercase().as_str())
    {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": format!("unknown state {s:?}"),
                "expected": ["SILENT", "UNDER_COVERED"],
            })),
        )
            .into_response();
    }

    let limit = params.limit.unwrap_or(500).clamp(1, 5_000);
    let rows = sqlx::query(
        r"SELECT malo_id, state, last_interval_end, hours_silent, coverage_pct,
                 interval_count, first_detected_at, last_seen_at, resolved_at
          FROM delivery_surveillance
          WHERE tenant = $1
            AND ($2::bool OR resolved_at IS NULL)
            AND ($3::text IS NULL OR state = $3)
          ORDER BY resolved_at NULLS FIRST, hours_silent DESC
          LIMIT $4",
    )
    .bind(&state.tenant)
    .bind(params.include_resolved.unwrap_or(false))
    .bind(params.state.as_ref().map(|s| s.to_uppercase()))
    .bind(limit)
    .fetch_all(state.repo.pool())
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    let ts = |c: &str| {
                        r.try_get::<Option<OffsetDateTime>, _>(c)
                            .ok()
                            .flatten()
                            .map(|t| t.to_string())
                    };
                    serde_json::json!({
                        "malo_id":           r.try_get::<String, _>("malo_id").unwrap_or_default(),
                        "state":             r.try_get::<String, _>("state").unwrap_or_default(),
                        "last_interval_end": ts("last_interval_end"),
                        "hours_silent":      r.try_get::<i64, _>("hours_silent").unwrap_or(0),
                        "coverage_pct":      r
                            .try_get::<Option<Decimal>, _>("coverage_pct")
                            .ok()
                            .flatten()
                            .map(|d| d.to_string()),
                        "interval_count":    r.try_get::<i64, _>("interval_count").unwrap_or(0),
                        "first_detected_at": ts("first_detected_at"),
                        "last_seen_at":      ts("last_seen_at"),
                        "resolved_at":       ts("resolved_at"),
                    })
                })
                .collect();
            axum::Json(serde_json::json!({
                "count":     items.len(),
                "truncated": i64::try_from(items.len()).unwrap_or(i64::MAX) >= limit,
                "points":    items,
                "legal_basis":
                    "§ 60 Abs. 2 MsbG — a measuring point that stops delivering leaves \
                     Plausibilisierung und Ersatzwertbildung owing",
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: surveillance query failed");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Query parameters for `GET /api/v1/surveillance/delivery`.
#[derive(Debug, serde::Deserialize)]
pub struct SurveillanceQuery {
    /// `SILENT` or `UNDER_COVERED`. Defaults to both.
    pub state: Option<String>,
    /// Include points that have recovered. Default: open only.
    pub include_resolved: Option<bool>,
    /// Max rows (default 500, hard cap 5000).
    pub limit: Option<i64>,
}

/// `POST /api/v1/surveillance/delivery/scan`
///
/// Run a sweep now: reconcile the register and emit the transitions. The daily
/// worker calls the same function.
pub async fn post_delivery_surveillance_scan(
    claims: mako_service::oidc::Claims,
    axum::Extension(enforcer): axum::Extension<std::sync::Arc<mako_service::cedar::CedarEnforcer>>,
    axum::extract::State(state): axum::extract::State<crate::handler::HandlerState>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    // A sweep writes the register and emits CloudEvents, so it is a write.
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-quality-rescore",
        state.tenant.as_str(),
    ) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let report = run_surveillance_sweep(
        &state.repo,
        &state.surveillance,
        &state.tenant,
        state.erp_webhook_url.as_deref(),
        state.erp_webhook_secret.as_ref().map(|s| {
            use secrecy::ExposeSecret as _;
            s.expose_secret()
        }),
    )
    .await;

    axum::Json(serde_json::json!({
        "scanned_at":     report.scanned_at.to_string(),
        "window_from":    report.window_from.to_string(),
        "points_scanned": report.points_scanned,
        "open":           report.findings.len(),
        "by_state":       group_by_state(&report.findings),
        "newly_overdue":  report.newly_overdue,
        "resumed":        report.resumed,
        "suppressed":     report.suppressed,
        "findings":       report.findings,
    }))
    .into_response()
}

// ── ESA "Werte nach Typ 2" surveillance ──────────────────────────────────────

/// Assess Typ-2 delivery per **(Meldepunkt, delivered register)**.
///
/// The same question as [`assess_delivery`], asked of the other store. It is a
/// separate sweep rather than a parameter because the two streams answer to
/// different regimes: a Typ-1 gap ends in a short Summenzeitreihe and a
/// Mehr-/Mindermengensaldo, while a Typ-2 gap breaches the §60 Abs. 1 MsbG
/// delivery duty toward one ESA and reaches no billing run that could come up
/// short. Nothing else in the platform would ever notice it.
///
/// The key is **(Meldepunkt, subscription, register)**. A MSCONS 13027 carries
/// `SG9 PIA+5 … :SRW` (the OBIS) per line item and names its subscription in
/// `SG1 RFF+AGI` (MSCONS AHB 3.2 §11.2 hint `[574]`) — `esa_typ2_reads` records
/// both, so a finding can say which subscription stopped rather than leaving an
/// operator to work it out from the registers.
///
/// Per-register stays part of the key because it is the finer signal: a
/// subscription whose Erzeugung register goes quiet while Verbrauch keeps
/// arriving is broken, and a subscription-only key would call it healthy.
/// `bestellung_ref` is `NULL` for a delivery whose sender omitted the Muss, and
/// those group together rather than being dropped.
///
/// Coverage is deliberately not scored. A Typ-2 series is delivered as ordered
/// and never reconciled, corrected or substituted, so "less than the window"
/// is not a defect the way it is for a billing series — only silence is.
///
/// # Errors
///
/// Propagates the store's error when the scan cannot run.
pub async fn assess_typ2_delivery(
    typ2: &crate::store::MeterStoreTyp2Repository,
    tenant: &str,
    cfg: &SurveillanceConfig,
    now: OffsetDateTime,
    thresholds: &Typ2Thresholds,
) -> anyhow::Result<(Vec<DeliveryFinding>, usize, OffsetDateTime)> {
    let window_from = now - time::Duration::days(cfg.coverage_window_days);
    let store = typ2.store();

    let sql = format!(
        r#"SELECT "malo_id",
                  "obis_code",
                  "bestellung_ref",
                  MAX("to")   AS last_interval_end,
                  COUNT(*)    AS interval_count
             FROM "{table}"
            WHERE "tenant" = $1
              AND "from" >= $2
            GROUP BY "malo_id", "obis_code", "bestellung_ref""#,
        table = store.resolved_table(),
    );

    let rows: Vec<serde_json::Value> = store
        .query_with_params(
            &sql,
            vec![
                datafusion::scalar::ScalarValue::Utf8(Some(tenant.to_owned())),
                crate::server::ts_param(window_from),
            ],
        )
        .await
        .map_err(|e| anyhow::anyhow!("ESA Typ-2 surveillance scan: {e}"))?
        .to_json()
        .map_err(|e| anyhow::anyhow!("ESA Typ-2 surveillance decode: {e}"))?;

    let mut findings = Vec::new();
    let scanned = rows.len();
    for row in rows {
        let malo_id = row
            .get("malo_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if malo_id.is_empty() {
            continue;
        }
        let obis = row
            .get("obis_code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let last_end = row
            .get("last_interval_end")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            });
        let hours_silent = last_end.map_or(i64::MAX, |t| (now - t).whole_hours());
        let bestellung_ref = row
            .get("bestellung_ref")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        // **The threshold comes from the ordered Messprodukt.** The Codeliste
        // publishes a cadence per product and they differ by a factor of
        // several; one flat setting alerts late on the Rohdaten products, which
        // are exactly the ones an ESA's own downstream service depends on.
        if hours_silent < thresholds.for_subscription(&bestellung_ref) {
            continue;
        }
        findings.push(DeliveryFinding {
            // Packed `Meldepunkt|OBIS|Bestellnummer`; the sweep unpacks it for
            // the register row and the event. The finding keeps the Meldepunkt
            // first so the two sweeps report alike.
            malo_id: format!("{malo_id}|{obis}|{bestellung_ref}"),
            state: DeliveryState::Silent,
            last_interval_end: last_end,
            hours_silent,
            coverage_pct: 0.0,
            interval_count: row
                .get("interval_count")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        });
    }
    Ok((findings, scanned, window_from))
}

/// Per-subscription silence thresholds, in hours.
///
/// # Why this is not one number
///
/// *Codeliste der Konfigurationen* 1.4 Kap. 4.6 publishes a delivery cadence
/// **per Messprodukt**: the Rohdaten products state „unverzüglich, jedoch
/// spätestens bis 9:30 Uhr" daily, while the aufbereitete-Daten products defer
/// to *WiM Teil 2 Kap. 2.5.5*, whose windows depend on the Werteart and the
/// installed equipment. Applying `typ2_silent_after_hours` to all of them
/// alerts late on the fast ones and early on the slow ones.
///
/// An inbound MSCONS 13027 names only the Belegnummer of the ORDERS it belongs
/// to (`SG1 RFF+AGI`), never the product — so the sweep resolves
/// Belegnummer → Messprodukt at `marktd`, which holds it on the accepted
/// Angebot, and asks [`mako_wim::esa::ueberfaellig_nach_stunden`] for the
/// clock.
///
/// The **fallback is the configured setting**, deliberately. A product that
/// publishes no wall-clock deadline gets no invented one, and neither does a
/// subscription whose Belegnummer resolves to nothing — a delivery whose sender
/// omitted the Muss is a conformance defect, not a reason to stop watching it.
#[derive(Debug, Clone, Default)]
pub struct Typ2Thresholds {
    fallback: i64,
    per_subscription: std::collections::HashMap<String, i64>,
}

impl Typ2Thresholds {
    /// A table that answers `fallback` for everything — the shape a deployment
    /// without a `marktd` client gets, and what the tests use.
    #[must_use]
    pub fn flat(fallback: i64) -> Self {
        Self {
            fallback,
            per_subscription: std::collections::HashMap::new(),
        }
    }

    /// Record the clock a subscription's own Messprodukt publishes.
    pub fn insert(&mut self, bestellung_ref: impl Into<String>, hours: i64) {
        self.per_subscription.insert(bestellung_ref.into(), hours);
    }

    /// Hours of silence after which this subscription is overdue.
    #[must_use]
    pub fn for_subscription(&self, bestellung_ref: &str) -> i64 {
        self.per_subscription
            .get(bestellung_ref)
            .copied()
            .unwrap_or(self.fallback)
    }
}

/// Resolve each subscription's published cadence from `marktd`.
///
/// One `marktd` round trip per distinct Belegnummer in the window, and only for
/// the ones that resolve to a product with a clock. A lookup that fails is
/// logged and left to the fallback: an unreachable `marktd` must not silence
/// the sweep, which exists precisely because nothing else notices a Typ-2 gap.
async fn resolve_typ2_thresholds(
    marktd: Option<&mako_markt::marktd_client::MarktdClient>,
    refs: &[String],
    fallback: i64,
) -> Typ2Thresholds {
    let mut out = Typ2Thresholds::flat(fallback);
    let Some(marktd) = marktd else {
        return out;
    };
    for r in refs {
        if r.is_empty() {
            continue;
        }
        match marktd.esa_messprodukt_of_bestellung(r).await {
            Ok(Some(code)) => {
                if let Some(hours) = mako_wim::esa::ueberfaellig_nach_stunden(&code) {
                    out.insert(r.clone(), hours);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(
                error = %e, bestellung_ref = %r,
                "edmd: ESA Typ-2 surveillance: Messprodukt lookup failed — using the \
                 configured threshold for this subscription"
            ),
        }
    }
    out
}

/// The distinct `bestellung_ref`s delivered in the surveillance window.
async fn typ2_subscription_refs(
    typ2: &crate::store::MeterStoreTyp2Repository,
    tenant: &str,
    window_from: OffsetDateTime,
) -> Vec<String> {
    let store = typ2.store();
    let sql = format!(
        r#"SELECT DISTINCT "bestellung_ref" FROM "{table}"
            WHERE "tenant" = $1 AND "from" >= $2"#,
        table = store.resolved_table(),
    );
    let rows = store
        .query_with_params(
            &sql,
            vec![
                datafusion::scalar::ScalarValue::Utf8(Some(tenant.to_owned())),
                crate::server::ts_param(window_from),
            ],
        )
        .await
        .and_then(|r| r.to_json());
    match rows {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| {
                r.get("bestellung_ref")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: ESA Typ-2 subscription scan failed");
            Vec::new()
        }
    }
}

/// Unpack a Typ-2 finding key into `(Meldepunkt, OBIS-Kennzahl, Bestellnummer)`.
///
/// [`DeliveryFinding`] is shared with the Typ-1 sweep and carries one string,
/// so the three parts of a Typ-2 key travel packed. Split on the **first two**
/// separators only: a Belegnummer is a sender-chosen Dokumentennummer and
/// nothing forbids the separator inside it.
fn split_typ2_key(packed: &str) -> (&str, &str, &str) {
    let mut parts = packed.splitn(3, '|');
    (
        parts.next().unwrap_or(packed),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    )
}

/// Run one ESA Typ-2 sweep: assess, register the transitions, emit.
///
/// Mirrors [`run_surveillance_sweep`] over the other store, with its own
/// `stream = 'TYP2'` rows so a silent ESA subscription and a silent billing
/// meter never collide in the register.
pub async fn run_typ2_surveillance_sweep(
    typ2: &crate::store::MeterStoreTyp2Repository,
    pool: &sqlx::PgPool,
    cfg: &SurveillanceConfig,
    tenant: &str,
    marktd: Option<&mako_markt::marktd_client::MarktdClient>,
    erp_webhook_url: Option<&str>,
    erp_webhook_secret: Option<&str>,
) -> SurveillanceReport {
    let scanned_at = OffsetDateTime::now_utc();
    let client = mako_service::http::default_client();
    let empty = |window_from| SurveillanceReport {
        scanned_at,
        window_from,
        points_scanned: 0,
        findings: Vec::new(),
        newly_overdue: 0,
        resumed: 0,
        suppressed: 0,
    };

    // Size the silence threshold per subscription before assessing: the
    // Codeliste publishes a cadence per Messprodukt, and `marktd` is what maps
    // a Belegnummer to the product that was ordered under it.
    let window_start = scanned_at - time::Duration::days(cfg.coverage_window_days);
    let refs = typ2_subscription_refs(typ2, tenant, window_start).await;
    let thresholds = resolve_typ2_thresholds(marktd, &refs, cfg.typ2_silent_after_hours).await;

    let (findings, points_scanned, window_from) =
        match assess_typ2_delivery(typ2, tenant, cfg, scanned_at, &thresholds).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, tenant, "edmd: ESA Typ-2 surveillance: scan failed");
                return empty(scanned_at);
            }
        };

    let mut newly_overdue = 0usize;
    let mut emitted = 0usize;
    let mut suppressed = 0usize;

    for finding in &findings {
        let (malo_id, obis, bestellung_ref) = split_typ2_key(&finding.malo_id);
        let transition: Option<bool> = match sqlx::query_scalar(
            r"INSERT INTO delivery_surveillance
                  (tenant, malo_id, stream, obis_code, subscription_ref, state,
                   last_interval_end, hours_silent, coverage_pct, interval_count)
              VALUES ($1,$2,'TYP2',$3,$4,$5,$6,$7,0,$8)
              ON CONFLICT (tenant, stream, malo_id, obis_code, subscription_ref) DO UPDATE SET
                  last_seen_at      = now(),
                  state             = EXCLUDED.state,
                  last_interval_end = EXCLUDED.last_interval_end,
                  hours_silent      = EXCLUDED.hours_silent,
                  interval_count    = EXCLUDED.interval_count,
                  first_detected_at = CASE
                      WHEN delivery_surveillance.resolved_at IS NOT NULL THEN now()
                      ELSE delivery_surveillance.first_detected_at END,
                  resolved_at       = NULL
              RETURNING first_detected_at = last_seen_at",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(obis)
        .bind(bestellung_ref)
        .bind(finding.state.as_str())
        .bind(finding.last_interval_end)
        .bind(finding.hours_silent)
        .bind(finding.interval_count)
        .fetch_optional(pool)
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, %malo_id,
                    "edmd: ESA Typ-2 surveillance: register write failed");
                continue;
            }
        };

        if transition != Some(true) {
            continue;
        }
        newly_overdue += 1;
        if emitted >= cfg.max_events_per_sweep {
            suppressed += 1;
            continue;
        }

        tracing::warn!(
            %malo_id, %obis, %bestellung_ref,
            hours_silent = finding.hours_silent,
            "edmd: ESA Typ-2 subscription has stopped delivering (§60 Abs. 1 MsbG)"
        );

        if let Some(url) = erp_webhook_url {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("edmd", tenant),
                mako_events::messwert::ESA_TYP2_DELIVERY_OVERDUE,
                malo_id.to_owned(),
                serde_json::json!({
                    "malo_id":           malo_id,
                    "obis_code":         obis,
                    // `SG1 RFF+AGI` — the ORDERS 17007 that ordered these
                    // values. Empty when the MSB omitted the Muss, which is
                    // itself worth an operator's attention.
                    "bestellung_ref":    bestellung_ref,
                    "last_interval_end": finding.last_interval_end.map(|t| t.to_string()),
                    "hours_silent":      finding.hours_silent,
                    "interval_count":    finding.interval_count,
                    "window_from":       window_from.to_string(),
                    "legal_basis":       "§60 Abs. 1 MsbG (Übermittlung an berechtigte Stellen)",
                    "recommended_action":
                        "Check the MSB's delivery and the subscription's state; a Typ-2 \
                         value is never substituted or reconciled, so a gap can only be \
                         closed by the MSB re-sending it",
                }),
            )
            .extension("tenantid", tenant)
            .extension("worker", "esa-typ2-surveillance");
            if let Err(e) = mako_service::post_ce_with_retry(
                &client,
                url,
                &ce,
                erp_webhook_secret.map(str::as_bytes),
            )
            .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
        emitted += 1;
    }

    // Close what this sweep no longer finds.
    let reported: Vec<String> = findings
        .iter()
        .map(|f| {
            let (m, o, b) = split_typ2_key(&f.malo_id);
            format!("{m}\u{1}{o}\u{1}{b}")
        })
        .collect();
    let resumed_rows = sqlx::query(
        r"UPDATE delivery_surveillance
             SET resolved_at = now()
           WHERE tenant = $1 AND stream = 'TYP2' AND resolved_at IS NULL
             AND NOT (malo_id || chr(1) || obis_code || chr(1) || subscription_ref = ANY($2))
       RETURNING malo_id, obis_code, subscription_ref, state, first_detected_at",
    )
    .bind(tenant)
    .bind(&reported)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for row in &resumed_rows {
        use sqlx::Row as _;
        let malo_id: String = row.get("malo_id");
        let obis: String = row.get("obis_code");
        let bestellung_ref: String = row.get("subscription_ref");
        tracing::info!(%malo_id, %obis, %bestellung_ref,
            "edmd: ESA Typ-2 subscription is delivering again");
        if let Some(url) = erp_webhook_url {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("edmd", tenant),
                mako_events::messwert::ESA_TYP2_DELIVERY_RESUMED,
                malo_id.clone(),
                serde_json::json!({
                    "malo_id":       malo_id,
                    "obis_code":     obis,
                    "bestellung_ref": bestellung_ref,
                    "was_state":     row.get::<String, _>("state"),
                    "overdue_since": row
                        .get::<OffsetDateTime, _>("first_detected_at")
                        .to_string(),
                    "resumed_at":    scanned_at.to_string(),
                }),
            )
            .extension("tenantid", tenant)
            .extension("worker", "esa-typ2-surveillance");
            if let Err(e) = mako_service::post_ce_with_retry(
                &client,
                url,
                &ce,
                erp_webhook_secret.map(str::as_bytes),
            )
            .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
    }

    SurveillanceReport {
        scanned_at,
        window_from,
        points_scanned,
        findings,
        newly_overdue,
        resumed: resumed_rows.len(),
        suppressed,
    }
}
