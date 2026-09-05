//! HTTP handlers for `billingd`.

use crate::error::{BillingError, BillingResult};
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

pub(crate) use mako_service::cedar::CedarEnforcer;

use crate::{
    clients::{BillingDeps, EdmdClient, ProductdClient},
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

/// Build a §41e EnWG VPP settlement through the engine's canonical path.
///
/// A dispatch settlement is a **Gutschrift**, not an invoice. The aggregator
/// owes the flexibility provider for energy the provider delivered, so the
/// document is issued by the aggregator *about* its own liability —
/// § 14 Abs. 2 Satz 2 UStG Gutschriftverfahren, the same self-billing shape
/// `eeg-billing` already uses for feed-in remuneration.
///
/// A **debit** invoice of type `AdvancePayment` would have the aggregator bill
/// the prosumer for flexibility the prosumer supplied, labelled an
/// Abschlagsrechnung — wrong in both the sign and the Rechnungsart, and taken
/// at face value by every downstream consumer from accountingd's ledger to the
/// customer's own books.
///
/// VPP-specific references (tx-id, SR-ID, dispatch process ids) are appended as
/// document-level ZusatzAttribute on the typed BO before serialisation.
#[allow(clippy::too_many_arguments)]
fn build_vpp_settlement(
    malo_id: &str,
    aggregator_mp_id: &str,
    rechnungsnummer: String,
    period_from: time::Date,
    period_to: time::Date,
    mwst_rate: rust_decimal::Decimal,
    positions: Vec<BillingPosition>,
    extra_attrs: Vec<rubo4e::current::ZusatzAttribut>,
) -> anyhow::Result<(Invoice, serde_json::Value)> {
    let ctx = BillingContext {
        malo_id: malo_id.to_owned(),
        lf_mp_id: aggregator_mp_id.to_owned(),
        rechnungsnummer,
        period: BillingPeriod::new(period_from, period_to)?,
        invoice_type: InvoiceType::CreditNote,
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
    // The outbound gate. `energy-billing`'s own emissions are test-guarded over
    // the shapes the engine produces, but a VPP settlement is assembled here
    // from runtime values — the dispatch credits, the VAT rate the aggregator
    // contract carries, the tax pass over both — and no fixture can cover the
    // arithmetic this checks for arbitrary amounts. The document is persisted
    // and published as `de.billing.rechnung.erstellt`, off which `accountingd`
    // books the CREDIT, so a Gutschrift whose totals disagree would be booked.
    mako_markt::bo4e::ensure_conformant(&rechnung)
        .map_err(|e| anyhow::anyhow!("the VPP settlement is not a valid BO4E document: {e}"))?;
    let json = serde_json::to_value(&rechnung)?;
    Ok((invoice, json))
}

/// One dispatch, as the credit position that settles it.
///
/// `credit` and not `debit`: the flexibility flowed from the provider to the
/// aggregator, so the money flows back the other way.
fn vpp_dispatch_position(
    description: String,
    flexibility_kwh: rust_decimal::Decimal,
    capacity_price_eur_per_kwh: rust_decimal::Decimal,
) -> BillingPosition {
    const BASIS: &str = "§ 41e EnWG, Art. 17 RL (EU) 2019/944, VPP-Vertrag";
    let mut pos = BillingPosition::credit(
        description,
        flexibility_kwh,
        "kWh",
        capacity_price_eur_per_kwh,
        PositionCategory::Credit,
    )
    .with_legal_basis(BASIS)
    .with_tag("vpp_dispatch");
    pos.trace = energy_billing::PositionTrace::commodity(
        flexibility_kwh,
        "kWh",
        capacity_price_eur_per_kwh,
        BASIS,
    );
    pos
}

/// Authorize `action` for the caller against the service tenant.
///
/// Authentication established *who* is calling; this decides what they may do.
/// Every business route runs one of these before it touches the database:
/// without it, any authenticated caller could reverse an issued invoice or
/// release one the risk gate is holding.
///
/// # Errors
///
/// `403` with the Cedar denial reason.
pub(crate) fn authorize(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &'static str,
    tenant: &str,
) -> BillingResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| {
            tracing::warn!(action, sub = %claims.sub(), "billingd: authorization denied");
            BillingError::Forbidden {
                code: "FORBIDDEN",
                message: format!("{action}: {e}"),
            }
        })
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
    let mut ctx = BillingContext {
        malo_id: subject_id.to_owned(),
        lf_mp_id: lf_mp_id.to_owned(),
        rechnungsnummer,
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from <= to"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let mut base: Vec<BillingPosition> = Vec::new();
    let mut warnings = Vec::new();
    // (malo_id, number of positions contributed) — for the JSON annotation.
    let mut slices: Vec<(String, usize)> = Vec::with_capacity(parts.len());
    for (malo_id, invoice) in parts {
        // The bundle settles what the parts settle. Each part's Abschlag
        // deductions travel into `base` below, and the advance they deduct has
        // to travel with them: the document's `abschlag_total_eur` comes from
        // its context, so a bundle whose context named no advances would state
        // the whole gross as due beside the deductions on its own page.
        ctx.abschlage
            .extend(invoice.context.abschlage.iter().cloned());
        let non_tax: Vec<BillingPosition> = invoice
            .positions
            .into_iter()
            .filter(|p| p.category != PositionCategory::Tax)
            .collect();
        // Counted with the engine's own predicate, not a local filter: the
        // annotation below indexes the *emitted* `rechnungspositionen`, and
        // those are net supply lines only. Counting a position `to_rechnung`
        // will not emit — an Abschlag deduction on a sub-invoice — shifts every
        // later slice and annotates positions with the wrong MaLo.
        slices.push((
            malo_id,
            non_tax.iter().filter(|p| p.is_rechnungsposition()).count(),
        ));
        base.extend(non_tax);
        warnings.extend(invoice.warnings);
    }

    let tax = MwStProvider::new(ctx.regulatory_rates.mwst_rate)
        .bill(&ctx, &Quantities::default(), &base)
        .map_err(|e| anyhow::anyhow!("aggregate tax pass failed: {e}"))?;
    base.extend(tax);
    let aggregate = Invoice::from_positions(ctx, base, warnings);

    let mut rechnung = aggregate.to_rechnung();
    // BO4E `Rechnungsposition` has no per-position Marktlokation field, so the
    // provenance annotation rides as a `mako:`-namespaced `zusatzAttribut` —
    // which *is* a BO4E field — rather than as a bare extension key.
    // `marktlokationsId` is a real BO4E field name elsewhere in the schema, so
    // an unprefixed copy of it on a position would be indistinguishable from
    // one BO4E might define with different semantics.
    if let Some(pos) = rechnung.rechnungspositionen.as_mut() {
        let mut idx = 0usize;
        for (malo_id, count) in &slices {
            for p in pos.iter_mut().skip(idx).take(*count) {
                p.zusatz_attribute.get_or_insert_with(Vec::new).push(
                    rubo4e::current::ZusatzAttribut {
                        name: Some("mako:malo_id".to_owned()),
                        wert: Some(serde_json::json!(malo_id)),
                        ..Default::default()
                    },
                );
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
    // The outbound gate. `energy-billing`'s own emissions are test-guarded, but
    // this document is assembled here at runtime — many invoices' positions
    // concatenated, re-taxed as one, then annotated — and the result is a shape
    // no engine test covers. mako refuses a received document that breaks a
    // BO4E-stated rule, so it must not send one.
    mako_markt::bo4e::ensure_conformant(&rechnung)
        .map_err(|e| anyhow::anyhow!("the aggregate invoice is not a valid BO4E document: {e}"))?;
    let json = serde_json::to_value(&rechnung)?;
    Ok((aggregate, json))
}

/// The identity a set of documents produced together shares.
///
/// A `billingRunId` ZusatzAttribut is only worth carrying if it groups
/// something, so it is the identity of the *run*: one id shared by every
/// invoice of a §40b sweep, of a Sammelrechnung, or of a GGV batch. A fresh
/// UUID per invoice would be a second record id that is not even the record's
/// id, and the ERP reconciliation it exists for — "did every invoice from run X
/// arrive?" — could not be performed at all. A single on-demand
/// `/calculate` belongs to no run and carries none, which is the honest answer
/// rather than a unique value pretending to be a group.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunId<'a>(pub Option<&'a str>);

impl RunId<'_> {
    /// No run — a single on-demand calculation.
    pub(crate) const NONE: Self = Self(None);
}

mod calculate;
mod correction;
mod dispatch;
mod enrichment;
mod ggv;
mod records;
mod sammelrechnung;
mod vpp;

// Path-preserving re-exports: every handler is reachable under
// `crate::handlers::…` regardless of which submodule defines it.
pub use calculate::*;
pub use correction::*;
pub(crate) use dispatch::*;
pub(crate) use enrichment::*;
pub use ggv::*;
pub use records::*;
pub use sammelrechnung::*;
pub use vpp::*;

// ── Helpers ────────────────────────────────────────────────────────────────────

/// One ISO-8601 calendar date from the wire.
///
/// # Errors
///
/// `400` naming the field, so a caller sees which date it got wrong.
pub(crate) fn parse_date(field: &str, value: &str) -> BillingResult<time::Date> {
    time::Date::parse(value, &Iso8601::DEFAULT).map_err(|_| {
        BillingError::bad_request(
            "INVALID_DATE",
            format!("{field} is not an ISO-8601 date (YYYY-MM-DD): {value:?}"),
        )
    })
}

/// A billing period from the wire — both bounds **inclusive**.
///
/// A one-day period is valid and common: a move-in and move-out on the same
/// day, a § 41e settlement of one day's dispatches. A `from < to` bound would
/// refuse all of them — and, since the Tarifwechsel handler parses its
/// `switch_date` by passing it as *both* bounds, would make that endpoint
/// answer `400` for every request. `BillingPeriod::new` accepts `from == to`.
///
/// # Errors
///
/// `400` when a bound is unparsable or the period runs backwards.
pub(crate) fn parse_period(from: &str, to: &str) -> BillingResult<(time::Date, time::Date)> {
    let pf = parse_date("period_from", from)?;
    let pt = parse_date("period_to", to)?;
    if pf > pt {
        return Err(BillingError::bad_request(
            "INVALID_PERIOD",
            format!("period_from {pf} must not be after period_to {pt}"),
        ));
    }
    Ok((pf, pt))
}

/// Issue a persisted document: enqueue its event and stamp it `dispatched`.
///
/// Both halves happen inside the caller's transaction, so the record and the
/// event it announces commit together or not at all.
///
/// **Issuance does not depend on having an ERP.** Whether an invoice has been
/// issued is a property of the document; whether an ERP hears about it is a
/// property of the deployment. Writing the stamp only when `erp_webhook_url` is
/// configured would leave an operator without one holding permanent drafts:
/// `insert_billing_record`'s overwrite guard never arms, so a re-run silently
/// rewrites a document the customer already has; `pin_template` refuses to pin,
/// so the PDF re-styles itself with every template rollout; and the § 147 AO
/// reproducibility the whole design rests on is off.
///
/// # Errors
///
/// `500` when the outbox row or the stamp cannot be written.
pub(crate) async fn issue_record(
    tx: &mut sqlx::PgConnection,
    cfg: &BillingdConfig,
    record_id: Uuid,
    ce: &mako_service::CloudEvent,
) -> BillingResult<()> {
    if cfg.erp_webhook_url.is_some() {
        mako_service::outbox::enqueue(&mut *tx, ce).await?;
    }
    mark_dispatched_tx(&mut *tx, record_id)
        .await
        .map_err(BillingError::Internal)?;
    Ok(())
}

/// The document classes the § 14 Abs. 4 Nr. 4 UStG number series is split into.
///
/// One counter per class keeps the series readable in an audit: an ordinary
/// invoice, a consolidated document, a reversal and a self-billed § 41e credit
/// are four different kinds of document and an auditor reading `ST-2026-000004`
/// knows which one it is without opening it.
pub(crate) mod series {
    /// Ordinary Rechnung — `/calculate`, Tarifwechsel, the §40b sweep, and each
    /// participant line of a bundle.
    pub(crate) const INVOICE: &str = "RE";
    /// Consolidated document — B2B Sammelrechnung, § 42b GGV bundle.
    pub(crate) const CONSOLIDATED: &str = "SR";
    /// Storno- / Korrekturrechnung.
    pub(crate) const CORRECTION: &str = "ST";
    /// § 41e Gutschrift (self-billed VPP dispatch settlement).
    pub(crate) const CREDIT: &str = "VG";
}

/// The next Rechnungsnummer of a series, unless the caller stated one.
///
/// A caller may always supply its own number — an operator migrating a legacy
/// series, or a test pinning a value. Otherwise the number comes from the
/// tenant's counter, keyed on the **period's** year rather than today's, so a
/// December period billed in January stays in the year it belongs to.
///
/// # Errors
///
/// `500` when the counter cannot be advanced — without a number there is no
/// invoice, so this is not degradable.
pub(crate) async fn next_rechnungsnummer(
    pool: &PgPool,
    tenant: &str,
    series: &'static str,
    stated: Option<&str>,
    period_from: time::Date,
) -> BillingResult<String> {
    match stated {
        Some(nr) if !nr.trim().is_empty() => Ok(nr.to_owned()),
        _ => Ok(
            crate::pg::allocate_rechnungsnummer(pool, tenant, series, period_from.year()).await?,
        ),
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

#[cfg(test)]
mod period_tests {
    use super::{parse_date, parse_period};
    use time::macros::date;

    /// Both bounds are inclusive and a one-day period is valid: a move-in and
    /// move-out on the same day, a § 41e settlement of one day's dispatches.
    #[test]
    fn a_one_day_period_is_a_period() {
        assert_eq!(
            parse_period("2026-03-04", "2026-03-04").unwrap(),
            (date!(2026 - 03 - 04), date!(2026 - 03 - 04))
        );
    }

    /// The bound that broke Tarifwechsel: the handler parsed its switch date by
    /// passing it as both bounds, and `from < to` refused every equal pair — so
    /// the endpoint answered 400 for every request ever made to it.
    #[test]
    fn a_single_date_parses_on_its_own() {
        assert_eq!(
            parse_date("switch_date", "2026-03-15").unwrap(),
            date!(2026 - 03 - 15)
        );
        let e = parse_date("switch_date", "15.03.2026").unwrap_err();
        assert_eq!(e.code(), "INVALID_DATE");
        assert!(e.to_string().contains("switch_date"), "{e}");
    }

    /// A period that runs backwards is still refused.
    #[test]
    fn a_reversed_period_is_refused() {
        let e = parse_period("2026-03-31", "2026-03-01").unwrap_err();
        assert_eq!(e.code(), "INVALID_PERIOD");
        assert_eq!(e.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}
