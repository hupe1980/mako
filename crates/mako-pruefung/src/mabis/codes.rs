//! The MaBiS and Redispatch Antwortcode catalogue.
//!
//! MaBiS answers ride different messages from GPKE answers (UTILMD
//! Prüfmitteilung, MSCONS, IFTSTA) and their clusters are a superset of the
//! Zustimmung / Ablehnung pair — see [`Abweisung`],
//! [`AblehnungDerGesamtenListe`] and [`KorrekturlisteWegenAblehnung`].
//!
//! [`Abweisung`]: crate::codes::Cluster::Abweisung
//! [`AblehnungDerGesamtenListe`]: crate::codes::Cluster::AblehnungDerGesamtenListe
//! [`KorrekturlisteWegenAblehnung`]: crate::codes::Cluster::KorrekturlisteWegenAblehnung
//!
//! Only the trees whose **prüfende Rolle** is one mako plays are catalogued —
//! NB, LF and BKV. The BIKO, ÜNB and Redispatch-Betreiber trees are absent:
//! shipping their codes would claim decisions this platform never makes.
//!
//! All of them are *receiving-side* checks. For Kategorie B the ÜNB is
//! aggregationsverantwortlich, so it sends the BG-SZR the **NB** checks and the
//! LF-SZR the **LF** checks, while the BK-SZR goes to the **BKV** — a role a
//! Lieferant ordinarily holds for its own Bilanzkreis.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* **4.3**
//! (01.04.2026) Kap. 7 (MaBiS) and Kap. 16 (Redispatch-Ausfallarbeit);
//! BNetzA BK6-24-174 Anlage 3.

use crate::codes::{AntwortCode, code};

// ═════════════════════════════════════════════════════════════════════════════
// Summenzeitreihen — „<Zeitreihe> prüfen"
// ═════════════════════════════════════════════════════════════════════════════
//
// Every one of these trees decides a single inbound Summenzeitreihe. The long
// form walks four Abweisungs-Prüfschritte (Frist, Zeitraum, Dublette, Version)
// before it ever looks at the energy figures; the short form has no Frist and
// no Version step because the series it decides carries neither.

/// `E_0007` — LF-SZR (Kategorie A) prüfen. Prüfende Rolle: **LF**.
pub const EBD_LF_SZR_A: &str = "E_0007";
/// `E_0041` — Lieferantensummenzeitreihe (Kategorie B) prüfen. Prüfende Rolle: **LF**.
pub const EBD_LF_SZR_B: &str = "E_0041";
/// `E_0093` — LF-AASZR prüfen (Ausfallarbeit). Prüfende Rolle: **LF**.
pub const EBD_LF_AASZR: &str = "E_0093";
/// `E_0040` — NZR (Netzzeitreihe) prüfen. Prüfende Rolle: **NB**.
pub const EBD_NZR: &str = "E_0040";
/// `E_0062` — BG-SZR (Kategorie B) prüfen. Prüfende Rolle: **NB**.
pub const EBD_BG_SZR_B: &str = "E_0062";
/// `E_0065` — DZÜ prüfen. Prüfende Rolle: **NB**.
pub const EBD_DZUE: &str = "E_0065";
/// `E_0063` — BK-SZR (Kategorie A) prüfen. Prüfende Rolle: **BKV**.
pub const EBD_BK_SZR_A: &str = "E_0063";
/// `E_0064` — BK-SZR (Kategorie B) prüfen. Prüfende Rolle: **BKV**.
pub const EBD_BK_SZR_B: &str = "E_0064";
/// `E_0098` — monatliche AAÜZ prüfen (Kap. 7.63). Prüfende Rolle: **BKV**.
///
/// `E_0098` and `E_0099` carry the identical title and the identical three
/// codes, and sit in two parallel chapter clusters (7.62–7.63 and 7.67–7.68)
/// that the document does not distinguish in the tree itself. Both are
/// catalogued so a code can be resolved against whichever the sender named;
/// [`crate::mabis`] deliberately maps neither onto a Zeitreihen-Familie,
/// because guessing which leg is which would put a code on the wrong one.
pub const EBD_AAUEZ_MONATLICH_A: &str = "E_0098";
/// `E_0099` — monatliche AAÜZ prüfen (Kap. 7.68). Prüfende Rolle: **BKV**.
/// See [`EBD_AAUEZ_MONATLICH_A`].
pub const EBD_AAUEZ_MONATLICH_B: &str = "E_0099";

/// Build the six-code „Zeitreihe prüfen" Codeliste for `ebd`.
///
/// `E_0007`, `E_0040`, `E_0041` and `E_0093` publish the identical code
/// sequence. They stay four separate catalogues rather than one shared slice
/// because [`AntwortCode::ebd`] is the identity a code is resolved against:
/// answering a NZR with a code drawn from the LF-SZR tree is an undefined code,
/// even though the two spell it the same way.
macro_rules! zeitreihe_codes {
    ($ebd:expr) => {
        &[
            code!("A01", Some($ebd), Abweisung, "Fristüberschreitung"),
            code!(
                "A02",
                Some($ebd),
                Abweisung,
                "Gewählter Zeitraum nicht zulässig"
            ),
            code!("A03", Some($ebd), Abweisung, "Zeitreihe bereits vorhanden"),
            code!("A04", Some($ebd), Abweisung, "Version nicht zugelassen"),
            code!(
                "A05",
                Some($ebd),
                Ablehnung,
                "Energiemenge falsch / nicht plausibel"
            ),
            code!("A06", Some($ebd), Zustimmung, "Zeitreihe akzeptiert"),
        ]
    };
}

/// `E_0007` — LF-SZR (Kategorie A) prüfen.
pub const E_0007_CODES: &[AntwortCode] = zeitreihe_codes!(EBD_LF_SZR_A);
/// `E_0041` — Lieferantensummenzeitreihe (Kategorie B) prüfen.
pub const E_0041_CODES: &[AntwortCode] = zeitreihe_codes!(EBD_LF_SZR_B);
/// `E_0093` — LF-AASZR prüfen.
pub const E_0093_CODES: &[AntwortCode] = zeitreihe_codes!(EBD_LF_AASZR);
/// `E_0040` — NZR prüfen.
pub const E_0040_CODES: &[AntwortCode] = zeitreihe_codes!(EBD_NZR);

/// Build the three-code short form shared by `E_0062`, `E_0063`, `E_0064`,
/// `E_0098` and `E_0099`.
///
/// Separate catalogues, not one shared slice: `A02` is „Energiemenge falsch"
/// here and „Gewählter Zeitraum nicht zulässig" in the long form, and
/// [`AntwortCode::ebd`] is what keeps the two apart.
macro_rules! kurzform_codes {
    ($ebd:expr) => {
        &[
            code!("A01", Some($ebd), Abweisung, "Zeitreihe bereits vorhanden"),
            code!(
                "A02",
                Some($ebd),
                Ablehnung,
                "Energiemenge falsch / nicht plausibel"
            ),
            code!("A03", Some($ebd), Zustimmung, "Zeitreihe akzeptiert"),
        ]
    };
}

/// `E_0062` — BG-SZR (Kategorie B) prüfen.
///
/// Three codes, not six: the BG-SZR carries no Versionsangabe and the tree
/// states no Frist, so the Abweisung reduces to the duplicate check.
pub const E_0062_CODES: &[AntwortCode] = kurzform_codes!(EBD_BG_SZR_B);

/// `E_0063` — BK-SZR (Kategorie A) prüfen.
pub const E_0063_CODES: &[AntwortCode] = kurzform_codes!(EBD_BK_SZR_A);
/// `E_0064` — BK-SZR (Kategorie B) prüfen.
pub const E_0064_CODES: &[AntwortCode] = kurzform_codes!(EBD_BK_SZR_B);
/// `E_0098` — monatliche AAÜZ prüfen.
pub const E_0098_CODES: &[AntwortCode] = kurzform_codes!(EBD_AAUEZ_MONATLICH_A);
/// `E_0099` — monatliche AAÜZ prüfen.
pub const E_0099_CODES: &[AntwortCode] = kurzform_codes!(EBD_AAUEZ_MONATLICH_B);

/// `E_0065` — DZÜ prüfen.
///
/// The DZÜ (Datenzuordnung Übertragungsnetz) is only assessable once its
/// **DZÜ-Liste** is held, which is why this tree carries an Ablehnung the
/// Summenzeitreihen-Trees do not have (`A02`).
pub const E_0065_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_DZUE),
        Abweisung,
        "Zeitreihe bereits vorhanden"
    ),
    code!(
        "A02",
        Some(EBD_DZUE),
        Ablehnung,
        "DZÜ-Liste nicht vorhanden"
    ),
    code!(
        "A03",
        Some(EBD_DZUE),
        Ablehnung,
        "Energiemenge falsch / nicht plausibel"
    ),
    code!("A04", Some(EBD_DZUE), Zustimmung, "Zeitreihe akzeptiert"),
];

// ═════════════════════════════════════════════════════════════════════════════
// Listenabgleich — „Marktlokationen mit <Liste> abgleichen"
// ═════════════════════════════════════════════════════════════════════════════
//
// A list answer is itself a list. The two clusters are not two severities of
// the same thing: `AblehnungDerGesamtenListe` carries no positions and demands
// a whole new list, `KorrekturlisteWegenAblehnung` carries one entry per
// disputed Marktlokation. Which one applies is decided before any position is
// looked at, so the Prüfschritte are ordered whole-list first.

/// `E_0004` — Marktlokationen mit LF-CL abgleichen (Erstabonnierung). Rolle: **LF**.
pub const EBD_LF_CL_ERSTABO: &str = "E_0004";
/// `E_0049` — Marktlokationen mit LF-CL abgleichen (Folgeabonnierung). Rolle: **LF**.
pub const EBD_LF_CL_FOLGEABO: &str = "E_0049";
/// `E_0014` — Marktlokationen mit LF-CL abgleichen (Einzelanforderung). Rolle: **LF**.
pub const EBD_LF_CL_EINZEL: &str = "E_0014";
/// `E_0047` — Marktlokationen mit LF-CL abgleichen (Clearing). Rolle: **LF**.
pub const EBD_LF_CL_CLEARING: &str = "E_0047";
/// `E_0052` — Marktlokationen mit BG-CL abgleichen (Abonnierung). Rolle: **NB**.
pub const EBD_BG_CL_ABO: &str = "E_0052";
/// `E_0017` — Marktlokationen mit BG-CL abgleichen (Einzelanforderung). Rolle: **NB**.
pub const EBD_BG_CL_EINZEL: &str = "E_0017";
/// `E_0096` — Marktlokationen mit LF-AACL abgleichen. Rolle: **LF**.
pub const EBD_LF_AACL: &str = "E_0096";
/// `E_0097` — Marktlokationen mit LF-AACL abgleichen (Einzelanforderung). Rolle: **LF**.
pub const EBD_LF_AACL_EINZEL: &str = "E_0097";
/// `E_0070` — DZÜ-Liste prüfen. Rolle: **NB**.
pub const EBD_DZUE_LISTE: &str = "E_0070";

/// Position-level Korrekturgründe, shared wording across every Clearingliste.
///
/// The **numbering is not shared** — see [`E_0049_CODES`].
macro_rules! korrektur {
    ($c:literal, $ebd:expr, "ergaenzt") => {
        code!(
            $c,
            Some($ebd),
            KorrekturlisteWegenAblehnung,
            "Zusätzlicher Datensatz / ergänzte Marktlokation"
        )
    };
    ($c:literal, $ebd:expr, "falscher_lf") => {
        code!(
            $c,
            Some($ebd),
            KorrekturlisteWegenAblehnung,
            "Marktlokation falschem LF zugeordnet"
        )
    };
    ($c:literal, $ebd:expr, "falscher_nb") => {
        code!(
            $c,
            Some($ebd),
            KorrekturlisteWegenAblehnung,
            "Marktlokation falschem NB zugeordnet"
        )
    };
    ($c:literal, $ebd:expr, "entfallen") => {
        code!(
            $c,
            Some($ebd),
            KorrekturlisteWegenAblehnung,
            "Zu viele Marktlokationen enthalten / entfallene Marktlokation"
        )
    };
    ($c:literal, $ebd:expr, "daten") => {
        code!(
            $c,
            Some($ebd),
            KorrekturlisteWegenAblehnung,
            "bilanzierungsrel. Daten nicht korrekt / fehlen"
        )
    };
}

/// Whole-list refusal: the subscription this list claims to serve does not exist.
macro_rules! kein_abo {
    ($c:literal, $ebd:expr) => {
        code!(
            $c, Some($ebd), AblehnungDerGesamtenListe,
            "Abonnement wurde nicht bestellt (bedeutet auch, dass ein Abonnement für diesen Zeitraum bereits beendet wurde)."
        )
    };
}

/// `E_0004` — Marktlokationen mit LF-CL abgleichen (Erstabonnierung).
pub const E_0004_CODES: &[AntwortCode] = &[
    kein_abo!("A01", EBD_LF_CL_ERSTABO),
    code!(
        "A02",
        Some(EBD_LF_CL_ERSTABO),
        AblehnungDerGesamtenListe,
        "Version nicht zugelassen"
    ),
    korrektur!("A03", EBD_LF_CL_ERSTABO, "ergaenzt"),
    korrektur!("A04", EBD_LF_CL_ERSTABO, "falscher_lf"),
    korrektur!("A05", EBD_LF_CL_ERSTABO, "entfallen"),
    korrektur!("A06", EBD_LF_CL_ERSTABO, "daten"),
];

/// `E_0049` — Marktlokationen mit LF-CL abgleichen (Folgeabonnierung).
///
/// **`A06` does not exist in this tree.** The published Codeliste runs
/// `A01`–`A05`, `A07`; the „bilanzierungsrel. Daten" Korrekturgrund that is
/// `A06` in [`E_0004_CODES`] is `A07` here. This is exactly why the catalogue
/// is per-tree: a shared position-to-code table would silently emit `A06`.
pub const E_0049_CODES: &[AntwortCode] = &[
    kein_abo!("A01", EBD_LF_CL_FOLGEABO),
    code!(
        "A02",
        Some(EBD_LF_CL_FOLGEABO),
        AblehnungDerGesamtenListe,
        "Version nicht zugelassen"
    ),
    korrektur!("A03", EBD_LF_CL_FOLGEABO, "ergaenzt"),
    korrektur!("A04", EBD_LF_CL_FOLGEABO, "falscher_lf"),
    korrektur!("A05", EBD_LF_CL_FOLGEABO, "entfallen"),
    korrektur!("A07", EBD_LF_CL_FOLGEABO, "daten"),
];

/// `E_0052` — Marktlokationen mit BG-CL abgleichen (Abonnierung).
pub const E_0052_CODES: &[AntwortCode] = &[
    kein_abo!("A01", EBD_BG_CL_ABO),
    code!(
        "A02",
        Some(EBD_BG_CL_ABO),
        AblehnungDerGesamtenListe,
        "Version nicht zugelassen"
    ),
    korrektur!("A03", EBD_BG_CL_ABO, "ergaenzt"),
    korrektur!("A04", EBD_BG_CL_ABO, "falscher_nb"),
    korrektur!("A05", EBD_BG_CL_ABO, "entfallen"),
    korrektur!("A06", EBD_BG_CL_ABO, "daten"),
];

/// The three whole-list refusals an Einzelanforderungs-Clearingliste can draw.
macro_rules! einzel_ablehnungen {
    ($ebd:expr) => {
        [
            code!(
                "A01",
                Some($ebd),
                AblehnungDerGesamtenListe,
                "Zeitraum nicht plausibel"
            ),
            code!(
                "A02",
                Some($ebd),
                AblehnungDerGesamtenListe,
                "MaBiS-ZP entspricht nicht dem angefragten MaBiS-ZP"
            ),
            code!(
                "A03",
                Some($ebd),
                AblehnungDerGesamtenListe,
                "Version nicht zugelassen"
            ),
        ]
    };
}

/// `E_0014` — Marktlokationen mit LF-CL abgleichen (Einzelanforderung).
pub const E_0014_CODES: &[AntwortCode] = &{
    let [a01, a02, a03] = einzel_ablehnungen!(EBD_LF_CL_EINZEL);
    [
        a01,
        a02,
        a03,
        korrektur!("A04", EBD_LF_CL_EINZEL, "ergaenzt"),
        korrektur!("A05", EBD_LF_CL_EINZEL, "falscher_lf"),
        korrektur!("A06", EBD_LF_CL_EINZEL, "entfallen"),
        korrektur!("A07", EBD_LF_CL_EINZEL, "daten"),
    ]
};

/// `E_0047` — Marktlokationen mit LF-CL abgleichen (Clearing).
pub const E_0047_CODES: &[AntwortCode] = &{
    let [a01, a02, a03] = einzel_ablehnungen!(EBD_LF_CL_CLEARING);
    [
        a01,
        a02,
        a03,
        korrektur!("A04", EBD_LF_CL_CLEARING, "ergaenzt"),
        korrektur!("A05", EBD_LF_CL_CLEARING, "falscher_lf"),
        korrektur!("A06", EBD_LF_CL_CLEARING, "entfallen"),
        korrektur!("A07", EBD_LF_CL_CLEARING, "daten"),
    ]
};

/// `E_0017` — Marktlokationen mit BG-CL abgleichen (Einzelanforderung).
pub const E_0017_CODES: &[AntwortCode] = &{
    let [a01, a02, a03] = einzel_ablehnungen!(EBD_BG_CL_EINZEL);
    [
        a01,
        a02,
        a03,
        korrektur!("A04", EBD_BG_CL_EINZEL, "ergaenzt"),
        korrektur!("A05", EBD_BG_CL_EINZEL, "falscher_nb"),
        korrektur!("A06", EBD_BG_CL_EINZEL, "entfallen"),
        korrektur!("A07", EBD_BG_CL_EINZEL, "daten"),
    ]
};

/// `E_0097` — Marktlokationen mit LF-AACL abgleichen (Einzelanforderung).
pub const E_0097_CODES: &[AntwortCode] = &{
    let [a01, a02, a03] = einzel_ablehnungen!(EBD_LF_AACL_EINZEL);
    [
        a01,
        a02,
        a03,
        korrektur!("A04", EBD_LF_AACL_EINZEL, "ergaenzt"),
        korrektur!("A05", EBD_LF_AACL_EINZEL, "falscher_lf"),
        korrektur!("A06", EBD_LF_AACL_EINZEL, "entfallen"),
        korrektur!("A07", EBD_LF_AACL_EINZEL, "daten"),
    ]
};

/// `E_0096` — Marktlokationen mit LF-AACL abgleichen.
///
/// Six codes, not seven: the AACL abonniert as a whole, so there is no
/// „MaBiS-ZP entspricht nicht dem angefragten MaBiS-ZP" step.
pub const E_0096_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_LF_AACL),
        AblehnungDerGesamtenListe,
        "Zeitraum nicht plausibel"
    ),
    code!(
        "A02",
        Some(EBD_LF_AACL),
        AblehnungDerGesamtenListe,
        "Version nicht zugelassen"
    ),
    korrektur!("A03", EBD_LF_AACL, "ergaenzt"),
    korrektur!("A04", EBD_LF_AACL, "falscher_lf"),
    korrektur!("A05", EBD_LF_AACL, "entfallen"),
    korrektur!("A06", EBD_LF_AACL, "daten"),
];

/// `E_0070` — DZÜ-Liste prüfen.
///
/// The only Clearingliste whose whole-list refusal is a **Frist**: a DZÜ-Liste
/// outside the Clearingphase DZÜ is refused entire.
pub const E_0070_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_DZUE_LISTE),
        AblehnungDerGesamtenListe,
        "Eingang liegt nicht innerhalb der Clearingphase DZÜ"
    ),
    code!(
        "A02",
        Some(EBD_DZUE_LISTE),
        KorrekturlisteWegenAblehnung,
        "Marktlokation ist nicht bekannt"
    ),
    korrektur!("A03", EBD_DZUE_LISTE, "daten"),
];

// ═════════════════════════════════════════════════════════════════════════════
// MaBiS-Zählpunkt — Aktivierung, Deaktivierung, Zuordnung
// ═════════════════════════════════════════════════════════════════════════════

/// `E_0020` — MaBiS-ZP Aktivierung prüfen. Prüfende Rolle: **NB**.
pub const EBD_ZP_AKTIVIERUNG: &str = "E_0020";
/// `E_0010` — MaBiS-ZP Deaktivierung prüfen. Prüfende Rolle: **NB**.
pub const EBD_ZP_DEAKTIVIERUNG: &str = "E_0010";
/// `E_0102` — Zuordnung (Netzgangzeitreihe zu Netzzeitreihe) prüfen. Rolle: **NB**.
pub const EBD_ZP_ZUORDNUNG: &str = "E_0102";
/// `E_0103` — Beendigung der Zuordnung prüfen. Prüfende Rolle: **NB**.
pub const EBD_ZP_BEENDIGUNG: &str = "E_0103";

/// `E_0020` — MaBiS-ZP Aktivierung prüfen.
///
/// Twelve codes: the widest MaBiS tree, because activating a MaBiS-ZP asserts a
/// Bilanzierungsgebiet, a Regelzone, a Zuordnungsermächtigung and an OBIS-Kennzahl
/// all at once, and each of them can be wrong on its own.
///
/// Note that its refusals are plain `Ablehnung`, **not** `Abweisung`: an
/// Aktivierung that arrives too late has still been assessed.
pub const E_0020_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Fristüberschreitung"
    ),
    code!(
        "A02",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Gewählter Zeitpunkt nicht zulässig"
    ),
    code!(
        "A03",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "ID bereits außerhalb MaBiS verwendet"
    ),
    code!(
        "A04",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Bilanzierungsgebiet des benachbarten NB nicht gültig"
    ),
    code!(
        "A05",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Regelzone falsch"
    ),
    code!(
        "A06",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Keine Berechtigung"
    ),
    code!(
        "A07",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Abweichender MaBiS-ZP bereits vorhanden"
    ),
    code!(
        "A08",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "Abweichende ID zum MaBiS-ZP bereits vorhanden"
    ),
    code!(
        "A09",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "ZRT Aktivierung nicht berechtigt"
    ),
    code!(
        "A10",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "OBIS nicht passend"
    ),
    code!(
        "A11",
        Some(EBD_ZP_AKTIVIERUNG),
        Ablehnung,
        "MaBiS-ZP bereits aktiviert"
    ),
    code!(
        "A12",
        Some(EBD_ZP_AKTIVIERUNG),
        Zustimmung,
        "Aktivierung durchgeführt"
    ),
];

/// `E_0010` — MaBiS-ZP Deaktivierung prüfen.
pub const E_0010_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_ZP_DEAKTIVIERUNG),
        Ablehnung,
        "Fristüberschreitung"
    ),
    code!(
        "A02",
        Some(EBD_ZP_DEAKTIVIERUNG),
        Ablehnung,
        "Gewählter Zeitpunkt nicht zulässig"
    ),
    code!(
        "A03",
        Some(EBD_ZP_DEAKTIVIERUNG),
        Ablehnung,
        "ID bereits außerhalb MaBiS verwendet"
    ),
    code!(
        "A04",
        Some(EBD_ZP_DEAKTIVIERUNG),
        Ablehnung,
        "MaBiS-ZP bereits deaktiviert"
    ),
    code!(
        "A05",
        Some(EBD_ZP_DEAKTIVIERUNG),
        Ablehnung,
        "Deaktivierung, Zeitreihen vorhanden"
    ),
    code!(
        "A06",
        Some(EBD_ZP_DEAKTIVIERUNG),
        Zustimmung,
        "Deaktivierung durchgeführt"
    ),
];

/// `E_0102` — Zuordnung prüfen.
pub const E_0102_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_ZP_ZUORDNUNG),
        Ablehnung,
        "ID bereits außerhalb MaBiS verwendet"
    ),
    code!(
        "A02",
        Some(EBD_ZP_ZUORDNUNG),
        Ablehnung,
        "Zuordnung passt nicht zur Vereinbarung"
    ),
    code!(
        "A03",
        Some(EBD_ZP_ZUORDNUNG),
        Ablehnung,
        "Keine Berechtigung für die Netzzeitreihe"
    ),
    code!(
        "A04",
        Some(EBD_ZP_ZUORDNUNG),
        Ablehnung,
        "Keine Beteiligung an der Netzzeitreihe"
    ),
    code!(
        "A05",
        Some(EBD_ZP_ZUORDNUNG),
        Ablehnung,
        "Zuordnung bereits vorhanden",
        bemerkung
    ),
    code!(
        "A06",
        Some(EBD_ZP_ZUORDNUNG),
        Zustimmung,
        "Zuordnung durchgeführt"
    ),
    code!(
        "A99",
        Some(EBD_ZP_ZUORDNUNG),
        Ablehnung,
        "Sonstiges",
        bemerkung
    ),
];

/// `E_0103` — Beendigung der Zuordnung prüfen.
pub const E_0103_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_ZP_BEENDIGUNG),
        Ablehnung,
        "Beendigung der Zuordnung passt nicht zur Vereinbarung"
    ),
    code!(
        "A02",
        Some(EBD_ZP_BEENDIGUNG),
        Ablehnung,
        "Keine Berechtigung für die Netzzeitreihe"
    ),
    code!(
        "A03",
        Some(EBD_ZP_BEENDIGUNG),
        Ablehnung,
        "Keine Beteiligung an der Netzzeitreihe"
    ),
    code!(
        "A04",
        Some(EBD_ZP_BEENDIGUNG),
        Ablehnung,
        "Zuordnung nicht vorhanden"
    ),
    code!(
        "A05",
        Some(EBD_ZP_BEENDIGUNG),
        Zustimmung,
        "Beendigung der Zuordnung durchgeführt"
    ),
    code!(
        "A99",
        Some(EBD_ZP_BEENDIGUNG),
        Ablehnung,
        "Sonstiges",
        bemerkung
    ),
];

// ═════════════════════════════════════════════════════════════════════════════
// Profile und Einzelanforderung — Reklamations-only trees
// ═════════════════════════════════════════════════════════════════════════════

/// `E_0100` — Profile bzw. Profilscharen prüfen. Prüfende Rolle: **LF**.
pub const EBD_PROFILE: &str = "E_0100";
/// `E_0068` — Einzelanforderung prüfen. Prüfende Rolle: **NB**.
pub const EBD_EINZELANFORDERUNG_NB: &str = "E_0068";
/// `E_0104` — Listeninhalt prüfen. Prüfende Rolle: **NB**.
pub const EBD_LISTENINHALT_NB: &str = "E_0104";

/// `E_0100` — Profile bzw. Profilscharen prüfen.
///
/// Every code is a Reklamationsgrund; the tree publishes no Zustimmung, because
/// an accepted profile is answered with silence. A Reklamation **does not
/// invalidate the profile** it complains about — the LF keeps bilanzierend with
/// it until a corrected version arrives.
pub const E_0100_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_PROFILE),
        Reklamation,
        "Profil bzw. Profilschar gehört nicht zu einer zuvor abonnierten Profilgruppe"
    ),
    code!(
        "A02",
        Some(EBD_PROFILE),
        Reklamation,
        "Version des Profils nicht zugelassen"
    ),
    code!(
        "A03",
        Some(EBD_PROFILE),
        Reklamation,
        "Version der Profilschar nicht zugelassen"
    ),
    code!(
        "A04",
        Some(EBD_PROFILE),
        Reklamation,
        "Maßeinheit weicht von Liste der Profildefinitionen ab"
    ),
    code!(
        "A05",
        Some(EBD_PROFILE),
        Reklamation,
        "Niedrigste Temperaturmaßzahl weicht von Liste der Profildefinitionen ab"
    ),
    code!(
        "A06",
        Some(EBD_PROFILE),
        Reklamation,
        "Anzahl der Temperaturmaßzahlen weicht von Liste der Profildefinitionen ab"
    ),
];

/// `E_0068` — Einzelanforderung prüfen.
pub const E_0068_CODES: &[AntwortCode] = &[code!(
    "A01",
    Some(EBD_EINZELANFORDERUNG_NB),
    Reklamation,
    "Kein Lieferant zugeordnet"
)];

/// `E_0104` — Listeninhalt prüfen.
pub const E_0104_CODES: &[AntwortCode] = &[code!(
    "A01",
    Some(EBD_LISTENINHALT_NB),
    Reklamation,
    "Kein Lieferant zugeordnet"
)];

// ═════════════════════════════════════════════════════════════════════════════
// Redispatch — Ausfallarbeit
// ═════════════════════════════════════════════════════════════════════════════

/// `E_0902` — Ausfallarbeit unter Einbeziehung Fahrplananteil plausibilisieren.
/// Prüfende Rolle: **NB**.
pub const EBD_AUSFALLARBEIT: &str = "E_0902";
/// `E_0901` — Gegenvorschlag prüfen. Prüfende Rolle: **NB**.
pub const EBD_GEGENVORSCHLAG: &str = "E_0901";

/// `E_0902` — Ausfallarbeit plausibilisieren.
///
/// The two Ablehnungen differ in what the NB owes next, not in why it refused:
/// `A02` sends a Gegenvorschlag (the NB states its own figures), `A03` demands
/// a correction from the Betreiber. Reading them as one code loses the
/// obligation.
pub const E_0902_CODES: &[AntwortCode] = &[
    code!("A01", Some(EBD_AUSFALLARBEIT), Zustimmung, "Zustimmung"),
    code!(
        "A02",
        Some(EBD_AUSFALLARBEIT),
        Ablehnung,
        "Energiemengen falsch / nicht plausibel — Übermittlung Gegenvorschlag",
        bemerkung
    ),
    code!(
        "A03",
        Some(EBD_AUSFALLARBEIT),
        Ablehnung,
        "Energiemengen falsch / nicht plausibel — inkl. Korrekturanforderung",
        bemerkung
    ),
];

/// `E_0901` — Gegenvorschlag prüfen.
pub const E_0901_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        Some(EBD_GEGENVORSCHLAG),
        Ablehnung,
        "Ausfallarbeitszeitreihe wurde bereits bestätigt."
    ),
    code!(
        "A02",
        Some(EBD_GEGENVORSCHLAG),
        Ablehnung,
        "Fristüberschreitung"
    ),
    code!(
        "A03",
        Some(EBD_GEGENVORSCHLAG),
        Ablehnung,
        "Gegenvorschlag liegt bereits vor"
    ),
    code!(
        "A04",
        Some(EBD_GEGENVORSCHLAG),
        Ablehnung,
        "Energiemengen falsch / nicht plausibel"
    ),
    code!("A05", Some(EBD_GEGENVORSCHLAG), Zustimmung, "Zustimmung"),
];

// ═════════════════════════════════════════════════════════════════════════════

/// Every MaBiS and Redispatch tree catalogued here, as `(ebd, codes)`.
pub const MABIS_TREES: &[(&str, &[AntwortCode])] = &[
    (EBD_LF_SZR_A, E_0007_CODES),
    (EBD_LF_SZR_B, E_0041_CODES),
    (EBD_LF_AASZR, E_0093_CODES),
    (EBD_NZR, E_0040_CODES),
    (EBD_BG_SZR_B, E_0062_CODES),
    (EBD_BK_SZR_A, E_0063_CODES),
    (EBD_BK_SZR_B, E_0064_CODES),
    (EBD_AAUEZ_MONATLICH_A, E_0098_CODES),
    (EBD_AAUEZ_MONATLICH_B, E_0099_CODES),
    (EBD_DZUE, E_0065_CODES),
    (EBD_LF_CL_ERSTABO, E_0004_CODES),
    (EBD_LF_CL_FOLGEABO, E_0049_CODES),
    (EBD_LF_CL_EINZEL, E_0014_CODES),
    (EBD_LF_CL_CLEARING, E_0047_CODES),
    (EBD_BG_CL_ABO, E_0052_CODES),
    (EBD_BG_CL_EINZEL, E_0017_CODES),
    (EBD_LF_AACL, E_0096_CODES),
    (EBD_LF_AACL_EINZEL, E_0097_CODES),
    (EBD_DZUE_LISTE, E_0070_CODES),
    (EBD_ZP_AKTIVIERUNG, E_0020_CODES),
    (EBD_ZP_DEAKTIVIERUNG, E_0010_CODES),
    (EBD_ZP_ZUORDNUNG, E_0102_CODES),
    (EBD_ZP_BEENDIGUNG, E_0103_CODES),
    (EBD_PROFILE, E_0100_CODES),
    (EBD_EINZELANFORDERUNG_NB, E_0068_CODES),
    (EBD_LISTENINHALT_NB, E_0104_CODES),
    (EBD_AUSFALLARBEIT, E_0902_CODES),
    (EBD_GEGENVORSCHLAG, E_0901_CODES),
];

/// Resolve `code` **within** `ebd`.
///
/// Returns `None` when the tree does not publish the code — which is the whole
/// point: `A06` is „bilanzierungsrel. Daten nicht korrekt" in `E_0004` and is
/// not published at all by `E_0049`.
#[must_use]
pub fn lookup(ebd: &str, code: &str) -> Option<&'static AntwortCode> {
    let (_, codes) = MABIS_TREES.iter().find(|(id, _)| *id == ebd)?;
    codes.iter().find(|c| c.code == code)
}

/// The tree's own Zustimmungscode, or `None` where it publishes none.
///
/// `E_0100`, `E_0068` and `E_0104` reach agreement by staying silent, so asking
/// them for a Zustimmung is a caller bug rather than a missing entry.
#[must_use]
pub fn zustimmung(ebd: &str) -> Option<&'static AntwortCode> {
    let (_, codes) = MABIS_TREES.iter().find(|(id, _)| *id == ebd)?;
    codes
        .iter()
        .find(|c| c.cluster == crate::codes::Cluster::Zustimmung)
}
