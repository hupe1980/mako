//! Which rules were in force for a delivery period.
//!
//! German network-charge law is not one timeline but several, each turning over
//! on its own date:
//!
//! | Axis | Turns over | Because |
//! |---|---|---|
//! | Netzzugang | 31.12.2025 | StromNZV and GasNZV lapsed; the competence moved to §20 Abs. 3 EnWG, exercised through BNetzA Festlegungen |
//! | Entgeltbildung Strom | 31.12.2028 | StromNEV and ARegV lapse; the successor is the BNetzA framework Festlegung *AgNeS* |
//! | Entgeltbildung Gas | 31.12.2027 | the GasNEV „tritt gem. Art. 15 Abs. 2 G v. 22.12.2023 I Nr. 405 mit Ablauf des 31.12.2027 außer Kraft" (GasNEV, gesetze-im-internet.de); no successor basis is published |
//! | Dezentrale Erzeugung | 2026–2028 | §18 StromNEV payments are being phased out in quarterly steps (GBK-25-02-1#1) |
//! | Umlagen | annually | the ÜNB publish new rates each October |
//!
//! A settlement that spans one of those dates is governed by different rules at
//! its start and its end, and a settlement produced today for a 2024 period must
//! still be computed under 2024's rules.
//!
//! ## Why the regime is resolved once
//!
//! Scattering `if period_to <= some_date` through the calculation is how a rule
//! change becomes a bug: each site has to be found and each has to agree. Here
//! the dates are read **once**, at the edge, into [`RegulatoryRegime`]; every
//! calculation then matches on an enum. Adding the AgNeS turnover means changing
//! the resolution and exhaustively matching the new variant — the compiler names
//! every site that has to decide.
//!
//! The Entgelt axis is resolved per Sparte: Strom and Gas charges rest on
//! different Verordnungen that lapse on different dates, so a period carries the
//! [`EntgeltAchse`] of its own Sparte.
//!
//! This also makes the regime an *input*, so a settlement can be recomputed
//! under a stated regime rather than under whatever today's calendar implies.

use time::Date;

/// The network-access regime — what governs Mehr-/Mindermengen and the
/// Lieferantenwechsel processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum NetzzugangRegime {
    /// **To 31.12.2025.** StromNZV §13 Abs. 3 (Strom) and GasNZV §25 (Gas).
    ///
    /// Both Verordnungen ceased to have effect with the end of 31.12.2025 —
    /// Art. 15 Abs. 4 (Strom) and Abs. 6 (Gas) of the Gesetz v. 22.12.2023,
    /// BGBl. 2023 I Nr. 405.
    Nzv,
    /// **From 01.01.2026.** §20 Abs. 3 EnWG, exercised through the BNetzA
    /// Festlegungen — GPKE (BK6-24-174) for Strom, GaBi Gas 2.1
    /// (BK7-24-01-008) for Gas.
    EnwgFestlegung,
}

/// The Strom charge-setting regime — what governs how Netzentgelte Strom are
/// formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum EntgeltRegime {
    /// **To 31.12.2028.** Verordnungsrecht: StromNEV / GasNEV together with the
    /// ARegV, and the §19 Abs. 2 individual charges decided by BNetzA
    /// Beschlusskammer 4 (BK4-22-089).
    Verordnung,
    /// **From 01.01.2029.** The BNetzA framework Festlegung *AgNeS*
    /// (Allgemeine Netzentgeltsystematik Strom), which replaces StromNEV and
    /// ARegV when they lapse at the end of 2028.
    ///
    /// Modelled ahead of time so a settlement for a 2029 period is refused
    /// explicitly rather than computed under rules that no longer exist —
    /// [`RegulatoryRegime::ensure_berechenbar`] is that refusal, and every
    /// builder that prices charges on this axis calls it
    /// ([`crate::billing::settle_nne`], [`crate::billing::settle_gas_awh`]).
    /// Builders whose charges are *not* formed on this axis are exempt and say
    /// so where they resolve their regime: Mehr-/Mindermengen (Netzzugang
    /// axis), Messstellenbetrieb (MsbG), §18 dezentrale Erzeugung (its own
    /// sunset, GBK-25-02-1#1) and reversals (pure mirrors of an
    /// already-computed settlement).
    ///
    /// The substantive methodology is not implemented: the framework
    /// Festlegung was still in consultation when this was written.
    AgNeS,
}

/// The Gas charge-setting regime — what governs how Netzentgelte Gas are formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum GasEntgeltRegime {
    /// **To 31.12.2027.** The GasNEV (§ 15 Netzentgelte) together with the ARegV.
    GasNev,
    /// **From 01.01.2028.** The GasNEV „tritt gem. Art. 15 Abs. 2 G v.
    /// 22.12.2023 I Nr. 405 mit Ablauf des 31.12.2027 außer Kraft"
    /// (gesetze-im-internet.de, GasNEV header), and no successor basis for the
    /// Gas Netzentgelte is published. [`RegulatoryRegime::ensure_berechenbar`]
    /// refuses a period on this axis rather than pricing it under a repealed
    /// § 15.
    OhneGrundlage,
}

/// The Entgelt axis of one Sparte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum EntgeltAchse {
    /// StromNEV / ARegV, then AgNeS.
    Strom(EntgeltRegime),
    /// GasNEV / ARegV, then no published basis.
    Gas(GasEntgeltRegime),
}

/// The rules in force for one delivery period.
///
/// Construct with [`RegulatoryRegime::for_period`] and pass it down; do not
/// re-derive it from dates further in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RegulatoryRegime {
    netzzugang: NetzzugangRegime,
    entgelt: EntgeltAchse,
    /// The calendar year the annual rates are taken from.
    tarifjahr: i32,
}

/// Last day on which StromNZV and GasNZV applied.
const NZV_LETZTER_TAG: Date = time::macros::date!(2025 - 12 - 31);

/// Last day on which StromNEV and ARegV apply.
const VERORDNUNG_LETZTER_TAG: Date = time::macros::date!(2028 - 12 - 31);

/// Last day on which the GasNEV applies (Art. 15 Abs. 2 G v. 22.12.2023,
/// BGBl. 2023 I Nr. 405).
pub const GASNEV_LETZTER_TAG: Date = time::macros::date!(2027 - 12 - 31);

/// §19 Abs. 3 StromNEV (Sondernetzentgelte für singulär genutzte
/// Betriebsmittel) no longer applies from 01.01.2026 (BK8-25-003-A,
/// 16.09.2025) — this crate has never emitted an Abs.-3 position.
///
/// Transition: Netznutzer that are **not** operators of general-supply
/// networks keep their entitlement through 31.12.2028; the individual
/// §19 Abs. 2 forms (atypische Netznutzung / Bandlast, implemented in
/// [`crate::sect19`]) are unaffected by BK8-25-003-A. The AgNes successor
/// system (GBK-25-01: Kapazitätspreis for Großverbraucher, generator
/// capacity charge of 4–7 €/kW/a with 20-year Bestandsschutz, from
/// 01.01.2029) is **draft only** — the Rahmenfestlegung is expected end-2026,
/// so [`EntgeltRegime::AgNeS`] carries no parameter tables yet; model AgNes
/// prices as configuration when they become binding.
pub const SECT19_ABS3_LETZTER_TAG: Date = time::macros::date!(2025 - 12 - 31);

/// End of the §19 Abs. 3 transition window for non-network-operator
/// Netznutzer (BK8-25-003-A Tenor 2).
pub const SECT19_ABS3_UEBERGANG_ENDE: Date = time::macros::date!(2028 - 12 - 31);

impl RegulatoryRegime {
    /// Resolve the regime governing a delivery period.
    ///
    /// A period is governed by the rules in force **at its end**: the settlement
    /// documents a supply that has finished, and it is the completed supply that
    /// the current rules attach to. A period straddling a turnover is reported
    /// by [`Self::straddles_turnover`] so the caller can split it rather than
    /// silently pick one side.
    #[must_use]
    pub fn for_period(sparte: crate::types::Sparte, period_from: Date, period_to: Date) -> Self {
        Self {
            netzzugang: if period_to <= NZV_LETZTER_TAG {
                NetzzugangRegime::Nzv
            } else {
                NetzzugangRegime::EnwgFestlegung
            },
            entgelt: match sparte {
                crate::types::Sparte::Strom => {
                    EntgeltAchse::Strom(if period_to <= VERORDNUNG_LETZTER_TAG {
                        EntgeltRegime::Verordnung
                    } else {
                        EntgeltRegime::AgNeS
                    })
                }
                crate::types::Sparte::Gas => {
                    EntgeltAchse::Gas(if period_to <= GASNEV_LETZTER_TAG {
                        GasEntgeltRegime::GasNev
                    } else {
                        GasEntgeltRegime::OhneGrundlage
                    })
                }
            },
            tarifjahr: period_from.year(),
        }
    }

    /// Build a regime explicitly, for recomputing a settlement under stated rules.
    ///
    /// Use this to reproduce a historical settlement exactly, rather than letting
    /// today's calendar decide what applied then.
    #[must_use]
    pub const fn new(netzzugang: NetzzugangRegime, entgelt: EntgeltAchse, tarifjahr: i32) -> Self {
        Self {
            netzzugang,
            entgelt,
            tarifjahr,
        }
    }

    /// The network-access regime.
    #[must_use]
    pub const fn netzzugang(&self) -> NetzzugangRegime {
        self.netzzugang
    }

    /// The charge-setting regime of the period's Sparte.
    #[must_use]
    pub const fn entgelt(&self) -> EntgeltAchse {
        self.entgelt
    }

    /// The calendar year annual rates are taken from.
    #[must_use]
    pub const fn tarifjahr(&self) -> i32 {
        self.tarifjahr
    }

    /// Refuse to price Netzentgelte under a regime whose methodology does not
    /// exist yet, or whose Verordnung is außer Kraft.
    ///
    /// The AgNeS successor system (GBK-25-01) is draft only and has no binding
    /// parameter tables, so a Strom settlement whose Entgelt axis resolves to
    /// [`EntgeltRegime::AgNeS`] is refused rather than computed with the lapsed
    /// Verordnung math and merely tagged. A Gas settlement after the GasNEV
    /// lapses ([`GasEntgeltRegime::OhneGrundlage`]) is refused the same way.
    ///
    /// Builders whose charges are not formed on the Entgelt axis do not call
    /// this — see the exemption comments at [`crate::billing::settle_mmm`],
    /// [`crate::billing::settle_msb`], [`crate::sect18`] and
    /// [`crate::billing::reverse`].
    ///
    /// # Errors
    ///
    /// [`crate::BillingError::UnsupportedEntgeltRegime`] when the Entgelt axis
    /// is [`EntgeltRegime::AgNeS`];
    /// [`crate::BillingError::GasNevAusserKraft`] when it is
    /// [`GasEntgeltRegime::OhneGrundlage`].
    pub fn ensure_berechenbar(&self) -> Result<(), crate::error::BillingError> {
        match self.entgelt {
            EntgeltAchse::Strom(EntgeltRegime::Verordnung)
            | EntgeltAchse::Gas(GasEntgeltRegime::GasNev) => Ok(()),
            EntgeltAchse::Strom(EntgeltRegime::AgNeS) => {
                Err(crate::error::BillingError::UnsupportedEntgeltRegime {
                    tarifjahr: self.tarifjahr,
                })
            }
            EntgeltAchse::Gas(GasEntgeltRegime::OhneGrundlage) => {
                Err(crate::error::BillingError::GasNevAusserKraft {
                    tarifjahr: self.tarifjahr,
                })
            }
        }
    }

    /// `true` when the period crosses a regime turnover.
    ///
    /// Such a period is governed by different rules at its start and its end, so
    /// a single settlement over it applies the wrong rules to part of the
    /// supply. The caller should split the period at the turnover.
    #[must_use]
    pub fn straddles_turnover(
        sparte: crate::types::Sparte,
        period_from: Date,
        period_to: Date,
    ) -> bool {
        let start = Self::for_period(sparte, period_from, period_from);
        let end = Self::for_period(sparte, period_to, period_to);
        start.netzzugang != end.netzzugang || start.entgelt != end.entgelt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Sparte;
    use time::macros::date;

    /// The NZVs governed periods ending on or before 31.12.2025.
    #[test]
    fn the_nzv_regime_ends_with_2025() {
        let last = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2025 - 12 - 01),
            date!(2025 - 12 - 31),
        );
        assert_eq!(last.netzzugang(), NetzzugangRegime::Nzv);

        let first_after = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2026 - 01 - 01),
            date!(2026 - 01 - 31),
        );
        assert_eq!(first_after.netzzugang(), NetzzugangRegime::EnwgFestlegung);
    }

    /// StromNEV and ARegV govern through 2028; AgNeS takes over after.
    #[test]
    fn the_verordnung_regime_ends_with_2028() {
        let last = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2028 - 12 - 01),
            date!(2028 - 12 - 31),
        );
        assert_eq!(
            last.entgelt(),
            EntgeltAchse::Strom(EntgeltRegime::Verordnung)
        );

        let first_after = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2029 - 01 - 01),
            date!(2029 - 01 - 31),
        );
        assert_eq!(
            first_after.entgelt(),
            EntgeltAchse::Strom(EntgeltRegime::AgNeS)
        );
    }

    /// The GasNEV lapses a year before StromNEV and ARegV, so the Gas axis turns
    /// over at the end of 2027 while the Strom axis stays on the Verordnung.
    #[test]
    fn the_gasnev_regime_ends_with_2027() {
        let last =
            RegulatoryRegime::for_period(Sparte::Gas, date!(2027 - 12 - 01), date!(2027 - 12 - 31));
        assert_eq!(last.entgelt(), EntgeltAchse::Gas(GasEntgeltRegime::GasNev));
        assert!(last.ensure_berechenbar().is_ok());

        let after =
            RegulatoryRegime::for_period(Sparte::Gas, date!(2028 - 01 - 01), date!(2028 - 01 - 31));
        assert_eq!(
            after.entgelt(),
            EntgeltAchse::Gas(GasEntgeltRegime::OhneGrundlage)
        );
        assert!(matches!(
            after.ensure_berechenbar(),
            Err(crate::error::BillingError::GasNevAusserKraft { tarifjahr: 2028 })
        ));

        let strom = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2028 - 01 - 01),
            date!(2028 - 01 - 31),
        );
        assert_eq!(
            strom.entgelt(),
            EntgeltAchse::Strom(EntgeltRegime::Verordnung)
        );

        assert!(RegulatoryRegime::straddles_turnover(
            Sparte::Gas,
            date!(2027 - 12 - 15),
            date!(2028 - 01 - 15)
        ));
        assert!(!RegulatoryRegime::straddles_turnover(
            Sparte::Strom,
            date!(2027 - 12 - 15),
            date!(2028 - 01 - 15)
        ));
    }

    /// A period ending after a turnover is governed by the later rules.
    #[test]
    fn a_period_is_governed_by_the_rules_at_its_end() {
        let straddling = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2025 - 12 - 15),
            date!(2026 - 01 - 15),
        );
        assert_eq!(straddling.netzzugang(), NetzzugangRegime::EnwgFestlegung);
    }

    /// …and a straddling period is reported, so it can be split rather than
    /// half-billed under rules that did not apply.
    #[test]
    fn a_straddling_period_is_detected() {
        assert!(RegulatoryRegime::straddles_turnover(
            Sparte::Strom,
            date!(2025 - 12 - 15),
            date!(2026 - 01 - 15)
        ));
        assert!(RegulatoryRegime::straddles_turnover(
            Sparte::Strom,
            date!(2028 - 12 - 15),
            date!(2029 - 01 - 15)
        ));
        assert!(!RegulatoryRegime::straddles_turnover(
            Sparte::Strom,
            date!(2026 - 01 - 01),
            date!(2026 - 12 - 31)
        ));
    }

    /// An explicit regime reproduces a historical settlement regardless of today.
    #[test]
    fn an_explicit_regime_overrides_the_calendar() {
        let historical = RegulatoryRegime::new(
            NetzzugangRegime::Nzv,
            EntgeltAchse::Strom(EntgeltRegime::Verordnung),
            2024,
        );
        assert_eq!(historical.netzzugang(), NetzzugangRegime::Nzv);
        assert_eq!(historical.tarifjahr(), 2024);
    }

    /// The Tarifjahr follows the period's start — annual rates attach to the
    /// year the supply began, not to when the settlement was produced.
    #[test]
    fn the_tarifjahr_follows_the_period_start() {
        let r = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2026 - 03 - 01),
            date!(2026 - 03 - 31),
        );
        assert_eq!(r.tarifjahr(), 2026);
    }

    /// The AgNeS refusal is real: a regime on the AgNeS Entgelt axis cannot be
    /// priced, and the error names the Festlegung and why nothing is computable.
    #[test]
    fn an_agnes_regime_is_not_berechenbar() {
        let last_verordnung = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2028 - 12 - 01),
            date!(2028 - 12 - 31),
        );
        assert!(last_verordnung.ensure_berechenbar().is_ok());

        let agnes = RegulatoryRegime::for_period(
            Sparte::Strom,
            date!(2029 - 01 - 01),
            date!(2029 - 01 - 31),
        );
        let err = agnes
            .ensure_berechenbar()
            .expect_err("an AgNeS period must be refused");
        assert!(matches!(
            err,
            crate::error::BillingError::UnsupportedEntgeltRegime { tarifjahr: 2029 }
        ));
        let msg = err.to_string();
        assert!(msg.contains("GBK-25-01"), "must name the Festlegung: {msg}");
        assert!(
            msg.contains("nicht") || msg.contains("not yet festgelegt"),
            "must say the parameter tables are not yet festgelegt: {msg}"
        );
    }

    /// The regulatory surface is re-exported at the crate root — resolving
    /// these paths is the (compile-time) proof.
    #[test]
    fn the_regulatory_surface_is_re_exported_at_the_crate_root() {
        use crate::{
            EntgeltRegime as ReexportedEntgelt, NetzzugangRegime as ReexportedNetzzugang,
            RegulatoryRegime as ReexportedRegime, SECT19_ABS3_LETZTER_TAG,
            SECT19_ABS3_UEBERGANG_ENDE,
        };
        let r = ReexportedRegime::new(
            ReexportedNetzzugang::EnwgFestlegung,
            crate::EntgeltAchse::Strom(ReexportedEntgelt::Verordnung),
            2026,
        );
        assert_eq!(r.tarifjahr(), 2026);
        assert!(SECT19_ABS3_LETZTER_TAG < SECT19_ABS3_UEBERGANG_ENDE);
    }
}
