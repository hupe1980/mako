//! The one path that settles a plant for a month.
//!
//! A settlement creates a payment obligation to the Anlagenbetreiber, so which
//! entry point triggered it must not change the amount. The REST endpoint, the
//! batch endpoint, the MCP `trigger_settle` tool and the auto-settle worker all
//! call [`settle_plant`] and differ only in what they choose to override.

use std::sync::Arc;

use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::config::EinsdConfig;
use crate::pg::{AnlageRow, Korrektur, SettleResult};

/// What a caller may override for one settlement run.
///
/// Every field left `None` is resolved from the plant, the market-data store or
/// `edmd` — which is the normal case. An explicit value always wins, because a
/// caller that supplies one has a reason the service cannot see.
#[derive(Debug, Default)]
pub struct SettleRequest {
    /// Einspeisemenge kWh. `None` → fetched from `edmd`.
    pub einspeisemenge_kwh: Option<Decimal>,
    /// Market reference ct/kWh. `None` → the stored monthly EPEX average, which
    /// `run_settlement` may still supersede with a Anlage 1 Marktwert.
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// §13a EnWG curtailed kWh for the period.
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51 feed-in during qualifying negative-price intervals. `None` **and**
    /// `negative_price_quarter_hours` `None` → derived from `edmd` × the spot store.
    pub kwh_during_negative_epex: Option<Decimal>,
    /// §51a qualifying quarter-hours.
    pub negative_price_quarter_hours: Option<u64>,
    /// § 147 AO / GoBD — set when this run supersedes an earlier receipt.
    pub correction: Option<Korrektur>,
}

/// Settle one plant for one month and commit the receipt with its CloudEvent.
///
/// Resolves everything the caller did not supply, runs the settlement and the
/// ERP event enqueue in a single transaction, and commits. A failure anywhere
/// rolls the whole thing back: a receipt without its payout event, or an event
/// without its receipt, are both worse than no settlement.
///
/// # Errors
/// Propagates resolution, settlement, enqueue and commit failures.
pub async fn settle_plant(
    pool: &PgPool,
    cfg: &EinsdConfig,
    http: &reqwest::Client,
    anlage: &AnlageRow,
    year: i16,
    month: i16,
    req: SettleRequest,
) -> anyhow::Result<SettleResult> {
    // ── Einspeisemenge ───────────────────────────────────────────────────────
    let einspeisemenge_kwh = match req.einspeisemenge_kwh {
        Some(kwh) => Some(kwh),
        None => {
            crate::handlers::fetch_einspeisemenge_from_edmd(cfg, http, &anlage.malo_id, year, month)
                .await
        }
    };

    // ── Market reference price ───────────────────────────────────────────────
    let epex_avg_ct_kwh = match req.epex_avg_ct_kwh {
        Some(p) => Some(p),
        None => crate::pg::fetch_epex_price(pool, year, month).await?,
    };

    // ── §51 / §51a ───────────────────────────────────────────────────────────
    // Derived only when the caller supplied neither figure: half a §51 input is
    // an instruction, not a gap to fill.
    let (kwh_during_negative_epex, negative_price_quarter_hours) =
        if req.kwh_during_negative_epex.is_none() && req.negative_price_quarter_hours.is_none() {
            crate::handlers::derive_negativpreis_from_edmd(
                cfg,
                http,
                pool,
                &anlage.malo_id,
                anlage.inbetriebnahme,
                year,
                month,
            )
            .await
            .into_overrides()
        } else {
            (
                req.kwh_during_negative_epex,
                req.negative_price_quarter_hours,
            )
        };

    // ── §51 Abs. 3 EEG — the Ausfallvergütung reporting duty ─────────────────
    // An operator on the Ausfallvergütung must report what it fed in while the
    // Spotmarktpreis was continuously negative. Where that quantity could not be
    // established at all, the claim falls 5 % per calendar day such a period
    // touched. A derived figure counts as established — it comes from the NB's
    // own metering, which is what the report would have supplied.
    let sect51_abs3_unreported_days = if anlage.settlement_model == crate::models::AUSFALLVERGUETUNG
        && kwh_during_negative_epex.is_none()
    {
        match crate::handlers::billing_month_range(year, month) {
            Some((from, to)) => crate::pg::negative_price_calendar_days(pool, from, to).await?,
            None => 0,
        }
    } else {
        0
    };

    // §14c UStG: a Gutschrift states the Umsatzsteuer of the party it credits.
    // `eeg_anlagen.einspeiser_id` is NOT NULL behind a foreign key, so this
    // resolves for every plant that exists.
    let einspeiser = crate::pg_einspeiser::find_for_anlage(pool, &cfg.tenant, &anlage.tr_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("plant {} no longer exists", anlage.tr_id))?;

    let mut tx = pool.begin().await?;

    // ── §21 Abs. 1 Satz 1 Nr. 3 — how long the Ausfallvergütung has run ──────
    // Read inside the transaction so the count and the receipt it produces are
    // taken from the same snapshot.
    let ausfallverguetung = if anlage.settlement_model == crate::models::AUSFALLVERGUETUNG {
        crate::pg::ausfallverguetung_nutzung(&mut tx, &anlage.tr_id, &cfg.tenant, year, month)
            .await?
    } else {
        crate::sect52::AusfallverguetungNutzung::default()
    };

    let input = crate::pg::build_settle_input(
        &cfg.tenant,
        anlage,
        &einspeiser,
        year,
        month,
        crate::pg::SettleOverrides {
            einspeisemenge_kwh,
            epex_avg_ct_kwh,
            einspeisemanagement_kwh: req.einspeisemanagement_kwh,
            kwh_during_negative_epex,
            negative_price_quarter_hours,
            correction: req.correction,
            jahresmarktwert_ct_kwh: None,
            sect51_abs3_unreported_days,
            ausfallverguetung,
        },
    )?;

    let result = crate::pg::run_settlement(&mut tx, input).await?;
    crate::handlers::enqueue_settlement_ce(&mut tx, cfg, anlage, &einspeiser, &result, year, month)
        .await?;
    tx.commit().await?;
    Ok(result)
}

/// Settle one plant looked up by `tr_id`.
///
/// Returns `Ok(None)` when the plant does not exist in the tenant.
///
/// # Errors
/// Propagates lookup and settlement failures.
pub async fn settle_by_tr_id(
    pool: &PgPool,
    cfg: &Arc<EinsdConfig>,
    http: &reqwest::Client,
    tr_id: &str,
    year: i16,
    month: i16,
    req: SettleRequest,
) -> anyhow::Result<Option<SettleResult>> {
    let Some(anlage) = crate::pg::fetch_anlage(pool, &cfg.tenant, tr_id).await? else {
        return Ok(None);
    };
    settle_plant(pool, cfg, http, &anlage, year, month, req)
        .await
        .map(Some)
}
