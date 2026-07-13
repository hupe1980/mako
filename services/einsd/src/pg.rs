//! PostgreSQL persistence for `einsd`.
//!
//! Settlement formulas are implemented in the [`eeg-billing`] crate.
//! This module is responsible for:
//! - Plant registration and lifecycle (CRUD on `eeg_anlagen`)
//! - Persisting settlement receipts (`settlement_receipts`)
//! - KWKG hour-limit state tracking (`kwk_strom_kwh_gesamt` column)
//! - EPEX monthly price storage
//!
//! [`eeg-billing`]: eeg_billing

use anyhow::Context as _;
use rust_decimal::Decimal;
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
    pub flex_praemie_ct_kwh: Option<Decimal>, // ── MaStR + Bankverbindung ────────────────────────────────────────────────────────────────
    /// Whether the plant is registered in the Marktstammdatenregister (MaStR).
    ///
    /// When `false`: §52 penalty applies until registration is confirmed.
    /// - EEG 2023 plants: €10/kW/month Pflichtzahlung (§52 Abs. 1 Nr. 11 EEG 2023)
    /// - EEG ≤2021 plants: Vergütung = 0 (old §52/§47 via §100 Übergangsregelung)
    ///
    /// Confirm via `POST /api/v1/anlagen/{tr_id}/mastr-registrierung`.
    #[serde(default = "default_mastr_true")]
    pub mastr_registriert: bool,
    /// MaStR Registrierungsnummer (e.g. `"SEE900000000001"`).
    pub mastr_nummer: Option<String>,
    /// Date of MaStR registration (ISO 8601).
    pub mastr_datum: Option<String>,
    // ── Bankverbindung for EEG Vergütung SEPA CT payment ────────────────────────────────
    /// IBAN of the plant operator for monthly EEG Vergütung payment (SEPA CT).
    pub bank_iban: Option<String>,
    /// BIC/SWIFT of operator bank (optional, derivable from IBAN for SEPA IBANs).
    pub bank_bic: Option<String>,
    /// Full name of payment recipient (Zahlungsempfänger).
    pub zahlungsempfaenger: Option<String>,
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
    // MaStR + Bankverbindung (migration 0002)
    pub mastr_registriert: bool,
    pub mastr_nummer: Option<String>,
    pub mastr_datum: Option<Date>,
    pub bank_iban: Option<String>,
    pub bank_bic: Option<String>,
    pub zahlungsempfaenger: Option<String>,
    pub notes: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn default_mastr_true() -> bool {
    true
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

    let mastr_datum = req
        .mastr_datum
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse mastr_datum")?;

    let ist_repowering = req.ist_repowering.unwrap_or(false);

    // ── foerderendedatum logic ──────────────────────────────────────────────
    // §25 Abs. 1 Satz 2 EEG 2023: statutory (non-tender) plants extend to
    // 31. December of the 20th year; tender plants use exact 20-year date.
    //
    // Repowering (§22 EEG): clock resets. KWKG: use kwk_foerderdauer_years.
    let is_ausschreibung = req.ausschreibungs_zuschlag_id.is_some();
    let foerderendedatum = if ist_repowering {
        let basis = repowering_datum.unwrap_or(inbetriebnahme);
        eeg_billing::foerderendedatum_repowering(basis)
            .context("compute repowering foerderendedatum")?
    } else if let Some(years) = req.kwk_foerderdauer_years {
        // KWKG: exact years (not December 31 extension — KWKG ≠ EEG)
        inbetriebnahme
            .replace_year(inbetriebnahme.year() + years as i32)
            .context("compute KWKG foerderendedatum")?
    } else if is_ausschreibung {
        // Tender plant: exact 20-year anniversary (§25 Satz 2 does NOT apply)
        eeg_billing::foerderendedatum_eeg_ausschreibung(inbetriebnahme)
            .context("compute tender foerderendedatum")?
    } else {
        // Statutory plant: extend to 31 December of the 20th year (§25 Abs. 1 Satz 2)
        eeg_billing::foerderendedatum_eeg(inbetriebnahme)
            .context("compute statutory foerderendedatum")?
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
               mastr_registriert, mastr_nummer, mastr_datum,
               bank_iban, bank_bic, zahlungsempfaenger,
               notes, updated_at
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               $7, $8, $9, $10,
               $11, $12, $13, $14, $15, $16,
               $17, $18, $19,
               $20,
               $21, $22,
               $23, $24,
               $25, $26, $27,
               $28, $29, $30,
               $31, now()
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
               mastr_registriert         = EXCLUDED.mastr_registriert,
               mastr_nummer              = COALESCE(EXCLUDED.mastr_nummer, eeg_anlagen.mastr_nummer),
               mastr_datum               = COALESCE(EXCLUDED.mastr_datum, eeg_anlagen.mastr_datum),
               bank_iban                 = COALESCE(EXCLUDED.bank_iban, eeg_anlagen.bank_iban),
               bank_bic                  = COALESCE(EXCLUDED.bank_bic, eeg_anlagen.bank_bic),
               zahlungsempfaenger        = COALESCE(EXCLUDED.zahlungsempfaenger, eeg_anlagen.zahlungsempfaenger),
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
    .bind(req.mastr_registriert)
    .bind(&req.mastr_nummer)
    .bind(mastr_datum)
    .bind(&req.bank_iban)
    .bind(&req.bank_bic)
    .bind(&req.zahlungsempfaenger)
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
    pub malo_id: Option<String>,
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
            AND ($2::text IS NULL OR malo_id = $2)
            AND ($3::text IS NULL OR erzeugungsart = $3)
            AND ($4::text IS NULL OR settlement_model = $4)
            AND ($5::text IS NULL OR status = $5)
          ORDER BY foerderendedatum ASC
          LIMIT $6",
    )
    .bind(tenant)
    .bind(&q.malo_id)
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
    pub einspeisemenge_kwh: Option<Decimal>,
    pub epex_avg_ct_kwh: Option<Decimal>,
    pub settlement_model: String,
    pub verguetungssatz_ct: Decimal,
    pub direktverm_aw_ct: Option<Decimal>,
    pub mieter_zuschlag_ct: Option<Decimal>,
    pub flex_praemie_ct_kwh: Option<Decimal>,
    pub managementpraemie_ct: Option<Decimal>,
    pub kwk_strom_kwh_gesamt: Option<Decimal>,
    pub kwk_max_kwh: Option<Decimal>,
    /// Derived from `mastr_registriert` in `run_settlement` — not set by caller.
    pub sanktion: Option<eeg_billing::SanktionAlt>,
    pub kwh_during_negative_epex: Option<Decimal>,
    /// Plant commissioning date — forwarded to `eeg-billing` for §27 EEG guard.
    pub inbetriebnahme: Option<Date>,
    /// Installed peak power — used for §27 threshold check (≥100 kWp) and auto Managementprämie.
    pub leistung_kwp: Option<Decimal>,
    /// EEG subsidy end date — triggers automatic `FoerderungBeendet` when billing_date > foerderendedatum.
    pub foerderendedatum: Option<Date>,
    /// First day of the billing month — supplied for FoerderungBeendet auto-detection.
    pub billing_date: Option<Date>,
    /// EEG law year (e.g. 2017, 2021, 2023, 0 for KWKG) — determines version-specific
    /// §51 Negativpreisregel threshold and kW exemption.
    pub eeg_gesetz: i16,
    /// Plant technology type for §51 EEG 2017 kW exemption dispatch.
    pub erzeugungsart: String,
    /// Whether the plant is registered in MaStR (Marktstammdatenregister).
    ///
    /// Replaces the old `notes.contains("mastr_not_registered")` hack.
    /// - `false` + EEG 2023  → Pflichtzahlung €10/kW/month (§52 Abs. 1 Nr. 11 EEG 2023)
    /// - `false` + EEG ≤2021 → `sanktion = Some(VerguetungAufNull)` (Vergütung = 0, old §47/§52 via §100)
    pub mastr_registriert: bool,
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
/// | `DIREKTVERMARKTUNG` | `max(0, aw_ct − epex) × kwh / 100 + managementpraemie_ct × kwh / 100` | §20 EEG 2023 |
/// | `AUSSCHREIBUNG` | same as DIREKTVERMARKTUNG (tendering AW) | §§22a,28 EEG 2023 |
/// | `POST_EEG_SPOT` | `kwh × epex_avg_ct / 100` | post-Förderung |
/// | `EIGENVERBRAUCH` | EUR 0 | self-consumption |
/// | `KWKG_ZUSCHLAG` | `kwh × verguetungssatz_ct / 100` (on top of market price, capped by hour limit) | §7 KWKG 2023 |
/// | `FLEXIBILITAET` | VERGUETUNG + `kwh × flex_praemie_ct / 100` | §50 EEG 2023 |
///
/// ## KWKG hour-limit enforcement (§8 KWKG 2023)
///
/// Plants >2 MW have a maximum full-load-hour Förderdauer (typically 30,000 h).
/// When `kwk_strom_kwh_gesamt + einspeisemenge_kwh > kwk_max_kwh`:
/// - Settlement is prorated to the remaining eligible kWh.
/// - Plant status transitions to `foerderung_beendet` (CE emitted by caller).
/// - `kwk_strom_kwh_gesamt` is updated atomically with the settlement.
///
/// ## §20 Abs. 3 EEG 2023 Managementprämie
///
/// Direktvermarktung plants receive a flat Managementprämie (statutory 0.4 ct/kWh,
/// reduced to 0.2 ct/kWh for plants >100 MW).  The Managementprämie is paid by the
/// Run the settlement calculation and persist the result.
///
/// Delegates all formula logic to the [`eeg_billing`] crate.
/// See [`eeg_billing::calculate_settlement`] for the formula table and
/// KWKG hour-limit enforcement details.
pub async fn run_settlement(pool: &PgPool, input: SettleInput) -> anyhow::Result<SettleResult> {
    use eeg_billing::{
        SettleInput as EegInput, SettlementModel, SettlementStatus, calculate_settlement,
    };

    // Map DB string → eeg-billing enum variant.
    let model = match input.settlement_model.as_str() {
        "VERGUETUNG" => SettlementModel::Verguetung,
        "MIETERSTROM" => SettlementModel::Mieterstrom,
        "DIREKTVERMARKTUNG" => SettlementModel::Direktvermarktung,
        "AUSSCHREIBUNG" => SettlementModel::Ausschreibung,
        "POST_EEG_SPOT" => SettlementModel::PostEegSpot,
        "EIGENVERBRAUCH" => SettlementModel::Eigenverbrauch,
        "KWKG_ZUSCHLAG" => SettlementModel::KwkgZuschlag,
        "FLEXIBILITAET" => SettlementModel::Flexibilitaet,
        "FLEXIBILITAET_ZUSCHLAG" => SettlementModel::FlexibilitaetZuschlag,
        other => anyhow::bail!("unknown settlement_model: {other}"),
    };

    let output = calculate_settlement(&EegInput {
        model,
        einspeisemenge_kwh: input.einspeisemenge_kwh,
        epex_avg_ct_kwh: input.epex_avg_ct_kwh,
        verguetungssatz_ct: input.verguetungssatz_ct,
        direktverm_aw_ct: input.direktverm_aw_ct,
        mieter_zuschlag_ct: input.mieter_zuschlag_ct,
        flex_praemie_ct_kwh: input.flex_praemie_ct_kwh,
        managementpraemie_ct: input.managementpraemie_ct,
        kwk_strom_kwh_gesamt: input.kwk_strom_kwh_gesamt,
        kwk_max_kwh: input.kwk_max_kwh,
        // Derive sanktion from mastr_registriert using EegGesetz version logic.
        // EEG ≤2021: Vergütung = 0 (Abs. 1 VerguetungAufNull); EEG 2023: pflichtverstoss.
        sanktion: if !input.mastr_registriert
            && eeg_billing::EegGesetz::from_db_year(input.eeg_gesetz)
                .unwrap_or(eeg_billing::EegGesetz::Eeg2023)
                .mastr_nichtregistrierung_suspendiert_verguetung()
        {
            Some(eeg_billing::SanktionAlt::VerguetungAufNull)
        } else {
            None
        },
        kwh_during_negative_epex: input.kwh_during_negative_epex,
        inbetriebnahme: input.inbetriebnahme,
        leistung_kwp: input.leistung_kwp,
        foerderendedatum: input.foerderendedatum,
        billing_date: input.billing_date,
        capacity_blocks: vec![],
        messkonzept: None,
        pflichtverstoss: None,
        eeg_gesetz: eeg_billing::EegGesetz::from_db_year(input.eeg_gesetz)
            .unwrap_or(eeg_billing::EegGesetz::Eeg2023),
        erzeugungsart: eeg_billing::ErzeugungsArt::from_db_str(&input.erzeugungsart).ok(),
    });

    let status = match output.status {
        SettlementStatus::Calculated => "calculated",
        SettlementStatus::NoData => "no_data",
        SettlementStatus::PriceMissing => "price_missing",
        SettlementStatus::FoerderungBeendet => "foerderung_beendet",
        SettlementStatus::Sanctioned => "sanctioned",
    };
    let settlement_eur = output.settlement_eur;
    let effective_kwh = output.eligible_kwh;

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
    .bind(effective_kwh.or(input.einspeisemenge_kwh))
    .bind(settlement_eur)
    .bind(status)
    .execute(pool)
    .await
    .context("persist settlement")?;

    // ── KWKG: update accumulated kWh + auto-expire when limit reached ────────
    if input.settlement_model == "KWKG_ZUSCHLAG"
        && let Some(kwh_this_period) = effective_kwh.filter(|&k| k > Decimal::ZERO)
    {
        let new_status = if status == "foerderung_beendet" {
            "foerderung_beendet"
        } else {
            "aktiv"
        };
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET kwk_strom_kwh_gesamt = COALESCE(kwk_strom_kwh_gesamt, 0) + $3,
                  status = $4,
                  updated_at = now()
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(kwh_this_period)
        .bind(new_status)
        .execute(pool)
        .await
        .context("update kwk_strom_kwh_gesamt")?;
    }

    Ok(SettleResult {
        id,
        tr_id: input.tr_id,
        billing_year: input.billing_year,
        billing_month: input.billing_month,
        settlement_model: input.settlement_model,
        einspeisemenge_kwh: effective_kwh.or(input.einspeisemenge_kwh),
        settlement_eur,
        status: status.to_owned(),
    })
}

/// List all active plants that have NOT been settled for `(year, month)` yet.
///
/// Used by the batch settlement endpoint and the monthly auto-settle worker.
pub async fn list_unsettled(
    pool: &PgPool,
    tenant: &str,
    year: i16,
    month: i16,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT a.*
          FROM eeg_anlagen a
          WHERE a.tenant = $1
            AND a.status = 'aktiv'
            AND NOT EXISTS (
                SELECT 1 FROM settlement_receipts s
                WHERE s.tr_id = a.tr_id
                  AND s.tenant = a.tenant
                  AND s.billing_year = $2
                  AND s.billing_month = $3
            )
          ORDER BY a.tr_id",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .fetch_all(pool)
    .await
    .context("list_unsettled")
}

/// §24 EEG 2023 — Zusammenlegung: merge a child plant into a parent entity.
///
/// Sets `parent_tr_id` on the child plant and updates its status to `abgemeldet`.
/// The parent plant continues as the active entity.
///
/// ## Legal basis
///
/// §24 EEG 2023: Multiple plants at the same Netzverknüpfungspunkt may be merged
/// into a single entity ("Gesamtanlage") for the purposes of the tariff threshold
/// (§ 21 EEG 2023 power ranges).  After Zusammenlegung:
/// - The child plant's `status → abgemeldet` (historical record preserved).
/// - The parent plant assumes the combined capacity and continues settlement.
/// - `foerderendedatum` of the parent is NOT reset (unlike Repowering).
///
/// Returns `Ok(true)` if the child was found and updated, `Ok(false)` if not found.
pub async fn zusammenlegen(
    pool: &PgPool,
    tenant: &str,
    child_tr_id: &str,
    parent_tr_id: &str,
    combined_leistung_kwp: Option<Decimal>,
) -> anyhow::Result<bool> {
    // Verify both plants exist for this tenant.
    let child = sqlx::query(
        "SELECT tr_id, leistung_kwp, status FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(child_tr_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch child plant for Zusammenlegung")?;

    let Some(_child) = child else {
        return Ok(false);
    };

    let parent_exists = sqlx::query(
        "SELECT 1 FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'",
    )
    .bind(parent_tr_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch parent plant for Zusammenlegung")?;

    if parent_exists.is_none() {
        anyhow::bail!("parent plant {} not found or not aktiv", parent_tr_id);
    }

    // Mark child as merged (preserves history, stops future settlements).
    sqlx::query(
        "UPDATE eeg_anlagen SET status = 'abgemeldet', parent_tr_id = $3, updated_at = now()
         WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'",
    )
    .bind(child_tr_id)
    .bind(tenant)
    .bind(parent_tr_id)
    .execute(pool)
    .await
    .context("mark child abgemeldet for Zusammenlegung")?;

    // Optionally update parent's combined capacity.
    if let Some(combined_kwp) = combined_leistung_kwp {
        sqlx::query(
            "UPDATE eeg_anlagen SET leistung_kwp = $3, updated_at = now()
             WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(parent_tr_id)
        .bind(tenant)
        .bind(combined_kwp)
        .execute(pool)
        .await
        .context("update parent leistung_kwp for Zusammenlegung")?;
    }

    Ok(true)
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
