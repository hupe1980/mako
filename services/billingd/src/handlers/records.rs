//! Billing record read endpoints (list / get / XRechnung download / PDF).

#[allow(unused_imports)]
use super::*;

use super::templates::{RENDER_BUDGET, pdf_response};
use crate::document::facturx::{self, Profile};
use crate::document::render::{RenderRequest, render_guarded};
use crate::document::view::DocumentView;
use crate::template_store::{self, StoredTemplate, TemplateKind};

// ── Records ────────────────────────────────────────────────────────────────────

pub async fn list_records(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Query(q): Query<RecordsQuery>,
) -> impl IntoResponse {
    match list_billing_records(
        &pool,
        &cfg.tenant,
        q.malo_id.as_deref(),
        q.lf_mp_id.as_deref(),
        q.outcome.as_deref(),
        q.limit.unwrap_or(100).min(1000),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RecordsQuery {
    pub malo_id: Option<String>,
    pub lf_mp_id: Option<String>,
    pub outcome: Option<String>,
    pub limit: Option<i64>,
}

pub async fn get_record(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_billing_record(&pool, &cfg.tenant, id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/billing/{id}/xrechnung` — ZUGFeRD 2.3 / XRechnung 3.0 CII XML.
pub async fn get_xrechnung(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, &cfg.tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    // Render from the stored EN 16931 semantic model (per-line VAT intact) via
    // `en16931-formats`.
    let Some(model) = row
        .en16931_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
    else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "record has no EN 16931 model — re-run the billing calculation",
        )
            .into_response();
    };
    let xml = crate::einvoice::render_cii(&model);
    (
        StatusCode::OK,
        [
            ("Content-Type", "application/xml; charset=UTF-8"),
            (
                "Content-Disposition",
                &format!("attachment; filename=\"xrechnung-{id}.xml\""),
            ),
        ],
        xml,
    )
        .into_response()
}

/// `GET /api/v1/billing/{id}/pdf` — the ZUGFeRD document.
///
/// One file that is both things a customer and their accounting software need:
/// a page a person reads, and the EN 16931 invoice embedded inside it. Both come
/// from the same stored model, so they cannot disagree.
///
/// The template is **pinned on the first render after dispatch**, so requesting
/// this a decade later reproduces the document that was issued rather than
/// re-styling it with whatever layout is current. A draft still renders with
/// the current template every time — see [`crate::pg::pin_template`].
pub async fn get_invoice_pdf(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, &cfg.tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let Some(model) = row
        .en16931_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
    else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "record has no EN 16931 model — re-run the billing calculation",
        )
            .into_response();
    };

    let template = match resolve_template(&pool, &cfg.tenant, &row).await {
        Ok(t) => t,
        Err(response) => return response,
    };

    // The profile the document declares decides the embedded filename and the
    // carrier's conformance level; a B2G document is additionally *proven*
    // against XRechnung before it is written, so a rejectable file cannot ship.
    let profile = facturx::profile_of(&model);
    let xml = match profile {
        Profile::XRechnung => match crate::einvoice::render_xrechnung_cii(&model) {
            Ok(xml) => xml,
            Err(findings) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "this document declares XRechnung but does not satisfy it:\n{}",
                        findings.join("\n")
                    ),
                )
                    .into_response();
            }
        },
        // Everything else is plain CII — validated against the profile the
        // document declares before it is embedded. The B2G arm above proves
        // itself through `to_string_for`; without this arm doing the same, a
        // retail record whose stored model has gone invalid would be wrapped in
        // a conformant carrier and shipped — the publish gate validates only
        // its own specimen, and a carrier round-trips an invalid payload
        // exactly as faithfully as a valid one.
        _ => {
            let fatal: Vec<String> = crate::einvoice::validate(&model)
                .fatal()
                .map(|f| format!("[{}] {} — {}", f.rule, f.path, f.message))
                .collect();
            if !fatal.is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "the stored model does not satisfy the profile it declares in \
                         BT-24 — re-run the billing calculation:\n{}",
                        fatal.join("\n")
                    ),
                )
                    .into_response();
            }
            crate::einvoice::render_cii(&model)
        }
    };

    let request = RenderRequest {
        template: template.source.clone(),
        data: match serde_json::to_string(&DocumentView::of(&model)) {
            Ok(json) => Some(json),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        attachment: Some(match facturx::attachment(profile, xml) {
            Ok(a) => a,
            Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
        }),
        standard: Some(
            template
                .pdf_standard
                .clone()
                .unwrap_or_else(|| crate::document::gate::DEFAULT_PDF_STANDARD.to_owned()),
        ),
        // BT-2, not the wall clock: `datetime.today()` in the template and the
        // PDF's creation timestamp are both the invoice's own date. Falling
        // back to the period end keeps a model without BT-2 deterministic
        // rather than letting the clock in through the back door.
        date: model.issue_date.map_or(row.period_to, Into::into),
        // Stable across renders, and distinct across documents and layouts.
        ident: format!("{}:{}:{}", cfg.tenant, template.hash, id),
    };

    let rendered = match render_guarded(request, RENDER_BUDGET).await {
        Ok(rendered) => rendered,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    };
    for warning in &rendered.warnings {
        tracing::warn!(record = %id, template = %template.hash, %warning, "invoice render warning");
    }
    match facturx::stamp(&rendered.pdf, profile) {
        Ok(pdf) => {
            let name = model.number.as_deref().map_or_else(
                || id.to_string(),
                |n| {
                    // BT-1 reaches an HTTP header here; a quote or a newline in
                    // it would end the header early. Invoice numbers are ours,
                    // but a filename is the wrong place to rely on that.
                    n.chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '-' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect()
                },
            );
            pdf_response(&format!("rechnung-{name}.pdf"), pdf)
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// The template this record's document is rendered with, pinning it if this is
/// the first render.
///
/// Racing renders of the same record are safe: the pin is a conditional update
/// that returns the winning hash, and both callers then use it.
async fn resolve_template(
    pool: &PgPool,
    tenant: &str,
    row: &crate::pg::BillingRecordRow,
) -> Result<StoredTemplate, axum::response::Response> {
    let chosen = match &row.template_hash {
        Some(hash) => hash.clone(),
        None => match template_store::current(pool, tenant, TemplateKind::Invoice).await {
            Ok(Some(t)) => t.hash,
            Ok(None) => {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "no invoice template is rolled out for this tenant — publish one and \
                     PUT /api/v1/templates/INVOICE/current",
                )
                    .into_response());
            }
            Err(e) => {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response());
            }
        },
    };
    let pinned = match crate::pg::pin_template(pool, row.id, &chosen).await {
        Ok(Some(hash)) => hash,
        Ok(None) => chosen,
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
    };
    match template_store::by_hash(pool, &pinned).await {
        Ok(Some(t)) => Ok(t),
        // The foreign key makes this unreachable; report it as the server fault
        // it would be rather than blaming the caller.
        Ok(None) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("record pins template {pinned}, which is not in the store"),
        )
            .into_response()),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
    }
}
