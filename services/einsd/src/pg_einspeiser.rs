//! The Einspeiser — the Anlagenbetreiber behind one or more plants.
//!
//! § 7 Abs. 1 EEG 2023 („Gesetzliches Schuldverhältnis") forbids the
//! Netzbetreiber from making its EEG obligations conditional on a contract, so
//! what a settlement needs is a **party record**, not a Vertrag. This is that
//! record, and it holds exactly the facts that belong to the person rather than
//! to any one installation: the § 19 UStG election that decides the VAT on
//! every Gutschrift issued to them, and the account those Gutschriften are paid
//! into.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// A plant operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Einspeiser {
    /// Operator-assigned identity — a customer number, a MaStR Marktakteur-ID,
    /// or a UUID the ERP mints. `einsd` does not invent identities for parties
    /// it did not register.
    pub einspeiser_id: String,
    pub name: String,
    /// MaStR Marktakteursnummer, where the operator has one.
    pub mastr_akteur_id: Option<String>,
    /// `KLEINUNTERNEHMER` (§ 19 UStG, 0 %) or `REGELBESTEUERUNG`
    /// (§ 12 Abs. 1 UStG, 19 %).
    pub ust_status: String,
    pub bank_iban: Option<String>,
    pub bank_bic: Option<String>,
    pub zahlungsempfaenger: Option<String>,
}

impl Einspeiser {
    /// The declared § 19 UStG election as the billing engine's type.
    ///
    /// An unrecognised token is **not** silently defaulted: the CHECK
    /// constraint makes one impossible, and guessing here would put a rate on a
    /// real Gutschrift that the operator never declared.
    ///
    /// # Errors
    ///
    /// When the stored token is neither published value.
    pub fn vat_status(&self) -> Result<eeg_billing::ust::VatStatus> {
        eeg_billing::ust::VatStatus::from_db_str(&self.ust_status).ok_or_else(|| {
            anyhow::anyhow!(
                "Einspeiser {} carries an unknown ust_status {:?}",
                self.einspeiser_id,
                self.ust_status
            )
        })
    }
}

/// Fields accepted when creating or replacing an operator.
#[derive(Debug, Clone, Deserialize)]
pub struct UpsertEinspeiser {
    pub name: String,
    #[serde(default)]
    pub mastr_akteur_id: Option<String>,
    /// Omitted keeps the stored value on an update and defaults to
    /// `REGELBESTEUERUNG` on a create — the § 19 election is a declaration, and
    /// silence is not one.
    #[serde(default)]
    pub ust_status: Option<String>,
    #[serde(default)]
    pub bank_iban: Option<String>,
    #[serde(default)]
    pub bank_bic: Option<String>,
    #[serde(default)]
    pub zahlungsempfaenger: Option<String>,
}

/// The projection every read shares. Qualified with `e.` so the joined read in
/// [`find_for_anlage`] can reuse it verbatim.
const COLS: &str = "e.einspeiser_id, e.name, e.mastr_akteur_id, e.ust_status,
     e.bank_iban, e.bank_bic, e.zahlungsempfaenger";

/// Look an operator up.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find(pool: &PgPool, tenant: &str, einspeiser_id: &str) -> Result<Option<Einspeiser>> {
    let sql =
        format!("SELECT {COLS} FROM einspeiser e WHERE e.tenant = $1 AND e.einspeiser_id = $2");
    Ok(sqlx::query_as::<_, Einspeiser>(&sql)
        .bind(tenant)
        .bind(einspeiser_id)
        .fetch_optional(pool)
        .await?)
}

/// The operator responsible for a plant, resolved through `eeg_anlagen`.
///
/// `eeg_anlagen.einspeiser_id` is `NOT NULL` behind a foreign key, so a plant
/// that exists has an operator. `Ok(None)` means the plant does not exist.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_for_anlage(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
) -> Result<Option<Einspeiser>> {
    let sql = format!(
        "SELECT {COLS} FROM einspeiser e
         JOIN eeg_anlagen a
           ON a.einspeiser_id = e.einspeiser_id AND a.tenant = e.tenant
         WHERE e.tenant = $1 AND a.tr_id = $2"
    );
    Ok(sqlx::query_as::<_, Einspeiser>(&sql)
        .bind(tenant)
        .bind(tr_id)
        .fetch_optional(pool)
        .await?)
}

/// List every operator in the tenant.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list(pool: &PgPool, tenant: &str) -> Result<Vec<Einspeiser>> {
    let sql = format!("SELECT {COLS} FROM einspeiser e WHERE e.tenant = $1 ORDER BY e.name");
    Ok(sqlx::query_as::<_, Einspeiser>(&sql)
        .bind(tenant)
        .fetch_all(pool)
        .await?)
}

/// Create or replace an operator.
///
/// A `ust_status` change takes effect for **every** plant this operator holds
/// on the next settlement — which is the point of the table. It does not
/// rewrite Gutschriften already issued: those are Buchungsbelege under § 147
/// AO, and a past period is corrected by a Storno, never by an update.
///
/// # Errors
///
/// Propagates storage errors, including the `ust_status` CHECK constraint.
pub async fn upsert(
    pool: &PgPool,
    tenant: &str,
    einspeiser_id: &str,
    input: &UpsertEinspeiser,
) -> Result<()> {
    sqlx::query(
        r"INSERT INTO einspeiser
              (einspeiser_id, tenant, name, mastr_akteur_id, ust_status,
               bank_iban, bank_bic, zahlungsempfaenger)
          VALUES ($1, $2, $3, $4, COALESCE($5, 'REGELBESTEUERUNG'), $6, $7, $8)
          ON CONFLICT (einspeiser_id, tenant) DO UPDATE
          SET name               = EXCLUDED.name,
              mastr_akteur_id    = COALESCE(EXCLUDED.mastr_akteur_id, einspeiser.mastr_akteur_id),
              ust_status         = COALESCE($5, einspeiser.ust_status),
              bank_iban          = COALESCE(EXCLUDED.bank_iban, einspeiser.bank_iban),
              bank_bic           = COALESCE(EXCLUDED.bank_bic, einspeiser.bank_bic),
              zahlungsempfaenger = COALESCE(EXCLUDED.zahlungsempfaenger,
                                            einspeiser.zahlungsempfaenger),
              version            = einspeiser.version + 1,
              updated_at         = now()",
    )
    .bind(einspeiser_id)
    .bind(tenant)
    .bind(&input.name)
    .bind(&input.mastr_akteur_id)
    .bind(&input.ust_status)
    .bind(&input.bank_iban)
    .bind(&input.bank_bic)
    .bind(&input.zahlungsempfaenger)
    .execute(pool)
    .await?;
    Ok(())
}
