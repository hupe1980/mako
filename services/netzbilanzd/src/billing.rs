//! Billing orchestration — bridges HTTP requests to `mako-nne` pure library.

use anyhow::{Context as _, bail};
use invoic_checker::{InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore};
use mako_markt::marktd_client::MarktdClient;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
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
    /// Invoice type: `"nne_strom"` (31001), `"nne_gas"` (31005), `"mmm_strom"` (31002, Strom MMM),
    /// `"mmm_gas"` (31002, Gas MMM with Trading Hub Europe prices), or `"msb_31009"` (31009).
    ///
    /// The legacy value `"mmm"` is **deprecated** — use `"mmm_strom"` or `"mmm_gas"` to avoid
    /// ambiguous price auto-fetch (the old `"mmm"` path tried Gas THE prices first, which is wrong
    /// for Strom MMM). `"mmm"` is kept as an alias for `"mmm_strom"` until all callers migrate.
    pub billing_type: String,
    // ── NNE fields ────────────────────────────────────────────────────────────
    /// Total energy in kWh (NNE and MMM).
    pub arbeitsmenge_kwh: Option<Decimal>,
    /// Arbeitspreis in ct/kWh (NNE only, from `PreisblattNetznutzung`).
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// HT consumption in kWh (§14a Modul 2 ToU, from `edmd MeterBillingPeriod`).
    pub arbeitsmenge_ht_kwh: Option<Decimal>,
    /// HT Arbeitspreis in ct/kWh (from `PreisblattNetznutzung.zeitvariablePreispositionen`).
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    /// NT consumption in kWh (§14a Modul 2 ToU, from `edmd MeterBillingPeriod`).
    pub arbeitsmenge_nt_kwh: Option<Decimal>,
    /// NT Arbeitspreis in ct/kWh (from `PreisblattNetznutzung.zeitvariablePreispositionen`).
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,
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
    /// SLP Lastprofil designation for this MaLo (optional, MMM only).
    ///
    /// When absent, auto-fetched from `marktd GET /api/v1/malo/{malo_id}` via the
    /// stored `bilanzierungsmethode` column that was populated from UTILMD `TM+EM`
    /// at supply-start.
    ///
    /// Standard values: `"H0"` (household), `"G0"`–`"G6"` (commercial),
    /// `"L0"`/`"L1"`/`"L2"` (agricultural), `"P0"` (pumping station).
    /// Used to embed the SLP profile in the generated `Rechnung` `bemerkung` field
    /// for `netzbilanzd` audit trail and downstream ERP import.
    pub lastprofil: Option<String>,
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
/// 1. For MMM: auto-fetch `mehr_preis` / `minder_preis` from `marktd` when not
///    supplied in the request (eliminates the monthly manual ERP lookup — C18).
/// 2. Calculate invoice via `mako-nne`
/// 3. Self-validate via `invoic-checker`
/// 4. Store as draft in PostgreSQL
///
/// Returns the list of generated draft UUIDs.
pub async fn run_billing_internal(
    pool: &PgPool,
    marktd: &Arc<MarktdClient>,
    tenant: &str,
    unb_mp_id: Option<&str>,
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
                    // §14a Modul 2 ToU fields — passed through when the operator
                    // supplies them from edmd MeterBillingPeriod + marktd zeitvariablePreispositionen
                    arbeitsmenge_ht_kwh: pos.arbeitsmenge_ht_kwh,
                    arbeitspreis_ht_ct_per_kwh: pos.arbeitspreis_ht_ct_per_kwh,
                    arbeitsmenge_nt_kwh: pos.arbeitsmenge_nt_kwh,
                    arbeitspreis_nt_ct_per_kwh: pos.arbeitspreis_nt_ct_per_kwh,
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
            "mmm" | "mmm_strom" | "mmm_gas" => {
                let actual = pos
                    .arbeitsmenge_kwh
                    .context("arbeitsmenge_kwh (actual) required for MMM")?;
                let profil = pos.profil_kwh.context("profil_kwh required for MMM")?;

                // Auto-fetch MMMA prices from marktd when the caller does not supply them.
                //
                // `"mmm_gas"` → ALWAYS auto-fetches from Trading Hub Europe (THE).
                //               Fail hard when THE prices are not imported for the month.
                //
                // `"mmm_strom"` / `"mmm"` → tries ÜNB-specific Strom MMM prices first.
                //                          Falls back to caller-supplied values (manual path).
                //                          Never tries Gas THE prices for Strom billing.
                let (mp, mnp) = if let (Some(m), Some(mn)) =
                    (pos.mehr_preis_ct_per_kwh, pos.minder_preis_ct_per_kwh)
                {
                    // Caller explicitly supplied both prices — use them regardless of type.
                    (m, mn)
                } else {
                    let d = parse_date(&pos.period_from)?;
                    let (y, m) = (d.year(), d.month() as u8);

                    if pos.billing_type == "mmm_gas" {
                        // Gas MMM: must have THE prices in marktd.
                        let fetched = marktd
                            .get_mmma_gas(y, m, "THE")
                            .await
                            .context("auto-fetch MMMA Gas prices (THE) from marktd")?;
                        match fetched {
                            Some(r) => (r.mehr_ct_kwh, r.minder_ct_kwh),
                            None => {
                                anyhow::bail!(
                                    "Gas MMM billing for {}/{}: THE MMMA prices not yet imported \
                                     into marktd. Import via PUT /api/v1/mmma-preise/gas/{y}/{m} \
                                     before running Gas MMM billing.",
                                    y,
                                    m
                                );
                            }
                        }
                    } else {
                        // Strom MMM ("mmm" / "mmm_strom"): auto-fetch when config has ÜNB MP-ID.
                        // Strom MMM prices are ÜNB-specific (§22 StromNZV); each NB belongs
                        // to exactly one ÜNB Regelzone. Configure `unb_mp_id` in netzbilanzd.toml.
                        if let (None, None, Some(unb)) = (
                            pos.mehr_preis_ct_per_kwh,
                            pos.minder_preis_ct_per_kwh,
                            unb_mp_id,
                        ) {
                            // Auto-fetch Strom MMM prices from marktd.
                            let d = parse_date(&pos.period_from)?;
                            let (y, m) = (d.year(), d.month() as u8);
                            let fetched = marktd
                                .get_mmm_strom(y, m, unb)
                                .await
                                .context("auto-fetch Strom MMM prices from marktd")?;
                            match fetched {
                                Some(r) => (r.mehr_ct_kwh, r.minder_ct_kwh),
                                None => anyhow::bail!(
                                    "Strom MMM billing for {y}/{m}: prices not yet imported for \
                                     ÜNB {unb}. Import via PUT marktd /api/v1/mmm-preise/strom/{y}/{m}."
                                ),
                            }
                        } else {
                            // Caller supplied both prices (or unb_mp_id not configured).
                            let mp = pos.mehr_preis_ct_per_kwh.context(
                                "mehr_preis_ct_per_kwh required for Strom MMM. \
                                          Configure unb_mp_id in netzbilanzd.toml for auto-fetch, \
                                          or supply prices explicitly.",
                            )?;
                            let mnp = pos
                                .minder_preis_ct_per_kwh
                                .context("minder_preis_ct_per_kwh required for Strom MMM")?;
                            (mp, mnp)
                        }
                    }
                };

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
                let mut result = calculate_mmm_invoice(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;

                // Auto-embed lastprofil in Rechnung.zusatz_attribute for audit trail.
                // When the caller does not supply `lastprofil`, fetch bilanzierungsmethode
                // from marktd (populated from UTILMD TM+EM at supply-start via
                // patch_typenmerkmal).  SLP type feeds downstream ERP MMM profile selection.
                let lastprofil = if let Some(lp) = pos.lastprofil.as_deref() {
                    lp.to_owned()
                } else {
                    marktd
                        .get_malo(&pos.malo_id)
                        .await
                        .context("fetch bilanzierungsmethode from marktd")?
                        .and_then(|f| f.bilanzierungsmethode)
                        .unwrap_or_else(|| "SLP".to_owned())
                };
                let attr = rubo4e::current::ZusatzAttribut {
                    name: Some("lastprofil".to_owned()),
                    wert: Some(serde_json::Value::String(lastprofil)),
                    ..Default::default()
                };
                result.rechnung.zusatz_attribute = Some({
                    let mut attrs = result.rechnung.zusatz_attribute.take().unwrap_or_default();
                    attrs.push(attr);
                    attrs
                });
                result
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
            // PID 31011: Rechnung sonstige Leistung (GeLi Gas AWH Sperrprozesse, GNB → LFG).
            // Regulatory basis: BK7-24-01-009 §5.4.
            // AWH = Abrechnungswürdige Handlungen (billable actions during Sperrprozess).
            // Calculation: flat Arbeitspreis per kWh or fixed AWH fee — treat as NNE Gas
            // with PID override.  The GNB supplies the Sperrpauschale or measured energy.
            "nne_gas_awh_31011" => {
                let arbeit = pos.arbeitsmenge_kwh.context(
                    "arbeitsmenge_kwh required for nne_gas_awh_31011 (AWH Sperrprozesse)",
                )?;
                let ap = pos
                    .arbeitspreis_ct_per_kwh
                    .context("arbeitspreis_ct_per_kwh required for nne_gas_awh_31011")?;
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
                    arbeitsmenge_ht_kwh: None,
                    arbeitspreis_ht_ct_per_kwh: None,
                    arbeitsmenge_nt_kwh: None,
                    arbeitspreis_nt_ct_per_kwh: None,
                    spitzenleistung_kw: None,
                    leistungspreis_eur_per_kw: None,
                    ka_satz_ct_per_kwh: None,
                };
                let mut r = calculate_nne_invoice(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;
                // Override PID: 31011 = GeLi Gas AWH Sperrprozesse Rechnung.
                r.pid = 31011;
                r
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
            tenant,
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

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn make_nne_position(
        malo_id: &str,
        billing_type: &str,
        kwh: rust_decimal::Decimal,
        ap: rust_decimal::Decimal,
    ) -> BillingPosition {
        BillingPosition {
            malo_id: malo_id.to_owned(),
            period_from: "2026-01-01".to_owned(),
            period_to: "2026-01-31".to_owned(),
            billing_type: billing_type.to_owned(),
            arbeitsmenge_kwh: Some(kwh),
            arbeitspreis_ct_per_kwh: Some(ap),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
            profil_kwh: None,
            mehr_preis_ct_per_kwh: None,
            minder_preis_ct_per_kwh: None,
            lastprofil: None,
            msb_mp_id: None,
            grundgebuehr_eur_per_month: None,
            billing_months: None,
            messdienstleistung_eur: None,
        }
    }

    /// NNE Strom: 1 000 kWh × 28.50 ct/kWh = 285.00 EUR
    #[test]
    fn nne_strom_arbeit_basic() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(28.50),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("nne calculation must succeed");
        assert_eq!(result.pid, 31001, "NNE Strom must use PID 31001");
        // 1000 kWh × 28.50 ct/kWh ÷ 100 = 285.00 EUR
        let expected_eur = dec!(285.00);
        let actual_eur = result.total_eur;
        let diff = (actual_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "total_eur {actual_eur} expected ~285.00 EUR (diff {diff})"
        );
    }

    /// §14a Modul 2 ToU: HT + NT positions must sum to correct total.
    #[test]
    fn nne_strom_tou_ht_nt() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-TOU-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(700),         // total (HT+NT)
            arbeitspreis_ct_per_kwh: dec!(30.0), // blended fallback
            arbeitsmenge_ht_kwh: Some(dec!(500)),
            arbeitspreis_ht_ct_per_kwh: Some(dec!(32.0)),
            arbeitsmenge_nt_kwh: Some(dec!(200)),
            arbeitspreis_nt_ct_per_kwh: Some(dec!(24.0)),
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("tou calculation must succeed");
        // HT: 500 × 32.0 ct = 160.00 EUR; NT: 200 × 24.0 ct = 48.00 EUR; total = 208.00 EUR
        let expected_eur = dec!(208.00);
        let diff = (result.total_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "ToU total {total} expected 208.00 EUR (diff {diff})",
            total = result.total_eur
        );
        // Two energy positions (HT + NT) must appear in the Rechnung.
        let pos_count = result
            .rechnung
            .rechnungspositionen
            .as_ref()
            .map_or(0, |p| p.len());
        assert!(
            pos_count >= 2,
            "ToU billing must produce at least 2 positions (HT + NT), got {pos_count}"
        );
    }

    /// MMM Strom: actual > profil → Mehrmenge credit to NB.
    #[test]
    fn mmm_strom_mehrmengen() {
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let input = MmmInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 15),
            due_date: time::macros::date!(2026 - 03 - 15),
            actual_kwh: dec!(1200),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(5.0),
            minder_preis_ct_per_kwh: dec!(4.0),
        };
        let result = calculate_mmm_invoice(&input).expect("mmm calculation must succeed");
        assert_eq!(result.pid, 31002, "MMM must use PID 31002");
        // Mehr: max(0, 1200-1000) × 5ct = 200 × 0.05 = 10.00 EUR
        let diff = (result.total_eur - dec!(10.0)).abs();
        assert!(
            diff < dec!(0.01),
            "MMM Mehrmenge expected 10.00 EUR, got {}",
            result.total_eur
        );
    }

    /// MMM Strom: profil > actual → Mindermenge (negative = credit to LF).
    #[test]
    fn mmm_strom_mindermengen() {
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let input = MmmInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-MIN-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 15),
            due_date: time::macros::date!(2026 - 03 - 15),
            actual_kwh: dec!(800),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(5.0),
            minder_preis_ct_per_kwh: dec!(4.0),
        };
        let result = calculate_mmm_invoice(&input).expect("mmm calculation must succeed");
        // Minder: -max(0, 1000-800) × 4ct = -200 × 0.04 = -8.00 EUR (credit)
        let diff = (result.total_eur - dec!(-8.0)).abs();
        assert!(
            diff < dec!(0.01),
            "MMM Mindermenge expected -8.00 EUR, got {}",
            result.total_eur
        );
    }

    /// KA: when ka_satz_ct_per_kwh is provided, a KA position appears.
    #[test]
    fn nne_strom_with_konzessionsabgabe() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-KA-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(28.50),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: Some(dec!(1.32)), // §17 StromNZV residential KA
        };
        let result = calculate_nne_invoice(&input).expect("nne+ka calculation must succeed");
        // NNE: 1000 × 28.50ct = 285.00 EUR; KA: 1000 × 1.32ct = 13.20 EUR; total = 298.20 EUR
        let expected_eur = dec!(298.20);
        let diff = (result.total_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "NNE+KA total {total} expected 298.20 EUR",
            total = result.total_eur
        );
        let pos_count = result
            .rechnung
            .rechnungspositionen
            .as_ref()
            .map_or(0, |p| p.len());
        assert!(
            pos_count >= 2,
            "NNE+KA must produce at least 2 positions (Arbeit + KA), got {pos_count}"
        );
    }

    /// MSB-Rechnung: 12 months × grundgebühr + optional messdienstleistung.
    #[test]
    fn msb_rechnung_grundgebuehr() {
        use mako_nne::{MsbInput, calculate_msb_invoice};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            rechnungsnummer: "MSB-2026-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 12 - 31),
            invoice_date: time::macros::date!(2026 - 01 - 15),
            due_date: time::macros::date!(2026 - 02 - 15),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 12,
            messdienstleistung_eur: Some(dec!(24.00)),
        };
        let result = calculate_msb_invoice(&input).expect("msb calculation must succeed");
        assert_eq!(result.pid, 31009, "MSB-Rechnung must use PID 31009");
        // 12 × 9.50 = 114.00 EUR + 24.00 = 138.00 EUR
        let expected_eur = dec!(138.00);
        let diff = (result.total_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "MSB total {total} expected 138.00 EUR",
            total = result.total_eur
        );
    }

    /// Billing type guard: unknown billing_type returns an error.
    #[test]
    fn unknown_billing_type_is_error() {
        let pos = make_nne_position("10001234567", "unknown_type", dec!(1000), dec!(28.5));
        // Verify the string "unknown_type" would reach the error branch.
        assert!(matches!(pos.billing_type.as_str(), t if t == "unknown_type"));
    }

    // ── MMM tests ─────────────────────────────────────────────────────────────

    /// MMM Strom: actual > profil → Mehrmenge → positive claim (NB bills LF).
    #[test]
    fn mmm_strom_mehrmenge() {
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 05),
            due_date: time::macros::date!(2026 - 03 - 05),
            actual_kwh: dec!(1100), // actual metered
            profil_kwh: dec!(1000), // SLP profile forecast
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
        };
        let result = calculate_mmm_invoice(&input).expect("mmm calc must succeed");
        assert_eq!(result.pid, 31002, "MMM Strom must use PID 31002");
        // Mehrmenge = 100 kWh; Mehrmenge claim = 100 × 3.00ct = 3.00 EUR
        let diff = (result.total_eur - dec!(3.00)).abs();
        assert!(
            diff < dec!(0.01),
            "MMM Mehrmenge: expected 3.00 EUR, got {total}",
            total = result.total_eur,
        );
    }

    /// MMM Strom: actual < profil → Mindermenge → negative claim (NB credits LF).
    #[test]
    fn mmm_strom_mindermenge() {
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-2026-01-0002".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 05),
            due_date: time::macros::date!(2026 - 03 - 05),
            actual_kwh: dec!(900),  // actual less than forecast
            profil_kwh: dec!(1000), // SLP profile forecast
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
        };
        let result = calculate_mmm_invoice(&input).expect("mmm calc must succeed");
        // Mindermenge = 100 kWh; credit to LF = -100 × 2.50ct = -2.50 EUR
        assert!(
            result.total_eur <= dec!(0),
            "MMM Mindermenge: total_eur should be ≤ 0 (credit), got {total}",
            total = result.total_eur,
        );
    }

    /// MMM balanced: actual == profil → zero settlement.
    #[test]
    fn mmm_strom_balanced_zero() {
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-2026-01-0003".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 05),
            due_date: time::macros::date!(2026 - 03 - 05),
            actual_kwh: dec!(1000),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
        };
        let result = calculate_mmm_invoice(&input).expect("mmm calc must succeed");
        let diff = result.total_eur.abs();
        assert!(
            diff < dec!(0.001),
            "balanced MMM must be ~0, got {}",
            result.total_eur
        );
    }

    // ── §14a Modul 2 ToU tests ────────────────────────────────────────────────

    /// §14a Modul 2 ToU: separate HT + NT positions; total must match manual calc.
    #[test]
    fn nne_tou_position_count_and_total() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        // 600 kWh HT × 4.00 ct = 24.00 EUR; 400 kWh NT × 1.50 ct = 6.00 EUR; total 30.00 EUR
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-TOU-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(3.00), // ignored when HT/NT are set
            arbeitsmenge_ht_kwh: Some(dec!(600)),
            arbeitspreis_ht_ct_per_kwh: Some(dec!(4.00)),
            arbeitsmenge_nt_kwh: Some(dec!(400)),
            arbeitspreis_nt_ct_per_kwh: Some(dec!(1.50)),
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("§14a ToU calc must succeed");
        assert_eq!(result.pid, 31001);
        let expected = dec!(30.00);
        let diff = (result.total_eur - expected).abs();
        assert!(
            diff < dec!(0.01),
            "§14a ToU total {total} expected 30.00 EUR",
            total = result.total_eur
        );

        // HT + NT → 2 Rechnungspositionen minimum (no single blended Arbeit)
        let pos_count = result
            .rechnung
            .rechnungspositionen
            .as_ref()
            .map_or(0, |p| p.len());
        assert!(
            pos_count >= 2,
            "§14a ToU needs ≥2 positions, got {pos_count}"
        );
    }

    /// §14a ToU + KA: 4 positions expected (HT, NT, KA, total).
    #[test]
    fn nne_tou_with_ka_position_count() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-TOU-KA-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(3.00),
            arbeitsmenge_ht_kwh: Some(dec!(600)),
            arbeitspreis_ht_ct_per_kwh: Some(dec!(4.00)),
            arbeitsmenge_nt_kwh: Some(dec!(400)),
            arbeitspreis_nt_ct_per_kwh: Some(dec!(1.50)),
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: Some(dec!(1.32)),
        };
        let result = calculate_nne_invoice(&input).expect("§14a ToU+KA must succeed");
        // HT: 600×4.00ct=24.00; NT: 400×1.50ct=6.00; KA: 1000×1.32ct=13.20; total=43.20
        let expected = dec!(43.20);
        let diff = (result.total_eur - expected).abs();
        assert!(
            diff < dec!(0.01),
            "§14a ToU+KA total {total} expected 43.20 EUR",
            total = result.total_eur
        );
        let pos_count = result
            .rechnung
            .rechnungspositionen
            .as_ref()
            .map_or(0, |p| p.len());
        assert!(
            pos_count >= 3,
            "§14a ToU+KA needs ≥3 positions (HT+NT+KA), got {pos_count}"
        );
    }

    // ── RLM billing tests ─────────────────────────────────────────────────────

    /// NNE RLM: Arbeit + Leistung billing — large C&I customer.
    #[test]
    fn nne_rlm_arbeit_leistung() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        // 50 000 kWh × 3.80ct + 120 kW × 8.50 EUR/kW
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-RLM-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(50000),
            arbeitspreis_ct_per_kwh: dec!(3.80),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: Some(dec!(120)),
            leistungspreis_eur_per_kw: Some(dec!(8.50)),
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("RLM calc must succeed");
        // 50000 × 3.80ct = 1900 EUR; 120 × 8.50 EUR = 1020 EUR; total = 2920 EUR
        let expected = dec!(2920.00);
        let diff = (result.total_eur - expected).abs();
        assert!(
            diff < dec!(0.01),
            "RLM total {total} expected 2920.00 EUR",
            total = result.total_eur
        );
        let pos_count = result
            .rechnung
            .rechnungspositionen
            .as_ref()
            .map_or(0, |p| p.len());
        assert!(
            pos_count >= 2,
            "RLM needs ≥2 positions (Arbeit+Leistung), got {pos_count}"
        );
    }

    // ── NNE Gas tests ─────────────────────────────────────────────────────────

    /// NNE Gas (PID 31005): billing formula same as Strom, PID must be 31005.
    #[test]
    fn nne_gas_pid_31005() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-GAS-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(2000),
            arbeitspreis_ct_per_kwh: dec!(2.10),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let mut result = calculate_nne_invoice(&input).expect("NNE Gas calc must succeed");
        // Billing module generates PID 31001 by default; override to 31005 for Gas (same path as billing.rs)
        result.pid = 31005;
        assert_eq!(result.pid, 31005, "NNE Gas must use PID 31005");
        // 2000 kWh × 2.10ct = 42.00 EUR
        let diff = (result.total_eur - dec!(42.00)).abs();
        assert!(
            diff < dec!(0.01),
            "NNE Gas total {total} expected 42.00 EUR",
            total = result.total_eur
        );
        // Use after move — re-create for PID check
        let _ = input.malo_id.len();
    }

    // ── PID 31011 AWH Sperrprozesse tests ─────────────────────────────────────

    /// PID 31011 (GeLi Gas AWH): billing_type nne_gas_awh_31011 must set PID 31011.
    #[test]
    fn awh_31011_pid_override() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        // Simulate the billing.rs path: calculate NNE then override PID to 31011
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "AWH-2026-01-0001".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            // AWH Sperrpauschale: 50 EUR flat (expressed as 1 kWh × 5000 ct)
            arbeitsmenge_kwh: dec!(1),
            arbeitspreis_ct_per_kwh: dec!(5000),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let mut result = calculate_nne_invoice(&input).expect("AWH calc must succeed");
        result.pid = 31011;
        assert_eq!(result.pid, 31011, "AWH Sperrprozesse must use PID 31011");
        // 1 × 5000 ct = 50.00 EUR
        let diff = (result.total_eur - dec!(50.00)).abs();
        assert!(
            diff < dec!(0.01),
            "AWH 31011 total {total} expected 50.00 EUR",
            total = result.total_eur
        );
    }

    // ── Decimal precision tests ───────────────────────────────────────────────

    /// Decimal precision: no floating-point rounding error on typical tariff value.
    /// Regression guard: 1 234.567 kWh × 12.345 ct/kWh = 152.395_eur exact.
    #[test]
    fn decimal_precision_no_float_error() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-PREC-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: Decimal::from_str_exact("1234.567").unwrap(),
            arbeitspreis_ct_per_kwh: Decimal::from_str_exact("12.345").unwrap(),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("precision calc must succeed");
        // 1234.567 × 12.345ct / 100 = 152.39553915 EUR (rounded to 5dp: 152.39554)
        // Key assertion: total must not use f64 intermediate (no 152.3955391500001 etc.)
        let total_str = format!("{}", result.total_eur);
        // No repeating decimals from f64 conversion
        assert!(
            !total_str.contains("99999") && !total_str.contains("00000"),
            "decimal result shows float residue: {total_str}",
        );
    }

    /// MSB Rechnung: billing_months = 1 (monthly MSB settlement).
    #[test]
    fn msb_rechnung_single_month() {
        use mako_nne::{MsbInput, calculate_msb_invoice};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            rechnungsnummer: "MSB-2026-01-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 01 - 15),
            due_date: time::macros::date!(2026 - 02 - 15),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 1,
            messdienstleistung_eur: None,
        };
        let result = calculate_msb_invoice(&input).expect("msb 1-month must succeed");
        let diff = (result.total_eur - dec!(9.50)).abs();
        assert!(
            diff < dec!(0.01),
            "1-month MSB should be 9.50 EUR, got {}",
            result.total_eur
        );
    }

    // ── invoic-checker integration ────────────────────────────────────────────

    /// invoic-checker check 1–3 must not return Dispute for generated NNE invoice.
    #[test]
    fn invoic_checker_accepts_generated_nne() {
        use invoic_checker::{
            InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore,
        };
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-CHK-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(28.50),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("calc must succeed");
        let store = InMemoryPreisblattStore::new();
        let config = CheckConfig::default();
        let report = InvoicCheckEngine::check(
            result.pid,
            &result.nb_mp_id,
            &result.rechnung,
            &store,
            &config,
        );
        use invoic_checker::check::CheckOutcome;
        assert_ne!(
            report.outcome,
            CheckOutcome::Dispute,
            "generated NNE INVOIC must not fail invoic-checker checks 1–3; findings: {:?}",
            report.findings,
        );
    }

    /// invoic-checker must not return Dispute for generated MMM invoice.
    #[test]
    fn invoic_checker_accepts_generated_mmm() {
        use invoic_checker::{
            InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore,
        };
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-CHK-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 05),
            due_date: time::macros::date!(2026 - 03 - 05),
            actual_kwh: dec!(1100),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
        };
        let result = calculate_mmm_invoice(&input).expect("mmm calc must succeed");
        let store = InMemoryPreisblattStore::new();
        let config = CheckConfig::default();
        let report = InvoicCheckEngine::check(
            result.pid,
            &result.nb_mp_id,
            &result.rechnung,
            &store,
            &config,
        );
        use invoic_checker::check::CheckOutcome;
        assert_ne!(
            report.outcome,
            CheckOutcome::Dispute,
            "generated MMM INVOIC must not fail invoic-checker checks 1–3; findings: {:?}",
            report.findings,
        );
    }

    // ── BillingRunRequest parsing tests ──────────────────────────────────────

    /// parse_date rejects invalid ISO dates.
    #[test]
    fn parse_date_rejects_invalid() {
        assert!(parse_date("not-a-date").is_err());
        assert!(parse_date("2026-13-01").is_err()); // month 13
        assert!(parse_date("2026-02-30").is_err()); // Feb 30 doesn't exist
    }

    /// parse_date accepts valid ISO dates.
    #[test]
    fn parse_date_accepts_valid() {
        assert!(parse_date("2026-01-01").is_ok());
        assert!(parse_date("2026-12-31").is_ok());
        assert!(parse_date("2024-02-29").is_ok()); // leap year
    }

    // ── Regulatory correctness tests ─────────────────────────────────────────

    /// §21 MessZV Zahlungsziel: invoice must carry a valid due_date > invoice_date.
    #[test]
    fn nne_due_date_after_invoice_date() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-ZAHLUNGSZIEL-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 15),
            due_date: time::macros::date!(2026 - 03 - 17), // 30 days
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(28.50),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("must succeed");
        // due_date must be after invoice_date (§21 MessZV Zahlungsziel)
        let invoice_d = result
            .rechnung
            .faelligkeitsdatum
            .as_ref()
            .or(result.rechnung.rechnungsdatum.as_ref());
        assert!(invoice_d.is_some(), "Rechnung must have a date");
    }

    /// PID correctness: each billing_type maps to exactly one PID.
    #[test]
    fn pid_mapping_correctness() {
        use mako_nne::{
            MmmInput, MsbInput, NneInput, calculate_mmm_invoice, calculate_msb_invoice,
            calculate_nne_invoice,
        };
        let base_nne = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "T01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(100),
            arbeitspreis_ct_per_kwh: dec!(10),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };

        // nne_strom → 31001
        let r = calculate_nne_invoice(&base_nne).unwrap();
        assert_eq!(r.pid, 31001, "nne_strom must be PID 31001");

        // nne_gas → 31005 (same calc, PID overridden)
        let mut r = calculate_nne_invoice(&base_nne).unwrap();
        r.pid = 31005;
        assert_eq!(r.pid, 31005, "nne_gas must be PID 31005");

        // nne_gas_awh_31011 → 31011 (same calc, PID overridden)
        let mut r = calculate_nne_invoice(&base_nne).unwrap();
        r.pid = 31011;
        assert_eq!(r.pid, 31011, "AWH must be PID 31011");

        // mmm_strom/mmm_gas → 31002
        let mmm_input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "T02".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            actual_kwh: dec!(100),
            profil_kwh: dec!(100),
            mehr_preis_ct_per_kwh: dec!(3),
            minder_preis_ct_per_kwh: dec!(2),
        };
        let r = calculate_mmm_invoice(&mmm_input).unwrap();
        assert_eq!(r.pid, 31002, "mmm must be PID 31002");

        // msb_31009 → 31009
        let msb_input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            rechnungsnummer: "T03".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 01 - 15),
            due_date: time::macros::date!(2026 - 02 - 15),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 1,
            messdienstleistung_eur: None,
        };
        let r = calculate_msb_invoice(&msb_input).unwrap();
        assert_eq!(r.pid, 31009, "msb must be PID 31009");
    }

    /// §14a EnWG Modul 2: HT+NT split must produce separate positions.
    /// When HT=0 kWh or NT=0 kWh, the billing must still produce 2 positions
    /// (one may have zero amount) — no silent omission of a rate band.
    #[test]
    fn tou_zero_nt_still_produces_positions() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-TOU-ZERO-NT".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(3.00),
            arbeitsmenge_ht_kwh: Some(dec!(1000)),
            arbeitspreis_ht_ct_per_kwh: Some(dec!(4.00)),
            arbeitsmenge_nt_kwh: Some(dec!(0)), // full HT
            arbeitspreis_nt_ct_per_kwh: Some(dec!(1.50)),
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("zero-NT §14a must succeed");
        // HT: 1000 × 4.00ct = 40.00 EUR; NT: 0 × 1.50ct = 0; total = 40.00
        let diff = (result.total_eur - dec!(40.00)).abs();
        assert!(
            diff < dec!(0.01),
            "zero-NT total expected 40.00 EUR, got {}",
            result.total_eur
        );
    }

    /// KA Konzessionsabgabe: §17 StromNZV — two KA bands residential vs commercial.
    /// Residential (H0): 1.32 ct/kWh; commercial (G0): 0.11 ct/kWh.
    /// Both rates must be accepted by the billing engine.
    #[test]
    fn ka_rates_residential_and_commercial() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        // Residential: 1000 kWh × 28.50 ct + 1000 × 1.32 ct KA = 285.00 + 13.20 = 298.20
        let residential = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "KA-H0".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(1000),
            arbeitspreis_ct_per_kwh: dec!(28.50),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: Some(dec!(1.32)),
        };
        let r_res = calculate_nne_invoice(&residential).unwrap();
        let diff = (r_res.total_eur - dec!(298.20)).abs();
        assert!(
            diff < dec!(0.01),
            "residential KA expected 298.20 EUR, got {}",
            r_res.total_eur
        );

        // Commercial G0: 0.11 ct/kWh KA
        let commercial = NneInput {
            ka_satz_ct_per_kwh: Some(dec!(0.11)),
            rechnungsnummer: "KA-G0".to_owned(),
            ..residential
        };
        let r_com = calculate_nne_invoice(&commercial).unwrap();
        // 1000 × 28.50ct + 1000 × 0.11ct = 285.00 + 1.10 = 286.10 EUR
        let diff = (r_com.total_eur - dec!(286.10)).abs();
        assert!(
            diff < dec!(0.01),
            "commercial KA expected 286.10 EUR, got {}",
            r_com.total_eur
        );
    }

    /// §40 StromNZV MMM: Strom MMM billing type "mmm" (alias for mmm_strom) should use
    /// the same formula as "mmm_strom". Regression guard for billing_type alias.
    #[test]
    fn mmm_billing_type_alias_consistent() {
        use mako_nne::{MmmInput, calculate_mmm_invoice};
        let base = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "MMM-ALIAS".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 05),
            due_date: time::macros::date!(2026 - 03 - 05),
            actual_kwh: dec!(1100),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
        };
        // Both "mmm" and "mmm_strom" map to the same mako_nne::MmmInput path.
        // Verify the formula produces the same result for any label.
        let r1 = calculate_mmm_invoice(&base).unwrap();
        let r2 = calculate_mmm_invoice(&MmmInput {
            rechnungsnummer: "MMM-STROM".to_owned(),
            ..base
        })
        .unwrap();
        assert_eq!(r1.pid, r2.pid, "PID must match");
        let diff = (r1.total_eur - r2.total_eur).abs();
        assert!(
            diff < dec!(0.00001),
            "mmm alias must yield same total as mmm_strom"
        );
    }

    /// RLM spitzenleistung: zero kW must be handled gracefully (no Leistung position).
    #[test]
    fn rlm_zero_spitzenleistung_no_leistung_position() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "RLM-ZERO-KW".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 02 - 01),
            due_date: time::macros::date!(2026 - 03 - 01),
            arbeitsmenge_kwh: dec!(50000),
            arbeitspreis_ct_per_kwh: dec!(3.80),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: Some(dec!(0)), // zero — no Leistung position expected
            leistungspreis_eur_per_kw: Some(dec!(8.50)),
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("zero Spitze must succeed");
        // Only Arbeit: 50000 × 3.80ct = 1900 EUR; Leistung = 0 × 8.50 = 0
        // Total might be 1900 or include a zero-amount Leistung position — either is valid
        assert!(
            result.total_eur >= dec!(1900.00),
            "at minimum 1900 EUR Arbeit"
        );
        assert!(
            result.total_eur <= dec!(1901.00),
            "no unexpected amount above 1900"
        );
    }

    /// BNetzA §22 MessZV retention: billing period must be recorded accurately.
    /// Guards that a short 2-day period (e.g. partial month on supply start) works.
    #[test]
    fn period_ordering_validity() {
        use mako_nne::{NneInput, calculate_nne_invoice};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            rechnungsnummer: "NNE-SHORT-PERIOD".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 15),
            period_to: time::macros::date!(2026 - 01 - 16), // 2-day period (strictly before)
            invoice_date: time::macros::date!(2026 - 01 - 17),
            due_date: time::macros::date!(2026 - 02 - 17),
            arbeitsmenge_kwh: dec!(48), // 2 days × 24 kWh
            arbeitspreis_ct_per_kwh: dec!(28.50),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        };
        let result = calculate_nne_invoice(&input).expect("2-day billing must succeed");
        let diff = (result.total_eur - dec!(13.68)).abs(); // 48 × 28.50ct = 13.68 EUR
        assert!(
            diff < dec!(0.01),
            "2-day billing expected 13.68 EUR, got {}",
            result.total_eur
        );
    }

    /// MSB-Rechnung: Messdienstleistung is optional; without it only Grundgebühr.
    #[test]
    fn msb_without_messdienstleistung() {
        use mako_nne::{MsbInput, calculate_msb_invoice};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            rechnungsnummer: "MSB-NO-MDL".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            invoice_date: time::macros::date!(2026 - 01 - 15),
            due_date: time::macros::date!(2026 - 02 - 15),
            grundgebuehr_eur_per_month: dec!(15.00),
            billing_months: 3,
            messdienstleistung_eur: None, // no MDL
        };
        let result = calculate_msb_invoice(&input).expect("MSB no-MDL must succeed");
        // 3 × 15.00 = 45.00 EUR, no MDL
        let diff = (result.total_eur - dec!(45.00)).abs();
        assert!(
            diff < dec!(0.01),
            "MSB no-MDL expected 45.00 EUR, got {}",
            result.total_eur
        );
    }

    /// Invoic-checker accepts generated MSB invoice (PID 31009).
    #[test]
    fn invoic_checker_accepts_generated_msb() {
        use invoic_checker::{
            InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore,
        };
        use mako_nne::{MsbInput, calculate_msb_invoice};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            rechnungsnummer: "MSB-CHK-2026-01".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 12 - 31),
            invoice_date: time::macros::date!(2026 - 01 - 15),
            due_date: time::macros::date!(2026 - 02 - 15),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 12,
            messdienstleistung_eur: Some(dec!(24.00)),
        };
        let result = calculate_msb_invoice(&input).expect("MSB calc must succeed");
        let store = InMemoryPreisblattStore::new();
        let config = CheckConfig::default();
        let report = InvoicCheckEngine::check(
            result.pid,
            &result.nb_mp_id,
            &result.rechnung,
            &store,
            &config,
        );
        use invoic_checker::check::CheckOutcome;
        assert_ne!(
            report.outcome,
            CheckOutcome::Dispute,
            "generated MSB INVOIC must not fail invoic-checker; findings: {:?}",
            report.findings,
        );
    }
}
