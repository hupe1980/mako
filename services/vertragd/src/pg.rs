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
    sqlx::query(
        "INSERT INTO kunden
         (id,tenant,kunden_nr,kundentyp,geschaeftspartner,
          organisations_id,umsatzsteuer_id,zahlungsziel_tage,sepa_erlaubt,erp_kunde_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
         ON CONFLICT (tenant, erp_kunde_id) DO UPDATE
           SET geschaeftspartner=EXCLUDED.geschaeftspartner, updated_at=now()",
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
    .execute(pool)
    .await?;

    // If a primary identity (oidc_sub) was provided at create time, upsert it too.
    if let Some(ref sub) = input.oidc_sub {
        let identity = UpsertIdentitaetInput {
            oidc_sub: sub.clone(),
            email: input.email.clone(),
            display_name: None,
            rolle: None,
            standort_filter: None,
        };
        upsert_identitaet(pool, id, tenant, &identity).await?;
    }

    Ok(id)
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
    sqlx::query(
        "INSERT INTO kunden_identitaeten
         (id, kunden_id, tenant, oidc_sub, email, display_name, rolle, standort_filter)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (tenant, oidc_sub) DO UPDATE
           SET email           = COALESCE(EXCLUDED.email, kunden_identitaeten.email),
               display_name    = COALESCE(EXCLUDED.display_name, kunden_identitaeten.display_name),
               rolle           = EXCLUDED.rolle,
               standort_filter = EXCLUDED.standort_filter,
               updated_at      = now()",
    )
    .bind(id)
    .bind(kunden_id)
    .bind(tenant)
    .bind(&input.oidc_sub)
    .bind(&input.email)
    .bind(&input.display_name)
    .bind(rolle)
    .bind(&input.standort_filter)
    .execute(pool)
    .await?;
    Ok(id)
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
    sqlx::query(
        "INSERT INTO rahmenvertraege
         (id,kunden_id,tenant,rahmenvertrag_nr,gueltig_von,gueltig_bis,
          kuendigungsfrist_monate,auto_renewal,renewal_monate,
          preisanpassungsformel,portfolio_rabatt_prozent,
          rechnungsstellung,sammelrechnung_intervall,erp_rahmenvertrag_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         ON CONFLICT (tenant,erp_rahmenvertrag_id) DO UPDATE SET updated_at=now()",
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
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn insert_versorgungsvertrag(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    lf_mp_id: &str,
    input: &CreateVersorgungsvertragInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO versorgungsvertraege
         (id,kunden_id,rahmenvertrag_id,tenant,kundentyp,bundle_code,
          vertragsbeginn,vertragsende,kuendigungsfrist_monate,
          preisgarantie_bis,auto_renewal,standort_bezeichnung,erp_contract_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
         ON CONFLICT (tenant,erp_contract_id) DO UPDATE SET updated_at=now()",
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
    .execute(pool)
    .await?;
    // Insert components
    for komp in &input.komponenten {
        insert_komponente(pool, id, lf_mp_id, komp).await?;
    }
    Ok(id)
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

pub async fn update_vertrag_status(pool: &PgPool, id: Uuid, status: &str) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege SET status=$1, updated_at=now(),
         completed_at = CASE WHEN $1 IN ('ABGELAUFEN','STORNIERT') THEN now() ELSE completed_at END
         WHERE id=$2",
    )
    .bind(status)
    .bind(id)
    .execute(pool)
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
    sqlx::query(
        "UPDATE vertragskomponenten SET status=$1, updated_at=now(),
         mako_process_id=COALESCE($2,mako_process_id),
         malo_id=COALESCE($3,malo_id),
         abgelehnt_erc=$4, abgelehnt_reason=$5
         WHERE id=$6",
    )
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
    pool: &PgPool,
    komp_id: Uuid,
    new_product_code: &str,
) -> Result<()> {
    sqlx::query("UPDATE vertragskomponenten SET product_code=$1, updated_at=now() WHERE id=$2")
        .bind(new_product_code)
        .bind(komp_id)
        .execute(pool)
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

/// Extract OIDC `sub` from a JWT Bearer token payload (no signature verification).
/// Decodes the middle base64url segment as JSON.
pub fn extract_sub_from_bearer(authorization: &str) -> Option<String> {
    let jwt = authorization.strip_prefix("Bearer ")?;
    let payload_b64 = jwt.split('.').nth(1)?;
    // base64url → standard base64
    let standard: String = payload_b64
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    // Add padding
    let pad = (4 - standard.len() % 4) % 4;
    let padded = format!("{}{}", standard, "=".repeat(pad));
    // Decode
    let bytes = decode_base64(&padded)?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("sub")?.as_str().map(str::to_owned)
}

/// Minimal base64 decoder (no external dep).
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let lookup = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i + 3 < bytes.len() {
        let (a, b, c, d) = (
            lookup(bytes[i])?,
            lookup(bytes[i + 1])?,
            lookup(bytes[i + 2])?,
            lookup(bytes[i + 3])?,
        );
        out.push((a << 2) | (b >> 4));
        if bytes[i + 2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if bytes[i + 3] != b'=' {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    Some(out)
}

// \u2500\u2500 Pending Tarifwechsel (B13 \u2014 \u00a741 Abs. 3 EnWG Preisanpassungsbenachrichtigung) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// A component with a scheduled future Tarifwechsel.
#[derive(Debug, sqlx::FromRow)]
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
    pool: &PgPool,
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
    .execute(pool)
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
/// The window is 1 day wide so the daily background run fires exactly once.
pub async fn find_tarifwechsel_needing_notif(
    pool: &PgPool,
    tenant: &str,
    today: Date,
) -> Result<Vec<PendingTarifwechselRow>> {
    let window_start = today + time::Duration::days(41);
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
            AND k.pending_wirksamkeit >= $2
            AND k.pending_wirksamkeit <  $3
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
    pool: &PgPool,
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
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch the stored `Preisgarantie` BO JSON for a Versorgungsvertrag, or `None` if not set.
pub async fn fetch_preisgarantie(
    pool: &PgPool,
    vertrag_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row = sqlx::query(
        "SELECT preisgarantie FROM versorgungsvertraege WHERE id=$1 AND tenant=$2",
    )
    .bind(vertrag_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.try_get::<serde_json::Value, _>("preisgarantie").ok()))
}
