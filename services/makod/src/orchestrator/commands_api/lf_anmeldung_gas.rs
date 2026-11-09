//! The Lieferant's contract facts a GeLi Gas Anmeldung NN **44001** carries,
//! set down as the segments UTILMD AHB Gas 1.2 Kap. 5.6 (S. 43–66) lists them.
//!
//! The body is [`mako_markt::commands::GeliLieferbeginnAnmelden`]; every field
//! is Muss in the column and none is defaulted here:
//!
//! | Fact | Segment | AHB S. |
//! |---|---|---|
//! | Abrechnungsintervall des LF | `SG6 RFF+Z18` (DE 1153 alone) `DTM+Z20:<n>:802` | 48 |
//! | Gruppenzuordnung (nach EnWG) | `SG8 SEQ+Z01` `SG10 CCI+++Z15`/`Z18` | 51 |
//! | Netznutzung | `SG10 CCI+++Z88`, `CAV+Z74:::Z08`/`Z09`, `CAV+Z73:::Z10`/`Z11` | 52 |
//! | Gemeinderabatt | `SG8 SEQ+Z12` `SG9 QTY+Z16:<p>:P1` | 55 |
//! | Kunde des Lieferanten | `SG12 NAD+Z09` | 63 |
//! | Korrespondenzanschrift | `SG12 NAD+Z04` | 64 |
//! | Name und Adresse für die Ablesekarte | `SG12 NAD+Z05` | 65 |
//!
//! The Bilanzkreis (`SG10 CCI+Z19`, S. 50), the Transaktionsgrund (`SG4 STS+7`,
//! S. 45) and the `SG4 IMD++Z36+Z12` Identifikationslogik (S. 44) are the
//! renderer's from the payload's own fields.

use mako_engine::payload::Stammdaten;
use mako_markt::commands::GeliLieferbeginnAnmelden;

use super::eog_stammdaten::{anschrift_elemente, block, c080, nad, name_of, seg};

/// The `SG6`, `SG8` and `SG12` segments of a 44001.
///
/// # Errors
///
/// Every defect, named with its segment: the body's own
/// ([`GeliLieferbeginnAnmelden::maengel`]) and a party whose name or address
/// does not fill its `NAD`.
pub(super) fn stammdaten_44001(a: &GeliLieferbeginnAnmelden) -> Result<Stammdaten, Vec<String>> {
    let maengel = a.maengel();
    if !maengel.is_empty() {
        return Err(maengel);
    }
    let mut out = Stammdaten::default();

    // `SG6 RFF+Z18` — the 44001 column lists DE 1153 alone, the ID der
    // Marktlokation only on the Bestätigung (S. 48) — with `DTM+Z20` in it.
    out.sg6.push(seg("RFF", vec![vec!["Z18"]]));
    out.sg6.push(seg(
        "DTM",
        vec![vec![
            "Z20",
            &a.abrechnungsintervall_monate.to_string(),
            "802",
        ]],
    ));

    // `SG8 SEQ+Z01` — the renderer heads it with `CCI+Z19` from the payload's
    // `bilanzkreis`; the Gruppenzuordnung and the Netznutzung follow in the
    // MIG's order.
    out.bloecke.push(block(
        "Z01",
        vec![
            seg(
                "CCI",
                vec![
                    vec![],
                    vec![],
                    vec![if a.haushaltskunde { "Z15" } else { "Z18" }],
                ],
            ),
            seg("CCI", vec![vec![], vec![], vec!["Z88"]]),
            seg(
                "CAV",
                vec![vec!["Z74", "", "", a.netznutzungsvertrag.utilmd_code()]],
            ),
            seg(
                "CAV",
                vec![vec!["Z73", "", "", a.netznutzung_zahler.utilmd_code()]],
            ),
        ],
    ));

    // `SG8 SEQ+Z12` Gemeinderabatt, in Prozent.
    out.bloecke.push(block(
        "Z12",
        vec![seg(
            "QTY",
            vec![vec![
                "Z16",
                &a.gemeinderabatt_prozent.normalize().to_string(),
                "P1",
            ]],
        )],
    ));

    // `SG12` — the Kunde by name, his Korrespondenzanschrift beside the name,
    // and the Ablesekarte's addressee.
    let mut fehlend = Vec::new();
    let mut partei = |rolle: &str, k: &mako_markt::utilmd_stammdaten::Kunde| {
        let name = name_of(k);
        let adresse = k.anschrift.as_ref().and_then(anschrift_elemente);
        match (name, adresse) {
            (Some((parts, format)), Some(adresse)) => Some((c080(&parts, format), adresse)),
            _ => {
                fehlend.push(format!("{rolle}: name and anschrift do not fill the NAD"));
                None
            }
        }
    };
    let kunde = partei("kunde (SG12 NAD+Z09/Z04)", &a.kunde);
    let ablesekarte = partei("ablesekarte (SG12 NAD+Z05)", &a.ablesekarte);
    if !fehlend.is_empty() {
        return Err(fehlend);
    }
    if let (Some((name, adresse)), Some((ak_name, ak_adresse))) = (kunde, ablesekarte) {
        use edi_energy::utilmd_codes::nad as q;
        out.sg12
            .push(nad(q::KUNDE_DES_LF, vec![], name.clone(), None));
        out.sg12.push(nad(
            q::KORRESPONDENZANSCHRIFT_KUNDE,
            vec![],
            name,
            Some(&adresse),
        ));
        out.sg12
            .push(nad(q::ABLESEKARTE, vec![], ak_name, Some(&ak_adresse)));
    }
    Ok(out)
}
