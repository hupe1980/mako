//! **Anmeldung E/G** — the Grund-/Ersatzversorger answers an assignment.
//!
//! | Sparte | Inbound | EBD | Answers |
//! |---|---|---|---|
//! | Strom | 55013 | `E_0615` „Anmeldung E/G prüfen" | 55014 / 55015 |
//! | Gas | 44013 | `E_3008` (`G_0013` / `G_0014`) | 44014 / 44015 |
//!
//! This is the supplier's **Anmeldung** tree, and its only one. A supplier
//! *sends* the ordinary Anmeldung (55001 / 44001) and the Netzbetreiber checks
//! it with `E_0622`; the one Anmeldung a supplier is asked to check is the one
//! the NB assigns to it under § 36 / § 38 EnWG.
//!
//! Refusing is narrowly bounded: the Grundversorger of a Netzgebiet has a
//! *statutory* duty to supply, so `E_0615` only admits „not my Netzgebiet",
//! „Frist", „Doppelmeldung" and „no statutory case at all". A supplier that is
//! the Grundversorger and receives an in-area assignment inside the Frist has
//! nothing to refuse with.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 6.7.1
//! and 13.7.1.

use crate::codes::{E_0615_CODES, E_3008_CODES, EBD_ANMELDUNG_EOG, EBD_ANMELDUNG_EOG_GAS};
use crate::lf::types::{Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage};

macro_rules! code {
    ($list:expr, $ebd:expr, $code:literal, $schritt:literal, $termin:expr) => {{
        let entry = $list
            .iter()
            .find(|c| c.code == $code)
            .unwrap_or_else(|| panic!("{} does not publish {}", $ebd, $code));
        return LfEntscheidung::antwort(entry, $schritt, $termin, None);
    }};
}

/// What the E/G supplier knows about its own statutory duty for this MaLo.
///
/// Separate from [`LfVertragslage`] because these are questions about the
/// *Netzgebiet* and the statute, not about a contract: the answers come from
/// the Grundversorger registry, not from `vertragd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EogZustaendigkeit {
    /// „Befindet sich die Marktlokation im Grundversorgungsgebiet des
    /// Empfängers, oder besteht eine vertragliche Vereinbarung zur
    /// Ersatzbelieferung?" (Prüfschritt 20).
    pub zustaendig: Option<bool>,
    /// „Wurden die Fristen zur Anmeldung eingehalten?" (Prüfschritt 30).
    pub frist_eingehalten: Option<bool>,
    /// „Besteht eine gesetzliche Pflicht zur Grund- oder Ersatzversorgung …?"
    /// (Prüfschritt 50).
    pub gesetzliche_pflicht: Option<bool>,
}

impl EogZustaendigkeit {
    fn bekannt(value: Option<bool>) -> Bekannt {
        Bekannt::from_option(value)
    }

    /// Prüfschritt 20, with the supply state as corroboration.
    ///
    /// Being flagged Grundversorger for this MaLo is positive evidence of
    /// Zuständigkeit. *Not* being flagged is **not** evidence against it — the
    /// flag is only set once a supply exists, and an EoG-Anmeldung arrives
    /// precisely when none does. Reading `false` as „nicht zuständig" would
    /// refuse a statutory supply duty on the absence of a record.
    fn resolve(self, lage: &LfVertragslage) -> Bekannt {
        match Self::bekannt(self.zustaendig) {
            Bekannt::Unbekannt if lage.ist_grundversorger => Bekannt::Ja,
            other => other,
        }
    }
}

/// Walk `E_0615` for an inbound **55013** Anmeldung / Zuordnung EOG.
///
/// # Panics
///
/// If the walk names a code [`E_0615_CODES`] does not publish — a defect in
/// this module, covered by `every_landing_resolves_to_a_published_code`.
#[must_use]
pub fn pruefe_anmeldung_eog(
    anfrage: &LfAnfrage,
    lage: &LfVertragslage,
    zustaendigkeit: &EogZustaendigkeit,
) -> LfEntscheidung {
    let list = E_0615_CODES;
    let ebd = EBD_ANMELDUNG_EOG;
    let termin = anfrage.termin;

    // Prüfschritt 20 — Zuständigkeit.
    let zustaendig = zustaendigkeit.resolve(lage);
    match zustaendig {
        Bekannt::Nein => code!(list, ebd, "A02", 20, termin),
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                20,
                format!(
                    "MaLo {}: unbekannt, ob sie im Grundversorgungsgebiet dieses Lieferanten \
                     liegt oder eine vertragliche Ersatzbelieferung vereinbart ist \
                     (E_0615 Prüfschritt 20 → A02).",
                    anfrage.malo_id
                ),
            );
        }
        Bekannt::Ja => {}
    }

    // Prüfschritt 30 — Anmeldefrist.
    match EogZustaendigkeit::bekannt(zustaendigkeit.frist_eingehalten) {
        Bekannt::Nein => code!(list, ebd, "A03", 30, termin),
        Bekannt::Unbekannt | Bekannt::Ja => {}
    }

    // Prüfschritt 40 — Doppelmeldung: schon einmal zum selben Termin bestätigt.
    if lage.beliefert
        && let (Some(beginn), Some(t)) = (lage.bestaetigtes_zuordnungsende, termin)
        && beginn == t
    {
        code!(list, ebd, "A04", 40, termin);
    }

    // Prüfschritt 50 — gesetzliche Pflicht zur Grund-/Ersatzversorgung.
    match EogZustaendigkeit::bekannt(zustaendigkeit.gesetzliche_pflicht) {
        Bekannt::Nein => code!(list, ebd, "A05", 50, termin),
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                50,
                format!(
                    "MaLo {}: unbekannt, ob eine gesetzliche Pflicht zur Grund- oder \
                     Ersatzversorgung besteht (E_0615 Prüfschritt 50 → A05).",
                    anfrage.malo_id
                ),
            );
        }
        Bekannt::Ja => {}
    }

    // Prüfschritt 90 — Zustimmung.
    code!(list, ebd, "A09", 90, termin)
}

/// Walk the Gas Codeliste `E_3008` for an inbound **44013**.
///
/// # Panics
///
/// If the walk names a code [`E_3008_CODES`] does not publish.
#[must_use]
pub fn pruefe_anmeldung_eog_gas(
    anfrage: &LfAnfrage,
    lage: &LfVertragslage,
    zustaendigkeit: &EogZustaendigkeit,
) -> LfEntscheidung {
    let list = E_3008_CODES;
    let ebd = EBD_ANMELDUNG_EOG_GAS;
    let termin = anfrage.termin;

    let zustaendig = zustaendigkeit.resolve(lage);
    if zustaendig == Bekannt::Nein {
        // Gas has no „keine Zuständigkeit" code; the catch-all carries the
        // reason in `FTX+ACB`, which is exactly what `E14` requires.
        let entry = list
            .iter()
            .find(|c| c.code == "E14")
            .expect("E_3008 publishes E14");
        return LfEntscheidung::antwort(
            entry,
            20,
            termin,
            Some(format!(
                "Keine Zuständigkeit: die Marktlokation {} liegt nicht im \
                 Grundversorgungsgebiet dieses Lieferanten.",
                anfrage.malo_id
            )),
        );
    }
    if zustaendig == Bekannt::Unbekannt {
        return LfEntscheidung::eskalation(
            20,
            format!(
                "Gas-EoG für MaLo {}: unbekannt, ob sie im Grundversorgungsgebiet dieses \
                 Lieferanten liegt.",
                anfrage.malo_id
            ),
        );
    }

    if EogZustaendigkeit::bekannt(zustaendigkeit.frist_eingehalten) == Bekannt::Nein {
        code!(list, ebd, "E17", 30, termin);
    }

    code!(list, ebd, "E15", 90, termin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lf::types::Lokationsart;
    use time::macros::{date, datetime};
    use uuid::Uuid;

    fn anfrage(pid: u32) -> LfAnfrage {
        LfAnfrage {
            pid,
            process_id: Uuid::nil(),
            malo_id: "51238696012".to_owned(),
            vorgangsnummer: None,
            absender_mp_id: "9900357000004".to_owned(),
            empfaenger_mp_id: "9900000000001".to_owned(),
            lokationsart: Some(Lokationsart::VerbrauchendeMalo),
            transaktionsgrund: Some("Z36".to_owned()),
            termin: Some(date!(2026 - 09 - 01)),
            terminart: crate::lf::types::Terminart::Fix,
            uet_lieferanmeldung: None,
            eingang: datetime!(2026-08-20 09:00 UTC),
        }
    }

    fn grundversorger() -> LfVertragslage {
        LfVertragslage {
            ist_grundversorger: true,
            ..LfVertragslage::default()
        }
    }

    fn alles_bekannt() -> EogZustaendigkeit {
        EogZustaendigkeit {
            zustaendig: Some(true),
            frist_eingehalten: Some(true),
            gesetzliche_pflicht: Some(true),
        }
    }

    /// The Grundversorger inside its own area, inside the Frist, with a
    /// statutory duty: `A09` Zustimmung. There is nothing to refuse with.
    #[test]
    fn the_grundversorger_in_area_agrees() {
        let d = pruefe_anmeldung_eog(&anfrage(55_013), &grundversorger(), &alles_bekannt());
        assert_eq!(d.as_antwort().expect("answer").code, "A09");
        assert!(d.ist_zustimmung());
    }

    /// Prüfschritt 20 → `A02` „Keine Zuständigkeit".
    #[test]
    fn out_of_area_is_a02() {
        let z = EogZustaendigkeit {
            zustaendig: Some(false),
            ..alles_bekannt()
        };
        let d = pruefe_anmeldung_eog(&anfrage(55_013), &LfVertragslage::default(), &z);
        assert_eq!(d.as_antwort().expect("answer").code, "A02");
        assert!(!d.ist_zustimmung());
    }

    /// Prüfschritt 50 → `A05`: in area, but no statutory Grund-/Ersatzversorgung
    /// case at all.
    #[test]
    fn no_statutory_case_is_a05() {
        let z = EogZustaendigkeit {
            gesetzliche_pflicht: Some(false),
            ..alles_bekannt()
        };
        assert_eq!(
            pruefe_anmeldung_eog(&anfrage(55_013), &grundversorger(), &z)
                .as_antwort()
                .expect("answer")
                .code,
            "A05"
        );
    }

    /// An unresolved Zuständigkeit escalates: a supplier must not decline a
    /// statutory supply duty on a guess, nor accept one.
    ///
    /// Specifically, an *unset* `ist_grundversorger` must not be read as
    /// „nicht zuständig" — the flag is only set once a supply exists, and an
    /// EoG-Anmeldung arrives precisely when none does.
    #[test]
    fn an_unknown_zustaendigkeit_escalates() {
        let z = EogZustaendigkeit {
            zustaendig: None,
            ..alles_bekannt()
        };
        let d = pruefe_anmeldung_eog(&anfrage(55_013), &LfVertragslage::default(), &z);
        assert!(d.ist_eskalation(), "{d:?}");
    }

    /// Being flagged Grundversorger *is* positive evidence, so it resolves
    /// Prüfschritt 20 without an explicit answer.
    #[test]
    fn a_flagged_grundversorger_resolves_zustaendigkeit() {
        let z = EogZustaendigkeit {
            zustaendig: None,
            ..alles_bekannt()
        };
        let d = pruefe_anmeldung_eog(&anfrage(55_013), &grundversorger(), &z);
        assert_eq!(d.as_antwort().expect("answer").code, "A09");
    }

    /// Gas answers from `G_0013`/`G_0014`: `E15`, not the Strom `A09`.
    #[test]
    fn the_gas_eog_uses_gas_codes() {
        let d = pruefe_anmeldung_eog_gas(&anfrage(44_013), &grundversorger(), &alles_bekannt());
        let a = d.as_antwort().expect("answer");
        assert_eq!(a.code, "E15");
        assert!(a.ebd.is_none(), "the Gas MIG names no Codeliste in DE 1131");
    }

    /// Gas has no „keine Zuständigkeit" code, so the refusal rides the catch-all
    /// `E14` — which the BDEW requires to carry a written Erläuterung.
    #[test]
    fn the_gas_refusal_carries_its_mandatory_erlaeuterung() {
        let z = EogZustaendigkeit {
            zustaendig: Some(false),
            ..alles_bekannt()
        };
        let d = pruefe_anmeldung_eog_gas(&anfrage(44_013), &LfVertragslage::default(), &z);
        let a = d.as_antwort().expect("answer");
        assert_eq!(a.code, "E14");
        assert!(a.bemerkung.is_some());
    }
}
