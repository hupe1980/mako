//! §13a EnWG Redispatch 2.0 compensation (angemessene Vergütung).
//!
//! §13a Abs. 2 EnWG: the plant operator affected by a redispatch measure is
//! left "wirtschaftlich weder besser noch schlechter" — the compensation is
//!
//! ```text
//! Vergütung = zusätzliche Aufwendungen        (Abs. 2 Satz 3 Nr. 1, 2, 4)
//!           + entgangene Einnahmen            (Nr. 3; Nr. 5 for EEG/KWKG)
//!           − ersparte Aufwendungen           (Satz 4 — reimbursed to the NB)
//! ```
//!
//! The `Verguetungsart` from the Redispatch Stammdaten (Z01 EEG / Z02 KWKG /
//! Z03 sonstige) decides how the *entgangene Einnahmen* basis is formed: for
//! EEG/KWKG plants it is the lost statutory remuneration for the
//! Ausfallarbeit; for other plants the proven lost market revenue.
//!
//! This module is the pure arithmetic — deterministic, Decimal-only, with a
//! per-component trace. Data acquisition (Ausfallarbeit from measured vs.
//! reference Lastgang in the Duldungsfall, from the transmitted schedule in
//! the Aufforderungsfall) and the payment run live in the service layer.

use billing::EuroAmount;
use rust_decimal::Decimal;

use crate::error::BillingError;

/// Vergütungsart of the affected resource (Redispatch Stammdaten field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RedispatchVerguetungsart {
    /// Z01 — EEG plant: entgangene Einnahmen = lost EEG remuneration.
    Eeg,
    /// Z02 — KWKG plant: lost KWKG remuneration (incl. heat-side effects as
    /// zusätzliche Aufwendungen).
    Kwkg,
    /// Z03 — other: proven lost market revenue.
    Sonstige,
}

/// Which §13a Abs. 2 basis the Ausfallarbeit was established on.
///
/// The two redispatch cases do not measure the curtailed energy the same way,
/// and the difference is money:
///
/// - **Duldungsfall** — the Netzbetreiber steers the resource itself, so what
///   the plant *would* have produced is not transmitted anywhere. The
///   Ausfallarbeit is derived from the measured Lastgang against a reference.
/// - **Aufforderungsfall** — the Einsatzverantwortliche steers to a transmitted
///   schedule, and that schedule *is* the counterfactual. Deriving it from the
///   Lastgang instead would settle against what happened rather than against
///   what was instructed.
///
/// This is carried on the input so the basis is stated rather than assumed: a
/// compensation computed on the wrong basis is a plain money error against
/// either the operator or the network, and nothing downstream can tell.
///
/// It mirrors `mako_redispatch::aktivierung::Abwicklung` without depending on
/// it — this crate settles, it does not run the activation workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AusfallarbeitBasis {
    /// Duldungsfall — measured Lastgang against a reference.
    GemessenerLastgang,
    /// Aufforderungsfall — the schedule transmitted to the EIV.
    UebermittelterFahrplan,
}

impl AusfallarbeitBasis {
    /// The §13a wording this basis rests on, for the calculation trace.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GemessenerLastgang => {
                "Duldungsfall — Ausfallarbeit aus gemessenem Lastgang (§13a Abs. 2 EnWG)"
            }
            Self::UebermittelterFahrplan => {
                "Aufforderungsfall — Ausfallarbeit aus übermitteltem Fahrplan (§13a Abs. 2 EnWG)"
            }
        }
    }
}

/// Inputs to the §13a Abs. 2 compensation for one activation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RedispatchVerguetungInput {
    /// Curtailed energy in kWh (Ausfallarbeit).
    pub ausfallarbeit_kwh: Decimal,
    /// How that figure was established — see [`AusfallarbeitBasis`].
    ///
    /// Required rather than defaulted: the two redispatch cases use different
    /// counterfactuals, and picking one silently misstates the compensation.
    pub basis: AusfallarbeitBasis,
    /// The resource's Vergütungsart (Stammdaten Z01/Z02/Z03).
    pub verguetungsart: RedispatchVerguetungsart,
    /// Entgangene Einnahmen in EUR (Abs. 2 Satz 3 Nr. 3 / Nr. 5).
    /// For EEG plants use [`eeg_entgangene_einnahmen`].
    pub entgangene_einnahmen_eur: Decimal,
    /// Zusätzliche Aufwendungen in EUR (Nr. 1: required expenses of the
    /// adjustment; Nr. 2: wear; Nr. 4: readiness/postponed maintenance).
    pub zusaetzliche_aufwendungen_eur: Decimal,
    /// Ersparte Aufwendungen in EUR (Satz 4) — fuel not burnt, avoided
    /// Netzentgelte; reimbursed to the Netzbetreiber.
    pub ersparte_aufwendungen_eur: Decimal,
}

/// The computed compensation with its component breakdown.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RedispatchVerguetung {
    /// Curtailed energy this compensation covers (kWh).
    pub ausfallarbeit_kwh: Decimal,
    /// How that figure was established — carried through so an audit can see
    /// which counterfactual the compensation rests on.
    pub basis: AusfallarbeitBasis,
    /// Vergütungsart the entgangene-Einnahmen basis was formed under.
    pub verguetungsart: RedispatchVerguetungsart,
    /// Entgangene Einnahmen component, cent-rounded (Nr. 3 / Nr. 5).
    pub entgangene_einnahmen_eur: Decimal,
    /// Zusätzliche Aufwendungen component, cent-rounded (Nr. 1/2/4).
    pub zusaetzliche_aufwendungen_eur: Decimal,
    /// Ersparte Aufwendungen component, cent-rounded (Satz 4).
    pub ersparte_aufwendungen_eur: Decimal,
    /// `entgangene + zusätzliche − ersparte`, rounded to cents (half away
    /// from zero). **May be negative**: §13a Abs. 2 Satz 4 obliges the
    /// operator to reimburse saved costs even beyond the claim — "weder
    /// besser noch schlechter" cuts both ways.
    pub verguetung_eur: Decimal,
    /// Human-readable derivation, one line per component.
    pub trace: Vec<String>,
}

/// Entgangene EEG-Einnahmen for the Ausfallarbeit:
/// `kWh × anzulegender Wert (ct/kWh) ÷ 100`, cent-rounded.
///
/// The anzulegender Wert is the plant's EEG rate (its `eeg-billing`
/// settlement scheme provides it); §13a Abs. 2 Satz 3 Nr. 5 makes the lost
/// statutory remuneration the compensation basis for EEG plants.
#[must_use]
pub fn eeg_entgangene_einnahmen(
    ausfallarbeit_kwh: Decimal,
    anzulegender_wert_ct: Decimal,
) -> Decimal {
    (ausfallarbeit_kwh * anzulegender_wert_ct / Decimal::ONE_HUNDRED)
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

/// Compute the §13a Abs. 2 EnWG compensation for one redispatch activation.
///
/// # Errors
///
/// Rejects negative component inputs — each component is a magnitude; the
/// only signed quantity is the resulting net compensation.
pub fn redispatch_verguetung(
    input: &RedispatchVerguetungInput,
) -> Result<RedispatchVerguetung, BillingError> {
    for (label, v) in [
        ("ausfallarbeit_kwh", input.ausfallarbeit_kwh),
        ("entgangene_einnahmen_eur", input.entgangene_einnahmen_eur),
        (
            "zusaetzliche_aufwendungen_eur",
            input.zusaetzliche_aufwendungen_eur,
        ),
        ("ersparte_aufwendungen_eur", input.ersparte_aufwendungen_eur),
    ] {
        if v < Decimal::ZERO {
            return Err(BillingError::InvalidInput {
                reason: format!("§13a component {label} must be non-negative, got {v}"),
            });
        }
    }

    let round = |d: Decimal| {
        d.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    };
    let entgangene = round(input.entgangene_einnahmen_eur);
    let zusaetzliche = round(input.zusaetzliche_aufwendungen_eur);
    let ersparte = round(input.ersparte_aufwendungen_eur);
    let total = entgangene + zusaetzliche - ersparte;

    // Same money boundary as the settle_* functions: every EUR result must be
    // representable as an EuroAmount before it leaves the crate.
    for v in [entgangene, zusaetzliche, ersparte, total] {
        let _representable =
            EuroAmount::checked_from_decimal(v).map_err(|_| BillingError::MonetaryOverflow {
                input_value: Some(v),
            })?;
    }

    let basis = match input.verguetungsart {
        RedispatchVerguetungsart::Eeg => "entgangene EEG-Vergütung (§13a Abs. 2 S. 3 Nr. 5 EnWG)",
        RedispatchVerguetungsart::Kwkg => "entgangene KWKG-Vergütung (§13a Abs. 2 S. 3 Nr. 5 EnWG)",
        RedispatchVerguetungsart::Sonstige => {
            "nachgewiesene entgangene Erlöse (§13a Abs. 2 S. 3 Nr. 3 EnWG)"
        }
    };

    Ok(RedispatchVerguetung {
        ausfallarbeit_kwh: input.ausfallarbeit_kwh,
        basis: input.basis,
        verguetungsart: input.verguetungsart,
        entgangene_einnahmen_eur: entgangene,
        zusaetzliche_aufwendungen_eur: zusaetzliche,
        ersparte_aufwendungen_eur: ersparte,
        verguetung_eur: total,
        trace: vec![
            format!("Ausfallarbeit: {} kWh", input.ausfallarbeit_kwh),
            input.basis.label().to_owned(),
            format!("+ {entgangene} € {basis}"),
            format!("+ {zusaetzliche} € zusätzliche Aufwendungen (Nr. 1/2/4)"),
            format!("− {ersparte} € ersparte Aufwendungen (S. 4 — an den NB zu erstatten)"),
            format!("= {total} € angemessene Vergütung (§13a Abs. 2 EnWG)"),
        ],
    })
}

/// BilAReM financial correction for fluctuating plants in the Planwertmodell
/// (BK6-23-241, BilAReM Kap. 4): the residual between actual Ausfallarbeit and
/// the plan-based bilanzieller Ausgleich is settled **financially only** —
/// no ex-post energy correction:
///
/// `Korr_fin = (W_A − W_Ausgl) / 1000 × ID-AEP`
///
/// with `W_A`/`W_Ausgl` in kWh per quarter-hour and the Intraday-
/// Auktionspreis (`ID-AEP`, fallback ID1/EPEX) in EUR/MWh. A positive result
/// is owed to the Anlagenbetreiber-side Bilanzkreis, a negative one to the
/// Netzbetreiber.
///
/// # Errors
///
/// Rejects non-finite arithmetic via the shared money boundary (result must
/// round to a valid EUR amount).
pub fn bilarem_finanzielle_korrektur(
    ausfallarbeit_kwh: Decimal,
    ausgleich_kwh: Decimal,
    id_aep_eur_per_mwh: Decimal,
) -> Result<Decimal, BillingError> {
    let korr = (ausfallarbeit_kwh - ausgleich_kwh) / Decimal::from(1000) * id_aep_eur_per_mwh;
    let rounded =
        korr.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    // Money boundary: must be representable as EUR cents.
    if rounded.abs() > Decimal::from(10_000_000) {
        return Err(BillingError::InvalidInput {
            reason: format!("BilAReM Korr_fin out of range: {rounded}"),
        });
    }
    Ok(rounded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn eeg_plant_compensation_from_the_anzulegender_wert() {
        // 12 500 kWh curtailed at 7.30 ct/kWh anzulegender Wert.
        let entgangene = eeg_entgangene_einnahmen(dec!(12_500), dec!(7.30));
        assert_eq!(entgangene, dec!(912.50));

        let v = redispatch_verguetung(&RedispatchVerguetungInput {
            ausfallarbeit_kwh: dec!(12_500),
            basis: AusfallarbeitBasis::GemessenerLastgang,
            verguetungsart: RedispatchVerguetungsart::Eeg,
            entgangene_einnahmen_eur: entgangene,
            zusaetzliche_aufwendungen_eur: dec!(40),
            ersparte_aufwendungen_eur: dec!(12.50),
        })
        .unwrap();
        assert_eq!(v.verguetung_eur, dec!(940.00));
        assert!(v.trace.iter().any(|l| l.contains("Nr. 5")));
    }

    #[test]
    fn bilarem_korrektur_settles_the_residual_financially() {
        // W_A 1200 kWh vs. plan-based Ausgleich 1000 kWh at ID-AEP 80 EUR/MWh:
        // (1200 − 1000)/1000 × 80 = 16.00 EUR to the Anlagenbetreiber side.
        let k = bilarem_finanzielle_korrektur(dec!(1200), dec!(1000), dec!(80)).unwrap();
        assert_eq!(k, dec!(16.00));
        // Overshoot of the Ausgleich flows back to the NB (negative).
        let k = bilarem_finanzielle_korrektur(dec!(800), dec!(1000), dec!(80)).unwrap();
        assert_eq!(k, dec!(-16.00));
        // Negative ID-AEP inverts the direction — no clamping.
        let k = bilarem_finanzielle_korrektur(dec!(1200), dec!(1000), dec!(-50)).unwrap();
        assert_eq!(k, dec!(-10.00));
    }

    #[test]
    fn saved_costs_can_exceed_the_claim() {
        // "Weder besser noch schlechter": a thermal plant whose saved fuel
        // exceeds lost revenue owes the difference to the NB.
        let v = redispatch_verguetung(&RedispatchVerguetungInput {
            ausfallarbeit_kwh: dec!(50_000),
            basis: AusfallarbeitBasis::GemessenerLastgang,
            verguetungsart: RedispatchVerguetungsart::Sonstige,
            entgangene_einnahmen_eur: dec!(2_000),
            zusaetzliche_aufwendungen_eur: dec!(100),
            ersparte_aufwendungen_eur: dec!(2_500),
        })
        .unwrap();
        assert_eq!(v.verguetung_eur, dec!(-400.00));
    }

    #[test]
    fn negative_components_are_rejected() {
        let err = redispatch_verguetung(&RedispatchVerguetungInput {
            ausfallarbeit_kwh: dec!(100),
            basis: AusfallarbeitBasis::GemessenerLastgang,
            verguetungsart: RedispatchVerguetungsart::Kwkg,
            entgangene_einnahmen_eur: dec!(-1),
            zusaetzliche_aufwendungen_eur: Decimal::ZERO,
            ersparte_aufwendungen_eur: Decimal::ZERO,
        });
        assert!(err.is_err());
    }
}

#[cfg(test)]
mod basis_tests {
    use super::*;
    use rust_decimal::dec;

    fn input(basis: AusfallarbeitBasis) -> RedispatchVerguetungInput {
        RedispatchVerguetungInput {
            ausfallarbeit_kwh: dec!(1000),
            basis,
            verguetungsart: RedispatchVerguetungsart::Eeg,
            entgangene_einnahmen_eur: dec!(80),
            zusaetzliche_aufwendungen_eur: dec!(10),
            ersparte_aufwendungen_eur: dec!(5),
        }
    }

    /// The basis travels into the result and its trace, so an audit can see
    /// which counterfactual the compensation rests on.
    ///
    /// §13a Abs. 2 measures the curtailed energy differently per case, and the
    /// two produce different figures for the same activation. A compensation
    /// that does not say which one it used cannot be checked.
    #[test]
    fn the_basis_is_carried_into_the_result_and_the_trace() {
        for basis in [
            AusfallarbeitBasis::GemessenerLastgang,
            AusfallarbeitBasis::UebermittelterFahrplan,
        ] {
            let v = redispatch_verguetung(&input(basis)).expect("computes");
            assert_eq!(v.basis, basis);
            assert!(
                v.trace.iter().any(|l| l == basis.label()),
                "the trace must name the basis: {:?}",
                v.trace
            );
        }
    }

    /// The labels name the case and the paragraph — they are read by auditors,
    /// not only by code.
    #[test]
    fn the_labels_name_the_case_and_the_paragraph() {
        assert!(
            AusfallarbeitBasis::GemessenerLastgang
                .label()
                .contains("Duldungsfall")
        );
        assert!(
            AusfallarbeitBasis::UebermittelterFahrplan
                .label()
                .contains("Aufforderungsfall")
        );
        for b in [
            AusfallarbeitBasis::GemessenerLastgang,
            AusfallarbeitBasis::UebermittelterFahrplan,
        ] {
            assert!(b.label().contains("§13a Abs. 2 EnWG"), "{}", b.label());
        }
    }

    /// The arithmetic itself does not change with the basis — only the input
    /// figure does. Making the basis alter the sum would double-count the
    /// distinction.
    #[test]
    fn the_basis_does_not_change_the_arithmetic() {
        let a = redispatch_verguetung(&input(AusfallarbeitBasis::GemessenerLastgang)).unwrap();
        let b = redispatch_verguetung(&input(AusfallarbeitBasis::UebermittelterFahrplan)).unwrap();
        assert_eq!(a.verguetung_eur, b.verguetung_eur);
        assert_eq!(a.verguetung_eur, dec!(85));
    }
}
