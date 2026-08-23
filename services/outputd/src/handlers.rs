//! Operator-facing REST surface for document templates.
//!
//! Four verbs, and the split between them is the design:
//!
//! | Route | What it decides |
//! |---|---|
//! | `POST /templates/preview` | nothing — renders a candidate against the gate's specimen so an operator can look at it |
//! | `POST /templates` | that a template *works* — it is proven, then stored forever |
//! | `PUT /templates/{kind}/current` | that a template is *in use* — a pointer move |
//! | `GET /templates/reference` | hands out the layout mako ships, as a starting point |
//!
//! Publishing and rolling out are separate because they are separate decisions,
//! separated in time: a template is proven and stored before anyone is billed
//! with it, and rolling back is the same call with the previous hash — possible
//! only because the store never deletes.
//!
//! There is deliberately no update and no delete. See [`crate::template_store`]:
//! an issued invoice pins the hash that rendered it, and § 147 AO / GoBD require
//! that to stay resolvable for 8 years.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_service::oidc::Claims;
use serde::Deserialize;
use sqlx::PgPool;

use crate::config::OutputdConfig;
use crate::document::gate::{self, DEFAULT_PDF_STANDARD};
use crate::error::{OutputError, OutputResult};
use crate::template_store::{self, TemplateKind};

pub(crate) use mako_service::cedar::CedarEnforcer;

/// Authorize `action` for the caller against the service tenant.
///
/// Authentication established *who* is calling; this decides what they may do.
/// outputd shipped with the `cedar` feature enabled and **no policy file at
/// all**, so any token the OIDC verifier accepted could roll out a new layout
/// for every invoice and Mahnung the tenant issues, or render arbitrary content
/// on the operator's letterhead. A template is not one document; it is the
/// shape of all of them.
///
/// # Errors
///
/// `403` with the Cedar denial reason.
pub(crate) fn authorize(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &'static str,
    tenant: &str,
) -> OutputResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| {
            tracing::warn!(action, sub = %claims.sub(), "outputd: authorization denied");
            OutputError::Forbidden {
                code: "FORBIDDEN",
                message: format!("{action}: {e}"),
            }
        })
}

/// How long a template render may take before the caller is freed.
///
/// Generous — a first-page-heavy layout on a cold font cache is not fast — but
/// finite, because a template with a runaway loop in it must not hold a request
/// or a billing run open. See [`crate::document::render`] for what happens to
/// the thread.
pub(crate) const RENDER_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// `POST /api/v1/templates` body.
#[derive(Debug, Deserialize)]
pub struct PublishTemplateRequest {
    /// Which document this renders.
    pub kind: TemplateKind,
    /// The template source (Typst). Must export `#let render(invoice) = ..`.
    pub source: String,
    /// PDF/A conformance level to enforce, in Typst's spelling. Defaults to
    /// `a-3b`, which is what ZUGFeRD 2.3 requires.
    ///
    /// Configurable rather than fixed so an operator on a profile that permits
    /// a different level (`a-3u` for searchable text, `a-4f`) can select it
    /// without a code change. A level that cannot carry an embedded file is
    /// refused rather than accepted and silently stripped of the invoice.
    #[serde(default)]
    pub pdf_standard: Option<String>,
}

/// What publishing a template established.
#[derive(Debug, serde::Serialize)]
pub struct PublishedTemplate {
    /// The template's identity — record this against anything it renders.
    pub hash: String,
    /// `RENDERED_PDFA` or `PARSED`. See [`gate::Proof`].
    pub proof: gate::Proof,
    /// Pages the specimen invoice came to under this layout.
    pub pages: usize,
    /// Typst warnings from the proving render. Not errors — but "unknown font
    /// family" is a warning, and rolling out a layout that silently fell back to
    /// a different typeface is worth one look first.
    pub warnings: Vec<String>,
}

/// `POST /api/v1/templates` — prove a template, then store it.
///
/// Idempotent: re-publishing identical source returns the same hash and stores
/// nothing new. Does **not** roll it out.
///
/// A template that does not render is `422` with the compiler's diagnostics,
/// each pointing at a line of the operator's own file.
pub async fn post_template(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Json(req): Json<PublishTemplateRequest>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "publish-template", &cfg.tenant)?;
    if req.source.trim().is_empty() {
        return Err(OutputError::bad_request("EMPTY_SOURCE", "source is empty"));
    }
    let standard = req
        .pdf_standard
        .clone()
        .unwrap_or_else(|| DEFAULT_PDF_STANDARD.to_owned());

    let proven = prove(req.kind, req.source.clone(), standard.clone()).await?;

    // Only the invoice kind has a PDF/A level to have met; recording one for a
    // Textform template would claim a conformance nothing checked.
    let pdf_standard = (req.kind == TemplateKind::Invoice).then_some(standard.as_str());
    let hash = template_store::publish(
        &pool,
        &cfg.tenant,
        req.kind,
        &req.source,
        pdf_standard,
        proven.proof,
        Some(claims.sub()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(PublishedTemplate {
            hash,
            proof: proven.proof,
            pages: proven.pages,
            warnings: proven.warnings,
        }),
    ))
}

/// `POST /api/v1/templates/preview` — render a candidate, store nothing.
///
/// The same specimen and the same pipeline as [`post_template`], but the result
/// is the PDF rather than a row. This is the loop an operator actually works in:
/// edit, look at it, edit. Publishing an unproven template to see what it looks
/// like would put a row in an append-only table for every iteration.
///
/// The returned file is stamped exactly as an issued invoice would be, so it can
/// be dropped into veraPDF or a ZUGFeRD validator as-is — a preview that skipped
/// the carrier would be a preview of something mako does not produce.
pub async fn post_template_preview(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Json(req): Json<PublishTemplateRequest>,
) -> OutputResult<axum::response::Response> {
    authorize(&cedar, &claims, "preview-template", &cfg.tenant)?;
    if req.source.trim().is_empty() {
        return Err(OutputError::bad_request("EMPTY_SOURCE", "source is empty"));
    }
    if req.kind == TemplateKind::Mahnung {
        // Its own specimen, no carrier: a Mahnung is Textform, and the preview
        // is the letter itself.
        let request = crate::document::RenderRequest {
            template: req.source,
            data: Some(
                serde_json::to_string(&crate::document::mahnung::specimen())
                    .map_err(|e| OutputError::Internal(e.into()))?,
            ),
            attachment: None,
            standard: None,
            date: gate::SPECIMEN_DATE,
            ident: "mako-template-preview-mahnung".to_owned(),
        };
        let rendered = crate::document::render::render_guarded(request, RENDER_BUDGET).await?;
        return Ok(pdf_response("mahnung-vorschau.pdf", rendered.pdf));
    }
    if req.kind != TemplateKind::Invoice {
        // Rendering a Preisanpassung against an invoice specimen would fail on
        // the first field it reads, with a diagnostic that blamed the template.
        return Err(OutputError::unprocessable(
            "NO_SPECIMEN",
            format!(
                "there is no specimen to preview a {} template against — its data contract \
                 does not live in mako yet; publishing checks that it parses",
                req.kind.as_str(),
            ),
        ));
    }
    let standard = req
        .pdf_standard
        .unwrap_or_else(|| DEFAULT_PDF_STANDARD.to_owned());
    // The same specimen the gate proves against, so a preview is the artefact
    // publishing would produce rather than an approximation of it.
    let model = gate::specimen_invoice();
    let profile = crate::document::facturx::profile_of(&model);
    let attachment =
        crate::document::facturx::attachment(profile, en16931_formats::cii::to_string(&model))
            .map_err(OutputError::Internal)?;
    let request = crate::document::RenderRequest {
        template: req.source,
        data: Some(
            serde_json::to_string(&crate::document::DocumentView::of(&model))
                .map_err(|e| OutputError::Internal(e.into()))?,
        ),
        attachment: Some(attachment),
        standard: Some(standard),
        date: gate::SPECIMEN_DATE,
        ident: "mako-template-preview".to_owned(),
    };
    let rendered = crate::document::render::render_guarded(request, RENDER_BUDGET).await?;
    let pdf =
        crate::document::facturx::stamp(&rendered.pdf, profile).map_err(OutputError::Internal)?;
    Ok(pdf_response("vorschau.pdf", pdf))
}

/// `GET /api/v1/templates/reference/{kind}` — the layout mako ships per kind.
///
/// Served rather than only documented so an operator's starting point is the
/// exact source the test suite compiles on every run, not a copy of it that has
/// drifted. Every kind has one, and each passes its own publish gate — which is
/// the property that makes it a starting point rather than a sketch.
pub async fn get_reference_template(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(kind): Path<String>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-template", &cfg.tenant)?;
    let source = match parse_kind(&kind)? {
        TemplateKind::Invoice => crate::document::REFERENCE_INVOICE_TEMPLATE,
        TemplateKind::Mahnung => crate::document::REFERENCE_MAHNUNG_TEMPLATE,
        TemplateKind::Preisanpassung => crate::document::REFERENCE_PREISANPASSUNG,
    };
    Ok((
        StatusCode::OK,
        [(
            "Content-Type",
            "text/plain; charset=UTF-8; x-typst-version=0.15",
        )],
        source,
    ))
}

/// `GET /api/v1/templates` query.
#[derive(Debug, Deserialize)]
pub struct ListTemplatesQuery {
    /// Restrict to one document kind.
    pub kind: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/templates` — every template this tenant has published.
///
/// Without this the documented rollback is not performable: the store keeps
/// every previous layout precisely so one can be restored, but "PUT the previous
/// hash" needs something that says what the previous hash was — and `current`
/// names exactly one while `by-hash` needs the answer already.
///
/// Newest first, `is_current` marking the one in use. The source is not
/// included; fetch it per hash.
pub async fn list_templates(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Query(q): Query<ListTemplatesQuery>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-template", &cfg.tenant)?;
    let kind = q.kind.as_deref().map(parse_kind).transpose()?;
    let rows = template_store::list(
        &pool,
        &cfg.tenant,
        kind,
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await
    .map_err(OutputError::Internal)?;
    Ok(Json(rows))
}

/// `PUT /api/v1/templates/{kind}/current` body.
#[derive(Debug, Deserialize)]
pub struct SetCurrentRequest {
    /// The published template to render with from now on.
    pub hash: String,
}

/// `PUT /api/v1/templates/{kind}/current` — roll a published template out.
///
/// A hash that was never published is refused by the foreign key; the handler
/// reports that as `422`, because it is a bad reference rather than a server
/// fault. Rolling back is the same call with the previous hash.
pub async fn put_current_template(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(kind): Path<String>,
    Json(req): Json<SetCurrentRequest>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "rollout-template", &cfg.tenant)?;
    let kind = parse_kind(&kind)?;
    template_store::set_current(&pool, &cfg.tenant, kind, &req.hash).await?;
    tracing::info!(
        tenant = %cfg.tenant, kind = kind.as_str(), hash = %req.hash, by = %claims.sub(),
        "outputd: template rolled out — every document of this kind now renders with it"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/templates/{kind}/current` — what this tenant renders with now.
pub async fn get_current_template(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(kind): Path<String>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-template", &cfg.tenant)?;
    let kind = parse_kind(&kind)?;
    template_store::current(&pool, &cfg.tenant, kind)
        .await
        .map_err(OutputError::Internal)?
        .map(Json)
        .ok_or_else(|| {
            OutputError::not_found(
                "NO_CURRENT_TEMPLATE",
                format!(
                    "no {} template is rolled out for this tenant",
                    kind.as_str()
                ),
            )
        })
}

/// `GET /api/v1/templates/by-hash/{hash}` — resolve a template by hash.
///
/// This is how an audit answers "why did the invoice from 2027 look like that":
/// the record carries `template_hash`, and the source is still here.
pub async fn get_template_by_hash(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(hash): Path<String>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-template", &cfg.tenant)?;
    template_store::by_hash(&pool, &cfg.tenant, &hash)
        .await
        .map_err(OutputError::Internal)?
        .map(Json)
        .ok_or_else(|| {
            OutputError::not_found(
                "TEMPLATE_NOT_FOUND",
                format!("no template of this tenant has hash {hash}"),
            )
        })
}

/// Run the publish gate off the async runtime.
///
/// Typesetting is CPU-bound and takes long enough to matter; leaving it on a
/// runtime worker would stall every other request on the same thread.
async fn prove(kind: TemplateKind, source: String, standard: String) -> OutputResult<gate::Proven> {
    let task =
        tokio::task::spawn_blocking(move || gate::prove(kind, &source, Some(standard.as_str())));
    match tokio::time::timeout(RENDER_BUDGET, task).await {
        Ok(Ok(Ok(proven))) => Ok(proven),
        // The template did not survive the gate. Its diagnostics — which name
        // the operator's file, line and column — are the response body.
        Ok(Ok(Err(e))) => Err(OutputError::diagnostics(
            "TEMPLATE_REJECTED_BY_GATE",
            "the template did not survive the publish gate",
            format!("{e:#}").lines().map(str::to_owned).collect(),
        )),
        Ok(Err(join)) => Err(OutputError::Internal(anyhow::anyhow!(
            "the publish gate panicked: {join}"
        ))),
        Err(_) => Err(OutputError::unprocessable(
            "RENDER_BUDGET_EXCEEDED",
            format!(
                "the template did not finish rendering within {RENDER_BUDGET:?}; \
                 it is doing far more work than one document needs"
            ),
        )),
    }
}

/// A PDF download response.
pub(crate) fn pdf_response(filename: &str, pdf: Vec<u8>) -> axum::response::Response {
    (
        StatusCode::OK,
        [
            ("Content-Type", "application/pdf".to_owned()),
            (
                "Content-Disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        pdf,
    )
        .into_response()
}

/// The `kind` path segment, as the stored vocabulary.
///
/// # Errors
///
/// `400` naming the kinds that exist, rather than a bare "unknown template
/// kind" that leaves the caller guessing the spelling.
fn parse_kind(s: &str) -> OutputResult<TemplateKind> {
    match s.to_ascii_uppercase().as_str() {
        "INVOICE" => Ok(TemplateKind::Invoice),
        "MAHNUNG" => Ok(TemplateKind::Mahnung),
        "PREISANPASSUNG" => Ok(TemplateKind::Preisanpassung),
        other => Err(OutputError::bad_request(
            "UNKNOWN_TEMPLATE_KIND",
            format!(
                "`{other}` is not a template kind — expected INVOICE, MAHNUNG or PREISANPASSUNG"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stored_kind_parses_back_from_its_path_segment() {
        // The `kind` in the URL and the `kind` in the CHECK constraint are the
        // same vocabulary; a mismatch would make a kind unreachable over HTTP.
        for k in [
            TemplateKind::Invoice,
            TemplateKind::Mahnung,
            TemplateKind::Preisanpassung,
        ] {
            assert_eq!(parse_kind(k.as_str()).expect("round-trips"), k);
        }
        assert_eq!(
            parse_kind("invoice").expect("case-insensitive"),
            TemplateKind::Invoice
        );
        assert!(parse_kind("nope").is_err());
    }

    /// The default the API applies is the one ZUGFeRD requires.
    #[test]
    fn the_default_standard_is_the_one_zugferd_requires() {
        assert_eq!(DEFAULT_PDF_STANDARD, "a-3b");
    }

    fn stored(kind: &str, tenant: &str) -> template_store::StoredTemplate {
        template_store::StoredTemplate {
            hash: "h".repeat(64),
            tenant: tenant.to_owned(),
            kind: kind.to_owned(),
            source: "#let render(i) = []".to_owned(),
            pdf_standard: (kind == "INVOICE").then(|| "a-3b".to_owned()),
            proof: if kind == "INVOICE" {
                "RENDERED_PDFA"
            } else {
                "RENDERED_TEXTFORM"
            }
            .to_owned(),
        }
    }

    /// A pinned hash resolves by identity, so the render endpoint itself must
    /// refuse the combination nothing upstream has checked: a template proven
    /// as one kind rendering another.
    ///
    /// The tenant case is gone from here because it is gone from the code —
    /// `by_hash` is tenant-scoped, so another operator's layout does not
    /// resolve at all. `a_foreign_tenants_template_does_not_resolve` in
    /// `tests/store_integration.rs` is what pins that.
    #[test]
    fn a_render_refuses_a_template_of_the_wrong_kind() {
        // The proof-discipline case: an INVOICE render with a Textform-proven
        // template would produce a carrier the gate never proved as one.
        let err = render_admissible(
            TemplateKind::Invoice,
            &stored("MAHNUNG", "9900000000004"),
            true,
        )
        .expect_err("kind mismatch must be refused");
        assert_eq!(err.code(), "TEMPLATE_WRONG_KIND");
        assert!(err.to_string().contains("MAHNUNG"), "{err}");
    }

    /// The attachment contract follows the kind, in both directions.
    #[test]
    fn the_attachment_contract_follows_the_kind() {
        let t = "9900000000004";
        // An invoice without its payload is the failure mode that looks like
        // success: a handsome PDF that is not an invoice.
        assert!(render_admissible(TemplateKind::Invoice, &stored("INVOICE", t), false).is_err());
        assert!(render_admissible(TemplateKind::Invoice, &stored("INVOICE", t), true).is_ok());
        // A Textform document carries no embedded invoice.
        assert!(render_admissible(TemplateKind::Mahnung, &stored("MAHNUNG", t), true).is_err());
        assert!(render_admissible(TemplateKind::Mahnung, &stored("MAHNUNG", t), false).is_ok());
    }
}

// ── The render API ────────────────────────────────────────────────────────────

/// `POST /api/v1/render/{kind}` body.
#[derive(Debug, Deserialize)]
pub struct RenderApiRequest {
    /// What the document says.
    ///
    /// For `INVOICE` this is the **EN 16931 semantic model** and outputd
    /// projects the page view from it; for the Textform kinds it is the kind's
    /// own view, verbatim. See [`RenderSubject`].
    #[serde(flatten)]
    pub subject: RenderSubject,
    /// A specific published template, or `None` for the tenant's current one.
    #[serde(default)]
    pub template_hash: Option<String>,
    /// For INVOICE: the CII payload and its BT-24, so the carrier is stamped
    /// exactly as the document declares itself. Absent for Textform kinds.
    #[serde(default)]
    pub attachment: Option<RenderAttachment>,
    /// The date the document bears (BT-2 / Mahnung date), ISO 8601 — becomes
    /// `datetime.today()` in the template and the PDF timestamp.
    pub date: String,
    /// Stable identity for the PDF `/ID` — the caller's record id, typically.
    pub ident: String,
}

/// What a render is *about*, in the only two shapes outputd accepts.
///
/// An invoice arrives as the semantic model, not as a projected view. That is
/// the fix for a duplication that had no business existing: the projection
/// `en16931::Invoice → DocumentView` lived in **both** services — outputd's
/// copy is what the publish gate proves templates against, billingd's copy is
/// what production actually sent — and nothing tied them together. A field
/// added on one side gives you templates that pass the gate and fail in
/// production. Both services already depend on `en16931`, so the model is a
/// type they share the way they already share `zugferd::Profile`; the
/// projection now exists once, here, where the gate and the renderer both use
/// it.
///
/// The Textform kinds keep sending their view directly: their producer
/// (`accountingd`, for a Mahnung) has no EN 16931 model to send, and their view
/// *is* the contract.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RenderSubject {
    /// The EN 16931 semantic model. outputd projects the page view from it.
    #[serde(rename = "model")]
    Model(Box<en16931::Invoice>),
    /// A Textform view, serialised by its producer.
    #[serde(rename = "view")]
    View(serde_json::Value),
}

/// The invoice payload to embed.
#[derive(Debug, Deserialize)]
pub struct RenderAttachment {
    /// The CII XML, rendered and (for B2G) validated by the caller.
    pub xml: String,
    /// BT-24 verbatim; the ZUGFeRD profile — filename, conformance level — is
    /// derived from it, so carrier and payload cannot disagree.
    pub specification_id: String,
}

/// Whether this template may render this request — the render endpoint's
/// admission rules, all `422` when violated.
///
/// **Kind** matters for proof discipline: an INVOICE render with a
/// Textform-proven template would wrap a carrier around a layout the gate never
/// proved as one (`RENDERED_PDFA` is exactly the claim it lacks). A pinned hash
/// resolves by identity, so nothing upstream has checked that.
///
/// There is no tenant check here any more, because there is nothing left to
/// check: [`template_store::by_hash`] is tenant-scoped, so a hash belonging to
/// another operator does not resolve at all. The rule lives in the query rather
/// than in a condition each caller-facing path has to remember.
///
/// The attachment contract follows the kind: an INVOICE *is* the hybrid
/// document, so a render without the CII payload would produce a handsome PDF
/// that is not an invoice — the one failure mode that looks like success; a
/// Textform kind carries no embedded invoice, so an attachment there means the
/// caller is confused about what it is rendering.
fn render_admissible(
    kind: TemplateKind,
    template: &template_store::StoredTemplate,
    has_attachment: bool,
) -> OutputResult<()> {
    if template.kind != kind.as_str() {
        return Err(OutputError::unprocessable(
            "TEMPLATE_WRONG_KIND",
            format!(
                "template {} is a {} template, not {}",
                template.hash,
                template.kind,
                kind.as_str()
            ),
        ));
    }
    match (kind, has_attachment) {
        (TemplateKind::Invoice, false) => Err(OutputError::unprocessable(
            "ATTACHMENT_REQUIRED",
            "an INVOICE render requires the attachment (the CII payload and its BT-24)",
        )),
        (TemplateKind::Mahnung | TemplateKind::Preisanpassung, true) => {
            Err(OutputError::unprocessable(
                "ATTACHMENT_NOT_ALLOWED",
                format!(
                    "a {} is Textform and carries no embedded invoice — omit the attachment",
                    kind.as_str()
                ),
            ))
        }
        _ => Ok(()),
    }
}

/// `POST /api/v1/render/{kind}` — render a view with a stored template.
///
/// Returns the PDF; `X-Mako-Template-Hash` names the template used, which is
/// how a caller pins. A `template_hash` that was never published is `422`; a
/// tenant with nothing rolled out for the kind is `422` with the fix.
pub async fn post_render(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(kind): Path<String>,
    Json(req): Json<RenderApiRequest>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "render-document", &cfg.tenant)?;
    let kind = parse_kind(&kind)?;
    let rendered = render_with_stored_template(&pool, &cfg, kind, &req).await?;
    Ok((
        StatusCode::OK,
        [
            ("Content-Type", rendered.media_type.to_owned()),
            ("X-Mako-Template-Hash", rendered.template_hash.clone()),
        ],
        rendered.bytes,
    ))
}

/// A finished document: the bytes and the template that produced them.
pub struct RenderedDocument {
    pub bytes: Vec<u8>,
    pub template_hash: String,
    pub media_type: &'static str,
}

/// Render `req` with the tenant's stored template — the body both
/// `POST /render/{kind}` and `POST /documents/{kind}` run.
///
/// One implementation, because issuing a document is rendering one plus
/// recording it: a second copy of the resolution, admission and stamping rules
/// would give the two endpoints different behaviour for the same input.
///
/// # Errors
///
/// Every refusal `post_render` can answer with: an unresolvable template, a
/// kind/subject mismatch, an unusable attachment, a template that does not
/// compile.
pub async fn render_with_stored_template(
    pool: &PgPool,
    cfg: &OutputdConfig,
    kind: TemplateKind,
    req: &RenderApiRequest,
) -> OutputResult<RenderedDocument> {
    let date = time::Date::parse(
        &req.date,
        &time::format_description::well_known::Iso8601::DATE,
    )
    .map_err(|e| OutputError::bad_request("INVALID_DATE", format!("date: {e}")))?;

    // The subject and the kind must agree: an invoice is rendered from the
    // semantic model, a Textform document from its own view. Mixing them would
    // hand the template a dictionary whose every field lookup fails, with a
    // diagnostic that blamed the operator's layout for the caller's mistake.
    let data = match (kind, &req.subject) {
        (TemplateKind::Invoice, RenderSubject::Model(model)) => {
            serde_json::to_string(&crate::document::DocumentView::of(model))
                .map_err(|e| OutputError::Internal(e.into()))?
        }
        (TemplateKind::Mahnung | TemplateKind::Preisanpassung, RenderSubject::View(view)) => {
            view.to_string()
        }
        (TemplateKind::Invoice, RenderSubject::View(_)) => {
            return Err(OutputError::unprocessable(
                "SUBJECT_MUST_BE_A_MODEL",
                "an INVOICE render takes `model` (the EN 16931 semantic model); outputd \
                 projects the page view from it so the gate and production cannot disagree \
                 about what a template may print",
            ));
        }
        (TemplateKind::Mahnung | TemplateKind::Preisanpassung, RenderSubject::Model(_)) => {
            return Err(OutputError::unprocessable(
                "SUBJECT_MUST_BE_A_VIEW",
                format!(
                    "a {} render takes `view` — it is not an EN 16931 document",
                    kind.as_str()
                ),
            ));
        }
    };

    let template = match req.template_hash {
        Some(ref hash) => template_store::by_hash(pool, &cfg.tenant, hash)
            .await
            .map_err(OutputError::Internal)?
            .ok_or_else(|| {
                OutputError::unprocessable(
                    "TEMPLATE_NOT_PUBLISHED",
                    format!("no template of this tenant has hash {hash}"),
                )
            })?,
        None => template_store::current(pool, &cfg.tenant, kind)
            .await
            .map_err(OutputError::Internal)?
            .ok_or_else(|| {
                OutputError::unprocessable(
                    "NO_CURRENT_TEMPLATE",
                    format!(
                        "no {} template is rolled out for this tenant — publish one and \
                         PUT /api/v1/templates/{}/current",
                        kind.as_str(),
                        kind.as_str(),
                    ),
                )
            })?,
    };

    render_admissible(kind, &template, req.attachment.is_some())?;

    // The attachment, when the document carries one, is derived from BT-24 —
    // the caller cannot ask for a filename or a conformance level the payload
    // does not declare.
    let (attachment, profile) = match req.attachment {
        None => (None, None),
        Some(ref a) => {
            let profile = crate::document::facturx::Profile::parse(&a.specification_id);
            let att = crate::document::facturx::attachment(profile, a.xml.clone())
                .map_err(|e| OutputError::unprocessable("ATTACHMENT_UNUSABLE", format!("{e:#}")))?;
            (Some(att), Some(profile))
        }
    };

    let request = crate::document::RenderRequest {
        template: template.source.clone(),
        data: Some(data),
        attachment,
        standard: (kind == TemplateKind::Invoice).then(|| {
            template
                .pdf_standard
                .clone()
                .unwrap_or_else(|| DEFAULT_PDF_STANDARD.to_owned())
        }),
        date,
        ident: format!("{}:{}:{}", cfg.tenant, template.hash, req.ident),
    };
    let rendered = crate::document::render::render_guarded(request, RENDER_BUDGET).await?;
    for warning in &rendered.warnings {
        tracing::warn!(template = %template.hash, %warning, "render warning");
    }

    let pdf = match profile {
        Some(profile) => crate::document::facturx::stamp(&rendered.pdf, profile)
            .map_err(OutputError::Internal)?,
        None => rendered.pdf,
    };

    Ok(RenderedDocument {
        bytes: pdf,
        template_hash: template.hash.clone(),
        media_type: "application/pdf",
    })
}

// ── The document API ──────────────────────────────────────────────────────────
//
// `POST /render/{kind}` produces bytes and forgets them — right for a preview,
// a re-print, or a caller with its own archive. `POST /documents/{kind}` is the
// same render, recorded and queued, which is what makes "did the customer get
// this?" a question with an answer.

/// `POST /api/v1/documents/{kind}` body — a render plus who it goes to.
#[derive(Debug, Deserialize)]
pub struct IssueDocumentRequest {
    /// Everything `POST /render/{kind}` takes.
    #[serde(flatten)]
    pub render: RenderApiRequest,
    /// What this document is *about*, in the issuing service's own terms: a
    /// Rechnungsnummer, a dunning-case id, a Vertragsnummer.
    ///
    /// The idempotency key: a retrying issuer gets the document it already
    /// issued rather than sending a second notice — and a second Mahnung starts
    /// a second § 41f clock nobody can reconcile with the first.
    pub subject_ref: String,
    /// The Marktlokation this document concerns, for the portal's per-MaLo
    /// scope and the operator's search.
    #[serde(default)]
    pub malo_id: Option<String>,
    /// The business partner (`vertragd` `kunden_nr`), for a B2B customer whose
    /// documents span several Marktlokationen.
    #[serde(default)]
    pub kunden_nr: Option<String>,
    /// Where it goes, snapshotted at issue — the question afterwards is where
    /// the notice *was sent*, which live master data cannot answer.
    #[serde(default)]
    pub recipient: crate::delivery::store::Recipient,
    /// Channels to queue. Defaults to `["PORTAL"]` — the durable medium
    /// § 126b BGB asks for, and the one that needs no external adapter.
    #[serde(default)]
    pub channels: Option<Vec<crate::delivery::Channel>>,
}

/// `POST /api/v1/documents/{kind}` — render, record, and queue for delivery.
///
/// `201` for a document that was issued now, `200` for one this
/// `(kind, subject_ref)` already has. Both answer the same body, so a caller
/// that retried does not have to distinguish them to proceed.
pub async fn post_document(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(kind): Path<String>,
    Json(req): Json<IssueDocumentRequest>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "issue-document", &cfg.tenant)?;
    let kind = parse_kind(&kind)?;
    if req.subject_ref.trim().is_empty() {
        return Err(OutputError::bad_request(
            "SUBJECT_REF_REQUIRED",
            "subject_ref names what this document is about and is its idempotency key — \
             a Rechnungsnummer, a dunning-case id, a Vertragsnummer",
        ));
    }
    let channels = req
        .channels
        .clone()
        .unwrap_or_else(|| vec![crate::delivery::Channel::Portal]);
    if channels.is_empty() {
        return Err(OutputError::bad_request(
            "NO_CHANNEL",
            "an empty channel list would store a document nobody is ever sent — \
             omit the field for the portal inbox, or name the channels",
        ));
    }

    // Before the render: discovering a retry afterwards costs twenty seconds of
    // Typst and, if the template has moved since, produces bytes that differ
    // from the ones actually sent.
    if let Some(existing) =
        crate::delivery::store::by_subject(&pool, &cfg.tenant, kind.as_str(), &req.subject_ref)
            .await
            .map_err(OutputError::Internal)?
    {
        return Ok((
            StatusCode::OK,
            [(
                "X-Mako-Template-Hash",
                existing.document.template_hash.clone(),
            )],
            Json(existing),
        ));
    }

    let rendered = render_with_stored_template(&pool, &cfg, kind, &req.render).await?;
    let (issued, created) = crate::delivery::store::issue(
        &pool,
        &crate::delivery::store::NewDocument {
            tenant: &cfg.tenant,
            kind: kind.as_str(),
            template_hash: &rendered.template_hash,
            subject_ref: &req.subject_ref,
            malo_id: req.malo_id.as_deref(),
            kunden_nr: req.kunden_nr.as_deref(),
            content: &rendered.bytes,
            media_type: rendered.media_type,
            recipient: req.recipient.clone(),
            issued_by: Some(claims.sub()),
        },
        &channels,
    )
    .await
    .map_err(OutputError::Internal)?;

    tracing::info!(
        document_id = %issued.document.document_id,
        kind = kind.as_str(),
        subject = %req.subject_ref,
        channels = channels.len(),
        created,
        "outputd: document issued"
    );
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        [("X-Mako-Template-Hash", rendered.template_hash)],
        Json(issued),
    ))
}

/// `GET /api/v1/documents/{document_id}` — metadata plus every delivery track.
pub async fn get_document(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(document_id): Path<uuid::Uuid>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-document", &cfg.tenant)?;
    crate::delivery::store::by_id(&pool, &cfg.tenant, document_id)
        .await
        .map_err(OutputError::Internal)?
        .map(Json)
        .ok_or_else(|| {
            OutputError::not_found("DOCUMENT_NOT_FOUND", format!("no document {document_id}"))
        })
}

/// `GET /api/v1/documents/{document_id}/content` — the bytes that were sent.
///
/// A reproduction, not a re-render: § 147 Abs. 1 AO asks for the document as
/// issued, and the template that produced it may have moved since.
pub async fn get_document_content(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(document_id): Path<uuid::Uuid>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-document", &cfg.tenant)?;
    let (bytes, media_type) = crate::delivery::store::content(&pool, &cfg.tenant, document_id)
        .await
        .map_err(OutputError::Internal)?
        .ok_or_else(|| {
            OutputError::not_found("DOCUMENT_NOT_FOUND", format!("no document {document_id}"))
        })?;
    Ok((StatusCode::OK, [("Content-Type", media_type)], bytes))
}

/// `GET /api/v1/documents?malo_id=…&kunden_nr=…&kind=…` — the portal inbox.
///
/// Scoped by construction: a request naming neither a MaLo nor a business
/// partner is refused rather than answered with the whole portfolio. `portald`
/// forwards a customer's scope into this query, and a filter that degrades to
/// "everything" is one bug away from serving it to whoever asks.
pub async fn list_documents(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Query(filter): Query<crate::delivery::store::DocumentFilter>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-document", &cfg.tenant)?;
    if filter.malo_id.is_none() && filter.kunden_nr.is_none() {
        return Err(OutputError::bad_request(
            "UNSCOPED_QUERY",
            "name a malo_id or a kunden_nr — an unscoped document list is not a query this \
             API answers",
        ));
    }
    crate::delivery::store::list(&pool, &cfg.tenant, &filter)
        .await
        .map(Json)
        .map_err(OutputError::Internal)
}

/// `POST /api/v1/deliveries/{delivery_id}/read` — the customer opened it.
///
/// Called by `portald` when a customer views the document. More than Textform
/// asks for, and exactly what a § 41f dispute asks about. Set once.
pub async fn post_delivery_read(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(delivery_id): Path<uuid::Uuid>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-document", &cfg.tenant)?;
    if crate::delivery::store::record_read(&pool, &cfg.tenant, delivery_id)
        .await
        .map_err(OutputError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(OutputError::not_found(
            "PORTAL_DELIVERY_NOT_FOUND",
            format!("no portal delivery {delivery_id} for this tenant"),
        ))
    }
}

/// `POST /api/v1/deliveries/{delivery_id}/status` body — what a channel
/// reports back.
#[derive(Debug, Deserialize)]
pub struct DeliveryStatusReport {
    /// `true` when the far end observed the document **arrive** — the
    /// recipient's server accepted it, the letter was posted. Anything less is
    /// the hand-off the send already recorded.
    #[serde(default)]
    pub delivered: bool,
    /// A failure reported after the fact — a bounce, a rejected print job.
    /// Present means the attempt failed however it looked at send time.
    #[serde(default)]
    pub error: Option<String>,
    /// The channel's own receipt: message id, batch reference, carrier
    /// tracking. Stored as the evidence.
    #[serde(default)]
    pub evidence: Option<serde_json::Value>,
}

/// `POST /api/v1/deliveries/{delivery_id}/status` — an asynchronous outcome.
///
/// The half of delivery that cannot be known at send time: a relay accepting a
/// message is not the recipient's server accepting it, so `EMAIL` and `POST`
/// reach `SENT` on the send and `DELIVERED` only here — and a bounce reported
/// here turns an apparently-successful notice into the failure it was.
pub async fn post_delivery_status(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Path(delivery_id): Path<uuid::Uuid>,
    Json(report): Json<DeliveryStatusReport>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "report-delivery", &cfg.tenant)?;
    match report.error {
        Some(error) => {
            // Terminal: the channel has told us it did not arrive, so retrying
            // the same hand-off would only produce the same bounce.
            crate::delivery::store::record_failure(
                &pool,
                &cfg.tenant,
                delivery_id,
                &error,
                true,
                time::Duration::ZERO,
            )
            .await
            .map_err(OutputError::Internal)?;
            tracing::error!(
                %delivery_id, %error,
                "outputd: a delivery channel reported the document did not arrive"
            );
        }
        None => {
            crate::delivery::store::record_success(
                &pool,
                &cfg.tenant,
                delivery_id,
                report.delivered,
                report.evidence,
            )
            .await
            .map_err(OutputError::Internal)?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/spool?limit=…` — what a print service collects.
///
/// The pull half of the `POST` channel, which is how most Druckdienstleister
/// integrate: list what is waiting, fetch each document's bytes from
/// `/documents/{id}/content`, and report back through
/// `/deliveries/{id}/status` once the letters are in the post.
pub async fn get_spool(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<OutputdConfig>>,
    Query(q): Query<SpoolQuery>,
) -> OutputResult<impl IntoResponse> {
    authorize(&cedar, &claims, "read-document", &cfg.tenant)?;
    let rows = crate::delivery::store::postal_spool(&pool, &cfg.tenant, q.limit.unwrap_or(100))
        .await
        .map_err(OutputError::Internal)?;
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "delivery_id":    d.delivery_id,
                "document_id":    d.document_id,
                "kind":           d.kind,
                "subject_ref":    d.subject_ref,
                "malo_id":        d.malo_id,
                "kunden_nr":      d.kunden_nr,
                "recipient_name": d.recipient_name,
                "address":        d.target,
                "media_type":     d.media_type,
                "content_url":    format!("/api/v1/documents/{}/content", d.document_id),
            })
        })
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct SpoolQuery {
    pub limit: Option<i64>,
}
