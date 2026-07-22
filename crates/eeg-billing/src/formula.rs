//! Pure settlement formula — [`calculate_settlement`].

use billing::EuroAmount;
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
    EuroAmount::checked_from_decimal(d)
        .map(billing::EuroAmount::into_decimal)
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

/// Version-aware §51 applicability check.
///
/// Takes typed [`EegGesetz`] and optional [`ErzeugungsArt`].
/// The caller must only pass `kwh_during_negative_epex` after verifying that the
/// version-specific consecutive-hour threshold was met (caller's responsibility).
/// This function enforces only the **kW exemption** and the iMSys post-rollout rule.
fn should_apply_negativpreis_versioned(
    kwh_during_negative_epex: Option<Decimal>,
    leistung_kwp: Option<Decimal>,
    eeg_gesetz: crate::version::EegGesetz,
    erzeugungsart: Option<crate::technology::ErzeugungsArt>,
    has_imesys: bool,
) -> bool {
    if kwh_during_negative_epex.is_none_or(|k| k <= Decimal::ZERO) {
        return false;
    }
    // §51 Abs. 2 Nr. 1 EEG 2023: once iMSys installed, ALL plant sizes subject to §51.
    // The <100 kW transitional exemption is lifted from the rollout date.
    if has_imesys && eeg_gesetz == crate::version::EegGesetz::Eeg2023 {
        return true;
    }
    let art = erzeugungsart.unwrap_or(crate::technology::ErzeugungsArt::Solar);
    let Some(threshold_kw) = eeg_gesetz.negativpreis_kw_grenze(&art) else {
        return false; // §51 not applicable for this EEG version
    };
    leistung_kwp.is_none_or(|kw| kw >= Decimal::from(threshold_kw))
}

/// Apply §51 EEG deduction: subtract kWh during negative-price hours.
/// Only called after `should_apply_negativpreis` returns `true`.
fn apply_negativpreis(kwh: Decimal, negative_kwh: Decimal) -> Decimal {
    (kwh - negative_kwh).max(Decimal::ZERO)
}

/// Resolve the effective §36k wind onshore Korrekturfaktor.
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

/// Resolve the effective Managementprämie for Direktvermarktung/Ausschreibung.
///
/// - If explicitly provided, use it.
/// - If `leistung_kwp` is provided, compute from the §20 Abs. 3 EEG threshold.
/// - Otherwise: 0 (caller omitted — audit log will show no premium).
fn resolve_managementpraemie(
    managementpraemie_ct: Option<Decimal>,
    leistung_kwp: Option<Decimal>,
) -> Decimal {
    managementpraemie_ct.unwrap_or_else(|| {
        leistung_kwp
            .map(crate::foerderdauer::managementpraemie_ct)
            .unwrap_or(Decimal::ZERO)
    })
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

        // Apply §51 Negativpreisregel for this block if applicable
        if should_apply_negativpreis_versioned(
            input.kwh_during_negative_epex,
            Some(primary_kwp),
            input.effective_eeg_gesetz(),
            input.erzeugungsart,
            input.has_imesys,
        ) {
            // Proportional share of negative kWh for this block
            let neg_share = input
                .kwh_during_negative_epex
                .map(|n| (n * share).round_dp(3))
                .unwrap_or(Decimal::ZERO);
            block_kwh = apply_negativpreis(block_kwh, neg_share);
        }

        let primary_rate = input.scheme.verguetungssatz_ct().unwrap_or(Decimal::ZERO);
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

        // §51 per-block: each block derives EegGesetz from its own inbetriebnahme
        let block_gesetz =
            crate::version::EegGesetz::from_inbetriebnahme_year(block.inbetriebnahme.year());
        if should_apply_negativpreis_versioned(
            input.kwh_during_negative_epex,
            Some(block.leistung_kwp),
            block_gesetz,
            input.erzeugungsart,
            input.has_imesys,
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

    // ── §51a EEG 2023 — Verlängerungsanspruch (payment period extension) ─────
    // §51a does NOT apply to §51b biogas Ausschreibungsanlagen (§51b Satz 2 EEG 2023).
    if !input.tariff_source.is_biogas_sect51b()
        && let Some(lost_qh) = input.negative_price_quarter_hours.filter(|&q| q > 0)
    {
        let was_applied = input
            .kwh_during_negative_epex
            .is_some_and(|k| k > rust_decimal::Decimal::ZERO);
        if was_applied {
            let is_solar = input.erzeugungsart.is_some_and(|a| a.is_solar());
            result.verlaengerungsanspruch_qh =
                crate::foerderdauer::verguetungszeitraum_verlaengerung_qh(lost_qh, is_solar);
        }
    }

    // ── §19 EEG — Einspeisemanagement (curtailment) compensation ─────────────
    // §19 Abs. 2 EEG 2023: §51 Negativpreisregel does NOT apply to EInsMan kWh.
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
            description: format!("Einspeisemanagement-Ausfall §19 EEG ({einsman_kwh} kWh)"),
            legal_basis: "§19 EEG 2023".to_owned(),
            kwh: einsman_kwh,
            rate_ct_kwh: comp_rate_ct,
            eur: comp_eur,
        });
        result.settlement_eur = Some(result.settlement_eur.unwrap_or(Decimal::ZERO) + comp_eur);
        result.eligible_kwh = Some(result.eligible_kwh.unwrap_or(Decimal::ZERO) + einsman_kwh);
        if result.status == SettlementStatus::Sanctioned {
            result.status = SettlementStatus::Calculated;
        }
    }

    // ── §53b EEG 2023 — Regionale Grünstromkennzeichnung reduction ───────────
    // BNetzA-certified reduction for plants in renewable-saturated grid areas.
    // Applies only to Verguetung / Mieterstrom / Flexibilitaet.
    if let Some(r53b_ct) = input
        .sect53b_regional_reduction_ct
        .filter(|r| *r > Decimal::ZERO)
        && !matches!(
            result.status,
            SettlementStatus::NoData | SettlementStatus::PriceMissing
        )
        && matches!(
            input.scheme,
            crate::scheme::SettlementScheme::FeedInTariff { .. }
                | crate::scheme::SettlementScheme::TenantElectricity { .. }
                | crate::scheme::SettlementScheme::FlexibilityPremium { .. }
        )
        && let Some(elig_kwh) = result.eligible_kwh.filter(|k| *k > Decimal::ZERO)
    {
        let reduction_eur = -validated_eur(elig_kwh * r53b_ct / Decimal::from(100));
        result.positions.push(crate::model::SettlePosition {
            description: format!(
                "\u{00a7}53b EEG 2023 Regionale Reduzierung ({r53b_ct}\u{202f}ct/kWh)"
            ),
            legal_basis: "\u{00a7}53b EEG 2023".to_owned(),
            kwh: elig_kwh,
            rate_ct_kwh: -r53b_ct,
            eur: reduction_eur,
        });
        result.settlement_eur =
            Some(result.settlement_eur.unwrap_or(Decimal::ZERO) + reduction_eur);
    }

    // ── §25 billing_days_fraction — auto-compute or use caller override ──────
    // Legal basis: §25 Abs. 1 Satz 3 EEG 2023 (commissioning day = start of entitlement)
    // When None: auto-compute from billing_date + inbetriebnahme + foerderendedatum.
    // When Some(x): use provided value directly (caller override for edge cases).
    let billing_days_fraction = input.billing_days_fraction.or_else(|| {
        crate::foerderdauer::compute_billing_days_fraction(
            input.inbetriebnahme,
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
                    let epex_source = input.marktwert_ct_kwh;
                    let excess_rate = epex_source.unwrap_or(rust_decimal::Decimal::ZERO);
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
    let apply_neg = should_apply_negativpreis_versioned(
        input.kwh_during_negative_epex,
        input.leistung_kwp,
        input.effective_eeg_gesetz(),
        input.erzeugungsart,
        input.has_imesys,
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
    // 5. §19 EInsMan compensation (separate position)
    // 6. §53b regional reduction (separate position)
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

        // ── §21 EEG — Feste Einspeisevergütung ───────────────────────────────
        // §21 Abs. 1 Nr. 2 EEG — Ausfallvergütung uses same formula as FeedInTariff
        // but at 80% of the statutory rate. Caller must supply the reduced rate.
        SettlementScheme::TemporaryFeedInTariff { verguetungssatz_ct }
        | SettlementScheme::FeedInTariff { verguetungssatz_ct } => {
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            let desc = if neg_kwh.is_some() {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}51 Negativpreisregel angewendet)"
            } else {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG"
            };
            let positions = vec![pos(
                desc,
                "\u{00a7}21 EEG 2023",
                effective,
                *verguetungssatz_ct,
            )];
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
            managementpraemie_ct,
            wind_korrekturfaktor,
            wind_standort,
        } => {
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

            // ── §36k EEG — Wind onshore Korrekturfaktor ───────────────────────
            // When supplied (via wind_korrekturfaktor or wind_standort), multiply
            // the base AW by the location correction factor.
            // Applies only to wind onshore plants; §36k Abs. 4: no correction for ≤EEG2012.
            let aw_ct = if let Some(k) =
                resolve_wind_korrekturfaktor(*wind_korrekturfaktor, wind_standort.as_ref())
            {
                (raw_aw_ct * k).round_dp(5)
            } else {
                raw_aw_ct
            };
            let mgmt_ct = resolve_managementpraemie(*managementpraemie_ct, input.leistung_kwp);

            // ── §20 Abs. 3 EEG 2023 — Managementprämie ────────────────────────
            // The Managementprämie is NOT a separate guaranteed floor payment.
            // §20 Abs. 3 EEG 2023: "der anzulegende Wert um 0,4 ct/kWh zu erhöhen"
            // → AW_eff = AW + Managementprämie; Marktprämie = max(0, AW_eff − EPEX).
            //
            // When EPEX > AW + Managementprämie: total = 0 (no payment at all).
            // The old EEG ≤2012 "separate floor" model (mgmt always paid) is gone.
            //
            // For billing positions we decompose into pure-spread and management components:
            //   pure_praemie = max(0, AW − EPEX)        — the spread ignoring Managementprämie
            //   effective_mgmt = total − pure_praemie   — the residual Managementprämie amount
            // Both can be zero when EPEX ≥ AW + Managementprämie.
            let eff_aw_ct = aw_ct + mgmt_ct;
            let total_spread_ct = (eff_aw_ct - epex_ct).max(Decimal::ZERO);
            let pure_praemie_ct = (aw_ct - epex_ct).max(Decimal::ZERO);
            let effective_mgmt_ct = total_spread_ct - pure_praemie_ct;
            // Invariant: pure_praemie_ct + effective_mgmt_ct == total_spread_ct

            let (praemie_desc, praemie_basis) = if input.tariff_source.is_auction() {
                (
                    "Gleitende Marktpr\u{00e4}mie \u{00a7}\u{00a7}22a,28 EEG 2023 (Ausschreibung)",
                    "\u{00a7}\u{00a7}22a,28 EEG 2023",
                )
            } else {
                (
                    "Gleitende Marktpr\u{00e4}mie \u{00a7}20 EEG 2023",
                    "\u{00a7}20 EEG 2023",
                )
            };

            let mut positions = vec![];
            if pure_praemie_ct > Decimal::ZERO {
                positions.push(pos(praemie_desc, praemie_basis, kwh, pure_praemie_ct));
            }
            if effective_mgmt_ct > Decimal::ZERO {
                positions.push(pos(
                    "Managementpr\u{00e4}mie \u{00a7}20 Abs.\u{202f}3 EEG 2023",
                    "\u{00a7}20 Abs. 3 EEG 2023",
                    kwh,
                    effective_mgmt_ct,
                ));
            }
            if positions.is_empty() {
                // EPEX ≥ AW + Managementprämie: zero payment — show audit position.
                positions.push(pos(praemie_desc, praemie_basis, kwh, Decimal::ZERO));
            }

            let total_eur = positions.iter().map(|p| p.eur).sum();
            SettleOutput {
                settlement_eur: Some(total_eur),
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
