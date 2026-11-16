//! Referenzreihen — the published figures billing reads, as one catalogue.
//!
//! A Referenzreihe is a figure a publication sets rather than a statute: the
//! Durchschnittserlös the KAV Grenzpreis is taken from, the EEG Marktwerte, the
//! day-ahead Spotmarktpreis. marktd stores the values (`referenzwerte`,
//! append-only and bitemporal); this module is what every reader and the
//! `cargo xtask check-referenzreihen` guard agree a series *is* — its key, unit,
//! period, dimensions, publisher, legal anchor, publication rule and licence.
//!
//! A statute-fixed rate (BEHG, EnergieStG, MwSt, § 30 MsbG) is not a
//! Referenzreihe: a law changes it, and a release is the right review gate.
//!
//! ## Licence decides seeding
//!
//! A [`Lizenz::Frei`] series may ship seed rows from the crate
//! ([`Reihe::seed`]); a [`Lizenz::Betreiber`] series ships none and is loaded
//! by the operator under its own licence. A series whose publisher's terms are
//! not verified is `Betreiber`: shipping a value is a distribution, and an
//! unverified licence is not one mako can grant.
#![expect(
    clippy::doc_markdown,
    reason = "German statute and domain names (EnergieStG, MsbG, Marktwert) are words, not code identifiers"
)]

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::{Date, Month, OffsetDateTime};

/// A catalogued Referenzreihe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reihe {
    /// KAV § 2 Abs. 4 — the Durchschnittserlös je kWh aus der Lieferung von Strom
    /// an alle Sondervertragskunden, per Kalenderjahr.
    KavGrenzpreisStrom,
    /// KAV § 2 Abs. 5 Nr. 2 — the Durchschnittserlös je kWh aus den Lieferungen
    /// von Gas an alle Letztverbraucher, per Kalenderjahr.
    ///
    /// Not itself the Gas Grenzpreis: the Verordnung scales 1,50 ct/kWh by
    /// Durchschnittserlöse, and for a contract after 1 January 1992 this
    /// published figure of the contract year is the base.
    KavDurchschnittserloesGas,
    /// EEG 2023 Anlage 1 Nr. 3 — the Monatsmarktwert per Energieträger.
    EegMonatsmarktwert,
    /// EEG 2023 Anlage 1 Nr. 4 — the Jahresmarktwert per Energieträger.
    EegJahresmarktwert,
    /// EEG 2023 § 3 Nr. 42a — the Spotmarktpreis of the day-ahead coupling,
    /// Preiszone Deutschland, per Viertelstunde.
    DayAheadDeLu,
}

/// The period one value of a series covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularitaet {
    /// A German calendar year, Berlin midnight to Berlin midnight.
    Kalenderjahr,
    /// A German calendar month, Berlin midnight to Berlin midnight.
    Kalendermonat,
    /// A quarter hour.
    Viertelstunde,
}

/// Under which terms a series' values may be distributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "art", rename_all = "snake_case")]
pub enum Lizenz {
    /// Free to redistribute with the stated attribution — mako may seed it.
    Frei {
        /// The attribution the licence requires.
        namensnennung: &'static str,
        /// Where the licence was read.
        lizenz_url: &'static str,
    },
    /// The operator loads it under its own licence; mako ships no value.
    Betreiber,
}

/// When a value for a period is due, as its publication rule states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "art", rename_all = "snake_case")]
pub enum Faelligkeit {
    /// The legal anchor names no publication date; the value is due when the
    /// publisher releases it, and a reader refuses until then.
    Ungeregelt {
        /// What the source says, so the refusal can repeat it.
        hinweis: &'static str,
    },
    /// The rule names a day after the period, in a unit no date can be
    /// computed for yet (the source does not define its Werktag).
    Regel {
        /// The rule, as its source states it.
        regel: &'static str,
    },
}

/// Everything the catalogue knows about one series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReiheInfo {
    /// The stable key — the `reihe` column, the route segment, the MCP argument.
    pub schluessel: &'static str,
    /// German display name.
    pub bezeichnung: &'static str,
    /// Unit of `wert`.
    pub einheit: &'static str,
    /// The period one value covers.
    pub granularitaet: Granularitaet,
    /// The admissible dimension values; empty when the series has none.
    pub dimensionen: &'static [&'static str],
    /// Who publishes it.
    pub herausgeber: &'static str,
    /// Where the publisher puts it.
    pub quelle_url: &'static str,
    /// The norm that makes billing read it.
    pub rechtsgrundlage: &'static str,
    /// When a value is due.
    pub faelligkeit: Faelligkeit,
    /// Who may distribute it.
    pub lizenz: Lizenz,
}

/// Destatis' GENESIS-Online terms, read on 2026-11-16 from the Impressum: the
/// database is under „Datenlizenz Deutschland - Namensnennung - Version 2.0".
const DESTATIS: Lizenz = Lizenz::Frei {
    namensnennung: "© Statistisches Bundesamt (Destatis), Datenlizenz Deutschland – Namensnennung – Version 2.0",
    lizenz_url: "https://www.destatis.de/DE/Service/Impressum/_inhalt.html",
};

/// The Energieträger dimensions of EEG 2023 Anlage 1 Nr. 3.2/3.3 and 4.2/4.3:
/// `spot` is the plain „MW"/„JW" (Wasserkraft, Deponie-, Klär-, Grubengas,
/// Biomasse, Geothermie), the others the energieträgerspezifische values.
const EEG_ENERGIETRAEGER: &[&str] = &["spot", "wind_an_land", "wind_auf_see", "solar"];

impl Reihe {
    /// Every catalogued series.
    pub const ALL: [Self; 5] = [
        Self::KavGrenzpreisStrom,
        Self::KavDurchschnittserloesGas,
        Self::EegMonatsmarktwert,
        Self::EegJahresmarktwert,
        Self::DayAheadDeLu,
    ];

    /// The catalogue entry.
    #[must_use]
    pub const fn info(self) -> ReiheInfo {
        match self {
            Self::KavGrenzpreisStrom => ReiheInfo {
                schluessel: "kav_grenzpreis_strom",
                bezeichnung: "KAV-Grenzpreis Strom (Durchschnittserlös Sondervertragskunden)",
                einheit: "ct/kWh",
                granularitaet: Granularitaet::Kalenderjahr,
                dimensionen: &[],
                herausgeber: "Statistisches Bundesamt (amtliche Statistik des Bundes)",
                quelle_url: "https://www-genesis.destatis.de/",
                rechtsgrundlage: "KAV § 2 Abs. 4",
                faelligkeit: Faelligkeit::Ungeregelt {
                    hinweis: "KAV § 2 Abs. 4 nennt keinen Veröffentlichungstermin; maßgeblich ist \
                              der für das vorletzte Kalenderjahr veröffentlichte Wert",
                },
                lizenz: DESTATIS,
            },
            Self::KavDurchschnittserloesGas => ReiheInfo {
                schluessel: "kav_durchschnittserloes_gas",
                bezeichnung: "Durchschnittserlös Gas aller Letztverbraucher (KAV § 2 Abs. 5 Nr. 2)",
                einheit: "ct/kWh",
                granularitaet: Granularitaet::Kalenderjahr,
                dimensionen: &[],
                herausgeber: "Statistisches Bundesamt (amtliche Statistik des Bundes)",
                quelle_url: "https://www-genesis.destatis.de/",
                rechtsgrundlage: "KAV § 2 Abs. 5 Nr. 2",
                faelligkeit: Faelligkeit::Ungeregelt {
                    hinweis: "KAV § 2 Abs. 5 Nr. 2 nennt keinen Veröffentlichungstermin",
                },
                lizenz: DESTATIS,
            },
            Self::EegMonatsmarktwert => ReiheInfo {
                schluessel: "eeg_monatsmarktwert",
                bezeichnung: "EEG-Monatsmarktwert",
                einheit: "ct/kWh",
                granularitaet: Granularitaet::Kalendermonat,
                dimensionen: EEG_ENERGIETRAEGER,
                herausgeber: "Übertragungsnetzbetreiber (gemeinsame Internetseite)",
                quelle_url: "https://www.netztransparenz.de/",
                rechtsgrundlage: "EEG 2023 Anlage 1 Nr. 3",
                faelligkeit: Faelligkeit::Regel {
                    regel: "EEG 2023 Anlage 1 Nr. 5.2: bis zum Ablauf des zehnten Werktags des \
                            Folgemonats",
                },
                lizenz: Lizenz::Betreiber,
            },
            Self::EegJahresmarktwert => ReiheInfo {
                schluessel: "eeg_jahresmarktwert",
                bezeichnung: "EEG-Jahresmarktwert",
                einheit: "ct/kWh",
                granularitaet: Granularitaet::Kalenderjahr,
                dimensionen: EEG_ENERGIETRAEGER,
                herausgeber: "Übertragungsnetzbetreiber (gemeinsame Internetseite)",
                quelle_url: "https://www.netztransparenz.de/",
                rechtsgrundlage: "EEG 2023 Anlage 1 Nr. 4",
                faelligkeit: Faelligkeit::Regel {
                    regel: "EEG 2023 Anlage 1 Nr. 5.3: bis zum Ablauf des zehnten Werktages des \
                            Folgejahres",
                },
                lizenz: Lizenz::Betreiber,
            },
            Self::DayAheadDeLu => ReiheInfo {
                schluessel: "day_ahead_de_lu",
                bezeichnung: "Day-Ahead-Spotmarktpreis Preiszone DE-LU",
                einheit: "ct/kWh",
                granularitaet: Granularitaet::Viertelstunde,
                dimensionen: &[],
                herausgeber: "Strombörsen (gekoppelte Day-Ahead-Auktion)",
                quelle_url: "https://www.epexspot.com/",
                rechtsgrundlage: "EEG 2023 § 3 Nr. 42a",
                faelligkeit: Faelligkeit::Ungeregelt {
                    hinweis: "Ergebnis der vortägigen Auktion; der Betreiber lädt die Werte unter \
                              seiner eigenen Lizenz",
                },
                lizenz: Lizenz::Betreiber,
            },
        }
    }

    /// The stable key.
    #[must_use]
    pub const fn schluessel(self) -> &'static str {
        self.info().schluessel
    }

    /// The series a key names.
    #[must_use]
    pub fn from_schluessel(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.schluessel() == key)
    }

    /// The values mako ships for this series.
    ///
    /// Empty for every series today: a `Betreiber` series may ship none, and no
    /// `Frei` value has been read from a document mirrored in the repository —
    /// a seeded figure without its document is one nobody can check.
    #[must_use]
    pub const fn seed(self) -> &'static [SeedWert] {
        match self {
            Self::KavGrenzpreisStrom
            | Self::KavDurchschnittserloesGas
            | Self::EegMonatsmarktwert
            | Self::EegJahresmarktwert
            | Self::DayAheadDeLu => &[],
        }
    }

    /// Whether `dim` is admissible for this series (`""` for a series without
    /// dimensions).
    #[must_use]
    pub fn admits_dim(self, dim: &str) -> bool {
        let dims = self.info().dimensionen;
        if dims.is_empty() {
            dim.is_empty()
        } else {
            dims.contains(&dim)
        }
    }
}

impl std::fmt::Display for Reihe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.schluessel())
    }
}

/// One value mako ships for a `Frei` series, with the document it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedWert {
    /// The dimension value, `""` for none.
    pub dim: &'static str,
    /// The period.
    pub zeitraum: Zeitraum,
    /// The value, in the series' unit.
    pub wert: Decimal,
    /// The document the value was read from.
    pub quelle_url: &'static str,
}

/// A German calendar period a value is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "snake_case")]
pub enum Zeitraum {
    /// A calendar year.
    Jahr {
        /// The year.
        jahr: i32,
    },
    /// A calendar month.
    Monat {
        /// The year.
        jahr: i32,
        /// The month, 1–12.
        monat: u8,
    },
}

/// A period that does not exist (month 13, a year `time` cannot hold).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("kein gültiger Zeitraum: {0}")]
pub struct UngueltigerZeitraum(pub String);

impl Zeitraum {
    /// The granularity this period has.
    #[must_use]
    pub const fn granularitaet(self) -> Granularitaet {
        match self {
            Self::Jahr { .. } => Granularitaet::Kalenderjahr,
            Self::Monat { .. } => Granularitaet::Kalendermonat,
        }
    }

    /// The first and the day after the last day of the period.
    ///
    /// # Errors
    /// A month outside 1–12 or a year `time` cannot represent.
    pub fn tage(self) -> Result<(Date, Date), UngueltigerZeitraum> {
        let bad = || UngueltigerZeitraum(format!("{self:?}"));
        match self {
            Self::Jahr { jahr } => {
                let von = Date::from_calendar_date(jahr, Month::January, 1).map_err(|_| bad())?;
                let bis =
                    Date::from_calendar_date(jahr + 1, Month::January, 1).map_err(|_| bad())?;
                Ok((von, bis))
            }
            Self::Monat { jahr, monat } => {
                let m = Month::try_from(monat).map_err(|_| bad())?;
                let von = Date::from_calendar_date(jahr, m, 1).map_err(|_| bad())?;
                let (nj, nm) = if m == Month::December {
                    (jahr + 1, Month::January)
                } else {
                    (jahr, m.next())
                };
                let bis = Date::from_calendar_date(nj, nm, 1).map_err(|_| bad())?;
                Ok((von, bis))
            }
        }
    }

    /// The half-open instant range `[von, bis)` between the Berlin midnights
    /// that bound the period, so a DST change is handled once, here.
    ///
    /// # Errors
    /// As [`Self::tage`].
    pub fn instants(self) -> Result<(OffsetDateTime, OffsetDateTime), UngueltigerZeitraum> {
        let (von, bis) = self.tage()?;
        Ok((
            mako_fristen::berlin_midnight(von),
            mako_fristen::berlin_midnight(bis),
        ))
    }
}

impl std::fmt::Display for Zeitraum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jahr { jahr } => write!(f, "{jahr}"),
            Self::Monat { jahr, monat } => write!(f, "{jahr}-{monat:02}"),
        }
    }
}

/// Whether a value is the publisher's provisional or its final figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WertStatus {
    /// Provisional.
    Vorlaeufig,
    /// Final.
    Endgueltig,
}

impl WertStatus {
    /// The `status` column value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vorlaeufig => "vorlaeufig",
            Self::Endgueltig => "endgueltig",
        }
    }

    /// The status a column value names.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "vorlaeufig" => Some(Self::Vorlaeufig),
            "endgueltig" => Some(Self::Endgueltig),
            _ => None,
        }
    }
}

/// One value as marktd serves it.
///
/// A response body from another service: unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Referenzwert {
    /// The row id — what a receipt records it read.
    pub id: uuid::Uuid,
    /// The series key.
    pub reihe: String,
    /// The dimension value, `""` for none.
    pub dim: String,
    /// Start of the valid period (inclusive).
    #[serde(with = "time::serde::rfc3339")]
    pub gilt_von: OffsetDateTime,
    /// End of the valid period (exclusive).
    #[serde(with = "time::serde::rfc3339")]
    pub gilt_bis: OffsetDateTime,
    /// The value, in the series' unit.
    #[serde(with = "rust_decimal::serde::str")]
    pub wert: Decimal,
    /// Provisional or final.
    pub status: WertStatus,
    /// Where the value was published.
    pub quelle_url: String,
    /// When the publisher released it, if known.
    pub veroeffentlicht_am: Option<Date>,
    /// SHA-256 of the imported document, if the value came from one.
    pub dokument_sha256: Option<String>,
    /// How the value entered marktd: `seed`, `import` or `api`.
    pub herkunft: String,
    /// When marktd learned it.
    #[serde(with = "time::serde::rfc3339")]
    pub erfasst_von: OffsetDateTime,
    /// When marktd superseded it; `None` while it is current.
    #[serde(with = "time::serde::rfc3339::option")]
    pub erfasst_bis: Option<OffsetDateTime>,
}

/// The `422 referenzwert_fehlt` body: no value for the period was known at the
/// asked instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenzwertFehlt {
    /// The series key.
    pub reihe: String,
    /// The dimension value asked for.
    pub dim: String,
    /// The period asked for.
    pub zeitraum: String,
    /// The publication rule, so the refusal says when to expect it.
    pub faelligkeit: String,
}

impl ReferenzwertFehlt {
    /// The refusal for `reihe`/`dim`/`zeitraum`.
    #[must_use]
    pub fn new(reihe: Reihe, dim: &str, zeitraum: Zeitraum) -> Self {
        Self {
            reihe: reihe.schluessel().to_owned(),
            dim: dim.to_owned(),
            zeitraum: zeitraum.to_string(),
            faelligkeit: match reihe.info().faelligkeit {
                Faelligkeit::Ungeregelt { hinweis } => hinweis.to_owned(),
                Faelligkeit::Regel { regel } => regel.to_owned(),
            },
        }
    }
}

impl std::fmt::Display for ReferenzwertFehlt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Referenzreihe {} hat keinen Wert für {}{} — {}",
            self.reihe,
            self.zeitraum,
            if self.dim.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.dim)
            },
            self.faelligkeit
        )
    }
}

/// The stable `code` of the refusal.
pub const REFERENZWERT_FEHLT: &str = "referenzwert_fehlt";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_round_trips_and_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for r in Reihe::ALL {
            assert!(seen.insert(r.schluessel()), "duplicate key {r}");
            assert_eq!(Reihe::from_schluessel(r.schluessel()), Some(r));
            // The serde name is the key, so a JSON reihe and a route segment agree.
            assert_eq!(
                serde_json::to_value(r).expect("serialises"),
                serde_json::Value::String(r.schluessel().to_owned())
            );
        }
        assert_eq!(Reihe::from_schluessel("kav_grenzpreis"), None);
    }

    /// A `Betreiber` series ships no value — its licence is the operator's.
    #[test]
    fn no_betreiber_series_ships_a_seed() {
        for r in Reihe::ALL {
            if r.info().lizenz == Lizenz::Betreiber {
                assert!(r.seed().is_empty(), "{r} is Betreiber and seeds a value");
            }
        }
    }

    /// A year runs between Berlin midnights: 2025 opens at 23:00 UTC on
    /// 31.12.2024 (CET) and a summer month at 22:00 UTC (CEST).
    #[test]
    fn periods_run_between_berlin_midnights() {
        let (von, bis) = Zeitraum::Jahr { jahr: 2025 }.instants().expect("valid");
        assert_eq!(von, time::macros::datetime!(2024-12-31 23:00 UTC));
        assert_eq!(bis, time::macros::datetime!(2025-12-31 23:00 UTC));
        let (von, bis) = Zeitraum::Monat {
            jahr: 2026,
            monat: 7,
        }
        .instants()
        .expect("valid");
        assert_eq!(von, time::macros::datetime!(2026-06-30 22:00 UTC));
        assert_eq!(bis, time::macros::datetime!(2026-07-31 22:00 UTC));
        assert!(
            Zeitraum::Monat {
                jahr: 2026,
                monat: 13
            }
            .tage()
            .is_err()
        );
    }

    #[test]
    fn dimensions_are_held_to_the_catalogue() {
        assert!(Reihe::KavGrenzpreisStrom.admits_dim(""));
        assert!(!Reihe::KavGrenzpreisStrom.admits_dim("solar"));
        assert!(Reihe::EegMonatsmarktwert.admits_dim("solar"));
        assert!(!Reihe::EegMonatsmarktwert.admits_dim(""));
    }
}
