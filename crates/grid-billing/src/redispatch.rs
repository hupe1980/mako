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

/// Inputs to the §13a Abs. 2 compensation for one activation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RedispatchVerguetungInput {
    /// Curtailed energy in kWh (Ausfallarbeit). Duldungsfall: measured vs.
    /// reference Lastgang; Aufforderungsfall: from the transmitted schedule.
    pub ausfallarbeit_kwh: Decimal,
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

    let basis = match input.verguetungsart {
        RedispatchVerguetungsart::Eeg => "entgangene EEG-Vergütung (§13a Abs. 2 S. 3 Nr. 5 EnWG)",
        RedispatchVerguetungsart::Kwkg => "entgangene KWKG-Vergütung (§13a Abs. 2 S. 3 Nr. 5 EnWG)",
        RedispatchVerguetungsart::Sonstige => {
            "nachgewiesene entgangene Erlöse (§13a Abs. 2 S. 3 Nr. 3 EnWG)"
        }
    };

    Ok(RedispatchVerguetung {
        ausfallarbeit_kwh: input.ausfallarbeit_kwh,
        verguetungsart: input.verguetungsart,
        entgangene_einnahmen_eur: entgangene,
        zusaetzliche_aufwendungen_eur: zusaetzliche,
        ersparte_aufwendungen_eur: ersparte,
        verguetung_eur: total,
        trace: vec![
            format!("Ausfallarbeit: {} kWh", input.ausfallarbeit_kwh),
            format!("+ {entgangene} € {basis}"),
            format!("+ {zusaetzliche} € zusätzliche Aufwendungen (Nr. 1/2/4)"),
            format!("− {ersparte} € ersparte Aufwendungen (S. 4 — an den NB zu erstatten)"),
            format!("= {total} € angemessene Vergütung (§13a Abs. 2 EnWG)"),
        ],
    })
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
    fn saved_costs_can_exceed_the_claim() {
        // "Weder besser noch schlechter": a thermal plant whose saved fuel
        // exceeds lost revenue owes the difference to the NB.
        let v = redispatch_verguetung(&RedispatchVerguetungInput {
            ausfallarbeit_kwh: dec!(50_000),
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
            verguetungsart: RedispatchVerguetungsart::Kwkg,
            entgangene_einnahmen_eur: dec!(-1),
            zusaetzliche_aufwendungen_eur: Decimal::ZERO,
            ersparte_aufwendungen_eur: Decimal::ZERO,
        });
        assert!(err.is_err());
    }
}
