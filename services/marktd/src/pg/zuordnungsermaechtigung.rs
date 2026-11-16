//! PostgreSQL storage of the Zuordnungsermächtigungen a BKV gives the NB by
//! UTILMD 55071 and withdraws by 55072 (BK6-24-174 Anlage 3 (MaBiS) Kap. 10.2,
//! 10.3).
//!
//! Written only by the event ingest, read by processd for `E_0622` Prüfschritt
//! 60. Validity is half-open `[valid_from, valid_to)`, and the table's GiST
//! exclusion keeps one answer per (LF, BK, BG, ZRT) and day.

use mako_markt::{error::MdmError, repository::ZuordnungsermaechtigungRecord};
use sqlx::{PgConnection, PgPool, Row, postgres::PgRow};

/// The (ZRT, BG, BK, LF) key of one Ermächtigung and the BKV that gave it.
#[derive(Debug, Clone, Copy)]
pub struct Schluessel<'a> {
    /// MP-ID of the BKV (the 55071's sender).
    pub bkv_mp_id: &'a str,
    /// MP-ID of the Lieferant.
    pub lf_mp_id: &'a str,
    /// Bilanzkreis EIC.
    pub bilanzkreis: &'a str,
    /// Bilanzierungsgebiet EIC.
    pub bilanzierungsgebiet: &'a str,
    /// Zeitreihentyp.
    pub zeitreihentyp: &'a str,
}

/// What a 55072 changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deaktivierung {
    /// The Ermächtigung active on the day before the Deaktivierungsdatum now
    /// ends on it.
    Beendet,
    /// No Ermächtigung of that key was active then: nothing to end. MaBiS
    /// Kap. 10.3.2 Nr. 2 has the NB clear it manually with the BKV.
    NichtsAktiv,
}

/// Record a 55071: the key is authorised from `ab`.
///
/// A day an existing row already covers stays as it is — a redelivered or
/// repeated Aktivierung authorises nothing new. Otherwise the new row runs
/// until the next row of the same key begins, so it never overlaps one.
///
/// # Errors
///
/// [`MdmError::Internal`] on a database failure.
pub async fn aktivieren(
    conn: &mut PgConnection,
    tenant: &str,
    key: Schluessel<'_>,
    ab: time::Date,
) -> Result<(), MdmError> {
    let gedeckt: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM zuordnungsermaechtigungen
            WHERE tenant = $1 AND lf_mp_id = $2 AND bilanzkreis = $3
              AND bilanzierungsgebiet = $4 AND zeitreihentyp = $5
              AND valid_from <= $6 AND (valid_to IS NULL OR valid_to > $6)
        )
        "#,
    )
    .bind(tenant)
    .bind(key.lf_mp_id)
    .bind(key.bilanzkreis)
    .bind(key.bilanzierungsgebiet)
    .bind(key.zeitreihentyp)
    .bind(ab)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| MdmError::Internal(e.to_string()))?;
    if gedeckt {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO zuordnungsermaechtigungen
            (tenant, bkv_mp_id, lf_mp_id, bilanzkreis, bilanzierungsgebiet,
             zeitreihentyp, valid_from, valid_to)
        VALUES ($1, $2, $3, $4, $5, $6, $7, (
            SELECT min(valid_from) FROM zuordnungsermaechtigungen
            WHERE tenant = $1 AND lf_mp_id = $3 AND bilanzkreis = $4
              AND bilanzierungsgebiet = $5 AND zeitreihentyp = $6
              AND valid_from > $7
        ))
        "#,
    )
    .bind(tenant)
    .bind(key.bkv_mp_id)
    .bind(key.lf_mp_id)
    .bind(key.bilanzkreis)
    .bind(key.bilanzierungsgebiet)
    .bind(key.zeitreihentyp)
    .bind(ab)
    .execute(&mut *conn)
    .await
    .map_err(|e| MdmError::Internal(e.to_string()))?;
    Ok(())
}

/// Record a 55072: the Ermächtigung active before `ab` ends on `ab`.
///
/// Kap. 10.3.1 (S. 121): „Eine Deaktivierung ist nur in die Zukunft gerichtet
/// möglich" and afterwards „können keine Zuordnungen mit Gültigkeit nach dem
/// Deaktivierungsdatum mehr durchgeführt werden" — `ab` is the first day no
/// longer covered. A row already ending on or before `ab` is left as it is,
/// so a redelivered 55072 changes nothing.
///
/// # Errors
///
/// [`MdmError::Internal`] on a database failure.
pub async fn deaktivieren(
    conn: &mut PgConnection,
    tenant: &str,
    key: Schluessel<'_>,
    ab: time::Date,
    nachrichtendatum: Option<time::Date>,
) -> Result<Deaktivierung, MdmError> {
    let geaendert = sqlx::query(
        r#"
        UPDATE zuordnungsermaechtigungen
        SET valid_to = $6, deaktivierung_nachrichtendatum = $7, updated_at = now()
        WHERE tenant = $1 AND lf_mp_id = $2 AND bilanzkreis = $3
          AND bilanzierungsgebiet = $4 AND zeitreihentyp = $5
          AND valid_from < $6 AND (valid_to IS NULL OR valid_to > $6)
        "#,
    )
    .bind(tenant)
    .bind(key.lf_mp_id)
    .bind(key.bilanzkreis)
    .bind(key.bilanzierungsgebiet)
    .bind(key.zeitreihentyp)
    .bind(ab)
    .bind(nachrichtendatum)
    .execute(&mut *conn)
    .await
    .map_err(|e| MdmError::Internal(e.to_string()))?
    .rows_affected();
    Ok(if geaendert == 0 {
        Deaktivierung::NichtsAktiv
    } else {
        Deaktivierung::Beendet
    })
}

fn map_row(row: &PgRow) -> Result<ZuordnungsermaechtigungRecord, sqlx::Error> {
    Ok(ZuordnungsermaechtigungRecord {
        bkv_mp_id: row.try_get("bkv_mp_id")?,
        lf_mp_id: row.try_get("lf_mp_id")?,
        bilanzkreis: row.try_get("bilanzkreis")?,
        bilanzierungsgebiet: row.try_get("bilanzierungsgebiet")?,
        zeitreihentyp: row.try_get("zeitreihentyp")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to")?,
        deaktivierung_nachrichtendatum: row.try_get("deaktivierung_nachrichtendatum")?,
    })
}

/// Read access for the REST surface.
#[derive(Clone, Debug)]
pub struct PgZuordnungsermaechtigungRepository {
    pool: PgPool,
}

impl PgZuordnungsermaechtigungRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every Ermächtigung, active or ended, that names `lf_mp_id`, oldest
    /// first.
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on a database failure.
    pub async fn fuer_lieferant(
        &self,
        tenant: &str,
        lf_mp_id: &str,
    ) -> Result<Vec<ZuordnungsermaechtigungRecord>, MdmError> {
        let rows = sqlx::query(
            r#"
            SELECT bkv_mp_id, lf_mp_id, bilanzkreis, bilanzierungsgebiet, zeitreihentyp,
                   valid_from, valid_to, deaktivierung_nachrichtendatum
            FROM zuordnungsermaechtigungen
            WHERE tenant = $1 AND lf_mp_id = $2
            ORDER BY bilanzkreis, bilanzierungsgebiet, zeitreihentyp, valid_from
            "#,
        )
        .bind(tenant)
        .bind(lf_mp_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        rows.iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }
}
