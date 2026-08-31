//! B12 — Monthly MMMA Gas / MMM Strom price import background worker.
//!
//! ## Design
//!
//! The worker wakes every hour and checks whether:
//! 1. Today is the 1st of the month (or `force_trigger_today = true`).
//! 2. The configured `check_hour_utc` has been reached.
//! 3. A price record for the **current** month does not already exist
//!    (idempotent — won't over-write a successful import).
//!
//! When all three conditions are met it fetches from the configured URLs,
//! parses the CSV/JSON response, and upserts into the `mmma_preise_gas` /
//! `mmm_preise_strom` tables via the repository layer.
//!
//! ## CloudEvents
//!
//! On success: `de.markt.mmma.import.success`
//! On failure: `de.markt.mmma.import.failed`
//!
//! Both are sent to the durable fan-out (all ERP webhooks subscribed to
//! `de.markt.*` receive them automatically).
//!
//! ## Manual trigger
//!
//! `POST /api/v1/mmma-preise/import-trigger` triggers an immediate import
//! regardless of the schedule.  Useful for testing and catch-up after
//! service downtime.

use std::sync::Arc;

use mako_markt::{
    cloudevents::MarktEvent,
    repository::{MmmPreisStromRepository, MmmaPreisGasRepository},
};
use rust_decimal::Decimal;
use std::str::FromStr;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::pg::mmma_preise::{PgMmmPreisStromRepository, PgMmmaPreisGasRepository};

/// Result of a single MMMA import attempt.
#[derive(Debug)]
pub struct ImportResult {
    pub commodity: &'static str,
    pub year: i32,
    pub month: u8,
    pub success: bool,
    pub error: Option<String>,
}

/// Fetch a price file as text from a URL or local path.
///
/// Supports `http(s)://…` and `file://…`. Returns `None` when the URL is empty
/// (that commodity's import is switched off).
///
/// `http` is the shared inter-service client, which carries a connect timeout
/// and refuses redirects. Both matter here: this is the one place marktd
/// reaches out to the public internet on a schedule, and the target is an
/// operator-supplied URL, so a redirect would let a compromised or mistyped
/// source point the fetch at cluster-internal infrastructure.
async fn fetch_raw(http: &reqwest::Client, url: &str) -> Option<Result<String, String>> {
    if url.is_empty() {
        return None;
    }
    if let Some(path) = url.strip_prefix("file://") {
        // On a blocking pool: a file read is not instantaneous on a network
        // mount, and this runs on the same runtime that serves HTTP.
        let path = path.to_owned();
        let joined = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await;
        return Some(match joined {
            Ok(read) => read.map_err(|e| format!("file read error {url}: {e}")),
            Err(e) => Err(format!("file read task failed: {e}")),
        });
    }
    match http.get(url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(t) => Some(Ok(t)),
            Err(e) => Some(Err(format!("HTTP body error from {url}: {e}"))),
        },
        Ok(resp) => Some(Err(format!("HTTP {} from {url}", resp.status().as_u16()))),
        Err(e) => Some(Err(format!("HTTP request error for {url}: {e}"))),
    }
}

/// Read one price field as an exact [`Decimal`].
///
/// A JSON number is taken from its literal text rather than through `f64`:
/// these are settlement prices to four decimal places in ct/kWh, and a detour
/// through binary floating point is a rounding error on money for no reason.
fn decimal_field(item: &serde_json::Value, field: &str) -> Result<Decimal, String> {
    let raw = item.get(field).ok_or_else(|| format!("missing {field}"))?;
    let text = match raw {
        serde_json::Value::String(s) => s.trim().to_owned(),
        serde_json::Value::Number(n) => n.to_string(),
        other => return Err(format!("{field}: expected a number or string, got {other}")),
    };
    Decimal::from_str(&text).map_err(|e| format!("{field}: {e}"))
}

/// Parse Gas MMMA prices from a CSV or JSON body.
///
/// ## CSV format (one header row, one data row per marktgebiet):
///
/// ```csv
/// year,month,marktgebiet,mehr_ct_kwh,minder_ct_kwh
/// 2026,7,THE,1.2300,0.8700
/// ```
///
/// ## JSON format (single object or array):
///
/// ```json
/// { "mehr_ct_kwh": "1.23", "minder_ct_kwh": "0.87" }
/// ```
fn parse_gas_prices(
    body: &str,
    year: i32,
    month: u8,
) -> Result<Vec<(String, Decimal, Decimal)>, String> {
    let body = body.trim();
    // Try JSON first.
    if body.starts_with('{') || body.starts_with('[') {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("JSON parse error: {e}"))?;
        let items: Vec<&serde_json::Value> = match v.as_array() {
            Some(arr) => arr.iter().collect(),
            None => vec![&v],
        };
        let mut result = Vec::new();
        for item in items {
            let marktgebiet = item
                .get("marktgebiet")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("THE")
                .to_owned();
            let mehr = decimal_field(item, "mehr_ct_kwh")?;
            let minder = decimal_field(item, "minder_ct_kwh")?;
            result.push((marktgebiet, mehr, minder));
        }
        return Ok(result);
    }

    // CSV: skip header row, parse data rows.
    let mut result = Vec::new();
    for (i, line) in body.lines().enumerate() {
        if i == 0 {
            continue; // header
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            // Try 3-column: marktgebiet,mehr,minder
            if cols.len() >= 3 {
                let marktgebiet = cols[0].trim().to_owned();
                let mehr: Decimal = cols[1]
                    .trim()
                    .parse()
                    .map_err(|e| format!("CSV row {i} mehr_ct_kwh: {e}"))?;
                let minder: Decimal = cols[2]
                    .trim()
                    .parse()
                    .map_err(|e| format!("CSV row {i} minder_ct_kwh: {e}"))?;
                result.push((marktgebiet, mehr, minder));
            }
            continue;
        }
        // year(0), month(1), marktgebiet(2), mehr(3), minder(4)
        let row_year: i32 = cols[0].trim().parse().unwrap_or(year);
        let row_month: u8 = cols[1].trim().parse().unwrap_or(month);
        if row_year != year || row_month != month {
            continue; // skip rows for other months
        }
        let marktgebiet = cols[2].trim().to_owned();
        let mehr: Decimal = cols[3]
            .trim()
            .parse()
            .map_err(|e| format!("CSV row {i} mehr_ct_kwh: {e}"))?;
        let minder: Decimal = cols[4]
            .trim()
            .parse()
            .map_err(|e| format!("CSV row {i} minder_ct_kwh: {e}"))?;
        result.push((marktgebiet, mehr, minder));
    }
    if result.is_empty() {
        Err(format!(
            "no valid price data found for {year}-{month:02} in CSV"
        ))
    } else {
        Ok(result)
    }
}

/// Parse the nationwide Strom Mehr-/Mindermengenpreise for one month.
///
/// The wire format matches the Gas file, minus the market-area dimension: the
/// BDEW series has no operator or area column, so any such field in the source
/// is ignored and a file carrying several rows for the month is a source error
/// rather than several valid prices.
fn parse_strom_prices(body: &str, year: i32, month: u8) -> Result<Vec<(Decimal, Decimal)>, String> {
    let rows = parse_gas_prices(body, year, month)?;
    if rows.len() > 1 {
        return Err(format!(
            "Strom Mehr-/Mindermengenpreise are nationwide and uniform (§ 13 Abs. 3 \
             StromNZV), so {}-{month:02} must carry exactly one price pair — the source \
             returned {}",
            year,
            rows.len()
        ));
    }
    Ok(rows
        .into_iter()
        .map(|(_, mehr, minder)| (mehr, minder))
        .collect())
}

/// Run one import cycle for the given year/month.
#[allow(clippy::too_many_arguments)]
pub async fn run_import_cycle(
    year: i32,
    month: u8,
    http: &reqwest::Client,
    gas_url: &str,
    strom_url: &str,
    gas_repo: &PgMmmaPreisGasRepository,
    strom_repo: &PgMmmPreisStromRepository,
    tenant: &str,
    event_tx: &EventSink<'_>,
) -> Vec<ImportResult> {
    let mut results = Vec::new();

    // ── Gas MMMA ─────────────────────────────────────────────────────────────
    if let Some(fetch_result) = fetch_raw(http, gas_url).await {
        match fetch_result {
            Err(e) => {
                warn!(year, month, error = %e, "MMMA import: Gas fetch failed");
                emit_event(
                    event_tx,
                    tenant,
                    mako_events::markt::MMMA_IMPORT_FAILED,
                    serde_json::json!({
                        "commodity": "gas",
                        "year": year, "month": month,
                        "error": e,
                    }),
                )
                .await;
                results.push(ImportResult {
                    commodity: "gas",
                    year,
                    month,
                    success: false,
                    error: Some(e),
                });
            }
            Ok(body) => match parse_gas_prices(&body, year, month) {
                Err(e) => {
                    warn!(year, month, error = %e, "MMMA import: Gas parse failed");
                    emit_event(
                        event_tx,
                        tenant,
                        mako_events::markt::MMMA_IMPORT_FAILED,
                        serde_json::json!({
                            "commodity": "gas",
                            "year": year, "month": month,
                            "error": e,
                        }),
                    )
                    .await;
                    results.push(ImportResult {
                        commodity: "gas",
                        year,
                        month,
                        success: false,
                        error: Some(e),
                    });
                }
                Ok(prices) => {
                    let mut ok = true;
                    for (marktgebiet, mehr, minder) in &prices {
                        let price_month = time::Date::from_calendar_date(
                            year,
                            time::Month::try_from(month).unwrap_or(time::Month::January),
                            1,
                        )
                        .unwrap_or_else(|_| mako_fristen::heute());
                        if let Err(e) = gas_repo
                            .upsert_gas(price_month, marktgebiet, *mehr, *minder, "the-api")
                            .await
                        {
                            warn!(year, month, error = %e, "MMMA import: Gas DB upsert failed");
                            ok = false;
                        }
                    }
                    if ok {
                        info!(
                            year,
                            month,
                            count = prices.len(),
                            "MMMA import: Gas prices imported"
                        );
                        emit_event(
                            event_tx,
                            tenant,
                            mako_events::markt::MMMA_IMPORT_SUCCESS,
                            serde_json::json!({
                                "commodity": "gas",
                                "year": year, "month": month,
                                "count": prices.len(),
                                "source": "the-api",
                            }),
                        )
                        .await;
                        results.push(ImportResult {
                            commodity: "gas",
                            year,
                            month,
                            success: true,
                            error: None,
                        });
                    } else {
                        results.push(ImportResult {
                            commodity: "gas",
                            year,
                            month,
                            success: false,
                            error: Some("DB upsert failed".into()),
                        });
                    }
                }
            },
        }
    }

    // ── Strom MMM ─────────────────────────────────────────────────────────────
    if let Some(fetch_result) = fetch_raw(http, strom_url).await {
        match fetch_result {
            Err(e) => {
                warn!(year, month, error = %e, "MMM import: Strom fetch failed");
                emit_event(
                    event_tx,
                    tenant,
                    mako_events::markt::MMMA_IMPORT_FAILED,
                    serde_json::json!({
                        "commodity": "strom",
                        "year": year, "month": month,
                        "error": e,
                    }),
                )
                .await;
                results.push(ImportResult {
                    commodity: "strom",
                    year,
                    month,
                    success: false,
                    error: Some(e),
                });
            }
            Ok(body) => match parse_strom_prices(&body, year, month) {
                Err(e) => {
                    warn!(year, month, error = %e, "MMM import: Strom parse failed");
                    emit_event(
                        event_tx,
                        tenant,
                        mako_events::markt::MMMA_IMPORT_FAILED,
                        serde_json::json!({
                            "commodity": "strom",
                            "year": year, "month": month,
                            "error": e,
                        }),
                    )
                    .await;
                    results.push(ImportResult {
                        commodity: "strom",
                        year,
                        month,
                        success: false,
                        error: Some(e),
                    });
                }
                Ok(prices) => {
                    let mut ok = true;
                    for (mehr, minder) in &prices {
                        let price_month = time::Date::from_calendar_date(
                            year,
                            time::Month::try_from(month).unwrap_or(time::Month::January),
                            1,
                        )
                        .unwrap_or_else(|_| mako_fristen::heute());
                        if let Err(e) = strom_repo
                            .upsert_strom(price_month, *mehr, *minder, "bdew-csv")
                            .await
                        {
                            warn!(year, month, error = %e, "MMM import: Strom DB upsert failed");
                            ok = false;
                        }
                    }
                    if ok {
                        info!(
                            year,
                            month,
                            count = prices.len(),
                            "MMM import: Strom prices imported"
                        );
                        emit_event(
                            event_tx,
                            tenant,
                            mako_events::markt::MMMA_IMPORT_SUCCESS,
                            serde_json::json!({
                                "commodity": "strom",
                                "year": year, "month": month,
                                "count": prices.len(),
                                "source": "bdew-csv",
                            }),
                        )
                        .await;
                        results.push(ImportResult {
                            commodity: "strom",
                            year,
                            month,
                            success: true,
                            error: None,
                        });
                    } else {
                        results.push(ImportResult {
                            commodity: "strom",
                            year,
                            month,
                            success: false,
                            error: Some("DB upsert failed".into()),
                        });
                    }
                }
            },
        }
    }

    results
}

async fn emit_event(sink: &EventSink<'_>, tenant: &str, event_type: &str, data: serde_json::Value) {
    // Every import event names its commodity; that *is* the Sparte, so a
    // subscriber that only settles Strom is not woken by the THE Gas import.
    let sparte = match data.get("commodity").and_then(serde_json::Value::as_str) {
        Some("gas") => Some("GAS".to_owned()),
        Some("strom") => Some("STROM".to_owned()),
        _ => None,
    };
    let evt = MarktEvent::new(tenant, event_type, "marktd/mmma-worker".to_owned(), data)
        .with_extensions(mako_markt::cloudevents::EventExtensions {
            marktsparte: sparte,
            ..Default::default()
        });
    // Background worker: no HTTP request to fail, so an enqueue failure is
    // logged at error level (the event is not silently dropped). Correctness of
    // the fan-out still holds — nothing is fanned out unless it is durable.
    if let Err(e) = crate::outbox::enqueue(sink.pool, &evt, sink.notify).await {
        tracing::error!(error = %e, event_type, "mmma-worker: durable enqueue failed");
    }
}

/// Bundles the durable-outbox handles (`event_log` pool + fan-out wake-up hint)
/// so the import cycle can persist events without an in-memory channel.
pub struct EventSink<'a> {
    pub pool: &'a sqlx::PgPool,
    pub notify: &'a tokio::sync::Notify,
}

/// Spawn the MMMA background import worker.
///
/// Wakes every hour, checks whether today is the 1st and the `check_hour_utc`
/// has been reached, then runs `run_import_cycle` if a price record for the
/// current month does not already exist.
#[allow(clippy::too_many_arguments)] // injected handles, each independently owned
pub fn spawn_mmma_worker(
    cfg: Arc<crate::config::MmmaImportConfig>,
    http: reqwest::Client,
    gas_repo: Arc<PgMmmaPreisGasRepository>,
    strom_repo: Arc<PgMmmPreisStromRepository>,
    tenant: String,
    pool: sqlx::PgPool,
    notify: Arc<tokio::sync::Notify>,
    shutdown: CancellationToken,
) {
    if !cfg.enabled {
        info!("MMMA import worker disabled (mmma_import.enabled = false)");
        return;
    }
    info!(
        gas_url = %cfg.gas_url,
        strom_url = %cfg.strom_url,
        check_hour_utc = cfg.check_hour_utc,
        "MMMA import worker starting (B12)"
    );

    tokio::spawn(async move {
        // Wake hourly and import whatever the current month is still missing.
        //
        // The previous schedule fired only *on the 1st* at or after
        // `check_hour_utc`, so a deployment that was down for that one day
        // never imported that month at all — and its idempotency check looked
        // only at Gas, so a month where Gas succeeded and Strom failed was
        // treated as complete and Strom stayed missing until someone noticed a
        // billing run refusing. Both are now per-commodity "is it there yet"
        // checks that keep retrying for as long as the month is incomplete.
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3_600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = shutdown.cancelled() => {
                    info!("MMMA import worker: shutting down");
                    break;
                }
            }

            let now = time::OffsetDateTime::now_utc();
            // The publications describe the current application month, and are
            // not there before `check_hour_utc` on its first day.
            if now.day() == 1 && now.hour() < cfg.check_hour_utc {
                continue;
            }
            let (year, month) = (now.year(), now.month() as u8);
            let price_month =
                time::Date::from_calendar_date(year, now.month(), 1).unwrap_or_else(|_| now.date());

            let gas_missing = !cfg.gas_url.is_empty()
                && !matches!(gas_repo.find_gas(price_month, "THE").await, Ok(Some(_)));
            let strom_missing = !cfg.strom_url.is_empty()
                && !matches!(strom_repo.find_strom(price_month).await, Ok(Some(_)));

            if !gas_missing && !strom_missing {
                continue;
            }

            info!(
                year,
                month, gas_missing, strom_missing, "MMMA import worker: fetching missing prices"
            );
            run_import_cycle(
                year,
                month,
                &http,
                if gas_missing { &cfg.gas_url } else { "" },
                if strom_missing { &cfg.strom_url } else { "" },
                &gas_repo,
                &strom_repo,
                &tenant,
                &EventSink {
                    pool: &pool,
                    notify: &notify,
                },
            )
            .await;
        }
    });
}
