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

use super::*;

use crate::document::gate::{self, DEFAULT_PDF_STANDARD};
use crate::template_store::{self, TemplateKind};

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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Json(req): Json<PublishTemplateRequest>,
) -> impl IntoResponse {
    if req.source.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "source is empty").into_response();
    }
    let standard = req
        .pdf_standard
        .clone()
        .unwrap_or_else(|| DEFAULT_PDF_STANDARD.to_owned());

    let proven = match prove(req.kind, req.source.clone(), standard.clone()).await {
        Ok(proven) => proven,
        Err(response) => return response,
    };

    // Only the invoice kind has a PDF/A level to have met; recording one for a
    // Textform template would claim a conformance nothing checked.
    let pdf_standard = (req.kind == TemplateKind::Invoice).then_some(standard.as_str());
    match template_store::publish(
        &pool,
        &cfg.tenant,
        req.kind,
        &req.source,
        pdf_standard,
        proven.proof,
        Some(claims.sub()),
    )
    .await
    {
        Ok(hash) => (
            StatusCode::CREATED,
            Json(PublishedTemplate {
                hash,
                proof: proven.proof,
                pages: proven.pages,
                warnings: proven.warnings,
            }),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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
    _claims: Claims,
    Json(req): Json<PublishTemplateRequest>,
) -> impl IntoResponse {
    if req.source.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "source is empty").into_response();
    }
    if req.kind != TemplateKind::Invoice {
        // Rendering a Mahnung against an invoice specimen would fail on the
        // first field it reads, with a diagnostic that blamed the template.
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "there is no specimen to preview a {} template against — its data contract \
                 does not live in billingd yet; publishing checks that it parses",
                req.kind.as_str(),
            ),
        )
            .into_response();
    }
    let standard = req
        .pdf_standard
        .unwrap_or_else(|| DEFAULT_PDF_STANDARD.to_owned());
    // The same specimen the gate proves against, so a preview is the artefact
    // publishing would produce rather than an approximation of it.
    let model = gate::specimen_invoice();
    let profile = crate::document::facturx::profile_of(&model);
    let attachment =
        match crate::document::facturx::attachment(profile, crate::einvoice::render_cii(&model)) {
            Ok(a) => a,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        };
    let request = crate::document::RenderRequest {
        template: req.source,
        data: Some(
            match serde_json::to_string(&crate::document::DocumentView::of(&model)) {
                Ok(json) => json,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            },
        ),
        attachment: Some(attachment),
        standard: Some(standard),
        date: gate::SPECIMEN_DATE,
        ident: "mako-template-preview".to_owned(),
    };
    let rendered = match crate::document::render::render_guarded(request, RENDER_BUDGET).await {
        Ok(rendered) => rendered,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    };
    match crate::document::facturx::stamp(&rendered.pdf, profile) {
        Ok(pdf) => pdf_response("vorschau.pdf", pdf),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `GET /api/v1/templates/reference` — the layout mako ships.
///
/// Served rather than only documented so an operator's starting point is the
/// exact source the test suite compiles on every run, not a copy of it that has
/// drifted.
pub async fn get_reference_template(_claims: Claims) -> impl IntoResponse {
    (
        StatusCode::OK,
        [(
            "Content-Type",
            "text/plain; charset=UTF-8; x-typst-version=0.15",
        )],
        crate::document::REFERENCE_INVOICE_TEMPLATE,
    )
        .into_response()
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
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Query(q): Query<ListTemplatesQuery>,
) -> impl IntoResponse {
    let kind = match q.kind.as_deref() {
        None => None,
        Some(k) => match parse_kind(k) {
            Some(kind) => Some(kind),
            None => return (StatusCode::BAD_REQUEST, "unknown template kind").into_response(),
        },
    };
    match template_store::list(
        &pool,
        &cfg.tenant,
        kind,
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(kind): Path<String>,
    Json(req): Json<SetCurrentRequest>,
) -> impl IntoResponse {
    let Some(kind) = parse_kind(&kind) else {
        return (StatusCode::BAD_REQUEST, "unknown template kind").into_response();
    };
    match template_store::set_current(&pool, &cfg.tenant, kind, &req.hash).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("no published template with that hash: {e}"),
        )
            .into_response(),
    }
}

/// `GET /api/v1/templates/{kind}/current` — what this tenant renders with now.
pub async fn get_current_template(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(kind): Path<String>,
) -> impl IntoResponse {
    let Some(kind) = parse_kind(&kind) else {
        return (StatusCode::BAD_REQUEST, "unknown template kind").into_response();
    };
    match template_store::current(&pool, &cfg.tenant, kind).await {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/templates/by-hash/{hash}` — resolve a template by hash.
///
/// This is how an audit answers "why did the invoice from 2027 look like that":
/// the record carries `template_hash`, and the source is still here.
pub async fn get_template_by_hash(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    match template_store::by_hash(&pool, &hash).await {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Run the publish gate off the async runtime.
///
/// Typesetting is CPU-bound and takes long enough to matter; leaving it on a
/// runtime worker would stall every other request on the same thread.
async fn prove(
    kind: TemplateKind,
    source: String,
    standard: String,
) -> Result<gate::Proven, axum::response::Response> {
    let task =
        tokio::task::spawn_blocking(move || gate::prove(kind, &source, Some(standard.as_str())));
    match tokio::time::timeout(RENDER_BUDGET, task).await {
        Ok(Ok(Ok(proven))) => Ok(proven),
        // The template did not survive the gate. Its diagnostics are the
        // response body: they name the operator's file, line and column.
        Ok(Ok(Err(e))) => Err((StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response()),
        Ok(Err(join)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the publish gate panicked: {join}"),
        )
            .into_response()),
        Err(_) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "the template did not finish rendering within {RENDER_BUDGET:?}; \
                 it is doing far more work than one invoice needs"
            ),
        )
            .into_response()),
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

fn parse_kind(s: &str) -> Option<TemplateKind> {
    match s.to_ascii_uppercase().as_str() {
        "INVOICE" => Some(TemplateKind::Invoice),
        "MAHNUNG" => Some(TemplateKind::Mahnung),
        "PREISANPASSUNG" => Some(TemplateKind::Preisanpassung),
        _ => None,
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
            assert_eq!(parse_kind(k.as_str()), Some(k));
        }
        assert_eq!(parse_kind("invoice"), Some(TemplateKind::Invoice));
        assert_eq!(parse_kind("nope"), None);
    }

    /// The default the API applies is the one ZUGFeRD requires.
    #[test]
    fn the_default_standard_is_the_one_zugferd_requires() {
        assert_eq!(DEFAULT_PDF_STANDARD, "a-3b");
    }
}
