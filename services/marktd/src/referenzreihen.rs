//! Referenzreihen — the reads, the refusal and the import the routes
//! (`handlers::referenzreihen`) and the MCP tools share.
//!
//! | Method | Path | Cedar | |
//! |---|---|---|---|
//! | `GET` | `/api/v1/referenzreihen` | `read-referenzreihen` | the catalogue with each series' newest value |
//! | `GET` | `/api/v1/referenzreihen/{reihe}/werte` | `read-referenzreihen` | values overlapping `von`–`bis`, as known at `stand` |
//! | `GET` | `/api/v1/referenzreihen/{reihe}/wert` | `read-referenzreihen` | the value of one `jahr` (or `jahr`+`monat`), or `422 referenzwert_fehlt` |
//! | `POST` | `/api/v1/referenzreihen/{reihe}/import` | `import-referenzreihen` | a publisher's file; `vorschau=true` writes nothing |
//!
//! A missing value is a refusal naming the series, the period and the
//! publication rule — never a zero and never the neighbouring period's value,
//! either of which misprices every unit of the period.

use mako_markt::referenzreihen::{
    REFERENZWERT_FEHLT, Referenzwert, ReferenzwertFehlt, Reihe, ReiheInfo, WertStatus, Zeitraum,
};
use mako_service::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Date, OffsetDateTime};

use crate::pg::referenzreihen::{self as store, Erfassung, Herkunft, ImportBeleg, NeuerWert};
use crate::referenzimport::{self, Format};

/// The most values one `werte` read returns.
pub const MAX_WERTE: i64 = 5_000;

/// The series a path segment names, or `404 referenzreihe_unbekannt`.
pub fn reihe_of(key: &str) -> ApiResult<Reihe> {
    Reihe::from_schluessel(key).ok_or_else(|| {
        ApiError::not_found(format!("Referenzreihe {key:?} ist nicht katalogisiert"))
            .with_code("referenzreihe_unbekannt")
    })
}

/// The `422 referenzwert_fehlt` refusal.
#[must_use]
pub fn fehlt(f: &ReferenzwertFehlt) -> ApiError {
    ApiError::unprocessable(f.to_string())
        .with_code(REFERENZWERT_FEHLT)
        .with_fields(serde_json::to_value(f).unwrap_or_default())
}

/// One catalogue entry with its newest current value.
#[derive(Debug, Serialize)]
pub struct ReiheMitStand {
    #[serde(flatten)]
    pub info: ReiheInfo,
    /// The current value with the latest period, if any.
    pub neuester_wert: Option<Referenzwert>,
}

/// The catalogue and each series' newest value.
///
/// # Errors
/// A database failure.
pub async fn katalog(pool: &PgPool) -> ApiResult<Vec<ReiheMitStand>> {
    let neueste = store::neueste(pool).await.map_err(ApiError::internal)?;
    Ok(Reihe::ALL
        .into_iter()
        .map(|r| ReiheMitStand {
            info: r.info(),
            neuester_wert: neueste.iter().find(|w| w.reihe == r.schluessel()).cloned(),
        })
        .collect())
}

/// Query of `GET …/werte`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WerteQuery {
    /// First Berlin calendar day (inclusive).
    pub von: Option<Date>,
    /// Last Berlin calendar day (inclusive).
    pub bis: Option<Date>,
    /// Dimension value; all when absent.
    pub dim: Option<String>,
    /// The instant the read is as of; now when absent.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub stand: Option<OffsetDateTime>,
    /// At most this many values (default and ceiling 5 000).
    pub limit: Option<i64>,
}

/// Query of `GET …/wert`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WertQuery {
    /// The calendar year.
    pub jahr: i32,
    /// The month (1–12), for a monthly series.
    pub monat: Option<u8>,
    /// Dimension value; `""` for a series without one.
    #[serde(default)]
    pub dim: String,
    /// The instant the read is as of; now when absent.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub stand: Option<OffsetDateTime>,
}

/// The value of one period, as known at `stand`, or the refusal.
///
/// # Errors
/// `422` for a period of the wrong granularity or a dimension the series does
/// not have; `422 referenzwert_fehlt` when no value is known; `500` on a
/// database failure.
pub async fn wert_fuer(
    pool: &PgPool,
    reihe: Reihe,
    zeitraum: Zeitraum,
    dim: &str,
    stand: Option<OffsetDateTime>,
) -> ApiResult<Referenzwert> {
    if zeitraum.granularitaet() != reihe.info().granularitaet {
        return Err(ApiError::unprocessable(format!(
            "{reihe} hat Werte je {:?}, nicht je {:?} — für eine Viertelstundenreihe gilt …/werte",
            reihe.info().granularitaet,
            zeitraum.granularitaet()
        ))
        .with_code("zeitraum_passt_nicht"));
    }
    if !reihe.admits_dim(dim) {
        return Err(ApiError::unprocessable(format!(
            "die Dimension {dim:?} gibt es für {reihe} nicht (zulässig: {:?})",
            reihe.info().dimensionen
        ))
        .with_code("dimension_unbekannt"));
    }
    let (von, bis) = zeitraum
        .instants()
        .map_err(|e| ApiError::unprocessable(e.to_string()).with_code("zeitraum_ungueltig"))?;
    store::wert(pool, reihe, dim, von, bis, stand)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| fehlt(&ReferenzwertFehlt::new(reihe, dim, zeitraum)))
}

/// Query of `POST …/import`; the body is the file itself.
#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportQuery {
    /// The file's native format.
    pub format: Format,
    /// Whether the file's values are provisional or final.
    #[param(value_type = String)]
    pub status: WertStatus,
    /// Where the file was published (`https://…`).
    pub quelle_url: String,
    /// When the publisher released it.
    pub veroeffentlicht_am: Option<Date>,
    /// `true` reports what the import would do and writes nothing.
    #[serde(default)]
    pub vorschau: bool,
}

/// One row of an import answer.
#[derive(Debug, Serialize)]
pub struct ImportZeileAntwort {
    pub dim: String,
    #[serde(with = "time::serde::rfc3339")]
    pub gilt_von: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub gilt_bis: OffsetDateTime,
    #[serde(with = "rust_decimal::serde::str")]
    pub wert: rust_decimal::Decimal,
    pub erfassung: Erfassung,
}

/// What an import did — or, under `vorschau`, would do.
#[derive(Debug, Serialize)]
pub struct ImportAntwort {
    pub vorschau: bool,
    /// The same document was imported before; nothing was added.
    pub bereits_importiert: bool,
    pub dokument_sha256: String,
    pub neu: usize,
    pub ersetzt: usize,
    pub unveraendert: usize,
    pub zeilen: Vec<ImportZeileAntwort>,
    /// The record of the earlier import, when there was one.
    pub frueherer_import: Option<ImportBeleg>,
}

/// SHA-256 of `body`, lower-case hex.
#[must_use]
pub fn sha256_hex(body: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Import `body` into `reihe` in one transaction; under `vorschau` the
/// transaction is rolled back, so the preview and the import run the same code.
///
/// # Errors
/// `422 datei_unlesbar` for a file the reader refuses, `422` for a source that
/// is not `https://`, `500` on a database failure.
pub async fn importiere(
    pool: &PgPool,
    reihe: Reihe,
    q: &ImportQuery,
    body: &[u8],
) -> ApiResult<ImportAntwort> {
    if !q.quelle_url.starts_with("https://") {
        return Err(ApiError::unprocessable(format!(
            "quelle_url {:?} ist keine https-Adresse — ein Wert ohne nachprüfbare Quelle wird \
             nicht gespeichert",
            q.quelle_url
        ))
        .with_code("quelle_fehlt"));
    }
    let zeilen = referenzimport::lies(reihe, q.format, body)
        .map_err(|e| ApiError::unprocessable(e.0).with_code("datei_unlesbar"))?;
    let sha = sha256_hex(body);

    let mut tx = pool.begin().await.map_err(ApiError::internal)?;
    if !store::claim_import(&mut tx, &sha, reihe, q.format.as_str(), &q.quelle_url)
        .await
        .map_err(ApiError::internal)?
    {
        drop(tx);
        let frueher = store::import_beleg(pool, &sha)
            .await
            .map_err(ApiError::internal)?;
        return Ok(ImportAntwort {
            vorschau: q.vorschau,
            bereits_importiert: true,
            dokument_sha256: sha,
            neu: 0,
            ersetzt: 0,
            unveraendert: 0,
            zeilen: Vec::new(),
            frueherer_import: frueher,
        });
    }
    let mut antwort = Vec::with_capacity(zeilen.len());
    for z in &zeilen {
        let erfassung = store::erfasse(
            &mut tx,
            &NeuerWert {
                reihe,
                dim: &z.dim,
                gilt_von: z.gilt_von,
                gilt_bis: z.gilt_bis,
                wert: z.wert,
                status: q.status,
                quelle_url: &q.quelle_url,
                veroeffentlicht_am: q.veroeffentlicht_am,
                dokument_sha256: Some(&sha),
                herkunft: Herkunft::Import,
            },
        )
        .await
        .map_err(ApiError::internal)?;
        antwort.push(ImportZeileAntwort {
            dim: z.dim.clone(),
            gilt_von: z.gilt_von,
            gilt_bis: z.gilt_bis,
            wert: z.wert,
            erfassung,
        });
    }
    let zaehle = |e: Erfassung| antwort.iter().filter(|z| z.erfassung == e).count();
    let (neu, ersetzt, unveraendert) = (
        zaehle(Erfassung::Neu),
        zaehle(Erfassung::Ersetzt),
        zaehle(Erfassung::Unveraendert),
    );
    let as_i32 = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    store::finish_import(
        &mut tx,
        &sha,
        as_i32(antwort.len()),
        as_i32(neu),
        as_i32(ersetzt),
    )
    .await
    .map_err(ApiError::internal)?;
    if q.vorschau {
        tx.rollback().await.map_err(ApiError::internal)?;
    } else {
        tx.commit().await.map_err(ApiError::internal)?;
    }
    Ok(ImportAntwort {
        vorschau: q.vorschau,
        bereits_importiert: false,
        dokument_sha256: sha,
        neu,
        ersetzt,
        unveraendert,
        zeilen: antwort,
        frueherer_import: None,
    })
}
