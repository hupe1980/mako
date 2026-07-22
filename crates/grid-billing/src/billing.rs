//! NNE, MMM, and MSB settlement calculation logic.
//!
//! Amounts are computed in `rust_decimal::Decimal`; every EUR result is
//! range-checked through [`billing::EuroAmount`] for exact
//! representation.  Functions return [`SettlementResult`] — a pure domain type
//! with no BO4E coupling.  The service layer (netzbilanzd / invoicd) converts
//! `SettlementResult` to `rubo4e::current::Rechnung` via a local `into_rechnung()`
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
    ArbeitspreisModell, BillingPositionKind, CalculationTrace, GasAwhInput, KaKundengruppe,
    LegalReference, MmmInput, MsbInput, NneInput, PriceReference, PriceStep, QuantityUnit,
    Sect14aModul3Interval, Sect14aModule, SettlementPosition, SettlementResult, SettlementStatus,
    SettlementType, SettlementWarning, Sparte, SpotPriceFormula, TariffCalculationMethod,
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
    text: &str,
    kind: BillingPositionKind,
    kwh: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> SettlementPosition {
    let gross_eur = kwh * unit_price_eur;
    SettlementPosition {
        text: text.to_owned(),
        kind,
        quantity: kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kwh, unit_price_eur),
        spot_price_formula: None,

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
    text: &str,
    kind: BillingPositionKind,
    kw: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> SettlementPosition {
    let gross_eur = kw * unit_price_eur;
    SettlementPosition {
        text: text.to_owned(),
        kind,
        quantity: kw.round_dp(3),
        unit: QuantityUnit::Kw,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kw, unit_price_eur),
        spot_price_formula: None,

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
    text: &str,
    kind: BillingPositionKind,
    months: Decimal,
    unit_price_eur: Decimal,
    legal_refs: Vec<LegalReference>,
    tariff_source: Option<TariffSource>,
) -> SettlementPosition {
    let gross_eur = months * unit_price_eur;
    SettlementPosition {
        text: text.to_owned(),
        kind,
        quantity: months.round_dp(3),
        unit: QuantityUnit::Monat,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(months, unit_price_eur),
        spot_price_formula: None,

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

/// Reject an invoice total that cannot be represented as a [`EuroAmount`].
///
/// The converted value is deliberately discarded — the call is a range check run
/// before returning a document, so that a total which would overflow is refused
/// here rather than truncated by a downstream consumer.
fn ensure_representable_eur(d: Decimal) -> Result<(), BillingError> {
    decimal_to_euro_amount(d).map(|_| ())
}

fn make_tariff_source(sheet_id: Option<&str>) -> Option<TariffSource> {
    sheet_id.map(|id| TariffSource::PublishedTariffSheet {
        sheet_id: id.to_owned(),
    })
}

// ── NNE invoice (PID 31001 / 31005 / 31006) ──────────────────────────────────

/// Calculate a NNE settlement (PID 31001 Strom, 31005 Gas, 31006 selbstausstellt).
///
/// Returns a [`SettlementResult`] with full [`CalculationTrace`] per position
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
pub fn settle_nne(input: &NneInput) -> Result<SettlementResult, BillingError> {
    // The period is ordered by construction, the Leistungspreis is paired by
    // construction, and the §14a modules are exclusive by construction — so the
    // guards that used to check those are gone with the states they checked.
    //
    // What remains is what the types cannot express. It runs here rather than in
    // a validator the caller may skip: these are the errors that otherwise
    // produce a plausible-looking invoice billed on the wrong basis.
    if input.arbeitspreis.menge_kwh() < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "metered energy must be non-negative".to_owned(),
        });
    }
    if let ArbeitspreisModell::Modul3Spotpreis { intervalle } = &input.arbeitspreis {
        if intervalle.is_empty() {
            return Err(BillingError::InvalidInput {
                reason: "§14a Modul 3 requires at least one dispatch interval".to_owned(),
            });
        }
        for (i, iv) in intervalle.iter().enumerate() {
            if iv.period_from >= iv.period_to {
                return Err(BillingError::InvalidInput {
                    reason: format!("Modul 3 interval {i}: start is not before end"),
                });
            }
            if iv.menge_kwh < Decimal::ZERO {
                return Err(BillingError::InvalidInput {
                    reason: format!("Modul 3 interval {i}: metered energy is negative"),
                });
            }
        }
    }
    if let Some(lp) = input.leistungspreis
        && lp.spitzenleistung_kw < Decimal::ZERO
    {
        return Err(BillingError::InvalidInput {
            reason: "Spitzenleistung must be non-negative".to_owned(),
        });
    }
    if let Some(gp) = input.grundpreis
        && gp.months < Decimal::ZERO
    {
        return Err(BillingError::InvalidInput {
            reason: "Grundpreis months must be non-negative".to_owned(),
        });
    }

    let tariff_src = make_tariff_source(input.tariff_sheet_id.as_deref());
    let mut positions: Vec<SettlementPosition> = Vec::new();
    let mut total = Decimal::ZERO;
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
    if let Some(gp) = input.grundpreis
        && gp.months > Decimal::ZERO
    {
        let months = gp.months;
        let p = monat_pos_traced(
            "Netzentgelt Grundpreis Gas (Verrechnungspreis)",
            BillingPositionKind::NneGasGrundpreis,
            months,
            gp.eur_per_month,
            vec![LegalReference::GasNev { paragraph: "§14" }],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
    }

    // §17 StromNEV context, recorded rather than applied: the Netzebene a rate
    // was published for, and the utilisation the price sheet should have been
    // read at. Neither selects a rate here — the caller supplies rates — but an
    // auditor cannot check that the right rate was used without them.
    if let (Some(arbeit), Some(peak)) = (input.jahresarbeit_kwh, input.jahreshoechstleistung_kw)
        && let Some(bh) = crate::netzebene::benutzungsstundenzahl(arbeit, peak)
    {
        warnings.push(SettlementWarning {
            severity: WarningSeverity::Info,
            code: "BENUTZUNGSSTUNDENZAHL",
            message: format!(
                "{bh} h/a ({arbeit} kWh / {peak} kW){}",
                input
                    .netzebene
                    .map(|e| format!(" in {}", e.label()))
                    .unwrap_or_default()
            ),
        });
    }
    // §17 Abs. 6 permits an Arbeitspreis-only tariff only in the
    // Niederspannungsnetz at or below 100 000 kWh a year. Billing without a
    // Leistungspreis outside that is a tariff-structure error, not a rounding one.
    if input.leistungspreis.is_none()
        && let (Some(ebene), Some(arbeit)) = (input.netzebene, input.jahresarbeit_kwh)
        && !crate::netzebene::arbeitspreis_nur_zulaessig(ebene, arbeit)
    {
        warnings.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "ARBEITSPREIS_ONLY_OUTSIDE_SECT17_ABS6",
            message: format!(
                "billed on an Arbeitspreis alone at {} with {arbeit} kWh/a —                  §17 Abs. 6 StromNEV allows this only in Niederspannung up to                  100 000 kWh/a",
                ebene.label()
            ),
        });
    }

    // Gas Kapazitätsentgelt (§15 GasNEV). The rate is annual, the settlement
    // is not — so it is pro-rated by calendar days, and the trace says so.
    if let Some(kap) = input.gas_kapazitaet {
        if input.sparte != Sparte::Gas {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "GAS_KAPAZITAET_ON_STROM",
                message: "a gas capacity charge was supplied on a Strom settlement — \
                          §15 GasNEV does not apply to Strom"
                    .to_owned(),
            });
        } else {
            let tage = Decimal::from(input.period.days());
            let anteil = tage / Decimal::from(365);
            let price_eur = (kap.entgelt_eur_per_kwh_h_a * anteil).round_dp(6);
            let net_eur = (kap.bestellte_kapazitaet_kwh_h * price_eur).round_dp(5);
            let stufe = kap
                .druckstufe
                .map(|d| format!(", {}", d.label()))
                .unwrap_or_default();
            let p = SettlementPosition {
                text: format!("Kapazitätsentgelt Gas ({}{stufe})", kap.produkt.label()),
                kind: BillingPositionKind::GasKapazitaetsentgelt,
                quantity: kap.bestellte_kapazitaet_kwh_h.round_dp(3),
                unit: QuantityUnit::Kw,
                unit_price_eur: price_eur,
                net_eur,
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "{:.3} kWh/h × {:.6} EUR (= {:.6} EUR/a × {tage}/365 days) = {:.5} EUR \
                         ({}{stufe})",
                        kap.bestellte_kapazitaet_kwh_h,
                        price_eur,
                        kap.entgelt_eur_per_kwh_h_a,
                        net_eur,
                        kap.produkt.label(),
                    ),
                    input_quantity: kap.bestellte_kapazitaet_kwh_h,
                    input_unit_price_eur: price_eur,
                    gross_eur: kap.bestellte_kapazitaet_kwh_h * price_eur,
                    legal_refs: vec![match kap.produkt {
                        crate::gas::Kapazitaetsprodukt::Fest => {
                            LegalReference::GasNev { paragraph: "§15" }
                        }
                        crate::gas::Kapazitaetsprodukt::Unterbrechbar => LegalReference::GasNev {
                            paragraph: "§15 Abs. 5",
                        },
                    }],
                    tariff_source: tariff_src.clone(),
                    regulatory_reduction_factor: None,
                    rounding_note: Some(
                        "annual rate pro-rated by calendar days over 365; unit price to 6 dp; \
                         net to 5 dp",
                    ),
                },
            };
            total += p.net_eur;
            positions.push(p);
        }
    }

    // The Arbeitspreis model decides what is billed; the four shapes are
    // mutually exclusive by construction, so there is no precedence to get wrong
    // and no partial state to fall through.
    match &input.arbeitspreis {
        ArbeitspreisModell::Modul2ZeitVariabel { ht, nt } => {
            for (label, kind, mp) in [
                (
                    "Netznutzung Arbeit HT (§14a Modul 2)",
                    BillingPositionKind::NneArbeitHt,
                    ht,
                ),
                (
                    "Netznutzung Arbeit NT (§14a Modul 2)",
                    BillingPositionKind::NneArbeitNt,
                    nt,
                ),
            ] {
                let p = kwh_pos_traced(
                    label,
                    kind,
                    mp.menge_kwh,
                    ct_to_eur(mp.preis_ct_per_kwh),
                    vec![
                        arbeit_ref.clone(),
                        LegalReference::Sect14aEnwg {
                            module: Sect14aModule::Modul2,
                        },
                        LegalReference::BnetzaDecision {
                            reference: "BK6-22-300",
                        },
                    ],
                    tariff_src.clone(),
                );
                total += p.net_eur;
                positions.push(p);
            }
        }

        ArbeitspreisModell::Modul1Pauschal { basis, reduktion } => {
            let base_eur = ct_to_eur(basis.preis_ct_per_kwh);
            let factor = reduktion.get();
            let reduced_eur = (base_eur * factor).round_dp(6);
            let gross = basis.menge_kwh * reduced_eur;
            let p = SettlementPosition {
                text: format!(
                    "Netznutzung Arbeit §14a Modul 1 ({:.0}% Reduzierung)",
                    (Decimal::ONE - factor) * HUNDRED
                ),
                kind: BillingPositionKind::NneArbeitModul1,
                quantity: basis.menge_kwh.round_dp(3),
                unit: QuantityUnit::Kwh,
                unit_price_eur: reduced_eur,
                net_eur: pos_net(basis.menge_kwh, reduced_eur),
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "{:.3} kWh × {:.6} EUR/kWh (= {:.6} × {factor} Modul 1) = {:.5} EUR",
                        basis.menge_kwh,
                        reduced_eur,
                        base_eur,
                        gross.round_dp(5)
                    ),
                    input_quantity: basis.menge_kwh,
                    input_unit_price_eur: reduced_eur,
                    gross_eur: gross,
                    legal_refs: vec![
                        arbeit_ref.clone(),
                        LegalReference::Sect14aEnwg {
                            module: Sect14aModule::Modul1,
                        },
                        LegalReference::BnetzaDecision {
                            reference: "BK6-22-300",
                        },
                    ],
                    tariff_source: tariff_src.clone(),
                    regulatory_reduction_factor: Some(factor),
                    rounding_note: Some(
                        "quantity rounded to 3 dp; unit price to 6 dp; net to 5 dp",
                    ),
                },
            };
            total += p.net_eur;
            positions.push(p);
        }

        ArbeitspreisModell::Einheitlich(mp) => {
            let p = kwh_pos_traced(
                "Netznutzung Arbeit",
                BillingPositionKind::NneArbeit,
                mp.menge_kwh,
                ct_to_eur(mp.preis_ct_per_kwh),
                vec![arbeit_ref.clone()],
                tariff_src.clone(),
            );
            total += p.net_eur;
            positions.push(p);
        }

        // Modul 3 positions are emitted below, per dispatch interval.
        ArbeitspreisModell::Modul3Spotpreis { .. } => {}
    }

    // Leistung (RLM only) — StromNEV §17
    if let Some(lp) = input.leistungspreis {
        let p = kw_pos_traced(
            "Netznutzung Leistung",
            BillingPositionKind::NneLeistung,
            lp.spitzenleistung_kw,
            lp.preis_eur_per_kw,
            vec![LegalReference::StromNev { paragraph: "§17" }],
            tariff_src.clone(),
        );
        total += p.net_eur;
        positions.push(p);
    }

    // §19 Abs. 2 StromNEV — an agreed individual charge replaces the published
    // Netzentgelt at a fraction the ordinance floors. The reduction covers the
    // Arbeits- and Leistungspreis positions and nothing else: the KA and the
    // levies are not the Netzbetreiber's revenue to reduce, and the lost NNE
    // revenue is recovered through the §19-Umlage billed below.
    if let Some(v) = &input.sect19 {
        let floor = match v.art {
            crate::sect19::Sect19Art::AtypischeNetznutzung => {
                Some(crate::sect19::ATYPISCH_MINDESTENTGELT)
            }
            crate::sect19::Sect19Art::IntensiveNetznutzung => {
                match (input.jahresarbeit_kwh, input.jahreshoechstleistung_kw) {
                    (Some(arbeit), Some(peak)) => {
                        crate::netzebene::benutzungsstundenzahl(arbeit, peak)
                            .and_then(|bh| crate::sect19::bandlast_mindestentgelt(bh, arbeit))
                    }
                    _ => None,
                }
            }
        };
        match floor {
            None => warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "SECT19_BANDLAST_CRITERIA_NOT_MET",
                message: "a §19 Abs. 2 Satz 2 agreement needs at least 7 000 \
                          Benutzungsstunden and 10 GWh a year — the utilisation data \
                          supplied does not qualify (or is missing)"
                    .to_owned(),
            }),
            Some(f) if v.vereinbarter_prozentsatz < f => warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "SECT19_BELOW_MINDESTENTGELT",
                message: format!(
                    "the agreed {} % is below the statutory Mindestentgelt of {} % \
                     (§19 Abs. 2 StromNEV)",
                    (v.vereinbarter_prozentsatz * HUNDRED).normalize(),
                    (f * HUNDRED).normalize()
                ),
            }),
            Some(_) => {}
        }

        let nne_basis: Decimal = positions
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    BillingPositionKind::NneArbeit
                        | BillingPositionKind::NneArbeitHt
                        | BillingPositionKind::NneArbeitNt
                        | BillingPositionKind::NneArbeitModul1
                        | BillingPositionKind::NneArbeitModul3
                        | BillingPositionKind::NneLeistung
                )
            })
            .map(|p| p.net_eur)
            .sum();
        let reduction = -(nne_basis * (Decimal::ONE - v.vereinbarter_prozentsatz)).round_dp(5);
        if !reduction.is_zero() {
            let art_label = match v.art {
                crate::sect19::Sect19Art::AtypischeNetznutzung => "atypische Netznutzung",
                crate::sect19::Sect19Art::IntensiveNetznutzung => "intensive Netznutzung",
            };
            let genehmigung = v
                .genehmigung
                .as_deref()
                .map(|g| format!(", {g}"))
                .unwrap_or_default();
            let p = SettlementPosition {
                text: format!(
                    "Individuelles Netzentgelt §19 Abs. 2 ({art_label}, {} %)",
                    (v.vereinbarter_prozentsatz * HUNDRED).normalize()
                ),
                kind: BillingPositionKind::Sect19IndividuellesEntgelt,
                quantity: Decimal::ONE,
                unit: QuantityUnit::Monat,
                unit_price_eur: reduction,
                net_eur: reduction,
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "-(1 − {}) × {nne_basis:.5} EUR Netzentgelt = {reduction:.5} EUR \
                         ({art_label}{genehmigung})",
                        v.vereinbarter_prozentsatz
                    ),
                    input_quantity: nne_basis,
                    input_unit_price_eur: reduction,
                    gross_eur: reduction,
                    legal_refs: vec![
                        LegalReference::StromNev {
                            paragraph: "§19 Abs. 2",
                        },
                        LegalReference::BnetzaDecision {
                            reference: "BK4-22-089",
                        },
                    ],
                    tariff_source: None,
                    regulatory_reduction_factor: Some(v.vereinbarter_prozentsatz),
                    rounding_note: Some("net to 5 dp"),
                },
            };
            total += p.net_eur;
            positions.push(p);
        }
    }

    // ── Netzseitige Umlagen (EnFG) ────────────────────────────────────────────
    //
    // The three levies ride on the same energy base as the Arbeitspreis and are
    // billed per Entnahmestelle at the rate its Letztverbrauchergruppe carries.
    // A missing tabled rate is a warning rather than a silent zero: billing a
    // levy at nothing understates the invoice by an amount the ÜNB will reclaim.
    let umlage_base_kwh = input.arbeitspreis.menge_kwh();
    if input.sparte == Sparte::Strom {
        let year = input.period.from().year();
        let gruppe = input.letztverbrauchergruppe;
        let levies: [(&str, BillingPositionKind, Option<Decimal>, LegalReference); 3] = [
            (
                "Aufschlag für besondere Netznutzung (§19 StromNEV)",
                BillingPositionKind::Sect19StromNevUmlage,
                input
                    .sect19_umlage_ct_per_kwh
                    .or_else(|| crate::umlagen::sect19_stromnev_ct_per_kwh(year, gruppe)),
                LegalReference::StromNev {
                    paragraph: "§19 Abs. 2",
                },
            ),
            (
                "Offshore-Netzumlage",
                BillingPositionKind::OffshoreNetzumlage,
                input
                    .offshore_umlage_ct_per_kwh
                    .or_else(|| crate::umlagen::offshore_netzumlage_ct_per_kwh(year, gruppe)),
                LegalReference::Enwg { paragraph: "§17f" },
            ),
            (
                "KWKG-Umlage",
                BillingPositionKind::KwkgUmlage,
                input
                    .kwkg_umlage_ct_per_kwh
                    .or_else(|| crate::umlagen::kwkg_umlage_ct_per_kwh(year, gruppe)),
                LegalReference::Kwkg { paragraph: "§26" },
            ),
        ];

        for (label, kind, rate, legal) in levies {
            let Some(rate_ct) = rate else {
                // Only for years the series undertakes to cover: below that it
                // claims nothing, and warning would be noise rather than signal.
                if year >= crate::umlagen::ERSTES_ERFASSTES_JAHR {
                    warnings.push(SettlementWarning {
                        severity: WarningSeverity::Warning,
                        code: "UMLAGE_RATE_MISSING",
                        message: format!(
                            "{label}: no published rate for {year} and no override — \
                             the levy is omitted from this invoice"
                        ),
                    });
                }
                continue;
            };
            if rate_ct.is_zero() {
                // §21 EnFG exempts entirely; a zero line adds nothing.
                continue;
            }
            let price_eur = ct_to_eur(rate_ct);
            let net_eur = pos_net(umlage_base_kwh, price_eur);
            total += net_eur;
            positions.push(SettlementPosition {
                text: label.to_owned(),
                kind,
                quantity: umlage_base_kwh.round_dp(3),
                unit: QuantityUnit::Kwh,
                unit_price_eur: price_eur.round_dp(6),
                net_eur,        spot_price_formula: None,

                trace: CalculationTrace {
                    explanation: format!(
                        "{umlage_base_kwh:.3} kWh × {price_eur:.6} EUR/kWh = {:.5} EUR ({gruppe:?})",
                        (umlage_base_kwh * price_eur).round_dp(5),
                    ),
                    input_quantity: umlage_base_kwh,
                    input_unit_price_eur: price_eur,
                    gross_eur: umlage_base_kwh * price_eur,
                    legal_refs: vec![legal, LegalReference::EnFG {
                        paragraph: "§§21 ff.",
                    }],
                    tariff_source: None,
                    regulatory_reduction_factor: None,
                    rounding_note: None,
                },
            });
        }
    }

    // Konzessionsabgabe (KAV §2 Abs. 2)
    let ka_base_kwh = input.arbeitspreis.menge_kwh();
    if let Some(ka) = input.konzessionsabgabe {
        let ka_ct = ka.satz_ct_per_kwh;
        let gruppe = ka.klasse;
        if ka_ct < Decimal::ZERO {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "KA_NEGATIVE_RATE",
                message: format!("KA rate {ka_ct} ct/kWh is negative — verify tariff sheet"),
            });
        }
        // KAV §2 rates are Höchstbeträge, so a rate above the statutory ceiling
        // is a compliance defect, not merely unusual. Because the rate and the
        // customer group now arrive together, this check can no longer be
        // skipped — it used to be conditional on a separately-optional group,
        // which is precisely when an over-charge goes unnoticed.
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
        let ka_klasse_note = format!(" ({})", gruppe.label());
        let p = SettlementPosition {
            text: format!("Konzessionsabgabe{ka_klasse_note}"),
            kind: BillingPositionKind::Konzessionsabgabe,
            quantity: ka_base_kwh.round_dp(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: ct_to_eur(ka_ct).round_dp(6),
            net_eur: pos_net(ka_base_kwh, ct_to_eur(ka_ct)),
            spot_price_formula: None,
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
                    paragraph: gruppe.kav_paragraph(),
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
    let modul3_intervalle: &[Sect14aModul3Interval] = match &input.arbeitspreis {
        ArbeitspreisModell::Modul3Spotpreis { intervalle } => intervalle,
        _ => &[],
    };
    for interval in modul3_intervalle.iter() {
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
        // The formula as a value, not as somebody's document schema. An adapter
        // that needs BO4E `LastvariablePreisposition` builds it from this.
        let formula = SpotPriceFormula {
            reference: PriceReference::Energiemenge,
            unit: QuantityUnit::Kwh,
            method: TariffCalculationMethod::Spotpreis,
            steps: vec![PriceStep {
                from: Decimal::ZERO,
                to: None,
                unit_price_eur: rate_eur,
            }],
        };

        let mut explanation = format!(
            "{:.3} kWh × {:.6} EUR/kWh (§14a Modul 3 Spotpreis, interval {}/{}) = {:.5} EUR",
            interval.menge_kwh, rate_eur, from_str, to_str, net
        );
        if let Some(epex) = interval.epex_spot_ct_per_kwh {
            explanation.push_str(&format!(" [EPEX {epex:.4} ct/kWh]"));
        }

        let p = SettlementPosition {
            text: label,
            kind: BillingPositionKind::NneArbeitModul3,
            quantity: interval.menge_kwh.round_dp(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: rate_eur.round_dp(6),
            net_eur: net,
            spot_price_formula: Some(formula),
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

    let total_eur = total.round_dp(2);
    ensure_representable_eur(total_eur)?;

    let result = SettlementResult {
        malo_id: input.malo_id.clone(),
        sparte: input.sparte,
        regime: crate::regulatory::RegulatoryRegime::for_period(
            input.period.from(),
            input.period.to(),
        ),
        settlement_type,
        status: SettlementStatus::Initial,
        period: input.period,
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
pub fn settle_mmm(input: &MmmInput) -> Result<SettlementResult, BillingError> {
    if input.period.from() >= input.period.to() {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }

    let mehr_eur = ct_to_eur(input.mehr_preis_ct_per_kwh);
    let minder_eur = ct_to_eur(input.minder_preis_ct_per_kwh);
    let diff = input.actual_kwh - input.profil_kwh;

    // Resolved once from the period; every decision below matches on the regime
    // rather than re-comparing dates, so a future turnover is a new variant the
    // compiler makes us handle everywhere it matters.
    let mut warnings: Vec<SettlementWarning> = Vec::new();
    let regime =
        crate::regulatory::RegulatoryRegime::for_period(input.period.from(), input.period.to());
    if crate::regulatory::RegulatoryRegime::straddles_turnover(
        input.period.from(),
        input.period.to(),
    ) {
        warnings.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "REGIME_TURNOVER_IN_PERIOD",
            message: "the delivery period crosses a regulatory turnover; different \
                      rules govern its start and its end — split the period"
                .to_owned(),
        });
    }
    use crate::regulatory::NetzzugangRegime as NZ;
    let mmm_refs = match (input.sparte, regime.netzzugang()) {
        (Sparte::Gas, NZ::Nzv) => vec![
            LegalReference::GasNzv { paragraph: "§25" },
            LegalReference::BdewAhb {
                reference: "GaBi Gas 2.1 (BK7-24-01-008)",
            },
        ],
        (Sparte::Gas, NZ::EnwgFestlegung) => vec![LegalReference::BdewAhb {
            reference: "GaBi Gas 2.1 (BK7-24-01-008)",
        }],
        (Sparte::Strom, NZ::Nzv) => vec![
            LegalReference::StromNzv {
                paragraph: "§13 Abs. 3",
            },
            LegalReference::BnetzaDecision {
                reference: "BK6-24-174",
            },
        ],
        (Sparte::Strom, NZ::EnwgFestlegung) => vec![
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
    let p1 = SettlementPosition {
        text: "Mehrmengen (Gutschrift)".to_owned(),
        kind: BillingPositionKind::Mehrmenge,
        quantity: mehr_kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: mehr_eur.round_dp(6),
        net_eur: mehr_net,
        spot_price_formula: None,

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
    let p2 = SettlementPosition {
        text: "Mindermengen".to_owned(),
        kind: BillingPositionKind::Mindermenge,
        quantity: minder_kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: minder_eur.round_dp(6),
        net_eur: minder_net,
        spot_price_formula: None,

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
    ensure_representable_eur(total_eur.abs())?;

    Ok(SettlementResult {
        malo_id: input.malo_id.clone(),
        sparte: input.sparte,
        regime: crate::regulatory::RegulatoryRegime::for_period(
            input.period.from(),
            input.period.to(),
        ),
        settlement_type: mmm_settlement_type,
        status: SettlementStatus::Initial,
        period: input.period,
        nb_mp_id: input.nb_mp_id.clone(),
        counterparty_mp_id: input.lf_mp_id.clone(),
        positions: vec![p1, p2],
        total_eur,
        warnings,
    })
}

// ── MSB invoice (PID 31009) ───────────────────────────────────────────────────

/// Calculate a MSB-Rechnung (PID 31009): NB → MSB metering service settlement.
///
/// ## Legal references
///
/// - Grundgebühr Messstellenbetrieb → `MsbG §§6–7`, `MsbG §2`
/// - Messdienstleistung → `MsbG §2`
///
/// ## Errors
///
/// [`BillingError::InvalidInput`] or [`BillingError::MonetaryOverflow`].
#[must_use = "handle the BillingError"]
pub fn settle_msb(input: &MsbInput) -> Result<SettlementResult, BillingError> {
    if input.period.from() >= input.period.to() {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.grundgebuehr_eur_per_month < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "grundgebuehr_eur_per_month must be non-negative".to_owned(),
        });
    }
    // §30 MsbG Preisobergrenze. The ceiling is annual and the charge monthly, so
    // the charge is annualised before comparison — billing a year in monthly
    // instalments does not raise the cap.
    let mut warnings: Vec<SettlementWarning> = Vec::new();
    if let (Some(kategorie), Some(schuldner)) =
        (input.messstellen_kategorie, input.entgeltschuldner)
    {
        let annual = input.grundgebuehr_eur_per_month * Decimal::from(12);
        if let Some(pog) = crate::msbg::preisobergrenze_eur_per_jahr(kategorie, schuldner)
            && annual > pog
        {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "MSB_ABOVE_MSBG_POG",
                message: format!(
                    "Messstellenbetrieb {annual} EUR/a exceeds the §30 MsbG                      Preisobergrenze {pog} EUR/a for {kategorie:?} / {schuldner:?}"
                ),
            });
        }
    }

    if input.billing_months == 0 {
        return Err(BillingError::InvalidInput {
            reason: "billing_months must be at least 1".to_owned(),
        });
    }

    let mut positions: Vec<SettlementPosition> = Vec::new();
    let mut total = Decimal::ZERO;

    let months = Decimal::from(input.billing_months);
    let p = monat_pos_traced(
        "Grundgebühr Messstellenbetrieb",
        BillingPositionKind::MsbGrundgebuehr,
        months,
        input.grundgebuehr_eur_per_month,
        vec![
            LegalReference::MsbG {
                paragraph: "§§6–7"
            },
            LegalReference::MsbG { paragraph: "§30" },
        ],
        None,
    );
    total += p.net_eur;
    positions.push(p);

    if let Some(msl_eur) = input.messdienstleistung_eur {
        let msl: Decimal = msl_eur.round_dp(5);
        let p = SettlementPosition {
            text: "Messdienstleistung".to_owned(),
            kind: BillingPositionKind::Messdienstleistung,
            quantity: Decimal::ONE,
            unit: QuantityUnit::Monat,
            unit_price_eur: msl,
            net_eur: msl,
            spot_price_formula: None,

            trace: CalculationTrace {
                explanation: format!("Messdienstleistung Pauschale {msl:.5} EUR"),
                input_quantity: Decimal::ONE,
                input_unit_price_eur: msl,
                gross_eur: msl,
                legal_refs: vec![LegalReference::MsbG {
                    paragraph: "§§34–35",
                }],
                tariff_source: None,
                regulatory_reduction_factor: None,
                rounding_note: Some("flat fee — rounded to 5 dp"),
            },
        };
        total += p.net_eur;
        positions.push(p);
    }

    let total_eur = total.round_dp(2);
    ensure_representable_eur(total_eur)?;

    Ok(SettlementResult {
        malo_id: input.malo_id.clone(),
        // Messstellenbetrieb is billed per metering point, not per commodity.
        sparte: Sparte::Strom,
        regime: crate::regulatory::RegulatoryRegime::for_period(
            input.period.from(),
            input.period.to(),
        ),
        settlement_type: SettlementType::MsbRechnung,
        status: SettlementStatus::Initial,
        period: input.period,
        nb_mp_id: input.nb_mp_id.clone(),
        counterparty_mp_id: input.msb_mp_id.clone(),
        positions,
        total_eur,
        warnings,
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
/// # use grid_billing::{SettlementResult, reverse};
/// # let original: SettlementResult = unimplemented!();
/// let reversal = reverse(&original);
/// assert_eq!(reversal.total_eur, -original.total_eur);
/// ```
#[must_use]
pub fn reverse(original: &SettlementResult) -> SettlementResult {
    use crate::types::SettlementStatus;
    let reversed_positions: Vec<_> = original
        .positions
        .iter()
        .map(|p| SettlementPosition {
            text: format!("Storno: {}", p.text),
            kind: p.kind,
            quantity: p.quantity,
            unit: p.unit,
            unit_price_eur: p.unit_price_eur,
            net_eur: -p.net_eur,
            spot_price_formula: p.spot_price_formula.clone(),
            trace: CalculationTrace {
                explanation: format!("Storno: {} (negated)", p.trace.explanation),
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

    SettlementResult {
        // A reversal is the same supply under the same rules — only the signs
        // differ, so identity and regime carry over unchanged.
        malo_id: original.malo_id.clone(),
        sparte: original.sparte,
        regime: original.regime,
        settlement_type: original.settlement_type,
        status: SettlementStatus::Reversal,
        period: original.period,
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
/// - `replacement` is the new calculation, carrying `status = Correction`.
///
/// Which document supersedes which is recorded on the [`crate::types::InvoiceDocument`]s built
/// around these two results, not here: the correction chain is a property of the
/// documents exchanged, and the same pair of settlements could be presented
/// under different invoice numbers.
///
/// ## Example
///
/// ```rust,no_run
/// # use grid_billing::{SettlementResult, correct};
/// # let original: SettlementResult = unimplemented!();
/// # let corrected: SettlementResult = unimplemented!();
/// let (reversal, replacement) = correct(&original, corrected);
/// assert_eq!(reversal.total_eur, -original.total_eur);
/// assert_eq!(replacement.status, grid_billing::SettlementStatus::Correction);
/// ```
#[must_use]
pub fn correct(
    original: &SettlementResult,
    mut replacement: SettlementResult,
) -> (SettlementResult, SettlementResult) {
    let reversal = reverse(original);
    replacement.status = SettlementStatus::Correction;
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
pub fn settle_gas_awh(input: &GasAwhInput) -> Result<SettlementResult, BillingError> {
    if input.period.from() >= input.period.to() {
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

    let mut positions: Vec<SettlementPosition> = Vec::new();
    let mut total = Decimal::ZERO;

    for awh in input.awh_positionen.iter() {
        let qty = Decimal::from(awh.anzahl);
        let gross = qty * awh.preis_eur;
        let net = pos_net(qty, awh.preis_eur);
        positions.push(SettlementPosition {
            text: awh.beschreibung.clone(),
            kind: BillingPositionKind::GasAwhSonstige, // service layer refines if artikel_id present
            quantity: qty,
            unit: QuantityUnit::Monat, // AWH positions have no standard EDIFACT unit; Monat placeholder
            unit_price_eur: awh.preis_eur.round_dp(6),
            net_eur: net,
            spot_price_formula: None,

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
    ensure_representable_eur(total_eur)?;

    let result = SettlementResult {
        malo_id: input.malo_id.clone(),
        // AWH Sperrprozesse are a Gas process (GeLi Gas 3.0 §5.4).
        sparte: Sparte::Gas,
        regime: crate::regulatory::RegulatoryRegime::for_period(
            input.period.from(),
            input.period.to(),
        ),
        settlement_type: SettlementType::GasAwhSperrung,
        status: SettlementStatus::Initial,
        period: input.period,
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
/// Build a §14a Modul 1 Arbeitspreis over the standard test basis.
fn modul1(factor: Decimal) -> ArbeitspreisModell {
    use crate::types::{MengePreis, Reduktionsfaktor};
    ArbeitspreisModell::Modul1Pauschal {
        basis: MengePreis {
            menge_kwh: rust_decimal::dec!(1500),
            preis_ct_per_kwh: rust_decimal::dec!(3.5),
        },
        reduktion: Reduktionsfaktor::new(factor).expect("valid reduction factor"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AwhPositionInput, GasAwhInput, InvoiceDocument, SettlementPeriod, validate_msb_input,
    };
    use crate::types::{
        GemeindeGroesse, Grundpreis, Konzessionsabgabe, Leistungspreis, MengePreis,
    };
    use rust_decimal::Decimal;
    use rust_decimal::dec;
    use time::macros::date;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("valid decimal literal")
    }

    fn base_nne() -> NneInput {
        NneInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("3.5"),
            }),
            leistungspreis: None,
            letztverbrauchergruppe: Default::default(),
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: Sparte::Strom,
        }
    }

    fn base_msb() -> MsbInput {
        MsbInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            period: SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
            grundgebuehr_eur_per_month: d("3.00"),
            billing_months: 1,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        }
    }

    fn base_mmm() -> MmmInput {
        MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            sparte: Sparte::Strom,
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        }
    }

    #[test]
    fn nne_slp_no_ka_arithmetic() {
        let r = settle_nne(&base_nne()).unwrap();
        assert_eq!(r.total_eur, d("52.50"));
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].unit, QuantityUnit::Kwh);
        assert_eq!(r.positions[0].net_eur, d("52.50000"));
    }

    #[test]
    fn nne_slp_with_ka() {
        let mut i = base_nne();
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.total_eur, d("54.15"));
        assert_eq!(r.positions.len(), 2);
        // The position names the KAV group, because the group now always
        // accompanies the rate — which is what lets the Höchstbetrag be checked.
        assert_eq!(
            r.positions[1].text,
            "Konzessionsabgabe (KAV §2 Abs. 3 Sondervertragskunde)"
        );
    }

    #[test]
    fn nne_rlm_with_leistungspreis() {
        let mut i = base_nne();
        i.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("12.5"),
            preis_eur_per_kw: d("4.20"),
        });
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.total_eur, d("106.65"));
        assert_eq!(r.positions.len(), 3);
        assert_eq!(r.positions[1].unit, QuantityUnit::Kw);
    }

    #[test]
    fn nne_sect14a_tou_arithmetic() {
        let mut i = base_nne();
        i.arbeitspreis = ArbeitspreisModell::Modul2ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("900"),
                preis_ct_per_kwh: d("4.0"),
            },
            nt: MengePreis {
                menge_kwh: d("600"),
                preis_ct_per_kwh: d("2.0"),
            },
        };
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.total_eur, d("48.00"));
        assert_eq!(r.positions.len(), 2);
        assert_eq!(r.positions[0].text, "Netznutzung Arbeit HT (§14a Modul 2)");
        assert_eq!(r.positions[0].net_eur, d("36.00000"));
        assert_eq!(r.positions[1].net_eur, d("12.00000"));
    }

    /// An inverted period cannot reach the engine at all.
    ///
    /// Constructing `SettlementPeriod` is the check, so the five per-calculation
    /// guards that used to re-test the same thing are gone with it.
    #[test]
    fn an_inverted_period_is_unrepresentable() {
        assert!(matches!(
            SettlementPeriod::new(date!(2025 - 01 - 31), date!(2025 - 01 - 01)),
            Err(BillingError::InvalidInput { .. })
        ));
        // A single day is a valid period, not an inverted one.
        assert!(SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 01)).is_ok());
    }

    /// A Bandlast agreement takes the Netzentgelt down to the agreed fraction,
    /// leaves the KA and levies whole, and records the factor in the trace.
    #[test]
    fn a_sect19_agreement_reduces_only_the_netzentgelt() {
        use crate::sect19::{Sect19Art, Sect19Vereinbarung};

        let mut i = base_nne();
        // 12 GWh at 1500 kW → 8000 h: the 10 % floor tier.
        i.jahresarbeit_kwh = Some(d("12000000"));
        i.jahreshoechstleistung_kw = Some(d("1500"));
        i.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("1500"),
            preis_eur_per_kw: d("10.00"),
        });
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        i.sect19 = Some(Sect19Vereinbarung {
            art: Sect19Art::IntensiveNetznutzung,
            vereinbarter_prozentsatz: d("0.10"),
            genehmigung: Some("BK4-22-089".to_owned()),
        });

        let r = settle_nne(&i).expect("settles");
        let reduction = r
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::Sect19IndividuellesEntgelt)
            .expect("the reduction position exists");

        // Netzentgelt basis: 1500 kWh × 0.035 + 1500 kW × 10 = 52.50 + 15000.
        // Reduction: −90 % of 15052.50 = −13547.25.
        assert_eq!(reduction.net_eur, d("-13547.25000"));
        assert_eq!(
            reduction.trace.regulatory_reduction_factor,
            Some(d("0.10")),
            "the agreed fraction is in the trace"
        );
        // The KA position is untouched by the reduction.
        let ka = r
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::Konzessionsabgabe)
            .expect("KA still billed");
        assert!(ka.net_eur > Decimal::ZERO);
        // 10 % is exactly the floor at 8000 h — no warning.
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.code == "SECT19_BELOW_MINDESTENTGELT"),
            "{:?}",
            r.warnings
        );
    }

    /// Below the statutory floor the settlement still computes, but says so.
    #[test]
    fn an_agreement_below_the_floor_is_reported() {
        use crate::sect19::{Sect19Art, Sect19Vereinbarung};

        let mut i = base_nne();
        // 10 GWh at ~1408 kW → 7102 h: the 20 % tier.
        i.jahresarbeit_kwh = Some(d("10000000"));
        i.jahreshoechstleistung_kw = Some(d("1408"));
        i.sect19 = Some(Sect19Vereinbarung {
            art: Sect19Art::IntensiveNetznutzung,
            vereinbarter_prozentsatz: d("0.10"),
            genehmigung: None,
        });
        let r = settle_nne(&i).expect("settles");
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "SECT19_BELOW_MINDESTENTGELT"),
            "10 % agreed where the floor is 20 %: {:?}",
            r.warnings
        );
    }

    /// A gas capacity charge is pro-rated by calendar days over the year.
    #[test]
    fn a_gas_capacity_charge_is_pro_rated_by_days() {
        use crate::gas::{Druckstufe, GasKapazitaet, Kapazitaetsprodukt};

        let mut i = base_nne();
        i.sparte = Sparte::Gas;
        // base period is January 2025: 31 days.
        i.gas_kapazitaet = Some(GasKapazitaet {
            bestellte_kapazitaet_kwh_h: d("500"),
            entgelt_eur_per_kwh_h_a: d("14.60"),
            produkt: Kapazitaetsprodukt::Unterbrechbar,
            druckstufe: Some(Druckstufe::Mitteldruck),
        });
        let r = settle_nne(&i).expect("settles");
        let kap = r
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::GasKapazitaetsentgelt)
            .expect("capacity position exists");
        // 14.60 × 31/365 = 1.24 EUR per kWh/h; × 500 = 620.00.
        assert_eq!(kap.unit_price_eur, d("1.24"));
        assert_eq!(kap.net_eur, d("620.00000"));
        assert!(
            kap.trace
                .legal_refs
                .iter()
                .any(|lr| lr.citation().contains("GasNEV §15 Abs. 5")),
            "interruptible capacity cites Abs. 5: {:?}",
            kap.trace.legal_refs
        );
        assert!(kap.text.contains("Mitteldruck"));
    }

    /// Supplied on Strom, the gas structure is refused with a warning, not billed.
    #[test]
    fn a_gas_capacity_charge_on_strom_is_not_billed() {
        use crate::gas::{GasKapazitaet, Kapazitaetsprodukt};

        let mut i = base_nne();
        i.gas_kapazitaet = Some(GasKapazitaet {
            bestellte_kapazitaet_kwh_h: d("500"),
            entgelt_eur_per_kwh_h_a: d("14.60"),
            produkt: Kapazitaetsprodukt::Fest,
            druckstufe: None,
        });
        let r = settle_nne(&i).expect("settles");
        assert!(
            !r.positions
                .iter()
                .any(|p| p.kind == BillingPositionKind::GasKapazitaetsentgelt)
        );
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "GAS_KAPAZITAET_ON_STROM")
        );
    }

    /// A demand charge is a pair, so half of one cannot be built.
    ///
    /// This used to be a runtime error checked in two separate places; the
    /// `Leistungspreis` type is now the check.
    #[test]
    fn a_demand_charge_is_a_pair() {
        let mut i = base_nne();
        i.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("10"),
            preis_eur_per_kw: d("4.20"),
        });
        let r = settle_nne(&i).expect("a complete pair settles");
        assert!(
            r.positions
                .iter()
                .any(|p| p.kind == BillingPositionKind::NneLeistung),
            "the demand charge must be billed"
        );
    }

    /// measured > profiled is an **ungewollte Mindermenge** — the NB supplied the
    /// shortfall and invoices it. GPKE (BK6-24-174) Teil 1 Kap. 8.4 Nr. 3.
    #[test]
    fn over_consumption_is_a_mindermenge_charge() {
        let input = MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            sparte: Sparte::Strom,
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let r = settle_mmm(&input).unwrap();
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
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            sparte: Sparte::Strom,
            actual_kwh: d("1400"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let r = settle_mmm(&input).unwrap();
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
            let r = settle_mmm(&i).unwrap();
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
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 1,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let r = settle_msb(&input).unwrap();
        assert_eq!(r.total_eur, d("12.50"));
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].unit, QuantityUnit::Monat);
    }

    #[test]
    fn msb_with_messdienstleistung() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 03 - 31)).unwrap(),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 3,
            messdienstleistung_eur: Some(d("8.00")),
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let r = settle_msb(&input).unwrap();
        assert_eq!(r.total_eur, d("45.50"));
        assert_eq!(r.positions.len(), 2);
    }

    /// The Prüfidentifikator is a property of the document, not the settlement.
    ///
    /// It used to be a mutable field the caller patched after calculation —
    /// netzbilanzd set 31005 for Gas and 31011 for AWH that way. It now lives on
    /// `InvoiceDocument`, where routing information belongs.
    #[test]
    fn the_pid_lives_on_the_document_not_the_settlement() {
        let settlement = settle_nne(&base_nne()).unwrap();
        let doc = InvoiceDocument {
            settlement,
            pid: 31005,
            rechnungsnummer: "NNE-2025-001".to_owned(),
            correction_of: None,
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
        };
        assert_eq!(doc.pid, 31005);
        // and numbering is assigned at rendering time
        let numbers: Vec<u32> = doc.numbered_positions().map(|(n, _)| n).collect();
        assert_eq!(numbers.first(), Some(&1));
    }

    // ── New: explainability and audit trail tests ─────────────────────────────

    #[test]
    fn nne_slp_has_legal_reference_stromnev() {
        let r = settle_nne(&base_nne()).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("StromNEV")),
            "expected StromNEV reference, got: {refs:?}"
        );
    }

    #[test]
    fn nne_ka_has_kav_reference() {
        let mut i = base_nne();
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let r = settle_nne(&i).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("KAV")),
            "expected KAV reference, got: {refs:?}"
        );
    }

    #[test]
    fn nne_tou_has_sect14a_reference() {
        let mut i = base_nne();
        i.arbeitspreis = ArbeitspreisModell::Modul2ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("900"),
                preis_ct_per_kwh: d("4.0"),
            },
            nt: MengePreis {
                menge_kwh: d("600"),
                preis_ct_per_kwh: d("2.0"),
            },
        };
        let r = settle_nne(&i).unwrap();
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
        let input = base_mmm();
        let r = settle_mmm(&input).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("StromNZV")),
            "expected StromNZV reference for a 2025 period, got: {refs:?}"
        );
    }

    /// A metering charge above the §30 MsbG Preisobergrenze is reported.
    ///
    /// It used to be unchecked — the MSB settlement validated only that the fee
    /// was non-negative, while the analogous KAV ceiling *was* checked. Both are
    /// Höchstbeträge, and an amount above either is one the customer may reclaim.
    #[test]
    fn a_metering_charge_above_the_msbg_ceiling_is_reported() {
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtBand};

        let mut i = base_msb();
        i.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtBand::Bis10000));
        i.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);

        // 40 EUR/a is the ceiling for this band; 5 EUR/month is 60 EUR/a.
        i.grundgebuehr_eur_per_month = d("5.00");
        let over = settle_msb(&i).expect("settles");
        assert!(
            over.warnings.iter().any(|w| w.code == "MSB_ABOVE_MSBG_POG"),
            "60 EUR/a exceeds the 40 EUR/a ceiling: {:?}",
            over.warnings
        );

        // 3 EUR/month is 36 EUR/a — within it.
        i.grundgebuehr_eur_per_month = d("3.00");
        let within = settle_msb(&i).expect("settles");
        assert!(
            !within
                .warnings
                .iter()
                .any(|w| w.code == "MSB_ABOVE_MSBG_POG"),
            "36 EUR/a is within the ceiling: {:?}",
            within.warnings
        );
    }

    /// Annualising is what makes the comparison right.
    ///
    /// The ceiling is per year and the charge per month; billing a year in
    /// instalments does not raise the cap.
    #[test]
    fn the_ceiling_is_compared_against_the_annualised_charge() {
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtBand};

        let mut i = base_msb();
        i.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtBand::Bis100000));
        i.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);
        // 140 EUR/a ceiling. 12 EUR/month = 144 EUR/a — over, even though a
        // single month is far below the annual figure.
        i.grundgebuehr_eur_per_month = d("12.00");
        let r = settle_msb(&i).expect("settles");
        assert!(r.warnings.iter().any(|w| w.code == "MSB_ABOVE_MSBG_POG"));
    }

    #[test]
    fn msb_has_msbg_reference() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 1,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let r = settle_msb(&input).unwrap();
        let refs = r.all_legal_refs();
        assert!(
            refs.iter().any(|r| r.contains("MsbG")),
            "expected MsbG reference, got: {refs:?}"
        );
    }

    #[test]
    fn calculation_trace_explanation_non_empty() {
        let r = settle_nne(&base_nne()).unwrap();
        for pos in &r.positions {
            assert!(
                !pos.trace.explanation.is_empty(),
                "every position must explain itself: {}",
                pos.text
            );
        }
    }

    #[test]
    fn settlement_type_and_status_set() {
        let r = settle_nne(&base_nne()).unwrap();
        assert_eq!(r.settlement_type, SettlementType::NneStrom);
        assert_eq!(r.status, SettlementStatus::Initial);
    }

    #[test]
    fn recomputed_total_matches_total_eur() {
        let mut i = base_nne();
        i.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("12.5"),
            preis_eur_per_kw: d("4.20"),
        });
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let r = settle_nne(&i).unwrap();
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
        let r = settle_nne(&i).unwrap();
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
        i.arbeitspreis = ArbeitspreisModell::Modul2ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("4.0"),
            },
            nt: MengePreis {
                menge_kwh: d("0"),
                preis_ct_per_kwh: d("2.0"),
            },
        };
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.positions[1].net_eur, Decimal::ZERO);
    }

    /// A Strom NNE invoice for a covered year carries all three network levies.
    ///
    /// 1500 kWh at the 2026 A′ rates: §19 1.559 + Offshore 0.941 + KWKG 0.446
    /// = 2.946 ct/kWh → 44.19 EUR on top of the Arbeitspreis.
    #[test]
    fn a_covered_year_bills_all_three_network_levies() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
        i.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::A;
        let r = settle_nne(&i).unwrap();

        let levies: Vec<_> = r
            .positions
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    BillingPositionKind::Sect19StromNevUmlage
                        | BillingPositionKind::OffshoreNetzumlage
                        | BillingPositionKind::KwkgUmlage
                )
            })
            .collect();
        assert_eq!(levies.len(), 3, "all three levies must appear");

        let levy_total: Decimal = levies.iter().map(|p| p.net_eur).sum();
        assert_eq!(levy_total.round_dp(2), dec!(44.19));
        assert!(r.is_clean(), "a covered year must raise no warning");
    }

    /// §21 EnFG exempts entirely — no line at all rather than a zero one.
    #[test]
    fn an_exempt_entnahmestelle_carries_no_levy_line() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
        i.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::Befreit;
        let r = settle_nne(&i).unwrap();

        assert!(
            !r.positions.iter().any(|p| matches!(
                p.kind,
                BillingPositionKind::Sect19StromNevUmlage
                    | BillingPositionKind::OffshoreNetzumlage
                    | BillingPositionKind::KwkgUmlage
            )),
            "an exempt Entnahmestelle must carry no levy line"
        );
    }

    /// A year the series does not cover omits the levy and says so.
    #[test]
    fn an_uncovered_year_warns_rather_than_billing_zero() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2027 - 01 - 01), date!(2027 - 01 - 31)).unwrap();
        let r = settle_nne(&i).unwrap();

        let missing = r
            .warnings
            .iter()
            .filter(|w| w.code == "UMLAGE_RATE_MISSING")
            .count();
        assert_eq!(missing, 3, "each unresolvable levy must be reported");
    }

    /// An override wins over the tabled rate — the EnFG-decision escape hatch.
    #[test]
    fn an_explicit_rate_overrides_the_tabled_one() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
        i.sect19_umlage_ct_per_kwh = Some(dec!(0.100));
        let r = settle_nne(&i).unwrap();

        let sect19 = r
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::Sect19StromNevUmlage)
            .expect("§19 position");
        // 1500 kWh × 0.100 ct/kWh = 1.50 EUR, not the tabled 23.39.
        assert_eq!(sect19.net_eur.round_dp(2), dec!(1.50));
    }

    #[test]
    fn settlement_is_clean_with_valid_inputs() {
        let r = settle_nne(&base_nne()).unwrap();
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
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.settlement_type, SettlementType::NneGas);
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
        let r = settle_nne(&base_nne()).unwrap();
        assert_eq!(r.counterparty_mp_id, "9900012345678");
    }

    #[test]
    fn counterparty_mp_id_is_msb_for_msb_invoice() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900999000001".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            grundgebuehr_eur_per_month: d("15.00"),
            billing_months: 1,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let r = settle_msb(&input).unwrap();
        assert_eq!(r.counterparty_mp_id, "9900999000001");
    }

    #[test]
    fn reversal_negates_all_positions_and_total() {
        let original = settle_nne(&base_nne()).unwrap();
        let storno = reverse(&original);
        assert_eq!(storno.total_eur, -original.total_eur);
        assert_eq!(storno.status, SettlementStatus::Reversal);
        for (orig, rev) in original.positions.iter().zip(storno.positions.iter()) {
            assert_eq!(rev.net_eur, -orig.net_eur);
            assert!(rev.text.starts_with("Storno:"));
        }
    }

    #[test]
    fn reversal_preserves_counterparty_mp_id() {
        let original = settle_nne(&base_nne()).unwrap();
        let storno = reverse(&original);
        assert_eq!(storno.counterparty_mp_id, original.counterparty_mp_id);
    }

    #[test]
    fn ka_gruppe_annotation_appears_in_position_text() {
        let mut i = base_nne();
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.09"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        if let Some(ka) = i.konzessionsabgabe.as_mut() {
            ka.klasse = KaKundengruppe::Sondervertragskunde;
        }
        let r = settle_nne(&i).unwrap();
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
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("1.32"), // the Tarifkunde ≤25k rate
            klasse: KaKundengruppe::Tarifkunde {
                gemeinde: GemeindeGroesse::Bis25k,
                nur_kochen_warmwasser: false,
            },
        });
        if let Some(ka) = i.konzessionsabgabe.as_mut() {
            ka.klasse = KaKundengruppe::Sondervertragskunde;
        }
        let r = settle_nne(&i).unwrap();
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
        let r = settle_mmm(&i).unwrap();
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
            i.period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
            let r = settle_mmm(&i).unwrap();
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
    fn validate_msb_zero_months_is_error() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 0,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let v = validate_msb_input(&input);
        assert!(!v.is_valid);
        assert!(v.warnings.iter().any(|w| w.code == "ZERO_BILLING_MONTHS"));
    }

    #[test]
    fn reversal_of_rlm_matches_negative_total() {
        let mut i = base_nne();
        i.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("12.5"),
            preis_eur_per_kw: d("4.20"),
        });
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let original = settle_nne(&i).unwrap();
        let storno = reverse(&original);
        assert_eq!(storno.positions.len(), original.positions.len());
        assert_eq!(storno.total_eur, -original.total_eur);
        assert_eq!(storno.recomputed_total(), storno.total_eur);
    }

    // ── §14a Modul 1 (BNetzA BK6-22-300 flat reduction) ──────────────────────

    #[test]
    fn nne_sect14a_modul1_applies_reduction_factor() {
        // 1500 kWh × 3.5 ct/kWh × 0.85 = 1500 × 0.02975 EUR = 44.625 → 44.63
        let mut i = base_nne();
        i.arbeitspreis = modul1(d("0.85"));
        let r = settle_nne(&i).unwrap();
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
        i.arbeitspreis = modul1(d("1.0"));
        let r = settle_nne(&i).unwrap();
        // 1500 × 0.035 × 1.0 = 52.50 — same as plain Arbeit
        assert_eq!(r.total_eur, d("52.50"));
    }

    // ── Gas Grundpreis ────────────────────────────────────────────────────────

    #[test]
    fn nne_gas_with_grundpreis_adds_position() {
        let mut i = base_nne();
        i.sparte = Sparte::Gas;
        i.grundpreis = Some(Grundpreis {
            eur_per_month: d("15.00"),
            months: Decimal::from(1),
        });
        let r = settle_nne(&i).unwrap();
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

    // ── Gas AWH Sperrprozesse (PID 31011) ─────────────────────────────────────

    #[test]
    fn gas_awh_single_sperrung_arithmetic() {
        let input = GasAwhInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            tariff_sheet_id: None,
            awh_positionen: vec![AwhPositionInput {
                beschreibung: "Sperrung Gaszähler".into(),
                anzahl: 1,
                preis_eur: d("45.00"),
                artikel_id: Some("2-01-7-001".to_owned()),
            }],
        };
        let r = settle_gas_awh(&input).unwrap();
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
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
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
        let r = settle_gas_awh(&input).unwrap();
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
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            tariff_sheet_id: None,
            awh_positionen: vec![],
        };
        assert!(matches!(
            settle_gas_awh(&input),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    // ── Correction lifecycle ──────────────────────────────────────────────────

    #[test]
    fn correction_pair_status_and_reference() {
        let original = settle_nne(&base_nne()).unwrap();
        let mut corrected_input = base_nne();
        if let ArbeitspreisModell::Einheitlich(mp) = &mut corrected_input.arbeitspreis {
            mp.menge_kwh = d("1600");
        }
        let replacement = settle_nne(&corrected_input).unwrap();

        let (reversal, corrected) = correct(&original, replacement);
        assert_eq!(reversal.status, SettlementStatus::Reversal);
        assert_eq!(reversal.total_eur, -original.total_eur);
        assert_eq!(corrected.status, SettlementStatus::Correction);
    }

    // ── recomputed_total consistency ──────────────────────────────────────────

    #[test]
    fn nne_recomputed_total_matches_total_eur() {
        let r = settle_nne(&base_nne()).unwrap();
        assert_eq!(r.recomputed_total(), r.total_eur);
    }

    #[test]
    fn mmm_recomputed_total_matches_total_eur() {
        let r = settle_mmm(&base_mmm()).unwrap();
        assert_eq!(r.recomputed_total(), r.total_eur);
    }

    // ── Gas MMM uses MmmGas settlement type ───────────────────────────────────

    #[test]
    fn mmm_gas_uses_mmm_gas_settlement_type() {
        let mut i = base_mmm();
        i.sparte = Sparte::Gas;
        let r = settle_mmm(&i).unwrap();
        assert_eq!(
            r.settlement_type,
            SettlementType::MmmGas,
            "Gas MMM must use MmmGas settlement type"
        );
    }

    #[test]
    fn mmm_strom_uses_mmm_strom_settlement_type() {
        let r = settle_mmm(&base_mmm()).unwrap();
        assert_eq!(r.settlement_type, SettlementType::MmmStrom);
    }
}

// ── Property tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::types::SettlementPeriod;
    use crate::types::{MengePreis, Reduktionsfaktor};
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
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 12 - 31)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: kwh,
                preis_ct_per_kwh: ct,
            }),
            leistungspreis: None,
                letztverbrauchergruppe: Default::default(),
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
                tariff_sheet_id: None,
                sparte: Sparte::Strom,
            };
            if let Ok(original) = settle_nne(&input) {
                let reversal = reverse(&original);
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
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 12 - 31)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: kwh,
                preis_ct_per_kwh: ct,
            }),
            leistungspreis: None,
                letztverbrauchergruppe: Default::default(),
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
                tariff_sheet_id: None,
                sparte: Sparte::Strom,
            };
            if let Ok(unreduced) = settle_nne(&base) {
                let mut reduced_input = base.clone();
                reduced_input.arbeitspreis = ArbeitspreisModell::Modul1Pauschal {
                    basis: MengePreis {
                        menge_kwh: kwh,
                        preis_ct_per_kwh: ct,
                    },
                    reduktion: Reduktionsfaktor::new(factor).expect("factor is in (0,1]"),
                };
                if let Ok(reduced) = settle_nne(&reduced_input) {
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
    use crate::types::MengePreis;
    use crate::types::Sect14aModul3Interval;
    use crate::types::SettlementPeriod;
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
            period: SettlementPeriod::new(date!(2026 - 01 - 15), date!(2026 - 01 - 16)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("3.5"),
            }),
            leistungspreis: None,
            letztverbrauchergruppe: Default::default(),
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: Sparte::Strom,
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
        i.arbeitspreis = ArbeitspreisModell::Modul3Spotpreis {
            intervalle: vec![Sect14aModul3Interval {
                period_from: start,
                period_to: end,
                menge_kwh: d("2.5"),
                nne_rate_ct_per_kwh: d("1.80"),
                epex_spot_ct_per_kwh: Some(d("12.50")),
            }],
        };
        let r = settle_nne(&i).unwrap();

        // Flat Arbeit + one Modul 3 interval, plus the three network levies a
        // Strom NNE invoice for a covered year always carries.
        assert_eq!(
            r.positions.len(),
            4,
            "1 Modul 3 position + 3 Umlagen — and no flat Arbeit position: \
             the interval rates replace it rather than adding to it"
        );
        assert_eq!(
            r.positions
                .iter()
                .filter(|p| matches!(
                    p.kind,
                    BillingPositionKind::Sect19StromNevUmlage
                        | BillingPositionKind::OffshoreNetzumlage
                        | BillingPositionKind::KwkgUmlage
                ))
                .count(),
            3
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

        // The pricing formula is a value, not a serialised document. What an
        // auditor needs is the method and the rate that applied; how a BO4E
        // `LastvariablePreisposition` renders that is the adapter's problem.
        let formula = modul3_pos
            .spot_price_formula
            .as_ref()
            .expect("a Modul 3 position states the formula behind its rate");
        assert_eq!(formula.method, TariffCalculationMethod::Spotpreis);
        assert_eq!(formula.reference, PriceReference::Energiemenge);
        assert_eq!(formula.unit, QuantityUnit::Kwh);
        assert_eq!(formula.steps.len(), 1);
        assert_eq!(formula.steps[0].unit_price_eur, d("0.018"));
        assert_eq!(formula.steps[0].from, Decimal::ZERO);
        assert_eq!(formula.steps[0].to, None, "the top step is open");

        // The EPEX price that produced the rate stays in the trace, which is
        // where an auditor looks for inputs.
        assert!(
            modul3_pos.trace.explanation.contains("12.5"),
            "the spot price behind the rate must be recoverable: {}",
            modul3_pos.trace.explanation
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
        let base = time::OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let mut i = base_nne();
        i.arbeitspreis = ArbeitspreisModell::Modul3Spotpreis {
            intervalle: vec![
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
            ],
        };
        let r = settle_nne(&i).unwrap();

        // 2 Modul 3 intervals + the three network levies. The flat Arbeit
        // position is absent by design: billing it alongside the interval rates
        // charged the same energy twice.
        assert_eq!(r.positions.len(), 5);
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
        // Each interval states its own rate, so the two formulas differ.
        let f0 = modul3[0].spot_price_formula.as_ref().unwrap();
        let f1 = modul3[1].spot_price_formula.as_ref().unwrap();
        assert_eq!(f0.steps[0].unit_price_eur, d("0.02"));
        assert_eq!(f1.steps[0].unit_price_eur, d("0.015"));
    }

    #[test]
    fn nne_modul3_zero_kwh_interval_is_skipped() {
        let base = time::OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let mut i = base_nne();
        i.arbeitspreis = ArbeitspreisModell::Modul3Spotpreis {
            intervalle: vec![
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
            ],
        };
        let r = settle_nne(&i).unwrap();
        let modul3: Vec<_> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::NneArbeitModul3)
            .collect();
        assert_eq!(modul3.len(), 1, "zero-kWh interval must be skipped");
    }

    /// The §14a modules are mutually exclusive by construction.
    ///
    /// Modul 1 applies a flat reduction to the whole Arbeitsmenge; Modul 3 prices
    /// each dispatch interval. Both together billed the same energy twice, and
    /// the engine did it silently because the conflict check lived in a validator
    /// nothing called. `ArbeitspreisModell` now holds one model at a time, so the
    /// combination cannot be expressed.
    #[test]
    fn the_sect14a_modules_are_mutually_exclusive() {
        let base = time::OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();

        let mut i = base_nne();
        i.arbeitspreis = modul1(d("0.85"));
        assert_eq!(i.arbeitspreis.sect14a_modul(), Some(Sect14aModule::Modul1));

        // Assigning Modul 3 replaces Modul 1 rather than adding to it.
        i.arbeitspreis = ArbeitspreisModell::Modul3Spotpreis {
            intervalle: vec![Sect14aModul3Interval {
                period_from: base,
                period_to: base + time::Duration::minutes(15),
                menge_kwh: d("1.0"),
                nne_rate_ct_per_kwh: d("2.0"),
                epex_spot_ct_per_kwh: None,
            }],
        };
        assert_eq!(i.arbeitspreis.sect14a_modul(), Some(Sect14aModule::Modul3));

        // And the settlement bills the interval once, not the flat rate as well.
        let r = settle_nne(&i).expect("Modul 3 settles");
        let modul1_positions = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::NneArbeitModul1)
            .count();
        assert_eq!(
            modul1_positions, 0,
            "no flat Modul 1 position alongside Modul 3"
        );
    }
}
