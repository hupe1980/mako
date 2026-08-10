//! Billing record read endpoints (list / get / XRechnung download / PDF).

#[allow(unused_imports)]
use super::*;

use crate::clients::OutputdClient;
use crate::document_view::DocumentView;
use en16931_formats::zugferd::Profile;

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
    Extension(outputd): Extension<Arc<OutputdClient>>,
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

    // The profile the document declares decides the embedded filename and the
    // carrier's conformance level; a B2G document is additionally *proven*
    // against XRechnung before it is sent to the renderer, so a rejectable file
    // cannot ship. What the invoice *says* is proven here, where the model
    // lives; how it *looks* is outputd's job.
    let profile = model
        .specification_id
        .as_deref()
        .map_or(Profile::Unknown, Profile::parse);
    // A profile that is not an EN 16931 invoice (MINIMUM, BASIC WL) — or one
    // this system does not recognise — cannot be wrapped in a carrier that
    // claims to hold an invoice. outputd would refuse it too, but from here
    // that surfaces as a gateway error; the defect is in this record's BT-24,
    // and the answer must say so.
    if let en16931_formats::zugferd::IsInvoice::No(why) = profile.is_en16931_invoice() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "the stored model's BT-24 declares {profile}, which is not an EN 16931 \
                 invoice ({why}) — re-run the billing calculation"
            ),
        )
            .into_response();
    }
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
        // Everything else is plain CII — validated before it leaves billingd.
        // outputd wraps whatever payload it is handed exactly as faithfully
        // when it is invalid, so the sender is the only place this check can
        // live.
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
    let Some(specification_id) = model.specification_id.clone() else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "the stored model carries no BT-24 — re-run the billing calculation",
        )
            .into_response();
    };

    let view = DocumentView::of(&model);
    // BT-2, not the wall clock: `datetime.today()` in the template and the
    // PDF's creation timestamp are both the invoice's own date. Falling back to
    // the period end keeps a model without BT-2 deterministic rather than
    // letting the clock in through the back door.
    let date: time::Date = model.issue_date.map_or(row.period_to, Into::into);

    // First render with the pinned template if the record has one, otherwise
    // with whatever outputd has rolled out for the tenant. outputd names the
    // template it used in `X-Mako-Template-Hash`; for an issued record that
    // hash is then pinned so a request a decade later reproduces the document
    // that was sent rather than re-styling it. A draft pins nothing and renders
    // with the current layout every time — see [`crate::pg::pin_template`].
    //
    // Racing first renders of the same record are safe: the pin is a
    // conditional update that returns the winning hash, and the loser re-renders
    // once with the winner. Two renders suffice whenever outputd honours its
    // contract (a requested hash is the hash used); the bound turns a renderer
    // that does not into a 502 instead of a hot loop.
    let mut chosen = row.template_hash.clone();
    let mut renders = 0;
    let rendered = loop {
        if renders >= 2 {
            return (
                StatusCode::BAD_GATEWAY,
                "outputd keeps answering with a template other than the requested one",
            )
                .into_response();
        }
        renders += 1;
        let rendered = match outputd
            .render_invoice(
                &view,
                xml.clone(),
                &specification_id,
                chosen.as_deref(),
                date,
                &id.to_string(),
            )
            .await
        {
            Ok(r) => r,
            // outputd's deterministic refusals are this record's (or this
            // tenant's rollout state's) fault — relay them as such. Only a
            // renderer that cannot answer is a gateway problem.
            Err(e @ crate::clients::OutputdError::Refused { .. }) => {
                return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response();
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        };
        if chosen.as_deref() == Some(rendered.template_hash.as_str()) {
            break rendered; // pinned render — nothing left to agree on
        }
        match crate::pg::pin_template(&pool, id, &rendered.template_hash).await {
            // Draft (no pin written) or this render won the pin: ship it.
            Ok(None) => break rendered,
            Ok(Some(pinned)) if pinned == rendered.template_hash => break rendered,
            // A concurrent render pinned a different hash first — reproduce
            // *that* document.
            Ok(Some(pinned)) => chosen = Some(pinned),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    let name = model.number.as_deref().map_or_else(
        || id.to_string(),
        |n| {
            // BT-1 reaches an HTTP header here; a quote or a newline in it
            // would end the header early. ASCII-only, because `HeaderValue`
            // refuses non-ASCII outright — `is_alphanumeric` would wave an
            // umlaut through and turn it into a 500 at header construction.
            n.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        },
    );
    (
        StatusCode::OK,
        [
            ("Content-Type", "application/pdf".to_owned()),
            (
                "Content-Disposition",
                format!("attachment; filename=\"rechnung-{name}.pdf\""),
            ),
        ],
        rendered.pdf,
    )
        .into_response()
}
