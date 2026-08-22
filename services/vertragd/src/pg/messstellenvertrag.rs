//! Messstellenverträge (§ 9, § 10 MsbG) — the contract a WiM Kündigung ends.
//!
//! `processd` reads this to answer a Kündigung MSB out of `E_0200`; it keeps no
//! copy. The next admissible Kündigungstermin is **derived** here from the
//! notice period rather than stored, so there is one date to keep correct
//! instead of two.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

/// The Messstellenbetriebsvertrag at one Messlokation.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MessstellenvertragRow {
    pub id: Uuid,
    pub tenant: String,
    pub melo_id: String,
    pub msb_mp_id: String,
    pub kunden_id: Option<Uuid>,
    pub vertragsbeginn: Date,
    pub kuendigungsfrist_monate: i32,
    pub kuendigung_zum: Option<Date>,
    pub kuendigung_eingang: Option<Date>,
    pub frueher_moeglich: Option<Date>,
    pub beendet_am: Option<Date>,
}

/// The contract plus the date a Kündigung received today could take effect.
///
/// `E_0200` `Z12` („Ablehnung Vertragsbindung") must name the nächstmöglicher
/// Kündigungszeitpunkt, so the read model computes it instead of leaving the
/// caller to re-derive the notice period.
#[derive(Debug, Clone, Serialize)]
pub struct MessstellenvertragView {
    #[serde(flatten)]
    pub vertrag: MessstellenvertragRow,
    /// The earliest date a Kündigung given on `stichtag` may take effect.
    ///
    /// `None` once the contract is terminated or ended — there is nothing left
    /// to terminate, and `E_0200` answers `Z34` / `Z29` rather than a date.
    pub naechstmoeglich: Option<Date>,
    /// The day `naechstmoeglich` was computed against.
    pub stichtag: Date,
}

impl MessstellenvertragRow {
    /// The earliest date a Kündigung given on `stichtag` may take effect.
    ///
    /// § 309 Nr. 9 lit. c BGB caps a consumer's notice period at one month, and
    /// [`crate::domain::zulaessige_kuendigungsfrist_monate`] applies that cap —
    /// a stored three months is not enforced against a Verbraucher because the
    /// clause is void, not the contract. Whether the Anschlussnutzer is a
    /// Haushaltskunde comes from the caller, which holds the Kunde.
    #[must_use]
    pub fn naechstmoeglich(&self, stichtag: Date, haushaltskunde: bool) -> Option<Date> {
        if self.beendet_am.is_some() || self.kuendigung_zum.is_some() {
            return None;
        }
        let monate = crate::domain::zulaessige_kuendigungsfrist_monate(
            haushaltskunde,
            self.kuendigungsfrist_monate,
        );
        Some(crate::domain::add_months(stichtag, monate))
    }

    /// Pair the contract with its computed next admissible date.
    #[must_use]
    pub fn view(self, stichtag: Date, haushaltskunde: bool) -> MessstellenvertragView {
        MessstellenvertragView {
            naechstmoeglich: self.naechstmoeglich(stichtag, haushaltskunde),
            stichtag,
            vertrag: self,
        }
    }
}

/// Fields accepted when creating or replacing a Messstellenvertrag.
#[derive(Debug, Clone, Deserialize)]
pub struct UpsertMessstellenvertragInput {
    pub vertragsbeginn: Date,
    #[serde(default = "default_frist")]
    pub kuendigungsfrist_monate: i32,
    #[serde(default)]
    pub kunden_id: Option<Uuid>,
    #[serde(default)]
    pub kuendigung_zum: Option<Date>,
    #[serde(default)]
    pub kuendigung_eingang: Option<Date>,
    #[serde(default)]
    pub frueher_moeglich: Option<Date>,
    #[serde(default)]
    pub beendet_am: Option<Date>,
}

const fn default_frist() -> i32 {
    1
}

const MSV_COLS: &str = "id, tenant, melo_id, msb_mp_id, kunden_id, vertragsbeginn,
     kuendigungsfrist_monate, kuendigung_zum, kuendigung_eingang,
     frueher_moeglich, beendet_am";

/// The contract this MSB holds at a Messlokation, if any.
///
/// `Ok(None)` is „no contract" — the `ZC9` case. A storage failure is an `Err`
/// and must never be read as absence: answering `ZC9` because a lookup failed
/// refuses a lawful Kündigung and keeps the customer bound.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_messstellenvertrag(
    pool: &PgPool,
    tenant: &str,
    melo_id: &str,
    msb_mp_id: &str,
) -> Result<Option<MessstellenvertragRow>> {
    let sql = format!(
        "SELECT {MSV_COLS} FROM messstellenvertraege
         WHERE tenant = $1 AND melo_id = $2 AND msb_mp_id = $3
         ORDER BY vertragsbeginn DESC LIMIT 1"
    );
    Ok(sqlx::query_as::<_, MessstellenvertragRow>(&sql)
        .bind(tenant)
        .bind(melo_id)
        .bind(msb_mp_id)
        .fetch_optional(pool)
        .await?)
}

/// Create or replace the contract at `(tenant, melo_id, msb_mp_id)`.
///
/// The `msv_no_overlap` exclusion constraint rejects a term that overlaps an
/// existing contract for the same MSB and Messlokation.
///
/// # Errors
///
/// Propagates storage errors, including the exclusion violation.
pub async fn upsert_messstellenvertrag(
    pool: &PgPool,
    tenant: &str,
    melo_id: &str,
    msb_mp_id: &str,
    input: &UpsertMessstellenvertragInput,
) -> Result<Uuid> {
    let existing = find_messstellenvertrag(pool, tenant, melo_id, msb_mp_id).await?;
    if let Some(row) = existing {
        sqlx::query(
            r"UPDATE messstellenvertraege
              SET vertragsbeginn = $2, kuendigungsfrist_monate = $3, kunden_id = $4,
                  kuendigung_zum = $5, kuendigung_eingang = $6,
                  frueher_moeglich = $7, beendet_am = $8, updated_at = now()
              WHERE id = $1",
        )
        .bind(row.id)
        .bind(input.vertragsbeginn)
        .bind(input.kuendigungsfrist_monate)
        .bind(input.kunden_id)
        .bind(input.kuendigung_zum)
        .bind(input.kuendigung_eingang)
        .bind(input.frueher_moeglich)
        .bind(input.beendet_am)
        .execute(pool)
        .await?;
        return Ok(row.id);
    }
    let id: Uuid = sqlx::query_scalar(
        r"INSERT INTO messstellenvertraege
              (tenant, melo_id, msb_mp_id, kunden_id, vertragsbeginn,
               kuendigungsfrist_monate, kuendigung_zum, kuendigung_eingang,
               frueher_moeglich, beendet_am)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          RETURNING id",
    )
    .bind(tenant)
    .bind(melo_id)
    .bind(msb_mp_id)
    .bind(input.kunden_id)
    .bind(input.vertragsbeginn)
    .bind(input.kuendigungsfrist_monate)
    .bind(input.kuendigung_zum)
    .bind(input.kuendigung_eingang)
    .bind(input.frueher_moeglich)
    .bind(input.beendet_am)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Record that a Kündigung has taken effect.
///
/// Called after the MSBA confirms one, so the next Kündigung on the same
/// Messlokation resolves through the Kap. 2.2.3 table (`Z34` / `Z29`) instead
/// of being confirmed a second time.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn record_kuendigung(
    pool: &PgPool,
    tenant: &str,
    melo_id: &str,
    msb_mp_id: &str,
    eingang: Date,
    zum: Date,
) -> Result<bool> {
    let n = sqlx::query(
        r"UPDATE messstellenvertraege
          SET kuendigung_zum = $4, kuendigung_eingang = $5, updated_at = now()
          WHERE tenant = $1 AND melo_id = $2 AND msb_mp_id = $3
            AND beendet_am IS NULL",
    )
    .bind(tenant)
    .bind(melo_id)
    .bind(msb_mp_id)
    .bind(zum)
    .bind(eingang)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn row(frist: i32, gekuendigt: Option<Date>, beendet: Option<Date>) -> MessstellenvertragRow {
        MessstellenvertragRow {
            id: Uuid::nil(),
            tenant: "9900357000004".to_owned(),
            melo_id: "DE000…1".to_owned(),
            msb_mp_id: "9900000000003".to_owned(),
            kunden_id: None,
            vertragsbeginn: Date::from_calendar_date(2024, Month::January, 1).unwrap(),
            kuendigungsfrist_monate: frist,
            kuendigung_zum: gekuendigt,
            kuendigung_eingang: None,
            frueher_moeglich: None,
            beendet_am: beendet,
        }
    }

    #[test]
    fn the_next_admissible_date_is_the_notice_period_from_the_stichtag() {
        let d = |y, m, day| Date::from_calendar_date(y, m, day).unwrap();
        assert_eq!(
            row(3, None, None).naechstmoeglich(d(2026, Month::March, 15), false),
            Some(d(2026, Month::June, 15)),
            "a business customer keeps the contractual three months"
        );
    }

    /// § 309 Nr. 9 lit. c BGB caps a consumer's notice at one month. The clause
    /// is void, not the contract, so the stored period is capped rather than
    /// rejected.
    #[test]
    fn a_consumers_notice_period_is_capped_at_one_month() {
        let d = |y, m, day| Date::from_calendar_date(y, m, day).unwrap();
        assert_eq!(
            row(3, None, None).naechstmoeglich(d(2026, Month::March, 15), true),
            Some(d(2026, Month::April, 15))
        );
    }

    /// A contract already terminated or ended has no next admissible date:
    /// `E_0200` answers `Z34` / `Z29`, which name the existing Vertragsende.
    #[test]
    fn a_terminated_contract_has_no_next_date() {
        let d = |y, m, day| Date::from_calendar_date(y, m, day).unwrap();
        let stichtag = d(2026, Month::March, 15);
        assert_eq!(
            row(1, Some(d(2026, Month::August, 1)), None).naechstmoeglich(stichtag, false),
            None
        );
        assert_eq!(
            row(1, None, Some(d(2026, Month::February, 1))).naechstmoeglich(stichtag, false),
            None
        );
    }
}
