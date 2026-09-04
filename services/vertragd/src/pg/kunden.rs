//! Kunden, portal identities, and the projections other services read.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use super::{KundeRow, KundenIdentitaetRow};

// ── Input types ───────────────────────────────────────────────────────────────

/// Create or replace a Kunde (legal entity).
///
/// `oidc_sub` / `email` — when supplied, the first KundenIdentitaet is created
/// alongside. For B2B customers with several portal users, POST
/// `/kunden/{id}/identitaeten` per user afterwards.
#[derive(Debug, Deserialize)]
pub struct CreateKundeInput {
    pub kunden_nr: Option<String>,
    /// Primary portal user OIDC sub — creates a KundenIdentitaet automatically.
    pub oidc_sub: Option<String>,
    pub email: Option<String>,
    pub kundentyp: String, // B2C | B2B_SLP | B2B_RLM | B2B_HV
    /// § 3 Nr. 57 EnWG. Defaults to `kundentyp == "B2C"` when omitted — but a
    /// commercial customer consuming ≤ 10 000 kWh a year is one too, and the
    /// operator says so here rather than having it guessed from the segment.
    pub haushaltskunde: Option<bool>,
    pub geschaeftspartner: Option<serde_json::Value>,
    pub organisations_id: Option<String>,
    pub umsatzsteuer_id: Option<String>,
    pub zahlungsziel_tage: Option<i32>,
    pub sepa_erlaubt: Option<bool>,
    pub erp_kunde_id: Option<String>,
    /// § 13b Abs. 2 Nr. 5 lit. b UStG Stromwiederverkäufer flag (USt 1 TH on
    /// file). Defaults to `false`; billingd derives reverse charge from it.
    pub stromwiederverkaeufer: Option<bool>,
    pub notizen: Option<String>,
}

/// Partial update of a Kunde. Absent fields keep their stored value.
#[derive(Debug, Deserialize)]
pub struct UpdateKundeInput {
    pub kunden_nr: Option<String>,
    pub geschaeftspartner: Option<serde_json::Value>,
    pub organisations_id: Option<String>,
    pub umsatzsteuer_id: Option<String>,
    pub zahlungsziel_tage: Option<i32>,
    pub sepa_erlaubt: Option<bool>,
    /// § 3 Nr. 57 EnWG — a customer's consumption changes, and with it the
    /// notice periods that apply, so this must be correctable after creation.
    pub haushaltskunde: Option<bool>,
    /// § 13b Abs. 2 Nr. 5 lit. b UStG — a USt 1 TH certificate arrives (or
    /// lapses) during the relationship, so this must be correctable too;
    /// leaving it create-only silently froze the reverse-charge decision.
    pub stromwiederverkaeufer: Option<bool>,
    pub notizen: Option<String>,
}

/// Add or update a portal user identity for a Kunde.
/// Idempotent on `oidc_sub`: re-POST updates rolle / standort_filter.
#[derive(Debug, Deserialize)]
pub struct UpsertIdentitaetInput {
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub rolle: Option<String>, // default: VOLLZUGRIFF
    pub standort_filter: Option<String>,
}

/// Lightweight Kunde row for the operator list view (no JSONB blobs).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KundeListRow {
    pub id: Uuid,
    pub tenant: String,
    pub kunden_nr: Option<String>,
    pub kundentyp: String,
    pub haushaltskunde: bool,
    pub organisations_id: Option<String>,
    pub erp_kunde_id: Option<String>,
    pub zahlungsziel_tage: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

// ── Kunde CRUD ────────────────────────────────────────────────────────────────

/// Create a Kunde, or update the one already carrying `erp_kunde_id`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn upsert_kunde(pool: &PgPool, tenant: &str, input: &CreateKundeInput) -> Result<Uuid> {
    let mut tx = pool.begin().await?;
    let id = Uuid::new_v4();
    let haushaltskunde = input.haushaltskunde.unwrap_or(input.kundentyp == "B2C");
    // RETURNING id so an ON CONFLICT resolves to the *existing* row's id, not
    // the freshly generated UUID that was never inserted. The WHERE clause
    // matches the partial unique index `kunden_erp_unique`.
    let row = sqlx::query(
        "INSERT INTO kunden
         (id,tenant,kunden_nr,kundentyp,haushaltskunde,geschaeftspartner,
          organisations_id,umsatzsteuer_id,zahlungsziel_tage,sepa_erlaubt,erp_kunde_id,
          stromwiederverkaeufer,notizen)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
         ON CONFLICT (tenant, erp_kunde_id) WHERE erp_kunde_id IS NOT NULL DO UPDATE
           SET kunden_nr             = COALESCE(EXCLUDED.kunden_nr, kunden.kunden_nr),
               kundentyp             = EXCLUDED.kundentyp,
               haushaltskunde        = EXCLUDED.haushaltskunde,
               geschaeftspartner     = COALESCE(EXCLUDED.geschaeftspartner, kunden.geschaeftspartner),
               organisations_id      = COALESCE(EXCLUDED.organisations_id, kunden.organisations_id),
               umsatzsteuer_id       = COALESCE(EXCLUDED.umsatzsteuer_id, kunden.umsatzsteuer_id),
               zahlungsziel_tage     = EXCLUDED.zahlungsziel_tage,
               sepa_erlaubt          = EXCLUDED.sepa_erlaubt,
               stromwiederverkaeufer = EXCLUDED.stromwiederverkaeufer,
               notizen               = COALESCE(EXCLUDED.notizen, kunden.notizen),
               updated_at            = now()
         RETURNING id",
    )
    .bind(id)
    .bind(tenant)
    .bind(&input.kunden_nr)
    .bind(&input.kundentyp)
    .bind(haushaltskunde)
    .bind(&input.geschaeftspartner)
    .bind(&input.organisations_id)
    .bind(&input.umsatzsteuer_id)
    .bind(input.zahlungsziel_tage.unwrap_or(14))
    .bind(input.sepa_erlaubt.unwrap_or(true))
    .bind(&input.erp_kunde_id)
    .bind(input.stromwiederverkaeufer.unwrap_or(false))
    .bind(&input.notizen)
    .fetch_one(&mut *tx)
    .await?;
    let actual_id: Uuid = row.try_get("id")?;

    // The primary identity belongs to the same unit of work: a Kunde that
    // exists without the login it was created with is a customer who cannot
    // reach the portal and an operator with no error to act on.
    if let Some(ref sub) = input.oidc_sub {
        let identity = UpsertIdentitaetInput {
            oidc_sub: sub.clone(),
            email: input.email.clone(),
            display_name: None,
            rolle: None,
            standort_filter: None,
        };
        upsert_identitaet_tx(&mut tx, actual_id, tenant, &identity).await?;
    }
    tx.commit().await?;
    Ok(actual_id)
}

/// Apply a partial update. `NULL` fields are left untouched.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn update_kunde(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    input: &UpdateKundeInput,
) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE kunden SET
         kunden_nr             = COALESCE($3, kunden_nr),
         geschaeftspartner     = COALESCE($4, geschaeftspartner),
         organisations_id      = COALESCE($5, organisations_id),
         umsatzsteuer_id       = COALESCE($6, umsatzsteuer_id),
         zahlungsziel_tage     = COALESCE($7, zahlungsziel_tage),
         sepa_erlaubt          = COALESCE($8, sepa_erlaubt),
         haushaltskunde        = COALESCE($9, haushaltskunde),
         stromwiederverkaeufer = COALESCE($10, stromwiederverkaeufer),
         notizen               = COALESCE($11, notizen),
         updated_at            = now()
         WHERE id=$1 AND tenant=$2",
    )
    .bind(id)
    .bind(tenant)
    .bind(&input.kunden_nr)
    .bind(&input.geschaeftspartner)
    .bind(&input.organisations_id)
    .bind(&input.umsatzsteuer_id)
    .bind(input.zahlungsziel_tage)
    .bind(input.sepa_erlaubt)
    .bind(input.haushaltskunde)
    .bind(input.stromwiederverkaeufer)
    .bind(&input.notizen)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// # Errors
///
/// Propagates storage errors.
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
///
/// Joins through `kunden_identitaeten` so that B2B users (one company, N
/// logins) all map to the same Kunde.
///
/// # Errors
///
/// Propagates storage errors.
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

/// List Kunden for the operator view.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_kunden(
    pool: &PgPool,
    tenant: &str,
    kundentyp: Option<&str>,
    limit: i64,
) -> Result<Vec<KundeListRow>> {
    Ok(sqlx::query_as::<_, KundeListRow>(
        r"SELECT id, tenant, kunden_nr, kundentyp, haushaltskunde, organisations_id,
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

// ── Person / Zahlungsinformation sub-objects ─────────────────────────────────

/// Store a canonical BO4E `Person` for a Kunde. `false` when no such Kunde.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn upsert_person(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    person: serde_json::Value,
) -> Result<bool> {
    let n = sqlx::query("UPDATE kunden SET person=$3, updated_at=now() WHERE id=$1 AND tenant=$2")
        .bind(kunden_id)
        .bind(tenant)
        .bind(&person)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_person(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Option<serde_json::Value>> {
    Ok(sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT person FROM kunden WHERE id=$1 AND tenant=$2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// Store a canonical BO4E `Zahlungsinformation`. `false` when no such Kunde.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn upsert_zahlungsinformation(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    zahlungsinformation: &serde_json::Value,
) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE kunden SET zahlungsinformation=$3, updated_at=now() WHERE id=$1 AND tenant=$2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .bind(zahlungsinformation)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_zahlungsinformation(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Option<serde_json::Value>> {
    Ok(sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT zahlungsinformation FROM kunden WHERE id=$1 AND tenant=$2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?
    .flatten())
}

// ── Portal identities ─────────────────────────────────────────────────────────

/// Upsert a portal identity. Idempotent on `oidc_sub`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn upsert_identitaet(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    input: &UpsertIdentitaetInput,
) -> Result<Uuid> {
    let mut conn = pool.acquire().await?;
    upsert_identitaet_tx(&mut conn, kunden_id, tenant, input).await
}

pub(crate) async fn upsert_identitaet_tx(
    conn: &mut sqlx::PgConnection,
    kunden_id: Uuid,
    tenant: &str,
    input: &UpsertIdentitaetInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let rolle = input.rolle.as_deref().unwrap_or("VOLLZUGRIFF");
    let row = sqlx::query(
        "INSERT INTO kunden_identitaeten
         (id, kunden_id, tenant, oidc_sub, email, display_name, rolle, standort_filter)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (tenant, oidc_sub) DO UPDATE
           SET email           = COALESCE(EXCLUDED.email, kunden_identitaeten.email),
               display_name    = COALESCE(EXCLUDED.display_name, kunden_identitaeten.display_name),
               rolle           = EXCLUDED.rolle,
               standort_filter = EXCLUDED.standort_filter,
               aktiv           = true,
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
    .fetch_one(&mut *conn)
    .await?;
    Ok(row.try_get("id")?)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_identitaeten(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<KundenIdentitaetRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM kunden_identitaeten
         WHERE kunden_id=$1 AND tenant=$2 AND aktiv=true ORDER BY created_at",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

/// Resolve an OIDC sub to its identity row (rolle / standort scope).
///
/// # Errors
///
/// Propagates storage errors.
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

/// Deactivate a portal identity by OIDC sub. `false` when none was active.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn deactivate_identitaet_by_sub(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    oidc_sub: &str,
) -> Result<bool> {
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

/// Stamp `letzter_login` after a successful portal authorization.
///
/// # Errors
///
/// Propagates storage errors.
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

/// Count active identities — enforces `max_identitaeten_per_kunde`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn count_active_identitaeten(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM kunden_identitaeten
         WHERE kunden_id = $1 AND tenant = $2 AND aktiv = true",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_one(pool)
    .await?)
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
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM kunden WHERE id=$1 AND tenant=$2)")
            .bind(kunden_id)
            .bind(tenant)
            .fetch_one(pool)
            .await?;
    if !exists {
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

// ── Rechnungsempfänger (BG-7 buyer) ───────────────────────────────────────────

/// The BG-7 BUYER terms an EN 16931 invoice needs.
///
/// `billingd` bills a MaLo and has no customer master of its own, so without
/// this it synthesises a buyer from the MaLo-ID alone — which costs four fatal
/// XRechnung findings (BR-DE-8 city, BR-DE-9 post code, BR-DE-15 buyer
/// reference, PEPPOL-EN16931-R010 electronic address). The Kunde behind the
/// contract carries the real terms; this is the projection of it that billing
/// is allowed to see.
///
/// Deliberately *not* the whole [`KundeRow`]: an invoice needs a name, a postal
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
    /// The customer's e-mail — the bevorzugter `E_MAIL` Kontaktweg, else the
    /// first. No EN 16931 BT carries it; it is here because the party an
    /// invoice is addressed to is the party it is sent to, and resolving those
    /// separately lets a document reach somebody else.
    pub email: Option<String>,
}

/// Read the BG-7 terms out of a BO4E `Geschaeftspartner`.
///
/// Typed, not key-probed: the column is JSONB and BO4E-shaped, so it is read
/// through `rubo4e` and the reader cannot drift from the schema. BO4E models an
/// organisation and a natural person in one object — `organisationsname` for
/// the first, `vorname`/`nachname` for the second, and no `name1` at all.
///
/// A payload that does not deserialise yields empty terms rather than an error:
/// the column is operator-populated and nullable, and a malformed one must not
/// fail the contract lookup § 40 Abs. 1 EnWG needs — the invoice then carries
/// the synthesised buyer and says so.
fn buyer_from_geschaeftspartner(
    gp: Option<&serde_json::Value>,
    vat_id: Option<String>,
    stromwiederverkaeufer: bool,
) -> RechnungsempfaengerRow {
    use rubo4e::current::Geschaeftspartner;

    // Fully typed, including the contact methods.
    //
    // `Kontaktweg.kontaktwert` is a `String` per the schema, so the whole
    // `Geschaeftspartner` — name, address, VAT-ID and contact methods — reads as
    // one typed object and the e-mail comes off it directly.
    let Some(gp) = gp
        .cloned()
        .and_then(|v| serde_json::from_value::<Geschaeftspartner>(v).ok())
    else {
        return RechnungsempfaengerRow {
            name: None,
            line1: None,
            post_code: None,
            city: None,
            country: None,
            vat_id,
            stromwiederverkaeufer,
            email: None,
        };
    };
    let email = email_from_kontaktwege(gp.kontaktwege.as_deref());

    // Organisation or natural person — BO4E models both in one object.
    let person = |gp: &Geschaeftspartner| match (gp.vorname.as_deref(), gp.nachname.as_deref()) {
        (Some(v), Some(n)) => Some(format!("{v} {n}")),
        (None, Some(n)) => Some(n.to_owned()),
        (Some(v), None) => Some(v.to_owned()),
        (None, None) => None,
    };
    let name = gp
        .organisationsname
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| person(&gp));

    let adresse = gp.adresse.as_ref();
    let line1 = match (
        adresse.and_then(|a| a.strasse.clone()),
        adresse.and_then(|a| a.hausnummer.clone()),
    ) {
        (Some(st), Some(nr)) => Some(format!("{st} {nr}")),
        (Some(st), None) => Some(st),
        (None, nr) => nr,
    };

    RechnungsempfaengerRow {
        name,
        line1,
        post_code: adresse.and_then(|a| a.postleitzahl.clone()),
        city: adresse.and_then(|a| a.ort.clone()),
        country: adresse.and_then(|a| a.landescode.map(|c| c.to_string())),
        vat_id: vat_id.or_else(|| gp.umsatzsteuer_id.clone()),
        stromwiederverkaeufer,
        email,
    }
}

/// The customer's e-mail out of a BO4E `kontaktwege` list: the bevorzugter
/// `E_MAIL` entry, else the first `E_MAIL`.
///
/// `kontaktwert` is a `String`, so the whole read is typed.
fn email_from_kontaktwege(kontaktwege: Option<&[rubo4e::current::Kontaktweg]>) -> Option<String> {
    use rubo4e::current::Kontaktart;

    let entries = kontaktwege?;
    let is_mail = |k: &&rubo4e::current::Kontaktweg| k.kontaktart == Some(Kontaktart::EMail);
    let value = |k: &rubo4e::current::Kontaktweg| {
        k.kontaktwert
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    entries
        .iter()
        .find(|k| is_mail(k) && k.ist_bevorzugter_kontaktweg == Some(true))
        .and_then(value)
        .or_else(|| entries.iter().find(is_mail).and_then(value))
}

type BuyerCols = (Option<serde_json::Value>, Option<String>, bool);

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
    let row: Option<BuyerCols> = sqlx::query_as(
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
    let row: Option<BuyerCols> = sqlx::query_as(
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
    let row: Option<BuyerCols> = sqlx::query_as(
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

#[cfg(test)]
mod buyer_tests {
    use super::{buyer_from_geschaeftspartner, email_from_kontaktwege};

    /// The projection reads BO4E's own field names. A reader probing for a key
    /// BO4E does not define leaves BT-44 empty on every invoice addressed to a
    /// correctly-formed `Geschaeftspartner`.
    #[test]
    fn an_organisation_is_named_by_its_bo4e_field() {
        let gp = serde_json::json!({
            "_typ": "GESCHAEFTSPARTNER",
            "organisationsname": "Stadtwerke Musterstadt GmbH",
            "adresse": {
                "strasse": "Musterstraße",
                "hausnummer": "1",
                "postleitzahl": "12345",
                "ort": "Musterstadt",
                "landescode": "DE"
            },
            "umsatzsteuerId": "DE123456789"
        });
        let b = buyer_from_geschaeftspartner(Some(&gp), None, false);
        assert_eq!(b.name.as_deref(), Some("Stadtwerke Musterstadt GmbH"));
        assert_eq!(b.line1.as_deref(), Some("Musterstraße 1"));
        assert_eq!(b.post_code.as_deref(), Some("12345"));
        assert_eq!(b.city.as_deref(), Some("Musterstadt"));
        assert_eq!(b.country.as_deref(), Some("DE"));
        assert_eq!(b.vat_id.as_deref(), Some("DE123456789"));
    }

    /// A natural person is named by `vorname`/`nachname`, which is how BO4E
    /// models the other half of one object.
    #[test]
    fn a_household_customer_is_named_by_their_person_fields() {
        let gp = serde_json::json!({
            "_typ": "GESCHAEFTSPARTNER",
            "vorname": "Erika",
            "nachname": "Mustermann"
        });
        let b = buyer_from_geschaeftspartner(Some(&gp), None, false);
        assert_eq!(b.name.as_deref(), Some("Erika Mustermann"));
    }

    /// A contact method must not cost the buyer their name: `rubo4e` types
    /// `kontaktwert` as a `Decimal`, so an object carrying an e-mail address
    /// fails to deserialise whole unless the array is lifted out first.
    #[test]
    fn an_email_contact_does_not_break_the_typed_read() {
        let gp = serde_json::json!({
            "_typ": "GESCHAEFTSPARTNER",
            "organisationsname": "Beispiel AG",
            "kontaktwege": [
                { "_typ": "KONTAKTWEG", "kontaktart": "TELEFON", "kontaktwert": "+49 30 123456" },
                { "_typ": "KONTAKTWEG", "kontaktart": "E_MAIL",
                  "kontaktwert": "rechnung@beispiel.test",
                  "istBevorzugterKontaktweg": true }
            ]
        });
        let b = buyer_from_geschaeftspartner(Some(&gp), None, false);
        assert_eq!(b.name.as_deref(), Some("Beispiel AG"));
        assert_eq!(b.email.as_deref(), Some("rechnung@beispiel.test"));
    }

    /// The bevorzugter address wins; otherwise the first e-mail does.
    ///
    /// Built from the typed `Kontaktweg`.
    #[test]
    fn the_preferred_email_is_the_one_documents_go_to() {
        use rubo4e::current::{Kontaktart, Kontaktweg};

        let way = |art: Kontaktart, wert: &str, bevorzugt: Option<bool>| Kontaktweg {
            kontaktart: Some(art),
            kontaktwert: Some(wert.to_owned()),
            ist_bevorzugter_kontaktweg: bevorzugt,
            ..Default::default()
        };

        let ways = [
            way(Kontaktart::EMail, "info@beispiel.test", None),
            way(Kontaktart::EMail, "buchhaltung@beispiel.test", Some(true)),
        ];
        assert_eq!(
            email_from_kontaktwege(Some(&ways)).as_deref(),
            Some("buchhaltung@beispiel.test")
        );

        let unranked = [
            way(Kontaktart::Telefon, "+49 30 1", None),
            way(Kontaktart::EMail, "info@beispiel.test", None),
        ];
        assert_eq!(
            email_from_kontaktwege(Some(&unranked)).as_deref(),
            Some("info@beispiel.test")
        );
        assert_eq!(email_from_kontaktwege(None), None);
    }

    /// A payload that does not deserialise yields empty terms, never an error:
    /// the contract lookup § 40 Abs. 1 EnWG needs must not fail because the
    /// master data is half-filled.
    #[test]
    fn an_unusable_payload_yields_empty_terms() {
        let b = buyer_from_geschaeftspartner(Some(&serde_json::json!(42)), None, true);
        assert!(b.name.is_none() && b.line1.is_none());
        assert!(b.stromwiederverkaeufer, "the columns still travel");
    }
}
