//! The NB's answer to a Geschäftsdatenanfrage Stammdaten Strom, built from
//! the Lokationsbündel marktd holds and set down segment by segment as the
//! AHB column of its Prüfidentifikator lists it.
//!
//! The answer to an ORDERS 17132 — UTILMD **55035** (verbrauchende
//! Marktlokation, to a LF), **55095** (erzeugende, to a LF), **55060** (to a
//! MSB) — transmits the Lokationsbündel the Marktlokation belongs to (UTILMD
//! AHB Strom 2.2, konsolidierte Lesefassung 29.06.2026 — the edition
//! `profiles/sources.json` binds — Kap. 9.5, S. 599–665): every object in `SG5 LOC` (S.
//! 601–604), the Lokationsbündelstruktur in `SG8 SEQ+ZD5` (S. 605), each
//! object's position in it in `SEQ+ZD6` (S. 607), then the „Informative
//! Daten" of the Marktlokation (`SEQ+Z98`, S. 615), its
//! Netznutzungsabrechnungsdaten (`SEQ+ZE1`, S. 622), its OBIS-Daten
//! (`SEQ+ZE3`, S. 630), its technische Einrichtungen (`SEQ+ZH9`, S. 640), each
//! Messlokation (`SEQ+ZF3`, S. 651), its Zähleinrichtungen (`SEQ+ZF5`, S. 654)
//! and their OBIS-Daten (`SEQ+ZF6`, S. 656), the Profildaten (`SEQ+ZG1`, S.
//! 662) and the Marktlokationsanschrift (`SG12 NAD+Z63`, S. 664).
//!
//! Two reads at the boundary supply it: the Marktlokation's UTILMD Stammdaten
//! (`eog_stammdaten::lesen`) and its Lokationsbündel ([`lesen`]), both on the
//! Stichtag. **Every fact comes from marktd, or the command is refused naming
//! it**; nothing is defaulted. Two facts follow from the process itself:
//!
//! - the Transaktionsgrundergänzung `ZW4` „Verbrauchende Marktlokation" of a
//!   55035, from the Marktlokation's Energierichtung `AUSSP` — which is also
//!   what makes 55035 and not 55095 the answer;
//! - the `SG9 QTY` Zu-/Abschlag code, from the sign of the position's
//!   percentage: `Z34` Zuschlag above 0, `Z35` Abschlag below, `Z37` „Kein
//!   Zu-/Abschlag" at 0 with the value `0` (Bedingungen `[926]`, `[172]`).
//!
//! What this module does not assemble yet is refused by name too: the
//! Netzlokation, Technische and Steuerbare Ressource groups (`SEQ+ZD7`,
//! `ZF0`, the SR data — Muss `[294]`, `[296]`, `[205]`), ruhende and further
//! Marktlokationen of the bündel (`LOC+Z22`), the references of a flexible
//! object (`RFF+Z34`/`Z35`), an intelligentes Messsystem's Smartmeter-Gateway
//! (`SEQ+ZG0`), a TLP Marktlokation, more than one Energiemenge-OBIS (the
//! Zählzeitdefinition of `[386]`), and the groups only 55095 and 55060 carry.

use mako_engine::payload::{Stammdaten, StammdatenLokation};
use mako_markt::domain::Lokationstyp;
use mako_markt::lokationsbuendel::{BuendelAngaben, MaloLokationsbuendel, verwendungszweck};
use mako_markt::utilmd_stammdaten::{MaloUtilmdStammdaten, MesslokationStammdaten};
use rubo4e::current::{
    Energierichtung, Netzebene, Profiltyp, Prognosegrundlage, Zaehlertyp, ZaehlertypSpezifikation,
};
use rubo4e::lokationsbuendel::{Flexibilitaet, Lokationsbuendelstruktur, Objekttyp};

use super::eog_stammdaten::{
    Fehlend, anschrift_elemente, block, gemeinsamer_msb, messtechnische_einordnung, msb_codes, nad,
    seg, wire,
};
use super::{CommandsApiState, DispatchError};

/// Read the Marktlokation's Lokationsbündel on `stichtag` from marktd.
///
/// # Errors
///
/// [`DispatchError::MarktdUnavailable`] when no marktd client is configured
/// or marktd does not answer, [`DispatchError::StammdatenUnvollstaendig`]
/// when marktd holds no such Marktlokation.
pub(super) async fn lesen(
    state: &CommandsApiState,
    malo_id: &str,
    stichtag: time::Date,
) -> Result<MaloLokationsbuendel, DispatchError> {
    let marktd = state.marktd_client.as_deref().ok_or_else(|| {
        DispatchError::MarktdUnavailable(
            "no [marktd] client is configured — the Marktlokation's Lokationsbündel cannot be \
             read"
                .to_owned(),
        )
    })?;
    marktd
        .get_lokationsbuendel(malo_id, stichtag)
        .await
        .map_err(|e| DispatchError::MarktdUnavailable(e.to_string()))?
        .ok_or_else(|| DispatchError::StammdatenUnvollstaendig {
            malo_id: malo_id.to_owned(),
            fehlend: vec!["the Marktlokation itself — marktd holds no record of it".to_owned()],
        })
}

/// What an answer carries beside the Transaktionsgrund the workflow sets.
#[derive(Debug)]
pub(super) struct GdaAntwort {
    /// `SG4 STS+7` Transaktionsgrundergänzung — `ZW4` on a 55035.
    pub ergaenzung: Option<&'static str>,
    /// The `SG5`/`SG8`/`SG12` segments.
    pub stammdaten: Stammdaten,
}

/// The Stammdaten of the answer `antwort_pid` to a Geschäftsdatenanfrage.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every Muss fact marktd
/// does not hold and every group this module does not assemble.
pub(super) fn antwort(
    antwort_pid: u32,
    sd: &MaloUtilmdStammdaten,
    lb: &MaloLokationsbuendel,
) -> Result<GdaAntwort, DispatchError> {
    let mut f = Fehlend::default();
    match antwort_pid {
        55_095 => f.push(
            "55095: the Art der erzeugenden Marktlokation (SG10 CCI+++Z34, S. 622) and SG8 \
             SEQ+ZE9 OBIS-Daten der Technischen Ressource (S. 638) are not assembled"
                .to_owned(),
        ),
        55_060 => f.push(
            "55060: the Lieferrichtung (SG10 CCI+Z30, S. 618), SG8 SEQ+ZE2 Erforderliches \
             Messprodukt (S. 627), SEQ+ZF4 (S. 652), SEQ+ZF8 Kommunikationseinrichtungsdaten \
             (S. 660) and SG12 NAD+Z64 Messlokationsadresse (S. 664) are not assembled"
                .to_owned(),
        ),
        _ => {}
    }
    let ergaenzung = match (antwort_pid, lb.energierichtung) {
        (55_035, Some(Energierichtung::Aussp)) => Some("ZW4"),
        (55_035, Some(Energierichtung::Einsp)) => {
            f.push(
                "the Marktlokation is erzeugend (energierichtung EINSP): its answer is 55095, \
                 not 55035"
                    .to_owned(),
            );
            None
        }
        (55_095, Some(Energierichtung::Aussp)) => {
            f.push(
                "the Marktlokation is verbrauchend (energierichtung AUSSP): its answer is 55035, \
                 not 55095"
                    .to_owned(),
            );
            None
        }
        (_, None) => {
            f.push(
                "the Marktlokation's energierichtung (AUSSP verbrauchend → 55035 with SG4 STS \
                 ZW4, EINSP erzeugend → 55095)"
                    .to_owned(),
            );
            None
        }
        _ => None,
    };
    let verbrauchend = lb.energierichtung == Some(Energierichtung::Aussp);

    // The bündel's objects beside the Marktlokation.
    let of = |t: Lokationstyp| -> Vec<&str> {
        lb.objekte
            .iter()
            .filter(|o| o.typ == t && !(t == Lokationstyp::Malo && o.id == lb.malo_id))
            .map(|o| o.id.as_str())
            .collect()
    };
    for (typ, was) in [
        (
            Lokationstyp::Malo,
            "further Marktlokationen of the bündel (SG5 LOC+Z22 ruhende Marktlokation, [2015])",
        ),
        (
            Lokationstyp::Nelo,
            "Netzlokationen (SG5 LOC+Z18 with SG8 SEQ+ZD7 Daten der Netzlokation, Muss [294], \
             S. 611)",
        ),
        (
            Lokationstyp::Tr,
            "Technische Ressourcen (SG5 LOC+Z20 with SG8 SEQ+ZF0, Muss [296], S. 641)",
        ),
        (
            Lokationstyp::Sr,
            "Steuerbare Ressourcen (SG5 LOC+Z19 with their SG8 data, Muss [205])",
        ),
    ] {
        let ids = of(typ);
        if !ids.is_empty() {
            f.push(format!("{was}: {} — not assembled", ids.join(", ")));
        }
    }
    let melo_ids = of(Lokationstyp::Melo);
    let mut melos: Vec<&MesslokationStammdaten> = Vec::new();
    for id in &melo_ids {
        match sd.messlokationen.iter().find(|m| m.melo_id == *id) {
            Some(m) => melos.push(m),
            None => f.push(format!(
                "Messlokation {id} is in the bündel, but marktd's UTILMD Stammdaten assign it no \
                 Messstellenbetrieb or Zähler (the MELO → MALO Lokationszuordnung)"
            )),
        }
    }
    let melos_owned: Vec<MesslokationStammdaten> = melos.iter().map(|m| (*m).clone()).collect();

    let mut out = Stammdaten::default();
    // `SG5` in the MIG's order: the Marktlokation, then every Messlokation
    // (Bedingung `[165]` „Wenn bekannt").
    out.lokationen.push(StammdatenLokation {
        qualifier: "Z16".to_owned(),
        id: lb.malo_id.clone(),
    });
    for id in &melo_ids {
        out.lokationen.push(StammdatenLokation {
            qualifier: "Z17".to_owned(),
            id: (*id).to_owned(),
        });
    }

    // `SEQ+ZD5` Referenz auf die Lokationsbündelstruktur, then one `SEQ+ZD6`
    // per object with its Objektcode (Muss `[309]` ∧ `[2359]` ∧ `[350]`).
    lokationsbuendel(lb, &melo_ids, &mut out, &mut f);

    // `SEQ+Z98` Informative Daten der Marktlokation.
    let angaben = f.need(
        lb.angaben.as_ref(),
        "the NB's Lokationsbündel-Angaben on the Stichtag (PUT /api/v1/malos/{id}/\
         lokationsbuendel-angaben)",
    );
    let slp = daten_der_marktlokation(sd, angaben, &melos_owned, &mut out, &mut f);

    if let Some(a) = angaben {
        netznutzungsabrechnung(sd, a, &mut out, &mut f);
        obis_der_marktlokation(sd, a, verbrauchend, &mut out, &mut f);
        if antwort_pid == 55_035 {
            technische_einrichtungen(a, &mut out, &mut f);
        }
    }

    // `SEQ+ZF3` Daten der Messlokation — one per Messlokation (`[2284]`).
    for melo in &melos {
        let Some(msb) = msb_codes(melo, &mut f, true) else {
            continue;
        };
        out.bloecke.push(block(
            "ZF3",
            vec![
                seg("RFF", vec![vec!["Z19", &melo.melo_id]]),
                seg("CCI", vec![vec![], vec![], vec!["ZB3"]]),
                seg("CAV", vec![vec!["Z91", &msb.0, "", msb.1, msb.2]]),
                seg("CAV", vec![vec!["ZF0", &msb.3]]),
            ],
        ));
    }
    zaehleinrichtungen(lb, angaben, &melos, &mut out, &mut f);

    // `SEQ+ZG1` Profildaten, Muss for an SLP Marktlokation (`[119]`).
    if slp && let Some(a) = angaben {
        let verfahren = f.need(
            a.profilverfahren,
            "profilverfahren of the normierte Profil (SG8 SEQ+ZG1 SG10 CCI+Z02, Muss [119])",
        );
        let profil = f.need(
            a.standardlastprofil.as_deref(),
            "standardlastprofil — Code des normierten Profils (SG8 SEQ+ZG1 SG10 CAV, Muss [119])",
        );
        if let (Some(v), Some(p)) = (verfahren, profil) {
            let code = match v {
                mako_markt::lokationsbuendel::Profilverfahren::Synthetisch => "E01",
                mako_markt::lokationsbuendel::Profilverfahren::Analytisch => "Z10",
            };
            out.bloecke.push(block(
                "ZG1",
                vec![
                    seg("CCI", vec![vec!["Z02"], vec![], vec![code]]),
                    seg("CAV", vec![vec![p]]),
                ],
            ));
        }
    }

    // `SG12 NAD+Z63` Marktlokationsanschrift, Muss.
    match sd.anschrift.as_ref().and_then(anschrift_elemente) {
        Some(adresse) => out.sg12.push(nad("Z63", vec![], vec![], Some(&adresse))),
        None => f.push(
            "the Marktlokationsanschrift with Straße, Ort, PLZ and Land (Marktlokation.\
             lokationsadresse, SG12 NAD+Z63, Muss)"
                .to_owned(),
        ),
    }

    f.refuse(&lb.malo_id)?;
    Ok(GdaAntwort {
        ergaenzung,
        stammdaten: out,
    })
}

/// `SEQ+ZD5` `RFF+Z31` and one `SEQ+ZD6` per object.
fn lokationsbuendel(
    lb: &MaloLokationsbuendel,
    melo_ids: &[&str],
    out: &mut Stammdaten,
    f: &mut Fehlend,
) {
    let code = match lb.lokationsbuendelcodes.as_slice() {
        [code] => Some(code.as_str()),
        [] => {
            f.push(
                "the bündel's lokationsbuendelcode (Lokationszuordnung.lokationsbuendelcode — \
                 SG8 SEQ+ZD5 RFF+Z31, Muss, S. 605)"
                    .to_owned(),
            );
            None
        }
        many => {
            f.push(format!(
                "the bündel's Lokationszuordnungen name different Lokationsbündelstrukturen ({}) \
                 — SG8 SEQ+ZD5 states one",
                many.join(", ")
            ));
            None
        }
    };
    let Some(code) = code else { return };
    let Some(struktur) = Lokationsbuendelstruktur::from_wire(code) else {
        f.push(format!(
            "lokationsbuendelcode {code} is no structure of the Codeliste der \
             Lokationsbündelstrukturen 1.0 (Kap. 3)"
        ));
        return;
    };
    out.bloecke
        .push(block("ZD5", vec![seg("RFF", vec![vec!["Z31", code]])]));
    let objekte = std::iter::once((Lokationstyp::Malo, lb.malo_id.as_str()))
        .chain(melo_ids.iter().map(|id| (Lokationstyp::Melo, *id)));
    for (typ, id) in objekte {
        let (qualifier, objekttyp) = match typ {
            Lokationstyp::Malo => ("Z18", Objekttyp::Marktlokation),
            _ => ("Z19", Objekttyp::Messlokation),
        };
        let objektcode = lb
            .objekte
            .iter()
            .find(|o| o.typ == typ && o.id == id)
            .and_then(|o| o.objektcode.as_deref());
        let Some(objektcode) = objektcode else {
            f.push(format!(
                "{id}: lokationsbuendelObjektcode (SG8 SEQ+ZD6 RFF+Z33, Muss [2359] ∧ [350], S. \
                 607)"
            ));
            continue;
        };
        match struktur.objekt(objektcode) {
            None => f.push(format!(
                "{id}: Objektcode {objektcode} is not part of the Lokationsbündelstruktur {code}"
            )),
            Some(o) if o.rolle().objekttyp != objekttyp => f.push(format!(
                "{id}: Objektcode {objektcode} stands for a {}, not a {objekttyp}",
                o.rolle().objekttyp
            )),
            Some(o) if o.flexibilitaet == Flexibilitaet::Flexibel => f.push(format!(
                "{id}: Objektcode {objektcode} is a flexible object of {code}, whose references \
                 (SG8 RFF+Z34/Z35 vorgelagerte Messlokation/Netzlokation, S. 609) are not \
                 assembled"
            )),
            Some(_) => out.bloecke.push(block(
                "ZD6",
                vec![
                    seg("RFF", vec![vec![qualifier, id]]),
                    seg("RFF", vec![vec!["Z33", objektcode]]),
                ],
            )),
        }
    }
}

/// `SEQ+Z98` — the Jahresmenge, the ÜNB and MSB, the Spannungsebene, the
/// messtechnische Einordnung and the Prognosegrundlage. `true` when the
/// Marktlokation is an SLP one, which makes `SEQ+ZG1` Muss.
fn daten_der_marktlokation(
    sd: &MaloUtilmdStammdaten,
    angaben: Option<&BuendelAngaben>,
    melos: &[MesslokationStammdaten],
    out: &mut Stammdaten,
    f: &mut Fehlend,
) -> bool {
    let mut z98 = Vec::new();
    let mut slp = false;
    let bilanzierung = f.need(
        sd.bilanzierung.as_ref(),
        "Bilanzierung on the Stichtag (SG10 CCI+++ZC0/ZA6 Prognosegrundlage)",
    );
    let mut prognose = Vec::new();
    if let Some(b) = bilanzierung {
        match (b.prognosegrundlage, b.profiltypen.as_slice()) {
            (Some(Prognosegrundlage::Werte), _) => {
                prognose.push(seg("CCI", vec![vec![], vec![], vec!["ZC0"]]));
            }
            (Some(Prognosegrundlage::Profile), [Profiltyp::SlpSep]) => {
                slp = true;
                prognose.push(seg("CCI", vec![vec![], vec![], vec!["ZA6"]]));
                prognose.push(seg("CAV", vec![vec!["E02"]]));
            }
            (Some(Prognosegrundlage::Profile), [Profiltyp::TlpTep]) => f.push(
                "a TLP Marktlokation: the SG9 QTY+265/Z08 Arbeit ([101]) and SEQ+ZG2 \
                 Profilschardaten ([227]) are not assembled"
                    .to_owned(),
            ),
            (Some(Prognosegrundlage::Profile), other) => f.push(format!(
                "detailsPrognosegrundlage {:?}: a Prognose auf Basis von Profilen states \
                 exactly one of SLP_SEP/TLP_TEP here (SG10 CAV E02/E14, [2182])",
                other.iter().map(wire).collect::<Vec<_>>()
            )),
            (other, _) => f.push(format!(
                "prognosegrundlage {} (SG10 CCI+++ZC0/ZA6)",
                other.map_or_else(|| "absent".to_owned(), |p| wire(&p))
            )),
        }
        // `SG9 QTY+31` Veranschlagte Jahresmenge Gesamt behind `E02`
        // (Bedingung `[118]`), „keine Nachkommastelle" (`[937]`).
        if slp {
            match b.jahresverbrauchsprognose_kwh {
                Some(menge) if menge.fract().is_zero() && !menge.is_sign_negative() => {
                    z98.push(seg(
                        "QTY",
                        vec![vec!["31", &menge.trunc().to_string(), "KWH"]],
                    ));
                }
                Some(menge) => f.push(format!(
                    "jahresverbrauchsprognose {menge} kWh: SG9 QTY+31 takes a whole number ≥ 0 \
                     (Bedingungen [902], [937])"
                )),
                None => {
                    f.push("jahresverbrauchsprognose in kWh (SG9 QTY+31, Muss [118])".to_owned())
                }
            }
        }
    }
    // `SG10 CCI+++ZB3` Zugeordnete Marktpartner: `CAV+Z90` ÜNB (Muss), `CAV+Z91`
    // MSB (Muss `[345]` behind a Messlokation).
    let uenb = angaben.and_then(|a| {
        f.need(
            a.uenb_mp_id.as_deref(),
            "uenb_mp_id — the Marktlokation's ÜNB (SG10 CAV+Z90, Muss)",
        )
    });
    let msb = if melos.is_empty() {
        None
    } else {
        gemeinsamer_msb(melos, f)
    };
    if melos.is_empty() || msb.is_some() {
        if let Some(uenb) = uenb {
            z98.push(seg("CCI", vec![vec![], vec![], vec!["ZB3"]]));
            z98.push(seg("CAV", vec![vec!["Z90", uenb]]));
            if let Some(msb) = &msb {
                z98.push(seg("CAV", vec![vec!["Z91", &msb.0, "", msb.1, msb.2]]));
            }
        }
    } else {
        f.push(
            "the MSB of the Marktlokation's Messlokationen (SG10 CAV+Z91, Muss [345])".to_owned(),
        );
    }
    // `SG10 CCI+++E03` Spannungsebene der Marktlokation.
    if let Some(n) = f.need(sd.netzebene, "netzebene (SG10 CCI+++E03 Spannungsebene)") {
        let code = match n {
            Netzebene::Hss => Some("E03"),
            Netzebene::Hsp => Some("E04"),
            Netzebene::Msp => Some("E05"),
            Netzebene::Nsp => Some("E06"),
            _ => None,
        };
        match code {
            Some(code) => {
                z98.push(seg("CCI", vec![vec![], vec![], vec!["E03"]]));
                z98.push(seg("CAV", vec![vec![code]]));
            }
            None => f.push(format!(
                "netzebene {} has no Spannungsebene code (SG10 CAV E03/E04/E05/E06)",
                wire(&n)
            )),
        }
    }
    // `SG10 CCI+++Z83` Messtechnische Einordnung der Marktlokation.
    if let Some(code) = messtechnische_einordnung(melos, f) {
        z98.push(seg("CCI", vec![vec![], vec![], vec!["Z83"]]));
        z98.push(seg("CAV", vec![vec![code]]));
    }
    z98.extend(prognose);
    out.bloecke.push(block("Z98", z98));
    slp
}

/// `SEQ+ZE1` — one per Netznutzungsabrechnungsposition, each with the
/// Gemeinderabatt and its Zu-/Abschlag.
fn netznutzungsabrechnung(
    sd: &MaloUtilmdStammdaten,
    a: &BuendelAngaben,
    out: &mut Stammdaten,
    f: &mut Fehlend,
) {
    if a.nna_positionen.is_empty() {
        f.push(
            "the Marktlokation's Netznutzungsabrechnungspositionen (nna_positionen — SG8 \
             SEQ+ZE1 PIA+Z02, Muss, S. 622)"
                .to_owned(),
        );
        return;
    }
    let rabatt = f.need(
        sd.netznutzung
            .as_ref()
            .and_then(|n| n.gemeinderabatt_prozent),
        "gemeinderabatt_prozent (SG8 SEQ+ZE1 SG9 QTY+Z16, Muss)",
    );
    for p in &a.nna_positionen {
        let id = p.artikel_id.trim();
        let teile: Vec<usize> = id.split('-').map(str::len).collect();
        let art = match teile.as_slice() {
            // `[392]` n1-n2-n1 Gruppenartikel-ID, `[393]` n1-n2-n1-n3 Artikel-ID.
            [1, 2, 1] => Some("Z10"),
            [1, 2, 1, 3] => Some("Z09"),
            _ => None,
        };
        let Some(art) = art else {
            f.push(format!(
                "Artikel-ID {id}: only the formats n1-n2-n1 ([392]) and n1-n2-n1-n3 ([393]) are \
                 assembled (SG8 PIA+Z02)"
            ));
            continue;
        };
        if id.starts_with("1-06") {
            f.push(format!(
                "Artikel-ID {id}: the SG9 QTY+Z38 Anzahl der Positionen it makes Muss ([237]) \
                 is not held"
            ));
            continue;
        }
        if id == "1-07-3-001" {
            f.push(format!(
                "Artikel-ID {id}: its UTILMD Preisangabe — SG10 CCI+Z44 Berechnungspreis and \
                 QTY+Z33 ([73]) — is not held"
            ));
            continue;
        }
        if ["1-08-1", "1-08-2", "1-08-3", "1-08-4", "1-08-5"]
            .iter()
            .any(|g| id.starts_with(g))
        {
            f.push(format!(
                "Artikel-ID {id}: the Zählzeitdefinition and -register it takes ([64], [65]) \
                 are not held"
            ));
            continue;
        }
        let zu_ab = p.zu_abschlag_prozent;
        if zu_ab.scale() > 2 {
            f.push(format!(
                "Artikel-ID {id}: zu_abschlag_prozent {zu_ab} has more than two decimals ([930])"
            ));
            continue;
        }
        let Some(rabatt) = rabatt else { continue };
        let (qualifier, wert) = if zu_ab.is_zero() {
            ("Z37", "0".to_owned())
        } else if zu_ab.is_sign_positive() {
            ("Z34", zu_ab.normalize().to_string())
        } else {
            ("Z35", zu_ab.abs().normalize().to_string())
        };
        out.bloecke.push(block(
            "ZE1",
            vec![
                seg("PIA", vec![vec!["Z02"], vec![id, art]]),
                seg(
                    "QTY",
                    vec![vec!["Z16", &rabatt.normalize().to_string(), "P1"]],
                ),
                seg("QTY", vec![vec![qualifier, &wert, "P1"]]),
            ],
        ));
    }
}

/// `C.D` of an OBIS-Kennzahl `A-B:C.D.E`.
fn obis_cd(obis: &str) -> Option<(&str, &str)> {
    let (_, rest) = obis.split_once(':')?;
    let mut it = rest.split('.');
    Some((it.next()?, it.next()?))
}

/// `SEQ+ZE3` — one per OBIS-Kennzahl of the Marktlokation (`[314]`), each
/// with the Verwendungszweck for the NB, the LF and the ÜNB (`[381]`–`[383]`).
fn obis_der_marktlokation(
    sd: &MaloUtilmdStammdaten,
    a: &BuendelAngaben,
    verbrauchend: bool,
    out: &mut Stammdaten,
    f: &mut Fehlend,
) {
    if sd.zaehlwerke.is_empty() {
        f.push(
            "the Marktlokation's OBIS-Kennzahlen (Marktlokation.zaehlwerke — SG8 SEQ+ZE3, Muss \
             [314], S. 630)"
                .to_owned(),
        );
        return;
    }
    let energie = sd
        .zaehlwerke
        .iter()
        .filter(|z| {
            obis_cd(&z.obis_kennzahl)
                .is_some_and(|(c, d)| matches!(c, "1" | "2") && matches!(d, "8" | "9"))
        })
        .count();
    if energie > 1 {
        f.push(format!(
            "{energie} Energiemenge OBIS-Kennzahlen at the Marktlokation: the Zählzeitdefinition \
             each takes (SG10 CCI+Z39, [386]) is not held"
        ));
        return;
    }
    for zw in &sd.zaehlwerke {
        let obis = zw.obis_kennzahl.as_str();
        let Some(angabe) = a.obis.iter().find(|o| o.obis_kennzahl == obis) else {
            f.push(format!(
                "OBIS {obis}: its Verwendungszwecke for NB, LF and ÜNB (SG8 SEQ+ZE3 PIA+5, \
                 [381]–[383])"
            ));
            continue;
        };
        let mut zwecke: Vec<&str> = Vec::new();
        for (rolle, code, zugelassen) in [
            (
                "NB",
                angabe.verwendungszweck_nb.as_deref(),
                (|v: &mako_markt::lokationsbuendel::Verwendungszweck| v.nb)
                    as fn(&mako_markt::lokationsbuendel::Verwendungszweck) -> bool,
            ),
            ("LF", angabe.verwendungszweck_lf.as_deref(), |v| v.lf),
            ("ÜNB", angabe.verwendungszweck_uenb.as_deref(), |v| v.uenb),
        ] {
            let Some(code) = code else {
                f.push(format!(
                    "OBIS {obis}: Verwendungszweck for the {rolle} (SG8 PIA+5 DE 7143)"
                ));
                continue;
            };
            match verwendungszweck(code) {
                Some(v) if !zugelassen(v) => f.push(format!(
                    "OBIS {obis}: {code} is not admitted for the {rolle} (Codeliste der \
                     Verwendungszwecke 1.0, Kap. 2)"
                )),
                Some(v) if v.nur_verbrauch.is_some_and(|nur| nur != verbrauchend) => {
                    f.push(format!(
                        "OBIS {obis}: {code} is „nur möglich, wenn Lieferrichtung {}\" \
                         (Codeliste der Verwendungszwecke 1.0, Kap. 2)",
                        if verbrauchend {
                            "Erzeugung"
                        } else {
                            "Verbrauch"
                        }
                    ));
                }
                Some(_) => zwecke.push(code),
                None => f.push(format!(
                    "OBIS {obis}: {code} is no Marktlokation code of the Codeliste der \
                     Verwendungszwecke 1.0"
                )),
            }
        }
        let [nb, lf, uenb] = zwecke.as_slice() else {
            continue;
        };
        let mut segmente = vec![seg(
            "PIA",
            vec![
                vec!["5"],
                vec![obis, "SRW"],
                vec!["", nb, "Z16"],
                vec!["", lf, "Z16"],
                vec!["", uenb, "Z16"],
            ],
        )];
        // `[257]`: a 1-b:1.9.e Kennzahl takes its Wertegranularität.
        if obis_cd(obis) == Some(("1", "9")) {
            match angabe.wertegranularitaet {
                Some(g) => {
                    segmente.push(seg("CCI", vec![vec![], vec![], vec!["ZE4"]]));
                    segmente.push(seg("CAV", vec![vec![g.code()]]));
                }
                None => {
                    f.push(format!(
                        "OBIS {obis}: wertegranularitaet (SG10 CCI+++ZE4, Muss [257])"
                    ));
                    continue;
                }
            }
        }
        out.bloecke.push(block("ZE3", segmente));
    }
}

/// `SEQ+ZH9` — whether technische Einrichtungen are present (Muss `[480]`
/// on a verbrauchende Marktlokation).
fn technische_einrichtungen(a: &BuendelAngaben, out: &mut Stammdaten, f: &mut Fehlend) {
    match a.technische_einrichtungen {
        Some(false) => out.bloecke.push(block(
            "ZH9",
            vec![seg("CCI", vec![vec!["Z63"]]), seg("CAV", vec![vec!["ZH8"]])],
        )),
        Some(true) => f.push(
            "technische Einrichtungen are present: their Art und Nutzung (SG10 CCI+Z69, Muss \
             [82]) is not held"
                .to_owned(),
        ),
        None => f.push(
            "technische_einrichtungen — vorhanden or not (SG8 SEQ+ZH9 CCI+Z63, Muss [480])"
                .to_owned(),
        ),
    }
}

/// `SEQ+ZF5` Zähleinrichtungsdaten per Zähler (`[2286]`), then `SEQ+ZF6`
/// per register OBIS-Kennzahl (`[2287]` ∧ `[121]`).
fn zaehleinrichtungen(
    lb: &MaloLokationsbuendel,
    angaben: Option<&BuendelAngaben>,
    melos: &[&MesslokationStammdaten],
    out: &mut Stammdaten,
    f: &mut Fehlend,
) {
    let mut zf6 = Vec::new();
    for melo in melos {
        if melo.zaehler.is_empty() {
            f.push(format!(
                "Messlokation {}: its Zähler (SG8 SEQ+ZF5, Muss [2286])",
                melo.melo_id
            ));
        }
        for z in &melo.zaehler {
            let spez = lb
                .zaehler
                .iter()
                .find(|b| b.zaehler_id == z.zaehler_id)
                .and_then(|b| b.zaehlertyp_spezifikation);
            let typ: Option<(&str, &str)> = match z.zaehlertyp {
                Some(Zaehlertyp::Drehstromzaehler) => Some(("AHZ", "")),
                Some(Zaehlertyp::Wechselstromzaehler) => Some(("WSZ", "")),
                Some(Zaehlertyp::Maximumzaehler) => Some(("MAZ", "")),
                // `[346]` / `[136]`: the Ausprägung of a mME / EHZ.
                Some(Zaehlertyp::ModerneMesseinrichtung) => match spez {
                    Some(ZaehlertypSpezifikation::MmeStandard) => Some(("MME", "Z04")),
                    Some(ZaehlertypSpezifikation::MmeMeda) => Some(("MME", "Z05")),
                    _ => {
                        f.push(format!(
                            "Zähler {}: zaehlertypSpezifikation MME_STANDARD or MME_MEDA \
                             (SG10 CAV+MME DE 7110, Muss [346])",
                            z.zaehler_id
                        ));
                        None
                    }
                },
                Some(Zaehlertyp::ElektronischerZaehler) => match spez {
                    Some(ZaehlertypSpezifikation::Edl40) => Some(("EHZ", "Z01")),
                    Some(ZaehlertypSpezifikation::Edl21) => Some(("EHZ", "Z02")),
                    Some(ZaehlertypSpezifikation::SonstigerEhz) => Some(("EHZ", "Z03")),
                    _ => {
                        f.push(format!(
                            "Zähler {}: zaehlertypSpezifikation EDL40, EDL21 or SONSTIGER_EHZ \
                             (SG10 CAV+EHZ DE 7110, Muss [136])",
                            z.zaehler_id
                        ));
                        None
                    }
                },
                Some(Zaehlertyp::IntelligentesMesssystem) => {
                    f.push(format!(
                        "Zähler {}: an intelligentes Messsystem — its Smartmeter-Gateway (SG8 \
                         SEQ+ZG0, RFF+Z14) is not assembled",
                        z.zaehler_id
                    ));
                    None
                }
                Some(other) => {
                    f.push(format!(
                        "Zähler {}: zaehlertyp {} has no Strom Zählertyp code here (SG10 \
                         CCI+++E13 AHZ/WSZ/LAZ/MAZ/MME/EHZ/IVA)",
                        z.zaehler_id,
                        wire(&other)
                    ));
                    None
                }
                None => {
                    f.push(format!(
                        "Zähler {}: zaehlertyp (SG10 CCI+++E13, Muss)",
                        z.zaehler_id
                    ));
                    None
                }
            };
            let nummer = f.need(
                z.zaehlernummer.as_deref(),
                &format!(
                    "Zähler {}: zaehlernummer (SG10 CAV+Z30 Gerätenummer, Muss)",
                    z.zaehler_id
                ),
            );
            let erfassung = match z.fernauslesbar {
                Some(true) => Some("AMR"),
                Some(false) => Some("MMR"),
                None => {
                    f.push(format!(
                        "Zähler {}: fernauslesbar (SG10 CCI+++E12 AMR/MMR, Muss [316])",
                        z.zaehler_id
                    ));
                    None
                }
            };
            let (Some((typ, auspraegung)), Some(nummer), Some(erfassung)) =
                (typ, nummer, erfassung)
            else {
                continue;
            };
            let cav_typ = if auspraegung.is_empty() {
                vec![typ]
            } else {
                vec![typ, "", "", auspraegung]
            };
            out.bloecke.push(block(
                "ZF5",
                vec![
                    seg("RFF", vec![vec!["Z19", &melo.melo_id]]),
                    seg("CCI", vec![vec![], vec![], vec!["E13"]]),
                    seg("CAV", vec![cav_typ]),
                    seg("CAV", vec![vec!["Z30", "", "", nummer]]),
                    seg("CCI", vec![vec![], vec![], vec!["E12"]]),
                    seg("CAV", vec![vec![erfassung]]),
                ],
            ));
            if z.obis_kennzahlen.is_empty() {
                f.push(format!(
                    "Zähler {}: the OBIS-Kennzahlen of its registers (SG8 SEQ+ZF6, Muss [2287])",
                    z.zaehler_id
                ));
            }
            for obis in &z.obis_kennzahlen {
                let mut segmente = vec![
                    seg("RFF", vec![vec!["MG", nummer]]),
                    seg("PIA", vec![vec!["5"], vec![obis, "SRW"]]),
                ];
                // `[256]`: a 1-b:1.8.e / 1-b:2.8.e register takes its
                // Wertegranularität.
                if obis_cd(obis).is_some_and(|(c, d)| matches!(c, "1" | "2") && d == "8") {
                    let gran = angaben.and_then(|a| {
                        a.register
                            .iter()
                            .find(|r| r.zaehler_id == z.zaehler_id && r.obis_kennzahl == *obis)
                    });
                    match gran {
                        Some(r) => {
                            segmente.push(seg("CCI", vec![vec![], vec![], vec!["ZE4"]]));
                            segmente.push(seg("CAV", vec![vec![r.wertegranularitaet.code()]]));
                        }
                        None => {
                            f.push(format!(
                                "Zähler {} register {obis}: wertegranularitaet (SG8 SEQ+ZF6 \
                                 CCI+++ZE4, Muss [256])",
                                z.zaehler_id
                            ));
                            continue;
                        }
                    }
                }
                zf6.push(block("ZF6", segmente));
            }
        }
    }
    out.bloecke.extend(zf6);
}

#[cfg(test)]
mod tests {
    use mako_engine::payload::StammdatenSegment;

    use super::*;

    #[test]
    fn obis_value_groups_read_c_and_d() {
        assert_eq!(obis_cd("1-1:1.8.0"), Some(("1", "8")));
        assert_eq!(obis_cd("1-0:2.9.1"), Some(("2", "9")));
        assert_eq!(obis_cd("1.8.0"), None);
    }

    #[test]
    fn segment_type_is_the_one_sent() {
        // The shared constructor: one element per vector, one component per
        // string.
        let s: StammdatenSegment = seg("QTY", vec![vec!["Z37", "0", "P1"]]);
        assert_eq!(s.elemente, vec![vec!["Z37", "0", "P1"]]);
    }
}
