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
/// The BDEW calls these „Cluster: Zustimmung" and „Cluster: Ablehnung" and
/// prints them beside every code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Cluster {
    /// The answer agrees — carried by the Bestätigungs-PID.
    Zustimmung,
    /// The answer refuses — carried by the Ablehnungs-PID.
    Ablehnung,
}

/// One published Antwortcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntwortCode {
    /// DE 9013 — the code itself (`A10`, `A35`, `E15`, `Z12`, …).
    pub code: &'static str,
    /// DE 1131 — the EBD that publishes it, or `None` for the Gas Codelisten,
    /// which the MIG does not require to be named in DE 1131.
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
    /// `true` when this code agrees with the request.
    #[must_use]
    pub const fn ist_zustimmung(&self) -> bool {
        matches!(self.cluster, Cluster::Zustimmung)
    }
}

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

// ── Lookup ───────────────────────────────────────────────────────────────────

/// Every Codeliste this crate knows, keyed by its EBD id.
pub const CODELISTEN: &[(&str, &[AntwortCode])] = &[
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
    (EBD_NETZNUTZUNGSRECHNUNG, E_0406_CODES),
];

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

    /// Every UTILMD-answered Codeliste must offer both a way to agree and a way
    /// to refuse; a tree with only one cluster cannot answer its process.
    ///
    /// `E_0406` is exempt: a REMADV Bestätigung (33001) carries no `AJT` at
    /// all, so the tree publishes Ablehnungscodes only.
    #[test]
    fn every_codeliste_has_both_clusters() {
        for (ebd, codes) in CODELISTEN {
            if *ebd == EBD_NETZNUTZUNGSRECHNUNG {
                assert!(
                    codes.iter().all(|c| !c.ist_zustimmung()),
                    "{ebd} answers a REMADV Abweisung and has no Zustimmungscode"
                );
                continue;
            }
            assert!(
                codes.iter().any(AntwortCode::ist_zustimmung),
                "{ebd} has no Zustimmungscode"
            );
            assert!(
                codes.iter().any(|c| !c.ist_zustimmung()),
                "{ebd} has no Ablehnungscode"
            );
        }
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
        assert!(c.ist_zustimmung());
        assert!(lookup(EBD_ABMELDUNG, "A29").expect("A29").ist_zustimmung());
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
