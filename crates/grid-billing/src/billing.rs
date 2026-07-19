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
    BillingPositionKind, CalculationTrace, GasAwhInput, GridSettlement, InvoicePosition,
    KaKundengruppe, LegalReference, MmmInput, MsbInput, NneInput, QuantityUnit, Sect14aModule,
    SettlementStatus, SettlementType, SettlementWarning, Sparte, TariffSource, WarningSeverity,
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
    kind: BillingPositionKind,
    kwh: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> InvoicePosition {
    let gross_eur = kwh * unit_price_eur;
    InvoicePosition {
        number,
        text: text.to_owned(),
        kind,
        quantity: kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kwh, unit_price_eur),
        artikel_id: None,
        lastvariable_preisposition_json: None,
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
    kind: BillingPositionKind,
    kw: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> InvoicePosition {
    let gross_eur = kw * unit_price_eur;
    InvoicePosition {
        number,
        text: text.to_owned(),
        kind,
        quantity: kw.round_dp(3),
        unit: QuantityUnit::Kw,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kw, unit_price_eur),
        artikel_id: None,
        lastvariable_preisposition_json: None,
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
    kind: BillingPositionKind,
    months: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> InvoicePosition {
    let gross_eur = months * unit_price_eur;
    InvoicePosition {
        number,
        text: text.to_owned(),
        kind,
        quantity: months.round_dp(3),
        unit: QuantityUnit::Monat,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(months, unit_price_eur),
        artikel_id: None,
        lastvariable_preisposition_json: None,
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
/// ## Positions (in order)
///
/// | # | Description | Condition |
/// |---|---|---|
/// | 1 | Gas Grundpreis (Verrechnungspreis) | when `nne_grundpreis_eur_per_month` set (Gas only) |
/// | next | Netznutzung Arbeit (§14a Modul 1 reduced) | Modul 1 flat reduction mode |
/// | next | Netznutzung Arbeit HT + NT (§14a Modul 2) | ToU mode (BK6-22-300) |
/// | next | Netznutzung Arbeit | flat mode (no §14a) |
/// | next | Netznutzung Leistung (StromNEV §17) | RLM only |
/// | last | Konzessionsabgabe (KAV §2) | when `ka_satz_ct_per_kwh` set |
///
/// ## Legal references
///
/// - Gas Grundpreis position → `GasNEV §14`
/// - Arbeit positions → `StromNEV §21` (or `GasNEV §14` for Gas)
/// - §14a Modul 1 positions → `Sect14aEnwg { module: Modul1 }` + `BNetzA BK6-22-300`
/// - §14a ToU positions → `Sect14aEnwg { module: Modul2 }` + `BNetzA BK6-22-300`
/// - Leistung position → `StromNEV §17`
/// - Konzessionsabgabe → `KAV §2 Abs. 2`
///
/// ## §14a Modul 1 flat reduction
///
/// Set `sect14a_modul1_reduction_factor = Some(dec!(0.85))` to apply the BNetzA
/// BK6-22-300 Modul 1 15 % discount: the Arbeitspreis is multiplied by the factor
/// before billing. The trace carries `regulatory_reduction_factor` for auditability.
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

    // Gas Grundpreis / Verrechnungspreis (Gas NNE monthly standing charge per GasNEV)
    if let (Some(gp_eur), Some(gp_months)) = (
        input.nne_grundpreis_eur_per_month,
        input.nne_grundpreis_months,
    ) && gp_months > 0
    {
        let months = Decimal::from(gp_months);
        let p = monat_pos_traced(
            next,
            "Netzentgelt Grundpreis Gas (Verrechnungspreis)",
            BillingPositionKind::NneGasGrundpreis,
            months,
            gp_eur,
            vec![LegalReference::GasNev { paragraph: "§14" }],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
        next += 1;
    }

    // §14a Modul 2 ToU or §14a Modul 1 flat reduction or plain Arbeit
    let has_tou = input.arbeitsmenge_ht_kwh.is_some()
        && input.arbeitspreis_ht_ct_per_kwh.is_some()
        && input.arbeitsmenge_nt_kwh.is_some()
        && input.arbeitspreis_nt_ct_per_kwh.is_some();
    let has_modul1 = input.sect14a_modul1_reduction_factor.is_some();

    if has_tou {
        let ht_kwh = input.arbeitsmenge_ht_kwh.unwrap();
        let ht_eur = ct_to_eur(input.arbeitspreis_ht_ct_per_kwh.unwrap());
        let p = kwh_pos_traced(
            next,
            "Netznutzung Arbeit HT (§14a Modul 2)",
            BillingPositionKind::NneArbeitHt,
            ht_kwh,
            ht_eur,
            vec![
                LegalReference::Sect14aEnwg {
                    module: Sect14aModule::Modul2,
                },
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
            BillingPositionKind::NneArbeitNt,
            nt_kwh,
            nt_eur,
            vec![
                LegalReference::Sect14aEnwg {
                    module: Sect14aModule::Modul2,
                },
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
    } else if has_modul1 {
        // §14a Modul 1: apply flat percentage reduction to the Arbeitspreis.
        // BK6-22-300 Anlage 2 default = 85 % (customer pays 85 % of full rate).
        let factor = input.sect14a_modul1_reduction_factor.unwrap();
        let base_eur = ct_to_eur(input.arbeitspreis_ct_per_kwh);
        let reduced_eur = (base_eur * factor).round_dp(6);
        let gross = input.arbeitsmenge_kwh * reduced_eur;
        let p = InvoicePosition {
            number: next,
            text: format!(
                "Netznutzung Arbeit §14a Modul 1 ({:.0}% Reduzierung)",
                factor * HUNDRED
            ),
            kind: BillingPositionKind::NneArbeitModul1,
            quantity: input.arbeitsmenge_kwh.round_dp(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: reduced_eur,
            net_eur: pos_net(input.arbeitsmenge_kwh, reduced_eur),
            artikel_id: None,
            lastvariable_preisposition_json: None,
            trace: CalculationTrace {
                explanation: format!(
                    "{:.3} kWh × {:.6} EUR/kWh (= {:.6} × {factor} Modul 1) = {:.5} EUR",
                    input.arbeitsmenge_kwh,
                    reduced_eur,
                    base_eur,
                    gross.round_dp(5)
                ),
                input_quantity: input.arbeitsmenge_kwh,
                input_unit_price_eur: reduced_eur,
                gross_eur: gross,
                legal_refs: vec![
                    LegalReference::Sect14aEnwg {
                        module: Sect14aModule::Modul1,
                    },
                    LegalReference::BnetzaDecision {
                        reference: "BK6-22-300",
                    },
                    arbeit_ref.clone(),
                ],
                tariff_source: tariff_src.clone(),
                regulatory_reduction_factor: Some(factor),
                rounding_note: Some("reduced unit price rounded to 6 dp per BK6-22-300"),
            },
        };
        total += p.net_eur;
        positions.push(p);
        next += 1;
    } else {
        let eur = ct_to_eur(input.arbeitspreis_ct_per_kwh);
        let p = kwh_pos_traced(
            next,
            "Netznutzung Arbeit",
            BillingPositionKind::NneArbeit,
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
            BillingPositionKind::NneLeistung,
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
        // KAV §2 rates are Höchstbeträge, so a rate above the statutory ceiling
        // for the customer group is a compliance defect, not merely unusual.
        if let Some(gruppe) = input.ka_klasse {
            match gruppe.hoechstsatz_ct_per_kwh(input.sparte) {
                Some(max) if ka_ct > max => warnings.push(SettlementWarning {
                    severity: WarningSeverity::Warning,
                    code: "KA_ABOVE_KAV_MAXIMUM",
                    message: format!(
                        "KA rate {ka_ct} ct/kWh exceeds the KAV §2 Höchstbetrag {max} ct/kWh for {}",
                        gruppe.label()
                    ),
                }),
                None if gruppe == KaKundengruppe::Exempt && ka_ct > Decimal::ZERO => {
                    warnings.push(SettlementWarning {
                        severity: WarningSeverity::Warning,
                        code: "KA_CHARGED_WHILE_EXEMPT",
                        message: format!(
                            "KA rate {ka_ct} ct/kWh charged although the customer is \
                             freigestellt nach KAV §2 Abs. 7"
                        ),
                    });
                }
                _ => {}
            }
        }
        let ka_klasse_note = input
            .ka_klasse
            .map(|k| format!(" ({})", k.label()))
            .unwrap_or_default();
        let p = InvoicePosition {
            number: next,
            text: format!("Konzessionsabgabe{ka_klasse_note}"),
            kind: BillingPositionKind::Konzessionsabgabe,
            quantity: ka_base_kwh.round_dp(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: ct_to_eur(ka_ct).round_dp(6),
            net_eur: pos_net(ka_base_kwh, ct_to_eur(ka_ct)),
            artikel_id: None,
            lastvariable_preisposition_json: None,
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

    // ── §14a Modul 3: per-dispatch-interval Spotpreis-NNE ─────────────────────
    // BNetzA BK6-22-300 Anlage 2 §3: One position per 15-min dispatch interval.
    // The rate is pre-calculated by the caller from the spot-price formula in
    // `PreisblattNetznutzung.lastvariablePreispositionen`.
    // Each position carries a `LastvariablePreisposition` JSON for ERP validation.
    for (seq, interval) in input.sect14a_modul3_intervals.iter().enumerate() {
        if interval.menge_kwh <= Decimal::ZERO {
            continue; // skip zero-energy intervals (e.g. overnight no-load)
        }
        let rate_eur = ct_to_eur(interval.nne_rate_ct_per_kwh);
        let net = pos_net(interval.menge_kwh, rate_eur);

        use time::format_description::well_known::Rfc3339;
        let from_str = interval
            .period_from
            .format(&Rfc3339)
            .unwrap_or_else(|_| interval.period_from.to_string());
        let to_str = interval
            .period_to
            .format(&Rfc3339)
            .unwrap_or_else(|_| interval.period_to.to_string());

        let label = format!("§14a Modul 3 Spotpreis-NNE {from_str}–{to_str}");

        // Build typed LastvariablePreisposition JSON for ERP-side validation.
        // Format follows BO4E v202607 `LastvariablePreisposition` COM schema.
        // The service layer deserializes to `rubo4e::current::LastvariablePreisposition`.
        let mut lvp = serde_json::json!({
            "_typ":                   "LASTVARIABLE_PREISPOSITION",
            "bezeichnung":            label.as_str(),
            "preisreferenz":          "ENERGIEMENGE",
            "preisBezugseinheit":     "KWH",
            "preisWaehrungseinheit":  "CT",
            "tarifkalkulationsmethode": "SPOTPREIS",
            "preisstaffeln": [{
                "_typ":         "PREISSTAFFEL",
                "einheitspreis": interval.nne_rate_ct_per_kwh.to_string(),
                "staffelgrenzeVon": "0"
            }]
        });
        if let Some(epex) = interval.epex_spot_ct_per_kwh {
            lvp["zusatzAttribute"] = serde_json::json!([{
                "_typ":  "ZUSATZ_ATTRIBUT",
                "name":  "epexSpotCtPerKwh",
                "wert":  epex.to_string()
            }]);
        }

        let mut explanation = format!(
            "{:.3} kWh × {:.6} EUR/kWh (§14a Modul 3 Spotpreis, interval {}/{}) = {:.5} EUR",
            interval.menge_kwh, rate_eur, from_str, to_str, net
        );
        if let Some(epex) = interval.epex_spot_ct_per_kwh {
            explanation.push_str(&format!(" [EPEX {epex:.4} ct/kWh]"));
        }

        let p = InvoicePosition {
            number: next + seq as u32,
            text: label,
            kind: BillingPositionKind::NneArbeitModul3,
            quantity: interval.menge_kwh.round_dp(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: rate_eur.round_dp(6),
            net_eur: net,
            artikel_id: None,
            lastvariable_preisposition_json: Some(lvp),
            trace: CalculationTrace {
                explanation,
                input_quantity: interval.menge_kwh,
                input_unit_price_eur: rate_eur,
                gross_eur: interval.menge_kwh * rate_eur,
                legal_refs: vec![
                    LegalReference::Sect14aEnwg {
                        module: Sect14aModule::Modul3,
                    },
                    LegalReference::BnetzaDecision {
                        reference: "BK6-22-300",
                    },
                    arbeit_ref.clone(),
                ],
                tariff_source: tariff_src.clone(),
                regulatory_reduction_factor: None,
                rounding_note: Some("rate ct→EUR 6 dp; net 5 dp; BK6-22-300 Anlage 2 §3"),
            },
        };
        total += p.net_eur;
        positions.push(p);
    }
    if !input.sect14a_modul3_intervals.is_empty() {
        next += input.sect14a_modul3_intervals.len() as u32;
        let _ = next; // consumed; next seq number not used further
    }

    let total_eur = total.round_dp(2);
    decimal_to_euro_amount(total_eur)?;

    let result = GridSettlement {
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
    };
    debug_assert_eq!(
        result.total_eur,
        result.recomputed_total(),
        "NNE: total_eur mismatch — calculation bug"
    );
    Ok(result)
}

// ── MMM invoice (PID 31002) ───────────────────────────────────────────────────

/// Last day on which StromNZV and GasNZV applied.
///
/// Both ceased to have effect with the end of 31.12.2025 — Art. 15 Abs. 4 (Strom)
/// and Abs. 6 (Gas) of the Gesetz v. 22.12.2023, BGBl. 2023 I Nr. 405. From
/// 01.01.2026 the basis is §20 Abs. 3 EnWG plus the BNetzA Festlegungen.
const NZV_LAST_DAY: time::Date = time::macros::date!(2025 - 12 - 31);

/// Calculate a Mehr-/Mindermengen settlement invoice (PID 31002, Strom and Gas).
///
/// ## Legal references
///
/// Selected from the **delivery period**, because StromNZV and GasNZV both ceased
/// to apply with effect from the end of 31.12.2025:
///
/// | Period | Strom | Gas |
/// |---|---|---|
/// | to 31.12.2025 | StromNZV §13 Abs. 3 | GasNZV §25 |
/// | from 01.01.2026 | GPKE (BK6-24-174) Teil 1 Kap. 8.4 | GaBi Gas 2.1 (BK7-24-01-008) |
///
/// GeLi Gas 3.0 does **not** carry Mehr-/Mindermengen; its transferred scope is
/// Netzzugangsverträge, Lieferantenwechsel and Messung.
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

    // The NZVs applied to Lieferzeiträume ending on or before 31.12.2025.
    let pre_2026 = input.period_to <= NZV_LAST_DAY;
    let mmm_refs = match (input.sparte, pre_2026) {
        (Sparte::Gas, true) => vec![
            LegalReference::GasNzv { paragraph: "§25" },
            LegalReference::BdewAhb {
                reference: "GaBi Gas 2.1 (BK7-24-01-008)",
            },
        ],
        (Sparte::Gas, false) => vec![LegalReference::BdewAhb {
            reference: "GaBi Gas 2.1 (BK7-24-01-008)",
        }],
        (Sparte::Strom, true) => vec![
            LegalReference::StromNzv {
                paragraph: "§13 Abs. 3",
            },
            LegalReference::BnetzaDecision {
                reference: "BK6-24-174",
            },
        ],
        (Sparte::Strom, false) => vec![
            LegalReference::Enwg {
                paragraph: "§20 Abs. 3",
            },
            LegalReference::BnetzaDecision {
                reference: "BK6-24-174",
            },
        ],
    };
    // Gas and Strom MMM use separate settlement types for correct audit references.
    let mmm_settlement_type = match input.sparte {
        Sparte::Gas => SettlementType::MmmGas,
        Sparte::Strom => SettlementType::MmmStrom,
    };

    // Sign convention per GPKE (BK6-24-174) Teil 1 Kap. 8.4 Nr. 3 and, for gas,
    // GaBi Gas 2.1 (BK7-24-01-008) Tenor Nr. 5. Both define the quantities from
    // the network operator's side, which inverts the intuitive reading:
    //
    //   measured < profiled  → ungewollte **Mehrmenge**   → NB vergütet   (credit)
    //   measured > profiled  → ungewollte **Mindermenge** → NB in Rechnung (charge)
    //
    // GPKE: "Unterschreitet die Summe der [...] ermittelten elektrischen Arbeit
    // die Summe der Arbeit, die den bilanzierten Profilen zu Grunde gelegt wurde
    // (ungewollte Mehrmenge), so vergütet der Netzbetreiber dem Lieferanten [...]
    // diese Differenzmenge."
    let mehr_kwh = if diff < Decimal::ZERO {
        -diff
    } else {
        Decimal::ZERO
    };
    let mehr_net = -pos_net(mehr_kwh, mehr_eur);
    let mehr_gross = mehr_kwh * mehr_eur;
    let p1 = InvoicePosition {
        number: 1,
        text: "Mehrmengen (Gutschrift)".to_owned(),
        kind: BillingPositionKind::Mehrmenge,
        artikel_id: None,
        quantity: mehr_kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: mehr_eur.round_dp(6),
        net_eur: mehr_net,
        lastvariable_preisposition_json: None,
        trace: CalculationTrace {
            explanation: format!(
                "{mehr_kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR (Gutschrift, negiert)",
                mehr_eur,
                mehr_gross.round_dp(5)
            ),
            input_quantity: mehr_kwh,
            input_unit_price_eur: mehr_eur,
            gross_eur: mehr_gross,
            legal_refs: mmm_refs.clone(),
            tariff_source: None,
            regulatory_reduction_factor: None,
            rounding_note: Some("Mehrmengen are credit positions — net_eur is negated"),
        },
    };

    let minder_kwh = if diff > Decimal::ZERO {
        diff
    } else {
        Decimal::ZERO
    };
    let minder_net = pos_net(minder_kwh, minder_eur);
    let minder_gross = minder_kwh * minder_eur;
    let p2 = InvoicePosition {
        number: 2,
        text: "Mindermengen".to_owned(),
        kind: BillingPositionKind::Mindermenge,
        artikel_id: None,
        quantity: minder_kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: minder_eur.round_dp(6),
        net_eur: minder_net,
        lastvariable_preisposition_json: None,
        trace: CalculationTrace {
            explanation: format!(
                "{minder_kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR",
                minder_eur,
                minder_gross.round_dp(5)
            ),
            input_quantity: minder_kwh,
            input_unit_price_eur: minder_eur,
            gross_eur: minder_gross,
            legal_refs: mmm_refs,
            tariff_source: None,
            regulatory_reduction_factor: None,
            rounding_note: None,
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
        BillingPositionKind::MsbGrundgebuehr,
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
            kind: BillingPositionKind::Messdienstleistung,
            artikel_id: None,
            quantity: Decimal::ONE,
            unit: QuantityUnit::Monat,
            unit_price_eur: msl,
            net_eur: msl,
            lastvariable_preisposition_json: None,
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
            kind: p.kind,
            artikel_id: p.artikel_id.clone(),
            quantity: p.quantity,
            unit: p.unit,
            unit_price_eur: p.unit_price_eur,
            net_eur: -p.net_eur,
            lastvariable_preisposition_json: None,
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

// ── Correction ────────────────────────────────────────────────────────────────

/// Create a correction of a prior settlement by applying a new settlement.
///
/// Combines the original settlement (reversed) and the corrected calculation
/// into a correction-pair. Callers typically dispatch both the reversal and the
/// new settlement to the EDIFACT channel — `calculate_correction` returns both
/// in order so dispatch logic stays simple.
///
/// Returns `(reversal, replacement)` where:
/// - `reversal` negates all original positions and references the original invoice.
/// - `replacement` is the new calculation, with `status = Correction` and
///   `correction_of = Some(original.rechnungsnummer)`.
///
/// ## Example
///
/// ```rust,no_run
/// # use grid_billing::{GridSettlement, calculate_correction};
/// # use time::macros::date;
/// # let original: GridSettlement = unimplemented!();
/// # let corrected: GridSettlement = unimplemented!();
/// let (reversal, replacement) = calculate_correction(
///     &original,
///     corrected,
///     "STORNO-NNE-2025-001".to_owned(),
///     date!(2025-04-01),
///     date!(2025-04-30),
/// );
/// assert_eq!(reversal.total_eur, -original.total_eur);
/// assert_eq!(replacement.status, grid_billing::SettlementStatus::Correction);
/// ```
#[must_use]
pub fn calculate_correction(
    original: &GridSettlement,
    mut replacement: GridSettlement,
    reversal_rechnungsnummer: String,
    invoice_date: time::Date,
    due_date: time::Date,
) -> (GridSettlement, GridSettlement) {
    let reversal = calculate_reversal(original, reversal_rechnungsnummer, invoice_date, due_date);
    replacement.status = SettlementStatus::Correction;
    replacement.correction_of = Some(original.rechnungsnummer.clone());
    (reversal, replacement)
}

// ── GeLi Gas AWH Sperrprozesse invoice (PID 31011) ────────────────────────────

/// Calculate a GeLi Gas AWH Sperrprozesse settlement (PID 31011).
///
/// **Rechnung sonstige Leistung (NB → LF)** — bills the LF (LFG/LFA) for
/// abrechnungswürdige Handlungen (AWH) performed by the GNB/VNB during the
/// Sperrung/Entsperrung Gas process.
///
/// Governed by **BK7-24-01-009 §5.4** (GeLi Gas 3.0, Beschluss 12.09.2025).
///
/// ## Positions
///
/// One position per [`crate::AwhPositionInput`] — quantity = `anzahl` (unit: pieces → Monat
/// placeholder), unit price = `preis_eur`. Positions are self-explaining with the
/// action description in the `text` field.
///
/// ## Legal references
///
/// Every position cites:
/// - `BdewAhb { reference: "GeLi Gas 3.0 (BK7-24-01-009) §5.4" }` (governing ruling)
/// - `GasNev { paragraph: "§14" }` (general GasNEV charge authorisation)
///
/// ## Errors
///
/// [`BillingError::InvalidInput`] when:
/// - `period_from >= period_to`
/// - `awh_positionen` is empty
/// - Any position has `anzahl == 0` or `preis_eur < 0`
#[must_use = "handle the BillingError"]
pub fn calculate_gas_awh_invoice(input: &GasAwhInput) -> Result<GridSettlement, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.awh_positionen.is_empty() {
        return Err(BillingError::InvalidInput {
            reason: "awh_positionen must contain at least one position".to_owned(),
        });
    }
    for (i, awh) in input.awh_positionen.iter().enumerate() {
        if awh.anzahl == 0 {
            return Err(BillingError::InvalidInput {
                reason: format!("awh_positionen[{i}].anzahl must be ≥ 1"),
            });
        }
        if awh.preis_eur < Decimal::ZERO {
            return Err(BillingError::InvalidInput {
                reason: format!("awh_positionen[{i}].preis_eur must be non-negative"),
            });
        }
    }

    let tariff_src = make_tariff_source(input.tariff_sheet_id.as_deref());
    let awh_legal_refs = vec![
        LegalReference::BdewAhb {
            reference: "GeLi Gas 3.0 (BK7-24-01-009) §5.4",
        },
        LegalReference::GasNev { paragraph: "§14" },
    ];

    let mut positions: Vec<InvoicePosition> = Vec::new();
    let mut total = Decimal::ZERO;

    for (i, awh) in input.awh_positionen.iter().enumerate() {
        let qty = Decimal::from(awh.anzahl);
        let gross = qty * awh.preis_eur;
        let net = pos_net(qty, awh.preis_eur);
        positions.push(InvoicePosition {
            number: (i + 1) as u32,
            text: awh.beschreibung.clone(),
            kind: BillingPositionKind::GasAwhSonstige, // service layer refines if artikel_id present
            artikel_id: awh.artikel_id.clone(),
            quantity: qty,
            unit: QuantityUnit::Monat, // AWH positions have no standard EDIFACT unit; Monat placeholder
            unit_price_eur: awh.preis_eur.round_dp(6),
            net_eur: net,
            lastvariable_preisposition_json: None,
            trace: CalculationTrace {
                explanation: format!(
                    "{} × {:.5} EUR = {:.5} EUR",
                    awh.anzahl,
                    awh.preis_eur,
                    gross.round_dp(5)
                ),
                input_quantity: qty,
                input_unit_price_eur: awh.preis_eur,
                gross_eur: gross,
                legal_refs: awh_legal_refs.clone(),
                tariff_source: tariff_src.clone(),
                regulatory_reduction_factor: None,
                rounding_note: Some("net rounded to 5 dp"),
            },
        });
        total += net;
    }

    let total_eur = total.round_dp(2);
    decimal_to_euro_amount(total_eur)?;

    let result = GridSettlement {
        pid: SettlementType::GasAwhSperrung.default_pid(),
        settlement_type: SettlementType::GasAwhSperrung,
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
        warnings: Vec::new(),
    };
    debug_assert_eq!(
        result.total_eur,
        result.recomputed_total(),
        "AWH: total_eur mismatch — calculation bug"
    );
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AwhPositionInput, GasAwhInput, validate_mmm_input, validate_msb_input, validate_nne_input,
    };
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
            sect14a_modul1_reduction_factor: None,
            nne_grundpreis_eur_per_month: None,
            nne_grundpreis_months: None,
            tariff_sheet_id: None,
            sparte: Sparte::Strom,
            ka_klasse: None,
            sect14a_modul3_intervals: vec![],
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

    /// measured > profiled is an **ungewollte Mindermenge** — the NB supplied the
    /// shortfall and invoices it. GPKE (BK6-24-174) Teil 1 Kap. 8.4 Nr. 3.
    #[test]
    fn over_consumption_is_a_mindermenge_charge() {
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
        // 100 kWh over profile × 2.0 ct = 2.00 EUR charged at the Mindermengen price.
        assert_eq!(r.total_eur, d("2.00"));
        assert_eq!(
            r.positions[0].net_eur,
            Decimal::ZERO,
            "no Mehrmenge position"
        );
        assert_eq!(r.positions[1].quantity, d("100.000"));
    }

    /// measured < profiled is an **ungewollte Mehrmenge** — the NB took the
    /// surplus and reimburses it. GPKE (BK6-24-174) Teil 1 Kap. 8.4 Nr. 3:
    /// "so vergütet der Netzbetreiber dem Lieferanten [...] diese Differenzmenge".
    #[test]
    fn under_consumption_is_a_mehrmenge_credit() {
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
        // 100 kWh under profile × 4.0 ct = 4.00 EUR credited at the Mehrmengen price.
        assert_eq!(r.total_eur, d("-4.00"));
        assert_eq!(r.positions[0].net_eur, d("-4.00000"));
        assert_eq!(
            r.positions[1].net_eur,
            Decimal::ZERO,
            "no Mindermenge position"
        );
    }

    /// The two quantities must never both be non-zero.
    #[test]
    fn mehr_and_minder_are_mutually_exclusive() {
        for (actual, profil) in [("1600", "1500"), ("1400", "1500"), ("1500", "1500")] {
            let mut i = base_mmm();
            i.actual_kwh = d(actual);
            i.profil_kwh = d(profil);
            let r = calculate_mmm_invoice(&i).unwrap();
            assert!(
                r.positions[0].quantity == Decimal::ZERO
                    || r.positions[1].quantity == Decimal::ZERO,
                "{actual}/{profil}: both positions carry a quantity"
            );
        }
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
            "expected StromNZV reference for a 2025 period, got: {refs:?}"
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
            LegalReference::Sect14aEnwg {
                module: Sect14aModule::Modul2,
            },
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

    // ── sparte, counterparty_mp_id, reversal, Gas path, KA group, validation ──

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
    fn ka_gruppe_annotation_appears_in_position_text() {
        let mut i = base_nne();
        i.ka_satz_ct_per_kwh = Some(d("0.09"));
        i.ka_klasse = Some(KaKundengruppe::Sondervertragskunde);
        let r = calculate_nne_invoice(&i).unwrap();
        let ka_pos = r
            .positions
            .iter()
            .find(|p| p.text.contains("Konzessionsabgabe"))
            .unwrap();
        assert!(
            ka_pos.text.contains("KAV"),
            "KA group annotation should appear in position text: {}",
            ka_pos.text
        );
    }

    /// KAV §2 rates are Höchstbeträge. Strom Sondervertragskunden cap at
    /// 0.11 ct/kWh, so a higher agreed rate is a compliance defect.
    #[test]
    fn ka_rate_above_kav_maximum_warns() {
        let mut i = base_nne();
        i.ka_satz_ct_per_kwh = Some(d("1.32")); // the Tarifkunde ≤25k rate
        i.ka_klasse = Some(KaKundengruppe::Sondervertragskunde);
        let r = calculate_nne_invoice(&i).unwrap();
        assert!(
            r.warnings.iter().any(|w| w.code == "KA_ABOVE_KAV_MAXIMUM"),
            "expected KAV ceiling warning, got: {:?}",
            r.warnings
        );
    }

    /// The Tarifkunde bands key on municipality inhabitants, not consumption.
    #[test]
    fn kav_hoechstbetraege_match_the_statutory_table() {
        use crate::types::GemeindeGroesse::{Bis25k, Bis100k, Bis500k, Ueber500k};
        let tarif = |g, kw| KaKundengruppe::Tarifkunde {
            gemeinde: g,
            nur_kochen_warmwasser: kw,
        };

        // Strom Tarifkunden, KAV §2 Abs. 2.
        for (g, want) in [
            (Bis25k, "1.32"),
            (Bis100k, "1.59"),
            (Bis500k, "1.99"),
            (Ueber500k, "2.39"),
        ] {
            assert_eq!(
                tarif(g, false).hoechstsatz_ct_per_kwh(Sparte::Strom),
                Some(d(want))
            );
        }

        // Gas splits Tariflieferungen into cooking/hot-water and all others.
        assert_eq!(
            tarif(Bis25k, true).hoechstsatz_ct_per_kwh(Sparte::Gas),
            Some(d("0.51"))
        );
        assert_eq!(
            tarif(Bis25k, false).hoechstsatz_ct_per_kwh(Sparte::Gas),
            Some(d("0.22"))
        );

        // Sondervertragskunden are flat and independent of municipality size.
        assert_eq!(
            KaKundengruppe::Sondervertragskunde.hoechstsatz_ct_per_kwh(Sparte::Strom),
            Some(d("0.11"))
        );
        assert_eq!(
            KaKundengruppe::Sondervertragskunde.hoechstsatz_ct_per_kwh(Sparte::Gas),
            Some(d("0.03"))
        );

        // Schwachlast exists for Strom only; KAV provides no gas equivalent.
        assert_eq!(
            KaKundengruppe::Schwachlast.hoechstsatz_ct_per_kwh(Sparte::Strom),
            Some(d("0.61"))
        );
        assert_eq!(
            KaKundengruppe::Schwachlast.hoechstsatz_ct_per_kwh(Sparte::Gas),
            None
        );

        assert_eq!(
            KaKundengruppe::Exempt.hoechstsatz_ct_per_kwh(Sparte::Strom),
            None
        );
    }

    /// A 2025 gas period still cites GasNZV §25, and never the Strom ordinance.
    #[test]
    fn gas_mmm_for_a_2025_period_cites_gasnzv() {
        let mut i = base_mmm();
        i.sparte = Sparte::Gas;
        let r = calculate_mmm_invoice(&i).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("GasNZV §25")),
            "Gas MMM must cite GasNZV §25, got: {refs:?}"
        );
        assert!(
            !refs.iter().any(|r| r.contains("StromNZV")),
            "Gas MMM must not cite StromNZV, got: {refs:?}"
        );
    }

    /// From 01.01.2026 the NZVs no longer apply, so a settlement for that period
    /// must not cite them.
    #[test]
    fn mmm_from_2026_drops_the_repealed_ordinances() {
        for sparte in [Sparte::Strom, Sparte::Gas] {
            let mut i = base_mmm();
            i.sparte = sparte;
            i.period_from = date!(2026 - 01 - 01);
            i.period_to = date!(2026 - 01 - 31);
            let r = calculate_mmm_invoice(&i).unwrap();
            let refs = r.all_legal_refs();
            assert!(
                !refs.iter().any(|r| r.contains("NZV")),
                "{sparte:?} 2026 settlement must not cite a repealed NZV, got: {refs:?}"
            );
            let expected = match sparte {
                Sparte::Strom => "BK6-24-174",
                Sparte::Gas => "BK7-24-01-008",
            };
            assert!(
                refs.iter().any(|r| r.contains(expected)),
                "{sparte:?} 2026 settlement must cite {expected}, got: {refs:?}"
            );
        }
    }

    /// A repealed ordinance must carry its expiry in the citation string, so an
    /// archived invoice stays self-explanatory.
    #[test]
    fn repealed_ordinance_citations_state_their_expiry() {
        let c = LegalReference::StromNzv {
            paragraph: "§13 Abs. 3",
        }
        .citation();
        assert!(c.contains("außer Kraft"), "got: {c}");
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

    // ── §14a Modul 1 (BNetzA BK6-22-300 flat reduction) ──────────────────────

    #[test]
    fn nne_sect14a_modul1_applies_reduction_factor() {
        // 1500 kWh × 3.5 ct/kWh × 0.85 = 1500 × 0.02975 EUR = 44.625 → 44.63
        let mut i = base_nne();
        i.sect14a_modul1_reduction_factor = Some(d("0.85"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.positions.len(), 1);
        assert!(
            r.positions[0].text.contains("Modul 1"),
            "position text must mention Modul 1"
        );
        // 1500 × 0.035 × 0.85 = 1500 × 0.02975 = 44.625 → round_dp(2, MidpointNearestEven) = 44.62
        assert_eq!(r.total_eur, d("44.62"), "expected Modul 1 reduced total");
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("Modul 1")),
            "must cite §14a Modul 1"
        );
        assert!(
            refs.iter().any(|r| r.contains("BK6-22-300")),
            "must cite BK6-22-300"
        );
        assert_eq!(
            r.positions[0].trace.regulatory_reduction_factor,
            Some(d("0.85"))
        );
    }

    #[test]
    fn nne_sect14a_modul1_full_reduction_yields_zero() {
        // Reduction factor = 0 is invalid; factor = 1 = no reduction
        let mut i = base_nne();
        i.sect14a_modul1_reduction_factor = Some(d("1.0"));
        let r = calculate_nne_invoice(&i).unwrap();
        // 1500 × 0.035 × 1.0 = 52.50 — same as plain Arbeit
        assert_eq!(r.total_eur, d("52.50"));
    }

    #[test]
    fn validate_modul1_and_modul2_conflict_is_error() {
        let mut i = base_nne();
        // Set both Modul 1 and Modul 2 HT/NT fields — must be rejected
        i.sect14a_modul1_reduction_factor = Some(d("0.85"));
        i.arbeitsmenge_ht_kwh = Some(d("900"));
        i.arbeitspreis_ht_ct_per_kwh = Some(d("4.0"));
        i.arbeitsmenge_nt_kwh = Some(d("600"));
        i.arbeitspreis_nt_ct_per_kwh = Some(d("2.0"));
        let v = validate_nne_input(&i);
        assert!(!v.is_valid, "Modul 1 + Modul 2 combination must be invalid");
        assert!(
            v.warnings
                .iter()
                .any(|w| w.code == "MODUL1_AND_MODUL2_CONFLICT"),
            "must produce MODUL1_AND_MODUL2_CONFLICT warning"
        );
    }

    #[test]
    fn validate_partial_tou_fields_is_error() {
        let mut i = base_nne();
        // Only 1 of the 4 HT/NT fields set — must be rejected
        i.arbeitsmenge_ht_kwh = Some(d("900"));
        let v = validate_nne_input(&i);
        assert!(!v.is_valid, "partial HT/NT set must be invalid");
        assert!(
            v.warnings.iter().any(|w| w.code == "PARTIAL_TOU_FIELDS"),
            "must produce PARTIAL_TOU_FIELDS warning"
        );
    }

    // ── Gas Grundpreis ────────────────────────────────────────────────────────

    #[test]
    fn nne_gas_with_grundpreis_adds_position() {
        let mut i = base_nne();
        i.sparte = Sparte::Gas;
        i.nne_grundpreis_eur_per_month = Some(d("15.00"));
        i.nne_grundpreis_months = Some(1);
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.positions.len(), 2, "Grundpreis + Arbeit");
        assert!(
            r.positions[0].text.contains("Grundpreis"),
            "first position must be Grundpreis"
        );
        assert_eq!(r.positions[0].net_eur, d("15.00000"));
        let refs_p0 = &r.positions[0].trace.legal_refs;
        assert!(
            refs_p0.iter().any(|lr| lr.citation().contains("GasNEV")),
            "Grundpreis must cite GasNEV"
        );
    }

    #[test]
    fn validate_grundpreis_mismatch_is_error() {
        let mut i = base_nne();
        i.nne_grundpreis_eur_per_month = Some(d("15.00"));
        // Missing nne_grundpreis_months — mismatch
        let v = validate_nne_input(&i);
        assert!(!v.is_valid);
        assert!(
            v.warnings
                .iter()
                .any(|w| w.code == "GRUNDPREIS_MONTHS_MISMATCH")
        );
    }

    // ── Gas AWH Sperrprozesse (PID 31011) ─────────────────────────────────────

    #[test]
    fn gas_awh_single_sperrung_arithmetic() {
        let input = GasAwhInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "AWH-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            tariff_sheet_id: None,
            awh_positionen: vec![AwhPositionInput {
                beschreibung: "Sperrung Gaszähler".into(),
                anzahl: 1,
                preis_eur: d("45.00"),
                artikel_id: Some("2-01-7-001".to_owned()),
            }],
        };
        let r = calculate_gas_awh_invoice(&input).unwrap();
        assert_eq!(r.pid, 31011);
        assert_eq!(r.settlement_type, SettlementType::GasAwhSperrung);
        assert_eq!(r.total_eur, d("45.00"));
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].text, "Sperrung Gaszähler");
        let refs = r.all_legal_refs();
        assert!(refs.iter().any(|r| r.contains("BK7-24-01-009")));
    }

    #[test]
    fn gas_awh_multiple_actions_total_correct() {
        let input = GasAwhInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "AWH-TEST-002".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            tariff_sheet_id: None,
            awh_positionen: vec![
                AwhPositionInput {
                    beschreibung: "Sperrung".into(),
                    anzahl: 1,
                    preis_eur: d("45.00"),
                    artikel_id: Some("2-01-7-001".to_owned()),
                },
                AwhPositionInput {
                    beschreibung: "Entsperrung".into(),
                    anzahl: 2,
                    preis_eur: d("30.00"),
                    artikel_id: Some("2-01-7-002".to_owned()),
                },
            ],
        };
        let r = calculate_gas_awh_invoice(&input).unwrap();
        // 45 + 2×30 = 105
        assert_eq!(r.total_eur, d("105.00"));
        assert_eq!(r.positions.len(), 2);
        assert_eq!(r.recomputed_total(), r.total_eur);
    }

    #[test]
    fn gas_awh_empty_positions_rejected() {
        let input = GasAwhInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "AWH-TEST-003".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            tariff_sheet_id: None,
            awh_positionen: vec![],
        };
        assert!(matches!(
            calculate_gas_awh_invoice(&input),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    // ── Correction lifecycle ──────────────────────────────────────────────────

    #[test]
    fn correction_pair_status_and_reference() {
        let original = calculate_nne_invoice(&base_nne()).unwrap();
        let mut corrected_input = base_nne();
        corrected_input.arbeitsmenge_kwh = d("1600");
        corrected_input.rechnungsnummer = "NNE-KORR-001".into();
        let replacement = calculate_nne_invoice(&corrected_input).unwrap();

        let (reversal, corrected) = calculate_correction(
            &original,
            replacement,
            "STORNO-NNE-001".into(),
            date!(2025 - 03 - 01),
            date!(2025 - 03 - 31),
        );
        assert_eq!(reversal.status, SettlementStatus::Reversal);
        assert_eq!(reversal.total_eur, -original.total_eur);
        assert_eq!(reversal.correction_of, Some("NNE-TEST-001".to_owned()));
        assert_eq!(corrected.status, SettlementStatus::Correction);
        assert_eq!(corrected.correction_of, Some("NNE-TEST-001".to_owned()));
    }

    // ── recomputed_total consistency ──────────────────────────────────────────

    #[test]
    fn nne_recomputed_total_matches_total_eur() {
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        assert_eq!(r.recomputed_total(), r.total_eur);
    }

    #[test]
    fn mmm_recomputed_total_matches_total_eur() {
        let r = calculate_mmm_invoice(&base_mmm()).unwrap();
        assert_eq!(r.recomputed_total(), r.total_eur);
    }

    // ── Gas MMM uses MmmGas settlement type ───────────────────────────────────

    #[test]
    fn mmm_gas_uses_mmm_gas_settlement_type() {
        let mut i = base_mmm();
        i.sparte = Sparte::Gas;
        let r = calculate_mmm_invoice(&i).unwrap();
        assert_eq!(
            r.settlement_type,
            SettlementType::MmmGas,
            "Gas MMM must use MmmGas settlement type"
        );
    }

    #[test]
    fn mmm_strom_uses_mmm_strom_settlement_type() {
        let r = calculate_mmm_invoice(&base_mmm()).unwrap();
        assert_eq!(r.settlement_type, SettlementType::MmmStrom);
    }
}

// ── Property tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use rust_decimal::Decimal;
    use time::macros::date;

    fn arb_positive_kwh() -> impl Strategy<Value = Decimal> {
        (1u64..100_000u64).prop_map(Decimal::from)
    }

    fn arb_ct_per_kwh() -> impl Strategy<Value = Decimal> {
        (1u64..2000u64).prop_map(|n| Decimal::new(n as i64, 2)) // 0.01 – 20.00 ct/kWh
    }

    proptest! {
        /// Invariant: reversal of any valid NNE settlement negates the total.
        ///
        /// For any valid (kwh, price) pair, the reversal total equals -original.total_eur.
        #[test]
        fn reversal_always_negates_total(
            kwh in arb_positive_kwh(),
            ct in arb_ct_per_kwh(),
        ) {
            let input = NneInput {
                malo_id: "51238696780".into(),
                nb_mp_id: "9900357000004".into(),
                lf_mp_id: "9900012345678".into(),
                rechnungsnummer: "PROP-NNE-001".into(),
                period_from: date!(2025 - 01 - 01),
                period_to: date!(2025 - 12 - 31),
                invoice_date: date!(2026 - 01 - 15),
                due_date: date!(2026 - 02 - 15),
                arbeitsmenge_kwh: kwh,
                arbeitspreis_ct_per_kwh: ct,
                arbeitsmenge_ht_kwh: None,
                arbeitspreis_ht_ct_per_kwh: None,
                arbeitsmenge_nt_kwh: None,
                arbeitspreis_nt_ct_per_kwh: None,
                spitzenleistung_kw: None,
                leistungspreis_eur_per_kw: None,
                ka_satz_ct_per_kwh: None,
                sect14a_modul1_reduction_factor: None,
                nne_grundpreis_eur_per_month: None,
                nne_grundpreis_months: None,
                tariff_sheet_id: None,
                sparte: Sparte::Strom,
                ka_klasse: None,
                sect14a_modul3_intervals: vec![],
            };
            if let Ok(original) = calculate_nne_invoice(&input) {
                let reversal = calculate_reversal(
                    &original,
                    "PROP-STORNO-001".to_owned(),
                    date!(2026 - 02 - 01),
                    date!(2026 - 02 - 28),
                );
                prop_assert_eq!(reversal.total_eur, -original.total_eur);
                prop_assert_eq!(reversal.recomputed_total(), reversal.total_eur);
                prop_assert_eq!(reversal.positions.len(), original.positions.len());
            }
        }

        /// Invariant: §14a Modul 1 reduction factor ∈ (0, 1] → billed total ≤ unreduced total.
        #[test]
        fn modul1_total_lte_unreduced(
            kwh in arb_positive_kwh(),
            ct in arb_ct_per_kwh(),
            // factor ∈ [1%, 100%]
            factor_pct in 1u64..=100u64,
        ) {
            let factor = Decimal::new(factor_pct as i64, 2);
            let base = NneInput {
                malo_id: "51238696780".into(),
                nb_mp_id: "9900357000004".into(),
                lf_mp_id: "9900012345678".into(),
                rechnungsnummer: "PROP-M1-001".into(),
                period_from: date!(2025 - 01 - 01),
                period_to: date!(2025 - 12 - 31),
                invoice_date: date!(2026 - 01 - 15),
                due_date: date!(2026 - 02 - 15),
                arbeitsmenge_kwh: kwh,
                arbeitspreis_ct_per_kwh: ct,
                arbeitsmenge_ht_kwh: None,
                arbeitspreis_ht_ct_per_kwh: None,
                arbeitsmenge_nt_kwh: None,
                arbeitspreis_nt_ct_per_kwh: None,
                spitzenleistung_kw: None,
                leistungspreis_eur_per_kw: None,
                ka_satz_ct_per_kwh: None,
                sect14a_modul1_reduction_factor: None,
                nne_grundpreis_eur_per_month: None,
                nne_grundpreis_months: None,
                tariff_sheet_id: None,
                sparte: Sparte::Strom,
                ka_klasse: None,
                sect14a_modul3_intervals: vec![],
            };
            if let Ok(unreduced) = calculate_nne_invoice(&base) {
                let mut reduced_input = base.clone();
                reduced_input.sect14a_modul1_reduction_factor = Some(factor);
                if let Ok(reduced) = calculate_nne_invoice(&reduced_input) {
                    prop_assert!(
                        reduced.total_eur <= unreduced.total_eur,
                        "Modul 1 reduced total must be ≤ unreduced total"
                    );
                }
            }
        }
    }
}

// ── §14a Modul 3 unit tests ───────────────────────────────────────────────────

#[cfg(test)]
mod modul3_tests {
    use super::*;
    use crate::types::{Sect14aModul3Interval, validate_nne_input};
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
            rechnungsnummer: "NNE-M3-TEST".into(),
            period_from: date!(2026 - 01 - 15),
            period_to: date!(2026 - 01 - 16),
            invoice_date: date!(2026 - 02 - 15),
            due_date: date!(2026 - 03 - 15),
            arbeitsmenge_kwh: d("1500"),
            arbeitspreis_ct_per_kwh: d("3.5"),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
            sect14a_modul1_reduction_factor: None,
            nne_grundpreis_eur_per_month: None,
            nne_grundpreis_months: None,
            tariff_sheet_id: None,
            sparte: Sparte::Strom,
            ka_klasse: None,
            sect14a_modul3_intervals: vec![],
        }
    }

    #[test]
    fn nne_sect14a_modul3_single_interval() {
        use time::OffsetDateTime;
        let start = OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let end = start + time::Duration::minutes(15);

        let mut i = base_nne();
        i.sect14a_modul3_intervals = vec![Sect14aModul3Interval {
            period_from: start,
            period_to: end,
            menge_kwh: d("2.5"),
            nne_rate_ct_per_kwh: d("1.80"),
            epex_spot_ct_per_kwh: Some(d("12.50")),
        }];
        let r = calculate_nne_invoice(&i).unwrap();

        // Exactly 2 positions: flat Arbeit + one Modul 3 interval
        assert_eq!(
            r.positions.len(),
            2,
            "expect base Arbeit + 1 Modul 3 position"
        );

        let modul3_pos = r
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::NneArbeitModul3)
            .expect("Modul 3 position must be present");

        // 2.5 kWh × 0.018 EUR/kWh = 0.045 EUR
        assert_eq!(modul3_pos.net_eur, d("0.04500"), "Modul 3 net_eur");
        assert_eq!(modul3_pos.quantity, d("2.500"));
        assert_eq!(modul3_pos.unit, QuantityUnit::Kwh);

        // LastvariablePreisposition JSON must be typed
        let lvp = modul3_pos
            .lastvariable_preisposition_json
            .as_ref()
            .expect("LastvariablePreisposition must be set on NneArbeitModul3 positions");
        assert_eq!(lvp["_typ"].as_str(), Some("LASTVARIABLE_PREISPOSITION"));
        assert_eq!(lvp["tarifkalkulationsmethode"].as_str(), Some("SPOTPREIS"));
        assert_eq!(lvp["preisreferenz"].as_str(), Some("ENERGIEMENGE"));
        assert_eq!(lvp["preisBezugseinheit"].as_str(), Some("KWH"));

        // EPEX spot price must be in zusatzAttribute
        let zas = lvp["zusatzAttribute"]
            .as_array()
            .expect("zusatzAttribute when epex_spot_ct_per_kwh is set");
        assert!(
            zas.iter()
                .any(|a| a["name"] == "epexSpotCtPerKwh" && a["wert"] == "12.50"),
            "epexSpotCtPerKwh must be in zusatzAttribute"
        );

        // Legal references
        let refs = &modul3_pos.trace.legal_refs;
        assert!(
            refs.iter().any(|r| matches!(
                r,
                LegalReference::Sect14aEnwg {
                    module: Sect14aModule::Modul3
                }
            )),
            "must reference §14a Modul 3"
        );
        assert!(
            refs.iter().any(|r| matches!(
                r,
                LegalReference::BnetzaDecision {
                    reference: "BK6-22-300"
                }
            )),
            "must reference BK6-22-300"
        );
    }

    #[test]
    fn nne_sect14a_modul3_multiple_intervals_sum_correctly() {
        use time::OffsetDateTime;
        let base = OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let mut i = base_nne();
        i.sect14a_modul3_intervals = vec![
            Sect14aModul3Interval {
                period_from: base,
                period_to: base + time::Duration::minutes(15),
                menge_kwh: d("1.25"),
                nne_rate_ct_per_kwh: d("2.00"),
                epex_spot_ct_per_kwh: None,
            },
            Sect14aModul3Interval {
                period_from: base + time::Duration::minutes(15),
                period_to: base + time::Duration::minutes(30),
                menge_kwh: d("1.75"),
                nne_rate_ct_per_kwh: d("1.50"),
                epex_spot_ct_per_kwh: None,
            },
        ];
        let r = calculate_nne_invoice(&i).unwrap();

        // 3 positions: flat Arbeit + 2 Modul 3 intervals
        assert_eq!(r.positions.len(), 3);
        let modul3: Vec<_> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::NneArbeitModul3)
            .collect();
        assert_eq!(modul3.len(), 2);
        // Interval 1: 1.25 kWh × 0.02 EUR/kWh = 0.025 EUR
        assert_eq!(modul3[0].net_eur, d("0.02500"));
        // Interval 2: 1.75 kWh × 0.015 EUR/kWh = 0.02625 EUR
        assert_eq!(modul3[1].net_eur, d("0.02625"));
        // No zusatzAttribute when epex not supplied
        let lvp0 = modul3[0].lastvariable_preisposition_json.as_ref().unwrap();
        assert!(lvp0.get("zusatzAttribute").is_none());
    }

    #[test]
    fn nne_modul3_zero_kwh_interval_is_skipped() {
        use time::OffsetDateTime;
        let base = OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let mut i = base_nne();
        i.sect14a_modul3_intervals = vec![
            Sect14aModul3Interval {
                period_from: base,
                period_to: base + time::Duration::minutes(15),
                menge_kwh: d("0"),
                nne_rate_ct_per_kwh: d("2.00"),
                epex_spot_ct_per_kwh: None,
            },
            Sect14aModul3Interval {
                period_from: base + time::Duration::minutes(15),
                period_to: base + time::Duration::minutes(30),
                menge_kwh: d("1.50"),
                nne_rate_ct_per_kwh: d("1.80"),
                epex_spot_ct_per_kwh: None,
            },
        ];
        let r = calculate_nne_invoice(&i).unwrap();
        let modul3: Vec<_> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::NneArbeitModul3)
            .collect();
        assert_eq!(modul3.len(), 1, "zero-kWh interval must be skipped");
    }

    #[test]
    fn nne_modul3_and_modul1_conflict_fails_validation() {
        use time::OffsetDateTime;
        let base = OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let mut i = base_nne();
        i.sect14a_modul1_reduction_factor = Some(d("0.85"));
        i.sect14a_modul3_intervals = vec![Sect14aModul3Interval {
            period_from: base,
            period_to: base + time::Duration::minutes(15),
            menge_kwh: d("1.0"),
            nne_rate_ct_per_kwh: d("2.0"),
            epex_spot_ct_per_kwh: None,
        }];
        let v = validate_nne_input(&i);
        assert!(
            !v.is_valid,
            "Modul 1 + Modul 3 must produce validation error"
        );
        assert!(
            v.warnings
                .iter()
                .any(|w| w.code == "MODUL1_AND_MODUL3_CONFLICT"),
            "error code must be MODUL1_AND_MODUL3_CONFLICT"
        );
    }
}
