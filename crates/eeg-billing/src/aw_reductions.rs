//! §§53b–54 EEG 2023 — reductions that act on the **anzulegender Wert**.
//!
//! # Why these are separate from the §52 pipeline
//!
//! §52 Pflichtzahlungen are a separate monetary obligation that may be netted
//! against the disbursement (§52 Abs. 6). §§53b–54 are not: each one reduces the
//! *anzulegender Wert* itself, before any settlement formula runs. The statutes
//! say so in as many words — §53b: "verringert sich … um 0,1 Cent pro
//! Kilowattstunde"; §53c: "Der anzulegende Wert verringert sich"; §54: "Der …
//! ermittelte anzulegende Wert verringert sich".
//!
//! The distinction is not cosmetic. The gleitende Marktprämie is
//! `max(0, AW + Managementprämie − Marktwert)`, floored at zero. Subtracting a
//! euro amount *after* that floor is a different number from reducing the AW
//! *before* it: once the Marktwert is at or above the AW the premium is already
//! zero, and a post-hoc deduction would push the settlement negative — charging
//! the operator for electricity they fed in. Applying the reduction to the AW
//! lets the floor absorb it, which is what the statute describes.
//!
//! # What each one is
//!
//! | § | Trigger | Amount |
//! |---|---|---|
//! | 53b | A Regionalnachweis (§79a) was issued for the electricity, and the AW is *gesetzlich bestimmt* | −0,1 ct/kWh |
//! | 53c | The electricity is transited through a grid and exempt from Stromsteuer | −the granted exemption per kWh |
//! | 54 | Solar first-segment auction, four distinct defects | −0,3 / −0,3 / −2,5 ct/kWh, or AW → 0 |
//!
//! Legal text: EEG 2023 in the Fassung vom 18.12.2025, in Kraft ab 23.12.2025
//! (Arbeitsausgabe der Clearingstelle EEG|KWKG).

use crate::scheme::TariffSource;
use crate::technology::ErzeugungsArt;
use rust_decimal::Decimal;
use rust_decimal::dec;

/// §53b EEG 2023 — the statutory Regionalnachweis deduction, in ct/kWh.
///
/// "Der anzulegende Wert für Strom, für den dem Anlagenbetreiber ein
/// Regionalnachweis ausgestellt worden ist, verringert sich bei Anlagen, deren
/// anzulegender Wert gesetzlich bestimmt ist, um 0,1 Cent pro Kilowattstunde."
pub const SECT53B_REGIONALNACHWEIS_CT_KWH: Decimal = dec!(0.1);

/// §54 Abs. 1 / Abs. 2 EEG 2023 — 0,3 ct/kWh, in ct/kWh.
pub const SECT54_ABS1_ABS2_CT_KWH: Decimal = dec!(0.3);

/// §54 Abs. 3 EEG 2023 — 2,5 ct/kWh for a missing Agri-PV Nutzungsnachweis.
pub const SECT54_ABS3_CT_KWH: Decimal = dec!(2.5);

/// §3 StromStG — the full electricity-tax rate, 20,50 EUR/MWh = 2,05 ct/kWh.
///
/// The upper bound on a §53c reduction: an exemption cannot exceed the tax.
pub const STROMSTEUER_VOLLSATZ_CT_KWH: Decimal = dec!(2.05);

// ── §54 ───────────────────────────────────────────────────────────────────────

/// §54 EEG 2023 — Verringerung des Zahlungsanspruchs bei Ausschreibungen für
/// Solaranlagen des ersten Segments.
///
/// Applies **only** to solar plants whose AW came from a first-segment tender.
/// Each Absatz is an independent defect; Abs. 1 and Abs. 2 stack.
///
/// Abs. 3 and Abs. 4 are not blanket penalties: Abs. 3 Satz 2/3 make the
/// 2,5 ct deduction lapse for the future once the missing proof is supplied, and
/// retroactively for the periods it is supplied for — so a caller that has
/// received a late Nachweis must clear the flag for those periods rather than
/// carrying it forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sect54SolarReduction {
    /// Abs. 1 — the Zahlungsberechtigung for the Gebotsmenge assigned to this
    /// plant was applied for only after the 18th calendar month following public
    /// announcement of the Zuschlag. −0,3 ct/kWh.
    ///
    /// Satz 2: where several bids feed one plant, only the Zuschlagswert of the
    /// late-assigned bids is reduced. Model that by settling those bid volumes
    /// as their own input rather than blending them.
    pub zahlungsberechtigung_nach_18_monaten: bool,

    /// Abs. 2 — the plant's location does not match, even partly, the Flurstücke
    /// named in the bid. −0,3 ct/kWh.
    pub flurstueck_abweichung: bool,

    /// Abs. 3 — for besondere Solaranlagen under §37 Abs. 1 Nr. 3 Buchst. a
    /// (gleichzeitiger Nutzpflanzenanbau) or Buchst. b/c (gleichzeitige
    /// landwirtschaftliche Nutzung), the proof required by the BNetzA
    /// Festlegung under §85c Abs. 1 Satz 4 was not supplied. −2,5 ct/kWh.
    pub agri_nutzungsnachweis_fehlt: bool,

    /// Abs. 4 — a plant under §37 Abs. 1 Nr. 2 Buchst. h or i whose eligibility
    /// in the Zuschlagsverfahren depended on a Landesverordnung (§37c Abs. 2)
    /// does not meet that Verordnung. The AW reduces **to zero**.
    pub landesverordnung_nicht_erfuellt: bool,
}

impl Sect54SolarReduction {
    /// `true` when no defect is recorded.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        *self == Self::default()
    }
}

// ── The applied-reduction audit record ────────────────────────────────────────

/// One reduction that actually fired, for the settlement's position list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AwReductionApplied {
    /// Human-readable description for the billing position.
    pub description: String,
    /// The § the deduction rests on.
    pub legal_basis: String,
    /// How much the AW was reduced by, in ct/kWh. Positive number.
    pub deduction_ct_kwh: Decimal,
}

/// Everything the AW-level reductions need to decide.
///
/// Grouped into one struct so the settlement engine passes a single value and a
/// new reduction cannot be added without every caller seeing it.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct AwReductionContext {
    /// §53b — a Regionalnachweis under §79a EEG was issued for this electricity.
    pub regionalnachweis_ausgestellt: bool,
    /// §53c — the per-kWh Stromsteuerbefreiung granted for this electricity,
    /// where it is transited through a grid. `None` = no exemption.
    pub stromsteuerbefreiung_ct_kwh: Option<Decimal>,
    /// §54 — solar first-segment auction defects.
    pub sect54_solar: Option<Sect54SolarReduction>,
}

impl AwReductionContext {
    /// `true` when nothing is set, so the engine can skip the whole pass.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.regionalnachweis_ausgestellt
            && self.stromsteuerbefreiung_ct_kwh.is_none()
            && self.sect54_solar.is_none_or(|s| s.is_clean())
    }
}

/// Apply §§53b–54 to an anzulegender Wert.
///
/// Returns the reduced AW — floored at zero, since none of these statutes
/// creates a negative entitlement — together with the list of reductions that
/// fired, so the caller can render one billing position per statute.
///
/// `tariff_source` decides §53b: the statute limits it to plants "deren
/// anzulegender Wert gesetzlich bestimmt ist", which excludes tender-determined
/// values. `art` gates §54, which is a solar-first-segment rule.
#[must_use]
pub fn apply_aw_reductions(
    aw_ct: Decimal,
    ctx: &AwReductionContext,
    tariff_source: &TariffSource,
    art: ErzeugungsArt,
) -> (Decimal, Vec<AwReductionApplied>) {
    let mut aw = aw_ct;
    let mut applied = Vec::new();

    // ── §53b — Regionalnachweise ─────────────────────────────────────────────
    // "bei Anlagen, deren anzulegender Wert gesetzlich bestimmt ist" — a tender
    // award is determined by the auction, not by law, so it is out of scope.
    if ctx.regionalnachweis_ausgestellt && !tariff_source.is_auction() {
        aw -= SECT53B_REGIONALNACHWEIS_CT_KWH;
        applied.push(AwReductionApplied {
            description: "\u{00a7}53b EEG 2023 Regionalnachweis (\u{00a7}79a)".to_owned(),
            legal_basis: "\u{00a7}53b EEG 2023".to_owned(),
            deduction_ct_kwh: SECT53B_REGIONALNACHWEIS_CT_KWH,
        });
    }

    // ── §53c — Stromsteuerbefreiung ──────────────────────────────────────────
    // The reduction is the exemption actually granted, capped at the full §3
    // StromStG rate: an exemption larger than the tax is not a thing, and
    // accepting one would silently invent a deduction.
    if let Some(befreiung) = ctx
        .stromsteuerbefreiung_ct_kwh
        .filter(|c| *c > Decimal::ZERO)
    {
        let deduction = befreiung.min(STROMSTEUER_VOLLSATZ_CT_KWH);
        aw -= deduction;
        applied.push(AwReductionApplied {
            description: format!(
                "\u{00a7}53c EEG 2023 Stromsteuerbefreiung ({deduction}\u{202f}ct/kWh)"
            ),
            legal_basis: "\u{00a7}53c EEG 2023".to_owned(),
            deduction_ct_kwh: deduction,
        });
    }

    // ── §54 — Ausschreibungen für Solaranlagen des ersten Segments ───────────
    if let Some(s54) = ctx.sect54_solar.filter(|s| !s.is_clean() && art.is_solar()) {
        if s54.landesverordnung_nicht_erfuellt {
            // Abs. 4 — "verringert sich der anzulegende Wert auf null". It
            // subsumes the others: there is nothing left for them to reduce.
            let deduction = aw.max(Decimal::ZERO);
            applied.push(AwReductionApplied {
                description: "\u{00a7}54 Abs.\u{202f}4 EEG 2023 Landesverordnung \
                              nicht erf\u{00fc}llt (AW \u{2192} 0)"
                    .to_owned(),
                legal_basis: "\u{00a7}54 Abs. 4 EEG 2023".to_owned(),
                deduction_ct_kwh: deduction,
            });
            return (Decimal::ZERO, applied);
        }
        if s54.zahlungsberechtigung_nach_18_monaten {
            aw -= SECT54_ABS1_ABS2_CT_KWH;
            applied.push(AwReductionApplied {
                description: "\u{00a7}54 Abs.\u{202f}1 EEG 2023 Zahlungsberechtigung erst \
                              nach dem 18.\u{202f}Kalendermonat beantragt"
                    .to_owned(),
                legal_basis: "\u{00a7}54 Abs. 1 EEG 2023".to_owned(),
                deduction_ct_kwh: SECT54_ABS1_ABS2_CT_KWH,
            });
        }
        if s54.flurstueck_abweichung {
            aw -= SECT54_ABS1_ABS2_CT_KWH;
            applied.push(AwReductionApplied {
                description: "\u{00a7}54 Abs.\u{202f}2 EEG 2023 Standort weicht von den \
                              Gebots-Flurst\u{00fc}cken ab"
                    .to_owned(),
                legal_basis: "\u{00a7}54 Abs. 2 EEG 2023".to_owned(),
                deduction_ct_kwh: SECT54_ABS1_ABS2_CT_KWH,
            });
        }
        if s54.agri_nutzungsnachweis_fehlt {
            aw -= SECT54_ABS3_CT_KWH;
            applied.push(AwReductionApplied {
                description: "\u{00a7}54 Abs.\u{202f}3 EEG 2023 Nachweis der gleichzeitigen \
                              landwirtschaftlichen Nutzung fehlt"
                    .to_owned(),
                legal_basis: "\u{00a7}54 Abs. 3 EEG 2023".to_owned(),
                deduction_ct_kwh: SECT54_ABS3_CT_KWH,
            });
        }
    }

    (aw.max(Decimal::ZERO), applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::{AusschreibungMetadata, TariffSource};

    fn auction() -> TariffSource {
        TariffSource::Auction(AusschreibungMetadata::default())
    }

    #[test]
    fn sect53b_deducts_the_statutory_tenth_of_a_cent() {
        let ctx = AwReductionContext {
            regionalnachweis_ausgestellt: true,
            ..AwReductionContext::default()
        };
        let (aw, applied) = apply_aw_reductions(
            dec!(8.11),
            &ctx,
            &TariffSource::Statutory,
            ErzeugungsArt::Solar,
        );
        assert_eq!(
            aw,
            dec!(8.01),
            "0,1 ct/kWh is fixed by §53b, not a parameter"
        );
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].legal_basis, "§53b EEG 2023");
    }

    /// §53b applies only "bei Anlagen, deren anzulegender Wert gesetzlich
    /// bestimmt ist" — a tender award is not.
    #[test]
    fn sect53b_does_not_touch_a_tender_determined_aw() {
        let ctx = AwReductionContext {
            regionalnachweis_ausgestellt: true,
            ..AwReductionContext::default()
        };
        let (aw, applied) = apply_aw_reductions(
            dec!(5.80),
            &ctx,
            &auction(),
            ErzeugungsArt::SolarFreiflaeche,
        );
        assert_eq!(aw, dec!(5.80));
        assert!(applied.is_empty());
    }

    /// §100 transitional plants still have a statutory AW, so §53b reaches them.
    #[test]
    fn sect53b_reaches_transitional_plants() {
        let ctx = AwReductionContext {
            regionalnachweis_ausgestellt: true,
            ..AwReductionContext::default()
        };
        let ts = TariffSource::Transitional(crate::scheme::Paragraph100Rule::OldPlantBeforeEeg2023);
        let (aw, _) = apply_aw_reductions(dec!(9.00), &ctx, &ts, ErzeugungsArt::Solar);
        assert_eq!(aw, dec!(8.90));
    }

    #[test]
    fn sect53c_deducts_the_granted_exemption() {
        let ctx = AwReductionContext {
            stromsteuerbefreiung_ct_kwh: Some(dec!(2.05)),
            ..AwReductionContext::default()
        };
        let (aw, applied) = apply_aw_reductions(
            dec!(8.11),
            &ctx,
            &TariffSource::Statutory,
            ErzeugungsArt::Solar,
        );
        assert_eq!(aw, dec!(6.06));
        assert_eq!(applied[0].legal_basis, "§53c EEG 2023");
    }

    /// An exemption cannot exceed the tax it exempts from (§3 StromStG).
    #[test]
    fn sect53c_is_capped_at_the_full_stromsteuer_rate() {
        let ctx = AwReductionContext {
            stromsteuerbefreiung_ct_kwh: Some(dec!(9.99)),
            ..AwReductionContext::default()
        };
        let (aw, applied) = apply_aw_reductions(
            dec!(8.11),
            &ctx,
            &TariffSource::Statutory,
            ErzeugungsArt::Solar,
        );
        assert_eq!(aw, dec!(6.06));
        assert_eq!(applied[0].deduction_ct_kwh, dec!(2.05));
    }

    /// §54 is a solar first-segment rule; a wind award is out of its scope.
    #[test]
    fn sect54_does_not_reach_wind() {
        let ctx = AwReductionContext {
            sect54_solar: Some(Sect54SolarReduction {
                zahlungsberechtigung_nach_18_monaten: true,
                ..Sect54SolarReduction::default()
            }),
            ..AwReductionContext::default()
        };
        let (aw, applied) =
            apply_aw_reductions(dec!(7.35), &ctx, &auction(), ErzeugungsArt::WindOnshore);
        assert_eq!(aw, dec!(7.35));
        assert!(applied.is_empty());
    }

    /// Abs. 1 and Abs. 2 are independent defects and stack.
    #[test]
    fn sect54_abs1_and_abs2_stack() {
        let ctx = AwReductionContext {
            sect54_solar: Some(Sect54SolarReduction {
                zahlungsberechtigung_nach_18_monaten: true,
                flurstueck_abweichung: true,
                ..Sect54SolarReduction::default()
            }),
            ..AwReductionContext::default()
        };
        let (aw, applied) = apply_aw_reductions(
            dec!(5.80),
            &ctx,
            &auction(),
            ErzeugungsArt::SolarFreiflaeche,
        );
        assert_eq!(aw, dec!(5.20));
        assert_eq!(applied.len(), 2);
    }

    #[test]
    fn sect54_abs3_deducts_two_and_a_half_cents() {
        let ctx = AwReductionContext {
            sect54_solar: Some(Sect54SolarReduction {
                agri_nutzungsnachweis_fehlt: true,
                ..Sect54SolarReduction::default()
            }),
            ..AwReductionContext::default()
        };
        let (aw, _) = apply_aw_reductions(dec!(8.00), &ctx, &auction(), ErzeugungsArt::SolarAgriPv);
        assert_eq!(aw, dec!(5.50));
    }

    /// Abs. 4 zeroes the AW outright and subsumes the other deductions.
    #[test]
    fn sect54_abs4_zeroes_the_aw_and_subsumes_the_rest() {
        let ctx = AwReductionContext {
            sect54_solar: Some(Sect54SolarReduction {
                zahlungsberechtigung_nach_18_monaten: true,
                flurstueck_abweichung: true,
                agri_nutzungsnachweis_fehlt: true,
                landesverordnung_nicht_erfuellt: true,
            }),
            ..AwReductionContext::default()
        };
        let (aw, applied) = apply_aw_reductions(
            dec!(5.80),
            &ctx,
            &auction(),
            ErzeugungsArt::SolarFreiflaeche,
        );
        assert_eq!(aw, Decimal::ZERO);
        assert_eq!(
            applied.len(),
            1,
            "Abs. 4 replaces the others, not adds to them"
        );
        assert_eq!(applied[0].legal_basis, "§54 Abs. 4 EEG 2023");
    }

    /// None of these statutes creates a negative entitlement.
    #[test]
    fn the_reduced_aw_never_goes_negative() {
        let ctx = AwReductionContext {
            regionalnachweis_ausgestellt: true,
            stromsteuerbefreiung_ct_kwh: Some(dec!(2.05)),
            sect54_solar: Some(Sect54SolarReduction {
                agri_nutzungsnachweis_fehlt: true,
                ..Sect54SolarReduction::default()
            }),
        };
        let (aw, _) = apply_aw_reductions(
            dec!(1.00),
            &ctx,
            &TariffSource::Statutory,
            ErzeugungsArt::Solar,
        );
        assert_eq!(aw, Decimal::ZERO);
    }

    #[test]
    fn an_empty_context_is_detected_and_changes_nothing() {
        let ctx = AwReductionContext::default();
        assert!(ctx.is_empty());
        let (aw, applied) = apply_aw_reductions(
            dec!(8.11),
            &ctx,
            &TariffSource::Statutory,
            ErzeugungsArt::Solar,
        );
        assert_eq!(aw, dec!(8.11));
        assert!(applied.is_empty());
    }
}
