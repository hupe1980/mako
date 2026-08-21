//! PostgreSQL persistence for `neuanlage_faelle` — the `E_0608` Prüflauf.
//!
//! A Neuanlage is the one NB decision that is a **loop with a deadline** rather
//! than a single evaluation: `E_0608` Prüfschritte 110 / 590 have the NB
//! re-check an unidentifiable Marktlokation daily for 60 Werktage before it may
//! refuse. A row here is that loop's memory.

use anyhow::Context as _;
use mako_pruefung::nb::types::Marktlokationsart;
use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Where a Neuanlage case stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NeuanlageStatus {
    /// In the Prüflauf — the Marktlokation is not identified yet.
    Offen,
    /// Answered, with a Bestätigung or an Ablehnung.
    Beantwortet,
    /// A fact the tree needs is missing; an operator decides.
    Eskaliert,
}

impl NeuanlageStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offen => "offen",
            Self::Beantwortet => "beantwortet",
            Self::Eskaliert => "eskaliert",
        }
    }
}

impl std::str::FromStr for NeuanlageStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "offen" => Ok(Self::Offen),
            "beantwortet" => Ok(Self::Beantwortet),
            "eskaliert" => Ok(Self::Eskaliert),
            other => Err(format!("unknown NeuanlageStatus: {other:?}")),
        }
    }
}

/// One row of `neuanlage_faelle`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeuanlageFall {
    pub id: Uuid,
    pub tenant: String,
    pub process_id: Uuid,
    pub pid: i32,
    pub lf_mp_id: String,
    pub marktlokationsart: String,
    pub veraeusserungsform: Option<String>,
    pub uebertragungstag: Date,
    pub zuordnungsbeginn: Date,
    /// ÜT + 60 Werktage. A refusal for non-identification before this date
    /// contradicts `E_0608` Prüfschritt 110 / 590.
    pub letzter_pruefungstag: Date,
    pub status: String,
    pub malo_id: Option<String>,
    pub pruefungen: i32,
    pub letzte_pruefung_am: Option<Date>,
    pub antwortcode: Option<String>,
    pub detail: Option<String>,
    pub beantwortet_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl NeuanlageFall {
    /// The `E_0608` branch this case is answered from.
    #[must_use]
    pub fn art(&self) -> Marktlokationsart {
        if self.marktlokationsart == "ERZEUGEND" {
            Marktlokationsart::Erzeugend
        } else {
            Marktlokationsart::Verbrauchend
        }
    }
}

const COLUMNS: &str = "id, tenant, process_id, pid, lf_mp_id, marktlokationsart, \
     veraeusserungsform, uebertragungstag, zuordnungsbeginn, letzter_pruefungstag, \
     status, malo_id, pruefungen, letzte_pruefung_am, antwortcode, detail, \
     beantwortet_at, created_at, updated_at";

fn row_to_fall(r: &sqlx::postgres::PgRow) -> anyhow::Result<NeuanlageFall> {
    Ok(NeuanlageFall {
        id: r.try_get("id")?,
        tenant: r.try_get("tenant")?,
        process_id: r.try_get("process_id")?,
        pid: r.try_get("pid")?,
        lf_mp_id: r.try_get("lf_mp_id")?,
        marktlokationsart: r.try_get("marktlokationsart")?,
        veraeusserungsform: r.try_get("veraeusserungsform")?,
        uebertragungstag: r.try_get("uebertragungstag")?,
        zuordnungsbeginn: r.try_get("zuordnungsbeginn")?,
        letzter_pruefungstag: r.try_get("letzter_pruefungstag")?,
        status: r.try_get("status")?,
        malo_id: r.try_get("malo_id")?,
        pruefungen: r.try_get("pruefungen")?,
        letzte_pruefung_am: r.try_get("letzte_pruefung_am")?,
        antwortcode: r.try_get("antwortcode")?,
        detail: r.try_get("detail")?,
        beantwortet_at: r.try_get("beantwortet_at")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

/// What an inbound 55600 / 55601 opens a case with.
#[derive(Debug, Clone)]
pub struct NewNeuanlageFall {
    pub process_id: Uuid,
    pub pid: i32,
    pub lf_mp_id: String,
    pub marktlokationsart: Marktlokationsart,
    pub veraeusserungsform: Option<String>,
    pub uebertragungstag: Date,
    pub zuordnungsbeginn: Date,
    pub letzter_pruefungstag: Date,
}

/// Open a case, or return the existing one.
///
/// `ON CONFLICT DO NOTHING` on `(tenant, process_id)`: an ORDERS redelivered
/// over AS4 must not restart the Prüflauf clock.
///
/// # Errors
///
/// Propagates database errors.
pub async fn open_case(
    pool: &PgPool,
    tenant: &str,
    new: &NewNeuanlageFall,
) -> anyhow::Result<Option<Uuid>> {
    let art = match new.marktlokationsart {
        Marktlokationsart::Erzeugend => "ERZEUGEND",
        _ => "VERBRAUCHEND",
    };
    let row = sqlx::query(
        r"INSERT INTO neuanlage_faelle
              (tenant, process_id, pid, lf_mp_id, marktlokationsart, veraeusserungsform,
               uebertragungstag, zuordnungsbeginn, letzter_pruefungstag)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
          ON CONFLICT (tenant, process_id) DO NOTHING
          RETURNING id",
    )
    .bind(tenant)
    .bind(new.process_id)
    .bind(new.pid)
    .bind(&new.lf_mp_id)
    .bind(art)
    .bind(&new.veraeusserungsform)
    .bind(new.uebertragungstag)
    .bind(new.zuordnungsbeginn)
    .bind(new.letzter_pruefungstag)
    .fetch_optional(pool)
    .await
    .context("insert neuanlage_fall")?;
    Ok(row.map(|r| r.get("id")))
}

/// Every open case whose daily Prüflauf has not run today.
///
/// Ordered by deadline: the ones closest to running out of Werktage are the ones
/// an operator must see first.
///
/// # Errors
///
/// Propagates database errors.
pub async fn due_for_pruefung(
    pool: &PgPool,
    tenant: &str,
    today: Date,
    limit: i64,
) -> anyhow::Result<Vec<NeuanlageFall>> {
    let rows = sqlx::query(&format!(
        r"SELECT {COLUMNS} FROM neuanlage_faelle
          WHERE tenant = $1 AND status = 'offen'
            AND (letzte_pruefung_am IS NULL OR letzte_pruefung_am < $2)
          ORDER BY letzter_pruefungstag
          LIMIT $3"
    ))
    .bind(tenant)
    .bind(today)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("select neuanlage cases due")?;
    rows.iter().map(row_to_fall).collect()
}

/// Record that today's Prüflauf ran and the case is still open.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_pruefung(
    conn: impl PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
    today: Date,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE neuanlage_faelle
          SET pruefungen = pruefungen + 1, letzte_pruefung_am = $3, updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'offen'",
    )
    .bind(id)
    .bind(tenant)
    .bind(today)
    .execute(conn)
    .await
    .context("record neuanlage Prüfung")?;
    Ok(())
}

/// Close a case with the answer that went out.
///
/// # Errors
///
/// Propagates database errors.
pub async fn close_case(
    conn: impl PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
    antwortcode: &str,
    detail: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE neuanlage_faelle
          SET status = 'beantwortet', antwortcode = $3, detail = $4,
              beantwortet_at = now(), updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'offen'",
    )
    .bind(id)
    .bind(tenant)
    .bind(antwortcode)
    .bind(detail)
    .execute(conn)
    .await
    .context("close neuanlage case")?;
    Ok(())
}

/// Hand a case to an operator — a Prüfschritt needs a fact nobody supplied.
///
/// # Errors
///
/// Propagates database errors.
pub async fn escalate_case(
    conn: impl PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
    detail: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE neuanlage_faelle
          SET status = 'eskaliert', detail = $3, updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'offen'",
    )
    .bind(id)
    .bind(tenant)
    .bind(detail)
    .execute(conn)
    .await
    .context("escalate neuanlage case")?;
    Ok(())
}

/// Record the Marktlokation an operator or an NIS integration identified.
///
/// Returns `false` when no open case with that id exists in this tenant.
///
/// # Errors
///
/// Propagates database errors.
pub async fn set_identifikation(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    malo_id: &str,
) -> anyhow::Result<bool> {
    let n = sqlx::query(
        r"UPDATE neuanlage_faelle
          SET malo_id = $3, letzte_pruefung_am = NULL, updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'offen'",
    )
    .bind(id)
    .bind(tenant)
    .bind(malo_id)
    .execute(pool)
    .await
    .context("set neuanlage identification")?
    .rows_affected();
    Ok(n > 0)
}

/// One case by id.
///
/// # Errors
///
/// Propagates database errors.
pub async fn fetch_case(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<NeuanlageFall>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM neuanlage_faelle WHERE id = $1 AND tenant = $2"
    ))
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch neuanlage case")?;
    row.as_ref().map(row_to_fall).transpose()
}

/// Cases in a tenant, newest first.
///
/// # Errors
///
/// Propagates database errors.
pub async fn list_cases(
    pool: &PgPool,
    tenant: &str,
    status: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<NeuanlageFall>> {
    let rows = sqlx::query(&format!(
        r"SELECT {COLUMNS} FROM neuanlage_faelle
          WHERE tenant = $1 AND ($2::text IS NULL OR status = $2)
          ORDER BY created_at DESC
          LIMIT $3"
    ))
    .bind(tenant)
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list neuanlage cases")?;
    rows.iter().map(row_to_fall).collect()
}
