//! What an INVOIC states about its own Umsatzsteuer — the two Prüfschritte of
//! the MSB invoice trees an invoice answers about itself.
//!
//! - **Prüfschritt 10** (`E_0264`/`E_0270`/`E_0273`, and 17 in the Storno
//!   trees): „Erfüllt die Rechnung weitere Anforderungen gem. §14 Abs. 4
//!   UStG?" (EBD 4.3 Kap. 8.27.1, S. 626; Kap. 9.3.1, S. 655; Kap. 9.4.1,
//!   S. 665; Kap. 8.27.4, S. 642). [`ustg_14_4_maengel`] names every
//!   Pflichtangabe the message lacks.
//! - **Prüfschritt 330**: „Wird für die Rechnungsposition der für diesen
//!   Zeitpunkt/Zeitraum korrekte gültige Umsatzsteuersatz angegeben?" (EBD 4.3
//!   Kap. 8.27.1, S. 628; Kap. 9.3.1, S. 658; Kap. 9.4.1, S. 668).
//!   [`steuersatz_korrekt`] answers it where the position's Steuerkategorie
//!   settles which rate is correct.
//!
//! # Where the Angaben sit on the wire
//!
//! INVOIC AHB 1.0b, column 31009 (S. 13–15), and INVOIC MIG 2.8e, S. 29–30:
//!
//! | § 14 Abs. 4 UStG | INVOIC | `Rechnung` |
//! |---|---|---|
//! | Nr. 1 Name, Anschrift des leistenden Unternehmers | `SG2 NAD+MS` C080, C059, 3164, 3251, 3207 | `rechnungsersteller` |
//! | Nr. 1 Name, Anschrift des Leistungsempfängers | `SG2 NAD+MR`, as above | `rechnungsempfaenger` |
//! | Nr. 2 Steuernummer oder USt-IdNr. | `SG3 RFF+FC` / `RFF+VA` after `NAD+MS` | `umsatzsteuer_id`, [`STEUERNUMMER_ATTRIBUT`] |
//! | Nr. 3 Ausstellungsdatum | `DTM+137` | `rechnungsdatum` |
//! | Nr. 4 Rechnungsnummer | `BGM` DE 1004 | `rechnungsnummer` |
//! | Nr. 5 Umfang und Art der Leistung | `SG26 LIN` Artikel-ID, `QTY` | `rechnungspositionen` |
//! | Nr. 6 Zeitpunkt der Leistung | `DTM+155`/`156` (Kopf or Position) | `rechnungsperiode`, `lieferungszeitraum` |
//! | Nr. 7 Entgelt je Steuersatz | `SG52 TAX` + `MOA+125` | `steuerbetraege[].basiswert` |
//! | Nr. 8 Steuersatz und Steuerbetrag | `SG52 TAX` DE 5278 + `MOA+161` | `steuerbetraege[].steuersatz`, `steuerwert` |
//!
//! The MIG leaves the choice between `FC` and `VA` to the Rechnungssteller
//! („Welche angegeben wird, entscheidet der Rechnungssteller", S. 30), so
//! either one satisfies Nr. 2. Nr. 1 asks for a name and an Anschrift: the
//! AHB makes Ort and Postleitzahl Muss and the street a Soll that a
//! Großkundenpostleitzahl replaces (`[7]`), so a missing street is not a
//! missing Anschrift.
//!
//! Two Angaben are not assessed, because neither binds this invoice: Nr. 9
//! applies „in den Fällen des § 14b Abs. 1 Satz 5" — a Leistungsempfänger who
//! is not an Unternehmer or uses the Leistung outside his business — and
//! `NAD+MR` names a Marktpartner by its MP-ID in its market role; Nr. 10 is a
//! Gutschrift's, and the 31009 is a „Handelsrechnung" (`BGM` 380, AHB S. 8)
//! the MSB issues itself.
//!
//! What is checked is that each Angabe is **stated**. Whether a stated
//! Steuernummer is the one the Finanzamt issued is not on the message; the
//! AHB has it exchanged beforehand by PARTIN (`[527]`).

use rubo4e::current::{Geschaeftspartner, Rechnung, Steuerart, Steuerbetrag};
use rust_decimal::Decimal;

/// The `ZusatzAttribut` a `Steuerbetrag` carries the INVOIC's `TAX` DE 5305
/// Steuerkategorie under — `S` Einheitssatz (Standard), `O` nicht steuerbar,
/// `AE` Steuerschuldnerschaft des Leistungsempfängers (AHB 1.0b, S. 20). BO4E
/// has no field for it; makod's inbound adapter writes it.
pub const STEUERKATEGORIE_ATTRIBUT: &str = "mako:steuerkategorie";

/// The `ZusatzAttribut` a `Geschaeftspartner` carries the `SG3 RFF+FC`
/// Steuernummer under. BO4E has a field for the Umsatzsteuer-ID (`RFF+VA`)
/// and none for the Steuernummer; makod's inbound adapter writes it.
pub const STEUERNUMMER_ATTRIBUT: &str = "mako:steuernummer";

/// Every § 14 Abs. 4 UStG Angabe `rechnung` does not state, each named with
/// its Nummer and its place on the INVOIC. Empty means Prüfschritt 10
/// answers „ja".
#[must_use]
pub fn ustg_14_4_maengel(rechnung: &Rechnung) -> Vec<String> {
    let mut m = Vec::new();
    partei(
        rechnung.rechnungsersteller.as_deref(),
        "des leistenden Unternehmers (NAD+MS)",
        &mut m,
    );
    partei(
        rechnung.rechnungsempfaenger.as_deref(),
        "des Leistungsempfängers (NAD+MR)",
        &mut m,
    );
    let ersteller = rechnung.rechnungsersteller.as_deref();
    let ust_id = ersteller
        .and_then(|g| g.umsatzsteuer_id.as_deref())
        .is_some_and(|v| !v.trim().is_empty());
    let steuernummer = ersteller.and_then(steuernummer).is_some();
    if !ust_id && !steuernummer {
        m.push(
            "Nr. 2 — Steuernummer oder Umsatzsteuer-Identifikationsnummer des leistenden \
             Unternehmers (SG3 RFF+FC/VA)"
                .to_owned(),
        );
    }
    if rechnung.rechnungsdatum.is_none() {
        m.push("Nr. 3 — Ausstellungsdatum (DTM+137)".to_owned());
    }
    if rechnung
        .rechnungsnummer
        .as_deref()
        .is_none_or(|n| n.trim().is_empty())
    {
        m.push("Nr. 4 — Rechnungsnummer (BGM DE 1004)".to_owned());
    }
    let positionen = rechnung.rechnungspositionen.as_deref().unwrap_or_default();
    if positionen.is_empty() {
        m.push("Nr. 5 — Umfang und Art der Leistung: keine Rechnungsposition (SG26)".to_owned());
    }
    for p in positionen {
        let nr = p
            .positionsnummer
            .map_or_else(|| "ohne Nummer".to_owned(), |n| n.to_string());
        if p.artikel_id.as_deref().is_none_or(|a| a.trim().is_empty()) {
            m.push(format!(
                "Nr. 5 — Art der Leistung in Position {nr} (SG26 LIN Artikel-ID)"
            ));
        }
        let menge = p.positions_menge.as_ref().and_then(|q| q.wert).is_some()
            || p.zeitbezogene_menge.as_ref().and_then(|q| q.wert).is_some();
        if !menge {
            m.push(format!(
                "Nr. 5 — Umfang der Leistung in Position {nr} (SG26 QTY)"
            ));
        }
    }
    let kopf_zeitpunkt = rechnung
        .rechnungsperiode
        .as_ref()
        .is_some_and(|z| z.startdatum.is_some() || z.enddatum.is_some());
    let positions_zeitpunkt = !positionen.is_empty()
        && positionen.iter().all(|p| {
            p.lieferungszeitraum
                .as_ref()
                .is_some_and(|z| z.startdatum.is_some() || z.enddatum.is_some())
        });
    if !kopf_zeitpunkt && !positions_zeitpunkt {
        m.push("Nr. 6 — Zeitpunkt der Leistung (DTM+155/156)".to_owned());
    }
    let steuern = rechnung.steuerbetraege.as_deref().unwrap_or_default();
    if steuern.is_empty() {
        m.push(
            "Nr. 7/8 — Entgelt, Steuersatz und Steuerbetrag je Steuersatz (SG52 TAX)".to_owned(),
        );
    }
    for s in steuern {
        let satz = s.steuersatz.map_or_else(
            || "ohne Satz".to_owned(),
            |v| format!("{} %", v.normalize()),
        );
        if s.basiswert.is_none() {
            m.push(format!(
                "Nr. 7 — Entgelt zum Steuersatz {satz} (SG52 MOA+125)"
            ));
        }
        if s.steuersatz.is_none() {
            m.push("Nr. 8 — anzuwendender Steuersatz (SG52 TAX DE 5278)".to_owned());
        }
        // Under the Steuerschuldnerschaft des Leistungsempfängers the
        // Rechnungssteller states no tax of his own; every other category
        // states the amount, `0` included.
        if s.steuerwert.is_none() && steuerkategorie(s).as_deref() != Some("AE") {
            m.push(format!(
                "Nr. 8 — Steuerbetrag zum Steuersatz {satz} (SG52 MOA+161)"
            ));
        }
    }
    m
}

fn partei(gp: Option<&Geschaeftspartner>, wer: &str, m: &mut Vec<String>) {
    let Some(gp) = gp else {
        m.push(format!("Nr. 1 — Name und Anschrift {wer}"));
        return;
    };
    let name = [&gp.organisationsname, &gp.nachname]
        .into_iter()
        .flatten()
        .any(|n| !n.trim().is_empty());
    if !name {
        m.push(format!("Nr. 1 — Name {wer}, C080 DE 3036"));
    }
    let adresse = gp.adresse.as_ref();
    let filled = |v: Option<&String>| v.is_some_and(|s| !s.trim().is_empty());
    if !filled(adresse.and_then(|a| a.ort.as_ref())) {
        m.push(format!("Nr. 1 — Ort der Anschrift {wer}, DE 3164"));
    }
    if !filled(adresse.and_then(|a| a.postleitzahl.as_ref())) {
        m.push(format!("Nr. 1 — Postleitzahl der Anschrift {wer}, DE 3251"));
    }
}

fn steuernummer(gp: &Geschaeftspartner) -> Option<&str> {
    attribut(gp.zusatz_attribute.as_deref(), STEUERNUMMER_ATTRIBUT).filter(|v| !v.trim().is_empty())
}

fn attribut<'a>(
    attrs: Option<&'a [rubo4e::current::ZusatzAttribut]>,
    name: &str,
) -> Option<&'a str> {
    attrs?
        .iter()
        .find(|a| a.name.as_deref() == Some(name))
        .and_then(|a| a.wert.as_ref())
        .and_then(serde_json::Value::as_str)
}

/// The `TAX` DE 5305 Steuerkategorie of `s`: the one the INVOIC stated, or
/// `AE` for a `Steuerbetrag` that says reverse charge in BO4E's own terms.
#[must_use]
pub fn steuerkategorie(s: &Steuerbetrag) -> Option<String> {
    attribut(s.zusatz_attribute.as_deref(), STEUERKATEGORIE_ATTRIBUT)
        .map(str::to_owned)
        .or_else(|| (s.steuerart == Some(Steuerart::Rcv)).then(|| "AE".to_owned()))
}

/// Prüfschritt 330 for one position: does its `SG34 TAX` state the rate valid
/// for `zeitraum`?
///
/// Decided only for Steuerkategorie `S`, „Einheitssatz (Standard)": that
/// category's rate is § 12 Abs. 1 UStG's („Die Steuer beträgt für jeden
/// steuerpflichtigen Umsatz 19 Prozent der Bemessungsgrundlage"), 16 % for
/// 01.07.–31.12.2020 ([`energy_billing::rates::mwst_rate_for_period`]).
/// `None` — the step stays open — for
///
/// - `O` nicht steuerbar and `AE` Steuerschuldnerschaft des
///   Leistungsempfängers, whose correctness is a judgement about the
///   Leistung the message does not settle;
/// - a position without a Steuerkategorie, a rate or a period;
/// - a period that straddles a change of the rate, for which no single rate is
///   correct.
#[must_use]
pub fn steuersatz_korrekt(
    s: &Steuerbetrag,
    zeitraum: Option<(time::Date, time::Date)>,
) -> Option<bool> {
    if steuerkategorie(s).as_deref() != Some("S") {
        return None;
    }
    let satz = s.steuersatz?;
    let (von, bis) = zeitraum?;
    let gueltig = energy_billing::rates::mwst_rate_for_period(von, bis)?;
    Some(satz == gueltig * Decimal::ONE_HUNDRED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubo4e::current::{Adresse, ZusatzAttribut};
    use rust_decimal::dec;
    use time::macros::date;

    fn kategorie(k: &str) -> Option<Vec<ZusatzAttribut>> {
        Some(vec![ZusatzAttribut {
            name: Some(STEUERKATEGORIE_ATTRIBUT.to_owned()),
            wert: Some(serde_json::Value::from(k)),
            ..ZusatzAttribut::default()
        }])
    }

    fn tax(satz: Decimal, k: &str) -> Steuerbetrag {
        Steuerbetrag {
            steuersatz: Some(satz),
            zusatz_attribute: kategorie(k),
            ..Steuerbetrag::default()
        }
    }

    /// Prüfschritt 330: an `S` position states § 12 Abs. 1 UStG's rate for its
    /// period, 16 % inside the 2020 window; another rate is wrong.
    #[test]
    fn a_standard_position_is_held_to_the_regelsatz_of_its_period() {
        let maerz = Some((date!(2026 - 03 - 01), date!(2026 - 03 - 31)));
        assert_eq!(steuersatz_korrekt(&tax(dec!(19), "S"), maerz), Some(true));
        assert_eq!(
            steuersatz_korrekt(&tax(dec!(19.00), "S"), maerz),
            Some(true)
        );
        assert_eq!(steuersatz_korrekt(&tax(dec!(7), "S"), maerz), Some(false));
        assert_eq!(steuersatz_korrekt(&tax(dec!(0), "S"), maerz), Some(false));
        let herbst_2020 = Some((date!(2020 - 08 - 01), date!(2020 - 08 - 31)));
        assert_eq!(
            steuersatz_korrekt(&tax(dec!(16), "S"), herbst_2020),
            Some(true)
        );
        assert_eq!(
            steuersatz_korrekt(&tax(dec!(19), "S"), herbst_2020),
            Some(false)
        );
    }

    /// What the message does not settle stays open: a category other than
    /// `S`, a missing period, a period across the 2020 Stichtag.
    #[test]
    fn what_the_message_does_not_settle_stays_open() {
        let maerz = Some((date!(2026 - 03 - 01), date!(2026 - 03 - 31)));
        assert_eq!(steuersatz_korrekt(&tax(dec!(0), "O"), maerz), None);
        assert_eq!(steuersatz_korrekt(&tax(dec!(0), "AE"), maerz), None);
        let ohne = Steuerbetrag {
            steuersatz: Some(dec!(19)),
            ..Steuerbetrag::default()
        };
        assert_eq!(steuersatz_korrekt(&ohne, maerz), None);
        assert_eq!(steuersatz_korrekt(&tax(dec!(19), "S"), None), None);
        let quer = Some((date!(2020 - 12 - 01), date!(2021 - 01 - 31)));
        assert_eq!(steuersatz_korrekt(&tax(dec!(19), "S"), quer), None);
    }

    fn partner(name: &str) -> Geschaeftspartner {
        Geschaeftspartner {
            organisationsname: Some(name.to_owned()),
            adresse: Some(Adresse {
                ort: Some("Köln".to_owned()),
                postleitzahl: Some("50667".to_owned()),
                ..Adresse::default()
            }),
            ..Geschaeftspartner::default()
        }
    }

    /// Prüfschritt 10: an invoice missing Pflichtangaben is refused, and each
    /// one is named; the Steuernummer satisfies Nr. 2 as well as the USt-IdNr.
    #[test]
    fn each_missing_pflichtangabe_is_named() {
        let leer = ustg_14_4_maengel(&Rechnung::default());
        for nr in [
            "Nr. 1", "Nr. 2", "Nr. 3", "Nr. 4", "Nr. 5", "Nr. 6", "Nr. 7/8",
        ] {
            assert!(leer.iter().any(|m| m.starts_with(nr)), "{nr}: {leer:?}");
        }

        let mut msb = partner("Messstellenbetrieb GmbH");
        msb.zusatz_attribute = Some(vec![ZusatzAttribut {
            name: Some(STEUERNUMMER_ATTRIBUT.to_owned()),
            wert: Some(serde_json::Value::from("123/456/78901")),
            ..ZusatzAttribut::default()
        }]);
        let r = Rechnung {
            rechnungsersteller: Some(Box::new(msb)),
            rechnungsempfaenger: Some(Box::new(partner("ESA AG"))),
            ..Rechnung::default()
        };
        let m = ustg_14_4_maengel(&r);
        assert!(
            !m.iter()
                .any(|m| m.starts_with("Nr. 1") || m.starts_with("Nr. 2")),
            "{m:?}"
        );
    }
}
