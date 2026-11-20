//! Online orders: the operator's legal documents, the append-only
//! Bestellnachweis, the Bestellung keyed by its `Idempotency-Key`, and the
//! consumer's Widerruf.

use anyhow::Result;
use serde::Serialize;
use sqlx::{PgPool, Row as _};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::bestellung::{self, BestellKunde, BestellungInput, DokumentArt};

// ── The type gate ─────────────────────────────────────────────────────────────

/// A Bestellnachweis that is stored. Its fields are private and the only
/// constructors read or write the `bestellnachweise` row, so holding one is
/// proof the record exists.
#[derive(Debug, Clone)]
pub struct GespeicherterNachweis {
    id: Uuid,
    sha256: String,
    belehrung_angezeigt: bool,
    wertersatz_info_angezeigt: bool,
    lieferung_verlangt: bool,
    bestellt_am: OffsetDateTime,
}

impl GespeicherterNachweis {
    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The § 357a Abs. 2 Satz 1 Nr. 1 and Nr. 3 BGB facts of the record.
    #[must_use]
    pub const fn wertersatz_nachweis(&self) -> crate::widerruf::WertersatzNachweis {
        crate::widerruf::WertersatzNachweis {
            lieferung_verlangt: self.lieferung_verlangt,
            belehrung_angezeigt: self.belehrung_angezeigt,
            wertersatz_info_angezeigt: self.wertersatz_info_angezeigt,
        }
    }

    /// The day the record proves the Widerrufsbelehrung was shown — the day
    /// of the order — or `None` when it records that it was not (§ 356 Abs. 3
    /// Satz 1 BGB).
    #[must_use]
    pub fn belehrung_am(&self) -> Option<Date> {
        self.belehrung_angezeigt
            .then(|| mako_fristen::berlin_date(self.bestellt_am))
    }
}

/// How an online consumer contract came about: its Bestellnachweis and the
/// instant of the Vertragsschluss.
///
/// The only argument under which `pg::vertraege::insert_versorgungsvertrag_tx`
/// writes `abschlussweg = 'ONLINE'`, and it cannot be built without a
/// [`GespeicherterNachweis`] — § 312j Abs. 4 BGB: „Ein Vertrag nach Absatz 2
/// kommt nur zustande, wenn der Unternehmer seine Pflicht aus Absatz 3
/// erfüllt."
#[derive(Debug, Clone)]
pub struct OnlineVertragsschluss {
    nachweis_id: Uuid,
    am: OffsetDateTime,
}

impl OnlineVertragsschluss {
    #[must_use]
    pub const fn new(nachweis: &GespeicherterNachweis, am: OffsetDateTime) -> Self {
        Self {
            nachweis_id: nachweis.id,
            am,
        }
    }

    #[must_use]
    pub const fn nachweis_id(&self) -> Uuid {
        self.nachweis_id
    }

    #[must_use]
    pub const fn am(&self) -> OffsetDateTime {
        self.am
    }
}

// ── Rechtsdokumente ───────────────────────────────────────────────────────────

/// One registered version of an operator legal document.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RechtsdokumentRow {
    pub id: Uuid,
    pub art: String,
    pub version: i32,
    #[serde(skip)]
    pub inhalt: Vec<u8>,
    pub media_type: String,
    pub sha256: String,
    pub gueltig_ab: Date,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Register a document version. Registering bytes already held returns the
/// existing version (`false`) — a hash names exactly one version.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn insert_rechtsdokument(
    pool: &PgPool,
    tenant: &str,
    art: DokumentArt,
    inhalt: &[u8],
    media_type: &str,
    gueltig_ab: Date,
) -> Result<(RechtsdokumentRow, bool)> {
    let sha256 = bestellung::sha256_hex(inhalt);
    let mut tx = pool.begin().await?;
    // One writer per (tenant, art) allocates the next version.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1 || '/rechtsdokument/' || $2))")
        .bind(tenant)
        .bind(art.as_db())
        .execute(&mut *tx)
        .await?;
    if let Some(row) = sqlx::query_as::<_, RechtsdokumentRow>(
        "SELECT id, art, version, inhalt, media_type, sha256, gueltig_ab, created_at
           FROM rechtsdokumente WHERE tenant = $1 AND sha256 = $2",
    )
    .bind(tenant)
    .bind(&sha256)
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return Ok((row, false));
    }
    let row = sqlx::query_as::<_, RechtsdokumentRow>(
        "INSERT INTO rechtsdokumente (tenant, art, version, inhalt, media_type, sha256, gueltig_ab)
         SELECT $1, $2, COALESCE(MAX(version), 0) + 1, $3, $4, $5, $6
           FROM rechtsdokumente WHERE tenant = $1 AND art = $2
         RETURNING id, art, version, inhalt, media_type, sha256, gueltig_ab, created_at",
    )
    .bind(tenant)
    .bind(art.as_db())
    .bind(inhalt)
    .bind(media_type)
    .bind(&sha256)
    .bind(gueltig_ab)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((row, true))
}

/// The document version with this hash.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_rechtsdokument(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    sha256: &str,
) -> Result<Option<RechtsdokumentRow>> {
    Ok(sqlx::query_as(
        "SELECT id, art, version, inhalt, media_type, sha256, gueltig_ab, created_at
           FROM rechtsdokumente WHERE tenant = $1 AND sha256 = $2",
    )
    .bind(tenant)
    .bind(sha256)
    .fetch_optional(executor)
    .await?)
}

/// The cited `rechtsdokumente` an order names that this tenant does not hold
/// under that `art`, as `ART:sha256`. `PREISBLATT` is not checked here — it
/// resolves to the offer's product version.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn unbekannte_dokumente(
    pool: &PgPool,
    tenant: &str,
    dokumente: &[bestellung::DokumentRef],
) -> Result<Vec<String>> {
    let hashes: Vec<&str> = dokumente.iter().map(|d| d.sha256.as_str()).collect();
    let bekannt: Vec<(String, String)> = sqlx::query_as(
        "SELECT art, sha256 FROM rechtsdokumente WHERE tenant = $1 AND sha256 = ANY($2)",
    )
    .bind(tenant)
    .bind(&hashes)
    .fetch_all(pool)
    .await?;
    Ok(dokumente
        .iter()
        .filter(|d| d.art != DokumentArt::Preisblatt)
        .filter(|d| {
            !bekannt
                .iter()
                .any(|(art, sha)| art == d.art.as_db() && *sha == d.sha256)
        })
        .map(|d| format!("{}:{}", d.art.as_db(), d.sha256))
        .collect())
}

// ── Bestellnachweis ───────────────────────────────────────────────────────────

/// The record of one order, as [`insert_nachweis`] stores it.
pub struct NeuerNachweis<'a> {
    pub tenant: &'a str,
    pub partner: &'a str,
    pub input: &'a BestellungInput,
    pub eingegangen_am: OffsetDateTime,
}

/// The RFC 8785 record of an order — what `kanonisch` holds and `sha256`
/// covers. Every key the Bestellnachweis proves and nothing mako adds later.
///
/// # Errors
///
/// When the input cannot be serialised.
pub fn nachweis_json(n: &NeuerNachweis<'_>) -> Result<serde_json::Value> {
    let i = n.input;
    let rfc3339 = |t: OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .map_err(anyhow::Error::from)
    };
    Ok(serde_json::json!({
        "partner": n.partner,
        "angebot_ref": i.angebot_ref,
        "angezeigte_informationen": i.angezeigte_informationen,
        "button_beschriftung": i.button_beschriftung,
        "dokumente": i.dokumente,
        "belehrung_angezeigt": i.belehrung_angezeigt,
        "wertersatz_info_angezeigt": i.wertersatz_info_angezeigt,
        "lieferung_in_widerrufsfrist_verlangt": i.lieferung_in_widerrufsfrist_verlangt,
        "verbraucher": i.verbraucher,
        "kunde": i.kunde,
        "gewuenschter_lieferbeginn": i.gewuenschter_lieferbeginn.to_string(),
        "bestellt_am": rfc3339(i.bestellt_am)?,
        "eingegangen_am": rfc3339(n.eingegangen_am)?,
        "kanal": i.kanal,
    }))
}

/// Store the Bestellnachweis. Append-only: the schema refuses any later
/// `UPDATE` or `DELETE`.
///
/// # Errors
///
/// Propagates storage and serialisation errors.
pub async fn insert_nachweis(
    conn: &mut sqlx::PgConnection,
    n: &NeuerNachweis<'_>,
) -> Result<GespeicherterNachweis> {
    // Postgres keeps microseconds; the stored instant and the hashed one must
    // be the same instant.
    let eingegangen_am = n
        .eingegangen_am
        .replace_nanosecond(n.eingegangen_am.nanosecond() / 1_000 * 1_000)?;
    let n = NeuerNachweis {
        eingegangen_am,
        ..*n
    };
    let json = nachweis_json(&n)?;
    let (kanonisch, sha256) = bestellung::kanonisch(&json)?;
    let i = n.input;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO bestellnachweise
           (tenant, partner, endkunden_angebot_id, angebot_sha256, angezeigte_informationen,
            angezeigte_informationen_sha256, button_beschriftung, dokumente,
            belehrung_angezeigt, wertersatz_info_angezeigt,
            lieferung_in_widerrufsfrist_verlangt, verlangen_text_sha256, verbraucher, kunde,
            gewuenschter_lieferbeginn, bestellt_am_gemeldet, eingegangen_am, kanal,
            kanonisch, sha256)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
         RETURNING id",
    )
    .bind(n.tenant)
    .bind(n.partner)
    .bind(i.angebot_ref.id)
    .bind(&i.angebot_ref.sha256)
    .bind(serde_json::to_value(&i.angezeigte_informationen)?)
    .bind(
        i.angezeigte_informationen
            .sha256
            .as_deref()
            .unwrap_or_default(),
    )
    .bind(&i.button_beschriftung)
    .bind(serde_json::to_value(&i.dokumente)?)
    .bind(i.belehrung_angezeigt)
    .bind(i.wertersatz_info_angezeigt)
    .bind(i.lieferung_in_widerrufsfrist_verlangt.verlangt)
    .bind(&i.lieferung_in_widerrufsfrist_verlangt.text_sha256)
    .bind(i.verbraucher)
    .bind(serde_json::to_value(&i.kunde)?)
    .bind(i.gewuenschter_lieferbeginn)
    .bind(i.bestellt_am)
    .bind(eingegangen_am)
    .bind(i.kanal.as_db())
    .bind(&kanonisch)
    .bind(&sha256)
    .fetch_one(&mut *conn)
    .await?;
    Ok(GespeicherterNachweis {
        id,
        sha256,
        belehrung_angezeigt: i.belehrung_angezeigt,
        wertersatz_info_angezeigt: i.wertersatz_info_angezeigt,
        lieferung_verlangt: i.lieferung_in_widerrufsfrist_verlangt.verlangt,
        bestellt_am: i.bestellt_am,
    })
}

/// A stored Bestellnachweis, read back.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_nachweis(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    id: Uuid,
) -> Result<Option<GespeicherterNachweis>> {
    let row = sqlx::query(
        "SELECT id, sha256, belehrung_angezeigt, wertersatz_info_angezeigt,
                lieferung_in_widerrufsfrist_verlangt, bestellt_am_gemeldet
           FROM bestellnachweise WHERE tenant = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .fetch_optional(executor)
    .await?;
    row.map(|r| {
        Ok(GespeicherterNachweis {
            id: r.try_get("id")?,
            sha256: r.try_get("sha256")?,
            belehrung_angezeigt: r.try_get("belehrung_angezeigt")?,
            wertersatz_info_angezeigt: r.try_get("wertersatz_info_angezeigt")?,
            lieferung_verlangt: r.try_get("lieferung_in_widerrufsfrist_verlangt")?,
            bestellt_am: r.try_get("bestellt_am_gemeldet")?,
        })
    })
    .transpose()
}

/// The canonical bytes and the stored hash of a Bestellnachweis.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_nachweis_bytes(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
) -> Result<Option<(Vec<u8>, String)>> {
    Ok(sqlx::query_as(
        "SELECT kanonisch, sha256 FROM bestellnachweise WHERE tenant = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

// ── Kunde ─────────────────────────────────────────────────────────────────────

/// The Kunde an order is for, in the order's transaction: the existing one it
/// names, or a new `kunden` row. `None` when it names a Kunde this tenant does
/// not hold.
///
/// An order for an existing Kunde leaves that Kunde's data as it is: the
/// Bestellnachweis records what the order said.
///
/// # Errors
///
/// Propagates storage and serialisation errors.
pub async fn insert_bestellkunde(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    kunde: &BestellKunde,
) -> Result<Option<Uuid>> {
    if let Some(kunden_id) = kunde.kunden_id {
        return Ok(
            sqlx::query_scalar("SELECT id FROM kunden WHERE id = $1 AND tenant = $2")
                .bind(kunden_id)
                .bind(tenant)
                .fetch_optional(&mut *conn)
                .await?,
        );
    }
    let haushaltskunde = kunde.haushaltskunde.unwrap_or(kunde.kundentyp == "B2C");
    Ok(sqlx::query_scalar(
        "INSERT INTO kunden (tenant, kundentyp, haushaltskunde, geschaeftspartner)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant)
    .bind(&kunde.kundentyp)
    .bind(haushaltskunde)
    .bind(kunde.geschaeftspartner.canonical_json()?)
    .fetch_one(&mut *conn)
    .await
    .map(Some)?)
}

// ── Bestellungen ──────────────────────────────────────────────────────────────

/// One order.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BestellungRow {
    pub id: Uuid,
    pub tenant: String,
    pub partner: String,
    pub idempotency_key: String,
    pub request_sha256: String,
    pub bestellreferenz: String,
    pub status: String,
    pub bestellnachweis_id: Uuid,
    pub vertrag_id: Option<Uuid>,
    pub auftrag: serde_json::Value,
    pub angebot: serde_json::Value,
    pub bonitaet_ref: Option<String>,
    pub ablehnungsgrund: Option<String>,
    pub antwort: serde_json::Value,
    pub antwort_status: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

const BESTELLUNG_COLUMNS: &str = "id, tenant, partner, idempotency_key, request_sha256, \
     bestellreferenz, status, bestellnachweis_id, vertrag_id, auftrag, angebot, bonitaet_ref, \
     ablehnungsgrund, antwort, antwort_status, created_at, updated_at";

/// The order a partner posted under `key`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_bestellung_by_key(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    partner: &str,
    key: &str,
) -> Result<Option<BestellungRow>> {
    Ok(sqlx::query_as(mako_service::sql::dynamic(format!(
        "SELECT {BESTELLUNG_COLUMNS} FROM bestellungen
          WHERE tenant = $1 AND partner = $2 AND idempotency_key = $3"
    )))
    .bind(tenant)
    .bind(partner)
    .bind(key)
    .fetch_optional(executor)
    .await?)
}

/// One order by id.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_bestellung(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    id: Uuid,
) -> Result<Option<BestellungRow>> {
    Ok(sqlx::query_as(mako_service::sql::dynamic(format!(
        "SELECT {BESTELLUNG_COLUMNS} FROM bestellungen WHERE tenant = $1 AND id = $2"
    )))
    .bind(tenant)
    .bind(id)
    .fetch_optional(executor)
    .await?)
}

/// The order that concluded contract `vertrag_id`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_bestellung_by_vertrag(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    vertrag_id: Uuid,
) -> Result<Option<BestellungRow>> {
    Ok(sqlx::query_as(mako_service::sql::dynamic(format!(
        "SELECT {BESTELLUNG_COLUMNS} FROM bestellungen WHERE tenant = $1 AND vertrag_id = $2"
    )))
    .bind(tenant)
    .bind(vertrag_id)
    .fetch_optional(executor)
    .await?)
}

/// A new order, claimed under its key.
pub struct NeueBestellung<'a> {
    pub tenant: &'a str,
    pub partner: &'a str,
    pub idempotency_key: &'a str,
    pub request_sha256: &'a str,
    pub bestellreferenz: &'a str,
    pub nachweis: &'a GespeicherterNachweis,
    pub auftrag: serde_json::Value,
    pub angebot: serde_json::Value,
}

/// Claim the key: insert the order `EINGEGANGEN`, or `None` when the key is
/// already taken — a concurrent duplicate waits on the unique index until the
/// first commits, then lands here.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn insert_bestellung(
    conn: &mut sqlx::PgConnection,
    b: NeueBestellung<'_>,
) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "INSERT INTO bestellungen
           (tenant, partner, idempotency_key, request_sha256, bestellreferenz, status,
            bestellnachweis_id, auftrag, angebot, antwort, antwort_status)
         VALUES ($1,$2,$3,$4,$5,'EINGEGANGEN',$6,$7,$8,'{}'::jsonb,0)
         ON CONFLICT (tenant, partner, idempotency_key) DO NOTHING
         RETURNING id",
    )
    .bind(b.tenant)
    .bind(b.partner)
    .bind(b.idempotency_key)
    .bind(b.request_sha256)
    .bind(b.bestellreferenz)
    .bind(b.nachweis.id)
    .bind(b.auftrag)
    .bind(b.angebot)
    .fetch_optional(&mut *conn)
    .await?)
}

/// Record the answer a replay returns.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn set_antwort(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
    antwort: &serde_json::Value,
    status: u16,
) -> Result<()> {
    sqlx::query(
        "UPDATE bestellungen SET antwort = $3, antwort_status = $4, updated_at = now()
          WHERE tenant = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .bind(antwort)
    .bind(i32::from(status))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Mark an order concluded into `vertrag_id`.
///
/// # Errors
///
/// Propagates storage errors; refuses an order that is no longer
/// `EINGEGANGEN`.
pub async fn annehmen(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
    vertrag_id: Uuid,
    bonitaet_ref: Option<&str>,
) -> Result<()> {
    let n = sqlx::query(
        "UPDATE bestellungen
            SET status = 'ANGENOMMEN', vertrag_id = $3, bonitaet_ref = $4, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND status = 'EINGEGANGEN'",
    )
    .bind(tenant)
    .bind(id)
    .bind(vertrag_id)
    .bind(bonitaet_ref)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    anyhow::ensure!(n == 1, "Bestellung {id} is not EINGEGANGEN");
    Ok(())
}

/// Close an order without a contract: `ABGELEHNT` by the operator or
/// `WIDERRUFEN` by the consumer. `false` when it is no longer `EINGEGANGEN`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn schliessen(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
    status: &str,
    grund: Option<&str>,
) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE bestellungen
            SET status = $3, ablehnungsgrund = COALESCE($4, ablehnungsgrund), updated_at = now()
          WHERE tenant = $1 AND id = $2 AND status = 'EINGEGANGEN'
            AND $3 IN ('ABGELEHNT','WIDERRUFEN')",
    )
    .bind(tenant)
    .bind(id)
    .bind(status)
    .bind(grund)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1)
}

// ── Widerrufe ─────────────────────────────────────────────────────────────────

/// One recorded Widerruf.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WiderrufRow {
    pub id: Uuid,
    pub vertrag_id: Option<Uuid>,
    pub bestellung_id: Option<Uuid>,
    pub kanal: String,
    #[serde(with = "time::serde::rfc3339")]
    pub abgesendet_am: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub eingegangen_am: OffsetDateTime,
    pub erklaerung: serde_json::Value,
    pub frist_beginn: Option<Date>,
    pub frist_ende: Option<Date>,
    pub erloeschen: Option<Date>,
    pub rechtzeitig: bool,
    pub wirkung: Option<String>,
    pub lieferende: Option<Date>,
    pub wertersatz_geschuldet: Option<bool>,
}

const WIDERRUF_COLUMNS: &str = "id, vertrag_id, bestellung_id, kanal, abgesendet_am, \
     eingegangen_am, erklaerung, frist_beginn, frist_ende, erloeschen, rechtzeitig, wirkung, \
     lieferende, wertersatz_geschuldet";

/// What the Widerruf concerns.
#[derive(Debug, Clone, Copy)]
pub enum WiderrufGegenstand {
    Vertrag(Uuid),
    Bestellung(Uuid),
}

/// A Widerruf to record.
pub struct NeuerWiderruf<'a> {
    pub tenant: &'a str,
    pub gegenstand: WiderrufGegenstand,
    pub kanal: &'a str,
    pub abgesendet_am: OffsetDateTime,
    pub eingegangen_am: OffsetDateTime,
    pub erklaerung: serde_json::Value,
    /// `None` for an order not yet accepted — no Frist runs against it.
    pub frist: Option<mako_fristen::vorlauf::Widerrufsfrist>,
    pub rechtzeitig: bool,
    pub wirkung: Option<&'a str>,
    pub lieferende: Option<Date>,
    pub wertersatz_geschuldet: Option<bool>,
}

/// The timely Widerruf already recorded for `gegenstand`, if any — a second
/// one is answered with it.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_rechtzeitiger_widerruf(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    gegenstand: WiderrufGegenstand,
) -> Result<Option<WiderrufRow>> {
    let (vertrag, bestellung) = match gegenstand {
        WiderrufGegenstand::Vertrag(v) => (Some(v), None),
        WiderrufGegenstand::Bestellung(b) => (None, Some(b)),
    };
    Ok(sqlx::query_as(mako_service::sql::dynamic(format!(
        "SELECT {WIDERRUF_COLUMNS} FROM widerrufe
          WHERE tenant = $1 AND rechtzeitig
            AND vertrag_id IS NOT DISTINCT FROM $2 AND bestellung_id IS NOT DISTINCT FROM $3"
    )))
    .bind(tenant)
    .bind(vertrag)
    .bind(bestellung)
    .fetch_optional(executor)
    .await?)
}

/// Record a Widerruf. Append-only.
///
/// # Errors
///
/// Propagates storage errors — among them the unique violation of a second
/// timely Widerruf of the same contract.
pub async fn insert_widerruf(
    conn: &mut sqlx::PgConnection,
    w: NeuerWiderruf<'_>,
) -> Result<WiderrufRow> {
    let (vertrag, bestellung) = match w.gegenstand {
        WiderrufGegenstand::Vertrag(v) => (Some(v), None),
        WiderrufGegenstand::Bestellung(b) => (None, Some(b)),
    };
    Ok(sqlx::query_as(mako_service::sql::dynamic(format!(
        "INSERT INTO widerrufe
           (tenant, vertrag_id, bestellung_id, kanal, abgesendet_am, eingegangen_am, erklaerung,
            frist_beginn, frist_ende, erloeschen, rechtzeitig, wirkung, lieferende,
            wertersatz_geschuldet)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         RETURNING {WIDERRUF_COLUMNS}"
    )))
    .bind(w.tenant)
    .bind(vertrag)
    .bind(bestellung)
    .bind(w.kanal)
    .bind(w.abgesendet_am)
    .bind(w.eingegangen_am)
    .bind(w.erklaerung)
    .bind(w.frist.and_then(|f| f.beginn))
    .bind(w.frist.map(|f| f.ende))
    .bind(w.frist.map(|f| f.erloeschen))
    .bind(w.rechtzeitig)
    .bind(w.wirkung)
    .bind(w.lieferende)
    .bind(w.wertersatz_geschuldet)
    .fetch_one(&mut *conn)
    .await?)
}

/// Set a withdrawn contract `WIDERRUFEN` and close its supply bookkeeping.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn vertrag_widerrufen(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    vertrag_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege
            SET status = 'WIDERRUFEN', auto_renewal = false, updated_at = now()
          WHERE tenant = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(vertrag_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE bestellungen SET status = 'WIDERRUFEN', updated_at = now()
          WHERE tenant = $1 AND vertrag_id = $2",
    )
    .bind(tenant)
    .bind(vertrag_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
