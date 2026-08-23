//! UTILMD SG4/SG5 wire vocabulary — the codes the MIG fixes, as constants.
//!
//! Every value here is quoted from the BDEW UTILMD MIG (Strom **S2.2**, Gas
//! **G1.2**, both effective 01.04.2026) rather than inferred from a sibling
//! message. The two tracks agree on all of it, so one module serves both.
//!
//! ## Why this module exists
//!
//! UTILMD encodes four different things in three segments that all look alike:
//!
//! | Wire | Meaning | Constants |
//! |---|---|---|
//! | `IDE+24+<Vorgangsnummer>` | the transaction — **never** a location ID | [`IDE_VORGANG`] |
//! | `SG5 LOC+<Z16…Z22>+<id>` | the Marktlokation / Messlokation / Tranche | [`loc`] |
//! | `SG4 DTM+<2005>` | eight distinct process dates | [`dtm`] |
//! | `SG4 STS+7` / `STS+E01` | Transaktionsgrund / **Antwortcode** | [`transaktionsgrund`], [`AntwortStatus`] |
//!
//! DE 7495 has exactly two values in UTILMD, and `163`/`164` are
//! Messperioden-Qualifier that never occur at SG4 level. A Marktlokations-ID in
//! `IDE`, or a process date as `DTM+163`, is rejected by a conformant
//! counterparty.

// ── IDE (SG4) ─────────────────────────────────────────────────────────────────

/// `IDE` DE 7495 — **the only two values UTILMD defines**.
///
/// MIG Strom S2.2 Zähler 0190 (Nr. 00010 / 00018), MIG Gas G1.2 likewise.
/// DE 7402 alongside it carries the **Vorgangsnummer**, not a location ID.
pub const IDE_VORGANG: &str = "24";

/// `IDE+Z01` — Identifikation einer Liste (`MaBiS` Summenzeitreihen).
pub const IDE_LISTE: &str = "Z01";

// ── SG5 LOC ───────────────────────────────────────────────────────────────────

/// `SG5 LOC` DE 3227 — the Lokationstyp qualifiers (MIG Zähler 0330).
pub mod loc {
    /// `LOC+Z15` — MaBiS-Zählpunkt.
    pub const MABIS_ZAEHLPUNKT: &str = "Z15";
    /// `LOC+Z16` — Marktlokation.
    pub const MARKTLOKATION: &str = "Z16";
    /// `LOC+Z17` — Messlokation.
    pub const MESSLOKATION: &str = "Z17";
    /// `LOC+Z18` — Netzlokation.
    pub const NETZLOKATION: &str = "Z18";
    /// `LOC+Z19` — Steuerbare Ressource (§14a).
    pub const STEUERBARE_RESSOURCE: &str = "Z19";
    /// `LOC+Z20` — Technische Ressource.
    pub const TECHNISCHE_RESSOURCE: &str = "Z20";
    /// `LOC+Z21` — Tranche.
    pub const TRANCHE: &str = "Z21";
    /// `LOC+Z22` — Ruhende Marktlokation (§ 20 Abs. 1d `EnWG` / § 10c EEG).
    pub const RUHENDE_MARKTLOKATION: &str = "Z22";
}

// ── SG4 DTM ───────────────────────────────────────────────────────────────────

/// `SG4 DTM` DE 2005 — the process-date qualifiers (MIG Zähler 0230).
///
/// `163`/`164` are deliberately absent: they are *Verarbeitung Beginn-/
/// Endedatum* on the SG8/SG9 Messperiode, never a SG4 process date.
pub mod dtm {
    /// `DTM+76` — Datum zum geplanten Leistungsbeginn (Nr. 00019).
    pub const LEISTUNGSBEGINN_GEPLANT: &str = "76";
    /// `DTM+92` — Beginn zum / Datum Vertragsbeginn (Nr. 00020).
    ///
    /// The Zuordnungsbeginn of every Anmeldung.
    pub const BEGINN_ZUM: &str = "92";
    /// `DTM+93` — Ende zum / Datum Vertragsende (Nr. 00021).
    ///
    /// The Zuordnungsende of every Abmeldung, Kündigung and Beendigung.
    pub const ENDE_ZUM: &str = "93";
    /// `DTM+Z05` — gegenüber Kunde bestätigtes Vertragsende (Nr. 00022).
    pub const BESTAETIGTES_VERTRAGSENDE: &str = "Z05";
    /// `DTM+157` — Änderung zum / Gültigkeit, Beginndatum (Nr. 00023).
    pub const AENDERUNG_ZUM: &str = "157";
    /// `DTM+471` — Ende zum nächstmöglichen Termin (Nr. 00024).
    ///
    /// The date an LFA returns with `Z01` „Zustimmung mit Terminänderung".
    pub const ENDE_NAECHSTMOEGLICH: &str = "471";
    /// `DTM+158` — Bilanzierungsbeginn (Nr. 00025).
    pub const BILANZIERUNGSBEGINN: &str = "158";
    /// `DTM+159` — Bilanzierungsende (Nr. 00026).
    pub const BILANZIERUNGSENDE: &str = "159";
    /// `DTM+154` — ÜT der Lieferanmeldung des LFN (Nr. 00027).
    ///
    /// The Übertragungstag the `E_0624` Prüfschritt 5 Frist runs from.
    pub const UET_LIEFERANMELDUNG: &str = "154";
    /// `DTM+Z01` — Kündigungsfrist des Vertrags (Nr. 00028).
    pub const KUENDIGUNGSFRIST: &str = "Z01";
    /// `DTM+Z10` — Kündigungstermin des Vertrags (Nr. 00029).
    pub const KUENDIGUNGSTERMIN: &str = "Z10";
    /// `DTM+137` — Dokumenten-/Nachrichtendatum (message header).
    pub const NACHRICHTENDATUM: &str = "137";
}

// ── SG4 STS ───────────────────────────────────────────────────────────────────

/// `STS` DE 9015 — Statuskategorie `7`, Transaktionsgrund (MIG Nr. 00033).
pub const STS_TRANSAKTIONSGRUND: &str = "7";

/// `STS` DE 9015 — Statuskategorie `E01`, **Status der Antwort** (MIG Nr. 00034).
///
/// Carries the EBD Antwortcode in DE 9013 and the EBD id in DE 1131:
/// `STS+E01++A10:E_0609'`.
pub const STS_STATUS_ANTWORT: &str = "E01";

/// `SG4 STS+7` DE 9013 (element 2) — Transaktionsgrund codes.
pub mod transaktionsgrund {
    /// `E01` — Ein-/Auszug (Umzug).
    pub const EIN_AUSZUG: &str = "E01";
    /// `E02` — Einzug in Neuanlage.
    pub const EINZUG_NEUANLAGE: &str = "E02";
    /// `E03` — Wechsel.
    pub const WECHSEL: &str = "E03";
    /// `E05` — Stornierung.
    pub const STORNIERUNG: &str = "E05";
    /// `E06` — Ersatzbelieferung.
    pub const ERSATZBELIEFERUNG: &str = "E06";
    /// `Z02` — Kündigung Lieferantenrahmenvertrag.
    pub const KUENDIGUNG_LRV: &str = "Z02";
    /// `Z26` — Information über existierende Zuordnung.
    pub const INFO_EXISTIERENDE_ZUORDNUNG: &str = "Z26";
    /// `Z33` — Auszug wegen Stilllegung.
    pub const AUSZUG_STILLLEGUNG: &str = "Z33";
    /// `Z36` — `EoG` aus Ein-/Auszug (Umzug).
    pub const EOG_UMZUG: &str = "Z36";
    /// `Z37` — `EoG` wegen Einzug in Neuanlage.
    pub const EOG_NEUANLAGE: &str = "Z37";
    /// `Z39` — `EoG` aus vorübergehendem Anschluss.
    pub const EOG_VORUEBERGEHEND: &str = "Z39";
    /// `Z41` — Ende der `ESV` ohne Folgelieferung.
    pub const ESV_ENDE_OHNE_FOLGE: &str = "Z41";
    /// `ZC6` — `EoG` aus Bilanzkreisschließung.
    pub const EOG_BK_SCHLIESSUNG: &str = "ZC6";
    /// `ZC7` — `EoG` aufgrund Erlöschen der Zuordnungsermächtigung.
    pub const EOG_ZUORDNUNGSERMAECHTIGUNG: &str = "ZC7";
    /// `ZC8` — Beendigung der Zuordnung.
    pub const BEENDIGUNG_ZUORDNUNG: &str = "ZC8";
}

/// `SG4 STS+7` DE 9013 (element 3) — Transaktionsgrundergänzung.
///
/// This is the element that says which *kind of object* the Vorgang is about,
/// and therefore which branch of `E_0609` / `E_0624` applies. Without it an
/// LFA cannot tell a verbrauchende Marktlokation from a Tranche.
pub mod ergaenzung {
    /// `ZW3` — Erzeugende Marktlokation.
    pub const ERZEUGENDE_MALO: &str = "ZW3";
    /// `ZW4` — Verbrauchende Marktlokation.
    pub const VERBRAUCHENDE_MALO: &str = "ZW4";
    /// `ZW5` — Tranche.
    pub const TRANCHE: &str = "ZW5";
    /// `ZAP` — Ruhende Marktlokation.
    pub const RUHENDE_MALO: &str = "ZAP";
}

// ── Typed SG4 payloads ────────────────────────────────────────────────────────

/// `SG4 STS+7` — Transaktionsgrund, Ergänzung and befristete Anmeldung.
///
/// Serialises as `STS+7++<grund>+<ergaenzung>+<befristet>'` — the MIG's own
/// example is `STS+7++E01+ZW4+E03'`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaktionsgrund {
    /// DE 9013 element 2 — the Transaktionsgrund (`E01`, `E03`, `Z33`, …).
    pub grund: String,
    /// DE 9013 element 3 — the Transaktionsgrundergänzung (`ZW3`…`ZAP`).
    pub ergaenzung: Option<String>,
    /// DE 9013 element 4 — Transaktionsgrund für das Lieferende einer
    /// befristeten Anmeldung.
    pub befristet: Option<String>,
}

impl Transaktionsgrund {
    /// A Transaktionsgrund for a **verbrauchende Marktlokation** — the ordinary
    /// case, and the one the AHB marks Muss on the GPKE core processes.
    #[must_use]
    pub fn verbrauchende_malo(grund: impl Into<String>) -> Self {
        Self {
            grund: grund.into(),
            ergaenzung: Some(ergaenzung::VERBRAUCHENDE_MALO.to_owned()),
            befristet: None,
        }
    }

    /// A Transaktionsgrund with an explicit Ergänzung.
    #[must_use]
    pub fn new(grund: impl Into<String>, ergaenzung: impl Into<String>) -> Self {
        Self {
            grund: grund.into(),
            ergaenzung: Some(ergaenzung.into()),
            befristet: None,
        }
    }

    /// A Transaktionsgrund with **no Ergänzung** — the `WiM` MSB-Wechsel shape.
    ///
    /// The `WiM` Anwendungsübersichten list `SG4 STS 9015 = 7` with DE 9013 and
    /// nothing after it (UTILMD AHB Strom 2.2 Kap. 10, Gas 1.2 Kap. 6). The
    /// GPKE Ergänzung (`ZW4` verbrauchende Marktlokation and friends) names a
    /// property of a *Marktlokation*, and a `WiM` Vorgang is keyed on the
    /// Messlokation — emitting one asserts something the Anwendungsfall has no
    /// element for.
    #[must_use]
    pub fn bare(grund: impl Into<String>) -> Self {
        Self {
            grund: grund.into(),
            ergaenzung: None,
            befristet: None,
        }
    }

    /// Attach the DE 9013 element 4 (befristete Anmeldung) code.
    #[must_use]
    pub fn befristet(mut self, code: impl Into<String>) -> Self {
        self.befristet = Some(code.into());
        self
    }
}

/// `SG4 STS+E01` — the EBD Antwortcode on a Bestätigung or Ablehnung.
///
/// Serialises as `STS+E01++<code>:<ebd>'`. The AHB marks this segment **Muss**
/// on every Antwortnachricht and constrains the code to the Zustimmungs- or
/// Ablehnungs-Cluster of the named EBD, so it is not optional metadata: an
/// answer without it is not a well-formed answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntwortStatus {
    /// DE 9013 — „Code des Prüfschritts" (`A10`, `A35`, `E15`, `Z12`, …).
    pub code: String,
    /// DE 1131 — the **Codeliste** the code is drawn from.
    ///
    /// The AHB prints one of two things in this column, and they are not
    /// interchangeable:
    ///
    /// | AHB wording | Example | Where |
    /// |---|---|---|
    /// | „EBD Nr. `E_xxxx`" | `E_0622`, `E_3005` | GPKE and `GeLi` Gas answers |
    /// | „Codeliste Strom/Gas Nr. `S_xxxx`/`G_xxxx`" | `S_0090`, `G_0051` | every `WiM` MSB-Wechsel answer |
    ///
    /// **Both Sparten require it.** UTILMD AHB Gas 1.2 Kap. 6.1 marks
    /// `SG4 STS 1131` with an `X` on 44040/44041 and names `G_0052`/`G_0051`.
    ///
    /// `None` only for an answer whose AHB column really is empty.
    pub codeliste: Option<String>,
}

impl AntwortStatus {
    /// An Antwortcode together with the Codeliste DE 1131 must name.
    ///
    /// Ask [`mako_pruefung::codes::AntwortCode::wire_codeliste`] for the second
    /// argument — it is the EBD number only where the AHB says „EBD-Nummer".
    ///
    /// [`mako_pruefung::codes::AntwortCode::wire_codeliste`]: https://docs.rs/mako-pruefung
    #[must_use]
    pub fn from_codeliste(code: impl Into<String>, codeliste: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            codeliste: Some(codeliste.into()),
        }
    }

    /// A bare Antwortcode, for the few answers whose DE 1131 column is empty.
    #[must_use]
    pub fn bare(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            codeliste: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MIG's own worked example, kept as a test so a future edit to the
    /// element order fails loudly.
    #[test]
    fn the_mig_example_shape_is_grund_ergaenzung_befristet() {
        let t = Transaktionsgrund::verbrauchende_malo(transaktionsgrund::EIN_AUSZUG)
            .befristet(transaktionsgrund::WECHSEL);
        assert_eq!(t.grund, "E01");
        assert_eq!(t.ergaenzung.as_deref(), Some("ZW4"));
        assert_eq!(t.befristet.as_deref(), Some("E03"));
    }

    #[test]
    fn ide_carries_a_vorgang_not_a_location() {
        // DE 7495 has exactly two values in UTILMD. `Z19` is a location
        // qualifier and belongs in SG5 LOC.
        assert_eq!(IDE_VORGANG, "24");
        assert_eq!(loc::STEUERBARE_RESSOURCE, "Z19");
        assert_ne!(IDE_VORGANG, loc::MARKTLOKATION);
    }
}
