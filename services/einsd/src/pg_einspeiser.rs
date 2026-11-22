//! The Einspeiser — the Anlagenbetreiber behind one or more plants.
//!
//! § 7 Abs. 1 EEG 2023 („Gesetzliches Schuldverhältnis") forbids the
//! Netzbetreiber from making its EEG obligations conditional on a contract, so
//! what a settlement needs is a **party record**, not a Vertrag. This is that
//! record, and it holds exactly the facts that belong to the person rather than
//! to any one installation: the § 19 UStG election that decides the VAT on
//! every Gutschrift issued to them, and the account those Gutschriften are paid
//! into — with the Zahlungsempfänger's postal address, the `Cdtr/PstlAdr` of
//! accountingd's pain.001.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};

/// The Zahlungsempfänger's postal address, in the structured ISO 20022
/// `PstlAdr` form accountingd's pain.001 emits as `Cdtr/PstlAdr`.
///
/// The field names are accountingd's `sepa::AddressParts`, which reads this
/// object from the settlement event unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Anschrift {
    /// `TwnNm` — required whenever an address is given.
    #[serde(default)]
    pub town: Option<String>,
    /// `Ctry` — ISO 3166-1 alpha-2, required whenever an address is given.
    #[serde(default)]
    pub country: Option<String>,
    /// `StrtNm`.
    #[serde(default)]
    pub street: Option<String>,
    /// `BldgNb`.
    #[serde(default)]
    pub building_number: Option<String>,
    /// `PstCd`.
    #[serde(default)]
    pub post_code: Option<String>,
    /// `CtrySubDvsn` (Bundesland).
    #[serde(default)]
    pub country_subdivision: Option<String>,
}

/// Why an [`Anschrift`] was refused at intake.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnschriftFehler {
    /// Town or country is missing — every structured `PstlAdr` carries both.
    #[error(
        "zahlungsempfaenger_anschrift: town and country are both required \
         (ISO 20022 structured address)"
    )]
    Unvollstaendig,
    /// The country is not an ISO 3166-1 alpha-2 country.
    #[error("zahlungsempfaenger_anschrift.country {0:?} is not an ISO 3166-1 alpha-2 country")]
    Land(String),
}

impl AnschriftFehler {
    /// The stable `code` the REST refusal carries.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unvollstaendig => "incomplete_postal_address",
            Self::Land(_) => "invalid_country_code",
        }
    }
}

impl Anschrift {
    /// The address as stored: blank parts dropped and the country upper-cased,
    /// checked by the `sepa` crate's own `PstlAdr` rules — the ones the payout
    /// applies — so an address einsd accepts is one the pain.001 can carry.
    ///
    /// # Errors
    ///
    /// [`AnschriftFehler::Unvollstaendig`] without both town and country;
    /// [`AnschriftFehler::Land`] for a country that is not one.
    pub fn geprueft(&self) -> Result<Self, AnschriftFehler> {
        let teil = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        };
        let (Some(town), Some(country)) = (teil(&self.town), teil(&self.country)) else {
            return Err(AnschriftFehler::Unvollstaendig);
        };
        // `town` is non-blank, so the only refusal left is the country.
        let pstl_adr = sepa::PostalAddress::new(town.clone(), &country)
            .map_err(|_| AnschriftFehler::Land(country.clone()))?;
        Ok(Self {
            town: Some(town),
            country: Some(pstl_adr.country().to_owned()),
            street: teil(&self.street),
            building_number: teil(&self.building_number),
            post_code: teil(&self.post_code),
            country_subdivision: teil(&self.country_subdivision),
        })
    }
}

/// A plant operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// The Zahlungsempfänger's postal address (`Cdtr/PstlAdr` of the payout);
    /// `None` where none is registered.
    pub zahlungsempfaenger_anschrift: Option<Anschrift>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for Einspeiser {
    fn from_row(r: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let anschrift = Anschrift {
            town: r.try_get("addr_town")?,
            country: r.try_get("addr_country")?,
            street: r.try_get("addr_street")?,
            building_number: r.try_get("addr_building_number")?,
            post_code: r.try_get("addr_post_code")?,
            country_subdivision: r.try_get("addr_country_subdivision")?,
        };
        Ok(Self {
            einspeiser_id: r.try_get("einspeiser_id")?,
            name: r.try_get("name")?,
            mastr_akteur_id: r.try_get("mastr_akteur_id")?,
            ust_status: r.try_get("ust_status")?,
            bank_iban: r.try_get("bank_iban")?,
            bank_bic: r.try_get("bank_bic")?,
            zahlungsempfaenger: r.try_get("zahlungsempfaenger")?,
            // The schema stores town and country together or not at all.
            zahlungsempfaenger_anschrift: anschrift.town.is_some().then_some(anschrift),
        })
    }
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
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
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
    /// The Zahlungsempfänger's postal address. Given, it replaces the stored
    /// one whole — town and country required — and omitted keeps it.
    #[serde(default)]
    pub zahlungsempfaenger_anschrift: Option<Anschrift>,
}

impl UpsertEinspeiser {
    /// The address to store, checked; `None` where the body gives none.
    ///
    /// # Errors
    ///
    /// The [`AnschriftFehler`] of a given address that is incomplete or names
    /// no country.
    pub fn anschrift(&self) -> Result<Option<Anschrift>, AnschriftFehler> {
        self.zahlungsempfaenger_anschrift
            .as_ref()
            .map(Anschrift::geprueft)
            .transpose()
    }
}

/// The projection every read shares. Qualified with `e.` so the joined read in
/// [`find_for_anlage`] can reuse it verbatim.
const COLS: &str = "e.einspeiser_id, e.name, e.mastr_akteur_id, e.ust_status,
     e.bank_iban, e.bank_bic, e.zahlungsempfaenger,
     e.addr_town, e.addr_country, e.addr_street, e.addr_building_number,
     e.addr_post_code, e.addr_country_subdivision";

/// Look an operator up.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find(pool: &PgPool, tenant: &str, einspeiser_id: &str) -> Result<Option<Einspeiser>> {
    let sql =
        format!("SELECT {COLS} FROM einspeiser e WHERE e.tenant = $1 AND e.einspeiser_id = $2");
    Ok(
        sqlx::query_as::<_, Einspeiser>(mako_service::sql::dynamic(sql))
            .bind(tenant)
            .bind(einspeiser_id)
            .fetch_optional(pool)
            .await?,
    )
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
    Ok(
        sqlx::query_as::<_, Einspeiser>(mako_service::sql::dynamic(sql))
            .bind(tenant)
            .bind(tr_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// List every operator in the tenant.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list(pool: &PgPool, tenant: &str) -> Result<Vec<Einspeiser>> {
    let sql = format!("SELECT {COLS} FROM einspeiser e WHERE e.tenant = $1 ORDER BY e.name");
    Ok(
        sqlx::query_as::<_, Einspeiser>(mako_service::sql::dynamic(sql))
            .bind(tenant)
            .fetch_all(pool)
            .await?,
    )
}

/// Create or replace an operator.
///
/// A `ust_status` change takes effect for **every** plant this operator holds
/// on the next settlement — which is the point of the table. It does not
/// rewrite Gutschriften already issued: those are Buchungsbelege under § 147
/// AO, and a past period is corrected by a Storno, never by an update.
///
/// `anschrift` is the address [`UpsertEinspeiser::anschrift`] checked: `Some`
/// replaces the stored address whole, so no part of an earlier one survives
/// beside it; `None` keeps it.
///
/// # Errors
///
/// Propagates storage errors, including the `ust_status` CHECK constraint.
pub async fn upsert(
    pool: &PgPool,
    tenant: &str,
    einspeiser_id: &str,
    input: &UpsertEinspeiser,
    anschrift: Option<&Anschrift>,
) -> Result<()> {
    let a = anschrift.cloned().unwrap_or_default();
    sqlx::query(
        r"INSERT INTO einspeiser
              (einspeiser_id, tenant, name, mastr_akteur_id, ust_status,
               bank_iban, bank_bic, zahlungsempfaenger,
               addr_town, addr_country, addr_street, addr_building_number,
               addr_post_code, addr_country_subdivision)
          VALUES ($1, $2, $3, $4, COALESCE($5, 'REGELBESTEUERUNG'), $6, $7, $8,
                  $10, $11, $12, $13, $14, $15)
          ON CONFLICT (einspeiser_id, tenant) DO UPDATE
          SET name               = EXCLUDED.name,
              mastr_akteur_id    = COALESCE(EXCLUDED.mastr_akteur_id, einspeiser.mastr_akteur_id),
              ust_status         = COALESCE($5, einspeiser.ust_status),
              bank_iban          = COALESCE(EXCLUDED.bank_iban, einspeiser.bank_iban),
              bank_bic           = COALESCE(EXCLUDED.bank_bic, einspeiser.bank_bic),
              zahlungsempfaenger = COALESCE(EXCLUDED.zahlungsempfaenger,
                                            einspeiser.zahlungsempfaenger),
              addr_town          = CASE WHEN $9 THEN EXCLUDED.addr_town
                                        ELSE einspeiser.addr_town END,
              addr_country       = CASE WHEN $9 THEN EXCLUDED.addr_country
                                        ELSE einspeiser.addr_country END,
              addr_street        = CASE WHEN $9 THEN EXCLUDED.addr_street
                                        ELSE einspeiser.addr_street END,
              addr_building_number = CASE WHEN $9 THEN EXCLUDED.addr_building_number
                                          ELSE einspeiser.addr_building_number END,
              addr_post_code     = CASE WHEN $9 THEN EXCLUDED.addr_post_code
                                        ELSE einspeiser.addr_post_code END,
              addr_country_subdivision = CASE WHEN $9 THEN EXCLUDED.addr_country_subdivision
                                              ELSE einspeiser.addr_country_subdivision END,
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
    .bind(anschrift.is_some())
    .bind(&a.town)
    .bind(&a.country)
    .bind(&a.street)
    .bind(&a.building_number)
    .bind(&a.post_code)
    .bind(&a.country_subdivision)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Anschrift, AnschriftFehler, UpsertEinspeiser};

    fn body(anschrift: serde_json::Value) -> UpsertEinspeiser {
        serde_json::from_value(serde_json::json!({
            "name": "Testbetreiber",
            "zahlungsempfaenger_anschrift": anschrift,
        }))
        .expect("deserialises")
    }

    #[test]
    fn a_given_address_needs_town_and_country() {
        for a in [
            serde_json::json!({ "street": "Sonnenweg", "town": "München" }),
            serde_json::json!({ "country": "DE" }),
            serde_json::json!({ "town": " ", "country": "DE" }),
        ] {
            assert_eq!(
                body(a.clone()).anschrift(),
                Err(AnschriftFehler::Unvollstaendig),
                "{a}"
            );
        }
        let e = body(serde_json::json!({ "town": "Atlantis", "country": "ZZ" }))
            .anschrift()
            .expect_err("ZZ is no country");
        assert_eq!(e.code(), "invalid_country_code");
        assert_eq!(
            AnschriftFehler::Unvollstaendig.code(),
            "incomplete_postal_address"
        );
    }

    #[test]
    fn a_stored_address_is_normalised() {
        let a = body(serde_json::json!({
            "town": " München ", "country": "de", "street": "", "post_code": "80331",
        }))
        .anschrift()
        .expect("valid");
        assert_eq!(
            a,
            Some(Anschrift {
                town: Some("München".into()),
                country: Some("DE".into()),
                street: None,
                building_number: None,
                post_code: Some("80331".into()),
                country_subdivision: None,
            })
        );
        let none: UpsertEinspeiser =
            serde_json::from_value(serde_json::json!({ "name": "x" })).expect("deserialises");
        assert_eq!(none.anschrift(), Ok(None));
    }

    #[test]
    fn an_unknown_address_field_is_refused() {
        let err = serde_json::from_value::<UpsertEinspeiser>(serde_json::json!({
            "name": "x",
            "zahlungsempfaenger_anschrift": { "town": "Berlin", "country": "DE", "city": "x" },
        }))
        .expect_err("`city` is no field");
        assert!(err.to_string().contains("unknown field"), "{err}");
    }
}
