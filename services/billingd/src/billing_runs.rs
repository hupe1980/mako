//! §40b EnWG scheduled billing runs.
//!
//! The worker sweeps once per day (after `billing_runs.run_hour_utc`):
//!
//! 1. Pull the active supply components + their contract's
//!    `abrechnungszyklus` from vertragd (`/api/v1/vertraege/billing-candidates`).
//! 2. For each, compute the most recently **completed** billing period:
//!    - `MONATLICH` — the previous calendar month (§40b: monthly option);
//!    - `VIERTELJAEHRLICH` — the previous calendar quarter;
//!    - `HALBJAEHRLICH` — the previous calendar half-year;
//!    - `JAEHRLICH` — the 12-month window ending the day before the most
//!      recent `vertragsbeginn` anniversary (rolling Stichtag, the common
//!      German annual-billing practice).
//!      The window is clipped to the component's supply dates.
//! 3. Skip periods that already have a `billing_records` row (per-invoice
//!    idempotency — the same guard the on-demand endpoint relies on), then
//!    run the exact `dispatch → persist → emit` pipeline the HTTP endpoint
//!    uses. §40c EnWG is why this worker exists: an invoice the customer
//!    must receive within six weeks (three for monthly billing) cannot wait
//!    for someone to call an endpoint.
//! 4. Accumulate the month's `billing_run_log` row (audit).
//! 5. For iMSys MaLos, deliver the free monthly **Abrechnungsinformation**
//!    (§40b Abs. 2 EnWG) as a `de.billing.abrechnungsinformation.monatlich`
//!    CloudEvent — a preview calculation, not a persisted invoice, logged in
//!    `abrechnungsinfo_log` so each month is delivered exactly once.

use std::sync::Arc;

use sqlx::PgPool;
use time::Date;

use crate::clients::{BillingCandidate, EdmdClient, TarifbdClient, VertragdClient};
use crate::config::BillingdConfig;
use crate::handlers::{self, CalculateRequest};
use crate::pg;

/// The most recently completed billing period for a cadence, as of `today`.
///
/// Returns `None` when no completed period exists yet (e.g. a contract in
/// its first year for `JAEHRLICH`).
fn due_period(zyklus: &str, today: Date, vertragsbeginn: Date) -> Option<(Date, Date)> {
    match zyklus {
        "MONATLICH" => {
            let first_of_this = today.replace_day(1).ok()?;
            let prev_end = first_of_this.previous_day()?;
            let prev_start = prev_end.replace_day(1).ok()?;
            Some((prev_start, prev_end))
        }
        "VIERTELJAEHRLICH" => {
            let q_start_month = 1 + 3 * ((u8::from(today.month()) - 1) / 3);
            let this_q_start = Date::from_calendar_date(
                today.year(),
                time::Month::try_from(q_start_month).ok()?,
                1,
            )
            .ok()?;
            let prev_end = this_q_start.previous_day()?;
            let prev_start = {
                let m = 1 + 3 * ((u8::from(prev_end.month()) - 1) / 3);
                Date::from_calendar_date(prev_end.year(), time::Month::try_from(m).ok()?, 1).ok()?
            };
            Some((prev_start, prev_end))
        }
        "HALBJAEHRLICH" => {
            let h_start_month = if u8::from(today.month()) <= 6 { 1 } else { 7 };
            let this_h_start = Date::from_calendar_date(
                today.year(),
                time::Month::try_from(h_start_month).ok()?,
                1,
            )
            .ok()?;
            let prev_end = this_h_start.previous_day()?;
            let prev_start = {
                let m = if u8::from(prev_end.month()) <= 6 {
                    1
                } else {
                    7
                };
                Date::from_calendar_date(prev_end.year(), time::Month::try_from(m).ok()?, 1).ok()?
            };
            Some((prev_start, prev_end))
        }
        // JAEHRLICH (and anything unknown, conservatively): rolling year
        // anchored on the vertragsbeginn anniversary.
        _ => {
            let anniv_this_year = vertragsbeginn
                .replace_year(today.year())
                .unwrap_or_else(|_| {
                    Date::from_calendar_date(today.year(), time::Month::February, 28)
                        .expect("Feb 28 exists")
                });
            let anniv = if anniv_this_year <= today {
                anniv_this_year
            } else {
                vertragsbeginn
                    .replace_year(today.year() - 1)
                    .unwrap_or_else(|_| {
                        Date::from_calendar_date(today.year() - 1, time::Month::February, 28)
                            .expect("Feb 28 exists")
                    })
            };
            let start = anniv
                .replace_year(anniv.year() - 1)
                .unwrap_or_else(|_| anniv - time::Duration::days(365));
            let end = anniv.previous_day()?;
            // First year not completed yet.
            if start < vertragsbeginn {
                return None;
            }
            Some((start, end))
        }
    }
}

/// Clip a period to the component's supply window. `None` = nothing billable.
fn clip(
    (from, to): (Date, Date),
    lieferbeginn: Date,
    lieferende: Option<Date>,
) -> Option<(Date, Date)> {
    let from = from.max(lieferbeginn);
    let to = match lieferende {
        Some(ende) => to.min(ende),
        None => to,
    };
    (from <= to).then_some((from, to))
}

/// Spawn the §40b billing-run worker. No-op when disabled in config.
#[allow(clippy::too_many_arguments)]
pub fn spawn_billing_run_worker(
    cfg: Arc<BillingdConfig>,
    pool: PgPool,
    tarifbd: Arc<TarifbdClient>,
    edmd: Arc<EdmdClient>,
    marktd: Arc<mako_markt::marktd_client::MarktdClient>,
    vertragd: Arc<VertragdClient>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    if !cfg.billing_runs.enabled {
        tracing::info!("billingd: §40b billing-run worker disabled ([billing_runs] enabled=false)");
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_sweep: Option<Date> = None;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = shutdown.cancelled() => {
                    tracing::info!("billingd: billing-run worker shutting down");
                    return;
                }
            }
            let now = time::OffsetDateTime::now_utc();
            if now.hour() < cfg.billing_runs.run_hour_utc || last_sweep == Some(now.date()) {
                continue;
            }
            last_sweep = Some(now.date());
            sweep(&cfg, &pool, &tarifbd, &edmd, &marktd, &vertragd, now.date()).await;
        }
    });
}

/// One daily sweep: bill everything due, deliver monthly infos, log the run.
async fn sweep(
    cfg: &Arc<BillingdConfig>,
    pool: &PgPool,
    tarifbd: &Arc<TarifbdClient>,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    vertragd: &Arc<VertragdClient>,
    today: Date,
) {
    let candidates = match vertragd.get_billing_candidates().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "billing-run: vertragd candidates unavailable — sweep skipped");
            return;
        }
    };
    tracing::info!(count = candidates.len(), %today, "billing-run: daily sweep");

    let mut billed = 0i32;
    let mut errors = 0i32;
    let mut lf_for_log: Option<String> = None;

    for cand in &candidates {
        lf_for_log.get_or_insert_with(|| cand.lf_mp_id.clone());

        let Some(period) = due_period(&cand.abrechnungszyklus, today, cand.vertragsbeginn) else {
            continue;
        };
        let Some((from, to)) = clip(period, cand.lieferbeginn, cand.lieferende) else {
            continue;
        };
        match pg::billing_record_exists_for_period(pool, &cfg.tenant, &cand.malo_id, from, to).await
        {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(malo_id = %cand.malo_id, error = %e, "billing-run: idempotency check failed — skipping");
                errors += 1;
                continue;
            }
        }
        match bill_one(cfg, pool, tarifbd, edmd, marktd, vertragd, cand, from, to).await {
            Ok(()) => billed += 1,
            Err(e) => {
                tracing::warn!(malo_id = %cand.malo_id, %from, %to, error = %e, "billing-run: billing failed");
                errors += 1;
            }
        }

        // §40b Abs. 2: monthly Abrechnungsinformation for iMSys MaLos —
        // independent of the invoice cadence.
        if cfg.billing_runs.abrechnungsinformation {
            deliver_abrechnungsinfo(cfg, pool, tarifbd, edmd, marktd, vertragd, cand, today).await;
        }
    }

    let lf = lf_for_log.unwrap_or_else(|| cfg.tenant.clone());
    if let Err(e) = pg::record_billing_run(
        pool,
        &cfg.tenant,
        &lf,
        today.year() as i16,
        u8::from(today.month()) as i16,
        billed,
        errors,
    )
    .await
    {
        tracing::warn!(error = %e, "billing-run: could not record run log");
    }
    tracing::info!(billed, errors, "billing-run: sweep complete");
}

/// Bill one candidate's period through the same pipeline as the HTTP endpoint.
#[allow(clippy::too_many_arguments)]
async fn bill_one(
    cfg: &Arc<BillingdConfig>,
    pool: &PgPool,
    tarifbd: &Arc<TarifbdClient>,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    vertragd: &Arc<VertragdClient>,
    cand: &BillingCandidate,
    from: Date,
    to: Date,
) -> anyhow::Result<()> {
    let req = CalculateRequest {
        lf_mp_id: cand.lf_mp_id.clone(),
        nb_mp_id: cand.nb_mp_id.clone(),
        period_from: from.to_string(),
        period_to: to.to_string(),
        ..Default::default()
    };
    let tariff = handlers::resolve_tariff(&req, tarifbd, &cand.malo_id)
        .await
        .map_err(|(_, msg)| anyhow::anyhow!("tariff: {msg}"))?;
    let rates = cfg.regulatory_rates_for_period(tariff.category_str(), from, to);
    let rechnungsnummer = format!(
        "BILL-{}-{}-{from}",
        cand.malo_id,
        tariff.product_code().unwrap_or(tariff.category_str())
    );
    let invoice = handlers::dispatch_calculator(
        cfg,
        &tariff,
        &req,
        &cand.malo_id,
        &rechnungsnummer,
        from,
        to,
        &rates,
        edmd,
        marktd,
        tarifbd,
        vertragd,
    )
    .await
    .map_err(|(_, msg)| anyhow::anyhow!("dispatch: {msg}"))?;

    let record_id = pg::insert_billing_record(
        pool,
        &cfg.tenant,
        &cand.malo_id,
        &cand.lf_mp_id,
        tariff.product_code().unwrap_or(tariff.category_str()),
        tariff.category_str(),
        from,
        to,
        &invoice.to_rechnung_json(),
        invoice.netto_eur,
        invoice.brutto_eur,
    )
    .await?;

    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        handlers::emit_cloud_event(
            webhook_url,
            cfg.erp_hmac_secret.as_deref(),
            pool,
            record_id,
            &cand.malo_id,
            &cand.lf_mp_id,
            &invoice.to_rechnung_json(),
        )
        .await;
    }
    tracing::info!(malo_id = %cand.malo_id, %from, %to, %record_id, "billing-run: invoice created");
    Ok(())
}

/// §40b Abs. 2 EnWG: deliver the previous month's consumption/cost info for
/// iMSys MaLos — a preview calculation emitted as a CloudEvent, never a
/// persisted invoice.
#[allow(clippy::too_many_arguments)]
async fn deliver_abrechnungsinfo(
    cfg: &Arc<BillingdConfig>,
    pool: &PgPool,
    tarifbd: &Arc<TarifbdClient>,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    vertragd: &Arc<VertragdClient>,
    cand: &BillingCandidate,
    today: Date,
) {
    let Some((from, to)) = due_period("MONATLICH", today, cand.vertragsbeginn)
        .and_then(|p| clip(p, cand.lieferbeginn, cand.lieferende))
    else {
        return;
    };

    // Only fernauslesbare (iMSys) MaLos get the monthly info.
    let is_imsys = matches!(
        edmd.get_billing_period(&cand.malo_id, from, to).await,
        Ok(Some(ref m)) if m.metering_mode == energy_billing::MeteringMode::Imsys
    );
    if !is_imsys {
        return;
    }

    match pg::claim_abrechnungsinfo(
        pool,
        &cfg.tenant,
        &cand.malo_id,
        from.year() as i16,
        u8::from(from.month()) as i16,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return, // already delivered this month
        Err(e) => {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: claim failed");
            return;
        }
    }

    let req = CalculateRequest {
        lf_mp_id: cand.lf_mp_id.clone(),
        nb_mp_id: cand.nb_mp_id.clone(),
        period_from: from.to_string(),
        period_to: to.to_string(),
        ..Default::default()
    };
    let preview = async {
        let tariff = handlers::resolve_tariff(&req, tarifbd, &cand.malo_id)
            .await
            .map_err(|(_, m)| anyhow::anyhow!("tariff: {m}"))?;
        let rates = cfg.regulatory_rates_for_period(tariff.category_str(), from, to);
        handlers::dispatch_calculator(
            cfg,
            &tariff,
            &req,
            &cand.malo_id,
            &format!("INFO-{}-{from}", cand.malo_id),
            from,
            to,
            &rates,
            edmd,
            marktd,
            tarifbd,
            vertragd,
        )
        .await
        .map_err(|(_, m)| anyhow::anyhow!("dispatch: {m}"))
    }
    .await;

    let invoice = match preview {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: preview failed");
            return;
        }
    };

    let Some(ref webhook_url) = cfg.erp_webhook_url else {
        return;
    };
    let ce = serde_json::json!({
        "specversion": "1.0",
        "type": "de.billing.abrechnungsinformation.monatlich",
        "source": format!("urn:billingd:lf:{}", cand.lf_mp_id),
        "id": uuid::Uuid::new_v4().to_string(),
        "time": time::OffsetDateTime::now_utc().to_string(),
        "subject": cand.malo_id,
        "datacontenttype": "application/json",
        "data": {
            "malo_id": cand.malo_id,
            "lf_mp_id": cand.lf_mp_id,
            "period_from": from.to_string(),
            "period_to": to.to_string(),
            "brutto_eur": invoice.brutto_eur,
            "netto_eur": invoice.netto_eur,
            "rechtsgrundlage": "§40b Abs. 2 EnWG",
            "hinweis": "Monatliche Abrechnungsinformation — keine Rechnung",
        }
    });
    let body = serde_json::to_vec(&ce).unwrap_or_default();
    let client = reqwest::Client::new();
    let mut http_req = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .body(body.clone());
    if let Some(secret) = cfg.erp_hmac_secret.as_deref() {
        let sig = mako_markt::cloudevents::compute_signature(secret.as_bytes(), &body);
        http_req = http_req.header("X-Mako-Signature", sig);
    }
    match http_req.send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(malo_id = %cand.malo_id, %from, %to, "abrechnungsinfo: delivered");
        }
        Ok(resp) => {
            tracing::warn!(malo_id = %cand.malo_id, status = %resp.status(), "abrechnungsinfo: webhook failed");
        }
        Err(e) => {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: webhook error");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn monatlich_bills_the_previous_calendar_month() {
        let p = due_period("MONATLICH", date!(2026 - 07 - 19), date!(2024 - 03 - 15)).unwrap();
        assert_eq!(p, (date!(2026 - 06 - 01), date!(2026 - 06 - 30)));
    }

    #[test]
    fn vierteljaehrlich_bills_the_previous_quarter() {
        let p = due_period(
            "VIERTELJAEHRLICH",
            date!(2026 - 07 - 19),
            date!(2024 - 03 - 15),
        )
        .unwrap();
        assert_eq!(p, (date!(2026 - 04 - 01), date!(2026 - 06 - 30)));
        let p2 = due_period(
            "VIERTELJAEHRLICH",
            date!(2026 - 01 - 02),
            date!(2024 - 03 - 15),
        )
        .unwrap();
        assert_eq!(p2, (date!(2025 - 10 - 01), date!(2025 - 12 - 31)));
    }

    #[test]
    fn halbjaehrlich_bills_the_previous_half() {
        let p = due_period(
            "HALBJAEHRLICH",
            date!(2026 - 07 - 19),
            date!(2024 - 03 - 15),
        )
        .unwrap();
        assert_eq!(p, (date!(2026 - 01 - 01), date!(2026 - 06 - 30)));
    }

    #[test]
    fn jaehrlich_bills_the_rolling_year_before_the_anniversary() {
        // Contract began 2024-03-15; today 2026-07-19 → most recent
        // anniversary 2026-03-15 → period [2025-03-15, 2026-03-14].
        let p = due_period("JAEHRLICH", date!(2026 - 07 - 19), date!(2024 - 03 - 15)).unwrap();
        assert_eq!(p, (date!(2025 - 03 - 15), date!(2026 - 03 - 14)));
    }

    #[test]
    fn jaehrlich_first_year_is_not_yet_billable() {
        // Contract began 2026-03-01; first anniversary 2027-03-01 not reached.
        assert!(due_period("JAEHRLICH", date!(2026 - 07 - 19), date!(2026 - 03 - 01)).is_none());
    }

    #[test]
    fn clip_respects_the_supply_window() {
        // Move-in mid-period clips the start.
        assert_eq!(
            clip(
                (date!(2026 - 06 - 01), date!(2026 - 06 - 30)),
                date!(2026 - 06 - 10),
                None
            ),
            Some((date!(2026 - 06 - 10), date!(2026 - 06 - 30)))
        );
        // Supply ended before the period → nothing billable.
        assert_eq!(
            clip(
                (date!(2026 - 06 - 01), date!(2026 - 06 - 30)),
                date!(2026 - 01 - 01),
                Some(date!(2026 - 05 - 31))
            ),
            None
        );
    }
}
