//! The **Paket-ID** — the identity of one NB-Wechsel, and its lifecycle.
//!
//! „Die Paket-ID identifiziert die von einem NB-Wechsel betroffenen Lokationen
//! und wird an den vom NB-Wechsel betroffenen Marktlokationen kommuniziert. Eine
//! Paket-ID kann somit einer Marktlokation bis zu allen Marktlokationen des NBA
//! zugeordnet sein" (Strom Kap. 2.2). It is applied for by the **NBA** at the
//! Energie Codes & Services GmbH, published in a generally accessible list so
//! that any Marktpartner can see an upcoming NB-Wechsel, and then carried at
//! every affected Marktlokation.
//!
//! **Strom only.** The Gas Anwendungshilfe contains no Paket-ID: there, NBA and
//! NBN „haben sich im Vorfeld über die vom NB-Wechsel betroffenen Lokationen
//! verständigt" (Gas Kap. 3.1 Rahmenbedingung 3).
//!
//! # The three cases of Kap. 3
//!
//! | NBN at application time | What the NBA files | Modelled as |
//! |---|---|---|
//! | not yet known | sprechender Name, MP-ID NBA, **geplanter** Änderungszeitpunkt | [`PaketAntrag::NbnUnbekannt`] |
//! | known, different from the NBA | sprechender Name, MP-ID NBA, MP-ID NBN, Änderungszeitpunkt | [`PaketAntrag::NbnBekannt`] |
//! | known, **identical** to the NBA | nothing — „ist keine Paket-ID anzulegen" | [`PaketAntragFehler::NbnIdentischMitNba`] |
//!
//! Kap. 3 excludes the third case from the rest of the document, and Kap. 1.2
//! („Abgrenzung") says why: where the MP-ID of the NB does not change, no
//! Abrechnungs-, Stamm- oder Bewegungsdaten of a Lokation change either.
//!
//! # Identity discovered late is not the same refusal
//!
//! When the NBN was **unknown** at application the Paket-ID already exists, and
//! Kap. 3 requires the NBN to be reported to the Energie Codes & Services GmbH
//! „auch dann …, wenn NBA und NBN identisch sind. In der Liste ist somit für die
//! Paket-ID ersichtlich, dass kein NB-Wechsel stattfinden wird." That is
//! [`PaketStatus::KeinNbWechsel`] — a Paket-ID that exists and is published, on
//! a handover that will not happen. Collapsing it into the application refusal
//! would delete an entry the market reads.

use serde::{Deserialize, Serialize};

use crate::{Aenderungszeitpunkt, Sparte};

pub use crate::zeitpunkt::{NBN_MELDUNG_VORLAUF_MONATE, PAKET_ID_VORLAUF_MONATE};

/// A Paket-ID as issued by the Energie Codes & Services GmbH.
///
/// # The Anwendungshilfe states no format
///
/// Kap. 3 describes what the NBA *supplies* — a sprechender Name, one or two
/// MP-IDs and a date — and says that the Paket-ID is created by the Energie
/// Codes & Services GmbH and communicated back to the NBA. It states no length,
/// no character set and no check digit, and neither does Kap. 2.2. This type
/// therefore accepts any non-empty token and normalises only surrounding
/// whitespace. Inventing a structural check here would refuse identifiers the
/// issuing body is free to hand out, and a rejected Paket-ID stops every one of
/// the Kap.-5 milestones — the failure is total and it is the crate's fault, not
/// the market's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct PaketId(String);

impl PaketId {
    /// Wrap an issued Paket-ID, trimming surrounding whitespace.
    ///
    /// # Errors
    ///
    /// [`PaketIdFehler`] when nothing but whitespace is left.
    pub fn neu(wert: impl Into<String>) -> Result<Self, PaketIdFehler> {
        let wert: String = wert.into();
        let getrimmt = wert.trim();
        if getrimmt.is_empty() {
            return Err(PaketIdFehler::Leer);
        }
        Ok(Self(getrimmt.to_owned()))
    }

    /// The issued token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PaketId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserialization goes through [`PaketId::neu`].
///
/// A Paket-ID reaches a service as JSON far more often than it is built in
/// code; deriving this would let a payload produce the one value the
/// constructor rejects.
impl<'de> Deserialize<'de> for PaketId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let roh = String::deserialize(d)?;
        Self::neu(roh).map_err(serde::de::Error::custom)
    }
}

/// A value offered as a Paket-ID that carries nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PaketIdFehler {
    /// Empty or whitespace only.
    #[error(
        "eine Paket-ID ohne Inhalt — die Paket-ID wird von der Energie Codes & Services GmbH \
         vergeben und an jeder betroffenen Marktlokation kommuniziert (Kap. 2.2, Kap. 3); ohne \
         sie ist keine der betroffenen Lokationen einem NB-Wechsel zuzuordnen"
    )]
    Leer,
}

/// The Marktpartner-ID of a Netzbetreiber.
pub type MpId = rubo4e::identifiers::MarktpartnerId;

/// What the NBA files at the Energie Codes & Services GmbH (Kap. 3).
///
/// The two variants differ in exactly the way Kap. 3 distinguishes them: with
/// the NBN unknown the date is the „geplanter Änderungszeitpunkt" and there is
/// no NBN MP-ID; with the NBN known it is the „Änderungszeitpunkt" and both
/// MP-IDs are filed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "fall", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PaketAntrag {
    /// „Ist der NBN dabei noch nicht bekannt, gibt der NBA für die Paket-ID
    /// einen sprechenden Namen, die MP-ID des NBA und den „geplanten
    /// Änderungszeitpunkt" an" (Kap. 3).
    NbnUnbekannt {
        /// The sprechender Name the NBA gives the Paket.
        name: String,
        /// MP-ID of the Netzbetreiber alt.
        nba: MpId,
        /// The **geplanter** Änderungszeitpunkt — replaced by the
        /// Änderungszeitpunkt once the NBN is reported (Kap. 3).
        geplanter_aenderungszeitpunkt: Aenderungszeitpunkt,
    },
    /// „Ist der NBN bereits bekannt und nicht identisch mit dem NBA …, gibt der
    /// NBA für die Paket-ID einen sprechenden Namen, die MP-ID des NBA, die
    /// MP-ID des NBN und den „Änderungszeitpunkt" an" (Kap. 3).
    NbnBekannt {
        /// The sprechender Name the NBA gives the Paket.
        name: String,
        /// MP-ID of the Netzbetreiber alt.
        nba: MpId,
        /// MP-ID of the Netzbetreiber neu.
        nbn: MpId,
        /// The Änderungszeitpunkt.
        aenderungszeitpunkt: Aenderungszeitpunkt,
    },
}

impl PaketAntrag {
    /// Kap. 3, first case — the NBN is not yet known.
    ///
    /// # Errors
    ///
    /// [`PaketAntragFehler::NameLeer`] without a sprechender Name;
    /// [`PaketAntragFehler::SparteOhnePaketId`] for a Gas Änderungszeitpunkt.
    pub fn nbn_unbekannt(
        name: impl Into<String>,
        nba: MpId,
        geplanter_aenderungszeitpunkt: Aenderungszeitpunkt,
    ) -> Result<Self, PaketAntragFehler> {
        let name = pruefe_name(name)?;
        pruefe_sparte(geplanter_aenderungszeitpunkt)?;
        Ok(Self::NbnUnbekannt {
            name,
            nba,
            geplanter_aenderungszeitpunkt,
        })
    }

    /// Kap. 3, second and third case — the NBN is known.
    ///
    /// # Errors
    ///
    /// [`PaketAntragFehler::NbnIdentischMitNba`] when the two MP-IDs are equal:
    /// Kap. 3 then says „ist keine Paket-ID anzulegen" and excludes the case
    /// from the rest of the Prozessbeschreibung.
    pub fn nbn_bekannt(
        name: impl Into<String>,
        nba: MpId,
        nbn: MpId,
        aenderungszeitpunkt: Aenderungszeitpunkt,
    ) -> Result<Self, PaketAntragFehler> {
        let name = pruefe_name(name)?;
        pruefe_sparte(aenderungszeitpunkt)?;
        if nba == nbn {
            return Err(PaketAntragFehler::NbnIdentischMitNba { mp_id: nbn });
        }
        Ok(Self::NbnBekannt {
            name,
            nba,
            nbn,
            aenderungszeitpunkt,
        })
    }

    /// The sprechender Name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::NbnUnbekannt { name, .. } | Self::NbnBekannt { name, .. } => name,
        }
    }

    /// The MP-ID of the Netzbetreiber alt.
    #[must_use]
    pub const fn nba(&self) -> &MpId {
        match self {
            Self::NbnUnbekannt { nba, .. } | Self::NbnBekannt { nba, .. } => nba,
        }
    }

    /// The Änderungszeitpunkt — „geplant" while the NBN is unknown.
    #[must_use]
    pub const fn aenderungszeitpunkt(&self) -> Aenderungszeitpunkt {
        match self {
            Self::NbnUnbekannt {
                geplanter_aenderungszeitpunkt: z,
                ..
            }
            | Self::NbnBekannt {
                aenderungszeitpunkt: z,
                ..
            } => *z,
        }
    }
}

fn pruefe_name(name: impl Into<String>) -> Result<String, PaketAntragFehler> {
    let name: String = name.into();
    let getrimmt = name.trim();
    if getrimmt.is_empty() {
        return Err(PaketAntragFehler::NameLeer);
    }
    Ok(getrimmt.to_owned())
}

const fn pruefe_sparte(zeitpunkt: Aenderungszeitpunkt) -> Result<(), PaketAntragFehler> {
    if zeitpunkt.sparte().hat_paket_id() {
        Ok(())
    } else {
        Err(PaketAntragFehler::SparteOhnePaketId {
            sparte: zeitpunkt.sparte(),
        })
    }
}

/// Why an Antrag cannot be filed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PaketAntragFehler {
    /// Kap. 3 asks the NBA for „einen sprechenden Namen".
    #[error(
        "die Paket-ID braucht einen sprechenden Namen (Kap. 3) — er steht in der allgemein \
         zugänglichen Liste der Energie Codes & Services GmbH, über die sich Marktpartner \
         frühzeitig über bevorstehende NB-Wechsel informieren"
    )]
    NameLeer,
    /// Kap. 3, third case — „ist keine Paket-ID anzulegen".
    #[error(
        "NBN und NBA sind derselbe Marktpartner ({mp_id}) — „ist der NBN bereits bekannt und \
         identisch mit dem NBA …, ist keine Paket-ID anzulegen\" (Kap. 3). Kap. 1.2 nennt den \
         Grund: ändert sich die MP-ID des NB an einer Lokation nicht, ändern sich auch keine \
         Abrechnungs-, Stamm- oder Bewegungsdaten, und die Prozessbeschreibung ist nicht \
         durchzuführen"
    )]
    NbnIdentischMitNba {
        /// The MP-ID both sides carry.
        mp_id: MpId,
    },
    /// Applied for against a Sparte whose Anwendungshilfe has no Paket-ID.
    #[error(
        "die Sparte {sparte} kennt keine Paket-ID — {anwendungshilfe} beschreibt sie nicht; NBA \
         und NBN verständigen sich dort im Vorfeld bilateral über die betroffenen Lokationen \
         (Kap. 3.1 Rahmenbedingung 3)",
        anwendungshilfe = sparte.anwendungshilfe()
    )]
    SparteOhnePaketId {
        /// The Sparte of the offered Änderungszeitpunkt.
        sparte: Sparte,
    },
}

/// Where one Paket stands.
///
/// Kap. 3 names the two application cases, the assignment of the Paket-ID by the
/// Energie Codes & Services GmbH, the later report of the NBN and the case in
/// which the published list ends up showing that no NB-Wechsel will take place.
/// [`Self::InUmsetzung`] and [`Self::Abgeschlossen`] are this crate's own
/// bookkeeping — the Anwendungshilfe states the gate into the first of them
/// (Kap. 4 Rahmenbedingungen 1–3) but gives neither state a name, and it names
/// no completion event at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PaketStatus {
    /// Applied for with the NBN unknown (Kap. 3, first case). The date held is
    /// the „geplanter Änderungszeitpunkt".
    BeantragtOhneNbn,
    /// The NBN is known and differs from the NBA — either filed that way
    /// (Kap. 3, second case) or reported afterwards.
    BeantragtMitNbn,
    /// The NBN turned out to be the NBA. The Paket-ID exists and stays in the
    /// published list, „in der Liste ist somit für die Paket-ID ersichtlich,
    /// dass kein NB-Wechsel stattfinden wird" (Kap. 3). No milestone of Kap. 5
    /// is owed.
    KeinNbWechsel,
    /// Kap. 4 Rahmenbedingungen 1–3 are met: the NBN is fixed and has an MP-ID,
    /// the Änderungszeitpunkt is known, and the Paket-ID is held by both NBA and
    /// NBN.
    InUmsetzung,
    /// The handover is done.
    Abgeschlossen,
}

/// One NB-Wechsel, from Antrag to Abschluss.
///
/// Serialized but not deserialized: the type's whole value is that its states are
/// only reachable through the transitions Kap. 3 and Kap. 4 permit, and a derived
/// `Deserialize` is a way around all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Paket {
    name: String,
    nba: MpId,
    nbn: Option<MpId>,
    aenderungszeitpunkt: Aenderungszeitpunkt,
    id: Option<PaketId>,
    status: PaketStatus,
}

impl Paket {
    /// File the Antrag. The Paket-ID itself is not yet known — the Energie
    /// Codes & Services GmbH assigns it and communicates it to the NBA
    /// ([`Self::id_zugeteilt`]).
    #[must_use]
    pub fn beantragen(antrag: PaketAntrag) -> Self {
        match antrag {
            PaketAntrag::NbnUnbekannt {
                name,
                nba,
                geplanter_aenderungszeitpunkt,
            } => Self {
                name,
                nba,
                nbn: None,
                aenderungszeitpunkt: geplanter_aenderungszeitpunkt,
                id: None,
                status: PaketStatus::BeantragtOhneNbn,
            },
            PaketAntrag::NbnBekannt {
                name,
                nba,
                nbn,
                aenderungszeitpunkt,
            } => Self {
                name,
                nba,
                nbn: Some(nbn),
                aenderungszeitpunkt,
                id: None,
                status: PaketStatus::BeantragtMitNbn,
            },
        }
    }

    /// „Ist die Paket-ID angelegt, wird diese dem NBA durch die Energie Codes &
    /// Services GmbH mitgeteilt" (Kap. 3).
    ///
    /// # Errors
    ///
    /// [`PaketFehler::IdBereitsZugeteilt`] — a Paket-ID identifies the affected
    /// Lokationen and is carried at each of them, so replacing it silently
    /// orphans every Marktlokation already tagged with the old one.
    pub fn id_zugeteilt(&mut self, id: PaketId) -> Result<(), PaketFehler> {
        if let Some(vorhanden) = &self.id {
            return Err(PaketFehler::IdBereitsZugeteilt {
                vorhanden: vorhanden.clone(),
                angeboten: id,
            });
        }
        self.id = Some(id);
        Ok(())
    }

    /// Report the NBN to the Energie Codes & Services GmbH once it is fixed
    /// (Kap. 3).
    ///
    /// The „geplanter Änderungszeitpunkt" is replaced by the Änderungszeitpunkt
    /// in the same act. When the NBN turns out to be the NBA the Paket moves to
    /// [`PaketStatus::KeinNbWechsel`] rather than being refused: Kap. 3 requires
    /// the report „auch dann …, wenn NBA und NBN identisch sind", and the
    /// published list then shows that no NB-Wechsel will take place.
    ///
    /// Lateness is **not** refused here. Kap. 3 sets the report at „spätestens
    /// 4 Monate vor dem Änderungszeitpunkt", but a report that misses it still
    /// has to happen — refusing it would leave the list wrong forever. Ask
    /// [`Self::nbn_meldung_fristgerecht`] whether the Frist was kept.
    ///
    /// # Errors
    ///
    /// [`PaketFehler::FalscherStatus`] unless the Paket is still waiting for its
    /// NBN.
    pub fn nbn_gemeldet(
        &mut self,
        nbn: MpId,
        aenderungszeitpunkt: Aenderungszeitpunkt,
    ) -> Result<PaketStatus, PaketFehler> {
        if self.status != PaketStatus::BeantragtOhneNbn {
            return Err(PaketFehler::FalscherStatus {
                erwartet: PaketStatus::BeantragtOhneNbn,
                ist: self.status,
                aktion: "Meldung des NBN",
            });
        }
        self.status = if nbn == self.nba {
            PaketStatus::KeinNbWechsel
        } else {
            PaketStatus::BeantragtMitNbn
        };
        self.nbn = Some(nbn);
        self.aenderungszeitpunkt = aenderungszeitpunkt;
        Ok(self.status)
    }

    /// Whether a report of the NBN made on `am` keeps the Kap.-3 Frist,
    /// „spätestens 4 Monate vor dem Änderungszeitpunkt".
    #[must_use]
    pub fn nbn_meldung_fristgerecht(&self, am: time::Date) -> bool {
        self.aenderungszeitpunkt
            .spaeteste_nbn_meldung()
            .is_none_or(|frist| am <= frist)
    }

    /// Whether a Paket-ID application made on `am` keeps the Kap.-3 Frist,
    /// „spätestens 6 Monate vor dem geplanten Änderungszeitpunkt".
    #[must_use]
    pub fn antrag_fristgerecht(&self, am: time::Date) -> bool {
        self.aenderungszeitpunkt
            .spaetester_paket_id_antrag()
            .is_none_or(|frist| am <= frist)
    }

    /// Enter the Kap.-5 milestones, once Kap. 4 Rahmenbedingungen 1–3 hold:
    /// the NBN is fixed and has an MP-ID, the Änderungszeitpunkt is known, and
    /// the Paket-ID is held by both NBA and NBN.
    ///
    /// # Errors
    ///
    /// [`PaketFehler::RahmenbedingungOffen`] when the Paket-ID has not been
    /// assigned; [`PaketFehler::KeinNbWechsel`] when NBA and NBN turned out
    /// identical; [`PaketFehler::FalscherStatus`] otherwise.
    pub fn umsetzung_beginnen(&mut self) -> Result<(), PaketFehler> {
        match self.status {
            PaketStatus::BeantragtMitNbn => {}
            PaketStatus::KeinNbWechsel => return Err(PaketFehler::KeinNbWechsel),
            ist => {
                return Err(PaketFehler::FalscherStatus {
                    erwartet: PaketStatus::BeantragtMitNbn,
                    ist,
                    aktion: "Beginn der Umsetzung",
                });
            }
        }
        if self.id.is_none() {
            return Err(PaketFehler::RahmenbedingungOffen {
                nummer: 3,
                text: "Eine Paket-ID liegt dem NBA und dem NBN vor",
            });
        }
        self.status = PaketStatus::InUmsetzung;
        Ok(())
    }

    /// Close the Paket.
    ///
    /// # Errors
    ///
    /// [`PaketFehler::FalscherStatus`] unless the Paket is in Umsetzung.
    pub fn abschliessen(&mut self) -> Result<(), PaketFehler> {
        if self.status != PaketStatus::InUmsetzung {
            return Err(PaketFehler::FalscherStatus {
                erwartet: PaketStatus::InUmsetzung,
                ist: self.status,
                aktion: "Abschluss",
            });
        }
        self.status = PaketStatus::Abgeschlossen;
        Ok(())
    }

    /// The sprechender Name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The MP-ID of the Netzbetreiber alt.
    #[must_use]
    pub const fn nba(&self) -> &MpId {
        &self.nba
    }

    /// The MP-ID of the Netzbetreiber neu, once it is known.
    #[must_use]
    pub const fn nbn(&self) -> Option<&MpId> {
        self.nbn.as_ref()
    }

    /// The Änderungszeitpunkt — the „geplanter Änderungszeitpunkt" while the
    /// status is [`PaketStatus::BeantragtOhneNbn`].
    #[must_use]
    pub const fn aenderungszeitpunkt(&self) -> Aenderungszeitpunkt {
        self.aenderungszeitpunkt
    }

    /// The assigned Paket-ID, once the Energie Codes & Services GmbH has
    /// communicated it.
    #[must_use]
    pub const fn id(&self) -> Option<&PaketId> {
        self.id.as_ref()
    }

    /// Where the Paket stands.
    #[must_use]
    pub const fn status(&self) -> PaketStatus {
        self.status
    }

    /// Whether the Kap.-5 milestones are owed at all.
    ///
    /// False for [`PaketStatus::KeinNbWechsel`]: Kap. 1.2 keeps the
    /// Prozessbeschreibung out of the case where the MP-ID of the NB does not
    /// change.
    #[must_use]
    pub const fn ist_nb_wechsel(&self) -> bool {
        !matches!(self.status, PaketStatus::KeinNbWechsel)
    }
}

/// Why a Paket cannot make the requested transition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PaketFehler {
    /// The transition is not defined from the current state.
    #[error("{aktion} ist im Status {ist:?} nicht möglich (erwartet: {erwartet:?})")]
    FalscherStatus {
        /// The state the transition starts from.
        erwartet: PaketStatus,
        /// The state the Paket is actually in.
        ist: PaketStatus,
        /// What was attempted.
        aktion: &'static str,
    },
    /// A second Paket-ID was offered for the same Paket.
    #[error(
        "die Paket-ID {vorhanden} ist bereits zugeteilt und kann nicht durch {angeboten} ersetzt \
         werden — sie wird an jeder betroffenen Marktlokation kommuniziert (Kap. 2.2), sodass ein \
         Austausch alle bereits zugeordneten Lokationen verwaist zurücklässt"
    )]
    IdBereitsZugeteilt {
        /// The Paket-ID already assigned.
        vorhanden: PaketId,
        /// The one offered.
        angeboten: PaketId,
    },
    /// One of Kap. 4's Rahmenbedingungen is not met.
    #[error("Rahmenbedingung {nummer} aus Kap. 4 ist offen: {text}")]
    RahmenbedingungOffen {
        /// Its number in Kap. 4.
        nummer: u32,
        /// Its wording.
        text: &'static str,
    },
    /// The reported NBN is the NBA.
    #[error(
        "NBA und NBN sind identisch — es findet kein NB-Wechsel statt (Kap. 3), und die \
         Prozessbeschreibung ist nach Kap. 1.2 nicht durchzuführen. Die Paket-ID bleibt in der \
         Liste der Energie Codes & Services GmbH sichtbar"
    )]
    KeinNbWechsel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn heute() -> Date {
        d(2026, Month::January, 15)
    }

    fn az(sparte: Sparte) -> Aenderungszeitpunkt {
        Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute(), sparte)
            .expect("gültiger Änderungszeitpunkt")
    }

    fn nba() -> MpId {
        MpId::new("9900000000001").expect("13-stellige MP-ID")
    }

    fn nbn() -> MpId {
        MpId::new("9900000000002").expect("13-stellige MP-ID")
    }

    /// Kap. 3 states no format for the Paket-ID, so the only thing that can be
    /// refused is a token carrying nothing.
    #[test]
    fn a_paket_id_is_an_opaque_non_empty_token() {
        assert_eq!(
            PaketId::neu("  NBW-2027-MUSTERSTADT ")
                .expect("gültig")
                .as_str(),
            "NBW-2027-MUSTERSTADT"
        );
        assert_eq!(PaketId::neu("   "), Err(PaketIdFehler::Leer));
        assert_eq!(PaketId::neu(""), Err(PaketIdFehler::Leer));
    }

    /// The Paket-ID usually arrives as JSON, so `Deserialize` runs the same
    /// check the constructor does.
    #[test]
    fn deserialization_cannot_bypass_the_constructor() {
        let ok: PaketId = serde_json::from_str("\"NBW-2027-MUSTERSTADT\"").expect("gültig");
        assert_eq!(ok.as_str(), "NBW-2027-MUSTERSTADT");
        assert!(serde_json::from_str::<PaketId>("\"\"").is_err());
    }

    /// Kap. 3, third case — „Ist der NBN bereits bekannt und identisch mit dem
    /// NBA …, ist keine Paket-ID anzulegen."
    #[test]
    fn an_nbn_identical_to_the_nba_gets_no_paket_id() {
        let err = PaketAntrag::nbn_bekannt("Musterstadt", nba(), nba(), az(Sparte::Strom))
            .expect_err("Kap. 3 legt hier keine Paket-ID an");
        assert!(matches!(err, PaketAntragFehler::NbnIdentischMitNba { .. }));
        assert!(
            err.to_string().contains("keine Paket-ID anzulegen"),
            "{err}"
        );
    }

    /// Kap. 3 asks for „einen sprechenden Namen" — it is what a Marktpartner
    /// reads in the published list.
    #[test]
    fn the_antrag_needs_a_sprechender_name() {
        assert_eq!(
            PaketAntrag::nbn_unbekannt("  ", nba(), az(Sparte::Strom)),
            Err(PaketAntragFehler::NameLeer)
        );
    }

    /// The Gas Anwendungshilfe describes no Paket-ID at all.
    #[test]
    fn gas_has_no_paket_id_to_apply_for() {
        assert_eq!(
            PaketAntrag::nbn_unbekannt("Musterstadt", nba(), az(Sparte::Gas)),
            Err(PaketAntragFehler::SparteOhnePaketId {
                sparte: Sparte::Gas
            })
        );
    }

    /// The lifecycle of Kap. 3 with the NBN unknown at application: beantragt →
    /// Paket-ID zugeteilt → NBN gemeldet → in Umsetzung → abgeschlossen.
    #[test]
    fn the_full_lifecycle_with_an_unknown_nbn() {
        let antrag = PaketAntrag::nbn_unbekannt("Musterstadt", nba(), az(Sparte::Strom))
            .expect("gültiger Antrag");
        let mut paket = Paket::beantragen(antrag);
        assert_eq!(paket.status(), PaketStatus::BeantragtOhneNbn);
        assert_eq!(paket.nbn(), None);

        paket
            .id_zugeteilt(PaketId::neu("NBW-2027-MUSTERSTADT").unwrap())
            .expect("erste Zuteilung");
        // Kap. 4 Rahmenbedingung 1 is still open — the NBN is not fixed.
        assert!(paket.umsetzung_beginnen().is_err());

        assert_eq!(
            paket
                .nbn_gemeldet(nbn(), az(Sparte::Strom))
                .expect("Meldung zulässig"),
            PaketStatus::BeantragtMitNbn
        );
        paket.umsetzung_beginnen().expect("Kap. 4 Nr. 1–3 erfüllt");
        assert_eq!(paket.status(), PaketStatus::InUmsetzung);
        paket.abschliessen().expect("Abschluss");
        assert_eq!(paket.status(), PaketStatus::Abgeschlossen);
    }

    /// Kap. 4 Rahmenbedingung 3 — „Eine Paket-ID liegt dem NBA und dem NBN vor".
    /// Without it none of the Kap.-5 milestones can be started.
    #[test]
    fn umsetzung_needs_the_assigned_paket_id() {
        let antrag = PaketAntrag::nbn_bekannt("Musterstadt", nba(), nbn(), az(Sparte::Strom))
            .expect("gültiger Antrag");
        let mut paket = Paket::beantragen(antrag);
        assert!(matches!(
            paket.umsetzung_beginnen(),
            Err(PaketFehler::RahmenbedingungOffen { nummer: 3, .. })
        ));
        paket
            .id_zugeteilt(PaketId::neu("NBW-2027-MUSTERSTADT").unwrap())
            .unwrap();
        assert!(paket.umsetzung_beginnen().is_ok());
    }

    /// Kap. 3 — the NBN is reported „auch dann …, wenn NBA und NBN identisch
    /// sind", and the list then shows that no NB-Wechsel will take place. That
    /// is not the application refusal: the Paket-ID already exists.
    #[test]
    fn an_nbn_reported_late_as_identical_ends_the_wechsel_without_deleting_the_paket() {
        let antrag = PaketAntrag::nbn_unbekannt("Musterstadt", nba(), az(Sparte::Strom)).unwrap();
        let mut paket = Paket::beantragen(antrag);
        paket
            .id_zugeteilt(PaketId::neu("NBW-2027-MUSTERSTADT").unwrap())
            .unwrap();
        assert_eq!(
            paket.nbn_gemeldet(nba(), az(Sparte::Strom)).unwrap(),
            PaketStatus::KeinNbWechsel
        );
        assert!(!paket.ist_nb_wechsel());
        assert!(paket.id().is_some(), "die Paket-ID bleibt in der Liste");
        assert!(matches!(
            paket.umsetzung_beginnen(),
            Err(PaketFehler::KeinNbWechsel)
        ));
    }

    /// The Paket-ID is carried at every affected Marktlokation (Kap. 2.2), so it
    /// cannot be swapped once assigned.
    #[test]
    fn an_assigned_paket_id_is_not_replaced() {
        let antrag = PaketAntrag::nbn_bekannt("Musterstadt", nba(), nbn(), az(Sparte::Strom))
            .expect("gültiger Antrag");
        let mut paket = Paket::beantragen(antrag);
        paket.id_zugeteilt(PaketId::neu("A").unwrap()).unwrap();
        assert!(matches!(
            paket.id_zugeteilt(PaketId::neu("B").unwrap()),
            Err(PaketFehler::IdBereitsZugeteilt { .. })
        ));
        assert_eq!(paket.id().map(PaketId::as_str), Some("A"));
    }

    /// Kap. 3 — the Antrag is due 6 Monate, the NBN report 4 Monate before the
    /// Änderungszeitpunkt. Both are reported, not enforced: a late report still
    /// has to happen, and refusing it would leave the published list wrong.
    #[test]
    fn the_kapitel_three_fristen_are_reported_rather_than_enforced() {
        let antrag = PaketAntrag::nbn_unbekannt("Musterstadt", nba(), az(Sparte::Strom)).unwrap();
        let mut paket = Paket::beantragen(antrag);
        // Änderungszeitpunkt 2027-01-01 → Antrag bis 2026-07-01, NBN bis 2026-09-01.
        assert!(paket.antrag_fristgerecht(d(2026, Month::July, 1)));
        assert!(!paket.antrag_fristgerecht(d(2026, Month::July, 2)));
        assert!(paket.nbn_meldung_fristgerecht(d(2026, Month::September, 1)));
        assert!(!paket.nbn_meldung_fristgerecht(d(2026, Month::September, 2)));

        // A late report is still accepted.
        assert!(paket.nbn_gemeldet(nbn(), az(Sparte::Strom)).is_ok());
    }

    /// The status transitions are total: nothing reaches Abgeschlossen without
    /// passing through InUmsetzung.
    #[test]
    fn abschluss_requires_umsetzung() {
        let antrag = PaketAntrag::nbn_bekannt("Musterstadt", nba(), nbn(), az(Sparte::Strom))
            .expect("gültiger Antrag");
        let mut paket = Paket::beantragen(antrag);
        assert!(matches!(
            paket.abschliessen(),
            Err(PaketFehler::FalscherStatus { .. })
        ));
    }

    /// The report of the NBN replaces the „geplanter Änderungszeitpunkt" with
    /// the Änderungszeitpunkt (Kap. 3).
    #[test]
    fn reporting_the_nbn_replaces_the_planned_date() {
        let geplant = Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute(), Sparte::Strom)
            .expect("gültig");
        let endgueltig = Aenderungszeitpunkt::neu(d(2027, Month::April, 1), heute(), Sparte::Strom)
            .expect("gültig");
        let mut paket =
            Paket::beantragen(PaketAntrag::nbn_unbekannt("Musterstadt", nba(), geplant).unwrap());
        assert_eq!(paket.aenderungszeitpunkt(), geplant);
        paket.nbn_gemeldet(nbn(), endgueltig).unwrap();
        assert_eq!(paket.aenderungszeitpunkt(), endgueltig);
    }
}
