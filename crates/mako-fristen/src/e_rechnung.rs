//! Whether an invoice to an Unternehmer may still go out other than as an
//! E-Rechnung — on paper, in another electronic format, or as EDI — under the
//! transition rules of § 27 Abs. 38 UStG.
//!
//! § 14 Abs. 2 Satz 2 Nr. 1 UStG makes the E-Rechnung mandatory „für eine
//! Leistung an einen anderen Unternehmer für dessen Unternehmen". § 27 Abs. 38
//! UStG (read on gesetze-im-internet.de, 2026-11-06) grants three graces from
//! it, each keyed on when the Umsatz was **ausgeführt**:
//!
//! | Nr. | Umsatz ausgeführt | Allowed until | Condition |
//! |---|---|---|---|
//! | 1 | after 31.12.2024, before 01.01.2027 | übermittelt by 31.12.2026 | paper; another electronic format „vorbehaltlich der Zustimmung des Empfängers" |
//! | 2 | after 31.12.2026, before 01.01.2028 | übermittelt by 31.12.2027 | as Nr. 1, and the issuer's Gesamtumsatz (§ 19 Abs. 2) im vorangegangenen Kalenderjahr „nicht mehr als 800 000 Euro" |
//! | 3 | after 31.12.2026, before 01.01.2028 | ausgestellt by 31.12.2027 | EDI under Art. 2 Empfehlung 94/820/EG, with the recipient's Zustimmung |
//!
//! EDI needs no grace when its data extract into the European norm: an agreed
//! format is an E-Rechnung if it allows „die richtige und vollständige
//! Extraktion der nach diesem Gesetz erforderlichen Angaben … in ein Format …,
//! das der Norm nach Nummer 1 entspricht" (§ 14 Abs. 1 Satz 6 Nr. 2 UStG).
//!
//! A supply executed before 01.01.2025 is outside the new § 14 altogether:
//! changes to the UStG reach „Umsätze …, die ab dem Inkrafttreten der
//! maßgeblichen Änderungsvorschrift ausgeführt werden" (§ 27 Abs. 1 Satz 1).
//!
//! Every fact a grace turns on is the caller's to supply, and an unrecorded
//! one refuses — a missing Zustimmung, turnover or Unternehmer status never
//! admits the grace.

use time::Date;
use time::macros::date;

/// § 27 Abs. 38 Nr. 2 UStG — „nicht mehr als 800 000 Euro", in cents.
pub const NR2_GESAMTUMSATZ_GRENZE_CENT: i64 = 80_000_000;

const NR1_ENDE: Date = date!(2026 - 12 - 31);
const NR23_ENDE: Date = date!(2027 - 12 - 31);
const NEU_AB: Date = date!(2025 - 01 - 01);

/// How a sonstige Rechnung would go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// On paper — no consent asked.
    Papier,
    /// In an electronic format that is not § 14 Abs. 1 Satz 6 UStG.
    Elektronisch,
}

/// What the caller knows about one sonstige Rechnung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonstigeRechnungFall {
    /// When the Umsatz was ausgeführt — for a supply period, its last day.
    pub ausgefuehrt: Date,
    /// When the document would be übermittelt.
    pub uebermittelt: Date,
    /// Paper, or another electronic format.
    pub form: Form,
    /// Whether the Leistungsempfänger is a Unternehmer receiving for his
    /// Unternehmen (§ 14 Abs. 2 Satz 2 Nr. 1 UStG); `None` when not recorded.
    pub empfaenger_unternehmer: Option<bool>,
    /// The recipient's Zustimmung to another electronic format; `None` when
    /// not recorded. Paper does not ask for it.
    pub zustimmung: Option<bool>,
    /// The issuer's Gesamtumsatz (§ 19 Abs. 2 UStG) in the calendar year before
    /// the supply, in cents; `None` when not recorded.
    pub gesamtumsatz_vorjahr_cent: Option<i64>,
}

/// What the caller knows about one EDI invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdiFall {
    /// When the Umsatz was ausgeführt — for a supply period, its last day.
    pub ausgefuehrt: Date,
    /// When the invoice is ausgestellt — Nr. 3's date.
    pub ausgestellt: Date,
    /// When it is übermittelt — Nr. 1's date.
    pub uebermittelt: Date,
    /// The recipient's Zustimmung to (agreement on) the EDI format; `None`
    /// when not recorded.
    pub zustimmung: Option<bool>,
    /// Whether the agreed format's data extract into an EN 16931 format
    /// (§ 14 Abs. 1 Satz 6 Nr. 2 UStG); `None` when not recorded.
    pub extrahierbar: Option<bool>,
}

/// Why the invoice is admissible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zulaessig {
    /// The recipient is no Unternehmer receiving for his Unternehmen.
    KeinB2b,
    /// The supply was executed before 01.01.2025 (§ 27 Abs. 1 Satz 1 UStG).
    UmsatzVor2025,
    /// An agreed format that extracts into EN 16931 — an E-Rechnung.
    ERechnungVereinbart,
    /// § 27 Abs. 38 Nr. 1 UStG.
    Nr1,
    /// § 27 Abs. 38 Nr. 2 UStG.
    Nr2,
    /// § 27 Abs. 38 Nr. 3 UStG.
    Nr3,
}

impl Zulaessig {
    /// The provision, as a refusal or a document names it.
    #[must_use]
    pub const fn grundlage(self) -> &'static str {
        match self {
            Self::KeinB2b => {
                "§ 14 Abs. 2 Satz 2 Nr. 1 UStG greift nicht (kein Unternehmer-Empfänger)"
            }
            Self::UmsatzVor2025 => "§ 27 Abs. 1 Satz 1 UStG (Umsatz vor dem 01.01.2025)",
            Self::ERechnungVereinbart => "§ 14 Abs. 1 Satz 6 Nr. 2 UStG (vereinbartes Format)",
            Self::Nr1 => "§ 27 Abs. 38 Nr. 1 UStG",
            Self::Nr2 => "§ 27 Abs. 38 Nr. 2 UStG",
            Self::Nr3 => "§ 27 Abs. 38 Nr. 3 UStG",
        }
    }
}

/// Why the invoice is not admissible — the provision, or the missing fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unzulaessig {
    /// Whether the recipient is an Unternehmer is not recorded.
    EmpfaengerUnbekannt,
    /// The recipient's Zustimmung is not recorded, or was not given.
    ZustimmungFehlt,
    /// A 2027 supply and the issuer's 2026 Gesamtumsatz is not recorded.
    GesamtumsatzUnbekannt {
        /// The calendar year whose figure is missing.
        jahr: i32,
    },
    /// A 2027 supply by an issuer above 800 000 € in 2026.
    Nr2UeberGrenze {
        /// The prior calendar year.
        jahr: i32,
        /// The recorded Gesamtumsatz, in cents.
        cent: i64,
    },
    /// EDI past every grace whose extractability is not recorded.
    ExtrahierbarkeitUnbekannt {
        /// The supply date.
        ausgefuehrt: Date,
    },
    /// No grace reaches this supply and these dates.
    KeineUebergangsfrist {
        /// The supply date.
        ausgefuehrt: Date,
        /// The transmission date.
        uebermittelt: Date,
    },
}

impl std::fmt::Display for Unzulaessig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmpfaengerUnbekannt => f.write_str(
                "ob der Leistungsempfänger Unternehmer für dessen Unternehmen ist \
                 (§ 14 Abs. 2 Satz 2 Nr. 1 UStG), ist nicht erfasst — die sonstige \
                 Rechnung wird nicht unterstellt",
            ),
            Self::ZustimmungFehlt => f.write_str(
                "die Zustimmung des Empfängers zu einem anderen elektronischen Format ist \
                 nicht erfasst — § 27 Abs. 38 UStG lässt es nur „vorbehaltlich der \
                 Zustimmung des Empfängers\" zu",
            ),
            Self::GesamtumsatzUnbekannt { jahr } => write!(
                f,
                "der Gesamtumsatz {jahr} des Rechnungsausstellers ist nicht erfasst — \
                 § 27 Abs. 38 Nr. 2 UStG wird nicht unterstellt"
            ),
            Self::Nr2UeberGrenze { jahr, cent } => write!(
                f,
                "Gesamtumsatz {jahr} von {},{:02} EUR übersteigt 800 000 EUR — § 27 Abs. 38 \
                 Nr. 2 UStG lässt die sonstige Rechnung nicht zu; eine E-Rechnung nach § 14 \
                 Abs. 1 Satz 6 UStG ist auszustellen",
                cent / 100,
                cent % 100
            ),
            Self::ExtrahierbarkeitUnbekannt { ausgefuehrt } => write!(
                f,
                "für einen am {ausgefuehrt} ausgeführten Umsatz trägt EDI nur als E-Rechnung \
                 nach § 14 Abs. 1 Satz 6 Nr. 2 UStG, und ob das vereinbarte Format in die \
                 EN 16931 extrahierbar ist, ist für diesen Empfänger nicht erfasst"
            ),
            Self::KeineUebergangsfrist {
                ausgefuehrt,
                uebermittelt,
            } => write!(
                f,
                "für einen am {ausgefuehrt} ausgeführten und am {uebermittelt} übermittelten \
                 Umsatz an einen Unternehmer gewährt § 27 Abs. 38 UStG keine Übergangsfrist — \
                 eine E-Rechnung nach § 14 Abs. 1 Satz 6 UStG ist auszustellen"
            ),
        }
    }
}

impl std::error::Error for Unzulaessig {}

/// Whether a sonstige Rechnung — paper, or another electronic format — may go
/// out (§ 27 Abs. 38 Nr. 1 and 2 UStG).
///
/// # Errors
///
/// [`Unzulaessig`] — naming the provision, or the missing fact.
pub fn sonstige_rechnung_zulaessig(f: SonstigeRechnungFall) -> Result<Zulaessig, Unzulaessig> {
    match f.empfaenger_unternehmer {
        None => return Err(Unzulaessig::EmpfaengerUnbekannt),
        Some(false) => return Ok(Zulaessig::KeinB2b),
        Some(true) => {}
    }
    if f.ausgefuehrt < NEU_AB {
        return Ok(Zulaessig::UmsatzVor2025);
    }
    let grace = if f.ausgefuehrt <= NR1_ENDE && f.uebermittelt <= NR1_ENDE {
        Zulaessig::Nr1
    } else if f.ausgefuehrt.year() == 2027 && f.uebermittelt <= NR23_ENDE {
        let jahr = 2026;
        match f.gesamtumsatz_vorjahr_cent {
            None => return Err(Unzulaessig::GesamtumsatzUnbekannt { jahr }),
            Some(cent) if cent <= NR2_GESAMTUMSATZ_GRENZE_CENT => Zulaessig::Nr2,
            Some(cent) => return Err(Unzulaessig::Nr2UeberGrenze { jahr, cent }),
        }
    } else {
        return Err(Unzulaessig::KeineUebergangsfrist {
            ausgefuehrt: f.ausgefuehrt,
            uebermittelt: f.uebermittelt,
        });
    };
    if f.form == Form::Elektronisch && f.zustimmung != Some(true) {
        return Err(Unzulaessig::ZustimmungFehlt);
    }
    Ok(grace)
}

/// Whether an EDI invoice to an Unternehmer may go out: as an E-Rechnung when
/// the agreed format extracts into EN 16931, else under § 27 Abs. 38 Nr. 1 or
/// Nr. 3 UStG — every route only with the recipient's recorded Zustimmung.
///
/// # Errors
///
/// [`Unzulaessig`] — naming the provision, or the missing fact.
pub fn edi_zulaessig(f: EdiFall) -> Result<Zulaessig, Unzulaessig> {
    if f.ausgefuehrt < NEU_AB {
        return Ok(Zulaessig::UmsatzVor2025);
    }
    if f.zustimmung != Some(true) {
        return Err(Unzulaessig::ZustimmungFehlt);
    }
    if f.extrahierbar == Some(true) {
        return Ok(Zulaessig::ERechnungVereinbart);
    }
    if f.ausgefuehrt <= NR1_ENDE && f.uebermittelt <= NR1_ENDE {
        return Ok(Zulaessig::Nr1);
    }
    if f.ausgefuehrt.year() == 2027 && f.ausgestellt <= NR23_ENDE {
        return Ok(Zulaessig::Nr3);
    }
    match f.extrahierbar {
        None => Err(Unzulaessig::ExtrahierbarkeitUnbekannt {
            ausgefuehrt: f.ausgefuehrt,
        }),
        Some(_) => Err(Unzulaessig::KeineUebergangsfrist {
            ausgefuehrt: f.ausgefuehrt,
            uebermittelt: f.uebermittelt,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn papier(ausgefuehrt: Date, uebermittelt: Date, cent: Option<i64>) -> SonstigeRechnungFall {
        SonstigeRechnungFall {
            ausgefuehrt,
            uebermittelt,
            form: Form::Papier,
            empfaenger_unternehmer: Some(true),
            zustimmung: None,
            gesamtumsatz_vorjahr_cent: cent,
        }
    }

    /// „nicht mehr als 800 000 Euro" keeps exactly 800 000 € inside the grace;
    /// one cent above is out. A `<` in place of `<=` fails the first row.
    #[test]
    fn the_nr2_threshold_keeps_exactly_800000_inside() {
        let (a, u) = (date!(2027 - 06 - 30), date!(2027 - 07 - 15));
        assert_eq!(
            sonstige_rechnung_zulaessig(papier(a, u, Some(80_000_000))),
            Ok(Zulaessig::Nr2)
        );
        assert_eq!(
            sonstige_rechnung_zulaessig(papier(a, u, Some(80_000_001))),
            Err(Unzulaessig::Nr2UeberGrenze {
                jahr: 2026,
                cent: 80_000_001
            })
        );
        assert_eq!(
            sonstige_rechnung_zulaessig(papier(a, u, None)),
            Err(Unzulaessig::GesamtumsatzUnbekannt { jahr: 2026 })
        );
    }

    /// From 2028 no grace reaches a sonstige Rechnung; a 2027 supply
    /// transmitted in 2028 is past Nr. 2's „bis zum 31. Dezember 2027".
    #[test]
    fn a_2028_document_has_no_grace() {
        for (a, u) in [
            (date!(2028 - 01 - 31), date!(2028 - 02 - 05)),
            (date!(2027 - 12 - 31), date!(2028 - 01 - 01)),
        ] {
            assert!(matches!(
                sonstige_rechnung_zulaessig(papier(a, u, Some(1000))),
                Err(Unzulaessig::KeineUebergangsfrist { .. })
            ));
        }
    }

    /// A December-2026 supply invoiced in January 2027 falls between Nr. 1
    /// (transmitted too late) and Nr. 2 (executed too early).
    #[test]
    fn nr1_is_keyed_on_supply_and_transmission() {
        assert_eq!(
            sonstige_rechnung_zulaessig(papier(date!(2026 - 12 - 31), date!(2026 - 12 - 31), None)),
            Ok(Zulaessig::Nr1)
        );
        assert!(matches!(
            sonstige_rechnung_zulaessig(papier(
                date!(2026 - 12 - 31),
                date!(2027 - 01 - 10),
                Some(1000)
            )),
            Err(Unzulaessig::KeineUebergangsfrist { .. })
        ));
    }

    /// Another electronic format is admitted only „vorbehaltlich der
    /// Zustimmung des Empfängers"; paper does not ask.
    #[test]
    fn the_electronic_form_needs_the_recorded_zustimmung() {
        let base = SonstigeRechnungFall {
            form: Form::Elektronisch,
            ..papier(date!(2026 - 06 - 30), date!(2026 - 07 - 05), None)
        };
        assert_eq!(
            sonstige_rechnung_zulaessig(base),
            Err(Unzulaessig::ZustimmungFehlt)
        );
        assert_eq!(
            sonstige_rechnung_zulaessig(SonstigeRechnungFall {
                zustimmung: Some(false),
                ..base
            }),
            Err(Unzulaessig::ZustimmungFehlt)
        );
        assert_eq!(
            sonstige_rechnung_zulaessig(SonstigeRechnungFall {
                zustimmung: Some(true),
                ..base
            }),
            Ok(Zulaessig::Nr1)
        );
    }

    /// No Unternehmer recipient, no duty; an unrecorded recipient refuses.
    #[test]
    fn the_recipient_decides_whether_the_duty_applies() {
        let base = papier(date!(2028 - 01 - 31), date!(2028 - 02 - 05), None);
        assert_eq!(
            sonstige_rechnung_zulaessig(SonstigeRechnungFall {
                empfaenger_unternehmer: Some(false),
                ..base
            }),
            Ok(Zulaessig::KeinB2b)
        );
        assert_eq!(
            sonstige_rechnung_zulaessig(SonstigeRechnungFall {
                empfaenger_unternehmer: None,
                ..base
            }),
            Err(Unzulaessig::EmpfaengerUnbekannt)
        );
    }

    #[test]
    fn a_supply_before_2025_is_outside_the_new_section_14() {
        assert_eq!(
            sonstige_rechnung_zulaessig(papier(date!(2024 - 12 - 31), date!(2028 - 03 - 01), None)),
            Ok(Zulaessig::UmsatzVor2025)
        );
    }

    fn edi(
        ausgefuehrt: Date,
        ausgestellt: Date,
        uebermittelt: Date,
        extrahierbar: Option<bool>,
    ) -> EdiFall {
        EdiFall {
            ausgefuehrt,
            ausgestellt,
            uebermittelt,
            zustimmung: Some(true),
            extrahierbar,
        }
    }

    /// Nr. 3 is keyed on the 2027 supply and the issuance date; Nr. 1 on the
    /// transmission. A December-2026 supply invoiced in January 2027 has
    /// neither, so it goes out only as an extractable format.
    #[test]
    fn edi_is_keyed_on_supply_issuance_and_transmission() {
        use time::Month::{December, January, June};
        let d = |y, m, t| Date::from_calendar_date(y, m, t).unwrap();

        assert_eq!(
            edi_zulaessig(edi(
                d(2026, December, 31),
                d(2026, December, 31),
                d(2026, December, 31),
                None
            )),
            Ok(Zulaessig::Nr1)
        );
        assert_eq!(
            edi_zulaessig(edi(
                d(2026, December, 31),
                d(2027, January, 5),
                d(2027, January, 5),
                None
            )),
            Err(Unzulaessig::ExtrahierbarkeitUnbekannt {
                ausgefuehrt: d(2026, December, 31)
            })
        );
        assert_eq!(
            edi_zulaessig(edi(
                d(2026, December, 31),
                d(2027, January, 5),
                d(2027, January, 5),
                Some(true)
            )),
            Ok(Zulaessig::ERechnungVereinbart)
        );
        // Nr. 3 reads the Ausstellung: issued 31.12.2027, sent 01.01.2028, admitted.
        assert_eq!(
            edi_zulaessig(edi(
                d(2027, December, 31),
                d(2027, December, 31),
                d(2028, January, 1),
                None
            )),
            Ok(Zulaessig::Nr3)
        );
        // From 2028 only the extractable format carries EDI.
        assert!(matches!(
            edi_zulaessig(edi(
                d(2028, June, 30),
                d(2028, June, 30),
                d(2028, June, 30),
                Some(false)
            )),
            Err(Unzulaessig::KeineUebergangsfrist { .. })
        ));
        assert_eq!(
            edi_zulaessig(edi(
                d(2028, June, 30),
                d(2028, June, 30),
                d(2028, June, 30),
                Some(true)
            )),
            Ok(Zulaessig::ERechnungVereinbart)
        );
    }

    /// Every EDI route, the E-Rechnung included, rests on the recipient's
    /// agreement; an unrecorded one refuses before any date is read.
    #[test]
    fn edi_without_a_recorded_zustimmung_is_refused() {
        let f = EdiFall {
            zustimmung: None,
            ..edi(
                date!(2027 - 03 - 31),
                date!(2027 - 04 - 02),
                date!(2027 - 04 - 02),
                Some(true),
            )
        };
        assert_eq!(edi_zulaessig(f), Err(Unzulaessig::ZustimmungFehlt));
    }
}
