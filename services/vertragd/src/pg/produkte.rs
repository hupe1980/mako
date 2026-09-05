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

/// Who brought a product slice about.
///
/// § 41 Abs. 5 Satz 1 EnWG obliges a supplier who reserved the right to change
/// the contract unilaterally to give notice of the *beabsichtigte Ausübung*
/// of that right, and Satz 4 gives the customer a termination right *"Übt der
/// Energielieferant ein Recht zur Änderung der Preise … aus"*. Both hang on the
/// supplier exercising a right — a tariff the customer asked for is an agreed
/// change, owes no announcement, and carries no Sonderkündigungsrecht.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Initiator {
    /// The supplier changes the price; § 41 Abs. 5 EnWG applies in full.
    Lieferant,
    /// The customer asked for this tariff; the change is confirmed, not
    /// announced.
    Kunde,
}

impl Initiator {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Lieferant => "LIEFERANT",
            Self::Kunde => "KUNDE",
        }
    }

    /// Read the stored value. Anything unrecognised reads as the stricter case,
    /// so an unexpected value owes the notice rather than escaping it.
    #[must_use]
    pub fn from_db(s: &str) -> Self {
        match s {
            "KUNDE" => Self::Kunde,
            _ => Self::Lieferant,
        }
    }

    /// Whether this slice owes a § 41 Abs. 5 EnWG Preisänderungsanzeige.
    #[must_use]
    pub const fn owes_preisanpassungsanzeige(self) -> bool {
        matches!(self, Self::Lieferant)
    }
}

/// One valid-time slice of a component's product assignment.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProduktSlice {
    pub id: Uuid,
    pub komp_id: Uuid,
    pub product_code: String,
    pub gueltig_von: Date,
    /// Exclusive; `None` = open-ended.
    pub gueltig_bis: Option<Date>,
    /// `LIEFERANT` or `KUNDE` — see [`Initiator`].
    pub initiator: String,
    pub preisanpassung_notif_sent: bool,
    pub grund: Option<String>,
}

const SLICE_COLS: &str = "id, komp_id, product_code, gueltig_von, gueltig_bis, initiator, \
                          preisanpassung_notif_sent, grund";

/// Open the first slice of a component, in the caller's transaction.
///
/// The customer chose this product when they signed, so the slice is
/// customer-initiated and no § 41 Abs. 5 notice is owed for it — which is why
/// it is created with the obligation already settled. Only a later change the
/// supplier makes owes one.
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
             (tenant, komp_id, product_code, gueltig_von, initiator,
              preisanpassung_notif_sent, grund)
         VALUES ($1, $2, $3, $4, 'KUNDE', true, 'Vertragsschluss')",
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
/// `notif_sent` says whether the § 41 Abs. 5 obligation is already settled:
/// `false` only for a future supplier-initiated change, where the notice is
/// owed and the daily sweep issues it. A retroactive correction announces a
/// date that has passed, and a change the customer asked for is not an exercise
/// of a change right — neither owes anything.
///
/// ## The obligation survives a replay
///
/// A slice whose notice is still owed carries the customer's § 41 Abs. 5 Satz 4
/// Sonderkündigungsrecht, and `initiator` is what says the notice is owed at
/// all. So a re-POST at the same `gueltig_von` may not change it while the
/// notice is outstanding: re-writing a pending `LIEFERANT` change as `KUNDE`
/// would take the row out of [`offene_preisanpassungen`], the customer would
/// never receive the Preisänderungsanzeige, and the breach report
/// [`unangekuendigt_wirksame`] would come back clean because the record — not
/// the fact — had been changed. That replay is refused.
///
/// A replay that keeps the initiator is the ordinary idempotent case and
/// updates the slice, including re-opening the obligation when a supplier
/// changes the product or the announced prices of a change already announced.
///
/// # Errors
///
/// Refuses a change behind a later slice, and a replay that flips the initiator
/// of a slice whose notice is still owed; otherwise propagates storage errors.
#[allow(clippy::too_many_arguments)]
pub async fn tarifwechsel(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    komp_id: Uuid,
    product_code: &str,
    ab: Date,
    grund: Option<&str>,
    initiator: Initiator,
    notif_sent: bool,
    // § 41 Abs. 5 Satz 3 EnWG — the announced price lines, as the notice
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

    // The slice this write replays, if there is one.
    let bestehend: Option<(String, bool)> = sqlx::query_as(
        "SELECT initiator, preisanpassung_notif_sent FROM komponenten_produkte
          WHERE komp_id = $1 AND gueltig_von = $2",
    )
    .bind(komp_id)
    .bind(ab)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((bestehender, anzeige_erledigt)) = &bestehend {
        let bestehender = Initiator::from_db(bestehender);
        if !anzeige_erledigt && bestehender != initiator {
            anyhow::bail!(
                "der Tarifwechsel zum {ab} ist als {} angelegt und seine \
                 Preisänderungsanzeige nach § 41 Abs. 5 Satz 1 EnWG steht noch aus; \
                 er kann nicht als {} überschrieben werden — der Kunde verlöre Anzeige \
                 und Sonderkündigungsrecht",
                bestehender.as_db(),
                initiator.as_db()
            );
        }
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
                initiator = $5,
                preisanpassung_notif_sent = $6,
                angekuendigte_preise = COALESCE($7, angekuendigte_preise),
                gueltig_bis = NULL, updated_at = now()
          WHERE komp_id = $1 AND gueltig_von = $2",
    )
    .bind(komp_id)
    .bind(ab)
    .bind(product_code)
    .bind(grund)
    .bind(initiator.as_db())
    .bind(notif_sent)
    .bind(preise)
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if updated == 0 {
        sqlx::query(
            "INSERT INTO komponenten_produkte
                 (tenant, komp_id, product_code, gueltig_von, initiator,
                  preisanpassung_notif_sent, grund, angekuendigte_preise)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(tenant)
        .bind(komp_id)
        .bind(product_code)
        .bind(ab)
        .bind(initiator.as_db())
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
    /// § 41 Abs. 5 Satz 3 EnWG — the announced price lines, as scheduled.
    /// `None` where the change was scheduled without them: the notice is then
    /// the CloudEvent, and the ERP composing the letter states the Umfang from
    /// its own price sheets. A rendered document cannot be built from `None`.
    pub angekuendigte_preise: Option<serde_json::Value>,
    /// How often issuing the notice has already failed, and why the last
    /// attempt did.
    pub notif_versuche: i32,
    pub notif_letzter_fehler: Option<String>,
    pub grund: Option<String>,
}

/// Every supplier-initiated slice whose § 41 Abs. 5 notice is still owed,
/// selected by where its Wirksamkeit lies relative to `heute`.
///
/// `>` is the sweep that announces; `<=` is the breach report — a price change
/// in force that the customer was never told about.
const OFFENE_SQL: &str = r"
    SELECT p.id AS slice_id, p.komp_id, k.vertrag_id, v.kunden_id,
           k.malo_id, k.sparte, v.vertrags_nr,
           p.angekuendigte_preise, p.notif_versuche, p.notif_letzter_fehler, p.grund,
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
      AND p.initiator = 'LIEFERANT'
      AND p.preisanpassung_notif_sent = false
      AND p.gueltig_von ";

/// Slices taking effect after `heute` whose notice has not gone out.
///
/// Deliberately unwindowed: § 41 Abs. 5 Satz 1 EnWG wants the notice
/// *rechtzeitig* and Satz 2 sets a floor, not a ceiling, so it goes out as soon
/// as the change is scheduled. Only supplier-initiated slices are here — a
/// switch the customer asked for is not an exercise of a change right and owes
/// neither the notice nor the Sonderkündigungsrecht it carries.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn offene_preisanpassungen(
    pool: &PgPool,
    tenant: &str,
    heute: Date,
) -> Result<Vec<AnzupassenderPreis>> {
    Ok(sqlx::query_as::<_, AnzupassenderPreis>(&format!(
        "{OFFENE_SQL} > $2 ORDER BY p.gueltig_von"
    ))
    .bind(tenant)
    .bind(heute)
    .fetch_all(pool)
    .await?)
}

/// Supplier-initiated slices already in force whose notice never went out.
///
/// A price change the customer was never validly told about took effect anyway:
/// § 41 Abs. 5 Satz 1 EnWG was not complied with and the customer never had the
/// Satz 4 Sonderkündigungsrecht. Nothing here can undo that, so the sweep
/// reports it every day until an operator resolves it.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn unangekuendigt_wirksame(
    pool: &PgPool,
    tenant: &str,
    heute: Date,
) -> Result<Vec<AnzupassenderPreis>> {
    Ok(sqlx::query_as::<_, AnzupassenderPreis>(&format!(
        "{OFFENE_SQL} <= $2 ORDER BY p.gueltig_von"
    ))
    .bind(tenant)
    .bind(heute)
    .fetch_all(pool)
    .await?)
}

/// Mark a slice's price-change notice as dispatched.
///
/// Written in the same transaction as the notice itself, and only once one has
/// actually been issued.
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

/// Record why the § 41 Abs. 5 notice for a slice could not be issued.
///
/// The slice stays unnotified, so the daily sweep tries again; what is stored
/// is why the last attempt produced nothing, so "the customer never got their
/// Preisänderungsanzeige" is answerable from the data rather than from logs.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn record_notif_failure(
    executor: impl sqlx::PgExecutor<'_>,
    slice_id: Uuid,
    fehler: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE komponenten_produkte
            SET notif_versuche = notif_versuche + 1,
                notif_letzter_fehler = $2,
                notif_letzter_versuch = now(),
                updated_at = now()
          WHERE id = $1",
    )
    .bind(slice_id)
    .bind(fehler)
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
