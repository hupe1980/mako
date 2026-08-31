//! The UTILMD qualifiers Modell 2 rides on, taken from the AHB rather than
//! from prose.
//!
//! Every constant here was read out of **UTILMD AHB Strom 2.2** (konsolidierte
//! Lesefassung mit Fehlerkorrekturen, 29.06.2026), Kapitel 11
//! „Anwendungsübersicht Ladevorgangscharfe bilanzielle Energiemengenzuordnung"
//! and Kapitel 4's Stammdaten tables. Nothing is inferred from a process
//! document: a qualifier that is not in the AHB is not in this module.
//!
//! # Read `SG10 CCI` by **both** data elements
//!
//! `CCI` carries a *Klassentyp* in DE 7059 and a *Merkmal* in DE 7037, and the
//! DE 7037 code space is reused across Klassentypen. Reading 7037 alone is the
//! defect this module exists to prevent — `ZE9` is „Modell 1" under
//! [`KLASSENTYP_ABWICKLUNGSMODELL`] and „Quartalsweise" elsewhere in the same
//! AHB. Ask [`abwicklungsmodell_from_cci`], which requires the pair.

/// `SG10 CCI` DE 7059 — Klassentyp **Abwicklungsmodell**.
pub const KLASSENTYP_ABWICKLUNGSMODELL: &str = "ZA2";

/// `SG10 CCI` DE 7037 under [`KLASSENTYP_ABWICKLUNGSMODELL`] — Modell 1,
/// „Bilanzierung an der Marktlokation".
pub const MERKMAL_MODELL_1: &str = "ZE9";

/// `SG10 CCI` DE 7037 under [`KLASSENTYP_ABWICKLUNGSMODELL`] — Modell 2,
/// „Bilanzierung im Bilanzierungsgebiet (BG) des LPB".
pub const MERKMAL_MODELL_2: &str = "ZF0";

/// `SG10 CCI` DE 7059 — Klassentyp **Bilanzierungsgebiet**.
///
/// On a 55238 the accompanying DE 7037 carries the BG itself, and AHB Bedingung
/// `[664]` fixes whose: „Es ist das BG des NB (LPB) anzugeben".
pub const KLASSENTYP_BILANZIERUNGSGEBIET: &str = "Z20";

/// `SG10 CCI` DE 7059 — Klassentyp **Stromverbrauchsart**.
pub const KLASSENTYP_STROMVERBRAUCHSART: &str = "Z17";

/// `SG10 CAV` DE 7111 — Verbrauchsart **E-Mobilität**.
///
/// This is the Verbrauchsart. [`ART_LADESAEULE`] is *not*: it answers a second,
/// narrower question in a second `CAV` of the same `SG10`.
pub const VERBRAUCHSART_E_MOBILITAET: &str = "ZE5";

/// `SG10 CAV` DE 7111 — Art der E-Mobilität: **Wallbox**.
pub const ART_WALLBOX: &str = "ZE6";

/// `SG10 CAV` DE 7111 — Art der E-Mobilität: **E-Mobilitätsladesäule**.
///
/// AHB Bedingung `[95]` makes the „Art der E-Mobilität" `CAV` a Muss only „wenn
/// in derselben SG10 das `CCI+Z17` (Stromverbrauchsart) `CAV+ZE5`
/// (E-Mobilität) vorhanden" — so it is a refinement of
/// [`VERBRAUCHSART_E_MOBILITAET`], never a substitute for it. A message that
/// carries `Z87` without `ZE5` states an Art with no Verbrauchsart and the
/// receiving AHB layer refuses it.
pub const ART_LADESAEULE: &str = "Z87";

/// `SG10 CAV` DE 7111 — Art der E-Mobilität: **Ladepark**.
pub const ART_LADEPARK: &str = "ZE7";

/// `SG5 LOC` DE 3227 — Marktlokation. DE 3225 carries the eleven-digit MaLo-ID.
pub const LOC_MARKTLOKATION: &str = "Z16";

/// `SG5 LOC` DE 3227 — MaBiS-Zählpunkt. DE 3225 carries a Zählpunktbezeichnung.
///
/// On a 55239 the VNB returns this beside [`LOC_MARKTLOKATION`]: AHB
/// Bedingung `[663]` says „Es ist die ID der Marktlokation und die ZPB des ZP der
/// NGZ anzugeben".
pub const LOC_MABIS_ZAEHLPUNKT: &str = "Z15";

/// `SG4 IDE` DE 7495 — Transaktion. DE 7402 carries the Vorgangsnummer.
pub const IDE_TRANSAKTION: &str = "24";

/// `SG4 DTM` DE 2005 — Datum Vertragsbeginn (the Modellwechseltermin).
pub const DTM_VERTRAGSBEGINN: &str = "92";

/// `SG4 DTM` DE 2005 — Bilanzierungsbeginn.
///
/// AHB Bedingung `[317]`: „Es ist derselbe Wert wie im DE2380 von `DTM+92`
/// (Datum Vertragsbeginn) einzutragen" — the two dates are one date on the
/// wire, and [`crate::uebergabestelle::Modellwechsel`] keeps them one in the
/// domain.
pub const DTM_BILANZIERUNGSBEGINN: &str = "158";

/// `SG4 STS` DE 9015 — Transaktionsgrund.
pub const STS_TRANSAKTIONSGRUND: &str = "7";

/// `SG4 STS` DE 9013 under [`STS_TRANSAKTIONSGRUND`] — Wechsel.
///
/// Both directions of the Modellwechsel carry it: an Anmeldung in Modell 2 and
/// an Abmeldung out of it are the same Transaktionsgrund, and what separates
/// them is the Prüfidentifikator.
pub const TRANSAKTIONSGRUND_WECHSEL: &str = "E03";

/// `SG4 STS` DE 9015 — Status der Antwort. DE 9013 carries the Antwortcode and
/// DE 1131 the EBD that publishes it.
pub const STS_STATUS_DER_ANTWORT: &str = "E01";

/// `SG6 RFF` DE 1153 — Prüfidentifikator. DE 1154 carries the PID itself.
pub const RFF_PRUEFIDENTIFIKATOR: &str = "Z13";

/// `SG6 RFF` DE 1153 — Transaktions-Referenznummer, echoing the request's
/// Vorgangsnummer on an answer.
pub const RFF_TRANSAKTIONSREFERENZ: &str = "TN";

/// `SG8 SEQ` DE 1229 — Daten der Marktlokation.
pub const SEQ_DATEN_MARKTLOKATION: &str = "Z01";

/// `SG8 SEQ` DE 1229 — OBIS-Daten der Marktlokation.
pub const SEQ_OBIS_DATEN: &str = "Z02";

/// `SG8 RFF` DE 1153 — Referenz auf die ID der Marktlokation.
pub const RFF_MARKTLOKATION: &str = "Z18";

/// `BGM` DE 1001 — Anmeldungen. All six Modellwechsel-PIDs carry it.
pub const BGM_ANMELDUNGEN: &str = "E01";

/// Which Abwicklungsmodell a `SG10 CCI` states, read from **both** data
/// elements.
///
/// Returns `None` when the Klassentyp is not
/// [`KLASSENTYP_ABWICKLUNGSMODELL`] — including when DE 7037 is a code this
/// module knows, because the same Merkmal means something else under another
/// Klassentyp.
#[must_use]
pub fn abwicklungsmodell_from_cci(
    klassentyp: &str,
    merkmal: &str,
) -> Option<crate::uebergabestelle::Abwicklungsmodell> {
    use crate::uebergabestelle::Abwicklungsmodell;
    if klassentyp != KLASSENTYP_ABWICKLUNGSMODELL {
        return None;
    }
    Some(match merkmal {
        MERKMAL_MODELL_1 => Abwicklungsmodell::Modell1,
        MERKMAL_MODELL_2 => Abwicklungsmodell::Modell2,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uebergabestelle::Abwicklungsmodell;

    #[test]
    fn the_pair_resolves_the_model() {
        assert_eq!(
            abwicklungsmodell_from_cci(KLASSENTYP_ABWICKLUNGSMODELL, MERKMAL_MODELL_2),
            Some(Abwicklungsmodell::Modell2)
        );
        assert_eq!(
            abwicklungsmodell_from_cci(KLASSENTYP_ABWICKLUNGSMODELL, MERKMAL_MODELL_1),
            Some(Abwicklungsmodell::Modell1)
        );
    }

    /// `ZE9` under another Klassentyp is „Quartalsweise", not „Modell 1".
    #[test]
    fn the_merkmal_alone_decides_nothing() {
        assert_eq!(
            abwicklungsmodell_from_cci(KLASSENTYP_STROMVERBRAUCHSART, MERKMAL_MODELL_1),
            None
        );
        assert_eq!(
            abwicklungsmodell_from_cci(KLASSENTYP_BILANZIERUNGSGEBIET, MERKMAL_MODELL_2),
            None
        );
    }

    /// The Verbrauchsart and the Art der E-Mobilität are different codes in
    /// different `CAV`s, and conflating them produces an invalid message.
    #[test]
    fn the_verbrauchsart_is_ze5_and_the_ladesaeule_is_z87() {
        assert_eq!(VERBRAUCHSART_E_MOBILITAET, "ZE5");
        assert_eq!(ART_LADESAEULE, "Z87");
        assert_ne!(VERBRAUCHSART_E_MOBILITAET, ART_LADESAEULE);
    }
}
