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

/// `BGM` DE 1001 `Z07` — **Aktivierung/Deaktivierung von `MaBiS`-ZP**.
///
/// All three Prüfidentifikatoren of the `MaBiS`-ZP lifecycle carry it (UTILMD
/// AHB Strom 2.2 Kap. 13.3). A Zählpunkt is activated, not angemeldet, so the
/// `E01` an ordinary Vorgang uses states the wrong Dokumentenart.
pub const BGM_MABIS_ZP_LIFECYCLE: &str = "Z07";

/// `BGM` DE 1001 `Z05` — **Clearingliste**.
///
/// Every `MaBiS` Clearingliste and every answer to one carries it (UTILMD AHB
/// Strom 2.2 Kap. 13.4), in place of the `E01`/`E02`/`E44` an ordinary Vorgang
/// uses.
pub const BGM_CLEARINGLISTE: &str = "Z05";

/// `SG8 SEQ+Z22` — Daten der Summenzeitreihe.
///
/// The Clearinglisten head block. Muss on both 55065 and 55066 (UTILMD AHB
/// Strom 2.2 Kap. 13.4), paired with [`RFF_ZEITREIHE`].
pub const SEQ_SUMMENZEITREIHE: &str = "Z22";

/// `SG8 RFF+AUU` — Referenz auf eine Zeitreihe; DE 1154 is its Version.
///
/// `MaBiS` keys a Summenzeitreihe's versions on the Erstellungszeitpunkt, so a
/// Clearingliste that names no version cannot be matched to the one it
/// reconciles.
pub const RFF_ZEITREIHE: &str = "AUU";

// ── SG5 LOC ───────────────────────────────────────────────────────────────────

/// `SG5 LOC` DE 3227 — the Lokationstyp qualifiers (MIG Zähler 0330).
pub mod loc {
    /// `LOC+172` — **Meldepunkt**, the one Lokationsqualifier UTILMD Gas uses.
    ///
    /// UTILMD AHB Gas G1.1/G1.2 names `172` in every `SG5 LOC` it defines and
    /// `Z16`/`Z17` in none: where Strom distinguishes the object by qualifier,
    /// Gas distinguishes it by the **format of DE 3225** — Bedingung `[950]`
    /// Marktlokations-ID, `[951]` Zählpunktbezeichnung. Sending `Z16` on a Gas
    /// UTILMD states a qualifier the receiving AHB does not define.
    pub const MELDEPUNKT: &str = "172";
    /// `LOC+Z15` — `MaBiS`-Zählpunkt.
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

/// `STS` DE 9015 — Statuskategorie `Z35`, **Status der Antwort des dritten
/// Marktbeteiligten** (MIG Nr. 00035).
///
/// A *second* `SG4 STS` beside `E01`, and the only place a Marktrolle restates
/// somebody else's Antwortcode. UTILMD AHB Strom 2.1/2.2 marks it **Muss** on a
/// 55003 „wenn `SG4 STS+E01++A50` vorhanden" (Bedingung `[356]`) and on a 55080
/// „wenn `STS+E01++A57` vorhanden" (`[84]`) — the two codes that mean „der LFA
/// hat der Anfrage zur Beendigung der Zuordnung widersprochen".
///
/// This is how GPKE Teil 2 § 2.1.2 Nr. 6's „der NB gibt zusätzlich den Grund der
/// Ablehnung des LFA an" reaches the wire. Without it the LFN learns that its
/// Anmeldung was refused and not why the incumbent refused to release the
/// Marktlokation, which is the only fact it can act on.
pub const STS_ANTWORT_DRITTER: &str = "Z35";

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
    /// `ZD9` — Beendigung wegen Rückzuordnungsmeldung.
    pub const BEENDIGUNG_RUECKZUORDNUNG: &str = "ZD9";
    /// `ZG5` — Aufhebung einer zukünftigen Zuordnung aufgrund § 38 EEG 2014
    /// bzw. § 21b Abs. 1 Nr. 2 EEG 2017.
    ///
    /// The one Aufhebungsgrund that names **no** beteiligter Marktpartner: the
    /// SG12 NAD is Muss on a 55038 only „wenn `ZG5` nicht vorhanden"
    /// (UTILMD AHB Strom 2.1/2.2 Bedingung `[206]`).
    pub const AUFHEBUNG_EEG38: &str = "ZG5";
    /// `ZG6` — Beendigung der Zuordnung aufgrund EEG 2014 § 38.
    pub const BEENDIGUNG_EEG38: &str = "ZG6";
    /// `ZG9` — Aufhebung einer zukünftigen Zuordnung wegen Auszug des Kunden.
    pub const AUFHEBUNG_AUSZUG: &str = "ZG9";
    /// `ZH0` — Aufhebung einer zukünftigen Zuordnung wegen Anmeldung eines
    /// anderen Lieferanten zu einem früheren Termin.
    pub const AUFHEBUNG_FRUEHERE_ANMELDUNG: &str = "ZH0";
    /// `ZH1` — Aufhebung einer zukünftigen Zuordnung wegen Stilllegung.
    pub const AUFHEBUNG_STILLLEGUNG: &str = "ZH1";
    /// `ZH2` — Aufhebung einer zukünftigen Zuordnung wegen aufgehobenem
    /// Vertragsverhältnis.
    ///
    /// „Vertrag zwischen Absender des Geschäftsvorfalls und Kunde wurde
    /// aufgehoben, wird z. B. verwendet wenn der Kunde den Vertrag widerruft."
    /// The one Aufhebungsgrund `E_0607` Prüfschritte 60 / 560 route on.
    pub const AUFHEBUNG_VERTRAGSVERHAELTNIS: &str = "ZH2";
    /// `Z15` — Zusätzlicher Datensatz.
    pub const ZUSAETZLICHER_DATENSATZ: &str = "Z15";
    /// `ZE3` — Stammdatenänderung.
    pub const STAMMDATENAENDERUNG: &str = "ZE3";
    /// `ZJ4` — Übernahme aufgrund nicht erfolgtem iMS-Einbau.
    pub const UEBERNAHME_KEIN_IMS: &str = "ZJ4";
    /// `ZP3` — Stammdaten.
    pub const STAMMDATEN: &str = "ZP3";
    /// `ZP4` — Werte.
    pub const WERTE: &str = "ZP4";
    /// `ZQ7` — Abmeldung wg. fehlender Zuordnungsermächtigung.
    pub const ABMELDUNG_FEHLENDE_ZUORDNUNGSERMAECHTIGUNG: &str = "ZQ7";
    /// `ZR9` — Kündigung aufgrund Vertrag mit Anschlussnehmer.
    pub const KUENDIGUNG_ANSCHLUSSNEHMER: &str = "ZR9";
    /// `ZT0` — Abmeldung wegen fehlender Zuordnungsermächtigung aufgrund
    /// Änderung ZRT.
    pub const ABMELDUNG_FEHLENDE_ZE_ZRT: &str = "ZT0";
    /// `ZT4` — Ende wegen Kündigung durch LF (den bislang beliefernden LFA).
    pub const ENDE_KUENDIGUNG_LF: &str = "ZT4";
    /// `ZT5` — Ende wegen Kündigung durch Kunde/LFN.
    ///
    /// Also covers „keine Kündigung des Vertrages notwendig da Vertrag nur auf
    /// bestimmte Zeit gelaufen ist" and a Kündigung durch Dritte.
    pub const ENDE_KUENDIGUNG_KUNDE: &str = "ZT5";
    /// `ZT6` — `EoG` wegen Kündigung durch LF.
    pub const EOG_KUENDIGUNG_LF: &str = "ZT6";
    /// `ZT7` — `EoG` wegen Kündigung durch Kunde/LFN.
    pub const EOG_KUENDIGUNG_KUNDE: &str = "ZT7";
    /// `ZU1` — Änderung von MSB Abrechnungsdaten.
    pub const AENDERUNG_MSB_ABRECHNUNGSDATEN: &str = "ZU1";
    /// `ZX2` — Abrechnungsdaten BK-Abrechnung erzeugender `MaLo`.
    pub const ABRECHNUNGSDATEN_BK_ERZEUGEND: &str = "ZX2";
    /// `ZX3` — Abrechnungsdaten BK-Abrechnung verbrauchender `MaLo`.
    pub const ABRECHNUNGSDATEN_BK_VERBRAUCHEND: &str = "ZX3";
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

/// `SG4 STS+7` DE 9013 (element 3) on the **Ankündigung Zuordnung LF**.
///
/// 55607 / 55608 / 55609 use their own code space in the same element:
/// `ZW8`–`ZX1` name Fall 1 to Fall 4 of GPKE Teil 2 § 2.4, and UTILMD AHB Strom
/// 2.2 Bedingungen `[161]`–`[164]` map them one-to-one onto the answering EBD
/// `E_0603`–`E_0606` in `SG4 STS+E01` DE 1131. They are **not** [`ergaenzung`]
/// values, and reading a 55607 through that module yields nothing.
pub mod zuordnungsfall {
    /// `ZW8` — Fall 1: EEG-MaLo bzw. KWKG-MaLo ohne DV-Pflicht (`E_0603`).
    pub const FALL_1: &str = "ZW8";
    /// `ZW9` — Fall 2: EEG-MaLo mit DV-Pflicht (`E_0604`).
    pub const FALL_2: &str = "ZW9";
    /// `ZX0` — Fall 3: KWKG-MaLo mit DV-Pflicht bzw. Nicht-EEG-/Nicht-KWKG-MaLo,
    /// nicht-tranchiert (`E_0605`).
    pub const FALL_3: &str = "ZX0";
    /// `ZX1` — Fall 4: dieselben Fälle, tranchiert abgebildet (`E_0606`).
    pub const FALL_4: &str = "ZX1";
}

/// `NAD` DE 3035 — Beteiligter, Qualifier.
///
/// `MS`/`MR` open `SG2` at message level; the rest are `SG12` parties inside a
/// Vorgang.
pub mod nad {
    /// `MS` — Dokumenten-/Nachrichtenaussteller (message sender).
    pub const ABSENDER: &str = "MS";
    /// `MR` — Nachrichtenempfänger.
    pub const EMPFAENGER: &str = "MR";
    /// `Z09` — **Kunde des Lieferanten**, `SG12`.
    ///
    /// Muss on a 55010 whose Transaktionsgrundergänzung is `ZW4`/`ZAP`
    /// (UTILMD AHB Strom 2.2 Bedingung `[279]`); Bedingung `[572]` says it is
    /// the „Kundenname aus Anmeldung Lieferant neu".
    pub const KUNDE_DES_LF: &str = "Z09";
    /// `VY` — andere zugehörige Partei, `SG12`. On a 55010 the
    /// **Neulieferant** (Bedingung `[567]`).
    pub const ZUGEHOERIGE_PARTEI: &str = "VY";
}

/// `NAD` `C080` DE 3045 — Format für den Namen des Beteiligten.
pub mod namensformat {
    /// `Z01` — Struktur von Personennamen: the `C080` components are
    /// Nachname, Vorname, …
    pub const PERSON: &str = "Z01";
    /// `Z02` — Struktur der Firmenbezeichnung.
    pub const FIRMA: &str = "Z02";
}

/// `SG8` / `SG10` — the **Produktpaket** an Anmeldung and its Bestätigung carry.
///
/// A UTILMD Anmeldung einer Zuordnung does not merely name a Marktlokation and
/// a date: the AHB makes `SG8 SEQ+Z79` („Bestandteil eines Produktpakets")
/// Muss on 55001, 55077, 55600, 55601, 55014 and 55608, and the Codeliste der
/// Konfigurationen 1.4 Kap. 6.1.1 lists the products that must appear in it.
/// One of them is unconditional:
///
/// > `9991000002082` **Bilanzkreis** — „Dieses Produkt ist je Produktpaket-ID
/// > in der UTILMD zwingend anzugeben."
///
/// So the Bilanzkreis is not a remark beside the answer; it is the answer's
/// mandatory payload, and `SG4 FTX+ACB` is not where it goes — the AHB admits
/// that segment on the Ablehnung only.
pub mod produkt {
    /// `SG8 SEQ` DE 1229 — Bestandteil eines Produktpakets.
    pub const SEQ_PRODUKTPAKET: &str = "Z79";
    /// `SG8 PIA` DE 4347 — Produktidentifikation.
    pub const PIA_ERFORDERLICHES_PRODUKT: &str = "5";
    /// `SG8 PIA` DE 7143 — Produkt.
    pub const PIA_TYP_PRODUKT: &str = "Z11";
    /// `SG10 CCI` DE 7059 — Produkteigenschaft.
    pub const CCI_PRODUKTEIGENSCHAFT: &str = "Z66";
    /// `SG10 CAV` DE 7111 — Code der Produkteigenschaft.
    pub const CAV_EIGENSCHAFT: &str = "ZH9";
    /// `SG10 CAV` DE 7111 — Wertedetails zum Produkt.
    pub const CAV_WERT: &str = "ZV4";
    /// `SG8 SEQ` DE 1229 — Priorisierung erforderliches Produktpaket.
    pub const SEQ_PRIORISIERUNG: &str = "ZH0";
    /// `SG10 CCI` DE 7059 — Umsetzungsgradvorgabe des Produktpakets.
    pub const CCI_UMSETZUNGSGRAD: &str = "Z65";
    /// `SG10 CCI` DE 4051 — Produktpaket ist vollumfänglich umzusetzen.
    pub const UMSETZUNG_VOLLUMFAENGLICH: &str = "Z01";
    /// `SG10 CCI` DE 4051 — Produktpaket kann in Teilen umgesetzt werden.
    pub const UMSETZUNG_IN_TEILEN: &str = "Z02";
    /// Produkt-Code `9991000002082` — **Bilanzkreis**, format `an..17`
    /// (Bedingung `[970]`).
    pub const BILANZKREIS: &str = "9991000002082";
    /// Produkt-Code `9991000002090` — Tranchengröße.
    pub const TRANCHENGROESSE: &str = "9991000002090";
    /// `SG10 CCI` DE 7059 — **Bilanzkreis**, the `GeLi` Gas shape.
    ///
    /// `GeLi` Gas has no Produktpaket: UTILMD AHB Gas 1.2 marks `SG10 CCI+Z19`
    /// with the Bilanzkreis in DE 7037 Muss on 44001 and on the Bestandsliste
    /// family. The Strom Produktpaket and this segment carry the same fact and
    /// are not interchangeable.
    pub const CCI_BILANZKREIS_GAS: &str = "Z19";
    /// `SG10 CAV` DE 7111 — Priorisierung erforderliches Produktpaket, 1. to
    /// 5. Priorität. Bedingung `[42]` requires it only where a Geschäftsvorfall
    /// carries more than one Produktpaket; the AHB caps it at five.
    pub const PRIORITAET: [&str; 5] = ["Z75", "Z76", "Z77", "Z78", "Z79"];
}

// ── Typed SG4 payloads ────────────────────────────────────────────────────────

/// One entry of a `SG8 SEQ+Z79` Produktpaket: a Produkt-Code with the
/// Produkteigenschaft and the Merkmalswert the Codeliste attaches to it.
///
/// Serialises as
///
/// ```text
/// SEQ+Z79+1
/// PIA+5+9991000002082:Z11
/// CCI+Z66
/// CAV+ZV4:::11XBK-EEG-----1
/// ```
///
/// `CAV+ZH9` is emitted only where the Codeliste gives the product a Code der
/// Produkteigenschaft; the Bilanzkreis has none („--"), and Bedingung `[36]`
/// makes the segment conditional on exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Produkt {
    /// `SG8 PIA+5` DE 7140 — the Produkt-Code from the Codeliste der
    /// Konfigurationen Kap. 6.1.
    pub produkt_code: String,
    /// `SG10 CAV+ZH9` DE 7110 — Code der Produkteigenschaft, where the product
    /// defines a Wertebereich.
    pub eigenschaft: Option<String>,
    /// `SG10 CAV+ZV4` DE 7110 — Merkmalswert, „Wertedetails für Position".
    pub wert: Option<String>,
}

impl Produkt {
    /// The mandatory **Bilanzkreis** product (`9991000002082`), whose
    /// Merkmalswert is the Bilanzkreis itself.
    #[must_use]
    pub fn bilanzkreis(bk: impl Into<String>) -> Self {
        Self {
            produkt_code: produkt::BILANZKREIS.to_owned(),
            eigenschaft: None,
            wert: Some(bk.into()),
        }
    }
}

/// `SG10 CCI+Z65` DE 4051 — how much of a Produktpaket the NB must honour.
///
/// UTILMD AHB Strom 2.2 Kap. 5.3: `Z01` means the NB may only assign the LF
/// when **every** product of the package can be applied from the
/// Zuordnungsbeginn; `Z02` means a partial application is enough — „unabhängig
/// vom Bilanzkreis, der immer erfüllt sein muss".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Umsetzungsgrad {
    /// `Z01` — Produktpaket ist vollumfänglich umzusetzen.
    #[default]
    Vollumfaenglich,
    /// `Z02` — Produktpaket kann in Teilen umgesetzt werden.
    InTeilen,
}

impl Umsetzungsgrad {
    /// The `CCI+Z65` DE 4051 code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Vollumfaenglich => produkt::UMSETZUNG_VOLLUMFAENGLICH,
            Self::InTeilen => produkt::UMSETZUNG_IN_TEILEN,
        }
    }
}

/// A `SG8 SEQ+Z79` Produktpaket — a Produktpaket-ID, its products, and the
/// `SG8 SEQ+ZH0` Umsetzungsgradvorgabe that goes with it.
///
/// DE 1050 is „Produktpaket-ID", Bedingungen `[914]` ∧ `[937]`: a positive integer
/// without decimals. A Geschäftsvorfall carries at most five (AHB Kap. 5.3),
/// and every one of them needs its own `SEQ+ZH0` — the AHB marks that group
/// Muss, so a Produktpaket emitted without it is an incomplete message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Produktpaket {
    /// `SG8 SEQ` DE 1050 — the Produktpaket-ID.
    pub paket_id: u32,
    /// The products in the package, in Codeliste order.
    pub produkte: Vec<Produkt>,
    /// `SG10 CCI+Z65` DE 4051 — the Umsetzungsgradvorgabe.
    pub umsetzung: Umsetzungsgrad,
}

impl Produktpaket {
    /// The single-product package a Zuordnung needs: Produktpaket 1 carrying
    /// the Bilanzkreis, to be applied in full.
    #[must_use]
    pub fn bilanzkreis(bk: impl Into<String>) -> Self {
        Self {
            paket_id: 1,
            produkte: vec![Produkt::bilanzkreis(bk)],
            umsetzung: Umsetzungsgrad::Vollumfaenglich,
        }
    }
}

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

/// `SG4 STS+Z35` — the **third market participant's** answer, restated by the
/// party that is refusing on the strength of it.
///
/// Only the Ablehnung einer Anmeldung uses it, and only when the ground is the
/// LFA's Widerspruch (`A50` verbrauchend, `A57` erzeugend). The erzeugende form
/// carries two more things than the verbrauchende one, because Geschäftsvorfall
/// 3 splits a Marktlokation across Tranchen and several LFA answer: which object
/// the restated answer is about (`ZW3` Erzeugende Marktlokation / `ZW5` Tranche)
/// and its MaLo-ID (DE 9012, UTILMD AHB Strom 2.2 Bedingung `[950]`).
///
/// Wire form: `STS+Z35++A35:E_0624'` on a 55003,
/// `STS+Z35+51238696781+A39:E_0624+ZW5'` on a 55080.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DritterAntwortStatus {
    /// DE 9013 — the third party's own Prüfschritt code.
    ///
    /// „Bis auf den Code `A30` sind alle Codes aus EBD `E_0624` im Cluster
    /// Ablehnung erlaubt" (Bedingung `[366]`; `[368]` says `A41` on the
    /// erzeugende branch) — the „bereits abgemeldet" answer confirms the
    /// Anmeldung instead, so it never reaches this segment.
    pub code: String,
    /// DE 1131 — always `E_0624`, the tree the LFA answered from.
    pub codeliste: String,
    /// DE 9012 in `C555` — „Referenz auf ID der Marktlokation / Tranche".
    ///
    /// `None` on a 55003, whose AHB column is empty: a verbrauchende
    /// Marktlokation has exactly one LFA and the Vorgang already names it.
    pub referenz_lokation: Option<String>,
    /// The second DE 9013 — `ZW3` Erzeugende Marktlokation or `ZW5` Tranche.
    ///
    /// `None` on a 55003, for the same reason.
    pub objekt: Option<String>,
}

impl DritterAntwortStatus {
    /// The verbrauchende form — code and Codeliste only (PID 55003).
    #[must_use]
    pub fn verbrauchend(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            codeliste: EBD_BEENDIGUNG_ZUORDNUNG.to_owned(),
            referenz_lokation: None,
            objekt: None,
        }
    }

    /// The erzeugende form — additionally naming the object the restated answer
    /// is about (PID 55080).
    #[must_use]
    pub fn erzeugend(
        code: impl Into<String>,
        referenz_lokation: impl Into<String>,
        objekt: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            codeliste: EBD_BEENDIGUNG_ZUORDNUNG.to_owned(),
            referenz_lokation: Some(referenz_lokation.into()),
            objekt: Some(objekt.into()),
        }
    }
}

/// The EBD a `SG4 STS+Z35` always names in DE 1131 — the tree the LFA answered
/// the Anfrage zur Beendigung der Zuordnung from.
pub const EBD_BEENDIGUNG_ZUORDNUNG: &str = "E_0624";

/// The `E_0623` Ablehnungscodes that make a `SG4 STS+Z35` **Muss**.
///
/// `A50` on a verbrauchende oder ruhende Marktlokation (Bedingung `[356]`),
/// `A57` on an erzeugende one (`[84]`). Both mean „der LFA hat der Anfrage zur
/// Beendigung der Zuordnung widersprochen", and neither is answerable without
/// naming the LFA's own Grund.
pub const CODES_REQUIRING_DRITTER: &[&str] = &["A50", "A57"];

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
