//! DSGVO Art. 15 access / Art. 20 portability and Art. 17 erasure.

use anyhow::Result;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    KundeRow, KundenIdentitaetRow, VersorgungsvertragRow, VertragskomponenteRow, kunden, vertraege,
};

/// Everything `vertragd` holds about one data subject.
#[derive(Debug, Serialize)]
pub struct GdprExportRow {
    pub kunde: KundeRow,
    pub person: Option<serde_json::Value>,
    pub zahlungsinformation: Option<serde_json::Value>,
    pub identitaeten: Vec<KundenIdentitaetRow>,
    pub vertraege: Vec<VersorgungsvertragRow>,
    pub komponenten: Vec<VertragskomponenteRow>,
}

/// DSGVO Art. 15 / Art. 20 — the complete record for one customer.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn gdpr_export(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Option<GdprExportRow>> {
    let Some(kunde) = kunden::fetch_kunde(pool, kunden_id, tenant).await? else {
        return Ok(None);
    };
    // Every read propagates: an Auskunft that silently omits a category
    // because a query failed is worse than no answer at all.
    let person = kunden::fetch_person(pool, kunden_id, tenant).await?;
    let zahlungsinformation = kunden::fetch_zahlungsinformation(pool, kunden_id, tenant).await?;
    let identitaeten = kunden::list_identitaeten(pool, kunden_id, tenant).await?;
    let vertraege = vertraege::list_vertraege_by_kunde(pool, kunden_id, tenant).await?;
    let mut komponenten = Vec::new();
    for v in &vertraege {
        komponenten.extend(vertraege::list_komponenten(pool, v.id).await?);
    }
    Ok(Some(GdprExportRow {
        kunde,
        person,
        zahlungsinformation,
        identitaeten,
        vertraege,
        komponenten,
    }))
}

/// The outcome of an Art. 17 erasure request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErasureOutcome {
    /// The customer's personal data was pseudonymised.
    Anonymized { fields: Vec<String> },
    /// No such customer in this tenant.
    NotFound,
    /// Erasure does not apply yet, with the reason to give the data subject.
    Refused {
        grund: String,
        laufende_vertraege: Vec<String>,
    },
}

/// The fields this operation overwrites, in the order the audit log records
/// them.
const ANONYMIZED_FIELDS: &[&str] = &[
    "kunden.geschaeftspartner",
    "kunden.person",
    "kunden.zahlungsinformation",
    "kunden.umsatzsteuer_id",
    "kunden.organisations_id",
    "kunden.kunden_nr",
    "kunden.notizen",
    "kunden_identitaeten.oidc_sub",
    "kunden_identitaeten.email",
    "kunden_identitaeten.display_name",
    "versorgungsvertraege.standort_adresse",
    "versorgungsvertraege.notizen",
];

/// DSGVO Art. 17 — pseudonymise every personal datum, keep the commercial
/// record.
///
/// # A running contract is not erasable
///
/// While supply is live the data is needed to perform the contract
/// (Art. 6 Abs. 1 lit. b DSGVO), so Art. 17 Abs. 1 lit. a does not apply and
/// Art. 17 Abs. 3 lit. b keeps what the § 41 EnWG obligations require. The
/// request is therefore refused with the contracts named, rather than silently
/// destroying the master data of a customer still being supplied and invoiced.
/// `force` exists for the cases where erasure applies regardless
/// (Art. 17 Abs. 1 lit. d unlawful processing) and demands a reason on record.
///
/// # What survives
///
/// The contract rows themselves: § 147 Abs. 3 AO keeps Handelsbriefe six years
/// and Buchungsbelege eight, and a contract that grounds bookings is the
/// latter. They are kept without the personal data that identified the party.
///
/// # Errors
///
/// Propagates storage errors. Everything below happens in one transaction, so a
/// half-pseudonymised customer is not a reachable state.
pub async fn anonymize_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    requested_by: &str,
    request_reason: Option<&str>,
    force: bool,
) -> Result<ErasureOutcome> {
    let mut tx = pool.begin().await?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM kunden WHERE id=$1 AND tenant=$2)")
            .bind(kunden_id)
            .bind(tenant)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Ok(ErasureOutcome::NotFound);
    }

    let laufend: Vec<(String,)> = sqlx::query_as(
        "SELECT vertrags_nr FROM versorgungsvertraege
          WHERE kunden_id = $1 AND tenant = $2
            AND status IN ('ANGELEGT','IN_BEARBEITUNG','TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
          ORDER BY vertrags_nr",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(&mut *tx)
    .await?;
    if !laufend.is_empty() && !force {
        return Ok(ErasureOutcome::Refused {
            grund: "Die Daten sind zur Erfüllung laufender Lieferverträge erforderlich \
                    (Art. 6 Abs. 1 lit. b DSGVO); Art. 17 Abs. 1 lit. a DSGVO greift \
                    daher nicht. Nach Vertragsende ist die Löschung möglich."
                .to_owned(),
            laufende_vertraege: laufend.into_iter().map(|(nr,)| nr).collect(),
        });
    }

    // One token identifies this erasure across every table, so the rows stay
    // joinable for the § 147 AO record without naming anybody.
    let anon_token = format!("anon:{}", Uuid::new_v4().simple());

    sqlx::query(
        r"UPDATE kunden
          SET geschaeftspartner  = jsonb_build_object(
                  '_typ', 'GESCHAEFTSPARTNER',
                  'name1', $3::TEXT
              ),
              person             = NULL,
              zahlungsinformation = NULL,
              umsatzsteuer_id    = NULL,
              organisations_id   = NULL,
              kunden_nr          = NULL,
              notizen            = NULL,
              updated_at         = now()
          WHERE id = $1 AND tenant = $2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .bind(&anon_token)
    .execute(&mut *tx)
    .await?;

    // A distinct token per identity. Writing one token into every row of a
    // customer with two portal users violated UNIQUE (tenant, oidc_sub) and
    // failed the whole erasure — for exactly the B2B customers that have more
    // than one login.
    sqlx::query(
        r"UPDATE kunden_identitaeten
          SET oidc_sub     = 'anon:' || replace(gen_random_uuid()::TEXT, '-', ''),
              email        = NULL,
              display_name = NULL,
              aktiv        = FALSE,
              updated_at   = now()
          WHERE kunden_id = $1 AND tenant = $2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;

    // The supply address is personal data too — it says where the person
    // lives. Leaving it behind made the erasure incomplete in the one place a
    // data subject would notice.
    sqlx::query(
        r"UPDATE versorgungsvertraege
          SET standort_adresse = NULL,
              notizen          = NULL,
              updated_at       = now()
          WHERE kunden_id = $1 AND tenant = $2",
    )
    .bind(kunden_id)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r"INSERT INTO anonymization_log
          (tenant, kunden_id, anonymized_fields, requested_by, request_reason, retention_basis)
          VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(tenant)
    .bind(kunden_id)
    .bind(ANONYMIZED_FIELDS)
    .bind(requested_by)
    .bind(request_reason)
    .bind(
        "§ 147 Abs. 3 AO: Handelsbriefe 6 Jahre, Buchungsbelege 8 Jahre — \
         Vertragsdaten bleiben ohne Personenbezug erhalten",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(ErasureOutcome::Anonymized {
        fields: ANONYMIZED_FIELDS.iter().map(|s| (*s).to_owned()).collect(),
    })
}
