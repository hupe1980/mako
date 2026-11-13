//! What a person states for the Prüfschritte no INVOIC answers.
//!
//! Two stores, by what the statement is about:
//!
//! - **`leistungsnachweise`** — a fact about the order: the MSB performed (or
//!   did not perform) the Leistung for an Artikel-ID over a period. It answers
//!   `E_0264`/`E_0270`/`E_0273` Prüfschritt 310 for every position it covers,
//!   whichever invoice bills it.
//! - **`invoic_receipts.feststellungen`** — a judgement about one held
//!   invoice: Prüfschritte 310, 320 and 330 per Positionsnummer, and 80. It
//!   answers that invoice only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// The answers a person states for one held invoice — each keyed by the
/// `SG26 LIN` Positionsnummer it answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Feststellungen {
    /// Prüfschritt 310 — did the MSB perform the billed Leistung?
    #[serde(default)]
    pub leistung_erbracht: BTreeMap<u16, bool>,
    /// Prüfschritt 320 — does the price match the offer valid for the
    /// position? Read only where no offer on record prices it.
    #[serde(default)]
    pub preis_wie_angebot: BTreeMap<u16, bool>,
    /// Prüfschritt 330 — is the Umsatzsteuersatz the one valid for the
    /// period? Read only where the INVOIC does not settle it.
    #[serde(default)]
    pub steuersatz_korrekt: BTreeMap<u16, bool>,
    /// Prüfschritt 80 (Preisblatt B) — does the recipient hold the Preisblatt
    /// version the invoice bills against?
    #[serde(default)]
    pub preisblatt_version_gueltig: Option<bool>,
    /// Second round, Prüfschritt 1 (`E_0266`/`E_0276`/`E_0277`) — could the
    /// MSB's COMDIS 29001 rebut every objection? A judgement of the COMDIS;
    /// `false` refuses with `A25` resp. `AC1`.
    #[serde(default)]
    pub einwaende_entkraeftet: Option<bool>,
}

impl Feststellungen {
    /// `other`'s answers over `self`'s: a later statement for the same step
    /// replaces the earlier one.
    #[must_use]
    pub fn ergaenzt(mut self, other: &Self) -> Self {
        self.leistung_erbracht.extend(&other.leistung_erbracht);
        self.preis_wie_angebot.extend(&other.preis_wie_angebot);
        self.steuersatz_korrekt.extend(&other.steuersatz_korrekt);
        if other.preisblatt_version_gueltig.is_some() {
            self.preisblatt_version_gueltig = other.preisblatt_version_gueltig;
        }
        if other.einwaende_entkraeftet.is_some() {
            self.einwaende_entkraeftet = other.einwaende_entkraeftet;
        }
        self
    }

    /// `true` when nothing is stated.
    #[must_use]
    pub fn ist_leer(&self) -> bool {
        self.leistung_erbracht.is_empty()
            && self.preis_wie_angebot.is_empty()
            && self.steuersatz_korrekt.is_empty()
            && self.preisblatt_version_gueltig.is_none()
            && self.einwaende_entkraeftet.is_none()
    }
}

/// One recorded Leistungsnachweis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Leistungsnachweis {
    /// The MSB the Leistung was ordered from — `NAD+MS` of its INVOIC.
    pub rechnungssteller_mp_id: String,
    /// The order — `SG1 RFF+ACE` of the INVOIC.
    pub bestellung_ref: String,
    pub artikel_id: String,
    /// First day the statement covers.
    #[schemars(with = "String")]
    pub von: Date,
    /// Last day the statement covers (inclusive).
    #[schemars(with = "String")]
    pub bis: Date,
    /// Whether the MSB performed the Leistung over the whole period.
    pub erbracht: bool,
    /// What the statement rests on — the values received, the change made.
    pub nachweis: String,
}

/// Record a Leistungsnachweis; a second one for the same order, Artikel-ID and
/// period replaces it.
///
/// # Errors
///
/// Propagates database errors, among them the `CHECK`s on an empty `nachweis`
/// and a period ending before it starts.
pub async fn record_leistungsnachweis(
    pool: &PgPool,
    tenant: &str,
    n: &Leistungsnachweis,
    von_wem: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO leistungsnachweise
           (tenant, rechnungssteller_mp_id, bestellung_ref, artikel_id, von, bis,
            erbracht, nachweis, festgestellt_von)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (tenant, rechnungssteller_mp_id, bestellung_ref, artikel_id, von, bis)
         DO UPDATE SET erbracht = EXCLUDED.erbracht, nachweis = EXCLUDED.nachweis,
                       festgestellt_von = EXCLUDED.festgestellt_von, festgestellt_am = now()",
    )
    .bind(tenant)
    .bind(&n.rechnungssteller_mp_id)
    .bind(&n.bestellung_ref)
    .bind(&n.artikel_id)
    .bind(n.von)
    .bind(n.bis)
    .bind(n.erbracht)
    .bind(&n.nachweis)
    .bind(von_wem)
    .execute(pool)
    .await?;
    Ok(())
}

/// Prüfschritt 310 for one position: what the Leistungsnachweise covering
/// `[von, bis]` state. `None` when none covers it, and when covering ones
/// disagree — a contradiction answers neither way.
///
/// # Errors
///
/// Propagates database errors.
pub async fn leistung_erbracht(
    pool: &PgPool,
    tenant: &str,
    rechnungssteller_mp_id: &str,
    bestellung_ref: &str,
    artikel_id: &str,
    von: Date,
    bis: Date,
) -> Result<Option<bool>, sqlx::Error> {
    let (alle, eine): (Option<bool>, Option<bool>) = sqlx::query_as(
        "SELECT bool_and(erbracht), bool_or(erbracht) FROM leistungsnachweise
          WHERE tenant = $1 AND rechnungssteller_mp_id = $2 AND bestellung_ref = $3
            AND artikel_id = $4 AND von <= $5 AND bis >= $6",
    )
    .bind(tenant)
    .bind(rechnungssteller_mp_id)
    .bind(bestellung_ref)
    .bind(artikel_id)
    .bind(von)
    .bind(bis)
    .fetch_one(pool)
    .await?;
    Ok(match (alle, eine) {
        (Some(a), Some(e)) if a == e => Some(a),
        _ => None,
    })
}

/// The statements recorded for the receipt of `process_id`.
///
/// # Errors
///
/// Propagates database errors; a stored value that does not read back is one.
pub async fn feststellungen_of(
    pool: &PgPool,
    tenant: &str,
    process_id: Uuid,
) -> Result<Feststellungen, sqlx::Error> {
    let v: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT feststellungen FROM invoic_receipts WHERE tenant = $1 AND process_id = $2",
    )
    .bind(tenant)
    .bind(process_id)
    .fetch_optional(pool)
    .await?;
    v.map_or(Ok(Feststellungen::default()), |v| {
        serde_json::from_value(v).map_err(|e| sqlx::Error::Decode(Box::new(e)))
    })
}

/// A held receipt as the walk needs it to run again.
#[derive(Debug)]
pub struct GehalteneRechnung {
    pub process_id: Uuid,
    pub pid: i16,
    /// The `process.initiated` payload the receipt was checked from.
    pub data: serde_json::Value,
}

macro_rules! gehalten_select {
    () => {
        "SELECT process_id, pid, invoice_ref, sender_mp_id, receiver_gln,
                malo_id, rechnung, bestellung_ref, rechnungstyp
           FROM invoic_receipts
          WHERE tenant = $1 AND outcome = 'Offen' AND dispatched_at IS NULL"
    };
}

fn gehalten(r: &sqlx::postgres::PgRow) -> Result<GehalteneRechnung, sqlx::Error> {
    use sqlx::Row as _;
    Ok(GehalteneRechnung {
        process_id: r.try_get("process_id")?,
        pid: r.try_get("pid")?,
        data: serde_json::json!({
            "invoice_ref":    r.try_get::<String, _>("invoice_ref")?,
            "sender_mp_id":   r.try_get::<String, _>("sender_mp_id")?,
            "receiver_gln":   r.try_get::<Option<String>, _>("receiver_gln")?,
            "malo_id":        r.try_get::<Option<String>, _>("malo_id")?,
            "rechnung":       r.try_get::<serde_json::Value, _>("rechnung")?,
            "bestellung_ref": r.try_get::<Option<String>, _>("bestellung_ref")?,
            "rechnungstyp":   r.try_get::<Option<String>, _>("rechnungstyp")?,
        }),
    })
}

/// The held receipt `id`, `None` when there is none or it is no longer held.
///
/// # Errors
///
/// Propagates database errors.
pub async fn gehaltene_rechnung(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
) -> Result<Option<GehalteneRechnung>, sqlx::Error> {
    sqlx::query(concat!(gehalten_select!(), " AND id = $2"))
        .bind(tenant)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .as_ref()
        .map(gehalten)
        .transpose()
}

/// Every held receipt of `rechnungssteller_mp_id` billing against
/// `bestellung_ref` — what a new Leistungsnachweis may decide.
///
/// # Errors
///
/// Propagates database errors.
pub async fn gehalten_fuer_bestellung(
    pool: &PgPool,
    tenant: &str,
    rechnungssteller_mp_id: &str,
    bestellung_ref: &str,
) -> Result<Vec<GehalteneRechnung>, sqlx::Error> {
    sqlx::query(concat!(
        gehalten_select!(),
        " AND sender_mp_id = $2 AND bestellung_ref = $3"
    ))
    .bind(tenant)
    .bind(rechnungssteller_mp_id)
    .bind(bestellung_ref)
    .fetch_all(pool)
    .await?
    .iter()
    .map(gehalten)
    .collect()
}

/// Merge `neu` into the held receipt `id`'s statements, recording who stated
/// them, when and why. `false` when the receipt is not held (any more).
///
/// # Errors
///
/// Propagates database errors.
pub async fn feststellen(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    neu: &Feststellungen,
    begruendung: &str,
    von_wem: &str,
    am: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let alt: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT feststellungen FROM invoic_receipts
          WHERE tenant = $1 AND id = $2 AND outcome = 'Offen' AND dispatched_at IS NULL
          FOR UPDATE",
    )
    .bind(tenant)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(alt) = alt else {
        return Ok(false);
    };
    let alt: Feststellungen =
        serde_json::from_value(alt).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    let merged =
        serde_json::to_value(alt.ergaenzt(neu)).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "UPDATE invoic_receipts
            SET feststellungen = $3, festgestellt_von = $4, festgestellt_am = $5,
                feststellung_begruendung = $6
          WHERE tenant = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .bind(merged)
    .bind(von_wem)
    .bind(am)
    .bind(begruendung)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// The outcome recorded for the receipt of `process_id`.
///
/// # Errors
///
/// Propagates database errors.
pub async fn outcome_of(
    pool: &PgPool,
    tenant: &str,
    process_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT outcome FROM invoic_receipts WHERE tenant = $1 AND process_id = $2")
        .bind(tenant)
        .bind(process_id)
        .fetch_optional(pool)
        .await
}

/// The round of the receipt of `process_id` — 1 when there is none yet.
///
/// # Errors
///
/// Propagates database errors.
pub async fn runde_of(pool: &PgPool, tenant: &str, process_id: Uuid) -> Result<i16, sqlx::Error> {
    Ok(sqlx::query_scalar(
        "SELECT runde FROM invoic_receipts WHERE tenant = $1 AND process_id = $2",
    )
    .bind(tenant)
    .bind(process_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(1))
}

/// What a COMDIS 29001 found on the receipt it answers.
#[derive(Debug)]
pub enum ZweiteRunde {
    /// Round 2 is open (now, or by an earlier delivery of the same COMDIS)
    /// and unanswered: walk it.
    Offen(GehalteneRechnung),
    /// Round 2 was answered already; a redelivery changes nothing.
    Beantwortet,
    /// No receipt for the process.
    KeinBeleg,
    /// The receipt is not a refusal that went out — a COMDIS 29001 refuses
    /// our Nicht-Zahlungsavis, and there is none.
    KeineAbweisung(String),
}

/// Open the second round on the receipt of `process_id`: keep round 1's
/// answer and dispatch time, record the COMDIS, and make the receipt
/// unanswered again.
///
/// # Errors
///
/// Propagates database errors.
pub async fn zweite_runde_eroeffnen(
    pool: &PgPool,
    tenant: &str,
    process_id: Uuid,
    comdis_ref: &str,
) -> Result<ZweiteRunde, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(Uuid, String, i16, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT id, outcome, runde, dispatched_at FROM invoic_receipts
          WHERE tenant = $1 AND process_id = $2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(process_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, outcome, runde, dispatched_at)) = row else {
        return Ok(ZweiteRunde::KeinBeleg);
    };
    if runde == 2 {
        tx.commit().await?;
        return Ok(if dispatched_at.is_some() {
            ZweiteRunde::Beantwortet
        } else {
            gehaltene_zweite(pool, tenant, id).await?
        });
    }
    if outcome != "Dispute" || dispatched_at.is_none() {
        return Ok(ZweiteRunde::KeineAbweisung(outcome));
    }
    sqlx::query(
        "UPDATE invoic_receipts
            SET runde = 2, comdis_ref = $3,
                erste_runde_antwort = antwort, erste_runde_dispatched_at = dispatched_at,
                antwort = NULL, dispatched_at = NULL, outcome = 'Offen',
                offen = '[{\"pruefschritt\": 1}]'::jsonb
          WHERE tenant = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .bind(comdis_ref)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    gehaltene_zweite(pool, tenant, id).await
}

async fn gehaltene_zweite(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
) -> Result<ZweiteRunde, sqlx::Error> {
    Ok(match gehaltene_rechnung(pool, tenant, id).await? {
        Some(g) => ZweiteRunde::Offen(g),
        None => ZweiteRunde::Beantwortet,
    })
}
