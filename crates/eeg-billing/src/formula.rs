//! Pure settlement formula — [`calculate_settlement`].

use crate::EuroAmount;
use rust_decimal::Decimal;
use rust_decimal::dec;

use crate::model::{SettleInput, SettleOutput, SettlePosition, SettlementStatus};
use crate::scheme::SettlementScheme;

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Round a settlement amount to 5 decimal places via [`EuroAmount`].
///
/// # Panics
///
/// Panics when the amount exceeds the `EuroAmount` range (≈ 9.2 × 10¹³ EUR).
/// This is the same contract as the `Decimal` arithmetic that produces the
/// amount — it panics on overflow one step earlier. No physical EEG
/// settlement can reach this range; reaching it means the input data is
/// corrupt, and a silently altered amount would be worse than the panic.
fn validated_eur(d: Decimal) -> Decimal {
    // `checked_from_decimal` is exact — it errors on excess precision rather than
    // rounding. This helper's job is to *round* the high-precision settlement
    // product down to the 5-dp money resolution, so it uses the explicit
    // `from_decimal_rounded`; the remaining error arm is a true range overflow,
    // which no physical EEG settlement can reach.
    EuroAmount::from_decimal_rounded(d, billing::RoundingStrategy::MidpointAwayFromZero)
        .map(crate::EuroAmount::into_decimal)
        .unwrap_or_else(|_| panic!("settlement amount {d} EUR exceeds the EuroAmount range"))
}

/// Build a single [`SettlePosition`] from its components.
fn pos(
    description: impl Into<String>,
    legal_basis: impl Into<String>,
    kwh: Decimal,
    rate_ct_kwh: Decimal,
) -> SettlePosition {
    let eur = validated_eur(kwh * rate_ct_kwh / Decimal::from(100));
    SettlePosition {
        description: description.into(),
        legal_basis: legal_basis.into(),
        kwh,
        rate_ct_kwh,
        eur,
    }
}

/// Sum `positions` to a total, returning `None` when the slice is empty.
fn total(positions: &[SettlePosition]) -> Option<Decimal> {
    if positions.is_empty() {
        None
    } else {
        Some(positions.iter().map(|p| p.eur).sum())
    }
}

/// Whether §51 actually bites for this plant in this period.
///
/// Two independent gates: something has to have been fed in during a qualifying
/// negative-price period, and the plant must not be exempt on size / technology
/// grounds. The **run-length** gate (4-3-2-1 h, 6 h, or the first quarter-hour)
/// is not re-checked here — it is a property of the interval series and is
/// applied by [`crate::negativpreis::derive_negativpreis`], which is what
/// produced `kwh_during_negative_epex` in the first place.
///
/// `leistung_kwp` must be the **aggregated** plant capacity: §51 Abs. 2 Satz 2
/// applies §24 to the size test, so §24-linked capacity blocks count as one
/// plant. Passing a single block's capacity would let a 180 kWp plant split into
/// three 60 kWp blocks escape §51 entirely.
fn sect51_greift(
    kwh_during_negative_epex: Option<Decimal>,
    leistung_kwp: Option<Decimal>,
    regime: crate::negativpreis::NegativpreisRegime,
    erzeugungsart: Option<crate::technology::ErzeugungsArt>,
    has_imesys: bool,
    ist_pilotwindanlage: bool,
) -> bool {
    if kwh_during_negative_epex.is_none_or(|k| k <= Decimal::ZERO) {
        return false;
    }
    !regime.ist_befreit(leistung_kwp, erzeugungsart, has_imesys, ist_pilotwindanlage)
}

/// Apply §51 EEG deduction: subtract kWh during negative-price hours.
/// Only called after `should_apply_negativpreis` returns `true`.
fn apply_negativpreis(kwh: Decimal, negative_kwh: Decimal) -> Decimal {
    (kwh - negative_kwh).max(Decimal::ZERO)
}

/// The plant capacity the size-dependent rules test against.
///
/// §51 Abs. 2 Satz 2 EEG 2023 applies §24 to the §51 size test, so §24-linked
/// capacity blocks are one plant. Returns `None` only when nothing is known.
fn aggregierte_leistung_kwp(input: &SettleInput) -> Option<Decimal> {
    if input.capacity_blocks.is_empty() {
        return input.leistung_kwp;
    }
    let blocks: Decimal = input.capacity_blocks.iter().map(|b| b.leistung_kwp).sum();
    Some(input.leistung_kwp.unwrap_or(Decimal::ZERO) + blocks)
}

/// Resolve the effective §36h wind onshore Korrekturfaktor.
///
/// Priority:
/// 1. explicit `wind_korrekturfaktor` override (always wins)
/// 2. `wind_standort.korrekturfaktor` (struct-based)
/// 3. `None` — no correction applied
fn resolve_wind_korrekturfaktor(
    wind_korrekturfaktor: Option<Decimal>,
    wind_standort: Option<&crate::wind::WindStandort>,
) -> Option<Decimal> {
    wind_korrekturfaktor.or_else(|| wind_standort.map(|ws| ws.korrekturfaktor))
}

// ── Multi-block settlement (§24 EEG Anlagenerweiterung) ──────────────────────

/// Settle a primary block (from `SettleInput`) plus additional `capacity_blocks`.
///
/// Called when `!input.capacity_blocks.is_empty()`.
fn calculate_with_capacity_blocks(input: &SettleInput, total_kwh: Decimal) -> SettleOutput {
    let billing_date = input.billing_date;

    // Collect all blocks: primary (from SettleInput) + additional blocks
    // leistung_kwp MUST be set when capacity_blocks are non-empty; return NoData if missing.
    let Some(primary_kwp) = input.leistung_kwp.filter(|kw| *kw > Decimal::ZERO) else {
        // §24 configuration error: cannot allocate proportionally without leistung_kwp.
        return SettleOutput {
            settlement_eur: None,
            eligible_kwh: None,
            positions: vec![crate::model::SettlePosition {
                description: "§24 Konfigurationsfehler: leistung_kwp fehlt oder ist null"
                    .to_owned(),
                legal_basis: "§24 EEG 2023".to_owned(),
                kwh: Decimal::ZERO,
                rate_ct_kwh: Decimal::ZERO,
                eur: Decimal::ZERO,
            }],
            status: SettlementStatus::NoData,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        };
    };
    let additional_total_kwp: Decimal = input.capacity_blocks.iter().map(|b| b.leistung_kwp).sum();
    let total_kwp = primary_kwp + additional_total_kwp;

    // §21 EEG needs a rate per block. `verguetungssatz_ct()` is `None` for the
    // market- and capacity-based schemes, and settling those at zero would report
    // a €0 claim as `Calculated` — a §24-extended Direktvermarktung plant would
    // silently lose its whole Marktprämie.
    let Some(primary_rate) = input.scheme.verguetungssatz_ct() else {
        return SettleOutput {
            settlement_eur: None,
            eligible_kwh: None,
            positions: vec![crate::model::SettlePosition {
                description: "§24 Anlagenerweiterung: das Abrechnungsmodell führt keinen \
                     Vergütungssatz je Block (Marktprämie/Post-EEG) — kein Preis ermittelbar"
                    .to_owned(),
                legal_basis: "§24 EEG 2023".to_owned(),
                kwh: Decimal::ZERO,
                rate_ct_kwh: Decimal::ZERO,
                eur: Decimal::ZERO,
            }],
            status: SettlementStatus::PriceMissing,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        };
    };

    let mut positions: Vec<SettlePosition> = Vec::new();
    let mut total_eligible = Decimal::ZERO;

    // ── Primary block ────────────────────────────────────────────────────────
    let primary_expired =
        billing_date.is_some_and(|d| input.foerderendedatum.is_some_and(|fed| d > fed));
    if !primary_expired {
        let share = if total_kwp.is_zero() {
            Decimal::ONE
        } else {
            (primary_kwp / total_kwp).round_dp(6)
        };
        let mut block_kwh = (total_kwh * share).round_dp(3);

        // §51 Negativpreisregel — the size test runs on the aggregated plant
        // (§51 Abs. 2 Satz 2 i.V.m. §24), the deduction on this block's share.
        if sect51_greift(
            input.kwh_during_negative_epex,
            Some(total_kwp),
            input.negativpreis_regime(),
            input.erzeugungsart,
            input.has_imesys,
            input.ist_pilotwindanlage,
        ) {
            // Proportional share of negative kWh for this block
            let neg_share = input
                .kwh_during_negative_epex
                .map(|n| (n * share).round_dp(3))
                .unwrap_or(Decimal::ZERO);
            block_kwh = apply_negativpreis(block_kwh, neg_share);
        }

        if block_kwh > Decimal::ZERO || primary_rate != Decimal::ZERO {
            let ibn_label = input
                .inbetriebnahme
                .map(|d| format!(" (IBN {d})"))
                .unwrap_or_default();
            positions.push(pos(
                format!("Einspeiseverg\u{00fc}tung {primary_kwp}\u{202f}kWp-Block{ibn_label}"),
                "\u{00a7}21 EEG",
                block_kwh,
                primary_rate,
            ));
        }
        total_eligible += block_kwh;
    }

    // ── Additional blocks ────────────────────────────────────────────────────
    for (idx, block) in input.capacity_blocks.iter().enumerate() {
        let block_expired = billing_date.is_some_and(|d| d > block.foerderendedatum);
        if block_expired {
            continue;
        }
        let share = if total_kwp.is_zero() {
            Decimal::ZERO
        } else {
            (block.leistung_kwp / total_kwp).round_dp(6)
        };
        let mut block_kwh = (total_kwh * share).round_dp(3);

        // §51 per-block: each block carries its own commissioning date, and the
        // §51 regime is keyed on that date — a block added after the
        // Solarspitzengesetz is governed by it even when the primary block is not.
        if sect51_greift(
            input.kwh_during_negative_epex,
            Some(total_kwp),
            crate::negativpreis::NegativpreisRegime::fuer_inbetriebnahme(block.inbetriebnahme),
            input.erzeugungsart,
            input.has_imesys,
            input.ist_pilotwindanlage,
        ) {
            let neg_share = input
                .kwh_during_negative_epex
                .map(|n| (n * share).round_dp(3))
                .unwrap_or(Decimal::ZERO);
            block_kwh = apply_negativpreis(block_kwh, neg_share);
        }

        let block_num = idx + 1;
        if block_kwh > Decimal::ZERO || block.verguetungssatz_ct != Decimal::ZERO {
            positions.push(pos(
                format!(
                    "Einspeiseverg\u{00fc}tung {}\u{202f}kWp-Block\u{202f}{} (IBN {})",
                    block.leistung_kwp, block_num, block.inbetriebnahme
                ),
                "\u{00a7}21 EEG",
                block_kwh,
                block.verguetungssatz_ct,
            ));
        }
        total_eligible += block_kwh;
    }

    let is_empty = positions.is_empty();
    let settlement_eur = total(&positions);
    SettleOutput {
        settlement_eur,
        eligible_kwh: Some(total_eligible),
        positions,
        status: if is_empty {
            SettlementStatus::FoerderungBeendet
        } else {
            SettlementStatus::Calculated
        },
        pflichtzahlung_eur: None,
        pflichtzahlung_faelligkeitsdatum: None,
        verlaengerungsanspruch_qh: 0,
        dezentrale_einspeisung_anspruch_verloren: false,
        billing_days_fraction_applied: None,
        faelligkeitsdatum: None,
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Calculate the settlement amount and billing positions for one period.
///
/// This function is **pure** — it performs no I/O and has no side effects.
/// All input rates are in `ct/kWh`; output amounts are in EUR.
///
/// ## Multi-EEG-version support
///
/// Supply `inbetriebnahme` and `leistung_kwp` for automatic version-aware
/// rule enforcement (§51 EEG Negativpreisregel, §8 KWKG Förderdauer limits).
/// The correct `verguetungssatz_ct` must be supplied by the caller (use
/// `eeg_billing::rates` or `einsd`'s rate lookup table).
///
/// ## Förderdauer auto-detection
///
/// Supply `billing_date` and `foerderendedatum` to enable automatic
/// `FoerderungBeendet` status when the billing period starts after the
/// subsidy end date. Without these, the caller must check expiry manually.
///
/// # Examples
///
/// ```rust
/// use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement, SettlementStatus};
/// use rust_decimal::Decimal;
/// use std::str::FromStr;
///
/// fn d(s: &str) -> Decimal { Decimal::from_str(s).unwrap() }
///
/// // §21 EEG 2023 — 100 kWh × 8.11 ct/kWh = 8.11 EUR
/// let out = calculate_settlement(&SettleInput {
///     scheme: eeg_billing::SettlementScheme::FeedInTariff { verguetungssatz_ct: d("8.11") },
///     einspeisemenge_kwh: Some(d("100")),
///     ..SettleInput::default()
/// });
/// assert_eq!(out.status, SettlementStatus::Calculated);
/// assert_eq!(out.settlement_eur, Some(d("8.11")));
/// ```
pub fn calculate_settlement(input: &SettleInput) -> SettleOutput {
    // ── §42a EEG 2023 — Holzbiomasse restriction from 2026-01-01 ─────────────
    // §42a EEG 2023: new Holzbiomasse plants commissioned from 01.01.2026 may
    // not use fresh wood for primary energy production and lose EEG eligibility.
    // Plants commissioned before that date retain Bestandsschutz.
    if input
        .erzeugungsart
        .is_some_and(|a| a == crate::technology::ErzeugungsArt::BiomassHolz)
        && input
            .inbetriebnahme
            .is_some_and(|d| d >= time::macros::date!(2026 - 01 - 01))
    {
        return SettleOutput {
            settlement_eur: Some(Decimal::ZERO),
            eligible_kwh: input.einspeisemenge_kwh,
            positions: vec![crate::model::SettlePosition {
                description: "§42a EEG 2023: Holzbiomasse-Anlage ab 2026-01-01 nicht förderfähig"
                    .to_owned(),
                legal_basis: "§42a EEG 2023".to_owned(),
                kwh: input.einspeisemenge_kwh.unwrap_or(Decimal::ZERO),
                rate_ct_kwh: Decimal::ZERO,
                eur: Decimal::ZERO,
            }],
            status: SettlementStatus::Sanctioned,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        };
    }

    // ── §43 Abs. 1 Nr. 2 EEG 2023 — Biomass substrate cap ────────────────────
    // Plants with >40 % Energiepflanzen vom Acker in the energy input lose EEG
    // support for the billing period (substrate_cap_ok = false).
    if let Some(biomasse) = &input.biomasse
        && !biomasse.substrate_cap_ok
    {
        return SettleOutput {
            settlement_eur: Some(Decimal::ZERO),
            eligible_kwh: input.einspeisemenge_kwh,
            positions: vec![crate::model::SettlePosition {
                description: "§43 Abs. 1 Nr. 2 EEG 2023: Substratdeckel überschritten — \
                     Energiepflanzen-Anteil > 40 %"
                    .to_owned(),
                legal_basis: "§43 Abs. 1 Nr. 2 EEG 2023".to_owned(),
                kwh: input.einspeisemenge_kwh.unwrap_or(Decimal::ZERO),
                rate_ct_kwh: Decimal::ZERO,
                eur: Decimal::ZERO,
            }],
            status: SettlementStatus::Sanctioned,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        };
    }

    // ── §52 EEG 2023 Pflichtzahlungen (multiple violations, §52 Abs. 5 cap) ────
    // All violations are summed. The §52 Abs. 5 monthly cap (€10/kW/month max) is
    // applied based on the largest leistung_kw across violations.
    //
    // Dedup: if the same SanktionsTyp appears more than once, only the entry with
    // the most months is counted (double-reporting the same violation type is a
    // caller error; we guard against it here to prevent over-charging operators).
    let deduplicated_pflichtverstoss: Vec<_> = {
        use std::collections::HashMap;
        let mut by_typ: HashMap<u8, &crate::model::Pflichtverstoss> = HashMap::new();
        for v in &input.pflichtverstoss {
            let key = v.typ as u8;
            let entry = by_typ.entry(key).or_insert(v);
            if v.monate_des_verstosses > entry.monate_des_verstosses {
                *entry = v;
            }
        }
        by_typ.into_values().collect()
    };
    let pflichtzahlung_eur = if deduplicated_pflichtverstoss.is_empty() {
        None
    } else {
        let raw_sum: rust_decimal::Decimal = deduplicated_pflichtverstoss
            .iter()
            .map(|v| crate::foerderdauer::calculate_pflichtzahlung(v))
            .sum();
        // §52 Abs. 5 cap: total ≤ €10/kW × monate (using largest leistung_kw + months).
        // We use the violation with the most months as the cap basis.
        let cap = deduplicated_pflichtverstoss
            .iter()
            .map(|v| {
                use rust_decimal::dec;
                v.leistung_kw * dec!(10) * rust_decimal::Decimal::from(v.monate_des_verstosses)
            })
            .fold(
                rust_decimal::Decimal::ZERO,
                |a, b| if b > a { b } else { a },
            );
        Some(raw_sum.min(cap))
    };

    // Delegate to inner function.
    let mut result = calculate_settlement_inner(input);
    result.pflichtzahlung_eur = pflichtzahlung_eur;

    // ── §52 Abs. 6 Satz 1 EEG 2023 — Pflichtzahlung Fälligkeitsdatum ─────────
    // The §52 penalty is due on the 15th of the month following the billing month,
    // same computation as §26 Abs. 1 Vergütung Fälligkeitsdatum.
    // Only set when there is actually a Pflichtzahlung.
    if result
        .pflichtzahlung_eur
        .is_some_and(|p| p > rust_decimal::Decimal::ZERO)
    {
        result.pflichtzahlung_faelligkeitsdatum = result.faelligkeitsdatum; // same formula
    }

    // ── §51a — Verlängerung des Vergütungszeitraums ──────────────────────────
    // §51a Abs. 1: the extension is "für Strom aus Anlagen, für den sich der
    // anzulegende Wert nach Maßgabe des § 51 verringert" — so it accrues only
    // where §51 actually bit. A plant under the Abs. 2 exemption, or on a scheme
    // §51 does not reach, was paid in full and gets no extension.
    //
    // Before the Solarspitzengesetz the extension existed only for
    // ausschreibungspflichtige Anlagen; a statutory-AW plant commissioned in 2024
    // loses the quarter-hours without compensation. §51a never applies to §51b
    // biogas Ausschreibungsanlagen (§51b Satz 2).
    if !input.tariff_source.is_biogas_sect51b()
        && let Some(lost_qh) = input.negative_price_quarter_hours.filter(|&q| q > 0)
        && input.scheme.negativpreis_rule_applicable()
        && input
            .negativpreis_regime()
            .verlaengerungsanspruch(input.tariff_source.is_auction())
        && sect51_greift(
            input.kwh_during_negative_epex,
            aggregierte_leistung_kwp(input),
            input.negativpreis_regime(),
            input.erzeugungsart,
            input.has_imesys,
            input.ist_pilotwindanlage,
        )
    {
        let is_solar = input.erzeugungsart.is_some_and(|a| a.is_solar());
        result.verlaengerungsanspruch_qh =
            crate::foerderdauer::verguetungszeitraum_verlaengerung_qh(lost_qh, is_solar);
    }

    // ── §51 Abs. 3 EEG — Ausfallvergütung: unreported negative-price feed-in ──
    // "verringert sich der Anspruch ... um 5 Prozent für jeden Kalendertag, an
    // dem der Zeitraum ... ganz oder teilweise lag." A per-day reduction of the
    // month's claim, floored at zero — twenty such days extinguish it.
    //
    // Sequenced before the §25 proration because it reduces the claim itself,
    // and after the scheme dispatch because it applies to whatever that produced.
    if matches!(input.scheme, SettlementScheme::TemporaryFeedInTariff { .. })
        && input.sect51_abs3_unreported_days > 0
        && !matches!(
            result.status,
            SettlementStatus::NoData | SettlementStatus::PriceMissing
        )
    {
        let tage = Decimal::from(input.sect51_abs3_unreported_days);
        let faktor = (Decimal::ONE - dec!(0.05) * tage).max(Decimal::ZERO);
        if let Some(eur) = result.settlement_eur {
            let gekuerzt = validated_eur(eur * faktor);
            result.positions.push(crate::model::SettlePosition {
                description: format!(
 "\u{00a7}51 Abs. 3 EEG: Meldung der Einspeisung w\u{00e4}hrend negativer Preise unterblieben \u{2014} {} % K\u{00fc}rzung ({tage} Kalendertage)",
                    (Decimal::ONE - faktor) * Decimal::from(100)
                ),
                legal_basis: "\u{00a7}51 Abs. 3 EEG".to_owned(),
                kwh: Decimal::ZERO,
                rate_ct_kwh: Decimal::ZERO,
                eur: gekuerzt - eur,
            });
            result.settlement_eur = Some(gekuerzt);
        }
    }

    // ── §25 billing_days_fraction — auto-compute or use caller override ──────
    // Legal basis: §25 Abs. 1 Satz 3 EEG 2023 (commissioning day = start of entitlement)
    // When None: auto-compute from billing_date + foerderendedatum. The auto rule
    // narrows the Förderende month only — a meter commissioned mid-month already
    // recorded only the eligible days, so prorating it again would bill a
    // fraction of a fraction. See `compute_billing_days_fraction`.
    // When Some(x): use provided value directly (caller override for edge cases).
    let billing_days_fraction = input.billing_days_fraction.or_else(|| {
        crate::foerderdauer::compute_billing_days_fraction(
            input.foerderendedatum,
            input.billing_date,
        )
    });

    // Apply billing_days_fraction when < 1.0 (partial month)
    if let Some(fraction) =
        billing_days_fraction.filter(|&f| f > Decimal::ZERO && f < rust_decimal::Decimal::ONE)
    {
        if let Some(eur) = result.settlement_eur {
            result.settlement_eur = Some(validated_eur(eur * fraction));
        }
        if let Some(kwh) = result.eligible_kwh {
            result.eligible_kwh = Some((kwh * fraction).round_dp(3));
        }
        // Annotate all positions with the fraction
        for p in &mut result.positions {
            p.eur = validated_eur(p.eur * fraction);
            p.kwh = (p.kwh * fraction).round_dp(3);
        }
    }

    // Record the applied fraction in SettleOutput for audit trail (§ 147 AO / GoBD)
    result.billing_days_fraction_applied =
        billing_days_fraction.filter(|&f| f > Decimal::ZERO && f < rust_decimal::Decimal::ONE);

    // ── §13a EnWG — Einspeisemanagement (curtailment) compensation ───────────
    // Curtailment of EEG plants is Redispatch 2.0: the entschädigungspflichtige
    // Abregelung under §13a EnWG (historically §15 EEG Härtefallregelung). §51
    // Negativpreisregel does not touch these kWh because they were never fed in
    // (not by virtue of §19 EEG).
    //
    // Sequenced after the §25 proration on purpose: this is actual curtailed
    // energy under a separate EnWG claim, not a share of the month's
    // entitlement, so it must not be scaled by the billing-days fraction.
    if let Some(einsman_kwh) = input.einspeisemanagement_kwh.filter(|k| *k > Decimal::ZERO)
        && !matches!(
            result.status,
            SettlementStatus::NoData | SettlementStatus::PriceMissing
        )
        && !matches!(
            input.scheme,
            crate::scheme::SettlementScheme::Eigenverbrauch
        )
    {
        let comp_rate_ct = match &input.scheme {
            crate::scheme::SettlementScheme::MarketPremium {
                direktverm_aw_ct,
                wind_korrekturfaktor,
                wind_standort,
                ..
            } => {
                let raw_aw = *direktverm_aw_ct;
                if let Some(k) =
                    resolve_wind_korrekturfaktor(*wind_korrekturfaktor, wind_standort.as_ref())
                {
                    (raw_aw * k).round_dp(5)
                } else {
                    raw_aw
                }
            }
            _ => input.scheme.verguetungssatz_ct().unwrap_or(Decimal::ZERO),
        };
        let comp_eur = validated_eur(einsman_kwh * comp_rate_ct / Decimal::from(100));
        result.positions.push(crate::model::SettlePosition {
            description: format!("Einspeisemanagement-Ausfall §13a EnWG ({einsman_kwh} kWh)"),
            legal_basis: "§13a EnWG (Redispatch 2.0)".to_owned(),
            kwh: einsman_kwh,
            rate_ct_kwh: comp_rate_ct,
            eur: comp_eur,
        });
        result.settlement_eur = Some(result.settlement_eur.unwrap_or(Decimal::ZERO) + comp_eur);
        result.eligible_kwh = Some(result.eligible_kwh.unwrap_or(Decimal::ZERO) + einsman_kwh);
        // The status stays whatever the EEG rules made it: a §13a EnWG
        // compensation rides alongside, it does not undo a §42a/§43 sanction,
        // and reporting `Calculated` would conceal one.
    }

    // ── §26 Abs. 1 EEG 2023 — Fälligkeitsdatum ───────────────────────────────────
    // §26 Abs. 1: "monatlich jeweils zum 15. Kalendertag für den Vormonat" —
    // advance payments for the prior (billing) month are due on the 15th of the
    // FOLLOWING calendar month.
    if let Some(bd) = input.billing_date {
        let m = bd.month();
        let y = bd.year();
        let (next_year, next_month) = if m == time::Month::December {
            (y + 1, time::Month::January)
        } else {
            (y, m.next())
        };
        result.faelligkeitsdatum = time::Date::from_calendar_date(next_year, next_month, 15).ok();
    }

    // ── §52 Abs. 7 EEG 2023 — dezentrale Einspeisung (§18 StromNEV) ─────────
    // When any §52 violation penalty is due, the operator also loses the §18 StromNEV
    // dezentrale Einspeisung entgelt for the entire calendar year.
    if result
        .pflichtzahlung_eur
        .is_some_and(|p| p > rust_decimal::Decimal::ZERO)
    {
        result.dezentrale_einspeisung_anspruch_verloren = true;
    }

    // ── §44b Abs. 1 EEG 2023 — Biogas >100kW: 45% Bemessungsleistung cap ─────
    // Only eligible kWh receive normal EEG payment; excess receives:
    //   - MarketPremium: AW → 0, Marktprämie = 0
    //   - FeedInTariff/Tenant/Flex: paid at EPEX Marktwert (§44b Abs. 1 Satz 2)
    if let Some(sect44b_eligible) = input.biogas_sect44b_eligible_kwh {
        let effective_kwh = result.eligible_kwh.unwrap_or(rust_decimal::Decimal::ZERO);
        if effective_kwh > rust_decimal::Decimal::ZERO && sect44b_eligible < effective_kwh {
            let excess_kwh = effective_kwh - sect44b_eligible;
            let ratio = (sect44b_eligible / effective_kwh).min(rust_decimal::Decimal::ONE);

            // Scale all positions to the eligible fraction
            if let Some(eur) = result.settlement_eur {
                result.settlement_eur = Some(validated_eur(eur * ratio));
            }
            for p in &mut result.positions {
                p.eur = validated_eur(p.eur * ratio);
                p.kwh = (p.kwh * ratio).round_dp(3);
            }

            // Add excess position per scheme
            match &input.scheme {
                crate::scheme::SettlementScheme::FeedInTariff { .. }
                | crate::scheme::SettlementScheme::TenantElectricity { .. }
                | crate::scheme::SettlementScheme::TemporaryFeedInTariff { .. }
                | crate::scheme::SettlementScheme::FlexibilityPremium { .. } => {
                    // §44b Abs. 1 Satz 2: Einspeisevergütung → Marktwert for excess
                    // Without a Marktwert there is no price for the excess —
                    // settling it at zero would silently expropriate the operator.
                    let Some(excess_rate) = input.marktwert_ct_kwh else {
                        result.status = SettlementStatus::PriceMissing;
                        return result;
                    };
                    let excess_eur =
                        validated_eur(excess_kwh * excess_rate / rust_decimal::Decimal::from(100));
                    result.positions.push(crate::model::SettlePosition {
                        description: format!("§44b Abs. 1 Überschuss Marktwert ({excess_kwh} kWh)"),
                        legal_basis: "§44b Abs. 1 EEG 2023".to_owned(),
                        kwh: excess_kwh,
                        rate_ct_kwh: excess_rate,
                        eur: excess_eur,
                    });
                    result.settlement_eur = Some(
                        result.settlement_eur.unwrap_or(rust_decimal::Decimal::ZERO) + excess_eur,
                    );
                }
                crate::scheme::SettlementScheme::MarketPremium { .. } => {
                    // §44b Abs. 1 Satz 2: Marktprämie → 0 for excess (AW = null)
                    result.positions.push(crate::model::SettlePosition {
                        description: format!("§44b Abs. 1 Überschuss (AW = 0, {excess_kwh} kWh)"),
                        legal_basis: "§44b Abs. 1 EEG 2023".to_owned(),
                        kwh: excess_kwh,
                        rate_ct_kwh: rust_decimal::Decimal::ZERO,
                        eur: rust_decimal::Decimal::ZERO,
                    });
                }
                _ => {} // §44b does not apply to Eigenverbrauch, KwkSurcharge, etc.
            }
        }
    }

    result
}

/// Inner implementation — all SettleOutput constructions use `pflichtzahlung_eur: None`
/// as a placeholder; the public wrapper overwrites it.
fn calculate_settlement_inner(input: &SettleInput) -> SettleOutput {
    use crate::model::SanktionAlt;

    // ── §52 EEG ≤2021 three-tier sanction dispatch ────────────────────────────
    //
    // Abs. 1 → Vergütung = 0   (VerguetungAufNull)
    // Abs. 2 → Vergütung = EPEX Marktwert (VerguetungAufMarktwert)
    // Abs. 3 → Vergütung × 0.80 (VerguetungReduziert20Prozent)
    match input.sanktion {
        Some(SanktionAlt::VerguetungAufNull) => {
            // §52 Abs. 1 EEG ≤2021: anzulegender Wert verringert sich auf null.
            return SettleOutput {
                settlement_eur: Some(Decimal::ZERO),
                eligible_kwh: input.einspeisemenge_kwh,
                positions: vec![],
                status: SettlementStatus::Sanctioned,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            };
        }
        Some(SanktionAlt::VerguetungAufMarktwert) => {
            // §52 Abs. 2 EEG ≤2021: verringert sich auf den Monatsmarktwert (EPEX).
            // Same formula as PostEegSpot but within Förderdauer.
            let Some(epex_ct) = input.marktwert_ct_kwh else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            };
            let Some(kwh) = input.einspeisemenge_kwh else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::NoData,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            };
            // No §23b cap here (only for PostEegSpot ausgeförderte Anlagen).
            let positions = vec![pos(
                "Einspeisevergütung §52 Abs. 2 EEG (auf Marktwert verringert)",
                "§52 Abs. 2 EEG",
                kwh,
                epex_ct,
            )];
            return SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(kwh),
                positions,
                status: SettlementStatus::Sanctioned,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            };
        }
        Some(SanktionAlt::VerguetungReduziert20Prozent) => {
            // §52 Abs. 3 EEG ≤2021: verringert sich um 20 Prozent.
            // "wobei das Ergebnis auf zwei Stellen nach dem Komma gerundet wird"
            // Compute normal settlement without sanction, then apply -20% with 2dp rounding.
            let base = settle_normal_body(input);
            let reduced_eur = base.settlement_eur.map(|e| (e * dec!(0.80)).round_dp(2));
            return SettleOutput {
                settlement_eur: reduced_eur,
                eligible_kwh: base.eligible_kwh,
                positions: base.positions,
                status: SettlementStatus::Sanctioned,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            };
        }
        None => {} // no sanction → normal settlement
    }
    settle_normal_body(input)
}

/// Apply §§53b–54 to an anzulegender Wert, using the input's context.
///
/// Returns the reduced AW and the list of cuts that fired. Reductions act on the
/// AW rather than the settled amount because that is what the statutes say and
/// because the Marktprämie floors at zero — see [`crate::aw_reductions`].
fn apply_aw_cuts(
    aw_ct: Decimal,
    input: &SettleInput,
) -> (Decimal, Vec<crate::aw_reductions::AwReductionApplied>) {
    // §100 EEG — a Bestandsanlage that opted into the Solarspitzengesetz regime
    // is paid 0,6 ct/kWh more on everything it does feed in, in exchange for
    // forgoing payment during negative prices. The uplift is on the AW and so
    // comes before the §§53b–54 cuts, which are also AW-level.
    let aw_ct = if sect51_optin_active(input) {
        aw_ct + crate::negativpreis::SECT51_OPTIN_ZUSCHLAG_CT_KWH
    } else {
        aw_ct
    };
    if input.aw_reductions.is_empty() {
        return (aw_ct, Vec::new());
    }
    crate::aw_reductions::apply_aw_reductions(
        aw_ct,
        &input.aw_reductions,
        &input.tariff_source,
        input.erzeugungsart.unwrap_or_default(),
    )
}

/// Whether the §100 Solarspitzengesetz opt-in is in force for this period.
fn sect51_optin_active(input: &SettleInput) -> bool {
    matches!(
        (input.sect51_optin_wirksam_ab, input.billing_date),
        (Some(ab), Some(bd)) if bd >= ab
    )
}

/// Render each AW cut as a zero-euro audit position.
///
/// The euro effect is already inside the reduced rate on the main position, so
/// these carry the deduction as a negative `rate_ct_kwh` and `eur: 0` — double
/// counting the money would misstate the settlement. They exist so the
/// Gutschrift names every statute that touched the AW.
fn aw_cut_positions(
    cuts: &[crate::aw_reductions::AwReductionApplied],
    kwh: Decimal,
) -> Vec<crate::model::SettlePosition> {
    cuts.iter()
        .map(|c| crate::model::SettlePosition {
            description: c.description.clone(),
            legal_basis: c.legal_basis.clone(),
            kwh,
            rate_ct_kwh: -c.deduction_ct_kwh,
            eur: Decimal::ZERO,
        })
        .collect()
}

/// Core settlement body — executes AFTER all §52 sanction checks.
/// Also called directly by the §52 Abs. 3 (-20%) path.
fn settle_normal_body(input: &SettleInput) -> SettleOutput {
    // ── Automatic FoerderungBeendet detection ────────────────────────────────
    // Only applies for single-block plants. Multi-block plants handle per-block
    // expiry inside calculate_with_capacity_blocks().
    if input.capacity_blocks.is_empty()
        && let (Some(billing), Some(fed)) = (input.billing_date, input.foerderendedatum)
        && billing > fed
    {
        return SettleOutput {
            settlement_eur: Some(Decimal::ZERO),
            eligible_kwh: input.einspeisemenge_kwh,
            positions: vec![],
            status: SettlementStatus::FoerderungBeendet,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        };
    }

    // ── No meter data ─────────────────────────────────────────────────────────
    // §50a FlexibilitaetZuschlag is capacity-based (not kWh-based) — bypass this check.
    let Some(kwh) = input.einspeisemenge_kwh else {
        if let SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year,
        } = &input.scheme
        {
            // Route to model dispatch with kwh = ZERO (unused for capacity payments)
            let kwh_dummy = Decimal::ZERO;
            let kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
            let rate_eur_per_kw_year = *rate_eur_per_kw_year;
            let monthly_eur = validated_eur(kw * rate_eur_per_kw_year / dec!(12));
            let positions = vec![SettlePosition {
                description: format!(
                    "Flexibilit\u{00e4}tszuschlag \u{00a7}50a EEG 2023 \
                    ({kw}\u{202f}kW \u{00d7} {rate_eur_per_kw_year}\u{202f}EUR/kW/Jahr \u{00f7} 12)"
                ),
                legal_basis: "\u{00a7}50a EEG 2023".to_owned(),
                kwh: kw,
                rate_ct_kwh: rate_eur_per_kw_year,
                eur: monthly_eur,
            }];
            let _ = kwh_dummy; // unused
            return SettleOutput {
                settlement_eur: Some(monthly_eur),
                eligible_kwh: Some(kw),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            };
        }
        return SettleOutput {
            settlement_eur: None,
            eligible_kwh: None,
            positions: vec![],
            status: SettlementStatus::NoData,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        };
    };

    // ── Multi-block (§24 Anlagenerweiterung) ─────────────────────────────────
    if !input.capacity_blocks.is_empty() {
        return calculate_with_capacity_blocks(input, kwh);
    }

    // ── Effective §51 application ─────────────────────────────────────────────
    let apply_neg = input.scheme.negativpreis_rule_applicable()
        && sect51_greift(
            input.kwh_during_negative_epex,
            aggregierte_leistung_kwp(input),
            input.negativpreis_regime(),
            input.erzeugungsart,
            input.has_imesys,
            input.ist_pilotwindanlage,
        );
    let neg_kwh = if apply_neg {
        input.kwh_during_negative_epex
    } else {
        None
    };

    // ══ SETTLEMENT PIPELINE ══════════════════════════════════════════════
    // 1. §52 sanction check (short-circuits to EUR 0 or EPEX Marktwert)
    // 2. FoerderungBeendet detection (billing_date > foerderendedatum)
    // 3. Scheme dispatch → gross settlement positions
    // 4. §51a verlängerungsanspruch (output field, informational)
    // 5. §13a EnWG EInsMan compensation (separate position)
    // 6. §§53b–54 AW-level reductions (applied to the AW, not the result)
    // Output: SettleOutput with all positions summed
    match &input.scheme {
        // ── EUR 0 — Eigenverbrauch ────────────────────────────────────────────
        SettlementScheme::Eigenverbrauch => SettleOutput {
            settlement_eur: Some(Decimal::ZERO),
            eligible_kwh: Some(kwh),
            positions: vec![],
            status: SettlementStatus::Calculated,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        },

        // ── EUR 0 — §21a Sonstige Direktvermarktung ───────────────────────────
        // The operator exercises their §21a EEG 2023 right to sell directly to a
        // third party (not via Marktprämie and not via Einspeisevergütung).
        // No NB payment for this period. Records the period for settlement history.
        SettlementScheme::SonstigeDirektvermarktung => SettleOutput {
            settlement_eur: Some(Decimal::ZERO),
            eligible_kwh: Some(kwh),
            positions: vec![crate::model::SettlePosition {
                description:
                    "Sonstige Direktvermarktung \u{00a7}21a EEG 2023 (kein EEG-Zahlungsanspruch)"
                        .to_owned(),
                legal_basis: "\u{00a7}21a EEG 2023".to_owned(),
                kwh,
                rate_ct_kwh: Decimal::ZERO,
                eur: Decimal::ZERO,
            }],
            status: SettlementStatus::Calculated,
            pflichtzahlung_eur: None,
            pflichtzahlung_faelligkeitsdatum: None,
            verlaengerungsanspruch_qh: 0,
            dezentrale_einspeisung_anspruch_verloren: false,
            billing_days_fraction_applied: None,
            faelligkeitsdatum: None,
        },

        // ── §21 EEG — Einspeisevergütung und Ausfallvergütung ────────────────
        SettlementScheme::TemporaryFeedInTariff { verguetungssatz_ct }
        | SettlementScheme::FeedInTariff { verguetungssatz_ct } => {
            let ist_ausfallverguetung =
                matches!(input.scheme, SettlementScheme::TemporaryFeedInTariff { .. });
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            // §53 Abs. 1 EEG 2023: subtract the flat AW deduction only when the
            // supplied rate is the GROSS AW. Default net → no change.
            let rate_ct = if input.aw_is_gross
                && !ist_ausfallverguetung
                && let Some(art) = input.erzeugungsart
            {
                (*verguetungssatz_ct - crate::rates::sect53_deduction(art)).max(Decimal::ZERO)
            } else {
                *verguetungssatz_ct
            };
            // §53 Abs. 3 EEG 2023 — Ausfallvergütung: "verringert sich der
            // anzulegende Wert um 20 Prozent", rounded to two decimals.
            //
            // The **engine** applies it, not the caller. Left to the caller,
            // the plant's ordinary tariff passes straight through and a plant on
            // the Ausfallvergütung is paid the full rate — 25 % more than the
            // statute allows, on the one scheme that exists because the
            // operator's Direktvermarkter dropped out.
            let rate_ct = if ist_ausfallverguetung {
                (rate_ct * dec!(0.8)).round_dp(2)
            } else {
                rate_ct
            };
            // §§53b–54 reduce the anzulegender Wert itself, after §53.
            let (rate_ct, aw_cuts) = apply_aw_cuts(rate_ct, input);
            let (desc, basis) = match (ist_ausfallverguetung, neg_kwh.is_some()) {
                (true, true) => (
                    "Ausfallverg\u{00fc}tung \u{00a7}21 Abs. 1 Satz 1 Nr. 3 EEG (\u{2212}20 % nach \u{00a7}53 Abs. 3; \u{00a7}51 Negativpreisregel angewendet)",
                    "\u{00a7}21 Abs. 1 Satz 1 Nr. 3 EEG 2023",
                ),
                (true, false) => (
                    "Ausfallverg\u{00fc}tung \u{00a7}21 Abs. 1 Satz 1 Nr. 3 EEG (\u{2212}20 % nach \u{00a7}53 Abs. 3)",
                    "\u{00a7}21 Abs. 1 Satz 1 Nr. 3 EEG 2023",
                ),
                (false, true) => (
                    "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}51 Negativpreisregel angewendet)",
                    "\u{00a7}21 EEG 2023",
                ),
                (false, false) => (
                    "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG",
                    "\u{00a7}21 EEG 2023",
                ),
            };
            let mut positions = vec![pos(desc, basis, effective, rate_ct)];
            positions.extend(aw_cut_positions(&aw_cuts, effective));
            SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(effective),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }

        // ── §21 Abs. 3 EEG — Mieterstrom ────────────────────────────────────────────
        SettlementScheme::TenantElectricity {
            verguetungssatz_ct,
            mieter_zuschlag_ct,
        } => {
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            let zuschlag = mieter_zuschlag_ct.unwrap_or(Decimal::ZERO);
            let base_desc = if neg_kwh.is_some() {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}51 Negativpreisregel angewendet)"
            } else {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG"
            };
            let mut positions = vec![pos(
                base_desc,
                "\u{00a7}21 EEG 2023",
                effective,
                *verguetungssatz_ct,
            )];
            if zuschlag != Decimal::ZERO {
                positions.push(pos(
                    "Mieterstrom-Zuschlag \u{00a7}21 Abs. 3 EEG 2023",
                    "\u{00a7}21 Abs. 3 EEG 2023",
                    effective,
                    zuschlag,
                ));
            }
            SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(effective),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }

        // ── §20 EEG — Gleitende Marktprämie ──────────────────────────────────
        // ── §§22a,28 EEG — Ausschreibungsanlagen ─────────────────────────────
        SettlementScheme::MarketPremium {
            direktverm_aw_ct,
            wind_korrekturfaktor,
            wind_standort,
        } => {
            // §51 Abs. 1 zeroes the anzulegender Wert for the negative-price
            // intervals, and Anlage 1 Nr. 1 defines "AW" as the anzulegender Wert
            // "unter Berücksichtigung der §§ 19 bis 54" — so those kWh earn no
            // Marktprämie. Excluding them is the same arithmetic as AW = 0.
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            // §20 Abs. 2 + Anlage 1 EEG 2023: Jahresmarktwert takes precedence over monthly EPEX
            // when provided. The ÜNB publishes technology-specific annual market values.
            let epex_source = input.marktwert_ct_kwh;
            let Some(epex_ct) = epex_source else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            };
            let raw_aw_ct = *direktverm_aw_ct;

            // A Marktprämie plant with no anzulegender Wert is not a plant owed
            // nothing — it is a plant whose AW nobody supplied. `max(0, 0 − MW)`
            // is zero for every market price, so settling it produced a
            // `Calculated` EUR 0 and a payout event for that amount, which reads
            // downstream as "correctly settled, nothing due". A missing AW is
            // exactly as fatal as a missing Marktwert and is reported the same way.
            //
            // Zero is only ever a *derived* AW: §54 Abs. 4, §51b and §51 all set it
            // to null explicitly, and each does so after this point.
            if raw_aw_ct <= Decimal::ZERO {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![crate::model::SettlePosition {
 description: "Marktpr\u{00e4}mie ohne anzulegenden Wert: direktverm_aw_ct fehlt oder ist null"
                            .to_owned(),
                        legal_basis: "\u{00a7}20 EEG 2023 i.V.m. Anlage 1".to_owned(),
                        kwh: Decimal::ZERO,
                        rate_ct_kwh: Decimal::ZERO,
                        eur: Decimal::ZERO,
                    }],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            }

            // ── §51b EEG 2023 — Biogas Ausschreibung at slightly-positive prices ──
            // For biogas plants (excl. biomethane) whose AW was set by auction:
            // the AW reduces to ZERO when EPEX ≤ 2 ct/kWh.
            // §51 and §51a do NOT apply to these plants (§51b Satz 2 EEG 2023).
            //
            // Source: EEG 2023 §51b, Clearingstelle EEG|KWKG Working Text 23.12.2025.
            // "verringert sich der anzulegende Wert auf null für Zeiträume, in denen
            //  der Spotmarktpreis 2 Cent pro Kilowattstunde oder weniger beträgt."
            if input.tariff_source.is_biogas_sect51b() && epex_ct <= dec!(2) {
                // AW = 0 for this period; payment is zero.
                return SettleOutput {
                    settlement_eur: Some(Decimal::ZERO),
                    eligible_kwh: input.einspeisemenge_kwh,
                    positions: vec![pos(
                        "\u{00a7}51b EEG 2023 Biogasanlage Ausschreibung \
                         (Spotmarktpreis \u{2264} 2\u{202f}ct/kWh \u{2192} AW = 0)",
                        "\u{00a7}51b EEG 2023",
                        kwh,
                        Decimal::ZERO,
                    )],
                    status: SettlementStatus::Calculated,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            }

            // ── §36h EEG — Wind onshore Korrekturfaktor ───────────────────────
            // When supplied (via wind_korrekturfaktor or wind_standort), multiply
            // the base AW by the location correction factor.
            // Applies only to wind onshore plants; §36h Abs. 4: no correction for ≤EEG2012.
            let aw_ct = if let Some(k) =
                resolve_wind_korrekturfaktor(*wind_korrekturfaktor, wind_standort.as_ref())
            {
                (raw_aw_ct * k).round_dp(5)
            } else {
                raw_aw_ct
            };
            // §§53b–54 cut the AW before the max(0, …) floor below, so a plant
            // whose Marktwert already exceeds its AW is not driven negative.
            let (aw_ct, aw_cuts) = apply_aw_cuts(aw_ct, input);
            // ── §39n EEG 2023 — Innovationsausschreibung: feste Marktprämie ───
            // Innovation-auction awards pay a FIXED premium (the Zuschlagswert per
            // kWh, §3 InnAusV) on top of the market sale — it is not reduced by the
            // Monatsmarktwert like the gleitende Marktprämie. §39n Abs. 3 delegates
            // the mechanism to the Innovationsausschreibungsverordnung (§88d); the
            // fixed-premium rule is the defining InnAusV feature. §51 still zeroes
            // the AW for negative-price intervals, which `effective` already
            // carries, so no separate handling is needed here.
            if input.tariff_source.is_innovation_auction() {
                let feste_praemie_eur = validated_eur(effective * aw_ct / Decimal::from(100));
                let mut positions = vec![pos(
                    "Feste Marktpr\u{00e4}mie \u{00a7}39n EEG 2023 (Innovationsausschreibung, \u{00a7}3 InnAusV)",
                    "\u{00a7}39n EEG 2023 i.V.m. \u{00a7}3 InnAusV",
                    effective,
                    aw_ct,
                )];
                positions.extend(aw_cut_positions(&aw_cuts, effective));
                return SettleOutput {
                    settlement_eur: Some(feste_praemie_eur),
                    eligible_kwh: Some(effective),
                    positions,
                    status: SettlementStatus::Calculated,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            }

            // ── Anlage 1 Nr. 3.1.2 / 4.1.2 EEG 2023 — MP = AW − MW ────────────
            // Floored at zero: "Ergibt sich bei der Berechnung ein Wert kleiner
            // null, wird … der Wert 'MP' mit null festgesetzt."
            let praemie_ct = (aw_ct - epex_ct).max(Decimal::ZERO);

            let (praemie_desc, praemie_basis) = if input.tariff_source.is_auction() {
                (
                    "Gleitende Marktpr\u{00e4}mie \u{00a7}\u{00a7}22a,28 EEG 2023 (Ausschreibung)",
                    "\u{00a7}\u{00a7}22a,28 EEG 2023",
                )
            } else if neg_kwh.is_some() {
                (
                    "Gleitende Marktpr\u{00e4}mie \u{00a7}23a EEG 2023 (\u{00a7}51 Negativpreisregel angewendet)",
                    "\u{00a7}23a EEG 2023 i.V.m. Anlage 1",
                )
            } else {
                (
                    "Gleitende Marktpr\u{00e4}mie \u{00a7}23a EEG 2023",
                    "\u{00a7}23a EEG 2023 i.V.m. Anlage 1",
                )
            };

            // A zero premium still gets a position: MW ≥ AW is a settled result,
            // not a missing one, and the Gutschrift has to show the period.
            let mut positions = vec![pos(praemie_desc, praemie_basis, effective, praemie_ct)];
            // Name every statute that cut the AW, even where the floor absorbed it.
            positions.extend(aw_cut_positions(&aw_cuts, effective));

            let total_eur = positions.iter().map(|p| p.eur).sum();
            SettleOutput {
                settlement_eur: Some(total_eur),
                eligible_kwh: Some(effective),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }

        // ── Post-EEG Spot (§21 post-Förderung + §23b cap) ─────────────────────
        // Negative EPEX → negative EUR (plant pays). No floor.
        // §23b EEG 2023: Jahresmarktwert capped at 10 ct/kWh for ausgeförderte Anlagen.
        SettlementScheme::PostEeg { price_floor } => {
            let Some(epex_ct) = input.marktwert_ct_kwh else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            };
            // §23b EEG 2023: "ab dem Kalenderjahr 2023 höchstens jedoch 10 Cent pro kWh"
            // The cap only applies when EPEX is POSITIVE (the plant gets at most 10 ct).
            //
            // Negative EPEX: whether the plant pays depends on the post-EEG marketing
            // contract — NOT a statutory rule. Use post_eeg_price_floor to configure:
            //   None            = full market exposure (default, EPEX used as-is)
            //   Some(ZERO)      = floor at 0 (no obligation for negative periods)
            //   Some(custom)    = contract-defined floor
            let epex_floored = if let Some(floor) = *price_floor {
                epex_ct.max(floor)
            } else {
                epex_ct
            };
            let effective_ct = if epex_floored > dec!(10) {
                dec!(10)
            } else {
                epex_floored
            };
            let was_capped = epex_floored > dec!(10);
            let desc = if was_capped {
                format!(
                    "Einspeiseverg\u{00fc}tung Post-EEG Spot \
                    (\u{00a7}23b Jahresmarktwert-Deckel: EPEX {:.2}\u{202f}ct \u{2192} 10\u{202f}ct)",
                    epex_ct
                )
            } else {
                "Einspeiseverg\u{00fc}tung Post-EEG Spot (\u{00a7}21 EEG, nach F\u{00f6}rderungsende)".to_owned()
            };
            let positions = vec![pos(
                desc,
                "\u{00a7}21 EEG (post-F\u{00f6}rderung)",
                kwh,
                effective_ct,
            )];
            SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(kwh),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }

        // ── §7 KWKG 2023 — KWK-Zuschlag ──────────────────────────────────────
        SettlementScheme::KwkSurcharge {
            verguetungssatz_ct,
            kwh_paid_gesamt,
            max_kwh,
        } => {
            use crate::foerderdauer::kwk_eligible_kwh;

            let (eligible, limit_reached) = match (*kwh_paid_gesamt, *max_kwh) {
                (Some(paid), Some(max)) => kwk_eligible_kwh(kwh, paid, max),
                _ => (kwh, false),
            };

            if eligible <= Decimal::ZERO {
                return SettleOutput {
                    settlement_eur: Some(Decimal::ZERO),
                    eligible_kwh: Some(Decimal::ZERO),
                    positions: vec![],
                    status: SettlementStatus::FoerderungBeendet,
                    pflichtzahlung_eur: None,
                    pflichtzahlung_faelligkeitsdatum: None,
                    verlaengerungsanspruch_qh: 0,
                    dezentrale_einspeisung_anspruch_verloren: false,
                    billing_days_fraction_applied: None,
                    faelligkeitsdatum: None,
                };
            }

            let desc = if limit_reached {
                format!(
                    "KWK-Zuschlag \u{00a7}7 KWKG 2023 (F\u{00f6}rderdauer-Endabrechnung: {eligible} von {kwh} kWh)"
                )
            } else {
                "KWK-Zuschlag \u{00a7}7 KWKG 2023".to_owned()
            };
            let positions = vec![pos(
                desc,
                "\u{00a7}7 KWKG 2023",
                eligible,
                *verguetungssatz_ct,
            )];
            let status = if limit_reached {
                SettlementStatus::FoerderungBeendet
            } else {
                SettlementStatus::Calculated
            };
            SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(eligible),
                positions,
                status,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }

        // ── §50b EEG — Flexibilitätsprämie (bestehende Anlagen) ──────────────
        SettlementScheme::FlexibilityPremium {
            verguetungssatz_ct,
            flex_praemie_ct_kwh,
        } => {
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            let flex_ct = flex_praemie_ct_kwh.unwrap_or(Decimal::ZERO);
            let base_desc = if neg_kwh.is_some() {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}51 Negativpreisregel angewendet)"
            } else {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG"
            };
            let mut positions = vec![pos(
                base_desc,
                "\u{00a7}21 EEG 2023",
                effective,
                *verguetungssatz_ct,
            )];
            if flex_ct != Decimal::ZERO {
                positions.push(pos(
                    "Flexibilit\u{00e4}tspr\u{00e4}mie \u{00a7}50b EEG 2023 (bestehende Anlage)",
                    "\u{00a7}50b EEG 2023",
                    effective,
                    flex_ct,
                ));
            }
            SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(effective),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }

        // ── §50a EEG 2023 — Flexibilitätszuschlag (neue Anlagen) ─────────────
        // Capacity-based payment: EUR/kW/year (statutory: 100 EUR/kW/year).
        // leistung_kwp = additional flexible capacity in kW.
        // rate_eur_per_kw_year = annual rate per kW in EUR (100 EUR/kW/year).
        // Monthly = leistung_kwp × rate / 12.
        SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year,
        } => {
            let kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
            let rate_eur_per_kw_year = *rate_eur_per_kw_year;
            let monthly_eur = validated_eur(kw * rate_eur_per_kw_year / dec!(12));
            let positions = vec![SettlePosition {
                description: format!(
                    "Flexibilit\u{00e4}tszuschlag \u{00a7}50a EEG 2023 \
                    ({kw}\u{202f}kW \u{00d7} {rate_eur_per_kw_year}\u{202f}EUR/kW/Jahr \u{00f7} 12)"
                ),
                legal_basis: "\u{00a7}50a EEG 2023".to_owned(),
                kwh: kw,                           // semantic: kW flexible capacity, not kWh
                rate_ct_kwh: rate_eur_per_kw_year, // semantic: EUR/kW/year
                eur: monthly_eur,
            }];
            SettleOutput {
                settlement_eur: Some(monthly_eur),
                eligible_kwh: Some(kw),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
                pflichtzahlung_faelligkeitsdatum: None,
                verlaengerungsanspruch_qh: 0,
                dezentrale_einspeisung_anspruch_verloren: false,
                billing_days_fraction_applied: None,
                faelligkeitsdatum: None,
            }
        }
    }
}
