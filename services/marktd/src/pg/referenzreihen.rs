//! The `referenzwerte` store — append-only, bitemporal.
//!
//! A value is never updated in place. Recording one for a period that already
//! has a current value either leaves it (same value and status) or closes the
//! current row's `erfasst` and inserts the new one, in the caller's
//! transaction, so a read at an earlier `stand` still finds what was known then.

use mako_markt::referenzreihen::{Referenzwert, Reihe, WertStatus};
use rust_decimal::Decimal;
use sqlx::{PgConnection, PgPool, Row as _, postgres::PgRow};
use time::{Date, OffsetDateTime};

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Herkunft {
    /// A publisher's file, imported.
    Import,
}

impl Herkunft {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
        }
    }
}

/// A value to record.
#[derive(Debug, Clone)]
pub struct NeuerWert<'a> {
    pub reihe: Reihe,
    pub dim: &'a str,
    pub gilt_von: OffsetDateTime,
    pub gilt_bis: OffsetDateTime,
    pub wert: Decimal,
    pub status: WertStatus,
    pub quelle_url: &'a str,
    pub veroeffentlicht_am: Option<Date>,
    pub dokument_sha256: Option<&'a str>,
    pub herkunft: Herkunft,
}

/// What recording one value did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Erfassung {
    /// No current value covered the period; one was added.
    Neu,
    /// A current value with a different figure or status was superseded.
    Ersetzt,
    /// The current value already says this.
    Unveraendert,
}

fn wert_of(r: &PgRow) -> Referenzwert {
    let status: String = r.get("status");
    Referenzwert {
        id: r.get("id"),
        reihe: r.get("reihe"),
        dim: r.get("dim"),
        gilt_von: r.get("gilt_von"),
        gilt_bis: r.get("gilt_bis"),
        wert: r.get("wert"),
        status: WertStatus::parse(&status)
            .unwrap_or_else(|| panic!("referenzwerte.status holds {status:?}")),
        quelle_url: r.get("quelle_url"),
        veroeffentlicht_am: r.get("veroeffentlicht_am"),
        dokument_sha256: r.get("dokument_sha256"),
        herkunft: r.get("herkunft"),
        erfasst_von: r.get("erfasst_von"),
        erfasst_bis: r.get("erfasst_bis"),
    }
}

/// Record `w` in the caller's transaction.
///
/// # Errors
/// A database failure, including the `referenzwerte_bitemporal` exclusion when
/// a concurrent writer recorded the same period first.
pub async fn erfasse(conn: &mut PgConnection, w: &NeuerWert<'_>) -> sqlx::Result<Erfassung> {
    let aktuell = sqlx::query(
        "SELECT id, wert, status, lower(gilt) AS gilt_von, upper(gilt) AS gilt_bis
           FROM referenzwerte
          WHERE reihe = $1 AND dim = $2
            AND gilt && tstzrange($3, $4, '[)')
            AND upper_inf(erfasst)
          FOR UPDATE",
    )
    .bind(w.reihe.schluessel())
    .bind(w.dim)
    .bind(w.gilt_von)
    .bind(w.gilt_bis)
    .fetch_all(&mut *conn)
    .await?;

    if let [einziger] = &aktuell[..] {
        let wert: Decimal = einziger.get("wert");
        let status: String = einziger.get("status");
        let von: OffsetDateTime = einziger.get("gilt_von");
        let bis: OffsetDateTime = einziger.get("gilt_bis");
        if wert == w.wert && status == w.status.as_str() && von == w.gilt_von && bis == w.gilt_bis {
            return Ok(Erfassung::Unveraendert);
        }
    }
    let ids: Vec<uuid::Uuid> = aktuell.iter().map(|r| r.get("id")).collect();
    if !ids.is_empty() {
        sqlx::query(
            "UPDATE referenzwerte
                SET erfasst = tstzrange(lower(erfasst), now(), '[)')
              WHERE id = ANY($1)",
        )
        .bind(&ids)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "INSERT INTO referenzwerte
            (reihe, dim, gilt, wert, status, quelle_url, veroeffentlicht_am,
             dokument_sha256, herkunft)
         VALUES ($1, $2, tstzrange($3, $4, '[)'), $5, $6, $7, $8, $9, $10)",
    )
    .bind(w.reihe.schluessel())
    .bind(w.dim)
    .bind(w.gilt_von)
    .bind(w.gilt_bis)
    .bind(w.wert)
    .bind(w.status.as_str())
    .bind(w.quelle_url)
    .bind(w.veroeffentlicht_am)
    .bind(w.dokument_sha256)
    .bind(w.herkunft.as_str())
    .execute(&mut *conn)
    .await?;
    Ok(if ids.is_empty() {
        Erfassung::Neu
    } else {
        Erfassung::Ersetzt
    })
}

/// The value covering `[von, bis)` as known at `stand` (now when `None`).
///
/// # Errors
/// A database failure.
pub async fn wert(
    pool: &PgPool,
    reihe: Reihe,
    dim: &str,
    von: OffsetDateTime,
    bis: OffsetDateTime,
    stand: Option<OffsetDateTime>,
) -> sqlx::Result<Option<Referenzwert>> {
    sqlx::query(
        "SELECT id, reihe, dim, lower(gilt) AS gilt_von, upper(gilt) AS gilt_bis, wert,
                status, quelle_url, veroeffentlicht_am, dokument_sha256, herkunft,
                lower(erfasst) AS erfasst_von, upper(erfasst) AS erfasst_bis
           FROM referenzwerte
          WHERE reihe = $1 AND dim = $2
            AND gilt @> tstzrange($3, $4, '[)')
            AND erfasst @> COALESCE($5, now())",
    )
    .bind(reihe.schluessel())
    .bind(dim)
    .bind(von)
    .bind(bis)
    .bind(stand)
    .fetch_optional(pool)
    .await
    .map(|r| r.as_ref().map(wert_of))
}

/// Every value overlapping `[von, bis)` as known at `stand`, oldest period
/// first, at most `limit`.
///
/// # Errors
/// A database failure.
pub async fn werte(
    pool: &PgPool,
    reihe: Reihe,
    dim: Option<&str>,
    von: Option<OffsetDateTime>,
    bis: Option<OffsetDateTime>,
    stand: Option<OffsetDateTime>,
    limit: i64,
) -> sqlx::Result<Vec<Referenzwert>> {
    sqlx::query(
        "SELECT id, reihe, dim, lower(gilt) AS gilt_von, upper(gilt) AS gilt_bis, wert,
                status, quelle_url, veroeffentlicht_am, dokument_sha256, herkunft,
                lower(erfasst) AS erfasst_von, upper(erfasst) AS erfasst_bis
           FROM referenzwerte
          WHERE reihe = $1
            AND ($2::text IS NULL OR dim = $2)
            AND gilt && tstzrange($3, $4, '[)')
            AND erfasst @> COALESCE($5, now())
          ORDER BY lower(gilt), dim
          LIMIT $6",
    )
    .bind(reihe.schluessel())
    .bind(dim)
    .bind(von)
    .bind(bis)
    .bind(stand)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map(|rows| rows.iter().map(wert_of).collect())
}

/// The latest current value of each series that has one — the period with the
/// latest start.
///
/// # Errors
/// A database failure.
pub async fn neueste(pool: &PgPool) -> sqlx::Result<Vec<Referenzwert>> {
    sqlx::query(
        "SELECT DISTINCT ON (reihe) id, reihe, dim, lower(gilt) AS gilt_von, upper(gilt) AS gilt_bis, wert,
                status, quelle_url, veroeffentlicht_am, dokument_sha256, herkunft,
                lower(erfasst) AS erfasst_von, upper(erfasst) AS erfasst_bis
           FROM referenzwerte
          WHERE upper_inf(erfasst)
          ORDER BY reihe, lower(gilt) DESC, dim",
    )
        .fetch_all(pool)
        .await
        .map(|rows| rows.iter().map(wert_of).collect())
}

/// One import, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportBeleg {
    /// The document's SHA-256.
    pub dokument_sha256: String,
    /// Rows the file carried.
    pub zeilen: i32,
    /// Rows added for a period that had no value.
    pub neu: i32,
    /// Current values superseded.
    pub ersetzt: i32,
    /// When it was imported.
    #[serde(with = "time::serde::rfc3339")]
    pub importiert_am: OffsetDateTime,
}

/// Claim `sha` as imported, in the caller's transaction; `None` when the same
/// document was imported before.
///
/// # Errors
/// A database failure.
pub async fn claim_import(
    conn: &mut PgConnection,
    sha: &str,
    reihe: Reihe,
    format: &str,
    quelle_url: &str,
) -> sqlx::Result<bool> {
    let claimed = sqlx::query(
        "INSERT INTO referenz_importe (dokument_sha256, reihe, format, quelle_url, zeilen, neu, ersetzt)
         VALUES ($1, $2, $3, $4, 0, 0, 0)
         ON CONFLICT (dokument_sha256) DO NOTHING",
    )
    .bind(sha)
    .bind(reihe.schluessel())
    .bind(format)
    .bind(quelle_url)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(claimed == 1)
}

/// Record the counts of a claimed import.
///
/// # Errors
/// A database failure.
pub async fn finish_import(
    conn: &mut PgConnection,
    sha: &str,
    zeilen: i32,
    neu: i32,
    ersetzt: i32,
) -> sqlx::Result<ImportBeleg> {
    let r = sqlx::query(
        "UPDATE referenz_importe SET zeilen = $2, neu = $3, ersetzt = $4
          WHERE dokument_sha256 = $1
          RETURNING dokument_sha256, zeilen, neu, ersetzt, importiert_am",
    )
    .bind(sha)
    .bind(zeilen)
    .bind(neu)
    .bind(ersetzt)
    .fetch_one(&mut *conn)
    .await?;
    Ok(beleg_of(&r))
}

/// The earlier import of `sha`.
///
/// # Errors
/// A database failure.
pub async fn import_beleg(pool: &PgPool, sha: &str) -> sqlx::Result<Option<ImportBeleg>> {
    sqlx::query(
        "SELECT dokument_sha256, zeilen, neu, ersetzt, importiert_am
           FROM referenz_importe WHERE dokument_sha256 = $1",
    )
    .bind(sha)
    .fetch_optional(pool)
    .await
    .map(|r| r.as_ref().map(beleg_of))
}

fn beleg_of(r: &PgRow) -> ImportBeleg {
    ImportBeleg {
        dokument_sha256: r.get("dokument_sha256"),
        zeilen: r.get("zeilen"),
        neu: r.get("neu"),
        ersetzt: r.get("ersetzt"),
        importiert_am: r.get("importiert_am"),
    }
}
