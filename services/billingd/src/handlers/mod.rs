//! HTTP handlers for `billingd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use energy_billing::RoundMoney;
use mako_service::oidc::Claims;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use time::format_description::well_known::Iso8601;
use uuid::Uuid;

use crate::{
    clients::{EdmdClient, TarifbdClient, VertragdClient},
    config::BillingdConfig,
    pg::{
        fetch_billing_record, insert_billing_record, insert_correction_record,
        insert_sammelrechnung_record, link_to_sammelrechnung, list_billing_records,
        mark_dispatched_tx,
    },
};
use energy_billing::{
    BillingContext, BillingPeriod, BillingPosition, BillingProvider as _, DynamicInterval,
    EegMeterInput, EmobilityMeterInput, GasMeterInput, GridInput, HemsMeterInput, Invoice,
    InvoiceType, MeterInput, MwStProvider, PositionCategory, Product, Quantities, RegulatoryRates,
    ServiceMeterInput, SolarMeterInput, WaermeMeterInput, WasserMeterInput,
    negate_rechnung_json_for_correction,
};

/// Build a VPP settlement through the engine's canonical invoice path.
///
/// The VPP paths hand-assembled BO4E JSON with their own inline VAT — a second
/// VAT implementation whose Steuerkennzeichen was hardcoded `UST_19` even when
/// the contract overrode the rate. Positions plus the engine's tax provider
/// plus `to_rechnung_json` replace all of it: steuerbetraege, traces, and the
/// ABSCHLAGSRECHNUNG rechnungsart come out the same way every other invoice
/// does.
///
/// VPP-specific references (tx-id, SR-ID, dispatch process ids) are appended as
/// document-level ZusatzAttribute on the typed BO before serialisation.
#[allow(clippy::too_many_arguments)]
fn build_vpp_invoice(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: String,
    period_from: time::Date,
    period_to: time::Date,
    mwst_rate: rust_decimal::Decimal,
    positions: Vec<BillingPosition>,
    extra_attrs: Vec<rubo4e::current::ZusatzAttribut>,
) -> anyhow::Result<(Invoice, serde_json::Value)> {
    let ctx = BillingContext {
        malo_id: malo_id.to_owned(),
        lf_mp_id: lf_mp_id.to_owned(),
        rechnungsnummer,
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from < to"),
        invoice_type: InvoiceType::AdvancePayment,
        regulatory_rates: RegulatoryRates {
            mwst_rate,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut all = positions;
    let tax = MwStProvider::new(mwst_rate)
        .bill(&ctx, &Quantities::default(), &all)
        .map_err(|e| anyhow::anyhow!("VPP tax pass failed: {e}"))?;
    all.extend(tax);
    let invoice = Invoice::from_positions(ctx, all, vec![]);
    let mut rechnung = invoice.to_rechnung();
    if !extra_attrs.is_empty() {
        rechnung
            .zusatz_attribute
            .get_or_insert_with(Vec::new)
            .extend(extra_attrs);
    }
    let json = serde_json::to_value(&rechnung)?;
    Ok((invoice, json))
}

/// A document-level BO4E ZusatzAttribut.
fn zusatz_attribut(name: &str, wert: serde_json::Value) -> rubo4e::current::ZusatzAttribut {
    rubo4e::current::ZusatzAttribut {
        name: Some(name.to_owned()),
        wert: Some(wert),
        ..Default::default()
    }
}

/// Assemble a consolidated document (Sammelrechnung, GGV-Sammel) from per-MaLo
/// engine invoices.
///
/// The per-MaLo runs stay stored as calculation records; the consolidated
/// document is the invoice the counterparty receives, so its VAT is computed
/// **once** over the combined base per rate — not summed from the per-MaLo
/// roundings, which can drift from a single consistent tax document by cents.
/// Concretely: the sub-invoices' Tax positions are stripped, the engine's tax
/// provider re-runs over the concatenated base (grouping by each position's
/// effective rate), and `to_rechnung_json` renders totals, steuerbetraege and
/// rechnungsdatum the same way every other invoice gets them.
///
/// Each rendered position carries the `marktlokationsId` it came from; the
/// document-level tax positions carry none, because they belong to the whole
/// document.
#[allow(clippy::too_many_arguments)]
fn build_aggregate_invoice(
    subject_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: String,
    period_from: time::Date,
    period_to: time::Date,
    rates: RegulatoryRates,
    parts: Vec<(String, Invoice)>,
    extra_attrs: Vec<rubo4e::current::ZusatzAttribut>,
) -> anyhow::Result<(Invoice, serde_json::Value)> {
    let ctx = BillingContext {
        malo_id: subject_id.to_owned(),
        lf_mp_id: lf_mp_id.to_owned(),
        rechnungsnummer,
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from < to"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let mut base: Vec<BillingPosition> = Vec::new();
    let mut warnings = Vec::new();
    // (malo_id, number of positions contributed) — for the JSON annotation.
    let mut slices: Vec<(String, usize)> = Vec::with_capacity(parts.len());
    for (malo_id, invoice) in parts {
        let non_tax: Vec<BillingPosition> = invoice
            .positions
            .into_iter()
            .filter(|p| p.category != PositionCategory::Tax)
            .collect();
        slices.push((malo_id, non_tax.len()));
        base.extend(non_tax);
        warnings.extend(invoice.warnings);
    }

    let tax = MwStProvider::new(ctx.regulatory_rates.mwst_rate)
        .bill(&ctx, &Quantities::default(), &base)
        .map_err(|e| anyhow::anyhow!("aggregate tax pass failed: {e}"))?;
    base.extend(tax);
    let aggregate = Invoice::from_positions(ctx, base, warnings);

    let mut rechnung = aggregate.to_rechnung();
    // BO4E Rechnungsposition has no per-position Marktlokation field, so the
    // provenance annotation rides as an extension key (`_additional`) on each
    // typed position — the same flat `marktlokationsId` key consumers read.
    if let Some(pos) = rechnung.rechnungspositionen.as_mut() {
        let mut idx = 0usize;
        for (malo_id, count) in &slices {
            for p in pos.iter_mut().skip(idx).take(*count) {
                p._additional
                    .try_insert("marktlokationsId".to_owned(), serde_json::json!(malo_id));
            }
            idx += count;
        }
    }
    if !extra_attrs.is_empty() {
        rechnung
            .zusatz_attribute
            .get_or_insert_with(Vec::new)
            .extend(extra_attrs);
    }
    let json = serde_json::to_value(&rechnung)?;
    Ok((aggregate, json))
}

/// Structured JSON error body for a typed engine error.
///
/// Carries the stable machine-readable code, the display message, and — for a
/// blocked validation — every warning the engine collected, so a caller can
/// act on `MODUL2_AND_FLAT_NNE` without parsing prose.
fn engine_error_body(context: &str, e: &energy_billing::EngineError) -> String {
    serde_json::json!({
        "error": {
            "code": e.code(),
            "context": context,
            "message": e.to_string(),
            "warnings": e.blocking_warnings(),
        }
    })
    .to_string()
}

mod calculate;
mod correction;
mod dispatch;
mod enrichment;
mod ggv;
mod records;
mod sammelrechnung;
mod vpp;

// Path-preserving re-exports: everything that used to live directly in
// `handlers.rs` stays reachable under `crate::handlers::…`.
pub use calculate::*;
pub use correction::*;
pub(crate) use dispatch::*;
pub(crate) use enrichment::*;
pub use ggv::*;
pub use records::*;
pub use sammelrechnung::*;
pub use vpp::*;

// ── Helpers ────────────────────────────────────────────────────────────────────

fn parse_period(from: &str, to: &str) -> Result<(time::Date, time::Date), String> {
    let pf =
        time::Date::parse(from, &Iso8601::DEFAULT).map_err(|_| "invalid period_from".to_owned())?;
    let pt =
        time::Date::parse(to, &Iso8601::DEFAULT).map_err(|_| "invalid period_to".to_owned())?;
    if pf >= pt {
        return Err("period_from must be before period_to".to_owned());
    }
    Ok((pf, pt))
}

pub(crate) async fn resolve_tariff(
    req: &CalculateRequest,
    tarifbd: &TarifbdClient,
    malo_id: &str,
) -> Result<Product, (StatusCode, String)> {
    if let Some(t) = req.tariff.clone() {
        return Ok(t);
    }
    match tarifbd.get_customer_product(malo_id, &req.lf_mp_id).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("No active product for MaLo {malo_id} / LF {}", req.lf_mp_id),
        )),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("tarifbd: {e}"))),
    }
}

/// Build the `de.billing.rechnung.erstellt` CloudEvent for a persisted record.
///
/// Pure constructor — no I/O. The caller enqueues the returned event into the
/// transactional outbox **inside the same transaction as the representing
/// business write** (`insert_billing_record` / `insert_correction_record` /
/// `insert_sammelrechnung_record`), so the event and the row commit atomically;
/// the `OutboxWorker` then delivers it (signed, retried, dead-lettered). The CE
/// `id` is a fresh UUID — enqueue is idempotent on it, so a retried request
/// cannot double-enqueue within its transaction.
pub(crate) fn rechnung_erstellt_ce(
    record_id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    rechnung: &serde_json::Value,
    is_correction: bool,
) -> mako_service::CloudEvent {
    mako_service::CloudEvent::new(
        mako_service::source("billingd", lf_mp_id),
        mako_events::billing::RECHNUNG_ERSTELLT,
        malo_id,
        serde_json::json!({
            "record_id": record_id.to_string(),
            "malo_id": malo_id,
            "lf_mp_id": lf_mp_id,
            "is_correction": is_correction,
            "rechnung": rechnung
        }),
    )
    .with_id(Uuid::new_v4().to_string())
}
