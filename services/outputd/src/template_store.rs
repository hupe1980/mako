//! Append-only, content-addressed store for operator document templates.
//!
//! # Why content-addressed
//!
//! An invoice is a Buchungsbeleg: § 14b UStG / § 147 AO require **8 years**
//! retention, and GoBD requires *Unveränderbarkeit*. A document issued today
//! must still be explicable in 2034 — including *why it looked the way it did*.
//!
//! A mutable template row cannot answer that. Editing one silently rewrites the
//! history of every document it ever rendered. So a template is identified by
//! the SHA-256 of its source, rows are never updated or deleted, and each issued
//! invoice records the hash that produced it. Publishing a change inserts a new
//! row and moves a pointer; the old row stays resolvable for as long as the
//! documents it rendered must be kept.
//!
//! This is the discipline the platform already uses elsewhere — `FormatVersion`
//! pinning on EDIFACT profiles, `doubleentry`'s append-only log, `edmd`'s
//! `as_known_at` reads. A mutable template would be the one place it did not
//! hold.
//!
//! # What is *not* here
//!
//! Admission control. The publish gate — render, PDF/A conformance, and the
//! round-trip that extracts the embedded invoice back out of the finished PDF —
//! lives in [`crate::document::gate`], and the HTTP handler runs it before
//! calling [`publish`]. This module is the durable layer: it records *which*
//! proof was obtained ([`crate::document::gate::Proof`]) rather than assuming
//! one, and the schema refuses an `INVOICE` row that was not fully proven.

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::document::gate::Proof;

/// Why a store write was refused — separated from plain storage errors so the
/// HTTP layer can answer the caller's fault as the caller's fault (`409`/`422`)
/// and a broken database as what it is (`500`), instead of folding both into
/// one status.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The identical source is already published under a different kind (or,
    /// in a shared database, by a different tenant). The hash is the identity
    /// of the *source*, and a template's kind decides which proof it was
    /// admitted on — so one row cannot honestly serve both. Without this
    /// refusal the second publish was a silent no-op that returned a hash whose
    /// row carried the *first* publisher's kind and proof: rollout then
    /// succeeded and every render answered 422, with no error anywhere naming
    /// the cause.
    #[error(
        "this exact source is already published {}as {existing_kind} — the hash is the identity \
         of the source, so one row cannot serve two owners; change the source (a comment line \
         suffices) to give it its own identity{}",
        if *other_tenant { "by another tenant " } else { "" },
        if *other_tenant { "" } else { ", or publish it under that kind" },
    )]
    IdentityCollision {
        existing_kind: String,
        /// Whether the colliding row belongs to a different tenant — in that
        /// case "publish it under {kind}" is not advice, it is what the caller
        /// just did.
        other_tenant: bool,
    },

    /// The hash names no template published as `kind` by this tenant.
    #[error("{0} is not a published {1} template of this tenant")]
    NotPublished(String, &'static str),

    /// The database, not the caller.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Which document a template renders.
///
/// The Textform kinds share this store and (once it exists) the same engine with
/// `Invoice`: an operator maintaining two template systems for one brand is how
/// a logo change reaches the invoice and not the Mahnung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TemplateKind {
    /// The ZUGFeRD PDF/A-3 invoice carrier.
    Invoice,
    /// Mahnung — Textform, § 126b BGB.
    Mahnung,
    /// § 41 Abs. 5 EnWG price-change notice — Textform.
    Preisanpassung,
}

impl TemplateKind {
    /// The `document_templates.kind` value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invoice => "INVOICE",
            Self::Mahnung => "MAHNUNG",
            Self::Preisanpassung => "PREISANPASSUNG",
        }
    }
}

/// A published template, as stored.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct StoredTemplate {
    /// SHA-256 of `source`, lowercase hex — the template's identity.
    pub hash: String,
    pub tenant: String,
    pub kind: String,
    pub source: String,
    /// PDF/A conformance level the publish gate enforced (e.g. `a-3b`);
    /// `None` for the Textform kinds, which have no PDF conformance to meet.
    pub pdf_standard: Option<String>,
    /// What the gate established — `RENDERED_PDFA` or `PARSED`. See
    /// [`crate::document::gate::Proof`].
    pub proof: String,
}

/// A published template as a *listing* shows it — everything except the source.
///
/// The source is deliberately absent: a template runs to tens of kilobytes, and
/// a listing exists to let an operator choose one, not to ship every version of
/// their layout at once. [`by_hash`] fetches the source for the one they pick.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TemplateSummary {
    pub hash: String,
    pub kind: String,
    /// PDF/A level the gate enforced; `None` for the Textform kinds.
    pub pdf_standard: Option<String>,
    /// `RENDERED_PDFA` or `PARSED` — what the gate established.
    pub proof: String,
    #[serde(with = "time::serde::rfc3339")]
    pub published_at: time::OffsetDateTime,
    pub published_by: Option<String>,
    /// Whether this is the template `(tenant, kind)` renders with now.
    pub is_current: bool,
}

/// Every template this tenant has published, newest first.
///
/// This is what makes the documented rollback actually performable. The store
/// never deletes precisely so a previous layout stays available — but "PUT the
/// previous hash" is not a usable instruction unless something tells you what
/// the previous hash *was*, and `current` only ever names one template while
/// `by_hash` needs an answer you do not have yet.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list(
    exec: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    kind: Option<TemplateKind>,
    limit: i64,
) -> Result<Vec<TemplateSummary>> {
    Ok(sqlx::query_as(
        "SELECT t.hash, t.kind, t.pdf_standard, t.proof, t.published_at, t.published_by,
                (c.hash IS NOT NULL) AS is_current
           FROM document_templates t
           LEFT JOIN document_template_current c
             ON c.hash = t.hash AND c.tenant = t.tenant AND c.kind = t.kind
          WHERE t.tenant = $1
            AND ($2::text IS NULL OR t.kind = $2)
          ORDER BY t.published_at DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(kind.map(TemplateKind::as_str))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The template's identity: SHA-256 of its source, lowercase hex.
///
/// Deliberately over the *source*, not over a rendered artefact — rendering is
/// not reproducible byte-for-byte across engine versions, and the question this
/// answers is "which template", not "which PDF".
#[must_use]
pub fn hash_source(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Publish a proven template and return its hash.
///
/// `proof` is what [`crate::document::gate::prove`] established. Taking it as
/// an argument rather than defaulting it is the point: this function cannot be
/// called without having proven something, and the schema refuses an `INVOICE`
/// row whose proof is not the full one.
///
/// Idempotent by construction: the same source yields the same hash, and a
/// re-publish under the same kind is a no-op rather than a duplicate or an
/// error. **Does not** move the current pointer — publishing and rolling out
/// are separate acts, so a template can be stored and rendered against before
/// anyone is billed with it.
///
/// # Errors
///
/// [`StoreError::IdentityCollision`] when the identical source is already
/// published under a different kind or tenant — the row keeps the first
/// publisher's kind and proof, so silently returning its hash would hand the
/// caller an identity that fails `render_admissible` on every render, with the
/// cause reported nowhere. Otherwise storage errors.
pub async fn publish(
    pool: &sqlx::PgPool,
    tenant: &str,
    kind: TemplateKind,
    source: &str,
    pdf_standard: Option<&str>,
    proof: Proof,
    published_by: Option<&str>,
) -> Result<String, StoreError> {
    let hash = hash_source(source);
    let inserted = sqlx::query(
        "INSERT INTO document_templates
             (hash, tenant, kind, source, pdf_standard, proof, published_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (hash) DO NOTHING",
    )
    .bind(&hash)
    .bind(tenant)
    .bind(kind.as_str())
    .bind(source)
    .bind(pdf_standard)
    .bind(proof.as_str())
    .bind(published_by)
    .execute(pool)
    .await?
    .rows_affected();
    if inserted == 0 {
        // The hash already exists. Rows are immutable, so this read cannot
        // race anything: either it is the same (tenant, kind) — the documented
        // idempotent re-publish — or the caller collided with an identity that
        // is not theirs.
        let (existing_tenant, existing_kind): (String, String) =
            sqlx::query_as("SELECT tenant, kind FROM document_templates WHERE hash = $1")
                .bind(&hash)
                .fetch_one(pool)
                .await?;
        if existing_tenant != tenant || existing_kind != kind.as_str() {
            return Err(StoreError::IdentityCollision {
                existing_kind,
                other_tenant: existing_tenant != tenant,
            });
        }
    }
    Ok(hash)
}

/// Point `(tenant, kind)` at an already-published template.
///
/// The guarded `INSERT … SELECT` requires the row to exist **as this kind, for
/// this tenant** — not merely to exist. A pointer is a rollout decision, and
/// rolling out a Mahnung layout as the invoice template must fail *here*, at
/// the `PUT`, not at the first render when invoices are due.
///
/// # Errors
///
/// [`StoreError::NotPublished`] when `hash` names no template published as
/// `kind` by this tenant; otherwise storage errors.
pub async fn set_current(
    exec: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    kind: TemplateKind,
    hash: &str,
) -> Result<(), StoreError> {
    let written = sqlx::query(
        "INSERT INTO document_template_current (tenant, kind, hash, updated_at)
         SELECT t.tenant, t.kind, t.hash, now()
           FROM document_templates t
          WHERE t.hash = $3 AND t.tenant = $1 AND t.kind = $2
         ON CONFLICT (tenant, kind) DO UPDATE
           SET hash = EXCLUDED.hash, updated_at = now()",
    )
    .bind(tenant)
    .bind(kind.as_str())
    .bind(hash)
    .execute(exec)
    .await?
    .rows_affected();
    if written == 0 {
        return Err(StoreError::NotPublished(hash.to_owned(), kind.as_str()));
    }
    Ok(())
}

/// The template `(tenant, kind)` renders with now, if one is configured.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn current(
    exec: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    kind: TemplateKind,
) -> Result<Option<StoredTemplate>> {
    Ok(sqlx::query_as(
        "SELECT t.hash, t.tenant, t.kind, t.source, t.pdf_standard, t.proof
           FROM document_template_current c
           JOIN document_templates t ON t.hash = c.hash
          WHERE c.tenant = $1 AND c.kind = $2",
    )
    .bind(tenant)
    .bind(kind.as_str())
    .fetch_optional(exec)
    .await?)
}

/// A specific template by hash — how a reissued or audited document resolves the
/// layout it was actually rendered with, years later.
///
/// Not tenant-scoped on purpose: the hash *is* the identity, and a document
/// carrying it has already established the right to see it.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn by_hash(
    exec: impl sqlx::PgExecutor<'_>,
    hash: &str,
) -> Result<Option<StoredTemplate>> {
    Ok(sqlx::query_as(
        "SELECT hash, tenant, kind, source, pdf_standard, proof
           FROM document_templates WHERE hash = $1",
    )
    .bind(hash)
    .fetch_optional(exec)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_the_identity_of_the_source() {
        // Same source, same identity — this is what makes `publish` idempotent
        // and a re-publish a no-op rather than a duplicate row.
        assert_eq!(hash_source("#set page()"), hash_source("#set page()"));
        assert_ne!(hash_source("#set page()"), hash_source("#set page() "));
        // Lowercase hex, 32 bytes.
        let h = hash_source("x");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    /// SHA-256("abc") — NIST FIPS 180-4 Example 1. Pins the algorithm, so a
    /// swap to a different digest cannot happen silently and orphan every
    /// template hash already recorded on an issued invoice.
    #[test]
    fn the_digest_is_sha256() {
        assert_eq!(
            hash_source("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }
}
