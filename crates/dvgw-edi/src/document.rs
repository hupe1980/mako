//! Message identity: the UN/EDIFACT carrier, the DVGW document-name code, and
//! the logical message family they resolve to.
//!
//! # Why the document code and not `UNH`
//!
//! Every DVGW gas-transport format is a *subset of a UN/EDIFACT D.07A message*.
//! The `UNH` message type therefore names the carrier — `ORDERS` for a
//! nomination, `ORDRSP` for an allocation or a nomination response — and never
//! the DVGW message:
//!
//! ```text
//! UNH+1+ORDERS:D:07A:UN:DVGW18'      ← NOMINT 4.6
//! BGM+01G::332+NOMINT00052'          ← *this* says NOMINT
//! ```
//!
//! Reading `UNH` for the message name makes every conformant message
//! unrecognisable, so identity is resolved from `BGM` C002 DE 1001 and the
//! carrier is used only as a cross-check.
//!
//! Sources: DVGW-Nachrichtenbeschreibungen ALOCAT 5.11a (ORDRSP / UN D.07A S3),
//! NOMINT 4.6 (ORDERS / UN D.07A S3), NOMRES 4.7 (ORDRSP / UN D.07A S3),
//! SSQNOT 5.7 (ORDRSP / UN D.07A S3).

use std::fmt;

/// The UN/EDIFACT message that carries a DVGW format on the wire (`UNH` DE 0065).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Carrier {
    /// `ORDERS` — Purchase Order. Carries NOMINT.
    Orders,
    /// `ORDRSP` — Purchase Order Response. Carries ALOCAT, NOMRES and SSQNOT.
    Ordrsp,
}

impl Carrier {
    /// The `UNH` DE 0065 value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orders => "ORDERS",
            Self::Ordrsp => "ORDRSP",
        }
    }

    /// Parse a `UNH` DE 0065 value; `None` for anything that is not a DVGW carrier.
    #[must_use]
    pub fn from_unh_code(code: &str) -> Option<Self> {
        match code {
            "ORDERS" => Some(Self::Orders),
            "ORDRSP" => Some(Self::Ordrsp),
            _ => None,
        }
    }
}

impl fmt::Display for Carrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The logical DVGW message family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DvgwMessageType {
    /// ALOCAT — Allokationsnachricht (NB ↔ MGV ↔ BKV).
    Alocat,
    /// NOMINT — Nominierung (Transportkunde → Netz-/Marktgebietsbetreiber).
    Nomint,
    /// NOMRES — Nominierungsantwort / Matching-Benachrichtigung.
    Nomres,
    /// SSQNOT — Mehr-/Mindermengenmeldung zur Führung des Netzkontos (NB → MGV).
    Ssqnot,
}

impl DvgwMessageType {
    /// The DVGW message name as it appears in the Nachrichtenbeschreibung.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alocat => "ALOCAT",
            Self::Nomint => "NOMINT",
            Self::Nomres => "NOMRES",
            Self::Ssqnot => "SSQNOT",
        }
    }

    /// The UN/EDIFACT carrier this family is transmitted on.
    #[must_use]
    pub fn carrier(self) -> Carrier {
        match self {
            Self::Nomint => Carrier::Orders,
            Self::Alocat | Self::Nomres | Self::Ssqnot => Carrier::Ordrsp,
        }
    }

    /// The `UNH` S009 DE 0057 value the Nachrichtenbeschreibung prescribes —
    /// the message version for ALOCAT (`5.11a`), the Nachrichtentypen-Paket
    /// for the others (`DVGW17`).
    #[must_use]
    pub fn anwendungscode(self) -> &'static str {
        match self {
            Self::Alocat => "5.11a",
            Self::Nomint | Self::Nomres | Self::Ssqnot => "DVGW17",
        }
    }

    /// The `QTY` C186 DE 6411 units the Segmentlayout admits, the default first.
    ///
    /// ALOCAT states rates (`KW1` kWh/h, `KW2` kWh/d); a nomination states a
    /// rate or an energy (`KW1`, `KWH`); a Mehr-/Mindermengenmeldung is energy
    /// only (`KWH`).
    #[must_use]
    pub fn admitted_units(self) -> &'static [&'static str] {
        use crate::model::unit;
        match self {
            Self::Alocat => &[unit::KWH_PER_HOUR, unit::KWH_PER_DAY],
            Self::Nomint | Self::Nomres => &[unit::KWH_PER_HOUR, unit::KWH],
            Self::Ssqnot => &[unit::KWH],
        }
    }

    /// The `QTY` C186 DE 6063 qualifiers the Segmentlayout admits.
    #[must_use]
    pub fn admitted_quantity_qualifiers(self) -> &'static [&'static str] {
        use crate::model::qty;
        match self {
            Self::Alocat | Self::Nomint | Self::Nomres => &[qty::EINSPEISUNG, qty::AUSSPEISUNG],
            Self::Ssqnot => &[qty::MEHRMENGE, qty::MINDERMENGE],
        }
    }

    /// The `LOC` DE 3227 qualifiers the Segmentlayout admits.
    #[must_use]
    pub fn admitted_location_qualifiers(self) -> &'static [&'static str] {
        match self {
            // „In der Nachricht ist keine Angabe eines spezifischen Ortes
            // erforderlich" — the segment is `LOC+Z99` and nothing else.
            Self::Alocat | Self::Ssqnot => &["Z99"],
            Self::Nomint | Self::Nomres => &["172", "Z17", "Z19"],
        }
    }

    /// Every document-name code that resolves to this family.
    #[must_use]
    pub fn documents(self) -> &'static [DvgwDocument] {
        use DvgwDocument as D;
        match self {
            Self::Alocat => &[
                D::AllokationSlp,
                D::KorrigierteMengenmeldungNkp,
                D::SlpErsatzwerte,
                D::UntertaegigeAllokation,
                D::EndgueltigeAllokation,
                D::KorrigierteAllokationBilanzierungsbrennwert,
                D::KorrigierteAllokationAbrechnungsbrennwert,
                D::TaeglicheMengenmeldungNkp,
            ],
            Self::Nomint => &[
                D::NominierungTransportkunde,
                D::NominierungVirtuellerHandelspunkt,
                D::Flexibilitaetsuebertragung,
                D::NominierungGebuendelteKapazitaet,
                D::NominierungsweitergabeNetzbetreiber,
            ],
            Self::Nomres => &[
                D::MatchingBenachrichtigung,
                D::Bestaetigung,
                D::VhpMatchingBenachrichtigung,
                D::VhpBestaetigung,
                D::BestaetigungFlexibilitaetsuebertragung,
            ],
            Self::Ssqnot => &[D::MehrMindermengenmeldung],
        }
    }

    /// All families, in catalogue order.
    pub const ALL: [Self; 4] = [Self::Alocat, Self::Nomint, Self::Nomres, Self::Ssqnot];
}

impl fmt::Display for DvgwMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The DVGW document-name code from `BGM` C002 DE 1001 — the field that says
/// which business message this actually is.
///
/// The variant set is exhaustive for the current Nachrichtentypen-Paket; a code
/// outside it surfaces as [`Error::UnknownDocumentCode`](crate::Error::UnknownDocumentCode)
/// rather than being guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DvgwDocument {
    // ── ALOCAT (ORDRSP) ──────────────────────────────────────────────────────
    /// `X1G` — Allokation anhand von Standardlastprofilen (SLP).
    AllokationSlp,
    /// `X2G` — Korrigierte Mengenmeldung NKP je Netzkonto.
    KorrigierteMengenmeldungNkp,
    /// `X3G` — SLP-Ersatzwerte.
    SlpErsatzwerte,
    /// `X4G` — Untertägige Allokation (Intraday).
    UntertaegigeAllokation,
    /// `X5G` — Endgültige Allokation (Bilanzierungsbrennwert).
    EndgueltigeAllokation,
    /// `X6G` — Korrigierte Allokation (Bilanzierungsbrennwert).
    KorrigierteAllokationBilanzierungsbrennwert,
    /// `X7G` — Korrigierte Allokation (Abrechnungsbrennwert).
    KorrigierteAllokationAbrechnungsbrennwert,
    /// `XBG` — Tägliche Mengenmeldung NKP je Netzkonto.
    TaeglicheMengenmeldungNkp,

    // ── NOMINT (ORDERS) ──────────────────────────────────────────────────────
    /// `01G` — Nominierung von einem Transportkunden.
    NominierungTransportkunde,
    /// `55G` — Nominierung an einem Virtuellen Handelspunkt.
    NominierungVirtuellerHandelspunkt,
    /// `Y1G` — Flexibilitätsübertragung.
    Flexibilitaetsuebertragung,
    /// `Y6G` — Nominierung gebündelter Kapazität an MÜP und GÜP.
    NominierungGebuendelteKapazitaet,
    /// `Y7G` — Nominierungsweitergabe zwischen Netzbetreibern.
    NominierungsweitergabeNetzbetreiber,

    // ── NOMRES (ORDRSP) ──────────────────────────────────────────────────────
    /// `07G` — Matching-Benachrichtigung.
    MatchingBenachrichtigung,
    /// `08G` — Bestätigung.
    Bestaetigung,
    /// `19G` — Virtueller Handelspunkt: Matching-Benachrichtigung.
    VhpMatchingBenachrichtigung,
    /// `20G` — Virtueller Handelspunkt: Bestätigung.
    VhpBestaetigung,
    /// `Y2G` — Bestätigung Flexibilitätsübertragung.
    BestaetigungFlexibilitaetsuebertragung,

    // ── SSQNOT (ORDRSP) ──────────────────────────────────────────────────────
    /// `BAG` — Mehr-/Mindermengenmeldung zur Führung des Netzkontos.
    MehrMindermengenmeldung,
}

/// `(wire code, document, German description)` — the single table every lookup
/// on [`DvgwDocument`] reads, so a new code is one line rather than three.
const CATALOGUE: &[(&str, DvgwDocument, &str)] = {
    use DvgwDocument as D;
    &[
        (
            "X1G",
            D::AllokationSlp,
            "Allokation anhand von Standardlastprofilen (SLP)",
        ),
        (
            "X2G",
            D::KorrigierteMengenmeldungNkp,
            "Korrigierte Mengenmeldung NKP je Netzkonto",
        ),
        ("X3G", D::SlpErsatzwerte, "SLP-Ersatzwerte"),
        (
            "X4G",
            D::UntertaegigeAllokation,
            "Untertägige Allokation (Intraday)",
        ),
        (
            "X5G",
            D::EndgueltigeAllokation,
            "Endgültige Allokation (Bilanzierungsbrennwert)",
        ),
        (
            "X6G",
            D::KorrigierteAllokationBilanzierungsbrennwert,
            "Korrigierte Allokation (Bilanzierungsbrennwert)",
        ),
        (
            "X7G",
            D::KorrigierteAllokationAbrechnungsbrennwert,
            "Korrigierte Allokation (Abrechnungsbrennwert)",
        ),
        (
            "XBG",
            D::TaeglicheMengenmeldungNkp,
            "Tägliche Mengenmeldung NKP je Netzkonto",
        ),
        (
            "01G",
            D::NominierungTransportkunde,
            "Nominierung von einem Transportkunden",
        ),
        (
            "55G",
            D::NominierungVirtuellerHandelspunkt,
            "Nominierung an einem Virtuellen Handelspunkt",
        ),
        (
            "Y1G",
            D::Flexibilitaetsuebertragung,
            "Flexibilitätsübertragung",
        ),
        (
            "Y6G",
            D::NominierungGebuendelteKapazitaet,
            "Nominierung gebündelter Kapazität an MÜP und GÜP",
        ),
        (
            "Y7G",
            D::NominierungsweitergabeNetzbetreiber,
            "Nominierungsweitergabe zwischen Netzbetreibern",
        ),
        (
            "07G",
            D::MatchingBenachrichtigung,
            "Matching-Benachrichtigung",
        ),
        ("08G", D::Bestaetigung, "Bestätigung"),
        (
            "19G",
            D::VhpMatchingBenachrichtigung,
            "Virtueller Handelspunkt: Matching-Benachrichtigung",
        ),
        (
            "20G",
            D::VhpBestaetigung,
            "Virtueller Handelspunkt: Bestätigung",
        ),
        (
            "Y2G",
            D::BestaetigungFlexibilitaetsuebertragung,
            "Bestätigung Flexibilitätsübertragung",
        ),
        (
            "BAG",
            D::MehrMindermengenmeldung,
            "Mehr-/Mindermengenmeldung zur Führung des Netzkontos",
        ),
    ]
};

impl DvgwDocument {
    /// The `BGM` C002 DE 1001 wire code.
    #[must_use]
    pub fn code(self) -> &'static str {
        CATALOGUE
            .iter()
            .find(|(_, d, _)| *d == self)
            .map_or("", |(c, _, _)| *c)
    }

    /// Parse a `BGM` DE 1001 value; `None` for codes outside the DVGW catalogue.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        CATALOGUE
            .iter()
            .find(|(c, _, _)| *c == code)
            .map(|(_, d, _)| *d)
    }

    /// The German description from the Nachrichtenbeschreibung.
    #[must_use]
    pub fn description(self) -> &'static str {
        CATALOGUE
            .iter()
            .find(|(_, d, _)| *d == self)
            .map_or("", |(_, _, t)| *t)
    }

    /// The logical message family this code belongs to.
    #[must_use]
    pub fn message_type(self) -> DvgwMessageType {
        for mt in DvgwMessageType::ALL {
            if mt.documents().contains(&self) {
                return mt;
            }
        }
        unreachable!("every DvgwDocument is listed in exactly one DvgwMessageType::documents()")
    }

    /// The UN/EDIFACT carrier that transmits this document.
    #[must_use]
    pub fn carrier(self) -> Carrier {
        self.message_type().carrier()
    }

    /// Every document code in catalogue order.
    pub fn all() -> impl Iterator<Item = Self> {
        CATALOGUE.iter().map(|(_, d, _)| *d)
    }

    /// The document-name code the Anwendungsfall column of `pid` admits in
    /// `BGM` DE 1001 — every published column marks exactly one.
    ///
    /// Source: ALOCAT 5.11a §4, NOMINT 4.6 §4, NOMRES 4.7 §4, SSQNOT 5.7 §4.
    /// `None` for a code no shipped column publishes.
    #[must_use]
    pub fn for_pid(pid: u32) -> Option<Self> {
        use DvgwDocument as D;
        Some(match pid {
            70001 | 70008 | 70013 | 70018 | 70022 => D::AllokationSlp,
            70002 | 70011 | 70023 => D::KorrigierteMengenmeldungNkp,
            70003 | 70012 => D::TaeglicheMengenmeldungNkp,
            70004 | 70014 => D::UntertaegigeAllokation,
            70005 | 70015 => D::EndgueltigeAllokation,
            70006 | 70009 | 70016 | 70019 => D::KorrigierteAllokationBilanzierungsbrennwert,
            70007 | 70010 | 70017 | 70020 => D::KorrigierteAllokationAbrechnungsbrennwert,
            70021 => D::SlpErsatzwerte,
            70030 => D::NominierungTransportkunde,
            70031 => D::NominierungVirtuellerHandelspunkt,
            70032 => D::Flexibilitaetsuebertragung,
            70033 => D::NominierungGebuendelteKapazitaet,
            70034 => D::NominierungsweitergabeNetzbetreiber,
            70035 => D::MatchingBenachrichtigung,
            70036 => D::Bestaetigung,
            70037 => D::VhpMatchingBenachrichtigung,
            70038 => D::VhpBestaetigung,
            70039 => D::BestaetigungFlexibilitaetsuebertragung,
            70095 | 70096 => D::MehrMindermengenmeldung,
            _ => return None,
        })
    }
}

/// The code-list responsible agency DVGW stamps on every coded value
/// (`DE 3055` = `332`).
pub const DVGW_AGENCY_CODE: &str = "332";

impl fmt::Display for DvgwDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_document_round_trips_through_its_code() {
        for doc in DvgwDocument::all() {
            assert!(!doc.code().is_empty(), "{doc:?} has no wire code");
            assert!(!doc.description().is_empty(), "{doc:?} has no description");
            assert_eq!(DvgwDocument::from_code(doc.code()), Some(doc));
        }
    }

    #[test]
    fn the_catalogue_has_no_duplicate_codes() {
        let mut codes: Vec<&str> = CATALOGUE.iter().map(|(c, _, _)| *c).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes.len(),
            total,
            "duplicate BGM 1001 code in the catalogue"
        );
    }

    #[test]
    fn message_type_documents_partition_the_catalogue() {
        let listed: usize = DvgwMessageType::ALL
            .iter()
            .map(|m| m.documents().len())
            .sum();
        assert_eq!(
            listed,
            CATALOGUE.len(),
            "a document code is unreachable from its family"
        );
    }

    /// Every published Anwendungsfall names one document, of its own family.
    #[test]
    fn every_catalogued_pid_names_a_document_of_its_family() {
        for info in crate::pruefidentifikator::catalogue() {
            let doc = DvgwDocument::for_pid(info.pid)
                .unwrap_or_else(|| panic!("{} has no BGM code", info.pid));
            assert_eq!(doc.message_type(), info.message_type, "{}", info.pid);
        }
        assert_eq!(DvgwDocument::for_pid(70_500), None);
    }

    /// The carrier is the cross-check, so it must follow the family exactly.
    #[test]
    fn nomint_rides_orders_and_the_rest_ride_ordrsp() {
        assert_eq!(DvgwMessageType::Nomint.carrier(), Carrier::Orders);
        assert_eq!(DvgwMessageType::Alocat.carrier(), Carrier::Ordrsp);
        assert_eq!(DvgwMessageType::Nomres.carrier(), Carrier::Ordrsp);
        assert_eq!(DvgwMessageType::Ssqnot.carrier(), Carrier::Ordrsp);
        assert_eq!(
            DvgwDocument::from_code("BAG"),
            Some(DvgwDocument::MehrMindermengenmeldung)
        );
        assert_eq!(
            Carrier::from_unh_code("ALOCAT"),
            None,
            "ALOCAT is not a wire carrier"
        );
    }
}
