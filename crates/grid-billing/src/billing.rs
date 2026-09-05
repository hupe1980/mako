//! NNE, MMM, and MSB settlement calculation logic.
//!
//! Amounts are computed in `rust_decimal::Decimal`; every EUR result is
//! range-checked through [`crate::EuroAmount`] for exact
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

use crate::EuroAmount;
use crate::rounding::RoundMoney;
use rust_decimal::Decimal;

use crate::error::BillingError;
use crate::types::{
    AbschlagInput, ArbeitspreisModell, BillingPositionKind, CalculationTrace, GasAwhInput,
    KaKundengruppe, KorrekturGrund, LegalReference, MmmInput, MsbInput, NneInput, PriceReference,
    PriceStep, QuantityUnit, Sect14aModule, SettlementPosition, SettlementResult, SettlementStatus,
    SettlementType, SettlementWarning, Sparte, SpotPriceFormula, SpotpreisInterval,
    TariffCalculationMethod, TariffSource, WarningSeverity,
};

// ── helpers ───────────────────────────────────────────────────────────────────

const HUNDRED: Decimal = Decimal::from_parts(100, 0, 0, false, 0);

fn ct_to_eur(ct: Decimal) -> Decimal {
    ct / HUNDRED
}

/// The net of a position, computed from the figures the position **states**.
///
/// A [`SettlementPosition`] presents its quantity at 3 dp and its unit price at
/// 6 dp, and the invoice recipient re-multiplies exactly those two. Computing
/// the net from unrounded inputs instead states a product of figures the
/// document does not carry, so every recipient's own arithmetic check
/// disagrees with it. Rounding here is idempotent for a caller that already
/// rounded.
pub(crate) fn pos_net(qty: Decimal, unit_price_eur: Decimal) -> Decimal {
    (qty.round_kfm(3) * unit_price_eur.round_kfm(6)).round_kfm(5)
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
        quantity: kwh.round_kfm(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: unit_price_eur.round_kfm(6),
        net_eur: pos_net(kwh, unit_price_eur),
        spot_price_formula: None,

        trace: CalculationTrace {
            explanation: format!(
                "{kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR",
                unit_price_eur,
                gross_eur.round_kfm(5)
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
        quantity: months.round_kfm(3),
        unit: QuantityUnit::Monat,
        unit_price_eur: unit_price_eur.round_kfm(6),
        net_eur: pos_net(months, unit_price_eur),
        spot_price_formula: None,

        trace: CalculationTrace {
            explanation: format!(
                "{months} Monate × {:.6} EUR/Monat = {:.5} EUR",
                unit_price_eur,
                gross_eur.round_kfm(5)
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

/// Push the `REGIME_TURNOVER_IN_PERIOD` warning when the delivery period
/// crosses a regulatory turnover (see [`crate::regulatory`]).
///
/// Such a period is governed by different rules at its start and its end, so a
/// single settlement over it applies the wrong rules to part of the supply.
/// Every settlement builder emits this the same way, so the caller learns to
/// split the period regardless of which document type it asked for.
pub(crate) fn warn_if_straddles_turnover(
    period_from: time::Date,
    period_to: time::Date,
    warnings: &mut Vec<SettlementWarning>,
) {
    if crate::regulatory::RegulatoryRegime::straddles_turnover(period_from, period_to) {
        warnings.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "REGIME_TURNOVER_IN_PERIOD",
            message: "the delivery period crosses a regulatory turnover; different \
                      rules govern its start and its end — split the period"
                .to_owned(),
        });
    }
}

// ── NNE invoice (PID 31002 — NN-Rechnung, Strom + Gas) ───────────────────────

/// Calculate a NNE settlement (PID 31002 NN-Rechnung, Strom and Gas).
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
/// | next | Netznutzung Arbeit HT + ST + NT (§14a Modul 3) | zeitvariables Netzentgelt (BK6-22-300) |
/// | next | Netznutzung Arbeit (§14a Modul 2, reduzierter Arbeitspreis) | prozentuale Reduzierung |
/// | next | Netznutzung Arbeit je Dispatch-Intervall (§14a Modul 3 Spot) | spot-priced mode |
/// | next | Netznutzung Arbeit | flat mode (no §14a) |
/// | next | Netznutzung Leistung (StromNEV §17, Jahresleistungspreis pro-rated by days) | RLM only |
/// | last | Konzessionsabgabe (KAV §2) | when `ka_satz_ct_per_kwh` set |
///
/// ## Legal references
///
/// - Gas Grundpreis position → `GasNEV §14`
/// - Arbeit positions → `StromNEV §21` (or `GasNEV §14` for Gas)
/// - §14a Modul 1 positions → `Sect14aEnwg { module: Modul1 }` + `BNetzA BK6-22-300`
/// - §14a Modul 2 position → `Sect14aEnwg { module: Modul2 }` + `BNetzA BK6-22-300`
/// - §14a Modul 3 positions (HT/ST/NT and Spot) → `Sect14aEnwg { module: Modul3 }` + `BNetzA BK6-22-300`
/// - Leistung position → `StromNEV §17` (Abs. 2 — a Jahresleistungspreis)
/// - Konzessionsabgabe → `KAV §2 Abs. 2`
///
/// ## §14a Modul 1 — pauschale Reduzierung
///
/// Select `ArbeitspreisModell::Modul1Pauschal` with the NB's published annual
/// amount and the fraction of a year the period covers: the energy is billed at
/// the full Arbeitspreis and the pauschale is credited pro rata alongside it.
///
/// **Known limitation.** BK6-22-300 permits Modul 1 alongside Modul 3, but
/// `ArbeitspreisModell` holds one model at a time, so that combination is not
/// yet representable. Modul 2 with Modul 3 is genuinely forbidden and stays
/// unrepresentable by design — see [`Sect14aModule::combinable_with`].
///
/// ## Errors
///
/// [`BillingError::InvalidInput`], [`BillingError::MonetaryOverflow`], or
/// [`BillingError::UnsupportedEntgeltRegime`] for a period governed by AgNeS
/// (from 01.01.2029), whose methodology is not yet festgelegt.
#[must_use = "handle the BillingError"]
pub fn settle_nne(input: &NneInput) -> Result<SettlementResult, BillingError> {
    // The period is ordered by construction, the Leistungspreis is paired by
    // construction, and the §14a modules are exclusive by construction — none of
    // those needs a runtime guard.
    //
    // What remains is what the types cannot express. It runs here rather than in
    // a validator the caller may skip: these are the errors that otherwise
    // produce a plausible-looking invoice billed on the wrong basis.
    if input.arbeitspreis.menge_kwh() < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "metered energy must be non-negative".to_owned(),
        });
    }
    if let ArbeitspreisModell::SpotpreisNetzentgelt { intervalle } = &input.arbeitspreis {
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
    // A published pauschale is an amount the Netzbetreiber grants, so it is
    // non-negative. A negative one silently inverts the position: the §14a
    // module the customer is entitled to becomes a charge against them.
    if let ArbeitspreisModell::Modul1Pauschal {
        pauschale_eur_pro_jahr,
        ..
    } = &input.arbeitspreis
        && *pauschale_eur_pro_jahr < Decimal::ZERO
    {
        return Err(BillingError::InvalidInput {
            reason: format!(
                "§14a Modul 1 pauschale must be non-negative, got {pauschale_eur_pro_jahr} \
                 EUR/Jahr — a negative pauschale charges the customer for the module"
            ),
        });
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

    // Resolved once from the period and recorded on the result. NNE positions
    // are priced on the Entgelt axis (StromNEV §§17/21, GasNEV §§14–15, the
    // §19 individual forms), which AgNeS replaces from 2029 — so a period the
    // Verordnung methodology no longer governs is refused here rather than
    // computed with lapsed math and merely tagged.
    let regime =
        crate::regulatory::RegulatoryRegime::for_period(input.period.from(), input.period.to());
    regime.ensure_berechenbar()?;

    let tariff_src = make_tariff_source(input.tariff_sheet_id.as_deref());
    let mut positions: Vec<SettlementPosition> = Vec::new();
    let mut total = Decimal::ZERO;
    let mut warnings: Vec<SettlementWarning> = Vec::new();
    warn_if_straddles_turnover(input.period.from(), input.period.to(), &mut warnings);

    // The share of a year credited has to be the share of a year served. The
    // range check on [`Jahresanteil`] cannot see the period, so `1` — a valid
    // share — passes it and credits the whole annual pauschale on every monthly
    // run, twelve times over the year.
    //
    // Loose on purpose, like the Messstellenbetrieb month count: a period may
    // run from the 15th to the 14th or cover a Gerätewechsel, so anything within
    // half a month of the period's own share passes. What it catches is the
    // order-of-magnitude mismatch, which is the error worth catching.
    if let ArbeitspreisModell::Modul1Pauschal { jahresanteil, .. } = &input.arbeitspreis {
        let aus_periode = input.period.jahresanteil();
        let anteil = jahresanteil.get();
        if (anteil - aus_periode).abs() > rust_decimal::dec!(0.042) {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "MODUL1_JAHRESANTEIL_MISMATCH",
                message: format!(
                    "crediting {anteil} of the §14a Modul 1 Jahrespauschale over a period of \
                     {} days (≈ {aus_periode:.6} of a year) — check the period or the \
                     Jahresanteil",
                    input.period.days()
                ),
            });
        }
    }

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

    // Gas Grundpreis / Verrechnungspreis (Gas NNE monthly standing charge per GasNEV).
    //
    // Sparte-guarded like the Kapazitätsentgelt below: the position kind, its
    // label and its GasNEV §14 citation are all gas-specific, so billing it on a
    // Strom settlement would put "Netzentgelt Grundpreis Gas" and a gas ordinance
    // on an electricity invoice.
    if let Some(gp) = input.grundpreis
        && gp.months > Decimal::ZERO
    {
        if input.sparte != Sparte::Gas {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "GRUNDPREIS_ON_STROM",
                message: "a Grundpreis was supplied on a Strom settlement — the Gas \
                          Verrechnungspreis position of §14 GasNEV does not apply to Strom"
                    .to_owned(),
            });
        } else {
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
                "billed on an Arbeitspreis alone at {} with {arbeit} kWh/a — §17 Abs. 6 \
                 StromNEV allows this only in Niederspannung up to 100 000 kWh/a",
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
            // The divisor is the actual length of the settlement year, not a
            // flat 365. §15 GasNEV fixes no day-count convention, so the only
            // defensible reading of an *annual* Entgelt is that a full year of
            // capacity costs exactly that Entgelt — a fixed 365 would bill a leap
            // year at 366/365 = 100.274 % of the price sheet's annual figure.
            let tage = Decimal::from(input.period.days());
            let anteil = input.period.jahresanteil();
            let price_eur = (kap.entgelt_eur_per_kwh_h_a * anteil).round_kfm(6);
            let net_eur = pos_net(kap.bestellte_kapazitaet_kwh_h, price_eur);
            let stufe = kap
                .druckstufe
                .map(|d| format!(", {}", d.label()))
                .unwrap_or_default();
            let p = SettlementPosition {
                text: format!("Kapazitätsentgelt Gas ({}{stufe})", kap.produkt.label()),
                kind: BillingPositionKind::GasKapazitaetsentgelt,
                quantity: kap.bestellte_kapazitaet_kwh_h.round_kfm(3),
                unit: QuantityUnit::Kw,
                unit_price_eur: price_eur,
                net_eur,
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "{:.3} kWh/h × {:.6} EUR (= {:.6} EUR/a × {anteil:.6} of a year, \
                         {tage} days) = {:.5} EUR ({}{stufe})",
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
                        "annual rate pro-rated by calendar days over each year the period \
                         touches, at that year's own length; unit price to 6 dp; net to 5 dp",
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
        ArbeitspreisModell::Modul3ZeitVariabel { ht, st, nt } => {
            for (label, kind, mp) in [
                (
                    "Netznutzung Arbeit HT (§14a Modul 3)",
                    BillingPositionKind::NneArbeitHt,
                    ht,
                ),
                (
                    "Netznutzung Arbeit ST (§14a Modul 3)",
                    BillingPositionKind::NneArbeitSt,
                    st,
                ),
                (
                    "Netznutzung Arbeit NT (§14a Modul 3)",
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
                            module: Sect14aModule::Modul3,
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

        ArbeitspreisModell::Modul1Pauschal {
            basis,
            pauschale_eur_pro_jahr,
            jahresanteil,
        } => {
            // Two positions, because Modul 1 is not a rate change: the energy is
            // billed at the published Arbeitspreis in full, and the pauschale is
            // credited alongside it. Folding it into the rate would make the
            // credit scale with consumption, which is precisely what "pauschal"
            // excludes — and is Modul 2's mechanism, not this one's.
            let arbeit_eur = ct_to_eur(basis.preis_ct_per_kwh);
            let p = kwh_pos_traced(
                "Netznutzung Arbeit (§14a Modul 1)",
                BillingPositionKind::NneArbeitModul1,
                basis.menge_kwh,
                arbeit_eur,
                vec![
                    arbeit_ref.clone(),
                    LegalReference::Sect14aEnwg {
                        module: Sect14aModule::Modul1,
                    },
                    LegalReference::BnetzaDecision {
                        reference: "BK6-22-300",
                    },
                ],
                tariff_src.clone(),
            );
            total += p.net_eur;
            positions.push(p);

            // A position states a quantity and a unit price whose product is
            // its net — that is the arithmetic the recipient re-runs, and a
            // position stating a period total as a *unit* price fails it by the
            // whole pro-rating factor. The pauschale is published per year, so
            // the rate is the negated annual amount and the quantity is the
            // share of a year billed. Six decimal places on the share, because
            // three would bill a month at 0.083 of a year — a 0.4 % error.
            // The credit is computed from the exact share, not from a rounded
            // one: six decimal places on a twelfth leave 0.048 EUR of a 120 EUR
            // pauschale uncredited over a year, on every §14a Modul 1 offtake.
            let anteil = jahresanteil.get();
            let rate_eur = -pauschale_eur_pro_jahr.round_kfm(6);
            let credit_eur = (anteil * rate_eur).round_kfm(5);
            // Nine decimal places on the stated share: enough that the
            // recipient's own re-multiplication lands inside the checker's
            // tolerance, and short enough to read on an invoice.
            let anteil = anteil.round_kfm(9);
            let c = SettlementPosition {
                text: "§14a Modul 1 pauschale Reduzierung".to_owned(),
                kind: BillingPositionKind::NneArbeitModul1,
                quantity: anteil,
                unit: QuantityUnit::Jahr,
                unit_price_eur: rate_eur,
                net_eur: credit_eur,
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "{anteil:.9} Jahresanteil × {rate_eur:.6} EUR/Jahr \
                         = {credit_eur:.5} EUR (Gutschrift)"
                    ),
                    input_quantity: anteil,
                    input_unit_price_eur: rate_eur,
                    gross_eur: anteil * rate_eur,
                    legal_refs: vec![
                        LegalReference::Sect14aEnwg {
                            module: Sect14aModule::Modul1,
                        },
                        LegalReference::BnetzaDecision {
                            reference: "BK6-22-300",
                        },
                    ],
                    tariff_source: tariff_src.clone(),
                    regulatory_reduction_factor: None,
                    rounding_note: Some(
                        "annual rate to 6 dp; net to 5 dp off the exact Jahresanteil, so twelve \
                         monthly credits sum to the annual pauschale",
                    ),
                },
            };
            total += c.net_eur;
            positions.push(c);
        }

        // §14a Modul 2 — the device's own Arbeitspreis, reduced by a percentage.
        // Unlike Modul 1's flat credit, this one scales with consumption, and it
        // attaches to the controllable device's *separately metered* energy —
        // which is why Modul 2 requires that metering and Modul 1 does not.
        ArbeitspreisModell::Modul2ProzentualeReduzierung { basis, reduktion } => {
            let base_eur = ct_to_eur(basis.preis_ct_per_kwh);
            let factor = reduktion.get();
            let reduced_eur = (base_eur * factor).round_kfm(6);
            let gross = basis.menge_kwh * reduced_eur;
            let p = SettlementPosition {
                text: format!(
                    "Netznutzung Arbeit §14a Modul 2 ({:.0}% Reduzierung)",
                    (Decimal::ONE - factor) * HUNDRED
                ),
                kind: BillingPositionKind::NneArbeitModul2,
                quantity: basis.menge_kwh.round_kfm(3),
                unit: QuantityUnit::Kwh,
                unit_price_eur: reduced_eur,
                net_eur: pos_net(basis.menge_kwh, reduced_eur),
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "{:.3} kWh × {:.6} EUR/kWh (= {:.6} × {factor} Modul 2) = {:.5} EUR",
                        basis.menge_kwh,
                        reduced_eur,
                        base_eur,
                        gross.round_kfm(5)
                    ),
                    input_quantity: basis.menge_kwh,
                    input_unit_price_eur: reduced_eur,
                    gross_eur: gross,
                    legal_refs: vec![
                        arbeit_ref.clone(),
                        LegalReference::Sect14aEnwg {
                            module: Sect14aModule::Modul2,
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
        ArbeitspreisModell::SpotpreisNetzentgelt { .. } => {}
    }

    // Leistung (RLM only) — StromNEV §17.
    //
    // Sparte-guarded like the Grundpreis and the Kapazitätsentgelt: §17 StromNEV
    // is the Leistungspreis authorisation for electricity, and gas prices
    // capacity through §15 GasNEV instead. Citing §17 on a gas invoice claims a
    // basis the ordinance does not give.
    //
    // §17 Abs. 2 Satz 2 StromNEV states the Leistungsentgelt as „das Produkt aus
    // dem jeweiligen Jahresleistungspreis und der Jahreshöchstleistung in
    // Kilowatt der jeweiligen Entnahme im Abrechnungsjahr" — two figures
    // multiplied, with no day-count convention anywhere in §17 to pro-rate by.
    // Abs. 8 names a „Jahres- und Monatsleistungspreissystem", so a price sheet
    // may instead publish a EUR/kW·Monat price; `LeistungspreisSystem` is where
    // it says which of the two it is, and each is billed on its own terms rather
    // than by scaling the other.
    if let Some(lp) = input.leistungspreis {
        if input.sparte == Sparte::Gas {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "LEISTUNGSPREIS_ON_GAS",
                message: "a Leistungspreis was supplied on a Gas settlement — §17 StromNEV \
                          does not apply to gas, which prices capacity through the \
                          Kapazitätsentgelt of §15 GasNEV"
                    .to_owned(),
            });
        }
        let (menge, price_eur, einheit, erklaerung, basis) = match lp.system {
            crate::types::LeistungspreisSystem::Jahr => {
                // A Jahresleistungsentgelt is the Abrechnungsjahr's, so billing
                // it over a shorter period bills a year of demand in that
                // period. Silently scaling it by days would invent a convention
                // no Preisblatt publishes; the settlement bills what §17 Abs. 2
                // Satz 2 says and says the period is not the Abrechnungsjahr.
                let anteil = input.period.jahresanteil();
                if (anteil - Decimal::ONE).abs() > rust_decimal::dec!(0.042) {
                    warnings.push(SettlementWarning {
                        severity: WarningSeverity::Warning,
                        code: "JAHRESLEISTUNGSPREIS_UNTERJAEHRIG",
                        message: format!(
                            "billing a Jahresleistungsentgelt over {} days (≈ {anteil:.4} of a \
                             year) — §17 Abs. 2 Satz 2 StromNEV prices the Jahreshöchstleistung \
                             im Abrechnungsjahr, so an unterjährige Abrechnung belongs on the \
                             Preisblatt's Monatsleistungspreis (LeistungspreisSystem::Monat)",
                            input.period.days()
                        ),
                    });
                }
                (
                    lp.spitzenleistung_kw,
                    lp.preis_eur_per_kw.round_kfm(6),
                    QuantityUnit::Kw,
                    "Jahreshöchstleistung × Jahresleistungspreis",
                    "unit price to 6 dp; net to 5 dp",
                )
            }
            crate::types::LeistungspreisSystem::Monat { monate } => {
                // A EUR/kW·Monat price against the period's Höchstleistung. The
                // months billed have to be the months served, on the same
                // reasoning as the Messstellenbetrieb month count.
                let period_months = Decimal::from(input.period.days()) / rust_decimal::dec!(30.44);
                if (monate - period_months).abs() > Decimal::ONE {
                    warnings.push(SettlementWarning {
                        severity: WarningSeverity::Warning,
                        code: "MONATSLEISTUNGSPREIS_MONATE_MISMATCH",
                        message: format!(
                            "billing {monate} months of Monatsleistungspreis over a period of {} \
                             days (≈ {period_months:.1} months) — check the period or the month \
                             count",
                            input.period.days()
                        ),
                    });
                }
                (
                    lp.spitzenleistung_kw * monate,
                    lp.preis_eur_per_kw.round_kfm(6),
                    QuantityUnit::Kw,
                    "Höchstleistung × Monate × Monatsleistungspreis",
                    "unit price to 6 dp; net to 5 dp",
                )
            }
        };
        let net_eur = pos_net(menge, price_eur);
        let p = SettlementPosition {
            text: "Netznutzung Leistung".to_owned(),
            kind: BillingPositionKind::NneLeistung,
            quantity: menge.round_kfm(3),
            unit: einheit,
            unit_price_eur: price_eur,
            net_eur,
            spot_price_formula: None,
            trace: CalculationTrace {
                explanation: format!(
                    "{erklaerung}: {menge:.3} kW × {price_eur:.6} EUR/kW = {net_eur:.5} EUR"
                ),
                input_quantity: menge,
                input_unit_price_eur: price_eur,
                gross_eur: menge * price_eur,
                legal_refs: vec![match input.sparte {
                    Sparte::Strom => LegalReference::StromNev { paragraph: "§17" },
                    Sparte::Gas => LegalReference::GasNev { paragraph: "§15" },
                }],
                tariff_source: tariff_src.clone(),
                regulatory_reduction_factor: None,
                rounding_note: Some(basis),
            },
        };
        total += p.net_eur;
        positions.push(p);
    }

    // ── Netzseitige Umlagen (EnFG) ────────────────────────────────────────────
    //
    // The three levies ride on the same energy base as the Arbeitspreis and are
    // billed per Entnahmestelle at the rate its Letztverbrauchergruppe carries.
    // A missing tabled rate is a warning rather than a silent zero: billing a
    // levy at nothing understates the invoice by an amount the ÜNB will reclaim.
    //
    // The §19 Aufschlag is the one levy published as an explicit A′/B′/C′
    // schedule, and B′/C′ apply „für Strommengen über 1 000 000 kWh" — so a
    // privileged Entnahmestelle carries A′ on the year's first Gigawattstunde
    // and its own rate above it, which is two positions rather than one. The
    // Offshore- and KWKG-Umlage are published only as the non-privileged rate:
    // a Begrenzung there is granted per Entnahmestelle and arrives as the
    // caller's override, so they stay one position at one rate.
    let umlage_base_kwh = input.arbeitspreis.menge_kwh();
    if input.sparte == Sparte::Strom {
        let year = input.period.from().year();
        let gruppe = input.letztverbrauchergruppe;
        let aufteilung = crate::umlagen::aufteilung(
            gruppe,
            umlage_base_kwh,
            input.enfg_jahresvorverbrauch_kwh.unwrap_or(Decimal::ZERO),
        );
        if input.enfg_jahresvorverbrauch_kwh.is_none()
            && matches!(
                gruppe,
                crate::umlagen::Letztverbrauchergruppe::B
                    | crate::umlagen::Letztverbrauchergruppe::C
            )
        {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "ENFG_VORVERBRAUCH_MISSING",
                message: format!(
                    "{gruppe:?}′ is privileged only above 1 GWh a year and no \
                     Jahresvorverbrauch was supplied — this period is billed as though \
                     it opened the year, so the first 1 GWh of it carries the full \
                     A′ rate"
                ),
            });
        }
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

            // Only the §19 Aufschlag has a published privileged rate to split
            // against, and only where the caller has not overridden it outright.
            // Either tranche may be empty — a period wholly inside the year's
            // first Gigawattstunde carries A′ on all of it, one wholly past the
            // boundary carries the group's rate on all of it — so the empty side
            // is dropped rather than billed as a zero line.
            let privilegiert = kind == BillingPositionKind::Sect19StromNevUmlage
                && input.sect19_umlage_ct_per_kwh.is_none()
                && matches!(
                    gruppe,
                    crate::umlagen::Letztverbrauchergruppe::B
                        | crate::umlagen::Letztverbrauchergruppe::C
                );

            let tranchen: Vec<(Decimal, Decimal, &str)> = if privilegiert {
                let voll = crate::umlagen::sect19_stromnev_ct_per_kwh(
                    year,
                    crate::umlagen::Letztverbrauchergruppe::A,
                )
                .unwrap_or(rate_ct);
                [
                    (aufteilung.voller_satz_kwh, voll, "erste GWh des Jahres, A′"),
                    (aufteilung.privilegiert_kwh, rate_ct, "über 1 GWh"),
                ]
                .into_iter()
                .filter(|(menge, ..)| *menge > Decimal::ZERO)
                .collect()
            } else {
                vec![(umlage_base_kwh, rate_ct, "")]
            };

            for (menge_kwh, tranche_ct, tranche_label) in tranchen {
                let price_eur = ct_to_eur(tranche_ct);
                let net_eur = pos_net(menge_kwh, price_eur);
                total += net_eur;
                let text = if tranche_label.is_empty() {
                    label.to_owned()
                } else {
                    format!("{label} — {tranche_label}")
                };
                let anteil = if tranche_label.is_empty() {
                    format!("{gruppe:?}")
                } else {
                    format!("{gruppe:?}, {tranche_label}")
                };
                positions.push(SettlementPosition {
                    text,
                    kind,
                    quantity: menge_kwh.round_kfm(3),
                    unit: QuantityUnit::Kwh,
                    unit_price_eur: price_eur.round_kfm(6),
                    net_eur,
                    spot_price_formula: None,
                    trace: CalculationTrace {
                        explanation: format!(
                            "{menge_kwh:.3} kWh × {price_eur:.6} EUR/kWh = {:.5} EUR ({anteil})",
                            (menge_kwh * price_eur).round_kfm(5),
                        ),
                        input_quantity: menge_kwh,
                        input_unit_price_eur: price_eur,
                        gross_eur: menge_kwh * price_eur,
                        legal_refs: vec![
                            legal.clone(),
                            LegalReference::EnFG {
                                paragraph: "§§21 ff.",
                            },
                        ],
                        tariff_source: None,
                        regulatory_reduction_factor: None,
                        rounding_note: None,
                    },
                });
            }
        }
    }

    // Blindmehrarbeit — reactive energy beyond the Preisblatt's free share.
    //
    // Billed on the excess only: the free share travels with the active energy,
    // and an unused allowance is not a credit. The share and the rate are terms
    // of the Netzbetreiber's price sheet, so both arrive as input rather than as
    // constants here — networks differ, and some set separate shares for
    // inductive and capacitive draw.
    if let Some(blind) = input.blindarbeit {
        let wirkarbeit_kwh = input.arbeitspreis.menge_kwh();
        let mehrarbeit = blind.mehrarbeit_kvarh(wirkarbeit_kwh);
        if mehrarbeit > Decimal::ZERO {
            let preis_eur = ct_to_eur(blind.preis_ct_per_kvarh);
            let gross = mehrarbeit * preis_eur;
            let p = SettlementPosition {
                text: "Blindmehrarbeit".to_owned(),
                kind: BillingPositionKind::Blindmehrarbeit,
                quantity: mehrarbeit.round_kfm(3),
                unit: QuantityUnit::Kvarh,
                unit_price_eur: preis_eur.round_kfm(6),
                net_eur: pos_net(mehrarbeit, preis_eur),
                spot_price_formula: None,
                trace: CalculationTrace {
                    explanation: format!(
                        "{:.3} kvarh bezogen − {:.3} kvarh frei ({:.3} kWh × {}) \
                         = {:.3} kvarh × {:.6} EUR/kvarh = {:.5} EUR",
                        blind.blindarbeit_kvarh,
                        (wirkarbeit_kwh * blind.freigrenze_anteil).round_kfm(3),
                        wirkarbeit_kwh,
                        blind.freigrenze_anteil,
                        mehrarbeit,
                        preis_eur,
                        gross.round_kfm(5)
                    ),
                    input_quantity: mehrarbeit,
                    input_unit_price_eur: preis_eur,
                    gross_eur: gross,
                    legal_refs: vec![LegalReference::StromNev { paragraph: "§17" }],
                    tariff_source: tariff_src.clone(),
                    regulatory_reduction_factor: None,
                    rounding_note: Some(
                        "quantity rounded to 3 dp; unit price to 6 dp; net to 5 dp",
                    ),
                },
            };
            total += p.net_eur;
            positions.push(p);
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
        // customer group arrive together, so this check cannot be skipped.
        // Making it conditional on a separately-optional group is precisely
        // when an over-charge goes unnoticed.
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
            quantity: ka_base_kwh.round_kfm(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: ct_to_eur(ka_ct).round_kfm(6),
            net_eur: pos_net(ka_base_kwh, ct_to_eur(ka_ct)),
            spot_price_formula: None,
            trace: CalculationTrace {
                explanation: format!(
                    "{ka_base_kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR{ka_klasse_note}",
                    ct_to_eur(ka_ct),
                    (ka_base_kwh * ct_to_eur(ka_ct)).round_kfm(5),
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
    let modul3_intervalle: &[SpotpreisInterval] = match &input.arbeitspreis {
        ArbeitspreisModell::SpotpreisNetzentgelt { intervalle } => intervalle,
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
            quantity: interval.menge_kwh.round_kfm(3),
            unit: QuantityUnit::Kwh,
            unit_price_eur: rate_eur.round_kfm(6),
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

    // §19 Abs. 2 StromNEV — an agreed individual charge replaces the published
    // Netzentgelt at a fraction the ordinance floors. The reduction covers the
    // Arbeits- and Leistungspreis positions and nothing else: the KA and the
    // levies are not the Netzbetreiber's revenue to reduce, and the lost NNE
    // revenue is recovered through the §19-Umlage billed above.
    //
    // Sequenced last on purpose. It reduces a *basis*, so every NNE position it
    // covers has to exist before it runs — the §14a Modul 3 Spotpreis positions
    // are emitted per dispatch interval further down and would otherwise be
    // outside the basis entirely, leaving a Modul-3 customer with a 10 %
    // agreement billed as though they had none.
    if let Some(v) = &input.sect19 {
        v.pruefe_prozentsatz()?;
        if v.genehmigung.is_none() {
            warnings.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "SECT19_OHNE_GENEHMIGUNG",
                message: "the §19 Abs. 2 agreement names neither a Genehmigung (Satz 5) \
                          nor an Anzeige (Satz 7) — one of the two is what makes an \
                          individuelles Netzentgelt effective, and the invoice cannot \
                          cite what it was billed under"
                    .to_owned(),
            });
        }
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
                message: "a §19 Abs. 2 Satz 2 agreement needs a Benutzungsstundenzahl \
                          reaching 7 000 h and a Stromverbrauch exceeding 10 GWh a year \
                          — the utilisation data supplied does not qualify (or is missing)"
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
                        | BillingPositionKind::NneArbeitSt
                        | BillingPositionKind::NneArbeitNt
                        | BillingPositionKind::NneArbeitModul1
                        | BillingPositionKind::NneArbeitModul2
                        | BillingPositionKind::NneArbeitModul3
                        | BillingPositionKind::NneLeistung
                )
            })
            .map(|p| p.net_eur)
            .sum();
        let reduction = -(nne_basis * (Decimal::ONE - v.vereinbarter_prozentsatz)).round_kfm(5);
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

    let total_eur = total.round_kfm(2);
    ensure_representable_eur(total_eur)?;

    // Netznutzung is a sonstige Leistung: UStAE 13b.3a excludes it from §13b by
    // name, so the Netzbetreiber always owes the tax at the Regelsteuersatz.
    let steuer = crate::umsatzsteuer::steuerausweis(
        total_eur,
        crate::umsatzsteuer::Leistungsart::SonstigeLeistung,
        crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
        input.period,
    )?;
    ensure_representable_eur(steuer.brutto_eur().abs())?;

    let result = SettlementResult {
        malo_id: input.malo_id.clone(),
        sparte: input.sparte,
        regime,
        settlement_type,
        status: SettlementStatus::Initial,
        korrektur_grund: None,
        period: input.period,
        sender_mp_id: input.nb_mp_id.clone(),
        recipient_mp_id: input.lf_mp_id.clone(),
        positions,
        total_eur,
        steuer,
        warnings,
    };
    debug_assert_eq!(
        result.total_eur,
        result.recomputed_total(),
        "NNE: total_eur mismatch — calculation bug"
    );
    Ok(result)
}

// ── MMM invoice (PID 31005) ───────────────────────────────────────────────────

/// Calculate a Mehr-/Mindermengen settlement invoice (PID 31005, Strom and Gas).
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
    //
    // Exempt from `ensure_berechenbar` (the AgNeS guard): Mehr-/Mindermengen
    // prices are formed on the *Netzzugang* axis — GPKE (BK6-24-174) Teil 1
    // Kap. 8.4 for Strom, GaBi Gas 2.1 (BK7-24-01-008) for Gas, both market-
    // price based — not by the StromNEV/ARegV Entgeltbildung that AgNeS
    // (GBK-25-01) replaces. A 2029 MMM settlement therefore stays computable.
    let mut warnings: Vec<SettlementWarning> = Vec::new();
    let regime =
        crate::regulatory::RegulatoryRegime::for_period(input.period.from(), input.period.to());
    warn_if_straddles_turnover(input.period.from(), input.period.to(), &mut warnings);
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
    // Gas and Strom MMM use separate settlement types for correct audit
    // references. A self-issued Mehrmenge leg is a third: the AHB marks it
    // *Selbstausgestellt*, and that is carried by the settlement type because
    // the BO4E rendering reads `netznutzungrechnungsart` from it.
    let mmm_settlement_type = if input.selbstausgestellt {
        SettlementType::MmmSelbstausstellt
    } else {
        match input.sparte {
            Sparte::Gas => SettlementType::MmmGas,
            Sparte::Strom => SettlementType::MmmStrom,
        }
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
    // A Mehrmenge is a credit, and the credit is carried by the **rate**. The
    // quantity is a metered energy and stays positive; negating the net instead
    // would leave a position whose stated quantity times its stated unit price
    // is the opposite of its stated net, which every recipient's own line
    // arithmetic reads as a 200 % error.
    let mehr_price_eur = -mehr_eur.round_kfm(6);
    let mehr_net = pos_net(mehr_kwh, mehr_price_eur);
    let p1 = SettlementPosition {
        text: "Mehrmengen (Gutschrift)".to_owned(),
        kind: BillingPositionKind::Mehrmenge,
        quantity: mehr_kwh.round_kfm(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: mehr_price_eur,
        net_eur: mehr_net,
        spot_price_formula: None,

        trace: CalculationTrace {
            explanation: format!(
                "{mehr_kwh:.3} kWh × {mehr_price_eur:.6} EUR/kWh = {mehr_net:.5} EUR (Gutschrift)"
            ),
            input_quantity: mehr_kwh,
            input_unit_price_eur: mehr_price_eur,
            gross_eur: mehr_kwh * mehr_price_eur,
            legal_refs: mmm_refs.clone(),
            tariff_source: None,
            regulatory_reduction_factor: None,
            rounding_note: Some("Mehrmengen are credit positions — the Mehrmengenpreis is negated"),
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
        quantity: minder_kwh.round_kfm(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: minder_eur.round_kfm(6),
        net_eur: minder_net,
        spot_price_formula: None,

        trace: CalculationTrace {
            explanation: format!(
                "{minder_kwh:.3} kWh × {:.6} EUR/kWh = {:.5} EUR",
                minder_eur,
                minder_gross.round_kfm(5)
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

    let total_eur = (p1.net_eur + p2.net_eur).round_kfm(2);
    ensure_representable_eur(total_eur.abs())?;

    // A Mehr-/Mindermenge is a **Lieferung** of the commodity, not a network
    // service, so §13b Abs. 2 Nr. 5 Buchst. b can shift the tax to the recipient
    // — and the gas rate reduction of §28 Abs. 5 UStG can reach it.
    let leistungsart = match input.sparte {
        Sparte::Strom => crate::umsatzsteuer::Leistungsart::LieferungStrom,
        Sparte::Gas => crate::umsatzsteuer::Leistungsart::LieferungGas,
    };
    let steuer = crate::umsatzsteuer::steuerausweis(
        total_eur,
        leistungsart,
        input.wiederverkaeufer,
        input.period,
    )?;
    ensure_representable_eur(steuer.brutto_eur().abs())?;

    Ok(SettlementResult {
        malo_id: input.malo_id.clone(),
        sparte: input.sparte,
        regime,
        settlement_type: mmm_settlement_type,
        status: SettlementStatus::Initial,
        korrektur_grund: None,
        period: input.period,
        sender_mp_id: input.nb_mp_id.clone(),
        recipient_mp_id: input.lf_mp_id.clone(),
        positions: vec![p1, p2],
        total_eur,
        steuer,
        warnings,
    })
}

// ── Abschlagsrechnung (PID 31001) ─────────────────────────────────────────────

/// Calculate an Abschlagsrechnung Netznutzung (PID 31001): a payment on account.
///
/// This settles nothing. It asks the Lieferant for an amount against a period
/// the Netzbetreiber has not yet billed, and the Abschlussrechnung that follows
/// deducts it by invoice number ([`crate::Abschlagsverrechnung`]).
///
/// **Exactly one position**, per INVOIC AHB 1.0b Änd-ID 26817 — "Eine
/// Abschlagsrechnung kann und muss genau eine Positionszeile enthalten", with
/// `LIN DE1082` fixed at 1. So there is no quantity and no unit price here: an
/// Abschlag prices no energy, and giving it a kWh figure would assert a
/// measurement nobody took.
///
/// # Errors
///
/// [`BillingError::InvalidInput`] when the amount is not positive — an Abschlag
/// asking for nothing, or crediting money, is not an Abschlag. Reverse it with
/// [`reverse`] instead.
#[must_use = "handle the BillingError"]
pub fn settle_abschlag(input: &AbschlagInput) -> Result<SettlementResult, BillingError> {
    if input.betrag_netto_eur <= Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: format!(
                "an Abschlag must ask for a positive amount, got {} EUR — to give money back, \
                 reverse the Abschlagsrechnung rather than issuing a negative one",
                input.betrag_netto_eur
            ),
        });
    }

    // The Abschlag rests on the same charge authorisation as the invoice it
    // anticipates, so a period AgNeS governs is refused here too.
    let regime =
        crate::regulatory::RegulatoryRegime::for_period(input.period.from(), input.period.to());
    regime.ensure_berechenbar()?;

    let mut warnings: Vec<SettlementWarning> = Vec::new();
    warn_if_straddles_turnover(input.period.from(), input.period.to(), &mut warnings);

    let betrag = input.betrag_netto_eur.round_kfm(2);
    ensure_representable_eur(betrag)?;

    let position = SettlementPosition {
        text: format!("Abschlag Netznutzung ({})", input.grundlage.label()),
        kind: BillingPositionKind::NneAbschlag,
        // No quantity and no unit price: the amount *is* the position.
        quantity: Decimal::ONE,
        unit: QuantityUnit::Monat,
        unit_price_eur: betrag,
        net_eur: betrag,
        spot_price_formula: None,
        trace: CalculationTrace {
            explanation: format!(
                "Abschlag {betrag:.2} EUR für {} – {} ({})",
                input.period.from(),
                input.period.to(),
                input.grundlage.label(),
            ),
            input_quantity: Decimal::ONE,
            input_unit_price_eur: betrag,
            gross_eur: betrag,
            legal_refs: vec![
                match input.sparte {
                    Sparte::Strom => LegalReference::StromNev { paragraph: "§21" },
                    Sparte::Gas => LegalReference::GasNev { paragraph: "§14" },
                },
                LegalReference::Ustg {
                    paragraph: "§14 Abs. 5",
                },
            ],
            tariff_source: None,
            regulatory_reduction_factor: None,
            rounding_note: Some("amount rounded to 2 dp"),
        },
    };

    Ok(SettlementResult {
        malo_id: input.malo_id.clone(),
        sparte: input.sparte,
        regime,
        settlement_type: SettlementType::NneAbschlag,
        status: SettlementStatus::Initial,
        korrektur_grund: None,
        period: input.period,
        sender_mp_id: input.nb_mp_id.clone(),
        recipient_mp_id: input.lf_mp_id.clone(),
        positions: vec![position],
        total_eur: betrag,
        // An Anzahlung is taxed when it is received (§14 Abs. 5 UStG), so the
        // Abschlagsrechnung states the tax like any other — and the
        // Abschlussrechnung must then not tax the same money twice, which is why
        // the deduction happens on `zu_zahlen` rather than on the net.
        steuer: crate::umsatzsteuer::steuerausweis(
            betrag,
            crate::umsatzsteuer::Leistungsart::SonstigeLeistung,
            crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
            input.period,
        )?,
        warnings,
    })
}

// ── MSB invoice (PID 31009) ───────────────────────────────────────────────────

/// Calculate a MSB-Rechnung (PID 31009): **MSB → NB / LF / ESA** metering
/// service settlement.
///
/// The MSB is the invoicer in all seven Anwendungsfälle of the PID overview 4.0;
/// the recipient's market role varies and is carried on
/// [`MsbInput::empfaenger`]. 31009 is Strom-only.
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
    if input.billing_months == 0 {
        return Err(BillingError::InvalidInput {
            reason: "billing_months must be at least 1".to_owned(),
        });
    }

    // Exempt from `ensure_berechenbar` (the AgNeS guard): Messstellenbetrieb
    // charges are formed under the MsbG (§§6–7, §30 Preisobergrenzen), which
    // does not lapse with StromNEV/ARegV at the end of 2028 — AgNeS (GBK-25-01)
    // replaces the Netzentgeltbildung, not the metering-charge law.
    let mut warnings: Vec<SettlementWarning> = Vec::new();
    warn_if_straddles_turnover(input.period.from(), input.period.to(), &mut warnings);

    // §30 MsbG Preisobergrenze. Two conversions before the comparison means
    // anything.
    //
    // The ceiling is annual and the charge monthly, so the charge is annualised
    // — billing a year in monthly instalments does not raise the cap.
    //
    // And every §30 ceiling is stated **brutto** („nicht mehr als 80 Euro brutto
    // jährlich", „jeweils nicht mehr als 50 Euro brutto jährlich", „brutto
    // jährlich nicht mehr als 60 Euro"), while `grundgebuehr_eur_per_month` is
    // the net the position is built from and taxed after. Comparing the net
    // against a gross ceiling tolerates the whole Umsatzsteuer rate: at 19 % a
    // charge may sit 18.9 % above the statutory maximum and never be reported.
    //
    // What §30 caps is the „Entgelt für den Messstellenbetrieb", and §3 Abs. 2
    // Nr. 1 MsbG puts the Messung inside it: Messstellenbetrieb umfasst die
    // „Gewährleistung einer mess- und eichrechtskonformen Messung entnommener,
    // verbrauchter und eingespeister Energie einschließlich der
    // Messwertaufbereitung […] sowie Standard- und Zusatzleistungen nach § 34".
    // §17 Abs. 7 Satz 1 StromNEV says the same from the other side — „ein Entgelt
    // für den Messstellenbetrieb, zu dem auch die Messung gehört". So the ceiling
    // measures the Grundgebühr and the Messdienstleistung together; testing the
    // Grundgebühr alone lets a settlement split a charge across two positions and
    // clear a ceiling neither of them alone would breach.
    //
    // The rate is the one this settlement will actually state, resolved from the
    // delivery period. A period straddling a rate change resolves to none, and
    // the check is skipped rather than run at a guessed rate — `steuerausweis`
    // refuses that period below anyway.
    if let (Some(kategorie), Some(schuldner)) =
        (input.messstellen_kategorie, input.entgeltschuldner)
    {
        let satz = crate::umsatzsteuer::regelsatz_prozent(
            crate::umsatzsteuer::Leistungsart::SonstigeLeistung,
            input.period.from(),
            input.period.to(),
        );
        if let (Some(satz), Some(pog)) = (
            satz,
            crate::msbg::preisobergrenze_eur_per_jahr(kategorie, schuldner),
        ) {
            // The Messdienstleistung is a flat fee for the whole period, so it
            // is spread over the months the period bills before annualising —
            // for the monthly settlement that is the common case, that is the
            // fee itself twelve times over.
            let msl_monatlich = input.messdienstleistung_eur.unwrap_or(Decimal::ZERO)
                / Decimal::from(input.billing_months);
            let netto_annual =
                (input.grundgebuehr_eur_per_month + msl_monatlich) * Decimal::from(12);
            let brutto_annual = (netto_annual * (Decimal::ONE + satz / HUNDRED)).round_kfm(2);
            if brutto_annual > pog {
                warnings.push(SettlementWarning {
                    severity: WarningSeverity::Warning,
                    code: "MSB_ABOVE_MSBG_POG",
                    message: format!(
                        "Messstellenbetrieb einschließlich Messdienstleistung {netto_annual} \
                         EUR/a netto = {brutto_annual} EUR/a brutto at {satz} % exceeds the §30 \
                         MsbG Preisobergrenze of {pog} EUR/a brutto for {kategorie:?} / \
                         {schuldner:?}"
                    ),
                });
            }
        }
    }

    // The months billed have to be the months served. `billing_months` and the
    // delivery period were independent, so a request could bill twelve months of
    // Grundgebühr over a one-month period — a twelvefold over-charge that adds
    // up perfectly and reads as a normal annual invoice.
    //
    // The comparison is deliberately loose: a period may run from the 15th to
    // the 14th, or cover a Gerätewechsel mid-month, so anything within a month
    // of the calendar length passes. What it catches is an order-of-magnitude
    // mismatch, which is the error worth catching.
    let period_months = Decimal::from(input.period.days()) / rust_decimal::dec!(30.44);
    let billed = Decimal::from(input.billing_months);
    if (billed - period_months).abs() > Decimal::ONE {
        warnings.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "BILLING_MONTHS_MISMATCH",
            message: format!(
                "billing {billed} months of Messstellenbetrieb over a period of {} days \
                 (≈ {period_months:.1} months) — check the period or the month count",
                input.period.days()
            ),
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
        let msl: Decimal = msl_eur.round_kfm(5);
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

    let total_eur = total.round_kfm(2);
    ensure_representable_eur(total_eur)?;

    Ok(SettlementResult {
        malo_id: input.malo_id.clone(),
        // §30 MsbG prices metering rather than energy, so the Sparte drives no
        // arithmetic here — it is carried because it is what the invoice states.
        sparte: input.sparte,
        regime: crate::regulatory::RegulatoryRegime::for_period(
            input.period.from(),
            input.period.to(),
        ),
        settlement_type: SettlementType::MsbRechnung,
        status: SettlementStatus::Initial,
        korrektur_grund: None,
        period: input.period,
        // PID 31009 is issued *by* the MSB, so the MSB is the sender. NB as
        // sender and MSB as recipient inverts the invoice: the party owed money
        // is named as the one billing for it.
        sender_mp_id: input.msb_mp_id.clone(),
        recipient_mp_id: input.empfaenger.mp_id.clone(),
        positions,
        total_eur,
        // Messstellenbetrieb is a sonstige Leistung — never reverse-charged.
        steuer: crate::umsatzsteuer::steuerausweis(
            total_eur,
            crate::umsatzsteuer::Leistungsart::SonstigeLeistung,
            crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
            input.period,
        )?,
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
/// let reversal = reverse(&original, grid_billing::KorrekturGrund::Messwertkorrektur);
/// assert_eq!(reversal.total_eur, -original.total_eur);
/// ```
#[must_use]
pub fn reverse(original: &SettlementResult, grund: KorrekturGrund) -> SettlementResult {
    use crate::types::SettlementStatus;
    let reversed_positions: Vec<_> = original
        .positions
        .iter()
        // The **rate** carries the sign, not the quantity: the energy delivered
        // is a fact the reversal does not undo, and a position stating a
        // positive quantity against a positive unit price with a negative net
        // is one whose own arithmetic contradicts it — which every recipient
        // re-runs on receipt.
        .map(|p| SettlementPosition {
            text: format!("Storno: {}", p.text),
            kind: p.kind,
            quantity: p.quantity,
            unit: p.unit,
            unit_price_eur: -p.unit_price_eur,
            net_eur: -p.net_eur,
            spot_price_formula: p.spot_price_formula.clone(),
            trace: CalculationTrace {
                explanation: format!("Storno: {} (negated)", p.trace.explanation),
                input_quantity: p.trace.input_quantity,
                input_unit_price_eur: -p.trace.input_unit_price_eur,
                gross_eur: -p.trace.gross_eur,
                legal_refs: p.trace.legal_refs.clone(),
                tariff_source: p.trace.tariff_source.clone(),
                regulatory_reduction_factor: p.trace.regulatory_reduction_factor,
                rounding_note: Some("reversal — all amounts negated"),
            },
        })
        .collect();

    // Exempt from `ensure_berechenbar` (the AgNeS guard): a reversal prices
    // nothing — it mirrors amounts an already-guarded builder computed, under
    // the regime recorded on the original. Refusing here would make a
    // Verordnung-era settlement irreversible once the 2029 turnover has
    // passed, which is the opposite of what the guard protects.
    //
    // The turnover warning is re-emitted, though: a straddling period is as
    // wrong reversed as it was billed, and the mirror should say so too.
    let mut warnings: Vec<SettlementWarning> = Vec::new();
    warn_if_straddles_turnover(original.period.from(), original.period.to(), &mut warnings);

    SettlementResult {
        // A reversal is the same supply under the same rules — only the signs
        // differ, so identity and regime carry over unchanged.
        malo_id: original.malo_id.clone(),
        sparte: original.sparte,
        regime: original.regime,
        settlement_type: original.settlement_type,
        status: SettlementStatus::Reversal,
        korrektur_grund: Some(grund),
        period: original.period,
        sender_mp_id: original.sender_mp_id.clone(),
        recipient_mp_id: original.recipient_mp_id.clone(),
        positions: reversed_positions,
        total_eur: -original.total_eur,
        // The tax is mirrored, not recomputed. A reversal cancels the invoice
        // that was issued, so it has to carry that invoice's treatment even if
        // the rate or the counterparty's §3g status has since changed —
        // recomputing would leave a tax residue the reversal never cancels.
        steuer: crate::umsatzsteuer::Steuerausweis {
            kategorie: original.steuer.kategorie,
            satz_prozent: original.steuer.satz_prozent,
            bemessungsgrundlage_eur: -original.steuer.bemessungsgrundlage_eur,
            steuer_eur: -original.steuer.steuer_eur,
            hinweis: original.steuer.hinweis,
            rechtsgrundlage: original.steuer.rechtsgrundlage,
        },
        warnings,
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
/// let (reversal, replacement) = correct(&original, corrected, grid_billing::KorrekturGrund::Tarifkorrektur);
/// assert_eq!(reversal.total_eur, -original.total_eur);
/// assert_eq!(replacement.status, grid_billing::SettlementStatus::Correction);
/// ```
#[must_use]
pub fn correct(
    original: &SettlementResult,
    mut replacement: SettlementResult,
    grund: KorrekturGrund,
) -> (SettlementResult, SettlementResult) {
    // No AgNeS guard here: the reversal prices nothing (see [`reverse`]) and
    // the replacement was computed by a settlement builder that already ran
    // `ensure_berechenbar` for its own period.
    let reversal = reverse(original, grund);
    replacement.status = SettlementStatus::Correction;
    replacement.korrektur_grund = Some(grund);
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
///
/// [`BillingError::UnsupportedEntgeltRegime`] for a period governed by AgNeS
/// (from 01.01.2029) — the GasNEV §14 charge authorisation the AWH positions
/// rest on lapses with 2028.
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

    // AWH charges rest on the GasNEV §14 charge authorisation, i.e. on the
    // Entgelt axis that lapses with 2028 — so a period under AgNeS is refused
    // like an NNE settlement, not billed under an authorisation that no
    // longer exists.
    let regime =
        crate::regulatory::RegulatoryRegime::for_period(input.period.from(), input.period.to());
    regime.ensure_berechenbar()?;

    let mut warnings: Vec<SettlementWarning> = Vec::new();
    warn_if_straddles_turnover(input.period.from(), input.period.to(), &mut warnings);

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
            unit_price_eur: awh.preis_eur.round_kfm(6),
            net_eur: net,
            spot_price_formula: None,

            trace: CalculationTrace {
                explanation: format!(
                    "{} × {:.5} EUR = {:.5} EUR",
                    awh.anzahl,
                    awh.preis_eur,
                    gross.round_kfm(5)
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

    let total_eur = total.round_kfm(2);
    ensure_representable_eur(total_eur)?;

    let result = SettlementResult {
        malo_id: input.malo_id.clone(),
        // AWH Sperrprozesse are a Gas process (GeLi Gas 3.0 §5.4).
        sparte: Sparte::Gas,
        regime,
        settlement_type: SettlementType::GasAwhSperrung,
        status: SettlementStatus::Initial,
        korrektur_grund: None,
        period: input.period,
        sender_mp_id: input.nb_mp_id.clone(),
        recipient_mp_id: input.lf_mp_id.clone(),
        positions,
        total_eur,
        // Abrechnungswürdige Handlungen are services performed during the
        // Sperrprozess — a sonstige Leistung, never reverse-charged.
        steuer: crate::umsatzsteuer::steuerausweis(
            total_eur,
            crate::umsatzsteuer::Leistungsart::SonstigeLeistung,
            crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
            input.period,
        )?,
        warnings,
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
///
/// `pauschale_eur_pro_jahr` is the NB's published annual amount; the period here
/// is one month, so a twelfth of it is credited.
fn modul1(pauschale_eur_pro_jahr: Decimal) -> ArbeitspreisModell {
    use crate::types::MengePreis;
    ArbeitspreisModell::Modul1Pauschal {
        basis: MengePreis {
            menge_kwh: rust_decimal::dec!(1500),
            preis_ct_per_kwh: rust_decimal::dec!(3.5),
        },
        pauschale_eur_pro_jahr,
        jahresanteil: crate::types::Jahresanteil::MONAT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AwhPositionInput, GasAwhInput, InvoiceDocument, MsbEmpfaengerRolle, MsbRechnungsempfaenger,
        SettlementPeriod, validate_msb_input,
    };
    use crate::types::{
        GemeindeGroesse, Grundpreis, Konzessionsabgabe, Leistungspreis, LeistungspreisSystem,
        MengePreis,
    };
    use rust_decimal::Decimal;
    use rust_decimal::dec;
    use time::macros::date;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("valid decimal literal")
    }

    fn base_nne() -> NneInput {
        NneInput {
            blindarbeit: None,
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("3.5"),
            }),
            leistungspreis: None,
            letztverbrauchergruppe: Default::default(),
            enfg_jahresvorverbrauch_kwh: None,
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

    /// Billing more months than the period serves is caught.
    ///
    /// `billing_months` and the delivery period were independent, so a request
    /// could bill twelve months of Grundgebühr over one month of service — a
    /// twelvefold over-charge that adds up perfectly and reads as an annual
    /// invoice.
    #[test]
    fn billing_more_months_than_the_period_serves_is_flagged() {
        let mut input = base_msb();
        input.billing_months = 12; // period is one month
        let r = settle_msb(&input).expect("settles");
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "BILLING_MONTHS_MISMATCH"),
            "{:#?}",
            r.warnings
        );

        // The matching case is silent.
        let matching = settle_msb(&base_msb()).expect("settles");
        assert!(
            !matching
                .warnings
                .iter()
                .any(|w| w.code == "BILLING_MONTHS_MISMATCH"),
            "{:#?}",
            matching.warnings
        );

        // A full year over a full year is silent too.
        let mut annual = base_msb();
        annual.period =
            SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 12 - 31)).unwrap();
        annual.billing_months = 12;
        let annual = settle_msb(&annual).expect("settles");
        assert!(
            !annual
                .warnings
                .iter()
                .any(|w| w.code == "BILLING_MONTHS_MISMATCH"),
            "{:#?}",
            annual.warnings
        );
    }

    /// A Leistungspreis on a Gas settlement cites GasNEV, and says it is odd.
    ///
    /// §17 StromNEV is the electricity Leistungspreis authorisation; gas prices
    /// capacity through §15 GasNEV. Citing §17 on a gas invoice claims a basis
    /// the ordinance does not give.
    #[test]
    fn a_leistungspreis_on_gas_does_not_cite_stromnev() {
        let mut input = base_nne();
        input.sparte = Sparte::Gas;
        input.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("40"),
            preis_eur_per_kw: d("12.50"),
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
        });
        let r = settle_nne(&input).expect("settles");
        assert!(
            r.warnings.iter().any(|w| w.code == "LEISTUNGSPREIS_ON_GAS"),
            "{:#?}",
            r.warnings
        );
        let refs: Vec<String> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::NneLeistung)
            .flat_map(|p| p.trace.legal_refs.iter().map(LegalReference::citation))
            .collect();
        assert!(
            refs.iter().all(|r| !r.contains("StromNEV")),
            "a gas Leistungspreis must not cite StromNEV: {refs:?}"
        );
    }

    fn base_msb() -> MsbInput {
        MsbInput {
            sparte: Sparte::Strom,
            malo_id: "51238696012".to_owned(),
            empfaenger: MsbRechnungsempfaenger {
                rolle: MsbEmpfaengerRolle::Netzbetreiber,
                mp_id: "9900357000004".to_owned(),
            },
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
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            sparte: Sparte::Strom,
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
            wiederverkaeufer: crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
            selbstausgestellt: false,
        }
    }

    // ── Every position multiplies out ────────────────────────────────────────

    /// `invoic-checker`'s own line-arithmetic rule at its default tolerance
    /// (`arithmetic_tolerance_ppm = 10_000`, i.e. 1 %), reproduced over a
    /// [`SettlementPosition`] rather than over the rendered document.
    ///
    /// The rule is the recipient's: stated quantity × stated unit price against
    /// stated net. It is checked here, on the engine's own output, because that
    /// is where a position can be built inconsistently — the renderer copies all
    /// three fields verbatim.
    fn multiplies_out(p: &SettlementPosition) -> bool {
        const PPM: Decimal = dec!(10_000);
        const MILLION: Decimal = dec!(1_000_000);
        let computed = p.quantity * p.unit_price_eur;
        if computed.is_zero() {
            return p.net_eur.is_zero();
        }
        (p.net_eur - computed).abs() * MILLION <= computed.abs() * PPM
    }

    /// Every settlement this crate can emit, named, for the invariant below.
    ///
    /// Built as a list rather than one big test so a failure names the
    /// settlement that produced the position.
    fn every_settlement() -> Vec<(String, SettlementResult)> {
        use crate::gas::{Druckstufe, GasKapazitaet, Kapazitaetsprodukt};
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtEinstufung};
        use crate::sect19::{Sect19Art, Sect19Vereinbarung};
        use crate::types::{AbschlagGrundlage, Blindarbeit};

        let mut out: Vec<(String, SettlementResult)> = Vec::new();

        out.push((
            "NNE flat".to_owned(),
            settle_nne(&base_nne()).expect("settles"),
        ));

        let mut ka = base_nne();
        ka.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        out.push((
            "NNE + Konzessionsabgabe".to_owned(),
            settle_nne(&ka).expect("settles"),
        ));

        let mut rlm = base_nne();
        rlm.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("12.5"),
            preis_eur_per_kw: d("4.20"),
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
        });
        rlm.blindarbeit = Some(Blindarbeit {
            blindarbeit_kvarh: d("900"),
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            preis_ct_per_kvarh: d("1.5"),
        });
        out.push((
            "NNE RLM + Blindmehrarbeit".to_owned(),
            settle_nne(&rlm).expect("settles"),
        ));

        let mut modul1 = base_nne();
        modul1.arbeitspreis = super::modul1(d("120.00"));
        out.push((
            "§14a Modul 1".to_owned(),
            settle_nne(&modul1).expect("settles"),
        ));

        let mut modul2 = base_nne();
        modul2.arbeitspreis = ArbeitspreisModell::Modul2ProzentualeReduzierung {
            basis: MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("3.5"),
            },
            reduktion: crate::types::Reduktionsfaktor::REGELFALL,
        };
        out.push((
            "§14a Modul 2".to_owned(),
            settle_nne(&modul2).expect("settles"),
        ));

        let mut modul3 = base_nne();
        modul3.arbeitspreis = ArbeitspreisModell::Modul3ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("600"),
                preis_ct_per_kwh: d("4.0"),
            },
            st: MengePreis {
                menge_kwh: d("300"),
                preis_ct_per_kwh: d("3.0"),
            },
            nt: MengePreis {
                menge_kwh: d("600"),
                preis_ct_per_kwh: d("1.5"),
            },
        };
        out.push((
            "§14a Modul 3 zeitvariabel".to_owned(),
            settle_nne(&modul3).expect("settles"),
        ));

        let base = time::OffsetDateTime::parse(
            "2025-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("valid RFC 3339");
        let mut spot = base_nne();
        spot.arbeitspreis = ArbeitspreisModell::SpotpreisNetzentgelt {
            intervalle: vec![
                SpotpreisInterval {
                    period_from: base,
                    period_to: base + time::Duration::minutes(15),
                    menge_kwh: d("12.345"),
                    nne_rate_ct_per_kwh: d("1.8"),
                    epex_spot_ct_per_kwh: None,
                },
                SpotpreisInterval {
                    period_from: base + time::Duration::minutes(15),
                    period_to: base + time::Duration::minutes(30),
                    menge_kwh: d("7.5"),
                    nne_rate_ct_per_kwh: d("2.05"),
                    epex_spot_ct_per_kwh: None,
                },
            ],
        };
        out.push((
            "Spotpreis-Netzentgelt".to_owned(),
            settle_nne(&spot).expect("settles"),
        ));

        // Every EnFG levy at once, on a privileged group so the §19 Aufschlag
        // splits into its A′ and B′ tranches.
        // 2026 is the year the levy schedule covers, and a B′ Entnahmestelle
        // 1 000 kWh short of the 1 GWh threshold splits the §19 Aufschlag into
        // its A′ and B′ tranches — two positions rather than one.
        let mut umlagen = base_nne();
        umlagen.period =
            SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).expect("ordered");
        umlagen.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::B;
        umlagen.enfg_jahresvorverbrauch_kwh = Some(d("999000"));
        umlagen.arbeitspreis = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: d("2000"),
            preis_ct_per_kwh: d("3.5"),
        });
        out.push((
            "NNE + EnFG levies".to_owned(),
            settle_nne(&umlagen).expect("settles"),
        ));

        let mut sect19 = base_nne();
        sect19.jahresarbeit_kwh = Some(d("12000000"));
        sect19.jahreshoechstleistung_kw = Some(d("1500"));
        sect19.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("1500"),
            preis_eur_per_kw: d("10.00"),
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
        });
        sect19.sect19 = Some(Sect19Vereinbarung {
            art: Sect19Art::IntensiveNetznutzung,
            vereinbarter_prozentsatz: d("0.10"),
            genehmigung: Some("BK4-22-089".to_owned()),
        });
        out.push((
            "§19 Abs. 2 individuelles Entgelt".to_owned(),
            settle_nne(&sect19).expect("settles"),
        ));

        let mut gas = base_nne();
        gas.sparte = Sparte::Gas;
        gas.grundpreis = Some(Grundpreis {
            eur_per_month: d("15.00"),
            months: Decimal::ONE,
        });
        gas.gas_kapazitaet = Some(GasKapazitaet {
            bestellte_kapazitaet_kwh_h: d("500"),
            entgelt_eur_per_kwh_h_a: d("14.60"),
            produkt: Kapazitaetsprodukt::Fest,
            druckstufe: Some(Druckstufe::Mitteldruck),
        });
        out.push((
            "NNE Gas + Kapazitätsentgelt".to_owned(),
            settle_nne(&gas).expect("settles"),
        ));

        let mut mehr = base_mmm();
        mehr.actual_kwh = d("1400");
        out.push((
            "MMM Mehrmenge (Gutschrift)".to_owned(),
            settle_mmm(&mehr).expect("settles"),
        ));

        let mut minder = base_mmm();
        minder.actual_kwh = d("1600");
        out.push((
            "MMM Mindermenge".to_owned(),
            settle_mmm(&minder).expect("settles"),
        ));

        let mut msb = base_msb();
        msb.messdienstleistung_eur = Some(d("2.50"));
        msb.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtEinstufung {
            jahresverbrauch_kwh: Some(d("9000")),
            ..PflichtEinstufung::default()
        }));
        msb.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);
        out.push(("MSB".to_owned(), settle_msb(&msb).expect("settles")));

        out.push((
            "Abschlag".to_owned(),
            settle_abschlag(&AbschlagInput {
                malo_id: "51238696012".to_owned(),
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".to_owned(),
                period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31))
                    .expect("ordered"),
                sparte: Sparte::Strom,
                betrag_netto_eur: d("250.00"),
                grundlage: AbschlagGrundlage::Prognose,
            })
            .expect("settles"),
        ));

        out.push((
            "Gas AWH".to_owned(),
            settle_gas_awh(&GasAwhInput {
                malo_id: "51238696012".to_owned(),
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".to_owned(),
                period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31))
                    .expect("ordered"),
                tariff_sheet_id: None,
                awh_positionen: vec![
                    AwhPositionInput {
                        beschreibung: "Sperrung Gaszähler".to_owned(),
                        anzahl: 1,
                        preis_eur: d("45.00"),
                        artikel_id: None,
                    },
                    AwhPositionInput {
                        beschreibung: "Erfolglose Unterbrechung".to_owned(),
                        anzahl: 2,
                        preis_eur: d("20.00"),
                        artikel_id: None,
                    },
                ],
            })
            .expect("settles"),
        ));

        out.push((
            "§18 dezentrale Einspeisung".to_owned(),
            crate::sect18::settle_dezentrale_einspeisung(
                &crate::sect18::DezentraleEinspeisungInput {
                    malo_id: "51238696012".to_owned(),
                    nb_mp_id: "9900357000004".to_owned(),
                    anlagenbetreiber_mp_id: "9900012345678".to_owned(),
                    period: SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
                        .expect("ordered"),
                    einspeisung_kwh: d("10000"),
                    vermiedene_kosten_ct_per_kwh: d("0.6"),
                    ist_eeg_gefoerdert: false,
                    tariff_sheet_id: None,
                },
            )
            .expect("settles"),
        ));

        // Every settlement above, reversed: a Stornorechnung is dispatched
        // through the same check as the invoice it cancels.
        let reversals: Vec<(String, SettlementResult)> = out
            .iter()
            .map(|(name, r)| {
                (
                    format!("Storno of {name}"),
                    reverse(r, crate::types::KorrekturGrund::Messwertkorrektur),
                )
            })
            .collect();
        out.extend(reversals);
        out
    }

    /// **Invariant: every emitted position states figures that multiply out.**
    ///
    /// A [`SettlementPosition`]'s quantity, unit price and net are three fields
    /// the renderer copies verbatim onto the invoice, and the recipient
    /// re-multiplies the first two against the third — `invoic-checker` stage 5
    /// does exactly that, and calls a mismatch a dispute. A position that states
    /// a period total where a unit price belongs, or negates its net without
    /// negating its rate, is therefore an invoice that cannot be dispatched at
    /// all: netzbilanzd refuses a Dispute outcome with 422.
    ///
    /// Asserting a settlement's `total_eur` does not reach this — the total adds
    /// the nets, and the nets can be right while the fields stating them are
    /// not.
    #[test]
    fn every_emitted_position_multiplies_out() {
        for (name, result) in every_settlement() {
            for (i, p) in result.positions.iter().enumerate() {
                assert!(
                    multiplies_out(p),
                    "{name} position {i} ({}): {} {:?} × {} EUR = {} EUR, but the position \
                     states {} EUR",
                    p.text,
                    p.quantity,
                    p.unit,
                    p.unit_price_eur,
                    p.quantity * p.unit_price_eur,
                    p.net_eur,
                );
            }
        }
    }

    /// The invariant has teeth: the shape it forbids fails it.
    ///
    /// A credit whose net is negated while its rate stays positive is the exact
    /// defect — the position reads as a 200 % arithmetic error to the recipient.
    #[test]
    fn a_position_whose_net_contradicts_its_rate_fails_the_invariant() {
        let mut result = settle_nne(&base_nne()).expect("settles");
        let p = &mut result.positions[0];
        p.net_eur = -p.net_eur;
        assert!(!multiplies_out(p));
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
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
        });
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let r = settle_nne(&i).unwrap();
        // 12.5 kW × 4.20 EUR/kW·Monat = 52.50 EUR for the month, beside 52.50
        // Arbeit and 1.65 Konzessionsabgabe.
        assert_eq!(r.positions.len(), 3);
        assert_eq!(r.positions[1].unit, QuantityUnit::Kw);
        assert_eq!(r.positions[1].unit_price_eur, d("4.20"));
        assert_eq!(r.positions[1].net_eur, d("52.50000"));
        assert_eq!(r.total_eur, d("106.65"));
    }

    #[test]
    fn nne_sect14a_tou_arithmetic() {
        let mut i = base_nne();
        i.arbeitspreis = ArbeitspreisModell::Modul3ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("900"),
                preis_ct_per_kwh: d("4.0"),
            },
            st: MengePreis {
                menge_kwh: d("0"),
                preis_ct_per_kwh: d("0"),
            },
            nt: MengePreis {
                menge_kwh: d("600"),
                preis_ct_per_kwh: d("2.0"),
            },
        };
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.total_eur, d("48.00"));
        // BK6-22-300 defines three Tarifstufen; the ST band carries no energy
        // here but is still billed, so the invoice shows the full structure.
        assert_eq!(r.positions.len(), 3);
        assert_eq!(r.positions[0].text, "Netznutzung Arbeit HT (§14a Modul 3)");
        assert_eq!(r.positions[0].net_eur, d("36.00000"));
        assert_eq!(r.positions[1].text, "Netznutzung Arbeit ST (§14a Modul 3)");
        assert_eq!(r.positions[1].net_eur, d("0.00000"));
        assert_eq!(r.positions[2].net_eur, d("12.00000"));
    }

    /// Blindmehrarbeit is billed on the excess only, and only when there is one.
    ///
    /// The position kind, the Artikelnummer and the BO4E bridge all existed
    /// before the calculation did — nothing could produce the position, so a
    /// network that charges reactive energy was simply under-billed with no
    /// signal anywhere.
    #[test]
    fn blindmehrarbeit_bills_only_the_excess() {
        use crate::types::Blindarbeit;

        let mut i = base_nne();
        i.arbeitspreis = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: d("1000"),
            preis_ct_per_kwh: d("5.0"),
        });

        // Inside the free share → no position at all.
        i.blindarbeit = Some(Blindarbeit {
            blindarbeit_kvarh: d("400"),
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            preis_ct_per_kvarh: d("2.0"),
        });
        let r = settle_nne(&i).expect("settles");
        assert!(
            !r.positions
                .iter()
                .any(|p| p.kind == BillingPositionKind::Blindmehrarbeit),
            "a draw inside the free share raises no charge"
        );

        // Beyond it → 600 − (1 000 × 0,4843) = 115,7 kvarh × 0,02 EUR = 2,314 EUR.
        i.blindarbeit = Some(Blindarbeit {
            blindarbeit_kvarh: d("600"),
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            preis_ct_per_kvarh: d("2.0"),
        });
        let r = settle_nne(&i).expect("settles");
        let p = r
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::Blindmehrarbeit)
            .expect("the excess is billed");
        assert_eq!(p.quantity, d("115.700"));
        assert_eq!(p.unit, QuantityUnit::Kvarh);
        assert_eq!(p.net_eur, d("2.31400"));
        // The basis is the NB's Preisblatt under StromNEV §17 — not §18.
        assert!(
            p.trace.legal_refs.iter().any(
                |r| matches!(r, LegalReference::StromNev { paragraph } if *paragraph == "§17")
            ),
            "{:?}",
            p.trace.legal_refs
        );
    }

    /// An inverted period cannot reach the engine at all.
    ///
    /// Constructing `SettlementPeriod` is the check, so no per-calculation guard
    /// re-tests it.
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
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
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

        // Netzentgelt basis: 1500 kWh × 0.035 = 52.50, plus 1500 kW at the
        // 10.00 EUR/kW·Monat Monatsleistungspreis = 15 000.00.
        // Reduction: −90 % of 15 052.50 = −13 547.25.
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
        // 10.1 GWh at ~1422 kW → 7102 h: the 20 % tier.
        i.jahresarbeit_kwh = Some(d("10100000"));
        i.jahreshoechstleistung_kw = Some(d("1422"));
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

    /// Satz 2 asks whether the Stromverbrauch *übersteigt* ten Gigawattstunden,
    /// so an Abnahmestelle at exactly 10 GWh has no Satz 2 agreement available
    /// however high its utilisation.
    #[test]
    fn exactly_ten_gigawatt_hours_does_not_qualify_for_satz_two() {
        use crate::sect19::{Sect19Art, Sect19Vereinbarung};

        let mut i = base_nne();
        i.jahresarbeit_kwh = Some(d("10000000"));
        i.jahreshoechstleistung_kw = Some(d("1250")); // 8000 h
        i.sect19 = Some(Sect19Vereinbarung {
            art: Sect19Art::IntensiveNetznutzung,
            vereinbarter_prozentsatz: d("0.10"),
            genehmigung: None,
        });
        let r = settle_nne(&i).expect("settles");
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "SECT19_BANDLAST_CRITERIA_NOT_MET"),
            "{:?}",
            r.warnings
        );
    }

    /// §19 Abs. 2 Satz 5 / Satz 7 — a Vereinbarung is effective on a Genehmigung
    /// or, under a §29 Abs. 1 EnWG Festlegung, an Anzeige. An agreement naming
    /// neither is reported.
    #[test]
    fn a_sect19_agreement_with_no_regulatory_reference_is_reported() {
        use crate::sect19::{Sect19Art, Sect19Vereinbarung};

        let mut i = base_nne();
        i.sect19 = Some(Sect19Vereinbarung {
            art: Sect19Art::AtypischeNetznutzung,
            vereinbarter_prozentsatz: d("0.20"),
            genehmigung: None,
        });
        let r = settle_nne(&i).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "SECT19_OHNE_GENEHMIGUNG"),
            "{:?}",
            r.warnings
        );

        i.sect19.as_mut().unwrap().genehmigung = Some("BK4-22-089, Anzeige vom 12.01.2026".into());
        let r = settle_nne(&i).unwrap();
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.code == "SECT19_OHNE_GENEHMIGUNG"),
            "{:?}",
            r.warnings
        );
    }

    /// The agreed fraction is a *reduction* of the published Netzentgelt, so
    /// anything outside (0, 1] is refused rather than settled. The value reaches
    /// the engine from a settlement request, so nothing upstream bounds it.
    #[test]
    fn an_agreed_fraction_outside_the_unit_interval_is_refused() {
        use crate::sect19::{Sect19Art, Sect19Vereinbarung};

        for pct in ["1.5", "0", "-0.25"] {
            let mut i = base_nne();
            i.sect19 = Some(Sect19Vereinbarung {
                art: Sect19Art::AtypischeNetznutzung,
                vereinbarter_prozentsatz: d(pct),
                genehmigung: None,
            });
            assert!(
                matches!(settle_nne(&i), Err(BillingError::InvalidInput { .. })),
                "{pct} must be refused"
            );
        }
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
    /// The `Leistungspreis` type is the check, rather than a runtime error
    /// repeated at each call site.
    #[test]
    fn a_demand_charge_is_a_pair() {
        let mut i = base_nne();
        i.leistungspreis = Some(Leistungspreis {
            spitzenleistung_kw: d("10"),
            preis_eur_per_kw: d("4.20"),
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
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
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            sparte: Sparte::Strom,
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
            wiederverkaeufer: crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
            selbstausgestellt: false,
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
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31)).unwrap(),
            sparte: Sparte::Strom,
            actual_kwh: d("1400"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
            wiederverkaeufer: crate::umsatzsteuer::Wiederverkaeuferstatus::KEINER,
            selbstausgestellt: false,
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
            sparte: Sparte::Strom,
            malo_id: "51238696012".into(),
            empfaenger: MsbRechnungsempfaenger {
                rolle: MsbEmpfaengerRolle::Netzbetreiber,
                mp_id: "9900357000004".to_owned(),
            },
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
            sparte: Sparte::Strom,
            malo_id: "51238696012".into(),
            empfaenger: MsbRechnungsempfaenger {
                rolle: MsbEmpfaengerRolle::Netzbetreiber,
                mp_id: "9900357000004".to_owned(),
            },
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
    /// It lives on `InvoiceDocument`, where routing information belongs — not
    /// as a mutable field the caller patches after calculation.
    #[test]
    fn the_pid_lives_on_the_document_not_the_settlement() {
        let settlement = settle_nne(&base_nne()).unwrap();
        let doc = InvoiceDocument {
            settlement,
            pid: 31002,
            rechnungsnummer: "NNE-2025-001".to_owned(),
            correction_of: None,
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            cadence: None,
            abschlaege: Vec::new(),
        };
        assert_eq!(doc.pid, 31002);
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
        i.arbeitspreis = ArbeitspreisModell::Modul3ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("900"),
                preis_ct_per_kwh: d("4.0"),
            },
            st: MengePreis {
                menge_kwh: d("0"),
                preis_ct_per_kwh: d("0"),
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
    /// The §30 MsbG Preisobergrenze and the KAV ceiling are both Höchstbeträge,
    /// so an amount above either is one the customer may reclaim. Validating
    /// only that the fee is non-negative catches neither.
    #[test]
    fn a_metering_charge_above_the_msbg_ceiling_is_reported() {
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtEinstufung};

        let mut i = base_msb();
        i.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtEinstufung {
            jahresverbrauch_kwh: Some(d("9000")),
            ..PflichtEinstufung::default()
        }));
        i.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);

        // 9 000 kWh is §30 Abs. 1 Nr. 5; 40 EUR/a brutto is its ceiling, and
        // 5 EUR/month is 60 EUR/a netto — 71.40 brutto.
        i.grundgebuehr_eur_per_month = d("5.00");
        let over = settle_msb(&i).expect("settles");
        assert!(
            over.warnings.iter().any(|w| w.code == "MSB_ABOVE_MSBG_POG"),
            "71.40 EUR/a brutto exceeds the 40 EUR/a ceiling: {:?}",
            over.warnings
        );

        // 2.80 EUR/month is 33.60 EUR/a netto = 39.98 brutto — within it.
        i.grundgebuehr_eur_per_month = d("2.80");
        let within = settle_msb(&i).expect("settles");
        assert!(
            !within
                .warnings
                .iter()
                .any(|w| w.code == "MSB_ABOVE_MSBG_POG"),
            "39.98 EUR/a brutto is within the ceiling: {:?}",
            within.warnings
        );
    }

    /// **Invariant: the §30 MsbG ceiling is measured against the gross charge.**
    ///
    /// Every §30 figure is stated „brutto jährlich", and the settlement's
    /// Grundgebühr is net. Comparing net against gross hands the check the whole
    /// Umsatzsteuer rate as headroom: 3.33 EUR/month is 39.96 EUR/a netto — under
    /// a 40 EUR ceiling read as net — and 47.55 EUR/a brutto, 18.9 % above the
    /// statutory maximum.
    #[test]
    fn the_msbg_ceiling_is_measured_against_the_gross_charge() {
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtEinstufung};

        let mut i = base_msb();
        i.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtEinstufung {
            jahresverbrauch_kwh: Some(d("9000")),
            ..PflichtEinstufung::default()
        }));
        i.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);
        i.grundgebuehr_eur_per_month = d("3.33");

        let r = settle_msb(&i).expect("settles");
        let finding = r
            .warnings
            .iter()
            .find(|w| w.code == "MSB_ABOVE_MSBG_POG")
            .unwrap_or_else(|| panic!("39.96 EUR/a netto is 47.55 brutto: {:?}", r.warnings));
        assert!(
            finding.message.contains("47.55"),
            "the finding states the gross charge: {}",
            finding.message
        );
        assert!(
            finding.message.contains("brutto"),
            "the finding says which side of the tax it compared: {}",
            finding.message
        );

        // The boundary case: exactly at the ceiling is not above it.
        i.grundgebuehr_eur_per_month = d("40") / (Decimal::ONE + dec!(0.19)) / Decimal::from(12);
        let at = settle_msb(&i).expect("settles");
        assert!(
            !at.warnings.iter().any(|w| w.code == "MSB_ABOVE_MSBG_POG"),
            "40.00 EUR/a brutto is the maximum, not an excess: {:?}",
            at.warnings
        );
    }

    /// **Invariant: the §30 MsbG ceiling measures the whole Messstellenbetrieb.**
    ///
    /// §30 caps the „Entgelt für den Messstellenbetrieb", and §3 Abs. 2 Nr. 1
    /// MsbG puts the Messung inside it — Messstellenbetrieb umfasst die
    /// „Gewährleistung einer mess- und eichrechtskonformen Messung […]
    /// einschließlich der Messwertaufbereitung". §17 Abs. 7 Satz 1 StromNEV says
    /// it from the other side: „ein Entgelt für den Messstellenbetrieb, zu dem
    /// auch die Messung gehört". A settlement that splits the charge over a
    /// Grundgebühr and a Messdienstleistung must be measured on the sum, or the
    /// split itself clears the ceiling.
    #[test]
    fn the_msbg_ceiling_includes_the_messdienstleistung() {
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtEinstufung};

        let mut i = base_msb();
        i.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtEinstufung {
            jahresverbrauch_kwh: Some(d("9000")),
            ..PflichtEinstufung::default()
        }));
        i.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);
        i.billing_months = 1;

        // 9 000 kWh is §30 Abs. 1 Nr. 5 — a 40 EUR/a brutto ceiling.
        // The Grundgebühr alone is 36 EUR/a netto = 42.84 brutto, already over.
        i.grundgebuehr_eur_per_month = d("3.00");
        i.messdienstleistung_eur = None;
        let ohne = settle_msb(&i).expect("settles");
        let allein = ohne
            .warnings
            .iter()
            .find(|w| w.code == "MSB_ABOVE_MSBG_POG")
            .unwrap_or_else(|| panic!("42.84 exceeds 40: {:?}", ohne.warnings));
        assert!(allein.message.contains("42.84"), "{}", allein.message);

        // Adding 2.50 EUR/month of Messdienstleistung takes the same metering
        // point to 66 EUR/a netto = 78.54 brutto. The finding has to state that
        // figure, not the Grundgebühr's.
        i.messdienstleistung_eur = Some(d("2.50"));
        let mit = settle_msb(&i).expect("settles");
        let zusammen = mit
            .warnings
            .iter()
            .find(|w| w.code == "MSB_ABOVE_MSBG_POG")
            .unwrap_or_else(|| panic!("78.54 exceeds 40: {:?}", mit.warnings));
        assert!(
            zusammen.message.contains("78.54"),
            "the finding measures both positions: {}",
            zusammen.message
        );

        // And a point that clears the ceiling only because the charge is split
        // is reported: 1.50 + 1.50 EUR/month is 36 EUR/a netto = 42.84 brutto.
        i.grundgebuehr_eur_per_month = d("1.50");
        i.messdienstleistung_eur = Some(d("1.50"));
        let geteilt = settle_msb(&i).expect("settles");
        assert!(
            geteilt
                .warnings
                .iter()
                .any(|w| w.code == "MSB_ABOVE_MSBG_POG"),
            "splitting a charge over two positions does not raise the ceiling: {:?}",
            geteilt.warnings
        );
    }

    /// Annualising is what makes the comparison right.
    ///
    /// The ceiling is per year and the charge per month; billing a year in
    /// instalments does not raise the cap.
    #[test]
    fn the_ceiling_is_compared_against_the_annualised_charge() {
        use crate::msbg::{Entgeltschuldner, MessstellenKategorie, PflichtEinstufung};

        let mut i = base_msb();
        i.messstellen_kategorie = Some(MessstellenKategorie::Pflichteinbau(PflichtEinstufung {
            jahresverbrauch_kwh: Some(d("80000")),
            ..PflichtEinstufung::default()
        }));
        i.entgeltschuldner = Some(Entgeltschuldner::Letztverbraucher);
        // 80 000 kWh is Nr. 2 — a 140 EUR/a ceiling. 12 EUR/month = 144 EUR/a — over, even though a
        // single month is far below the annual figure.
        i.grundgebuehr_eur_per_month = d("12.00");
        let r = settle_msb(&i).expect("settles");
        assert!(r.warnings.iter().any(|w| w.code == "MSB_ABOVE_MSBG_POG"));
    }

    #[test]
    fn msb_has_msbg_reference() {
        let input = MsbInput {
            sparte: Sparte::Strom,
            malo_id: "51238696012".into(),
            empfaenger: MsbRechnungsempfaenger {
                rolle: MsbEmpfaengerRolle::Netzbetreiber,
                mp_id: "9900357000004".to_owned(),
            },
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
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
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
        i.arbeitspreis = ArbeitspreisModell::Modul3ZeitVariabel {
            ht: MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("4.0"),
            },
            st: MengePreis {
                menge_kwh: d("0"),
                preis_ct_per_kwh: d("0"),
            },
            nt: MengePreis {
                menge_kwh: d("0"),
                preis_ct_per_kwh: d("2.0"),
            },
        };
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.positions[1].net_eur, Decimal::ZERO);
    }

    /// B′ is published „für Strommengen über 1 000 000 kWh", so a privileged
    /// Entnahmestelle carries A′ on the year's first Gigawattstunde and B′ only
    /// above it. Billing the whole quantity at B′ would understate the
    /// §19-Aufschlag by 1 GWh × (A′ − B′).
    #[test]
    fn a_privileged_entnahmestelle_pays_the_full_rate_on_the_first_gigawatt_hour() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
        i.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::B;
        i.arbeitspreis = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: d("2500000"),
            preis_ct_per_kwh: d("3.5"),
        });
        // The year opens with this period.
        i.enfg_jahresvorverbrauch_kwh = Some(Decimal::ZERO);

        let r = settle_nne(&i).unwrap();
        let sect19: Vec<_> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::Sect19StromNevUmlage)
            .collect();
        assert_eq!(sect19.len(), 2, "one tranche each side of the boundary");
        // 1 000 000 kWh × 1.559 ct = 15 590.00 EUR
        assert_eq!(sect19[0].quantity, d("1000000.000"));
        assert_eq!(sect19[0].net_eur, d("15590.00000"));
        // 1 500 000 kWh × 0.050 ct = 750.00 EUR
        assert_eq!(sect19[1].quantity, d("1500000.000"));
        assert_eq!(sect19[1].net_eur, d("750.00000"));

        // The other two levies stay one position at the non-privileged rate.
        for kind in [
            BillingPositionKind::OffshoreNetzumlage,
            BillingPositionKind::KwkgUmlage,
        ] {
            assert_eq!(
                r.positions.iter().filter(|p| p.kind == kind).count(),
                1,
                "{kind:?}"
            );
        }
    }

    /// A period wholly inside the year's first Gigawattstunde carries A′ on all
    /// of it. A privileged group is not a privileged rate — the tranche is.
    #[test]
    fn a_privileged_period_inside_the_first_gigawatt_hour_is_billed_at_the_full_rate() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
        i.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::B;
        i.arbeitspreis = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: d("300000"),
            preis_ct_per_kwh: d("3.5"),
        });
        i.enfg_jahresvorverbrauch_kwh = Some(d("200000"));

        let r = settle_nne(&i).unwrap();
        let sect19: Vec<_> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::Sect19StromNevUmlage)
            .collect();
        assert_eq!(sect19.len(), 1, "one tranche, and it is the A′ one");
        // 300 000 kWh × 1.559 ct = 4 677.00 EUR, not 300 000 × 0.050 = 150.00.
        assert_eq!(sect19[0].net_eur, d("4677.00000"));
    }

    /// Once the year's allowance is spent the whole period is privileged, and
    /// the levy is one position again.
    #[test]
    fn a_privileged_entnahmestelle_past_the_threshold_bills_one_tranche() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 06 - 01), date!(2026 - 06 - 30)).unwrap();
        i.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::B;
        i.arbeitspreis = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: d("400000"),
            preis_ct_per_kwh: d("3.5"),
        });
        i.enfg_jahresvorverbrauch_kwh = Some(d("4000000"));

        let r = settle_nne(&i).unwrap();
        let sect19: Vec<_> = r
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::Sect19StromNevUmlage)
            .collect();
        assert_eq!(sect19.len(), 1);
        assert_eq!(sect19[0].net_eur, d("200.00000"));
    }

    /// Without a Jahresvorverbrauch the period is billed as though it opened the
    /// year — the over-billing direction — and says so.
    #[test]
    fn a_privileged_entnahmestelle_without_a_vorverbrauch_is_reported() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2026 - 06 - 01), date!(2026 - 06 - 30)).unwrap();
        i.letztverbrauchergruppe = crate::umlagen::Letztverbrauchergruppe::C;
        i.enfg_jahresvorverbrauch_kwh = None;
        let r = settle_nne(&i).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "ENFG_VORVERBRAUCH_MISSING"),
            "{:?}",
            r.warnings
        );
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
        assert_eq!(levy_total.round_kfm(2), dec!(44.19));
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
        assert_eq!(sect19.net_eur.round_kfm(2), dec!(1.50));
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
                module: Sect14aModule::Modul3,
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
        assert_eq!(SettlementType::NneStrom.default_pid(), 31002);
        assert_eq!(SettlementType::NneGas.default_pid(), 31002);
        assert_eq!(SettlementType::MmmStrom.default_pid(), 31005);
        assert_eq!(SettlementType::MmmGas.default_pid(), 31005);
        assert_eq!(SettlementType::MmmSelbstausstellt.default_pid(), 31006);
        assert_eq!(SettlementType::MsbRechnung.default_pid(), 31009);
        assert_eq!(SettlementType::GasAwhSperrung.default_pid(), 31011);
    }

    // ── sparte, recipient_mp_id, reversal, Gas path, KA group, validation ──

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
    fn recipient_mp_id_is_populated_for_nne() {
        let r = settle_nne(&base_nne()).unwrap();
        assert_eq!(r.recipient_mp_id, "9900012345678");
    }

    /// PID 31009 is issued **by** the MSB, to the NB / LF / ESA.
    ///
    /// Naming the MSB as `counterparty` (recipient) and the NB as sender inverts
    /// the invoice: the party owed money becomes the one billing for it.
    /// Verified against the
    /// *Anwendungsübersicht der Prüfidentifikatoren* 4.0, which lists seven
    /// Anwendungsfälle for 31009, all `MSB -> {NB, LF, ESA}`.
    #[test]
    fn msb_invoice_is_sent_by_the_msb_to_each_of_the_three_recipient_roles() {
        for (rolle, empfaenger_id) in [
            (MsbEmpfaengerRolle::Netzbetreiber, "9900357000004"),
            (MsbEmpfaengerRolle::Lieferant, "9900111000002"),
            (MsbEmpfaengerRolle::Energieserviceanbieter, "9905550000005"),
        ] {
            let input = MsbInput {
                sparte: Sparte::Strom,
                malo_id: "51238696012".into(),
                msb_mp_id: "9900999000001".into(),
                empfaenger: MsbRechnungsempfaenger {
                    rolle,
                    mp_id: empfaenger_id.to_owned(),
                },
                period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 01 - 31))
                    .unwrap(),
                grundgebuehr_eur_per_month: d("15.00"),
                billing_months: 1,
                messdienstleistung_eur: None,
                messstellen_kategorie: None,
                entgeltschuldner: None,
            };
            let r = settle_msb(&input).unwrap();
            assert_eq!(
                r.sender_mp_id,
                "9900999000001",
                "the MSB issues the invoice ({})",
                rolle.code()
            );
            assert_eq!(
                r.recipient_mp_id,
                empfaenger_id,
                "the {} is billed",
                rolle.code()
            );
        }
    }

    #[test]
    fn reversal_negates_all_positions_and_total() {
        let original = settle_nne(&base_nne()).unwrap();
        let storno = reverse(&original, KorrekturGrund::Messwertkorrektur);
        assert_eq!(storno.total_eur, -original.total_eur);
        assert_eq!(storno.status, SettlementStatus::Reversal);
        for (orig, rev) in original.positions.iter().zip(storno.positions.iter()) {
            assert_eq!(rev.net_eur, -orig.net_eur);
            assert!(rev.text.starts_with("Storno:"));
        }
    }

    #[test]
    fn reversal_preserves_recipient_mp_id() {
        let original = settle_nne(&base_nne()).unwrap();
        let storno = reverse(&original, KorrekturGrund::Messwertkorrektur);
        assert_eq!(storno.recipient_mp_id, original.recipient_mp_id);
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
            sparte: Sparte::Strom,
            malo_id: "51238696012".into(),
            empfaenger: MsbRechnungsempfaenger {
                rolle: MsbEmpfaengerRolle::Netzbetreiber,
                mp_id: "9900357000004".to_owned(),
            },
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
            system: LeistungspreisSystem::Monat {
                monate: Decimal::ONE,
            },
        });
        i.konzessionsabgabe = Some(Konzessionsabgabe {
            satz_ct_per_kwh: d("0.11"),
            klasse: KaKundengruppe::Sondervertragskunde,
        });
        let original = settle_nne(&i).unwrap();
        let storno = reverse(&original, KorrekturGrund::Messwertkorrektur);
        assert_eq!(storno.positions.len(), original.positions.len());
        assert_eq!(storno.total_eur, -original.total_eur);
        assert_eq!(storno.recomputed_total(), storno.total_eur);
    }

    // ── §14a Modul 1 (BNetzA BK6-22-300 flat reduction) ──────────────────────

    /// Modul 1 is a *pauschale* reduction: the energy is billed at the full
    /// Arbeitspreis and a flat annual amount is credited pro rata alongside it.
    ///
    /// The credit does not scale with consumption — that is what makes it
    /// pauschal, and what separates it from Modul 2, which reduces the
    /// Arbeitspreis by a percentage. Both were once the same computation here,
    /// with Modul 1 wearing Modul 2's mechanism.
    #[test]
    fn sect14a_modul1_credits_a_flat_amount_beside_the_full_arbeitspreis() {
        let mut i = base_nne();
        i.arbeitspreis = modul1(d("120.00"));
        let r = settle_nne(&i).unwrap();

        assert_eq!(r.positions.len(), 2, "the Arbeit position plus the credit");
        // 1500 kWh × 3.5 ct = 52.50 EUR, billed in full.
        assert_eq!(r.positions[0].net_eur, d("52.50000"));
        // A twelfth of a year at 120 EUR/Jahr. The credit is computed from the
        // exact twelfth, so twelve of them come to the published 120 EUR and
        // not to 119.99952.
        let credit = &r.positions[1];
        assert_eq!(credit.quantity, d("0.083333333"));
        assert_eq!(credit.unit, QuantityUnit::Jahr);
        assert_eq!(credit.unit_price_eur, d("-120.000000"));
        assert_eq!(credit.net_eur, d("-10.00000"));
        assert_eq!(
            credit.net_eur * Decimal::from(12),
            d("-120.00000"),
            "twelve monthly credits are the annual pauschale"
        );
        assert_eq!(r.total_eur, d("42.50"));

        assert!(
            r.positions[1].text.contains("pauschale"),
            "{}",
            r.positions[1].text
        );
        let refs = r.all_legal_refs();
        assert!(refs.iter().any(|x| x.contains("Modul 1")));
        assert!(refs.iter().any(|x| x.contains("BK6-22-300")));
    }

    /// Doubling the consumption does not double the credit — the defining
    /// property of a pauschale, and the one the old factor model got wrong.
    #[test]
    fn the_modul1_credit_does_not_scale_with_consumption() {
        let credit_for = |kwh: &str| {
            let mut i = base_nne();
            i.arbeitspreis = ArbeitspreisModell::Modul1Pauschal {
                basis: MengePreis {
                    menge_kwh: d(kwh),
                    preis_ct_per_kwh: d("3.5"),
                },
                pauschale_eur_pro_jahr: d("120.00"),
                jahresanteil: crate::types::Jahresanteil::MONAT,
            };
            settle_nne(&i).unwrap().positions[1].net_eur
        };
        assert_eq!(credit_for("1500"), credit_for("3000"));
    }

    #[test]
    /// A zero pauschale bills exactly like plain Arbeit — the credit position is
    /// still emitted, so the invoice shows the module was in force.
    fn sect14a_modul1_with_a_zero_pauschale_bills_the_full_arbeitspreis() {
        let mut i = base_nne();
        i.arbeitspreis = modul1(d("0.00"));
        let r = settle_nne(&i).unwrap();
        assert_eq!(r.total_eur, d("52.50"));
    }

    /// **Invariant: the Modul 1 credit is a rate times a quantity.**
    ///
    /// The position states the Netzbetreiber's published annual pauschale as the
    /// unit price and the share of a year billed as the quantity, so the two
    /// multiply out to the net. Stating the period's whole credit as the *unit*
    /// price instead leaves the position out by the pro-rating factor — an
    /// arithmetic error of about 1 100 % on a monthly run, far outside the
    /// checker's 1 % tolerance, which makes the invoice undispatchable.
    #[test]
    fn the_modul1_credit_states_the_annual_rate_as_its_unit_price() {
        let mut i = base_nne();
        i.arbeitspreis = super::modul1(d("120.00"));
        let credit = &settle_nne(&i).expect("settles").positions[1];

        assert_eq!(credit.unit_price_eur, d("-120.000000"), "EUR per year");
        assert_eq!(
            credit.quantity,
            d("0.083333333"),
            "the share of a year billed"
        );
        assert!(multiplies_out(credit));
    }

    /// **Invariant: a Jahresanteil is a share of a year.**
    ///
    /// Outside `(0, 1]` it is not one, and the type refuses it before any
    /// arithmetic sees it — including when it arrives over the wire, which is
    /// where an unconstrained `Decimal` reached the engine unchecked.
    #[test]
    fn a_jahresanteil_outside_the_unit_interval_is_refused() {
        use crate::types::Jahresanteil;

        for bad in ["-1", "0", "1.5"] {
            assert!(
                Jahresanteil::new(d(bad)).is_err(),
                "{bad} is not a share of a year"
            );
            assert!(
                serde_json::from_str::<Jahresanteil>(&format!("\"{bad}\"")).is_err(),
                "{bad} must be refused on the wire too"
            );
        }
        assert_eq!(
            Jahresanteil::new(Decimal::ONE).expect("a whole year"),
            Jahresanteil::JAHR
        );
    }

    /// **Invariant: a negative pauschale is refused, not billed.**
    ///
    /// The pauschale is an amount the Netzbetreiber grants. A negative one turns
    /// the §14a credit the customer is entitled to into a charge against them,
    /// and the invoice still adds up.
    #[test]
    fn a_negative_modul1_pauschale_is_refused() {
        let mut i = base_nne();
        i.arbeitspreis = super::modul1(d("-120.00"));
        assert!(matches!(
            settle_nne(&i),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    /// **Invariant: the share credited is the share served.**
    ///
    /// `1` is a valid Jahresanteil and the wrong one for a month: it credits the
    /// whole annual pauschale on every monthly run, twelve times over the year.
    /// The range check cannot see the period, so the settlement measures the two
    /// against each other.
    #[test]
    fn a_jahresanteil_that_disagrees_with_the_period_is_reported() {
        use crate::types::Jahresanteil;

        let modul1_with = |anteil: Jahresanteil| {
            let mut i = base_nne();
            i.arbeitspreis = ArbeitspreisModell::Modul1Pauschal {
                basis: MengePreis {
                    menge_kwh: d("1500"),
                    preis_ct_per_kwh: d("3.5"),
                },
                pauschale_eur_pro_jahr: d("120.00"),
                jahresanteil: anteil,
            };
            settle_nne(&i).expect("settles")
        };
        let flagged = |r: &SettlementResult| {
            r.warnings
                .iter()
                .any(|w| w.code == "MODUL1_JAHRESANTEIL_MISMATCH")
        };

        // The period is one month.
        assert!(flagged(&modul1_with(Jahresanteil::JAHR)));
        assert!(!flagged(&modul1_with(Jahresanteil::MONAT)));
    }

    /// **Invariant: a period's share of a year is measured year by year.**
    ///
    /// Each calendar year a period touches contributes its own days over its own
    /// length. Taking the divisor from the starting year alone mis-scales every
    /// period that crosses a leap-year boundary, and the result still looks like
    /// a plausible fraction, so nothing downstream can catch it.
    #[test]
    fn a_period_share_is_measured_against_each_year_it_touches() {
        let anteil = |from, to| {
            SettlementPeriod::new(from, to)
                .expect("ordered")
                .jahresanteil()
        };

        // A whole calendar year is exactly one, leap year included.
        assert_eq!(anteil(date!(2025 - 01 - 01), date!(2025 - 12 - 31)), d("1"));
        assert_eq!(anteil(date!(2024 - 01 - 01), date!(2024 - 12 - 31)), d("1"));

        // 17 days of 2023 (365) plus 14 of 2024 (366) — not 31 over either.
        let straddle = anteil(date!(2023 - 12 - 15), date!(2024 - 01 - 14));
        let erwartet = d("17") / d("365") + d("14") / d("366");
        assert_eq!(straddle, erwartet);
        assert_ne!(
            straddle,
            d("31") / d("365"),
            "the starting year's length does not measure the whole period"
        );
    }

    // ── §17 StromNEV — the two Leistungspreissysteme ─────────────────────────

    /// **Invariant: a Jahresleistungsentgelt is a product of two figures.**
    ///
    /// §17 Abs. 2 Satz 2 StromNEV: „Das Jahresleistungsentgelt ist das Produkt
    /// aus dem jeweiligen Jahresleistungspreis und der Jahreshöchstleistung in
    /// Kilowatt der jeweiligen Entnahme im Abrechnungsjahr." §17 names no
    /// day-count convention for it, so the rate is billed as published against
    /// the Jahreshöchstleistung — dividing it by the days in the year would
    /// invent a Zwölftelung the ordinance does not state and no Preisblatt
    /// publishes.
    #[test]
    fn a_jahresleistungspreis_is_billed_against_the_jahreshoechstleistung() {
        let bill = |from, to| {
            let mut i = base_nne();
            i.period = SettlementPeriod::new(from, to).expect("ordered");
            i.leistungspreis = Some(Leistungspreis {
                spitzenleistung_kw: d("500"),
                preis_eur_per_kw: d("90.00"),
                system: LeistungspreisSystem::Jahr,
            });
            settle_nne(&i).expect("settles")
        };
        let leistung = |r: &SettlementResult| {
            r.positions
                .iter()
                .find(|p| p.kind == BillingPositionKind::NneLeistung)
                .cloned()
                .expect("the demand charge is billed")
        };
        let unterjaehrig = |r: &SettlementResult| {
            r.warnings
                .iter()
                .any(|w| w.code == "JAHRESLEISTUNGSPREIS_UNTERJAEHRIG")
        };

        // The Abrechnungsjahr: 500 kW × 90 EUR/kW = 45 000 EUR, as published.
        let jahr = bill(date!(2025 - 01 - 01), date!(2025 - 12 - 31));
        assert_eq!(leistung(&jahr).unit_price_eur, d("90"));
        assert_eq!(leistung(&jahr).net_eur, d("45000.00000"));
        assert!(!unterjaehrig(&jahr));

        // A leap year is still one Abrechnungsjahr and costs the same.
        let schaltjahr = bill(date!(2024 - 01 - 01), date!(2024 - 12 - 31));
        assert_eq!(leistung(&schaltjahr).net_eur, d("45000.00000"));
        assert!(!unterjaehrig(&schaltjahr));

        // A month is not the Abrechnungsjahr. The settlement still bills what
        // Abs. 2 Satz 2 states and says the period does not match it.
        let monat = bill(date!(2025 - 01 - 01), date!(2025 - 01 - 31));
        assert_eq!(leistung(&monat).net_eur, d("45000.00000"));
        assert!(
            unterjaehrig(&monat),
            "an unterjährige Abrechnung of a Jahresleistungspreis is reported"
        );

        for r in [&jahr, &schaltjahr, &monat] {
            assert!(multiplies_out(&leistung(r)));
        }
    }

    /// **Invariant: a Monatsleistungspreis bills the months it serves.**
    ///
    /// §17 Abs. 8 StromNEV offers Tagesleistungspreise for Landstrom „neben
    /// einem Jahres- und Monatsleistungspreissystem", so a Preisblatt may state
    /// a EUR/kW·Monat price. It is billed against the period's Höchstleistung
    /// for the months billed, and the month count has to be the months served.
    #[test]
    fn a_monatsleistungspreis_bills_the_months_it_serves() {
        let bill = |monate| {
            let mut i = base_nne();
            i.leistungspreis = Some(Leistungspreis {
                spitzenleistung_kw: d("500"),
                preis_eur_per_kw: d("7.50"),
                system: LeistungspreisSystem::Monat { monate },
            });
            settle_nne(&i).expect("settles")
        };

        // The January period: 500 kW × 7.50 EUR/kW·Monat = 3 750 EUR.
        let einer = bill(Decimal::ONE);
        let p = einer
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::NneLeistung)
            .expect("the demand charge is billed");
        assert_eq!(p.net_eur, d("3750.00000"));
        assert!(multiplies_out(p));
        assert!(
            !einer
                .warnings
                .iter()
                .any(|w| w.code == "MONATSLEISTUNGSPREIS_MONATE_MISMATCH")
        );

        // Twelve months over a one-month period is a twelvefold over-charge
        // that multiplies out perfectly, so only the month count can catch it.
        assert!(
            bill(Decimal::from(12))
                .warnings
                .iter()
                .any(|w| w.code == "MONATSLEISTUNGSPREIS_MONATE_MISMATCH")
        );
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
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
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
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
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
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
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

        let (reversal, corrected) = correct(&original, replacement, KorrekturGrund::Tarifkorrektur);
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

    // ── AgNeS refusal (regime turnover 2028 → 2029) ───────────────────────────

    /// NNE positions are priced under StromNEV/GasNEV — a 2029 period is
    /// governed by AgNeS (GBK-25-01), whose tables are not festgelegt, so the
    /// settlement is refused rather than computed under lapsed rules.
    #[test]
    fn a_2029_nne_settlement_is_refused_under_agnes() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2029 - 01 - 01), date!(2029 - 01 - 31)).unwrap();
        let err = settle_nne(&i).expect_err("an AgNeS period must be refused");
        assert!(matches!(
            err,
            BillingError::UnsupportedEntgeltRegime { tarifjahr: 2029 }
        ));
        assert!(err.to_string().contains("GBK-25-01"), "{err}");
    }

    /// AWH charges rest on the GasNEV §14 authorisation — same refusal.
    #[test]
    fn a_2029_gas_awh_settlement_is_refused_under_agnes() {
        let input = GasAwhInput {
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2029 - 01 - 01), date!(2029 - 01 - 31)).unwrap(),
            tariff_sheet_id: None,
            awh_positionen: vec![AwhPositionInput {
                beschreibung: "Sperrung Gaszähler".into(),
                anzahl: 1,
                preis_eur: d("45.00"),
                artikel_id: Some("2-01-7-001".to_owned()),
            }],
        };
        assert!(matches!(
            settle_gas_awh(&input),
            Err(BillingError::UnsupportedEntgeltRegime { tarifjahr: 2029 })
        ));
    }

    /// MMM prices are formed on the Netzzugang axis (GPKE / GaBi Gas), not by
    /// the Entgeltbildung AgNeS replaces — a 2029 MMM settlement stays
    /// computable, and its regime tag records the AgNeS Entgelt axis.
    #[test]
    fn a_2029_mmm_settlement_stays_computable() {
        let mut i = base_mmm();
        i.period = SettlementPeriod::new(date!(2029 - 02 - 01), date!(2029 - 02 - 28)).unwrap();
        let r = settle_mmm(&i).expect("MMM does not price on the Entgelt axis");
        assert_eq!(r.regime.entgelt(), crate::regulatory::EntgeltRegime::AgNeS);
    }

    /// MSB charges are formed under MsbG, which does not lapse with the
    /// Verordnungen — a 2029 MSB settlement stays computable.
    #[test]
    fn a_2029_msb_settlement_stays_computable() {
        let mut i = base_msb();
        i.period = SettlementPeriod::new(date!(2029 - 02 - 01), date!(2029 - 02 - 28)).unwrap();
        let r = settle_msb(&i).expect("MSB does not price on the Entgelt axis");
        assert_eq!(r.regime.entgelt(), crate::regulatory::EntgeltRegime::AgNeS);
    }

    // ── REGIME_TURNOVER_IN_PERIOD is emitted by every builder ─────────────────

    /// A period across the 2025/2026 Netzzugang turnover warns on NNE too —
    /// not only on MMM, where the check first lived.
    #[test]
    fn a_straddling_nne_period_warns() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2025 - 12 - 15), date!(2026 - 01 - 15)).unwrap();
        let r = settle_nne(&i).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "REGIME_TURNOVER_IN_PERIOD"),
            "warnings: {:?}",
            r.warnings
        );
    }

    /// …and on MSB, which is exempt from the AgNeS guard but must still report
    /// a period the turnover cuts in two.
    #[test]
    fn a_straddling_msb_period_warns() {
        let mut i = base_msb();
        i.period = SettlementPeriod::new(date!(2028 - 12 - 15), date!(2029 - 01 - 15)).unwrap();
        let r = settle_msb(&i).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.code == "REGIME_TURNOVER_IN_PERIOD"),
            "warnings: {:?}",
            r.warnings
        );
    }

    /// A reversal re-emits the turnover warning: the mirror of a straddling
    /// settlement straddles just the same.
    #[test]
    fn a_reversal_of_a_straddling_settlement_carries_the_warning() {
        let mut i = base_nne();
        i.period = SettlementPeriod::new(date!(2025 - 12 - 15), date!(2026 - 01 - 15)).unwrap();
        let original = settle_nne(&i).unwrap();
        let reversal = reverse(&original, KorrekturGrund::Messwertkorrektur);
        assert!(
            reversal
                .warnings
                .iter()
                .any(|w| w.code == "REGIME_TURNOVER_IN_PERIOD"),
            "warnings: {:?}",
            reversal.warnings
        );
    }
}

// ── Property tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::types::MengePreis;
    use crate::types::SettlementPeriod;
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
                blindarbeit: None,
                malo_id: "51238696012".into(),
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 12 - 31)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: kwh,
                preis_ct_per_kwh: ct,
            }),
            leistungspreis: None,
                letztverbrauchergruppe: Default::default(),
                enfg_jahresvorverbrauch_kwh: None,
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
                let reversal = reverse(&original, KorrekturGrund::Messwertkorrektur);
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
                blindarbeit: None,
                malo_id: "51238696012".into(),
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2025 - 01 - 01), date!(2025 - 12 - 31)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: kwh,
                preis_ct_per_kwh: ct,
            }),
            leistungspreis: None,
                letztverbrauchergruppe: Default::default(),
                enfg_jahresvorverbrauch_kwh: None,
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
                    // Any non-negative pauschale is a credit, so the total can
                    // only move down — the invariant, and it holds independently
                    // of consumption.
                    pauschale_eur_pro_jahr: (Decimal::ONE - factor)
                        * Decimal::from(1200u32),
                    // The period is a full year, so the whole pauschale is due.
                    jahresanteil: crate::types::Jahresanteil::JAHR,
                };
                if let Ok(reduced) = settle_nne(&reduced_input) {
                    prop_assert!(
                        reduced.total_eur <= unreduced.total_eur,
                        "Modul 1 reduced total must be ≤ unreduced total"
                    );
                }
            }
        }

        /// **Invariant: every position a settlement emits multiplies out.**
        ///
        /// `quantity × unit_price_eur ≈ net_eur` for every position of every
        /// settlement, at the tolerance `invoic-checker` applies to the rendered
        /// document (`arithmetic_tolerance_ppm = 10_000`, 1 %). The three fields
        /// travel onto the invoice verbatim and the recipient re-multiplies
        /// them, so a position whose own figures disagree is an invoice that
        /// cannot be dispatched.
        ///
        /// Randomised over the inputs that reach the arithmetic — the quantity,
        /// the rate, the Modul 1 pauschale and the demand charge — because the
        /// defect is a mis-stated *field*, which a total-only assertion cannot
        /// see at any input.
        #[test]
        fn every_position_multiplies_out(
            kwh in arb_positive_kwh(),
            ct in arb_ct_per_kwh(),
            pauschale_eur in 0u64..50_000u64,
            kw in 0u64..10_000u64,
            eur_per_kw in 0u64..20_000u64,
        ) {
            let mut input = NneInput {
                blindarbeit: None,
                malo_id: "51238696012".into(),
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".into(),
                period: SettlementPeriod::new(date!(2025 - 03 - 01), date!(2025 - 03 - 31))
                    .unwrap(),
                arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                    menge_kwh: kwh,
                    preis_ct_per_kwh: ct,
                }),
                leistungspreis: Some(crate::types::Leistungspreis {
                    spitzenleistung_kw: Decimal::from(kw),
                    preis_eur_per_kw: Decimal::new(eur_per_kw as i64, 2),
                    system: crate::types::LeistungspreisSystem::Monat { monate: Decimal::ONE },
                }),
                letztverbrauchergruppe: Default::default(),
                enfg_jahresvorverbrauch_kwh: None,
                sect19_umlage_ct_per_kwh: None,
                offshore_umlage_ct_per_kwh: None,
                kwkg_umlage_ct_per_kwh: None,
                netzebene: None,
                sect19: None,
                gas_kapazitaet: None,
                jahreshoechstleistung_kw: None,
                jahresarbeit_kwh: None,
                konzessionsabgabe: Some(crate::types::Konzessionsabgabe {
                    satz_ct_per_kwh: Decimal::new(11, 2),
                    klasse: crate::types::KaKundengruppe::Sondervertragskunde,
                }),
                grundpreis: None,
                tariff_sheet_id: None,
                sparte: Sparte::Strom,
            };

            for arbeitspreis in [
                ArbeitspreisModell::Einheitlich(MengePreis {
                    menge_kwh: kwh,
                    preis_ct_per_kwh: ct,
                }),
                ArbeitspreisModell::Modul1Pauschal {
                    basis: MengePreis {
                        menge_kwh: kwh,
                        preis_ct_per_kwh: ct,
                    },
                    pauschale_eur_pro_jahr: Decimal::new(pauschale_eur as i64, 2),
                    jahresanteil: crate::types::Jahresanteil::MONAT,
                },
            ] {
                input.arbeitspreis = arbeitspreis;
                let Ok(result) = settle_nne(&input) else { continue };
                let reversal = reverse(&result, KorrekturGrund::Messwertkorrektur);
                for settlement in [&result, &reversal] {
                    for p in &settlement.positions {
                        let computed = p.quantity * p.unit_price_eur;
                        if computed.is_zero() {
                            prop_assert!(p.net_eur.is_zero(), "{p:?}");
                            continue;
                        }
                        // `invoic-checker`'s own rule: |stated − computed| × 10⁶
                        // ≤ |computed| × ppm.
                        prop_assert!(
                            (p.net_eur - computed).abs() * Decimal::from(1_000_000u32)
                                <= computed.abs() * Decimal::from(10_000u32),
                            "{} states {} × {} = {} but nets {}",
                            p.text,
                            p.quantity,
                            p.unit_price_eur,
                            computed,
                            p.net_eur,
                        );
                    }
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
    use crate::types::SettlementPeriod;
    use crate::types::SpotpreisInterval;
    use rust_decimal::Decimal;
    use time::macros::date;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("valid decimal literal")
    }

    fn base_nne() -> NneInput {
        NneInput {
            blindarbeit: None,
            malo_id: "51238696012".into(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".into(),
            period: SettlementPeriod::new(date!(2026 - 01 - 15), date!(2026 - 01 - 16)).unwrap(),
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: d("1500"),
                preis_ct_per_kwh: d("3.5"),
            }),
            leistungspreis: None,
            letztverbrauchergruppe: Default::default(),
            enfg_jahresvorverbrauch_kwh: None,
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
        i.arbeitspreis = ArbeitspreisModell::SpotpreisNetzentgelt {
            intervalle: vec![SpotpreisInterval {
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
        i.arbeitspreis = ArbeitspreisModell::SpotpreisNetzentgelt {
            intervalle: vec![
                SpotpreisInterval {
                    period_from: base,
                    period_to: base + time::Duration::minutes(15),
                    menge_kwh: d("1.25"),
                    nne_rate_ct_per_kwh: d("2.00"),
                    epex_spot_ct_per_kwh: None,
                },
                SpotpreisInterval {
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
        i.arbeitspreis = ArbeitspreisModell::SpotpreisNetzentgelt {
            intervalle: vec![
                SpotpreisInterval {
                    period_from: base,
                    period_to: base + time::Duration::minutes(15),
                    menge_kwh: d("0"),
                    nne_rate_ct_per_kwh: d("2.00"),
                    epex_spot_ct_per_kwh: None,
                },
                SpotpreisInterval {
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
        i.arbeitspreis = modul1(d("120.00"));
        assert_eq!(i.arbeitspreis.sect14a_modul(), Some(Sect14aModule::Modul1));

        // A spot-linked Netzentgelt replaces Modul 1 rather than adding to it —
        // and is not itself one of the three modules BK6-22-300 defines.
        i.arbeitspreis = ArbeitspreisModell::SpotpreisNetzentgelt {
            intervalle: vec![SpotpreisInterval {
                period_from: base,
                period_to: base + time::Duration::minutes(15),
                menge_kwh: d("1.0"),
                nne_rate_ct_per_kwh: d("2.0"),
                epex_spot_ct_per_kwh: None,
            }],
        };
        assert_eq!(
            i.arbeitspreis.sect14a_modul(),
            None,
            "a spot-linked Netzentgelt is the NB's own price model, not §14a Modul 3"
        );

        // And the settlement bills the interval once, not the flat rate as well.
        let r = settle_nne(&i).expect("the spot model settles");
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

    // ── §19 Abs. 2 StromNEV — the reduction basis ────────────────────────────

    /// The §19 Abs. 2 reduction must cover the §14a Modul 3 Spotpreis positions.
    ///
    /// They are emitted per dispatch interval and must be in the basis before
    /// the §19 block runs. Pushed after it, a Modul-3 customer with a 10 %
    /// agreement has a basis of zero and is billed the published Netzentgelt in
    /// full.
    #[test]
    fn sect19_reduction_covers_the_modul3_spot_positions() {
        use time::macros::datetime;
        let interval = |kwh: &str, ct: &str, hour: u8| SpotpreisInterval {
            period_from: datetime!(2025-01-01 00:00 UTC) + time::Duration::hours(hour as i64),
            period_to: datetime!(2025-01-01 00:15 UTC) + time::Duration::hours(hour as i64),
            menge_kwh: d(kwh),
            nne_rate_ct_per_kwh: d(ct),
            epex_spot_ct_per_kwh: None,
        };
        let out = settle_nne(&NneInput {
            arbeitspreis: ArbeitspreisModell::SpotpreisNetzentgelt {
                intervalle: vec![interval("400", "5.0", 1), interval("600", "5.0", 2)],
            },
            jahresarbeit_kwh: Some(d("70000000")),
            jahreshoechstleistung_kw: Some(d("8000")),
            sect19: Some(crate::sect19::Sect19Vereinbarung {
                art: crate::sect19::Sect19Art::IntensiveNetznutzung,
                vereinbarter_prozentsatz: d("0.10"),
                genehmigung: Some("BK4-24-001".to_owned()),
            }),
            ..base_nne()
        })
        .expect("a settleable NNE");

        let modul3: Decimal = out
            .positions
            .iter()
            .filter(|p| p.kind == BillingPositionKind::NneArbeitModul3)
            .map(|p| p.net_eur)
            .sum();
        assert_eq!(modul3, d("50.00000"), "1 000 kWh × 5 ct");

        let reduktion = out
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::Sect19IndividuellesEntgelt)
            .expect("a §19 reduction position");
        // 10 % agreed → 90 % of the Modul-3 NNE is credited back.
        assert_eq!(reduktion.net_eur, d("-45.00000"));
    }

    /// The basis covers every NNE kind the settlement can emit — including the
    /// Modul 3 ST band and the Modul 2 reduced Arbeitspreis, which a filter
    /// keyed on the plain Arbeitspreis alone would omit.
    #[test]
    fn sect19_reduction_covers_the_modul3_time_of_use_bands() {
        let mp = |kwh: &str| MengePreis {
            menge_kwh: d(kwh),
            preis_ct_per_kwh: d("10.0"),
        };
        let out = settle_nne(&NneInput {
            arbeitspreis: ArbeitspreisModell::Modul3ZeitVariabel {
                ht: mp("100"),
                st: mp("200"),
                nt: mp("300"),
            },
            sect19: Some(crate::sect19::Sect19Vereinbarung {
                art: crate::sect19::Sect19Art::AtypischeNetznutzung,
                vereinbarter_prozentsatz: d("0.20"),
                genehmigung: None,
            }),
            ..base_nne()
        })
        .expect("a settleable NNE");

        let reduktion = out
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::Sect19IndividuellesEntgelt)
            .expect("a §19 reduction position");
        // HT+ST+NT = 600 kWh × 10 ct = 60 EUR; 80 % is credited back.
        assert_eq!(reduktion.net_eur, d("-48.00000"));
    }

    // ── §15 GasNEV — Kapazitätsentgelt pro-ration ────────────────────────────

    /// A full year of capacity costs exactly the annual Entgelt, leap year or not.
    ///
    /// §15 GasNEV fixes no day-count convention; a hard-coded 365 divisor billed
    /// a leap year at 366/365 = 100.274 % of the price sheet figure.
    #[test]
    fn gas_kapazitaetsentgelt_a_full_year_costs_the_annual_entgelt() {
        use crate::gas::{GasKapazitaet, Kapazitaetsprodukt};
        let jahr = |from, to| {
            settle_nne(&NneInput {
                sparte: Sparte::Gas,
                period: SettlementPeriod::new(from, to).unwrap(),
                gas_kapazitaet: Some(GasKapazitaet {
                    bestellte_kapazitaet_kwh_h: d("100"),
                    entgelt_eur_per_kwh_h_a: d("36.5"),
                    produkt: Kapazitaetsprodukt::Fest,
                    druckstufe: None,
                }),
                ..base_nne()
            })
            .expect("a settleable NNE")
            .positions
            .iter()
            .find(|p| p.kind == BillingPositionKind::GasKapazitaetsentgelt)
            .expect("a Kapazitätsentgelt position")
            .net_eur
        };
        // 2024 is a leap year (366 days), 2025 is not (365).
        let leap = jahr(date!(2024 - 01 - 01), date!(2024 - 12 - 31));
        let common = jahr(date!(2025 - 01 - 01), date!(2025 - 12 - 31));
        assert_eq!(common, d("3650.00000"), "100 kWh/h × 36.50 EUR/a");
        assert_eq!(leap, common, "a leap year is not 0.274 % more capacity");
    }

    // ── Sparte guards ────────────────────────────────────────────────────────

    /// A Grundpreis on a Strom settlement is not billed as a GasNEV §14 position.
    #[test]
    fn a_grundpreis_on_strom_is_refused_not_labelled_gas() {
        let out = settle_nne(&NneInput {
            sparte: Sparte::Strom,
            grundpreis: Some(crate::types::Grundpreis {
                eur_per_month: d("12.00"),
                months: Decimal::ONE,
            }),
            ..base_nne()
        })
        .expect("a settleable NNE");

        assert!(
            !out.positions
                .iter()
                .any(|p| p.kind == BillingPositionKind::NneGasGrundpreis),
            "no Gas Grundpreis position on a Strom invoice"
        );
        assert!(
            out.warnings.iter().any(|w| w.code == "GRUNDPREIS_ON_STROM"),
            "the refusal must be visible to the caller"
        );
    }

    /// …and on Gas it is billed, unchanged.
    #[test]
    fn a_grundpreis_on_gas_is_billed() {
        let out = settle_nne(&NneInput {
            sparte: Sparte::Gas,
            grundpreis: Some(crate::types::Grundpreis {
                eur_per_month: d("12.00"),
                months: Decimal::ONE,
            }),
            ..base_nne()
        })
        .expect("a settleable NNE");
        assert_eq!(
            out.positions
                .iter()
                .find(|p| p.kind == BillingPositionKind::NneGasGrundpreis)
                .map(|p| p.net_eur),
            Some(d("12.00000"))
        );
    }
}
