//! Billing orchestration — bridges HTTP requests to `mako-nne` pure library.

use anyhow::{Context as _, bail};
use invoic_checker::{InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use mako_nne::{
    MmmInput, MsbInput, NneInput, calculate_mmm_invoice, calculate_msb_invoice,
    calculate_nne_invoice,
};

use crate::pg::upsert_draft;

// ── BillingRunRequest ─────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/run`.
///
/// Each entry in `positions` describes one MaLo to bill.
/// The operator fetches meter data from `edmd` and tariff from `marktd`
/// before calling this endpoint.
#[derive(Debug, Deserialize)]
pub struct BillingRunRequest {
    /// Netzbetreiber MP-ID — invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — invoice recipient.
    pub lf_mp_id: String,
    /// Invoice issue date (`YYYY-MM-DD`).
    pub invoice_date: String,
    /// Payment due date (`YYYY-MM-DD`).
    pub due_date: String,
    /// Prefix for auto-generated invoice numbers (`rechnungsnummer = prefix + "-" + index`).
    pub rechnungsnummer_prefix: String,
    /// Billing positions — one per MaLo.
    pub positions: Vec<BillingPosition>,
}

/// One MaLo billing entry inside [`BillingRunRequest`].
#[derive(Debug, Deserialize)]
pub struct BillingPosition {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Start of billing period (`YYYY-MM-DD`).
    pub period_from: String,
    /// End of billing period (`YYYY-MM-DD`).
    pub period_to: String,
    /// Invoice type: `"nne_strom"` (31001), `"nne_gas"` (31005), `"mmm"` (31002), or `"msb_31009"` (31009).
    pub billing_type: String,
    // ── NNE fields ────────────────────────────────────────────────────────────
    /// Total energy in kWh (NNE and MMM).
    pub arbeitsmenge_kwh: Option<Decimal>,
    /// Arbeitspreis in ct/kWh (NNE only, from `PreisblattNetznutzung`).
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// Spitzenleistung in kW (NNE RLM only).
    pub spitzenleistung_kw: Option<Decimal>,
    /// Leistungspreis in EUR/kW (NNE RLM only).
    pub leistungspreis_eur_per_kw: Option<Decimal>,
    /// Konzessionsabgabe rate in ct/kWh (optional).
    pub ka_satz_ct_per_kwh: Option<Decimal>,
    // ── MMM fields ────────────────────────────────────────────────────────────
    /// SLP profil consumption in kWh (MMM only).
    pub profil_kwh: Option<Decimal>,
    /// Mehrmengen price in ct/kWh (MMM only).
    pub mehr_preis_ct_per_kwh: Option<Decimal>,
    /// Mindermengen price in ct/kWh (MMM only).
    pub minder_preis_ct_per_kwh: Option<Decimal>,
    // ── MSB fields (31009) ───────────────────────────────────────────────────
    /// MSB (Messstellenbetreiber) MP-ID — invoice recipient for `"msb_31009"`.
    pub msb_mp_id: Option<String>,
    /// Grundgebühr Messstellenbetrieb in EUR/month (from `PreisblattMessung`).
    pub grundgebuehr_eur_per_month: Option<Decimal>,
    /// Number of full calendar months in the billing period.
    pub billing_months: Option<u32>,
    /// Optional Messdienstleistung flat fee in EUR for the full period.
    pub messdienstleistung_eur: Option<Decimal>,
}

fn parse_date(s: &str) -> anyhow::Result<time::Date> {
    use time::format_description::well_known::Iso8601;
    time::Date::parse(s, &Iso8601::DEFAULT).context("parse date")
}

/// Core billing orchestration called by the handler.
///
/// For each position:
/// 1. Calculate invoice via `mako-nne`
/// 2. Self-validate via `invoic-checker`
/// 3. Store as draft in PostgreSQL
///
/// Returns the list of generated draft UUIDs.
pub async fn run_billing_internal(
    pool: &PgPool,
    req: BillingRunRequest,
) -> anyhow::Result<Vec<Uuid>> {
    let invoice_date = parse_date(&req.invoice_date)?;
    let due_date = parse_date(&req.due_date)?;
    let empty_store = InMemoryPreisblattStore::new();
    let config = CheckConfig::default();

    let mut draft_ids = Vec::new();

    for (i, pos) in req.positions.iter().enumerate() {
        let period_from = parse_date(&pos.period_from)?;
        let period_to = parse_date(&pos.period_to)?;
        let rechnungsnummer = format!("{}-{:04}", req.rechnungsnummer_prefix, i + 1);

        let result = match pos.billing_type.as_str() {
            "nne_strom" | "nne_gas" => {
                let arbeit = pos
                    .arbeitsmenge_kwh
                    .context("arbeitsmenge_kwh required for NNE")?;
                let ap = pos
                    .arbeitspreis_ct_per_kwh
                    .context("arbeitspreis_ct_per_kwh required for NNE")?;
                let input = NneInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    lf_mp_id: req.lf_mp_id.clone(),
                    rechnungsnummer,
                    period_from,
                    period_to,
                    invoice_date,
                    due_date,
                    arbeitsmenge_kwh: arbeit,
                    arbeitspreis_ct_per_kwh: ap,
                    spitzenleistung_kw: pos.spitzenleistung_kw,
                    leistungspreis_eur_per_kw: pos.leistungspreis_eur_per_kw,
                    ka_satz_ct_per_kwh: pos.ka_satz_ct_per_kwh,
                };
                let mut r = calculate_nne_invoice(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;
                // Adjust PID for Gas
                if pos.billing_type == "nne_gas" {
                    r.pid = 31005;
                }
                r
            }
            "mmm" => {
                let actual = pos
                    .arbeitsmenge_kwh
                    .context("arbeitsmenge_kwh (actual) required for MMM")?;
                let profil = pos.profil_kwh.context("profil_kwh required for MMM")?;
                let mp = pos
                    .mehr_preis_ct_per_kwh
                    .context("mehr_preis_ct_per_kwh required for MMM")?;
                let mnp = pos
                    .minder_preis_ct_per_kwh
                    .context("minder_preis_ct_per_kwh required for MMM")?;
                let input = MmmInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    lf_mp_id: req.lf_mp_id.clone(),
                    rechnungsnummer,
                    period_from,
                    period_to,
                    invoice_date,
                    due_date,
                    actual_kwh: actual,
                    profil_kwh: profil,
                    mehr_preis_ct_per_kwh: mp,
                    minder_preis_ct_per_kwh: mnp,
                };
                calculate_mmm_invoice(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?
            }
            "msb_31009" => {
                let msb_mp_id = pos
                    .msb_mp_id
                    .as_deref()
                    .context("msb_mp_id required for msb_31009")?
                    .to_owned();
                let grundgebuehr = pos
                    .grundgebuehr_eur_per_month
                    .context("grundgebuehr_eur_per_month required for msb_31009")?;
                let months = pos
                    .billing_months
                    .context("billing_months required for msb_31009")?;
                let input = MsbInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    msb_mp_id,
                    rechnungsnummer,
                    period_from,
                    period_to,
                    invoice_date,
                    due_date,
                    grundgebuehr_eur_per_month: grundgebuehr,
                    billing_months: months,
                    messdienstleistung_eur: pos.messdienstleistung_eur,
                };
                calculate_msb_invoice(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?
            }
            t => bail!("unknown billing_type: {t}"),
        };

        // Self-validate via invoic-checker (checks 1–3 pass by construction;
        // check 4–5 may warn if tariff store is empty, but won't dispute).
        let report = InvoicCheckEngine::check(
            result.pid,
            &result.nb_mp_id,
            &result.rechnung,
            &empty_store,
            &config,
        );

        let rechnung_json = serde_json::to_value(&result.rechnung).context("serialize Rechnung")?;

        // For msb_31009 the invoice recipient is the MSB, not the LF.
        let counterparty = pos
            .msb_mp_id
            .as_deref()
            .filter(|_| pos.billing_type == "msb_31009")
            .unwrap_or(&req.lf_mp_id);

        let draft_id = upsert_draft(
            pool,
            &pos.malo_id,
            &req.nb_mp_id,
            counterparty,
            result.pid as i32,
            period_from,
            period_to,
            rechnung_json,
            result.total_eur,
            report.outcome,
        )
        .await
        .context("persist draft")?;

        draft_ids.push(draft_id);
    }

    Ok(draft_ids)
}
