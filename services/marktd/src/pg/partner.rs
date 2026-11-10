//! PostgreSQL storage: [`PgPartnerRepository`].

use mako_markt::{
    domain::{MarktpartnerId, Sparte},
    error::MdmError,
    repository::PartnerRecord,
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed partner repository.
#[derive(Clone, Debug)]
pub struct PgPartnerRepository {
    pool: PgPool,
}

impl PgPartnerRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const SELECT_COLS: &str = "mp_id, display_name, marktrolle, sparte, rollencodetyp, makoadresse, geschaeftspartner, edi_zustimmung, edi_extrahierbar, version, updated_at";

impl PgPartnerRepository {
    /// Insert or update a trading partner.
    ///
    /// Returns the new version number.
    pub async fn upsert(&self, partner: PartnerRecord) -> Result<i64, MdmError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::upsert_tx(&mut conn, partner).await
    }

    /// [`Self::upsert`] on the caller's transaction, so the row and the
    /// CloudEvent announcing it commit together.
    pub async fn upsert_tx(
        conn: &mut sqlx::PgConnection,
        partner: PartnerRecord,
    ) -> Result<i64, MdmError> {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM partners WHERE mp_id = $1")
                .bind(partner.mp_id.as_ref())
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = current.map_or(1, |v| v + 1);
        let sparte_str = partner.sparte.map(|s| s.to_string());
        // Columns stay TEXT; the typed enums serialise to their BDEW codes
        // ("LF", "NB", … / "BDEW", "DVGW", "GLN") through `Display`.
        let marktrolle_str = partner.marktrolle.map(|m| m.to_string());
        let rollencodetyp_str = partner.rollencodetyp.map(|r| r.to_string());

        sqlx::query(
            r#"INSERT INTO partners (mp_id, display_name, marktrolle, sparte, rollencodetyp, makoadresse, geschaeftspartner, edi_zustimmung, edi_extrahierbar, version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
               ON CONFLICT (mp_id) DO UPDATE
               SET display_name  = EXCLUDED.display_name,
                   marktrolle    = EXCLUDED.marktrolle,
                   sparte        = EXCLUDED.sparte,
                   rollencodetyp = EXCLUDED.rollencodetyp,
                   makoadresse   = EXCLUDED.makoadresse,
                   geschaeftspartner = EXCLUDED.geschaeftspartner,
                   edi_zustimmung   = EXCLUDED.edi_zustimmung,
                   edi_extrahierbar = EXCLUDED.edi_extrahierbar,
                   version       = EXCLUDED.version,
                   updated_at    = now()"#,
        )
        .bind(partner.mp_id.as_ref())
        .bind(&partner.display_name)
        .bind(&marktrolle_str)
        .bind(sparte_str)
        .bind(&rollencodetyp_str)
        .bind(&partner.makoadresse)
        .bind(&partner.geschaeftspartner)
        .bind(partner.edi_zustimmung)
        .bind(partner.edi_extrahierbar)
        .bind(new_version)
        .execute(&mut *conn)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    /// Return a partner by their 13-digit `MarktpartnerId`.
    pub async fn find(&self, id: &MarktpartnerId) -> Result<Option<PartnerRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM partners WHERE mp_id = $1"
        )))
        .bind(id.as_ref())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_partner))
    }

    /// List all partners.
    pub async fn list(&self) -> Result<Vec<PartnerRecord>, MdmError> {
        let rows: Vec<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM partners ORDER BY mp_id"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_partner).collect())
    }
}

/// Decode a BO4E enum column, reporting a value the schema does not define.
///
/// Returns `None` so one bad row cannot fail every read, but says so — the
/// previous `.parse().ok()` discarded it without a trace.
fn decode_enum<T: rubo4e::Bo4eEnum>(column: &str, raw: &str) -> Option<T> {
    match T::from_wire(raw) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::error!(
                column,
                value = raw,
                error = %e,
                "partners: stored value is not a BO4E wire value — the CHECK \
                 constraint should have refused it; serving the partner without it"
            );
            None
        }
    }
}

fn row_to_partner(r: PgRow) -> PartnerRecord {
    let sparte_str: Option<String> = r.get("sparte");
    let makoadresse: Option<Vec<String>> = r.try_get("makoadresse").unwrap_or(None);
    let marktrolle_str: Option<String> = r.get("marktrolle");
    let rollencodetyp_str: Option<String> = r.try_get("rollencodetyp").unwrap_or(None);
    PartnerRecord {
        mp_id: super::id(&r, "mp_id"),
        display_name: r.get("display_name"),
        // Stored as the BO4E wire strings, and `CHECK`-constrained to them.
        //
        // `from_wire` rather than `.parse().ok()`: both refuse a value the
        // schema does not define, but the silent `None` was indistinguishable
        // from "this partner has no market role" — and this field is served
        // verbatim in `GET /partners/{id}/marktteilnehmer`, so a dropped role
        // reads to the counterparty as a fact rather than a fault. The column
        // constraint makes this unreachable; the log is what tells us if it
        // ever is not.
        marktrolle: marktrolle_str.and_then(|s| decode_enum("marktrolle", &s)),
        sparte: sparte_str.as_deref().map(parse_sparte),
        rollencodetyp: rollencodetyp_str.and_then(|s| decode_enum("rollencodetyp", &s)),
        makoadresse: makoadresse.unwrap_or_default(),
        geschaeftspartner: r.get("geschaeftspartner"),
        edi_zustimmung: r.get("edi_zustimmung"),
        edi_extrahierbar: r.get("edi_extrahierbar"),
        version: r.get("version"),
        updated_at: r.get("updated_at"),
    }
}

fn parse_sparte(s: &str) -> Sparte {
    match s {
        "GAS" => Sparte::Gas,
        _ => Sparte::Strom,
    }
}
