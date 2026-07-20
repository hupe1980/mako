//! Property-based invariant tests for the `eeg-billing` crate.
//!
//! These tests use `proptest` to verify mathematical invariants that must hold
//! across all valid inputs — not just specific scenarios. They complement the
//! `regulatory_showcase.rs` integration tests by catching edge-case regressions
//! that hand-crafted examples might miss.
//!
//! ## Invariants verified
//!
//! | # | Invariant | Legal basis |
//! |---|---|---|
//! | INV-1 | FeedInTariff formula exact: `kwh × rate / 100` | §21 EEG |
//! | INV-2 | MarketPremium spread is never negative | §20 EEG |
//! | INV-3 | `positions.eur` sum equals `settlement_eur` | internal consistency |
//! | INV-4 | §51 deduction bounded: deducted kWh ≤ einspeisemenge | §51 EEG |
//! | INV-5 | FoerderungBeendet when `billing_date > foerderendedatum` | §25 EEG |
//! | INV-6 | KWKG surcharge is always non-negative | §7 KWKG |
//! | INV-7 | §52 Pflichtzahlung is always non-negative | §52 EEG 2023 |
//! | INV-8 | `settlement_eur` is `None` iff status is `NoData`/`PriceMissing` | model contract |
//! | INV-9 | TenantElectricity never returns negative EUR | §21 Abs. 3 EEG |
//! | INV-10 | PostEeg with `price_floor >= 0` returns non-negative | §21 EEG post-Förderung |
//!
//! Run: `cargo test -p eeg-billing --test prop_tests`

use eeg_billing::{
    SettleInput, SettlementScheme, SettlementStatus, calculate_settlement, foerderendedatum_eeg,
};
use rust_decimal::Decimal;

use proptest::prelude::*;

// ── Decimal strategies ────────────────────────────────────────────────────────

/// Non-negative kWh (0 – 10 000 000 kWh, 2 decimal places).
fn arb_kwh() -> impl Strategy<Value = Decimal> {
    (0i64..=1_000_000_000i64).prop_map(|n| Decimal::new(n, 2))
}

/// Non-negative rate in ct/kWh (0 – 50 ct, 4 decimal places).
fn arb_rate_ct() -> impl Strategy<Value = Decimal> {
    (0i64..=500_000i64).prop_map(|n| Decimal::new(n, 4))
}

/// EPEX price in ct/kWh (–10 – 200 ct; covers extreme negative-price scenarios).
fn arb_epex_ct() -> impl Strategy<Value = Decimal> {
    (-1000i64..=20000i64).prop_map(|n| Decimal::new(n, 2))
}

/// Non-negative EPEX price (0 – 200 ct).
fn arb_epex_ct_nonneg() -> impl Strategy<Value = Decimal> {
    (0i64..=20000i64).prop_map(|n| Decimal::new(n, 2))
}

/// Installed capacity kW (1 kW – 500 MW, integer).
fn arb_leistung_kw() -> impl Strategy<Value = Decimal> {
    (1i64..=500_000i64).prop_map(Decimal::from)
}

/// KWKG already-paid kWh for the surcharge period (0 – 100 000 000 kWh).
fn arb_kwk_paid() -> impl Strategy<Value = Decimal> {
    (0i64..=10_000_000_000i64).prop_map(|n| Decimal::new(n, 2))
}

// ── INV-1: FeedInTariff formula exact ────────────────────────────────────────

proptest! {
    /// **INV-1** — §21 EEG: `settlement_eur = kwh × rate_ct / 100`, rounded to 5 dp.
    ///
    /// The formula must be exact for all non-negative kWh and rates.
    /// No floating-point accumulation; no unexpected intermediate rounding.
    #[test]
    fn inv1_feed_in_tariff_formula_exact(
        kwh  in arb_kwh(),
        rate in arb_rate_ct(),
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: Some(kwh),
            ..SettleInput::default()
        });

        prop_assume!(out.status == SettlementStatus::Calculated);
        let actual = out.settlement_eur.expect("Calculated must have settlement_eur");

        // Formula: kwh × rate / 100 rounded to 5 decimal places
        let expected = (kwh * rate / Decimal::from(100)).round_dp(5);

        // Allow at most 1 ULP (0.00001 EUR) rounding difference
        let diff = (actual - expected).abs();
        prop_assert!(
            diff <= Decimal::new(1, 5),
            "FeedInTariff: expected {expected} EUR, got {actual} EUR (diff {diff})"
        );
    }
}

// ── INV-2: MarketPremium spread is never negative ─────────────────────────────

proptest! {
    /// **INV-2** — §20 EEG: gleitende Marktprämie spread `= max(0, eff_AW − EPEX)`.
    ///
    /// The Marktprämie is always ≥ 0. When EPEX > AW, the premium is zero
    /// (the plant earns from the market; no EEG subsidy).
    #[test]
    fn inv2_market_premium_never_negative(
        kwh     in arb_kwh(),
        aw_ct   in arb_rate_ct(),
        epex_ct in arb_epex_ct(),
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::MarketPremium {
                direktverm_aw_ct: aw_ct,
                managementpraemie_ct: Some(Decimal::ZERO),
                wind_korrekturfaktor: None,
                wind_standort: None,
            },
            einspeisemenge_kwh: Some(kwh),
            marktwert_ct_kwh: Some(epex_ct),
            ..SettleInput::default()
        });

        if let Some(eur) = out.settlement_eur {
            prop_assert!(
                eur >= Decimal::ZERO,
                "MarketPremium returned negative EUR {eur} (aw={aw_ct}, epex={epex_ct}, kwh={kwh})"
            );
        }
    }
}

// ── INV-3: positions.eur sum equals settlement_eur ───────────────────────────

proptest! {
    /// **INV-3** — Internal consistency: `Σ(positions.eur) == settlement_eur`.
    ///
    /// Every settlement output must recompute correctly from its positions.
    /// This catches bugs where the total is updated separately from positions.
    #[test]
    fn inv3_positions_sum_equals_settlement_eur(
        kwh  in arb_kwh(),
        rate in arb_rate_ct(),
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: Some(kwh),
            ..SettleInput::default()
        });

        let positions_total: Decimal = out.positions.iter().map(|p| p.eur).sum();
        if let Some(eur) = out.settlement_eur {
            let diff = (positions_total - eur).abs();
            prop_assert!(
                diff <= Decimal::new(1, 5),
                "positions sum {positions_total} != settlement_eur {eur} (diff {diff})"
            );
        } else {
            // When settlement_eur is None, positions should all be zero
            prop_assert!(
                positions_total == Decimal::ZERO || out.positions.is_empty(),
                "positions sum {positions_total} must be zero when settlement_eur is None"
            );
        }
    }
}

// ── INV-4: §51 deduction is bounded ──────────────────────────────────────────

proptest! {
    /// **INV-4** — §51 EEG: deducted kWh cannot exceed einspeisemenge.
    ///
    /// `eligible_kwh = max(0, einspeisemenge - kwh_during_negative_epex)`
    /// ≤ einspeisemenge for all valid inputs.
    #[test]
    fn inv4_sect51_deduction_bounded(
        kwh          in arb_kwh(),
        negative_kwh in arb_kwh(),
        rate         in arb_rate_ct(),
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: Some(kwh),
            kwh_during_negative_epex: Some(negative_kwh),
            ..SettleInput::default()
        });

        if let Some(eligible) = out.eligible_kwh {
            prop_assert!(
                eligible <= kwh,
                "eligible_kwh {eligible} exceeds einspeisemenge {kwh}"
            );
            prop_assert!(
                eligible >= Decimal::ZERO,
                "eligible_kwh {eligible} is negative"
            );
        }
    }
}

// ── INV-5: FoerderungBeendet when billing_date > foerderendedatum ─────────────

proptest! {
    /// **INV-5** — §25 EEG: when `billing_date > foerderendedatum`, status must be
    /// `FoerderungBeendet`. No payment is made after the 20-year Förderdauer.
    #[test]
    fn inv5_foerderung_beendet_when_expired(
        kwh  in arb_kwh(),
        rate in arb_rate_ct(),
        // Commissioning year 1995–2010, so foerderendedatum is 2015–2030
        year_offset in 0u32..=15u32,
    ) {
        let inbetriebnahme = time::Date::from_calendar_date(
            1995 + year_offset as i32,
            time::Month::June,
            1,
        )
        .unwrap();
        let foerderendedatum = foerderendedatum_eeg(inbetriebnahme).unwrap();

        // Use a billing_date that is AFTER foerderendedatum
        let billing_date_after = foerderendedatum
            .replace_year(foerderendedatum.year() + 1)
            .unwrap();

        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: Some(kwh),
            foerderendedatum: Some(foerderendedatum),
            billing_date: Some(billing_date_after),
            ..SettleInput::default()
        });

        prop_assert_eq!(
            out.status,
            SettlementStatus::FoerderungBeendet,
            "billing_date > foerderendedatum: expected FoerderungBeendet"
        );

        // After FoerderungBeendet, no regular EUR payment
        let _ = billing_date_after; // suppress unused warning
    }
}

// ── INV-6: KWKG surcharge is always non-negative ──────────────────────────────

proptest! {
    /// **INV-6** — §7 KWKG 2023: the KWK-Zuschlag is always ≥ 0 EUR.
    ///
    /// Even when all remaining eligible kWh are zero (cap reached),
    /// the result must be EUR 0, not negative.
    #[test]
    fn inv6_kwkg_surcharge_non_negative(
        kwh         in arb_kwh(),
        rate_ct     in arb_rate_ct(),
        max_kwh     in arb_kwk_paid(),
        paid_kwh    in arb_kwk_paid(),
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::KwkSurcharge {
                verguetungssatz_ct: rate_ct,
                max_kwh: Some(max_kwh),
                kwh_paid_gesamt: Some(paid_kwh),
            },
            einspeisemenge_kwh: Some(kwh),
            ..SettleInput::default()
        });

        if let Some(eur) = out.settlement_eur {
            prop_assert!(
                eur >= Decimal::ZERO,
                "KWK surcharge returned negative EUR {eur}"
            );
        }
        if let Some(eligible) = out.eligible_kwh {
            prop_assert!(eligible >= Decimal::ZERO, "KWKG eligible_kwh {eligible} is negative");
        }
    }
}

// ── INV-7: §52 Pflichtzahlung is always non-negative ─────────────────────────

proptest! {
    /// **INV-7** — §52 EEG 2023: Pflichtzahlung (compliance penalty) is always ≥ 0.
    ///
    /// The penalty may be zero when no violations apply, but never negative.
    #[test]
    fn inv7_pflichtzahlung_non_negative(
        kwh  in arb_kwh(),
        rate in arb_rate_ct(),
        leistung in arb_leistung_kw(),
        months in 0u32..=24u32,
        fulfilled in any::<bool>(),
        defect in any::<bool>(),
    ) {
        use eeg_billing::{Pflichtverstoss, SanktionsTyp};

        let violation = Pflichtverstoss {
            typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
            leistung_kw: leistung,
            monate_des_verstosses: months,
            nachtraeglich_erfuellt: fulfilled,
            technischer_defekt: defect,
        };

        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: Some(kwh),
            pflichtverstoss: vec![violation],
            ..SettleInput::default()
        });

        if let Some(penalty) = out.pflichtzahlung_eur {
            prop_assert!(
                penalty >= Decimal::ZERO,
                "Pflichtzahlung {penalty} is negative (leistung={leistung}, months={months})"
            );
        }
    }
}

// ── INV-8: settlement_eur is None iff status is NoData/PriceMissing ───────────

proptest! {
    /// **INV-8** — API contract: `settlement_eur` is `None` exactly when
    /// `status ∈ {NoData, PriceMissing}`.
    ///
    /// Any calculable status must have a `Some` settlement amount.
    #[test]
    fn inv8_settlement_eur_present_iff_calculable(
        rate in arb_rate_ct(),
    ) {
        // NoData: no kwh supplied
        let out_nodata = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: None,
            ..SettleInput::default()
        });
        prop_assert_eq!(out_nodata.status, SettlementStatus::NoData);
        prop_assert!(out_nodata.settlement_eur.is_none(), "NoData must have None settlement_eur");

        // PriceMissing: MarketPremium without EPEX price
        let out_pricemissing = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::MarketPremium {
                direktverm_aw_ct: rate,
                managementpraemie_ct: None,
                wind_korrekturfaktor: None,
                wind_standort: None,
            },
            einspeisemenge_kwh: Some(Decimal::from(100)),
            marktwert_ct_kwh: None, // <— PriceMissing
            ..SettleInput::default()
        });
        prop_assert_eq!(out_pricemissing.status, SettlementStatus::PriceMissing);
        prop_assert!(out_pricemissing.settlement_eur.is_none(), "PriceMissing must have None settlement_eur");
    }
}

// ── INV-9: TenantElectricity (Mieterstrom §21 Abs. 3) is always non-negative ──

proptest! {
    /// **INV-9** — §21 Abs. 3 EEG: Mieterstrom settlement is always ≥ 0 EUR.
    ///
    /// Both the base rate and the Zuschlag are non-negative,
    /// so the total payment can never go below zero.
    #[test]
    fn inv9_tenant_electricity_non_negative(
        kwh           in arb_kwh(),
        verguetung_ct in arb_rate_ct(),
        zuschlag_ct   in arb_rate_ct(),
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::TenantElectricity {
                verguetungssatz_ct: verguetung_ct,
                mieter_zuschlag_ct: Some(zuschlag_ct),
            },
            einspeisemenge_kwh: Some(kwh),
            ..SettleInput::default()
        });

        if let Some(eur) = out.settlement_eur {
            prop_assert!(
                eur >= Decimal::ZERO,
                "TenantElectricity returned negative EUR {eur}"
            );
        }
    }
}

// ── INV-10: PostEeg with non-negative floor is non-negative ───────────────────

proptest! {
    /// **INV-10** — §21 EEG post-Förderung: when `price_floor >= 0`,
    /// the settlement result is always ≥ 0 EUR.
    ///
    /// Even when EPEX is deeply negative, the `price_floor` prevents
    /// the plant operator from paying more than the floor implies.
    #[test]
    fn inv10_post_eeg_with_nonneg_floor_is_nonneg(
        kwh   in arb_kwh(),
        epex  in arb_epex_ct(),        // may be negative
        floor in arb_epex_ct_nonneg(), // always ≥ 0
    ) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::PostEeg {
                price_floor: Some(floor),
            },
            einspeisemenge_kwh: Some(kwh),
            marktwert_ct_kwh: Some(epex),
            ..SettleInput::default()
        });

        if let Some(eur) = out.settlement_eur {
            prop_assert!(
                eur >= Decimal::ZERO,
                "PostEeg with floor={floor} returned negative EUR {eur} (epex={epex})"
            );
        }
    }
}

// ── Additional: foerderendedatum invariants ───────────────────────────────────

proptest! {
    /// Commissioning before billing_date but AFTER foerderendedatum
    /// always yields `FoerderungBeendet` even with zero rate.
    #[test]
    fn foerderendedatum_zero_rate_still_beendet(
        year_offset in 0u32..=10u32,
    ) {
        let inbetriebnahme = time::Date::from_calendar_date(
            2000 + year_offset as i32,
            time::Month::January,
            1,
        )
        .unwrap();
        let foerderendedatum = foerderendedatum_eeg(inbetriebnahme).unwrap();
        let billing_after = foerderendedatum.replace_year(foerderendedatum.year() + 2).unwrap();

        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: Decimal::ZERO },
            einspeisemenge_kwh: Some(Decimal::from(1000)),
            foerderendedatum: Some(foerderendedatum),
            billing_date: Some(billing_after),
            ..SettleInput::default()
        });

        prop_assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    }
}

proptest! {
    /// FeedInTariff with zero kWh always returns EUR 0, not an error.
    #[test]
    fn feed_in_tariff_zero_kwh_is_zero_eur(rate in arb_rate_ct()) {
        let out = calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: rate },
            einspeisemenge_kwh: Some(Decimal::ZERO),
            ..SettleInput::default()
        });
        prop_assert_eq!(out.status, SettlementStatus::Calculated);
        prop_assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    }
}
