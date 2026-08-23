//! The Antwortcode catalogue — every code an LF may put in `SG4 STS+E01`.
//!
//! Each EBD is its own Codeliste (UTILMD MIG Strom S2.2, `STS` DE 1131:
//! „Diesem Datenelement werden Codes aus den Codelisten des Dokumentes
//! Entscheidungsbaum-Diagramme verwendet. Jeder Entscheidungsbaum gilt als
//! Codeliste."). The AHB then restricts a Bestätigung to that EBD's
//! **Zustimmungs**-Cluster and an Ablehnung to its **Ablehnungs**-Cluster.
//!
//! Two consequences the type system enforces here:
//!
//! 1. A code belongs to exactly one EBD. `A32` means „kein Einzug, Kunde
//!    identisch" in `E_0624` and appears in no other tree — putting it on a
//!    `55009` (whose tree is `E_0609`) is not a wrong reason, it is an
//!    undefined code.
//! 2. The **cluster decides the PID**. `A31` is a Zustimmung, so it rides
//!    `55011`; `A30` is an Ablehnung, so it rides `55012`. Deriving the PID
//!    from an `accepted: bool` the caller passes alongside the code lets the
//!    two disagree.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* **4.3**
//! (01.04.2026) — Strom Kap. 6.2.1 (`E_0614`), 6.4.1 (`E_0609`), 6.6.3
//! (`E_0624`); Gas Kap. 13.3.1 (`E_3001`), 13.5.1 (`E_3002`), 13.6.3
//! (`E_3020`).

use serde::{Deserialize, Serialize};

/// Which side of an EBD a code sits on.
///
/// The BDEW prints „Cluster: …" beside every code, and **the name is the tree's
/// own**. Most answer trees pair Zustimmung with Ablehnung, and there the
/// cluster selects the answer PID. `E_0595` („Bestellung prüfen") pairs
/// „Änderung der Daten" with „keine Änderung der Daten", which is a *different
/// axis*: it says whether a Stammdatenänderung follows the Bearbeitungsstand,
/// not whether the Bestellung was granted. `A06` sits in „Änderung der Daten"
/// while stating that no change is made — because the Verantwortliche still
/// sends its own data back.
///
/// Collapsing the two pairs would read `A06` as an agreement, so they are
/// distinct variants and [`AntwortCode::ist_zustimmung`] answers `None` off the
/// agreement axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Cluster {
    /// The answer agrees — carried by the Bestätigungs-PID.
    Zustimmung,
    /// The answer refuses — carried by the Ablehnungs-PID.
    Ablehnung,
    /// `E_0595` — a Stammdatenänderung follows this Bearbeitungsstand.
    AenderungDerDaten,
    /// `E_0595` — none follows.
    KeineAenderungDerDaten,
}

impl Cluster {
    /// The BDEW's own wording, as it appears beside the code.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Zustimmung => "Zustimmung",
            Self::Ablehnung => "Ablehnung",
            Self::AenderungDerDaten => "Änderung der Daten",
            Self::KeineAenderungDerDaten => "keine Änderung der Daten",
        }
    }

    /// `true` when a Stammdatenänderung follows the answer carrying this code.
    ///
    /// `None` for the Zustimmung/Ablehnung pair, which says nothing about
    /// follow-up data.
    #[must_use]
    pub const fn sendet_stammdatenaenderung(self) -> Option<bool> {
        match self {
            Self::AenderungDerDaten => Some(true),
            Self::KeineAenderungDerDaten => Some(false),
            Self::Zustimmung | Self::Ablehnung => None,
        }
    }
}

/// One published Antwortcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntwortCode {
    /// DE 9013 — the code itself (`A10`, `A35`, `E15`, `Z12`, …).
    pub code: &'static str,
    /// The **Entscheidungsbaum** that publishes it, or `None` where the answer
    /// names no tree at all.
    ///
    /// This is the identity a code is resolved against — it is *not* necessarily
    /// what goes on the wire. Several BDEW answer trees publish their codes
    /// through separately numbered **Codelisten** (`S_00xx` Strom, `G_00xx`
    /// Gas), and it is the Codeliste that DE 1131 / DE 1082 must name. Ask
    /// [`AntwortCode::wire_codeliste`] for the wire value.
    pub ebd: Option<&'static str>,
    /// Zustimmung or Ablehnung.
    pub cluster: Cluster,
    /// The BDEW's own wording, for the operator queue and the audit log.
    pub bedeutung: &'static str,
    /// `true` when the BDEW requires a written Erläuterung alongside the code
    /// (`FTX+ACB`). Sending one of these bare is an incomplete answer.
    pub braucht_bemerkung: bool,
}

impl AntwortCode {
    /// `true` when this code agrees with the request, `false` when it refuses.
    ///
    /// `None` on a tree whose cluster is not the agreement axis — `E_0595` says
    /// whether a Stammdatenänderung follows, and neither of its clusters is a
    /// Zustimmung. A caller that derives an answer PID from agreement must
    /// handle that rather than read `false` as a refusal.
    #[must_use]
    pub const fn ist_zustimmung(&self) -> Option<bool> {
        match self.cluster {
            Cluster::Zustimmung => Some(true),
            Cluster::Ablehnung => Some(false),
            Cluster::AenderungDerDaten | Cluster::KeineAenderungDerDaten => None,
        }
    }

    /// `true` when a Stammdatenänderung follows the answer carrying this code
    /// (`E_0595`); `None` on the agreement axis.
    #[must_use]
    pub const fn sendet_stammdatenaenderung(&self) -> Option<bool> {
        self.cluster.sendet_stammdatenaenderung()
    }

    /// The value the answer must carry in **UTILMD `SG4 STS+E01` DE 1131** or
    /// **ORDRSP `SG2 AJT` DE 1082** — the identifier of the *Codeliste*, which
    /// is not always the EBD number.
    ///
    /// The BDEW prints two different things in that data element:
    ///
    /// | AHB wording | Example | Trees |
    /// |---|---|---|
    /// | „EBD Nr. `E_xxxx`" | `E_0622`, `E_0249` | every GPKE/GeLi Gas tree, and the Messlokationsänderung |
    /// | „Codeliste Strom/Gas Nr. `S_xxxx`/`G_xxxx`" | `S_0090`, `G_0051` | every WiM MSB-Wechsel, Weiterverpflichtung, Gerätewechselabsicht and Geräteübernahme answer |
    ///
    /// Where a tree publishes through Codelisten, the **cluster** picks which
    /// one — a Bestätigung and an Ablehnung name different lists. Sending the
    /// EBD number instead is a rejected message, not a cosmetic difference.
    ///
    /// Source: UTILMD AHB Strom 2.2 Kap. 10, UTILMD AHB Gas 1.2 Kap. 6,
    /// ORDRSP AHB 1.1b Kap. 4.
    #[must_use]
    pub fn wire_codeliste(&self) -> Option<&'static str> {
        let ebd = self.ebd?;
        Some(wire_codeliste(ebd, self.cluster).unwrap_or(ebd))
    }
}

/// The Codeliste identifier DE 1131 / DE 1082 must carry for an answer drawn
/// from `ebd` on `cluster`, or `None` when the EBD number itself is the wire
/// value.
///
/// See [`AntwortCode::wire_codeliste`] for why the two differ.
#[must_use]
pub fn wire_codeliste(ebd: &str, cluster: Cluster) -> Option<&'static str> {
    let (_, zustimmung, ablehnung) = WIRE_CODELISTEN.iter().find(|(id, _, _)| *id == ebd)?;
    match cluster {
        Cluster::Zustimmung => Some(zustimmung),
        Cluster::Ablehnung => Some(ablehnung),
        // The Datenänderung axis belongs to `E_0595`, which names its EBD.
        Cluster::AenderungDerDaten | Cluster::KeineAenderungDerDaten => None,
    }
}

/// Trees whose codes ride the wire under a **Codeliste** number rather than the
/// EBD number, as `(ebd, Zustimmungs-Codeliste, Ablehnungs-Codeliste)`.
///
/// Every entry is read off the AHB column „SG4 STS 1131" resp. „SG2 AJT 1082"
/// for the two answer Prüfidentifikatoren of that process. An EBD absent from
/// this table names itself, which is what every GPKE and GeLi Gas tree does.
///
/// Sources: UTILMD AHB Strom 2.2 (01.04.2026) Kap. 10.1–10.4; UTILMD AHB Gas
/// 1.2 (01.04.2026) Kap. 6.1–6.5; ORDRSP AHB 1.1b Kap. 4; and BDEW
/// *Entscheidungsbaum-Diagramme und Codelisten* 4.3 Kap. 8 (Strom) and 14 (Gas),
/// which is where the `S_`/`G_` lists themselves are published.
pub const WIRE_CODELISTEN: &[(&str, &str, &str)] = &[
    // ── WiM Strom (EBD 4.3 Kap. 8) ────────────────────────────────────────────
    (EBD_KUENDIGUNG_MSB, "S_0090", "S_0054"),
    (EBD_ANMELDUNG_MSB, "S_0055", "S_0056"),
    (EBD_ABMELDUNG_MSB, "S_0059", "S_0060"),
    (EBD_VERPFLICHTUNGSANFRAGE, "S_0063", "S_0064"),
    (EBD_WEITERVERPFLICHTUNG, "S_0061", "S_0062"),
    (EBD_GERAETEWECHSELABSICHT, "S_0065", "S_0066"),
    (EBD_BESTELLUNG_GERAETEUEBERNAHME, "S_0067", "S_0068"),
    (EBD_GESAMTVORGANG, "S_0057", "S_0057"),
    // ── WiM Gas (EBD 4.3 Kap. 14) ─────────────────────────────────────────────
    (EBD_KUENDIGUNG_MSB_GAS, "G_0052", "G_0051"),
    (EBD_ANMELDUNG_MSB_GAS, "G_0054", "G_0053"),
    (EBD_ABMELDUNG_MSB_GAS, "G_0058", "G_0057"),
    (EBD_VERPFLICHTUNGSANFRAGE_GAS, "G_0070", "G_0071"),
    (EBD_WEITERVERPFLICHTUNG_GAS, "G_0072", "G_0073"),
    (EBD_GERAETEWECHSELABSICHT_GAS, "G_0059", "G_0060"),
    (EBD_BESTELLUNG_GERAETEUEBERNAHME_GAS, "G_0061", "G_0074"),
    (EBD_GESAMTVORGANG_GAS, "G_0055", "G_0055"),
    (EBD_WIM_RECHNUNG_NB_GAS, "G_0083", "G_0083"),
    (EBD_WIM_RECHNUNG_MSBN_GAS, "G_0084", "G_0084"),
    (EBD_WIM_RECHNUNG_MELO_GAS, "G_0083", "G_0083"),
    (EBD_WIM_STORNO_GAS, "G_0085", "G_0085"),
    (EBD_WIM_STORNO_MSBN_GAS, "G_0086", "G_0086"),
];

macro_rules! code {
    ($code:literal, $ebd:expr, $cluster:ident, $bedeutung:literal) => {
        AntwortCode {
            code: $code,
            ebd: $ebd,
            cluster: Cluster::$cluster,
            bedeutung: $bedeutung,
            braucht_bemerkung: false,
        }
    };
    ($code:literal, $ebd:expr, $cluster:ident, $bedeutung:literal, bemerkung) => {
        AntwortCode {
            code: $code,
            ebd: $ebd,
            cluster: Cluster::$cluster,
            bedeutung: $bedeutung,
            braucht_bemerkung: true,
        }
    };
}

// ── E_0609 — Abmeldung prüfen (Lieferende von NB an LF, 55007 → 55008/55009) ──

/// EBD id of the „Abmeldung prüfen" tree the LF runs on an inbound `55007`.
pub const EBD_ABMELDUNG: &str = "E_0609";
const E_0609: Option<&'static str> = Some(EBD_ABMELDUNG);

/// `E_0609` — verbrauchende / ruhende Marktlokation branch (Prüfschritte 10–130).
pub const E_0609_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        E_0609,
        Ablehnung,
        "Bei der in der Abmeldung genannten Marktlokation handelt es sich nicht um eine „ruhende Marktlokation\" einer Kundenanlage"
    ),
    code!(
        "A02",
        E_0609,
        Ablehnung,
        "Lieferende zum Abmeldedatum wurde bereits bestätigt"
    ),
    code!(
        "A03",
        E_0609,
        Ablehnung,
        "Vorlauffrist wurde nicht eingehalten"
    ),
    code!(
        "A04",
        E_0609,
        Ablehnung,
        "Dem LF liegen Informationen vor, dass die Marktlokation nicht stillgelegt wird/wurde",
        bemerkung
    ),
    code!(
        "A05",
        E_0609,
        Ablehnung,
        "Das Lieferende muss auf dem 1. eines Kalendermonats 00:00 Uhr liegen"
    ),
    code!(
        "A06",
        E_0609,
        Ablehnung,
        "Es liegt eine Änderung auf einen Zeitreihentyp vor, für welchen eine Zuordnungsermächtigung aus Sicht des LF besteht"
    ),
    code!(
        "A07",
        E_0609,
        Ablehnung,
        "Aus Sicht des LF wurde die Zuordnungsermächtigung für den an der Marktlokation genannten ZRT nicht deaktiviert"
    ),
    code!("A09", E_0609, Ablehnung, "Fristüberschreitung"),
    code!("A10", E_0609, Zustimmung, "Lieferende wird zugestimmt"),
    // Tranche / erzeugende Marktlokation branch (Prüfschritte 510–610).
    code!(
        "A21",
        E_0609,
        Ablehnung,
        "Lieferende zum Abmeldedatum wurde bereits bestätigt (Tranche)"
    ),
    code!(
        "A22",
        E_0609,
        Ablehnung,
        "Vorlauffrist wurde nicht eingehalten (Tranche)"
    ),
    code!(
        "A23",
        E_0609,
        Ablehnung,
        "Dem LF liegen Informationen vor, dass die Marktlokation bzw. Tranche nicht stillgelegt wird/wurde",
        bemerkung
    ),
    code!(
        "A24",
        E_0609,
        Ablehnung,
        "Das Lieferende muss auf dem 1. eines Kalendermonats 00:00 Uhr liegen (Tranche)"
    ),
    code!(
        "A25",
        E_0609,
        Ablehnung,
        "Es liegt eine Änderung auf einen Zeitreihentyp vor, für welchen eine Zuordnungsermächtigung aus Sicht des LF besteht (Tranche)"
    ),
    code!(
        "A26",
        E_0609,
        Ablehnung,
        "Aus Sicht des LF wurde die Zuordnungsermächtigung für den an der Tranche genannten ZRT nicht deaktiviert"
    ),
    code!("A28", E_0609, Ablehnung, "Fristüberschreitung (Tranche)"),
    code!(
        "A29",
        E_0609,
        Zustimmung,
        "Lieferende wird zugestimmt (Tranche)"
    ),
    code!("A99", E_0609, Ablehnung, "Sonstiges", bemerkung),
];

// ── E_0624 — Anfrage zur Beendigung der Zuordnung (55010 → 55011/55012) ───────

/// EBD id of the „Anfrage zur Beendigung der Zuordnung prüfen" tree.
pub const EBD_BEENDIGUNG_ZUORDNUNG: &str = "E_0624";
const E_0624: Option<&'static str> = Some(EBD_BEENDIGUNG_ZUORDNUNG);

/// `E_0624` — the LFA's answer to the NB's Abmeldeanfrage inside a Lieferbeginn.
pub const E_0624_CODES: &[AntwortCode] = &[
    code!(
        "A30",
        E_0624,
        Ablehnung,
        "Die Belieferung wurde zu dem angefragten Termin bereits beendet und eine vom NB bestätigte Abmeldung liegt vor"
    ),
    code!(
        "A31",
        E_0624,
        Zustimmung,
        "Zustimmung zum Termin der bereits versendeten, noch nicht beantworteten Abmeldung"
    ),
    code!(
        "A32",
        E_0624,
        Ablehnung,
        "Es handelt sich nicht um einen Einzug, da der Kunde aus der Anfrage identisch mit dem Kunden beim LFA ist"
    ),
    code!(
        "A33",
        E_0624,
        Ablehnung,
        "Der LFA hat die Information, dass der Kunde nicht ausgezogen ist"
    ),
    code!(
        "A34",
        E_0624,
        Zustimmung,
        "Der LFA beendet die Belieferung und teilt sein Lieferendedatum in der Antwort mit"
    ),
    code!("A35", E_0624, Ablehnung, "Es besteht eine Vertragsbindung"),
    code!(
        "A36",
        E_0624,
        Zustimmung,
        "Vertragsverhältnis wurde zum angefragten oder davor liegenden Termin beendet"
    ),
    code!(
        "A38",
        E_0624,
        Zustimmung,
        "Ersatzversorgung wurde zum angefragten Termin beendet"
    ),
    // Tranche branch (Prüfschritte 200–220).
    code!(
        "A39",
        E_0624,
        Ablehnung,
        "Es besteht eine Vertragsbindung (Tranche)"
    ),
    code!(
        "A40",
        E_0624,
        Zustimmung,
        "Vertragsverhältnis wurde zum angefragten oder davor liegenden Termin beendet (Tranche)"
    ),
    code!(
        "A41",
        E_0624,
        Ablehnung,
        "Die Belieferung wurde bereits beendet und eine vom NB bestätigte Abmeldung liegt vor (Tranche)"
    ),
    code!(
        "A42",
        E_0624,
        Zustimmung,
        "Zustimmung zum Termin der bereits versendeten Abmeldung (Tranche)"
    ),
    code!(
        "A43",
        E_0624,
        Ablehnung,
        "Fristüberschreitung — die Anfrage ging nicht bis 07:00 Uhr des nächsten Werktages nach dem ÜT der Lieferanmeldung ein"
    ),
];

// ── E_0614 — Kündigung Vertrag prüfen (55016 → 55017/55018) ───────────────────

/// EBD id of the „Kündigung Vertrag prüfen" tree the LFA runs on a `55016`.
pub const EBD_KUENDIGUNG: &str = "E_0614";
const E_0614: Option<&'static str> = Some(EBD_KUENDIGUNG);

/// `E_0614` — the LFA's answer to the LFN's Kündigung.
pub const E_0614_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        E_0614,
        Ablehnung,
        "Fristüberschreitung — der Kündigungstermin liegt vor dem Nachrichteneingang"
    ),
    code!(
        "A03",
        E_0614,
        Zustimmung,
        "Vertrag wurde bereits zum angefragten Kündigungstermin gekündigt"
    ),
    code!(
        "A04",
        E_0614,
        Ablehnung,
        "Zum Kündigungstermin besteht kein Vertragsverhältnis mehr"
    ),
    code!(
        "A05",
        E_0614,
        Ablehnung,
        "Vertragsbindung bei bereits in der Zukunft beendetem Vertrag"
    ),
    code!("A06", E_0614, Ablehnung, "Vertragsbindung"),
    code!(
        "A08",
        E_0614,
        Ablehnung,
        "Die vom LFN eingereichte Vollmacht wird als nicht wirksam vom LFA betrachtet",
        bemerkung
    ),
    code!("A09", E_0614, Zustimmung, "Zustimmung"),
    // Non-verbrauchende branch (Prüfschritte 500–630).
    code!("A10", E_0614, Ablehnung, "Fristüberschreitung"),
    code!(
        "A12",
        E_0614,
        Zustimmung,
        "Vertrag wurde bereits zum angefragten Kündigungstermin gekündigt"
    ),
    code!(
        "A13",
        E_0614,
        Ablehnung,
        "Zum Kündigungstermin besteht kein Vertragsverhältnis mehr"
    ),
    code!(
        "A14",
        E_0614,
        Ablehnung,
        "Vertragsbindung bei bereits in der Zukunft beendetem Vertrag"
    ),
    code!("A15", E_0614, Ablehnung, "Vertragsbindung"),
    code!(
        "A16",
        E_0614,
        Ablehnung,
        "Die vom LFN eingereichte Vollmacht wird als nicht wirksam vom LFA betrachtet",
        bemerkung
    ),
    code!("A17", E_0614, Zustimmung, "Zustimmung"),
    code!(
        "A18",
        E_0614,
        Ablehnung,
        "Zu dem genannten Objekt liegt kein Vertrag vor"
    ),
    code!("A99", E_0614, Ablehnung, "Sonstiges", bemerkung),
];

// ── Gas ───────────────────────────────────────────────────────────────────────
//
// The Gas trees are published as plain Codelisten (`G_xxxx`) rather than as
// numbered Prüfschritte, and the MIG does not name them in DE 1131 — so their
// codes carry `ebd: None` and render as a bare `STS+E01++<code>'`.

/// `E_3001` / `G_0005` + `G_0006` — Kündigung Gasliefervertrag (44016 → 44017/44018).
pub const EBD_KUENDIGUNG_GAS: &str = "E_3001";
pub const E_3001_CODES: &[AntwortCode] = &[
    code!("E15", None, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        None,
        Zustimmung,
        "Zustimmung mit Terminänderung — der nächstmögliche Kündigungszeitpunkt steht im DTM+471"
    ),
    code!(
        "Z44",
        None,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!("E14", None, Ablehnung, "Ablehnung Sonstiges", bemerkung),
    code!("Z12", None, Ablehnung, "Ablehnung Vertragsbindung"),
    code!(
        "Z29",
        None,
        Ablehnung,
        "Ablehnung — kein Vertragsverhältnis mehr vorhanden"
    ),
    code!("Z34", None, Ablehnung, "Ablehnung — Mehrfachkündigung"),
    code!(
        "A03",
        None,
        Ablehnung,
        "Ablehnung — keine Identifizierung einer Marktlokation"
    ),
    code!(
        "A04",
        None,
        Ablehnung,
        "Ablehnung — mehrere Marktlokationen identifiziert, Kunde keiner bzw. mehreren zugeordnet"
    ),
];

/// `E_3002` / `G_0067` + `G_0068` — Abmeldung NN vom NB (44007 → 44008/44009).
pub const EBD_ABMELDUNG_GAS: &str = "E_3002";
pub const E_3002_CODES: &[AntwortCode] = &[
    code!("E15", None, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "E13",
        None,
        Ablehnung,
        "Ablehnung — Bilanzierungsproblem (Bilanzkreis unbekannt oder nicht in der Zuordnungsermächtigung)"
    ),
    code!("E14", None, Ablehnung, "Ablehnung Sonstiges", bemerkung),
    code!(
        "E17",
        None,
        Ablehnung,
        "Ablehnung wegen Fristüberschreitung"
    ),
    code!(
        "Z08",
        None,
        Ablehnung,
        "Ablehnung — Transaktion schon stattgefunden"
    ),
    code!(
        "Z09",
        None,
        Ablehnung,
        "Ablehnung — Transaktionsgrund unplausibel"
    ),
    code!("Z14", None, Ablehnung, "Ablehnung — Doppelmeldung"),
];

/// `E_3020` / `G_0009` + `G_0010` — Abmeldungsanfrage des NB (44010 → 44011/44012).
pub const EBD_ABMELDUNGSANFRAGE_GAS: &str = "E_3020";
pub const E_3020_CODES: &[AntwortCode] = &[
    code!("E15", None, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        None,
        Zustimmung,
        "Zustimmung mit Terminänderung (nur bei Transaktionsgrund E01 Ein-/Auszug)"
    ),
    code!(
        "E17",
        None,
        Ablehnung,
        "Ablehnung wegen Fristüberschreitung"
    ),
    code!(
        "Z08",
        None,
        Ablehnung,
        "Ablehnung — Transaktion schon stattgefunden"
    ),
    code!(
        "Z09",
        None,
        Ablehnung,
        "Ablehnung — Transaktionsgrund unplausibel"
    ),
    code!("Z12", None, Ablehnung, "Ablehnung Vertragsbindung"),
    code!("Z14", None, Ablehnung, "Ablehnung — Doppelmeldung"),
];

// ── E_0615 / E_3008 — Anmeldung E/G prüfen (55013 / 44013) ───────────────────

/// EBD id of „Anmeldung E/G prüfen" — the tree the Grund-/Ersatzversorger runs
/// when the NB assigns it a contractless Marktlokation (§ 36 / § 38 EnWG).
///
/// This is the LF's *Anmeldung* tree. The supplier has no other: it **sends**
/// the ordinary Anmeldung (55001) and the NB answers it (`E_0622`); the one
/// Anmeldung a supplier is asked to check is the one it is assigned.
pub const EBD_ANMELDUNG_EOG: &str = "E_0615";
const E_0615: Option<&'static str> = Some(EBD_ANMELDUNG_EOG);

/// `E_0615` — Anmeldung E/G (55013 → 55014 / 55015).
pub const E_0615_CODES: &[AntwortCode] = &[
    code!(
        "A02",
        E_0615,
        Ablehnung,
        "Keine Zuständigkeit — die Marktlokation liegt nicht im Grundversorgungsgebiet des Empfängers und es besteht keine vertragliche Vereinbarung zur Ersatzbelieferung"
    ),
    code!("A03", E_0615, Ablehnung, "Frist nicht eingehalten"),
    code!(
        "A04",
        E_0615,
        Ablehnung,
        "Doppelmeldung — der Geschäftsvorfall wurde bereits zum gleichen Zeitpunkt bestätigt"
    ),
    code!(
        "A05",
        E_0615,
        Ablehnung,
        "Kein Grund-/Ersatzversorgungsfall bzw. kein Ersatzbelieferungs- oder Übergangsversorgungsfall"
    ),
    code!("A09", E_0615, Zustimmung, "Zustimmung"),
    code!("A99", E_0615, Ablehnung, "Sonstiges", bemerkung),
];

/// `E_3008` / `G_0013` + `G_0014` — Anmeldung E/G Gas (44013 → 44014 / 44015).
pub const EBD_ANMELDUNG_EOG_GAS: &str = "E_3008";
pub const E_3008_CODES: &[AntwortCode] = &[
    code!("E15", None, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z43",
        None,
        Zustimmung,
        "Zustimmung mit Korrektur von bilanzierungsrelevanten Daten"
    ),
    code!(
        "Z44",
        None,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!("E14", None, Ablehnung, "Ablehnung Sonstiges", bemerkung),
    code!(
        "E17",
        None,
        Ablehnung,
        "Ablehnung wegen Fristüberschreitung"
    ),
];

// ── E_0603…E_0606 — Zuordnung prüfen (55607 → 55608 / 55609) ─────────────────

/// The four „Zuordnung prüfen" trees, one per Anwendungsfall of the NB's
/// Ankündigung der LF-Zuordnung.
///
/// | EBD | Fall |
/// |---|---|
/// | `E_0603` | LF-Zuordnung bei EEG-Marktlokation (Fall 1) |
/// | `E_0604` | LF-Zuordnung bei EEG-Marktlokation mit DV-Pflicht (Fall 2) |
/// | `E_0605` | LF-Zuordnung bei KWKG-Marktlokation (Fall 3) |
/// | `E_0606` | LF-Zuordnung (Fall 4) |
///
/// All four publish the same two codes and differ only in which Anwendungsfall
/// they belong to, so one table serves them; the caller names the id the
/// inbound message carried in `SG4 STS+E01` DE 1131.
pub const EBD_ZUORDNUNG_LF: &[&str] = &["E_0603", "E_0604", "E_0605", "E_0606"];

macro_rules! zuordnung_codes {
    ($konst:ident, $ebd:literal) => {
        #[doc = concat!("`", $ebd, "` — Zuordnung prüfen (55607 → 55608 / 55609).")]
        pub const $konst: &[AntwortCode] = &[
            code!("A01", Some($ebd), Zustimmung, "Zustimmung"),
            code!("A99", Some($ebd), Ablehnung, "Sonstiges", bemerkung),
        ];
    };
}
zuordnung_codes!(E_0603_CODES, "E_0603");
zuordnung_codes!(E_0604_CODES, "E_0604");
zuordnung_codes!(E_0605_CODES, "E_0605");
zuordnung_codes!(E_0606_CODES, "E_0606");

// ── E_0406 — Netznutzungsrechnung prüfen (INVOIC → REMADV 33001 / 33002) ─────

/// EBD id of „Netznutzungsrechnung prüfen" — the tree the invoice recipient
/// runs on an inbound Netznutzungsrechnung.
///
/// Its codes travel in REMADV `AJT` DE 4465, with this id in DE 1082 — the
/// REMADV counterpart of UTILMD's `STS+E01++<code>:<ebd>`.
pub const EBD_NETZNUTZUNGSRECHNUNG: &str = "E_0406";
const E_0406: Option<&'static str> = Some(EBD_NETZNUTZUNGSRECHNUNG);

/// The `E_0406` codes this crate resolves.
///
/// **Partial by design.** The published tree has 205 Prüfschritte across three
/// levels (Kopf, Position, Summe) and 87 codes, and its result is a *set* of
/// (Positionsnummer, code) pairs rather than one code. The entries here are the
/// ones an arithmetic invoice check can land on without walking the tree:
/// the Summen- and Positions-level catch-alls, and the sum check that has an
/// exact counterpart. Anything else belongs to the full walk.
pub const E_0406_CODES: &[AntwortCode] = &[
    code!(
        "A70",
        E_0406,
        Ablehnung,
        "Der Rechnungsbetrag entspricht nicht der Summe aller Rechnungspositionen (Prüfschritt 900)"
    ),
    code!(
        "A96",
        E_0406,
        Ablehnung,
        "Sonstiges — ein zuvor nicht spezifizierter Fehler im Summenteil (Prüfschritt 940)",
        bemerkung
    ),
    code!(
        "A99",
        E_0406,
        Ablehnung,
        "Sonstiges — ein zuvor nicht spezifizierter Fehler in der Rechnungsposition",
        bemerkung
    ),
];

// ── E_0622 — Prüfen, ob Anmeldung direkt ablehnbar (55001/55077 → 55003/55080) ─

/// EBD id of „Prüfen, ob Anmeldung direkt ablehnbar" — the Netzbetreiber's
/// first tree on an inbound Strom Anmeldung.
///
/// Every code it publishes is an **Ablehnung**: the tree has no Zustimmung of
/// its own. A message that survives it continues into `E_0621` and then
/// [`EBD_LIEFERBEGINN`] (`E_0623`), which is where the Bestätigungscode comes
/// from. That asymmetry is why `CODELISTEN`'s both-clusters invariant exempts
/// this tree by name.
pub const EBD_ANMELDUNG_DIREKT_ABLEHNBAR: &str = "E_0622";
const E_0622: Option<&'static str> = Some(EBD_ANMELDUNG_DIREKT_ABLEHNBAR);

/// `E_0622` — the codes of the **verbrauchende / ruhende** Marktlokation branch
/// (Prüfschritte 15–70) and of the **erzeugende** branch (220–830).
///
/// The two branches are disjoint and reached by Prüfschritt 10; they share no
/// code. `A06` „Andere Anmeldung in Bearbeitung" is the verbrauchende answer
/// and `A45` the erzeugende one for the *same* condition — sending `A06` on an
/// Anmeldung erzeugender Marktlokation is an undefined code, not a wrong reason.
pub const E_0622_CODES: &[AntwortCode] = &[
    // ── verbrauchende / ruhende Marktlokation (Prüfschritte 15–70) ──
    code!(
        "A07",
        E_0622,
        Ablehnung,
        "Vorlauffrist wurde nicht eingehalten (Prüfschritt 15)"
    ),
    code!(
        "A09",
        E_0622,
        Ablehnung,
        "Bei der angemeldeten „ruhenden Marktlokation\" handelt es sich nicht um eine verbrauchende Marktlokation (Prüfschritt 18)"
    ),
    code!(
        "A47",
        E_0622,
        Ablehnung,
        "Die genannte Marktlokation entspricht nicht den Anforderungen, da die messtechnische Einordnung nicht iMS ist (Prüfschritt 22)"
    ),
    code!(
        "A08",
        E_0622,
        Ablehnung,
        "Bei der in der Anmeldung genannten Marktlokation (SG5 LOC+Z16) handelt es sich nicht um eine „Kundenanlage\" (Prüfschritt 25)"
    ),
    code!(
        "A37",
        E_0622,
        Ablehnung,
        "Die zu integrierende Marktlokation befindet sich nicht hinter der/den gleichen Netzlokation(en) wie die Marktlokation der Kundenanlage (Prüfschritt 26)"
    ),
    code!(
        "A46",
        E_0622,
        Ablehnung,
        "Die zu integrierende Marktlokation entspricht nicht den Anforderungen, da die messtechnische Einordnung nicht iMS ist (Prüfschritt 28)"
    ),
    code!(
        "A02",
        E_0622,
        Ablehnung,
        "Marktlokation, die über die Marktlokations-ID identifiziert wurde, nimmt nicht an der Marktkommunikation teil (Prüfschritt 30)"
    ),
    code!(
        "A04",
        E_0622,
        Ablehnung,
        "Falscher Prozess — es handelt sich um eine Anmeldung für eine Neuanlage (Prüfschritt 50)"
    ),
    code!(
        "A05",
        E_0622,
        Ablehnung,
        "Anforderungen können nicht erfüllt werden — die Abweichungen sind zu benennen (Prüfschritt 60)",
        bemerkung
    ),
    code!(
        "A06",
        E_0622,
        Ablehnung,
        "Andere Anmeldung in Bearbeitung (Prüfschritt 70)"
    ),
    // ── erzeugende Marktlokation / Tranche (Prüfschritte 220–830) ──
    code!(
        "A21",
        E_0622,
        Ablehnung,
        "Falscher Prozess — es handelt sich um einen „Einzug in Neuanlage\" (Prüfschritt 220)"
    ),
    code!(
        "A24",
        E_0622,
        Ablehnung,
        "Es liegt nicht an allen Messeinrichtungen, die für die Energiemengenermittlung der Marktlokation notwendig sind, die Messtechnik für eine viertelstündliche Messung vor (Prüfschritt 250)"
    ),
    code!(
        "A25",
        E_0622,
        Ablehnung,
        "Anforderungen können nicht erfüllt werden — die Abweichungen sind zu benennen (Prüfschritt 260)",
        bemerkung
    ),
    code!(
        "A45",
        E_0622,
        Ablehnung,
        "Andere Anmeldung in Bearbeitung (Prüfschritt 270)"
    ),
    code!(
        "A34",
        E_0622,
        Ablehnung,
        "Die Vorlauffrist für eine „Nicht-EEG-/-KWKG\"-Marktlokation wurde nicht eingehalten (Geschäftsvorfall 1, Prüfschritt 406)"
    ),
    code!(
        "A27",
        E_0622,
        Ablehnung,
        "Vorgaben EEG nicht eingehalten — der Lieferbeginn ist nicht der 1. eines Kalendermonats, 00:00 Uhr (Geschäftsvorfall 1, Prüfschritt 410)"
    ),
    code!(
        "A28",
        E_0622,
        Ablehnung,
        "Die Vorlauffrist für EEG-/KWKG-Marktlokationen im Geschäftsvorfall 1 wurde nicht eingehalten (Prüfschritt 430)"
    ),
    code!(
        "A29",
        E_0622,
        Ablehnung,
        "Die verkürzte Vorlauffrist für EEG-/KWKG-Marktlokationen im Geschäftsvorfall 1 wurde nicht eingehalten (Prüfschritt 440)"
    ),
    code!(
        "A30",
        E_0622,
        Ablehnung,
        "Die Vorlauffrist für eine „Nicht-EEG-/-KWKG\"-Marktlokation im Geschäftsvorfall 2 wurde nicht eingehalten (Prüfschritt 610)"
    ),
    code!(
        "A31",
        E_0622,
        Ablehnung,
        "Der Lieferbeginn darf nur der 1. eines Kalendermonats, 00:00 Uhr sein (Geschäftsvorfall 2, Prüfschritt 620)"
    ),
    code!(
        "A32",
        E_0622,
        Ablehnung,
        "Die Vorlauffrist für EEG-/KWKG-Marktlokationen im Geschäftsvorfall 2 wurde nicht eingehalten (Prüfschritt 630)"
    ),
    code!(
        "A35",
        E_0622,
        Ablehnung,
        "Die Vorlauffrist für eine „Nicht-EEG-/-KWKG\"-Marktlokation wurde nicht eingehalten (Geschäftsvorfall 3, Prüfschritt 806)"
    ),
    code!(
        "A44",
        E_0622,
        Ablehnung,
        "Fristüberschreitung — die Vorlauffrist von einem Monat wurde nicht eingehalten (Geschäftsvorfall 3, Prüfschritt 810)"
    ),
];

// ── E_0623 — Lieferbeginn prüfen (55001/55077 → 55002/55078 · 55003/55080) ────

/// EBD id of „Lieferbeginn prüfen" — the tree that produces the **Bestätigung**
/// of an Anmeldung, once `E_0622` has not refused it and the `E_0621` Anfrage
/// zur Beendigung der Zuordnung (where one was needed) has been answered.
///
/// A Bestätigung is `A51` (verbrauchende / ruhende Marktlokation), `A58`
/// (erzeugende Marktlokation, Geschäftsvorfall 1 / 2) or `A55` / `A56`
/// (Geschäftsvorfall 3, with or without a direktvermarktungspflichtiger
/// Restanteil). „Kein Code" is not one of the options: the AHB marks
/// `SG4 STS+E01` Muss on every Antwortnachricht.
pub const EBD_LIEFERBEGINN: &str = "E_0623";
const E_0623: Option<&'static str> = Some(EBD_LIEFERBEGINN);

/// `E_0623` — Lieferbeginn prüfen.
pub const E_0623_CODES: &[AntwortCode] = &[
    code!(
        "A50",
        E_0623,
        Ablehnung,
        "Der LFA hat der Anfrage zur Beendigung der Zuordnung widersprochen (Prüfschritt 50)"
    ),
    code!("A51", E_0623, Zustimmung, "Zustimmung (Prüfschritt 60)"),
    code!(
        "A53",
        E_0623,
        Ablehnung,
        "Der gewünschte Prozentsatz an der Marktlokation ist nicht frei — keiner Anfrage zur Beendigung der Zuordnung wurde zugestimmt (Prüfschritt 510)"
    ),
    code!(
        "A54",
        E_0623,
        Ablehnung,
        "Der gewünschte Prozentsatz an der Marktlokation ist nicht frei (Prüfschritt 520)"
    ),
    code!(
        "A55",
        E_0623,
        Zustimmung,
        "Zustimmung unter Bildung einer neuen Tranche, mit Information über fehlende Anteile an der Marktlokation in der Bilanzierung (Prüfschritt 540)"
    ),
    code!(
        "A56",
        E_0623,
        Zustimmung,
        "Zustimmung unter Bildung einer neuen Tranche (Prüfschritt 600)"
    ),
    code!(
        "A57",
        E_0623,
        Ablehnung,
        "Der LFA hat der Anfrage zur Beendigung der Zuordnung widersprochen (Prüfschritt 440)"
    ),
    code!(
        "A58",
        E_0623,
        Zustimmung,
        "Zustimmung (erzeugende Marktlokation, Prüfschritt 450)"
    ),
    code!("A99", E_0623, Ablehnung, "Sonstiges", bemerkung),
];

// ── E_0607 — Abmeldung prüfen (55004 → 55005 / 55006) ────────────────────────

/// EBD id of „Abmeldung prüfen" — the NB's tree on an inbound Strom Abmeldung.
///
/// `A02` here is „Vorlauffrist nicht eingehalten"; in
/// [`EBD_ANMELDUNG_DIREKT_ABLEHNBAR`] the same
/// string means „nimmt nicht an der Marktkommunikation teil". The lookup is
/// per tree for exactly this reason.
pub const EBD_ABMELDUNG_NB: &str = "E_0607";
const E_0607: Option<&'static str> = Some(EBD_ABMELDUNG_NB);

/// `E_0607` — Abmeldung prüfen.
pub const E_0607_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        E_0607,
        Ablehnung,
        "Bei der in der Abmeldung genannten Marktlokation handelt es sich nicht um eine Kundenanlage (Prüfschritt 30)"
    ),
    code!(
        "A02",
        E_0607,
        Ablehnung,
        "Vorlauffrist nicht eingehalten (Prüfschritt 50)"
    ),
    code!(
        "A05",
        E_0607,
        Ablehnung,
        "Die Marktlokation wurde nicht innerhalb der letzten 3 Monate zur Ersatz-/Grundversorgung angemeldet — es kann sich nicht um eine Beendigung einer ESV handeln (Prüfschritt 80)"
    ),
    code!(
        "A06",
        E_0607,
        Ablehnung,
        "Die Aufhebung einer zukünftigen Zuordnung muss zu demselben Zeitpunkt angegeben werden, der im Lieferbeginn bestätigt wurde (Prüfschritt 90)"
    ),
    code!(
        "A09",
        E_0607,
        Ablehnung,
        "Lieferende zum Abmeldedatum wurde bereits bestätigt (Prüfschritt 120)"
    ),
    code!(
        "A10",
        E_0607,
        Ablehnung,
        "Lieferende zum Abmeldedatum wurde aus gleichem Grund bereits bestätigt (Prüfschritt 130)"
    ),
    code!("A11", E_0607, Zustimmung, "Zustimmung (Prüfschritt 140)"),
    code!("A99", E_0607, Ablehnung, "Sonstiges", bemerkung),
];

// ── E_0608 — Anmeldung einer Zuordnung / Neuanlage (55600/55601 → 55602–55605) ─

/// EBD id of „Anmeldung einer Zuordnung" — the NB's tree on a **Neuanlage**
/// (GPKE Teil 2 § 2.2), where the Lieferant registers a Marktlokation that is
/// being commissioned for the first time.
///
/// Prüfschritte 110 and 590 are the tree's own loop: an Anmeldung whose
/// Marktlokation cannot yet be identified is **not refused** — the NB re-checks
/// it daily and only answers `A07` / `A16` once it has been open for more than
/// **60 Werktage**. That is why the answer Frist is 00:00 Uhr des 61. WT and
/// not a day.
pub const EBD_NEUANLAGE: &str = "E_0608";
const E_0608: Option<&'static str> = Some(EBD_NEUANLAGE);

/// `E_0608` — Anmeldung einer Zuordnung (Neuanlage).
///
/// Prüfschritte 10–130 are the verbrauchende branch (`A01`–`A09`), 500–610 the
/// erzeugende one (`A10`–`A19`). `A09` and `A18` are the two Zustimmungen.
pub const E_0608_CODES: &[AntwortCode] = &[
    // verbrauchende Marktlokation (Prüfschritte 20–130)
    code!(
        "A01",
        E_0608,
        Ablehnung,
        "Vorlauffrist wurde nicht eingehalten (Prüfschritt 20)"
    ),
    code!(
        "A02",
        E_0608,
        Ablehnung,
        "Identifizierte Marktlokation nimmt nicht an der Marktkommunikation teil; weiterhin handelt es sich nicht um eine Neuanlage (Prüfschritt 40)"
    ),
    code!(
        "A03",
        E_0608,
        Ablehnung,
        "Keine Neuanlage, falscher Anwendungsfall (Prüfschritt 60)"
    ),
    code!(
        "A04",
        E_0608,
        Ablehnung,
        "Falscher Anwendungsfall — es ist bereits ein LF zugeordnet (Prüfschritt 70)"
    ),
    code!(
        "A05",
        E_0608,
        Ablehnung,
        "Marktlokation befindet sich zum Eingangsdatum der Meldung nicht mehr im Netzgebiet des NB (Prüfschritt 80)"
    ),
    code!(
        "A06",
        E_0608,
        Ablehnung,
        "Anforderungen können nicht erfüllt werden — die Abweichungen sind zu benennen (Prüfschritt 90)",
        bemerkung
    ),
    code!(
        "A07",
        E_0608,
        Ablehnung,
        "Neu angelegte Marktlokation konnte nicht identifiziert werden (Prüfschritt 110 — mehr als 60 WT offen)"
    ),
    code!(
        "A08",
        E_0608,
        Ablehnung,
        "Keine- oder Mehrfachidentifizierung (Prüfschritt 55)"
    ),
    code!("A09", E_0608, Zustimmung, "Zustimmung (Prüfschritt 130)"),
    // erzeugende Marktlokation / Tranche (Prüfschritte 500–610)
    code!(
        "A10",
        E_0608,
        Ablehnung,
        "Vorlauffrist wurde nicht eingehalten (Prüfschritt 500)"
    ),
    code!(
        "A11",
        E_0608,
        Ablehnung,
        "Identifizierte Marktlokation nimmt nicht an der Marktkommunikation teil; weiterhin handelt es sich nicht um eine Neuanlage (Prüfschritt 520)"
    ),
    code!(
        "A12",
        E_0608,
        Ablehnung,
        "Keine Neuanlage, falscher Anwendungsfall (Prüfschritt 540)"
    ),
    code!(
        "A13",
        E_0608,
        Ablehnung,
        "Falscher Anwendungsfall — es ist bereits ein LF zugeordnet (Prüfschritt 550)"
    ),
    code!(
        "A14",
        E_0608,
        Ablehnung,
        "Marktlokation befindet sich zum Eingangsdatum der Meldung nicht mehr im Netzgebiet des NB (Prüfschritt 560)"
    ),
    code!(
        "A15",
        E_0608,
        Ablehnung,
        "Anforderungen können nicht erfüllt werden — die Abweichungen sind zu benennen (Prüfschritt 570)",
        bemerkung
    ),
    code!(
        "A16",
        E_0608,
        Ablehnung,
        "Neu angelegte Marktlokation konnte nicht identifiziert werden (Prüfschritt 590 — mehr als 60 WT offen)"
    ),
    code!(
        "A17",
        E_0608,
        Ablehnung,
        "Keine- oder Mehrfachidentifizierung (Prüfschritt 535)"
    ),
    code!("A18", E_0608, Zustimmung, "Zustimmung (Prüfschritt 610)"),
    code!(
        "A19",
        E_0608,
        Ablehnung,
        "Es liegt nicht an allen Messeinrichtungen, die für die Energiemengenermittlung der Marktlokation notwendig sind, die Messtechnik für eine viertelstündliche Messung vor (Prüfschritt 545)"
    ),
    code!("A99", E_0608, Ablehnung, "Sonstiges", bemerkung),
];

// ── E_3005 / E_3007 — Anmeldung Gas (44001 → 44002 / 44003) ──────────────────

/// EBD id of the Gas „Prüfen, ob Anmeldung direkt ablehnbar" (Codeliste
/// `G_0011`).
///
/// The Gas Ablehnungscodes are a **different alphabet** from Strom's: the
/// „nimmt nicht an der Marktkommunikation teil" refusal is `A16`, not `A02`;
/// „andere Anmeldung in Bearbeitung" is `ZC5`, not `A06`; a Fristüberschreitung
/// is `E17`, not `A07`; and a Bilanzkreis-/Zuordnungsermächtigungsproblem is
/// `E13`, not `A05`. Putting a Strom code on a 44003 is an undefined code.
pub const EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS: &str = "E_3005";

/// `E_3005` / `G_0011` — Ablehnung der Anmeldung (Gas).
///
/// The AHB requires the checks behind `A03`, `A04`, `A16` and `A17` to run
/// **first**, before any of the Frist- or Bilanzierungsprüfungen.
pub const E_3005_CODES: &[AntwortCode] = &[
    code!("A03", None, Ablehnung, "Ablehnung (Keine Identifizierung)"),
    code!(
        "A04",
        None,
        Ablehnung,
        "Ablehnung — Marktlokation befindet sich zum Eingangsdatum der Meldung nicht mehr im Netzgebiet des NB"
    ),
    code!(
        "A16",
        None,
        Ablehnung,
        "Ablehnung — identifizierte Marktlokation nimmt nicht an der Marktkommunikation teil"
    ),
    code!(
        "A17",
        None,
        Ablehnung,
        "Ablehnung (Mehrfachidentifizierung)"
    ),
    code!(
        "E13",
        None,
        Ablehnung,
        "Ablehnung (Bilanzierungsproblem) — der Bilanzkreis ist unbekannt oder Bilanzkreis/Zeitreihentyp sind in der Zuordnungsermächtigung nicht aufgeführt"
    ),
    code!("E14", None, Ablehnung, "Ablehnung Sonstiges", bemerkung),
    code!(
        "E17",
        None,
        Ablehnung,
        "Ablehnung wegen Fristüberschreitung"
    ),
    code!(
        "Z08",
        None,
        Ablehnung,
        "Ablehnung (Transaktion schon stattgefunden)"
    ),
    code!(
        "Z09",
        None,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
    code!("Z14", None, Ablehnung, "Ablehnung (Doppelmeldung)"),
    code!(
        "Z35",
        None,
        Ablehnung,
        "Ablehnung der Abmeldeanfrage — negative Antwort des LFA auf die Abmeldeanfrage des NB"
    ),
    code!(
        "ZC5",
        None,
        Ablehnung,
        "Ablehnung (andere Anmeldung in Bearbeitung)"
    ),
    code!(
        "ZE2",
        None,
        Ablehnung,
        "Ablehnung Kapazitätsproblem — im angemeldeten Marktgebiet ist keine Kapazität vorhanden"
    ),
];

/// EBD id of the Gas „Lieferbeginn prüfen" (Codelisten `G_0012` Bestätigung and
/// `G_0011` Ablehnung).
pub const EBD_LIEFERBEGINN_GAS: &str = "E_3007";

/// `E_3007` — Bestätigung (`G_0012`) und Ablehnung (`G_0011`) der Gas-Anmeldung.
pub const E_3007_CODES: &[AntwortCode] = &[
    code!("E15", None, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!("Z01", None, Zustimmung, "Zustimmung mit Terminänderung"),
    code!(
        "Z43",
        None,
        Zustimmung,
        "Zustimmung mit Korrektur von bilanzierungsrelevanten Daten"
    ),
    code!(
        "Z44",
        None,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!(
        "E13",
        None,
        Ablehnung,
        "Ablehnung (Bilanzierungsproblem) — der Bilanzkreis ist unbekannt oder Bilanzkreis/Zeitreihentyp sind in der Zuordnungsermächtigung nicht aufgeführt"
    ),
    code!("E14", None, Ablehnung, "Ablehnung Sonstiges", bemerkung),
    code!(
        "E17",
        None,
        Ablehnung,
        "Ablehnung wegen Fristüberschreitung"
    ),
    code!(
        "Z08",
        None,
        Ablehnung,
        "Ablehnung (Transaktion schon stattgefunden)"
    ),
    code!(
        "Z09",
        None,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
    code!("Z14", None, Ablehnung, "Ablehnung (Doppelmeldung)"),
    code!(
        "Z35",
        None,
        Ablehnung,
        "Ablehnung der Abmeldeanfrage — negative Antwort des LFA auf die Abmeldeanfrage des NB"
    ),
];

// ── E_3019 — Abmeldung prüfen Gas (44004 → 44005 / 44006) ────────────────────

/// EBD id of the Gas „Abmeldung prüfen" (Codelisten `G_0007` Ablehnung and
/// `G_0008` Bestätigung).
///
/// Four Ablehnungscodes only — the Strom tree's `A02` / `A09` / `A10` do not
/// exist here. A Fristüberschreitung is `E17`; a Lieferende that was already
/// confirmed is `Z08`; a redelivered message is `Z14`.
pub const EBD_ABMELDUNG_GAS_NB: &str = "E_3019";

/// `E_3019` — Bestätigung (`G_0008`) und Ablehnung (`G_0007`) der Gas-Abmeldung.
pub const E_3019_CODES: &[AntwortCode] = &[
    code!("E15", None, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!("Z01", None, Zustimmung, "Zustimmung mit Terminänderung"),
    code!("E14", None, Ablehnung, "Ablehnung Sonstiges", bemerkung),
    code!(
        "E17",
        None,
        Ablehnung,
        "Ablehnung wegen Fristüberschreitung"
    ),
    code!(
        "Z08",
        None,
        Ablehnung,
        "Ablehnung (Transaktion schon stattgefunden)"
    ),
    code!("Z14", None, Ablehnung, "Ablehnung (Doppelmeldung)"),
];

// ── E_0595 — Bestellung prüfen (55156/55220/55673 → IFTSTA 21047) ────────────

/// EBD id of „Bestellung prüfen" — the NB's answer to a Bestellung einer
/// Änderung von Abrechnungsdaten (GPKE Teil 2 § 3.1.3) and to a Bestellung zur
/// Stammdatenänderung (Teil 4 § 1.5).
///
/// **Its clusters are not Zustimmung and Ablehnung.** The BDEW prints „Änderung
/// der Daten" / „keine Änderung der Daten", and the Hinweis spells out what that
/// decides: „Eine Stammdatenänderung wird versendet" versus „wird nicht
/// versendet". `A06` is „Änderung der Daten" *while stating that no change is
/// made*, because the Verantwortliche still sends its own data back. Reading
/// that cluster as agreement inverts five codes.
///
/// The answer always rides IFTSTA **21047** regardless of cluster, so nothing
/// here selects a PID — which is exactly why the axis had to stay separate.
pub const EBD_BESTELLUNG: &str = "E_0595";
const E_0595: Option<&'static str> = Some(EBD_BESTELLUNG);

/// The `E_0595` codes that answer a **UTILMD** Bestellung des Stammdaten-Clearing.
///
/// Prüfschritt 10 splits the tree: „ja" (→ 15) is a Bestellung mittels ORDERS,
/// „nein" (→ 210) the Clearing. Only the second branch answers 55156 / 55220 /
/// 55673 with an IFTSTA 21047, so an `A2x` code on one of those is a code the
/// branch does not define.
pub const E_0595_CLEARING_CODES: &[&str] = &["A01", "A02", "A03", "A04", "A05", "A06"];

/// `E_0595` — Bestellung prüfen.
///
/// Prüfschritte 10–200 answer an ORDERS Bestellung, 210–250 a UTILMD
/// Stammdaten-Clearing ([`E_0595_CLEARING_CODES`]). The six codes of the
/// Clearing branch encode a *pair* of facts — whether the Berechtigte's data
/// matched, and whether the Verantwortliche changes anything — which is why
/// there are six and not four.
pub const E_0595_CODES: &[AntwortCode] = &[
    // ── Stammdaten-Clearing (Prüfschritte 210–250) ──
    code!(
        "A01",
        E_0595,
        KeineAenderungDerDaten,
        "Die vorliegenden Daten stimmen überein; es wurden keine Stammdaten zur Änderung angegeben (Prüfschritt 220)"
    ),
    code!(
        "A02",
        E_0595,
        AenderungDerDaten,
        "Die vorliegenden Daten stimmen überein; Änderungen an den Stammdaten werden vorgenommen (Prüfschritt 230)"
    ),
    code!(
        "A03",
        E_0595,
        KeineAenderungDerDaten,
        "Die vorliegenden Daten stimmen überein; Änderungen an den Stammdaten werden nicht vorgenommen (Prüfschritt 230)"
    ),
    code!(
        "A04",
        E_0595,
        AenderungDerDaten,
        "Die vorliegenden Daten stimmen nicht überein; es wurden keine Stammdaten zur Änderung angegeben (Prüfschritt 240)"
    ),
    code!(
        "A05",
        E_0595,
        AenderungDerDaten,
        "Die vorliegenden Daten stimmen nicht überein; Änderungen an den Stammdaten werden vorgenommen (Prüfschritt 250)"
    ),
    code!(
        "A06",
        E_0595,
        AenderungDerDaten,
        "Die vorliegenden Daten stimmen nicht überein; Änderungen werden nicht vorgenommen — die Stammdaten werden dennoch versendet (Prüfschritt 250)"
    ),
    // ── ORDERS-Bestellung (Prüfschritte 10–200) ──
    code!(
        "A20",
        E_0595,
        AenderungDerDaten,
        "Der Bestellung der Stammdaten konnte zugestimmt werden — der NB versendet neue Stammdaten (Prüfschritt 200)"
    ),
    code!(
        "A21",
        E_0595,
        KeineAenderungDerDaten,
        "Die Frist zur Änderung wurde nicht eingehalten — Änderung der Netzentgelte aufgrund netzorientierter Steuerungsmöglichkeit, mindestens 2 WT vor dem Änderungszeitpunkt (Prüfschritt 17)"
    ),
    code!(
        "A22",
        E_0595,
        KeineAenderungDerDaten,
        "Die Frist zur Änderung von Bilanzkreis oder Jahresverbrauchsprognose wurde nicht eingehalten — mindestens 7 WT vor dem Änderungszeitpunkt (Prüfschritt 100)"
    ),
    code!(
        "A23",
        E_0595,
        KeineAenderungDerDaten,
        "Sondervertragskunden-Konzessionsabgabe gemäß § 2 Abs. 3 KAV, daher keine Änderung möglich (Prüfschritt 40)"
    ),
    code!(
        "A24",
        E_0595,
        KeineAenderungDerDaten,
        "Änderung nicht möglich, da die Marktlokation von der Konzessionsabgabe befreit ist (Prüfschritt 50)"
    ),
    code!(
        "A25",
        E_0595,
        KeineAenderungDerDaten,
        "Der gewünschte Zustand ist bereits an der Marktlokation hinterlegt (Prüfschritt 55)"
    ),
    code!(
        "A26",
        E_0595,
        KeineAenderungDerDaten,
        "Eine rückwirkende Änderung der Konzessionsabgabe wird abgelehnt (Prüfschritt 65)"
    ),
    code!(
        "A27",
        E_0595,
        KeineAenderungDerDaten,
        "An der Marktlokation kann die Energie in den Schwachlastzeiten nicht zum angefragten Zeitpunkt separat erfasst werden (Prüfschritt 70)"
    ),
    code!(
        "A28",
        E_0595,
        KeineAenderungDerDaten,
        "Bilanzkreis nicht gültig (Prüfschritt 110)"
    ),
    code!(
        "A29",
        E_0595,
        KeineAenderungDerDaten,
        "Bilanzkreis und der erforderliche Zeitreihentyp sind in der Zuordnungsermächtigung nicht aufgeführt (Prüfschritt 120)"
    ),
    code!(
        "A99",
        E_0595,
        KeineAenderungDerDaten,
        "Der Bestellung der Stammdaten konnte nicht zugestimmt werden — das identifizierte Problem ist zu benennen (Prüfschritt 200)",
        bemerkung
    ),
];

// ── Lookup ───────────────────────────────────────────────────────────────────

// ── WiM Strom (Messstellenbetrieb) ────────────────────────────────────────────
//
// Source: Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 8 „WiM Strom".
//
// None of these alphabets overlaps the GPKE ones, and two of them reuse GPKE
// spellings for unrelated meanings: `A01`/`A02` are „Frist nicht eingehalten"
// and „Änderung kann durchgeführt werden" in the Messlokationsänderung trees,
// where in `E_0607` they are „Marktlokation nicht identifizierbar" and
// „Vorlauffrist nicht eingehalten". Resolve through [`lookup`], never by code.

/// `E_0200` — Kündigung Messstellenbetrieb prüfen. Prüfende Rolle: **MSBA**.
pub const EBD_KUENDIGUNG_MSB: &str = "E_0200";
const E_0200: Option<&'static str> = Some(EBD_KUENDIGUNG_MSB);

/// `E_0200` — the MSBA's answer to a Kündigung des Messstellenbetriebsvertrags.
///
/// `Z34` and `Z29` are the two outcomes WiM Teil 1 Kap. 2.2.3 („Antwort MSBA
/// bei Kündigung eines bereits wirksam gekündigten Vertrages") resolves to, and
/// `Z12` is the answer to a fixed Kündigungstermin the contract cannot honour —
/// it must carry the nächstmöglicher Kündigungszeitpunkt in `SG4 DTM`.
pub const E_0200_CODES: &[AntwortCode] = &[
    code!("E15", E_0200, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_0200,
        Zustimmung,
        "Zustimmung mit Terminänderung (nur wenn SG4 DTM+471 „Ende zum nächstmöglichen Termin\" vorhanden)"
    ),
    code!(
        "Z44",
        E_0200,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!("E11", E_0200, Ablehnung, "Ablehnung (Messproblem)"),
    code!(
        "Z12",
        E_0200,
        Ablehnung,
        "Ablehnung Vertragsbindung — der nächstmögliche Kündigungszeitpunkt ist in SG4 DTM mitzugeben"
    ),
    code!(
        "Z29",
        E_0200,
        Ablehnung,
        "Ablehnung (kein Vertragsverhältnis mehr vorhanden)"
    ),
    code!("Z34", E_0200, Ablehnung, "Ablehnung (Mehrfachkündigung)"),
    code!(
        "ZC9",
        E_0200,
        Ablehnung,
        "Ablehnung (keine Zuordnung möglich)"
    ),
];

/// `E_0201` — Anmeldung Messstellenbetrieb prüfen. Prüfende Rolle: **NB**.
pub const EBD_ANMELDUNG_MSB: &str = "E_0201";
const E_0201: Option<&'static str> = Some(EBD_ANMELDUNG_MSB);

/// `E_0201` — the NB's answer to an Anmeldung des Messstellenbetriebs.
///
/// `E17` is the Mindestvorlaufzeit of WiM Teil 1 Kap. 2.3.2 Nr. 1 (15 Werktage,
/// 7 bei erstmaliger Einrichtung) — the check Prozessschritt 2 requires and
/// [`mako_fristen::vorlauf`] computes. `ZB6` is the missing Versicherung über
/// die Beauftragung durch den Anschlussnutzer.
///
/// The tree publishes **no code for an unknown Marktpartner**. An MSB missing
/// from the Verzeichnisdienst is not an Ablehnungsgrund here, so that case must
/// escalate rather than resolve to a rejection.
pub const E_0201_CODES: &[AntwortCode] = &[
    code!("E15", E_0201, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_0201,
        Zustimmung,
        "Zustimmung mit Terminänderung (nur wenn SG4 STS+7++E02 „Einzug in eine Neuanlage\" vorhanden)"
    ),
    code!(
        "Z44",
        E_0201,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!("E11", E_0201, Ablehnung, "Ablehnung (Messproblem)"),
    code!(
        "E17",
        E_0201,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung — die Mindestvorlaufzeit ist nicht eingehalten"
    ),
    code!(
        "Z09",
        E_0201,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
    code!(
        "Z29",
        E_0201,
        Ablehnung,
        "Ablehnung (kein Vertragsverhältnis mehr vorhanden)"
    ),
    code!("ZB6", E_0201, Ablehnung, "Erforderliche Versicherung fehlt"),
    code!(
        "ZC9",
        E_0201,
        Ablehnung,
        "Ablehnung (keine Zuordnung möglich)"
    ),
];

/// `E_0202` — Abmeldung Messstellenbetrieb prüfen. Prüfende Rolle: **NB**.
pub const EBD_ABMELDUNG_MSB: &str = "E_0202";
const E_0202: Option<&'static str> = Some(EBD_ABMELDUNG_MSB);

/// `E_0202` — the NB's answer to an Ende Messstellenbetrieb.
///
/// The narrowest tree of the four, and deliberately so: it has **no `ZC9`**, so
/// the NB may not refuse an Abmeldung for „keine Zuordnung möglich", and no
/// `E11`. A Zuordnungsende that undershoots the 20-Werktage Mindestvorlauffrist
/// is not refused either — Kap. 2.4.2 Nr. 2 has the NB *move* it to the
/// nächstmögliches Zuordnungsende and confirm with `Z01`.
pub const E_0202_CODES: &[AntwortCode] = &[
    code!("E15", E_0202, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_0202,
        Zustimmung,
        "Zustimmung mit Terminänderung — das vom NB festgesetzte nächstmögliche Zuordnungsende"
    ),
    code!(
        "E17",
        E_0202,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung (nur bei Transaktionsgrund ZG9/ZH1/ZH2, Aufhebung einer zukünftigen Zuordnung)"
    ),
    code!(
        "Z09",
        E_0202,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
];

/// `E_0240` — Verpflichtungsanfrage prüfen. Prüfende Rolle: **gMSB**.
pub const EBD_VERPFLICHTUNGSANFRAGE: &str = "E_0240";
const E_0240: Option<&'static str> = Some(EBD_VERPFLICHTUNGSANFRAGE);

/// `E_0240` — the grundzuständiger MSB's answer to the NB's Verpflichtungsanfrage.
///
/// `Z07` „Keine Berechtigung" is published here and in no other MSB-Wechsel
/// tree: only this one lets the answering party say the sender was not entitled
/// to ask at all.
pub const E_0240_CODES: &[AntwortCode] = &[
    code!("E15", E_0240, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_0240,
        Zustimmung,
        "Zustimmung mit Terminänderung (nur wenn SG4 STS+7++E02 „Einzug in eine Neuanlage\" vorhanden)"
    ),
    code!(
        "Z44",
        E_0240,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!(
        "E17",
        E_0240,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung"
    ),
    code!("Z07", E_0240, Ablehnung, "Ablehnung (Keine Berechtigung)"),
    code!(
        "Z09",
        E_0240,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
    code!("ZB6", E_0240, Ablehnung, "Erforderliche Versicherung fehlt"),
];

/// `E_0203` — Weiterverpflichtung prüfen. Prüfende Rolle: **MSBA** (ORDRSP).
pub const EBD_WEITERVERPFLICHTUNG: &str = "E_0203";
const E_0203: Option<&'static str> = Some(EBD_WEITERVERPFLICHTUNG);

/// `E_0203` — the outgoing MSB's answer to the NB's Weiterverpflichtung.
///
/// The NB may keep the abmeldender MSB on the Messlokation for at most three
/// months on an Anschlussnutzerwechsel and one month otherwise (WiM Teil 1
/// Kap. 2.4.2 Nr. 4). `Z14` is how the MSBA answers a demand that overshoots
/// that window on the *first* ORDERS — it confirms and states the corrected
/// Abmeldetermin in `DTM` DE 2380. `Z22` refuses, and only on a *further*
/// ORDERS after the maximum has already been reached.
pub const E_0203_CODES: &[AntwortCode] = &[
    code!("Z13", E_0203, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z14",
        E_0203,
        Zustimmung,
        "Zustimmung mit Terminänderung — der korrigierte Abmeldetermin ist im DTM DE 2380 anzugeben"
    ),
    code!(
        "Z22",
        E_0203,
        Ablehnung,
        "Ablehnung wegen Überschreiten des Weiterverpflichtungszeitraums"
    ),
];

/// `E_0204` — Anzeige Gerätewechselabsicht prüfen. Prüfende Rolle: **MSBA**.
pub const EBD_GERAETEWECHSELABSICHT: &str = "E_0204";
const E_0204: Option<&'static str> = Some(EBD_GERAETEWECHSELABSICHT);

/// `E_0204` — the outgoing MSB's answer to the incoming MSB's
/// Gerätewechselabsicht.
///
/// **Neither cluster refuses the Gerätewechsel.** The AHB names 19015
/// „Bestätigung" and 19016 „Ablehnung Gerätewechselabsicht", but the codes say
/// what the two actually decide: `ZB4` „Eigenausbau wird erfolgen" — the MSBA
/// removes its own devices — versus `ZB5` „Kein Eigenausbau des MSBA", where
/// the MSBN does. Reading 19016 as a refusal aborts a Gerätewechsel the
/// counterparty has just agreed to carry out.
///
/// `E17` and `Z07` are the genuine refusals, and the Codeliste marks both Kann.
pub const E_0204_CODES: &[AntwortCode] = &[
    code!("ZB4", E_0204, Zustimmung, "Eigenausbau wird erfolgen"),
    code!("ZB5", E_0204, Ablehnung, "Kein Eigenausbau des MSBA"),
    code!(
        "E17",
        E_0204,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung — die Antwort ist spätestens am 2. WT vor dem Gerätewechseltermin fällig"
    ),
    code!("Z07", E_0204, Ablehnung, "Ablehnung (Keine Berechtigung)"),
];

/// `E_0247` — Bestellung (Geräteübernahme) prüfen. Prüfende Rolle: **MSBA**.
pub const EBD_BESTELLUNG_GERAETEUEBERNAHME: &str = "E_0247";
const E_0247: Option<&'static str> = Some(EBD_BESTELLUNG_GERAETEUEBERNAHME);

/// `E_0247` — the outgoing MSB's answer to a Geräteübernahme Bestellung.
///
/// `5` is a bare numeric code, not a truncation: the ORDRSP Codeliste uses the
/// plain EDIFACT DE 4465 value for „Preis / Rechenregel falsch".
pub const E_0247_CODES: &[AntwortCode] = &[
    code!("Z13", E_0247, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!("5", E_0247, Ablehnung, "Preis / Rechenregel falsch"),
    code!(
        "Z32",
        E_0247,
        Ablehnung,
        "Ablehnung Bestellumfang übersteigt Angebotsumfang"
    ),
];

/// `E_0249` — Beauftragung zur Messlokationsänderung prüfen, **vom NB**.
pub const EBD_MESSLOKATIONSAENDERUNG_NB: &str = "E_0249";
const E_0249: Option<&'static str> = Some(EBD_MESSLOKATIONSAENDERUNG_NB);

/// `E_0249` — the MSB's answer to an NB-initiated Messlokationsänderung.
///
/// One Prüfschritt: „Liegt das gewünschte Änderungsdatum mindestens 20 WT nach
/// dem Nachrichteneingangsdatum?" `A01` is that Frist and `A02` the Zustimmung,
/// and neither means what the same spelling means in any GPKE tree.
pub const E_0249_CODES: &[AntwortCode] = &[
    code!(
        "A02",
        E_0249,
        Zustimmung,
        "Änderung kann durchgeführt werden (Prüfschritt 1)"
    ),
    code!(
        "A01",
        E_0249,
        Ablehnung,
        "Frist nicht eingehalten — das Änderungsdatum liegt weniger als 20 WT nach dem Nachrichteneingang (Prüfschritt 1)"
    ),
];

/// `E_0250` — Beauftragung zur Messlokationsänderung prüfen, **vom LF**.
pub const EBD_MESSLOKATIONSAENDERUNG_LF: &str = "E_0250";
const E_0250: Option<&'static str> = Some(EBD_MESSLOKATIONSAENDERUNG_LF);

/// `E_0250` — the MSB's answer to an LF-initiated Messlokationsänderung.
///
/// The LF variant adds the Vollmacht Prüfschritte (20, 30) the NB variant has
/// no need for, on the **same answer PIDs** 19005/19006. Two trees, one PID
/// pair — which is why a code must be resolved against the tree the *sender's*
/// Marktrolle selects and never against the answer PID.
pub const E_0250_CODES: &[AntwortCode] = &[
    code!(
        "A02",
        E_0250,
        Zustimmung,
        "Änderung kann durchgeführt werden (Prüfschritt 40)"
    ),
    code!(
        "A03",
        E_0250,
        Ablehnung,
        "Vollmacht des Letztverbrauchers bzw. Erzeugers liegt nicht vor (Prüfschritt 20)"
    ),
    code!(
        "A04",
        E_0250,
        Ablehnung,
        "Vollmacht ist nicht plausibel und gültig (Prüfschritt 30)"
    ),
    code!(
        "A01",
        E_0250,
        Ablehnung,
        "Frist nicht eingehalten — das Änderungsdatum liegt weniger als 20 WT nach dem Nachrichteneingang (Prüfschritt 40)"
    ),
];

// ── E_0232 / E_2003 — Mitteilung über Gesamtvorgang prüfen ───────────────────
//
// The leg that makes a Zuordnung constitutive. The NB answers the MSBN's
// IFTSTA 21010/21009 with 21012 (erfolgreich) or 21011 (Scheitermeldung liegt
// vor), and only the *negative* side publishes a code: the AHB names an EBD on
// 21011 and none on 21012. The Zustimmung is therefore the PID itself, and the
// tree carries a single Ablehnungscode.

/// `E_0232` — Mitteilung über Gesamtvorgang prüfen (Strom). Prüfende Rolle: **NB**.
pub const EBD_GESAMTVORGANG: &str = "E_0232";
const E_0232: Option<&'static str> = Some(EBD_GESAMTVORGANG);

/// `E_0232` — the NB's answer to the MSBN's Gesamtvorgang report.
///
/// `Z66` is the only published code and it rides **21011**; the positive answer
/// 21012 carries no `STS+E01` at all (PID-Übersicht 4.0 rows 30140/30150).
/// The Zustimmung entry below is a `mako` construct so the „cluster picks the
/// PID" rule still resolves 21012 — it is never rendered, because the answer
/// has no Status-der-Antwort segment to put it in.
pub const E_0232_CODES: &[AntwortCode] = &[code!(
    "Z66",
    E_0232,
    Ablehnung,
    "MSB-Scheitermeldung liegt vor — der MSBA bleibt der Messlokation zugeordnet"
)];

/// `E_2003` — Mitteilung über Gesamtvorgang prüfen (**Gas**). Prüfende Rolle: **NB**.
pub const EBD_GESAMTVORGANG_GAS: &str = "E_2003";
const E_2003: Option<&'static str> = Some(EBD_GESAMTVORGANG_GAS);

/// `E_2003` — the Gas twin of [`E_0232_CODES`], published as Codeliste `G_0055`.
pub const E_2003_CODES: &[AntwortCode] = &[code!(
    "Z66",
    E_2003,
    Ablehnung,
    "MSB-Scheitermeldung liegt vor — der MSBA bleibt der Messlokation zugeordnet"
)];

// ── WiM Gas — Messstellenbetrieb (EBD 4.3 Kap. 14) ───────────────────────────
//
// WiM Gas is a structural mirror of WiM Strom: the same Use-Cases, the same
// Fristen and the same Prüfschritte, on the 44xxx UTILMD namespace and the
// shared ORDERS/ORDRSP/IFTSTA/REQOTE/QUOTES PIDs. What is **not** shared is the
// alphabet: every Gas answer resolves against an `E_20xx` tree and rides a
// `G_00xx` Codeliste. `A02`/`A05`/`S_0054` are undefined on a 44041, and the
// Gas lists are not always the Strom ones — `S_0056` publishes `ZC9` where
// `G_0053` does not.
//
// Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 Kap. 14,
// AWH WiM Gas 2.0 (gültig ab 01.10.2026), UTILMD AHB Gas 1.2 Kap. 6.

/// `E_2000` — Kündigung Messstellenbetrieb (Gas). Prüfende Rolle: **MSBA**.
pub const EBD_KUENDIGUNG_MSB_GAS: &str = "E_2000";
const E_2000: Option<&'static str> = Some(EBD_KUENDIGUNG_MSB_GAS);

/// `E_2000` — the MSBA's answer to a Kündigung des Messstellenbetriebsvertrags
/// (44039 → 44040 `G_0052` / 44041 `G_0051`).
///
/// Differs from the Strom twin `E_0200` in one code: Gas publishes no `ZC9`
/// („keine Zuordnung möglich"). An unidentifiable Messlokation therefore has no
/// Ablehnungscode in Gas and must escalate.
pub const E_2000_CODES: &[AntwortCode] = &[
    code!("E15", E_2000, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_2000,
        Zustimmung,
        "Zustimmung mit Terminänderung — nur wenn SG4 DTM+471 (Ende zum nächstmöglichen Termin) vorhanden"
    ),
    code!(
        "Z44",
        E_2000,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!("E11", E_2000, Ablehnung, "Ablehnung (Messproblem)"),
    code!(
        "Z12",
        E_2000,
        Ablehnung,
        "Ablehnung Vertragsbindung — der nächstmögliche Kündigungszeitpunkt gehört in SG4 DTM+157"
    ),
    code!(
        "Z29",
        E_2000,
        Ablehnung,
        "Ablehnung (kein Vertragsverhältnis mehr vorhanden)"
    ),
    code!("Z34", E_2000, Ablehnung, "Ablehnung (Mehrfachkündigung)"),
];

/// `E_2002` — Anmeldung Messstellenbetrieb prüfen (Gas). Prüfende Rolle: **NB**.
pub const EBD_ANMELDUNG_MSB_GAS: &str = "E_2002";
const E_2002: Option<&'static str> = Some(EBD_ANMELDUNG_MSB_GAS);

/// `E_2002` — the NB's answer to an Anmeldung des Messstellenbetriebs
/// (44042 → 44043 `G_0054` / 44044 `G_0053`).
///
/// The Bestätigung is *vorläufig*: the Zuordnung follows the Gesamtvorgang, at
/// **06:00 Uhr** on the reported day (AWH WiM Gas 2.0 Kap. 3.1.1) — the Gastag
/// boundary, where Strom assigns at 00:00.
pub const E_2002_CODES: &[AntwortCode] = &[
    code!("E15", E_2002, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_2002,
        Zustimmung,
        "Zustimmung mit Terminänderung — nur wenn SG4 STS+7++E02 (Einzug in eine Neuanlage) vorhanden"
    ),
    code!(
        "Z44",
        E_2002,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!("E11", E_2002, Ablehnung, "Ablehnung (Messproblem)"),
    code!(
        "E17",
        E_2002,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung"
    ),
    code!(
        "Z09",
        E_2002,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
    code!(
        "Z29",
        E_2002,
        Ablehnung,
        "Ablehnung (kein Vertragsverhältnis mehr vorhanden)"
    ),
    code!("ZB6", E_2002, Ablehnung, "Erforderliche Versicherung fehlt"),
];

/// `E_2005` — Abmeldung Messstellenbetrieb prüfen (Gas). Prüfende Rolle: **NB**.
pub const EBD_ABMELDUNG_MSB_GAS: &str = "E_2005";
const E_2005: Option<&'static str> = Some(EBD_ABMELDUNG_MSB_GAS);

/// `E_2005` — the NB's answer to an Ende Messstellenbetrieb
/// (44051 → 44052 `G_0058` / 44053 `G_0057`).
///
/// The 20-Werktage Mindestvorlauffrist is **not** an Ablehnungsgrund: AWH WiM
/// Gas 2.0 Kap. 3.6.2 Nr. 2 has the NB move the Zuordnungsende to the
/// nächstmögliches and confirm with `Z01`.
pub const E_2005_CODES: &[AntwortCode] = &[
    code!("E15", E_2005, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_2005,
        Zustimmung,
        "Zustimmung mit Terminänderung — das vom NB festgesetzte nächstmögliche Zuordnungsende"
    ),
    code!(
        "E17",
        E_2005,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung — nur bei Transaktionsgrund ZG9/ZH1/ZH2 (Aufhebung einer zukünftigen Zuordnung)"
    ),
    code!(
        "Z09",
        E_2005,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
];

/// `E_2006` — Verpflichtungsanfrage prüfen (Gas). Prüfende Rolle: **gMSB**.
pub const EBD_VERPFLICHTUNGSANFRAGE_GAS: &str = "E_2006";
const E_2006: Option<&'static str> = Some(EBD_VERPFLICHTUNGSANFRAGE_GAS);

/// `E_2006` — the grundzuständiger MSB's answer to the NB's Verpflichtungsanfrage.
///
/// **The Gas Ablehnung has no Prüfidentifikator of its own.** PID-Übersicht 4.0
/// publishes 44168 (Anfrage) and 44169 (Bestätigung) and nothing else — the
/// 44170 of PID 3.3 was withdrawn. `G_0071` is still published, so an Ablehnung
/// under FV2026-10-01 has a code and no carrier; mako escalates rather than
/// inventing one.
pub const E_2006_CODES: &[AntwortCode] = &[
    code!("E15", E_2006, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z01",
        E_2006,
        Zustimmung,
        "Zustimmung mit Terminänderung — nur wenn SG4 STS+7++E02 (Einzug in eine Neuanlage) vorhanden"
    ),
    code!(
        "Z44",
        E_2006,
        Zustimmung,
        "Zustimmung mit Korrektur von nicht bilanzierungsrelevanten Daten"
    ),
    code!(
        "E17",
        E_2006,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung"
    ),
    code!("Z07", E_2006, Ablehnung, "Ablehnung (Keine Berechtigung)"),
    code!(
        "Z09",
        E_2006,
        Ablehnung,
        "Ablehnung (Transaktionsgrund unplausibel)"
    ),
    code!("ZB6", E_2006, Ablehnung, "Erforderliche Versicherung fehlt"),
];

/// `E_2004` — Weiterverpflichtung prüfen (Gas). Prüfende Rolle: **MSBA** (ORDRSP).
pub const EBD_WEITERVERPFLICHTUNG_GAS: &str = "E_2004";
const E_2004: Option<&'static str> = Some(EBD_WEITERVERPFLICHTUNG_GAS);

/// `E_2004` — the outgoing MSB's answer to the NB's Weiterverpflichtung
/// (17002 → 19003 `G_0072` / 19004 `G_0073`).
pub const E_2004_CODES: &[AntwortCode] = &[
    code!("Z13", E_2004, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!(
        "Z14",
        E_2004,
        Zustimmung,
        "Zustimmung mit Terminänderung — der Termin lag außerhalb des maximalen Weiterverpflichtungszeitraums; der korrigierte Abmeldetermin gehört in DTM DE 2380"
    ),
    code!(
        "Z22",
        E_2004,
        Ablehnung,
        "Ablehnung wegen Überschreiten des Weiterverpflichtungszeitraums"
    ),
];

/// `E_2007`/`E_2008` — Anzeige Gerätewechselabsicht prüfen (Gas). Rolle: **MSBA**.
///
/// The BDEW splits the decision in two („Anzeige prüfen" and „Prüfen, ob
/// Eigenausbau gewünscht") but publishes one pair of Codelisten for both, so
/// mako carries one tree. `E_2007` is the id the PID-Übersicht names on the
/// Ablehnung 19016 and `E_2008` the one it names on 19015 — the same tree.
pub const EBD_GERAETEWECHSELABSICHT_GAS: &str = "E_2007";
const E_2007: Option<&'static str> = Some(EBD_GERAETEWECHSELABSICHT_GAS);

/// `E_2007` — the Gas twin of [`E_0204_CODES`] (`G_0059` / `G_0060`).
///
/// As in Strom, **neither cluster refuses the Gerätewechsel**: `ZB4` says the
/// MSBA removes its own devices and `ZB5` that the MSBN does.
pub const E_2007_CODES: &[AntwortCode] = &[
    code!("ZB4", E_2007, Zustimmung, "Eigenausbau wird erfolgen"),
    code!("ZB5", E_2007, Ablehnung, "Kein Eigenausbau des MSBA"),
    code!(
        "E17",
        E_2007,
        Ablehnung,
        "Ablehnung wg. Fristüberschreitung — die Antwort ist spätestens am 2. WT vor dem Gerätewechseltermin fällig"
    ),
    code!("Z07", E_2007, Ablehnung, "Ablehnung (Keine Berechtigung)"),
];

/// `E_2011` — Bestellung (Geräteübernahme) prüfen (Gas). Prüfende Rolle: **MSBA**.
pub const EBD_BESTELLUNG_GERAETEUEBERNAHME_GAS: &str = "E_2011";
const E_2011: Option<&'static str> = Some(EBD_BESTELLUNG_GERAETEUEBERNAHME_GAS);

/// `E_2011` — the Gas twin of [`E_0247_CODES`] (19001 `G_0061` / 19002 `G_0074`).
pub const E_2011_CODES: &[AntwortCode] = &[
    code!("Z13", E_2011, Zustimmung, "Zustimmung ohne Korrekturen"),
    code!("5", E_2011, Ablehnung, "Preis / Rechenregel falsch"),
    code!(
        "Z32",
        E_2011,
        Ablehnung,
        "Ablehnung Bestellumfang übersteigt Angebotsumfang"
    ),
];

// ── WiM Gas Abrechnung (EBD 4.3 Kap. 14.7) ───────────────────────────────────
//
// Five trees for one INVOIC family, separated by **who rejects whose invoice**
// rather than by which invoice it is. Their codes travel in REMADV `AJT`
// DE 4465 with the Gas Codeliste in DE 1082 — never the EBD id, so
// `wire_codeliste` carries the `G_00xx` name beside each.
//
// Like `E_0406`, all five publish Ablehnungscodes only: the Gas Zahlungsavis
// 33001 carries no `AJT`. `E_2017` („Nichtzahlungsavis prüfen") has no tree,
// „da keine Antwort gegeben wird", so it is absent here.

/// `E_2014` — Rechnung verarbeiten, NB → MSBA (Codeliste `G_0083`).
pub const EBD_WIM_RECHNUNG_NB_GAS: &str = "E_2014";
const E_2014: Option<&'static str> = Some(EBD_WIM_RECHNUNG_NB_GAS);

/// `E_2014` — Ablehnungsgründe für eine WiM-Rechnung Gas (NB → MSBA).
pub const E_2014_CODES: &[AntwortCode] = &[
    code!("5", E_2014, Ablehnung, "Preis / Rechenregel falsch"),
    code!(
        "9",
        E_2014,
        Ablehnung,
        "Falscher Abrechnungszeitraum (innerhalb gültiger Vertragsgrenzen)"
    ),
    code!(
        "14",
        E_2014,
        Ablehnung,
        "Unbekannte Marktlokation, Messlokation"
    ),
    code!("53", E_2014, Ablehnung, "Doppelte Rechnung"),
    code!(
        "Z01",
        E_2014,
        Ablehnung,
        "Abrechnungsbeginn ungleich Vertragsbeginn"
    ),
    code!(
        "Z02",
        E_2014,
        Ablehnung,
        "Abrechnungsende ungleich Vertragsende"
    ),
    code!("Z06", E_2014, Ablehnung, "Artikel nicht vereinbart"),
    code!(
        "Z08",
        E_2014,
        Ablehnung,
        "Rechnungsnummer bereits erhalten — zwei unterschiedliche Rechnungen tragen dieselbe \
         Rechnungsnummer"
    ),
    code!(
        "Z40",
        E_2014,
        Ablehnung,
        "Reverse Charge Anwendung fehlt oder unzulässig"
    ),
    code!(
        "Z43",
        E_2014,
        Ablehnung,
        "Ungültiges Rechnungsdatum — DTM+137 liegt bei Eingang in der Zukunft"
    ),
];

/// `E_2015` — Rechnung verarbeiten, MSBN → MSBA (Codeliste `G_0084`).
pub const EBD_WIM_RECHNUNG_MSBN_GAS: &str = "E_2015";
const E_2015: Option<&'static str> = Some(EBD_WIM_RECHNUNG_MSBN_GAS);

/// `E_2015` — Ablehnungsgründe für eine WiM-Rechnung Gas (MSBN → MSBA).
pub const E_2015_CODES: &[AntwortCode] = &[
    code!("5", E_2015, Ablehnung, "Preis / Rechenregel falsch"),
    code!(
        "9",
        E_2015,
        Ablehnung,
        "Falscher Abrechnungszeitraum (innerhalb gültiger Vertragsgrenzen)"
    ),
    code!(
        "14",
        E_2015,
        Ablehnung,
        "Unbekannte Marktlokation, Messlokation"
    ),
    code!("53", E_2015, Ablehnung, "Doppelte Rechnung"),
    code!(
        "Z01",
        E_2015,
        Ablehnung,
        "Abrechnungsbeginn ungleich Vertragsbeginn"
    ),
    code!(
        "Z02",
        E_2015,
        Ablehnung,
        "Abrechnungsende ungleich Vertragsende"
    ),
    code!("Z06", E_2015, Ablehnung, "Artikel nicht vereinbart"),
    code!(
        "Z08",
        E_2015,
        Ablehnung,
        "Rechnungsnummer bereits erhalten — zwei unterschiedliche Rechnungen tragen dieselbe \
         Rechnungsnummer"
    ),
    code!(
        "Z40",
        E_2015,
        Ablehnung,
        "Reverse Charge Anwendung fehlt oder unzulässig"
    ),
    code!(
        "Z43",
        E_2015,
        Ablehnung,
        "Ungültiges Rechnungsdatum — DTM+137 liegt bei Eingang in der Zukunft"
    ),
];

/// `E_2016` — Rechnung verarbeiten, NB → MSBA, Messlokations-Abrechnung
/// (Codeliste `G_0083`).
///
/// Publishes the same alphabet as [`EBD_WIM_RECHNUNG_NB_GAS`] with one word
/// changed: `14` names „Unbekannte Messlokation" alone, because this
/// Abrechnung has no Marktlokation to be unknown.
pub const EBD_WIM_RECHNUNG_MELO_GAS: &str = "E_2016";
const E_2016: Option<&'static str> = Some(EBD_WIM_RECHNUNG_MELO_GAS);

/// `E_2016` — Ablehnungsgründe für eine WiM-Rechnung Gas (NB → MSBA).
pub const E_2016_CODES: &[AntwortCode] = &[
    code!("5", E_2016, Ablehnung, "Preis / Rechenregel falsch"),
    code!(
        "9",
        E_2016,
        Ablehnung,
        "Falscher Abrechnungszeitraum (innerhalb gültiger Vertragsgrenzen)"
    ),
    code!("14", E_2016, Ablehnung, "Unbekannte Messlokation"),
    code!("53", E_2016, Ablehnung, "Doppelte Rechnung"),
    code!(
        "Z01",
        E_2016,
        Ablehnung,
        "Abrechnungsbeginn ungleich Vertragsbeginn"
    ),
    code!(
        "Z02",
        E_2016,
        Ablehnung,
        "Abrechnungsende ungleich Vertragsende"
    ),
    code!("Z06", E_2016, Ablehnung, "Artikel nicht vereinbart"),
    code!(
        "Z08",
        E_2016,
        Ablehnung,
        "Rechnungsnummer bereits erhalten — zwei unterschiedliche Rechnungen tragen dieselbe \
         Rechnungsnummer"
    ),
    code!(
        "Z40",
        E_2016,
        Ablehnung,
        "Reverse Charge Anwendung fehlt oder unzulässig"
    ),
    code!(
        "Z43",
        E_2016,
        Ablehnung,
        "Ungültiges Rechnungsdatum — DTM+137 liegt bei Eingang in der Zukunft"
    ),
];

/// `E_2018` — Storno verarbeiten (Codeliste `G_0085`).
pub const EBD_WIM_STORNO_GAS: &str = "E_2018";
const E_2018: Option<&'static str> = Some(EBD_WIM_STORNO_GAS);

/// `E_2018` — Ablehnungsgründe für eine Stornorechnung Gas (G_0085).
pub const E_2018_CODES: &[AntwortCode] = &[
    code!(
        "28",
        E_2018,
        Ablehnung,
        "Sonstiges — etwa: Originalrechnungsnummer nicht gefunden",
        bemerkung
    ),
    code!("Z08", E_2018, Ablehnung, "Rechnungsnummer bereits erhalten"),
    code!(
        "Z43",
        E_2018,
        Ablehnung,
        "Ungültiges Rechnungsdatum — DTM+137 liegt bei Eingang in der Zukunft"
    ),
];

/// `E_2019` — Storno verarbeiten (Codeliste `G_0086`).
pub const EBD_WIM_STORNO_MSBN_GAS: &str = "E_2019";
const E_2019: Option<&'static str> = Some(EBD_WIM_STORNO_MSBN_GAS);

/// `E_2019` — Ablehnungsgründe für eine Stornorechnung Gas (G_0086).
pub const E_2019_CODES: &[AntwortCode] = &[
    code!(
        "28",
        E_2019,
        Ablehnung,
        "Sonstiges — etwa: Originalrechnungsnummer nicht gefunden",
        bemerkung
    ),
    code!("Z08", E_2019, Ablehnung, "Rechnungsnummer bereits erhalten"),
    code!(
        "Z43",
        E_2019,
        Ablehnung,
        "Ungültiges Rechnungsdatum — DTM+137 liegt bei Eingang in der Zukunft"
    ),
];

// ── ESA Wertebestellung (WiM Strom Teil 2 Kap. 4) ────────────────────────────
//
// The three trees whose prüfende Rolle is the **MSB serving an ESA**. All
// three answer with an ORDRSP, so the code rides `SG2 AJT` DE 4465 and the
// tree id DE 1082 — not `STS+E01`. ORDRSP AHB 1.1b §4.15 conditions [17]/[18]
// say the code „muss im EBD dem Cluster Zustimmung / Ablehnung zugeordnet
// sein", which is precisely what [`Cluster`] decides here.
//
// Two neighbouring decisions deliberately have **no** tree, and the EBD
// document says so in as many words: `E_0253` („Angebot zur Anfrage prüfen")
// and `E_0258` („Antwort auf Bestellung prüfen") — „derzeit ist für diese
// Entscheidung kein Entscheidungsbaum notwendig, da keine Antwort gegeben
// wird". So the QUOTES 15003 Ablehnung carries a free-text Begründung and no
// Antwortcode at all.

/// `E_0256` — Bestellung prüfen (ORDERS 17007 → ORDRSP 19011/19012).
pub const EBD_ESA_BESTELLUNG: &str = "E_0256";
const E_0256: Option<&'static str> = Some(EBD_ESA_BESTELLUNG);

/// `E_0256` — the MSB's answer to an ESA Bestellung von Werten.
///
/// Prüfschritte 1–11. `A11` is the sole Zustimmung and is reached from two
/// places: directly at Prüfschritt 10 for a Messlokation order, or at
/// Prüfschritt 11 for a Marktlokation/Tranche/Netzlokation order once the MSB
/// has confirmed it also operates every underlying Messlokation — the UC 4.1.1
/// Vorbedingung, checked here rather than assumed.
pub const E_0256_CODES: &[AntwortCode] = &[
    code!("A11", E_0256, Zustimmung, "Bestellung ist angenommen"),
    code!(
        "A01",
        E_0256,
        Ablehnung,
        "Die Bindungsfrist des Angebots ist abgelaufen"
    ),
    code!(
        "A04",
        E_0256,
        Ablehnung,
        "Der MSB sieht für das gewünschte Messprodukt keine Übermittlung als Abo vor"
    ),
    code!(
        "A05",
        E_0256,
        Ablehnung,
        "Der MSB sieht für das gewünschte Messprodukt keine einmalige Übermittlung vor"
    ),
    code!(
        "A06",
        E_0256,
        Ablehnung,
        "Die vertragliche Grundlage zwischen dem MSB und dem ESA ist nicht mehr gültig"
    ),
    code!(
        "A07",
        E_0256,
        Ablehnung,
        "Der MSB ist der Lokation für den im Angebot spezifizierten Zeitraum / Zeitpunkt der Messwertermittlung nicht zugeordnet"
    ),
    code!(
        "A08",
        E_0256,
        Ablehnung,
        "Der Anschlussnutzer hat gegenüber dem ESA seine Einwilligung widerrufen oder ihre Gültigkeit ist abgelaufen"
    ),
    code!(
        "A09",
        E_0256,
        Ablehnung,
        "Die Gerätetechnik misst die angeforderten Messwerte nicht"
    ),
    code!(
        "A10",
        E_0256,
        Ablehnung,
        "Der MSB der Marktlokation / Netzlokation ist nicht zeitgleich der allen Messlokationen zugeordnete MSB"
    ),
];

/// `E_0257` — Stornierung prüfen (ORDCHG 39002 → ORDRSP 19013/19014).
pub const EBD_ESA_STORNIERUNG: &str = "E_0257";
const E_0257: Option<&'static str> = Some(EBD_ESA_STORNIERUNG);

/// `E_0257` — the MSB's answer to an ESA Stornierung einer Bestellung.
///
/// The tree splits on the Abo mode (`IMD+7081`) and lands on different refusal
/// codes for the same fact: `A02` when a **subscription** has already started
/// delivering, `A03` when a **one-shot** has already been transmitted. Reading
/// „delivery has begun" as one condition would put the wrong code on the wire.
pub const E_0257_CODES: &[AntwortCode] = &[
    code!("A04", E_0257, Zustimmung, "Stornierung wird bestätigt"),
    code!(
        "A01",
        E_0257,
        Ablehnung,
        "Die Bestellung des ESA wurde durch den MSB nicht bestätigt"
    ),
    code!(
        "A02",
        E_0257,
        Ablehnung,
        "Mit der Übermittlung von Werten aus dem Abo wurde bereits begonnen"
    ),
    code!(
        "A03",
        E_0257,
        Ablehnung,
        "Die einmalige Übermittlung der Werte ist bereits erfolgt"
    ),
];

/// `E_0254` — Beendigung prüfen (ORDERS 17008 → ORDRSP 19011/19012).
pub const EBD_ESA_BEENDIGUNG: &str = "E_0254";
const E_0254: Option<&'static str> = Some(EBD_ESA_BEENDIGUNG);

/// `E_0254` — the MSB's answer to an ESA Abbestellung von Werten.
///
/// Prüfschritt 1 makes the UC 4.3 Vorbedingung executable: a one-shot order is
/// **stornierbar, nicht abbestellbar**, and `A01` says so. Prüfschritt 2 says
/// the same about a Beendigung dated before the Abo even starts (`A02` — „Die
/// Bestellung ist zu stornieren"), which is why the two termination paths are
/// not interchangeable.
pub const E_0254_CODES: &[AntwortCode] = &[
    code!("A05", E_0254, Zustimmung, "Beendigung wird bestätigt"),
    code!(
        "A01",
        E_0254,
        Ablehnung,
        "Es handelte sich bei der Bestellung um eine einmalige Übermittlung"
    ),
    code!("A02", E_0254, Ablehnung, "Die Bestellung ist zu stornieren"),
    code!(
        "A03",
        E_0254,
        Ablehnung,
        "Die Übermittlung wurde bereits zu einem früheren oder zu dem in der Beendigung genannten Zeitpunkt beendet"
    ),
    code!(
        "A04",
        E_0254,
        Ablehnung,
        "Es wurden bereits Daten nach dem gewünschten Beendigungsdatum übermittelt"
    ),
];

/// Every Codeliste this crate knows, keyed by its EBD id.
pub const CODELISTEN: &[(&str, &[AntwortCode])] = &[
    (EBD_ANMELDUNG_DIREKT_ABLEHNBAR, E_0622_CODES),
    (EBD_LIEFERBEGINN, E_0623_CODES),
    (EBD_ABMELDUNG_NB, E_0607_CODES),
    (EBD_NEUANLAGE, E_0608_CODES),
    (EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, E_3005_CODES),
    (EBD_LIEFERBEGINN_GAS, E_3007_CODES),
    (EBD_ABMELDUNG_GAS_NB, E_3019_CODES),
    (EBD_ABMELDUNG, E_0609_CODES),
    (EBD_BEENDIGUNG_ZUORDNUNG, E_0624_CODES),
    (EBD_KUENDIGUNG, E_0614_CODES),
    (EBD_KUENDIGUNG_GAS, E_3001_CODES),
    (EBD_ABMELDUNG_GAS, E_3002_CODES),
    (EBD_ABMELDUNGSANFRAGE_GAS, E_3020_CODES),
    (EBD_ANMELDUNG_EOG, E_0615_CODES),
    (EBD_ANMELDUNG_EOG_GAS, E_3008_CODES),
    ("E_0603", E_0603_CODES),
    ("E_0604", E_0604_CODES),
    ("E_0605", E_0605_CODES),
    ("E_0606", E_0606_CODES),
    (EBD_BESTELLUNG, E_0595_CODES),
    (EBD_NETZNUTZUNGSRECHNUNG, E_0406_CODES),
    // WiM Strom — Messstellenbetrieb.
    (EBD_KUENDIGUNG_MSB, E_0200_CODES),
    (EBD_ANMELDUNG_MSB, E_0201_CODES),
    (EBD_ABMELDUNG_MSB, E_0202_CODES),
    (EBD_VERPFLICHTUNGSANFRAGE, E_0240_CODES),
    (EBD_WEITERVERPFLICHTUNG, E_0203_CODES),
    (EBD_GERAETEWECHSELABSICHT, E_0204_CODES),
    (EBD_BESTELLUNG_GERAETEUEBERNAHME, E_0247_CODES),
    (EBD_MESSLOKATIONSAENDERUNG_NB, E_0249_CODES),
    (EBD_MESSLOKATIONSAENDERUNG_LF, E_0250_CODES),
    (EBD_GESAMTVORGANG, E_0232_CODES),
    // WiM Gas — Messstellenbetrieb (EBD 4.3 Kap. 14).
    (EBD_KUENDIGUNG_MSB_GAS, E_2000_CODES),
    (EBD_ANMELDUNG_MSB_GAS, E_2002_CODES),
    (EBD_GESAMTVORGANG_GAS, E_2003_CODES),
    (EBD_WEITERVERPFLICHTUNG_GAS, E_2004_CODES),
    (EBD_ABMELDUNG_MSB_GAS, E_2005_CODES),
    (EBD_VERPFLICHTUNGSANFRAGE_GAS, E_2006_CODES),
    (EBD_GERAETEWECHSELABSICHT_GAS, E_2007_CODES),
    (EBD_BESTELLUNG_GERAETEUEBERNAHME_GAS, E_2011_CODES),
    // WiM Gas — Abrechnung (EBD 4.3 Kap. 14.7).
    (EBD_WIM_RECHNUNG_NB_GAS, E_2014_CODES),
    (EBD_WIM_RECHNUNG_MSBN_GAS, E_2015_CODES),
    (EBD_WIM_RECHNUNG_MELO_GAS, E_2016_CODES),
    (EBD_WIM_STORNO_GAS, E_2018_CODES),
    (EBD_WIM_STORNO_MSBN_GAS, E_2019_CODES),
    // WiM Strom Teil 2 — ESA Wertebestellung.
    (EBD_ESA_BESTELLUNG, E_0256_CODES),
    (EBD_ESA_STORNIERUNG, E_0257_CODES),
    (EBD_ESA_BEENDIGUNG, E_0254_CODES),
];

/// The trees that publish **Ablehnungscodes only**, each paired with the tree
/// its Zustimmung comes from.
///
/// A „Prüfen, ob Anmeldung direkt ablehnbar" tree is a pre-check: it can refuse
/// a message but never agree to one. Surviving it means continuing into the
/// tree named here, which is where the Bestätigungscode is drawn from. Listing
/// the pairs makes the asymmetry a stated fact rather than a gap the
/// both-clusters invariant has to be silent about — and it is what a caller
/// needs to know to answer at all.
pub const VORPRUEFUNG_TREES: &[(&str, &str)] = &[
    (EBD_ANMELDUNG_DIREKT_ABLEHNBAR, EBD_LIEFERBEGINN),
    (EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, EBD_LIEFERBEGINN_GAS),
];

/// Trees that publish **Ablehnungscodes only**, because their positive answer
/// carries no code at all — paired with the reason.
///
/// This is a different shape from [`VORPRUEFUNG_TREES`]: there the Zustimmung
/// exists and lives in another tree; here the confirming Prüfidentifikator
/// simply has no Status-der-Antwort segment for a code to sit in, so the PID
/// alone is the agreement.
///
/// Adding a tree here is a claim about an AHB column. The test below holds it
/// to the consequence: such a tree must publish no Zustimmungscode.
pub const ABLEHNUNG_ONLY_TREES: &[(&str, &str)] = &[
    (
        EBD_NETZNUTZUNGSRECHNUNG,
        "REMADV: the Bestätigung 33001 carries no AJT at all (REMADV AHB 1.0a)",
    ),
    (
        EBD_GESAMTVORGANG,
        "IFTSTA: 21012 carries no STS+E01; only the Scheitermeldung 21011 names \
         E_0232 (PID-Übersicht 4.0 rows 30140/30150)",
    ),
    (
        EBD_GESAMTVORGANG_GAS,
        "IFTSTA: 21012 carries no STS+E01; only the Scheitermeldung 21011 names \
         E_2003 (PID-Übersicht 4.0 rows 39150/39160)",
    ),
    (
        EBD_WIM_RECHNUNG_NB_GAS,
        "REMADV: the Gas Zahlungsavis 33001 carries no AJT at all",
    ),
    (
        EBD_WIM_RECHNUNG_MSBN_GAS,
        "REMADV: the Gas Zahlungsavis 33001 carries no AJT at all",
    ),
    (
        EBD_WIM_RECHNUNG_MELO_GAS,
        "REMADV: the Gas Zahlungsavis 33001 carries no AJT at all",
    ),
    (
        EBD_WIM_STORNO_GAS,
        "REMADV: the Gas Zahlungsavis 33001 carries no AJT at all",
    ),
    (
        EBD_WIM_STORNO_MSBN_GAS,
        "REMADV: the Gas Zahlungsavis 33001 carries no AJT at all",
    ),
];

/// `true` when `ebd` publishes Ablehnungscodes only — see [`ABLEHNUNG_ONLY_TREES`].
#[must_use]
pub fn ist_ablehnung_only(ebd: &str) -> bool {
    ABLEHNUNG_ONLY_TREES.iter().any(|(id, _)| *id == ebd)
}

/// The tree a Vorprüfung hands a surviving message to, if `ebd` is one.
#[must_use]
pub fn zustimmung_tree_of(ebd: &str) -> Option<&'static str> {
    VORPRUEFUNG_TREES
        .iter()
        .find(|(vor, _)| *vor == ebd)
        .map(|(_, ok)| *ok)
}

/// Look a code up inside the EBD that publishes it.
///
/// Returns `None` when the code is not in *that* tree — which is the check that
/// keeps an `E_0624` code off an `E_0609` answer.
#[must_use]
pub fn lookup(ebd: &str, code: &str) -> Option<&'static AntwortCode> {
    CODELISTEN
        .iter()
        .find(|(id, _)| *id == ebd)
        .and_then(|(_, codes)| codes.iter().find(|c| c.code == code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every Codeliste must offer **both sides of whichever axis it uses** — a
    /// tree with one cluster cannot answer its process.
    ///
    /// Two kinds of tree are exempt, and both say so out loud: `E_0406` answers
    /// a REMADV Abweisung, whose Bestätigung (33001) carries no `AJT` at all;
    /// and a Vorprüfung named in [`VORPRUEFUNG_TREES`] hands a surviving message
    /// to the tree that holds its Zustimmung.
    ///
    /// A tree may not mix axes: `E_0595` is „Änderung der Daten" throughout and
    /// every other tree is Zustimmung/Ablehnung. One code from the wrong pair
    /// would make „which PID does this ride" unanswerable.
    #[test]
    fn every_codeliste_has_both_sides_of_its_axis() {
        for (ebd, codes) in CODELISTEN {
            let auf_zustimmungsachse = codes.iter().any(|c| c.ist_zustimmung().is_some());
            let auf_datenachse = codes
                .iter()
                .any(|c| c.sendet_stammdatenaenderung().is_some());
            assert!(
                auf_zustimmungsachse != auf_datenachse,
                "{ebd} mixes the Zustimmung/Ablehnung axis with the Datenänderung one"
            );

            if auf_datenachse {
                for want in [true, false] {
                    assert!(
                        codes
                            .iter()
                            .any(|c| c.sendet_stammdatenaenderung() == Some(want)),
                        "{ebd} has no '{}Änderung der Daten' code",
                        if want { "" } else { "keine " }
                    );
                }
                continue;
            }

            let ablehnung_only = ist_ablehnung_only(ebd) || zustimmung_tree_of(ebd).is_some();
            if ablehnung_only {
                assert!(
                    codes.iter().all(|c| c.ist_zustimmung() != Some(true)),
                    "{ebd} is declared Ablehnung-only but publishes a Zustimmungscode"
                );
                continue;
            }
            assert!(
                codes.iter().any(|c| c.ist_zustimmung() == Some(true)),
                "{ebd} has no Zustimmungscode"
            );
            assert!(
                codes.iter().any(|c| c.ist_zustimmung() == Some(false)),
                "{ebd} has no Ablehnungscode"
            );
        }
    }

    /// `E_0595`'s cluster is not agreement. `A06` says „Änderungen werden nicht
    /// vorgenommen" and still sits in „Änderung der Daten", because the
    /// Verantwortliche sends its own data back — reading that as a Zustimmung,
    /// or `A03` as an Ablehnung, inverts the meaning.
    #[test]
    fn the_bestellung_tree_is_not_on_the_agreement_axis() {
        for (code, sendet) in [
            ("A01", false),
            ("A02", true),
            ("A03", false),
            ("A04", true),
            ("A05", true),
            ("A06", true),
            ("A20", true),
            ("A21", false),
            ("A99", false),
        ] {
            let e = lookup(EBD_BESTELLUNG, code).unwrap_or_else(|| panic!("{code}"));
            assert_eq!(e.sendet_stammdatenaenderung(), Some(sendet), "{code}");
            assert_eq!(
                e.ist_zustimmung(),
                None,
                "{code} must not answer the agreement question"
            );
        }
    }

    /// The Clearing branch is a subset of the tree, and every member resolves.
    #[test]
    fn the_clearing_branch_is_part_of_its_tree() {
        for code in E_0595_CLEARING_CODES {
            assert!(lookup(EBD_BESTELLUNG, code).is_some(), "{code}");
        }
        // The ORDERS branch answers PID 55555, not the Abrechnungsdaten pair.
        assert!(!E_0595_CLEARING_CODES.contains(&"A20"));
    }

    /// The tree a Vorprüfung defers to must itself be a Codeliste this crate
    /// knows, and must carry a Zustimmung — otherwise a message that passes
    /// every check still has no code to be confirmed with.
    #[test]
    fn a_vorpruefung_defers_to_a_tree_that_can_agree() {
        for (vor, ok) in VORPRUEFUNG_TREES {
            let codes = CODELISTEN
                .iter()
                .find(|(id, _)| id == ok)
                .unwrap_or_else(|| panic!("{vor} defers to unknown tree {ok}"))
                .1;
            assert!(
                codes.iter().any(|c| c.ist_zustimmung() == Some(true)),
                "{vor} defers to {ok}, which has no Zustimmungscode"
            );
        }
    }

    /// `A06` is „andere Anmeldung in Bearbeitung" for a verbrauchende
    /// Marktlokation. The erzeugende branch of the same tree answers the same
    /// condition with `A45`, and Gas answers it with `ZC5` — three codes, one
    /// question, and no tree publishes another's.
    #[test]
    fn the_same_condition_has_a_different_code_per_branch_and_sparte() {
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR, "A06").is_some());
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR, "A45").is_some());
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, "ZC5").is_some());
        // The Strom codes are not Gas codes.
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, "A06").is_none());
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, "A07").is_none());
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, "A05").is_none());
        // …and the Gas codes are not Strom codes.
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR, "E17").is_none());
        assert!(lookup(EBD_ANMELDUNG_DIREKT_ABLEHNBAR, "ZC5").is_none());
    }

    /// The Gas Abmeldung tree has no `A02` / `A09` / `A10`: the Strom Abmeldung
    /// codes are undefined on a 44006.
    #[test]
    fn the_gas_abmeldung_tree_has_no_strom_codes() {
        for code in ["A02", "A09", "A10"] {
            assert!(lookup(EBD_ABMELDUNG_NB, code).is_some(), "Strom {code}");
            assert!(lookup(EBD_ABMELDUNG_GAS_NB, code).is_none(), "Gas {code}");
        }
        assert!(lookup(EBD_ABMELDUNG_GAS_NB, "E17").is_some());
        assert!(lookup(EBD_ABMELDUNG_GAS_NB, "Z08").is_some());
    }

    /// A code is looked up *within* its tree: `A32` is an `E_0624` code and
    /// nothing else. `E_0609` governs `55007` and does not define it.
    #[test]
    fn a32_belongs_to_e0624_only() {
        assert!(lookup(EBD_BEENDIGUNG_ZUORDNUNG, "A32").is_some());
        assert!(lookup(EBD_ABMELDUNG, "A32").is_none());
    }

    /// `A35` „Vertragsbindung" likewise: `E_0624`, not `E_0609`.
    #[test]
    fn a35_belongs_to_e0624_only() {
        assert!(lookup(EBD_BEENDIGUNG_ZUORDNUNG, "A35").is_some());
        assert!(lookup(EBD_ABMELDUNG, "A35").is_none());
    }

    /// The Zustimmung to a `55007` is `A10` (or `A29` for a Tranche) — the code
    /// that decides which PID the answer rides on.
    #[test]
    fn the_e0609_zustimmung_is_a10() {
        let c = lookup(EBD_ABMELDUNG, "A10").expect("A10 is published");
        assert_eq!(c.ist_zustimmung(), Some(true));
        assert_eq!(
            lookup(EBD_ABMELDUNG, "A29").expect("A29").ist_zustimmung(),
            Some(true)
        );
    }

    /// No Codeliste may declare the same code twice — a duplicate would make
    /// the cluster, and therefore the answer PID, ambiguous.
    #[test]
    fn codes_are_unique_within_a_codeliste() {
        for (ebd, codes) in CODELISTEN {
            for (i, c) in codes.iter().enumerate() {
                assert!(
                    !codes[..i].iter().any(|prev| prev.code == c.code),
                    "{ebd} declares {} twice",
                    c.code
                );
            }
        }
    }

    /// The catch-all codes require a written Erläuterung; forgetting it is how
    /// an Ablehnung arrives with no stated reason.
    #[test]
    fn the_catch_all_codes_require_a_bemerkung() {
        assert!(lookup(EBD_ABMELDUNG, "A99").expect("A99").braucht_bemerkung);
        assert!(
            lookup(EBD_KUENDIGUNG_GAS, "E14")
                .expect("E14")
                .braucht_bemerkung
        );
    }
}
