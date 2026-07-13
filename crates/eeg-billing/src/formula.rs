//! Pure settlement formula — [`calculate_settlement`].

use billing::EuroAmount;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use time::macros::date;

use crate::model::{SettleInput, SettleOutput, SettlePosition, SettlementModel, SettlementStatus};

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Validate and round a settlement amount to 5 decimal places via [`EuroAmount`].
fn validated_eur(d: Decimal) -> Decimal {
    EuroAmount::checked_from_decimal(d)
        .map(|a| a.into_decimal())
        .unwrap_or(Decimal::ZERO)
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

/// §51 EEG — determine whether the Negativpreisregel applies to this plant.
///
/// Uses the plant's `eeg_gesetz` to apply the correct version-specific threshold:
/// - EEG ≤2014: no rule (returns false)
/// - EEG 2017: ≥6 consecutive hours, exempt <500 kW (non-wind) / <3 MW (wind)
/// - EEG 2021: ≥4 consecutive hours, exempt <500 kW (all types)
/// - EEG 2023 (or None): any period, exempt <100 kW
fn should_apply_negativpreis(
    kwh_during_negative_epex: Option<Decimal>,
    inbetriebnahme: Option<time::Date>,
    leistung_kwp: Option<Decimal>,
) -> bool {
    use crate::version::EegGesetz;
    let gesetz = inbetriebnahme
        .map(|d| EegGesetz::from_inbetriebnahme_year(d.year()))
        .unwrap_or_default();
    should_apply_negativpreis_versioned(kwh_during_negative_epex, leistung_kwp, gesetz, None)
}

/// Version-aware §51 applicability check.
///
/// Takes typed [`EegGesetz`] and optional [`ErzeugungsArt`].
/// The caller must only pass `kwh_during_negative_epex` after verifying that the
/// version-specific consecutive-hour threshold was met (caller's responsibility).
/// This function enforces only the **kW exemption**.
fn should_apply_negativpreis_versioned(
    kwh_during_negative_epex: Option<Decimal>,
    leistung_kwp: Option<Decimal>,
    eeg_gesetz: crate::version::EegGesetz,
    erzeugungsart: Option<crate::technology::ErzeugungsArt>,
) -> bool {
    if kwh_during_negative_epex.map_or(true, |k| k <= Decimal::ZERO) {
        return false;
    }
    let art = erzeugungsart.unwrap_or(crate::technology::ErzeugungsArt::Solar);
    let Some(threshold_kw) = eeg_gesetz.negativpreis_kw_grenze(&art) else {
        return false; // §51 not applicable for this EEG version
    };
    leistung_kwp.map_or(true, |kw| kw >= Decimal::from(threshold_kw))
}

/// Apply §51 EEG deduction: subtract kWh during negative-price hours.
/// Only called after `should_apply_negativpreis` returns `true`.
fn apply_negativpreis(kwh: Decimal, negative_kwh: Decimal) -> Decimal {
    (kwh - negative_kwh).max(Decimal::ZERO)
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
    // Primary block represented inline if leistung_kwp is set.
    let primary_kwp = input.leistung_kwp.unwrap_or(dec!(1)); // default 1 kWp if not specified
    let additional_total_kwp: Decimal = input.capacity_blocks.iter().map(|b| b.leistung_kwp).sum();
    let total_kwp = primary_kwp + additional_total_kwp;

    let mut positions: Vec<SettlePosition> = Vec::new();
    let mut total_eligible = Decimal::ZERO;

    // ── Primary block ────────────────────────────────────────────────────────
    let primary_expired = billing_date.map_or(false, |d| {
        input.foerderendedatum.map_or(false, |fed| d > fed)
    });
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
            input.eeg_gesetz,
            input.erzeugungsart,
        ) {
            // Proportional share of negative kWh for this block
            let neg_share = input
                .kwh_during_negative_epex
                .map(|n| (n * share).round_dp(3))
                .unwrap_or(Decimal::ZERO);
            block_kwh = apply_negativpreis(block_kwh, neg_share);
        }

        if block_kwh > Decimal::ZERO || input.verguetungssatz_ct != Decimal::ZERO {
            let ibn_label = input
                .inbetriebnahme
                .map(|d| format!(" (IBN {d})"))
                .unwrap_or_default();
            positions.push(pos(
                format!("Einspeiseverg\u{00fc}tung {primary_kwp}\u{202f}kWp-Block{ibn_label}"),
                "\u{00a7}21 EEG",
                block_kwh,
                input.verguetungssatz_ct,
            ));
        }
        total_eligible += block_kwh;
    }

    // ── Additional blocks ────────────────────────────────────────────────────
    for (idx, block) in input.capacity_blocks.iter().enumerate() {
        let block_expired = billing_date.map_or(false, |d| d > block.foerderendedatum);
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
/// use eeg_billing::{SettleInput, SettlementModel, calculate_settlement, SettlementStatus};
/// use rust_decimal::Decimal;
/// use std::str::FromStr;
///
/// fn d(s: &str) -> Decimal { Decimal::from_str(s).unwrap() }
///
/// // §21 EEG 2023 — 100 kWh × 8.11 ct/kWh = 8.11 EUR
/// let out = calculate_settlement(&SettleInput {
///     model: SettlementModel::Verguetung,
///     einspeisemenge_kwh: Some(d("100")),
///     verguetungssatz_ct: d("8.11"),
///     ..SettleInput::default()
/// });
/// assert_eq!(out.status, SettlementStatus::Calculated);
/// assert_eq!(out.settlement_eur, Some(d("8.11")));
/// ```
pub fn calculate_settlement(input: &SettleInput) -> SettleOutput {
    // ── §52 EEG 2023 Pflichtzahlung (computed independently of Vergütung) ─────
    // For EEG 2023 plants: penalty ≠ Vergütung reduction. Computed separately.
    // For old plants (§100 regime): use `sanktion` instead.
    let pflichtzahlung_eur = input
        .pflichtverstoss
        .as_ref()
        .map(crate::foerderdauer::calculate_pflichtzahlung);

    // Delegate to inner function, then inject pflichtzahlung_eur.
    let mut result = calculate_settlement_inner(input);
    result.pflichtzahlung_eur = pflichtzahlung_eur;
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
            };
        }
        Some(SanktionAlt::VerguetungAufMarktwert) => {
            // §52 Abs. 2 EEG ≤2021: verringert sich auf den Monatsmarktwert (EPEX).
            // Same formula as PostEegSpot but within Förderdauer.
            let Some(epex_ct) = input.epex_avg_ct_kwh else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                };
            };
            let Some(kwh) = input.einspeisemenge_kwh else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::NoData,
                    pflichtzahlung_eur: None,
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
    if input.capacity_blocks.is_empty() {
        if let (Some(billing), Some(fed)) = (input.billing_date, input.foerderendedatum) {
            if billing > fed {
                return SettleOutput {
                    settlement_eur: Some(Decimal::ZERO),
                    eligible_kwh: input.einspeisemenge_kwh,
                    positions: vec![],
                    status: SettlementStatus::FoerderungBeendet,
                    pflichtzahlung_eur: None,
                };
            }
        }
    }

    // ── No meter data ─────────────────────────────────────────────────────────
    // §50a FlexibilitaetZuschlag is capacity-based (not kWh-based) — bypass this check.
    let Some(kwh) = input.einspeisemenge_kwh else {
        if input.model == SettlementModel::FlexibilitaetZuschlag {
            // Route to model dispatch with kwh = ZERO (unused for capacity payments)
            let kwh_dummy = Decimal::ZERO;
            let kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
            let rate_eur_per_kw_year = input.verguetungssatz_ct;
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
            };
        }
        return SettleOutput {
            settlement_eur: None,
            eligible_kwh: None,
            positions: vec![],
            status: SettlementStatus::NoData,
            pflichtzahlung_eur: None,
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
        input.eeg_gesetz,
        input.erzeugungsart,
    );
    let neg_kwh = if apply_neg {
        input.kwh_during_negative_epex
    } else {
        None
    };

    match input.model {
        // ── EUR 0 — Eigenverbrauch ────────────────────────────────────────────
        SettlementModel::Eigenverbrauch => SettleOutput {
            settlement_eur: Some(Decimal::ZERO),
            eligible_kwh: Some(kwh),
            positions: vec![],
            status: SettlementStatus::Calculated,
            pflichtzahlung_eur: None,
        },

        // ── §21 EEG — Feste Einspeisevergütung ───────────────────────────────
        SettlementModel::Verguetung => {
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            let desc = if neg_kwh.is_some() {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}27 Negativpreisregel angewendet)"
            } else {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG"
            };
            let positions = vec![pos(
                desc,
                "\u{00a7}21 EEG 2023",
                effective,
                input.verguetungssatz_ct,
            )];
            SettleOutput {
                settlement_eur: total(&positions),
                eligible_kwh: Some(effective),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
            }
        }

        // ── §38a EEG — Mieterstrom ────────────────────────────────────────────
        SettlementModel::Mieterstrom => {
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            let zuschlag = input.mieter_zuschlag_ct.unwrap_or(Decimal::ZERO);
            let base_desc = if neg_kwh.is_some() {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}27 Negativpreisregel angewendet)"
            } else {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG"
            };
            let mut positions = vec![pos(
                base_desc,
                "\u{00a7}21 EEG 2023",
                effective,
                input.verguetungssatz_ct,
            )];
            if zuschlag != Decimal::ZERO {
                positions.push(pos(
                    "Mieterstrom-Zuschlag \u{00a7}38a EEG 2023",
                    "\u{00a7}38a EEG 2023",
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
            }
        }

        // ── §20 EEG — Gleitende Marktprämie ──────────────────────────────────
        // ── §§22a,28 EEG — Ausschreibungsanlagen ─────────────────────────────
        SettlementModel::Direktvermarktung | SettlementModel::Ausschreibung => {
            let (Some(aw_ct), Some(epex_ct)) = (input.direktverm_aw_ct, input.epex_avg_ct_kwh)
            else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                };
            };
            let praemie_ct = (aw_ct - epex_ct).max(Decimal::ZERO);
            let mgmt_ct = resolve_managementpraemie(input.managementpraemie_ct, input.leistung_kwp);

            let (praemie_desc, praemie_basis) = if input.model == SettlementModel::Ausschreibung {
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
            if praemie_ct > Decimal::ZERO || mgmt_ct == Decimal::ZERO {
                positions.push(pos(praemie_desc, praemie_basis, kwh, praemie_ct));
            }
            if mgmt_ct > Decimal::ZERO {
                positions.push(pos(
                    "Managementpr\u{00e4}mie \u{00a7}20 Abs.\u{202f}3 EEG 2023",
                    "\u{00a7}20 Abs. 3 EEG 2023",
                    kwh,
                    mgmt_ct,
                ));
            }
            if positions.is_empty() {
                positions.push(pos(praemie_desc, praemie_basis, kwh, Decimal::ZERO));
            }

            let total_eur = positions.iter().map(|p| p.eur).sum();
            SettleOutput {
                settlement_eur: Some(total_eur),
                eligible_kwh: Some(kwh),
                positions,
                status: SettlementStatus::Calculated,
                pflichtzahlung_eur: None,
            }
        }

        // ── Post-EEG Spot (§21 post-Förderung + §23b cap) ─────────────────────
        // Negative EPEX → negative EUR (plant pays). No floor.
        // §23b EEG 2023: Jahresmarktwert capped at 10 ct/kWh for ausgeförderte Anlagen.
        SettlementModel::PostEegSpot => {
            let Some(epex_ct) = input.epex_avg_ct_kwh else {
                return SettleOutput {
                    settlement_eur: None,
                    eligible_kwh: None,
                    positions: vec![],
                    status: SettlementStatus::PriceMissing,
                    pflichtzahlung_eur: None,
                };
            };
            // §23b EEG 2023: "ab dem Kalenderjahr 2023 höchstens jedoch 10 Cent pro kWh"
            // The cap only applies when EPEX is POSITIVE (the plant gets at most 10 ct).
            // When EPEX is negative, the cap does NOT apply — plant owes the NB.
            let effective_ct = if epex_ct > dec!(10) {
                dec!(10)
            } else {
                epex_ct
            };
            let was_capped = epex_ct > dec!(10);
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
            }
        }

        // ── §7 KWKG 2023 — KWK-Zuschlag ──────────────────────────────────────
        SettlementModel::KwkgZuschlag => {
            use crate::foerderdauer::kwk_eligible_kwh;

            let (eligible, limit_reached) = match (input.kwk_strom_kwh_gesamt, input.kwk_max_kwh) {
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
                input.verguetungssatz_ct,
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
            }
        }

        // ── §50b EEG — Flexibilitätsprämie (bestehende Anlagen) ──────────────
        SettlementModel::Flexibilitaet => {
            let effective = match neg_kwh {
                Some(n) => apply_negativpreis(kwh, n),
                None => kwh,
            };
            let flex_ct = input.flex_praemie_ct_kwh.unwrap_or(Decimal::ZERO);
            let base_desc = if neg_kwh.is_some() {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG (\u{00a7}51 Negativpreisregel angewendet)"
            } else {
                "Einspeiseverg\u{00fc}tung \u{00a7}21 EEG"
            };
            let mut positions = vec![pos(
                base_desc,
                "\u{00a7}21 EEG 2023",
                effective,
                input.verguetungssatz_ct,
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
            }
        }

        // ── §50a EEG 2023 — Flexibilitätszuschlag (neue Anlagen) ─────────────
        // Capacity-based payment: EUR/kW/year (statutory: 100 EUR/kW/year).
        // leistung_kwp = additional flexible capacity in kW.
        // verguetungssatz_ct = annual rate per kW in EUR (100 EUR/kW/year).
        // Monthly = leistung_kwp × rate / 12.
        SettlementModel::FlexibilitaetZuschlag => {
            let kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
            let rate_eur_per_kw_year = input.verguetungssatz_ct; // typically 100 EUR/kW/year
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
            }
        }
    }
}
