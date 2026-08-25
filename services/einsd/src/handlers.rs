//! HTTP handlers for `einsd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_service::cedar::CedarEnforcer;
use mako_service::error::ApiError;
use mako_service::oidc::Claims;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use time::{Date, OffsetDateTime};

use crate::{
    config::EinsdConfig,
    pg::{
        AnlageUpsertRequest, AnlagenQuery, decommission_anlage, fetch_anlage, fetch_epex_price,
        fetch_jahresmarktwert_single, list_anlagen, list_expiring, list_settlement_receipts,
        list_unsettled, lookup_verguetungssatz, upsert_anlage, upsert_epex_price,
        upsert_jahresmarktwert, zusammenlegen,
    },
};

// ── edmd auto-fetch helper ────────────────────────────────────────────────────

/// Fetch the month's **Einspeisemenge** from `edmd` for a MaLo.
///
/// Calls `GET {edmd_url}/api/v1/energy/{malo_id}?direction=EINSPEISUNG` over the
/// billing month in German local time and sums the projected intervals.
///
/// # Why not `/billing-period`
///
/// It used to read `arbeitsmenge_kwh` off
/// `GET /api/v1/billing-period/{malo_id}` — and that field is the **Bezug**, the
/// grid draw, projected onto the consumption registers (edmd's
/// `domain::register`). An Erzeugungs-MaLo reports only `1-0:2.8.x`, so the
/// consumption projection over it is empty and the figure came back as **0 kWh**:
/// every auto-settled EEG month paid on nothing, and the batch dry-run counted
/// the plant as "has data" because `Some(0)` is not `None`.
///
/// The direction has to be stated, and only `/energy` lets it be.
///
/// Returns `None` when `edmd_url` is not configured, the read fails, or the MaLo
/// reports no feed-in intervals at all — a plant with no metered feed-in is not a
/// plant that fed in nothing.
pub async fn fetch_einspeisemenge_from_edmd(
    cfg: &EinsdConfig,
    client: &reqwest::Client,
    malo_id: &str,
    year: i16,
    month: i16,
) -> Option<Decimal> {
    let edmd = cfg.edmd(client.clone())?;
    // The billing month in German local time — the same window the §51 overlay
    // uses. Taking it from midnight UTC shifts the boundary by an hour (two at
    // DST), moving an hour of feed-in into the neighbouring month.
    let (from, to) = billing_month_range(year, month)?;
    use time::format_description::well_known::Rfc3339;
    let (Ok(from_s), Ok(to_s)) = (from.format(&Rfc3339), to.format(&Rfc3339)) else {
        return None;
    };
    let path = format!("/api/v1/energy/{malo_id}");
    let request = edmd.get(&path).query(&[
        ("from", from_s),
        ("to", to_s),
        ("direction", "EINSPEISUNG".to_owned()),
    ]);
    let feed = match edmd.json::<FeedInResponse>(request).await {
        Ok(Some(feed)) => feed,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(
                malo_id, year, month, error = %e,
                "einsd: edmd Einspeisemenge could not be read — settling on the \
                 caller-supplied quantity only"
            );
            return None;
        }
    };
    if feed.intervals.is_empty() {
        return None;
    }
    Some(
        feed.intervals
            .iter()
            .filter_map(|iv| iv.kwh.parse::<Decimal>().ok())
            .sum(),
    )
}

#[derive(serde::Deserialize)]
struct FeedInResponse {
    coverage_pct: f64,
    /// Share of the Einspeisung series that is billable at all, by duration.
    ///
    /// `Option`, and that is the fix: edmd documented this field and did not
    /// emit it, so `serde` failed the whole response on a missing key, the
    /// `Ok(Some(_))` arm never matched, and **every** § 51 auto-derivation
    /// answered `Negativpreis::Unbekannt` — silently, because a failed
    /// derivation and an inapplicable one looked identical from here. `None`
    /// now means "the point reports no Einspeisung register", which is a
    /// different fact from 0 %.
    billable_pct: Option<f64>,
    intervals: Vec<FeedInInterval>,
}

#[derive(serde::Deserialize)]
struct FeedInInterval {
    start: String,
    kwh: String,
}

/// The billing month as a half-open instant range in **German local time**.
///
/// The §51 overlay matches feed-in quarter-hours against day-ahead prices by
/// their start instant, and both series are published for the German market
/// time (CET/CEST). Taking the month from midnight *UTC* shifted the window by
/// an hour — two at DST — so the first hour of the month was read from the
/// previous month's prices and the last hour was dropped. On a negative-price
/// night that is a whole hour of feed-in either paid or not paid in error.
pub fn billing_month_range(year: i16, month: i16) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let berlin = time_tz::timezones::db::europe::BERLIN;
    let start_date = Date::from_calendar_date(
        i32::from(year),
        time::Month::try_from(u8::try_from(month).ok()?).ok()?,
        1,
    )
    .ok()?;
    // First day of the following month — a half-open upper bound needs no
    // knowledge of the month's length or of a leap year.
    let end_date = if month == 12 {
        Date::from_calendar_date(i32::from(year) + 1, time::Month::January, 1).ok()?
    } else {
        Date::from_calendar_date(
            i32::from(year),
            time::Month::try_from(u8::try_from(month).ok()? + 1).ok()?,
            1,
        )
        .ok()?
    };
    // Midnight Berlin is never inside a DST gap (transitions happen at 02:00/03:00),
    // so the ambiguous/none arms are unreachable; they are handled rather than
    // unwrapped because a month boundary that panics takes the whole batch down.
    let to_instant = |d: Date| -> Option<OffsetDateTime> {
        use time_tz::{OffsetResult, PrimitiveDateTimeExt as _};
        let midnight = time::PrimitiveDateTime::new(d, time::Time::MIDNIGHT);
        match midnight.assume_timezone(berlin) {
            OffsetResult::Some(dt) => Some(dt.to_offset(time::UtcOffset::UTC)),
            OffsetResult::Ambiguous(earlier, _) => Some(earlier.to_offset(time::UtcOffset::UTC)),
            OffsetResult::None => None,
        }
    };
    Some((to_instant(start_date)?, to_instant(end_date)?))
}

/// §51 auto-derivation: fetch ¼h feed-in from edmd, overlay the stored EPEX spot
/// prices, and derive `(kwh_during_negative_epex, negative_price_quarter_hours)`
/// via the date-aware `eeg-billing::negativpreis` engine.
///
/// Returns [`Negativpreis::Unbekannt`] — leaving the two figures caller-supplied
/// — when edmd is not configured, there is no feed-in or spot data, or the
/// metering coverage / quality is below the §60 Abs. 2 MsbG threshold. Deriving
/// on an incomplete or partly-faulty month would find too few negative-price kWh
/// and thus *overpay*, so a gap is surfaced (logged) rather than silently
/// under-reduced.
///
/// A month that genuinely had no qualifying negative quarter-hour returns
/// [`Negativpreis::Ermittelt`] with zeroes. Collapsing that into "unknown" left
/// the settlement carrying `None`, which reads downstream as "no data supplied"
/// — indistinguishable from a failed lookup in the audit trail.
pub async fn derive_negativpreis_from_edmd(
    cfg: &EinsdConfig,
    client: &reqwest::Client,
    pool: &PgPool,
    malo_id: &str,
    inbetriebnahme: time::Date,
    year: i16,
    month: i16,
) -> Negativpreis {
    use time::format_description::well_known::Rfc3339;

    let Some(edmd) = cfg.edmd(client.clone()) else {
        return Negativpreis::Unbekannt;
    };
    let regime = eeg_billing::NegativpreisRegime::fuer_inbetriebnahme(inbetriebnahme);
    let Some((range_from, range_to)) = billing_month_range(year, month) else {
        return Negativpreis::Unbekannt;
    };

    let (Ok(from_s), Ok(to_s)) = (range_from.format(&Rfc3339), range_to.format(&Rfc3339)) else {
        return Negativpreis::Unbekannt;
    };
    // edmd's canonical projected series: the Einspeisung registers only, through
    // `domain::register`, with `billable_pct` measured over the direction's whole
    // series before the projection filtered the non-billable readings out.
    let path = format!("/api/v1/energy/{malo_id}");
    let request = edmd.get(&path).query(&[
        ("from", from_s),
        ("to", to_s),
        ("direction", "EINSPEISUNG".to_owned()),
    ]);
    let feed = match edmd.json::<FeedInResponse>(request).await {
        Ok(Some(feed)) => feed,
        // A read that failed is not "no negative-price hours". Answering
        // `Unbekannt` is right either way, but it has to be visible: this gate
        // decides whether a § 51 EEG reduction is applied at all, and it spent
        // its whole life silently failing on a response field edmd documented
        // and never emitted (`billable_pct`).
        Ok(None) => {
            tracing::warn!(
                malo_id,
                "§51 auto-derivation skipped — edmd holds no Einspeisung series for the period"
            );
            return Negativpreis::Unbekannt;
        }
        Err(e) => {
            tracing::warn!(
                malo_id, error = %e,
                "§51 auto-derivation skipped — edmd's energy series could not be read"
            );
            return Negativpreis::Unbekannt;
        }
    };

    // §60 Abs. 2 gate: auto-derive only on near-complete, fully-billable data.
    // `billable_pct` is absent when the MaLo reports no Einspeisung register at
    // all — which is not "0 % billable", it is nothing to judge, and it must not
    // pass a gate that exists to refuse incomplete data.
    let billable_pct = feed.billable_pct.unwrap_or(0.0);
    if feed.coverage_pct < 95.0 || billable_pct < 100.0 {
        tracing::warn!(
            malo_id,
            coverage = feed.coverage_pct,
            billable = billable_pct,
            "§51 auto-derivation skipped — metering coverage/quality below threshold; \
             supply kwh_during_negative_epex manually or backfill substitute values"
        );
        return Negativpreis::Unbekannt;
    }

    let spot = match crate::pg::fetch_spot_prices(pool, range_from, range_to).await {
        Ok(spot) => spot,
        Err(e) => {
            tracing::warn!(
                malo_id, year, month, error = %e,
                "§51 auto-derivation skipped — the EPEX spot store could not be read"
            );
            return Negativpreis::Unbekannt;
        }
    };
    if spot.is_empty() {
        // Without prices no quarter-hour can be negative, so the plant is paid
        // in full for a month §51 may well have excluded. That is an operator
        // problem (import the day-ahead curve), not a quiet skip.
        tracing::warn!(
            malo_id,
            year,
            month,
            "§51 auto-derivation skipped — the EPEX spot store has no coverage for the \
             period; import the day-ahead prices or supply kwh_during_negative_epex"
        );
        return Negativpreis::Unbekannt;
    }
    let negative_starts: std::collections::HashSet<OffsetDateTime> = spot
        .iter()
        .filter(|(_, p)| p.is_sign_negative())
        .map(|(t, _)| *t)
        .collect();

    let mut intervals: Vec<eeg_billing::NegativpreisInterval> = feed
        .intervals
        .iter()
        .filter_map(|iv| {
            let start = OffsetDateTime::parse(&iv.start, &Rfc3339).ok()?;
            let feed_in_kwh = iv.kwh.parse::<Decimal>().ok()?;
            Some(eeg_billing::NegativpreisInterval {
                start,
                feed_in_kwh,
                price_negative: negative_starts.contains(&start),
            })
        })
        .collect();
    // The run detector needs ascending order to recognise a consecutive run;
    // edmd sorts its series, but a §51 threshold that silently stops applying
    // because an upstream sort changed is not a failure mode worth keeping.
    intervals.sort_unstable_by_key(|iv| iv.start);

    let r = eeg_billing::derive_negativpreis(&intervals, regime);
    Negativpreis::Ermittelt {
        kwh: r.kwh_during_negative,
        quarter_hours: r.negative_quarter_hours,
    }
}

/// What the §51 auto-derivation could establish for a billing period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Negativpreis {
    /// The overlay ran. Zero values mean the month genuinely had no qualifying
    /// negative quarter-hour, not that nothing was known.
    Ermittelt {
        /// Feed-in kWh in qualifying negative-price intervals (§51).
        kwh: Decimal,
        /// Count of those quarter-hours (§51a).
        quarter_hours: u64,
    },
    /// Nothing could be established — no edmd, no prices, or metering below the
    /// §60 Abs. 2 MsbG threshold. The settlement is left unreduced and the
    /// receipt records no §51 figures.
    Unbekannt,
}

impl Negativpreis {
    /// Split into the two override fields, `(None, None)` when unknown.
    #[must_use]
    pub fn into_overrides(self) -> (Option<Decimal>, Option<u64>) {
        match self {
            Self::Ermittelt { kwh, quarter_hours } => (Some(kwh), Some(quarter_hours)),
            Self::Unbekannt => (None, None),
        }
    }
}

// A hand-rolled `days_in_month` used to stand here, for the calendar-date bounds
// the `/billing-period` fetch needed. Both are gone: the Einspeisemenge is read
// over an instant range from `billing_month_range`, which takes the month in
// German local time through the `time` crate's own calendar — the one that
// already knows about leap years and does not silently answer 28 for a month
// number outside 1..=12.

// ── CloudEvent emission ───────────────────────────────────────────────────────

/// Build a settlement CloudEvent for the transactional outbox.
///
/// Returns `Some(CloudEvent)` when the ERP webhook is configured (the event must
/// then be `enqueue`d **inside the same transaction as the settlement write**, so
/// it commits atomically and a background [`mako_service::outbox::OutboxWorker`]
/// delivers it at-least-once). Returns `None` when no webhook is configured, so
/// the caller skips the enqueue entirely.
///
/// CE types emitted:
/// - `de.eeg.verguetung.berechnet` — VERGUETUNG, MIETERSTROM, POST_EEG_SPOT,
///   EIGENVERBRAUCH, KWKG_ZUSCHLAG, FLEXIBILITAET
/// - `de.eeg.marktpraemie.berechnet` — DIREKTVERMARKTUNG, AUSSCHREIBUNG
///
/// `bank_iban` and `bank_bic` are included when present so `accountingd` can
/// generate a SEPA Credit Transfer pain.001 without a secondary DB lookup.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn build_settlement_ce(
    cfg: &EinsdConfig,
    ce_type: &str,
    tr_id: &str,
    malo_id: &str,
    result: &crate::pg::SettleResult,
    year: i16,
    month: i16,
    bank_iban: Option<&str>,
    bank_bic: Option<&str>,
    zahlungsempfaenger: Option<&str>,
) -> Option<mako_service::CloudEvent> {
    // Gate on webhook configuration — no ERP endpoint means nothing to enqueue.
    cfg.erp_webhook_url.as_deref()?;
    let ce_id = uuid::Uuid::new_v4();

    let ce = mako_service::CloudEvent::new(
        mako_service::source("einsd", &cfg.tenant),
        ce_type,
        tr_id,
        serde_json::json!({
            "tr_id": tr_id,
            "malo_id": malo_id,
            "billing_year": year,
            "billing_month": month,
            "settlement_model": result.settlement_model,
            "einspeisemenge_kwh": result.einspeisemenge_kwh,
            "settlement_eur": result.settlement_eur,
            "status": result.status,
            // §14 UStG Gutschrift document facts — the settlement_eur is the net;
            // downstream (accountingd) books the net credit but now references the
            // issued document (number + USt + brutto), not just an amount.
            "gutschrift_nummer": result.gutschrift_nummer,
            "gutschrift_steuer_eur": result.gutschrift_steuer_eur,
            "gutschrift_brutto_eur": result.gutschrift_brutto_eur,
            // Bank routing fields — enables accountingd SCT Inst auto-payout
            // without a secondary DB lookup. Absent for EIGENVERBRAUCH (no payout).
            "bank_iban": bank_iban,
            "bank_bic": bank_bic,
            "zahlungsempfaenger": zahlungsempfaenger,
        }),
    )
    .with_id(ce_id.to_string());

    Some(ce)
}

/// Enqueue the settlement ERP CloudEvent into the transactional outbox — the one
/// place that decides the event **type** and the emission **gate**.
///
/// accountingd dispatches on the exact CE type, crediting the Massenkontokorrent
/// and issuing the pain.001 payout only for `de.eeg.verguetung.berechnet` and
/// `de.eeg.marktpraemie.berechnet`. The REST and MCP settle paths both call this
/// so neither can drift to a type accountingd ignores (a dead-letter payout) or
/// emit a payout event for a non-`calculated` run.
///
/// # Errors
/// Propagates the outbox insert error so the caller can roll back the settlement.
pub async fn enqueue_settlement_ce(
    tx: &mut sqlx::PgConnection,
    cfg: &EinsdConfig,
    anlage: &crate::pg::AnlageRow,
    einspeiser: &crate::pg_einspeiser::Einspeiser,
    result: &crate::pg::SettleResult,
    year: i16,
    month: i16,
) -> Result<(), sqlx::Error> {
    // Only a completed calculation credits a ledger / triggers a payout.
    if result.status != "calculated" {
        return Ok(());
    }
    // de.eeg.marktpraemie.berechnet — the gleitende/wettbewerbliche Marktprämie
    // de.eeg.verguetung.berechnet   — everything else (FiT, Mieterstrom, Post-EEG, Flex, KWKG)
    let ce_type = if crate::models::ist_marktpraemie(&anlage.settlement_model) {
        mako_events::eeg::MARKTPRAEMIE_BERECHNET
    } else {
        mako_events::eeg::VERGUETUNG_BERECHNET
    };
    if let Some(ce) = build_settlement_ce(
        cfg,
        ce_type,
        &result.tr_id,
        &anlage.malo_id,
        result,
        year,
        month,
        einspeiser.bank_iban.as_deref(),
        einspeiser.bank_bic.as_deref(),
        einspeiser.zahlungsempfaenger.as_deref(),
    ) {
        mako_service::outbox::enqueue(tx, &ce).await?;
    }
    Ok(())
}

/// Emit `de.eeg.anlage.foerderung-auslaufend` for a plant about to expire.
pub async fn emit_foerderung_alert_ce(
    cfg: &EinsdConfig,
    client: &reqwest::Client,
    tr_id: &str,
    malo_id: &str,
    foerderendedatum: time::Date,
    days_remaining: i64,
) {
    let Some(webhook_url) = cfg.erp_webhook_url.as_deref() else {
        return;
    };

    let ce = mako_service::CloudEvent::new(
        mako_service::source("einsd", &cfg.tenant),
        mako_events::eeg::ANLAGE_FOERDERUNG_AUSLAUFEND,
        tr_id,
        serde_json::json!({
            "tr_id": tr_id,
            "malo_id": malo_id,
            "foerderendedatum": foerderendedatum.to_string(),
            "days_remaining": days_remaining,
        }),
    );

    let secret = cfg.erp_hmac_secret.as_deref().map(str::as_bytes);
    if let Err(e) = mako_service::post_ce_with_retry(client, webhook_url, &ce, secret).await {
        tracing::warn!(tr_id, error = %e, "einsd: förderung alert delivery failed");
    }
}

/// Emit `de.eeg.settlement.batch-due` when a monthly settlement batch is due.
///
/// The auto-settle worker fires this at the start of its monthly run so the
/// `einsd-batch-agent` in `agentd` can run its §52 sweep / review over the same
/// batch. Best-effort (an orchestration signal, not a payout).
pub async fn emit_batch_due_ce(
    cfg: &EinsdConfig,
    client: &reqwest::Client,
    billing_year: i16,
    billing_month: i16,
    unsettled_count: usize,
) {
    let Some(webhook_url) = cfg.erp_webhook_url.as_deref() else {
        return;
    };
    let ce = mako_service::CloudEvent::new(
        mako_service::source("einsd", &cfg.tenant),
        mako_events::eeg::SETTLEMENT_BATCH_DUE,
        format!("{billing_year:04}-{billing_month:02}"),
        serde_json::json!({
            "billing_year": billing_year,
            "billing_month": billing_month,
            "unsettled_plants": unsettled_count,
        }),
    );
    let secret = cfg.erp_hmac_secret.as_deref().map(str::as_bytes);
    if let Err(e) = mako_service::post_ce_with_retry(client, webhook_url, &ce, secret).await {
        tracing::warn!(error = %e, "einsd: batch-due signal delivery failed");
    }
}

// ── EEG Anlage CRUD ───────────────────────────────────────────────────────────

/// `POST /api/v1/anlagen`  — Register or replace a plant.
pub async fn post_anlage(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Json(req): Json<AnlageUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match upsert_anlage(&pool, &cfg.tenant, req).await {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// `PUT /api/v1/anlagen/{tr_id}`  — Update an existing plant.
pub async fn put_anlage(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(mut req): Json<AnlageUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    req.tr_id = tr_id;
    match upsert_anlage(&pool, &cfg.tenant, req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/v1/anlagen/{tr_id}`
pub async fn get_anlage(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform`
///
/// The plant register's answer to the one question `E_0622` Prüfschritte
/// 400–830 ask that the UTILMD cannot: **which Veräußerungsform is in force**
/// at a Marktlokation today.
///
/// `processd` needs it to choose between the six Vorlauffristen GPKE Teil 2
/// § 2.1.1 publishes for an Anmeldung erzeugender Marktlokation. Two facts come
/// back, because the wire cannot carry the second:
///
/// - `veraeusserungsform` — the UTILMD `SG10 CCI+Z22` DE 7037 code.
/// - `ausfallverguetung` — `Z90` covers both the uneingeschränkte
///   Einspeisevergütung (§ 21 Abs. 1 Nr. 1 EEG 2023) and the Ausfallvergütung
///   (Nr. 2), and the two take *different* Fristen: a month versus the verkürzte
///   fünf Werktage. Only the register separates them.
///
/// `404` means the Marktlokation is not in this NB's EEG-/KWKG-Register. That is
/// **not** the same as „Nicht-EEG-Marktlokation" — it may equally be a plant
/// nobody has registered yet — so `processd` escalates rather than assuming.
pub async fn get_veraeusserungsform_by_malo(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let row = sqlx::query(
        r"SELECT tr_id, settlement_model
          FROM eeg_anlagen
          WHERE malo_id = $1 AND tenant = $2
          ORDER BY inbetriebnahme DESC
          LIMIT 1",
    )
    .bind(&malo_id)
    .bind(&cfg.tenant)
    .fetch_optional(&pool)
    .await;

    match row {
        Ok(Some(r)) => {
            use sqlx::Row as _;
            let tr_id: String = r.get("tr_id");
            let model: String = r.get("settlement_model");
            match veraeusserungsform_of(&model) {
                Some(form) => Json(serde_json::json!({
                    "malo_id":            malo_id,
                    "tr_id":              tr_id,
                    "settlement_model":   model,
                    "veraeusserungsform": form,
                    "ausfallverguetung":  model == "AUSFALLVERGUETUNG",
                }))
                .into_response(),
                // Mieterstrom, GGV, Eigenverbrauch, Post-EEG and the
                // Flexibilitäts-models are settlement models, not
                // Veräußerungsformen — `CCI+Z22` has no code for them.
                None => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "NO_VERAEUSSERUNGSFORM",
                        "message": format!(
                            "settlement_model {model:?} is not a Veräußerungsform in the \
                             CCI+Z22 sense"
                        ),
                        "settlement_model": model,
                    })),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "NOT_REGISTERED",
                "message": "no plant with that MaLo-ID in this EEG-/KWKG-Register",
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// Map a settlement model onto its UTILMD `SG10 CCI+Z22` DE 7037 code.
///
/// `AUSSCHREIBUNG` is the wettbewerblich ermittelte Marktprämie (§ 22 EEG 2023)
/// — still a Marktprämie on the wire; the auction only sets the anzulegender
/// Wert. `Z90` is deliberately shared by `VERGUETUNG` and `AUSFALLVERGUETUNG`:
/// the MIG has one code for both, which is why the caller also gets the flag.
#[must_use]
pub fn veraeusserungsform_of(settlement_model: &str) -> Option<&'static str> {
    match settlement_model {
        "VERGUETUNG" | "AUSFALLVERGUETUNG" => Some("Z90"),
        "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" => Some("Z91"),
        "SONSTIGE_DIREKTVERMARKTUNG" => Some("Z92"),
        "KWKG_ZUSCHLAG" => Some("Z94"),
        _ => None,
    }
}

/// `GET /api/v1/anlagen`  — List plants with optional filters.
pub async fn get_anlagen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Query(q): Query<AnlagenQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match list_anlagen(&pool, &cfg.tenant, &q).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `DELETE /api/v1/anlagen/{tr_id}`  — Decommission (set status = 'abgemeldet').
pub async fn delete_anlage(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match decommission_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

// ── 180-day expiry alert ──────────────────────────────────────────────────────

/// `GET /api/v1/anlagen/foerderung-auslaufend`
///
/// Returns plants whose `foerderendedatum` is within 180 days of today.
/// Used by the background alert worker and ERP dashboards.
pub async fn get_foerderung_auslaufend(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Query(q): Query<HorizonQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let days = q.days.unwrap_or(180);
    match list_expiring(&pool, &cfg.tenant, days).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct HorizonQuery {
    /// Look-ahead window in days (default: 180).
    pub days: Option<i32>,
}

// ── Settlement ────────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}`.
#[derive(Debug, Deserialize)]
pub struct SettleTriggerRequest {
    /// Einspeisemenge kWh for the billing month.
    /// When absent, `einsd` will return `status = "no_data"`.
    pub einspeisemenge_kwh: Option<Decimal>,
    /// Override EPEX monthly average ct/kWh (only for DIREKTVERMARKTUNG /
    /// POST_EEG_SPOT).  When absent, the value stored in `epex_monthly_prices`
    /// is used automatically.
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// §13a EnWG (Redispatch 2.0) — kWh curtailed by the NB this billing month.
    ///
    /// The NB must compensate the operator at the AW rate for these kWh
    /// (§19 Abs. 2 EEG 2023: §51 Negativpreisregel does NOT apply to EInsMan kWh).
    /// Pass the total curtailed kWh from MSCONS IFTSTA messages in this period.
    #[serde(default)]
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51 EEG 2023 — kWh fed in during negative-spot-price intervals.
    ///
    /// The anzulegender Wert for these kWh is reduced to null (Negativpreisregel),
    /// version- and threshold-aware in the engine (EEG 2017 ≥6 h / EEG 2021 ≥4 h /
    /// EEG 2023 any interval; kW-exemptions applied). Pass the feed-in quantity
    /// that fell in negative-price intervals for the billing month; omit (or the
    /// §51 exemption applies) to leave the settlement unreduced. Consistent with
    /// `negative_price_quarter_hours` (§51a), which extends the Förderzeitraum for
    /// the same intervals.
    #[serde(default)]
    pub kwh_during_negative_epex: Option<Decimal>,
    /// §51a EEG 2023 — quarter-hours during which the EPEX price was negative
    /// AND the plant's §51 threshold was met.
    ///
    /// Used to compute the §51a Verlängerungsanspruch. Non-solar plants extend by
    /// whole calendar days (96 QH/day, rounded up once over the total); solar
    /// plants convert at factor 0,5 into Volllastviertelstunden and draw them
    /// down against the §51a Abs. 2 monthly table (73 in December, 508 in June).
    /// Pass the raw qualifying quarter-hour count for the billing month.
    #[serde(default)]
    pub negative_price_quarter_hours: Option<u64>,
}

/// `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}`
///
/// Trigger monthly EEG settlement for one plant.  Idempotent — re-running
/// overwrites the previous result for the same (tr_id, year, month).
pub async fn post_settle(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path((tr_id, year, month)): Path<(String, i16, i16)>,
    Json(req): Json<SettleTriggerRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "run-settlement", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match crate::settle::settle_by_tr_id(
        &pool,
        &cfg,
        &http_client,
        &tr_id,
        year,
        month,
        crate::settle::SettleRequest {
            einspeisemenge_kwh: req.einspeisemenge_kwh,
            epex_avg_ct_kwh: req.epex_avg_ct_kwh,
            einspeisemanagement_kwh: req.einspeisemanagement_kwh,
            kwh_during_negative_epex: req.kwh_during_negative_epex,
            negative_price_quarter_hours: req.negative_price_quarter_hours,
            correction: None,
        },
    )
    .await
    {
        Ok(Some(result)) => (StatusCode::OK, Json(result)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/v1/anlagen/{tr_id}/settlements`
pub async fn get_settlements(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Query(q): Query<SettlementsQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-settlement", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match list_settlement_receipts(&pool, &cfg.tenant, &tr_id, q.limit.unwrap_or(24).min(200)).await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct SettlementsQuery {
    pub limit: Option<i64>,
}

// ── EPEX monthly prices ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EpexPriceBody {
    pub avg_ct_kwh: Decimal,
    pub source: Option<String>,
}

// ── §20 Abs. 2 Jahresmarktwert prices ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JahresmarktwertBody {
    pub avg_ct_kwh: Decimal,
    pub source: Option<String>,
}

/// `PUT /api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}`
///
/// Import or update a technology-specific monthly Jahresmarktwert price
/// (§20 Abs. 2 + Anlage 1 EEG 2023), published by ÜNB at netztransparenz.de.
///
/// `erzeugungsart` must match an `erzeugungsart` column value (e.g. `WIND_ONSHORE`,
/// `SOLAR_AUFDACH`, `BIOMASSE`) or `DEFAULT` for the generic fallback row.
///
/// For MarketPremium (Direktvermarktung / Ausschreibung) settlements, the
/// technology-specific Jahresmarktwert takes precedence over the generic EPEX
/// monthly average from `epex_monthly_prices`.
pub async fn put_jahresmarktwert(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(pool): Extension<PgPool>,
    Path((year, month, erzeugungsart)): Path<(i16, i16, String)>,
    Json(body): Json<JahresmarktwertBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-marktdaten", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if !(1..=12).contains(&month) {
        return (StatusCode::BAD_REQUEST, "month must be 1–12").into_response();
    }
    let source = body.source.as_deref().unwrap_or("manual");
    match upsert_jahresmarktwert(&pool, year, month, &erzeugungsart, body.avg_ct_kwh, source).await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}`
pub async fn get_jahresmarktwert(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(pool): Extension<PgPool>,
    Path((year, month, erzeugungsart)): Path<(i16, i16, String)>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-marktdaten", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match fetch_jahresmarktwert_single(&pool, year, month, &erzeugungsart).await {
        Ok(Some(p)) => Json(serde_json::json!({
            "billing_year": year,
            "billing_month": month,
            "erzeugungsart": erzeugungsart,
            "avg_ct_kwh": p,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `PUT /api/v1/epex-monthly/{year}/{month}`
pub async fn put_epex_price(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(pool): Extension<PgPool>,
    Path((year, month)): Path<(i16, i16)>,
    Json(body): Json<EpexPriceBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-marktdaten", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let source = body.source.as_deref().unwrap_or("manual");
    match upsert_epex_price(&pool, year, month, body.avg_ct_kwh, source).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// One EPEX spot interval in the bulk-load request body.
#[derive(Debug, Deserialize)]
pub struct SpotPriceEntry {
    /// Interval start, RFC 3339 (UTC).
    pub delivery_start: String,
    /// 15 or 60. Defaults to 15.
    #[serde(default = "default_spot_resolution")]
    pub resolution_min: i16,
    /// Price ct/kWh (may be negative).
    pub price_ct_kwh: Decimal,
}

fn default_spot_resolution() -> i16 {
    15
}

/// `PUT /api/v1/epex-spot` — bulk-load EPEX day-ahead spot prices (§51 input).
///
/// Body: `{ "source": "epex-day-ahead", "prices": [ {delivery_start, resolution_min, price_ct_kwh}, … ] }`.
/// einsd overlays a plant's ¼h feed-in against these to derive the §51
/// Negativpreisregel reduction.
pub async fn put_epex_spot(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(pool): Extension<PgPool>,
    Json(body): Json<SpotPriceLoadBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-marktdaten", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let mut prices = Vec::with_capacity(body.prices.len());
    for e in &body.prices {
        let Ok(start) = time::OffsetDateTime::parse(
            &e.delivery_start,
            &time::format_description::well_known::Rfc3339,
        ) else {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("invalid delivery_start (RFC 3339): {}", e.delivery_start),
            )
                .into_response();
        };
        prices.push(crate::pg::SpotPrice {
            delivery_start: start,
            resolution_min: e.resolution_min,
            price_ct_kwh: e.price_ct_kwh,
        });
    }
    let source = body.source.as_deref().unwrap_or("manual");
    match crate::pg::upsert_spot_prices(&pool, &prices, source).await {
        Ok(n) => (StatusCode::OK, Json(serde_json::json!({ "upserted": n }))).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// Bulk spot-price load request body.
#[derive(Debug, Deserialize)]
pub struct SpotPriceLoadBody {
    #[serde(default)]
    pub source: Option<String>,
    pub prices: Vec<SpotPriceEntry>,
}

/// `GET /api/v1/epex-monthly/{year}/{month}`
pub async fn get_epex_price(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(pool): Extension<PgPool>,
    Path((year, month)): Path<(i16, i16)>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-marktdaten", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match fetch_epex_price(&pool, year, month).await {
        Ok(Some(p)) => Json(serde_json::json!({
            "billing_year": year,
            "billing_month": month,
            "avg_ct_kwh": p,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

// ── Repowering (§3 Nr. 30 i.V.m. §25 EEG 2023) ───────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/repowering`.
#[derive(Debug, Deserialize)]
pub struct RepoweringRequest {
    /// ISO 8601 date when the new components were commissioned.
    /// The Förderendedatum is reset to `repowering_datum + 20 years`.
    pub repowering_datum: String,
    /// New installed capacity in kWp (may differ from original).
    pub leistung_kwp_neu: Option<Decimal>,
    /// New Vergütungssatz at the repowering date (ct/kWh).
    /// When absent, auto-lookup via `eeg_verguetungssaetze` table.
    pub verguetungssatz_ct_neu: Option<Decimal>,
}

/// Request body for `POST /api/v1/anlagen/{tr_id}/wind-reevaluation`.
#[derive(Debug, Deserialize)]
pub struct WindReevaluationRequest {
    /// Year after commissioning from which the adjusted AW takes effect: 6, 11 or 16.
    pub wirksam_ab_jahr: i16,
    /// Gütefaktor recomputed from the measured 5-year Standortertrag.
    pub guetefaktor: Decimal,
    /// Korrekturfaktor certified by the Gutachten (§36h Abs. 3 Nr. 2).
    ///
    /// The Netzbetreiber settles on the certified value, so it is accepted
    /// rather than derived. Omit it to interpolate the Anlage 2 Nr. 7 table
    /// from `guetefaktor`.
    pub korrekturfaktor: Option<Decimal>,
}

/// `POST /api/v1/anlagen/{tr_id}/wind-reevaluation`
///
/// Record a §36h Abs. 2 EEG 2023 Standortgüte re-evaluation for a wind plant. The
/// anzulegender Wert re-adjusts from the start of the 6th/11th/16th operating year
/// based on the measured Standortertrag; `build_settle_input` then applies the
/// re-evaluated Korrekturfaktor to every settlement from that year on. The
/// response flags whether the reviewed five-year period must be reconciled
/// (§36h Abs. 2 Satz 2: recomputed Gütefaktor deviates > 2 pp) — run that as a
/// `§147 AO` correction settlement (interest EURIBOR-12M + 1 pp).
pub async fn post_wind_reevaluation(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<WindReevaluationRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    if !matches!(req.wirksam_ab_jahr, 6 | 11 | 16) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "wirksam_ab_jahr must be 6, 11, or 16 (§36h Abs. 2 Satz 1)".to_owned(),
        )
            .into_response();
    }
    match crate::pg::record_wind_reevaluation(
        &pool,
        &cfg.tenant,
        &tr_id,
        req.wirksam_ab_jahr,
        req.guetefaktor,
        req.korrekturfaktor,
    )
    .await
    {
        Ok(Some((reconciliation_required, previous))) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "recorded": true,
                "wirksam_ab_jahr": req.wirksam_ab_jahr,
                "new_guetefaktor": req.guetefaktor,
                "previous_guetefaktor": previous,
                "reconciliation_required": reconciliation_required,
            })),
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `POST /api/v1/anlagen/{tr_id}/repowering`
///
/// Record a **Vollrepowering** — replacing the generator unit — for an existing
/// plant.
///
/// The Förderdauer restarts because the replacement is a fresh Inbetriebnahme
/// (§3 Nr. 30 EEG 2023: the first commissioning of the *generator* after its
/// renewal), so §25 Abs. 1 runs again from `repowering_datum`. **§22 governs the
/// wettbewerbliche Ermittlung der Marktprämie and has nothing to do with this**;
/// citing it here was simply wrong.
///
/// - `inbetriebnahme` becomes `repowering_datum`; the original date is kept in
///   `ursprungs_inbetriebnahme`.
/// - The Vergütungssatz is re-looked-up for `repowering_datum` unless supplied.
/// - The §51 regime is re-derived from the new date, so a plant repowered after
///   25.02.2025 falls under the Solarspitzengesetz rules.
/// - The 180-day Förderende alert is re-armed: the new expiry is a new fact.
/// - The plant stays `aktiv`.
///
/// This is only correct for a full replacement. Partial repowering (rotor or
/// nacelle only) leaves the original commissioning date governing — do not use
/// this endpoint for it.
///
/// Idempotent: re-posting with the same date is safe.
pub async fn post_repowering(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<RepoweringRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    use time::format_description::well_known::Iso8601;

    let repowering_datum = match time::Date::parse(&req.repowering_datum, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid repowering_datum, expected ISO 8601",
            )
                .into_response();
        }
    };

    let anlage = match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    // §25 Abs. 1 Satz 2 EEG 2023: statutory plants extend to Dec 31 of the 20th
    // year, not to the exact +20 y anniversary (which is the Ausschreibung rule).
    let foerderendedatum_neu = match eeg_billing::foerderendedatum_repowering(repowering_datum) {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    // Auto-lookup new Vergütungssatz when not supplied.
    let verguetungssatz_ct_neu = if let Some(ct) = req.verguetungssatz_ct_neu {
        ct
    } else {
        match lookup_verguetungssatz(
            &pool,
            &anlage.erzeugungsart,
            &anlage.verguetungsform,
            req.leistung_kwp_neu.unwrap_or(anlage.leistung_kwp),
            &req.repowering_datum,
        ).await {
            Ok(Some(ct)) => ct,
            Ok(None) => return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "No Vergütungssatz found for this plant type and repowering date — supply verguetungssatz_ct_neu explicitly",
            ).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        }
    };

    let res = sqlx::query(
        r"UPDATE eeg_anlagen SET
              ist_repowering           = true,
              ursprungs_inbetriebnahme = COALESCE(ursprungs_inbetriebnahme, inbetriebnahme),
              repowering_datum         = $3,
              inbetriebnahme           = $3,
              foerderendedatum         = $4,
              verguetungssatz_ct       = $5,
              leistung_kwp             = COALESCE($6, leistung_kwp),
              -- The Förderende moved, so the 180-day alert has to be able to
              -- fire again for the new one.
              foerderung_alert_sent_at = NULL,
              updated_at               = now()
          WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(repowering_datum)
    .bind(foerderendedatum_neu)
    .bind(verguetungssatz_ct_neu)
    .bind(req.leistung_kwp_neu)
    .execute(&pool)
    .await;

    match res {
        Ok(r) if r.rows_affected() > 0 => Json(serde_json::json!({
            "tr_id": tr_id,
            "repowering_datum": repowering_datum.to_string(),
            "foerderendedatum_neu": foerderendedatum_neu.to_string(),
            "verguetungssatz_ct_neu": verguetungssatz_ct_neu,
        }))
        .into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

// ── Vergütungssatz lookup ─────────────────────────────────────────────────────

/// Request body for `POST /api/v1/verguetungssatz-lookup`.
#[derive(Debug, Deserialize)]
pub struct VerguetungssatzLookupRequest {
    pub erzeugungsart: String,
    pub leistung_kwp: Decimal,
    /// ISO 8601 Inbetriebnahmedatum.
    pub inbetriebnahme: String,
    /// `"UEBERSCHUSS"` (default), `"VOLLEINSPEISUNG"` (§48 Abs. 2a) or
    /// `"KWK_ZUSCHLAG"`. The two solar forms differ by several ct/kWh, so a
    /// lookup that omits it is answered on the Überschuss column.
    #[serde(default)]
    pub verguetungsform: Option<String>,
}

// ── MaStR registration confirmation ──────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/mastr-registrierung`.
#[derive(Debug, Deserialize)]
pub struct MastrRegistrierungRequest {
    /// MaStR Registrierungsnummer (format: `SEE000000000000`, `EEE000000000000`, etc.).
    ///
    /// Issued by BNetzA at marktstammdatenregister.de.
    pub mastr_nummer: String,
    /// Date of MaStR registration (ISO 8601). Defaults to today if omitted.
    pub mastr_datum: Option<String>,
}

/// `POST /api/v1/anlagen/{tr_id}/mastr-registrierung`
///
/// Confirm MaStR registration for a plant:
/// - `mastr_registriert` → `true`
/// - the §52 Abs. 1 Nr. 11 violation clock (`mastr_violation_start`) is cleared
///
/// The plant status is untouched. `eeg_anlagen.status` has no pre-activation
/// value — a plant is `aktiv` from registration — so there is no transition to
/// make here.
///
/// ## Legal basis
///
/// §52 Abs. 1 Nr. 11 EEG 2023: plant operators must register in MaStR.
/// - EEG 2023 plants: until confirmed, €10/kW/month Pflichtzahlung accrues.
/// - EEG ≤2021 plants: until confirmed, Vergütung = 0 (old §52/§47 via §100).
///
/// ## CloudEvent emitted
///
/// `de.eeg.anlage.mastr-registriert` — signals ERP to release pending Vergütung.
pub async fn post_mastr_registrierung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<MastrRegistrierungRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    use time::format_description::well_known::Iso8601;

    let mastr_datum = if let Some(ref ds) = req.mastr_datum {
        match time::Date::parse(ds, &Iso8601::DEFAULT) {
            Ok(d) => d,
            Err(_) => {
                return (StatusCode::UNPROCESSABLE_ENTITY, "invalid mastr_datum").into_response();
            }
        }
    } else {
        time::OffsetDateTime::now_utc().date()
    };

    // Transactional outbox: the `eeg_anlagen` state change (mastr_registriert)
    // and its ERP CloudEvent commit as one unit, so the registration-release
    // signal cannot be lost on a webhook 5xx/crash. A background OutboxWorker
    // drains `event_outbox` (persist-before-dispatch).
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    let rows = sqlx::query(
        "UPDATE eeg_anlagen SET \
            mastr_registriert    = true, \
            mastr_nummer         = $3, \
            mastr_datum          = $4, \
            mastr_violation_start = NULL, \
            updated_at           = now() \
         WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(&req.mastr_nummer)
    .bind(mastr_datum)
    .execute(&mut *tx)
    .await;

    match rows {
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Ok(r) if r.rows_affected() == 0 => return StatusCode::NOT_FOUND.into_response(),
        Ok(_) => {}
    }

    // Enqueue the release signal in the same transaction (skipped when no ERP
    // endpoint is configured — nothing to deliver).
    if cfg.erp_webhook_url.is_some() {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("einsd", &cfg.tenant),
            mako_events::eeg::ANLAGE_MASTR_REGISTRIERT,
            tr_id.clone(),
            serde_json::json!({
                "tr_id": tr_id,
                "mastr_nummer": req.mastr_nummer,
                "mastr_datum": mastr_datum.to_string(),
            }),
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            tracing::error!(tr_id, error = %e, "einsd: MaStR registriert outbox enqueue failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/v1/verguetungssatz-lookup`
///
/// Returns the applicable EEG feed-in tariff rate for a plant.
/// Used during Anlage registration to auto-populate `verguetungssatz_ct`
/// without requiring the operator to manually look up BNetzA tables.
pub async fn post_verguetungssatz_lookup(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(pool): Extension<PgPool>,
    Json(req): Json<VerguetungssatzLookupRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-marktdaten", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let verguetungsform = req.verguetungsform.as_deref().unwrap_or("UEBERSCHUSS");
    match lookup_verguetungssatz(
        &pool,
        &req.erzeugungsart,
        verguetungsform,
        req.leistung_kwp,
        &req.inbetriebnahme,
    )
    .await
    {
        Ok(Some(ct)) => Json(serde_json::json!({
            "erzeugungsart": req.erzeugungsart,
            "verguetungsform": verguetungsform,
            "leistung_kwp": req.leistung_kwp,
            "inbetriebnahme": req.inbetriebnahme,
            "verguetungssatz_ct": ct,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "No matching EEG tariff rate found. Use PUT /api/v1/verguetungssaetze to import additional rates.",
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

// ── Batch settlement (POST /api/v1/settle/{year}/{month}) ────────────────────

/// Request body for `POST /api/v1/settle/{year}/{month}`.
#[derive(Debug, serde::Deserialize)]
pub struct BatchSettleRequest {
    /// EPEX monthly average ct/kWh.  When absent, uses stored `epex_monthly_prices`.
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// Dry-run mode — calculates but does not persist or emit CloudEvents.
    #[serde(default)]
    pub dry_run: bool,
    /// Maximum plants to settle in one request (default 500, max 2000).
    pub limit: Option<i64>,
}

/// `POST /api/v1/settle/{year}/{month}`
///
/// **Batch EEG/KWKG settlement — settle all unsettled active plants for a month.**
///
/// Idempotent: plants already settled for this period are skipped.
/// Auto-fetches Einspeisemenge from `edmd` for each plant's MaLo when
/// `edmd_url` is configured.
///
/// Returns a summary with per-plant results and aggregate totals.
pub async fn post_batch_settle(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path((year, month)): Path<(i16, i16)>,
    Json(req): Json<BatchSettleRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "run-settlement", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Resolve EPEX price once for the whole batch.
    let epex_avg_ct_kwh = match req.epex_avg_ct_kwh {
        Some(p) => Some(p),
        None => match fetch_epex_price(&pool, year, month).await {
            Ok(p) => p,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        },
    };

    let limit = req.limit.unwrap_or(500).min(2000);
    let plants = match list_unsettled(&pool, &cfg.tenant, year, month).await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    let plants: Vec<_> = plants.into_iter().take(limit as usize).collect();
    let total_plants = plants.len();
    let mut settled = 0u32;
    let mut skipped_no_data = 0u32;
    let mut skipped_price_missing = 0u32;
    let mut errors = 0u32;
    let mut total_settlement_eur = rust_decimal::Decimal::ZERO;
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(plants.len().min(100));

    if req.dry_run {
        // Dry-run: count without persisting (no DB writes needed).
        for anlage in &plants {
            let has_data =
                fetch_einspeisemenge_from_edmd(&cfg, &http_client, &anlage.malo_id, year, month)
                    .await
                    .is_some();
            if has_data {
                settled += 1;
            } else {
                skipped_no_data += 1;
            }
        }
    } else {
        // ── Parallel batch settlement with bounded concurrency ────────────────
        // Use JoinSet + Semaphore (20 concurrent) to parallelize DB + edmd I/O.
        // Each task has its own pool handle (PgPool is Arc-backed, clone is cheap).
        use std::sync::Arc;
        use tokio::sync::Semaphore;
        use tokio::task::JoinSet;

        const MAX_CONCURRENT: usize = 20;
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
        let mut join_set: JoinSet<(String, String, anyhow::Result<crate::pg::SettleResult>)> =
            JoinSet::new();

        for anlage in plants {
            let cfg = Arc::clone(&cfg);
            let pool = pool.clone();
            let sem = Arc::clone(&sem);
            let client = Arc::clone(&http_client);
            let tr_id = anlage.tr_id.clone();
            let settlement_model = anlage.settlement_model.clone();
            join_set.spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                // The same path the REST, MCP and worker settles take, so a
                // plant settled in bulk gets the identical amount.
                let res = crate::settle::settle_plant(
                    &pool,
                    &cfg,
                    &client,
                    &anlage,
                    year,
                    month,
                    crate::settle::SettleRequest {
                        epex_avg_ct_kwh,
                        ..Default::default()
                    },
                )
                .await;
                (tr_id, settlement_model, res)
            });
        }

        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok((_tr_id, _model, Ok(result))) => match result.status.as_str() {
                    "calculated" | "foerderung_beendet" => {
                        settled += 1;
                        if let Some(eur) = result.settlement_eur {
                            total_settlement_eur += eur;
                        }
                        if results.len() < 100 {
                            results.push(serde_json::json!({
                                "tr_id": result.tr_id,
                                "status": result.status,
                                "settlement_eur": result.settlement_eur,
                                "einspeisemenge_kwh": result.einspeisemenge_kwh,
                            }));
                        }
                    }
                    "no_data" => skipped_no_data += 1,
                    "price_missing" => skipped_price_missing += 1,
                    _ => errors += 1,
                },
                Ok((tr_id, _, Err(e))) => {
                    tracing::warn!(tr_id, error = %e, "batch_settle: settlement error");
                    errors += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "batch_settle: task join error");
                    errors += 1;
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "billing_year": year,
            "billing_month": month,
            "dry_run": req.dry_run,
            "total_plants": total_plants,
            "settled": settled,
            "skipped_no_data": skipped_no_data,
            "skipped_price_missing": skipped_price_missing,
            "errors": errors,
            "total_settlement_eur": total_settlement_eur.to_string(),
            "results_sample": results,
            "hint": if total_plants == limit as usize {
                format!("More plants may be unsettled. Re-run to settle the next batch of {}.", limit)
            } else {
                "All unsettled plants processed.".to_owned()
            }
        })),
    )
        .into_response()
}

// ── §24 EEG 2023 — Zusammenlegung ────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/zusammenlegen`.
#[derive(Debug, serde::Deserialize)]
pub struct ZusammenlegungRequest {
    /// TR-ID of the parent (surviving) plant.
    pub parent_tr_id: String,
    /// Combined installed capacity in kWp after merger.
    /// When absent, the parent's capacity is unchanged.
    pub combined_leistung_kwp: Option<Decimal>,
    /// §24 Abs. 1 Satz 1 Nr. 1, second limb — the two plants are in
    /// *unmittelbarer räumlicher Nähe* although they do not share a
    /// `standort_id`.
    ///
    /// A human judgement about the pair, so it is asserted per request rather
    /// than derived. It only matters when the two sites differ; sharing a
    /// `standort_id` already satisfies Nr. 1.
    #[serde(default)]
    pub unmittelbare_raeumliche_naehe: bool,
}

/// `POST /api/v1/anlagen/{tr_id}/zusammenlegen`
///
/// **§24 EEG 2023 — Zusammenlegung (plant merger).**
///
/// Merges `{tr_id}` (child) into `parent_tr_id`, but **only where §24 Abs. 1
/// actually deems them one plant** — all four conditions of Satz 1, and none of
/// the Sätze 2–5 carve-outs. A merge the statute does not support moves the
/// survivor into a tariff band and past a tender threshold it never qualified
/// for, for the rest of its Förderdauer, and no later correction detects it.
/// A refused merge answers `422` naming the rule that decided.
///
/// On a permitted merge:
/// - The child plant is deregistered (`status = abgemeldet`).
/// - `parent_tr_id` is set on the child for audit trail.
/// - The parent plant assumes the combined capacity (`combined_leistung_kwp`).
/// - The parent's `foerderendedatum` is **NOT** reset (only Repowering resets it).
/// - Future settlements continue only on the parent plant.
///
/// This is distinct from Repowering: Zusammenlegung is an administrative merger,
/// not a hardware replacement, so there is no new commissioning date and no new
/// Förderdauer.
pub async fn post_zusammenlegen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(child_tr_id): Path<String>,
    Json(req): Json<ZusammenlegungRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if child_tr_id == req.parent_tr_id {
        return (
            StatusCode::BAD_REQUEST,
            "child and parent tr_id must differ",
        )
            .into_response();
    }

    match zusammenlegen(
        &pool,
        &cfg.tenant,
        &child_tr_id,
        &req.parent_tr_id,
        req.combined_leistung_kwp,
        req.unmittelbare_raeumliche_naehe,
    )
    .await
    {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "child_tr_id": child_tr_id,
                "parent_tr_id": req.parent_tr_id,
                "child_status": "abgemeldet",
                "combined_leistung_kwp": req.combined_leistung_kwp,
                "note": "§24 EEG 2023 Zusammenlegung complete. Future settlements run on parent plant only.",
            })),
        )
            .into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, format!("plant {child_tr_id} not found or not aktiv")).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

// ── §21b EEG 2023 — Veräußerungsform Wechsel ─────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/switch-veraeusserungsform`.
#[derive(Debug, serde::Deserialize)]
pub struct VeraeusserungsformWechselRequest {
    /// The Veräußerungsform to switch to: `"VERGUETUNG"` (Einspeisevergütung,
    /// §21 Abs. 1) or `"DIREKTVERMARKTUNG"` (gleitende Marktprämie, §20).
    pub new_model: String,
    /// Effective date for the switch (must be the 1st of a calendar month).
    pub effective_date: String,
    /// For Direktvermarktung switches: the Direktvermarkter's MP-ID.
    pub direktvermarkter_mp_id: Option<String>,
    /// For Direktvermarktung switches: the agreed Anzulegender Wert ct/kWh.
    pub direktverm_aw_ct: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/anlagen/{tr_id}/switch-veraeusserungsform`
///
/// **§21b EEG 2023 — Veräußerungsform Wechsel.**
///
/// Switches the plant between Einspeisevergütung (§21) and Direktvermarktung (§20).
///
/// Rules enforced:
/// - §21b Abs. 1: the switch takes effect on the 1st of a calendar month, and a
///   plant may change form once per month.
/// - §21 Abs. 1 Nr. 3: a plant above the mandatory-Direktvermarktung threshold
///   cannot switch back to Einspeisevergütung.
/// - §21c Abs. 1: the Netzbetreiber must be notified before the start of the
///   preceding calendar month — so the earliest reachable date is the 1st of the
///   month after next.
///
/// On success: updates `settlement_model`, `direktverm_mp_id`, `direktverm_aw_ct`,
/// and `last_veraeusserungsform_switch` on the plant record.
pub async fn post_switch_veraeusserungsform(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<VeraeusserungsformWechselRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    use eeg_billing::EegGesetz;
    use eeg_billing::direktverm::{SwitchBlockedReason, validate_switch_to_vergütung};
    use time::format_description::well_known::Iso8601;

    let anlage = match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    let effective_date = match time::Date::parse(&req.effective_date, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid effective_date — use ISO 8601 format (YYYY-MM-DD)",
            )
                .into_response();
        }
    };

    if effective_date.day() != 1 {
        return (
            StatusCode::BAD_REQUEST,
            "effective_date must be the 1st of a calendar month (§21b Abs. 1 EEG 2023)",
        )
            .into_response();
    }

    // §21c Abs. 1 EEG 2023: the switch must reach the Netzbetreiber **before the
    // beginning of the preceding calendar month**. A switch effective 1 June has
    // to be notified by 30 April.
    //
    // The check is on the request date because that is when the notification is
    // enqueued. A backdated switch is refused rather than silently accepted: it
    // would change what the plant is owed for a month already settled.
    if let Some(earliest) = fruehester_wechseltermin(time::OffsetDateTime::now_utc().date())
        && effective_date < earliest
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "§21c Abs. 1 EEG 2023: a Veräußerungsform switch must reach the \
                 Netzbetreiber before the start of the preceding calendar month — \
                 the earliest effective_date that can still be notified today is {earliest}"
            ),
        )
            .into_response();
    }

    let eeg_gesetz = EegGesetz::from_db_year(anlage.eeg_gesetz).unwrap_or(EegGesetz::Eeg2023);

    // Only validate the switch-to-Vergütung direction (mandatory plants cannot switch back).
    // Switching to Direktvermarktung is always allowed.
    let is_switching_to_verguetung = req.new_model == crate::models::VERGUETUNG;

    if is_switching_to_verguetung
        && let Err(reason) = validate_switch_to_vergütung(
            anlage.leistung_kwp,
            eeg_gesetz,
            effective_date,
            anlage.last_veraeusserungsform_switch,
        )
    {
        let msg = match reason {
            SwitchBlockedReason::PflichtgemasseDirektvermarktung => {
                "plant is subject to mandatory Direktvermarktung (§20 EEG 2023 — >100 kW) and cannot switch back to Einspeisevergütung"
            }
            SwitchBlockedReason::AlreadySwitchedThisMonth { last_switch } => &format!(
                "already switched this calendar month (last switch: {last_switch}) — §21b EEG 2023 allows only one switch per month"
            ),
        };
        return (StatusCode::UNPROCESSABLE_ENTITY, msg.to_owned()).into_response();
    }

    let new_model = match req.new_model.as_str() {
        crate::models::VERGUETUNG => crate::models::VERGUETUNG,
        crate::models::DIREKTVERMARKTUNG => crate::models::DIREKTVERMARKTUNG,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "§21b EEG 2023 knows two Veräußerungsformen — expected VERGUETUNG or \
                     DIREKTVERMARKTUNG, got {other}"
                ),
            )
                .into_response();
        }
    };

    // ── Transactional outbox: the Veräußerungsform switch (eeg_anlagen UPDATE) and
    // its §21c notification CloudEvent commit atomically. ─────────────────────────
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    // `direktvermarktung` moves with the model. Left behind, the plant read back
    // as "in Direktvermarktung" while settling as Einspeisevergütung, and the
    // next plain upsert then rewrote settlement_model from the stale flag.
    let updated = sqlx::query(
        r"UPDATE eeg_anlagen
          SET settlement_model               = $3,
              direktvermarktung              = ($3 = 'DIREKTVERMARKTUNG'),
              direktverm_mp_id               = $4,
              direktverm_aw_ct               = $5,
              last_veraeusserungsform_switch = $6,
              updated_at                     = now()
          WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(new_model)
    .bind(&req.direktvermarkter_mp_id)
    .bind(req.direktverm_aw_ct)
    .bind(effective_date)
    .execute(&mut *tx)
    .await;

    match updated {
        Ok(r) if r.rows_affected() > 0 => {
            // ── §21c EEG 2023: notify the NB of the switch by end of the calendar
            // month. de.eeg.veraeusserungsform.gewechselt is enqueued in this tx and
            // the OutboxWorker forwards it to the GPKE handler (makod PID 55022/55023).
            let ce = build_veraeusserungsform_ce(&cfg, &tr_id, new_model, &req.effective_date);
            let notification_sent = ce.is_some();
            if let Some(ce) = &ce {
                if let Err(e) = mako_service::outbox::enqueue(&mut tx, ce).await {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
                }
                // Record the notification timestamp in the same tx.
                if let Err(e) = sqlx::query(
                    "UPDATE eeg_anlagen
                     SET veraeusserungsform_notification_sent_at = now()
                     WHERE tr_id = $1 AND tenant = $2",
                )
                .bind(&tr_id)
                .bind(&cfg.tenant)
                .execute(&mut *tx)
                .await
                {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
                }
            }
            if let Err(e) = tx.commit().await {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "tr_id": tr_id,
                    "new_model": new_model,
                    "effective_date": req.effective_date,
                    "notification_sent": notification_sent,
                    "note": format!(
                        "§21b EEG 2023 Veräußerungsform Wechsel to {} recorded. \
                         §21c Abs. 1 notification {}.",
                        new_model,
                        if notification_sent { "enqueued for delivery" } else { "pending — configure erp_webhook_url" }
                    )
                })),
            )
                .into_response()
        }
        // rows_affected == 0 → plant not found; tx dropped (nothing changed).
        Ok(_) => (StatusCode::NOT_FOUND, format!("plant {tr_id} not found")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// The earliest first-of-month a switch notified **today** can take effect.
///
/// §21c Abs. 1 EEG 2023 requires the notification before the start of the
/// preceding calendar month, so a notification sent in April is in time for
/// 1 June but not for 1 May.
#[must_use]
pub fn fruehester_wechseltermin(heute: time::Date) -> Option<time::Date> {
    let mut d = heute.replace_day(1).ok()?;
    for _ in 0..2 {
        let (y, m) = if d.month() == time::Month::December {
            (d.year() + 1, time::Month::January)
        } else {
            (d.year(), d.month().next())
        };
        d = time::Date::from_calendar_date(y, m, 1).ok()?;
    }
    Some(d)
}

/// Build the `de.eeg.veraeusserungsform.gewechselt` CloudEvent for §21c EEG 2023.
///
/// Returns `Some(CloudEvent)` when the ERP webhook is configured — the caller must
/// `enqueue` it in the same transaction as the `eeg_anlagen` update, so the switch
/// and its §21c notification commit atomically. The ERP webhook is expected to
/// forward this to the GPKE process handler (makod, PID 55022 Wechsel Marktrollen /
/// PID 55023 Wechselbestätigung).
#[must_use]
fn build_veraeusserungsform_ce(
    cfg: &EinsdConfig,
    tr_id: &str,
    new_model: &str,
    effective_date: &str,
) -> Option<mako_service::CloudEvent> {
    cfg.erp_webhook_url.as_deref()?;
    let ce_id = uuid::Uuid::new_v4();
    let ce = mako_service::CloudEvent::new(
        mako_service::source("einsd", &cfg.tenant),
        mako_events::eeg::VERAEUSSERUNGSFORM_GEWECHSELT,
        tr_id,
        serde_json::json!({
            "tr_id": tr_id,
            "new_model": new_model,
            "effective_date": effective_date,
            "legal_basis": "§21c Abs. 1 EEG 2023",
            "frist": "vor Beginn des dem Wechsel vorangegangenen Kalendermonats"
        }),
    )
    .with_id(ce_id.to_string());

    Some(ce)
}

// ── § 147 AO / GoBD — Correction Settlement ───────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/settlements/{year}/{month}/correction`.
#[derive(Debug, serde::Deserialize)]
pub struct CorrectionSettleRequest {
    /// Corrected Einspeisemenge kWh.
    pub einspeisemenge_kwh: Option<rust_decimal::Decimal>,
    /// Corrected EPEX average ct/kWh (for Direktvermarktung / Post-EEG).
    pub epex_avg_ct_kwh: Option<rust_decimal::Decimal>,
    /// Reason for the correction.
    pub reason: eeg_billing::scheme::CorrectionReason,
    /// Free-text explanation for audit trail.
    pub reason_detail: Option<String>,
}

/// `POST /api/v1/anlagen/{tr_id}/settlements/{year}/{month}/correction`
///
/// **§ 147 AO / GoBD — Correction Settlement.**
///
/// Creates a correction receipt that supersedes the original settlement for the
/// given billing period. The original receipt is preserved for audit trail.
///
/// Use cases:
/// - Corrected meter reading arrives (§ 147 AO / GoBD).
/// - Tariff error discovered.
/// - MaStR registration retroactively confirmed (retroactive §52 sanction removal).
/// - Capacity correction.
///
/// The correction stores `SettlementType::Correction { original_id, reason }` for
/// traceability per § 147 AO / GoBD.
pub async fn post_correction_settle(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path((tr_id, year, month)): Path<(String, i16, i16)>,
    Json(req): Json<CorrectionSettleRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "correct-settlement", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // The link points at the period's **initial** receipt, not at whichever row
    // was written last. Ordering by `settled_at` made a second correction claim
    // to supersede the first one, so the chain no longer led back to the
    // settlement that actually created the payment obligation.
    let original_id: Option<uuid::Uuid> = sqlx::query_scalar(
        r"SELECT id FROM settlement_receipts
          WHERE tr_id = $1 AND tenant = $2 AND billing_year = $3 AND billing_month = $4
            AND is_correction = false",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(year)
    .bind(month)
    .fetch_optional(&pool)
    .await
    .ok()
    .flatten();

    let original_id_str = original_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| format!("{tr_id}/{year}/{month}"));

    // A correction runs the same path as the settlement it supersedes, so a
    // corrected month is recomputed under exactly the rules the original was,
    // §51 derivation included.
    let result = match crate::settle::settle_by_tr_id(
        &pool,
        &cfg,
        &http_client,
        &tr_id,
        year,
        month,
        crate::settle::SettleRequest {
            einspeisemenge_kwh: req.einspeisemenge_kwh,
            epex_avg_ct_kwh: req.epex_avg_ct_kwh,
            correction: Some(crate::pg::Korrektur {
                original_id,
                reason: req.reason,
                detail: req.reason_detail.clone(),
            }),
            ..Default::default()
        },
    )
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": result.id,
            "original_id": original_id_str,
            "correction_reason": format!("{:?}", req.reason),
            "reason_detail": req.reason_detail,
            "billing_year": year,
            "billing_month": month,
            "settlement_eur": result.settlement_eur,
            "status": result.status,
            "note": "§ 147 AO / GoBD correction receipt created. Original receipt preserved for audit trail.",
        })),
    )
        .into_response()
}

/// `POST /api/v1/anlagen/{tr_id}/jahresabrechnung/{year}`
///
/// Build the annual reconciliation over the year's monthly settlements.
///
/// The statement is derived from the stored receipts, not recomputed, so it
/// always agrees with what was actually settled. An incomplete year yields
/// `status = "vorlaeufig"` and names the months still missing rather than
/// presenting a partial sum as if it were the year.
pub async fn post_jahresabrechnung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path((tr_id, year)): Path<(String, i16)>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "run-settlement", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match crate::pg::run_jahresabrechnung(&pool, &cfg.tenant, &tr_id, year).await {
        Ok(ja) => (StatusCode::OK, Json(ja)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod veraeusserungsform_tests {
    use super::veraeusserungsform_of;

    /// The settlement model → `SG10 CCI+Z22` DE 7037 mapping is a regulatory
    /// judgement, not a rename: `processd` picks one of six Vorlauffristen from
    /// it (`E_0622` Prüfschritte 400–830).
    ///
    /// Two facts it has to get right — `AUSSCHREIBUNG` is still the Marktprämie
    /// (§ 22 EEG 2023 sets the anzulegender Wert competitively, not the form),
    /// and `Z90` is shared by the uneingeschränkte Einspeisevergütung and the
    /// Ausfallvergütung, which is why the endpoint returns the flag beside it.
    #[test]
    fn the_settlement_model_maps_onto_the_cci_z22_code() {
        assert_eq!(veraeusserungsform_of("VERGUETUNG"), Some("Z90"));
        assert_eq!(veraeusserungsform_of("AUSFALLVERGUETUNG"), Some("Z90"));
        assert_eq!(veraeusserungsform_of("DIREKTVERMARKTUNG"), Some("Z91"));
        assert_eq!(veraeusserungsform_of("AUSSCHREIBUNG"), Some("Z91"));
        assert_eq!(
            veraeusserungsform_of("SONSTIGE_DIREKTVERMARKTUNG"),
            Some("Z92")
        );
        assert_eq!(veraeusserungsform_of("KWKG_ZUSCHLAG"), Some("Z94"));
    }

    /// Settlement models that are not Veräußerungsformen have no code —
    /// answering one would send the Frist decision down the wrong branch.
    #[test]
    fn a_settlement_model_that_is_not_a_veraeusserungsform_has_no_code() {
        for model in [
            "MIETERSTROM",
            "GGV",
            "EIGENVERBRAUCH",
            "POST_EEG_SPOT",
            "FLEXIBILITAET",
            "FLEXIBILITAET_ZUSCHLAG",
        ] {
            assert_eq!(veraeusserungsform_of(model), None, "{model}");
        }
    }

    /// Every code the mapping emits is one `mako-pruefung` can parse — the two
    /// sides of the lookup must agree on the alphabet.
    #[test]
    fn every_emitted_code_parses_on_the_reading_side() {
        for model in [
            "VERGUETUNG",
            "AUSFALLVERGUETUNG",
            "DIREKTVERMARKTUNG",
            "AUSSCHREIBUNG",
            "SONSTIGE_DIREKTVERMARKTUNG",
            "KWKG_ZUSCHLAG",
        ] {
            let code = veraeusserungsform_of(model).expect("mapped");
            assert!(
                mako_pruefung::nb::types::Veraeusserungsform::from_wire_code(code).is_some(),
                "{model} → {code} is not a code mako-pruefung knows"
            );
        }
    }
}

#[cfg(test)]
mod calendar_tests {
    use super::billing_month_range;
    use time::Duration;

    /// EEG settlement is per calendar month, so a wrong month boundary changes
    /// the Vergütung on a legally binding Gutschrift. The window comes from
    /// `billing_month_range`, in **German local time** — the hand-rolled
    /// `days_in_month` this replaced produced calendar dates that were then read
    /// as UTC, shifting the boundary by an hour and by two across a DST change.
    #[test]
    fn a_billing_month_spans_its_own_length_in_berlin_time() {
        for (year, month, hours) in [
            // Ordinary months, in hours.
            (2026_i16, 1_i16, 31 * 24_i64),
            (2026, 4, 30 * 24),
            (2026, 2, 28 * 24),
            // Leap February, on the full Gregorian rule.
            (2024, 2, 29 * 24),
            (2000, 2, 29 * 24),
            (1900, 2, 28 * 24),
            (2100, 2, 28 * 24),
            // March loses an hour to CEST, October gains one back.
            (2026, 3, 31 * 24 - 1),
            (2026, 10, 31 * 24 + 1),
        ] {
            let (from, to) = billing_month_range(year, month).expect("a real month");
            assert_eq!(
                to - from,
                Duration::hours(hours),
                "{year}-{month:02} should span {hours} h"
            );
        }
    }

    /// A month number outside 1..=12 has no window. The old helper answered 28
    /// days for one.
    #[test]
    fn an_impossible_month_has_no_window() {
        assert!(billing_month_range(2026, 0).is_none());
        assert!(billing_month_range(2026, 13).is_none());
    }
}

// ── §§53b–54 EEG 2023 — the facts that cut the anzulegender Wert ─────────────
//
// These rows change what a plant is paid, silently: a settlement run picks them
// up and the Gutschrift is simply smaller. Recording them through the API rather
// than by hand keeps that behind the same Cedar gate and audit path as every
// other lifecycle change, and the GET answers "why is this plant paid less"
// without reading a settlement back.
//
// No amount is accepted except §53c's, which the statute ties to the exemption
// actually granted. §53b's 0,1 ct/kWh and §54's 0,3 / 2,5 ct/kWh are fixed by
// law and live in `eeg_billing::aw_reductions`.

/// Request body for `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/regionalnachweis`.
#[derive(Debug, serde::Deserialize)]
pub struct RegionalnachweisRequest {
    /// Register reference of the issued Regionalnachweis (§79a EEG).
    pub nachweis_ref: String,
    /// First day the Nachweis covers (ISO 8601).
    pub effective_from: String,
    /// Last day it covers; open-ended when absent.
    pub effective_until: Option<String>,
}

/// Request body for `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/stromsteuerbefreiung`.
#[derive(Debug, serde::Deserialize)]
pub struct StromsteuerbefreiungRequest {
    /// The exemption granted, in ct/kWh. Capped at the §3 StromStG full rate of
    /// 2,05 ct/kWh — an exemption cannot exceed the tax it exempts from.
    pub befreiung_ct_kwh: Decimal,
    /// Which StromStG provision it rests on, e.g. `"§9 Abs. 1 Nr. 1 StromStG"`.
    pub rechtsgrundlage: String,
    pub effective_from: String,
    pub effective_until: Option<String>,
}

/// Request body for `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/sect54-defekt`.
#[derive(Debug, serde::Deserialize)]
pub struct Sect54DefektRequest {
    /// Abs. 1 — Zahlungsberechtigung applied for after the 18th Kalendermonat.
    #[serde(default)]
    pub zahlungsberechtigung_nach_18_monaten: bool,
    /// Abs. 2 — location does not match the Flurstücke named in the bid.
    #[serde(default)]
    pub flurstueck_abweichung: bool,
    /// Abs. 3 — Agri-PV Nutzungsnachweis not supplied.
    #[serde(default)]
    pub agri_nutzungsnachweis_fehlt: bool,
    /// Abs. 4 — Landesverordnung under §37c Abs. 2 not met; AW → 0.
    #[serde(default)]
    pub landesverordnung_nicht_erfuellt: bool,
    pub bnetza_ref: Option<String>,
    pub notes: Option<String>,
    pub effective_from: String,
    pub effective_until: Option<String>,
}

/// Request body for closing a §54 defect period.
#[derive(Debug, serde::Deserialize)]
pub struct Sect54NachweisRequest {
    /// Last day the defect applied — §54 Abs. 3 Satz 2/3.
    pub effective_until: String,
}

fn parse_iso_date(s: &str) -> Result<time::Date, String> {
    time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| format!("invalid date `{s}`: {e}"))
}

/// `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/regionalnachweis`
///
/// Record that a Regionalnachweis (§79a EEG) was issued for this plant's
/// electricity. §53b then cuts the anzulegender Wert by the statutory
/// 0,1 ct/kWh for the recorded period — but only where the AW is *gesetzlich
/// bestimmt*, so a tender-awarded plant is unaffected even with a Nachweis on
/// file.
///
/// **Cedar action**: `manage-lifecycle`
pub async fn post_regionalnachweis(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<RegionalnachweisRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let (from, until) = match parse_period(&req.effective_from, req.effective_until.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match crate::pg::record_regionalnachweis(
        &pool,
        &cfg.tenant,
        &tr_id,
        &req.nachweis_ref,
        from,
        until,
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id,
                "tr_id": tr_id,
                "paragraph": "§53b EEG 2023",
                "abzug_ct_kwh": eeg_billing::aw_reductions::SECT53B_REGIONALNACHWEIS_CT_KWH,
                "hinweis": "gilt nur bei gesetzlich bestimmtem anzulegendem Wert",
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/stromsteuerbefreiung`
///
/// Record a granted per-kWh Stromsteuerbefreiung for grid-transited electricity
/// (§53c EEG 2023). The schema rejects an amount above the §3 StromStG full rate.
///
/// **Cedar action**: `manage-lifecycle`
pub async fn post_stromsteuerbefreiung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<StromsteuerbefreiungRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let (from, until) = match parse_period(&req.effective_from, req.effective_until.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match crate::pg::record_stromsteuerbefreiung(
        &pool,
        &cfg.tenant,
        &tr_id,
        req.befreiung_ct_kwh,
        &req.rechtsgrundlage,
        from,
        until,
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id,
                "tr_id": tr_id,
                "paragraph": "§53c EEG 2023",
                "abzug_ct_kwh": req.befreiung_ct_kwh,
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/sect54-defekt`
///
/// Record §54 defects for a solar first-segment auction plant. At least one
/// defect must be set — a row recording none deducts nothing and is a
/// data-entry error, which the schema rejects.
///
/// **Cedar action**: `manage-lifecycle`
pub async fn post_sect54_defekt(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<Sect54DefektRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let (from, until) = match parse_period(&req.effective_from, req.effective_until.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let defekte = eeg_billing::Sect54SolarReduction {
        zahlungsberechtigung_nach_18_monaten: req.zahlungsberechtigung_nach_18_monaten,
        flurstueck_abweichung: req.flurstueck_abweichung,
        agri_nutzungsnachweis_fehlt: req.agri_nutzungsnachweis_fehlt,
        landesverordnung_nicht_erfuellt: req.landesverordnung_nicht_erfuellt,
    };
    if defekte.is_clean() {
        return (
            StatusCode::BAD_REQUEST,
            "at least one §54 defect must be set — a row recording none deducts nothing",
        )
            .into_response();
    }
    match crate::pg::record_sect54_defekt(
        &pool,
        &cfg.tenant,
        &tr_id,
        defekte,
        req.bnetza_ref.as_deref(),
        req.notes.as_deref(),
        from,
        until,
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": id, "tr_id": tr_id, "paragraph": "§54 EEG 2023" })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// `POST /api/v1/anlagen/{tr_id}/aw-reduktionen/sect54-defekt/{id}/nachweis-erbracht`
///
/// §54 Abs. 3 Satz 2/3 — the missing Nachweis has been supplied, so the
/// deduction lapses from `effective_until` onwards. The row is closed rather
/// than deleted: that the plant was short for the earlier periods is exactly
/// what the §147 AO audit trail has to keep.
///
/// **Cedar action**: `manage-lifecycle`
pub async fn post_sect54_nachweis_erbracht(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path((tr_id, id)): Path<(String, uuid::Uuid)>,
    Json(req): Json<Sect54NachweisRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "manage-lifecycle", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let until = match parse_iso_date(&req.effective_until) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match crate::pg::close_sect54_defekt(&pool, &cfg.tenant, id, until).await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": id,
                "tr_id": tr_id,
                "effective_until": until.to_string(),
                "legal_basis": "§54 Abs. 3 Satz 2/3 EEG 2023",
            })),
        )
            .into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no open §54 defect with that id").into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

/// Query for `GET /api/v1/anlagen/{tr_id}/aw-reduktionen`.
#[derive(Debug, serde::Deserialize)]
pub struct AwReduktionenQuery {
    /// The date to evaluate; today when absent.
    pub on: Option<String>,
}

/// `GET /api/v1/anlagen/{tr_id}/aw-reduktionen?on=YYYY-MM-DD`
///
/// What is cutting this plant's anzulegender Wert on a given day, with the
/// statutory amount for each. A settlement changes silently when one of these
/// rows exists, so this is the path that answers "why is this plant paid less"
/// without settling again.
///
/// **Cedar action**: `read-anlage`
pub async fn get_aw_reduktionen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<AwReduktionenQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-anlage", &cfg.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let on = match q.on.as_deref() {
        Some(s) => match parse_iso_date(s) {
            Ok(d) => d,
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        },
        None => time::OffsetDateTime::now_utc().date(),
    };
    match crate::pg::aw_reduktionen_am(&pool, &cfg.tenant, &tr_id, on).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// Parse a validity period, refusing one that ends before it starts.
///
/// The schema CHECKs this too; catching it here answers `400` with the reason
/// instead of surfacing a constraint name.
fn parse_period(
    from: &str,
    until: Option<&str>,
) -> Result<(time::Date, Option<time::Date>), String> {
    let from = parse_iso_date(from)?;
    let until = until.map(parse_iso_date).transpose()?;
    if let Some(u) = until
        && u < from
    {
        return Err(format!(
            "effective_until {u} is before effective_from {from}"
        ));
    }
    Ok((from, until))
}

// ── Einspeiser (Anlagenbetreiber) ─────────────────────────────────────────────

/// `GET /api/v1/einspeiser` — list the tenant's operators.
pub async fn list_einspeiser(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einspeiser", &cfg.tenant)
        .is_err()
    {
        return ApiError::Forbidden.into_response();
    }
    match crate::pg_einspeiser::list(&pool, &cfg.tenant).await {
        Ok(rows) => Json(serde_json::json!({ "einspeiser": rows })).into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `GET /api/v1/einspeiser/{einspeiser_id}` — one operator.
pub async fn get_einspeiser(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Path(einspeiser_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einspeiser", &cfg.tenant)
        .is_err()
    {
        return ApiError::Forbidden.into_response();
    }
    match crate::pg_einspeiser::find(&pool, &cfg.tenant, &einspeiser_id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `PUT /api/v1/einspeiser/{einspeiser_id}` — register or update an operator.
///
/// The § 19 UStG election lives here and nowhere else, so one call switches the
/// VAT on every future Gutschrift issued to this operator's plants at once.
pub async fn put_einspeiser(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Path(einspeiser_id): Path<String>,
    Json(body): Json<crate::pg_einspeiser::UpsertEinspeiser>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einspeiser", &cfg.tenant)
        .is_err()
    {
        return ApiError::Forbidden.into_response();
    }
    match crate::pg_einspeiser::upsert(&pool, &cfg.tenant, &einspeiser_id, &body).await {
        Ok(()) => (StatusCode::NO_CONTENT).into_response(),
        Err(e) => ApiError::bad_request(e.to_string()).into_response(),
    }
}
