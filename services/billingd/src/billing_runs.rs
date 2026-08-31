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
//!    (§ 40b Abs. 3 EnWG) as a `de.billing.abrechnungsinformation.monatlich`
//!    CloudEvent — a preview calculation, not a persisted invoice, logged in
//!    `abrechnungsinfo_log` so each month is delivered exactly once.

use std::sync::Arc;

use sqlx::PgPool;
use time::Date;

use crate::clients::{BillingCandidate, BillingDeps};
use crate::handlers::{self, CalculateRequest, RunId, series};
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

/// The most-recently-completed billing periods for a cadence, newest first,
/// up to `max_back`.
///
/// `due_period(zyklus, period_start, …)` returns the period ending the day
/// before `period_start`, so stepping the cursor back walks the history. The
/// daily sweep bills every returned period still missing a record — a worker
/// that was down for one or more full cycles catches up the periods it slept
/// through instead of skipping them forever (§40c EnWG deadline). The bound
/// caps catch-up work; `JAEHRLICH` self-limits at `vertragsbeginn`.
fn due_periods(
    zyklus: &str,
    today: Date,
    vertragsbeginn: Date,
    max_back: usize,
) -> Vec<(Date, Date)> {
    let mut out = Vec::new();
    let mut cursor = today;
    for _ in 0..max_back {
        let Some((from, to)) = due_period(zyklus, cursor, vertragsbeginn) else {
            break;
        };
        out.push((from, to));
        cursor = from; // the next-older period ends the day before `from`
    }
    out
}

/// Whether a cadence produces a **settlement** invoice — one that must itemise
/// and deduct the advance payments collected over the period (§40 Abs. 1 EnWG).
///
/// `JAEHRLICH` and every unrecognised cadence, which `due_period` also treats as
/// a rolling year. Sub-annual cadences bill the period itself and collect no
/// advances against it.
fn settles_advances(zyklus: &str) -> bool {
    !matches!(zyklus, "MONATLICH" | "VIERTELJAEHRLICH" | "HALBJAEHRLICH")
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
pub fn spawn_billing_run_worker(
    deps: Arc<BillingDeps>,
    pool: PgPool,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let cfg = Arc::clone(&deps.cfg);
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
            // The trigger is a UTC hour — an operator schedules the run against
            // a clock, not a calendar. What it sweeps *for* is a German business
            // day, so both the once-a-day guard and the date handed to `sweep`
            // are Berlin dates: an invoice must not be dated into the previous
            // month because the worker woke an hour before German midnight.
            let now = time::OffsetDateTime::now_utc();
            let today = mako_fristen::heute();
            if now.hour() < cfg.billing_runs.run_hour_utc || last_sweep == Some(today) {
                continue;
            }
            last_sweep = Some(today);
            sweep(&deps, &pool, today).await;
        }
    });
}

/// One daily sweep: bill everything due, deliver monthly infos, log the run.
async fn sweep(deps: &Arc<BillingDeps>, pool: &PgPool, today: Date) {
    let cfg = &deps.cfg;
    // One id for this sweep. Every invoice it produces carries it as
    // `billingRunId`, so an ERP can ask "did all of last night's run arrive?" —
    // which is what the attribute was documented for and, while it was a fresh
    // UUID per invoice, could not answer.
    let run_id = uuid::Uuid::new_v4().to_string();
    let candidates = match deps.vertragd.get_billing_candidates().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "billing-run: vertragd candidates unavailable — sweep skipped");
            return;
        }
    };
    tracing::info!(count = candidates.len(), %today, "billing-run: daily sweep");

    // Counters per Lieferant, because `billing_run_log` is keyed per Lieferant.
    // One shared set filed under whichever LF came first would give an operator
    // running two supply licences one LF's month carrying the other's errors,
    // and the other's month empty.
    let mut per_lf: std::collections::HashMap<String, SweepCounters> =
        std::collections::HashMap::new();

    for cand in &candidates {
        let counters = per_lf.entry(cand.lf_mp_id.clone()).or_default();

        // §40 Abs. 1 EnWG: a Jahresrechnung itemises and deducts the paid
        // Abschläge, and §14 Abs. 5 Satz 2 UStG makes each one's rate part of
        // the deduction. `bill_one` reads both from the accountingd register.
        // Without one there is no source, so the run would state the whole
        // year's gross as `zuZahlen` — refused rather than emitted; the manual
        // `/calculate` path supplies `abschlaege` and is unaffected.
        let refuse_settlement = settles_advances(&cand.abrechnungszyklus)
            && deps.accountingd.is_none()
            && !cfg.billing_runs.jahresrechnung;
        if refuse_settlement {
            // A deliberate skip, not a fault: counted as errors, it would mark
            // every month `failed` for an operator with annual contracts.
            tracing::info!(
                malo_id = %cand.malo_id,
                zyklus = %cand.abrechnungszyklus,
                "billing-run: skipping annual settlement — no accountingd_url is configured, \
                 so the sweep has no source for the paid Abschläge (§40 Abs. 1 EnWG). Configure \
                 accountingd_url, bill via POST /api/v1/billing/{{malo_id}}/calculate, or set \
                 [billing_runs] jahresrechnung=true to emit without the deduction anyway"
            );
            counters.skipped += 1;
        }

        // Bill every completed period still missing a record — oldest first, so
        // invoices are created in chronological order and a worker that missed a
        // cycle catches up rather than skipping periods (§40c).
        let periods = if refuse_settlement {
            Vec::new()
        } else {
            due_periods(
                &cand.abrechnungszyklus,
                today,
                cand.vertragsbeginn,
                cfg.billing_runs.catch_up_periods,
            )
        };
        for period in periods.into_iter().rev() {
            let Some((from, to)) = clip(period, cand.lieferbeginn, cand.lieferende) else {
                continue;
            };
            match pg::billing_record_exists_for_period(pool, &cfg.tenant, &cand.malo_id, from, to)
                .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(malo_id = %cand.malo_id, error = %e, "billing-run: idempotency check failed — skipping");
                    counters.errors += 1;
                    continue;
                }
            }
            match bill_one(deps, pool, cand, from, to, &run_id).await {
                Ok(()) => counters.billed += 1,
                Err(e) => {
                    tracing::warn!(malo_id = %cand.malo_id, %from, %to, error = %e, "billing-run: billing failed");
                    counters.errors += 1;
                }
            }
        }

        // § 40b Abs. 3: monthly Abrechnungsinformation for iMSys MaLos — once per
        // candidate per sweep, independent of the invoice cadence.
        if cfg.billing_runs.abrechnungsinformation {
            deliver_abrechnungsinfo(deps, pool, cand, today).await;
        }
    }

    let (mut billed, mut skipped, mut errors) = (0, 0, 0);
    for (lf, c) in &per_lf {
        billed += c.billed;
        skipped += c.skipped;
        errors += c.errors;
        if let Err(e) = pg::record_billing_run(
            pool,
            &cfg.tenant,
            lf,
            today.year() as i16,
            i16::from(u8::from(today.month())),
            c.billed,
            c.skipped,
            c.errors,
        )
        .await
        {
            tracing::warn!(lf, error = %e, "billing-run: could not record run log");
        }
    }
    tracing::info!(billed, skipped, errors, %run_id, "billing-run: sweep complete");
}

/// What one Lieferant's share of a sweep did.
#[derive(Debug, Default, Clone, Copy)]
struct SweepCounters {
    billed: i32,
    skipped: i32,
    errors: i32,
}

/// Bill one candidate's period through the same pipeline as the HTTP endpoint.
async fn bill_one(
    deps: &Arc<BillingDeps>,
    pool: &PgPool,
    cand: &BillingCandidate,
    from: Date,
    to: Date,
    run_id: &str,
) -> anyhow::Result<()> {
    let cfg = &deps.cfg;
    // A settling cadence bills the period *and* discharges the advances
    // collected against it (§ 40 Abs. 1 EnWG). They come from the accountingd
    // register, already filtered to the ones § 14 Abs. 5 Satz 2 UStG allows a
    // settlement to deduct.
    //
    // An unreachable accountingd is an error, not an empty list: the year's
    // gross with no Vorauszahlungen looks like an ordinary invoice and demands
    // money the customer already paid.
    let settles = settles_advances(&cand.abrechnungszyklus);
    let abschlaege = match (settles, deps.accountingd.as_ref()) {
        (true, Some(accounting)) => accounting
            .get_abschlaege(&cand.malo_id, from, to)
            .await
            .map_err(|e| anyhow::anyhow!("accountingd abschlaege: {e}"))?
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if settles && !abschlaege.is_empty() {
        tracing::info!(
            malo_id = %cand.malo_id,
            count = abschlaege.len(),
            "billing-run: settling advances on the annual invoice"
        );
    }
    let req = CalculateRequest {
        lf_mp_id: cand.lf_mp_id.clone(),
        nb_mp_id: cand.nb_mp_id.clone(),
        period_from: from.to_string(),
        period_to: to.to_string(),
        // §40b Abs. 1 monthly billing shortens the §40c deadline to three weeks.
        // The cadence comes from the contract, not from this period's length.
        monatliche_abrechnung: cand.abrechnungszyklus.eq_ignore_ascii_case("MONATLICH"),
        abschlaege,
        ..Default::default()
    };
    // The product assignments covering this period, from vertragd — the mapping
    // is a contract fact. A Tarifwechsel inside the period splits it, and each
    // leg is billed under its own product, its own statutory rates and its own
    // meter reading; billing the whole period at whichever tariff happens to be
    // in force on the day the run executes charged the new price for weeks the
    // customer spent on the old one.
    let slices = deps
        .vertragd
        .get_product_slices(&cand.malo_id, from, to)
        .await
        .unwrap_or_default();

    let legs: Vec<handlers::TariffLeg> = if slices.is_empty() {
        // No assignment on file — `resolve_tariff` reports it by name, and the
        // request may still carry an explicit tariff override.
        vec![handlers::TariffLeg {
            tariff: handlers::resolve_tariff(&req, deps, &cand.malo_id, to)
                .await
                .map_err(|e| anyhow::anyhow!("tariff: {e}"))?,
            from,
            to,
            meter: None,
        }]
    } else {
        // One round trip prices every leg: asking productd per leg is an N+1 on
        // every invoice, and two calls could disagree if the catalogue changed
        // between them.
        let anfragen: Vec<(String, Date)> = slices
            .iter()
            .map(|s| (s.product_code.clone(), s.gueltig_von.max(from)))
            .collect();
        let produkte = deps
            .productd
            .resolve_products(&cand.lf_mp_id, &anfragen)
            .await
            .map_err(|e| anyhow::anyhow!("productd resolve: {e}"))?;
        let mut legs = Vec::with_capacity(slices.len());
        for (slice, produkt) in slices.iter().zip(produkte) {
            let tariff = produkt.ok_or_else(|| {
                anyhow::anyhow!(
                    "product {} has no version valid on {}",
                    slice.product_code,
                    slice.gueltig_von.max(from)
                )
            })?;
            legs.push(handlers::TariffLeg {
                tariff,
                from: slice.gueltig_von.max(from),
                to: slice.last_day(to),
                meter: None,
            });
        }
        legs
    };

    // A statutory rate boundary inside the period splits it exactly as a price
    // change does — same mechanism, same merge.
    let legs = handlers::split_on_rate_boundaries(cfg, legs);

    // § 14 Abs. 4 Nr. 4 UStG: from the tenant's `RE` series, keyed on the
    // billed period's year, so a December period swept in January stays in the
    // year it belongs to.
    let rechnungsnummer =
        pg::allocate_rechnungsnummer(pool, &cfg.tenant, series::INVOICE, from.year()).await?;
    let billed = handlers::dispatch_invoice_multi(
        deps,
        &legs,
        &req,
        &cand.malo_id,
        &rechnungsnummer,
        RunId(Some(run_id)),
    )
    .await
    .map_err(|e| anyhow::anyhow!("dispatch: {e}"))?;
    let (invoice, buyer) = (billed.invoice, billed.buyer);
    let summary = handlers::LegSummary::of(&legs);

    // Same deterministic risk gate as the on-demand endpoint — scored read-only
    // before the outbox tx, because a HELD band withholds the dispatch enqueue.
    // A split period is scored against the rates of its **last** leg: those are
    // the ones in force when the document is issued, and the ones an anomaly in
    // the total would be measured against.
    let last_leg = legs
        .last()
        .expect("dispatch_invoice_multi rejected an empty period");
    let rates = cfg
        .try_regulatory_rates_for_period(last_leg.tariff.category_str(), last_leg.from, last_leg.to)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let assessment =
        handlers::assess_risk(pool, cfg, &cand.malo_id, &invoice, &rates, from, to).await;
    let held = assessment
        .as_ref()
        .is_some_and(|a| cfg.risk.hold_dispatch && a.band == crate::risk::RiskBand::Held);

    // Invoice row + its `de.billing.rechnung.erstellt` outbox event commit
    // atomically — a scheduled billing run can never leave a billed period
    // without its ERP event.
    let mut tx = pool.begin().await?;
    let record_id = pg::insert_billing_record(
        &mut *tx,
        &pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &cand.malo_id,
            lf_mp_id: &cand.lf_mp_id,
            product_code: &summary.product_code,
            category: &summary.category,
            rechnungsnummer: &rechnungsnummer,
            period_from: from,
            period_to: to,
            rechnung_json: &invoice.to_rechnung_json(),
            total_netto_eur: invoice.netto_eur,
            total_brutto_eur: invoice.brutto_eur,
        },
    )
    .await?;
    if held {
        tracing::warn!(
            malo_id = %cand.malo_id, %record_id,
            score = assessment.as_ref().map(|a| a.score),
            "billing-run: invoice HELD by risk gate"
        );
    } else {
        let ce = handlers::rechnung_erstellt_ce(
            record_id,
            &cand.malo_id,
            &cand.lf_mp_id,
            &invoice.to_rechnung_json(),
            false,
        );
        handlers::issue_record(&mut tx, cfg, record_id, &ce)
            .await
            .map_err(|e| anyhow::anyhow!("issue: {e}"))?;
    }
    crate::einvoice::store(
        &mut *tx,
        record_id,
        &invoice,
        cfg,
        &cand.malo_id,
        buyer.as_ref(),
    )
    .await?;
    handlers::persist_risk(&mut *tx, record_id, assessment.as_ref()).await?;
    tx.commit().await?;

    tracing::info!(malo_id = %cand.malo_id, %from, %to, %record_id, "billing-run: invoice created");

    // ── Send it ──────────────────────────────────────────────────────────────
    //
    // Outside the transaction: the invoice exists, its receivable is booked and
    // its ERP event is enqueued. Rendering and delivery fail on their own terms
    // — no template rolled out, outputd down — and rolling back a billed period
    // for that would re-bill it under a second Rechnungsnummer.
    //
    // A held invoice is never sent: the risk gate withheld its issuance, so no
    // receivable stands behind it.
    if cfg.billing_runs.versand && !held {
        match handlers::record_with_model(pool, &cfg.tenant, record_id).await {
            Ok((row, model)) => {
                if let Err(e) =
                    handlers::issue_and_deliver(pool, deps, record_id, &row, &model).await
                {
                    // Not an error for the run: the invoice is billed and will
                    // not be re-issued. What has not happened is the sending,
                    // which `POST /api/v1/billing/{id}/versenden` repeats
                    // idempotently.
                    tracing::error!(
                        malo_id = %cand.malo_id, %record_id, error = %e,
                        "billing-run: the invoice was billed but NOT sent to the customer — \
                         POST /api/v1/billing/{{id}}/versenden retries it (§ 40c Abs. 2 EnWG \
                         puts it in their hands within three or six weeks)"
                    );
                }
            }
            Err(e) => {
                tracing::error!(%record_id, error = %e, "billing-run: could not re-read the record to send it");
            }
        }
    }
    Ok(())
}

/// § 40b Abs. 3 EnWG: deliver the previous month's consumption/cost info for
/// iMSys MaLos — a preview calculation emitted as a CloudEvent, never a
/// persisted invoice.
///
/// **Abs. 3, not Abs. 2.** § 40b splits the duty by whether the metering point
/// is *fernauslesbar*: Abs. 2 covers customers **without** remote read-out
/// (every six months, or three on request), Abs. 3 those **with** it — monthly
/// and free. This worker runs for iMSys MaLos, so it is Abs. 3 throughout; the
/// citation said Abs. 2 in nine places including the `rechtsgrundlage` field of
/// the CloudEvent itself.
async fn deliver_abrechnungsinfo(
    deps: &Arc<BillingDeps>,
    pool: &PgPool,
    cand: &BillingCandidate,
    today: Date,
) {
    let cfg = &deps.cfg;
    let Some((from, to)) = due_period("MONATLICH", today, cand.vertragsbeginn)
        .and_then(|p| clip(p, cand.lieferbeginn, cand.lieferende))
    else {
        return;
    };

    // Only fernauslesbare (iMSys) MaLos get the monthly info.
    let is_imsys = matches!(
        deps.edmd.get_billing_period(&cand.malo_id, from, to).await,
        Ok(Some(ref m)) if m.metering_mode == energy_billing::MeteringMode::Imsys
    );
    if !is_imsys {
        return;
    }

    let (year, month) = (from.year() as i16, u8::from(from.month()) as i16);
    match pg::claim_abrechnungsinfo(pool, &cfg.tenant, &cand.malo_id, year, month).await {
        Ok(true) => {}
        Ok(false) => return, // already delivered this month
        Err(e) => {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: claim failed");
            return;
        }
    }

    // From here the claim is held. Every path that does not deliver must give
    // it back: holding a claim whose delivery failed suppresses that month's
    // § 40b Abs. 3 information for good, and the customer's statutory
    // entitlement is not something a transient edmd outage may consume.
    let release = || async {
        if let Err(e) =
            pg::release_abrechnungsinfo_claim(pool, &cfg.tenant, &cand.malo_id, year, month).await
        {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: claim release failed");
        }
    };

    let req = CalculateRequest {
        lf_mp_id: cand.lf_mp_id.clone(),
        nb_mp_id: cand.nb_mp_id.clone(),
        period_from: from.to_string(),
        period_to: to.to_string(),
        // §40b Abs. 1 monthly billing shortens the §40c deadline to three weeks.
        // The cadence comes from the contract, not from this period's length.
        monatliche_abrechnung: cand.abrechnungszyklus.eq_ignore_ascii_case("MONATLICH"),
        ..Default::default()
    };
    let preview = async {
        let tariff = handlers::resolve_tariff(&req, deps, &cand.malo_id, to)
            .await
            .map_err(|e| anyhow::anyhow!("tariff: {e}"))?;
        let rates = cfg
            .try_regulatory_rates_for_period(tariff.category_str(), from, to)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        handlers::dispatch_invoice(
            deps,
            &tariff,
            &req,
            &cand.malo_id,
            &format!("INFO-{}-{from}", cand.malo_id),
            from,
            to,
            &rates,
            RunId::NONE,
        )
        .await
        .map(|b| b.invoice)
        .map_err(|e| anyhow::anyhow!("dispatch: {e}"))
    }
    .await;

    let invoice = match preview {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: preview failed");
            release().await;
            return;
        }
    };

    // Unlike an invoice, the § 40b Abs. 3 information *is* the CloudEvent — there
    // is no record to persist and nothing else delivers it. Without a webhook
    // there is nowhere to send it, so the claim goes back and the month stays
    // open for a sweep that runs once one is configured.
    if cfg.erp_webhook_url.is_none() {
        release().await;
        return;
    }
    let ce = mako_service::CloudEvent::new(
        mako_service::source("billingd", &cand.lf_mp_id),
        mako_events::billing::ABRECHNUNGSINFORMATION_MONATLICH,
        cand.malo_id.clone(),
        serde_json::json!({
            "malo_id": cand.malo_id,
            "lf_mp_id": cand.lf_mp_id,
            "period_from": from.to_string(),
            "period_to": to.to_string(),
            "brutto_eur": invoice.brutto_eur,
            "netto_eur": invoice.netto_eur,
            "rechtsgrundlage": "§ 40b Abs. 3 EnWG",
            "hinweis": "Monatliche Abrechnungsinformation — keine Rechnung",
        }),
    );
    // Through the outbox, like every other event this service emits: the info
    // is a statutory obligation with a monthly deadline, and posting it inline
    // dropped it whenever the ERP happened to be restarting.
    let enqueued = async {
        let mut tx = pool.begin().await?;
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    match enqueued {
        Ok(()) => {
            tracing::info!(malo_id = %cand.malo_id, %from, %to, "abrechnungsinfo: enqueued");
        }
        Err(e) => {
            tracing::warn!(malo_id = %cand.malo_id, error = %e, "abrechnungsinfo: enqueue failed");
            release().await;
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
    fn monatlich_catch_up_walks_back_missed_months() {
        // A worker down for three months still sees June, May and April, newest
        // first — the sweep bills every one still missing a record.
        let ps = due_periods(
            "MONATLICH",
            date!(2026 - 07 - 19),
            date!(2024 - 03 - 15),
            13,
        );
        assert_eq!(ps.len(), 13);
        assert_eq!(ps[0], (date!(2026 - 06 - 01), date!(2026 - 06 - 30)));
        assert_eq!(ps[1], (date!(2026 - 05 - 01), date!(2026 - 05 - 31)));
        assert_eq!(ps[2], (date!(2026 - 04 - 01), date!(2026 - 04 - 30)));
        // Contiguous, no gaps or overlaps across the year boundary.
        for w in ps.windows(2) {
            assert_eq!(w[1].1.next_day().unwrap(), w[0].0, "periods are contiguous");
        }
    }

    #[test]
    fn jaehrlich_catch_up_self_limits_at_vertragsbeginn() {
        // Contract began 2024-03-15, today 2026-07-19 → two anniversaries passed,
        // so both completed years are billable; catch-up stops at the contract
        // start rather than inventing history before it.
        let ps = due_periods(
            "JAEHRLICH",
            date!(2026 - 07 - 19),
            date!(2024 - 03 - 15),
            13,
        );
        assert_eq!(ps.len(), 2, "two full anniversary years have completed");
        assert_eq!(ps[0], (date!(2025 - 03 - 15), date!(2026 - 03 - 14)));
        assert_eq!(ps[1], (date!(2024 - 03 - 15), date!(2025 - 03 - 14)));
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
