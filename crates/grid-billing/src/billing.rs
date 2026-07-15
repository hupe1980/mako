//! NNE, MMM, and MSB settlement calculation logic.
//!
//! All monetary arithmetic uses [`billing::EuroAmount`] internally for exact
//! representation.  Functions return [`GridSettlement`] — a pure domain type
//! with no BO4E coupling.  The service layer (netzbilanzd / invoicd) converts
//! `GridSettlement` to `rubo4e::current::Rechnung` via a local `into_rechnung()`
//! helper, keeping BO4E as a service-layer concern.
//!
//! ## Explainability
//!
//! Every position carries a [`CalculationTrace`] that answers *"why is this
//! amount here?"* with:
//! - input values (quantity, unit price before rounding)
//! - gross intermediate result
//! - applicable [`LegalReference`]s (e.g. `StromNEV §17`, `KAV §2`)
//! - the [`TariffSource`] used
//! - any regulatory reduction factor
//!
//! This enables AI-assisted invoice explainability and regulator audits without
//! re-running the calculation.

use billing::EuroAmount;
use rust_decimal::Decimal;

use crate::error::BillingError;
use crate::types::{
    CalculationTrace, GridSettlement, InvoicePosition, KaKlasse, LegalReference, MmmInput,
    MsbInput, NneInput, QuantityUnit, SettlementStatus, SettlementType, SettlementWarning, Sparte,
    TariffSource, WarningSeverity,
};

// ── helpers ───────────────────────────────────────────────────────────────────

const HUNDRED: Decimal = Decimal::from_parts(100, 0, 0, false, 0);

fn ct_to_eur(ct: Decimal) -> Decimal {
    ct / HUNDRED
}

fn pos_net(qty: Decimal, unit_price_eur: Decimal) -> Decimal {
    (qty * unit_price_eur).round_dp(5)
}

fn kwh_pos_traced(
    number: u32,
    text: &str,
    kwh: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> InvoicePosition {
    let gross_eur = kwh * unit_price_eur;
    InvoicePosition {
        number,
        text: text.to_owned(),
        quantity: kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kwh, unit_price_eur),
        trace: CalculationTrace {
            explanation: format!(
                "{kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR",
                unit_price_eur,
                gross_eur.round_dp(5)
            ),
            input_quantity: kwh,
            input_unit_price_eur: unit_price_eur,
            gross_eur,
            legal_refs,
            tariff_source,
            regulatory_reduction_factor: None,
            rounding_note: Some("quantity rounded to 3 dp; unit price to 6 dp; net to 5 dp"),
        },
    }
}

fn kw_pos_traced(
    number: u32,
    text: &str,
    kw: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> InvoicePosition {
    let gross_eur = kw * unit_price_eur;
    InvoicePosition {
        number,
        text: text.to_owned(),
        quantity: kw.round_dp(3),
        unit: QuantityUnit::Kw,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kw, unit_price_eur),
        trace: CalculationTrace {
            explanation: format!(
                "{kw:.3} kW × {:.6} EUR/kW = {:.5} EUR",
                unit_price_eur,
                gross_eur.round_dp(5)
            ),
            input_quantity: kw,
            input_unit_price_eur: unit_price_eur,
            gross_eur,
            legal_refs,
            tariff_source,
            regulatory_reduction_factor: None,
            rounding_note: Some("quantity rounded to 3 dp; unit price to 6 dp; net to 5 dp"),
        },
    }
}

fn monat_pos_traced(
    number: u32,
    text: &str,
    months: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> InvoicePosition {
    let gross_eur = months * unit_price_eur;
    InvoicePosition {
        number,
        text: text.to_owned(),
        quantity: months.round_dp(3),
        unit: QuantityUnit::Monat,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(months, unit_price_eur),
        trace: CalculationTrace {
            explanation: format!(
                "{months} Monate × {:.6} EUR/Monat = {:.5} EUR",
                unit_price_eur,
                gross_eur.round_dp(5)
            ),
            input_quantity: months,
            input_unit_price_eur: unit_price_eur,
            gross_eur,
            legal_refs,
            tariff_source,
            regulatory_reduction_factor: None,
            rounding_note: Some("quantity rounded to 3 dp; unit price to 6 dp; net to 5 dp"),
        },
    }
}

fn decimal_to_euro_amount(d: Decimal) -> Result<EuroAmount, BillingError> {
    EuroAmount::checked_from_decimal(d).map_err(|_| BillingError::MonetaryOverflow {
        input_value: Some(d),
    })
}

fn make_tariff_source(sheet_id: Option<&str>) -> Option<TariffSource> {
    sheet_id.map(|id| TariffSource::PublishedTariffSheet {
        sheet_id: id.to_owned(),
    })
}

// ── NNE invoice (PID 31001 / 31005 / 31006) ──────────────────────────────────

/// Calculate a NNE settlement (PID 31001 Strom, 31005 Gas, 31006 selbstausstellt).
///
/// Returns a [`GridSettlement`] with full [`CalculationTrace`] per position
/// and applicable [`LegalReference`]s. The service layer converts this to
/// BO4E `Rechnung` and validates via `invoic-checker`.
///
/// ## Positions
///
/// | # | Description | Condition |
/// |---|---|---|
/// | 1 | Netznutzung Arbeit | flat mode |
/// | 1+2 | Arbeit HT + NT (§14a Modul 2) | ToU mode (BK6-22-300) |
/// | next | Netznutzung Leistung (StromNEV §17) | RLM only |
/// | last | Konzessionsabgabe (KAV §2) | when `ka_satz_ct_per_kwh` set |
///
/// ## Legal references
///
/// - Arbeit positions → `StromNEV §21` (or `GasNEV §14` for Gas)
/// - §14a ToU positions → `Sect14aEnwg { module: 2 }` + `BNetzA BK6-22-300`
/// - Leistung position → `StromNEV §17`
/// - Konzessionsabgabe → `KAV §2 Abs. 2`
///
/// ## Errors
///
/// [`BillingError::InvalidInput`] or [`BillingError::MonetaryOverflow`].
#[must_use = "handle the BillingError"]
pub fn calculate_nne_invoice(input: &NneInput) -> Result<GridSettlement, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.arbeitsmenge_kwh < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "arbeitsmenge_kwh must be non-negative".to_owned(),
        });
    }
    if input.spitzenleistung_kw.is_some() != input.leistungspreis_eur_per_kw.is_some() {
        return Err(BillingError::InvalidInput {
            reason:
                "spitzenleistung_kw and leistungspreis_eur_per_kw must both be set or both absent"
                    .to_owned(),
        });
    }

    let tariff_src = make_tariff_source(input.tariff_sheet_id.as_deref());
    let mut positions: Vec<InvoicePosition> = Vec::new();
    let mut total = Decimal::ZERO;
    let mut next: u32 = 1;
    let mut warnings: Vec<SettlementWarning> = Vec::new();

    // Sparte determines settlement type and Arbeit legal reference
    let (settlement_type, arbeit_ref) = match input.sparte {
        Sparte::Gas => (
            SettlementType::NneGas,
            LegalReference::GasNev { paragraph: "§14" },
        ),
        Sparte::Strom => (
            SettlementType::NneStrom,
            LegalReference::StromNev { paragraph: "§21" },
        ),
    };

    // §14a Modul 2 ToU or flat Arbeit
    let has_tou = input.arbeitsmenge_ht_kwh.is_some()
        && input.arbeitspreis_ht_ct_per_kwh.is_some()
        && input.arbeitsmenge_nt_kwh.is_some()
        && input.arbeitspreis_nt_ct_per_kwh.is_some();

    if has_tou {
        let ht_kwh = input.arbeitsmenge_ht_kwh.unwrap();
        let ht_eur = ct_to_eur(input.arbeitspreis_ht_ct_per_kwh.unwrap());
        let p = kwh_pos_traced(
            next,
            "Netznutzung Arbeit HT (§14a Modul 2)",
            ht_kwh,
            ht_eur,
            vec![
                LegalReference::Sect14aEnwg { module: 2 },
                LegalReference::BnetzaDecision {
                    reference: "BK6-22-300",
                },
                arbeit_ref.clone(),
            ],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
        next += 1;

        let nt_kwh = input.arbeitsmenge_nt_kwh.unwrap();
        let nt_eur = ct_to_eur(input.arbeitspreis_nt_ct_per_kwh.unwrap());
        let p = kwh_pos_traced(
            next,
            "Netznutzung Arbeit NT (§14a Modul 2)",
            nt_kwh,
            nt_eur,
            vec![
                LegalReference::Sect14aEnwg { module: 2 },
                LegalReference::BnetzaDecision {
                    reference: "BK6-22-300",
                },
                arbeit_ref.clone(),
            ],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
        next += 1;
    } else {
        let eur = ct_to_eur(input.arbeitspreis_ct_per_kwh);
        let p = kwh_pos_traced(
            next,
            "Netznutzung Arbeit",
            input.arbeitsmenge_kwh,
            eur,
            vec![arbeit_ref.clone()],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
        next += 1;
    }

    // Leistung (RLM only) — StromNEV §17
    if let (Some(sl_kw), Some(lp_eur)) = (input.spitzenleistung_kw, input.leistungspreis_eur_per_kw)
    {
        let p = kw_pos_traced(
            next,
            "Netznutzung Leistung",
            sl_kw,
            lp_eur,
            vec![LegalReference::StromNev { paragraph: "§17" }],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
        next += 1;
    }

    // Konzessionsabgabe (KAV §2 Abs. 2)
    let ka_base_kwh = if has_tou {
        input.arbeitsmenge_ht_kwh.unwrap_or(Decimal::ZERO)
            + input.arbeitsmenge_nt_kwh.unwrap_or(Decimal::ZERO)
    } else {
        input.arbeitsmenge_kwh
    };
    if let Some(ka_ct) = input.ka_satz_ct_per_kwh {
        if ka_ct < Decimal::ZERO {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "KA_NEGATIVE_RATE",
                message: format!("KA rate {ka_ct} ct/kWh is negative — verify tariff sheet"),
            });
        }
        let ka_klasse_note = input
            .ka_klasse
            .map(|k| match k {
                KaKlasse::TarifkundeLow => " (KAV §2 Tarif ≤25 MWh/a)",
                KaKlasse::TarifkundeMedium => " (KAV §2 Tarif ≪150 MWh/a)",
                KaKlasse::SonderkundeHigh => " (KAV §2 Sonderkunde)",
                KaKlasse::Exempt => " (KAV §2 Abs. 7 — freigestellt)",
            })
            .unwrap_or("");
        let p = InvoicePosition {
            number: next,
            text: format!("Konzessionsabgabe{ka_klasse_note}"),
            quantity: ka_base_kwh.round_dp(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: ct_to_eur(ka_ct).round_dp(6),
            net_eur: pos_net(ka_base_kwh, ct_to_eur(ka_ct)),
            trace: CalculationTrace {
                explanation: format!(
                    "{ka_base_kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR{ka_klasse_note}",
                    ct_to_eur(ka_ct),
                    (ka_base_kwh * ct_to_eur(ka_ct)).round_dp(5),
                ),
                input_quantity: ka_base_kwh,
                input_unit_price_eur: ct_to_eur(ka_ct),
                gross_eur: ka_base_kwh * ct_to_eur(ka_ct),
                legal_refs: vec![LegalReference::Kav {
                    paragraph: "§2 Abs. 2",
                }],
                tariff_source: tariff_src.clone(),
                regulatory_reduction_factor: None,
                rounding_note: Some("quantity rounded to 3 dp; unit price to 6 dp; net to 5 dp"),
            },
        };
        total += p.net_eur;
        positions.push(p);
    }

    let total_eur = total.round_dp(2);
    decimal_to_euro_amount(total_eur)?;

    Ok(GridSettlement {
        pid: settlement_type.default_pid(),
        settlement_type,
        status: SettlementStatus::Initial,
        rechnungsnummer: input.rechnungsnummer.clone(),
        correction_of: None,
        invoice_date: input.invoice_date,
        due_date: input.due_date,
        period_from: input.period_from,
        period_to: input.period_to,
        nb_mp_id: input.nb_mp_id.clone(),
        counterparty_mp_id: input.lf_mp_id.clone(),
        positions,
        total_eur,
        warnings,
    })
}

// ── MMM invoice (PID 31002) ───────────────────────────────────────────────────

/// Calculate a Mehr-/Mindermengen settlement invoice (PID 31002 Strom, Gas via GasNZV).
///
/// ## Legal references
///
/// - Strom: Mehrmengen/Mindermengen → `StromNZV §15`, `GPKE BK6-22-024`
/// - Gas: Mehrmengen/Mindermengen → `GasNZV §14`, `GeLi Gas BK7-24-01-009`
///
/// ## Errors
///
/// [`BillingError::InvalidInput`] when `period_from >= period_to`.
#[must_use = "handle the BillingError"]
pub fn calculate_mmm_invoice(input: &MmmInput) -> Result<GridSettlement, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }

    let mehr_eur = ct_to_eur(input.mehr_preis_ct_per_kwh);
    let minder_eur = ct_to_eur(input.minder_preis_ct_per_kwh);
    let diff = input.actual_kwh - input.profil_kwh;

    let mmm_refs = match input.sparte {
        Sparte::Gas => vec![
            LegalReference::GasNzv { paragraph: "§14" },
            LegalReference::BdewAhb {
                reference: "GeLi Gas BK7-24-01-009",
            },
        ],
        Sparte::Strom => vec![
            LegalReference::StromNzv { paragraph: "§15" },
            LegalReference::BdewAhb {
                reference: "GPKE BK6-22-024",
            },
        ],
    };
    let mmm_settlement_type = SettlementType::MmmStrom; // Gas MMM uses same invoice type

    let mehr_kwh = if diff > Decimal::ZERO {
        diff
    } else {
        Decimal::ZERO
    };
    let p1 = kwh_pos_traced(1, "Mehrmengen", mehr_kwh, mehr_eur, mmm_refs.clone(), None);

    let minder_kwh = if diff < Decimal::ZERO {
        -diff
    } else {
        Decimal::ZERO
    };
    let minder_net = -pos_net(minder_kwh, minder_eur);
    let minder_gross = minder_kwh * minder_eur;
    let p2 = InvoicePosition {
        number: 2,
        text: "Mindermengen (Gutschrift)".to_owned(),
        quantity: minder_kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: minder_eur.round_dp(6),
        net_eur: minder_net,
        trace: CalculationTrace {
            explanation: format!(
                "{minder_kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR (Gutschrift, negiert)",
                minder_eur,
                minder_gross.round_dp(5)
            ),
            input_quantity: minder_kwh,
            input_unit_price_eur: minder_eur,
            gross_eur: minder_gross,
            legal_refs: mmm_refs,
            tariff_source: None,
            regulatory_reduction_factor: None,
            rounding_note: Some("Mindermengen are credit positions — net_eur is negated"),
        },
    };

    let total_eur = (p1.net_eur + p2.net_eur).round_dp(2);
    decimal_to_euro_amount(total_eur.abs())?;

    Ok(GridSettlement {
        pid: mmm_settlement_type.default_pid(),
        settlement_type: mmm_settlement_type,
        status: SettlementStatus::Initial,
        rechnungsnummer: input.rechnungsnummer.clone(),
        correction_of: None,
        invoice_date: input.invoice_date,
        due_date: input.due_date,
        period_from: input.period_from,
        period_to: input.period_to,
        nb_mp_id: input.nb_mp_id.clone(),
        counterparty_mp_id: input.lf_mp_id.clone(),
        positions: vec![p1, p2],
        total_eur,
        warnings: Vec::new(),
    })
}

// ── MSB invoice (PID 31009) ───────────────────────────────────────────────────

/// Calculate a MSB-Rechnung (PID 31009): NB → MSB metering service settlement.
///
/// ## Legal references
///
/// - Grundgebühr Messstellenbetrieb → `MsbG §§6–7`, `MessZV §2`
/// - Messdienstleistung → `MessZV §2`
///
/// ## Errors
///
/// [`BillingError::InvalidInput`] or [`BillingError::MonetaryOverflow`].
#[must_use = "handle the BillingError"]
pub fn calculate_msb_invoice(input: &MsbInput) -> Result<GridSettlement, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.grundgebuehr_eur_per_month < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "grundgebuehr_eur_per_month must be non-negative".to_owned(),
        });
    }
    if input.billing_months == 0 {
        return Err(BillingError::InvalidInput {
            reason: "billing_months must be at least 1".to_owned(),
        });
    }

    let mut positions: Vec<InvoicePosition> = Vec::new();
    let mut total = Decimal::ZERO;

    let months = Decimal::from(input.billing_months);
    let p = monat_pos_traced(
        1,
        "Grundgebühr Messstellenbetrieb",
        months,
        input.grundgebuehr_eur_per_month,
        vec![
            LegalReference::MsbG {
                paragraph: "§§6–7"
            },
            LegalReference::MessZv { paragraph: "§2" },
        ],
        None,
    );
    total += p.net_eur;
    positions.push(p);

    if let Some(msl_eur) = input.messdienstleistung_eur {
        let msl: Decimal = msl_eur.round_dp(5);
        let p = InvoicePosition {
            number: 2,
            text: "Messdienstleistung".to_owned(),
            quantity: Decimal::ONE,
            unit: QuantityUnit::Monat,
            unit_price_eur: msl,
            net_eur: msl,
            trace: CalculationTrace {
                explanation: format!("Messdienstleistung Pauschale {msl:.5} EUR"),
                input_quantity: Decimal::ONE,
                input_unit_price_eur: msl,
                gross_eur: msl,
                legal_refs: vec![LegalReference::MessZv { paragraph: "§2" }],
                tariff_source: None,
                regulatory_reduction_factor: None,
                rounding_note: Some("flat fee — rounded to 5 dp"),
            },
        };
        total += p.net_eur;
        positions.push(p);
    }

    let total_eur = total.round_dp(2);
    decimal_to_euro_amount(total_eur)?;

    Ok(GridSettlement {
        pid: SettlementType::MsbRechnung.default_pid(),
        settlement_type: SettlementType::MsbRechnung,
        status: SettlementStatus::Initial,
        rechnungsnummer: input.rechnungsnummer.clone(),
        correction_of: None,
        invoice_date: input.invoice_date,
        due_date: input.due_date,
        period_from: input.period_from,
        period_to: input.period_to,
        nb_mp_id: input.nb_mp_id.clone(),
        counterparty_mp_id: input.msb_mp_id.clone(),
        positions,
        total_eur,
        warnings: Vec::new(),
    })
}

// ── Reversal (Stornorechnung) ─────────────────────────────────────────────────────────

/// Create a reversal (Stornorechnung) of a prior settlement.
///
/// All positions are negated. The result references the original via
/// `correction_of`. No re-calculation is performed — the reversal is
/// a pure mirror of the original, ensuring auditability.
///
/// ## Usage
///
/// ```rust,no_run
/// # use grid_billing::{GridSettlement, calculate_reversal};
/// # use time::macros::date;
/// # let original: GridSettlement = unimplemented!();
/// let reversal = calculate_reversal(
///     &original,
///     "STORNO-NNE-2025-001".to_owned(),
///     date!(2025-03-01),
///     date!(2025-03-31),
/// );
/// assert_eq!(reversal.total_eur, -original.total_eur);
/// ```
#[must_use]
pub fn calculate_reversal(
    original: &GridSettlement,
    new_rechnungsnummer: String,
    invoice_date: time::Date,
    due_date: time::Date,
) -> GridSettlement {
    use crate::types::SettlementStatus;
    let reversed_positions: Vec<_> = original
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| InvoicePosition {
            number: (i + 1) as u32,
            text: format!("Storno: {}", p.text),
            quantity: p.quantity,
            unit: p.unit,
            unit_price_eur: p.unit_price_eur,
            net_eur: -p.net_eur,
            trace: CalculationTrace {
                explanation: format!(
                    "Storno of position {}: {} (negated)",
                    p.number, p.trace.explanation
                ),
                input_quantity: p.trace.input_quantity,
                input_unit_price_eur: p.trace.input_unit_price_eur,
                gross_eur: -p.trace.gross_eur,
                legal_refs: p.trace.legal_refs.clone(),
                tariff_source: p.trace.tariff_source.clone(),
                regulatory_reduction_factor: p.trace.regulatory_reduction_factor,
                rounding_note: Some("reversal — all amounts negated"),
            },
        })
        .collect();

    GridSettlement {
        pid: original.pid,
        settlement_type: original.settlement_type,
        status: SettlementStatus::Reversal,
        rechnungsnummer: new_rechnungsnummer,
        correction_of: Some(original.rechnungsnummer.clone()),
        invoice_date,
        due_date,
        period_from: original.period_from,
        period_to: original.period_to,
        nb_mp_id: original.nb_mp_id.clone(),
        counterparty_mp_id: original.counterparty_mp_id.clone(),
        positions: reversed_positions,
        total_eur: -original.total_eur,
        warnings: Vec::new(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{validate_mmm_input, validate_msb_input, validate_nne_input};
    use rust_decimal::Decimal;
    use time::macros::date;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("valid decimal literal")
    }

    fn base_nne() -> NneInput {
        NneInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "NNE-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            arbeitsmenge_kwh: d("1500"),
            arbeitspreis_ct_per_kwh: d("3.5"),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
            tariff_sheet_id: None,
            sparte: Sparte::Strom,
            ka_klasse: None,
        }
    }

    fn base_mmm() -> MmmInput {
        MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "MMM-TEST-BASE".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            sparte: Sparte::Strom,
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        }
    }

    #[test]
    fn nne_slp_no_ka_arithmetic() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        assert_eq!(r.total_eur, d("52.50"));
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].unit, QuantityUnit::Kwh);
        assert_eq!(r.positions[0].net_eur, d("52.50000"));
    }

    #[test]
    fn nne_slp_with_ka() {
        let mut i = base_nne();
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.total_eur, d("54.15"));
        assert_eq!(r.positions.len(), 2);
        assert_eq!(r.positions[1].text, "Konzessionsabgabe");
    }

    #[test]
    fn nne_rlm_with_leistungspreis() {
        let mut i = base_nne();
        i.spitzenleistung_kw = Some(d("12.5"));
        i.leistungspreis_eur_per_kw = Some(d("4.20"));
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.total_eur, d("106.65"));
        assert_eq!(r.positions.len(), 3);
        assert_eq!(r.positions[1].unit, QuantityUnit::Kw);
    }

    #[test]
    fn nne_sect14a_tou_arithmetic() {
        let mut i = base_nne();
        i.arbeitsmenge_ht_kwh = Some(d("900"));
        i.arbeitspreis_ht_ct_per_kwh = Some(d("4.0"));
        i.arbeitsmenge_nt_kwh = Some(d("600"));
        i.arbeitspreis_nt_ct_per_kwh = Some(d("2.0"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.total_eur, d("48.00"));
        assert_eq!(r.positions.len(), 2);
        assert_eq!(r.positions[0].text, "Netznutzung Arbeit HT (§14a Modul 2)");
        assert_eq!(r.positions[0].net_eur, d("36.00000"));
        assert_eq!(r.positions[1].net_eur, d("12.00000"));
    }

    #[test]
    fn nne_invalid_period() {
        let mut i = base_nne();
        i.period_to = i.period_from;
        assert!(matches!(
            calculate_nne_invoice(&i),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    #[test]
    fn nne_mismatched_rlm_fields() {
        let mut i = base_nne();
        i.spitzenleistung_kw = Some(d("10"));
        assert!(matches!(
            calculate_nne_invoice(&i),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    #[test]
    fn mmm_mehr_settlement() {
        let input = MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "MMM-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            sparte: Sparte::Strom,
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let r = calculate_mmm_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("4.00"));
        assert_eq!(r.positions[1].net_eur, Decimal::ZERO);
    }

    #[test]
    fn mmm_minder_credit() {
        let input = MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "MMM-TEST-002".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            sparte: Sparte::Strom,
            actual_kwh: d("1400"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let r = calculate_mmm_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("-2.00"));
        assert_eq!(r.positions[1].net_eur, d("-2.00000"));
    }

    #[test]
    fn msb_grundgebuehr_only() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            rechnungsnummer: "MSB-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 1,
            messdienstleistung_eur: None,
        };
        let r = calculate_msb_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("12.50"));
        assert_eq!(r.pid, 31009);
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].unit, QuantityUnit::Monat);
    }

    #[test]
    fn msb_with_messdienstleistung() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            rechnungsnummer: "MSB-TEST-002".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 03 - 31),
            invoice_date: date!(2025 - 04 - 15),
            due_date: date!(2025 - 05 - 15),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 3,
            messdienstleistung_eur: Some(d("8.00")),
        };
        let r = calculate_msb_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("45.50"));
        assert_eq!(r.positions.len(), 2);
    }

    #[test]
    fn pid_is_mutable_for_overrides() {
        let mut r = calculate_nne_invoice(&base_nne()).unwrap();
        r.pid = 31005;
        assert_eq!(r.pid, 31005);
    }

    // ── New: explainability and audit trail tests ─────────────────────────────

    #[test]
    fn nne_slp_has_legal_reference_stromnev() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("StromNEV")),
            "expected StromNEV reference, got: {refs:?}"
        );
    }

    #[test]
    fn nne_ka_has_kav_reference() {
        let mut i = base_nne();
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let r = calculate_nne_invoice(&i).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("KAV")),
            "expected KAV reference, got: {refs:?}"
        );
    }

    #[test]
    fn nne_tou_has_sect14a_reference() {
        let mut i = base_nne();
        i.arbeitsmenge_ht_kwh = Some(d("900"));
        i.arbeitspreis_ht_ct_per_kwh = Some(d("4.0"));
        i.arbeitsmenge_nt_kwh = Some(d("600"));
        i.arbeitspreis_nt_ct_per_kwh = Some(d("2.0"));
        let r = calculate_nne_invoice(&i).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("§14a EnWG")),
            "expected §14a EnWG reference, got: {refs:?}"
        );
        assert!(
            refs.iter().any(|r| r.contains("BK6-22-300")),
            "expected BK6-22-300 reference, got: {refs:?}"
        );
    }

    #[test]
    fn mmm_has_strom_nzv_reference() {
        let mut input = base_mmm();
        input.rechnungsnummer = "MMM-TEST-REFS".into();
        let r = calculate_mmm_invoice(&input).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("StromNZV")),
            "expected StromNZV reference, got: {refs:?}"
        );
    }

    #[test]
    fn msb_has_msbg_reference() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            rechnungsnummer: "MSB-TEST-REFS".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 1,
            messdienstleistung_eur: None,
        };
        let r = calculate_msb_invoice(&input).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("MsbG")),
            "expected MsbG reference, got: {refs:?}"
        );
    }

    #[test]
    fn calculation_trace_explanation_non_empty() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        for pos in &r.positions {
            assert!(
                !pos.trace.explanation.is_empty(),
                "position {} has empty explanation",
                pos.number
            );
        }
    }

    #[test]
    fn settlement_type_and_status_set() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        assert_eq!(r.settlement_type, SettlementType::NneStrom);
        assert_eq!(r.status, SettlementStatus::Initial);
        assert!(r.correction_of.is_none());
    }

    #[test]
    fn recomputed_total_matches_total_eur() {
        let mut i = base_nne();
        i.spitzenleistung_kw = Some(d("12.5"));
        i.leistungspreis_eur_per_kw = Some(d("4.20"));
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(
            r.total_eur,
            r.recomputed_total(),
            "total_eur does not match sum of positions"
        );
    }

    #[test]
    fn tariff_sheet_id_propagates_to_traces() {
        let mut i = base_nne();
        i.tariff_sheet_id = Some("Preisblatt-NNE-2025-Q1".to_owned());
        let r = calculate_nne_invoice(&i).unwrap();
        for pos in &r.positions {
            if pos.text != "Konzessionsabgabe" {
                assert!(
                    pos.trace.tariff_source.is_some(),
                    "position '{}' should have a tariff source",
                    pos.text
                );
            }
        }
    }

    #[test]
    fn nne_negative_zero_nt_does_not_panic() {
        // Guard: zero consumption in one ToU band must produce zero position, not NaN
        let mut i = base_nne();
        i.arbeitsmenge_ht_kwh = Some(d("1500"));
        i.arbeitspreis_ht_ct_per_kwh = Some(d("4.0"));
        i.arbeitsmenge_nt_kwh = Some(d("0"));
        i.arbeitspreis_nt_ct_per_kwh = Some(d("2.0"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.positions[1].net_eur, Decimal::ZERO);
    }

    #[test]
    fn settlement_is_clean_with_valid_inputs() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        assert!(r.is_clean(), "clean NNE should have no warnings");
    }

    #[test]
    fn legal_reference_citations_non_empty() {
        for lr in [
            LegalReference::StromNev { paragraph: "§17" },
            LegalReference::GasNev { paragraph: "§14" },
            LegalReference::Kav {
                paragraph: "§2 Abs. 2",
            },
            LegalReference::Sect14aEnwg { module: 2 },
            LegalReference::MessZv { paragraph: "§2" },
            LegalReference::MsbG {
                paragraph: "§§6–7"
            },
            LegalReference::BnetzaDecision {
                reference: "BK6-22-300",
            },
            LegalReference::BdewAhb {
                reference: "GPKE BK6-22-024",
            },
            LegalReference::StromNzv { paragraph: "§15" },
            LegalReference::GasNzv { paragraph: "§14" },
            LegalReference::Enwg { paragraph: "§14a" },
            LegalReference::ARegV { paragraph: "§17" },
        ] {
            assert!(!lr.citation().is_empty());
        }
    }

    #[test]
    fn settlement_type_default_pids() {
        assert_eq!(SettlementType::NneStrom.default_pid(), 31001);
        assert_eq!(SettlementType::NneGas.default_pid(), 31005);
        assert_eq!(SettlementType::NneSelbstausstellt.default_pid(), 31006);
        assert_eq!(SettlementType::MmmStrom.default_pid(), 31002);
        assert_eq!(SettlementType::MsbRechnung.default_pid(), 31009);
        assert_eq!(SettlementType::GasAwhSperrung.default_pid(), 31011);
    }

    // ── New: sparte, counterparty_mp_id, reversal, Gas path, KaKlasse, validation ──

    #[test]
    fn nne_gas_sparte_sets_gas_type_and_ref() {
        let mut i = base_nne();
        i.sparte = Sparte::Gas;
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.settlement_type, SettlementType::NneGas);
        assert_eq!(r.pid, 31005);
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("GasNEV")),
            "Gas NNE must cite GasNEV, got: {refs:?}"
        );
        assert!(
            !refs.iter().any(|r| r.contains("StromNEV")),
            "Gas NNE must not cite StromNEV, got: {refs:?}"
        );
    }

    #[test]
    fn counterparty_mp_id_is_populated_for_nne() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        assert_eq!(r.counterparty_mp_id, "9900012345678");
    }

    #[test]
    fn counterparty_mp_id_is_msb_for_msb_invoice() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900999000001".into(),
            rechnungsnummer: "MSB-CMP-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            grundgebuehr_eur_per_month: d("15.00"),
            billing_months: 1,
            messdienstleistung_eur: None,
        };
        let r = calculate_msb_invoice(&input).unwrap();
        assert_eq!(r.counterparty_mp_id, "9900999000001");
    }

    #[test]
    fn reversal_negates_all_positions_and_total() {
        let original = calculate_nne_invoice(&base_nne()).unwrap();
        let storno = calculate_reversal(
            &original,
            "STORNO-NNE-TEST-001".to_owned(),
            date!(2025 - 03 - 01),
            date!(2025 - 03 - 31),
        );
        assert_eq!(storno.total_eur, -original.total_eur);
        assert_eq!(storno.status, SettlementStatus::Reversal);
        assert_eq!(storno.correction_of.as_deref(), Some("NNE-TEST-001"));
        for (orig, rev) in original.positions.iter().zip(storno.positions.iter()) {
            assert_eq!(rev.net_eur, -orig.net_eur);
            assert!(rev.text.starts_with("Storno:"));
        }
    }

    #[test]
    fn reversal_preserves_counterparty_mp_id() {
        let original = calculate_nne_invoice(&base_nne()).unwrap();
        let storno = calculate_reversal(
            &original,
            "STORNO-NNE-TEST-002".to_owned(),
            date!(2025 - 03 - 01),
            date!(2025 - 03 - 31),
        );
        assert_eq!(storno.counterparty_mp_id, original.counterparty_mp_id);
    }

    #[test]
    fn ka_klasse_annotation_appears_in_position_text() {
        let mut i = base_nne();
        i.ka_satz_ct_per_kwh = Some(d("0.13"));
        i.ka_klasse = Some(KaKlasse::TarifkundeLow);
        let r = calculate_nne_invoice(&i).unwrap();
        let ka_pos = r
            .positions
            .iter()
            .find(|p| p.text.contains("Konzessionsabgabe"))
            .unwrap();
        assert!(
            ka_pos.text.contains("KAV"),
            "KaKlasse annotation should appear in position text: {}",
            ka_pos.text
        );
    }

    #[test]
    fn gas_mmm_uses_gasnzv_reference() {
        let mut i = base_mmm();
        i.sparte = Sparte::Gas;
        let r = calculate_mmm_invoice(&i).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("GasNZV")),
            "Gas MMM must cite GasNZV, got: {refs:?}"
        );
        assert!(
            !refs.iter().any(|r| r.contains("StromNZV")),
            "Gas MMM must not cite StromNZV, got: {refs:?}"
        );
    }

    #[test]
    fn validate_nne_input_catches_invalid_period() {
        let mut i = base_nne();
        i.period_to = i.period_from;
        let v = validate_nne_input(&i);
        assert!(!v.is_valid);
        assert!(v.warnings.iter().any(|w| w.code == "INVALID_PERIOD"));
    }

    #[test]
    fn validate_nne_input_clean_on_valid_input() {
        let v = validate_nne_input(&base_nne());
        assert!(v.is_valid);
        assert!(v.warnings.is_empty());
    }

    #[test]
    fn validate_mmm_input_catches_invalid_period() {
        let mut i = base_mmm();
        i.period_to = i.period_from;
        let v = validate_mmm_input(&i);
        assert!(!v.is_valid);
        assert!(v.warnings.iter().any(|w| w.code == "INVALID_PERIOD"));
    }

    #[test]
    fn validate_msb_zero_months_is_error() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            rechnungsnummer: "MSB-VAL-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 0,
            messdienstleistung_eur: None,
        };
        let v = validate_msb_input(&input);
        assert!(!v.is_valid);
        assert!(v.warnings.iter().any(|w| w.code == "ZERO_BILLING_MONTHS"));
    }

    #[test]
    fn reversal_of_rlm_matches_negative_total() {
        let mut i = base_nne();
        i.spitzenleistung_kw = Some(d("12.5"));
        i.leistungspreis_eur_per_kw = Some(d("4.20"));
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let original = calculate_nne_invoice(&i).unwrap();
        let storno = calculate_reversal(
            &original,
            "STORNO-RLM-001".to_owned(),
            date!(2025 - 03 - 01),
            date!(2025 - 03 - 31),
        );
        assert_eq!(storno.positions.len(), original.positions.len());
        assert_eq!(storno.total_eur, -original.total_eur);
        assert_eq!(storno.recomputed_total(), storno.total_eur);
    }
}
