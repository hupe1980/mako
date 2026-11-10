//! Persistence for issued documents and their delivery attempts.
//!
//! Append-only for the content columns, like [`crate::template_store`] and for
//! the same statute (§ 147 AO): a corrected document is a new row. The delivery
//! rows beside it advance in place. A pushed delivery (`EMAIL`, `POST` to a
//! relay, `ERP`) is owed as a `mako_service::outbox` command committed with the
//! document; its attempts are read off that command
//! (`DELIVERY_OUTBOX_JOIN`), and its evidence is written when the outbox
//! marks it delivered ([`record_success`]).

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;
use uuid::Uuid;

use super::channel::Channel;
use crate::config::DeliveryConfig;

/// The outbox key of a delivery: the Standard Webhooks message id a relay
/// deduplicates on. The delivery, not the attempt — a retry is the same
/// message, and a relay that already accepted it must be able to say so rather
/// than print the letter twice.
#[must_use]
pub fn delivery_key(delivery_id: Uuid) -> String {
    format!("de.output.delivery/{delivery_id}")
}

/// The delivery behind an outbox key, when it is one.
#[must_use]
pub fn delivery_of_key(key: &str) -> Option<Uuid> {
    key.strip_prefix("de.output.delivery/")?.parse().ok()
}

/// The outbox upstream a pushed channel's commands are owed to.
#[must_use]
pub const fn upstream(channel: Channel) -> &'static str {
    match channel {
        Channel::Portal => "outputd-portal",
        Channel::Email => "outputd-email",
        Channel::Post => "outputd-post",
        Channel::Erp => "outputd-erp",
    }
}

/// The `event_outbox` join that reads a delivery's attempt track off the
/// command that carries it.
const DELIVERY_OUTBOX_JOIN: &str =
    "LEFT JOIN event_outbox o ON o.event_id = 'de.output.delivery/' || dd.delivery_id::TEXT";

/// The [`DeliveryRow`] columns, read through [`DELIVERY_OUTBOX_JOIN`].
const DELIVERY_COLUMNS: &str = "dd.delivery_id, dd.document_id, dd.channel, dd.status, dd.target, \
     COALESCE(o.attempts, 0)::INTEGER AS attempts, \
     CASE WHEN o.delivered_at IS NULL AND o.dead_lettered_at IS NULL \
          THEN o.next_attempt_at END AS next_attempt_at, \
     dd.first_sent_at, dd.delivered_at, dd.read_at, dd.evidence, \
     COALESCE(dd.last_error, o.last_error) AS last_error";

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
#[serde(deny_unknown_fields)]
pub struct Recipient {
    /// The addressee as printed.
    #[serde(default)]
    pub name: Option<String>,
    /// The address the `EMAIL` channel uses. Absent → that channel is
    /// suppressed with a reason rather than silently skipped.
    #[serde(default)]
    pub email: Option<String>,
    /// The postal address the `POST` channel prints.
    ///
    /// Typed rather than free-form JSON. A letter needs a street, a postcode
    /// and a town; anything less is not an address, and while this was a
    /// `serde_json::Value` an issuing service that found a customer record
    /// with no address at all still produced `{"line1":null,"post_code":null,…}`
    /// — which is `Some`, so the `POST` channel was not suppressed and the
    /// delivery target became that JSON text, recorded as where the letter
    /// went.
    #[serde(default)]
    pub address: Option<PostalAddress>,
}

/// A postal address complete enough to send a letter to.
///
/// Every field is required, which is the point: [`Recipient::address`] is
/// `Option`, so "we do not know where they live" has a representation already,
/// and it is the one that suppresses the `POST` channel with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostalAddress {
    /// Street and house number.
    pub line1: String,
    /// Postcode.
    pub post_code: String,
    /// Town.
    pub city: String,
    /// ISO 3166-1 alpha-2. Defaults to `DE`.
    #[serde(default = "default_country")]
    pub country: String,
}

fn default_country() -> String {
    "DE".to_owned()
}

impl std::fmt::Display for PostalAddress {
    /// One line, the way a delivery target is recorded: `Musterstr. 1, 10115
    /// Berlin, DE`. Was `serde_json::Value::to_string`, which recorded the
    /// document's *JSON* as the address a letter went to.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, {} {}, {}",
            self.line1, self.post_code, self.city, self.country
        )
    }
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
    /// Attempts made by the outbox command carrying this delivery.
    pub attempts: i32,
    /// When the outbox tries again; `None` when nothing is pending.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_attempt_at: Option<OffsetDateTime>,
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
/// In the same transaction each queued delivery is set going under `cfg`: the
/// portal publication is recorded as delivered, and a pushed channel's relay
/// POST is enqueued as a `mako_service::outbox` command — except a `POST` with
/// no postal relay, which waits in the spool for a print service. With
/// delivery disabled every row stays `PENDING`.
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
    cfg: &DeliveryConfig,
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
    .bind(
        doc.recipient
            .address
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| anyhow::anyhow!("recipient address is not serialisable: {e}"))?,
    )
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
        let delivery_id: Uuid = sqlx::query_scalar(
            r"INSERT INTO document_deliveries
                (document_id, tenant, channel, status, target, last_error)
              VALUES ($1,$2,$3,$4,$5,$6)
              RETURNING delivery_id",
        )
        .bind(document.document_id)
        .bind(doc.tenant)
        .bind(channel.as_str())
        .bind(status)
        .bind(&target)
        .bind(last_error)
        .fetch_one(&mut *tx)
        .await
        .context("queue delivery")?;
        if status == "PENDING" && cfg.enabled {
            set_going(
                &mut tx,
                cfg,
                doc,
                &document,
                *channel,
                delivery_id,
                target.as_deref(),
            )
            .await?;
        }
    }
    let rows = sqlx::query(mako_service::sql::dynamic(format!(
        "SELECT {DELIVERY_COLUMNS} FROM document_deliveries dd {DELIVERY_OUTBOX_JOIN} \
         WHERE dd.document_id = $1 ORDER BY dd.channel"
    )))
    .bind(document.document_id)
    .fetch_all(&mut *tx)
    .await
    .context("read the queued deliveries")?;
    for r in &rows {
        deliveries.push(delivery_row(r)?);
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

/// Set one queued delivery going, on the issuing transaction.
async fn set_going(
    tx: &mut sqlx::PgConnection,
    cfg: &DeliveryConfig,
    doc: &NewDocument<'_>,
    document: &DocumentRow,
    channel: Channel,
    delivery_id: Uuid,
    target: Option<&str>,
) -> Result<()> {
    match channel {
        // Publishing *is* the delivery: the document is in the store and
        // `portald` serves it from there, which is why this channel can claim
        // arrival by itself.
        Channel::Portal => {
            record_success(
                &mut *tx,
                doc.tenant,
                delivery_id,
                true,
                Some(serde_json::json!({
                    "published": "portal-inbox",
                    "document_id": document.document_id,
                })),
            )
            .await
        }
        // A print service *pulls* from `GET /api/v1/spool`; the push is for
        // partners that offer an endpoint. With none configured the letter
        // waits in the spool, which is a supported integration and not a
        // failed push.
        Channel::Post if !cfg.has_postal_relay() => Ok(()),
        Channel::Email | Channel::Post | Channel::Erp => {
            let to = matches!(channel, Channel::Email)
                .then_some(target)
                .flatten();
            let body = relay_body(cfg, doc, document, channel, delivery_id, to);
            mako_service::outbox::enqueue_command(
                tx,
                &mako_service::outbox::Command {
                    upstream: upstream(channel).to_owned(),
                    label: format!("output.delivery.{}", channel.as_str().to_ascii_lowercase()),
                    method: mako_service::outbox::CommandMethod::Post,
                    path: String::new(),
                    body,
                    key: delivery_key(delivery_id),
                    settled_statuses: Vec::new(),
                },
            )
            .await
            .context("enqueue the relay delivery")
        }
    }
}

/// The JSON a relay receives.
///
/// Carries the delivery id, so a relay reporting asynchronously through
/// `POST /api/v1/deliveries/{id}/status` has a key to report against, and the
/// document base64-encoded, so an adapter needs nothing but a JSON parser.
fn relay_body(
    cfg: &DeliveryConfig,
    doc: &NewDocument<'_>,
    document: &DocumentRow,
    channel: Channel,
    delivery_id: Uuid,
    to: Option<&str>,
) -> serde_json::Value {
    use base64::Engine as _;
    let filename = format!(
        "{}-{}.{}",
        doc.kind.to_ascii_lowercase(),
        doc.subject_ref.replace(['/', '\\', ' '], "-"),
        if doc.media_type == "application/pdf" {
            "pdf"
        } else {
            "bin"
        }
    );
    serde_json::json!({
        "delivery_id":    delivery_id,
        "document_id":    document.document_id,
        "channel":        channel.as_str(),
        "kind":           doc.kind,
        "subject_ref":    doc.subject_ref,
        "malo_id":        doc.malo_id,
        "kunden_nr":      doc.kunden_nr,
        "from":           cfg.from_address,
        "to":             to,
        "recipient_name": doc.recipient.name,
        "subject":        cfg.subject_for(doc.kind),
        "filename":       filename,
        "media_type":     doc.media_type,
        "content_base64": base64::engine::general_purpose::STANDARD.encode(doc.content),
    })
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
    let deliveries = deliveries_of(pool, tenant, document.document_id).await?;
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
    let deliveries = deliveries_of(pool, tenant, document.document_id).await?;
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
/// Every delivery attempt recorded for one document.
///
/// Bounded by tenant as well as by document: `document_id` is a foreign key
/// here, not this table's own key, so it is the predicate that decides which
/// rows are returned. What these rows carry is delivery evidence — the target
/// address, the timestamps, the read receipt — so an id resolving in another
/// tenant would disclose that tenant's, and the caller having read the document
/// in-tenant is a property of the caller rather than of this statement.
pub async fn deliveries_of(
    pool: &PgPool,
    tenant: &str,
    document_id: Uuid,
) -> Result<Vec<DeliveryRow>> {
    let rows = sqlx::query(mako_service::sql::dynamic(format!(
        "SELECT {DELIVERY_COLUMNS} FROM document_deliveries dd {DELIVERY_OUTBOX_JOIN} \
         WHERE dd.document_id = $1 AND dd.tenant = $2 ORDER BY dd.channel"
    )))
    .bind(document_id)
    .bind(tenant)
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

/// One letter waiting in the postal spool.
#[derive(Debug, Clone)]
pub struct PendingDelivery {
    pub delivery_id: Uuid,
    pub document_id: Uuid,
    pub channel: Channel,
    pub target: Option<String>,
    pub kind: String,
    pub subject_ref: String,
    pub malo_id: Option<String>,
    pub kunden_nr: Option<String>,
    pub recipient_name: Option<String>,
    pub media_type: String,
}

/// Record what a channel reported for one delivery.
///
/// `delivered` separates handed off (`SENT`) from known to have arrived
/// (`DELIVERED`). Only the second is evidence, and only channels that can
/// observe arrival pass `true`. `delivered_at` moves with the status — set on
/// `DELIVERED`, cleared on anything else — because the table holds
/// `CHECK ((status = 'DELIVERED') = (delivered_at IS NOT NULL))`.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_success(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    delivery_id: Uuid,
    delivered: bool,
    evidence: Option<serde_json::Value>,
) -> Result<()> {
    sqlx::query(
        r"UPDATE document_deliveries
             SET status        = CASE WHEN $3 THEN 'DELIVERED' ELSE 'SENT' END,
                 first_sent_at = COALESCE(first_sent_at, now()),
                 delivered_at  = CASE WHEN $3 THEN now() ELSE NULL END,
                 evidence      = COALESCE($4, evidence),
                 last_error    = NULL,
                 updated_at    = now()
           WHERE tenant = $1 AND delivery_id = $2",
    )
    .bind(tenant)
    .bind(delivery_id)
    .bind(delivered)
    .bind(evidence)
    .execute(executor)
    .await
    .context("record delivery success")?;
    Ok(())
}

/// Record that a delivery did not reach its recipient: `FAILED`, the state
/// that says a customer never received something the platform believes it
/// sent. `delivered_at` is cleared with it, so a bounce reported on a
/// `DELIVERED` row turns it into the failure it was.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_failure(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    delivery_id: Uuid,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r"UPDATE document_deliveries
             SET status       = 'FAILED',
                 delivered_at = NULL,
                 last_error   = $3,
                 updated_at   = now()
           WHERE tenant = $1 AND delivery_id = $2",
    )
    .bind(tenant)
    .bind(delivery_id)
    .bind(error)
    .execute(executor)
    .await
    .context("record delivery failure")?;
    Ok(())
}

/// A delivery whose outbox command was dead-lettered, now `FAILED`.
#[derive(Debug, Clone)]
pub struct GaveUp {
    pub delivery_id: Uuid,
    pub channel: String,
    pub attempts: i32,
    pub error: String,
}

/// Turn every `PENDING` delivery whose outbox command was dead-lettered into
/// `FAILED`, with the command's last error, and return them.
///
/// The outbox gave up — its budget is spent or the relay refused the push —
/// so nothing will deliver the document any more, and the row has to say so.
///
/// # Errors
///
/// Propagates database errors.
pub async fn fail_dead_lettered(pool: &PgPool, tenant: &str) -> Result<Vec<GaveUp>> {
    let rows = sqlx::query(
        r"UPDATE document_deliveries dd
             SET status       = 'FAILED',
                 delivered_at = NULL,
                 last_error   = COALESCE(o.last_error, 'the outbox gave up on the delivery'),
                 updated_at   = now()
            FROM event_outbox o
           WHERE dd.tenant = $1 AND dd.status = 'PENDING'
             AND o.event_id = 'de.output.delivery/' || dd.delivery_id::TEXT
             AND o.dead_lettered_at IS NOT NULL
          RETURNING dd.delivery_id, dd.channel, o.attempts::INTEGER AS attempts,
                    dd.last_error",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("fail dead-lettered deliveries")?;
    rows.iter()
        .map(|r| {
            Ok(GaveUp {
                delivery_id: r.try_get("delivery_id")?,
                channel: r.try_get("channel")?,
                attempts: r.try_get("attempts")?,
                error: r.try_get("last_error")?,
            })
        })
        .collect()
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
/// `PENDING` is the whole spool: with no postal relay configured a `POST` row
/// is never enqueued, so nothing can turn it `FAILED` behind the print
/// service's back.
///
/// # Errors
///
/// Propagates database errors.
pub async fn postal_spool(pool: &PgPool, tenant: &str, limit: i64) -> Result<Vec<PendingDelivery>> {
    let rows = sqlx::query(
        r"SELECT dd.delivery_id, dd.document_id, dd.channel, dd.target,
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
