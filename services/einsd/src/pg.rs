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
    /// §51b EEG 2023 — Biogas Ausschreibungsanlage with slightly-positive price rule.
    ///
    /// When `true`, the anzulegender Wert reduces to **zero** for any billing period
    /// where `epex_avg_ct_kwh ≤ 2 ct/kWh`. §51/§51a Negativpreisregel do NOT apply.
    ///
    /// Only valid for biogas plants (fermentation, excluding biomethane) that received
    /// their AW via BNetzA tender (`ausschreibungs_zuschlag_id` must be set).
    ///
    /// Legal basis: §51b EEG 2023. Default: `false`.
    #[serde(default)]
    pub is_biogas_sect51b: bool,
    /// Netzgebiet identifier for §53b regional reduction lookups (migration 0007).
    ///
    /// Set to the BNetzA-assigned grid area code for the plant's connection point.
    /// Required for §53b Regionalnachweise reductions to apply.
    /// Example: `"DE-TN-001"` (BNetzA Netzgebiet format).
    pub grid_area: Option<String>,
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
    // Plant attributes (migration 0003)
    pub inbetriebnahme_typ: Option<String>,
    pub solar_bauform: Option<String>,
    pub wind_guetegrad: Option<Decimal>,
    pub wind_korrekturfaktor: Option<Decimal>,
    pub fernsteuerbarkeit_datum: Option<Date>,
    pub direktvermarktung_pflicht: Option<bool>,
    pub metering_mode: Option<String>,
    pub sect52_netting_enabled: Option<bool>,
    // Settlement lifecycle (migration 0004)
    pub settlement_state: Option<String>,
    // §51b EEG 2023 biogas Ausschreibungsanlage (migration 0005)
    pub is_biogas_sect51b: bool,
    // Ausschreibung lifecycle (migration 0006)
    pub award_expired: bool,
    pub zuschlag_erloeschen_datum: Option<Date>,
    // §52 violation tracking (migration 0006)
    pub mastr_violation_start: Option<Date>,
    pub fernsteuerbarkeit_violation_start: Option<Date>,
    // §21b Veräußerungsform switch guard (migration 0006)
    pub last_veraeusserungsform_switch: Option<Date>,
    // §51a cumulative Verlängerungsanspruch (migration 0006)
    pub verlaengerungsanspruch_qh_gesamt: i64,
    // §24 Erweiterung capacity blocks (migration 0003, JSONB)
    pub capacity_blocks: Option<serde_json::Value>,
    // §53b grid area for regional reduction lookups (migration 0007)
    pub grid_area: Option<String>,
    // §44b Biogas annual quota tracking (migration 0009)
    pub biogas_quota_kwh_ytd: Decimal,
    pub biogas_quota_ytd_year: Option<i16>,
    // §51 Abs. 2 iMSys rollout datum (migration 0009)
    pub imesys_rollout_datum: Option<Date>,
    // §42b GGV Nutzungsplan (migration 0009)
    pub ggv_nutzungsplan: Option<serde_json::Value>,
    // §21c notification tracking (migration 0009)
    pub veraeusserungsform_notification_sent_at: Option<OffsetDateTime>,
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
               notes, is_biogas_sect51b, grid_area, updated_at
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
               $31, $32, $33, now()
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
               is_biogas_sect51b         = EXCLUDED.is_biogas_sect51b,
               grid_area                 = EXCLUDED.grid_area,
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
    .bind(req.is_biogas_sect51b)
    .bind(&req.grid_area)
    .execute(pool)
    .await
    .context("upsert eeg_anlage")?;

    // ── Auto-set mastr_violation_start on first registration without MaStR ──
    // §52 Abs. 1 Nr. 11 EEG 2023: penalty accrues from when the NB registers
    // the plant and notes the missing MaStR entry. Set the start date to today
    // (using CURRENT_DATE) only when the column is NULL (not already tracking).
    if !req.mastr_registriert {
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET mastr_violation_start = COALESCE(mastr_violation_start, CURRENT_DATE)
              WHERE tr_id = $1 AND tenant = $2 AND mastr_violation_start IS NULL",
        )
        .bind(&req.tr_id)
        .bind(tenant)
        .execute(pool)
        .await
        .context("set mastr_violation_start")?;
    } else {
        // Plant registered with MaStR confirmed: clear any outstanding violation start.
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET mastr_violation_start = NULL
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&req.tr_id)
        .bind(tenant)
        .execute(pool)
        .await
        .context("clear mastr_violation_start")?;
    }
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
    /// Plant commissioning date — forwarded to `eeg-billing` for §51 EEG guard.
    pub inbetriebnahme: Option<Date>,
    /// Installed peak power — used for §51 threshold check (≥100 kWp) and auto Managementprämie.
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
    /// - `false` + EEG 2023  → Pflichtzahlung €10/kW/month (§52 Abs. 1 Nr. 11 EEG 2023)
    /// - `false` + EEG ≤2021 → `sanktion = Some(VerguetungAufNull)` (Vergütung = 0, old §47/§52 via §100)
    pub mastr_registriert: bool,
    /// §36k EEG — certified wind onshore Korrekturfaktor from the plant DB record.
    /// Forwarded directly to `eeg-billing` for MarketPremium wind plants.
    pub wind_korrekturfaktor: Option<Decimal>,
    /// §9 EEG — date Fernsteuerbarkeit was installed, if any.
    /// Used to determine whether `FernsteuerbarkeitmFehlend` §52 violation is active.
    pub fernsteuerbarkeit_datum: Option<Date>,
    /// Whether this is a §51b biogas Ausschreibungsanlage.
    pub is_biogas_sect51b: bool,
    /// §52 MaStR violation start date for cumulative penalty calculation (migration 0006).
    pub mastr_violation_start: Option<Date>,
    /// §52 Fernsteuerbarkeit violation start date (migration 0006).
    pub fernsteuerbarkeit_violation_start: Option<Date>,
    /// §33/§35a: whether the Zuschlag has expired. Short-circuits to FoerderungBeendet.
    pub award_expired: bool,
    /// §24 capacity blocks JSONB (migration 0003) — deserialized in run_settlement.
    pub capacity_blocks_json: Option<serde_json::Value>,
    /// §53b grid area identifier for regional reduction lookup (migration 0007).
    pub grid_area: Option<String>,
    /// §19 EEG 2023 — kWh curtailed by NB; NB must compensate at AW rate.
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51a EEG 2023 — quarter-hours during negative-price periods for Verlängerungsanspruch.
    pub negative_price_quarter_hours: Option<u64>,
    // fernsteuerbarkeit_datum is declared above alongside other plant fields
    /// §22 MessZV — UUID of the original receipt this corrects (None for initial settlements).
    ///
    /// When Some, `run_settlement` will:
    /// 1. Snapshot the existing receipt to `settlement_receipt_history`.
    /// 2. Upsert the correction, storing `correction_of` and `is_correction = true`.
    pub correction_of: Option<uuid::Uuid>,
    /// §44b Abs. 1 EEG 2023 — Biogas >100kW: eligible kWh for this billing period.
    /// Caller tracks cumulative annual kWh and passes `min(kwh, remaining_annual_quota)`.
    /// `None` = cap does not apply.
    pub biogas_sect44b_eligible_kwh: Option<Decimal>,
    /// §20 Abs. 2 + Anlage 1 EEG 2023 — technology-specific Jahresmarktwert.
    /// Alternative to `epex_avg_ct_kwh` for MarketPremium. `None` = auto-fetch.
    pub jahresmarktwert_ct_kwh: Option<Decimal>,
    /// §44b: year-to-date Einspeisemenge for the Biogas annual quota (from AnlageRow).
    pub biogas_quota_kwh_ytd: Decimal,
    /// §44b: calendar year the biogas_quota_kwh_ytd tracks (None = never settled).
    pub biogas_quota_ytd_year: Option<i16>,
    /// §51 Abs. 2 Nr. 1 EEG 2023: date iMSys was installed (None = not yet rolled out).
    pub imesys_rollout_datum: Option<Date>,
    /// §3 EEG 2023: plant lifecycle type (Erstinbetriebnahme / Wiederinbetriebnahme / Repowering …).
    /// Stored as TEXT in `eeg_anlagen.inbetriebnahme_typ`; `None` = Erstinbetriebnahme.
    pub inbetriebnahme_typ: Option<String>,
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

// ── §44b quota computation ─────────────────────────────────────────────────────

/// Compute the §44b eligible kWh for a Biogas plant billing period.
///
/// §44b Abs. 1 EEG 2023: fermentation-Biogas plants >100 kW (excl. §39 Ausschreibung)
/// are capped at 45% of rated capacity × 8760 h/year. Excess kWh receive:
/// - MarketPremium: AW = 0, Marktprämie = 0
/// - FeedInTariff: paid at EPEX Marktwert
///
/// Returns `None` when the cap does not apply to this plant.
/// Returns `Some(eligible_kwh)` = max(0, annual_quota − ytd_before_this_period).
async fn compute_biogas_sect44b_eligible(
    pool: &PgPool,
    input: &SettleInput,
) -> anyhow::Result<Option<Decimal>> {
    use rust_decimal_macros::dec;

    // §44b applies only to: fermentation Biogas, >100 kW, not §51b Ausschreibung
    let is_applicable = input.erzeugungsart == "BIOGAS"
        && input.leistung_kwp.is_some_and(|kw| kw > dec!(100))
        && !input.is_biogas_sect51b;

    if !is_applicable {
        return Ok(None);
    }

    // Reset YTD counter when entering a new calendar year
    let ytd = if input.biogas_quota_ytd_year == Some(input.billing_year) {
        input.biogas_quota_kwh_ytd
    } else {
        // New year: reset the counter atomically before settlement
        sqlx::query(
            "UPDATE eeg_anlagen
             SET biogas_quota_kwh_ytd = 0, biogas_quota_ytd_year = $3
             WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(input.billing_year)
        .execute(pool)
        .await
        .context("reset biogas §44b YTD counter")?;
        Decimal::ZERO
    };

    let leistung_kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
    // §44b Abs. 1: annual quota = leistung_kw × 0.45 × 8760 h
    let annual_quota = leistung_kw * dec!(0.45) * dec!(8760);
    let remaining = (annual_quota - ytd).max(Decimal::ZERO);
    Ok(Some(remaining))
}

/// Update the Biogas §44b year-to-date counter after a successful settlement.
async fn update_biogas_quota_ytd(
    pool: &PgPool,
    tr_id: &str,
    tenant: &str,
    billing_year: i16,
    kwh_settled: Decimal,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE eeg_anlagen
         SET biogas_quota_kwh_ytd  = biogas_quota_kwh_ytd + $4,
             biogas_quota_ytd_year = $3
         WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_year)
    .bind(kwh_settled)
    .execute(pool)
    .await
    .context("update biogas §44b YTD counter")?;
    Ok(())
}

// ── §20 Abs. 2 Jahresmarktwert fetch ──────────────────────────────────────────

/// Fetch the technology-specific Jahresmarktwert (§20 Abs. 2 + Anlage 1 EEG 2023).
///
/// Lookup order:
/// 1. Exact technology match in `jahresmarktwert_preise`
/// 2. `DEFAULT` fallback row in `jahresmarktwert_preise`
/// 3. Generic EPEX monthly average from `epex_monthly_prices`
/// 4. `None` (PriceMissing)
pub async fn fetch_marktwert(
    pool: &PgPool,
    billing_year: i16,
    billing_month: i16,
    erzeugungsart: &str,
    epex_fallback: Option<Decimal>,
) -> anyhow::Result<Option<Decimal>> {
    // 1 & 2: try jahresmarktwert_preise (exact match, then DEFAULT)
    let jmw: Option<Decimal> = sqlx::query_scalar(
        "SELECT avg_ct_kwh FROM jahresmarktwert_preise
         WHERE billing_year = $1 AND billing_month = $2
           AND erzeugungsart = ANY(ARRAY[$3, 'DEFAULT'])
         ORDER BY (erzeugungsart = $3) DESC
         LIMIT 1",
    )
    .bind(billing_year)
    .bind(billing_month)
    .bind(erzeugungsart)
    .fetch_optional(pool)
    .await
    .context("fetch Jahresmarktwert")?;

    Ok(jmw.or(epex_fallback))
}

/// Override values callers can supply to `build_settle_input`.
///
/// Fields left `None` use plant-DB or handler-default values.
#[derive(Debug, Default)]
pub struct SettleOverrides {
    /// Explicit Einspeisemenge (overrides edmd auto-fetch).
    pub einspeisemenge_kwh: Option<Decimal>,
    /// Explicit EPEX / Jahresmarktwert ct/kWh (overrides DB lookup).
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// §20 Abs. 3 Managementprämie override ct/kWh.
    pub managementpraemie_ct_override: Option<Decimal>,
    /// §19 EEG curtailed kWh for this billing period.
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51a quarter-hours during negative EPEX for this period.
    pub negative_price_quarter_hours: Option<u64>,
    /// §22 MessZV correction: UUID of original receipt this corrects.
    pub correction_of: Option<uuid::Uuid>,
    /// §20 Abs. 2 technology-specific Jahresmarktwert (explicit override).
    pub jahresmarktwert_ct_kwh: Option<Decimal>,
}

/// Build a [`SettleInput`] from a plant row and a billing period.
///
/// This single function is the authoritative mapping between the plant DB record
/// and the settlement engine input. All four settlement entry points
/// (single settle, batch settle, correction settle, MCP settle) use this
/// function so that any new field is automatically threaded everywhere.
#[must_use]
pub fn build_settle_input(
    tenant: &str,
    anlage: &AnlageRow,
    billing_year: i16,
    billing_month: i16,
    overrides: SettleOverrides,
) -> SettleInput {
    use rust_decimal_macros::dec;

    let is_dv = matches!(
        anlage.settlement_model.as_str(),
        "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" | "MARKET_PREMIUM"
    );
    let managementpraemie_ct = if is_dv {
        Some(overrides.managementpraemie_ct_override.unwrap_or_else(|| {
            if anlage.leistung_kwp > dec!(100_000) {
                dec!(0.2)
            } else {
                dec!(0.4)
            }
        }))
    } else {
        None
    };

    let billing_date = time::Date::from_calendar_date(
        billing_year as i32,
        time::Month::try_from(billing_month as u8).unwrap_or(time::Month::January),
        1,
    )
    .ok();

    SettleInput {
        tr_id: anlage.tr_id.clone(),
        tenant: tenant.to_owned(),
        billing_year,
        billing_month,
        einspeisemenge_kwh: overrides.einspeisemenge_kwh,
        epex_avg_ct_kwh: overrides.epex_avg_ct_kwh,
        settlement_model: anlage.settlement_model.clone(),
        verguetungssatz_ct: anlage.verguetungssatz_ct,
        direktverm_aw_ct: anlage.direktverm_aw_ct,
        mieter_zuschlag_ct: anlage.mieter_zuschlag_ct,
        flex_praemie_ct_kwh: anlage.flex_praemie_ct_kwh,
        managementpraemie_ct,
        kwk_strom_kwh_gesamt: if anlage.settlement_model == "KWKG_ZUSCHLAG" {
            anlage.kwk_strom_kwh_gesamt
        } else {
            None
        },
        kwk_max_kwh: anlage
            .kwk_foerderdauer_h
            .map(|h| Decimal::from(h) * anlage.leistung_kwp),
        sanktion: None, // derived from mastr_registriert in run_settlement
        mastr_registriert: anlage.mastr_registriert,
        kwh_during_negative_epex: None,
        inbetriebnahme: Some(anlage.inbetriebnahme),
        leistung_kwp: Some(anlage.leistung_kwp),
        foerderendedatum: Some(anlage.foerderendedatum),
        billing_date,
        eeg_gesetz: anlage.eeg_gesetz,
        erzeugungsart: anlage.erzeugungsart.clone(),
        wind_korrekturfaktor: anlage.wind_korrekturfaktor,
        fernsteuerbarkeit_datum: anlage.fernsteuerbarkeit_datum,
        is_biogas_sect51b: anlage.is_biogas_sect51b,
        mastr_violation_start: anlage.mastr_violation_start,
        fernsteuerbarkeit_violation_start: anlage.fernsteuerbarkeit_violation_start,
        award_expired: anlage.award_expired,
        capacity_blocks_json: anlage.capacity_blocks.clone(),
        grid_area: anlage.grid_area.clone(),
        einspeisemanagement_kwh: overrides.einspeisemanagement_kwh,
        negative_price_quarter_hours: overrides.negative_price_quarter_hours,
        correction_of: overrides.correction_of,
        biogas_sect44b_eligible_kwh: None, // computed by run_settlement from biogas_quota_kwh_ytd
        jahresmarktwert_ct_kwh: overrides.jahresmarktwert_ct_kwh,
        biogas_quota_kwh_ytd: anlage.biogas_quota_kwh_ytd,
        biogas_quota_ytd_year: anlage.biogas_quota_ytd_year,
        imesys_rollout_datum: anlage.imesys_rollout_datum,
        inbetriebnahme_typ: anlage.inbetriebnahme_typ.clone(),
    }
}

/// Run the settlement calculation and persist the result.
///
/// Delegates all formula logic to the [`eeg_billing`] crate.
///
/// ## §52 EEG 2023 Pflichtzahlungen
///
/// For EEG 2023 plants, this function automatically derives §52 violations:
/// - MaStR not registered → `SanktionsTyp::MastrNichtRegistriert`
/// - Fernsteuerbarkeit not installed (plant ≥ 25 kW) → `SanktionsTyp::FernsteuerbarkeitmFehlend`
///
/// ## §25/§26 billing_days_fraction
///
/// When the plant was commissioned in the current billing month, the settlement
/// is prorated to the days with entitlement (commissioning day to end of month).
pub async fn run_settlement(pool: &PgPool, input: SettleInput) -> anyhow::Result<SettleResult> {
    use eeg_billing::{
        AusschreibungMetadata, SettleInput as EegInput, SettlementScheme, SettlementStatus,
        TariffSource, calculate_settlement,
    };

    // Map DB string → SettlementScheme + TariffSource.
    // Both old (VERGUETUNG) and new (FEED_IN_TARIFF) naming accepted for migration compatibility.
    // Note: scheme is built AFTER §54 computation so direktverm_aw_ct_effective is available.

    let eeg_gesetz_enum = eeg_billing::EegGesetz::from_db_year(input.eeg_gesetz)
        .unwrap_or(eeg_billing::EegGesetz::Eeg2023);

    // ── §52 EEG 2023 Pflichtverstoss derivation ──────────────────────────────
    // EEG 2023 plants: separate Pflichtzahlungen (Vergütung continues).
    // EEG ≤2021 plants: old three-tier SanktionAlt model reduces Vergütung.
    let (sanktion, pflichtverstoss) =
        if !eeg_gesetz_enum.mastr_nichtregistrierung_suspendiert_verguetung() {
            // EEG 2023 path: build Pflichtverstoss list from plant compliance status.
            // monate_des_verstosses is computed from the violation start date stored in the DB
            // (migration 0006 adds mastr_violation_start / fernsteuerbarkeit_violation_start).
            // Falls back to 1 when the violation start date is not yet tracked.
            let billing_date_for_months = time::Date::from_calendar_date(
                input.billing_year as i32,
                time::Month::try_from(input.billing_month as u8).unwrap_or(time::Month::January),
                1,
            )
            .unwrap_or(time::Date::MIN);

            let months_since = |start: Option<time::Date>| -> u32 {
                match start {
                    None => 1, // violation start not tracked yet → assume this month only
                    Some(s) => {
                        // Count inclusive calendar months from start to billing_date
                        let years = billing_date_for_months.year() - s.year();
                        let months = billing_date_for_months.month() as i32 - s.month() as i32;
                        (years * 12 + months + 1).max(1) as u32
                    }
                }
            };

            let mut violations: Vec<eeg_billing::Pflichtverstoss> = vec![];

            if !input.mastr_registriert {
                // §52 Abs. 1 Nr. 11 EEG 2023: MaStR not registered → €10/kW/month (cumulative)
                violations.push(eeg_billing::Pflichtverstoss {
                    typ: eeg_billing::SanktionsTyp::MastrNichtRegistriert,
                    leistung_kw: input.leistung_kwp.unwrap_or(rust_decimal::Decimal::ZERO),
                    monate_des_verstosses: months_since(input.mastr_violation_start),
                    nachtraeglich_erfuellt: false,
                    technischer_defekt: false,
                });
            }

            // §52 Abs. 1 Nr. 1 EEG 2023: Fernsteuerbarkeit (§9) required for plants ≥ 25 kW
            if input.fernsteuerbarkeit_datum.is_none()
                && input
                    .leistung_kwp
                    .is_some_and(|kw| kw >= rust_decimal::Decimal::from(25))
            {
                violations.push(eeg_billing::Pflichtverstoss {
                    typ: eeg_billing::SanktionsTyp::FernsteuerbarkeitmFehlend,
                    leistung_kw: input.leistung_kwp.unwrap_or(rust_decimal::Decimal::ZERO),
                    monate_des_verstosses: months_since(input.fernsteuerbarkeit_violation_start),
                    nachtraeglich_erfuellt: false,
                    technischer_defekt: false,
                });
            }

            (None, violations)
        } else {
            // EEG ≤2021 path: Vergütung reduced to 0 for unregistered plants
            let sanktion = if !input.mastr_registriert {
                Some(eeg_billing::SanktionAlt::VerguetungAufNull)
            } else {
                input.sanktion
            };
            (sanktion, vec![])
        };

    // ── The eeg-billing library now auto-computes billing_days_fraction from dates ─
    // No local computation needed — pass billing_days_fraction: None and the library
    // will derive it from billing_date, inbetriebnahme, and foerderendedatum.

    // ── §33/§35a: short-circuit when Zuschlag has expired ────────────────────
    if input.award_expired {
        let id = Uuid::new_v4();
        sqlx::query(
            r"INSERT INTO settlement_receipts
                  (id, tr_id, tenant, billing_year, billing_month,
                   settlement_model, einspeisemenge_kwh, settlement_eur, status)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
              ON CONFLICT (tr_id, tenant, billing_year, billing_month) DO UPDATE
              SET status = EXCLUDED.status, settled_at = now()",
        )
        .bind(id)
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(input.billing_year)
        .bind(input.billing_month)
        .bind(&input.settlement_model)
        .bind(input.einspeisemenge_kwh)
        .bind(rust_decimal::Decimal::ZERO)
        .bind("foerderung_beendet")
        .execute(pool)
        .await
        .context("persist expired-award receipt")?;
        return Ok(SettleResult {
            id,
            tr_id: input.tr_id,
            billing_year: input.billing_year,
            billing_month: input.billing_month,
            settlement_model: input.settlement_model,
            einspeisemenge_kwh: input.einspeisemenge_kwh,
            settlement_eur: Some(rust_decimal::Decimal::ZERO),
            status: "foerderung_beendet".to_owned(),
        });
    }

    // ── §24 Abs. 1 EEG 2023 — deserialize CapacityBlocks from JSONB ─────────
    let capacity_blocks: Vec<eeg_billing::CapacityBlock> = input
        .capacity_blocks_json
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // ── §54 EEG 2023 — Ausschreibungsreduzierung: query per-plant AW deduction ─
    // §54: BNetzA may reduce the awarded AW after commissioning (e.g. grid violations).
    let sect54_deduction_ct: Option<rust_decimal::Decimal> = if let Some(bd) = input.billing_date {
        sqlx::query_scalar::<_, rust_decimal::Decimal>(
            r"SELECT deduction_ct_kwh FROM sect54_reductions
              WHERE tr_id = $1 AND tenant = $2
                AND effective_from <= $3
                AND (effective_until IS NULL OR effective_until >= $3)
              ORDER BY effective_from DESC LIMIT 1",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(bd)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    } else {
        None
    };

    // Apply §54 deduction to direktverm_aw_ct (floor 0)
    let direktverm_aw_ct_effective = input.direktverm_aw_ct.map(|aw| {
        let deduction = sect54_deduction_ct.unwrap_or(rust_decimal::Decimal::ZERO);
        (aw - deduction).max(rust_decimal::Decimal::ZERO)
    });

    // Build data-bearing SettlementScheme variant now that direktverm_aw_ct_effective is ready.
    let (scheme, tariff_source) = match input.settlement_model.as_str() {
        "FEED_IN_TARIFF" | "VERGUETUNG" => (
            SettlementScheme::FeedInTariff {
                verguetungssatz_ct: input.verguetungssatz_ct,
            },
            TariffSource::Statutory,
        ),
        "TENANT_ELECTRICITY" | "MIETERSTROM" => (
            SettlementScheme::TenantElectricity {
                verguetungssatz_ct: input.verguetungssatz_ct,
                mieter_zuschlag_ct: input.mieter_zuschlag_ct,
            },
            TariffSource::Statutory,
        ),
        "MARKET_PREMIUM" | "DIREKTVERMARKTUNG" => (
            SettlementScheme::MarketPremium {
                direktverm_aw_ct: direktverm_aw_ct_effective.unwrap_or(rust_decimal::Decimal::ZERO),
                managementpraemie_ct: input.managementpraemie_ct,
                wind_korrekturfaktor: input.wind_korrekturfaktor,
                wind_standort: None,
            },
            TariffSource::Statutory,
        ),
        "AUSSCHREIBUNG" => (
            SettlementScheme::MarketPremium {
                direktverm_aw_ct: direktverm_aw_ct_effective.unwrap_or(rust_decimal::Decimal::ZERO),
                managementpraemie_ct: input.managementpraemie_ct,
                wind_korrekturfaktor: input.wind_korrekturfaktor,
                wind_standort: None,
            },
            TariffSource::Auction(AusschreibungMetadata {
                is_biogas_sect51b: input.is_biogas_sect51b,
                ..AusschreibungMetadata::default()
            }),
        ),
        "POST_EEG" | "POST_EEG_SPOT" => (
            SettlementScheme::PostEeg { price_floor: None },
            TariffSource::Statutory,
        ),
        "EIGENVERBRAUCH" => (SettlementScheme::Eigenverbrauch, TariffSource::Statutory),
        "KWK_SURCHARGE" | "KWKG_ZUSCHLAG" => (
            SettlementScheme::KwkSurcharge {
                verguetungssatz_ct: input.verguetungssatz_ct,
                kwh_paid_gesamt: input.kwk_strom_kwh_gesamt,
                max_kwh: input.kwk_max_kwh,
            },
            TariffSource::Statutory,
        ),
        "FLEXIBILITY_PREMIUM" | "FLEXIBILITAET" => (
            SettlementScheme::FlexibilityPremium {
                verguetungssatz_ct: input.verguetungssatz_ct,
                flex_praemie_ct_kwh: input.flex_praemie_ct_kwh,
            },
            TariffSource::Statutory,
        ),
        "FLEXIBILITY_SURCHARGE" | "FLEXIBILITAET_ZUSCHLAG" => (
            SettlementScheme::FlexibilitySurcharge {
                rate_eur_per_kw_year: input.verguetungssatz_ct,
            },
            TariffSource::Statutory,
        ),
        "TEMPORARY_FEED_IN_TARIFF" => (
            SettlementScheme::TemporaryFeedInTariff {
                verguetungssatz_ct: input.verguetungssatz_ct,
            },
            TariffSource::Statutory,
        ),
        // ── §42b EEG 2023 Gemeinschaftliche Gebäudeversorgung ─────────────────
        // GGV plants receive EEG Einspeisevergütung from the NB like any other
        // solar plant. The settlement is against the Einspeisemessung (grid
        // feed-in) at the GGV MaLo, not per-tenant. TenantElectricity is the
        // correct scheme: Vergütungssatz = §21 EEG rate; mieter_zuschlag_ct =
        // None (no Mieterstrom surcharge for the NB→LF EEG flow).
        // The Nutzungsplan allocation among tenants is handled separately in
        // billingd (POST /api/v1/billing/ggv/{ggv_id}).
        "GGV" => (
            SettlementScheme::TenantElectricity {
                verguetungssatz_ct: input.verguetungssatz_ct,
                mieter_zuschlag_ct: input.mieter_zuschlag_ct,
            },
            TariffSource::Statutory,
        ),
        // ── §21a EEG 2023 Sonstige Direktvermarktung ─────────────────────────
        // No NB EEG payment. Records the period for settlement history.
        "SONSTIGE_DIREKTVERMARKTUNG" => (
            SettlementScheme::SonstigeDirektvermarktung,
            TariffSource::Statutory,
        ),
        other => anyhow::bail!("unknown settlement_model: {other}"),
    };

    // ── §53b EEG 2023 — Regional Grünstromkennzeichnung reduction ────────────
    // §53b: BNetzA-certified grid areas get a reduction on Einspeisevergütung.
    // Requires the plant's grid_area to be set. Only applies to Vergütung schemes.
    let sect53b_reduction_ct: Option<rust_decimal::Decimal> =
        if let (Some(ga), Some(bd)) = (&input.grid_area, input.billing_date) {
            sqlx::query_scalar::<_, rust_decimal::Decimal>(
                r"SELECT reduction_ct_kwh FROM sect53b_reductions
              WHERE tenant = $1 AND grid_area = $2
                AND effective_from <= $3
                AND (effective_until IS NULL OR effective_until >= $3)
              ORDER BY effective_from DESC LIMIT 1",
            )
            .bind(&input.tenant)
            .bind(ga)
            .bind(bd)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
        } else {
            None
        };

    // ── §44b Abs. 1 EEG 2023 — Biogas annual 45%-cap quota ────────────────────
    // Auto-computed here when the caller did not supply an explicit eligible_kwh.
    // compute_biogas_sect44b_eligible resets the YTD counter when billing_year changed.
    let biogas_sect44b_eligible_kwh = if input.biogas_sect44b_eligible_kwh.is_some() {
        input.biogas_sect44b_eligible_kwh // caller-provided explicit override
    } else {
        compute_biogas_sect44b_eligible(pool, &input)
            .await
            .context("compute §44b Biogas quota")?
    };

    // ── §20 Abs. 2 + Anlage 1 EEG 2023 — technology-specific Jahresmarktwert ─
    // For MarketPremium (Direktvermarktung / Ausschreibung), prefer the
    // technology-specific Jahresmarktwert over the generic EPEX monthly average.
    // Lookup order: caller override → jahresmarktwert_preise exact → DEFAULT fallback → EPEX.
    let effective_marktwert = if input.jahresmarktwert_ct_kwh.is_some() {
        input.jahresmarktwert_ct_kwh
    } else if matches!(
        input.settlement_model.as_str(),
        "MARKET_PREMIUM" | "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG"
    ) {
        fetch_marktwert(
            pool,
            input.billing_year,
            input.billing_month,
            &input.erzeugungsart,
            input.epex_avg_ct_kwh,
        )
        .await
        .context("fetch Jahresmarktwert")?
    } else {
        input.epex_avg_ct_kwh
    };

    // ── §51 Abs. 2 Nr. 1 — iMSys rollout: lift <100 kW exemption if installed ─
    let has_imesys = input
        .imesys_rollout_datum
        .zip(input.billing_date)
        .is_some_and(|(rollout, billing)| rollout <= billing);

    let output = calculate_settlement(&EegInput {
        scheme,
        tariff_source,
        einspeisemenge_kwh: input.einspeisemenge_kwh,
        // §20 Abs. 2: use technology-specific Jahresmarktwert for DV plants; EPEX for others.
        marktwert_ct_kwh: effective_marktwert,
        sanktion,
        kwh_during_negative_epex: input.kwh_during_negative_epex,
        inbetriebnahme: input.inbetriebnahme,
        leistung_kwp: input.leistung_kwp,
        foerderendedatum: input.foerderendedatum,
        billing_date: input.billing_date,
        // §24 Abs. 1 EEG 2023: pass deserialized capacity blocks
        capacity_blocks,
        messkonzept: None,
        pflichtverstoss,
        eeg_gesetz: eeg_gesetz_enum,
        erzeugungsart: eeg_billing::ErzeugungsArt::from_db_str(&input.erzeugungsart).ok(),
        // §19 EEG 2023: curtailment compensation (NB must pay for suppressed kWh)
        einspeisemanagement_kwh: input.einspeisemanagement_kwh,
        billing_days_fraction: None, // auto-computed by eeg-billing from billing_date + dates
        // §53b: regional reduction from BNetzA-certified grid area
        sect53b_regional_reduction_ct: sect53b_reduction_ct,
        // §51a: pass quarter-hours for Verlängerungsanspruch computation
        negative_price_quarter_hours: input.negative_price_quarter_hours,
        // §44b Abs. 1 EEG 2023: computed above from annual quota tracking
        biogas_sect44b_eligible_kwh,
        // §51 Abs. 2 Nr. 1 EEG 2023: iMSys rollout lifts <100 kW exemption
        has_imesys,
        marktwert_kategorie: None,
        settlement_type: eeg_billing::SettlementType::default(),
        // §3 EEG 2023: plant lifecycle type — drives audit labels and Förderdauer semantics
        inbetriebnahme_typ: input
            .inbetriebnahme_typ
            .as_deref()
            .and_then(|s| eeg_billing::InbetriebnahmeTyp::from_db_str(s).ok())
            .unwrap_or_default(),
    });

    let status = match output.status {
        SettlementStatus::Calculated => "calculated",
        SettlementStatus::NoData => "no_data",
        SettlementStatus::PriceMissing => "price_missing",
        SettlementStatus::FoerderungBeendet => "foerderung_beendet",
        SettlementStatus::Sanctioned => "sanctioned",
        // Forward-compatible: any future status variant stores as "unknown" and does not block
        _ => "unknown",
    };
    let settlement_eur = output.settlement_eur;
    let effective_kwh = output.eligible_kwh;
    let pflichtzahlung_eur = output.pflichtzahlung_eur;
    let faelligkeitsdatum = output.faelligkeitsdatum;
    let verlaengerungsanspruch_qh = output.verlaengerungsanspruch_qh as i64;
    // Use the fraction actually applied by the library (may be auto-computed from dates)
    let billing_days_fraction_stored = output.billing_days_fraction_applied;
    // Serialize positions to JSONB for §22 MessZV 3-year audit trail.
    // Each position: { description, legal_basis, kwh, rate_ct_kwh, eur }
    let positions_json = serde_json::to_value(
        output
            .positions
            .iter()
            .map(|p| {
                serde_json::json!({
                    "description": p.description,
                    "legal_basis": p.legal_basis,
                    "kwh": p.kwh.to_string(),
                    "rate_ct_kwh": p.rate_ct_kwh.to_string(),
                    "eur": p.eur.to_string()
                })
            })
            .collect::<Vec<_>>(),
    )
    .ok();

    let id = Uuid::new_v4();

    // ── §22 MessZV: snapshot ANY existing initial receipt before overwrite ────
    // This ensures a complete audit trail even for re-runs of initial settlements.
    // Corrections already snapshot via the correction_of path below; initial re-runs
    // (operator clicking "re-settle" without using the correction endpoint) also need
    // to be snapshotted so no calculation is ever silently lost.
    let existing_initial_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM settlement_receipts
         WHERE tr_id = $1 AND tenant = $2
           AND billing_year = $3 AND billing_month = $4
           AND is_correction = false",
    )
    .bind(&input.tr_id)
    .bind(&input.tenant)
    .bind(input.billing_year)
    .bind(input.billing_month)
    .fetch_optional(pool)
    .await
    .context("check existing initial receipt")?;

    // §22 MessZV: snapshot original receipt before correction overwrites it
    let snapshot_id = existing_initial_id.or(input.correction_of);
    if let Some(original_id) = snapshot_id {
        sqlx::query(
            r"INSERT INTO settlement_receipt_history
                  (original_id, tr_id, tenant, billing_year, billing_month,
                   settlement_eur, status, settlement_data)
              SELECT id, tr_id, tenant, billing_year, billing_month,
                     settlement_eur, status,
                     to_jsonb(settlement_receipts) AS settlement_data
              FROM settlement_receipts
              WHERE id = $1
              ON CONFLICT DO NOTHING",
        )
        .bind(original_id)
        .execute(pool)
        .await
        .context("snapshot receipt before overwrite")?;
    }

    sqlx::query(
        r"INSERT INTO settlement_receipts
              (id, tr_id, tenant, billing_year, billing_month,
               settlement_model, einspeisemenge_kwh, settlement_eur, status,
               pflichtzahlung_eur, faelligkeitsdatum,
               verlaengerungsanspruch_qh, billing_days_fraction, positions_json,
               is_correction, correction_of)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                  $10, $11, $12, $13, $14, $15, $16)
          ON CONFLICT ON CONSTRAINT sr_unique_initial DO UPDATE
          SET settlement_model          = EXCLUDED.settlement_model,
              einspeisemenge_kwh        = EXCLUDED.einspeisemenge_kwh,
              settlement_eur            = EXCLUDED.settlement_eur,
              status                    = EXCLUDED.status,
              pflichtzahlung_eur        = EXCLUDED.pflichtzahlung_eur,
              faelligkeitsdatum         = EXCLUDED.faelligkeitsdatum,
              verlaengerungsanspruch_qh = EXCLUDED.verlaengerungsanspruch_qh,
              billing_days_fraction     = EXCLUDED.billing_days_fraction,
              positions_json            = EXCLUDED.positions_json,
              is_correction             = EXCLUDED.is_correction,
              correction_of             = EXCLUDED.correction_of,
              settled_at                = now()",
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
    .bind(pflichtzahlung_eur)
    .bind(faelligkeitsdatum)
    .bind(verlaengerungsanspruch_qh)
    .bind(billing_days_fraction_stored)
    .bind(positions_json)
    .bind(input.correction_of.is_some())
    .bind(input.correction_of)
    .execute(pool)
    .await
    .context("persist settlement")?;

    // ── §44b: update Biogas year-to-date production counter ──────────────────
    // Only update when settled (Calculated / FoerderungBeendet), not for NoData / PriceMissing.
    if matches!(
        output.status,
        SettlementStatus::Calculated | SettlementStatus::FoerderungBeendet
    ) && biogas_sect44b_eligible_kwh.is_some()
    {
        let kwh_to_add = effective_kwh.unwrap_or(rust_decimal::Decimal::ZERO);
        if kwh_to_add > rust_decimal::Decimal::ZERO {
            update_biogas_quota_ytd(
                pool,
                &input.tr_id,
                &input.tenant,
                input.billing_year,
                kwh_to_add,
            )
            .await
            .context("update biogas §44b YTD")?;
        }
    }

    // ── §51a: update cumulative Verlängerungsanspruch on the plant record ─────
    if verlaengerungsanspruch_qh > 0 {
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET verlaengerungsanspruch_qh_gesamt =
                      COALESCE(verlaengerungsanspruch_qh_gesamt, 0) + $3,
                  updated_at = now()
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(verlaengerungsanspruch_qh)
        .execute(pool)
        .await
        .context("update verlaengerungsanspruch")?;
    }

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

    // ── H-2: derive_settlement_state and update plant record ─────────────────
    // §52 EEG 2023 state machine: drive settlement_state from compliance status.
    if let Some(bd) = input.billing_date {
        let new_settlement_state = eeg_billing::settlement_state::derive_settlement_state(
            input.mastr_registriert,
            input.fernsteuerbarkeit_datum,
            input.leistung_kwp.unwrap_or(Decimal::ZERO),
            input.foerderendedatum,
            bd,
            eeg_gesetz_enum.to_db_year(),
        );
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET settlement_state = $3, updated_at = now()
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(new_settlement_state.to_db_str())
        .execute(pool)
        .await
        .context("update settlement_state")?;
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

/// Upsert a technology-specific Jahresmarktwert price (§20 Abs. 2 + Anlage 1 EEG 2023).
///
/// `erzeugungsart` must match a value from `eeg_anlagen.erzeugungsart` (e.g. `WIND_ONSHORE`,
/// `SOLAR_AUFDACH`) or the special value `DEFAULT` for the generic fallback row.
/// Published by ÜNB at netztransparenz.de.
pub async fn upsert_jahresmarktwert(
    pool: &PgPool,
    year: i16,
    month: i16,
    erzeugungsart: &str,
    avg_ct_kwh: Decimal,
    source: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"INSERT INTO jahresmarktwert_preise
            (billing_year, billing_month, erzeugungsart, avg_ct_kwh, source)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (billing_year, billing_month, erzeugungsart) DO UPDATE
          SET avg_ct_kwh   = EXCLUDED.avg_ct_kwh,
              source       = EXCLUDED.source,
              imported_at  = now()",
    )
    .bind(year)
    .bind(month)
    .bind(erzeugungsart)
    .bind(avg_ct_kwh)
    .bind(source)
    .execute(pool)
    .await
    .context("upsert_jahresmarktwert")?;
    Ok(())
}

/// Fetch a single Jahresmarktwert row (exact technology match only — no DEFAULT fallback).
/// Returns `None` when no row exists for the given (year, month, erzeugungsart) triple.
pub async fn fetch_jahresmarktwert_single(
    pool: &PgPool,
    year: i16,
    month: i16,
    erzeugungsart: &str,
) -> anyhow::Result<Option<Decimal>> {
    let row: Option<Decimal> = sqlx::query_scalar(
        "SELECT avg_ct_kwh FROM jahresmarktwert_preise
          WHERE billing_year = $1 AND billing_month = $2 AND erzeugungsart = $3",
    )
    .bind(year)
    .bind(month)
    .bind(erzeugungsart)
    .fetch_optional(pool)
    .await
    .context("fetch_jahresmarktwert_single")?;
    Ok(row)
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
