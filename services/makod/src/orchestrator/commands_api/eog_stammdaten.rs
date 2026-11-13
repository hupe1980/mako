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
//! `SG5`/`SG6`/`SG8`/`SG12` segments. The E/G's Bestätigung **44014** (LF →
//! NB, same Kap. 5.9) restates the record with the LF's Bilanzkreis and
//! Abrechnungsintervall beside it ([`gas_44014`]); the Strom Bestätigung
//! **55014** its Gruppenzuordnung and the Kunde des LF ([`strom_55014`]).
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
//!
//! The same record answers a GeLi Gas **Anfrage zur Stammdatenänderung**: on
//! `ZG2` the NB's Antwort **44142** carries „die zum Änderungsdatum gültigen
//! Stammdaten" (EBD 4.3 Codeliste `G_0031`, S. 742) for each group the Anfrage
//! filled (UTILMD AHB Gas 1.2 Kap. 5.12.1, `[202]` ∧ `[336]`) — [`gas_44142`];
//! the **44157** its Prognosegrundlage ([`gas_44157`], Kap. 5.12.2); the
//! **44181**/**44182** its Messlokationen ([`gas_malo_struktur`], Kap.
//! 5.12.3). The Lieferant's Antworten **44138** and **44151** (Kap. 5.12.4,
//! 5.12.5) restate the LF's own facts, which the command states
//! ([`lf_44138`], [`lf_44151`]).

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
/// „Kunde des Lieferanten" is Muss, on the others Soll „Wenn bekannt"
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

/// Refuse a command whose outbound message the pre-send AHB gate would
/// dead-letter.
///
/// The workflow is pure, so its `handle` on a fresh process yields the exact
/// outbox entries the spawn writes; each EDIFACT message among them — every
/// type the outbox worker renders against a BDEW profile, not only UTILMD — is
/// rendered in the release of `format_version` (the process's own, or for a
/// spawn the one it is stamped with), parsed back and checked with
/// `is_sendable()` as `transport::as4_sender` checks it before delivery. A
/// refusal names every finding.
///
/// It is also where a rehearsal stops: inside [`super::probe::rehearse`] the
/// door's outbox is handed to the probe and the door ends here with
/// [`DispatchError::Geprobt`], before its duplicate guard and its spawn.
pub(super) fn refuse_what_the_gate_refuses(
    state: &CommandsApiState,
    format_version: &mako_engine::version::FormatVersion,
    outbox: &[PendingOutbox],
) -> Result<(), DispatchError> {
    if super::probe::halt(format_version, outbox) {
        return Err(DispatchError::Geprobt);
    }
    gate_verdict(state, format_version, outbox)
}

/// The gate's own answer on `outbox` — the refusal a door returns, word for
/// word, and the verdict a rehearsal reports.
pub(super) fn gate_verdict(
    state: &CommandsApiState,
    format_version: &mako_engine::version::FormatVersion,
    outbox: &[PendingOutbox],
) -> Result<(), DispatchError> {
    for pending in outbox {
        let message_type = &*pending.message_type;
        let findings = if message_type == "UTILMD" {
            crate::orchestrator::edifact_renderer::utilmd_ahb_findings(
                pending,
                state.tenant_id,
                format_version,
            )
        } else if GATED_MESSAGE_TYPES.contains(&message_type) {
            edifact_findings(state, pending, format_version)
        } else {
            continue;
        }
        .map_err(|e| {
            DispatchError::InvalidPayload(format!("the {message_type} does not render: {e}"))
        })?;
        if !findings.is_empty() {
            return Err(DispatchError::InvalidPayload(format!(
                "the {message_type} violates its AHB profile: {}",
                findings.join("; ")
            )));
        }
    }
    Ok(())
}

/// The message types the outbox worker renders against a BDEW profile beside
/// UTILMD. The DVGW documents (ALOCAT, NOMINT, NOMRES, SSQNOT) have no BDEW
/// AHB to hold them to, and a type with no renderer leaves the outbox as JSON.
const GATED_MESSAGE_TYPES: &[&str] = &[
    "APERAK", "CONTRL", "REQOTE", "QUOTES", "ORDERS", "ORDCHG", "ORDRSP", "INVOIC", "REMADV",
    "MSCONS", "IFTSTA", "INSRPT",
];

/// Render `pending` as the outbox worker will — its own `sender`, else this
/// deployment's, as the party; the release of `format_version` — parse it back
/// and return what keeps it from being sent.
fn edifact_findings(
    state: &CommandsApiState,
    pending: &PendingOutbox,
    format_version: &mako_engine::version::FormatVersion,
) -> Result<Vec<String>, String> {
    use edi_energy::EdiEnergyMessage as _;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId};
    let sender = pending
        .payload
        .get("sender")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&state.sender_party_id);
    let registry =
        crate::party_registry::MpIdRegistry::from_config(&[crate::config::PartyConfig {
            mp_id: sender.to_owned(),
            roles: Vec::new(),
            primary: true,
            agency: None,
            as4: None,
        }])
        .map_err(|e| e.to_string())?;
    let process_id = ProcessId::new();
    let mut msg = mako_engine::outbox::OutboxMessage::new(
        StreamId::for_process(state.tenant_id, &process_id),
        process_id,
        state.tenant_id,
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        pending.message_type.clone(),
        pending.recipient.clone(),
        pending.payload.clone(),
    );
    msg.format_version = Some(format_version.clone());
    let rendered = crate::orchestrator::edifact_renderer::render_to_wire_bytes(&msg, &registry)
        .map_err(|e| e.to_string())?;
    let parsed = edi_energy::parse(&rendered.bytes).map_err(|e| e.to_string())?;
    // The sender's role decides an AHB role Bedingung; this door knows it when
    // the deployment plays one AHB role. A combined deployment leaves it
    // undecided here, and the outbox's pre-send gate decides it per MP-ID.
    let mut rollen = state
        .configured_marktrollen
        .iter()
        .filter_map(|r| crate::party_registry::ahb_rolle(r));
    let absender = rollen
        .next()
        .filter(|erste| rollen.all(|r| r == *erste))
        .map(str::to_owned);
    let report = parsed
        .validate_on_date_with_rollen(
            mako_fristen::heute(),
            &edi_energy::profile::Parteirollen {
                absender,
                empfaenger: None,
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(report
        .sending_refusals()
        .map(|e| e.message.to_string())
        .collect())
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
            marktlokation: Some("Z63"),
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

/// What a Bestätigung EoG 55014 takes from the Marktlokation's record.
#[derive(Debug)]
pub(super) struct Eog55014 {
    /// `SG8 SEQ+Z01 CCI+++Z15`/`Z18` Gruppenzuordnung, Muss (S. 136).
    pub haushaltskunde: bool,
    /// The `SG12` Kunde des LF and his Korrespondenzanschrift.
    pub stammdaten: Stammdaten,
}

/// The record a Strom Bestätigung EoG 55014 (UTILMD AHB Strom 2.2 Kap. 8.6,
/// LF → NB) restates: the Gruppenzuordnung nach EnWG in `SG8 SEQ+Z01` (Muss,
/// S. 136) and, Muss on the Gründe of `[203]` and sent when known otherwise
/// (`[165]`), `SG12 NAD+Z09` Kunde des LF with `NAD+Z04` his
/// Korrespondenzanschrift (`[213]`, S. 139–140). The column lists no `SG5`
/// Lokation and no Marktlokationsanschrift, so none is sent; the Bilanzkreis,
/// the Versorgungsart and the Grundlage nach EnFG are the E/G's own facts and
/// come with the command.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every Muss fact marktd
/// does not hold.
pub(super) fn strom_55014(
    sd: &MaloUtilmdStammdaten,
    transaktionsgrund: &str,
) -> Result<Eog55014, DispatchError> {
    let mut f = Fehlend::default();
    let mut out = Stammdaten::default();
    let haushaltskunde = f
        .need(
            sd.netznutzung.as_ref(),
            "Netznutzungsdaten on the Stichtag (PUT /api/v1/malos/{id}/netznutzungsdaten)",
        )
        .and_then(|n| {
            f.need(
                n.haushaltskunde,
                "haushaltskunde (SG10 CCI+++Z15/Z18 Gruppenzuordnung)",
            )
        });
    kunde_und_anschrift(
        sd,
        transaktionsgrund,
        Qualifier {
            kunde: "Z09",
            korrespondenz: "Z04",
            marktlokation: None,
            ablesekarte: None,
        },
        &mut out,
        &mut f,
    );
    f.refuse(&sd.malo_id)?;
    haushaltskunde
        .map(|haushaltskunde| Eog55014 {
            haushaltskunde,
            stammdaten: out,
        })
        .ok_or_else(|| DispatchError::StammdatenUnvollstaendig {
            malo_id: sd.malo_id.clone(),
            fehlend: vec!["haushaltskunde (SG10 CCI+++Z15/Z18 Gruppenzuordnung)".to_owned()],
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
    gas_eog(sd, transaktionsgrund, None)
}

/// What the E/G Lieferant states on a Bestätigung EoG 44014 beside the
/// Marktlokation's record — its own facts, which marktd does not hold.
#[derive(Debug)]
pub(super) struct LfEogFakten<'a> {
    /// `SG10 CCI+Z19` — the LF's Bilanzkreis, Muss with `DTM+158` (Bedingungen
    /// `[2061]` ∧ `[64]`).
    pub bilanzkreis: &'a str,
    /// `SG6 DTM+Z20` Abrechnungsintervall des LF, in Monaten (DE 2379 `802`).
    pub abrechnungsintervall_monate: u8,
}

/// The Stammdaten of a Gas Bestätigung EoG 44014 (UTILMD AHB Gas 1.2 Kap.
/// 5.9, LF → NB): the record the 44013 carried, as marktd holds it on the
/// Stichtag, with the LF's Bilanzkreis and Abrechnungsintervall. The 44014's
/// column lists no `CCI+Z42` Betriebszustand, so none is sent.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every Muss fact marktd
/// does not hold.
pub(super) fn gas_44014(
    sd: &MaloUtilmdStammdaten,
    transaktionsgrund: &str,
    lf: &LfEogFakten<'_>,
) -> Result<Stammdaten, DispatchError> {
    gas_eog(sd, transaktionsgrund, Some(lf))
}

/// The shared body of [`gas_44013`] and [`gas_44014`].
fn gas_eog(
    sd: &MaloUtilmdStammdaten,
    transaktionsgrund: &str,
    lf: Option<&LfEogFakten<'_>>,
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
            if let Some(lf) = lf {
                out.sg6.push(seg(
                    "DTM",
                    vec![vec![
                        "Z20",
                        &lf.abrechnungsintervall_monate.to_string(),
                        "802",
                    ]],
                ));
            }
            out.sg6.push(seg("DTM", vec![vec!["Z21", termin, "106"]]));
            out.sg6
                .push(seg("DTM", vec![vec!["Z09", &jahr.to_string(), "602"]]));
            out.sg6
                .push(seg("DTM", vec![vec!["Z22", &intervall.to_string(), "802"]]));
        }
    }

    // `SG8 SEQ+Z01` Daten der Marktlokation — on a 44014 headed by the LF's
    // Bilanzkreis.
    let mut z01 = Vec::new();
    if let Some(lf) = lf {
        z01.push(seg("CCI", vec![vec!["Z19"], vec![], vec![lf.bilanzkreis]]));
    }
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
        match (n.betriebszustand, lf) {
            (_, Some(_)) => {}
            (Some(b), None) => z01.push(seg("CCI", vec![vec!["Z42"], vec![], vec![betrieb(b)]])),
            (None, None) => {
                f.push("betriebszustand (SG10 CCI+Z42 Betriebszustand)".to_owned());
            }
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
            marktlokation: Some("DP"),
            ablesekarte: Some("Z05"),
        },
        &mut out,
        &mut f,
    );

    f.refuse(&sd.malo_id)?;
    Ok(out)
}

/// The Stammdaten of a GeLi Gas Antwort auf Anfrage **44142** (UTILMD AHB Gas
/// 1.2 Kap. 5.12.1, NB → LF / MSB) with `STS+E01+ZG2`: each group in
/// `angefragt`, as marktd holds it on the Änderungsdatum, in the MIG's order.
///
/// | Group | Segments | Column |
/// |---|---|---|
/// | Termine | `SG6 RFF+Z18`, `DTM+Z21` with `DTM+Z09` (`[24]`), `DTM+Z22` | Soll `[202]` ∧ `[336]` |
/// | Daten der Marktlokation | `SEQ+Z01`: `CCI+Z21` Marktgebiet, `CCI+++Y01` Druckebene, the Muss `CCI+++ZB0`/`ZB1` Prognosegrundlage, `CCI+++Z88` Netznutzung | Soll `[92]`, its `SG10` Soll `[202]` ∧ `[336]` |
/// | OBIS, Konzessionsabgabe, Gemeinderabatt | `SEQ+Z02` — every OBIS of the Marktlokation (`[530]`) —, `SEQ+Z07`, `SEQ+Z12` | Soll `[25]` ∧ `[202]` ∧ `[336]` |
/// | Messlokation | `SEQ+Z18` `RFF+Z19` with `CCI+++ZB3` Zugeordnete Marktpartner and/or `CCI+++Y02` Gasqualität | Soll `[202]` ∧ `[336]` |
/// | Marktlokationsanschrift | `SG12 NAD+DP` | Soll `[202]` ∧ `[336]` |
///
/// | Turnusablesung des NB | `SG6 RFF+Z19` with `DTM+752` for each Messlokation | Soll `[336]` ∧ `[202]` |
/// | Netznutzung | `CCI+++Z88` with `CAV+Z74`, `CAV+Z73`, `CAV+ZB1` | Muss in the group |
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every fact marktd does
/// not hold.
pub(super) fn gas_44142(
    sd: &MaloUtilmdStammdaten,
    angefragt: &[mako_geli_gas::AngefragteGruppe],
) -> Result<Stammdaten, DispatchError> {
    gas_marktlokation_gruppen(sd, angefragt, true)
}

/// The Stammdaten of the GNB's nicht bilanzierungsrelevante Änderung
/// **44112** (UTILMD AHB Gas 1.2 Kap. 5.10.1, S. 97–110, NB → LF): the groups
/// of [`gas_44142`] that changed, each Soll `[92]` „Wenn Wert innerhalb SG bzw.
/// Segment geändert wird". Its `SEQ+Z01` lists Marktgebiet (`CCI+Z21`, S. 101),
/// Druckebene (`CCI+++Y01`) and Netznutzung (`CCI+++Z88`, S. 102) and no
/// Prognosegrundlage, which the 44142 restates as Muss.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every fact marktd does
/// not hold.
pub(super) fn gas_44112(
    sd: &MaloUtilmdStammdaten,
    geaendert: &[mako_geli_gas::AngefragteGruppe],
) -> Result<Stammdaten, DispatchError> {
    gas_marktlokation_gruppen(sd, geaendert, false)
}

/// [`gas_44142`] and [`gas_44112`]; `prognosegrundlage` says whether the
/// column's `SEQ+Z01` carries the `CCI+++ZB0`/`ZB1`.
fn gas_marktlokation_gruppen(
    sd: &MaloUtilmdStammdaten,
    angefragt: &[mako_geli_gas::AngefragteGruppe],
    prognosegrundlage: bool,
) -> Result<Stammdaten, DispatchError> {
    use mako_geli_gas::AngefragteGruppe as G;
    let hat = |g: G| angefragt.contains(&g);
    let mut f = Fehlend::default();
    let mut out = Stammdaten::default();

    // `SG6 RFF+Z18` Termine der Marktlokation.
    let termin = hat(G::TerminNetznutzungsabrechnung);
    let intervall = hat(G::Netznutzungsabrechnungsintervall);
    if termin || intervall {
        let mut dtm = Vec::new();
        match sd.netznutzung.as_ref() {
            None => f.push(
                "Netznutzungsdaten on the Änderungsdatum (PUT /api/v1/malos/{id}/netznutzungsdaten)"
                    .to_owned(),
            ),
            Some(n) => {
                if termin {
                    let t = f.need(
                        n.abrechnungstermin.as_deref(),
                        "abrechnungstermin MMDD (SG6 DTM+Z21 Termin der Netznutzungsabrechnung)",
                    );
                    let j = f.need(
                        n.naechste_abrechnung_jahr,
                        "naechste_abrechnung_jahr CCYY (SG6 DTM+Z09, Muss beside DTM+Z21, [24])",
                    );
                    if let (Some(t), Some(j)) = (t, j) {
                        dtm.push(seg("DTM", vec![vec!["Z21", t, "106"]]));
                        dtm.push(seg("DTM", vec![vec!["Z09", &j.to_string(), "602"]]));
                    }
                }
                if intervall
                    && let Some(i) = f.need(
                        n.abrechnungsintervall_monate,
                        "abrechnungsintervall_monate (SG6 DTM+Z22 Netznutzungsabrechnungsintervall)",
                    )
                {
                    dtm.push(seg("DTM", vec![vec!["Z22", &i.to_string(), "802"]]));
                }
            }
        }
        if !dtm.is_empty() {
            out.sg6.push(seg("RFF", vec![vec!["Z18", &sd.malo_id]]));
            out.sg6.extend(dtm);
        }
    }
    // `SG6 RFF+Z19` `DTM+752` Turnusablesung des NB — „Der Termin der
    // Turnusablesung wird an der Marktlokation vorgegeben und muss an den
    // zugordneten Messlokationen eingehalten werden" (UTILMD MIG Gas G1.2,
    // S. 60), so marktd's one value is stated for each Messlokation.
    if hat(G::Turnusablesung) {
        match sd
            .netznutzung
            .as_ref()
            .and_then(|n| n.turnusablesung.as_deref())
            .and_then(turnusablesung)
        {
            None => f.push(
                "turnusablesung MMDD or MMWWMMWW (SG6 RFF+Z19 DTM+752 Turnusablesung des NB; \
                 PUT /api/v1/malos/{id}/netznutzungsdaten)"
                    .to_owned(),
            ),
            Some(_) if sd.messlokationen.is_empty() => f.push(
                "the Marktlokation's Messlokationen the Turnusablesung is stated for \
                 (SG6 RFF+Z19)"
                    .to_owned(),
            ),
            Some((wert, format)) => {
                for melo in &sd.messlokationen {
                    out.sg6.push(seg("RFF", vec![vec!["Z19", &melo.melo_id]]));
                    out.sg6.push(seg("DTM", vec![vec!["752", wert, format]]));
                }
            }
        }
    }

    // `SG8 SEQ+Z01` Daten der Marktlokation, when one of its groups is asked.
    if hat(G::Marktgebiet) || hat(G::Druckebene) || hat(G::Netznutzung) {
        let mut z01 = Vec::new();
        if hat(G::Marktgebiet)
            && let Some(gebiet) = f.need(
                sd.marktgebiet.as_deref(),
                "marktgebiet (SG10 CCI+Z21 Marktgebiet)",
            )
        {
            z01.push(seg("CCI", vec![vec!["Z21"], vec![], vec![gebiet]]));
        }
        if hat(G::Druckebene) {
            match f.need(sd.netzebene, "netzebene (SG10 CCI+++Y01 Druckebene)") {
                Some(Netzebene::Hd) => druckebene(&mut z01, "Y01"),
                Some(Netzebene::Md) => druckebene(&mut z01, "Y02"),
                Some(Netzebene::Nd) => druckebene(&mut z01, "Y03"),
                Some(other) => f.push(format!(
                    "netzebene {} is no Druckebene (SG10 CAV Y01/Y02/Y03)",
                    wire(&other)
                )),
                None => {}
            }
        }
        // `SG10 CCI+++ZB0`/`ZB1` „Bereits ausgetauschte Prognosegrundlage der
        // Marktlokation" — Muss in the 44142's group.
        match sd.bilanzierung.as_ref().and_then(|b| b.prognosegrundlage) {
            _ if !prognosegrundlage => {}
            Some(Prognosegrundlage::Werte) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["ZB0"]]));
            }
            Some(Prognosegrundlage::Profile) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["ZB1"]]));
            }
            other => f.push(format!(
                "prognosegrundlage {} (SG10 CCI+++ZB0/ZB1, Muss in SEQ+Z01)",
                other.map_or_else(|| "absent".to_owned(), |p| wire(&p))
            )),
        }
        // `SG10 CCI+++Z88` Netznutzung with its three Muss `CAV`: the
        // Netznutzungsvertrag (`Z74`), who pays (`Z73`, `Z10` only under `Z08`,
        // `[216]`) and the Netznutzungsabrechnungsvariante (`ZB1`).
        if hat(G::Netznutzung) {
            match sd.netznutzung.as_ref() {
                None => f.push(
                    "Netznutzungsdaten on the Änderungsdatum (SG10 CCI+++Z88 Netznutzung)"
                        .to_owned(),
                ),
                Some(n) => {
                    let vertrag = f.need(
                        n.netznutzungsvertrag,
                        "netznutzungsvertrag (SG10 CAV+Z74 Z08/Z09 Netznutzungsvertrag)",
                    );
                    let zahler = f.need(
                        n.netznutzung_zahler,
                        "netznutzung_zahler (SG10 CAV+Z73 Z10/Z11 Zahlung der Netznutzung)",
                    );
                    let abrechnung = f.need(
                        n.abrechnungsvariante,
                        "abrechnungsvariante (SG10 CAV+ZB1 Netznutzungsabrechnungsvariante)",
                    );
                    if let (Some(vertrag), Some(zahler), Some(abrechnung)) =
                        (vertrag, zahler, abrechnung)
                    {
                        z01.push(seg("CCI", vec![vec![], vec![], vec!["Z88"]]));
                        z01.push(seg("CAV", vec![vec!["Z74", "", "", vertrag.utilmd_code()]]));
                        z01.push(seg("CAV", vec![vec!["Z73", "", "", zahler.utilmd_code()]]));
                        z01.push(seg("CAV", vec![vec!["ZB1", "", "", variante(abrechnung)]]));
                    }
                }
            }
        }
        out.bloecke.push(block("Z01", z01));
    }

    // `SEQ+Z02` — „alle an dem Meldepunkt vorhandenen Daten … die zum Datum
    // „Änderung zum“ Gültigkeit haben" (`[530]`), so every OBIS.
    let obis_asked = hat(G::ObisDaten);
    if obis_asked {
        if sd.zaehlwerke.is_empty() {
            f.push("the Marktlokation's OBIS-Kennzahlen (SG8 SEQ+Z02)".to_owned());
        }
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
        }
    }
    if hat(G::Konzessionsabgabe) {
        let mut any = false;
        for zw in sd
            .zaehlwerke
            .iter()
            .filter(|zw| OBIS_MIT_KONZESSIONSABGABE.contains(&zw.obis_kennzahl.as_str()))
        {
            any = true;
            if let Some(cav) = konzessionsabgabe(zw, &mut f) {
                out.bloecke.push(block(
                    "Z07",
                    vec![
                        seg("RFF", vec![vec!["Z10", &zw.obis_kennzahl]]),
                        seg("CCI", vec![vec![], vec![], vec!["Z08"]]),
                        StammdatenSegment {
                            tag: "CAV".to_owned(),
                            elemente: vec![cav],
                        },
                    ],
                ));
            }
        }
        if !any {
            f.push(format!(
                "Konzessionsabgabedaten: marktd holds none of the OBIS {} at the Marktlokation \
                 (SG8 SEQ+Z07) — if it has none, the answer is ZG0",
                OBIS_MIT_KONZESSIONSABGABE.join(" / ")
            ));
        }
    }
    if hat(G::Gemeinderabatt) {
        match sd
            .netznutzung
            .as_ref()
            .and_then(|n| n.gemeinderabatt_prozent)
        {
            Some(p) => out.bloecke.push(block(
                "Z12",
                vec![seg(
                    "QTY",
                    vec![vec!["Z16", &p.normalize().to_string(), "P1"]],
                )],
            )),
            None => f.push("gemeinderabatt_prozent (SG8 SEQ+Z12 QTY+Z16)".to_owned()),
        }
    }

    // `SEQ+Z18` Daten der Messlokation.
    let msb = hat(G::ZugeordneteMarktpartner);
    let gas = hat(G::Gasqualitaet);
    if msb || gas {
        if sd.messlokationen.is_empty() {
            f.push("the Marktlokation's Messlokationen (SG8 SEQ+Z18 RFF+Z19)".to_owned());
        }
        let gasqualitaet = if gas {
            let code = sd.gasqualitaet.and_then(|g| match g {
                Gasqualitaet::HGas => Some("Y04"),
                Gasqualitaet::LGas => Some("Y05"),
                _ => None,
            });
            f.need(
                code,
                "gasqualitaet H-Gas/L-Gas (SG10 CCI+++Y02 CAV Y04/Y05)",
            )
        } else {
            None
        };
        for melo in &sd.messlokationen {
            let mut z18 = vec![seg("RFF", vec![vec!["Z19", &melo.melo_id]])];
            if msb && let Some(m) = msb_codes(melo, &mut f, false) {
                z18.push(seg("CCI", vec![vec![], vec![], vec!["ZB3"]]));
                z18.push(seg("CAV", vec![vec!["Z91", &m.0, "", m.2]]));
                z18.push(seg("CAV", vec![vec!["ZF0", &m.3]]));
            }
            if let Some(gq) = gasqualitaet {
                z18.push(seg("CCI", vec![vec![], vec![], vec!["Y02"]]));
                z18.push(seg("CAV", vec![vec![gq]]));
            }
            out.bloecke.push(block("Z18", z18));
        }
    }

    // `SG12 NAD+DP` Marktlokationsanschrift.
    if hat(G::Marktlokationsanschrift) {
        match sd.anschrift.as_ref().and_then(anschrift_elemente) {
            Some(adresse) => out.sg12.push(nad("DP", vec![], vec![], Some(&adresse))),
            None => f.push(
                "the Marktlokationsanschrift with Straße, Ort, PLZ and Land (SG12 NAD+DP)"
                    .to_owned(),
            ),
        }
    }

    f.refuse(&sd.malo_id)?;
    Ok(out)
}

/// `DTM+752` DE 2380 and the DE 2379 its shape names: `MMDD` is `106`,
/// `MMWWMMWW` is `104` (UTILMD MIG Gas G1.2, S. 60–61). marktd holds only
/// these two shapes; anything else is no value.
fn turnusablesung(wert: &str) -> Option<(&str, &'static str)> {
    if !wert.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match wert.len() {
        4 => Some((wert, "106")),
        8 => Some((wert, "104")),
        _ => None,
    }
}

/// The Stammdaten of the NB's Antwort **44157** auf eine bilanzierungsrelevante
/// Anfrage mit Abhängigkeiten zur Prognosegrundlage (UTILMD AHB Gas 1.2 Kap.
/// 5.12.2, S. 163–167, NB → LF) — on `ZG2` and on `ZG4` alike, since `ZG4`
/// „übermittelt die zum angefragten Datum gültigen Stammdaten" too (EBD 4.3
/// Codeliste `G_0034`, S. 743) and the column's `SG8 SEQ+Z01` is Muss without
/// a code condition:
///
/// | Segment | Column |
/// |---|---|
/// | `SG10 CCI+++ZC0`/`ZA6` Prognosegrundlage der Marktlokation | Muss |
/// | `SG10 CCI+++Z17` Fallgruppenzuordnung | Muss `[19]` behind `ZC0` |
/// | `SG9 QTY+31`/`Y02`, `SG8 SEQ+Z35` Lastprofildaten | Muss `[106]` behind `ZA6` |
///
/// A Prognose auf Basis von Profilen is refused by name: its `SEQ+Z35`
/// Lastprofil code and Klimazone / Temperaturmessstelle are not held.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every fact marktd does
/// not hold.
pub(super) fn gas_44157(sd: &MaloUtilmdStammdaten) -> Result<Stammdaten, DispatchError> {
    let mut f = Fehlend::default();
    let mut out = Stammdaten::default();
    let mut z01 = Vec::new();
    match sd.bilanzierung.as_ref() {
        None => f.push(
            "Bilanzierung on the Änderungsdatum (SG10 CCI+++ZC0/ZA6 Prognosegrundlage, Muss)"
                .to_owned(),
        ),
        Some(b) => match b.prognosegrundlage {
            Some(Prognosegrundlage::Werte) => {
                z01.push(seg("CCI", vec![vec![], vec![], vec!["ZC0"]]));
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
                    None => {
                        f.push("fallgruppe (SG10 CCI+++Z17 Fallgruppenzuordnung, [19])".to_owned());
                    }
                }
            }
            Some(Prognosegrundlage::Profile) => f.push(
                "a Prognose auf Basis von Profilen: the SG8 SEQ+Z35 Lastprofildaten (CCI+Z12 \
                 Lastprofil, CCI+Z99/ZA0 Klimazone / Temperaturmessstelle) and the SG9 \
                 QTY+31/Y02 it makes Muss ([106]) are not held"
                    .to_owned(),
            ),
            other => f.push(format!(
                "prognosegrundlage {} (SG10 CCI+++ZC0/ZA6, Muss)",
                other.map_or_else(|| "absent".to_owned(), |p| wire(&p))
            )),
        },
    }
    out.bloecke.push(block("Z01", z01));
    f.refuse(&sd.malo_id)?;
    Ok(out)
}

/// The `SG5` of the NB's Antwort **44181** / Ablehnung **44182** auf eine
/// Anfrage der Marktlokationsstruktur (UTILMD AHB Gas 1.2 Kap. 5.12.3, S. 168,
/// NB → LF): „Es ist die ID der Marktlokation, welche dem LF zugeordnet sind,
/// sowie alle Identifikatoren der Messlokationen anzugeben" (Bedingung
/// `[590]`, every column). The renderer writes the Marktlokation; these are
/// the Messlokationen marktd assigns it on the Änderungsdatum. The columns
/// carry no further group.
pub(super) fn gas_malo_struktur(sd: &MaloUtilmdStammdaten) -> Stammdaten {
    Stammdaten {
        lokationen: sd
            .messlokationen
            .iter()
            .map(|m| StammdatenLokation {
                qualifier: "172".to_owned(),
                id: m.melo_id.clone(),
            })
            .collect(),
        ..Stammdaten::default()
    }
}

/// The Lieferant's own facts its Antwort auf Anfrage restates — they are the
/// LF's, so the command states them; marktd is the NB's record.
#[derive(Debug, Default)]
pub(super) struct LfAnfrageFakten<'a> {
    /// `SG6 DTM+Z20` Abrechnungsintervall des LF, in Monaten.
    pub abrechnungsintervall_monate: Option<u8>,
    /// `SG10 CCI+++Z15`/`Z18` Gruppenzuordnung nach EnWG.
    pub haushaltskunde: Option<bool>,
    /// `SG12 NAD+Z09` Kunde des LF and, with its Anschrift, `NAD+Z04`.
    pub kunde: Option<&'a Kunde>,
    /// `SG10 CCI+Z19` Bilanzkreis.
    pub bilanzkreis: Option<&'a str>,
}

/// The Stammdaten of the Lieferant's Antwort **44138** auf eine nicht
/// bilanzierungsrelevante Anfrage an LF (UTILMD AHB Gas 1.2 Kap. 5.12.4,
/// S. 171–175, LF → NB) on `ZG2`, for each group the NB's 44137 filled
/// (Soll `[202]` ∧ `[336]`):
///
/// | Group | Segments |
/// |---|---|
/// | Abrechnungsintervall des LF | `SG6 RFF+Z18` `DTM+Z20` (`802`) |
/// | Gruppenzuordnung | `SG8 SEQ+Z01` `SG10 CCI+++Z15`/`Z18` |
/// | Kunde des LF | `SG12 NAD+Z09` — the name, `Z01` Person / `Z02` Firma |
/// | Korrespondenzanschrift | `SG12 NAD+Z04` — name and Straße, Ort, PLZ, Land |
///
/// # Errors
///
/// [`DispatchError::InvalidPayload`] naming every fact the command does not
/// state.
pub(super) fn lf_44138(
    malo_id: &str,
    angefragt: &[mako_geli_gas::AngefragteGruppe],
    lf: &LfAnfrageFakten<'_>,
) -> Result<Stammdaten, DispatchError> {
    use mako_geli_gas::AngefragteGruppe as G;
    let hat = |g: G| angefragt.contains(&g);
    let mut f = Fehlend::default();
    let mut out = Stammdaten::default();
    if hat(G::AbrechnungsintervallLf)
        && let Some(m) = f.need(
            lf.abrechnungsintervall_monate,
            "abrechnungsintervall_monate 1/3/6/12 (SG6 DTM+Z20 Abrechnungsintervall des LF)",
        )
    {
        out.sg6.push(seg("RFF", vec![vec!["Z18", malo_id]]));
        out.sg6
            .push(seg("DTM", vec![vec!["Z20", &m.to_string(), "802"]]));
    }
    if hat(G::Gruppenzuordnung)
        && let Some(h) = f.need(
            lf.haushaltskunde,
            "haushaltskunde (SG10 CCI+++Z15/Z18 Gruppenzuordnung nach EnWG)",
        )
    {
        out.bloecke.push(block(
            "Z01",
            vec![seg(
                "CCI",
                vec![vec![], vec![], vec![if h { "Z15" } else { "Z18" }]],
            )],
        ));
    }
    let kunde = hat(G::KundeDesLf);
    let anschrift = hat(G::KorrespondenzanschriftKundeLf);
    if kunde || anschrift {
        match lf.kunde.and_then(name_of) {
            None => f.push(
                "kunde with nachname or organisationsname (SG12 NAD+Z09 Kunde des LF / NAD+Z04 \
                 Korrespondenzanschrift)"
                    .to_owned(),
            ),
            Some((parts, format)) => {
                let c080 = c080(&parts, format);
                if kunde {
                    out.sg12.push(nad("Z09", vec![], c080.clone(), None));
                }
                if anschrift {
                    match lf
                        .kunde
                        .and_then(|k| k.anschrift.as_ref())
                        .and_then(anschrift_elemente)
                    {
                        Some(adresse) => out.sg12.push(nad("Z04", vec![], c080, Some(&adresse))),
                        None => f.push(
                            "kunde.anschrift with strasse, ort, postleitzahl and landescode \
                             (SG12 NAD+Z04 Korrespondenzanschrift des Kunden des LF)"
                                .to_owned(),
                        ),
                    }
                }
            }
        }
    }
    f.refuse_command("44138")?;
    Ok(out)
}

/// The Stammdaten of the Lieferant's Antwort **44151** auf eine
/// bilanzierungsrelevante Anfrage an LF (UTILMD AHB Gas 1.2 Kap. 5.12.5,
/// S. 176–177, LF → NB): `SG8 SEQ+Z01` with the Muss `SG10 CCI+Z19`
/// Bilanzkreis, Soll `[336]` — when the NB's 44150 filled it.
///
/// # Errors
///
/// [`DispatchError::InvalidPayload`] when the command states no Bilanzkreis.
pub(super) fn lf_44151(
    angefragt: &[mako_geli_gas::AngefragteGruppe],
    lf: &LfAnfrageFakten<'_>,
) -> Result<Stammdaten, DispatchError> {
    let mut f = Fehlend::default();
    let mut out = Stammdaten::default();
    if angefragt.contains(&mako_geli_gas::AngefragteGruppe::Bilanzkreis)
        && let Some(bk) = f.need(
            lf.bilanzkreis.map(str::trim).filter(|s| !s.is_empty()),
            "bilanzkreis (SG10 CCI+Z19 Bilanzkreis)",
        )
    {
        out.bloecke.push(block(
            "Z01",
            vec![seg("CCI", vec![vec!["Z19"], vec![], vec![bk]])],
        ));
    }
    f.refuse_command("44151")?;
    Ok(out)
}

/// `SG10 CCI+++Y01` Druckebene der Marktlokation and its `CAV`.
fn druckebene(z01: &mut Vec<StammdatenSegment>, code: &str) {
    z01.push(seg("CCI", vec![vec![], vec![], vec!["Y01"]]));
    z01.push(seg("CAV", vec![vec![code]]));
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
    /// `None` where the column lists no Marktlokationsanschrift (the 55014).
    marktlokation: Option<&'static str>,
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
    if let Some(qual) = q.marktlokation {
        match sd.anschrift.as_ref().and_then(anschrift_elemente) {
            Some(adresse) => out.sg12.push(nad(qual, vec![], vec![], Some(&adresse))),
            None => f.push(format!(
                "the Marktlokationsanschrift with Straße, Ort, PLZ and Land (Marktlokation.\
                 lokationsadresse, SG12 NAD+{qual}, Muss)"
            )),
        }
    }
    out.sg12.extend(ablesekarte);
}

/// `C080` and `DE 3045`: a Firma as `Z02`, a person as `Z01`. Both AHB
/// columns list the first `DE 3036` alone, so the name rides it whole —
/// Vorname before Nachname, as UTILMD MIG Gas G1.2's example writes
/// `NAD+Z04+++Fank Müller:::::Z01`.
pub(super) fn name_of(k: &Kunde) -> Option<(Vec<String>, &'static str)> {
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
pub(super) fn c080(parts: &[String], format: &str) -> Vec<String> {
    let mut c = parts.to_vec();
    c.resize(5, String::new());
    c.push(format.to_owned());
    c
}

/// `C059` Straße und Hausnummer, `3164` Ort, `3251` PLZ, `3207` Land —
/// `None` unless all four are held. Both AHB columns list the first `DE 3042`
/// alone, so Straße and Hausnummer ride it together; the Ortsteil has no
/// place the columns list and is not sent.
pub(super) fn anschrift_elemente(a: &Anschrift) -> Option<[Vec<String>; 4]> {
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
pub(super) fn nad(
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
pub(super) fn msb_codes(
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
pub(super) fn gemeinsamer_msb(
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
pub(super) fn messtechnische_einordnung(
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
pub(super) fn wire<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned())
}

pub(super) fn seg(tag: &str, elemente: Vec<Vec<&str>>) -> StammdatenSegment {
    StammdatenSegment {
        tag: tag.to_owned(),
        elemente: elemente
            .into_iter()
            .map(|e| e.into_iter().map(str::to_owned).collect())
            .collect(),
    }
}

pub(super) fn block(seq: &str, segmente: Vec<StammdatenSegment>) -> StammdatenBlock {
    StammdatenBlock {
        seq: seq.to_owned(),
        segmente,
    }
}

/// The Muss facts marktd does not hold, collected so one refusal names all.
#[derive(Default)]
pub(super) struct Fehlend(Vec<String>);

impl Fehlend {
    pub(super) fn push(&mut self, what: String) {
        self.0.push(what);
    }

    pub(super) fn need<T>(&mut self, v: Option<T>, what: &str) -> Option<T> {
        if v.is_none() {
            self.0.push(what.to_owned());
        }
        v
    }

    /// Refuse a command that does not state a fact its Antwort `pid` must
    /// carry — the facts are the sender's own, so the caller is named.
    fn refuse_command(self, pid: &str) -> Result<(), DispatchError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(DispatchError::InvalidPayload(format!(
                "the Antwort {pid} restates facts of the sender's own the command must state: {}",
                self.0.join("; ")
            )))
        }
    }

    pub(super) fn refuse(self, malo_id: &str) -> Result<(), DispatchError> {
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
