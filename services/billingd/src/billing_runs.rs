//! Scheduled and manual billing use the same issuer-scoped discovery and invoice decisions.
//! The run records its window before reading active and ended supplies from vertragd.
//! A durable refusal keeps an unbilled period visible until its issuer dispatches an eligible invoice.

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

/// One period the run owes an invoice for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Planned {
    from: Date,
    /// Inclusive.
    to: Date,
    /// The period ends with the supply: an Abschlussrechnung (§ 40c Abs. 2
    /// Satz 1 EnWG), billed with `schlussrechnung = true`.
    schluss: bool,
}

/// Whether the candidate's supply is over, so its last period is a
/// Schlussrechnung. vertragd flags it `beendet` once its Ablauf worker ran; a
/// `lieferende` already in the past says the same before it has.
fn supply_ended(cand: &BillingCandidate, today: Date) -> Option<Date> {
    cand.lieferende.filter(|ende| cand.beendet || *ende < today)
}

/// The periods a candidate owes, oldest first.
///
/// Every completed cadence period inside the supply window, and — once supply
/// has ended — the Schlussrechnung from the day after the last billed day
/// (`latest_billed_to`, or the last ordinary period planned here) up to
/// `lieferende`. The cadence period the supply ends in is the Schlussrechnung's,
/// never an ordinary invoice; a supply billed through its end already (billed in
/// advance, or the Schlussrechnung issued) plans none, which is what makes a
/// second run issue nothing.
fn plan_periods(
    cand: &BillingCandidate,
    today: Date,
    catch_up: usize,
    latest_billed_to: Option<Date>,
) -> Vec<Planned> {
    let mut out: Vec<Planned> = due_periods(
        &cand.abrechnungszyklus,
        today,
        cand.vertragsbeginn,
        catch_up,
    )
    .into_iter()
    .rev()
    .filter_map(|p| clip(p, cand.lieferbeginn, cand.lieferende))
    .map(|(from, to)| Planned {
        from,
        to,
        schluss: false,
    })
    .collect();
    let Some(ende) = supply_ended(cand, today) else {
        return out;
    };
    out.retain(|p| p.to < ende);
    let after = out.iter().map(|p| p.to).chain(latest_billed_to).max();
    let from = match after {
        Some(d) => match d.next_day() {
            Some(next) => next.max(cand.lieferbeginn),
            None => return out,
        },
        None => cand.lieferbeginn,
    };
    if from <= ende {
        out.push(Planned {
            from,
            to: ende,
            schluss: true,
        });
    }
    out
}

/// The stable code a failed period is recorded under: the refusal's own code
/// when billing refused it, `INTERNAL` otherwise.
fn reason_of(e: &anyhow::Error) -> &'static str {
    e.downcast_ref::<crate::error::BillingError>()
        .map_or("INTERNAL", crate::error::BillingError::code)
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
        loop {
            let now = time::OffsetDateTime::now_utc();
            let next = next_scheduled_at(&cfg.billing_runs, now).expect("worker is enabled");
            if now.hour() >= cfg.billing_runs.run_hour_utc
                && let Err(error) =
                    sweep(&deps, &pool, mako_fristen::heute(), Some(next), None).await
            {
                tracing::error!(%error, "billing sweep could not finish");
            }
            let wait = (next - time::OffsetDateTime::now_utc())
                .try_into()
                .unwrap_or_default();
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = shutdown.cancelled() => return,
            }
        }
    });
}

/// First configured UTC slot strictly after the supplied instant.
#[must_use]
pub fn next_scheduled_at(
    cfg: &crate::config::BillingRunsConfig,
    now: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    let hour = time::Time::from_hms(cfg.run_hour_utc, 0, 0).ok()?;
    let now = now.to_offset(time::UtcOffset::UTC);
    let slot = now.replace_time(hour);
    cfg.enabled.then_some(if slot > now {
        slot
    } else {
        slot + time::Duration::DAY
    })
}

/// Run the scheduled discovery and invoice decisions for one Berlin business date.
pub async fn sweep(
    deps: &Arc<BillingDeps>,
    pool: &PgPool,
    today: Date,
    next: Option<time::OffsetDateTime>,
    requested: Option<uuid::Uuid>,
) -> anyhow::Result<Option<uuid::Uuid>> {
    let cfg = &deps.cfg;
    let mut attempt =
        match crate::discovery::Attempt::start(pool, &cfg.tenant, today, next, requested).await? {
            crate::discovery::Start::Busy => return Ok(None),
            crate::discovery::Start::Replay(id) => return Ok(Some(id)),
            crate::discovery::Start::New(attempt) => attempt,
        };
    if let Err(error) = pg::purge_settled_unbilled(&mut attempt.conn, &cfg.tenant).await {
        attempt.fail(&cfg.tenant, "evidence_write_failed").await?;
        return Err(error);
    }
    let run_id = attempt.id.to_string();
    let candidates = match deps
        .vertragd
        .get_billing_candidates(attempt.from, attempt.through)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            attempt
                .fail(&cfg.tenant, "candidate_source_unavailable")
                .await?;
            return Err(error);
        }
    };
    let mut incomplete = false;
    tracing::info!(count = candidates.len(), %today, "billing-run: daily sweep");

    // Counters per Lieferant, because `billing_run_log` is keyed per Lieferant.
    // One shared set filed under whichever LF came first would give an operator
    // running two supply licences one LF's month carrying the other's errors,
    // and the other's month empty.
    let mut per_lf: std::collections::HashMap<String, SweepCounters> =
        std::collections::HashMap::new();

    for cand in &candidates {
        let counters = per_lf.entry(cand.lf_mp_id.clone()).or_default();
        counters.candidates += 1;

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
        }

        // Bill every completed period still missing a record — oldest first, so
        // invoices are created in chronological order and a worker that missed a
        // cycle catches up rather than skipping periods (§40c) — and, once supply
        // has ended, the Schlussrechnung after them.
        let latest_billed_to = if supply_ended(cand, today).is_some() {
            match pg::latest_billed_to(pool, &cfg.tenant, &cand.malo_id, &cand.lf_mp_id).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(malo_id = %cand.malo_id, error = %e, "billing-run: last billed day unreadable — skipping");
                    counters.errors += 1;
                    incomplete = true;
                    continue;
                }
            }
        } else {
            None
        };
        let periods = plan_periods(
            cand,
            today,
            cfg.billing_runs.catch_up_periods,
            latest_billed_to,
        );
        for Planned { from, to, schluss } in periods {
            match pg::billing_record_exists_for_period(
                pool,
                &cfg.tenant,
                &cand.malo_id,
                &cand.lf_mp_id,
                from,
                to,
            )
            .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(malo_id = %cand.malo_id, error = %e, "billing-run: idempotency check failed — skipping");
                    counters.errors += 1;
                    incomplete = true;
                    continue;
                }
            }
            let outcome = if refuse_settlement {
                Err(anyhow::Error::from(
                    crate::error::BillingError::unprocessable(
                        "ABSCHLAEGE_UNBEKANNT",
                        "no accountingd_url is configured, so the run has no source for the \
                     paid Abschläge this settlement must deduct (§ 40 Abs. 1 EnWG)",
                    ),
                ))
            } else {
                bill_one(deps, pool, cand, from, to, schluss, &run_id).await
            };
            match outcome {
                Ok(()) => counters.billed += 1,
                Err(e) => {
                    let reason_code = reason_of(&e);
                    if refuse_settlement {
                        counters.skipped += 1;
                    } else {
                        tracing::warn!(malo_id = %cand.malo_id, %from, %to, error = %e, "billing-run: billing failed");
                        counters.errors += 1;
                    }
                    // The § 40c watch: which customer, which period, which
                    // refusal, and the day the invoice is owed by.
                    let due_on = mako_fristen::vorlauf::abrechnung_faellig(
                        to,
                        mako_fristen::vorlauf::Abrechnungsart::aus_zyklus(
                            &cand.abrechnungszyklus,
                            schluss,
                        ),
                    );
                    // A crash before this write is repaired by the next run,
                    // which attempts the same period again and records it then.
                    // committed apart: a refused period emits no event to share a transaction with.
                    if let Err(e) = pg::record_unbilled_period(
                        pool,
                        &pg::NewUnbilledPeriod {
                            tenant: &cfg.tenant,
                            malo_id: &cand.malo_id,
                            lf_mp_id: &cand.lf_mp_id,
                            period_from: from,
                            period_to: to,
                            final_invoice: schluss,
                            reason_code,
                            due_on,
                        },
                    )
                    .await
                    {
                        // A period nobody can see is the failure this watch
                        // exists for, so it fails the month even when the
                        // refusal itself was a deliberate skip.
                        tracing::error!(malo_id = %cand.malo_id, %from, %to, error = %e, "billing-run: the unbilled period could not be recorded");
                        incomplete = true;
                        counters.errors += 1;
                    }
                }
            }
        }

        // § 40b Abs. 3: monthly Abrechnungsinformation for iMSys MaLos — once per
        // candidate per sweep, independent of the invoice cadence.
        if cfg.billing_runs.abrechnungsinformation {
            deliver_abrechnungsinfo(deps, pool, cand, today).await;
        }
    }

    let finish = record_outcome(
        &mut attempt,
        &cfg.tenant,
        today,
        &per_lf,
        candidates.len(),
        incomplete,
    )
    .await;
    if let Err(error) = finish {
        attempt.fail(&cfg.tenant, "evidence_write_failed").await?;
        return Err(error);
    }
    anyhow::ensure!(
        !incomplete,
        "billing discovery evidence could not be recorded"
    );
    tracing::info!(%run_id, "billing sweep finished");
    Ok(Some(attempt.id))
}

async fn record_outcome(
    attempt: &mut crate::discovery::Attempt,
    tenant: &str,
    today: Date,
    results: &std::collections::HashMap<String, SweepCounters>,
    count: usize,
    incomplete: bool,
) -> anyhow::Result<()> {
    use sqlx::Connection as _;
    let mut tx = attempt.conn.begin().await?;
    for (lf, c) in results {
        pg::record_billing_run(
            &mut *tx,
            tenant,
            lf,
            today.year() as i16,
            i16::from(u8::from(today.month())),
            c.billed,
            c.skipped,
            c.errors,
        )
        .await?;
        sqlx::query("INSERT INTO billing_discovery_issuers (tenant, run_id, lf_mp_id, candidates, billed, skipped, errors) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(tenant).bind(attempt.id).bind(lf).bind(c.candidates).bind(c.billed).bind(c.skipped).bind(c.errors).execute(&mut *tx).await?;
    }
    let (status, error, count) = if incomplete {
        ("failed", Some("evidence_write_failed"), None)
    } else {
        ("completed", None, Some(i64::try_from(count)?))
    };
    sqlx::query("UPDATE billing_discovery SET status=$3, error_code=$4, candidate_count=$5, finished_at=clock_timestamp()
        WHERE tenant=$1 AND run_id=$2 AND status='running'").bind(tenant).bind(attempt.id).bind(status).bind(error).bind(count).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// What one Lieferant's share of a sweep did.
#[derive(Debug, Default, Clone, Copy)]
struct SweepCounters {
    candidates: i32,
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
    schluss: bool,
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
            .get_abschlaege(&cand.malo_id, &cand.lf_mp_id, from, to)
            .await
            .map_err(|e| crate::error::BillingError::upstream("accountingd", e))?
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
        // § 40c Abs. 2 Satz 1 EnWG: the period that ends with the supply is
        // settled by the Abschlussrechnung, whose credit § 40c Abs. 3 Satz 2
        // pays out.
        schlussrechnung: schluss,
        abschlaege,
        ..Default::default()
    };
    // The product assignments covering this period, from vertragd — the mapping
    // is a contract fact — each split further at every statutory rate boundary
    // inside it. Every endpoint that bills a period resolves its legs this way,
    // so a price change or a levy Stichtag is billed as the parts it consists of
    // wherever the run started.
    let legs = handlers::resolve_legs(&req, deps, &cand.malo_id, from, to)
        .await
        .map_err(anyhow::Error::from)?;

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
    .map_err(anyhow::Error::from)?;
    let summary = handlers::LegSummary::of(&legs);
    // A public-law Gebühr is issued as a Bescheid beside the supply invoice, in
    // the same run.
    let mut belege = crate::gebuehrenbescheid::trennen(billed.invoice);
    crate::gebuehrenbescheid::eine_lauf_id(&mut belege);
    let bescheid = match belege.gebuehr.take() {
        Some(mut g) => {
            g.context.rechnungsnummer =
                pg::allocate_rechnungsnummer(pool, &cfg.tenant, series::BESCHEID, from.year())
                    .await?;
            let view =
                crate::gebuehrenbescheid::view(&g, cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
            Some((g, view))
        }
        None => None,
    };
    let Some(invoice) = belege.lieferung else {
        let (g, view) =
            bescheid.ok_or_else(|| anyhow::anyhow!("a billed period yielded no document"))?;
        let mut tx = pool.begin().await?;
        let id = crate::gebuehrenbescheid::speichern(
            &mut tx,
            cfg,
            &g,
            &view,
            &summary.product_code,
            &summary.category,
            false,
        )
        .await?;
        tx.commit().await?;
        tracing::info!(malo_id = %cand.malo_id, %from, %to, record_id = %id, "billing-run: Gebührenbescheid created");
        if cfg.billing_runs.versand {
            versenden(pool, deps, &cand.malo_id, &[id]).await;
        }
        return Ok(());
    };

    // Same deterministic risk gate as the on-demand endpoint — scored read-only
    // before the outbox tx, because a HELD band withholds the dispatch enqueue.
    // A split period is scored against the rates of its **last** leg: those are
    // the ones in force when the document is issued, and the ones an anomaly in
    // the total would be measured against.
    let last_leg = legs
        .last()
        .expect("dispatch_invoice_multi rejected an empty period");
    let rates = cfg
        .try_regulatory_rates_for_period(
            last_leg.tariff.category_str(),
            last_leg.from,
            last_leg.to,
            None,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let assessment =
        handlers::assess_risk(pool, cfg, &cand.malo_id, &invoice, &rates, from, to).await;
    let held = assessment
        .as_ref()
        .is_some_and(|a| cfg.risk.hold_dispatch && a.band == crate::risk::RiskBand::Held);

    // Invoice row + its `de.mako.billing.rechnung.erstellt` outbox event commit
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
            steuerzeilen: &crate::steuerzeilen::aus_rechnung(&invoice),
            due_on: Some(handlers::sect40c_due_on(&req, to)),
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
            &cfg.tenant,
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
    crate::einvoice::store(&mut tx, record_id, &invoice, cfg, &cand.malo_id).await?;
    handlers::persist_risk(&mut *tx, record_id, assessment.as_ref()).await?;
    let bescheid_id = match &bescheid {
        Some((g, view)) => Some(
            crate::gebuehrenbescheid::speichern(
                &mut tx,
                cfg,
                g,
                view,
                &summary.product_code,
                &summary.category,
                held,
            )
            .await?,
        ),
        None => None,
    };
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
        let ids: Vec<uuid::Uuid> = std::iter::once(record_id).chain(bescheid_id).collect();
        versenden(pool, deps, &cand.malo_id, &ids).await;
    }
    Ok(())
}

/// Send each issued document of one billed period to the customer — the
/// invoice and, beside it, its Gebührenbescheid — on the same channels and
/// through the same outputd delivery path and retry.
///
/// Not an error for the run when one fails: the document is billed and will
/// not be re-issued. What has not happened is the sending, which
/// `POST /api/v1/billing/{id}/versenden` repeats idempotently. A Bescheid
/// matters as much as the invoice here: its Rechtsbehelfsfrist runs from its
/// Bekanntgabe, so one never sent starts nothing.
pub async fn versenden(pool: &PgPool, deps: &Arc<BillingDeps>, malo_id: &str, ids: &[uuid::Uuid]) {
    for &record_id in ids {
        if let Err(e) = handlers::deliver_record(pool, deps, record_id).await {
            tracing::error!(
                %malo_id, %record_id, error = %e,
                "billing-run: the document was billed but NOT sent to the customer — \
                 POST /api/v1/billing/{{id}}/versenden retries it"
            );
        }
    }
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

    // Without a sink this information cannot leave the service.
    if cfg.erp_webhook_url.is_none() {
        return;
    }
    let (year, month) = (from.year() as i16, i16::from(u8::from(from.month())));
    match pg::abrechnungsinfo_queued(
        pool,
        &cfg.tenant,
        &cand.lf_mp_id,
        &cand.malo_id,
        year,
        month,
    )
    .await
    {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(malo_id = %cand.malo_id, %error, "abrechnungsinfo: reading issued months failed");
            return;
        }
    }

    // Only fernauslesbare (iMSys) MaLos get the monthly info — § 40b Abs. 3
    // attaches the duty to remote read-out, and Abs. 2 covers the rest on a
    // six-month cadence.
    //
    // Said out loud, per MaLo. A silent `return` here is indistinguishable from
    // a MaLo that has the duty and is not being served: the skip is the whole
    // decision, and an edmd that never reports `Imsys` — for a broken
    // classifier, an unreachable service, a period with no reads — makes the
    // duty vanish for every customer with nothing in any log to say so.
    let period = deps.edmd.get_billing_period(&cand.malo_id, from, to).await;
    let reason = match period {
        Ok(Some(ref m)) if m.metering_mode == energy_billing::MeteringMode::Imsys => None,
        Ok(Some(ref m)) => Some(format!("Messtyp is {:?}, not iMSys", m.metering_mode)),
        Ok(None) => Some("edmd holds no billing period for the month".to_owned()),
        Err(ref e) => Some(format!("edmd is not answering: {e}")),
    };
    if let Some(reason) = reason {
        tracing::info!(
            malo_id = %cand.malo_id, %from, %to, %reason,
            "billing-run: no §40b Abs. 3 Abrechnungsinformation — the duty applies only to \
             fernauslesbare Messstellen"
        );
        return;
    }

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
        // The same legs an invoice for this month would be billed from: the
        // § 40b Abs. 3 information states what the month cost, so it has to be
        // priced the way the month will be billed.
        let legs = handlers::resolve_legs(&req, deps, &cand.malo_id, from, to)
            .await
            .map_err(|e| anyhow::anyhow!("tariff: {e}"))?;
        handlers::dispatch_invoice_multi(
            deps,
            &legs,
            &req,
            &cand.malo_id,
            &format!("INFO-{}-{from}", cand.malo_id),
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
            return;
        }
    };

    let ce = mako_service::CloudEvent::new(
        mako_service::source("billingd", &cfg.tenant),
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
    match pg::enqueue_abrechnungsinfo(
        pool,
        &cfg.tenant,
        &cand.lf_mp_id,
        &cand.malo_id,
        year,
        month,
        &ce,
    )
    .await
    {
        Ok(true) => {
            tracing::info!(malo_id = %cand.malo_id, %from, %to, "abrechnungsinfo: enqueued")
        }
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(malo_id = %cand.malo_id, %error, "abrechnungsinfo: enqueue failed")
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

    fn cand(zyklus: &str, lieferende: Option<Date>, beendet: bool) -> BillingCandidate {
        BillingCandidate {
            malo_id: "51238696781".to_owned(),
            lf_mp_id: "9900000000003".to_owned(),
            nb_mp_id: None,
            sparte: "STROM".to_owned(),
            abrechnungszyklus: zyklus.to_owned(),
            vertragsbeginn: date!(2024 - 01 - 01),
            vertragsende: lieferende,
            lieferbeginn: date!(2024 - 01 - 01),
            lieferende,
            beendet,
        }
    }

    /// A household moved out on 15.03. on a monthly contract, February
    /// billed — one run plans the Schlussrechnung 01.03.–15.03., not an
    /// ordinary March invoice.
    #[test]
    fn an_ended_supply_plans_its_schlussrechnung() {
        let c = cand("MONATLICH", Some(date!(2026 - 03 - 15)), true);
        let plan = plan_periods(&c, date!(2026 - 04 - 05), 2, Some(date!(2026 - 02 - 28)));
        assert_eq!(
            plan,
            vec![
                Planned {
                    from: date!(2026 - 02 - 01),
                    to: date!(2026 - 02 - 28),
                    schluss: false
                },
                Planned {
                    from: date!(2026 - 03 - 01),
                    to: date!(2026 - 03 - 15),
                    schluss: true
                },
            ]
        );
    }

    /// Once the Schlussrechnung is written, the next run plans no
    /// second one.
    #[test]
    fn a_second_run_plans_no_second_schlussrechnung() {
        let c = cand("MONATLICH", Some(date!(2026 - 03 - 15)), true);
        let plan = plan_periods(&c, date!(2026 - 04 - 06), 2, Some(date!(2026 - 03 - 15)));
        assert!(plan.iter().all(|p| !p.schluss), "{plan:?}");
    }

    /// A supply ending inside a period already billed in advance: the
    /// Schlussrechnung covers only the days after the last billed one.
    #[test]
    fn a_schlussrechnung_starts_after_the_last_billed_day() {
        let c = cand("JAEHRLICH", Some(date!(2026 - 08 - 20)), true);
        let plan = plan_periods(&c, date!(2026 - 09 - 01), 3, Some(date!(2026 - 07 - 31)));
        assert_eq!(
            plan.last(),
            Some(&Planned {
                from: date!(2026 - 08 - 01),
                to: date!(2026 - 08 - 20),
                schluss: true
            })
        );
        let billed_through =
            plan_periods(&c, date!(2026 - 09 - 01), 3, Some(date!(2026 - 08 - 31)));
        assert!(
            billed_through.iter().all(|p| !p.schluss),
            "billed in advance: nothing left"
        );
    }

    /// Two components ending on different days are two candidates and get one
    /// Schlussrechnung each.
    #[test]
    fn two_components_get_one_schlussrechnung_each() {
        let strom = cand("JAEHRLICH", Some(date!(2026 - 03 - 15)), true);
        let gas = cand("JAEHRLICH", Some(date!(2026 - 03 - 31)), true);
        let today = date!(2026 - 04 - 10);
        let ends: Vec<Date> = [strom, gas]
            .iter()
            .flat_map(|c| plan_periods(c, today, 3, Some(date!(2025 - 12 - 31))))
            .filter(|p| p.schluss)
            .map(|p| p.to)
            .collect();
        assert_eq!(ends, vec![date!(2026 - 03 - 15), date!(2026 - 03 - 31)]);
    }

    /// A `lieferende` already past ends the supply before vertragd's Ablauf
    /// worker flags it — the last period is still a Schlussrechnung.
    #[test]
    fn a_past_lieferende_is_ended_before_the_flag() {
        let c = cand("MONATLICH", Some(date!(2026 - 03 - 31)), false);
        let plan = plan_periods(&c, date!(2026 - 04 - 02), 1, Some(date!(2026 - 02 - 28)));
        assert_eq!(
            plan,
            vec![Planned {
                from: date!(2026 - 03 - 01),
                to: date!(2026 - 03 - 31),
                schluss: true
            }]
        );
    }

    /// A supply still running plans no Schlussrechnung.
    #[test]
    fn a_running_supply_plans_none() {
        let c = cand("MONATLICH", Some(date!(2026 - 12 - 31)), false);
        assert!(
            plan_periods(&c, date!(2026 - 04 - 02), 2, None)
                .iter()
                .all(|p| !p.schluss)
        );
    }

    #[test]
    fn a_refusal_keeps_its_code_and_an_internal_error_its_secret() {
        let refused = anyhow::Error::from(crate::error::BillingError::unprocessable(
            "NO_METER_DATA",
            "no reading",
        ));
        assert_eq!(reason_of(&refused), "NO_METER_DATA");
        let internal = anyhow::anyhow!("connection to 10.0.0.5 refused");
        assert_eq!(reason_of(&internal), "INTERNAL");
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
