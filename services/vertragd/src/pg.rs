//! PostgreSQL data access for `vertragd`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::Date;
use uuid::Uuid;

// ── Row types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KundeRow {
    pub id: Uuid,
    pub tenant: String,
    pub kunden_nr: Option<String>,
    pub kundentyp: String,
    pub geschaeftspartner: Option<serde_json::Value>,
    pub organisations_id: Option<String>,
    pub umsatzsteuer_id: Option<String>,
    pub zahlungsziel_tage: i32,
    pub sepa_erlaubt: bool,
    pub erp_kunde_id: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// One portal user (OIDC identity) for a Kunde.
/// B2C: 1:1 with Kunde.  B2B: 1:N — multiple users share one company account.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KundenIdentitaetRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub tenant: String,
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub rolle: String,
    pub standort_filter: Option<String>,
    pub aktiv: bool,
    pub letzter_login: Option<time::OffsetDateTime>,
    pub created_at: time::OffsetDateTime,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RahmenvertragRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub tenant: String,
    pub rahmenvertrag_nr: Option<String>,
    pub vertrag: Option<serde_json::Value>,
    pub status: String,
    pub gueltig_von: Date,
    pub gueltig_bis: Option<Date>,
    pub kuendigungsfrist_monate: i32,
    pub auto_renewal: bool,
    pub renewal_monate: i32,
    pub preisanpassungsformel: Option<String>,
    pub portfolio_rabatt_prozent: Option<rust_decimal::Decimal>,
    pub rechnungsstellung: String,
    pub sammelrechnung_intervall: Option<String>,
    pub erp_rahmenvertrag_id: Option<String>,
    pub created_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VersorgungsvertragRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub rahmenvertrag_id: Option<Uuid>,
    pub tenant: String,
    pub vertrags_nr: Option<String>,
    pub vertrag: Option<serde_json::Value>,
    pub status: String,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub kundentyp: String,
    pub preisgarantie_bis: Option<Date>,
    pub kuendigungsfrist_monate: i32,
    /// §40b EnWG billing cadence: MONATLICH / VIERTELJAEHRLICH /
    /// HALBJAEHRLICH / JAEHRLICH.
    pub abrechnungszyklus: String,
    pub auto_renewal: bool,
    pub bundle_code: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub erp_contract_id: Option<String>,
    pub created_at: time::OffsetDateTime,
    pub completed_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VertragskomponenteRow {
    pub id: Uuid,
    pub vertrag_id: Uuid,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub lf_mp_id: String,
    pub nb_mp_id: Option<String>,
    pub product_code: String,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
    pub status: String,
    pub mako_process_id: Option<String>,
    pub fulfillment_data: Option<serde_json::Value>,
    pub abgelehnt_erc: Option<String>,
    pub abgelehnt_reason: Option<String>,
    pub ablese_auftrag_id: Option<Uuid>,
}

// ── Input types ───────────────────────────────────────────────────────────────

/// Create a new Kunde (legal entity).
/// `oidc_sub` / `email` — if supplied, also creates the first KundenIdentitaet.
/// For B2B customers with multiple portal users, call POST /kunden/{id}/identitaeten
/// for each additional user after the Kunde is created.
#[derive(Debug, Deserialize)]
pub struct CreateKundeInput {
    pub kunden_nr: Option<String>,
    /// Primary portal user OIDC sub — creates a KundenIdentitaet automatically.
    pub oidc_sub: Option<String>,
    pub email: Option<String>,
    pub kundentyp: String, // B2C | B2B_SLP | B2B_RLM | B2B_HV
    pub geschaeftspartner: Option<serde_json::Value>,
    pub organisations_id: Option<String>,
    pub umsatzsteuer_id: Option<String>,
    pub zahlungsziel_tage: Option<i32>,
    pub sepa_erlaubt: Option<bool>,
    pub erp_kunde_id: Option<String>,
    /// § 13b Abs. 2 Nr. 5 lit. b UStG Stromwiederverkäufer flag (USt 1 TH on
    /// file). Defaults to `false`; billingd derives reverse charge from it.
    pub stromwiederverkaeufer: Option<bool>,
    #[allow(dead_code)]
    pub notizen: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRahmenvertragInput {
    /// References the customer — from URL path (kunden_id)
    pub rahmenvertrag_nr: Option<String>,
    pub gueltig_von: Date,
    pub gueltig_bis: Option<Date>,
    pub kuendigungsfrist_monate: Option<i32>,
    pub auto_renewal: Option<bool>,
    pub renewal_monate: Option<i32>,
    pub preisanpassungsformel: Option<String>,
    pub portfolio_rabatt_prozent: Option<rust_decimal::Decimal>,
    pub rechnungsstellung: Option<String>, // EINZEL | SAMMEL | POSITIONEN
    pub sammelrechnung_intervall: Option<String>,
    pub erp_rahmenvertrag_id: Option<String>,
    /// Traceability link to the B2B Angebot that led to this Rahmenvertrag (CPQ pipeline).
    pub angebot_id: Option<Uuid>,
    #[allow(dead_code)]
    pub notizen: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateVersorgungsvertragInput {
    /// kunden_id from URL path
    pub rahmenvertrag_id: Option<Uuid>, // B2B: required; B2C: None
    pub kundentyp: String,
    pub bundle_code: Option<String>,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub kuendigungsfrist_monate: Option<i32>,
    pub preisgarantie_bis: Option<Date>,
    pub auto_renewal: Option<bool>,
    pub standort_bezeichnung: Option<String>,
    pub erp_contract_id: Option<String>,
    #[allow(dead_code)]
    pub notizen: Option<String>,
    pub komponenten: Vec<CreateKomponenteInput>,
}

#[derive(Debug, Deserialize)]
pub struct CreateKomponenteInput {
    pub sparte: String,
    pub malo_id: Option<String>,
    pub melo_id: Option<String>,
    pub nb_mp_id: Option<String>,
    pub product_code: String,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
    pub fulfillment_data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct KuendigungInput {
    pub lieferende: Date,
    #[allow(dead_code)]
    pub grund: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TarifwechselInput {
    /// UUID of the Vertragskomponente to be re-tariffed.
    pub komp_id: Uuid,
    /// New product code in `tarifbd`.
    pub new_product_code: String,
    /// When the new tariff takes effect (must be a valid Tarifwechsel date per §41 EnWG notice).
    pub wirksamkeit: Date,
    /// Optional reason for audit trail.
    #[allow(dead_code)]
    pub grund: Option<String>,
    /// Operator override: bypass `preisgarantie_bis` contract-lock guard.
    /// Must only be set by operators with explicit customer consent (price-lock waiver).
    /// Default `false` — normal requests are blocked when within the guarantee window.
    #[serde(default)]
    pub override_preisgarantie: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct UmzugInput {
    /// Last day of supply at old address.
    pub einzug_datum: Date,
    /// New MaLo ID at new address.
    pub neue_malo_id: Option<String>,
    /// New NB at new address.
    pub neues_nb_mp_id: Option<String>,
    /// Product code for new supply contract (defaults to current product).
    pub new_product_code: Option<String>,
    /// New address description.
    pub neue_standort_bezeichnung: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateKundeInput {
    pub geschaeftspartner: Option<serde_json::Value>,
    pub umsatzsteuer_id: Option<String>,
    pub zahlungsziel_tage: Option<i32>,
    pub sepa_erlaubt: Option<bool>,
}

/// Add or update a portal user identity for a Kunde.
/// Idempotent on oidc_sub: re-PUT updates rolle / standort_filter.
#[derive(Debug, Deserialize)]
pub struct UpsertIdentitaetInput {
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub rolle: Option<String>, // default: VOLLZUGRIFF
    pub standort_filter: Option<String>,
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

pub async fn upsert_kunde(pool: &PgPool, tenant: &str, input: &CreateKundeInput) -> Result<Uuid> {
    let id = Uuid::new_v4();
    // Use RETURNING id so ON CONFLICT returns the *existing* row's id,
    // not the freshly generated UUID that was never inserted.
    // The WHERE clause matches the unique partial index kunden_erp_unique
    // created in the consolidated schema (0001_schema.sql) (erp_kunde_id IS NOT NULL rows only).
    let row = sqlx::query(
        "INSERT INTO kunden
         (id,tenant,kunden_nr,kundentyp,geschaeftspartner,
          organisations_id,umsatzsteuer_id,zahlungsziel_tage,sepa_erlaubt,erp_kunde_id,
          stromwiederverkaeufer)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
         ON CONFLICT (tenant, erp_kunde_id) WHERE erp_kunde_id IS NOT NULL DO UPDATE
           SET geschaeftspartner=EXCLUDED.geschaeftspartner,
               stromwiederverkaeufer=EXCLUDED.stromwiederverkaeufer, updated_at=now()
         RETURNING id",
    )
    .bind(id)
    .bind(tenant)
    .bind(&input.kunden_nr)
    .bind(&input.kundentyp)
    .bind(&input.geschaeftspartner)
    .bind(&input.organisations_id)
    .bind(&input.umsatzsteuer_id)
    .bind(input.zahlungsziel_tage.unwrap_or(14))
    .bind(input.sepa_erlaubt.unwrap_or(true))
    .bind(&input.erp_kunde_id)
    .bind(input.stromwiederverkaeufer.unwrap_or(false))
    .fetch_one(pool)
    .await?;
    let actual_id: Uuid = row.try_get("id")?;

    // If a primary identity (oidc_sub) was provided at create time, upsert it too.
    if let Some(ref sub) = input.oidc_sub {
        let identity = UpsertIdentitaetInput {
            oidc_sub: sub.clone(),
            email: input.email.clone(),
            display_name: None,
            rolle: None,
            standort_filter: None,
        };
        upsert_identitaet(pool, actual_id, tenant, &identity).await?;
    }

    Ok(actual_id)
}

/// Upsert a KundenIdentitaet (portal user) for a Kunde.
/// Idempotent: re-call with same oidc_sub updates rolle / standort_filter.
pub async fn upsert_identitaet(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    input: &UpsertIdentitaetInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let rolle = input.rolle.as_deref().unwrap_or("VOLLZUGRIFF");
    // RETURNING id resolves ON CONFLICT to the existing row's id.
    let row = sqlx::query(
        "INSERT INTO kunden_identitaeten
         (id, kunden_id, tenant, oidc_sub, email, display_name, rolle, standort_filter)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (tenant, oidc_sub) DO UPDATE
           SET email           = COALESCE(EXCLUDED.email, kunden_identitaeten.email),
               display_name    = COALESCE(EXCLUDED.display_name, kunden_identitaeten.display_name),
               rolle           = EXCLUDED.rolle,
               standort_filter = EXCLUDED.standort_filter,
               updated_at      = now()
         RETURNING id",
    )
    .bind(id)
    .bind(kunden_id)
    .bind(tenant)
    .bind(&input.oidc_sub)
    .bind(&input.email)
    .bind(&input.display_name)
    .bind(rolle)
    .bind(&input.standort_filter)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get("id")?)
}

pub async fn list_identitaeten(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<KundenIdentitaetRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM kunden_identitaeten WHERE kunden_id=$1 AND tenant=$2 AND aktiv=true ORDER BY created_at"
    ).bind(kunden_id).bind(tenant).fetch_all(pool).await?)
}

#[allow(dead_code)]
pub async fn deactivate_identitaet(pool: &PgPool, id: Uuid, tenant: &str) -> Result<()> {
    sqlx::query(
        "UPDATE kunden_identitaeten SET aktiv=false, updated_at=now() WHERE id=$1 AND tenant=$2",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fetch_kunde(pool: &PgPool, id: Uuid, tenant: &str) -> Result<Option<KundeRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM kunden WHERE id=$1 AND tenant=$2")
            .bind(id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?,
    )
}

/// Resolve an OIDC sub to the associated Kunde.
/// Joins through kunden_identitaeten so that B2B users (1 company, N logins)
/// all map to the same KundeRow.
pub async fn fetch_kunde_by_sub(
    pool: &PgPool,
    oidc_sub: &str,
    tenant: &str,
) -> Result<Option<KundeRow>> {
    Ok(sqlx::query_as(
        "SELECT k.* FROM kunden k
         JOIN kunden_identitaeten i ON i.kunden_id = k.id
         WHERE i.oidc_sub = $1 AND i.tenant = $2 AND i.aktiv = true",
    )
    .bind(oidc_sub)
    .bind(tenant)
    .fetch_optional(pool)
    .await?)
}

// ── Person sub-object (BO4E Person BO, L13) ───────────────────────────────────

/// Store a canonical `rubo4e::current::Person` BO JSON for a B2C Kunde.
///
/// Validates `_typ: "PERSON"` and re-serialises to camelCase before calling.
/// GDPR Art. 15 right-to-access requires structured `Person` data.
pub async fn upsert_person(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    person: serde_json::Value,
) -> Result<()> {
    let updated =
        sqlx::query("UPDATE kunden SET person=$3, updated_at=now() WHERE id=$1 AND tenant=$2")
            .bind(kunden_id)
            .bind(tenant)
            .bind(&person)
            .execute(pool)
            .await?
            .rows_affected();
    anyhow::ensure!(
        updated > 0,
        "Kunde {kunden_id} not found in tenant {tenant}"
    );
    Ok(())
}

/// Fetch the stored `Person` BO JSON for a Kunde, or `None` if not set.
pub async fn fetch_person(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Option<serde_json::Value>> {
    let row = sqlx::query("SELECT person FROM kunden WHERE id=$1 AND tenant=$2")
        .bind(kunden_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| {
        r.try_get::<Option<serde_json::Value>, _>("person")
            .ok()
            .flatten()
    }))
}

/// Resolve an OIDC sub to the identity row (needed for scope/rolle checks in portald).
pub async fn fetch_identitaet_by_sub(
    pool: &PgPool,
    oidc_sub: &str,
    tenant: &str,
) -> Result<Option<KundenIdentitaetRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM kunden_identitaeten WHERE oidc_sub=$1 AND tenant=$2 AND aktiv=true",
    )
    .bind(oidc_sub)
    .bind(tenant)
    .fetch_optional(pool)
    .await?)
}

pub async fn insert_rahmenvertrag(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    input: &CreateRahmenvertragInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO rahmenvertraege
         (id,kunden_id,tenant,rahmenvertrag_nr,gueltig_von,gueltig_bis,
          kuendigungsfrist_monate,auto_renewal,renewal_monate,
          preisanpassungsformel,portfolio_rabatt_prozent,
          rechnungsstellung,sammelrechnung_intervall,erp_rahmenvertrag_id,angebot_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
         ON CONFLICT (tenant,erp_rahmenvertrag_id) WHERE erp_rahmenvertrag_id IS NOT NULL
           DO UPDATE SET updated_at=now()
         RETURNING id",
    )
    .bind(id)
    .bind(kunden_id)
    .bind(tenant)
    .bind(&input.rahmenvertrag_nr)
    .bind(input.gueltig_von)
    .bind(input.gueltig_bis)
    .bind(input.kuendigungsfrist_monate.unwrap_or(3))
    .bind(input.auto_renewal.unwrap_or(true))
    .bind(input.renewal_monate.unwrap_or(12))
    .bind(&input.preisanpassungsformel)
    .bind(input.portfolio_rabatt_prozent.as_ref())
    .bind(input.rechnungsstellung.as_deref().unwrap_or("EINZEL"))
    .bind(&input.sammelrechnung_intervall)
    .bind(&input.erp_rahmenvertrag_id)
    .bind(input.angebot_id) // $15 — CPQ traceability
    .fetch_one(pool)
    .await?;
    Ok(row.try_get("id")?)
}

/// Compute the earliest legally valid Kündigung date given a contract start and
/// notice period.
///
/// Per §14 StromGVV / §13 GasGVV: the notice period (Kündigungsfrist) runs
/// from the date the notice is received. We return `vertragsbeginn + monate`.
/// The actual end-of-month rounding is the customer's responsibility in practice;
/// we store the strict calendar minimum here.
pub fn earliest_kuendigungsdatum(vertragsbeginn: Date, kuendigungsfrist_monate: i32) -> Date {
    add_months(vertragsbeginn, kuendigungsfrist_monate)
}

/// Add `months` calendar months to `from`, clamping the day to the target month.
///
/// Contract terms are expressed in months, and a term of 1, 3 or 6 months is not
/// expressible in whole years: `year + months / 12` truncated every such term to
/// zero, so an auto-renewal renewed a contract to the day it already ended.
#[must_use]
pub fn add_months(from: Date, months: i32) -> Date {
    // Year carries over when the month index passes 12.
    let total_months = from.month() as i32 + months;
    let extra_years = (total_months - 1).div_euclid(12);
    let new_month = ((total_months - 1).rem_euclid(12) + 1) as u8;
    let new_year = from.year() + extra_years;
    // Clamp day to last day of target month (e.g. Jan 31 + 1M = Feb 28).
    let days_in_month = days_in_month(new_year, new_month);
    let day = from.day().min(days_in_month);
    time::Date::from_calendar_date(
        new_year,
        time::Month::try_from(new_month).unwrap_or(time::Month::January),
        day,
    )
    .unwrap_or(from)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// A component row as it exists in the database after insert — carries the
/// real primary key the MaKo dispatch needs (not a request-body echo).
#[derive(Debug, Clone)]
pub struct InsertedKomponente {
    pub id: Uuid,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub melo_id: Option<String>,
    pub nb_mp_id: Option<String>,
    pub lieferbeginn: Date,
}

/// Result of [`insert_versorgungsvertrag`].
///
/// `komponenten` is **empty on an idempotent conflict replay** — the second
/// POST of the same `erp_contract_id` returns the existing contract with no
/// components to dispatch, which is what prevents a duplicate Lieferbeginn
/// UTILMD. The handler dispatches over exactly these rows, never over the
/// request body.
#[derive(Debug, Clone)]
pub struct InsertedVertrag {
    pub id: Uuid,
    pub is_new: bool,
    pub komponenten: Vec<InsertedKomponente>,
}

pub async fn insert_versorgungsvertrag(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    lf_mp_id: &str,
    input: &CreateVersorgungsvertragInput,
) -> Result<InsertedVertrag> {
    let id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO versorgungsvertraege
         (id,kunden_id,rahmenvertrag_id,tenant,kundentyp,bundle_code,
          vertragsbeginn,vertragsende,kuendigungsfrist_monate,
          preisgarantie_bis,auto_renewal,standort_bezeichnung,erp_contract_id,
          naechste_moegliche_kuendigung)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         ON CONFLICT (tenant,erp_contract_id) WHERE erp_contract_id IS NOT NULL
           DO UPDATE SET updated_at=now()
         RETURNING id, id = $1 AS is_new_insert",
    )
    .bind(id)
    .bind(kunden_id)
    .bind(input.rahmenvertrag_id)
    .bind(tenant)
    .bind(&input.kundentyp)
    .bind(&input.bundle_code)
    .bind(input.vertragsbeginn)
    .bind(input.vertragsende)
    .bind(input.kuendigungsfrist_monate.unwrap_or(1))
    .bind(input.preisgarantie_bis)
    .bind(input.auto_renewal.unwrap_or(false))
    .bind(&input.standort_bezeichnung)
    .bind(&input.erp_contract_id)
    .bind(earliest_kuendigungsdatum(
        input.vertragsbeginn,
        input.kuendigungsfrist_monate.unwrap_or(1),
    ))
    .fetch_one(pool)
    .await?;
    let actual_id: Uuid = row.try_get("id")?;
    // Only insert components for fresh inserts (not on idempotent conflict replay).
    // id = $1 is true only when the row was genuinely inserted; false on conflict.
    let is_new: bool = row.try_get("is_new_insert").unwrap_or(true);
    let mut komponenten = Vec::new();
    if is_new {
        for komp in &input.komponenten {
            let komp_id = insert_komponente(pool, actual_id, lf_mp_id, komp).await?;
            komponenten.push(InsertedKomponente {
                id: komp_id,
                sparte: komp.sparte.clone(),
                malo_id: komp.malo_id.clone(),
                melo_id: komp.melo_id.clone(),
                nb_mp_id: komp.nb_mp_id.clone(),
                lieferbeginn: komp.lieferbeginn,
            });
        }
    }
    Ok(InsertedVertrag {
        id: actual_id,
        is_new,
        komponenten,
    })
}

pub async fn insert_komponente(
    pool: &PgPool,
    vertrag_id: Uuid,
    lf_mp_id: &str,
    k: &CreateKomponenteInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vertragskomponenten
         (id,vertrag_id,tenant,sparte,malo_id,melo_id,lf_mp_id,nb_mp_id,
          product_code,lieferbeginn,lieferende,fulfillment_data)
         SELECT $1,$2,v.tenant,$3,$4,$5,$6,$7,$8,$9,$10,$11
         FROM versorgungsvertraege v WHERE v.id=$2",
    )
    .bind(id)
    .bind(vertrag_id)
    .bind(&k.sparte)
    .bind(&k.malo_id)
    .bind(&k.melo_id)
    .bind(lf_mp_id)
    .bind(&k.nb_mp_id)
    .bind(&k.product_code)
    .bind(k.lieferbeginn)
    .bind(k.lieferende)
    .bind(&k.fulfillment_data)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn fetch_vertrag(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> Result<Option<VersorgungsvertragRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM versorgungsvertraege WHERE id=$1 AND tenant=$2")
            .bind(id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn list_komponenten(
    pool: &PgPool,
    vertrag_id: Uuid,
) -> Result<Vec<VertragskomponenteRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE vertrag_id=$1 ORDER BY created_at")
            .bind(vertrag_id)
            .fetch_all(pool)
            .await?,
    )
}

/// The active Versorgungsvertrag delivering to a MaLo, with its component.
///
/// This is the lookup `billingd` uses to put §40 Abs. 1 EnWG contract facts
/// (Vertragsdauer, Kündigungsfrist, next Kündigungstermin) on the invoice —
/// the contract, not the tariff, is where they live. Newest active contract
/// wins when a MaLo re-contracted within the tenant.
/// One active supply component with its contract's §40b billing cadence —
/// the unit of work for billingd's billing-run worker.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BillingCandidateRow {
    pub malo_id: String,
    pub lf_mp_id: String,
    pub nb_mp_id: Option<String>,
    pub sparte: String,
    /// §40b EnWG cadence chosen on the contract.
    pub abrechnungszyklus: String,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
}

/// All active supply components eligible for scheduled billing (§40b EnWG).
///
/// A component is in supply from the moment the MaKo Lieferbeginn is confirmed.
/// `BESTAETIGT` is that state — nothing ever promotes it to `AKTIV` — so
/// requiring `AKTIV` alone left the §40b billing feed permanently empty. Every
/// other reader already accepts both.
pub async fn list_billing_candidates(
    pool: &PgPool,
    tenant: &str,
) -> Result<Vec<BillingCandidateRow>> {
    let rows: Vec<BillingCandidateRow> = sqlx::query_as(
        "SELECT k.malo_id, k.lf_mp_id, k.nb_mp_id, k.sparte,
                v.abrechnungszyklus, v.vertragsbeginn, v.vertragsende,
                k.lieferbeginn, k.lieferende
         FROM versorgungsvertraege v
         JOIN vertragskomponenten k ON k.vertrag_id = v.id
         WHERE v.tenant = $1
           AND v.status IN ('TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
           AND k.status IN ('AKTIV','BESTAETIGT')
           AND k.malo_id IS NOT NULL
         ORDER BY k.malo_id",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn fetch_vertrag_by_malo(
    pool: &PgPool,
    malo_id: &str,
    tenant: &str,
) -> Result<Option<(VersorgungsvertragRow, VertragskomponenteRow)>> {
    let vertrag: Option<VersorgungsvertragRow> = sqlx::query_as(
        "SELECT v.* FROM versorgungsvertraege v
         JOIN vertragskomponenten k ON k.vertrag_id = v.id
         WHERE k.malo_id=$1 AND v.tenant=$2
           AND v.status IN ('TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
           AND k.status IN ('AKTIV','BESTAETIGT')
         ORDER BY v.vertragsbeginn DESC LIMIT 1",
    )
    .bind(malo_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    let Some(vertrag) = vertrag else {
        return Ok(None);
    };
    // The same status filter as the contract lookup. Without it a later
    // ABGELEHNT/STORNIERT retry row for the same MaLo won the ORDER BY and fed
    // billingd the §40 facts of a component that never went into supply.
    let komponente: Option<VertragskomponenteRow> = sqlx::query_as(
        "SELECT * FROM vertragskomponenten
         WHERE vertrag_id=$1 AND malo_id=$2
           AND status IN ('AKTIV','BESTAETIGT')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(vertrag.id)
    .bind(malo_id)
    .fetch_optional(pool)
    .await?;
    Ok(komponente.map(|k| (vertrag, k)))
}

pub async fn list_offene_vertraege(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM versorgungsvertraege
         WHERE tenant=$1 AND status IN ('ANGELEGT','IN_BEARBEITUNG','TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
         ORDER BY created_at LIMIT $2"
    ).bind(tenant).bind(limit).fetch_all(pool).await?)
}

pub async fn list_vertraege_by_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as("SELECT * FROM versorgungsvertraege WHERE kunden_id=$1 AND tenant=$2 ORDER BY vertragsbeginn DESC")
        .bind(kunden_id).bind(tenant).fetch_all(pool).await?)
}

// ── Sammelrechnung helper (L2) ─────────────────────────────────────────────────

/// One active supply site (MaLo) for a Rahmenvertrag — returned by
/// `GET /api/v1/rahmenvertraege/{id}/malos` consumed by `billingd`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RahmenvertragMaloRow {
    pub malo_id: String,
    pub product_code: Option<String>,
    pub kundentyp: Option<String>,
}

/// List all active MaLo IDs + product codes for a Rahmenvertrag.
pub async fn list_rahmenvertrag_malos(
    pool: &PgPool,
    rahmenvertrag_id: Uuid,
    tenant: &str,
) -> Result<Vec<RahmenvertragMaloRow>> {
    let rows = sqlx::query(
        r"SELECT k.malo_id,
                 vv.bundle_code   AS product_code,
                 ku.kundentyp
          FROM vertragskomponenten k
          JOIN versorgungsvertraege vv ON vv.id = k.vertrag_id
          JOIN kunden ku               ON ku.id  = vv.kunden_id
          WHERE vv.rahmenvertrag_id = $1
            AND vv.tenant           = $2
            AND k.status IN ('AKTIV', 'BESTAETIGT')
            AND k.malo_id IS NOT NULL
          ORDER BY k.malo_id",
    )
    .bind(rahmenvertrag_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| RahmenvertragMaloRow {
            malo_id: r.try_get("malo_id").unwrap_or_default(),
            product_code: r.try_get("product_code").unwrap_or(None),
            kundentyp: r.try_get("kundentyp").unwrap_or(None),
        })
        .collect())
}

pub async fn list_aktive_malo_ids(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT k.malo_id FROM vertragskomponenten k
         JOIN versorgungsvertraege v ON v.id = k.vertrag_id
         WHERE v.kunden_id=$1 AND v.tenant=$2
           AND k.status IN ('AKTIV','BESTAETIGT')
           AND k.malo_id IS NOT NULL",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(m,)| m).collect())
}

/// Statuses a Versorgungsvertrag never leaves.
///
/// A late or replayed MaKo outcome re-derives the contract status from its
/// components, and that derivation knows nothing about a Kündigung or a
/// Stornierung — it happily returned AKTIV for a contract that had already
/// ended. Terminal states are therefore not overwritten.
const VERTRAG_TERMINAL: &str = "('GEKÜNDIGT','ABGELAUFEN','STORNIERT')";

/// Statuses a Vertragskomponente never leaves.
///
/// A replayed rejection flipped an already-confirmed or already-ended component
/// to ABGELEHNT, which took it out of the billing feed retroactively.
const KOMPONENTE_TERMINAL: &str = "('BEENDET','ABGELEHNT','STORNIERT')";

pub async fn update_vertrag_status(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
    status: &str,
) -> Result<()> {
    // A terminal contract may still progress between terminal states
    // (GEKÜNDIGT → ABGELAUFEN once every component has ended), but it never
    // returns to a live one.
    sqlx::query(&format!(
        "UPDATE versorgungsvertraege SET status=$1, updated_at=now(),
         completed_at = CASE WHEN $1 IN ('ABGELAUFEN','STORNIERT') THEN now() ELSE completed_at END
         WHERE id=$2 AND tenant=$3
           AND (status NOT IN {VERTRAG_TERMINAL} OR $1 IN {VERTRAG_TERMINAL})"
    ))
    .bind(status)
    .bind(id)
    .bind(tenant)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn update_komponente_status(
    pool: &PgPool,
    id: Uuid,
    status: &str,
    mako_process_id: Option<&str>,
    malo_id: Option<&str>,
    erc: Option<&str>,
    reason: Option<&str>,
) -> Result<()> {
    // The rejection detail is written only by a rejection. Overwriting it with
    // NULL on every later update erased the ERC code the customer was told.
    sqlx::query(&format!(
        "UPDATE vertragskomponenten SET status=$1, updated_at=now(),
         mako_process_id=COALESCE($2,mako_process_id),
         malo_id=COALESCE($3,malo_id),
         abgelehnt_erc=COALESCE($4,abgelehnt_erc),
         abgelehnt_reason=COALESCE($5,abgelehnt_reason)
         WHERE id=$6 AND status NOT IN {KOMPONENTE_TERMINAL}"
    ))
    .bind(status)
    .bind(mako_process_id)
    .bind(malo_id)
    .bind(erc)
    .bind(reason)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// End a component's supply on `lieferende`.
///
/// The date is what ties the component to the Kündigung that ended it: a
/// Widerruf can then revert exactly the components whose supply had not started
/// running out yet, instead of every BEENDET component the contract ever had.
pub async fn end_komponente(pool: &PgPool, id: Uuid, lieferende: Date) -> Result<()> {
    sqlx::query(&format!(
        "UPDATE vertragskomponenten
         SET status='BEENDET', lieferende=$2, updated_at=now()
         WHERE id=$1 AND status NOT IN {KOMPONENTE_TERMINAL}"
    ))
    .bind(id)
    .bind(lieferende)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn idempotent_event(
    pool: &PgPool,
    event_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
) -> Result<bool> {
    let rows = sqlx::query(
        "INSERT INTO received_events (event_id,event_type,payload)
         VALUES ($1,$2,$3) ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(payload)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(rows > 0)
}

/// Recompute Versorgungsvertrag status from component statuses.
pub fn derive_vertrag_status(komponenten: &[VertragskomponenteRow]) -> &'static str {
    if komponenten.is_empty() {
        return "ANGELEGT";
    }
    let all_terminal = komponenten.iter().all(|k| {
        matches!(
            k.status.as_str(),
            "AKTIV" | "BESTAETIGT" | "BEENDET" | "ABGELEHNT" | "STORNIERT"
        )
    });
    let any_rejected = komponenten.iter().any(|k| k.status == "ABGELEHNT");
    let any_active = komponenten
        .iter()
        .any(|k| matches!(k.status.as_str(), "AKTIV" | "BESTAETIGT"));
    let any_pending = komponenten
        .iter()
        .any(|k| matches!(k.status.as_str(), "ANGELEGT" | "ANGEMELDET"));
    if any_rejected && !any_active && !any_pending {
        return "STORNIERT";
    }
    if all_terminal && any_active {
        return "AKTIV";
    }
    // All components ended (BEENDET), none active → supply fully concluded.
    if all_terminal && !any_active && !any_rejected {
        return "ABGELAUFEN";
    }
    if any_active && any_pending {
        return "TEILERFUELLUNG";
    }
    if komponenten.iter().any(|k| k.status == "ANGEMELDET") {
        return "IN_BEARBEITUNG";
    }
    "ANGELEGT"
}

pub async fn update_kunde(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    input: &UpdateKundeInput,
) -> Result<()> {
    sqlx::query(
        "UPDATE kunden SET
         geschaeftspartner = COALESCE($3, geschaeftspartner),
         umsatzsteuer_id  = COALESCE($4, umsatzsteuer_id),
         zahlungsziel_tage = COALESCE($5, zahlungsziel_tage),
         sepa_erlaubt     = COALESCE($6, sepa_erlaubt),
         updated_at       = now()
         WHERE id=$1 AND tenant=$2",
    )
    .bind(id)
    .bind(tenant)
    .bind(&input.geschaeftspartner)
    .bind(&input.umsatzsteuer_id)
    .bind(input.zahlungsziel_tage)
    .bind(input.sepa_erlaubt)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_komponente_product(
    executor: impl sqlx::PgExecutor<'_>,
    komp_id: Uuid,
    new_product_code: &str,
) -> Result<()> {
    sqlx::query("UPDATE vertragskomponenten SET product_code=$1, updated_at=now() WHERE id=$2")
        .bind(new_product_code)
        .bind(komp_id)
        .execute(executor)
        .await?;
    Ok(())
}

pub async fn fetch_komponente(pool: &PgPool, id: Uuid) -> Result<Option<VertragskomponenteRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn list_rahmenvertraege_by_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<RahmenvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM rahmenvertraege WHERE kunden_id=$1 AND tenant=$2 ORDER BY gueltig_von DESC",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

// \u2500\u2500 Pending Tarifwechsel (B13 \u2014 \u00a741 Abs. 3 EnWG Preisanpassungsbenachrichtigung) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// A component with a scheduled future Tarifwechsel.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
#[allow(dead_code)]
pub struct PendingTarifwechselRow {
    pub komp_id: Uuid,
    pub vertrag_id: Uuid,
    pub malo_id: Option<String>,
    pub lf_mp_id: String,
    pub current_product_code: String,
    pub pending_product_code: String,
    pub pending_wirksamkeit: Date,
    pub preisanpassung_notif_sent: bool,
    pub tenant: String,
}

/// Store a planned future Tarifwechsel without applying it yet.
///
/// Called when `wirksamkeit > today`.  The background worker applies the change
/// on the `wirksamkeit` date and emits the 6-week advance notification.
pub async fn store_pending_tarifwechsel(
    executor: impl sqlx::PgExecutor<'_>,
    komp_id: Uuid,
    new_product_code: &str,
    wirksamkeit: Date,
) -> Result<()> {
    sqlx::query(
        r"UPDATE vertragskomponenten
          SET pending_product_code      = $1,
              pending_wirksamkeit       = $2,
              preisanpassung_notif_sent = FALSE,
              updated_at                = now()
          WHERE id = $3",
    )
    .bind(new_product_code)
    .bind(wirksamkeit)
    .bind(komp_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Apply the pending Tarifwechsel and clear the pending fields.
///
/// Called by the background worker on `pending_wirksamkeit` date.
pub async fn apply_pending_tarifwechsel(pool: &PgPool, komp_id: Uuid) -> Result<()> {
    sqlx::query(
        r"UPDATE vertragskomponenten
          SET product_code              = pending_product_code,
              pending_product_code      = NULL,
              pending_wirksamkeit       = NULL,
              preisanpassung_notif_sent = FALSE,
              updated_at                = now()
          WHERE id = $1
            AND pending_product_code IS NOT NULL",
    )
    .bind(komp_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the advance notification as sent.
pub async fn mark_preisanpassung_notif_sent(pool: &PgPool, komp_id: Uuid) -> Result<()> {
    sqlx::query(
        r"UPDATE vertragskomponenten
          SET preisanpassung_notif_sent = TRUE, updated_at = now()
          WHERE id = $1",
    )
    .bind(komp_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Find components whose pending Tarifwechsel is due today (must be applied).
pub async fn find_tarifwechsel_due_today(
    pool: &PgPool,
    tenant: &str,
    today: Date,
) -> Result<Vec<PendingTarifwechselRow>> {
    Ok(sqlx::query_as::<_, PendingTarifwechselRow>(
        r"SELECT k.id AS komp_id,
                 k.vertrag_id,
                 k.malo_id,
                 k.lf_mp_id,
                 k.product_code AS current_product_code,
                 k.pending_product_code,
                 k.pending_wirksamkeit,
                 k.preisanpassung_notif_sent,
                 k.tenant
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          WHERE k.tenant = $1
            AND k.pending_wirksamkeit <= $2
            AND k.pending_product_code IS NOT NULL",
    )
    .bind(tenant)
    .bind(today)
    .fetch_all(pool)
    .await?)
}

/// Find components whose pending Tarifwechsel falls in the 6-week notification window
/// (i.e., `pending_wirksamkeit` is in [today+41d, today+42d]) and the notification
/// has not yet been sent.
///
/// Catch-up semantics: everything whose Wirksamkeit lies within the next 42
/// days and has not been notified yet is returned. `preisanpassung_notif_sent`
/// (set by the caller after emission) is what makes the daily run fire once
/// per component — NOT a narrow date window. A one-day window would silently
/// skip the notice whenever the worker missed a single day; a late notice is
/// still owed (§5 Abs. 2 StromGVV/GasGVV six weeks; §41 Abs. 5 EnWG one month
/// for Haushaltskunden), and the caller can see from `pending_wirksamkeit`
/// how much notice actually remains.
pub async fn find_tarifwechsel_needing_notif(
    pool: &PgPool,
    tenant: &str,
    today: Date,
) -> Result<Vec<PendingTarifwechselRow>> {
    let window_start = today;
    let window_end = today + time::Duration::days(42);
    Ok(sqlx::query_as::<_, PendingTarifwechselRow>(
        r"SELECT k.id AS komp_id,
                 k.vertrag_id,
                 k.malo_id,
                 k.lf_mp_id,
                 k.product_code AS current_product_code,
                 k.pending_product_code,
                 k.pending_wirksamkeit,
                 k.preisanpassung_notif_sent,
                 k.tenant
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          WHERE k.tenant = $1
            AND k.pending_wirksamkeit >  $2
            AND k.pending_wirksamkeit <= $3
            AND k.pending_product_code IS NOT NULL
            AND k.preisanpassung_notif_sent = FALSE",
    )
    .bind(tenant)
    .bind(window_start)
    .bind(window_end)
    .fetch_all(pool)
    .await?)
}

// ── Preisgarantie typed REST resource ─────────────────────────────────────────

/// Store or replace the BO4E `Preisgarantie` COM JSON for a Versorgungsvertrag.
///
/// Also updates the `preisgarantie_bis` date column (used by the `tarifwechsel`
/// guard to reject price-lock violations without loading the full JSONB).
pub async fn upsert_preisgarantie(
    executor: impl sqlx::PgExecutor<'_>,
    vertrag_id: Uuid,
    tenant: &str,
    preisgarantie: serde_json::Value,
    bis: Option<time::Date>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege \
         SET preisgarantie = $3, preisgarantie_bis = $4, updated_at = now() \
         WHERE id = $1 AND tenant = $2",
    )
    .bind(vertrag_id)
    .bind(tenant)
    .bind(&preisgarantie)
    .bind(bis)
    .execute(executor)
    .await?;
    Ok(())
}

/// Fetch the stored `Preisgarantie` BO JSON for a Versorgungsvertrag, or `None` if not set.
pub async fn fetch_preisgarantie(
    pool: &PgPool,
    vertrag_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row =
        sqlx::query("SELECT preisgarantie FROM versorgungsvertraege WHERE id=$1 AND tenant=$2")
            .bind(vertrag_id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|r| r.try_get::<serde_json::Value, _>("preisgarantie").ok()))
}

// ── Operator / CRM helpers ────────────────────────────────────────────────────

/// Row returned by list_kunden (lightweight — no JSONB blobs).
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct KundeListRow {
    pub id: Uuid,
    pub tenant: String,
    pub kunden_nr: Option<String>,
    pub kundentyp: String,
    pub organisations_id: Option<String>,
    pub erp_kunde_id: Option<String>,
    pub zahlungsziel_tage: i32,
    pub created_at: time::OffsetDateTime,
}

/// List all Kunden for a tenant (operator / CRM endpoint).
pub async fn list_kunden(
    pool: &PgPool,
    tenant: &str,
    kundentyp: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<KundeListRow>> {
    Ok(sqlx::query_as::<_, KundeListRow>(
        r"SELECT id, tenant, kunden_nr, kundentyp, organisations_id,
                 erp_kunde_id, zahlungsziel_tage, created_at
          FROM kunden
          WHERE tenant = $1
            AND ($2::TEXT IS NULL OR kundentyp = $2)
          ORDER BY created_at DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(kundentyp)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Storniere a contract that is still ANGELEGT or IN_BEARBEITUNG (no supply active yet).
///
/// Sets all non-terminal components to STORNIERT and the contract itself to STORNIERT.
/// For IN_BEARBEITUNG contracts the caller must separately cancel the in-flight MaKo
/// process via `processd` (there is no automated MaKo rollback yet — this is a known
/// limitation: processd processes are idempotent and will be rejected by the NB if
/// Lieferbeginn was already confirmed).
pub async fn storniere_vertrag(pool: &PgPool, id: Uuid, tenant: &str) -> anyhow::Result<()> {
    // The contract row FIRST, guarded on state: Stornierung is a
    // pre-activation cancel. A contract in AKTIV/TEILERFUELLUNG carries a
    // confirmed MaKo commitment — cancelling it here would contradict the
    // NB-confirmed Lieferbeginn; that path is Kündigung, not Stornierung.
    // The guard lives in SQL, not only in the handler, so no other caller
    // can bypass it.
    let updated = sqlx::query(
        "UPDATE versorgungsvertraege SET status = 'STORNIERT', updated_at = now()
         WHERE id = $1 AND tenant = $2
           AND status IN ('ANGELEGT','IN_BEARBEITUNG')",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(
            "Stornierung refused: contract is not in ANGELEGT/IN_BEARBEITUNG \
             (active supply must be terminated via Kündigung)"
        );
    }
    // Then the components — tenant-scoped like every other mutation.
    sqlx::query(
        r"UPDATE vertragskomponenten
          SET status = 'STORNIERT', updated_at = now()
          WHERE vertrag_id = $1
            AND tenant = $2
            AND status NOT IN ('AKTIV','BEENDET','BESTAETIGT','STORNIERT')",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await?;
    Ok(())
}

/// Deactivate a KundenIdentitaet (portal user) by OIDC sub.
pub async fn deactivate_identitaet_by_sub(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    oidc_sub: &str,
) -> anyhow::Result<bool> {
    let n = sqlx::query(
        "UPDATE kunden_identitaeten
         SET aktiv = false, updated_at = now()
         WHERE kunden_id = $1 AND tenant = $2 AND oidc_sub = $3 AND aktiv = true",
    )
    .bind(kunden_id)
    .bind(tenant)
    .bind(oidc_sub)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// GDPR Art. 15 — full customer data export (all tables, no JSONB truncation).
#[derive(Debug, serde::Serialize)]
pub struct GdprExportRow {
    pub kunde: KundeRow,
    pub person: Option<serde_json::Value>,
    pub zahlungsinformation: Option<serde_json::Value>,
    pub identitaeten: Vec<KundenIdentitaetRow>,
    pub vertraege: Vec<VersorgungsvertragRow>,
    pub komponenten: Vec<VertragskomponenteRow>,
}

pub async fn gdpr_export(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<GdprExportRow>> {
    let Some(kunde) = fetch_kunde(pool, kunden_id, tenant).await? else {
        return Ok(None);
    };
    let person = fetch_person(pool, kunden_id, tenant).await.ok().flatten();
    let zahlungsinformation = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT zahlungsinformation FROM kunden WHERE id = $1 AND tenant = $2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let identitaeten = list_identitaeten(pool, kunden_id, tenant)
        .await
        .unwrap_or_default();
    let vertraege = list_vertraege_by_kunde(pool, kunden_id, tenant)
        .await
        .unwrap_or_default();
    let mut all_komponenten = Vec::new();
    for v in &vertraege {
        if let Ok(komps) = list_komponenten(pool, v.id).await {
            all_komponenten.extend(komps);
        }
    }
    Ok(Some(GdprExportRow {
        kunde,
        person,
        zahlungsinformation,
        identitaeten,
        vertraege,
        komponenten: all_komponenten,
    }))
}

// ── Additional query helpers for MCP tools ────────────────────────────────────

/// Fetch a single Rahmenvertrag by UUID.
pub async fn fetch_rahmenvertrag(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<RahmenvertragRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM rahmenvertraege WHERE id=$1 AND tenant=$2")
            .bind(id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?,
    )
}

/// List all Rahmenverträge for a tenant (operator CRM view).
pub async fn list_all_rahmenvertraege(
    pool: &PgPool,
    tenant: &str,
    status: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<RahmenvertragRow>> {
    Ok(sqlx::query_as(
        r"SELECT * FROM rahmenvertraege
          WHERE tenant = $1
            AND ($2::text IS NULL OR status = $2)
          ORDER BY gueltig_von DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// List contracts with GEKÜNDIGT status (active Kündigung, lieferende in future).
/// Used by MCP tool `list_pending_kuendigungen`.
pub async fn list_pending_kuendigungen(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> anyhow::Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        r"SELECT * FROM versorgungsvertraege
          WHERE tenant = $1
            AND status = 'GEKÜNDIGT'
            AND (vertragsende IS NULL OR vertragsende >= CURRENT_DATE)
          ORDER BY vertragsende ASC NULLS LAST
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Fetch Zahlungsinformation JSONB for a customer (SEPA/IBAN details).
pub async fn fetch_zahlungsinformation(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row = sqlx::query("SELECT zahlungsinformation FROM kunden WHERE id=$1 AND tenant=$2")
        .bind(kunden_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| {
        r.try_get::<Option<serde_json::Value>, _>("zahlungsinformation")
            .ok()
            .flatten()
    }))
}

/// Check whether a Tarifwechsel is currently blocked by an active Preisgarantie.
/// Returns (is_blocked, preisgarantie_bis) for the given contract.
pub async fn check_preisgarantie_for_mcp(
    pool: &PgPool,
    vertrag_id: Uuid,
    tenant: &str,
    wirksamkeit: time::Date,
) -> anyhow::Result<(bool, Option<time::Date>)> {
    let row =
        sqlx::query("SELECT preisgarantie_bis FROM versorgungsvertraege WHERE id=$1 AND tenant=$2")
            .bind(vertrag_id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?;

    let garantie_bis: Option<time::Date> = row.and_then(|r| {
        r.try_get::<Option<time::Date>, _>("preisgarantie_bis")
            .ok()
            .flatten()
    });
    let blocked = garantie_bis.is_some_and(|g| wirksamkeit <= g);
    Ok((blocked, garantie_bis))
}

/// Summary of MaKo trigger status for a Versorgungsvertrag.
/// Returns count of components by status for quick health check.
pub async fn mako_trigger_status(
    pool: &PgPool,
    vertrag_id: Uuid,
) -> anyhow::Result<serde_json::Value> {
    let komps: Vec<VertragskomponenteRow> =
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE vertrag_id=$1")
            .bind(vertrag_id)
            .fetch_all(pool)
            .await?;

    let mut by_status: std::collections::HashMap<&str, Vec<serde_json::Value>> =
        std::collections::HashMap::new();
    for k in &komps {
        let entry = serde_json::json!({
            "komp_id": k.id,
            "sparte": k.sparte,
            "malo_id": k.malo_id,
            "mako_process_id": k.mako_process_id,
            "abgelehnt_erc": k.abgelehnt_erc,
        });
        by_status.entry(k.status.as_str()).or_default().push(entry);
    }

    let lieferbeginn_dispatched = komps
        .iter()
        .any(|k| k.mako_process_id.is_some() || k.status != "ANGELEGT");

    Ok(serde_json::json!({
        "vertrag_id": vertrag_id,
        "komponenten_count": komps.len(),
        "by_status": by_status,
        "lieferbeginn_dispatched": lieferbeginn_dispatched,
        "all_confirmed": komps.iter().all(|k| matches!(k.status.as_str(), "AKTIV" | "BESTAETIGT")),
        "any_rejected": komps.iter().any(|k| k.status == "ABGELEHNT"),
        "any_stuck": komps.iter().any(|k| k.status == "ANGEMELDET"),
    }))
}

// ── Portal identity helpers ───────────────────────────────────────────────────

/// Update `letzter_login` timestamp for a KundenIdentitaet after successful authentication.
/// Called by `get_authenticate` on every successful MaLo ownership check.
pub async fn update_letzter_login(pool: &PgPool, oidc_sub: &str, tenant: &str) -> Result<()> {
    sqlx::query(
        "UPDATE kunden_identitaeten SET letzter_login = now(), updated_at = now()
         WHERE oidc_sub = $1 AND tenant = $2 AND aktiv = true",
    )
    .bind(oidc_sub)
    .bind(tenant)
    .execute(pool)
    .await?;
    Ok(())
}

/// Count active KundenIdentitaeten for a Kunde.
/// Used to enforce `max_identitaeten_per_kunde` limit.
pub async fn count_active_identitaeten(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<i64> {
    let row = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM kunden_identitaeten
         WHERE kunden_id = $1 AND tenant = $2 AND aktiv = true",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// ── Kündigung Widerruf (withdrawal before effective date) ─────────────────────

/// Revert a GEKÜNDIGT contract back to AKTIV (Kündigung Widerruf).
///
/// Valid only when:
/// 1. The contract is in GEKÜNDIGT status.
/// 2. The `lieferende` has not yet passed (i.e., today < min(lieferende) across components).
///
/// Sets all BEENDET components back to AKTIV. Callers must separately cancel
/// the in-flight Lieferende UTILMD via processd.
pub async fn widerruf_kuendigung(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    tenant: &str,
) -> Result<()> {
    // Verify contract is in GEKÜNDIGT state
    let vertrag: Option<(String,)> =
        sqlx::query_as("SELECT status FROM versorgungsvertraege WHERE id = $1 AND tenant = $2")
            .bind(id)
            .bind(tenant)
            .fetch_optional(&mut *conn)
            .await?;

    let status = vertrag
        .ok_or_else(|| anyhow::anyhow!("Vertrag {id} not found"))?
        .0;
    if status != "GEKÜNDIGT" {
        anyhow::bail!(
            "Kündigung Widerruf only allowed for GEKÜNDIGT contracts, current status: {status}"
        );
    }

    // Revert only the components this Kündigung ended — those whose supply has
    // not run out yet. Reverting every BEENDET component put a component that
    // had ended for its own reasons, possibly years earlier, back into supply
    // and nulled the lieferende that said when it left.
    sqlx::query(
        "UPDATE vertragskomponenten
         SET status = 'AKTIV', lieferende = NULL, updated_at = now()
         WHERE vertrag_id = $1 AND status = 'BEENDET'
           AND (lieferende IS NULL OR lieferende > CURRENT_DATE)",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;

    // Revert contract status to AKTIV
    sqlx::query(
        "UPDATE versorgungsvertraege
         SET status = 'AKTIV', updated_at = now()
         WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .execute(&mut *conn)
    .await?;

    Ok(())
}

// ── Rahmenvertrag helpers ─────────────────────────────────────────────────────

/// List all Versorgungsverträge belonging to a Rahmenvertrag.
pub async fn list_versorgungsvertraege_by_rahmenvertrag(
    pool: &PgPool,
    rahmenvertrag_id: Uuid,
    tenant: &str,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM versorgungsvertraege
         WHERE rahmenvertrag_id = $1 AND tenant = $2
           AND status IN ('AKTIV', 'TEILERFUELLUNG', 'GEKÜNDIGT')
         ORDER BY vertragsbeginn",
    )
    .bind(rahmenvertrag_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}
// ── Expiring contracts (contract lifecycle monitoring) ─────────────────────────

/// Summary row for the expiring-contracts endpoint and MCP tool.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ExpiringVertragRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub vertrags_nr: Option<String>,
    pub status: String,
    pub kundentyp: String,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub preisgarantie_bis: Option<Date>,
    pub bundle_code: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub auto_renewal: bool,
}

/// List Versorgungsverträge where `vertragsende` OR `preisgarantie_bis` falls
/// within the next `within_days` calendar days.
///
/// Used for proactive customer contact (renewal offers, price-lock warnings).
pub async fn find_expiring_vertraege(
    pool: &PgPool,
    tenant: &str,
    within_days: i64,
) -> anyhow::Result<Vec<ExpiringVertragRow>> {
    let today = time::OffsetDateTime::now_utc().date();
    let cutoff = today + time::Duration::days(within_days);
    Ok(sqlx::query_as::<_, ExpiringVertragRow>(
        r"SELECT id, kunden_id, vertrags_nr, status, kundentyp,
                 vertragsbeginn, vertragsende, preisgarantie_bis,
                 bundle_code, standort_bezeichnung, auto_renewal
          FROM versorgungsvertraege
          WHERE tenant = $1
            AND status IN ('AKTIV', 'GEKÜNDIGT')
            AND (
                  (vertragsende IS NOT NULL AND vertragsende BETWEEN $2 AND $3)
               OR (preisgarantie_bis IS NOT NULL AND preisgarantie_bis BETWEEN $2 AND $3)
            )
          ORDER BY LEAST(
              COALESCE(vertragsende, 'infinity'::DATE),
              COALESCE(preisgarantie_bis, 'infinity'::DATE)
          )",
    )
    .bind(tenant)
    .bind(today)
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

// ── Stuck MaKo workflows ───────────────────────────────────────────────────────

/// A component stuck in ANGEMELDET status beyond the expected deadline.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct StuckKomponenteRow {
    pub komp_id: Uuid,
    pub vertrag_id: Uuid,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub lf_mp_id: String,
    pub nb_mp_id: Option<String>,
    pub status: String,
    pub mako_process_id: Option<String>,
    pub angemeldet_since: time::OffsetDateTime,
    pub days_stuck: i64,
}

/// Find Vertragskomponenten stuck in `ANGEMELDET` status beyond a threshold.
///
/// Regulatory deadlines: GPKE §20 EnWG — Strom 5 WT, GeLi Gas 10 WT.
/// The `threshold_days` parameter should be set to `5` (Strom) or `10` (Gas)
/// depending on the consumer's intended filtering.
pub async fn find_stuck_komponents(
    pool: &PgPool,
    tenant: &str,
    threshold_days: i64,
) -> anyhow::Result<Vec<StuckKomponenteRow>> {
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(threshold_days);
    Ok(sqlx::query_as::<_, StuckKomponenteRow>(
        r"SELECT k.id AS komp_id,
                 k.vertrag_id,
                 k.sparte,
                 k.malo_id,
                 k.lf_mp_id,
                 k.nb_mp_id,
                 k.status,
                 k.mako_process_id,
                 k.updated_at AS angemeldet_since,
                 EXTRACT(DAY FROM now() - k.updated_at)::BIGINT AS days_stuck
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          WHERE k.tenant = $1
            AND k.status = 'ANGEMELDET'
            AND k.updated_at < $2
          ORDER BY k.updated_at ASC",
    )
    .bind(tenant)
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

// ── B2B portfolio summary ─────────────────────────────────────────────────────

/// Per-MaLo summary for a B2B portfolio overview.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct PortfolioItemRow {
    pub vertrag_id: Uuid,
    pub vertrags_nr: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub product_code: String,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
    pub status: String,
    pub vertrag_status: String,
}

/// Return all active Vertragskomponenten for a Kunde (B2B portfolio view).
pub async fn list_portfolio_by_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Vec<PortfolioItemRow>> {
    Ok(sqlx::query_as::<_, PortfolioItemRow>(
        r"SELECT k.vertrag_id,
                 v.vertrags_nr,
                 v.standort_bezeichnung,
                 k.sparte,
                 k.malo_id,
                 k.product_code,
                 k.lieferbeginn,
                 k.lieferende,
                 k.status,
                 v.status AS vertrag_status
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          WHERE v.kunden_id = $1 AND v.tenant = $2
            AND k.status IN ('AKTIV','BESTAETIGT','ANGEMELDET')
          ORDER BY v.standort_bezeichnung, k.sparte",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

// ── Auto-renewal (§13 GasGVV / §14 StromGVV) ─────────────────────────────────

/// Verträge due for auto-renewal within the given look-ahead window.
/// These need a 30-day customer notification before the new term starts.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct AutoRenewalRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub vertrags_nr: Option<String>,
    pub vertragsende: Date,
    pub renewal_monate: i32,
    pub bundle_code: Option<String>,
}

/// Find AKTIV vertraege with `auto_renewal = TRUE` whose `vertragsende` falls
/// within the next `look_ahead_days` days and whose notice has not gone out yet.
///
/// The notice is tracked against the term it announces
/// (`autoerneuerung_notif_fuer`), so the daily loop stops re-sending it and the
/// next term's notice is still due.
pub async fn find_auto_renewal_due(
    pool: &PgPool,
    tenant: &str,
    look_ahead_days: i64,
) -> anyhow::Result<Vec<AutoRenewalRow>> {
    let today = time::OffsetDateTime::now_utc().date();
    let cutoff = today + time::Duration::days(look_ahead_days);
    Ok(sqlx::query_as::<_, AutoRenewalRow>(
        r"SELECT id, kunden_id, vertrags_nr, vertragsende, renewal_monate, bundle_code
          FROM versorgungsvertraege
          WHERE tenant = $1
            AND status = 'AKTIV'
            AND auto_renewal = TRUE
            AND vertragsende IS NOT NULL
            AND vertragsende BETWEEN $2 AND $3
            AND autoerneuerung_notif_fuer IS DISTINCT FROM vertragsende",
    )
    .bind(tenant)
    .bind(today)
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

/// Record that the 30-day Ankündigung for the term ending `vertragsende` went out.
pub async fn mark_auto_renewal_notified(
    pool: &PgPool,
    id: Uuid,
    vertragsende: Date,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege \
         SET autoerneuerung_notif_fuer = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(vertragsende)
    .execute(pool)
    .await?;
    Ok(())
}

/// Verträge whose term has ended and that renew automatically.
///
/// `vertragsende <= today` rather than `= today`: the worker runs once a day, so
/// a single missed run otherwise skipped the renewal permanently and left the
/// contract expired.
pub async fn find_auto_renewal_overdue(
    pool: &PgPool,
    tenant: &str,
    today: Date,
) -> anyhow::Result<Vec<AutoRenewalRow>> {
    Ok(sqlx::query_as::<_, AutoRenewalRow>(
        r"SELECT id, kunden_id, vertrags_nr, vertragsende, renewal_monate, bundle_code
          FROM versorgungsvertraege
          WHERE tenant = $1
            AND status = 'AKTIV'
            AND auto_renewal = TRUE
            AND vertragsende IS NOT NULL
            AND vertragsende <= $2",
    )
    .bind(tenant)
    .bind(today)
    .fetch_all(pool)
    .await?)
}

/// The end date an overdue term renews to.
///
/// Renewal runs from the term that ended, not from today, and repeats until the
/// contract is in force again — a contract several terms overdue is caught up in
/// one pass rather than one term per day.
#[must_use]
pub fn renewed_vertragsende(vertragsende: Date, renewal_monate: i32, today: Date) -> Option<Date> {
    if renewal_monate <= 0 {
        return None;
    }
    let mut end = vertragsende;
    // 12 terms is more catch-up than any real outage needs, and bounds the loop.
    for _ in 0..12 {
        end = add_months(end, renewal_monate);
        if end > today {
            return Some(end);
        }
    }
    Some(end)
}

/// Apply auto-renewal: extend vertragsende by renewal_monate months.
pub async fn apply_auto_renewal(pool: &PgPool, id: Uuid, new_end: Date) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege \
         SET vertragsende = $2, updated_at = now() \
         WHERE id = $1 AND auto_renewal = TRUE AND status = 'AKTIV'",
    )
    .bind(id)
    .bind(new_end)
    .execute(pool)
    .await?;
    Ok(())
}

// ── GDPR Art. 17 — Anonymization (right to erasure) ──────────────────────────

/// Pseudonymize all PII for a customer (GDPR Art. 17 — right to erasure).
///
/// Retains contract records for the legal retention period (§ 147 Abs. 3 AO:
/// Handelsbriefe 6 years, Buchungsbelege 8 years — kept 8, the longest applicable)
/// but replaces all personal data with non-reversible pseudonyms.
///
/// Fields anonymized:
/// - `kunden.geschaeftspartner` — company name / address replaced with pseudonym
/// - `kunden.person` — natural-person details nulled
/// - `kunden.zahlungsinformation` — IBAN/BIC replaced with pseudonym token
/// - `kunden.umsatzsteuer_id` — VAT ID nulled
/// - `kunden_identitaeten.oidc_sub` — replaced with `anon:{hash}` token
/// - `kunden_identitaeten.email` / `display_name` — nulled
/// - `kunden_identitaeten.aktiv` — set false (prevents portal access)
///
/// An immutable log entry is created in `anonymization_log` (the consolidated schema (0001_schema.sql)).
pub async fn anonymize_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    requested_by: &str,
) -> anyhow::Result<bool> {
    // Verify the customer exists and belongs to this tenant.
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM kunden WHERE id=$1 AND tenant=$2)",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_one(pool)
    .await?;

    if !exists {
        return Ok(false);
    }

    // Generate a stable opaque token for cross-field consistency.
    let anon_token = format!("anon:{}", uuid::Uuid::new_v4().simple());

    // Pseudonymize kunden PII.
    sqlx::query(
        r"UPDATE kunden
          SET geschaeftspartner  = jsonb_build_object(
                  '_typ', 'GESCHAEFTSPARTNER',
                  'name1', $3,
                  'anrede', 'INDIVIDUELL'
              ),
              person             = NULL,
              zahlungsinformation = jsonb_build_object(
                  '_typ', 'ZAHLUNGSINFORMATION',
                  'iban', 'ANONYMIZED',
                  'zahlungsart', 'UEBERWEISUNG'
              ),
              umsatzsteuer_id    = NULL,
              kunden_nr          = NULL,
              updated_at         = now()
          WHERE id = $1 AND tenant = $2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .bind(&anon_token)
    .execute(pool)
    .await?;

    // Pseudonymize / deactivate all portal identities.
    sqlx::query(
        r"UPDATE kunden_identitaeten
          SET oidc_sub     = $3,
              email        = NULL,
              display_name = NULL,
              aktiv        = FALSE,
              updated_at   = now()
          WHERE kunden_id = $1 AND tenant = $2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .bind(format!("anon:{}", uuid::Uuid::new_v4().simple()))
    .execute(pool)
    .await?;

    // Write immutable audit log entry (the consolidated schema (0001_schema.sql)).
    sqlx::query(
        r"INSERT INTO anonymization_log
          (tenant, kunden_id, anonymized_fields, requested_by, retention_basis)
          VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant)
    .bind(kunden_id)
    .bind(
        &[
            "geschaeftspartner",
            "person",
            "zahlungsinformation",
            "umsatzsteuer_id",
            "kunden_nr",
            "oidc_sub",
            "email",
            "display_name",
        ][..],
    )
    .bind(requested_by)
    .bind("§ 147 Abs. 3 AO: Handelsbriefe 6 Jahre / Buchungsbelege 8 Jahre Aufbewahrungspflicht")
    .execute(pool)
    .await?;

    Ok(true)
}

// ── Aggregatorverträge (§41e EnWG) ────────────────────────────────────────────

/// A §41e EnWG Aggregatorvertrag: SR-ID → agreed capacity price and validity.
///
/// Transposes Art. 17 RL (EU) 2019/944 ("Demand response through aggregation").
/// `billingd` reads this when settling a `de.vpp.dispatch.confirmed` event; it
/// keeps no copy of the contract.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AggregatorvertragRow {
    pub id: Uuid,
    pub tenant: String,
    pub sr_id: String,
    pub vpp_id: String,
    pub malo_id: String,
    pub aggregator_mp_id: String,
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
    pub kunden_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Fields accepted when creating or replacing an Aggregatorvertrag.
#[derive(Debug, Clone, Deserialize)]
pub struct UpsertAggregatorvertragInput {
    pub vpp_id: String,
    pub malo_id: String,
    pub aggregator_mp_id: String,
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    pub vertragsbeginn: Date,
    #[serde(default)]
    pub vertragsende: Option<Date>,
    #[serde(default)]
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
    #[serde(default)]
    pub kunden_id: Option<Uuid>,
}

/// Upsert an Aggregatorvertrag, keyed on `(tenant, sr_id, vertragsbeginn)`.
///
/// The `agg_no_overlap` exclusion constraint rejects a validity window that
/// overlaps an existing contract for the same SR — surfaced to the caller as a
/// conflict rather than silently creating a second active contract.
pub async fn upsert_aggregatorvertrag(
    pool: &PgPool,
    tenant: &str,
    sr_id: &str,
    input: &UpsertAggregatorvertragInput,
) -> Result<Uuid> {
    let r = sqlx::query(
        r"INSERT INTO aggregatorvertraege
              (tenant, sr_id, vpp_id, malo_id, aggregator_mp_id,
               capacity_price_eur_per_kwh, vertragsbeginn, vertragsende,
               mwst_rate_override, kunden_id)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          ON CONFLICT (tenant, sr_id, vertragsbeginn) DO UPDATE
          SET vpp_id                     = EXCLUDED.vpp_id,
              malo_id                    = EXCLUDED.malo_id,
              aggregator_mp_id           = EXCLUDED.aggregator_mp_id,
              capacity_price_eur_per_kwh = EXCLUDED.capacity_price_eur_per_kwh,
              vertragsende               = EXCLUDED.vertragsende,
              mwst_rate_override         = EXCLUDED.mwst_rate_override,
              kunden_id                  = EXCLUDED.kunden_id,
              updated_at                 = now()
          RETURNING id",
    )
    .bind(tenant)
    .bind(sr_id)
    .bind(&input.vpp_id)
    .bind(&input.malo_id)
    .bind(&input.aggregator_mp_id)
    .bind(input.capacity_price_eur_per_kwh)
    .bind(input.vertragsbeginn)
    .bind(input.vertragsende)
    .bind(input.mwst_rate_override)
    .bind(input.kunden_id)
    .fetch_one(pool)
    .await?;
    Ok(r.try_get("id")?)
}

/// The Aggregatorvertrag in force for `sr_id` on `on_date`, if any.
pub async fn find_active_aggregatorvertrag(
    pool: &PgPool,
    tenant: &str,
    sr_id: &str,
    on_date: Date,
) -> Result<Option<AggregatorvertragRow>> {
    Ok(sqlx::query_as::<_, AggregatorvertragRow>(
        r"SELECT id, tenant, sr_id, vpp_id, malo_id, aggregator_mp_id,
                 capacity_price_eur_per_kwh, vertragsbeginn, vertragsende,
                 mwst_rate_override, kunden_id, updated_at
          FROM aggregatorvertraege
          WHERE tenant = $1
            AND sr_id  = $2
            AND vertragsbeginn <= $3
            AND (vertragsende IS NULL OR vertragsende > $3)
          ORDER BY vertragsbeginn DESC
          LIMIT 1",
    )
    .bind(tenant)
    .bind(sr_id)
    .bind(on_date)
    .fetch_optional(pool)
    .await?)
}

/// List every Aggregatorvertrag for a tenant, newest validity first.
pub async fn list_aggregatorvertraege(
    pool: &PgPool,
    tenant: &str,
) -> Result<Vec<AggregatorvertragRow>> {
    Ok(sqlx::query_as::<_, AggregatorvertragRow>(
        r"SELECT id, tenant, sr_id, vpp_id, malo_id, aggregator_mp_id,
                 capacity_price_eur_per_kwh, vertragsbeginn, vertragsende,
                 mwst_rate_override, kunden_id, updated_at
          FROM aggregatorvertraege
          WHERE tenant = $1
          ORDER BY sr_id, vertragsbeginn DESC",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

// ── Rechnungsempfänger (BG-7 buyer) ───────────────────────────────────────────

/// The BG-7 BUYER terms an EN 16931 invoice needs, resolved for one MaLo.
///
/// `billingd` bills a MaLo and has no customer master of its own, so without
/// this it synthesises a buyer from the MaLo-ID alone — which costs four fatal
/// XRechnung findings (BR-DE-8 city, BR-DE-9 post code, BR-DE-15 buyer
/// reference, PEPPOL-EN16931-R010 electronic address). The Kunde behind the
/// contract carries the real terms; this is the projection of it that billing
/// is allowed to see.
///
/// Deliberately *not* the whole `KundeRow`: an invoice needs a name, a postal
/// address and a VAT-ID, and nothing about payment details or portal identities.
#[derive(Debug, Clone, Serialize)]
pub struct RechnungsempfaengerRow {
    /// BT-44 buyer name.
    pub name: Option<String>,
    /// BT-50 address line (Straße + Hausnummer).
    pub line1: Option<String>,
    /// BT-53 post code.
    pub post_code: Option<String>,
    /// BT-52 city.
    pub city: Option<String>,
    /// BT-55 country code; BO4E `landescode`, defaulted to `DE` by the caller.
    pub country: Option<String>,
    /// BT-48 buyer VAT identifier — B2B only, `NULL` for a household.
    pub vat_id: Option<String>,
    /// § 13b Abs. 2 Nr. 5 lit. b UStG — the buyer is a Stromwiederverkäufer;
    /// billingd derives reverse charge (net invoice, `AE` breakdown) from it.
    pub stromwiederverkaeufer: bool,
}

/// Read `geschaeftspartner->>'name1'`-style fields out of the BO4E JSONB.
///
/// The column holds a BO4E `Geschaeftspartner`, whose address is a nested
/// `Adresse` (`strasse`/`hausnummer`/`postleitzahl`/`ort`/`landescode`). Read
/// defensively — the column is nullable and operator-populated, and a partly
/// filled address must yield partly filled terms rather than an error.
fn buyer_from_geschaeftspartner(
    gp: Option<&serde_json::Value>,
    vat_id: Option<String>,
    stromwiederverkaeufer: bool,
) -> RechnungsempfaengerRow {
    let s = |v: Option<&serde_json::Value>, k: &str| {
        v.and_then(|v| v.get(k))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let adresse = gp.and_then(|g| g.get("adresse"));
    let line1 = match (s(adresse, "strasse"), s(adresse, "hausnummer")) {
        (Some(st), Some(nr)) => Some(format!("{st} {nr}")),
        (Some(st), None) => Some(st),
        (None, nr) => nr,
    };
    RechnungsempfaengerRow {
        // BO4E carries a company name in `name1`; a natural person's name is
        // assembled by vertragd's own portal layer, so `name1` is the one field
        // that is populated for both.
        name: s(gp, "name1"),
        line1,
        post_code: s(adresse, "postleitzahl"),
        city: s(adresse, "ort"),
        country: s(adresse, "landescode"),
        vat_id,
        stromwiederverkaeufer,
    }
}

/// The BG-7 buyer behind `malo_id`, or `None` when no contract/Kunde is on file.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_rechnungsempfaenger_by_malo(
    pool: &PgPool,
    malo_id: &str,
    tenant: &str,
) -> Result<Option<RechnungsempfaengerRow>> {
    let row: Option<(Option<serde_json::Value>, Option<String>, bool)> = sqlx::query_as(
        "SELECT ku.geschaeftspartner, ku.umsatzsteuer_id, ku.stromwiederverkaeufer
           FROM versorgungsvertraege v
           JOIN vertragskomponenten k ON k.vertrag_id = v.id
           JOIN kunden ku            ON ku.id = v.kunden_id
          WHERE k.malo_id=$1 AND v.tenant=$2
            AND v.status IN ('TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
            AND k.status IN ('AKTIV','BESTAETIGT')
          ORDER BY v.vertragsbeginn DESC LIMIT 1",
    )
    .bind(malo_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(gp, vat, wv)| buyer_from_geschaeftspartner(gp.as_ref(), vat, wv)))
}

/// The BG-7 buyer behind a Rahmenvertrag — the holder a Sammelrechnung bills.
///
/// A Sammelrechnung bundles many supply sites onto one document addressed to the
/// **framework-contract holder**, not to any one site's customer, so the
/// per-MaLo projection would name the wrong party.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_rechnungsempfaenger_by_rahmenvertrag(
    pool: &PgPool,
    rahmenvertrag_id: Uuid,
    tenant: &str,
) -> Result<Option<RechnungsempfaengerRow>> {
    let row: Option<(Option<serde_json::Value>, Option<String>, bool)> = sqlx::query_as(
        "SELECT ku.geschaeftspartner, ku.umsatzsteuer_id, ku.stromwiederverkaeufer
           FROM rahmenvertraege r
           JOIN kunden ku ON ku.id = r.kunden_id
          WHERE r.id=$1 AND r.tenant=$2",
    )
    .bind(rahmenvertrag_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(gp, vat, wv)| buyer_from_geschaeftspartner(gp.as_ref(), vat, wv)))
}

// ── GGV-Betreiber (§ 42b EnWG) ────────────────────────────────────────────────

/// Point `(tenant, ggv_id)` at the Kunde who operates the community.
///
/// `false` when the Kunde does not exist for this tenant — checked explicitly
/// rather than left to the foreign key, so the handler can answer 404 with the
/// actual problem instead of relaying a constraint violation as a 500.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn upsert_ggv_betreiber(
    pool: &PgPool,
    tenant: &str,
    ggv_id: &str,
    kunden_id: Uuid,
) -> Result<bool> {
    let kunde_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM kunden WHERE id=$1 AND tenant=$2")
            .bind(kunden_id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?;
    if kunde_exists.is_none() {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO ggv_betreiber (tenant, ggv_id, kunden_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (tenant, ggv_id) DO UPDATE
           SET kunden_id = EXCLUDED.kunden_id, updated_at = now()",
    )
    .bind(tenant)
    .bind(ggv_id)
    .bind(kunden_id)
    .execute(pool)
    .await?;
    Ok(true)
}

/// The BG-7 buyer behind a GGV — the § 42b operator the bundled Sammelrechnung
/// bills.
///
/// Same projection as the Rahmenvertrag holder: the per-Teilnehmer documents
/// resolve their buyers per MaLo, but the bundle addresses the community's
/// operator, who is a Kunde here and nowhere else.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_rechnungsempfaenger_by_ggv(
    pool: &PgPool,
    ggv_id: &str,
    tenant: &str,
) -> Result<Option<RechnungsempfaengerRow>> {
    let row: Option<(Option<serde_json::Value>, Option<String>, bool)> = sqlx::query_as(
        "SELECT ku.geschaeftspartner, ku.umsatzsteuer_id, ku.stromwiederverkaeufer
           FROM ggv_betreiber g
           JOIN kunden ku ON ku.id = g.kunden_id
          WHERE g.ggv_id=$1 AND g.tenant=$2",
    )
    .bind(ggv_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(gp, vat, wv)| buyer_from_geschaeftspartner(gp.as_ref(), vat, wv)))
}
