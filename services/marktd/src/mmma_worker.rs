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
//! Both are sent to the EventBus fan-out (all ERP webhooks subscribed to
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

/// Fetch raw bytes from a URL or local file path.
///
/// Supports `http(s)://...` and `file:///...`.
/// Returns `None` when the URL is empty (commodity import disabled).
async fn fetch_raw(url: &str) -> Option<Result<String, String>> {
    if url.is_empty() {
        return None;
    }
    if let Some(path) = url.strip_prefix("file://") {
        match std::fs::read_to_string(path) {
            Ok(s) => Some(Ok(s)),
            Err(e) => Some(Err(format!("file read error {path}: {e}"))),
        }
    } else {
        match reqwest::get(url).await {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(t) => Some(Ok(t)),
                Err(e) => Some(Err(format!("HTTP body error from {url}: {e}"))),
            },
            Ok(resp) => Some(Err(format!("HTTP {} from {url}", resp.status().as_u16()))),
            Err(e) => Some(Err(format!("HTTP request error for {url}: {e}"))),
        }
    }
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
        let items: Vec<&serde_json::Value> = if v.is_array() {
            v.as_array().unwrap().iter().collect()
        } else {
            vec![&v]
        };
        let mut result = Vec::new();
        for item in items {
            let marktgebiet = item
                .get("marktgebiet")
                .and_then(|v| v.as_str())
                .unwrap_or("THE")
                .to_owned();
            let mehr = item
                .get("mehr_ct_kwh")
                .and_then(|v| v.as_str().or_else(|| v.as_f64().map(|_| "")))
                .unwrap_or("");
            let mehr: Decimal = mehr
                .parse()
                .or_else(|_| {
                    item.get("mehr_ct_kwh")
                        .and_then(|v| v.as_f64())
                        .map(|f| Decimal::from_str(&format!("{f:.5}")).unwrap_or_default())
                        .ok_or_else(|| "missing mehr_ct_kwh".to_string())
                })
                .map_err(|e| format!("mehr_ct_kwh: {e}"))?;
            let minder_str = item
                .get("minder_ct_kwh")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let minder: Decimal = minder_str
                .parse()
                .or_else(|_| {
                    item.get("minder_ct_kwh")
                        .and_then(|v| v.as_f64())
                        .map(|f| Decimal::from_str(&format!("{f:.5}")).unwrap_or_default())
                        .ok_or_else(|| "missing minder_ct_kwh".to_string())
                })
                .map_err(|e| format!("minder_ct_kwh: {e}"))?;
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

/// Parse Strom MMM prices.  Same format as Gas MMMA but `uenb` field instead of `marktgebiet`.
fn parse_strom_prices(
    body: &str,
    year: i32,
    month: u8,
) -> Result<Vec<(String, Decimal, Decimal)>, String> {
    // Re-use Gas parser — the field name mapping happens at the call site.
    parse_gas_prices(body, year, month)
}

/// Run one import cycle for the given year/month.
#[allow(clippy::too_many_arguments)]
pub async fn run_import_cycle(
    year: i32,
    month: u8,
    gas_url: &str,
    strom_url: &str,
    gas_repo: &PgMmmaPreisGasRepository,
    strom_repo: &PgMmmPreisStromRepository,
    tenant: &str,
    event_tx: &tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
) -> Vec<ImportResult> {
    let mut results = Vec::new();

    // ── Gas MMMA ─────────────────────────────────────────────────────────────
    if let Some(fetch_result) = fetch_raw(gas_url).await {
        match fetch_result {
            Err(e) => {
                warn!(year, month, error = %e, "MMMA import: Gas fetch failed");
                emit_event(
                    event_tx,
                    tenant,
                    "de.markt.mmma.import.failed",
                    serde_json::json!({
                        "commodity": "gas",
                        "year": year, "month": month,
                        "error": e,
                    }),
                );
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
                        "de.markt.mmma.import.failed",
                        serde_json::json!({
                            "commodity": "gas",
                            "year": year, "month": month,
                            "error": e,
                        }),
                    );
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
                        .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date());
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
                            "de.markt.mmma.import.success",
                            serde_json::json!({
                                "commodity": "gas",
                                "year": year, "month": month,
                                "count": prices.len(),
                                "source": "the-api",
                            }),
                        );
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
    if let Some(fetch_result) = fetch_raw(strom_url).await {
        match fetch_result {
            Err(e) => {
                warn!(year, month, error = %e, "MMM import: Strom fetch failed");
                emit_event(
                    event_tx,
                    tenant,
                    "de.markt.mmma.import.failed",
                    serde_json::json!({
                        "commodity": "strom",
                        "year": year, "month": month,
                        "error": e,
                    }),
                );
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
                        "de.markt.mmma.import.failed",
                        serde_json::json!({
                            "commodity": "strom",
                            "year": year, "month": month,
                            "error": e,
                        }),
                    );
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
                    for (uenb, mehr, minder) in &prices {
                        let price_month = time::Date::from_calendar_date(
                            year,
                            time::Month::try_from(month).unwrap_or(time::Month::January),
                            1,
                        )
                        .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date());
                        if let Err(e) = strom_repo
                            .upsert_strom(price_month, uenb, *mehr, *minder, "uenb-api")
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
                            "de.markt.mmma.import.success",
                            serde_json::json!({
                                "commodity": "strom",
                                "year": year, "month": month,
                                "count": prices.len(),
                                "source": "uenb-api",
                            }),
                        );
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

fn emit_event(
    event_tx: &tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    tenant: &str,
    event_type: &str,
    data: serde_json::Value,
) {
    let evt = MarktEvent::new(tenant, event_type, "marktd/mmma-worker".to_owned(), data);
    if let Ok(payload) = serde_json::to_value(&evt) {
        let _ = event_tx.send(payload);
    }
}

/// Spawn the MMMA background import worker.
///
/// Wakes every hour, checks whether today is the 1st and the `check_hour_utc`
/// has been reached, then runs `run_import_cycle` if a price record for the
/// current month does not already exist.
pub fn spawn_mmma_worker(
    cfg: Arc<crate::config::MmmaImportConfig>,
    gas_repo: Arc<PgMmmaPreisGasRepository>,
    strom_repo: Arc<PgMmmPreisStromRepository>,
    tenant: String,
    event_tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
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
        // Wake every hour to check if import is needed.
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3_600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown.cancelled() => {
                    info!("MMMA import worker: shutting down");
                    break;
                }
            }

            let now = time::OffsetDateTime::now_utc();
            let day = now.day();
            let hour = now.hour();
            let year = now.year();
            let month = now.month() as u8;

            // Only import on 1st of month at or after check_hour_utc.
            if day != 1 || hour < cfg.check_hour_utc {
                continue;
            }

            // Check if Gas prices already exist for this month (idempotency).
            let price_month = time::Date::from_calendar_date(
                year,
                time::Month::try_from(month).unwrap_or(time::Month::January),
                1,
            )
            .unwrap_or(now.date());
            let already_exists = matches!(gas_repo.find_gas(price_month, "THE").await, Ok(Some(_)));

            if already_exists {
                // Already imported for this month — no-op.
                continue;
            }

            info!(year, month, "MMMA import worker: running monthly import");
            run_import_cycle(
                year,
                month,
                &cfg.gas_url,
                &cfg.strom_url,
                &gas_repo,
                &strom_repo,
                &tenant,
                &event_tx,
            )
            .await;
        }
    });
}
