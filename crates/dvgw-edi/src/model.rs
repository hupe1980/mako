//! The typed message model, shared by all four DVGW families.
//!
//! ALOCAT, NOMINT, NOMRES and SSQNOT have the same shape — they differ in
//! which qualifiers are legal, not in structure — so one model serves all
//! four and the per-family rules live in the validation layer.
//!
//! ```text
//! BGM DTM×3 RFF+ NAD+MS NAD+MR
//! └─ LIN                          ← LineItem (Positionsnummer)
//!    ├─ IMD                       ← NOMRES: nominated / counterparty / matched
//!    ├─ LOC                       ← LocationGroup, repeats
//!    │  ├─ DTM+2                  ← period for the quantity that follows
//!    │  └─ QTY (+STS)             ← Quantity; STS = Zeitreihentyp (ALOCAT) / Verfahren (SSQNOT)
//!    └─ NAD+ZEU / NAD+ZSH / …     ← Bilanzkreis, Netzkonto, VHP
//! ```
//!
//! The DVGW column of every Nachrichtenstruktur caps `DTM+2` and `SG37 QTY`
//! at **one per `LOC` group**, so a profile is a run of `LOC` groups, one per
//! period. The reader still keeps every `QTY` it meets under a `LOC` — a
//! counterparty that packs a series under one `LOC` loses nothing — and
//! validation reports the excess.

use rust_decimal::Decimal;

use crate::datetime::DvgwPeriod;

/// A party from a `NAD` segment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Party {
    /// DE 3035 party function qualifier — `MS`, `MR`, `ZEU`, `ZES`, `ZSZ`, …
    pub role: String,
    /// C082 DE 3039 party identifier (DVGW code, GLN or EIC).
    pub id: String,
    /// C082 DE 3055 code-list responsible agency — `332` (DVGW), `9` (GS1),
    /// `305` (ETSO/EIC).
    pub agency: Option<String>,
}

impl Party {
    /// `true` when this party was coded under the DVGW agency (`332`).
    #[must_use]
    pub fn is_dvgw_coded(&self) -> bool {
        self.agency.as_deref() == Some(crate::document::DVGW_AGENCY_CODE)
    }
}

/// A reference from an `RFF` segment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Reference {
    /// C506 DE 1153 reference qualifier — `Z13`, `ANX`, `AGO`, …
    pub qualifier: String,
    /// C506 DE 1154 reference value.
    pub value: String,
}

/// `RFF` qualifiers the DVGW Nachrichtenbeschreibungen define.
pub mod rff {
    /// `Z13` — Prüfidentifikator. Present in every DVGW message.
    pub const PRUEFIDENTIFIKATOR: &str = "Z13";
    /// `ANX` — Clearingnummer (ALOCAT).
    pub const CLEARINGNUMMER: &str = "ANX";
    /// `AGO` — Referenz auf die Original-Nominierung (NOMINT).
    ///
    /// This — not `Z13` — is the back-reference that correlates a re-nomination
    /// to the nomination it corrects.
    pub const ORIGINAL_NOMINIERUNG: &str = "AGO";
}

/// `QTY` C186 DE 6063 qualifiers the DVGW Nachrichtenbeschreibungen define.
pub mod qty {
    /// `Z02` — Einspeisung (ALOCAT, NOMINT, NOMRES).
    pub const EINSPEISUNG: &str = "Z02";
    /// `Z03` — Ausspeisung (ALOCAT, NOMINT, NOMRES).
    pub const AUSSPEISUNG: &str = "Z03";
    /// `ZY0` — Mehrmenge (SSQNOT).
    pub const MEHRMENGE: &str = "ZY0";
    /// `ZY2` — Mindermenge (SSQNOT).
    pub const MINDERMENGE: &str = "ZY2";
}

/// `QTY` C186 DE 6411 units the DVGW Nachrichtenbeschreibungen define.
pub mod unit {
    /// `KW1` — Kilowattstunden pro Stunde (kWh/h): a rate.
    pub const KWH_PER_HOUR: &str = "KW1";
    /// `KW2` — Kilowattstunden pro Tag (kWh/d): a rate (ALOCAT).
    pub const KWH_PER_DAY: &str = "KW2";
    /// `KWH` — Kilowattstunden: an energy (NOMINT, NOMRES, SSQNOT).
    pub const KWH: &str = "KWH";
}

/// `STS` DE 9015 codes the DVGW Nachrichtenbeschreibungen define.
pub mod sts {
    /// `A1G` — SLP: the Mehr-/Mindermenge was determined by Standardlastprofil (SSQNOT).
    pub const SLP: &str = "A1G";
    /// `A2G` — RLM: registrierende Leistungsmessung (SSQNOT; Zeiträume before
    /// 1.10.2015 only, Hinweis \[501\]).
    pub const RLM: &str = "A2G";
    /// `09G` — Lastprofil (SLP) synthetisch (ALOCAT Zeitreihentyp).
    pub const SLP_SYNTHETISCH: &str = "09G";
    /// `14G` — Gemessen (RLM) Tagesregime (ALOCAT Zeitreihentyp).
    pub const RLM_TAGESREGIME: &str = "14G";
    /// `15G` — Lastprofil (SLP) analytisch (ALOCAT Zeitreihentyp).
    pub const SLP_ANALYTISCH: &str = "15G";
    /// `18G` — Gemessen (RLM) Stundenregime (ALOCAT Zeitreihentyp).
    pub const RLM_STUNDENREGIME: &str = "18G";
}

/// `NAD` party function qualifiers the DVGW Nachrichtenbeschreibungen define.
pub mod nad {
    /// `MS` — Absender der Nachricht.
    pub const ABSENDER: &str = "MS";
    /// `MR` — Empfänger der Nachricht.
    pub const EMPFAENGER: &str = "MR";
    /// `ZSY` — zusätzlicher Bilanzkreisverantwortlicher (NOMINT header).
    pub const ZUSAETZLICHER_BKV: &str = "ZSY";
    /// `ZEU` — Bilanzkreis des internen Transportkunden.
    pub const BILANZKREIS_INTERN: &str = "ZEU";
    /// `ZES` — Bilanzkreis des externen Transportkunden.
    pub const BILANZKREIS_EXTERN: &str = "ZES";
    /// `ZSZ` — Netzkontonummer.
    pub const NETZKONTO: &str = "ZSZ";
    /// `ZSO` — Netzbetreibercode.
    pub const NETZBETREIBER: &str = "ZSO";
    /// `ZSH` — Netzkontonummer (ALOCAT `ZO-T3`; the SSQNOT position party).
    pub const NETZKONTO_ZO_T3: &str = "ZSH";
    /// `ZET` — vorgelagerter Netzbetreiber (Netzkopplungspunktmeldung).
    pub const VORGELAGERTER_NETZBETREIBER: &str = "ZET";
    /// `VHP` — Virtueller Handelspunkt.
    pub const VIRTUELLER_HANDELSPUNKT: &str = "VHP";
}

/// A `QTY` segment together with the period and status that qualify it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Quantity {
    /// C186 DE 6063 quantity qualifier — `Z02` (Einspeisung), `Z03` (Ausspeisung).
    pub qualifier: String,
    /// C186 DE 6060 value, parsed exactly.
    ///
    /// Gas quantities are settled to at least three decimal places, so this is a
    /// [`Decimal`]; binary floating point cannot hold those fractions exactly.
    /// `None` when the wire value is not a number — the raw text is kept in
    /// [`raw_value`](Self::raw_value) so the defect is reportable.
    pub value: Option<Decimal>,
    /// The value exactly as it appeared on the wire.
    pub raw_value: String,
    /// C186 DE 6411 measurement unit — `KW1` (kWh/h), `KW2` (kWh/d) or `KWH`;
    /// see [`unit`](mod@unit).
    pub unit: Option<String>,
    /// The period from the `DTM+2` in effect for this quantity.
    ///
    /// `None` only when the message omitted it, which the Segmentlayout does not
    /// permit — DVGW marks the `DTM` inside the `LOC` group `R` (Erforderlich).
    /// It is **not** defaulted to the message's `DTM+Z01`: a quantity is a rate,
    /// so substituting the whole Gültigkeitszeitraum for a missing hourly period
    /// would multiply that hour's rate across the entire gas day.
    /// `DVGW-DTM-2-REQUIRED` reports the omission instead.
    pub period: Option<DvgwPeriod>,
    /// `STS` DE 9015 codes attached to this quantity — the Zeitreihentyp of
    /// an ALOCAT (`09G` SLP synthetisch, `14G` RLM, …), the Verfahren of a
    /// SSQNOT (`A1G` SLP, `A2G` RLM); see [`sts`].
    pub status: Vec<String>,
}

/// One `LOC` group: a location plus the quantity time series reported for it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct LocationGroup {
    /// DE 3227 place qualifier — `Z19` (Netzpunkt), `Z99` (keine Ortsangabe).
    pub qualifier: String,
    /// C517 DE 3225 location identifier.
    ///
    /// `None` for `LOC+Z99`, which ALOCAT sends when the message needs no
    /// specific place. An absent code is normal, not a reason to drop the group.
    pub code: Option<String>,
    /// C517 DE 3055 code-list responsible agency.
    pub agency: Option<String>,
    /// The quantities reported for this location, in wire order.
    pub quantities: Vec<Quantity>,
}

/// An `IMD` description — NOMRES uses it to say which side of the match a
/// position reports.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct ItemDescription {
    /// DE 7081 item characteristic — `05G`.
    pub characteristic: Option<String>,
    /// C273 DE 7009 description code — `17G` nominiert, `18G` Gegenseite,
    /// `16G` gematcht.
    pub code: Option<String>,
}

/// `IMD` DE 7009 codes NOMRES uses to label a position (NOMRES 4.7 §3.2).
pub mod imd {
    /// `12G` — Akzeptiert vom Netzbetreiber.
    pub const AKZEPTIERT_NB: &str = "12G";
    /// `13G` — Akzeptiert vom benachbarten Netzbetreiber.
    pub const AKZEPTIERT_NACHBAR_NB: &str = "13G";
    /// `14G` — Verarbeitet vom Netzbetreiber.
    pub const VERARBEITET_NB: &str = "14G";
    /// `15G` — Verarbeitet vom benachbarten Netzbetreiber.
    pub const VERARBEITET_NACHBAR_NB: &str = "15G";
    /// `16G` — Bestätigt: die gematchten Mengen.
    pub const GEMATCHT: &str = "16G";
    /// `17G` — Nominiert vom Empfänger des Dokumentes (eigene Seite).
    pub const NOMINIERT: &str = "17G";
    /// `18G` — Nominiert vom Geschäftspartner (Gegenseite).
    pub const GEGENSEITE: &str = "18G";
}

/// One `LIN` loop — a position of the message.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct LineItem {
    /// DE 1082 Positionsnummer.
    pub number: Option<String>,
    /// C212 DE 7143 item type — the Zeitreihentyp in ALOCAT (`LIN+1++:Z01::332`).
    pub item_type: Option<String>,
    /// `IMD` descriptions (NOMRES).
    pub descriptions: Vec<ItemDescription>,
    /// The `LOC` groups of this position, in wire order.
    pub locations: Vec<LocationGroup>,
    /// Position-level parties — Bilanzkreis, Netzkonto, VHP, Netzbetreiber.
    pub parties: Vec<Party>,
}

impl Quantity {
    /// The energy this quantity represents, in kWh.
    ///
    /// A `KW1` (kWh/h) or `KW2` (kWh/d) `QTY` is a **rate** over the period its
    /// own `DTM+2` names, so the energy is rate × duration; summing the raw
    /// values of a profile adds rates together and yields a number in no unit
    /// at all — the single most tempting way to get a gas quantity wrong. A
    /// `KWH` `QTY` is the energy itself.
    ///
    /// Returns `None` when the value is not numeric, when a rate has no period
    /// to integrate over, or when the unit is not one this can convert.
    #[must_use]
    pub fn energy_kwh(&self) -> Option<Decimal> {
        let value = self.value?;
        // A unit this does not know is not assumed to be a rate — silently
        // treating one as kWh/h is how a wrong figure becomes an invoice.
        let per_seconds = match self.unit.as_deref() {
            Some(unit::KWH) => return Some(value),
            Some(unit::KWH_PER_HOUR) => Decimal::from(3600),
            Some(unit::KWH_PER_DAY) => Decimal::from(86_400),
            _ => return None,
        };
        let period = self.period?;
        let seconds = Decimal::from(period.duration().whole_seconds());
        if seconds <= Decimal::ZERO {
            return None;
        }
        // rate × (duration / the rate's own period). Seconds keep a
        // sub-hourly period exact.
        Some(value * seconds / per_seconds)
    }

    /// The first `STS` DE 9015 code attached to this quantity, if any.
    #[must_use]
    pub fn status_code(&self) -> Option<&str> {
        self.status.first().map(String::as_str)
    }
}

impl LineItem {
    /// The first position-level party with the given `NAD` role.
    #[must_use]
    pub fn party(&self, role: &str) -> Option<&Party> {
        self.parties.iter().find(|p| p.role == role)
    }

    /// Every quantity of this position, flattened across its `LOC` groups.
    pub fn quantities(&self) -> impl Iterator<Item = &Quantity> {
        self.locations.iter().flat_map(|l| l.quantities.iter())
    }

    /// The `IMD` DE 7009 code of this position, when it carries one.
    #[must_use]
    pub fn description_code(&self) -> Option<&str> {
        self.descriptions.iter().find_map(|d| d.code.as_deref())
    }

    /// The `STS` DE 9015 code of this position's first quantity — the
    /// Zeitreihentyp of an ALOCAT position, the Verfahren of a SSQNOT one.
    #[must_use]
    pub fn status_code(&self) -> Option<&str> {
        self.quantities().find_map(Quantity::status_code)
    }
}

/// Energy totals per `QTY` DE 6063 qualifier, in kWh.
///
/// Kept per qualifier because the qualifier is the **direction**: `Z02` is
/// Einspeisung and `Z03` Ausspeisung, and a message may carry both (a
/// Virtueller-Handelspunkt nomination states a purchase and a sale in one
/// interchange). One scalar across them is a difference dressed up as a total.
pub type EnergyByQualifier = std::collections::BTreeMap<String, Decimal>;

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromPrimitive;

    fn qty(value: i64) -> Quantity {
        Quantity {
            qualifier: "Z03".into(),
            value: Decimal::from_i64(value),
            raw_value: value.to_string(),
            unit: Some("KW1".into()),
            period: None,
            status: Vec::new(),
        }
    }

    #[test]
    fn energy_follows_the_unit() {
        use crate::datetime::DvgwPeriod;
        use time::macros::datetime;
        let day = DvgwPeriod {
            start: datetime!(2026-03-01 05:00 UTC),
            end: datetime!(2026-03-02 05:00 UTC),
        };
        let q = |unit: &str| Quantity {
            qualifier: "Z03".into(),
            value: Decimal::from_i64(100),
            raw_value: "100".into(),
            unit: Some(unit.into()),
            period: Some(day),
            status: Vec::new(),
        };
        // 100 kWh/h over a day, 100 kWh/d over a day, 100 kWh.
        assert_eq!(q("KW1").energy_kwh().unwrap().to_string(), "2400");
        assert_eq!(q("KW2").energy_kwh().unwrap().to_string(), "100");
        assert_eq!(q("KWH").energy_kwh().unwrap().to_string(), "100");
        assert_eq!(
            q("MWH").energy_kwh(),
            None,
            "an unknown unit is not guessed"
        );
    }

    #[test]
    fn a_position_flattens_quantities_across_its_location_groups() {
        let item = LineItem {
            number: Some("1".into()),
            item_type: Some("Z01".into()),
            descriptions: Vec::new(),
            locations: vec![
                LocationGroup {
                    qualifier: "Z99".into(),
                    code: None,
                    agency: None,
                    quantities: vec![qty(100), qty(200)],
                },
                LocationGroup {
                    qualifier: "Z19".into(),
                    code: Some("ABCD1234".into()),
                    agency: Some("332".into()),
                    quantities: vec![qty(300)],
                },
            ],
            parties: vec![Party {
                role: nad::BILANZKREIS_INTERN.into(),
                id: "THE0BFH000000001".into(),
                agency: Some("332".into()),
            }],
        };
        assert_eq!(item.quantities().count(), 3, "the time series must survive");
        assert_eq!(
            item.party(nad::BILANZKREIS_INTERN).unwrap().id,
            "THE0BFH000000001"
        );
        assert!(item.party(nad::BILANZKREIS_EXTERN).is_none());
        assert!(item.parties[0].is_dvgw_coded());
    }
}
