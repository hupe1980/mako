//! The typed message model, shared by all three DVGW families.
//!
//! ALOCAT, NOMINT and NOMRES have the same shape — they differ in which
//! qualifiers are legal, not in structure — so one model serves all three and
//! the per-family rules live in the validation layer.
//!
//! ```text
//! BGM DTM×3 RFF+ NAD+MS NAD+MR
//! └─ LIN                          ← LineItem (Positionsnummer, Zeitreihentyp)
//!    ├─ IMD                       ← NOMRES: nominated / counterparty / matched
//!    ├─ LOC                       ← LocationGroup, repeats
//!    │  ├─ DTM+2                  ← period for the quantities that follow
//!    │  └─ QTY (+STS)             ← Quantity, repeats — a time series
//!    └─ NAD+ZEU / NAD+ZES / …     ← Bilanzkreis, Netzkonto, VHP
//! ```
//!
//! A `LOC` group carries **many** `QTY` segments — Edig@s `SG37` repeats up to
//! 199 times — one per period of the profile.

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
    /// `ZSH` — Netzkontonummer (Allokationsmeldung 3-Tupel ZO-T3).
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
    /// C186 DE 6411 measurement unit — `KW1` (kWh/h) throughout DVGW.
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
    /// `STS` DE 9015 status codes attached to this quantity (ALOCAT).
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

/// `IMD` DE 7009 codes NOMRES uses to label a position.
pub mod imd {
    /// `17G` — die nominierten Mengen (eigene Seite).
    pub const NOMINIERT: &str = "17G";
    /// `18G` — die Mengen der Gegenseite.
    pub const GEGENSEITE: &str = "18G";
    /// `16G` — die gematchten Mengen.
    pub const GEMATCHT: &str = "16G";
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
    /// A DVGW `QTY` is a **rate** — `KW1` is kWh/h — over the period its own
    /// `DTM+2` names, so the energy is rate × duration. Summing the raw values
    /// of a profile adds rates together and yields a number in no unit at all;
    /// it is the single most tempting way to get a gas quantity wrong.
    ///
    /// Returns `None` when the value is not numeric, when there is no period to
    /// integrate over, or when the unit is not one this can convert.
    #[must_use]
    pub fn energy_kwh(&self) -> Option<Decimal> {
        // Only kWh/h is converted. A unit this does not know is not assumed to
        // be a rate — silently treating one as kWh/h is how a wrong figure
        // becomes an invoice.
        if self.unit.as_deref() != Some("KW1") {
            return None;
        }
        let value = self.value?;
        let period = self.period?;
        let seconds = Decimal::from(period.duration().whole_seconds());
        if seconds <= Decimal::ZERO {
            return None;
        }
        // kWh/h × h = kWh. Seconds keep a sub-hourly period exact.
        Some(value * seconds / Decimal::from(3600))
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
