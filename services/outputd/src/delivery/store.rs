//! Persistence for issued documents and their delivery attempts.
//!
//! Append-only for the content columns, like [`crate::template_store`] and for
//! the same statute (§ 147 AO): a corrected document is a new row. The delivery
//! rows beside it advance in place — an attempt track is mutable state, which
//! is what `attempts` counts.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;
use uuid::Uuid;

use super::channel::Channel;

/// A document to record, with the bytes that were produced for it.
#[derive(Debug)]
pub struct NewDocument<'a> {
    pub tenant: &'a str,
    pub kind: &'a str,
    pub template_hash: &'a str,
    /// What this document is *about* — a Rechnungsnummer, a dunning-case id, a
    /// slice id. Unique per `(tenant, kind)`, so an issuing service's retry
    /// returns the existing document instead of sending a second notice.
    pub subject_ref: &'a str,
    pub malo_id: Option<&'a str>,
    pub kunden_nr: Option<&'a str>,
    pub content: &'a [u8],
    pub media_type: &'a str,
    pub recipient: Recipient,
    pub issued_by: Option<&'a str>,
}

/// Where a document is addressed, snapshotted at issue.
///
/// A copy rather than a reference into `vertragd`: a dispute asks where the
/// notice *was sent*, which live master data cannot answer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recipient {
    /// The addressee as printed.
    #[serde(default)]
    pub name: Option<String>,
    /// The address the `EMAIL` channel uses. Absent → that channel is
    /// suppressed with a reason rather than silently skipped.
    #[serde(default)]
    pub email: Option<String>,
    /// The postal address the `POST` channel prints, free-form JSON so the
    /// print service's own schema can travel unchanged.
    #[serde(default)]
    pub address: Option<serde_json::Value>,
}

/// A stored document, without its bytes.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentRow {
    pub document_id: Uuid,
    pub kind: String,
    pub template_hash: String,
    pub subject_ref: String,
    pub malo_id: Option<String>,
    pub kunden_nr: Option<String>,
    pub content_sha256: String,
    pub byte_size: i32,
    pub media_type: String,
    pub recipient_name: Option<String>,
    pub recipient_email: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
}

/// One channel's attempt track for one document.
#[derive(Debug, Clone, Serialize)]
pub struct DeliveryRow {
    pub delivery_id: Uuid,
    pub document_id: Uuid,
    pub channel: String,
    pub status: String,
    pub target: Option<String>,
    pub attempts: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub next_attempt_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub first_sent_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub delivered_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub read_at: Option<OffsetDateTime>,
    pub evidence: Option<serde_json::Value>,
    pub last_error: Option<String>,
}

/// A document plus the state of every channel it was queued on.
#[derive(Debug, Clone, Serialize)]
pub struct IssuedDocument {
    #[serde(flatten)]
    pub document: DocumentRow,
    pub deliveries: Vec<DeliveryRow>,
}

fn document_row(r: &sqlx::postgres::PgRow) -> Result<DocumentRow> {
    Ok(DocumentRow {
        document_id: r.try_get("document_id")?,
        kind: r.try_get("kind")?,
        template_hash: r.try_get("template_hash")?,
        subject_ref: r.try_get("subject_ref")?,
        malo_id: r.try_get("malo_id")?,
        kunden_nr: r.try_get("kunden_nr")?,
        content_sha256: r.try_get("content_sha256")?,
        byte_size: r.try_get("byte_size")?,
        media_type: r.try_get("media_type")?,
        recipient_name: r.try_get("recipient_name")?,
        recipient_email: r.try_get("recipient_email")?,
        issued_at: r.try_get("issued_at")?,
    })
}

fn delivery_row(r: &sqlx::postgres::PgRow) -> Result<DeliveryRow> {
    Ok(DeliveryRow {
        delivery_id: r.try_get("delivery_id")?,
        document_id: r.try_get("document_id")?,
        channel: r.try_get("channel")?,
        status: r.try_get("status")?,
        target: r.try_get("target")?,
        attempts: r.try_get("attempts")?,
        next_attempt_at: r.try_get("next_attempt_at")?,
        first_sent_at: r.try_get("first_sent_at")?,
        delivered_at: r.try_get("delivered_at")?,
        read_at: r.try_get("read_at")?,
        evidence: r.try_get("evidence")?,
        last_error: r.try_get("last_error")?,
    })
}

/// SHA-256 of the stored bytes, lowercase hex — the same content-addressing the
/// template store uses, so "is this the document that was sent?" is answerable
/// by hashing a copy.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Record a document and queue it on `channels`, in one transaction.
///
/// **Idempotent on `(tenant, kind, subject_ref)`.** A retrying issuer gets the
/// document it already issued: a duplicate row is untidy, a duplicate *Mahnung*
/// is a second statutory notice with its own deadline and a second § 41f clock.
///
/// A channel with nothing to send to (`EMAIL` with no address on file) is
/// stored `SUPPRESSED` with the reason, never omitted — "why did this never go
/// out" has to be answerable from the row, not from its absence.
///
/// Returns the document, its queued deliveries, and whether it was new.
///
/// # Errors
///
/// Propagates database errors.
pub async fn issue(
    pool: &PgPool,
    doc: &NewDocument<'_>,
    channels: &[Channel],
) -> Result<(IssuedDocument, bool)> {
    // The already-issued case first, and outside the write: the common retry
    // path then costs one indexed read.
    if let Some(existing) = by_subject(pool, doc.tenant, doc.kind, doc.subject_ref).await? {
        return Ok((existing, false));
    }

    let sha = content_hash(doc.content);
    let byte_size = i32::try_from(doc.content.len())
        .context("document exceeds 2 GiB — refused rather than truncated")?;
    anyhow::ensure!(byte_size > 0, "refusing to issue an empty document");

    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        r"INSERT INTO documents
            (tenant, kind, template_hash, subject_ref, malo_id, kunden_nr,
             content, content_sha256, byte_size, media_type,
             recipient_name, recipient_email, recipient_address, issued_by)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
          ON CONFLICT (tenant, kind, subject_ref) DO NOTHING
          RETURNING document_id, kind, template_hash, subject_ref, malo_id, kunden_nr,
                    content_sha256, byte_size, media_type,
                    recipient_name, recipient_email, issued_at",
    )
    .bind(doc.tenant)
    .bind(doc.kind)
    .bind(doc.template_hash)
    .bind(doc.subject_ref)
    .bind(doc.malo_id)
    .bind(doc.kunden_nr)
    .bind(doc.content)
    .bind(&sha)
    .bind(byte_size)
    .bind(doc.media_type)
    .bind(doc.recipient.name.as_deref())
    .bind(doc.recipient.email.as_deref())
    .bind(doc.recipient.address.as_ref())
    .bind(doc.issued_by)
    .fetch_optional(&mut *tx)
    .await
    .context("insert document")?;

    let Some(row) = inserted else {
        // Lost the race against a concurrent issue of the same subject.
        tx.rollback().await.ok();
        let existing = by_subject(pool, doc.tenant, doc.kind, doc.subject_ref)
            .await?
            .context("document vanished between conflict and read")?;
        return Ok((existing, false));
    };
    let document = document_row(&row)?;

    let mut deliveries = Vec::with_capacity(channels.len());
    for channel in channels {
        let target = channel.target_for(&doc.recipient);
        let (status, last_error) = match (channel, &target) {
            // The portal needs no target: the document is served from this
            // store, so publishing it *is* the delivery.
            (Channel::Portal, _) => ("PENDING", None),
            (_, Some(_)) => ("PENDING", None),
            (Channel::Email, None) => (
                "SUPPRESSED",
                Some("no e-mail address on file for this recipient"),
            ),
            (Channel::Post, None) => (
                "SUPPRESSED",
                Some("no postal address on file for this recipient"),
            ),
            (Channel::Erp, None) => ("PENDING", None),
        };
        let r = sqlx::query(
            r"INSERT INTO document_deliveries
                (document_id, tenant, channel, status, target, last_error)
              VALUES ($1,$2,$3,$4,$5,$6)
              RETURNING delivery_id, document_id, channel, status, target, attempts,
                        next_attempt_at, first_sent_at, delivered_at, read_at,
                        evidence, last_error",
        )
        .bind(document.document_id)
        .bind(doc.tenant)
        .bind(channel.as_str())
        .bind(status)
        .bind(target)
        .bind(last_error)
        .fetch_one(&mut *tx)
        .await
        .context("queue delivery")?;
        deliveries.push(delivery_row(&r)?);
    }
    tx.commit().await?;

    Ok((
        IssuedDocument {
            document,
            deliveries,
        },
        true,
    ))
}

/// The document issued for `(tenant, kind, subject_ref)`, with its deliveries.
///
/// # Errors
///
/// Propagates database errors.
pub async fn by_subject(
    pool: &PgPool,
    tenant: &str,
    kind: &str,
    subject_ref: &str,
) -> Result<Option<IssuedDocument>> {
    let row = sqlx::query(
        r"SELECT document_id, kind, template_hash, subject_ref, malo_id, kunden_nr,
                 content_sha256, byte_size, media_type,
                 recipient_name, recipient_email, issued_at
          FROM documents
          WHERE tenant = $1 AND kind = $2 AND subject_ref = $3",
    )
    .bind(tenant)
    .bind(kind)
    .bind(subject_ref)
    .fetch_optional(pool)
    .await
    .context("document by subject")?;
    let Some(row) = row else { return Ok(None) };
    let document = document_row(&row)?;
    let deliveries = deliveries_of(pool, document.document_id).await?;
    Ok(Some(IssuedDocument {
        document,
        deliveries,
    }))
}

/// One document by id, tenant-scoped.
///
/// # Errors
///
/// Propagates database errors.
pub async fn by_id(pool: &PgPool, tenant: &str, id: Uuid) -> Result<Option<IssuedDocument>> {
    let row = sqlx::query(
        r"SELECT document_id, kind, template_hash, subject_ref, malo_id, kunden_nr,
                 content_sha256, byte_size, media_type,
                 recipient_name, recipient_email, issued_at
          FROM documents WHERE tenant = $1 AND document_id = $2",
    )
    .bind(tenant)
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("document by id")?;
    let Some(row) = row else { return Ok(None) };
    let document = document_row(&row)?;
    let deliveries = deliveries_of(pool, document.document_id).await?;
    Ok(Some(IssuedDocument {
        document,
        deliveries,
    }))
}

/// The stored bytes and their media type — the § 147 AO reproduction.
///
/// # Errors
///
/// Propagates database errors.
pub async fn content(pool: &PgPool, tenant: &str, id: Uuid) -> Result<Option<(Vec<u8>, String)>> {
    let row = sqlx::query(
        "SELECT content, media_type FROM documents WHERE tenant = $1 AND document_id = $2",
    )
    .bind(tenant)
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("document content")?;
    row.map(|r| Ok((r.try_get("content")?, r.try_get("media_type")?)))
        .transpose()
}

/// Every delivery track of one document.
///
/// # Errors
///
/// Propagates database errors.
pub async fn deliveries_of(pool: &PgPool, document_id: Uuid) -> Result<Vec<DeliveryRow>> {
    let rows = sqlx::query(
        r"SELECT delivery_id, document_id, channel, status, target, attempts,
                 next_attempt_at, first_sent_at, delivered_at, read_at, evidence, last_error
          FROM document_deliveries WHERE document_id = $1 ORDER BY channel",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await
    .context("deliveries of document")?;
    rows.iter().map(delivery_row).collect()
}

/// Filters for the document list — the portal inbox and the operator's search.
#[derive(Debug, Default, Deserialize)]
pub struct DocumentFilter {
    pub kind: Option<String>,
    pub malo_id: Option<String>,
    pub kunden_nr: Option<String>,
    pub limit: Option<i64>,
}

/// Documents matching `filter`, newest first.
///
/// At least one of `malo_id` / `kunden_nr` must be set: no caller here wants
/// every document a tenant ever issued, and offering it leaves the portal one
/// bug away from serving it.
///
/// # Errors
///
/// Propagates database errors; returns `Ok(vec![])` for an unscoped filter.
pub async fn list(
    pool: &PgPool,
    tenant: &str,
    filter: &DocumentFilter,
) -> Result<Vec<DocumentRow>> {
    if filter.malo_id.is_none() && filter.kunden_nr.is_none() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r"SELECT document_id, kind, template_hash, subject_ref, malo_id, kunden_nr,
                 content_sha256, byte_size, media_type,
                 recipient_name, recipient_email, issued_at
          FROM documents
          WHERE tenant = $1
            AND ($2::text IS NULL OR kind = $2)
            AND ($3::text IS NULL OR malo_id = $3)
            AND ($4::text IS NULL OR kunden_nr = $4)
          ORDER BY issued_at DESC
          LIMIT $5",
    )
    .bind(tenant)
    .bind(filter.kind.as_deref())
    .bind(filter.malo_id.as_deref())
    .bind(filter.kunden_nr.as_deref())
    .bind(filter.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(pool)
    .await
    .context("list documents")?;
    rows.iter().map(document_row).collect()
}

/// One pending delivery, claimed by the worker, with what it needs to send.
#[derive(Debug, Clone)]
pub struct PendingDelivery {
    pub delivery_id: Uuid,
    pub document_id: Uuid,
    pub channel: Channel,
    pub target: Option<String>,
    pub attempts: i32,
    pub kind: String,
    pub subject_ref: String,
    pub malo_id: Option<String>,
    pub kunden_nr: Option<String>,
    pub recipient_name: Option<String>,
    pub media_type: String,
}

/// Claim up to `limit` deliveries that are due, marking them in flight.
///
/// `FOR UPDATE SKIP LOCKED` plus an immediate `next_attempt_at` push: two
/// replicas never send the same document at once, and a replica that dies
/// mid-send releases its claim when the backoff elapses. At-least-once, which
/// is tolerable only because the far end can deduplicate — the portal
/// republishes the same row, and the relay body carries the delivery id.
///
/// `postal_push` says whether this deployment has a postal relay to push to.
/// With `false`, `POST` rows are not claimed at all: they are letters a print
/// service collects from [`postal_spool`], and a claim is the first step of a
/// path that ends in `FAILED` — which would delete them from that spool, since
/// it lists `PENDING` rows. The pull model is a supported integration, not a
/// broken push, so it must not consume the retry budget.
///
/// # Errors
///
/// Propagates database errors.
pub async fn claim_due(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
    retry_after: time::Duration,
    postal_push: bool,
) -> Result<Vec<PendingDelivery>> {
    let rows = sqlx::query(
        r"WITH due AS (
              SELECT dd.delivery_id
              FROM document_deliveries dd
              WHERE dd.tenant = $1 AND dd.status = 'PENDING' AND dd.next_attempt_at <= now()
                AND ($4 OR dd.channel <> 'POST')
              ORDER BY dd.next_attempt_at
              LIMIT $2
              FOR UPDATE SKIP LOCKED
          )
          UPDATE document_deliveries dd
             SET attempts        = dd.attempts + 1,
                 next_attempt_at = now() + $3::interval,
                 updated_at      = now()
            FROM due, documents d
           WHERE dd.delivery_id = due.delivery_id AND d.document_id = dd.document_id
          RETURNING dd.delivery_id, dd.document_id, dd.channel, dd.target, dd.attempts,
                    d.kind, d.subject_ref, d.malo_id, d.kunden_nr,
                    d.recipient_name, d.media_type",
    )
    .bind(tenant)
    .bind(limit)
    .bind(retry_after)
    .bind(postal_push)
    .fetch_all(pool)
    .await
    .context("claim due deliveries")?;

    rows.iter()
        .map(|r| {
            let channel: String = r.try_get("channel")?;
            Ok(PendingDelivery {
                delivery_id: r.try_get("delivery_id")?,
                document_id: r.try_get("document_id")?,
                channel: Channel::parse(&channel).with_context(|| {
                    format!("stored channel {channel} is not one this build knows")
                })?,
                target: r.try_get("target")?,
                attempts: r.try_get("attempts")?,
                kind: r.try_get("kind")?,
                subject_ref: r.try_get("subject_ref")?,
                malo_id: r.try_get("malo_id")?,
                kunden_nr: r.try_get("kunden_nr")?,
                recipient_name: r.try_get("recipient_name")?,
                media_type: r.try_get("media_type")?,
            })
        })
        .collect()
}

/// Record what a channel reported for one delivery.
///
/// `delivered` separates handed off (`SENT`) from known to have arrived
/// (`DELIVERED`). Only the second is evidence, and only channels that can
/// observe arrival pass `true`.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_success(
    pool: &PgPool,
    tenant: &str,
    delivery_id: Uuid,
    delivered: bool,
    evidence: Option<serde_json::Value>,
) -> Result<()> {
    sqlx::query(
        r"UPDATE document_deliveries
             SET status        = CASE WHEN $3 THEN 'DELIVERED' ELSE 'SENT' END,
                 first_sent_at = COALESCE(first_sent_at, now()),
                 delivered_at  = CASE WHEN $3 THEN now() ELSE delivered_at END,
                 evidence      = COALESCE($4, evidence),
                 last_error    = NULL,
                 updated_at    = now()
           WHERE tenant = $1 AND delivery_id = $2",
    )
    .bind(tenant)
    .bind(delivery_id)
    .bind(delivered)
    .bind(evidence)
    .execute(pool)
    .await
    .context("record delivery success")?;
    Ok(())
}

/// Record a failed attempt, scheduling a retry or giving up.
///
/// The row stays `PENDING` while retries remain and becomes `FAILED` at the
/// ceiling — the state that says a customer never received something the
/// platform believes it sent.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_failure(
    pool: &PgPool,
    tenant: &str,
    delivery_id: Uuid,
    error: &str,
    give_up: bool,
    retry_after: time::Duration,
) -> Result<()> {
    sqlx::query(
        r"UPDATE document_deliveries
             SET status          = CASE WHEN $4 THEN 'FAILED' ELSE 'PENDING' END,
                 next_attempt_at = now() + $5::interval,
                 last_error      = $3,
                 updated_at      = now()
           WHERE tenant = $1 AND delivery_id = $2",
    )
    .bind(tenant)
    .bind(delivery_id)
    .bind(error)
    .bind(give_up)
    .bind(retry_after)
    .execute(pool)
    .await
    .context("record delivery failure")?;
    Ok(())
}

/// Record that the recipient opened the document in the portal.
///
/// More than Textform asks for (§ 126b BGB is satisfied once the document is on
/// a durable medium in the recipient's sphere) and exactly what a § 41f dispute
/// asks about. Set once; a second read does not move it.
///
/// Returns `false` when there is no such portal delivery for this tenant.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_read(pool: &PgPool, tenant: &str, delivery_id: Uuid) -> Result<bool> {
    let n = sqlx::query(
        r"UPDATE document_deliveries
             SET read_at = COALESCE(read_at, now()), updated_at = now()
           WHERE tenant = $1 AND delivery_id = $2 AND channel = 'PORTAL'",
    )
    .bind(tenant)
    .bind(delivery_id)
    .execute(pool)
    .await
    .context("record portal read")?;
    Ok(n.rows_affected() > 0)
}

/// What a print service pulls: the `POST` deliveries still waiting, with the
/// document ids whose bytes it then fetches.
///
/// `PENDING` is the whole spool, which is why [`claim_due`] must never let a
/// `POST` row that has no relay to push to reach `FAILED`: a letter that left
/// this list was never printed, and nobody is watching the status column.
///
/// # Errors
///
/// Propagates database errors.
pub async fn postal_spool(pool: &PgPool, tenant: &str, limit: i64) -> Result<Vec<PendingDelivery>> {
    let rows = sqlx::query(
        r"SELECT dd.delivery_id, dd.document_id, dd.channel, dd.target, dd.attempts,
                 d.kind, d.subject_ref, d.malo_id, d.kunden_nr, d.recipient_name, d.media_type
          FROM document_deliveries dd
          JOIN documents d ON d.document_id = dd.document_id
          WHERE dd.tenant = $1 AND dd.channel = 'POST' AND dd.status = 'PENDING'
          ORDER BY dd.created_at
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit.clamp(1, 1000))
    .fetch_all(pool)
    .await
    .context("postal spool")?;
    rows.iter()
        .map(|r| {
            Ok(PendingDelivery {
                delivery_id: r.try_get("delivery_id")?,
                document_id: r.try_get("document_id")?,
                channel: Channel::Post,
                target: r.try_get("target")?,
                attempts: r.try_get("attempts")?,
                kind: r.try_get("kind")?,
                subject_ref: r.try_get("subject_ref")?,
                malo_id: r.try_get("malo_id")?,
                kunden_nr: r.try_get("kunden_nr")?,
                recipient_name: r.try_get("recipient_name")?,
                media_type: r.try_get("media_type")?,
            })
        })
        .collect()
}
