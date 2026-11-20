//! The Kündigung beim Vorversorger — the new supplier terminating the
//! customer's old contract on their behalf.
//!
//! GPKE Teil 2 Kap. 1.2 (55016 → 55017/55018) and GeLi Gas Anlage Kap. 3.1
//! (44016 → 44017/44018). GPKE recommends it run first: „empfiehlt es sich, den
//! Use-Case „Kündigung" generell einem Use-Case „Lieferbeginn" vorzuschalten"
//! (Teil 2 Kap. 1.2, S. 8). So the contract's Anmeldung waits for the answer,
//! and the answer's Antwortcode decides what follows ([`klasse`],
//! [`reaktion`]).
//!
//! The Vollmacht is held, never sent: „im Regelfall auf den Versand von
//! Vollmachten zu verzichten und die Existenz der Vollmachten vertraglich
//! zuzusichern" (GPKE Teil 1 Kap. 5, S. 25; GeLi Gas Anlage Kap. 2.4). Its date
//! and the front end's reference are the proof.

use mako_pruefung::codes::{self, Cluster};
use serde::{Deserialize, Serialize};
use time::Date;

/// The customer's previous supplier, as the order or the contract states it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vorversorger {
    /// The previous supplier's MP-ID — the receiver of the Kündigung.
    pub mp_id: String,
    /// The customer's number there, stored as the front end states it. Only
    /// a 44016 has an element for it (`SG12 RFF+Z01`, Kann, UTILMD AHB Gas 1.2
    /// Kap. 5.3); the 55016 has none: „ist einzig die MaLo-ID der
    /// Marktlokation anzugeben" (GPKE Teil 2 Kap. 1.2.2 Nr. 1, S. 9).
    #[serde(default)]
    pub kundennummer: Option<String>,
    #[serde(default)]
    pub vollmacht: Option<Vollmacht>,
}

/// The customer's authorisation to terminate on their behalf.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vollmacht {
    pub erteilt_am: Date,
    /// The front end's reference or digest of the signed Vollmacht.
    pub referenz: String,
}

/// The request to terminate the old contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KuendigungBeimVorversorger {
    /// The requested end of the old contract — `SG4 DTM+93` „Ende zum".
    pub zum: Date,
}

/// The longest free text the target elements take: `RFF` DE 1154 is an..35.
pub const FREITEXT_MAX: usize = 35;

/// Why an intake request is refused, with its stable `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ablehnung {
    pub code: &'static str,
    pub message: String,
}

fn ablehnung(code: &'static str, message: impl Into<String>) -> Ablehnung {
    Ablehnung {
        code,
        message: message.into(),
    }
}

/// Hold an intake's previous-supplier facts to what the Kündigung needs.
///
/// `vertragsart` is the new contract's; `heute` the Berlin date.
///
/// # Errors
///
/// The first rule that refuses it.
pub fn pruefe(
    vorversorger: Option<&Vorversorger>,
    kuendigung: Option<&KuendigungBeimVorversorger>,
    vertragsart: &str,
    sparte: &str,
    heute: Date,
) -> Result<(), Ablehnung> {
    if let Some(v) = vorversorger {
        if rubo4e::identifiers::MarktpartnerId::new(v.mp_id.trim()).is_err() {
            return Err(ablehnung(
                "vorversorger_mp_id_ungueltig",
                format!("vorversorger.mp_id {:?} is not an MP-ID", v.mp_id),
            ));
        }
        if let Some(nr) = &v.kundennummer {
            freitext("vorversorger.kundennummer", nr, FREITEXT_MAX)?;
        }
        if let Some(vm) = &v.vollmacht {
            freitext("vorversorger.vollmacht.referenz", &vm.referenz, 256)?;
            if vm.erteilt_am > heute {
                return Err(ablehnung(
                    "vollmacht_in_zukunft",
                    format!(
                        "the Vollmacht is dated {} — after today ({heute})",
                        vm.erteilt_am
                    ),
                ));
            }
        }
    }
    let Some(kuendigung) = kuendigung else {
        return Ok(());
    };
    if vertragsart == "ERSATZVERSORGUNG" {
        return Err(ablehnung(
            "kuendigung_bei_ersatzversorgung",
            "an Ersatzversorgung ends without a Kündigung („keine Kündigung erforderlich \
             (vgl. § 38 Abs. 4 EnWG)\", GPKE Teil 2 Kap. 1.2, S. 7)",
        ));
    }
    if !matches!(sparte, "STROM" | "GAS") {
        return Err(ablehnung(
            "kuendigung_nur_strom_gas",
            "a Kündigung beim Vorversorger is a UTILMD 55016/44016: Strom or Gas only",
        ));
    }
    if vorversorger.and_then(|v| v.vollmacht.as_ref()).is_none() {
        return Err(ablehnung(
            "vorversorger_fehlt",
            "a Kündigung on the customer's behalf needs vorversorger.mp_id and the Vollmacht \
             (erteilt_am, referenz) — the LFN „besitzt die Vollmacht des Letztverbrauchers\" \
             (GPKE Teil 2 Kap. 1.2, S. 7)",
        ));
    }
    if kuendigung.zum < heute {
        return Err(ablehnung(
            "kuendigung_zum_vergangen",
            format!(
                "kuendigung_beim_vorversorger.zum {} lies before today ({heute})",
                kuendigung.zum
            ),
        ));
    }
    Ok(())
}

/// A free-text intake field is held to the element it lands in: bounded, and
/// without control characters — an EDIFACT separator is escaped by the
/// builder, a line break is never meant.
fn freitext(feld: &str, wert: &str, max: usize) -> Result<(), Ablehnung> {
    if wert.trim().is_empty() || wert.chars().count() > max || wert.chars().any(char::is_control) {
        return Err(ablehnung(
            "freitext_ungueltig",
            format!("{feld} must be 1 to {max} characters without control characters"),
        ));
    }
    Ok(())
}

/// Where the Kündigung beim Vorversorger stands, as `kuendigung_status`
/// stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KuendigungStatus {
    /// Sent or about to be; the Anmeldung waits.
    Ausstehend,
    /// The old supplier confirmed.
    Bestaetigt,
    /// The old supplier refused.
    Abgelehnt,
    /// No answer inside the old supplier's Frist.
    FristAbgelaufen,
    /// A held Anmeldung was released by a person.
    Freigegeben,
    /// A held component was withdrawn by a person.
    Zurueckgezogen,
}

impl KuendigungStatus {
    /// Every variant, in the order the schema lists them.
    pub const ALL: &'static [Self] = &[
        Self::Ausstehend,
        Self::Bestaetigt,
        Self::Abgelehnt,
        Self::FristAbgelaufen,
        Self::Freigegeben,
        Self::Zurueckgezogen,
    ];

    /// The column value.
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Ausstehend => "AUSSTEHEND",
            Self::Bestaetigt => "BESTAETIGT",
            Self::Abgelehnt => "ABGELEHNT",
            Self::FristAbgelaufen => "FRIST_ABGELAUFEN",
            Self::Freigegeben => "FREIGEGEBEN",
            Self::Zurueckgezogen => "ZURUECKGEZOGEN",
        }
    }

    /// Parse the column value.
    #[must_use]
    pub fn from_db(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.as_db() == s)
    }
}

/// What an answer to the Kündigung means for the Anmeldung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Klasse {
    /// The old supplier confirmed.
    Zustimmung,
    /// It confirmed for another date — „Zustimmung mit Terminänderung"
    /// (`E_3001` Z01, the date in `DTM+471`). The Anmeldung waits: the
    /// confirmed date is not read from the answer, and a Lieferbeginn
    /// before it would meet a bound contract.
    ZustimmungMitTerminaenderung,
    /// „Zum Kündigungstermin besteht kein Vertragsverhältnis mehr" and its
    /// kin: there is nothing to terminate.
    KeinVertrag,
    /// The old contract is still bound.
    Vertragsbindung,
    /// Date passed, Vollmacht not accepted, or Sonstiges: the Kündigung itself
    /// has to be fixed.
    Sonstige,
}

/// Strom `E_0614` (EBD 4.3 Kap. 6.2.1) refusals by class.
const STROM_KEIN_VERTRAG: &[&str] = &["A04", "A13", "A18"];
const STROM_VERTRAGSBINDUNG: &[&str] = &["A05", "A06", "A14", "A15"];
/// Gas `E_3001` (`G_0005` / `G_0006`) refusals by class.
const GAS_KEIN_VERTRAG: &[&str] = &["Z29"];
const GAS_VERTRAGSBINDUNG: &[&str] = &["Z12"];

/// The class of an answer, from the Codeliste it names and its code.
///
/// `None` when the code is not published for the Kündigung — an answer mako
/// cannot read is never guessed into a class.
#[must_use]
pub fn klasse(codeliste: &str, code: &str) -> Option<Klasse> {
    let gas = matches!(codeliste, "E_3001" | "G_0005" | "G_0006");
    let tree = if gas {
        codes::E_3001_CODES
    } else if codeliste == codes::EBD_KUENDIGUNG {
        codes::E_0614_CODES
    } else {
        return None;
    };
    let c = tree.iter().find(|c| c.code == code)?;
    Some(match c.cluster {
        Cluster::Zustimmung if gas && code == "Z01" => Klasse::ZustimmungMitTerminaenderung,
        Cluster::Zustimmung => Klasse::Zustimmung,
        Cluster::Ablehnung => {
            let (kein, bindung) = if gas {
                (GAS_KEIN_VERTRAG, GAS_VERTRAGSBINDUNG)
            } else {
                (STROM_KEIN_VERTRAG, STROM_VERTRAGSBINDUNG)
            };
            if kein.contains(&code) {
                Klasse::KeinVertrag
            } else if bindung.contains(&code) {
                Klasse::Vertragsbindung
            } else {
                Klasse::Sonstige
            }
        }
        _ => return None,
    })
}

pub use crate::config::NachKuendigungsablehnung;

/// Whether the Anmeldung goes out after the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reaktion {
    Angemeldet,
    Gehalten,
}

/// The reaction to an answer of `klasse` under the tenant's policy.
#[must_use]
pub const fn reaktion(klasse: Klasse, policy: NachKuendigungsablehnung) -> Reaktion {
    match klasse {
        Klasse::Zustimmung | Klasse::KeinVertrag => Reaktion::Angemeldet,
        Klasse::Vertragsbindung => match policy {
            NachKuendigungsablehnung::Anmelden => Reaktion::Angemeldet,
            NachKuendigungsablehnung::Halten => Reaktion::Gehalten,
        },
        Klasse::Sonstige | Klasse::ZustimmungMitTerminaenderung => Reaktion::Gehalten,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// Every code either tree publishes has a class: a code the BDEW adds
    /// fails here instead of falling silently into `Sonstige` without review.
    #[test]
    fn every_published_code_has_a_class() {
        for (liste, tree) in [
            ("E_0614", codes::E_0614_CODES),
            ("E_3001", codes::E_3001_CODES),
        ] {
            for c in tree {
                assert!(klasse(liste, c.code).is_some(), "{liste} {}", c.code);
            }
            let named: Vec<&str> = if liste == "E_3001" {
                [GAS_KEIN_VERTRAG, GAS_VERTRAGSBINDUNG].concat()
            } else {
                [STROM_KEIN_VERTRAG, STROM_VERTRAGSBINDUNG].concat()
            };
            for code in named {
                let c = tree.iter().find(|c| c.code == code);
                assert!(
                    c.is_some_and(|c| c.cluster == Cluster::Ablehnung),
                    "{liste} {code} is not a published Ablehnung"
                );
            }
        }
    }

    #[test]
    fn the_classes_follow_the_wording() {
        assert_eq!(klasse("E_0614", "A09"), Some(Klasse::Zustimmung));
        assert_eq!(klasse("E_0614", "A04"), Some(Klasse::KeinVertrag));
        assert_eq!(klasse("E_0614", "A06"), Some(Klasse::Vertragsbindung));
        assert_eq!(klasse("E_0614", "A08"), Some(Klasse::Sonstige));
        assert_eq!(klasse("G_0006", "Z12"), Some(Klasse::Vertragsbindung));
        assert_eq!(
            klasse("E_3001", "Z01"),
            Some(Klasse::ZustimmungMitTerminaenderung)
        );
        assert_eq!(klasse("E_3001", "E15"), Some(Klasse::Zustimmung));
        assert_eq!(klasse("E_0614", "ZZZ"), None);
        assert_eq!(klasse("E_0622", "A04"), None);
    }

    #[test]
    fn only_vertragsbindung_follows_the_policy() {
        use NachKuendigungsablehnung::{Anmelden, Halten};
        assert_eq!(reaktion(Klasse::KeinVertrag, Halten), Reaktion::Angemeldet);
        assert_eq!(
            reaktion(Klasse::Vertragsbindung, Halten),
            Reaktion::Gehalten
        );
        assert_eq!(
            reaktion(Klasse::Vertragsbindung, Anmelden),
            Reaktion::Angemeldet
        );
        assert_eq!(reaktion(Klasse::Sonstige, Anmelden), Reaktion::Gehalten);
    }

    fn vv() -> Vorversorger {
        Vorversorger {
            mp_id: "9900357000004".to_owned(),
            kundennummer: Some("K-4711".to_owned()),
            vollmacht: Some(Vollmacht {
                erteilt_am: date!(2026 - 11 - 20),
                referenz: "sha256:ab".to_owned(),
            }),
        }
    }

    #[test]
    fn a_kuendigung_needs_the_vollmacht() {
        let heute = date!(2026 - 11 - 20);
        let k = KuendigungBeimVorversorger {
            zum: date!(2027 - 01 - 31),
        };
        assert!(pruefe(Some(&vv()), Some(&k), "SONDERVERTRAG", "STROM", heute).is_ok());
        let ohne = Vorversorger {
            vollmacht: None,
            ..vv()
        };
        let e = pruefe(Some(&ohne), Some(&k), "SONDERVERTRAG", "STROM", heute).unwrap_err();
        assert_eq!(e.code, "vorversorger_fehlt");
        let e = pruefe(None, Some(&k), "SONDERVERTRAG", "STROM", heute).unwrap_err();
        assert_eq!(e.code, "vorversorger_fehlt");
    }

    #[test]
    fn intake_refusals_are_named() {
        let heute = date!(2026 - 11 - 20);
        let k = KuendigungBeimVorversorger {
            zum: date!(2027 - 01 - 31),
        };
        let code = |v: &Vorversorger, art: &str| {
            pruefe(Some(v), Some(&k), art, "STROM", heute).map_err(|e| e.code)
        };
        assert_eq!(
            code(&vv(), "ERSATZVERSORGUNG"),
            Err("kuendigung_bei_ersatzversorgung")
        );
        let mut v = vv();
        v.mp_id = "12".to_owned();
        assert_eq!(
            code(&v, "SONDERVERTRAG"),
            Err("vorversorger_mp_id_ungueltig")
        );
        let mut v = vv();
        v.kundennummer = Some("x".repeat(36));
        assert_eq!(code(&v, "SONDERVERTRAG"), Err("freitext_ungueltig"));
        let mut v = vv();
        v.kundennummer = Some("K\n1".to_owned());
        assert_eq!(code(&v, "SONDERVERTRAG"), Err("freitext_ungueltig"));
        let mut v = vv();
        v.vollmacht.as_mut().unwrap().erteilt_am = date!(2026 - 11 - 21);
        assert_eq!(code(&v, "SONDERVERTRAG"), Err("vollmacht_in_zukunft"));
    }
}
