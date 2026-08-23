//! Valid-time product assignment per Vertragskomponente.
//!
//! Which product a supply component is on is a **contract** fact: agreeing it is
//! a Tarifwechsel, governed by § 41 Abs. 5 EnWG and the contract's
//! Preisgarantie. It is decided here, so it is stored here — once.
//!
//! Ranges are half-open, `[gueltig_von, gueltig_bis)`: the end is the first day
//! **not** covered, so consecutive slices tile a billing period exactly and no
//! day belongs to two products.
//!
//! A future-dated Tarifwechsel is simply a slice that starts in the future —
//! there is no pending state and nothing applies it on the day.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

/// One valid-time slice of a component's product assignment.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProduktSlice {
    pub id: Uuid,
    pub komp_id: Uuid,
    pub product_code: String,
    pub gueltig_von: Date,
    /// Exclusive; `None` = open-ended.
    pub gueltig_bis: Option<Date>,
    pub preisanpassung_notif_sent: bool,
    pub grund: Option<String>,
}

const SLICE_COLS: &str =
    "id, komp_id, product_code, gueltig_von, gueltig_bis, preisanpassung_notif_sent, grund";

/// Open the first slice of a component, in the caller's transaction.
///
/// Created with the § 41 Abs. 5 notice already marked sent: the customer agreed
/// to the initial product when they signed. Only a later change owes a notice.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn open_initial(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    komp_id: Uuid,
    product_code: &str,
    ab: Date,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO komponenten_produkte
             (tenant, komp_id, product_code, gueltig_von, preisanpassung_notif_sent, grund)
         VALUES ($1, $2, $3, $4, true, 'Vertragsschluss')",
    )
    .bind(tenant)
    .bind(komp_id)
    .bind(product_code)
    .bind(ab)
    .execute(executor)
    .await?;
    Ok(())
}

/// Record a Tarifwechsel: close the slice it supersedes and open the new one.
///
/// Idempotent on `(komp_id, ab)`. A change dated **behind** a later slice is
/// refused: it would reprice a period already decided, and possibly announced.
///
/// `notif_sent` is `false` for a future change — the § 41 Abs. 5 notice is then
/// owed — and `true` for a retroactive correction, which announces nothing.
///
/// # Errors
///
/// Refuses a change behind a later slice; otherwise propagates storage errors.
#[allow(clippy::too_many_arguments)]
pub async fn tarifwechsel(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    komp_id: Uuid,
    product_code: &str,
    ab: Date,
    grund: Option<&str>,
    notif_sent: bool,
    // § 41 Abs. 5 Satz 1 EnWG — the announced price lines, as the notice
    // states them. `None` leaves whatever the slice already carried.
    preise: Option<&serde_json::Value>,
) -> Result<()> {
    let spaeter: Option<(Date,)> = sqlx::query_as(
        "SELECT gueltig_von FROM komponenten_produkte
          WHERE komp_id = $1 AND gueltig_von > $2
          ORDER BY gueltig_von LIMIT 1",
    )
    .bind(komp_id)
    .bind(ab)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((spaeter,)) = spaeter {
        anyhow::bail!(
            "ein späterer Tarifwechsel beginnt bereits am {spaeter}; \
             er ist zuerst aufzuheben"
        );
    }

    // Half-open: the new slice's first day is the old one's first uncovered day.
    sqlx::query(
        "UPDATE komponenten_produkte
            SET gueltig_bis = $2, updated_at = now()
          WHERE komp_id = $1 AND gueltig_von < $2
            AND (gueltig_bis IS NULL OR gueltig_bis > $2)",
    )
    .bind(komp_id)
    .bind(ab)
    .execute(&mut *conn)
    .await?;

    // Replay first, insert second: a genuine overlap then surfaces as the
    // constraint violation it is rather than being swallowed.
    let updated = sqlx::query(
        "UPDATE komponenten_produkte
            SET product_code = $3, grund = COALESCE($4, grund),
                preisanpassung_notif_sent = $5,
                angekuendigte_preise = COALESCE($6, angekuendigte_preise),
                gueltig_bis = NULL, updated_at = now()
          WHERE komp_id = $1 AND gueltig_von = $2",
    )
    .bind(komp_id)
    .bind(ab)
    .bind(product_code)
    .bind(grund)
    .bind(notif_sent)
    .bind(preise)
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if updated == 0 {
        sqlx::query(
            "INSERT INTO komponenten_produkte
                 (tenant, komp_id, product_code, gueltig_von, preisanpassung_notif_sent,
                  grund, angekuendigte_preise)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(tenant)
        .bind(komp_id)
        .bind(product_code)
        .bind(ab)
        .bind(notif_sent)
        .bind(grund)
        .bind(preise)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The product a component is on at `am`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn produkt_am(
    executor: impl sqlx::PgExecutor<'_>,
    komp_id: Uuid,
    am: Date,
) -> Result<Option<ProduktSlice>> {
    Ok(sqlx::query_as::<_, ProduktSlice>(&format!(
        "SELECT {SLICE_COLS} FROM komponenten_produkte
          WHERE komp_id = $1 AND gueltig_von <= $2
            AND (gueltig_bis IS NULL OR gueltig_bis > $2)
          LIMIT 1"
    ))
    .bind(komp_id)
    .bind(am)
    .fetch_optional(executor)
    .await?)
}

/// Every slice of a component, newest first — the Tarifwechsel history.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn historie(pool: &PgPool, komp_id: Uuid) -> Result<Vec<ProduktSlice>> {
    Ok(sqlx::query_as::<_, ProduktSlice>(&format!(
        "SELECT {SLICE_COLS} FROM komponenten_produkte
          WHERE komp_id = $1 ORDER BY gueltig_von DESC"
    ))
    .bind(komp_id)
    .fetch_all(pool)
    .await?)
}

/// One product slice as a billing reader sees it, clipped to the period asked
/// about and carrying the market location it belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MaloProduktSlice {
    pub malo_id: String,
    pub lf_mp_id: String,
    pub sparte: String,
    pub product_code: String,
    pub gueltig_von: Date,
    /// Exclusive; `None` when the slice runs past the period asked about.
    pub gueltig_bis: Option<Date>,
}

/// The product slices covering `[von, bis]` for a MaLo, clipped and ordered.
///
/// What `billingd` bills from: an invoice covers a period, and a Tarifwechsel
/// inside it splits that period into legs.
///
/// # Errors
///
/// Propagates storage errors; refuses a reversed period.
pub async fn malo_slices(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    von: Date,
    bis: Date,
) -> Result<Vec<MaloProduktSlice>> {
    anyhow::ensure!(von <= bis, "von ({von}) darf nicht nach bis ({bis}) liegen");
    // `bis` is inclusive for the caller and exclusive in the range algebra.
    let bis_exkl = bis.next_day().unwrap_or(bis);
    Ok(sqlx::query_as::<_, MaloProduktSlice>(
        r"SELECT k.malo_id, k.lf_mp_id, k.sparte, p.product_code,
                 GREATEST(p.gueltig_von, $3) AS gueltig_von,
                 CASE WHEN p.gueltig_bis IS NULL OR p.gueltig_bis > $4
                      THEN $4 ELSE p.gueltig_bis END AS gueltig_bis
          FROM komponenten_produkte p
          JOIN vertragskomponenten k ON k.id = p.komp_id
          WHERE p.tenant = $1 AND k.malo_id = $2
            AND daterange(p.gueltig_von, p.gueltig_bis, '[)')
                && daterange($3::DATE, $4::DATE, '[)')
          ORDER BY p.gueltig_von",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(von)
    .bind(bis_exkl)
    .fetch_all(pool)
    .await?)
}

/// A future slice whose § 41 Abs. 5 EnWG price-change notice is still owed.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AnzupassenderPreis {
    pub slice_id: Uuid,
    pub komp_id: Uuid,
    pub vertrag_id: Uuid,
    pub kunden_id: Uuid,
    pub malo_id: Option<String>,
    pub sparte: String,
    pub neues_produkt: String,
    pub bisheriges_produkt: Option<String>,
    pub wirksam_ab: Date,
    pub vertragsart: String,
    pub haushaltskunde: bool,
    /// The contract number as the customer knows it, for the notice.
    pub vertrags_nr: String,
    /// § 41 Abs. 5 Satz 1 EnWG — the announced price lines, as scheduled.
    /// `None` means the change was scheduled without them; see the column
    /// comment on `komponenten_produkte.angekuendigte_preise`.
    pub angekuendigte_preise: Option<serde_json::Value>,
    pub grund: Option<String>,
}

/// Slices taking effect after `heute` whose notice has not gone out.
///
/// Deliberately unwindowed: § 41 Abs. 5 Satz 1 EnWG wants the notice
/// *rechtzeitig* and Satz 2 sets a floor, not a ceiling, so it goes out as soon
/// as the change is scheduled.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn offene_preisanpassungen(
    pool: &PgPool,
    tenant: &str,
    heute: Date,
) -> Result<Vec<AnzupassenderPreis>> {
    Ok(sqlx::query_as::<_, AnzupassenderPreis>(
        r"SELECT p.id AS slice_id, p.komp_id, k.vertrag_id, v.kunden_id,
                 k.malo_id, k.sparte, v.vertrags_nr,
                 p.angekuendigte_preise, p.grund,
                 p.product_code AS neues_produkt,
                 (SELECT vor.product_code FROM komponenten_produkte vor
                   WHERE vor.komp_id = p.komp_id AND vor.gueltig_von < p.gueltig_von
                   ORDER BY vor.gueltig_von DESC LIMIT 1) AS bisheriges_produkt,
                 p.gueltig_von AS wirksam_ab,
                 v.vertragsart, ku.haushaltskunde
          FROM komponenten_produkte p
          JOIN vertragskomponenten k   ON k.id  = p.komp_id
          JOIN versorgungsvertraege v  ON v.id  = k.vertrag_id
          JOIN kunden ku               ON ku.id = v.kunden_id
          WHERE p.tenant = $1
            AND p.preisanpassung_notif_sent = false
            AND p.gueltig_von > $2
          ORDER BY p.gueltig_von",
    )
    .bind(tenant)
    .bind(heute)
    .fetch_all(pool)
    .await?)
}

/// Mark a slice's price-change notice as dispatched.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn mark_notif_sent(executor: impl sqlx::PgExecutor<'_>, slice_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE komponenten_produkte
            SET preisanpassung_notif_sent = true, updated_at = now()
          WHERE id = $1",
    )
    .bind(slice_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Stamp the `outputd` document that communicated a price change.
///
/// Written after outputd has recorded the document, so a crash between the two
/// leaves the slice undocumented and the worker retries — safe because outputd
/// keys the document on the slice id and answers the second call with the
/// first document.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn mark_dokument(
    executor: impl sqlx::PgExecutor<'_>,
    slice_id: Uuid,
    dokument_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE komponenten_produkte
            SET dokument_id = $2, dokument_issued_at = now(), updated_at = now()
          WHERE id = $1 AND dokument_id IS NULL",
    )
    .bind(slice_id)
    .bind(dokument_id)
    .execute(executor)
    .await?;
    Ok(())
}
