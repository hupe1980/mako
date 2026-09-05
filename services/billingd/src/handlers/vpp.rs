//! § 41e EnWG VPP dispatch settlement — the manual endpoint and the
//! `de.vpp.dispatch.confirmed` auto-settlement webhook.
//!
//! Both produce a **Gutschrift**: the flexibility provider delivered the
//! energy, the aggregator owes the remuneration, and the aggregator writes the
//! document (§ 14 Abs. 2 Satz 2 UStG Gutschriftverfahren).

use super::*;

// ── § 41e EnWG dispatch settlement (Art. 17 RL (EU) 2019/944) ─────────────────

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
    /// The dispatch transaction id, when the caller has one.
    ///
    /// Supplying it makes the manual endpoint share `vpp_dispatch_ledger` with
    /// the auto-settlement webhook: a dispatch already settled by either path is
    /// skipped instead of paid twice. Without it the two writers were blind to
    /// each other, and a period back-filled by hand after the webhook had
    /// auto-billed part of it remunerated the same flexibility twice with
    /// nothing in the store to show it.
    #[serde(default)]
    pub tx_id: Option<String>,
}

/// Request body for `POST /api/v1/billing/vpp/{vpp_id}`.
///
/// `vpp_id` is the operator-assigned virtual power plant identifier
/// (typically the SR-ID of the `SteuerbareRessource` portfolio in `marktd`).
#[derive(Debug, serde::Deserialize)]
pub struct VppBillingRequest {
    /// Aggregator MP-ID — the party issuing the Gutschrift.
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
    /// Override the settlement's number. Absent — the normal case — takes the
    /// next number of the tenant's `VG` (Gutschrift) series.
    #[serde(default)]
    pub rechnungsnummer: Option<String>,
    /// MwSt rate override (default from billingd config, typically 0.19).
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/billing/vpp/{vpp_id}` — settle a period of VPP dispatches.
///
/// One credit position per confirmed dispatch event, one Gutschrift for the
/// period. The webhook below settles per dispatch instead; this endpoint is the
/// manual and back-fill path.
///
/// ## Calculation
///
/// ```text
/// DispatchCredit_eur = flexibility_kwh × capacity_price_eur_per_kwh
/// Total_netto        = −Σ DispatchCredit_eur      (the aggregator owes it)
/// MwSt               = Total_netto × mwst_rate
/// Total_brutto       = Total_netto + MwSt
/// ```
///
/// ## Direction
///
/// The document is a **Gutschrift** (§ 14 Abs. 2 Satz 2 UStG Gutschriftverfahren):
/// the flexibility provider delivered the energy, the aggregator owes the
/// remuneration, and the aggregator writes the document. Totals are therefore
/// negative from the aggregator's side — the same shape as an EEG feed-in
/// settlement.
///
/// ## CloudEvent emitted
///
/// `de.vpp.settlement.berechnet` (type) — consumed by ERP/DSO settlement systems.
///
/// ## Regulatory basis
///
/// § 41e EnWG (Verträge zwischen Aggregatoren und Betreibern einer Erzeugungsanlage
/// oder Letztverbrauchern), transposing Art. 17 RL (EU) 2019/944 (demand response
/// through aggregation). The remuneration itself is contractual; §41e governs the
/// contract's form and the data the provider may demand.
pub async fn post_vpp_billing(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(vpp_id): Path<String>,
    Json(req): Json<VppBillingRequest>,
) -> BillingResult<impl IntoResponse> {
    use rust_decimal::Decimal;

    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "settle-flexibility", &cfg.tenant)?;

    if req.dispatch_events.is_empty() {
        return Err(BillingError::bad_request(
            "NO_DISPATCH_EVENTS",
            "VPP billing requires at least one dispatch event",
        ));
    }

    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;

    let mwst_rate = req
        .mwst_rate_override
        .unwrap_or_else(|| cfg.regulatory_rates().mwst_rate);

    // Dispatches already settled — by an earlier run of this endpoint or by the
    // auto-settlement webhook — are excluded before anything is priced.
    let stated_tx_ids: Vec<String> = req
        .dispatch_events
        .iter()
        .filter_map(|ev| ev.tx_id.clone())
        .collect();
    let already_settled = crate::pg::settled_vpp_dispatches(&pool, &cfg.tenant, &stated_tx_ids)
        .await
        .map_err(BillingError::Internal)?;

    // ── Positions from dispatch events, through the engine ────────────────────
    // Every event becomes a BillingPosition; VAT, steuerbetraege and traces come
    // from the same machinery as every other invoice.
    let mut positions: Vec<BillingPosition> = Vec::with_capacity(req.dispatch_events.len());
    let mut settled_now: Vec<String> = Vec::new();
    let mut total_flex_kwh = Decimal::ZERO;
    // Load-increase (negative flexibility) dispatches are not remunerated on
    // this capacity-price model, but dropping them without trace makes a
    // settlement silently smaller than the dispatch log. Count and report them.
    let mut skipped_non_positive = 0usize;
    let mut skipped_duplicate = 0usize;
    for ev in &req.dispatch_events {
        if ev
            .tx_id
            .as_ref()
            .is_some_and(|tx| already_settled.contains(tx))
        {
            skipped_duplicate += 1;
            continue;
        }
        if ev.flexibility_kwh <= Decimal::ZERO {
            skipped_non_positive += 1;
            continue;
        }
        total_flex_kwh += ev.flexibility_kwh;
        if let Some(tx) = ev.tx_id.clone() {
            settled_now.push(tx);
        }
        positions.push(vpp_dispatch_position(
            format!("VPP Dispatch {} bis {}", ev.start_utc, ev.end_utc),
            ev.flexibility_kwh,
            req.capacity_price_eur_per_kwh,
        ));
    }

    if positions.is_empty() {
        return Err(BillingError::unprocessable_with(
            "NOTHING_TO_SETTLE",
            "no billable dispatch remains: every event was already settled or carries \
             zero/negative flexibility",
            serde_json::json!({
                "skipped_already_settled": skipped_duplicate,
                "skipped_non_positive": skipped_non_positive,
            }),
        ));
    }
    if skipped_non_positive > 0 || skipped_duplicate > 0 {
        tracing::warn!(
            %vpp_id,
            malo_id = %req.malo_id,
            skipped_non_positive,
            skipped_duplicate,
            billed_events = positions.len(),
            "billingd VPP: events excluded from this settlement"
        );
    }

    let billed_events = positions.len();
    let rechnungsnummer = next_rechnungsnummer(
        &pool,
        &cfg.tenant,
        series::CREDIT,
        req.rechnungsnummer.as_deref(),
        period_from,
    )
    .await?;

    let attrs = vec![
        zusatz_attribut("mako:vpp_id", serde_json::json!(vpp_id)),
        zusatz_attribut(
            "mako:total_flexibility_kwh",
            serde_json::json!(total_flex_kwh.to_string()),
        ),
        zusatz_attribut(
            "mako:dispatch_event_count",
            serde_json::json!(billed_events.to_string()),
        ),
        zusatz_attribut(
            "mako:dispatch_process_ids",
            serde_json::json!(
                req.dispatch_events
                    .iter()
                    .filter_map(|ev| ev.process_id.as_deref())
                    .collect::<Vec<_>>()
            ),
        ),
    ];
    let (invoice, rechnung_json) = build_vpp_settlement(
        &req.malo_id,
        &req.lf_mp_id,
        rechnungsnummer.clone(),
        period_from,
        period_to,
        mwst_rate,
        positions,
        attrs,
    )
    .map_err(BillingError::Internal)?;
    let total_netto = invoice.netto_eur;
    let total_brutto = invoice.brutto_eur;

    // The §41e settlement is issued against the prosumer behind the MaLo, so the
    // ordinary BG-7 lookup resolves the right party. This path builds the
    // document from dispatch events rather than through `dispatch_invoice`, so
    // it looks the buyer up itself.
    let buyer = vpp_buyer(&deps, &req.malo_id).await;

    // VPP settlement row, its ledger claims and its `de.vpp.settlement.berechnet`
    // outbox event commit atomically, so a settled dispatch can never be
    // persisted without its event or without its idempotency claim.
    let mut tx = pool.begin().await?;
    let record_id = insert_billing_record(
        &mut *tx,
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &req.malo_id,
            lf_mp_id: &req.lf_mp_id,
            product_code: &format!("VPP_{vpp_id}"),
            category: "VPP",
            rechnungsnummer: &rechnungsnummer,
            period_from,
            period_to,
            rechnung_json: &rechnung_json,
            total_netto_eur: total_netto,
            total_brutto_eur: total_brutto,
        },
    )
    .await
    .map_err(crate::error::BillingError::from)?;
    for tx_id in &settled_now {
        crate::pg::record_vpp_dispatch(&mut *tx, tx_id, &cfg.tenant, Some(record_id))
            .await
            .map_err(BillingError::Internal)?;
    }
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
            "dispatch_count": billed_events,
            "trigger": "manual",
            "rechnung": rechnung_json,
        }),
    );
    issue_record(&mut tx, cfg, record_id, &ce).await?;
    crate::einvoice::store(
        &mut *tx,
        record_id,
        &invoice,
        cfg,
        &req.malo_id,
        buyer.as_ref(),
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "record_id": record_id,
            "vpp_id": vpp_id,
            "malo_id": req.malo_id,
            "rechnungsnummer": rechnungsnummer,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "dispatch_count": req.dispatch_events.len(),
            "billed_dispatch_count": billed_events,
            // Stated so the caller can reconcile the settlement against its
            // dispatch log rather than wondering where the difference went.
            "skipped_already_settled": skipped_duplicate,
            "skipped_non_positive_events": skipped_non_positive,
            "total_flexibility_kwh": total_flex_kwh.to_string(),
            "total_netto_eur": total_netto.to_string(),
            "total_brutto_eur": total_brutto.to_string(),
            "mwst_eur": invoice.mwst_eur.to_string(),
            "rechnung": rechnung_json,
        })),
    ))
}

// ── Auto-settlement webhook ───────────────────────────────────────────────────

/// Which legal instrument a confirmed Steuerungsauftrag belongs to.
///
/// § 14a EnWG netzorientierte Steuerung and § 41e flexibility dispatch reach the
/// MSB as the *same* WiM Steuerungsauftrag, and one
/// SteuerbareRessource can be subject to both. Only the sender separates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// The contracted Aggregator called the flexibility it pays for
    /// (§ 41e EnWG, Art. 17 RL (EU) 2019/944) — settle it.
    Sect41eDelivery,
    /// Somebody else ordered the Steuerung — the Netzbetreiber dimming a
    /// controllable load under § 14a EnWG. Its compensation is a reduced
    /// Netzentgelt (Modul 1/2/3), not a Gutschrift from the aggregator, so
    /// settling it here would pay for flexibility nobody dispatched and
    /// compensate one curtailment twice.
    Sect14aGridIntervention,
}

/// Decide [`Disposition`] from the dispatch's sender and the contracted
/// aggregator.
///
/// An **absent** `sender_mp_id` is a grid intervention, not a delivery: a
/// payment needs positive evidence that the party being paid is the party that
/// dispatched, and a missing field is not that.
pub(crate) fn disposition(sender_mp_id: &str, aggregator_mp_id: &str) -> Disposition {
    if !sender_mp_id.is_empty() && sender_mp_id == aggregator_mp_id {
        Disposition::Sect41eDelivery
    } else {
        Disposition::Sect14aGridIntervention
    }
}

/// `POST /api/v1/webhooks/vpp-dispatch`
///
/// **Dispatch-confirmed auto-settlement.**
///
/// Receives `de.vpp.dispatch.confirmed` CloudEvents emitted by `makod` when the
/// MSB sends a positive `EndantwortPositiv` for a WiM Steuerungsauftrag
/// and writes one Gutschrift per dispatch against the
/// §41e Aggregatorvertrag in force **on the day the dispatch executed** —
/// `vertragd` owns that contract; billingd keeps no copy.
///
/// ## Idempotency
///
/// Each `tx_id` is recorded in `vpp_dispatch_ledger` **inside the settlement
/// transaction**, so a dispatch is settled exactly once and a redelivery
/// returns `202 Accepted` without re-settling. That ledger is also why
/// per-dispatch records are exempt from `br_unique_original`: a portfolio is
/// dispatched several times a day, and a one-row-per-period index cannot
/// express that.
///
/// ## HMAC verification
///
/// When `[inbound_webhook_secret]` is configured in `billingd.toml`, the
/// `webhook-signature` header is verified, together with the `webhook-timestamp`
/// freshness that stops a captured request replaying.  Requests with
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
///
/// `sender_mp_id` must be the contracted `aggregator_mp_id` for the dispatch to
/// settle. § 14a EnWG and § 41e ride the same Steuerungsauftrag, so the sender
/// is what separates a Netzbetreiber's grid intervention from the aggregator's
/// flexibility call — see step 5b.
///
/// The event carries **no price**, deliberately: what a dispatch is worth is the
/// § 41e Aggregatorvertrag's `capacity_price_eur_per_kwh` in `vertragd`, and a
/// counterparty does not get to state what it is owed.
pub async fn post_vpp_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let (cfg, vertragd) = (&deps.cfg, &deps.vertragd);
    // ── 1. Standard Webhooks verification ─────────────────────────────────────
    //
    // A dispatch settles money, so a replay is a second Gutschrift. The shared
    // verifier refuses a stale `webhook-timestamp`; a bare signature compare
    // cannot.
    if let Err(err) = mako_service::webhook::verify_request(
        cfg.inbound_webhook_secret.as_deref().map(str::as_bytes),
        &headers,
        &body,
    ) {
        tracing::warn!(%err, "billingd: vpp-dispatch webhook refused");
        return StatusCode::from(err).into_response();
    }
    if cfg.inbound_webhook_secret.is_none() {
        tracing::warn!(
            "billingd: inbound_webhook_secret not set — accepting vpp-dispatch webhooks \
             unverified (dev mode)"
        );
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

    // The idempotency key. Falling back to a literal `"unknown"` would be worse
    // than having none: the first such event claims that key, and every later
    // one — any portfolio, any day — is then seen as its duplicate and silently
    // dropped. An event without an identifiable transaction cannot be settled
    // exactly once, so it is refused.
    let Some(tx_id) = data
        .get("tx_id")
        .and_then(|v| v.as_str())
        .or_else(|| event.get("id").and_then(|v| v.as_str()))
        .map(str::to_owned)
    else {
        tracing::warn!("billingd: vpp-dispatch webhook — event carries neither data.tx_id nor id");
        return (
            StatusCode::BAD_REQUEST,
            "the event must carry data.tx_id (or a CloudEvent id) as its idempotency key",
        )
            .into_response();
    };

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
    // Who ordered the Steuerung. §14a and §41e ride the same wire.
    let sender_mp_id = data
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

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
    let dispatch_date =
        parse_dispatch_date(&execution_time_from).unwrap_or_else(mako_fristen::heute);
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

    // ── 5b. Only the aggregator's own dispatch is a § 41e delivery ────────────
    //
    // § 14a EnWG netzorientierte Steuerung and § 41e flexibility dispatch reach
    // the MSB as the *same* WiM Steuerungsauftrag (ORDERS/ORDRSP),
    // and a SteuerbareRessource can be subject to both. What separates them is
    // who sent it: the Netzbetreiber dimming a controllable load under § 14a, or
    // the Aggregator calling the flexibility it contracted under § 41e /
    // Art. 17 RL (EU) 2019/944.
    //
    // They are compensated on different legal bases and by different parties. A
    // § 14a intervention is answered by a reduced Netzentgelt (Modul 1/2/3),
    // which `billingd` prices on the network-charge side; paying the
    // `capacity_price_eur_per_kwh` for it as well would credit the aggregator
    // for flexibility it never dispatched and compensate the customer twice for
    // one curtailment.
    //
    // The dispatch is still recorded, so the idempotency key is consumed and the
    // § 14a audit trail keeps it.
    if disposition(&sender_mp_id, &contract.aggregator_mp_id)
        == Disposition::Sect14aGridIntervention
    {
        tracing::info!(
            tx_id,
            sr_id = %location_id,
            sender_mp_id = %sender_mp_id,
            aggregator_mp_id = %contract.aggregator_mp_id,
            "billingd: vpp-dispatch — sender is not the contracted Aggregator; \
             recording as a § 14a Steuerung, not settling it under § 41e"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

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

    // ── 8. Build and run the VPP settlement ───────────────────────────────────
    // Settlement period = the calendar day the dispatch executed — the same day
    // the contract above was selected by. Several dispatches legitimately settle
    // within one day, so these records are exempt from `br_unique_original`;
    // `vpp_dispatch_ledger` is their idempotency guard and the per-tx
    // Rechnungsnummer keeps § 14 Abs. 4 Nr. 4 UStG satisfied.
    let period_from = dispatch_date;
    let period_to = period_from;

    let mwst_rate = contract
        .mwst_rate_override
        .unwrap_or_else(|| cfg.regulatory_rates().mwst_rate);

    // From the tenant's `VG` (Gutschrift) series, like the manual path. The old
    // `VPP-{vpp}-{date}-{tx-prefix}` string was einmalig only as long as no two
    // transaction ids of a day shared their first eight characters.
    let rechnungsnummer = match crate::pg::allocate_rechnungsnummer(
        &pool,
        &cfg.tenant,
        crate::handlers::series::CREDIT,
        period_from.year(),
    )
    .await
    {
        Ok(nr) => nr,
        Err(e) => {
            tracing::error!(tx_id, error = %e, "billingd: could not allocate a Rechnungsnummer");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // One credit position through the engine's canonical path — VAT,
    // steuerbetraege and the trace come from the same machinery as every other
    // document.
    let pos = vpp_dispatch_position(
        format!(
            "VPP Dispatch {} bis {} (SR: {})",
            execution_time_from,
            execution_time_until.as_deref().unwrap_or("open"),
            location_id
        ),
        flexibility_kwh,
        contract.capacity_price_eur_per_kwh,
    );

    let attrs = vec![
        zusatz_attribut("mako:vpp_id", serde_json::json!(contract.vpp_id.clone())),
        zusatz_attribut("mako:tx_id", serde_json::json!(tx_id.clone())),
        zusatz_attribut("mako:sr_id", serde_json::json!(location_id.clone())),
        zusatz_attribut(
            "mako:flexibility_kwh",
            serde_json::json!(flexibility_kwh.to_string()),
        ),
    ];
    let (invoice, rechnung_json) = match build_vpp_settlement(
        &contract.malo_id,
        &contract.aggregator_mp_id,
        rechnungsnummer.clone(),
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
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &contract.malo_id,
            lf_mp_id: &contract.aggregator_mp_id,
            product_code: &format!("VPP_{}", contract.vpp_id),
            category: "VPP",
            rechnungsnummer: &rechnungsnummer,
            period_from,
            period_to,
            rechnung_json: &rechnung_json,
            total_netto_eur: position_netto,
            total_brutto_eur: total_brutto,
        },
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
    {
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
        if let Err(e) = issue_record(&mut tx, cfg, record_id, &ce).await {
            tracing::error!(tx_id, error = %e, "billingd: vpp settlement issuance failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    // The §41e settlement is issued against the prosumer behind the MaLo, so the
    // ordinary BG-7 lookup resolves the right party.
    let buyer = vpp_buyer(&deps, &contract.malo_id).await;
    if let Err(e) = crate::einvoice::store(
        &mut *tx,
        record_id,
        &invoice,
        cfg,
        &contract.malo_id,
        buyer.as_ref(),
    )
    .await
    {
        tracing::error!(%record_id, error = ?e, "billingd: attach en16931 model failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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

/// The BG-7 buyer for a § 41e settlement, best-effort.
///
/// The VPP paths assemble their document from dispatch events instead of running
/// a period through `dispatch_invoice`, so they have no priced invoice to carry
/// the buyer along with.
async fn vpp_buyer(
    deps: &BillingDeps,
    malo_id: &str,
) -> Option<crate::clients::Rechnungsempfaenger> {
    deps.vertragd
        .get_vertrag_by_malo(malo_id)
        .await
        .inspect_err(|e| {
            tracing::warn!(%malo_id, error = %e, "billingd VPP: BG-7 buyer lookup failed");
        })
        .ok()
        .flatten()
        .and_then(|v| v.rechnungsempfaenger)
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

/// The German calendar date an ISO-8601 dispatch timestamp falls on.
///
/// The date selects the aggregator contract version in force and groups the
/// dispatch for § 41e settlement, so it is the day the German market counts —
/// a dispatch at 00:30 Berlin belongs to that day, not to the one the UTC
/// clock is still on.
pub(crate) fn parse_dispatch_date(ts: &str) -> Option<time::Date> {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(mako_fristen::berlin_date)
}

#[cfg(test)]
mod disposition_tests {
    use super::{Disposition, disposition};

    const AGGREGATOR: &str = "9900123456789";
    const NETZBETREIBER: &str = "9900000000001";

    /// The aggregator calling its own contracted flexibility is the § 41e case.
    #[test]
    fn the_contracted_aggregator_delivers_under_sect41e() {
        assert_eq!(
            disposition(AGGREGATOR, AGGREGATOR),
            Disposition::Sect41eDelivery
        );
    }

    /// § 14a EnWG and § 41e ride the same Steuerungsauftrag, and one
    /// SteuerbareRessource can carry both. Settling the Netzbetreiber's grid
    /// intervention here would credit the aggregator for flexibility it never
    /// dispatched, on top of the reduced Netzentgelt the customer already gets
    /// for it.
    #[test]
    fn the_netzbetreibers_sect14a_steuerung_is_not_a_delivery() {
        assert_eq!(
            disposition(NETZBETREIBER, AGGREGATOR),
            Disposition::Sect14aGridIntervention
        );
    }

    /// A payment needs positive evidence of who dispatched. An event that names
    /// no sender is not that evidence, whatever the contract says.
    #[test]
    fn an_unnamed_sender_never_settles() {
        assert_eq!(
            disposition("", AGGREGATOR),
            Disposition::Sect14aGridIntervention
        );
        // Not even when the contract itself names no aggregator.
        assert_eq!(disposition("", ""), Disposition::Sect14aGridIntervention);
    }
}
