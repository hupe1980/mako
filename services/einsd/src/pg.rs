//! PostgreSQL persistence for `einsd`.
//!
//! All monetary amounts use `billing::EuroAmount` (`Amount<5>`) internally for
//! precision validation before DB storage.  `Decimal` is used only at the
//! sqlx boundary (NUMERIC column mapping).
//! `foerderendedatum` is computed by the caller:
//!   - Standard EEG: `inbetriebnahme + 20 years` (§20 EEG 2023)
//!   - Repowering: `repowering_datum + 20 years` (§22 EEG 2023 — clock resets)
//!   - KWKG: computed from KWKG Förderdauer (§8 KWKG 2023)

use anyhow::Context as _;
use billing::EuroAmount;
use rust_decimal::Decimal;

/// Validate and round a computed settlement amount to 5dp.
///
/// Uses [`EuroAmount`] (`Amount<5>`) as an intermediate type to enforce
/// precision and overflow bounds, then converts back to `Decimal` for
/// `sqlx` DB storage.  Returns `Err` only when the value exceeds the
/// representable range (> ±92_233_720_368 EUR) — impossible in practice.
fn to_settlement_eur(d: Decimal) -> anyhow::Result<Decimal> {
    EuroAmount::checked_from_decimal(d)
        .map(|a| a.into_decimal())
        .map_err(|e| anyhow::anyhow!("settlement amount out of range: {e}"))
}
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ── EEG/KWKG Anlage ──────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen` and `PUT /api/v1/anlagen/{tr_id}`.
#[derive(Debug, Deserialize)]
pub struct AnlageUpsertRequest {
    pub tr_id: String,
    pub malo_id: String,
    pub melo_id: Option<String>,
    /// EEG law year of commissioning (2000/2004/2009/2012/2017/2021/2023) or `0` for KWKG.
    pub eeg_gesetz: i16,
    /// ISO 8601 commissioning date (Inbetriebnahmedatum).
    pub inbetriebnahme: String,
    /// Installed peak power in kWp (or kW_el for KWKG).
    pub leistung_kwp: Decimal,
    /// Generator type — see schema CHECK constraint for all valid values.
    pub erzeugungsart: String,
    /// EEG feed-in tariff / KWKG KWK-Zuschlag rate in ct/kWh.
    pub verguetungssatz_ct: Decimal,
    /// Settlement model.
    #[serde(default = "default_verguetung")]
    pub settlement_model: String,
    pub direktvermarktung: Option<bool>,
    /// Anzulegender Wert ct/kWh (Direktvermarktung / Ausschreibungswert).
    pub direktverm_aw_ct: Option<Decimal>,
    /// Direktvermarkter MP-ID.
    pub direktverm_mp_id: Option<String>,
    /// §38a EEG Mieterstrom surcharge ct/kWh.
    pub mieter_zuschlag_ct: Option<Decimal>,
    /// BNetzA Zuschlag-ID for Ausschreibungsanlagen.
    pub ausschreibungs_zuschlag_id: Option<String>,
    // ── Repowering (§22 EEG 2023) ───────────────────────────────────────────
    /// `true` when replacing old components with new higher-capacity ones.
    /// When set, `foerderendedatum` = `repowering_datum + 20 years` (clock reset).
    pub ist_repowering: Option<bool>,
    /// Original commissioning date before repowering (for audit trail).
    pub ursprungs_inbetriebnahme: Option<String>,
    /// Date of repowering — new `inbetriebnahme` for Förderungsdauer calculation.
    pub repowering_datum: Option<String>,
    // ── Zusammenlegung (§24 EEG 2023) ───────────────────────────────────────
    /// For merged plants: TR-ID of the parent entity.
    pub parent_tr_id: Option<String>,
    // ── KWKG (Kraft-Wärme-Kopplungsgesetz 2023) ──────────────────────────────
    /// KWKG Förderdauer in full-load hours (for plants >2 MW — e.g. 30000 h).
    pub kwk_foerderdauer_h: Option<i32>,
    /// KWKG Förderdauer in years (for plants ≤2 MW).
    pub kwk_foerderdauer_years: Option<i16>,
    // ── Flexibilitätsprämie (§50 EEG) ───────────────────────────────────────
    /// Registered flex capacity in kW (§50 EEG biomass flex premium).
    pub flex_leistung_kw: Option<Decimal>,
    /// Flexibilitätsprämie rate in ct/kWh.
    pub flex_praemie_ct_kwh: Option<Decimal>,
    pub notes: Option<String>,
}

fn default_verguetung() -> String {
    "VERGUETUNG".to_owned()
}

/// Stored plant record returned by GET endpoints.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AnlageRow {
    pub tr_id: String,
    pub tenant: String,
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub eeg_gesetz: i16,
    pub inbetriebnahme: Date,
    pub leistung_kwp: Decimal,
    pub erzeugungsart: String,
    pub verguetungssatz_ct: Decimal,
    pub foerderendedatum: Date,
    pub settlement_model: String,
    pub direktvermarktung: bool,
    pub direktverm_aw_ct: Option<Decimal>,
    pub direktverm_mp_id: Option<String>,
    pub mieter_zuschlag_ct: Option<Decimal>,
    pub ausschreibungs_zuschlag_id: Option<String>,
    pub ist_repowering: bool,
    pub ursprungs_inbetriebnahme: Option<Date>,
    pub repowering_datum: Option<Date>,
    pub parent_tr_id: Option<String>,
    pub kwk_foerderdauer_h: Option<i32>,
    pub kwk_foerderdauer_years: Option<i16>,
    pub kwk_strom_kwh_gesamt: Option<Decimal>,
    pub flex_leistung_kw: Option<Decimal>,
    pub flex_praemie_ct_kwh: Option<Decimal>,
    pub status: String,
    pub notes: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

pub async fn upsert_anlage(
    pool: &PgPool,
    tenant: &str,
    req: AnlageUpsertRequest,
) -> anyhow::Result<()> {
    use time::format_description::well_known::Iso8601;
    let inbetriebnahme =
        Date::parse(&req.inbetriebnahme, &Iso8601::DEFAULT).context("parse inbetriebnahme")?;

    let repowering_datum = req
        .repowering_datum
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse repowering_datum")?;

    let ursprungs_inbetriebnahme = req
        .ursprungs_inbetriebnahme
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse ursprungs_inbetriebnahme")?;

    let ist_repowering = req.ist_repowering.unwrap_or(false);

    // ── foerderendedatum logic ──────────────────────────────────────────────
    // Repowering (§22 EEG): clock resets to repowering_datum + 20 years.
    // KWKG: use kwk_foerderdauer_years if provided.
    // Standard EEG: inbetriebnahme + 20 years.
    let foerderendedatum = if ist_repowering {
        let basis = repowering_datum.unwrap_or(inbetriebnahme);
        basis
            .replace_year(basis.year() + 20)
            .context("compute repowering foerderendedatum")?
    } else if let Some(years) = req.kwk_foerderdauer_years {
        inbetriebnahme
            .replace_year(inbetriebnahme.year() + years as i32)
            .context("compute KWKG foerderendedatum")?
    } else {
        inbetriebnahme
            .replace_year(inbetriebnahme.year() + 20)
            .context("compute foerderendedatum")?
    };

    let settlement_model = if req.direktvermarktung.unwrap_or(false) {
        "DIREKTVERMARKTUNG"
    } else {
        &req.settlement_model
    };

    sqlx::query(
        r"INSERT INTO eeg_anlagen (
               tr_id, tenant, malo_id, melo_id, eeg_gesetz, inbetriebnahme,
               leistung_kwp, erzeugungsart, verguetungssatz_ct, foerderendedatum,
               direktvermarktung, direktverm_aw_ct, direktverm_mp_id,
               settlement_model, mieter_zuschlag_ct, ausschreibungs_zuschlag_id,
               ist_repowering, ursprungs_inbetriebnahme, repowering_datum,
               parent_tr_id,
               kwk_foerderdauer_h, kwk_foerderdauer_years,
               flex_leistung_kw, flex_praemie_ct_kwh,
               notes, updated_at
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               $7, $8, $9, $10,
               $11, $12, $13, $14, $15, $16,
               $17, $18, $19,
               $20,
               $21, $22,
               $23, $24,
               $25, now()
           )
           ON CONFLICT (tr_id, tenant) DO UPDATE SET
               malo_id                   = EXCLUDED.malo_id,
               melo_id                   = EXCLUDED.melo_id,
               eeg_gesetz                = EXCLUDED.eeg_gesetz,
               inbetriebnahme            = EXCLUDED.inbetriebnahme,
               leistung_kwp              = EXCLUDED.leistung_kwp,
               erzeugungsart             = EXCLUDED.erzeugungsart,
               verguetungssatz_ct        = EXCLUDED.verguetungssatz_ct,
               foerderendedatum          = EXCLUDED.foerderendedatum,
               direktvermarktung         = EXCLUDED.direktvermarktung,
               direktverm_aw_ct          = EXCLUDED.direktverm_aw_ct,
               direktverm_mp_id          = EXCLUDED.direktverm_mp_id,
               settlement_model          = EXCLUDED.settlement_model,
               mieter_zuschlag_ct        = EXCLUDED.mieter_zuschlag_ct,
               ausschreibungs_zuschlag_id = EXCLUDED.ausschreibungs_zuschlag_id,
               ist_repowering            = EXCLUDED.ist_repowering,
               ursprungs_inbetriebnahme  = EXCLUDED.ursprungs_inbetriebnahme,
               repowering_datum          = EXCLUDED.repowering_datum,
               parent_tr_id              = EXCLUDED.parent_tr_id,
               kwk_foerderdauer_h        = EXCLUDED.kwk_foerderdauer_h,
               kwk_foerderdauer_years    = EXCLUDED.kwk_foerderdauer_years,
               flex_leistung_kw          = EXCLUDED.flex_leistung_kw,
               flex_praemie_ct_kwh       = EXCLUDED.flex_praemie_ct_kwh,
               notes                     = EXCLUDED.notes,
               updated_at                = now()",
    )
    .bind(&req.tr_id)
    .bind(tenant)
    .bind(&req.malo_id)
    .bind(&req.melo_id)
    .bind(req.eeg_gesetz)
    .bind(inbetriebnahme)
    .bind(req.leistung_kwp)
    .bind(&req.erzeugungsart)
    .bind(req.verguetungssatz_ct)
    .bind(foerderendedatum)
    .bind(req.direktvermarktung.unwrap_or(false))
    .bind(req.direktverm_aw_ct)
    .bind(&req.direktverm_mp_id)
    .bind(settlement_model)
    .bind(req.mieter_zuschlag_ct)
    .bind(&req.ausschreibungs_zuschlag_id)
    .bind(ist_repowering)
    .bind(ursprungs_inbetriebnahme)
    .bind(repowering_datum)
    .bind(&req.parent_tr_id)
    .bind(req.kwk_foerderdauer_h)
    .bind(req.kwk_foerderdauer_years)
    .bind(req.flex_leistung_kw)
    .bind(req.flex_praemie_ct_kwh)
    .bind(&req.notes)
    .execute(pool)
    .await
    .context("upsert eeg_anlage")?;
    Ok(())
}

pub async fn fetch_anlage(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
) -> anyhow::Result<Option<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>("SELECT * FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2")
        .bind(tr_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("fetch eeg_anlage")
}

#[derive(Debug, Deserialize)]
pub struct AnlagenQuery {
    pub erzeugungsart: Option<String>,
    pub settlement_model: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_anlagen(
    pool: &PgPool,
    tenant: &str,
    q: &AnlagenQuery,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT * FROM eeg_anlagen
          WHERE tenant = $1
            AND ($2::text IS NULL OR erzeugungsart = $2)
            AND ($3::text IS NULL OR settlement_model = $3)
            AND ($4::text IS NULL OR status = $4)
          ORDER BY foerderendedatum ASC
          LIMIT $5",
    )
    .bind(tenant)
    .bind(&q.erzeugungsart)
    .bind(&q.settlement_model)
    .bind(q.status.as_deref().or(Some("aktiv")))
    .bind(q.limit.unwrap_or(200).min(2000))
    .fetch_all(pool)
    .await
    .context("list eeg_anlagen")
}

/// Plants whose `foerderendedatum` is within `horizon_days` of today.
pub async fn list_expiring(
    pool: &PgPool,
    tenant: &str,
    horizon_days: i32,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT * FROM eeg_anlagen
          WHERE tenant = $1
            AND status = 'aktiv'
            AND foerderendedatum BETWEEN CURRENT_DATE AND CURRENT_DATE + ($2 * INTERVAL '1 day')
          ORDER BY foerderendedatum ASC",
    )
    .bind(tenant)
    .bind(horizon_days)
    .fetch_all(pool)
    .await
    .context("list_expiring")
}

pub async fn decommission_anlage(pool: &PgPool, tenant: &str, tr_id: &str) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        "UPDATE eeg_anlagen SET status = 'abgemeldet', updated_at = now() \
         WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'",
    )
    .bind(tr_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("decommission_anlage")?;
    Ok(rows.rows_affected() > 0)
}

// ── Settlement receipts ───────────────────────────────────────────────────────

/// Input for a monthly settlement calculation.
pub struct SettleInput {
    pub tr_id: String,
    pub tenant: String,
    pub billing_year: i16,
    pub billing_month: i16,
    /// Einspeisemenge kWh for the billing month.
    /// `None` → `status = 'no_data'`.
    pub einspeisemenge_kwh: Option<Decimal>,
    /// Monthly average EPEX price in ct/kWh.
    /// Required for DIREKTVERMARKTUNG, POST_EEG_SPOT, KWKG_ZUSCHLAG.
    /// `None` → `status = 'price_missing'` for those models.
    pub epex_avg_ct_kwh: Option<Decimal>,
    pub settlement_model: String,
    /// Fixed tariff / KWK-Zuschlag rate in ct/kWh.
    pub verguetungssatz_ct: Decimal,
    /// Anzulegender Wert for Direktvermarktung / Ausschreibung.
    pub direktverm_aw_ct: Option<Decimal>,
    /// §38a Mieterstrom surcharge ct/kWh.
    pub mieter_zuschlag_ct: Option<Decimal>,
    /// Flexibilitätsprämie rate ct/kWh (§50 EEG, biomass only).
    pub flex_praemie_ct_kwh: Option<Decimal>,
}

#[derive(Debug, Serialize)]
pub struct SettleResult {
    pub id: Uuid,
    pub tr_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub settlement_model: String,
    pub einspeisemenge_kwh: Option<Decimal>,
    pub settlement_eur: Option<Decimal>,
    pub status: String,
}

/// Run the settlement calculation and persist the result.
///
/// | Model | Formula | Legal basis |
/// |---|---|---|
/// | `VERGUETUNG` | `kwh × verguetungssatz_ct / 100` | §21 EEG 2023 |
/// | `MIETERSTROM` | VERGUETUNG + `kwh × mieter_zuschlag_ct / 100` | §38a EEG 2023 |
/// | `DIREKTVERMARKTUNG` | `max(0, aw_ct − epex) × kwh / 100` | §20 EEG 2023 |
/// | `AUSSCHREIBUNG` | same as DIREKTVERMARKTUNG (tendering AW) | §§22a,28 EEG 2023 |
/// | `POST_EEG_SPOT` | `kwh × epex_avg_ct / 100` | post-Förderung |
/// | `EIGENVERBRAUCH` | EUR 0 | self-consumption |
/// | `KWKG_ZUSCHLAG` | `kwh × verguetungssatz_ct / 100` (on top of market price) | §7 KWKG 2023 |
/// | `FLEXIBILITAET` | VERGUETUNG + `kwh × flex_praemie_ct / 100` | §50 EEG 2023 |
pub async fn run_settlement(pool: &PgPool, input: SettleInput) -> anyhow::Result<SettleResult> {
    let (settlement_eur, status) = match (input.einspeisemenge_kwh, input.settlement_model.as_str())
    {
        (None, _) => (None, "no_data"),
        (Some(_), "EIGENVERBRAUCH") => (Some(EuroAmount::ZERO.into_decimal()), "calculated"),

        (Some(kwh), "VERGUETUNG") => {
            let base = kwh * input.verguetungssatz_ct / Decimal::from(100);
            (Some(to_settlement_eur(base)?), "calculated")
        }

        (Some(kwh), "MIETERSTROM") => {
            // §38a EEG: base Einspeisevergütung + Mieterstrom-Zuschlag
            let base = kwh * input.verguetungssatz_ct / Decimal::from(100);
            let zuschlag = input
                .mieter_zuschlag_ct
                .map(|z| kwh * z / Decimal::from(100))
                .unwrap_or(Decimal::ZERO);
            (Some(to_settlement_eur(base + zuschlag)?), "calculated")
        }

        (Some(kwh), "DIREKTVERMARKTUNG") | (Some(kwh), "AUSSCHREIBUNG") => {
            // Gleitende Marktprämie: max(0, AW − EPEX_monatsmittel)
            match (input.direktverm_aw_ct, input.epex_avg_ct_kwh) {
                (Some(aw), Some(epex)) => {
                    let praemie_ct = (aw - epex).max(Decimal::ZERO);
                    (
                        Some(to_settlement_eur(kwh * praemie_ct / Decimal::from(100))?),
                        "calculated",
                    )
                }
                _ => (None, "price_missing"),
            }
        }

        (Some(kwh), "POST_EEG_SPOT") => match input.epex_avg_ct_kwh {
            Some(epex) => (
                Some(to_settlement_eur(kwh * epex / Decimal::from(100))?),
                "calculated",
            ),
            None => (None, "price_missing"),
        },

        (Some(kwh), "KWKG_ZUSCHLAG") => {
            // KWK-Zuschlag is paid on top of the electricity market price.
            // The total payment = market_price + KWK-Zuschlag.
            // We record only the KWK-Zuschlag component here (market revenue tracked separately).
            let kwk_eur = to_settlement_eur(kwh * input.verguetungssatz_ct / Decimal::from(100))?;
            (Some(kwk_eur), "calculated")
        }

        (Some(kwh), "FLEXIBILITAET") => {
            // §50 EEG: base Einspeisevergütung + Flexibilitätsprämie
            let base = kwh * input.verguetungssatz_ct / Decimal::from(100);
            let flex = input
                .flex_praemie_ct_kwh
                .map(|f| kwh * f / Decimal::from(100))
                .unwrap_or(Decimal::ZERO);
            (Some(to_settlement_eur(base + flex)?), "calculated")
        }

        _ => (None, "error"),
    };

    let id = Uuid::new_v4();
    sqlx::query(
        r"INSERT INTO settlement_receipts
              (id, tr_id, tenant, billing_year, billing_month,
               settlement_model, einspeisemenge_kwh, settlement_eur, status)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
          ON CONFLICT (tr_id, tenant, billing_year, billing_month) DO UPDATE
          SET settlement_model   = EXCLUDED.settlement_model,
              einspeisemenge_kwh = EXCLUDED.einspeisemenge_kwh,
              settlement_eur     = EXCLUDED.settlement_eur,
              status             = EXCLUDED.status,
              settled_at         = now()",
    )
    .bind(id)
    .bind(&input.tr_id)
    .bind(&input.tenant)
    .bind(input.billing_year)
    .bind(input.billing_month)
    .bind(&input.settlement_model)
    .bind(input.einspeisemenge_kwh)
    .bind(settlement_eur)
    .bind(status)
    .execute(pool)
    .await
    .context("persist settlement")?;

    Ok(SettleResult {
        id,
        tr_id: input.tr_id,
        billing_year: input.billing_year,
        billing_month: input.billing_month,
        settlement_model: input.settlement_model,
        einspeisemenge_kwh: input.einspeisemenge_kwh,
        settlement_eur,
        status: status.to_owned(),
    })
}

pub async fn list_settlement_receipts(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    limit: i64,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let rows = sqlx::query(
        r"SELECT id, tr_id, billing_year, billing_month, settlement_model,
                 einspeisemenge_kwh, settlement_eur, status, settled_at
          FROM settlement_receipts
          WHERE tr_id = $1 AND tenant = $2
          ORDER BY billing_year DESC, billing_month DESC
          LIMIT $3",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_settlement_receipts")?;

    Ok(rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.try_get::<Uuid, _>("id").ok().map(|u| u.to_string()),
                "tr_id": r.try_get::<String, _>("tr_id").ok(),
                "billing_year": r.try_get::<i16, _>("billing_year").ok(),
                "billing_month": r.try_get::<i16, _>("billing_month").ok(),
                "settlement_model": r.try_get::<String, _>("settlement_model").ok(),
                "einspeisemenge_kwh": r.try_get::<Option<Decimal>, _>("einspeisemenge_kwh").ok().flatten(),
                "settlement_eur": r.try_get::<Option<Decimal>, _>("settlement_eur").ok().flatten(),
                "status": r.try_get::<String, _>("status").ok(),
                "settled_at": r.try_get::<OffsetDateTime, _>("settled_at").ok().map(|t| t.to_string()),
            })
        })
        .collect())
}

// ── EPEX monthly prices ───────────────────────────────────────────────────────

pub async fn lookup_verguetungssatz(
    pool: &PgPool,
    erzeugungsart: &str,
    leistung_kwp: Decimal,
    inbetriebnahme: &str,
) -> anyhow::Result<Option<Decimal>> {
    use time::format_description::well_known::Iso8601;
    let date = Date::parse(inbetriebnahme, &Iso8601::DEFAULT)
        .context("parse inbetriebnahme for lookup")?;

    let row = sqlx::query(
        r"SELECT verguetungssatz_ct
          FROM eeg_verguetungssaetze
          WHERE erzeugungsart = $1
            AND leistung_min_kwp <= $2
            AND (leistung_max_kwp IS NULL OR leistung_max_kwp > $2)
            AND billing_start <= $3
            AND (billing_end IS NULL OR billing_end >= $3)
          ORDER BY billing_start DESC
          LIMIT 1",
    )
    .bind(erzeugungsart)
    .bind(leistung_kwp)
    .bind(date)
    .fetch_optional(pool)
    .await
    .context("lookup_verguetungssatz")?;

    Ok(row.and_then(|r| r.try_get::<Decimal, _>("verguetungssatz_ct").ok()))
}

pub async fn upsert_epex_price(
    pool: &PgPool,
    year: i16,
    month: i16,
    avg_ct_kwh: Decimal,
    source: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"INSERT INTO epex_monthly_prices (billing_year, billing_month, avg_ct_kwh, source)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (billing_year, billing_month) DO UPDATE
          SET avg_ct_kwh = EXCLUDED.avg_ct_kwh,
              source     = EXCLUDED.source,
              imported_at = now()",
    )
    .bind(year)
    .bind(month)
    .bind(avg_ct_kwh)
    .bind(source)
    .execute(pool)
    .await
    .context("upsert_epex_price")?;
    Ok(())
}

pub async fn fetch_epex_price(
    pool: &PgPool,
    year: i16,
    month: i16,
) -> anyhow::Result<Option<Decimal>> {
    let row = sqlx::query(
        "SELECT avg_ct_kwh FROM epex_monthly_prices WHERE billing_year = $1 AND billing_month = $2",
    )
    .bind(year)
    .bind(month)
    .fetch_optional(pool)
    .await
    .context("fetch_epex_price")?;
    Ok(row.and_then(|r| r.try_get::<Decimal, _>("avg_ct_kwh").ok()))
}
