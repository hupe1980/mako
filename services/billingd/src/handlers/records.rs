//! Billing record read endpoints (list / get / XRechnung download / PDF).

use super::*;

use en16931_formats::zugferd::Profile;

// ── Records ────────────────────────────────────────────────────────────────────

pub async fn list_records(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Query(q): Query<RecordsQuery>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    let rows = list_billing_records(
        &pool,
        &cfg.tenant,
        &crate::pg::RecordFilter {
            malo_id: q.malo_id.as_deref(),
            lf_mp_id: q.lf_mp_id.as_deref(),
            outcome: q.outcome.as_deref(),
            category: q.category.as_deref(),
            is_correction: q.is_correction,
            // Clamped at both ends: `min` alone let `?limit=-1` through to
            // `LIMIT -1`, which PostgreSQL rejects — a 500 for a request the
            // API should simply have bounded.
            limit: q.limit.unwrap_or(100).clamp(1, 1000),
        },
    )
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
pub struct RecordsQuery {
    pub malo_id: Option<String>,
    pub lf_mp_id: Option<String>,
    pub outcome: Option<String>,
    /// One category — `VPP`, `SAMMEL`, `STROM`, …
    pub category: Option<String>,
    /// `true` = only Storno/Korrektur rows, `false` = only originals.
    pub is_correction: Option<bool>,
    pub limit: Option<i64>,
}

pub async fn get_record(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    let row = fetch_billing_record(&pool, &cfg.tenant, id)
        .await?
        .ok_or_else(|| BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"))?;
    Ok(Json(row))
}

/// Fetch a record and its stored EN 16931 model, or say precisely which is
/// missing.
///
/// Every render endpoint needs exactly this pair, so the two `else` branches
/// are written once. `MODEL_MISSING` is its own code because it is a different
/// problem from a missing record: the invoice exists, but nothing can be
/// rendered from it until the calculation is re-run.
async fn record_with_model(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
) -> BillingResult<(crate::pg::BillingRecordRow, en16931::Invoice)> {
    let row = fetch_billing_record(pool, tenant, id)
        .await?
        .ok_or_else(|| BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"))?;
    let model = row
        .en16931_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
        .ok_or_else(|| {
            BillingError::unprocessable(
                "MODEL_MISSING",
                "record has no EN 16931 model — re-run the billing calculation",
            )
        })?;
    Ok((row, model))
}

/// `GET /api/v1/billing/{id}/xrechnung` — ZUGFeRD 2.3 / XRechnung 3.0 CII XML.
pub async fn get_xrechnung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    // Render from the stored EN 16931 semantic model (per-line VAT intact) via
    // `en16931-formats`.
    let (_, model) = record_with_model(&pool, &cfg.tenant, id).await?;
    Ok(xml_download(
        crate::einvoice::render_cii(&model),
        format!("xrechnung-{id}.xml"),
    ))
}

/// An XML document as an attachment download.
pub(crate) fn xml_download(xml: String, filename: String) -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/xml; charset=UTF-8".to_owned(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        xml,
    )
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
///
/// # Conditional GET
///
/// A **pinned** record's PDF is immutable by construction: same stored model,
/// same pinned template, and a creation timestamp taken from BT-2 rather than
/// the clock, so the bytes are identical on every render. That is exactly what
/// a strong `ETag` and `Cache-Control: immutable` describe, and an
/// `If-None-Match` hit answers `304` without waking the renderer at all — a
/// customer portal was otherwise spending outputd's full render budget every
/// time somebody re-opened an invoice they had already seen.
///
/// A **draft** carries neither header: it re-renders with whatever template is
/// current, so it is not the same document twice and must not be cached.
pub async fn get_invoice_pdf(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
) -> BillingResult<axum::response::Response> {
    let (cfg, outputd) = (&deps.cfg, &deps.outputd);
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    let (row, model) = record_with_model(&pool, &cfg.tenant, id).await?;

    // The identity of the bytes: this record, rendered by this template. Only a
    // pinned record has one — a draft's appearance can still change.
    let etag = row
        .template_hash
        .as_deref()
        .map(|hash| format!("\"{id}-{hash}\""));
    if let Some(ref tag) = etag
        && headers
            .get(axum::http::header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|c| c.trim() == tag))
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }

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
        return Err(BillingError::unprocessable(
            "BT24_NOT_AN_INVOICE",
            format!(
                "the stored model's BT-24 declares {profile}, which is not an EN 16931 \
                 invoice ({why}) — re-run the billing calculation"
            ),
        ));
    }
    let xml = match profile {
        Profile::XRechnung => {
            crate::einvoice::render_xrechnung_cii(&model).map_err(|findings| {
                BillingError::unprocessable_with(
                    "XRECHNUNG_NOT_CONFORMANT",
                    "this document declares XRechnung but does not satisfy it",
                    serde_json::json!({ "violated_rules": findings }),
                )
            })?
        }
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
                return Err(BillingError::unprocessable_with(
                    "PROFILE_NOT_SATISFIED",
                    "the stored model does not satisfy the profile it declares in BT-24 — \
                     re-run the billing calculation",
                    serde_json::json!({ "violated_rules": fatal }),
                ));
            }
            crate::einvoice::render_cii(&model)
        }
    };
    let specification_id = model.specification_id.clone().ok_or_else(|| {
        BillingError::unprocessable(
            "BT24_MISSING",
            "the stored model carries no BT-24 — re-run the billing calculation",
        )
    })?;

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
    // Whether the record's appearance is now fixed. Only then is the response
    // cacheable — a draft renders with whatever template is current.
    let mut is_pinned = row.template_hash.is_some();
    let mut renders = 0;
    let rendered = loop {
        if renders >= 2 {
            return Err(BillingError::upstream(
                "outputd",
                "keeps answering with a template other than the requested one",
            ));
        }
        renders += 1;
        let rendered = outputd
            .render_invoice(
                &model,
                xml.clone(),
                &specification_id,
                chosen.as_deref(),
                date,
                &id.to_string(),
            )
            .await
            .map_err(|e| match e {
                // outputd's deterministic refusals are this record's (or this
                // tenant's rollout state's) fault — relay them as such. Only a
                // renderer that cannot answer is a gateway problem.
                e @ crate::clients::OutputdError::Refused { .. } => {
                    BillingError::unprocessable("RENDER_REFUSED", e.to_string())
                }
                e => BillingError::upstream("outputd", format!("{e:#}")),
            })?;
        if chosen.as_deref() == Some(rendered.template_hash.as_str()) {
            break rendered; // pinned render — nothing left to agree on
        }
        match crate::pg::pin_template(&pool, id, &rendered.template_hash).await? {
            // Draft (no pin written): ship it, and do not claim it is immutable.
            None => {
                is_pinned = false;
                break rendered;
            }
            // This render won the pin: from now on the document is fixed.
            Some(pinned) if pinned == rendered.template_hash => {
                is_pinned = true;
                break rendered;
            }
            // A concurrent render pinned a different hash first — reproduce
            // *that* document.
            Some(pinned) => chosen = Some(pinned),
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
    let mut response_headers = axum::http::HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/pdf"),
    );
    if let Ok(v) =
        axum::http::HeaderValue::from_str(&format!("attachment; filename=\"rechnung-{name}.pdf\""))
    {
        response_headers.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    // The document that was actually rendered decides the validator, not the one
    // the record carried when the request arrived: this render may be the one
    // that pinned it. `private`, because an invoice is one customer's document
    // and must never land in a shared cache.
    if is_pinned
        && let Ok(v) =
            axum::http::HeaderValue::from_str(&format!("\"{id}-{}\"", rendered.template_hash))
    {
        response_headers.insert(axum::http::header::ETAG, v);
        response_headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"),
        );
    }
    Ok((StatusCode::OK, response_headers, rendered.pdf).into_response())
}
