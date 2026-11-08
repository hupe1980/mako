//! The Marktlokation's Stammdaten an Anmeldung EoG carries, read from marktd at
//! the boundary and set down segment by segment as the AHB column of its
//! Prüfidentifikator lists them.
//!
//! The NB's Anmeldung EoG — UTILMD **55013** (Strom, UTILMD AHB Strom 2.2) and
//! **44013** (Gas, UTILMD AHB Gas 1.2 Kap. 5.9) — carries the Marktlokation's
//! record whole: GeLi Gas 3.0 Kap. 3.3.2 has the NB tell the E/G „Namen und
//! Adressen des Anschlussnehmers und des Anschlussnutzers, … sowie die
//! Information, ob der an der Marktlokation versorgte Letztverbraucher ein
//! „Haushaltskunde“ ist" and „die Identität des derzeitigen
//! Messstellenbetreibers". marktd holds that record; the command names the
//! Marktlokation and the Grund, and this module turns marktd's facts into the
//! `SG5`/`SG6`/`SG8`/`SG12` segments.
//!
//! **Every fact comes from marktd, or the command is refused naming it.** A
//! Muss segment marktd holds no value for is collected into
//! [`DispatchError::StammdatenUnvollstaendig`]; nothing is defaulted. The two
//! facts that follow from the process itself, not from a stored record, are
//! stated where they are derived:
//!
//! - the Transaktionsgrundergänzung of a 55013 — `ZW7` „Gemessene
//!   Marktlokation" when marktd assigns the Marktlokation a Messlokation, `ZW6`
//!   „Pauschale Marktlokation" when it assigns none, which is how the same
//!   column defines `CAV+Z68` „keine Messung … (Pauschale-Marktlokation)";
//! - the Netznutzungsvertrag of a 44013 — the Zuordnung makes the E/G the
//!   Marktlokation's Lieferant (GeLi Gas 3.0 Kap. 3.3.1), so the Netznutzung
//!   runs under the Vertrag zwischen Lieferanten und NB (`CAV+Z74:::Z09`) and
//!   the Lieferant pays it (`CAV+Z73:::Z11`).
//!
//! What this module does not assemble yet is refused by name too: a
//! Prognose auf Basis von Profilen on a Gas Marktlokation (the `SEQ+Z35`
//! Lastprofildaten), a TLP Marktlokation in Strom (the `QTY+265`/`Z08`
//! Arbeit), and the Wertegranularität and Vor-/Nachkommastellen an OBIS of
//! Bedingungen `[257]`/`[274]` requires.

use mako_engine::outbox::PendingOutbox;
use mako_engine::payload::{Stammdaten, StammdatenBlock, StammdatenLokation, StammdatenSegment};
use mako_markt::utilmd_stammdaten::{
    Abrechnungsvariante, Anschrift, Betriebszustand, Kunde, MaloUtilmdStammdaten,
    MesslokationStammdaten, MsbGrundlage, MsbRolle,
};
use rubo4e::current::{
    AbgabeArt, Fallgruppenzuordnung, Gasqualitaet, Netzebene, Profiltyp, Prognosegrundlage,
    Zaehlertyp,
};

use super::{CommandsApiState, DispatchError};

/// The Transaktionsgründe Bedingung `[203]` names — on these the
/// „Kunde des Lieferanten" is Muss, on the others Soll „wenn bekannt"
/// (`[165]`). UTILMD AHB Strom 2.2 / Gas 1.2, Anwendungsfall 55013 / 44013.
const GRUENDE_MIT_KUNDE: &[&str] = &["E06", "Z39", "ZC6", "ZC7", "ZT6", "ZT7", "Z02", "ZZD"];

/// OBIS-Kennzahlen of a Gas Marktlokation whose `SEQ+Z02` takes a `SEQ+Z07`
/// Konzessionsabgabedaten (UTILMD AHB Gas 1.2, Bedingung `[2335]`).
const OBIS_MIT_KONZESSIONSABGABE: &[&str] = &["7-20:99.33.17", "7-0:33.86.0"];

/// The OBIS-Kennzahl of a Gas Marktlokation whose `SEQ+Z02` takes a
/// Wertegranularität (Bedingung `[257]`).
const OBIS_MIT_WERTEGRANULARITAET: &str = "7-0:33.86.0";

/// The OBIS value groups C:D.E of a Zähleinrichtung whose `SEQ+Z20` takes a
/// Wertegranularität and Vor-/Nachkommastellen (Bedingung `[274]`,
/// `7-b?:<C.D.E>`).
const ZAEHLER_OBIS_MIT_STELLEN: &[&str] = &[
    "3.0.0", "6.0.0", "3.1.0", "6.1.0", "3.2.0", "6.2.0", "13.2.0", "16.2.0", "1.0.0", "2.0.0",
    "4.0.0", "5.0.0", "11.2.0", "12.2.0", "14.2.0", "15.2.0",
];

/// Read the Marktlokation's UTILMD Stammdaten on `stichtag` from marktd.
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
) -> Result<MaloUtilmdStammdaten, DispatchError> {
    let marktd = state.marktd_client.as_deref().ok_or_else(|| {
        DispatchError::MarktdUnavailable(
            "no [marktd] client is configured — the Marktlokation's Stammdaten cannot be read"
                .to_owned(),
        )
    })?;
    marktd
        .get_utilmd_stammdaten(malo_id, stichtag)
        .await
        .map_err(|e| DispatchError::MarktdUnavailable(e.to_string()))?
        .ok_or_else(|| DispatchError::StammdatenUnvollstaendig {
            malo_id: malo_id.to_owned(),
            fehlend: vec!["the Marktlokation itself — marktd holds no record of it".to_owned()],
        })
}

/// The Zuordnungsbeginn of a command as a date — `YYYY-MM-DD` or `YYYYMMDD`.
pub(super) fn stichtag(process_date: &str) -> Result<time::Date, DispatchError> {
    let digits: String = process_date
        .chars()
        .take(10)
        .filter(char::is_ascii_digit)
        .collect();
    let parsed = (digits.len() == 8)
        .then(|| {
            time::Date::parse(
                &digits,
                time::macros::format_description!("[year][month][day]"),
            )
            .ok()
        })
        .flatten();
    parsed.ok_or_else(|| {
        DispatchError::InvalidPayload(format!(
            "\"process_date\" {process_date:?} is not a date (YYYY-MM-DD or YYYYMMDD)"
        ))
    })
}

/// Refuse a command whose UTILMD the pre-send AHB gate would dead-letter.
///
/// The workflow is pure, so its `handle` on a fresh process yields the exact
/// outbox entries the spawn writes; each UTILMD among them is rendered and
/// checked against its profile as `transport::as4_sender` checks it before
/// delivery. A refusal names every finding.
pub(super) fn refuse_what_the_gate_refuses(
    state: &CommandsApiState,
    outbox: &[PendingOutbox],
) -> Result<(), DispatchError> {
    for pending in outbox.iter().filter(|p| &*p.message_type == "UTILMD") {
        let findings =
            crate::orchestrator::edifact_renderer::utilmd_ahb_findings(pending, state.tenant_id)
                .map_err(|e| {
                    DispatchError::InvalidPayload(format!("the UTILMD does not render: {e}"))
                })?;
        if !findings.is_empty() {
            return Err(DispatchError::InvalidPayload(format!(
                "the UTILMD violates its AHB profile: {}",
                findings.join("; ")
            )));
        }
    }
    Ok(())
}

/// What a 55013 carries beside its Grund.
#[derive(Debug)]
pub(super) struct Eog55013 {
    /// `ZW6` or `ZW7`.
    pub ergaenzung: &'static str,
    /// Gruppenzuordnung nach EnWG.
    pub haushaltskunde: Option<bool>,
    /// The `SG5`/`SG8`/`SG12` segments.
    pub stammdaten: Stammdaten,
}

/// The Stammdaten of a Strom Anmeldung EoG 55013.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every Muss fact marktd
/// does not hold.
pub(super) fn strom_55013(
    sd: &MaloUtilmdStammdaten,
    transaktionsgrund: &str,
) -> Result<Eog55013, DispatchError> {
    let mut f = Fehlend::default();
    let melos = &sd.messlokationen;
    let gemessen = !melos.is_empty();
    let ergaenzung = if gemessen {
        edi_energy::utilmd_codes::ergaenzung::GEMESSENE_MALO
    } else {
        edi_energy::utilmd_codes::ergaenzung::PAUSCHALE_MALO
    };

    let mut out = Stammdaten::default();
    // `SG5 LOC+Z17` — every Messlokation the Energiemenge is computed from
    // (Bedingungen `[483]`, `[623]`).
    for melo in melos {
        out.lokationen.push(StammdatenLokation {
            qualifier: "Z17".to_owned(),
            id: melo.melo_id.clone(),
        });
    }

    // `SG8 SEQ+Z98` Informative Daten der Marktlokation, genau einmal.
    let mut z98 = Vec::new();
    let netzebene = f.need(sd.netzebene, "netzebene (SG10 CCI+++E03 Spannungsebene)");
    let spannung = netzebene.and_then(|n| {
        let code = match n {
            Netzebene::Hss => Some("E03"),
            Netzebene::Hsp => Some("E04"),
            Netzebene::Msp => Some("E05"),
            Netzebene::Nsp => Some("E06"),
            _ => None,
        };
        if code.is_none() {
            f.push(format!(
                "netzebene {} has no Spannungsebene code (SG10 CAV E03/E04/E05/E06)",
                wire(&n)
            ));
        }
        code
    });
    let bilanzierung = f.need(
        sd.bilanzierung.as_ref(),
        "Bilanzierung on the Stichtag (SG10 CCI+++ZC0/ZA6 Prognosegrundlage)",
    );
    // `SG9 QTY+31` Veranschlagte Jahresmenge, Muss in der Niederspannung
    // (Bedingung `[122]`), „keine Nachkommastelle" (`[937]`).
    if spannung == Some("E06")
        && let Some(b) = bilanzierung
    {
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
            None => f.push(
                "jahresverbrauchsprognose in kWh (SG9 QTY+31, Muss in der Niederspannung, [122])"
                    .to_owned(),
            ),
        }
    }
    // `SG10 CCI+++ZB3` / `CAV+Z91` — the MSB, behind `ZW7` (Bedingung `[483]`).
    // The MSB is assigned to the Messlokation („Der MSB ist ausschließlich
    // dem Objekt Messlokation zugeordnet", WiM Strom Teil 1 Kap. 2.1.2 d), so
    // the Marktlokation's is the one its Messlokationen share.
    if gemessen && let Some(msb) = gemeinsamer_msb(melos, &mut f) {
        z98.push(seg("CCI", vec![vec![], vec![], vec!["ZB3"]]));
        z98.push(seg("CAV", vec![vec!["Z91", &msb.0, "", msb.1, msb.2]]));
    }
    if let Some(code) = spannung {
        z98.push(seg("CCI", vec![vec![], vec![], vec!["E03"]]));
        z98.push(seg("CAV", vec![vec![code]]));
    }
    // `SG10 CCI+++Z83` Messtechnische Einordnung: `Z52` iMS when every
    // Messlokation is equipped with one, `Z53` kME/mME when at least one is
    // not, `Z68` keine Messung for a Pauschale-Marktlokation.
    if let Some(code) = messtechnische_einordnung(melos, &mut f) {
        z98.push(seg("CCI", vec![vec![], vec![], vec!["Z83"]]));
        z98.push(seg("CAV", vec![vec![code]]));
    }
    if let Some(b) = bilanzierung {
        match (b.prognosegrundlage, b.profiltypen.as_slice()) {
            (Some(Prognosegrundlage::Werte), _) => {
                z98.push(seg("CCI", vec![vec![], vec![], vec!["ZC0"]]));
            }
            (Some(Prognosegrundlage::Profile), [Profiltyp::SlpSep]) => {
                z98.push(seg("CCI", vec![vec![], vec![], vec!["ZA6"]]));
                z98.push(seg("CAV", vec![vec!["E02"]]));
            }
            (Some(Prognosegrundlage::Profile), [Profiltyp::TlpTep]) => f.push(
                "a TLP Marktlokation: the SG9 QTY+265/Z08 Arbeit für tagesparameterabhängige \
                 Marktlokationen ([101]) is not assembled"
                    .to_owned(),
            ),
            (Some(Prognosegrundlage::Profile), other) => f.push(format!(
                "detailsPrognosegrundlage {:?}: a Prognose auf Basis von Profilen states \
                 exactly one of SLP_SEP/TLP_TEP here (SG10 CAV E02/E14)",
                other.iter().map(wire).collect::<Vec<_>>()
            )),
            (other, _) => f.push(format!(
                "prognosegrundlage {} (SG10 CCI+++ZC0/ZA6)",
                other.map_or_else(|| "absent".to_owned(), |p| wire(&p))
            )),
        }
    }
    let netznutzung = f.need(
        sd.netznutzung.as_ref(),
        "Netznutzungsdaten on the Stichtag (PUT /api/v1/malos/{id}/netznutzungsdaten)",
    );
    let haushaltskunde = netznutzung.and_then(|n| n.haushaltskunde);
    if netznutzung.is_some() {
        match haushaltskunde {
            Some(h) => z98.push(seg(
                "CCI",
                vec![vec![], vec![], vec![if h { "Z15" } else { "Z18" }]],
            )),
            None => f.push("haushaltskunde (SG10 CCI+++Z15/Z18 Gruppenzuordnung)".to_owned()),
        }
        match netznutzung.and_then(|n| n.betriebszustand) {
            Some(b) => z98.push(seg("CCI", vec![vec!["Z42"], vec![], vec![betrieb(b)]])),
            None => f.push("betriebszustand (SG10 CCI+Z42 Betriebszustand)".to_owned()),
        }
    }
    out.bloecke.push(block("Z98", z98));

    // `SG8 SEQ+ZF3` — one block per Messlokation (Bedingung `[2284]`).
    if gemessen {
        for melo in melos {
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
    }

    // `SG12` — the Kunde des LF and his Korrespondenzanschrift, then the
    // Marktlokationsanschrift.
    kunde_und_anschrift(
        sd,
        transaktionsgrund,
        Qualifier {
            kunde: "Z65",
            korrespondenz: "Z66",
            marktlokation: "Z63",
            ablesekarte: None,
        },
        &mut out,
        &mut f,
    );

    f.refuse(&sd.malo_id)?;
    Ok(Eog55013 {
        ergaenzung,
        haushaltskunde,
        stammdaten: out,
    })
}

/// The Stammdaten of a Gas Anmeldung EoG 44013.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every Muss fact marktd
/// does not hold.
pub(super) fn gas_44013(
    sd: &MaloUtilmdStammdaten,
    transaktionsgrund: &str,
) -> Result<Stammdaten, DispatchError> {
    let mut f = Fehlend::default();
    let mut out = Stammdaten::default();
    let melos = &sd.messlokationen;

    // `SG5 LOC+172` — „die ID der Marktlokation und alle Identifikatoren der
    // Messlokationen" (Bedingung `[527]`); the Marktlokation is the primary
    // Lokation the renderer writes first.
    for melo in melos {
        out.lokationen.push(StammdatenLokation {
            qualifier: "172".to_owned(),
            id: melo.melo_id.clone(),
        });
    }

    // `SG6 RFF+Z18` Termine der Marktlokation.
    let netznutzung = f.need(
        sd.netznutzung.as_ref(),
        "Netznutzungsdaten on the Stichtag (PUT /api/v1/malos/{id}/netznutzungsdaten)",
    );
    if let Some(n) = netznutzung {
        let termin = f.need(
            n.abrechnungstermin.as_deref(),
            "abrechnungstermin MMDD (SG6 DTM+Z21 Termin der Netznutzungsabrechnung)",
        );
        let jahr = f.need(
            n.naechste_abrechnung_jahr,
            "naechste_abrechnung_jahr CCYY (SG6 DTM+Z09 Nächste Netznutzungsabrechnung)",
        );
        let intervall = f.need(
            n.abrechnungsintervall_monate,
            "abrechnungsintervall_monate (SG6 DTM+Z22 Netznutzungsabrechnungsintervall)",
        );
        if let (Some(termin), Some(jahr), Some(intervall)) = (termin, jahr, intervall) {
            out.sg6.push(seg("RFF", vec![vec!["Z18", &sd.malo_id]]));
            out.sg6.push(seg("DTM", vec![vec!["Z21", termin, "106"]]));
            out.sg6
                .push(seg("DTM", vec![vec!["Z09", &jahr.to_string(), "602"]]));
            out.sg6
                .push(seg("DTM", vec![vec!["Z22", &intervall.to_string(), "802"]]));
        }
    }

    // `SG8 SEQ+Z01` Daten der Marktlokation.
    let mut z01 = Vec::new();
    if let Some(gebiet) = f.need(
        sd.marktgebiet.as_deref(),
        "marktgebiet (SG10 CCI+Z21 Marktgebiet)",
    ) {
        z01.push(seg("CCI", vec![vec!["Z21"], vec![], vec![gebiet]]));
    }
    if let Some(n) = f.need(sd.netzebene, "netzebene (SG10 CCI+++Y01 Druckebene)") {
        match n {
            Netzebene::Hd | Netzebene::Md | Netzebene::Nd => {
                let code = match n {
                    Netzebene::Hd => "Y01",
                    Netzebene::Md => "Y02",
                    _ => "Y03",
                };
                z01.push(seg("CCI", vec![vec![], vec![], vec!["Y01"]]));
                z01.push(seg("CAV", vec![vec![code]]));
            }
            other => f.push(format!(
                "netzebene {} is no Druckebene (SG10 CAV Y01/Y02/Y03)",
                wire(&other)
            )),
        }
    }
    let werte = match f.need(
        sd.bilanzierung.as_ref(),
        "Bilanzierung on the Stichtag (SG10 CCI+++ZC0/ZA6 Prognosegrundlage)",
    ) {
        Some(b) => match b.prognosegrundlage {
            Some(Prognosegrundlage::Werte) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["ZC0"]]));
                Some(b)
            }
            Some(Prognosegrundlage::Profile) => {
                f.push(
                    "a Prognose auf Basis von Profilen: the SG8 SEQ+Z35 Lastprofildaten and the \
                     SG9 QTY+31 Jahresmenge it makes Muss ([106]) are not assembled"
                        .to_owned(),
                );
                None
            }
            other => {
                f.push(format!(
                    "prognosegrundlage {} (SG10 CCI+++ZC0/ZA6)",
                    other.map_or_else(|| "absent".to_owned(), |p| wire(&p))
                ));
                None
            }
        },
        None => None,
    };
    if let Some(n) = netznutzung {
        match n.haushaltskunde {
            Some(h) => z01.push(seg(
                "CCI",
                vec![vec![], vec![], vec![if h { "Z15" } else { "Z18" }]],
            )),
            None => f.push("haushaltskunde (SG10 CCI+++Z15/Z18 Gruppenzuordnung)".to_owned()),
        }
    }
    // `SG10 CCI+++Z17` Fallgruppenzuordnung — Muss behind `ZC0` (Bedingung
    // `[19]`); the codes are UTILMD MIG Gas G1.2's („GABi-RLMmT" RLM-Kunde in
    // Tagesregime, „GABi-RLMoT" im Stundenregime, „GABi-RLMNEV" only for
    // Liefermonate vor dem 01.10.2016).
    if let Some(b) = werte {
        match b.fallgruppe {
            Some(Fallgruppenzuordnung::GabiRlmMitTagesband) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["Z17", "GABi-RLMmT"]]));
            }
            Some(Fallgruppenzuordnung::GabiRlmOhneTagesband) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["Z17", "GABi-RLMoT"]]));
            }
            Some(other) => f.push(format!(
                "fallgruppe {}: GABi-RLMNEV is admitted only for Liefermonate vor dem \
                 01.10.2016 (SG10 CCI+++Z17)",
                wire(&other)
            )),
            None => f.push("fallgruppe (SG10 CCI+++Z17 Fallgruppenzuordnung, [19])".to_owned()),
        }
    }
    // `SG10 CCI+++Z88` Netznutzung: the E/G is the Lieferant, so the
    // Netznutzungsvertrag is the one between Lieferant and NB and the
    // Lieferant pays (module doc).
    if let Some(n) = netznutzung {
        match n.abrechnungsvariante {
            Some(v) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["Z88"]]));
                z01.push(seg("CAV", vec![vec!["Z74", "", "", "Z09"]]));
                z01.push(seg("CAV", vec![vec!["Z73", "", "", "Z11"]]));
                z01.push(seg("CAV", vec![vec!["ZB1", "", "", variante(v)]]));
            }
            None => f.push(
                "abrechnungsvariante (SG10 CAV+ZB1 Netznutzungsabrechnungsvariante)".to_owned(),
            ),
        }
        match n.betriebszustand {
            Some(b) => z01.push(seg("CCI", vec![vec!["Z42"], vec![], vec![betrieb(b)]])),
            None => f.push("betriebszustand (SG10 CCI+Z42 Betriebszustand)".to_owned()),
        }
    }
    out.bloecke.push(block("Z01", z01));

    // `SG8 SEQ+Z02` OBIS-Daten der Marktlokation, then `SEQ+Z07` for each
    // OBIS Bedingung `[2335]` names.
    if sd.zaehlwerke.is_empty() {
        f.push("the Marktlokation's OBIS-Kennzahlen (SG8 SEQ+Z02, Muss)".to_owned());
    }
    let mut z07 = Vec::new();
    for zw in &sd.zaehlwerke {
        let obis = zw.obis_kennzahl.as_str();
        if obis == OBIS_MIT_WERTEGRANULARITAET {
            f.push(format!(
                "OBIS {obis}: its SG10 CCI+++ZE4 Wertegranularität ([257]) is not held"
            ));
        }
        out.bloecke.push(block(
            "Z02",
            vec![seg("PIA", vec![vec!["5"], vec![obis, "SRW"]])],
        ));
        if OBIS_MIT_KONZESSIONSABGABE.contains(&obis) {
            match konzessionsabgabe(zw, &mut f) {
                Some(cav) => z07.push(block(
                    "Z07",
                    vec![
                        seg("RFF", vec![vec!["Z10", obis]]),
                        seg("CCI", vec![vec![], vec![], vec!["Z08"]]),
                        StammdatenSegment {
                            tag: "CAV".to_owned(),
                            elemente: vec![cav],
                        },
                    ],
                )),
                None => continue,
            }
        }
    }
    out.bloecke.extend(z07);

    // `SG8 SEQ+Z12` Gemeinderabatt, Muss.
    if let Some(n) = netznutzung {
        match n.gemeinderabatt_prozent {
            Some(p) => out.bloecke.push(block(
                "Z12",
                vec![seg(
                    "QTY",
                    vec![vec!["Z16", &p.normalize().to_string(), "P1"]],
                )],
            )),
            None => f.push("gemeinderabatt_prozent (SG8 SEQ+Z12 QTY+Z16, Muss)".to_owned()),
        }
    }

    // `SG8 SEQ+Z18` Daten der Messlokation — one per Messlokation (`[2284]`).
    let gasqualitaet = sd.gasqualitaet.and_then(|g| match g {
        Gasqualitaet::HGas => Some("Y04"),
        Gasqualitaet::LGas => Some("Y05"),
        _ => None,
    });
    if !melos.is_empty() && gasqualitaet.is_none() {
        f.push("gasqualitaet (SG10 CCI+++Y02 Gasqualität of the Messlokation)".to_owned());
    }
    let mut zaehler_bloecke = Vec::new();
    for melo in melos {
        if let (Some(msb), Some(gq)) = (msb_codes(melo, &mut f, false), gasqualitaet) {
            out.bloecke.push(block(
                "Z18",
                vec![
                    seg("RFF", vec![vec!["Z19", &melo.melo_id]]),
                    seg("CCI", vec![vec![], vec![], vec!["ZB3"]]),
                    seg("CAV", vec![vec!["Z91", &msb.0, "", msb.2]]),
                    seg("CAV", vec![vec!["ZF0", &msb.3]]),
                    seg("CCI", vec![vec![], vec![], vec!["Y02"]]),
                    seg("CAV", vec![vec![gq]]),
                ],
            ));
        }
        zaehler_bloecke.extend(gas_zaehler(melo, &mut f));
    }
    // `SEQ+Z03` Zähleinrichtungsdaten and `SEQ+Z20` their OBIS-Daten follow
    // the Messlokationen in the MIG's order.
    zaehler_bloecke.sort_by_key(|b| b.seq != "Z03");
    out.bloecke.extend(zaehler_bloecke);

    kunde_und_anschrift(
        sd,
        transaktionsgrund,
        Qualifier {
            kunde: "Z09",
            korrespondenz: "Z04",
            marktlokation: "DP",
            ablesekarte: Some("Z05"),
        },
        &mut out,
        &mut f,
    );

    f.refuse(&sd.malo_id)?;
    Ok(out)
}

/// The `SEQ+Z03` and `SEQ+Z20` blocks of a Gas Messlokation's Zähler.
fn gas_zaehler(melo: &MesslokationStammdaten, f: &mut Fehlend) -> Vec<StammdatenBlock> {
    let mut out = Vec::new();
    if melo.zaehler.is_empty() {
        f.push(format!(
            "Messlokation {}: no Zähler (SG8 SEQ+Z03 Zähleinrichtungsdaten, [2286])",
            melo.melo_id
        ));
    }
    for z in &melo.zaehler {
        let id = &z.zaehler_id;
        let nummer = f.need(
            z.zaehlernummer.as_deref(),
            &format!("Zähler {id}: zaehlernummer (SG10 CAV+Z30 Gerätenummer)"),
        );
        let typ = match z.zaehlertyp {
            Some(Zaehlertyp::Balgengaszaehler) => Some("BGZ"),
            Some(Zaehlertyp::Drehkolbenzaehler) => Some("DKZ"),
            Some(Zaehlertyp::Turbinenradgaszaehler) => Some("TRZ"),
            Some(Zaehlertyp::Ultraschallgaszaehler) => Some("UGZ"),
            Some(Zaehlertyp::Wirbelgaszaehler) => Some("WGZ"),
            Some(other) => {
                f.push(format!(
                    "Zähler {id}: zaehlertyp {} has no UTILMD Gas Zählertyp (SG10 CAV E13)",
                    wire(&other)
                ));
                None
            }
            None => {
                f.push(format!(
                    "Zähler {id}: zaehlertyp (SG10 CCI+++E13 Zählertyp)"
                ));
                None
            }
        };
        let groesse = f
            .need(
                z.zaehlergroesse.as_ref(),
                &format!("Zähler {id}: zaehlergroesse (SG10 CAV Zählergröße)"),
            )
            .map(|g| wire(g).replace("KOMMA", "."));
        let erfassung = f
            .need(
                z.fernauslesbar,
                &format!("Zähler {id}: fernauslesbar (SG10 CCI+++E12 Messwerterfassung)"),
            )
            .map(|fern| if fern { "AMR" } else { "MMR" });
        let (Some(nummer), Some(typ), Some(groesse), Some(erfassung)) =
            (nummer, typ, groesse, erfassung)
        else {
            continue;
        };
        out.push(block(
            "Z03",
            vec![
                seg("RFF", vec![vec!["Z19", &melo.melo_id]]),
                seg("CCI", vec![vec![], vec![], vec!["E13"]]),
                seg("CAV", vec![vec![typ]]),
                seg("CAV", vec![vec!["", "", "", &groesse]]),
                seg("CAV", vec![vec!["Z30", "", "", nummer]]),
                seg("CCI", vec![vec![], vec![], vec!["E12"]]),
                seg("CAV", vec![vec![erfassung]]),
            ],
        ));
        // `SEQ+Z20` — at least one per Zähleinrichtung (`[2287]`), one per
        // OBIS-Kennzahl, the Zähler named by `RFF+MG` (`[442]`: no
        // Mengenumwerter is held, so none names it).
        if z.obis_kennzahlen.is_empty() {
            f.push(format!(
                "Zähler {id}: no OBIS-Kennzahl on its registers (SG8 SEQ+Z20, [2287])"
            ));
        }
        for obis in &z.obis_kennzahlen {
            let cde = obis.split_once(':').map_or("", |(_, rest)| rest);
            if obis.starts_with("7-") && ZAEHLER_OBIS_MIT_STELLEN.contains(&cde) {
                f.push(format!(
                    "Zähler {id} OBIS {obis}: its Wertegranularität and Vor-/Nachkommastellen \
                     (SG10 CCI+++ZE4, CCI+11+Z33, [274]) are not held"
                ));
                continue;
            }
            out.push(block(
                "Z20",
                vec![
                    seg("RFF", vec![vec!["MG", nummer]]),
                    seg("RFF", vec![vec!["Z19", &melo.melo_id]]),
                    seg("PIA", vec![vec!["5"], vec![obis, "SRW"]]),
                ],
            ));
        }
    }
    out
}

/// `CAV+<satz>` of a Konzessionsabgabe, with the EUR/kWh where the satz is a
/// Sonderregelung (Bedingung `[128]`: TAS/TKS/SAS/KAS), at most six decimals
/// (`[912]`).
fn konzessionsabgabe(
    zw: &mako_markt::utilmd_stammdaten::MaloZaehlwerk,
    f: &mut Fehlend,
) -> Option<Vec<String>> {
    let obis = &zw.obis_kennzahl;
    let satz = match zw.konzessionsabgabe_satz {
        Some(s) => s,
        None => {
            f.push(format!(
                "OBIS {obis}: konzessionsabgabe.satz (SG8 SEQ+Z07 CAV Konzessionsabgabe)"
            ));
            return None;
        }
    };
    let (code, mit_preis) = match satz {
        AbgabeArt::Kas => ("KAS", true),
        AbgabeArt::Sas => ("SAS", true),
        AbgabeArt::Tas => ("TAS", true),
        AbgabeArt::Tks => ("TKS", true),
        AbgabeArt::Sa => ("SA", false),
        AbgabeArt::Ta => ("TA", false),
        AbgabeArt::Tk => ("TK", false),
        other => {
            f.push(format!(
                "OBIS {obis}: konzessionsabgabe.satz {} is no UTILMD Gas KA code",
                wire(&other)
            ));
            return None;
        }
    };
    if !mit_preis {
        return Some(vec![code.to_owned()]);
    }
    match zw.konzessionsabgabe_eur_kwh {
        Some(k) if k.scale() <= 6 && !k.is_sign_negative() => Some(vec![
            code.to_owned(),
            String::new(),
            String::new(),
            k.normalize().to_string(),
        ]),
        Some(k) => {
            f.push(format!(
                "OBIS {obis}: konzessionsabgabe.kosten {k} EUR/kWh takes at most six decimals \
                 and is ≥ 0 ([912], [902])"
            ));
            None
        }
        None => {
            f.push(format!(
                "OBIS {obis}: konzessionsabgabe.kosten EUR/kWh (Muss beside {code}, [128])"
            ));
            None
        }
    }
}

/// The `NAD` qualifiers of one Sparte's `SG12`.
struct Qualifier {
    kunde: &'static str,
    korrespondenz: &'static str,
    marktlokation: &'static str,
    ablesekarte: Option<&'static str>,
}

/// `SG12` — the Kunde des LF (Muss on the Gründe of `[203]`, otherwise sent
/// when known), his Korrespondenzanschrift beside him (`[219]`/`[213]`), on
/// Gas the Name und Adresse für die Ablesekarte composed from the two
/// (`[573]`), and the Marktlokationsanschrift (Muss).
fn kunde_und_anschrift(
    sd: &MaloUtilmdStammdaten,
    transaktionsgrund: &str,
    q: Qualifier,
    out: &mut Stammdaten,
    f: &mut Fehlend,
) {
    let muss = GRUENDE_MIT_KUNDE.contains(&transaktionsgrund);
    // The Ablesekarte follows the Marktlokationsanschrift in the MIG.
    let mut ablesekarte = Vec::new();
    match (sd.kunde.as_ref(), muss) {
        (Some(kunde), _) => {
            let name = name_of(kunde);
            let adresse = kunde.anschrift.as_ref().and_then(anschrift_elemente);
            match (name, adresse) {
                (Some((parts, format)), Some(adresse)) => {
                    let c080 = c080(&parts, format);
                    out.sg12.push(nad(q.kunde, vec![], c080.clone(), None));
                    out.sg12
                        .push(nad(q.korrespondenz, vec![], c080.clone(), Some(&adresse)));
                    if let Some(qual) = q.ablesekarte {
                        for melo in &sd.messlokationen {
                            ablesekarte.push(nad(qual, vec![], c080.clone(), Some(&adresse)));
                            ablesekarte.push(seg("RFF", vec![vec!["Z19", &melo.melo_id]]));
                        }
                    }
                }
                _ if !muss => {}
                (None, _) => f.push(format!(
                    "the Kunde's name — Nachname or Organisationsname (SG12 NAD+{}, Muss on \
                     Transaktionsgrund {transaktionsgrund}, [203])",
                    q.kunde
                )),
                (Some(_), None) => f.push(format!(
                    "the Kunde's Korrespondenzanschrift with Straße, Ort, PLZ and Land \
                     (SG12 NAD+{}, Muss beside NAD+{})",
                    q.korrespondenz, q.kunde
                )),
            }
        }
        (None, true) => f.push(format!(
            "the Kunde (Marktlokation.endkunde) — SG12 NAD+{}, Muss on Transaktionsgrund \
             {transaktionsgrund} ([203])",
            q.kunde
        )),
        (None, false) => {}
    }
    match sd.anschrift.as_ref().and_then(anschrift_elemente) {
        Some(adresse) => out
            .sg12
            .push(nad(q.marktlokation, vec![], vec![], Some(&adresse))),
        None => f.push(format!(
            "the Marktlokationsanschrift with Straße, Ort, PLZ and Land (Marktlokation.\
             lokationsadresse, SG12 NAD+{}, Muss)",
            q.marktlokation
        )),
    }
    out.sg12.extend(ablesekarte);
}

/// `C080` and `DE 3045`: a Firma as `Z02`, a person as `Z01`. Both AHB
/// columns list the first `DE 3036` alone, so the name rides it whole —
/// Vorname before Nachname, as UTILMD MIG Gas G1.2's example writes
/// `NAD+Z04+++Fank Müller:::::Z01`.
fn name_of(k: &Kunde) -> Option<(Vec<String>, &'static str)> {
    fn nonempty(s: &Option<String>) -> Option<&str> {
        s.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(firma) = nonempty(&k.organisationsname) {
        return Some((vec![firma.to_owned()], "Z02"));
    }
    let nachname = nonempty(&k.nachname)?;
    let name = match nonempty(&k.vorname) {
        Some(vorname) => format!("{vorname} {nachname}"),
        None => nachname.to_owned(),
    };
    Some((vec![name], "Z01"))
}

/// The five `DE 3036` of `C080` and `DE 3045` after them.
fn c080(parts: &[String], format: &str) -> Vec<String> {
    let mut c = parts.to_vec();
    c.resize(5, String::new());
    c.push(format.to_owned());
    c
}

/// `C059` Straße und Hausnummer, `3164` Ort, `3251` PLZ, `3207` Land —
/// `None` unless all four are held. Both AHB columns list the first `DE 3042`
/// alone, so Straße and Hausnummer ride it together; the Ortsteil has no
/// place the columns list and is not sent.
fn anschrift_elemente(a: &Anschrift) -> Option<[Vec<String>; 4]> {
    let t = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let strasse = t(&a.strasse)?;
    let zeile = match t(&a.hausnummer) {
        Some(nr) => format!("{strasse} {nr}"),
        None => strasse,
    };
    Some([
        vec![zeile],
        vec![t(&a.ort)?],
        vec![t(&a.postleitzahl)?],
        vec![t(&a.landescode)?],
    ])
}

/// One `SG12 NAD`: `3035`, `C082`, `C058`, `C080` and — with an address —
/// `C059`, `3164`, `C819`, `3251`, `3207`.
fn nad(
    qualifier: &str,
    c058: Vec<String>,
    c080: Vec<String>,
    adresse: Option<&[Vec<String>; 4]>,
) -> StammdatenSegment {
    let mut elemente = vec![vec![qualifier.to_owned()], vec![], c058, c080];
    if let Some([c059, ort, plz, land]) = adresse {
        elemente.extend([c059.clone(), ort.clone(), vec![], plz.clone(), land.clone()]);
    }
    StammdatenSegment {
        tag: "NAD".to_owned(),
        elemente,
    }
}

/// `(MP-ID, Rolle Z39/Z40/Z41, Grundlage Z19/Z20, gMSB MP-ID)` of a
/// Messlokation's MSB. The Rolle is asked for only where the column lists it
/// (Strom); Gas states the Grundlage alone.
fn msb_codes(
    melo: &MesslokationStammdaten,
    f: &mut Fehlend,
    mit_rolle: bool,
) -> Option<(String, &'static str, &'static str, String)> {
    let id = &melo.melo_id;
    let Some(m) = melo.messstellenbetrieb.as_ref() else {
        f.push(format!(
            "Messlokation {id}: no MSB on the Stichtag (SG10 CAV+Z91 Messstellenbetreiber)"
        ));
        return None;
    };
    let rolle = match (m.rolle, mit_rolle) {
        (Some(MsbRolle::Grundzustaendig), _) => Some("Z39"),
        (Some(MsbRolle::Wettbewerblich), _) => Some("Z40"),
        (Some(MsbRolle::Auffang), _) => Some("Z41"),
        (None, true) => {
            f.push(format!(
                "Messlokation {id}: msb_rolle (SG10 CAV+Z91 Z39/Z40/Z41)"
            ));
            None
        }
        (None, false) => Some(""),
    };
    let grundlage = match m.grundlage {
        Some(MsbGrundlage::Vertrag) => Some("Z19"),
        Some(MsbGrundlage::Weiterverpflichtung) => Some("Z20"),
        None => {
            f.push(format!(
                "Messlokation {id}: grundlage of the Messstellenbetrieb (SG10 CAV+Z91 Z19/Z20)"
            ));
            None
        }
    };
    let gmsb = f.need(
        m.gmsb_mp_id.as_deref(),
        &format!("Messlokation {id}: gmsb_mp_id (SG10 CAV+ZF0 grundzuständiger MSB)"),
    );
    Some((
        m.msb_mp_id.clone(),
        if mit_rolle { rolle? } else { "" },
        grundlage?,
        gmsb?.to_owned(),
    ))
}

/// The MSB every Messlokation of the Marktlokation shares.
fn gemeinsamer_msb(
    melos: &[MesslokationStammdaten],
    f: &mut Fehlend,
) -> Option<(String, &'static str, &'static str, String)> {
    let mut seen: Option<(String, &'static str, &'static str, String)> = None;
    let mut scratch = Fehlend::default();
    for melo in melos {
        let msb = msb_codes(melo, &mut scratch, true)?;
        match &seen {
            None => seen = Some(msb),
            Some(first) if first.0 == msb.0 && first.1 == msb.1 && first.2 == msb.2 => {}
            Some(first) => {
                f.push(format!(
                    "the Messlokationen name different MSBs ({} and {}): SG8 SEQ+Z98 names the \
                     Marktlokation's one",
                    first.0, msb.0
                ));
                return None;
            }
        }
    }
    seen
}

/// `Z52` iMS, `Z53` kME/mME, `Z68` keine Messung (UTILMD AHB Strom 2.2,
/// `SG10 CAV` Messtechnische Einordnung der Marktlokation).
fn messtechnische_einordnung(
    melos: &[MesslokationStammdaten],
    f: &mut Fehlend,
) -> Option<&'static str> {
    if melos.is_empty() {
        return Some("Z68");
    }
    let mut alle_ims = true;
    for melo in melos {
        if melo.zaehler.is_empty() {
            f.push(format!(
                "Messlokation {}: no Zähler (SG10 CCI+++Z83 Messtechnische Einordnung)",
                melo.melo_id
            ));
            return None;
        }
        for z in &melo.zaehler {
            match z.zaehlertyp {
                Some(Zaehlertyp::IntelligentesMesssystem) => {}
                Some(_) => alle_ims = false,
                None => {
                    f.push(format!(
                        "Zähler {}: zaehlertyp (SG10 CCI+++Z83 Messtechnische Einordnung)",
                        z.zaehler_id
                    ));
                    return None;
                }
            }
        }
    }
    Some(if alle_ims { "Z52" } else { "Z53" })
}

fn betrieb(b: Betriebszustand) -> &'static str {
    match b {
        Betriebszustand::Regelbetrieb => "ZD3",
        Betriebszustand::Gesperrt => "ZB9",
    }
}

fn variante(v: Abrechnungsvariante) -> &'static str {
    match v {
        Abrechnungsvariante::ArbeitspreisGrundpreis => "Z14",
        Abrechnungsvariante::ArbeitspreisLeistungspreis => "Z15",
    }
}

/// A BO4E enum's wire value, for a refusal.
fn wire<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned())
}

fn seg(tag: &str, elemente: Vec<Vec<&str>>) -> StammdatenSegment {
    StammdatenSegment {
        tag: tag.to_owned(),
        elemente: elemente
            .into_iter()
            .map(|e| e.into_iter().map(str::to_owned).collect())
            .collect(),
    }
}

fn block(seq: &str, segmente: Vec<StammdatenSegment>) -> StammdatenBlock {
    StammdatenBlock {
        seq: seq.to_owned(),
        segmente,
    }
}

/// The Muss facts marktd does not hold, collected so one refusal names all.
#[derive(Default)]
struct Fehlend(Vec<String>);

impl Fehlend {
    fn push(&mut self, what: String) {
        self.0.push(what);
    }

    fn need<T>(&mut self, v: Option<T>, what: &str) -> Option<T> {
        if v.is_none() {
            self.0.push(what.to_owned());
        }
        v
    }

    fn refuse(self, malo_id: &str) -> Result<(), DispatchError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(DispatchError::StammdatenUnvollstaendig {
                malo_id: malo_id.to_owned(),
                fehlend: self.0,
            })
        }
    }
}
