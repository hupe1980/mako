//! Booking drafts — how a business change and its ledger entry are committed
//! together although the ledger joins no external transaction.
//!
//! The ledger owns its pool (doubleentry's design: "takes no external pool or
//! connection and joins no external transaction"), so a dunning step and its
//! Mahngebühr, or a bank import and its payments, cannot share a commit. What
//! can is the **draft**: [`stage`] writes the whole [`PostEntry`] into
//! `booking_drafts` inside the caller's transaction, and [`book`] records it
//! right after the commit. The ledger is idempotent by key, so a retry —
//! inline, or by [`recover`] after a crash — books the draft once:
//!
//! | Ledger answer | Draft becomes |
//! |---|---|
//! | recorded, or replayed | `gebucht`, with the entry id |
//! | a transient failure, or one that may have applied | stays `offen`; retried |
//! | a refusal (`301` sealed period, …) | `geparkt` with the code — a person decides |
//! | integrity / configuration | stays `offen`; the failure is the operator's |
//!
//! A parked draft is listed by [`parked`] and resolved by [`rebook`] on an
//! open date or by [`discard`] with a reason; `cargo xtask check-ledger-drafts`
//! keeps every other module from calling the ledger's write methods.

use anyhow::Context as _;
use serde::Serialize;
use sqlx::{PgConnection, PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::ledger::{LedgerError, PgLedger, PostEntry, entry_uuid};

/// What [`book`] did with a draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Booked {
    /// The entry is in the ledger; `is_new` is `false` for a replay.
    Gebucht {
        /// The ledger entry.
        entry_id: Uuid,
        /// Whether this call recorded it.
        is_new: bool,
    },
    /// The ledger refused it for good; a person decides.
    Geparkt {
        /// doubleentry's rejection code, when the ledger refused.
        code: Option<u16>,
    },
    /// Discarded earlier with a reason.
    Verworfen,
}

impl Booked {
    /// The entry id when booked.
    #[must_use]
    pub fn entry_id(self) -> Option<Uuid> {
        match self {
            Self::Gebucht { entry_id, .. } => Some(entry_id),
            _ => None,
        }
    }
}

/// A draft could not be staged or booked.
#[derive(Debug, thiserror::Error)]
pub enum BookingError {
    /// The draft is refused before anything is written — an unknown entry
    /// type, a zero amount — or its key already holds a different draft.
    #[error("{0}")]
    Refused(String),
    /// No parked booking has the key — it was rebooked or discarded already,
    /// or never parked.
    #[error("no parked booking {0}")]
    NotParked(String),
    /// The ledger failed; the draft stays `offen` and is retried.
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    /// accountingd's own database failed.
    #[error(transparent)]
    Database(#[from] anyhow::Error),
}

impl BookingError {
    /// The answer a handler gives: a refusal `422`, a ledger failure as
    /// [`LedgerError::to_api`] answers it, a database failure `500`.
    #[must_use]
    pub fn to_api(&self) -> mako_service::ApiError {
        match self {
            Self::Refused(msg) => mako_service::ApiError::unprocessable(msg.clone()),
            Self::NotParked(key) => {
                mako_service::ApiError::not_found(format!("no parked booking {key}"))
            }
            Self::Ledger(e) => e.to_api(),
            Self::Database(e) => mako_service::ApiError::internal(anyhow::anyhow!("{e:#}")),
        }
    }
}

impl From<BookingError> for mako_service::ApiError {
    fn from(e: BookingError) -> Self {
        e.to_api()
    }
}

/// Stage `draft` inside the caller's transaction; returns the id the entry
/// will have, so the caller's own rows can name it in the same commit.
///
/// Validates it first, so a draft the ledger could never take is refused
/// before any row is written. Staging the same key twice with the same draft is
/// a no-op; with a different draft it is refused, as the ledger would refuse
/// it (`200`).
///
/// # Errors
///
/// [`BookingError::Refused`] for an invalid draft or a key holding another
/// draft; [`BookingError::Database`] for a failed write.
pub async fn stage(
    conn: &mut PgConnection,
    ledger: &PgLedger,
    tenant: &str,
    draft: &PostEntry,
) -> Result<Uuid, BookingError> {
    draft
        .validate()
        .map_err(|e| BookingError::Refused(e.to_string()))?;
    let entry_id = ledger
        .entry_id_for(&draft.idempotency)
        .map_err(|e| BookingError::Refused(e.to_string()))?;
    let entwurf = serde_json::to_value(draft).context("serialise booking draft")?;
    let stored: serde_json::Value = sqlx::query_scalar(
        "INSERT INTO booking_drafts (tenant, key, entwurf) VALUES ($1, $2, $3) \
         ON CONFLICT (tenant, key) DO UPDATE SET key = EXCLUDED.key \
         RETURNING entwurf",
    )
    .bind(tenant)
    .bind(&draft.idempotency)
    .bind(&entwurf)
    .fetch_one(&mut *conn)
    .await
    .context("stage booking draft")?;
    if stored != entwurf {
        return Err(BookingError::Refused(format!(
            "idempotency key {} already holds a different booking",
            draft.idempotency
        )));
    }
    Ok(entry_id)
}

/// Book the staged draft under `key`, after the staging transaction committed.
///
/// Locks the draft row for the duration, so the inline call and [`recover`]
/// never book the same draft concurrently — and if they did, the ledger's
/// idempotency would make the second a replay.
///
/// # Errors
///
/// [`BookingError::Ledger`] when the ledger failed or refused (a refusal parks
/// the draft first); [`BookingError::Database`] for accountingd's own tables.
pub async fn book(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    key: &str,
) -> Result<Booked, BookingError> {
    let mut tx = pool.begin().await.context("begin booking")?;
    let row = sqlx::query(
        "SELECT entwurf, status, code, entry_id FROM booking_drafts \
         WHERE tenant = $1 AND key = $2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(key)
    .fetch_optional(&mut *tx)
    .await
    .context("read booking draft")?
    .with_context(|| format!("no booking draft {key}"))?;
    let status: String = row.get("status");
    match status.as_str() {
        "gebucht" => {
            return Ok(Booked::Gebucht {
                entry_id: row.get("entry_id"),
                is_new: false,
            });
        }
        "geparkt" => {
            let code: Option<i32> = row.get("code");
            return Ok(Booked::Geparkt {
                code: code.and_then(|c| u16::try_from(c).ok()),
            });
        }
        "verworfen" => return Ok(Booked::Verworfen),
        _ => {}
    }
    let draft: PostEntry =
        serde_json::from_value(row.get("entwurf")).context("decode booking draft")?;

    match ledger.post(&draft).await {
        Ok(posted) => {
            let entry_id = entry_uuid(posted.id);
            sqlx::query(
                "UPDATE booking_drafts SET status = 'gebucht', entry_id = $3, \
                 versuche = versuche + 1, updated_at = now() WHERE tenant = $1 AND key = $2",
            )
            .bind(tenant)
            .bind(key)
            .bind(entry_id)
            .execute(&mut *tx)
            .await
            .context("mark booking draft booked")?;
            tx.commit().await.context("commit booking")?;
            settle(ledger, pool, tenant, &draft).await;
            Ok(Booked::Gebucht {
                entry_id,
                is_new: posted.is_new,
            })
        }
        Err(e @ LedgerError::Refused { .. }) => {
            sqlx::query(
                "UPDATE booking_drafts SET status = 'geparkt', code = $3, meldung = $4, \
                 versuche = versuche + 1, updated_at = now() WHERE tenant = $1 AND key = $2",
            )
            .bind(tenant)
            .bind(key)
            .bind(e.code().map(i32::from))
            .bind(e.to_string())
            .execute(&mut *tx)
            .await
            .context("park booking draft")?;
            tx.commit().await.context("commit parked draft")?;
            tracing::warn!(%key, code = ?e.code(), "booking draft parked: {e}");
            Err(e.into())
        }
        Err(e) => {
            sqlx::query(
                "UPDATE booking_drafts SET versuche = versuche + 1, updated_at = now() \
                 WHERE tenant = $1 AND key = $2",
            )
            .bind(tenant)
            .bind(key)
            .execute(&mut *tx)
            .await
            .context("count booking attempt")?;
            tx.commit().await.context("commit booking attempt")?;
            Err(e.into())
        }
    }
}

/// Stage and book a draft that has no business row of its own to commit with.
///
/// # Errors
///
/// As [`stage`] and [`book`].
pub async fn post(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    draft: &PostEntry,
) -> Result<Booked, BookingError> {
    let mut tx = pool.begin().await.context("begin staging")?;
    stage(&mut tx, ledger, tenant, draft).await?;
    tx.commit().await.context("commit staging")?;
    book(ledger, pool, tenant, &draft.idempotency).await
}

/// [`post`] for a caller that needs the booked entry: a parked or discarded
/// draft is a refusal.
///
/// # Errors
///
/// As [`post`]; a parked draft is [`BookingError::Ledger`] with its refusal.
pub async fn post_booked(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    draft: &PostEntry,
) -> Result<Uuid, BookingError> {
    match post(ledger, pool, tenant, draft).await? {
        Booked::Gebucht { entry_id, .. } => Ok(entry_id),
        Booked::Geparkt { .. } | Booked::Verworfen => Err(BookingError::Refused(format!(
            "booking {} is parked; clear it in the parked list",
            draft.idempotency
        ))),
    }
}

/// What follows a booking: the balance cache set absolutely from the ledger,
/// the FIFO Zahlungszuordnung, then the Verzug cache, which reads residuals
/// and so runs after the clearing. Best-effort: the booking is the record, and
/// each step is reconcilable from it.
async fn settle(ledger: &PgLedger, pool: &PgPool, tenant: &str, draft: &PostEntry) {
    let (malo, lf) = (draft.malo_id.as_str(), draft.lf_mp_id.as_str());
    match ledger.balance_ct(lf, malo).await {
        Ok(net) => {
            let updated = sqlx::query(
                "UPDATE accounts SET balance_ct = $1, updated_at = now() \
                 WHERE malo_id = $2 AND lf_mp_id = $3 AND tenant = $4",
            )
            .bind(net)
            .bind(malo)
            .bind(lf)
            .bind(tenant)
            .execute(pool)
            .await;
            match updated {
                Ok(r) if r.rows_affected() == 0 => tracing::warn!(
                    %malo,
                    "booking: no accounts row for the balance cache (the ledger stays authoritative)"
                ),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(%malo, error = %e, "booking: balance cache refresh failed")
                }
            }
        }
        Err(e) => tracing::warn!(%malo, error = %e, "booking: balance read failed"),
    }
    if let Err(e) = ledger
        .apply_fifo_clearing(lf, malo, draft.booking_date)
        .await
    {
        tracing::warn!(%malo, error = %e, "booking: FIFO clearing skipped");
    }
    if let Err(e) = crate::pg::refresh_verzug(ledger, pool, tenant, malo, lf).await {
        tracing::warn!(%malo, error = %e, "booking: Verzug cache refresh failed");
    }
}

/// How long a draft stays `offen` before [`recover`] takes it: longer than the
/// inline booking takes, so the worker does not race the normal path.
pub const RECOVERY_GRACE_SECS: i64 = 30;

/// Book every draft a crash or a failed booking left `offen`, oldest first.
/// Returns how many were booked or parked.
///
/// # Errors
///
/// A failure reading `booking_drafts`; a single draft's failure is logged and
/// left for the next run.
pub async fn recover(ledger: &PgLedger, pool: &PgPool, tenant: &str) -> anyhow::Result<usize> {
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM booking_drafts WHERE tenant = $1 AND status = 'offen' \
         AND updated_at < now() - $2 * interval '1 second' ORDER BY created_at LIMIT 200",
    )
    .bind(tenant)
    .bind(RECOVERY_GRACE_SECS)
    .fetch_all(pool)
    .await
    .context("list open booking drafts")?;
    let mut done = 0;
    for key in keys {
        match book(ledger, pool, tenant, &key).await {
            Ok(_) | Err(BookingError::Ledger(LedgerError::Refused { .. })) => done += 1,
            Err(e) => tracing::warn!(%key, error = %e, "booking recovery: draft stays open"),
        }
    }
    Ok(done)
}

/// Run [`recover`] every `interval` until `shutdown`.
pub async fn run_recovery(
    ledger: std::sync::Arc<PgLedger>,
    pool: PgPool,
    tenant: String,
    interval: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
        match recover(&ledger, &pool, &tenant).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(n, "booking recovery: drafts booked or parked"),
            Err(e) => tracing::warn!(error = %e, "booking recovery failed"),
        }
    }
}

/// A parked booking, as the Desk shows it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GeparkteBuchung {
    /// The draft's key — what rebook and discard name.
    pub key: String,
    /// The Buchungsart.
    pub art: String,
    /// The Marktlokation.
    pub malo_id: String,
    /// The Lieferant's MP-ID.
    pub lf_mp_id: String,
    /// Signed amount in minor units (positive: the customer owes).
    pub betrag_ct: i64,
    /// The booking date the ledger refused.
    #[schemars(extend("format" = "date"))]
    pub buchungsdatum: String,
    /// The Beleg, when the draft cites one.
    pub beleg: Option<String>,
    /// doubleentry's rejection code; `None` for a refusal accountingd made.
    pub code: Option<u16>,
    /// The refusal in words.
    pub meldung: String,
    /// What the operator can do about it, in words.
    pub hinweis: String,
    /// The first date a rebooking is accepted on, when the refusal is a
    /// sealed period (`301`) — the day after the last seal.
    #[schemars(extend("format" = "date"))]
    pub vorschlag_datum: Option<String>,
    /// When the draft was staged (RFC 3339).
    #[schemars(extend("format" = "date-time"))]
    pub seit: String,
}

/// The parked list, oldest first.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GeparkteBuchungen {
    /// Up to `limit` parked bookings.
    pub buchungen: Vec<GeparkteBuchung>,
    /// How many are parked in all.
    pub count: i64,
    /// Whether `count` exceeds what `buchungen` carries.
    pub saturated: bool,
}

/// What a refusal code asks of the operator.
fn hinweis(code: Option<u16>) -> &'static str {
    match code {
        Some(301) => {
            "Die Periode ist festgeschrieben. Auf ein Datum im ersten offenen Monat neu buchen."
        }
        Some(300) => "Für das Datum ist keine Periode angelegt. Auf ein Datum ab 2015 neu buchen.",
        Some(304) => "Das Konto war an dem Tag nicht eröffnet. Auf ein späteres Datum neu buchen.",
        Some(200) => "Der Schlüssel trägt schon eine andere Buchung. Prüfen und verwerfen.",
        Some(_) => "Das Hauptbuch lehnt die Buchung ab. Prüfen, dann neu buchen oder verwerfen.",
        None => "Die Buchung ist unvollständig. Prüfen und verwerfen.",
    }
}

/// The parked bookings, oldest first, at most `limit`.
///
/// # Errors
///
/// A database or ledger read failure.
pub async fn parked(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> anyhow::Result<GeparkteBuchungen> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM booking_drafts WHERE tenant = $1 AND status = 'geparkt'",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("count parked drafts")?;
    let rows = sqlx::query(
        "SELECT key, entwurf, code, meldung, created_at FROM booking_drafts \
         WHERE tenant = $1 AND status = 'geparkt' ORDER BY created_at LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list parked drafts")?;
    let first_open = first_open_day(ledger).await?;
    let mut buchungen = Vec::with_capacity(rows.len());
    for row in rows {
        buchungen.push(geparkt(&row, first_open.as_deref())?);
    }
    let saturated = count > i64::try_from(buchungen.len()).unwrap_or(i64::MAX);
    Ok(GeparkteBuchungen {
        buchungen,
        count,
        saturated,
    })
}

/// One parked booking by its key; `None` when no parked draft has it.
///
/// # Errors
///
/// A database or ledger read failure.
pub async fn parked_one(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    key: &str,
) -> anyhow::Result<Option<GeparkteBuchung>> {
    let row = sqlx::query(
        "SELECT key, entwurf, code, meldung, created_at FROM booking_drafts \
         WHERE tenant = $1 AND key = $2 AND status = 'geparkt'",
    )
    .bind(tenant)
    .bind(key)
    .fetch_optional(pool)
    .await
    .context("read parked draft")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let first_open = first_open_day(ledger).await?;
    Ok(Some(geparkt(&row, first_open.as_deref())?))
}

/// The first day a booking is accepted on, as ISO text.
async fn first_open_day(ledger: &PgLedger) -> anyhow::Result<Option<String>> {
    Ok(ledger
        .sealed_through()
        .await?
        .and_then(|d| d.next_day())
        .map(|d| d.to_string()))
}

/// A `booking_drafts` row as the Desk shows it.
fn geparkt(
    row: &sqlx::postgres::PgRow,
    first_open: Option<&str>,
) -> anyhow::Result<GeparkteBuchung> {
    let draft: PostEntry =
        serde_json::from_value(row.get("entwurf")).context("decode parked draft")?;
    let code = row
        .get::<Option<i32>, _>("code")
        .and_then(|c| u16::try_from(c).ok());
    let seit: OffsetDateTime = row.get("created_at");
    Ok(GeparkteBuchung {
        key: row.get("key"),
        art: draft.entry_type,
        malo_id: draft.malo_id,
        lf_mp_id: draft.lf_mp_id,
        betrag_ct: draft.amount_ct,
        buchungsdatum: draft.booking_date.to_string(),
        beleg: draft.document,
        code,
        meldung: row.get("meldung"),
        hinweis: hinweis(code).to_owned(),
        vorschlag_datum: (code == Some(301))
            .then(|| first_open.map(str::to_owned))
            .flatten(),
        seit: seit
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    })
}

/// The key a rebooking of `key` on `on` books under — derived, so it stays
/// inside doubleentry's 128-byte limit however long the original key is, and a
/// repeated rebooking on the same day is the same booking.
#[must_use]
pub fn rebook_key(key: &str, on: Date) -> String {
    let digest = blake3::hash(key.as_bytes());
    let hex: String = digest.as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("neu:{hex}:{on}")
}

/// Rebook a parked draft on `on`: the successor is staged and the parked one
/// marked `verworfen` in one transaction, then the successor is booked.
///
/// # Errors
///
/// [`BookingError::Refused`] when `key` is not parked; as [`book`].
pub async fn rebook(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    key: &str,
    on: Date,
    actor: &str,
) -> Result<Booked, BookingError> {
    let mut tx = pool.begin().await.context("begin rebooking")?;
    let entwurf: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT entwurf FROM booking_drafts WHERE tenant = $1 AND key = $2 \
         AND status = 'geparkt' FOR UPDATE",
    )
    .bind(tenant)
    .bind(key)
    .fetch_optional(&mut *tx)
    .await
    .context("read parked draft")?;
    let Some(entwurf) = entwurf else {
        return Err(BookingError::NotParked(key.to_owned()));
    };
    let mut draft: PostEntry = serde_json::from_value(entwurf).context("decode parked draft")?;
    let successor = rebook_key(key, on);
    draft.idempotency.clone_from(&successor);
    draft.booking_date = on;
    draft.value_date = on;
    draft.actor = Some(actor.to_owned());
    stage(&mut tx, ledger, tenant, &draft).await?;
    sqlx::query(
        "UPDATE booking_drafts SET status = 'verworfen', erledigt_von = $3, grund = $4, \
         updated_at = now() WHERE tenant = $1 AND key = $2",
    )
    .bind(tenant)
    .bind(key)
    .bind(actor)
    .bind(format!("neu gebucht: {successor}"))
    .execute(&mut *tx)
    .await
    .context("retire parked draft")?;
    tx.commit().await.context("commit rebooking")?;
    book(ledger, pool, tenant, &successor).await
}

/// Discard a parked draft with a reason.
///
/// # Errors
///
/// [`BookingError::Refused`] when `key` is not parked or `grund` is blank.
pub async fn discard(
    pool: &PgPool,
    tenant: &str,
    key: &str,
    grund: &str,
    actor: &str,
) -> Result<(), BookingError> {
    if grund.trim().is_empty() {
        return Err(BookingError::Refused("a discard needs a reason".to_owned()));
    }
    let updated = sqlx::query(
        "UPDATE booking_drafts SET status = 'verworfen', erledigt_von = $3, grund = $4, \
         updated_at = now() WHERE tenant = $1 AND key = $2 AND status = 'geparkt'",
    )
    .bind(tenant)
    .bind(key)
    .bind(actor)
    .bind(grund.trim())
    .execute(pool)
    .await
    .context("discard parked draft")?;
    if updated.rows_affected() == 0 {
        return Err(BookingError::NotParked(key.to_owned()));
    }
    Ok(())
}

/// Entry type and Beleg per booked entry, from the drafts that booked them —
/// one query for a statement page instead of one ledger read per line.
///
/// # Errors
///
/// A database failure.
pub async fn describe(
    pool: &PgPool,
    tenant: &str,
    entries: &[Uuid],
) -> anyhow::Result<std::collections::HashMap<Uuid, (String, Option<String>)>> {
    let rows = sqlx::query(
        "SELECT entry_id, entwurf->>'entry_type' AS art, entwurf->>'document' AS beleg \
         FROM booking_drafts WHERE tenant = $1 AND entry_id = ANY($2)",
    )
    .bind(tenant)
    .bind(entries)
    .fetch_all(pool)
    .await
    .context("describe booked entries")?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("entry_id"), (r.get("art"), r.get("beleg"))))
        .collect())
}

/// One booked entry of a given type, with the customer it belongs to.
#[derive(Debug, Clone)]
pub struct KindEntry {
    /// The LF market-partner id of the owning Kontokorrent.
    pub lf_mp_id: String,
    /// The Marktlokation of the owning Kontokorrent.
    pub malo_id: String,
    /// The ledger entry.
    pub entry_id: Uuid,
    /// The signed amount in minor units.
    pub amount_ct: i64,
    /// The originating CloudEvent id, if any.
    pub correlation: Option<String>,
}

/// Every booked entry of `art` dated in `(year, month)` — the input to the
/// batch EEG-payout run.
///
/// Read from the drafts that booked them: a draft is `gebucht` only once the
/// ledger recorded it, and it carries the customer, the amount and the
/// correlation the ledger entry holds as postings and dimensions — one indexed
/// query instead of a scan of the log.
///
/// # Errors
///
/// A database failure, or a stored draft that does not decode.
pub async fn booked_of_kind_in_month(
    pool: &PgPool,
    tenant: &str,
    art: &str,
    year: i32,
    month: u8,
) -> anyhow::Result<Vec<KindEntry>> {
    let rows = sqlx::query(
        "SELECT entry_id, entwurf FROM booking_drafts          WHERE tenant = $1 AND status = 'gebucht' AND entwurf->>'entry_type' = $2          AND date_part('year', (entwurf->>'booking_date')::date) = $3          AND date_part('month', (entwurf->>'booking_date')::date) = $4          ORDER BY created_at",
    )
    .bind(tenant)
    .bind(art)
    .bind(year)
    .bind(i32::from(month))
    .fetch_all(pool)
    .await
    .context("list booked entries of a type")?;
    rows.into_iter()
        .map(|r| {
            let draft: PostEntry =
                serde_json::from_value(r.get("entwurf")).context("decode booked draft")?;
            Ok(KindEntry {
                lf_mp_id: draft.lf_mp_id,
                malo_id: draft.malo_id,
                entry_id: r.get("entry_id"),
                amount_ct: draft.amount_ct,
                correlation: draft.correlation,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rebook_key_fits_the_ledger_and_is_stable() {
        let long = "x".repeat(128);
        let on = time::macros::date!(2026 - 02 - 01);
        let key = rebook_key(&long, on);
        assert!(key.len() <= 128, "{key}");
        assert_eq!(key, rebook_key(&long, on));
        assert_ne!(key, rebook_key("y", on));
    }

    #[test]
    fn every_hint_names_an_action() {
        for code in [Some(301), Some(300), Some(304), Some(200), Some(999), None] {
            let h = hinweis(code);
            assert!(h.contains("buchen") || h.contains("verwerfen"), "{h}");
        }
    }
}
