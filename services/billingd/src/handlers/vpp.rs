//! VPP aggregation billing, contract registry and auto-billing webhook (B12 — Art. 17 RL (EU) 2019/944).

#[allow(unused_imports)]
use super::*;

// ── VPP Aggregation Billing (B12 — Art. 17 RL (EU) 2019/944) ───────────────────────

/// One confirmed dispatch event for VPP settlement billing.
///
/// Source: WiM Steuerungsauftrag IFTSTA confirmation (PID 21039) or equivalent
/// VPP aggregator dispatch confirmation.
#[derive(Debug, serde::Deserialize)]
pub struct VppDispatchEvent {
    /// UTC dispatch start — ISO-8601 e.g. `"2026-01-15T10:00:00Z"`.
    pub start_utc: String,
    /// UTC dispatch end — ISO-8601 e.g. `"2026-01-15T10:15:00Z"`.
    pub end_utc: String,
    /// Actual flexibility delivered in kWh (positive = load reduction; negative = load increase).
    pub flexibility_kwh: rust_decimal::Decimal,
    /// IFTSTA process UUID from makod (for §20 audit trail).
    pub process_id: Option<String>,
}

/// Request body for `POST /api/v1/billing/vpp/{vpp_id}`.
///
/// `vpp_id` is the operator-assigned virtual power plant identifier
/// (typically the SR-ID of the `SteuerbareRessource` portfolio in `marktd`).
#[derive(Debug, serde::Deserialize)]
pub struct VppBillingRequest {
    /// LF/Aggregator MP-ID (invoice issuer).
    pub lf_mp_id: String,
    /// MaLo-ID of the VPP aggregation point (or primary resource).
    pub malo_id: String,
    /// Billing period start (`YYYY-MM-DD`).
    pub period_from: String,
    /// Billing period end (`YYYY-MM-DD`).
    pub period_to: String,
    /// Capacity price EUR/kWh (agreed in VPP contract or dynamic market price).
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    /// All confirmed dispatch events in the billing period.
    pub dispatch_events: Vec<VppDispatchEvent>,
    /// Optional invoice number prefix.
    pub rechnungsnummer_prefix: Option<String>,
    /// MwSt rate override (default from billingd config, typically 0.19).
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/billing/vpp/{vpp_id}`
///
/// **B12 — VPP Aggregation Settlement (Art. 17 RL (EU) 2019/944).**
///
/// Generates a settlement `Rechnung` for a Virtual Power Plant aggregator.
/// Each dispatch event becomes one `Rechnungsposition`.
///
/// ## Calculation
///
/// ```text
/// DispatchPosition_eur = flexibility_kwh * capacity_price_eur_per_kwh
/// Total_netto          = sum(DispatchPosition_eur)
/// MwSt                 = Total_netto * mwst_rate
/// Total_brutto         = Total_netto + MwSt
/// ```
///
/// ## CloudEvent emitted
///
/// `de.vpp.settlement.berechnet` (type) — consumed by ERP/DSO settlement systems.
///
/// ## Regulatory basis
///
/// § 41e EnWG (Verträge zwischen Aggregatoren und Betreibern einer Erzeugungsanlage
/// oder Letztverbrauchern), transposing Art. 17 RL (EU) 2019/944 (Demand response
/// through aggregation):
/// Aggregators must provide transparent settlement invoices per dispatch event.
pub async fn post_vpp_billing(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(vertragd): Extension<Arc<crate::clients::VertragdClient>>,
    Path(vpp_id): Path<String>,
    Json(req): Json<VppBillingRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    if req.dispatch_events.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "VPP billing requires at least one dispatch event",
        )
            .into_response();
    }

    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let mwst_rate = req
        .mwst_rate_override
        .unwrap_or_else(|| cfg.regulatory_rates().mwst_rate);

    // ── Positions from dispatch events, through the engine ────────────────────
    // Every event becomes a BillingPosition; VAT, steuerbetraege and traces come
    // from the same machinery as every other invoice instead of an inline block
    // whose Steuerkennzeichen said UST_19 whatever the override rate was.
    let mut positions: Vec<BillingPosition> = Vec::with_capacity(req.dispatch_events.len());
    let mut total_flex_kwh = Decimal::ZERO;
    // Load-increase (negative flexibility) dispatches are not billable on this
    // capacity-price model, but dropping them without trace makes a settlement
    // silently smaller than the dispatch log. Count them and report the count.
    let mut skipped_events = 0usize;
    for ev in &req.dispatch_events {
        if ev.flexibility_kwh <= Decimal::ZERO {
            skipped_events += 1;
            continue;
        }
        total_flex_kwh += ev.flexibility_kwh;
        let mut pos = BillingPosition::debit(
            format!("VPP Dispatch {} bis {}", ev.start_utc, ev.end_utc),
            ev.flexibility_kwh,
            "kWh",
            req.capacity_price_eur_per_kwh,
            PositionCategory::Fee,
        )
        .with_legal_basis("§ 41e EnWG, Art. 17 RL (EU) 2019/944, VPP-Vertrag")
        .with_tag("vpp_dispatch");
        pos.trace = energy_billing::PositionTrace::commodity(
            ev.flexibility_kwh,
            "kWh",
            req.capacity_price_eur_per_kwh,
            "§ 41e EnWG, Art. 17 RL (EU) 2019/944, VPP-Vertrag",
        );
        positions.push(pos);
    }

    if positions.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "all dispatch events have zero or negative flexibility — no billing generated",
        )
            .into_response();
    }
    if skipped_events > 0 {
        tracing::warn!(
            %vpp_id,
            malo_id = %req.malo_id,
            skipped_events,
            billed_events = positions.len(),
            "billingd VPP: dispatch events with zero or negative flexibility are not billable \
             on the capacity-price model — excluded from this settlement"
        );
    }

    let rechnungsnummer = req
        .rechnungsnummer_prefix
        .as_deref()
        .map(|p| format!("{p}-{period_from}"))
        .unwrap_or_else(|| format!("VPP-{vpp_id}-{period_from}"));

    let attrs = vec![
        zusatz_attribut("vpp_id", serde_json::json!(vpp_id)),
        zusatz_attribut(
            "total_flexibility_kwh",
            serde_json::json!(total_flex_kwh.to_string()),
        ),
        zusatz_attribut(
            "dispatch_event_count",
            serde_json::json!(req.dispatch_events.len().to_string()),
        ),
        zusatz_attribut(
            "dispatch_process_ids",
            serde_json::json!(
                req.dispatch_events
                    .iter()
                    .filter_map(|ev| ev.process_id.as_deref())
                    .collect::<Vec<_>>()
            ),
        ),
    ];
    let (invoice, rechnung_json) = match build_vpp_invoice(
        &req.malo_id,
        &req.lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        mwst_rate,
        positions,
        attrs,
    ) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let total_netto = invoice.netto_eur;
    let total_brutto = invoice.brutto_eur;

    // VPP settlement row + its `de.vpp.settlement.berechnet` outbox event commit
    // atomically, so a settled dispatch can never be persisted without its event.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let record_id = match insert_billing_record(
        &mut *tx,
        &cfg.tenant,
        &req.malo_id,
        &req.lf_mp_id,
        &format!("VPP_{vpp_id}"),
        "VPP",
        period_from,
        period_to,
        &rechnung_json,
        total_netto,
        total_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if cfg.erp_webhook_url.is_some() {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("billingd", &cfg.tenant),
            mako_events::vpp::SETTLEMENT_BERECHNET,
            vpp_id.clone(),
            serde_json::json!({
                "record_id": record_id.to_string(),
                "vpp_id": vpp_id,
                "malo_id": req.malo_id,
                "lf_mp_id": req.lf_mp_id,
                "total_flexibility_kwh": total_flex_kwh.to_string(),
                "total_netto_eur": total_netto.to_string(),
                "total_brutto_eur": total_brutto.to_string(),
                "dispatch_count": req.dispatch_events.len(),
                "rechnung": rechnung_json,
            }),
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        if let Err(e) = mark_dispatched_tx(&mut *tx, record_id).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    // The §41e settlement is issued against the prosumer behind the MaLo, so the
    // ordinary BG-7 lookup resolves the right party.
    let buyer = vertragd
        .get_vertrag_by_malo(&req.malo_id)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.rechnungsempfaenger);
    if let Err(e) = crate::einvoice::store(
        &mut *tx,
        record_id,
        &invoice,
        &cfg,
        &req.malo_id,
        buyer.as_ref(),
    )
    .await
    {
        tracing::warn!(%record_id, error = %e, "billingd: attach en16931 model failed");
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "record_id": record_id,
            "vpp_id": vpp_id,
            "malo_id": req.malo_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "dispatch_count": req.dispatch_events.len(),
            // Not billable on the capacity-price model — stated so the caller can
            // reconcile the settlement against its dispatch log.
            "skipped_non_positive_events": skipped_events,
            "total_flexibility_kwh": total_flex_kwh.to_string(),
            "total_netto_eur": total_netto.to_string(),
            "total_brutto_eur": total_brutto.to_string(),
            "mwst_eur": invoice.mwst_eur.to_string(),
            "rechnung": rechnung_json,
        })),
    )
        .into_response()
}

// ── VPP Auto-Billing Webhook (B12 — Art. 17 RL (EU) 2019/944) ────────────────

/// `POST /api/v1/webhooks/vpp-dispatch`
///
/// **VPP Dispatch Confirmed auto-billing trigger.**
///
/// Receives `de.vpp.dispatch.confirmed` CloudEvents emitted by `makod` when
/// the MSB sends a positive `EndantwortPositiv` for a WiM Steuerungsauftrag
/// (PID 55168).  Auto-generates a VPP settlement `Rechnung` using the
/// pre-configured `VppContractRow` for the dispatched SR-ID.
///
/// ## Idempotency
///
/// Each `tx_id` is recorded in `vpp_dispatch_ledger`.  Repeated delivery
/// (outbox retry) returns `202 Accepted` without re-billing.
///
/// ## HMAC verification
///
/// When `[inbound_webhook_secret]` is configured in `billingd.toml`, the
/// `X-Mako-Signature: sha256=<hex>` header is verified.  Requests with
/// invalid or missing signatures are rejected with `401 Unauthorized`.
///
/// ## Auto-billing disabled
///
/// When `vpp_auto_billing = false` in config (the default), the webhook accepts
/// events and records them in `vpp_dispatch_ledger` but does **not** generate a
/// `Rechnung`.  The manual `POST /api/v1/billing/vpp/{vpp_id}` endpoint remains
/// available.
///
/// ## CloudEvent data schema
///
/// ```json
/// {
///   "tx_id":               "abc123",
///   "location_id":         "C0001234567890",
///   "location_type":       "sr",
///   "execution_time_from": "2026-01-15T10:00:00Z",
///   "execution_time_until": "2026-01-15T10:15:00Z",
///   "max_power_kw":        "11.0",
///   "command_type":        "Konfiguration",
///   "sender_mp_id":        "9900123456789",
///   "produkt_code":        "TX-MODUL2-HT"
/// }
/// ```
pub async fn post_vpp_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(vertragd): Extension<Arc<crate::clients::VertragdClient>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // ── 1. HMAC signature verification ────────────────────────────────────────
    if let Some(ref secret) = cfg.inbound_webhook_secret {
        let sig = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.strip_prefix("sha256=").unwrap_or(v));
        match sig {
            Some(hex) if mako_service::webhook::verify_hmac(secret.as_bytes(), &body, hex) => {}
            Some(_) => {
                tracing::warn!("billingd: vpp-dispatch webhook — invalid HMAC signature");
                return StatusCode::UNAUTHORIZED.into_response();
            }
            None => {
                tracing::warn!("billingd: vpp-dispatch webhook — missing X-Mako-Signature");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }

    // ── 2. Parse CloudEvent ───────────────────────────────────────────────────
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid JSON: {e}")).into_response();
        }
    };
    let data = event
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let tx_id = data
        .get("tx_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            event
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
        })
        .to_owned();

    // ── 3. Idempotency check ───────────────────────────────────────────────────
    match crate::pg::is_vpp_dispatch_processed(&pool, &tx_id, &cfg.tenant).await {
        Ok(true) => {
            tracing::debug!(tx_id, "billingd: vpp-dispatch already processed — skipping");
            return StatusCode::ACCEPTED.into_response();
        }
        Ok(false) => {}
        Err(e) => {
            // Fail closed: proceeding without the ledger answer risks billing
            // the same dispatch twice. The sender retries on 5xx.
            tracing::error!(tx_id, error = %e, "billingd: vpp_dispatch_ledger check failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "idempotency ledger unavailable — retry later",
            )
                .into_response();
        }
    }

    // ── 4. Extract dispatch metadata ──────────────────────────────────────────
    let location_id = data
        .get("location_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let location_type = data
        .get("location_type")
        .and_then(|v| v.as_str())
        .unwrap_or("sr");
    let execution_time_from = data
        .get("execution_time_from")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let execution_time_until = data
        .get("execution_time_until")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let max_power_kw: rust_decimal::Decimal = data
        .get("max_power_kw")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(rust_decimal::Decimal::ZERO);

    // Only SR-IDs are currently supported for VPP contract lookup.
    // NeLo-IDs (grid constraint redispatch) use a different billing flow.
    if location_type != "sr" {
        tracing::debug!(
            tx_id,
            location_type,
            "billingd: vpp-dispatch webhook — skipping non-SR location"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

    // ── 5. Look up active VPP contract ────────────────────────────────────────
    // The contract is selected by the day the dispatch was *executed*, not the
    // day this webhook happens to be processed. Selecting by "today" meant a
    // replayed or delayed event could bill under a different contract version
    // than the one in force when the flexibility was actually delivered.
    let dispatch_date = parse_dispatch_date(&execution_time_from)
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    let contract = match vertragd
        .get_aggregatorvertrag(&location_id, dispatch_date)
        .await
    {
        Ok(Some(c)) => c,
        Ok(None) => {
            tracing::warn!(
                tx_id,
                sr_id = %location_id,
                "billingd: vpp-dispatch — no Aggregatorvertrag in force; cannot auto-bill"
            );
            let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
            return StatusCode::ACCEPTED.into_response();
        }
        Err(e) => {
            // vertragd unreachable: do NOT record the tx_id, so the outbox
            // retry can settle it once vertragd is back. Recording here would
            // consume the idempotency key and silently drop the dispatch.
            tracing::error!(tx_id, error = %e, "billingd: Aggregatorvertrag lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // ── 6. Check vpp_auto_billing flag ────────────────────────────────────────
    if !cfg.vpp_auto_billing {
        tracing::info!(
            tx_id,
            vpp_id = %contract.vpp_id,
            "billingd: vpp-dispatch — auto-billing disabled; recording dispatch only"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

    // ── 7. Compute flexibility_kwh from dispatch window ────────────────────────
    // flexibility_kwh = max_power_kw × duration_hours
    // Duration is derived from execution_time_until − execution_time_from.
    // Falls back to 15 minutes (standard §14a dispatch window) if no end time.
    let flexibility_kwh = compute_dispatch_flexibility_kwh(
        max_power_kw,
        &execution_time_from,
        execution_time_until.as_deref(),
    );

    if flexibility_kwh <= rust_decimal::Decimal::ZERO {
        tracing::warn!(
            tx_id,
            "billingd: vpp-dispatch — zero flexibility; no billing"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

    // ── 8. Build and run VPP billing ──────────────────────────────────────────
    // Billing period = calendar day of dispatch_from — the same day the
    // contract above was selected by.
    let period_from = dispatch_date;
    let period_to = period_from; // single-day billing record per dispatch

    let mwst_rate = contract
        .mwst_rate_override
        .unwrap_or_else(|| cfg.regulatory_rates().mwst_rate);

    let rechnungsnummer = format!(
        "VPP-{}-{}-{}",
        contract.vpp_id,
        period_from,
        tx_id.get(..8).unwrap_or(&tx_id)
    );

    // One position through the engine's canonical path — VAT, steuerbetraege
    // and the trace come from the same machinery as every other invoice.
    let mut pos = BillingPosition::debit(
        format!(
            "VPP Dispatch {} bis {} (SR: {})",
            execution_time_from,
            execution_time_until.as_deref().unwrap_or("open"),
            location_id
        ),
        flexibility_kwh,
        "kWh",
        contract.capacity_price_eur_per_kwh,
        PositionCategory::Fee,
    )
    .with_legal_basis("§ 41e EnWG, Art. 17 RL (EU) 2019/944, VPP-Vertrag")
    .with_tag("vpp_dispatch");
    pos.trace = energy_billing::PositionTrace::commodity(
        flexibility_kwh,
        "kWh",
        contract.capacity_price_eur_per_kwh,
        "§ 41e EnWG, Art. 17 RL (EU) 2019/944, VPP-Vertrag",
    );

    let attrs = vec![
        zusatz_attribut("vpp_id", serde_json::json!(contract.vpp_id.clone())),
        zusatz_attribut("tx_id", serde_json::json!(tx_id.clone())),
        zusatz_attribut("sr_id", serde_json::json!(location_id.clone())),
        zusatz_attribut(
            "flexibility_kwh",
            serde_json::json!(flexibility_kwh.to_string()),
        ),
    ];
    let (invoice, rechnung_json) = match build_vpp_invoice(
        &contract.malo_id,
        &contract.aggregator_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        mwst_rate,
        vec![pos],
        attrs,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(tx_id, error = %e, "billingd: vpp invoice build failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let position_netto = invoice.netto_eur;
    let total_brutto = invoice.brutto_eur;

    // Auto-billing is atomic across three writes that must never diverge: the
    // settlement row, the `vpp_dispatch_ledger` idempotency record, and the
    // `de.vpp.settlement.berechnet` outbox event. One transaction guarantees a
    // dispatch is billed exactly once and its event is never lost — and if the
    // commit fails the top-of-handler idempotency guard lets the sender's retry
    // reprocess cleanly (nothing was committed).
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(tx_id, error = %e, "billingd: vpp auto-billing tx begin failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let record_id = match insert_billing_record(
        &mut *tx,
        &cfg.tenant,
        &contract.malo_id,
        &contract.aggregator_mp_id,
        &format!("VPP_{}", contract.vpp_id),
        "VPP",
        period_from,
        period_to,
        &rechnung_json,
        position_netto,
        total_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(tx_id, error = %e, "billingd: vpp auto-billing insert failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Idempotency ledger — inside the tx, so a re-delivered dispatch cannot be
    // billed twice and a ledger failure fails the whole settlement closed.
    if let Err(e) =
        crate::pg::record_vpp_dispatch(&mut *tx, &tx_id, &cfg.tenant, Some(record_id)).await
    {
        tracing::error!(tx_id, error = %e, "billingd: vpp_dispatch_ledger write failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // ── 9. Enqueue de.vpp.settlement.berechnet ────────────────────────────────
    // The VPP dispatch subscriber keys on `de.vpp.settlement.berechnet`, so the
    // event carries that type directly (never the Rechnung helper's type).
    if cfg.erp_webhook_url.is_some() {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("billingd", &cfg.tenant),
            mako_events::vpp::SETTLEMENT_BERECHNET,
            &contract.vpp_id,
            serde_json::json!({
                "record_id":          record_id.to_string(),
                "vpp_id":             contract.vpp_id,
                "malo_id":            contract.malo_id,
                "aggregator_mp_id":   contract.aggregator_mp_id,
                "tx_id":              tx_id,
                "sr_id":              location_id,
                "flexibility_kwh":    flexibility_kwh.to_string(),
                "total_netto_eur":    position_netto.to_string(),
                "total_brutto_eur":   total_brutto.to_string(),
                "trigger":            "auto",
                "rechnung":           rechnung_json,
            }),
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            tracing::error!(tx_id, error = %e, "billingd: vpp settlement enqueue failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        if let Err(e) = mark_dispatched_tx(&mut *tx, record_id).await {
            tracing::error!(error = %e, "billingd: mark_dispatched failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    // The §41e settlement is issued against the prosumer behind the MaLo, so the
    // ordinary BG-7 lookup resolves the right party.
    let buyer = vertragd
        .get_vertrag_by_malo(&contract.malo_id)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.rechnungsempfaenger);
    if let Err(e) = crate::einvoice::store(
        &mut *tx,
        record_id,
        &invoice,
        &cfg,
        &contract.malo_id,
        buyer.as_ref(),
    )
    .await
    {
        tracing::warn!(%record_id, error = %e, "billingd: attach en16931 model failed");
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(tx_id, error = %e, "billingd: vpp auto-billing commit failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    tracing::info!(
        tx_id,
        %record_id,
        vpp_id = %contract.vpp_id,
        malo_id = %contract.malo_id,
        flexibility_kwh = %flexibility_kwh,
        total_brutto = %total_brutto,
        "billingd: VPP dispatch auto-billed"
    );

    StatusCode::ACCEPTED.into_response()
}

/// Compute delivered flexibility in kWh from dispatch parameters.
///
/// `flexibility_kwh = max_power_kw × duration_hours`
///
/// Duration is parsed from ISO-8601 UTC timestamps.  Falls back to 15 minutes
/// (the standard BNetzA §14a dispatch window minimum) when `time_until` is
/// absent or parsing fails.
pub(crate) fn compute_dispatch_flexibility_kwh(
    max_power_kw: rust_decimal::Decimal,
    time_from: &str,
    time_until: Option<&str>,
) -> rust_decimal::Decimal {
    use rust_decimal::dec;

    let duration_hours = time_until
        .and_then(|tu| {
            let f = time::OffsetDateTime::parse(
                time_from,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()?;
            let u = time::OffsetDateTime::parse(tu, &time::format_description::well_known::Rfc3339)
                .ok()?;
            let secs = (u - f).whole_seconds();
            if secs > 0 {
                Some(rust_decimal::Decimal::from(secs) / dec!(3600))
            } else {
                None
            }
        })
        .unwrap_or(dec!(0.25)); // 15-minute default

    (max_power_kw * duration_hours).round_kfm(6)
}

/// Extract the calendar date (UTC) from an ISO-8601 timestamp string.
pub(crate) fn parse_dispatch_date(ts: &str) -> Option<time::Date> {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.date())
}
