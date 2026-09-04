//! PostgreSQL persistence for `sperrd`.
//!
//! ## The IFTSTA invariant
//!
//! A terminal order whose `iftsta_dispatched_at` is NULL is an order whose
//! Lieferant has not been told the outcome — their `gpke-sperrung-lf` process
//! cannot close, and GPKE gives them no way to find out but to ask.
//!
//! So it is a queue, not a one-shot: [`claim_iftsta_retry`] hands the worker one
//! such order at a time under `FOR UPDATE SKIP LOCKED`, a successful dispatch
//! closes it, and orders past [`IFTSTA_MAX_ATTEMPTS`] are announced once as
//! `de.sperr.iftsta.ausstehend` and left for a human.

use anyhow::Context as _;
use mako_markt::makod_client::{ForwardCommand, MakodClient};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::model::{Arbeitszeit, OrderStatus, OrderType};

/// The window GPKE Teil 2 § 3.5.1.2 Nr. 1 gives the NB for the physical act:
/// „Die Sperrung der Marktlokation ist durch den NB spätestens innerhalb von
/// **6 WT** nach dem frühestmöglichen Sperrtermin durchzuführen."
pub const AUSFUEHRUNG_WERKTAGE: u32 = 6;

/// „Unverzüglich, jedoch spätester ÜT ist der **1. WT** nach dem Abschluss des
/// Sperrauftrags" — the IFTSTA 21039 window (GPKE Teil 2 § 3.5.1.2 Nr. 5,
/// § 3.5.2.2 Nr. 4).
pub const IFTSTA_WERKTAGE: u32 = 1;

/// „Der NB führt bis zu **zwei** Sperrversuche innerhalb eines Sperrauftrags
/// durch" (GPKE Teil 2 § 3.5.1.2 Nr. 5).
pub const MAX_SPERRVERSUCHE: i32 = 2;

/// Whether the Lieferant gave the NB the lead time GPKE Teil 2 § 3.5.1.2 Nr. 1
/// requires, and which of the two windows applied.
///
/// The Festlegung states **two**, and the wire tells them apart on its own:
/// `DTM+203` fixes date, time and place — the Festlegung's example is a
/// Gerichtsvollzieher — so the NB cannot move the visit to fit its own
/// scheduling and needs twice the room. `DTM+469` names an earliest start the NB
/// then schedules within. The AHB makes the two mutually exclusive, which is why
/// `sperr_orders` carries a `CHECK` for it, and why the distinction needs no
/// extra inbound field.
///
/// **Recording, not refusing.** Prozessschritt 2 lists what the NB checks before
/// it answers — „ob die Marktlokation dem LF zugeordnet ist, ob die
/// Marktlokation identifiziert werden kann und die Zusicherung der Berechtigung
/// nach Netznutzungsvertrag vorliegt" — and the Vorlauffrist is not among them.
/// Refusing on a ground the Festlegung does not publish is the § 20
/// EnWG-unsafe direction, so a short lead is surfaced to the operator instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VorlaufBefund {
    /// `true` when the Übertragungstag fell on or before the latest admissible
    /// one.
    pub eingehalten: bool,
    /// The latest ÜT the order could have carried.
    pub spaetester_ut: Date,
    /// The catalogue entry that was applied, for the operator-facing reason.
    pub obligation: &'static mako_fristen::vorlauf::VorlaufObligation,
}

/// Check a Sperrauftrag against its Vorlauffrist.
///
/// `None` when the order names no Sperrtermin at all — an Entsperrauftrag
/// carries neither `DTM+203` nor `DTM+469`, and § 41f Abs. 7 EnWG makes
/// restoration „unverzüglich" rather than dated, so there is no anchor to count
/// back from.
#[must_use]
pub fn vorlauffrist_befund(
    ausfuehrung_am: Option<Date>,
    fruehestens_am: Option<Date>,
    uebertragungstag: Date,
) -> Option<VorlaufBefund> {
    // `DTM+203` first: a fixed date is the termingebundene case, and the two
    // are mutually exclusive on the wire.
    let (key, sperrtermin) = match (ausfuehrung_am, fruehestens_am) {
        (Some(d), _) => ("gpke.sperrauftrag.termingebunden", d),
        (None, Some(d)) => ("gpke.sperrauftrag", d),
        (None, None) => return None,
    };
    let obligation = mako_fristen::vorlauf::vorlauf(key)?;
    let verdict = obligation.shape.check(
        uebertragungstag,
        sperrtermin,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    Some(VorlaufBefund {
        eingehalten: verdict.is_ok(),
        spaetester_ut: mako_fristen::sub_werktage(
            sperrtermin,
            match key {
                "gpke.sperrauftrag.termingebunden" => {
                    mako_fristen::vorlauf::SPERRAUFTRAG_TERMINGEBUNDEN_VORLAUF_WT
                }
                _ => mako_fristen::vorlauf::SPERRAUFTRAG_VORLAUF_WT,
            },
            mako_fristen::HolidayCalendar::BdewMaKo,
        ),
        obligation,
    })
}

/// The earliest Sperrtermin the NB may set when the MSB has given no **generelle
/// Zustimmung** to Sperrung/Entsperrung by the NB.
///
/// GPKE Teil 2 § 3.5.1.2 Nr. 2 puts this on the NB, not on the LF: „Sofern keine
/// generelle Zustimmung des MSB … vorliegt, ist der Sperrtermin vom NB so
/// festzulegen, dass dem MSB noch eine fristgerechte Antwort auf Anfrage vor dem
/// Sperrtermin möglich ist (s. dazu Fristen der SD-Schritte 3 und 4)." Those two
/// are the Anfrage's 3 WT before the Sperrtermin (Nr. 3) and the MSB's 3 WT to
/// answer it (Nr. 4), so the earliest admissible date is six Werktage out.
///
/// With a generelle Zustimmung on file no Anfrage is sent at all and the NB may
/// schedule freely.
#[must_use]
pub fn fruehester_sperrtermin_ohne_msb_zustimmung(heute: Date) -> Date {
    mako_fristen::add_werktage(
        heute,
        mako_fristen::vorlauf::SPERRUNG_MSB_ANFRAGE_WT + MSB_ANTWORT_WERKTAGE,
        mako_fristen::HolidayCalendar::BdewMaKo,
    )
}

/// „Unverzüglich, jedoch spätester ÜT ist der **3. WT** nach dem ÜT von Nr. 3" —
/// the MSB's window to answer the Anfrage Sperrung (GPKE Teil 2 § 3.5.1.2 Nr. 4).
///
/// Its absence is consent: „Verstreicht die Frist, ohne dass die Antwort auf die
/// Anfrage beim NB eingeht, gilt dies als Zustimmung."
pub const MSB_ANTWORT_WERKTAGE: u32 = 3;

/// The date by which the physical act is due, or `None` when the order names no
/// date at all — an Entsperrauftrag carries neither `DTM+203` nor `DTM+469`, and
/// § 41f Abs. 7 EnWG makes restoration „unverzüglich" rather than dated.
#[must_use]
pub fn ausfuehrung_faellig_am(
    ausfuehrung_am: Option<Date>,
    fruehestens_am: Option<Date>,
) -> Option<Date> {
    ausfuehrung_am.or(fruehestens_am).map(|start| {
        mako_fristen::add_werktage(
            start,
            AUSFUEHRUNG_WERKTAGE,
            mako_fristen::HolidayCalendar::BdewMaKo,
        )
    })
}

/// The date by which the IFTSTA 21039 is due, counted from the day the order
/// was completed.
#[must_use]
pub fn iftsta_faellig_am(abschluss: Date) -> Date {
    mako_fristen::add_werktage(
        abschluss,
        IFTSTA_WERKTAGE,
        mako_fristen::HolidayCalendar::BdewMaKo,
    )
}

/// How many times the worker re-tries an IFTSTA before escalating to a human.
///
/// Bounded because a dispatch that keeps failing is not a transport problem: the
/// makod process is in the wrong state, or was never spawned. Retrying that
/// forever hides it behind a growing attempt count.
pub const IFTSTA_MAX_ATTEMPTS: i32 = 8;

// ── Requests ──────────────────────────────────────────────────────────────────

/// Body of `POST /api/v1/sperr-orders`, and the shape the ORDERS ingest builds.
#[derive(Debug, Deserialize)]
pub struct CreateOrderRequest {
    /// 11-digit MaLo-ID (`SG2 LOC+172`, hint \[521\]).
    pub malo_id: String,
    /// The ordering Lieferant's MP-ID (`SG2 NAD+MS`).
    pub lf_mp_id: String,
    pub order_type: OrderType,
    /// The `makod` process the IFTSTA 21039 is reported into. Absent for an
    /// operator-created order, which has no market correspondent.
    pub process_id: Option<String>,
    /// `DTM+203` — a fixed execution date the LF requires.
    pub ausfuehrung_am: Option<Date>,
    /// `DTM+469` — execute at the next possible date, but not before this one.
    pub fruehestens_am: Option<Date>,
    /// `IMD+7081` on an Entsperrauftrag.
    pub arbeitszeit: Option<Arbeitszeit>,
    /// `SG2 NAD+Z24` Treffpunkt — where the technician goes.
    #[serde(default)]
    pub treffpunkt: Treffpunkt,
    /// `SG29 FTX+ACB` free text from the LF.
    pub hinweis: Option<String>,
}

/// `SG2 NAD+Z24` — the meeting point for the field visit.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Treffpunkt {
    /// `NAD 3124` Zusatzinformation zur Identifizierung ("Keller links",
    /// "Zählerschrank Hof"). The AHB accepts this *instead of* a street.
    pub hinweis: Option<String>,
    /// `NAD 3042` Straße und Hausnummer.
    pub strasse: Option<String>,
    /// `NAD 3251` Postleitzahl.
    pub plz: Option<String>,
    /// `NAD 3164` Ort.
    pub ort: Option<String>,
    /// `NAD 3207` Ländername, Code (ISO 3166 alpha-2).
    pub land: Option<String>,
}

impl CreateOrderRequest {
    /// Reject what the ORDERS AHB does not allow, before it reaches the database.
    ///
    /// # Errors
    ///
    /// Returns the AHB condition that was violated.
    pub fn validate(&self) -> Result<(), String> {
        // Conditions [55]/[56]: DTM+203 and DTM+469 are alternatives, never both.
        if self.ausfuehrung_am.is_some() && self.fruehestens_am.is_some() {
            return Err("ausfuehrung_am (DTM+203) and fruehestens_am (DTM+469) are \
                        alternatives — an ORDERS carries one or the other, not both"
                .to_owned());
        }
        if let Some(land) = self.treffpunkt.land.as_deref()
            && (land.len() != 2 || !land.bytes().all(|b| b.is_ascii_uppercase()))
        {
            return Err(format!(
                "treffpunkt.land must be an ISO 3166 alpha-2 code, got {land:?}"
            ));
        }
        Ok(())
    }
}

// ── Rows ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SperrOrderRow {
    pub id: String,
    pub tenant: String,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub order_type: OrderType,
    pub pruefidentifikator: Option<i32>,
    pub process_id: Option<String>,
    pub ausfuehrung_am: Option<Date>,
    pub fruehestens_am: Option<Date>,
    pub arbeitszeit: Option<Arbeitszeit>,
    pub treffpunkt_hinweis: Option<String>,
    pub treffpunkt_strasse: Option<String>,
    pub treffpunkt_plz: Option<String>,
    pub treffpunkt_ort: Option<String>,
    pub treffpunkt_land: Option<String>,
    pub hinweis: Option<String>,
    /// The § 3.5.1.2 Nr. 1 deadline for the physical act.
    pub ausfuehrung_faellig_am: Option<Date>,
    pub status: OrderStatus,
    /// How many Sperrversuche have been made — the Festlegung allows two.
    pub sperrversuche: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub letzter_versuch_am: Option<OffsetDateTime>,
    pub letzter_versuch_grund: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub executed_at: Option<OffsetDateTime>,
    pub execution_note: Option<String>,
    pub fail_reason: Option<String>,
    pub pruefschritt_code: Option<String>,
    /// The § 3.5.1.2 Nr. 5 deadline for the IFTSTA 21039.
    pub iftsta_faellig_am: Option<Date>,
    pub iftsta_ref: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub iftsta_dispatched_at: Option<OffsetDateTime>,
    pub iftsta_attempts: i32,
    pub iftsta_last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

const ORDER_COLUMNS: &str = "id::TEXT, tenant, malo_id, lf_mp_id, order_type, \
     pruefidentifikator, process_id, ausfuehrung_am, fruehestens_am, arbeitszeit, \
     treffpunkt_hinweis, treffpunkt_strasse, treffpunkt_plz, treffpunkt_ort, \
     treffpunkt_land, hinweis, ausfuehrung_faellig_am, status, sperrversuche, \
     letzter_versuch_am, letzter_versuch_grund, executed_at, execution_note, \
     fail_reason, pruefschritt_code, iftsta_faellig_am, iftsta_ref, \
     iftsta_dispatched_at, iftsta_attempts, iftsta_last_error, created_at, updated_at";

/// Aggregate counters for the compliance sweep.
#[derive(Debug, Serialize)]
pub struct SperrStats {
    pub total: i64,
    pub pending: i64,
    pub executed: i64,
    pub failed: i64,
    pub cancelled: i64,
    /// Pending orders whose requested execution date has passed. The date comes
    /// from the Lieferant (`DTM+203` or `DTM+469`) — it is when the LF wanted
    /// the work done, not when the Festlegung requires it.
    pub overdue_pending: i64,
    /// Pending orders past the **regulatory** execution window: 6 Werktage after
    /// the frühestmöglicher Sperrtermin (GPKE Teil 2 § 3.5.1.2 Nr. 1). This is
    /// the number a BNetzA audit asks about.
    pub frist_ueberschritten: i64,
    /// Terminal orders whose IFTSTA 21039 is past its own Frist — 1 Werktag
    /// after completion (§ 3.5.1.2 Nr. 5).
    pub iftsta_ueberfaellig: i64,
    /// Terminal orders whose IFTSTA 21039 has not been dispatched. The LF has
    /// not learned the outcome and their process cannot close.
    pub iftsta_outstanding: i64,
    /// …of which have exhausted the retry budget and need a human.
    pub iftsta_stuck: i64,
    /// Orders the **Lieferant** sent later than its own Vorlauffrist allowed —
    /// 6 Werktage before the frühestmöglicher Sperrtermin, or 12 before a
    /// termingebundener one (GPKE Teil 2 § 3.5.1.2 Nr. 1).
    ///
    /// A different question from every other figure here: the others count the
    /// NB's own windows, this one counts the counterparty's. The NB executes
    /// them anyway — Prozessschritt 2 publishes no ground to refuse on — so the
    /// number is what a Lieferantenrahmenvertrag review asks about, not an
    /// operations backlog.
    pub vorlauffrist_verletzt: i64,
}

// ── Create ────────────────────────────────────────────────────────────────────

/// Insert a new order.
///
/// Returns `None` when an order for the same `(tenant, process_id)` already
/// exists — an ORDERS redelivered over AS4 must not put a second disconnection in
/// front of the field team.
///
/// # Errors
///
/// Propagates database errors.
pub async fn create_order_pg(
    pool: &PgPool,
    tenant: &str,
    req: &CreateOrderRequest,
) -> anyhow::Result<Option<Uuid>> {
    let befund = vorlauffrist_befund(
        req.ausfuehrung_am,
        req.fruehestens_am,
        mako_fristen::heute(),
    );
    if let Some(b) = befund.filter(|b| !b.eingehalten) {
        tracing::warn!(
            malo_id = %req.malo_id,
            lf_mp_id = %req.lf_mp_id,
            spaetester_ut = %b.spaetester_ut,
            frist = b.obligation.name,
            "sperrd: Sperrauftrag arrived after its Vorlauffrist — recorded, not refused \
             (GPKE Teil 2 § 3.5.1.2 Nr. 2 does not publish it as a ground)",
        );
    }
    let row = sqlx::query(
        r"INSERT INTO sperr_orders
              (tenant, malo_id, lf_mp_id, order_type, pruefidentifikator, process_id,
               ausfuehrung_am, fruehestens_am, arbeitszeit,
               treffpunkt_hinweis, treffpunkt_strasse, treffpunkt_plz,
               treffpunkt_ort, treffpunkt_land, hinweis, ausfuehrung_faellig_am,
               vorlauffrist_eingehalten, vorlauffrist_spaetester_ut)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                  $17, $18)
          ON CONFLICT (tenant, process_id) DO NOTHING
          RETURNING id::TEXT",
    )
    .bind(tenant)
    .bind(&req.malo_id)
    .bind(&req.lf_mp_id)
    .bind(req.order_type)
    .bind(req.process_id.as_ref().map(|_| req.order_type.pid()))
    .bind(&req.process_id)
    .bind(req.ausfuehrung_am)
    .bind(req.fruehestens_am)
    .bind(req.arbeitszeit)
    .bind(&req.treffpunkt.hinweis)
    .bind(&req.treffpunkt.strasse)
    .bind(&req.treffpunkt.plz)
    .bind(&req.treffpunkt.ort)
    .bind(&req.treffpunkt.land)
    .bind(&req.hinweis)
    .bind(ausfuehrung_faellig_am(
        req.ausfuehrung_am,
        req.fruehestens_am,
    ))
    // The Übertragungstag is the day the order reached us — an operator-created
    // order has no ÜT of its own, and dating it today is the only reading that
    // does not invent one.
    .bind(befund.map(|b| b.eingehalten))
    .bind(befund.map(|b| b.spaetester_ut))
    .fetch_optional(pool)
    .await
    .context("insert sperr_order")?;

    let Some(row) = row else { return Ok(None) };
    let id: String = row.try_get("id")?;
    Ok(Some(id.parse::<Uuid>().context("parse UUID")?))
}

// ── Read ──────────────────────────────────────────────────────────────────────

pub async fn list_orders_pg(
    pool: &PgPool,
    tenant: &str,
    status: Option<OrderStatus>,
    malo_id: Option<&str>,
    only_due: bool,
    limit: i64,
) -> anyhow::Result<Vec<SperrOrderRow>> {
    sqlx::query_as::<_, SperrOrderRow>(&format!(
        r"SELECT {ORDER_COLUMNS}
          FROM sperr_orders
          WHERE tenant = $1
            AND ($2::TEXT IS NULL OR status = $2)
            AND ($3::TEXT IS NULL OR malo_id = $3)
            AND (NOT $4 OR COALESCE(ausfuehrung_am, fruehestens_am) <= heute())
          ORDER BY COALESCE(ausfuehrung_am, fruehestens_am) NULLS LAST, created_at DESC
          LIMIT $5"
    ))
    .bind(tenant)
    .bind(status.map(OrderStatus::as_str))
    .bind(malo_id)
    .bind(only_due)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_orders_pg")
}

pub async fn fetch_order_pg(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<SperrOrderRow>> {
    sqlx::query_as::<_, SperrOrderRow>(&format!(
        "SELECT {ORDER_COLUMNS} FROM sperr_orders WHERE id = $1 AND tenant = $2"
    ))
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_order_pg")
}

// ── Execute / fail ────────────────────────────────────────────────────────────

/// The outcome the field team reported.
#[derive(Debug)]
pub enum Outcome<'a> {
    /// Carried out. `at` becomes `DTM+293 Fertigstellungsdatum`.
    Executed {
        at: OffsetDateTime,
        note: Option<&'a str>,
        /// `SG15 STS DE9013` from the EBD "erfolgreich" cluster.
        pruefschritt_code: Option<&'a str>,
    },
    /// Attempted and not carried out.
    Failed {
        reason: &'a str,
        /// `SG15 STS DE9013` from the EBD "gescheitert" cluster.
        pruefschritt_code: Option<&'a str>,
        /// `true` when no further attempt will be made — a court injunction, a
        /// glaubhaft gemachter Verhinderungsgrund, or the second of the two
        /// Sperrversuche the Festlegung allows.
        ///
        /// `false` records the attempt and leaves the order `pending`: GPKE
        /// Teil 2 § 3.5.1.2 Nr. 5 gives the NB **two** Sperrversuche within one
        /// Sperrauftrag, and reporting `Z13 gescheitert` after the first ends a
        /// process that still owes a second visit.
        endgueltig: bool,
    },
}

impl Outcome<'_> {
    #[must_use]
    pub const fn status(&self) -> OrderStatus {
        match self {
            Self::Executed { .. } => OrderStatus::Executed,
            Self::Failed { .. } => OrderStatus::Failed,
        }
    }
}

/// What [`report_outcome`] recorded.
#[derive(Debug, PartialEq, Eq)]
pub enum Reported {
    /// The order moved to its terminal state. The IFTSTA is either dispatched
    /// (`iftsta_ref` set) or queued for the retry worker.
    Recorded { iftsta_dispatched: bool },
    /// A Sperrversuch was recorded and the order stays `pending` — the
    /// Festlegung allows a second visit, so no IFTSTA goes out yet.
    VersuchNotiert {
        /// How many of the two allowed attempts have been made.
        sperrversuche: i32,
        /// The Marktlokation, for the announcement.
        malo_id: String,
    },
    /// No pending order with that id in this tenant.
    NotFound,
}

/// Record a field outcome and report it to the Lieferant with IFTSTA 21039.
///
/// The order is **claimed first** — a single guarded `UPDATE … WHERE status =
/// 'pending'` — and only then is the IFTSTA dispatched. Reading, dispatching and
/// guarding the write afterwards let a concurrent execute and fail both pass the
/// read, so the LF received an Ausführungs- *and* a Fehlmeldung for one order.
///
/// If the dispatch then fails, the claim is **kept**, not rolled back: the field
/// team's report is a fact about the physical world and must not be discarded
/// because a downstream service was unreachable. The order lands in the retry
/// queue instead, which is what [`claim_iftsta_retry`] drains.
///
/// # Errors
///
/// Propagates database errors. A failed dispatch is *not* an error — it is a
/// queued retry.
pub async fn report_outcome(
    pool: &PgPool,
    makod: &Arc<MakodClient>,
    id: Uuid,
    tenant: &str,
    outcome: &Outcome<'_>,
) -> anyhow::Result<Reported> {
    let status = outcome.status();
    let (executed_at, note, reason, code) = match outcome {
        Outcome::Executed {
            at,
            note,
            pruefschritt_code,
        } => (Some(*at), *note, None, *pruefschritt_code),
        Outcome::Failed {
            reason,
            pruefschritt_code,
            ..
        } => (None, None, Some(*reason), *pruefschritt_code),
    };

    // A non-final Sperrversuch is recorded and the order stays `pending`: GPKE
    // Teil 2 § 3.5.1.2 Nr. 5 allows two attempts inside one Sperrauftrag, and
    // the guard is on the *stored* count so two concurrent field reports cannot
    // both read "one attempt left".
    if let Outcome::Failed {
        reason,
        endgueltig: false,
        ..
    } = outcome
    {
        let recorded = sqlx::query(
            r"UPDATE sperr_orders
              SET sperrversuche = sperrversuche + 1,
                  letzter_versuch_am = now(),
                  letzter_versuch_grund = $3,
                  updated_at = now()
              WHERE id = $1 AND tenant = $2 AND status = 'pending'
                AND sperrversuche + 1 < $4
              RETURNING sperrversuche, malo_id",
        )
        .bind(id)
        .bind(tenant)
        .bind(*reason)
        .bind(MAX_SPERRVERSUCHE)
        .fetch_optional(pool)
        .await
        .context("record Sperrversuch")?;
        if let Some(row) = recorded {
            return Ok(Reported::VersuchNotiert {
                sperrversuche: row.try_get("sperrversuche")?,
                malo_id: row.try_get("malo_id")?,
            });
        }
        // The allowance is used up — fall through and close the order.
    }

    // The IFTSTA Frist runs from the day the Sperrauftrag was completed, which
    // for a success is the Fertigstellungsdatum and otherwise today.
    let abschluss = executed_at
        .unwrap_or_else(OffsetDateTime::now_utc)
        .to_offset(time::UtcOffset::UTC)
        .date();

    let claimed = sqlx::query(
        r"UPDATE sperr_orders
          SET status = $3, executed_at = $4, execution_note = $5, fail_reason = $6,
              pruefschritt_code = $7, iftsta_faellig_am = $8,
              sperrversuche = LEAST(sperrversuche + $9, $10),
              updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'pending'
          RETURNING malo_id, lf_mp_id, order_type, process_id",
    )
    .bind(id)
    .bind(tenant)
    .bind(status.as_str())
    .bind(executed_at)
    .bind(note)
    .bind(reason)
    .bind(code)
    .bind(iftsta_faellig_am(abschluss))
    .bind(i32::from(matches!(outcome, Outcome::Failed { .. })))
    .bind(MAX_SPERRVERSUCHE)
    .fetch_optional(pool)
    .await
    .context("claim order")?;

    let Some(row) = claimed else {
        return Ok(Reported::NotFound);
    };

    let order = DispatchTarget {
        id,
        malo_id: row.try_get("malo_id")?,
        lf_mp_id: row.try_get("lf_mp_id")?,
        order_type: row.try_get("order_type")?,
        process_id: row.try_get("process_id")?,
        status,
        executed_at,
        note: note.map(str::to_owned),
        reason: reason.map(str::to_owned),
        pruefschritt_code: code.map(str::to_owned),
    };
    let dispatched = dispatch_iftsta(pool, makod, tenant, &order).await;
    Ok(Reported::Recorded {
        iftsta_dispatched: dispatched,
    })
}

/// Everything the IFTSTA 21039 dispatch needs about an order.
#[derive(Debug)]
pub struct DispatchTarget {
    pub id: Uuid,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub order_type: OrderType,
    pub process_id: Option<String>,
    pub status: OrderStatus,
    pub executed_at: Option<OffsetDateTime>,
    pub note: Option<String>,
    pub reason: Option<String>,
    pub pruefschritt_code: Option<String>,
}

/// Hand the IFTSTA 21039 to `makod`, recording success or the reason it failed.
///
/// Returns whether the dispatch succeeded. Never returns an error: a failure is
/// recorded on the row and retried by the worker, because the alternative is
/// losing the field team's report.
pub async fn dispatch_iftsta(
    pool: &PgPool,
    makod: &Arc<MakodClient>,
    tenant: &str,
    order: &DispatchTarget,
) -> bool {
    // An operator-created order has no inbound ORDERS behind it, so there is no
    // Lieferant waiting and no process to report into. Marking it dispatched is
    // honest: nothing is outstanding.
    let Some(process_id) = order.process_id.as_deref() else {
        let _ = record_iftsta(pool, order.id, tenant, "local").await;
        return true;
    };

    let command = match order.status {
        OrderStatus::Executed => "gpke.sperrung.bestaetigen",
        OrderStatus::Failed => "gpke.sperrung.fehlgeschlagen",
        // Unreachable: only terminal outcomes are dispatched.
        OrderStatus::Pending | OrderStatus::Cancelled => return false,
    };

    let cmd = ForwardCommand {
        command: command.to_owned(),
        marktrolle: Some("NB".to_owned()),
        malo_id: Some(order.malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "lf_mp_id":   order.lf_mp_id,
            "process_id": process_id,
            // SG15 STS DE9015 — Z37 Sperren / Z38 Entsperren. Derived from the
            // order type so an executed *Entsperrung* is not reported as a
            // Sperren-Auftragsstatus, which is what a single command name for
            // both would produce.
            "auftragsstatus_qualifier": order.order_type.iftsta_qualifier(),
            // SG15 STS DE4405 — Z14 erfolgreich / Z13 gescheitert.
            "auftragsstatus_code": order.status.iftsta_code(),
            // SG15 STS DE9013 — Code des Prüfschritts. Muss in the AHB.
            "pruefschritt_code": order.pruefschritt_code,
            // DTM+293 Fertigstellungsdatum — Muss on Z14, and condition [495]
            // requires it to be ≤ the document date.
            "fertigstellung": order.executed_at.and_then(|t| {
                t.format(&time::format_description::well_known::Rfc3339).ok()
            }),
            // SG25 FTX+ACB Freier Text.
            "note":   order.note,
            "reason": order.reason,
        }),
    };

    // Keyed on the order, not the attempt: makod deduplicates a retry into the
    // same command rather than putting a second IFTSTA on the wire.
    let key = format!("sperrd-iftsta-{}", order.id);
    match makod.post_command(&key, &cmd).await {
        Ok(accepted) => {
            let reference = accepted.process_id.to_string();
            if let Err(e) = record_iftsta(pool, order.id, tenant, &reference).await {
                tracing::error!(
                    order_id = %order.id, error = %e,
                    "sperrd: IFTSTA 21039 accepted by makod but not recorded — the retry \
                     worker will re-send it under the same idempotency key"
                );
                return false;
            }
            true
        }
        Err(e) => {
            let _ = record_iftsta_failure(pool, order.id, tenant, &e.to_string()).await;
            tracing::warn!(
                order_id = %order.id, error = %e,
                "sperrd: IFTSTA 21039 dispatch failed — queued for retry"
            );
            false
        }
    }
}

/// Record a successful IFTSTA dispatch.
async fn record_iftsta(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    reference: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE sperr_orders
          SET iftsta_ref = $3, iftsta_dispatched_at = now(),
              iftsta_last_error = NULL, updated_at = now()
          WHERE id = $1 AND tenant = $2 AND iftsta_dispatched_at IS NULL",
    )
    .bind(id)
    .bind(tenant)
    .bind(reference)
    .execute(pool)
    .await
    .context("record IFTSTA dispatch")?;
    Ok(())
}

/// Record a failed IFTSTA dispatch and count the attempt.
async fn record_iftsta_failure(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE sperr_orders
          SET iftsta_attempts = iftsta_attempts + 1,
              iftsta_last_error = $3, updated_at = now()
          WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    // Bounded: a provider error can be arbitrarily long and this column is read
    // in list responses.
    .bind(error.chars().take(500).collect::<String>())
    .execute(pool)
    .await
    .context("record IFTSTA failure")?;
    Ok(())
}

/// Take one order off the IFTSTA retry queue, or `None` when it is empty.
///
/// `FOR UPDATE SKIP LOCKED` so replicas drain it in parallel without one
/// blocking on another's row. Only orders inside the retry budget are handed
/// out; the rest are [`list_stuck_iftsta`]'s problem.
///
/// # Errors
///
/// Propagates database errors.
pub async fn claim_iftsta_retry(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Option<DispatchTarget>> {
    let row = sqlx::query(
        r"SELECT id, malo_id, lf_mp_id, order_type, process_id, status,
                 executed_at, execution_note, fail_reason, pruefschritt_code
          FROM sperr_orders
          WHERE tenant = $1
            AND status IN ('executed', 'failed')
            AND iftsta_dispatched_at IS NULL
            AND iftsta_attempts < $2
          ORDER BY updated_at
          LIMIT 1
          FOR UPDATE SKIP LOCKED",
    )
    .bind(tenant)
    .bind(IFTSTA_MAX_ATTEMPTS)
    .fetch_optional(pool)
    .await
    .context("claim_iftsta_retry")?;

    let Some(r) = row else { return Ok(None) };
    Ok(Some(DispatchTarget {
        id: r.try_get("id")?,
        malo_id: r.try_get("malo_id")?,
        lf_mp_id: r.try_get("lf_mp_id")?,
        order_type: r.try_get("order_type")?,
        process_id: r.try_get("process_id")?,
        status: r.try_get("status")?,
        executed_at: r.try_get("executed_at")?,
        note: r.try_get("execution_note")?,
        reason: r.try_get("fail_reason")?,
        pruefschritt_code: r.try_get("pruefschritt_code")?,
    }))
}

/// Orders that exhausted the retry budget and have not been escalated yet.
///
/// # Errors
///
/// Propagates database errors.
pub async fn list_stuck_iftsta(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Vec<(Uuid, String, String, String)>> {
    let rows = sqlx::query(
        r"SELECT id, malo_id, lf_mp_id, COALESCE(iftsta_last_error, '') AS err
          FROM sperr_orders
          WHERE tenant = $1
            AND status IN ('executed', 'failed')
            AND iftsta_dispatched_at IS NULL
            AND iftsta_attempts >= $2
            AND iftsta_escalated_at IS NULL",
    )
    .bind(tenant)
    .bind(IFTSTA_MAX_ATTEMPTS)
    .fetch_all(pool)
    .await
    .context("list_stuck_iftsta")?;
    rows.into_iter()
        .map(|r| {
            Ok((
                r.try_get("id")?,
                r.try_get("malo_id")?,
                r.try_get("lf_mp_id")?,
                r.try_get("err")?,
            ))
        })
        .collect()
}

/// Mark a stuck order as escalated so it is announced once, not every cycle.
///
/// # Errors
///
/// Propagates database errors.
pub async fn mark_iftsta_escalated(
    exec: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sperr_orders SET iftsta_escalated_at = now() \
         WHERE id = $1 AND tenant = $2 AND iftsta_escalated_at IS NULL",
    )
    .bind(id)
    .bind(tenant)
    .execute(exec)
    .await
    .context("mark_iftsta_escalated")?;
    Ok(())
}

// ── Cancel ────────────────────────────────────────────────────────────────────

/// Withdraw a pending order. Terminal orders cannot be cancelled.
///
/// Returns the `(malo_id, lf_mp_id)` of the cancelled order, or `None`.
///
/// # Errors
///
/// Propagates database errors.
pub async fn cancel_order_pg(
    exec: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let row = sqlx::query(
        r"UPDATE sperr_orders
          SET status = 'cancelled', updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'pending'
          RETURNING malo_id, lf_mp_id",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(exec)
    .await
    .context("cancel_order_pg")?;
    match row {
        Some(r) => Ok(Some((r.try_get("malo_id")?, r.try_get("lf_mp_id")?))),
        None => Ok(None),
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────────

pub async fn stats_pg(pool: &PgPool, tenant: &str) -> anyhow::Result<SperrStats> {
    let row = sqlx::query(
        r"SELECT
              COUNT(*)                                        AS total,
              COUNT(*) FILTER (WHERE status = 'pending')      AS pending,
              COUNT(*) FILTER (WHERE status = 'executed')     AS executed,
              COUNT(*) FILTER (WHERE status = 'failed')       AS failed,
              COUNT(*) FILTER (WHERE status = 'cancelled')    AS cancelled,
              COUNT(*) FILTER (
                  WHERE status = 'pending'
                    AND COALESCE(ausfuehrung_am, fruehestens_am) < heute()
              )                                                AS overdue_pending,
              COUNT(*) FILTER (
                  WHERE status = 'pending'
                    AND ausfuehrung_faellig_am IS NOT NULL
                    AND ausfuehrung_faellig_am < heute()
              )                                                AS frist_ueberschritten,
              COUNT(*) FILTER (WHERE vorlauffrist_eingehalten = false)
                                                               AS vorlauffrist_verletzt,
              COUNT(*) FILTER (
                  WHERE status IN ('executed', 'failed')
                    AND iftsta_dispatched_at IS NULL
                    AND iftsta_faellig_am IS NOT NULL
                    AND iftsta_faellig_am < heute()
              )                                                AS iftsta_ueberfaellig,
              COUNT(*) FILTER (
                  WHERE status IN ('executed', 'failed')
                    AND iftsta_dispatched_at IS NULL
              )                                                AS iftsta_outstanding,
              COUNT(*) FILTER (
                  WHERE status IN ('executed', 'failed')
                    AND iftsta_dispatched_at IS NULL
                    AND iftsta_attempts >= $2
              )                                                AS iftsta_stuck
          FROM sperr_orders
          WHERE tenant = $1",
    )
    .bind(tenant)
    .bind(IFTSTA_MAX_ATTEMPTS)
    .fetch_one(pool)
    .await
    .context("stats_pg")?;

    Ok(SperrStats {
        total: row.try_get("total")?,
        pending: row.try_get("pending")?,
        executed: row.try_get("executed")?,
        failed: row.try_get("failed")?,
        cancelled: row.try_get("cancelled")?,
        frist_ueberschritten: row.try_get("frist_ueberschritten")?,
        iftsta_ueberfaellig: row.try_get("iftsta_ueberfaellig")?,
        overdue_pending: row.try_get("overdue_pending")?,
        iftsta_outstanding: row.try_get("iftsta_outstanding")?,
        iftsta_stuck: row.try_get("iftsta_stuck")?,
        vorlauffrist_verletzt: row.try_get("vorlauffrist_verletzt")?,
    })
}

// ── Execution-window sweep (GPKE Teil 2 § 3.5.1.2 Nr. 1) ─────────────────────

/// Pending orders past their 6-Werktage execution window that have not been
/// announced yet.
///
/// Returns `(id, malo_id, lf_mp_id, faellig_am)`. The announcement is marked on
/// the row by [`mark_ausfuehrung_escalated`], so a missed Frist is reported once
/// rather than on every sweep.
///
/// # Errors
///
/// Propagates database errors.
pub async fn list_ausfuehrung_ueberfaellig(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Vec<(Uuid, String, String, Date)>> {
    let rows = sqlx::query(
        r"SELECT id, malo_id, lf_mp_id, ausfuehrung_faellig_am
          FROM sperr_orders
          WHERE tenant = $1
            AND status = 'pending'
            AND ausfuehrung_faellig_am IS NOT NULL
            AND ausfuehrung_faellig_am < heute()
            AND ausfuehrung_eskaliert_at IS NULL
          ORDER BY ausfuehrung_faellig_am
          LIMIT 100",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list overdue executions")?;
    rows.into_iter()
        .map(|r| {
            Ok((
                r.try_get("id")?,
                r.try_get("malo_id")?,
                r.try_get("lf_mp_id")?,
                r.try_get("ausfuehrung_faellig_am")?,
            ))
        })
        .collect()
}

/// Mark an overdue execution as announced.
///
/// # Errors
///
/// Propagates database errors.
pub async fn mark_ausfuehrung_escalated(
    conn: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE sperr_orders SET ausfuehrung_eskaliert_at = now()
          WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .execute(conn)
    .await
    .context("mark execution escalated")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn req() -> CreateOrderRequest {
        CreateOrderRequest {
            malo_id: "51238696012".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            order_type: OrderType::Sperrung,
            process_id: None,
            ausfuehrung_am: None,
            fruehestens_am: None,
            arbeitszeit: None,
            treffpunkt: Treffpunkt::default(),
            hinweis: None,
        }
    }

    #[test]
    fn a_fixed_and_an_earliest_date_are_alternatives() {
        // ORDERS AHB conditions [55]/[56]: DTM+203 is Muss when DTM+469 is
        // absent and vice versa, so a message carrying both is malformed.
        let mut r = req();
        r.ausfuehrung_am = Some(date!(2026 - 09 - 01));
        r.fruehestens_am = Some(date!(2026 - 09 - 03));
        assert!(r.validate().is_err());

        r.fruehestens_am = None;
        assert!(r.validate().is_ok());

        r.ausfuehrung_am = None;
        r.fruehestens_am = Some(date!(2026 - 09 - 03));
        assert!(r.validate().is_ok());
    }

    #[test]
    fn an_order_with_neither_date_is_accepted() {
        // A 17117 Entsperrauftrag carries neither DTM+203 nor DTM+469 — §41f
        // Abs. 7 makes it unverzüglich rather than scheduled.
        let mut r = req();
        r.order_type = OrderType::Entsperrung;
        assert!(r.validate().is_ok());
    }

    #[test]
    fn treffpunkt_country_must_be_an_iso_code() {
        let mut r = req();
        r.treffpunkt.land = Some("Deutschland".to_owned());
        assert!(r.validate().is_err());
        r.treffpunkt.land = Some("de".to_owned());
        assert!(
            r.validate().is_err(),
            "NAD 3207 is the upper-case alpha-2 code"
        );
        r.treffpunkt.land = Some("DE".to_owned());
        assert!(r.validate().is_ok());
    }

    #[test]
    fn the_retry_budget_is_bounded() {
        // A dispatch that keeps failing is a wrong-state problem, not a
        // transport one; retrying forever hides it behind an attempt counter.
        const { assert!(IFTSTA_MAX_ATTEMPTS > 0 && IFTSTA_MAX_ATTEMPTS <= 32) }
    }

    /// The execution window is 6 Werktage, counted with the BDEW-MaKo calendar
    /// from the frühestmöglicher Sperrtermin — not 6 calendar days.
    #[test]
    fn the_execution_window_is_six_werktage() {
        use time::{Date, Month};
        let start = Date::from_calendar_date(2026, Month::March, 2).expect("Mon 2026-03-02");
        // Mon +6 WT = the following Tuesday.
        assert_eq!(
            ausfuehrung_faellig_am(Some(start), None),
            Date::from_calendar_date(2026, Month::March, 10).ok()
        );
        // The earliest-start date is used when no fixed date was given.
        assert_eq!(
            ausfuehrung_faellig_am(None, Some(start)),
            Date::from_calendar_date(2026, Month::March, 10).ok()
        );
        // An Entsperrauftrag carries neither date — „unverzüglich" governs and
        // there is no computed Frist to miss.
        assert_eq!(ausfuehrung_faellig_am(None, None), None);
    }

    /// The IFTSTA is due the first Werktag after completion; a Friday
    /// completion is due Monday, not Saturday.
    #[test]
    fn the_iftsta_window_is_one_werktag_after_completion() {
        use time::{Date, Month};
        let friday = Date::from_calendar_date(2026, Month::March, 6).expect("Fri");
        assert_eq!(
            iftsta_faellig_am(friday),
            Date::from_calendar_date(2026, Month::March, 9).expect("Mon")
        );
    }

    /// Two Sperrversuche, and the CHECK constraint agrees with the constant.
    #[test]
    fn the_festlegung_allows_two_sperrversuche() {
        const { assert!(MAX_SPERRVERSUCHE == 2) }
    }

    #[test]
    fn the_column_list_covers_every_row_field() {
        // `SperrOrderRow` is decoded by name, so a field added to the struct but
        // not to ORDER_COLUMNS fails at runtime on the first query — in
        // production, on a route nobody ran in CI.
        let selected: Vec<&str> = ORDER_COLUMNS
            .split(',')
            .map(|c| c.trim().split("::").next().unwrap_or_default().trim())
            .collect();
        for field in [
            "id",
            "tenant",
            "malo_id",
            "lf_mp_id",
            "order_type",
            "pruefidentifikator",
            "process_id",
            "ausfuehrung_am",
            "fruehestens_am",
            "arbeitszeit",
            "treffpunkt_hinweis",
            "treffpunkt_strasse",
            "treffpunkt_plz",
            "treffpunkt_ort",
            "treffpunkt_land",
            "hinweis",
            "status",
            "executed_at",
            "execution_note",
            "fail_reason",
            "pruefschritt_code",
            "iftsta_ref",
            "iftsta_dispatched_at",
            "iftsta_attempts",
            "iftsta_last_error",
            "created_at",
            "updated_at",
        ] {
            assert!(
                selected.contains(&field),
                "ORDER_COLUMNS is missing {field}"
            );
        }
    }
}

#[cfg(test)]
mod vorlauf_tests {
    use super::*;
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    /// The two windows the Festlegung states, and the fact that the wire tells
    /// them apart on its own: `DTM+203` is termingebunden, `DTM+469` is not.
    ///
    /// Reading both as 6 WT accepts a termingebundener Auftrag six Werktage
    /// short — one the NB then cannot execute as instructed, because the date,
    /// time and place are fixed.
    #[test]
    fn a_termingebundener_auftrag_needs_twice_the_lead() {
        // Sperrtermin Monday 2026-11-02.
        let sperrtermin = d(2026, Month::November, 2);
        // 6 WT before is Friday 2026-10-23; 12 WT before is Thursday 2026-10-15.
        let ut = d(2026, Month::October, 20);

        let locker = vorlauffrist_befund(None, Some(sperrtermin), ut).expect("DTM+469");
        assert!(locker.eingehalten, "6 WT is met on {ut}");
        assert_eq!(locker.obligation.key, "gpke.sperrauftrag");

        let strikter = vorlauffrist_befund(Some(sperrtermin), None, ut).expect("DTM+203");
        assert!(!strikter.eingehalten, "12 WT is not met on {ut}");
        assert_eq!(strikter.obligation.key, "gpke.sperrauftrag.termingebunden");
        assert!(strikter.spaetester_ut < locker.spaetester_ut);
    }

    /// An Entsperrauftrag carries neither date, and § 41f Abs. 7 EnWG makes
    /// restoration „unverzüglich" — there is no anchor to count back from, so
    /// there is no verdict to record either.
    #[test]
    fn an_order_without_a_sperrtermin_has_no_vorlauffrist() {
        assert!(vorlauffrist_befund(None, None, d(2026, Month::October, 20)).is_none());
    }

    /// GPKE Teil 2 § 3.5.1.2 Nr. 2 puts a floor on the **NB**: without a
    /// generelle MSB-Zustimmung the Sperrtermin must leave room for the
    /// Anfrage (3 WT before it) *and* the MSB's answer (3 WT to give it).
    #[test]
    fn the_earliest_sperrtermin_leaves_the_msb_its_window() {
        // Monday 2026-11-02 + 6 WT = Tuesday 2026-11-10.
        let heute = d(2026, Month::November, 2);
        assert_eq!(
            fruehester_sperrtermin_ohne_msb_zustimmung(heute),
            d(2026, Month::November, 10)
        );
        // …which is exactly the Anfrage window plus the answer window.
        assert_eq!(
            mako_fristen::vorlauf::SPERRUNG_MSB_ANFRAGE_WT + MSB_ANTWORT_WERKTAGE,
            6
        );
    }
}
