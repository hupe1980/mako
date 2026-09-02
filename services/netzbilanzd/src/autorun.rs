//! Convenience endpoints that assemble a billing run from other services.
//!
//! Both endpoints here are thin: they gather inputs, build ordinary
//! [`crate::request::BillingRunRequest`] positions, and hand them to
//! [`crate::handlers::run_billing`]'s machinery. Nothing settles money here —
//! anything they can do, an operator can do by POSTing the same positions.

use std::sync::Arc;

use axum::{Extension, Json, extract::Path, http::StatusCode};
use mako_markt::marktd_client::MarktdClient;
use mako_service::{ApiError, ApiResult};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;

use crate::config::NetzbilanzConfig;
use crate::pg;
use crate::request::{BillingPositionRequest, MmmRequest, NneRequest, SettlementRequest};

type Cfg = Extension<Arc<NetzbilanzConfig>>;

/// The last day of a calendar month.
fn month_end(year: i32, month: u8) -> ApiResult<time::Date> {
    let m =
        time::Month::try_from(month).map_err(|_| ApiError::bad_request("month must be 1–12"))?;
    let first = time::Date::from_calendar_date(year, m, 1)
        .map_err(|_| ApiError::bad_request("invalid year/month"))?;
    Ok(first
        .replace_day(time::util::days_in_month(m, year))
        .unwrap_or(first))
}

// ── MMM auto-run ──────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/mmm-run/{malo_id}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmmAutoRunRequest {
    /// Netzbetreiber MP-ID — the invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — the invoice recipient.
    pub lf_mp_id: String,
    /// `"Strom"` or `"Gas"`. It decides which price series is fetched *and*
    /// which balancing day `edmd` aggregates over — gas balances on the
    /// 06:00 Gastag, so a Gas saldo aggregated over calendar days misplaces six
    /// hours of every day's energy.
    pub sparte: grid_billing::Sparte,
    /// Calendar year of the settlement month.
    pub period_year: i32,
    /// Calendar month of the settlement month.
    pub period_month: u8,
    /// The **bilanzierte** (profile-allocated) quantity for the month, in kWh.
    ///
    /// Required, and not obtainable from `edmd`: it is what the Bilanzkreis was
    /// charged from the load profile, which lives on the balancing side. `edmd`
    /// holds only the measured half.
    pub bilanziert_kwh: Decimal,
    /// Invoice issue date. Defaults to today.
    #[serde(default)]
    pub invoice_date: Option<time::Date>,
    /// Payment due date. Defaults to 30 days after the issue date.
    #[serde(default)]
    pub due_date: Option<time::Date>,
    /// Rechnungskreis for the generated invoice number.
    #[serde(default)]
    pub rechnungskreis: Option<String>,
    /// Override the published Mehrmengen price, in ct/kWh.
    #[serde(default)]
    pub mehr_preis_ct_per_kwh: Option<Decimal>,
    /// Override the published Mindermengen price, in ct/kWh.
    #[serde(default)]
    pub minder_preis_ct_per_kwh: Option<Decimal>,
    /// SLP Lastprofil. Auto-derived from `marktd` when absent.
    #[serde(default)]
    pub lastprofil: Option<String>,
    /// Who holds §3g Wiederverkäufer status, evidenced by a *USt 1 TH*.
    ///
    /// A Mehr-/Mindermenge is a Lieferung, so §13b Abs. 2 Nr. 5 Buchst. b can
    /// shift the tax to the recipient. Defaults to neither party holding it.
    #[serde(default)]
    pub wiederverkaeufer: grid_billing::Wiederverkaeuferstatus,
}

/// `POST /api/v1/billing/mmm-run/{malo_id}`
///
/// Settles one MaLo's Mehr-/Mindermengensaldo for a month, taking the measured
/// half from `edmd` and the prices from `marktd`.
///
/// # Errors
///
/// - `404` when `edmd` holds no readings for the MaLo and month.
/// - `409` when the month is already billed.
/// - `422` when the settlement is not computable, when both quantities are
///   zero, or when `edmd` is unreachable or unconfigured.
pub async fn post_mmm_run(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Cfg,
    Extension(http): Extension<Arc<reqwest::Client>>,
    Path(malo_id): Path<String>,
    Json(req): Json<MmmAutoRunRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let period_to = month_end(req.period_year, req.period_month)?;
    let period_from = period_to.replace_day(1).unwrap_or(period_to);

    let gemessen_kwh = fetch_gemessen(&http, &cfg, &malo_id, &req).await?;
    if gemessen_kwh.is_zero() && req.bilanziert_kwh.is_zero() {
        return Err(ApiError::unprocessable(
            "both the measured and the bilanzierte quantity are zero — nothing to settle",
        ));
    }

    let invoice_date = req.invoice_date.unwrap_or_else(mako_fristen::heute);
    let due_date = req
        .due_date
        .unwrap_or_else(|| invoice_date.saturating_add(time::Duration::days(30)));

    let mut position = BillingPositionRequest {
        malo_id: malo_id.clone(),
        period_from,
        period_to,
        cadence: None,
        abschlaege: Vec::new(),
        settlement: SettlementRequest::Mmm(MmmRequest {
            nb_mp_id: req.nb_mp_id.clone(),
            lf_mp_id: req.lf_mp_id.clone(),
            sparte: req.sparte,
            gemessen_kwh,
            bilanziert_kwh: req.bilanziert_kwh,
            mehr_preis_ct_per_kwh: req.mehr_preis_ct_per_kwh,
            minder_preis_ct_per_kwh: req.minder_preis_ct_per_kwh,
            lastprofil: req.lastprofil.clone(),
            wiederverkaeufer: req.wiederverkaeufer,
        }),
    };

    let drafted = crate::handlers::draft_positions(
        &pool,
        &marktd,
        &cfg,
        std::slice::from_mut(&mut position),
        invoice_date,
        due_date,
        req.rechnungskreis.as_deref(),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "malo_id": malo_id,
            "sparte": format!("{:?}", req.sparte),
            "period": format!("{period_from}/{period_to}"),
            "gemessen_kwh": gemessen_kwh.to_string(),
            "bilanziert_kwh": req.bilanziert_kwh.to_string(),
            "drafts": drafted,
        })),
    ))
}

/// The measured quantity for the month, from `edmd`.
async fn fetch_gemessen(
    http: &reqwest::Client,
    cfg: &NetzbilanzConfig,
    malo_id: &str,
    req: &MmmAutoRunRequest,
) -> ApiResult<Decimal> {
    let edmd = cfg
        .edmd(http.clone())
        .ok_or_else(|| ApiError::Unprocessable("edmd_url is not configured".to_owned()))?;

    // `sparte` is passed through: edmd aggregates a Gas saldo over the 06:00
    // Gastag and a Strom saldo over the calendar day, so omitting it settles gas
    // on the wrong day boundary.
    let sparte = match req.sparte {
        grid_billing::Sparte::Strom => "strom",
        grid_billing::Sparte::Gas => "gas",
    };
    let path = format!(
        "/api/v1/imbalance/{malo_id}/{}/{}",
        req.period_year, req.period_month
    );
    let request = edmd.get(&path).query(&[
        ("sparte", sparte.to_owned()),
        ("bilanziert_kwh", req.bilanziert_kwh.to_string()),
    ]);
    let body: serde_json::Value = edmd
        .json(request)
        .await
        .map_err(|e| ApiError::Unprocessable(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    body.get("gemessen_kwh")
        .and_then(|v| match v {
            serde_json::Value::String(s) => s.parse().ok(),
            serde_json::Value::Number(n) => n.to_string().parse().ok(),
            _ => None,
        })
        .ok_or_else(|| {
            ApiError::Unprocessable("edmd imbalance response carries no gemessen_kwh".to_owned())
        })
}

// ── §42b EnWG GGV ─────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/ggv-nne/{ggv_malo_id}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GgvNneRequest {
    /// Netzbetreiber MP-ID — the invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — the invoice recipient.
    pub lf_mp_id: String,
    /// Delivery period start.
    pub period_from: time::Date,
    /// Delivery period end.
    pub period_to: time::Date,
    /// Invoice issue date. Defaults to today.
    #[serde(default)]
    pub invoice_date: Option<time::Date>,
    /// Payment due date. Defaults to 30 days after the issue date.
    #[serde(default)]
    pub due_date: Option<time::Date>,
    /// Rechnungskreis for the generated invoice numbers.
    #[serde(default)]
    pub rechnungskreis: Option<String>,
    /// NNE Arbeitspreis in ct/kWh, from the `PreisblattNetznutzung`.
    pub arbeitspreis_ct_per_kwh: Decimal,
    /// Konzessionsabgabe — rate and KAV §2 group together.
    #[serde(default)]
    pub konzessionsabgabe: Option<grid_billing::Konzessionsabgabe>,
    /// Per-tenant metered consumption, keyed by MaLo-ID.
    ///
    /// Required. §42b attributes the Netzentgelt to each tenant Marktlokation,
    /// and an equal split is not an attribution — it is a guess that bills one
    /// tenant for another's consumption.
    pub tenant_consumption: std::collections::BTreeMap<String, Decimal>,
}

/// `POST /api/v1/billing/ggv-nne/{ggv_malo_id}`
///
/// §42b EnWG Gemeinschaftliche Gebäudeversorgung: the NB bills each tenant
/// Marktlokation for its own Netzentgelt.
///
/// # Errors
///
/// - `400` when no tenant consumption was supplied.
/// - `409` when one of the tenants' periods is already billed.
/// - `422` when a settlement is not computable.
pub async fn post_ggv_nne(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Cfg,
    Path(ggv_malo_id): Path<String>,
    Json(req): Json<GgvNneRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if req.tenant_consumption.is_empty() {
        return Err(ApiError::bad_request(
            "tenant_consumption must name at least one tenant MaLo and its metered kWh",
        ));
    }
    if req.period_to < req.period_from {
        return Err(ApiError::bad_request("period_to is before period_from"));
    }

    let invoice_date = req.invoice_date.unwrap_or_else(mako_fristen::heute);
    let due_date = req
        .due_date
        .unwrap_or_else(|| invoice_date.saturating_add(time::Duration::days(30)));

    let total_kwh: Decimal = req.tenant_consumption.values().copied().sum();
    for (tenant_malo, kwh) in &req.tenant_consumption {
        if *kwh < Decimal::ZERO {
            return Err(ApiError::unprocessable(format!(
                "tenant {tenant_malo}: consumption {kwh} kWh is negative"
            )));
        }
    }

    // The shares are of the sum actually supplied, so they describe the whole of
    // it and have to add to exactly 100 %. Rounding each one on its own does not
    // give that: three equal tenants take 33.3333 % each and the statement
    // accounts for 99.9999 % of the supply. `proportional_split` allocates the
    // hundred by largest remainder instead.
    let share_pct = ggv_shares(&req.tenant_consumption, total_kwh);

    let mut positions: Vec<BillingPositionRequest> = Vec::new();
    let mut attribution = Vec::new();

    for ((tenant_malo, kwh), share_pct) in req.tenant_consumption.iter().zip(share_pct) {
        attribution.push(serde_json::json!({
            "tenant_malo": tenant_malo,
            "kwh": kwh.to_string(),
            "share_pct": share_pct.to_string(),
        }));

        positions.push(BillingPositionRequest {
            malo_id: tenant_malo.clone(),
            period_from: req.period_from,
            period_to: req.period_to,
            cadence: None,
            abschlaege: Vec::new(),
            settlement: SettlementRequest::Nne(Box::new(NneRequest {
                nb_mp_id: req.nb_mp_id.clone(),
                lf_mp_id: req.lf_mp_id.clone(),
                sparte: grid_billing::Sparte::Strom,
                arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(
                    grid_billing::MengePreis {
                        menge_kwh: *kwh,
                        preis_ct_per_kwh: req.arbeitspreis_ct_per_kwh,
                    },
                ),
                leistungspreis: None,
                grundpreis: None,
                konzessionsabgabe: req.konzessionsabgabe,
                blindarbeit: None,
                gas_kapazitaet: None,
                letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::default(),
                enfg_jahresvorverbrauch_kwh: None,
                sect19_umlage_ct_per_kwh: None,
                offshore_umlage_ct_per_kwh: None,
                kwkg_umlage_ct_per_kwh: None,
                netzebene: None,
                sect19: None,
                jahreshoechstleistung_kw: None,
                jahresarbeit_kwh: None,
                tariff_sheet_id: None,
            })),
        });
    }

    // One transaction for the whole building: a GGV run that bills six of nine
    // tenants and reports success is worse than one that bills none.
    let drafted = crate::handlers::draft_positions(
        &pool,
        &marktd,
        &cfg,
        &mut positions,
        invoice_date,
        due_date,
        req.rechnungskreis.as_deref(),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ggv_malo_id": ggv_malo_id,
            "tenant_count": req.tenant_consumption.len(),
            "total_kwh": total_kwh.to_string(),
            "attribution": attribution,
            "drafts": drafted,
            "legal_basis": "§42b EnWG",
        })),
    ))
}

// ── Background workers ────────────────────────────────────────────────────────

/// Enqueue an alert CloudEvent on the outbox.
///
/// Alerts take the same delivery path as the business events rather than
/// posting for themselves: one retry policy, one dead-letter queue, and a
/// receiver that is down when the timer fires still hears about it.
async fn enqueue_alert(
    pool: &PgPool,
    cfg: &NetzbilanzConfig,
    ce_type: &'static str,
    payload: serde_json::Value,
) {
    let ce = mako_service::CloudEvent::new(
        mako_service::source("netzbilanzd", &cfg.tenant),
        ce_type,
        String::new(),
        payload,
    )
    .without_subject();
    let enqueued = async {
        let mut conn = pool.acquire().await?;
        mako_service::outbox::enqueue(&mut conn, &ce).await
    }
    .await;
    if let Err(e) = enqueued {
        tracing::warn!(error = %e, ce_type, "netzbilanzd: could not enqueue alert");
    }
}

/// Emit `de.netzbilanz.invoic.dispatch-overdue` for drafts stuck undispatched.
pub async fn dispatch_overdue_alert(pool: &PgPool, cfg: &NetzbilanzConfig, stale_hours: i64) {
    if cfg.erp_webhook_url.is_none() {
        return;
    }
    let rows = match pg::list_undispatched_stale(pool, &cfg.tenant, stale_hours, 100).await {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "netzbilanzd: dispatch-overdue query failed");
            return;
        }
    };
    tracing::warn!(
        count = rows.len(),
        stale_hours,
        "netzbilanzd: drafts undispatched past their window"
    );
    enqueue_alert(
        pool,
        cfg,
        mako_events::netzbilanz::INVOIC_DISPATCH_OVERDUE,
        serde_json::json!({
            "tenant": cfg.tenant,
            "stale_hours": stale_hours,
            "undispatched_count": rows.len(),
            "drafts": rows.iter().map(|r| serde_json::json!({
                "draft_id": r.id,
                "malo_id": r.malo_id,
                "rechnungsnummer": r.rechnungsnummer,
                "check_outcome": r.check_outcome,
                "due_date": r.due_date.to_string(),
                // What is actually outstanding — the gross less any Abschlag
                // already invoiced. An overdue report that names the gross
                // overstates the exposure on every settled period.
                "zu_zahlen_eur": crate::billing::format_eur(r.zu_zahlen_eur_units),
            })).collect::<Vec<_>>(),
            "action": "PUT /api/v1/billing/drafts/{id}/dispatch, or reject the draft",
        }),
    )
    .await;
}

/// Emit `de.netzbilanz.kostenblatt.deadline-approaching` before the 15th.
pub async fn kostenblatt_deadline_alert(pool: &PgPool, cfg: &NetzbilanzConfig) {
    if cfg.erp_webhook_url.is_none() {
        return;
    }
    // The 15th and the Aktivierungsmonat are German calendar dates.
    let today = mako_fristen::heute();
    let day = today.day();
    // The submission is due on the 15th for the *previous* month's activations.
    if !(10..=14).contains(&day) {
        return;
    }
    let (year, month) = if today.month() as u8 > 1 {
        (today.year(), today.month() as u8 - 1)
    } else {
        (today.year() - 1, 12)
    };

    let pending = match pg::list_kostenblatt(
        pool,
        &cfg.tenant,
        i16::try_from(year).unwrap_or_default(),
        i16::from(month),
        Some("pending"),
    )
    .await
    {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "netzbilanzd: Kostenblatt deadline query failed");
            return;
        }
    };

    let days_left = 15_u8.saturating_sub(day);
    tracing::warn!(
        count = pending.len(),
        year,
        month,
        days_left,
        "netzbilanzd: Kostenblatt still pending before the 15th"
    );
    enqueue_alert(
        pool,
        cfg,
        mako_events::netzbilanz::KOSTENBLATT_DEADLINE_APPROACHING,
        serde_json::json!({
            "tenant": cfg.tenant,
            "period_year": year,
            "period_month": month,
            "pending_count": pending.len(),
            "days_until_deadline": days_left,
            "deadline": format!("{}-{:02}-15", today.year(), today.month() as u8),
            "action": format!("POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}"),
        }),
    )
    .await;
}


/// Each tenant's share of the supplied energy in percent, summing to exactly 100.
///
/// Ordered the way the map iterates, so the caller can zip it back onto the
/// tenants. An empty supply has no shares to state and answers zero for each.
fn ggv_shares(
    consumption: &std::collections::BTreeMap<String, Decimal>,
    total_kwh: Decimal,
) -> Vec<Decimal> {
    if total_kwh.is_zero() {
        return vec![Decimal::ZERO; consumption.len()];
    }
    let fractions: Vec<Decimal> = consumption.values().map(|kwh| kwh / total_kwh).collect();
    billing::proportional_split(Decimal::from(100), &fractions, 4)
        .unwrap_or_else(|_| vec![Decimal::ZERO; consumption.len()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    /// Month ends land on the real last day, leap years included.
    #[test]
    fn a_month_ends_on_its_last_day() {
        assert_eq!(
            month_end(2026, 1).expect("january"),
            time::macros::date!(2026 - 01 - 31)
        );
        assert_eq!(
            month_end(2026, 2).expect("february"),
            time::macros::date!(2026 - 02 - 28)
        );
        assert_eq!(
            month_end(2028, 2).expect("leap february"),
            time::macros::date!(2028 - 02 - 29)
        );
        assert!(month_end(2026, 13).is_err());
        assert!(month_end(2026, 0).is_err());
    }

    fn consumption(kwh: &[Decimal]) -> std::collections::BTreeMap<String, Decimal> {
        kwh.iter()
            .enumerate()
            .map(|(i, k)| (format!("tenant-{i}"), *k))
            .collect()
    }

    /// GGV shares are of the sum actually supplied, so they add to 100 %.
    #[test]
    fn ggv_shares_add_to_one_hundred_percent() {
        let c = consumption(&[dec!(1200), dec!(800), dec!(2000)]);
        let total: Decimal = c.values().copied().sum();
        assert_eq!(
            super::ggv_shares(&c, total),
            vec![dec!(30.0), dec!(20.0), dec!(50.0)]
        );
    }

    /// The case per-share rounding gets wrong: three equal tenants each take a
    /// third, which no 4-dp figure represents, so one of them carries the
    /// remainder rather than the statement losing it.
    #[test]
    fn ggv_shares_of_equal_tenants_still_add_to_one_hundred() {
        for n in 3..=7u32 {
            let c = consumption(&vec![dec!(1000); n as usize]);
            let total: Decimal = c.values().copied().sum();
            let shares = super::ggv_shares(&c, total);
            assert_eq!(shares.len(), n as usize);
            assert_eq!(
                shares.iter().sum::<Decimal>(),
                Decimal::from(100),
                "{n} equal tenants"
            );
        }
    }

    /// Nothing supplied is no shares to state, not a division by zero.
    #[test]
    fn no_supply_states_no_shares() {
        let c = consumption(&[Decimal::ZERO, Decimal::ZERO]);
        assert_eq!(
            super::ggv_shares(&c, Decimal::ZERO),
            vec![Decimal::ZERO, Decimal::ZERO]
        );
    }
}
