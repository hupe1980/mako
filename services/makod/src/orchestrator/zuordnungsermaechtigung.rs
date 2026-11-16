//! Inbound UTILMD 55071 / 55072 — the Zuordnungsermächtigung a BKV activates
//! or deactivates at this NB.
//!
//! The message is no MaBiS-Zählpunkt Anfrage: it names the (ZRT, BG, BK, LF)
//! key of BK6-24-174 Anlage 3 (MaBiS) Kap. 10.2.1 (S. 119) and a day, and the
//! NB keeps it as the Prüfgrundlage of `E_0622` Prüfschritt 60. [`registry`]
//! builds [`ZpLifecycleCommand::ReceiveZuordnungsermaechtigung`] from it; the
//! workflow hands the Ermächtigung to marktd as a `ProcessCompleted`.

use std::any::Any;

use edi_energy::messages::utilmd::UtilmdMessage;
use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_engine::{
    error::EngineError,
    message_adapter::{AdapterRegistry, FnAdapter},
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::FormatVersion,
};
use mako_mabis::{
    MabisZpLifecycleWorkflow, ZP_FAMILIEN, ZpLifecycleCommand, ZpSerie, ZpVorgang,
    ZuordnungsermaechtigungAngaben,
};

use super::adapters::{ahb_verdict, known_fvs, parse_dtm_datetime, prozesstag, sg4_dtm};

/// Whether `pid` is a Zuordnungsermächtigung Anfrage (55071 / 55072).
#[must_use]
pub fn ist_zuordnungsermaechtigung(pid: u32) -> bool {
    ZP_FAMILIEN
        .iter()
        .any(|f| f.serie == ZpSerie::Zuordnungsermaechtigung && f.anfrage == pid)
}

/// The business key a 55071 / 55072 process is spawned under.
///
/// The message names no Lokation, so the key the other lifecycle Anfragen use
/// is empty here. The BKV's `SG4 IDE+24` Vorgangsnummer identifies this one
/// Vorgang: a redelivery lands on the same key, a second Ermächtigung does not.
#[must_use]
pub fn geschaeftsschluessel(msg: &AnyMessage) -> String {
    let AnyMessage::Utilmd(u) = msg else {
        return String::new();
    };
    let bkv = u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("");
    let vorgang = u
        .transactions()
        .first()
        .and_then(|t| t.vorgangsnummer())
        .unwrap_or("");
    format!("zuordnungsermaechtigung|{bkv}|{vorgang}")
}

/// The [`AdapterRegistry`] that reads a 55071 / 55072 into
/// [`ZpLifecycleCommand::ReceiveZuordnungsermaechtigung`].
#[must_use]
pub fn registry() -> AdapterRegistry<MabisZpLifecycleWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        |fv: &FormatVersion| known_fvs().iter().any(|k| k.as_str() == fv.as_str()),
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for the Zuordnungsermächtigung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "Zuordnungsermächtigung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "Zuordnungsermächtigung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(|p| {
                    Pruefidentifikator::new(p.as_u32())
                        .map_err(|e| EngineError::Deserialization(format!("PID out of range: {e}")))
                })?;
            let vorgang = ZP_FAMILIEN
                .iter()
                .find(|f| f.serie == ZpSerie::Zuordnungsermaechtigung && f.anfrage == pid.as_u32())
                .map(|f| f.vorgang)
                .ok_or_else(|| {
                    EngineError::Deserialization(format!(
                        "Zuordnungsermächtigung adapter: PID {pid} is not 55071 / 55072"
                    ))
                })?;
            let (validation_passed, validation_errors) = ahb_verdict(msg);
            let party = |n: Option<&edi_energy::messages::segments::Nad>| {
                MarktpartnerCode::new(n.and_then(|n| n.party_id.as_deref()).unwrap_or(""))
            };
            Ok(ZpLifecycleCommand::ReceiveZuordnungsermaechtigung {
                pid,
                vorgang,
                sender: party(u.sender()),
                receiver: party(u.receiver()),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                angaben: angaben(u, vorgang)?,
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// What a 55071 / 55072 carries (UTILMD AHB Strom 2.2 Kap. 13.5): `SG12
/// NAD+VY` the Lieferant, `SG10 CCI+Z19` the Bilanzkreis, `CCI+Z20` the
/// Bilanzierungsgebiet, `CCI+15++Z21` with its `CAV` the Zeitreihentyp, and
/// `SG4 DTM+158` Bilanzierungsbeginn (55071) or `DTM+159` Bilanzierungsende
/// (55072) — the Deaktivierungsdatum, which MaBiS Kap. 10.3.2 Nr. 1 (S. 122)
/// puts at 00:00 on the 1. of a Bilanzierungsmonat.
///
/// A value the message lacks stays empty: the workflow refuses it by name.
///
/// # Errors
///
/// A `303` date that is no instant or begins no German day.
fn angaben(
    u: &UtilmdMessage,
    vorgang: ZpVorgang,
) -> Result<ZuordnungsermaechtigungAngaben, EngineError> {
    use edi_energy::utilmd_codes::{dtm, nad};
    let tx = u.transactions().first();
    let merkmale = || {
        tx.into_iter()
            .flat_map(|t| t.sequences.iter())
            .flat_map(|g| g.characteristics.iter())
    };
    let klasse = |k: &str| {
        merkmale()
            .find(|c| c.cci.category.as_deref() == Some(k))
            .and_then(|c| c.cci.characteristic_id.clone())
            .unwrap_or_default()
    };
    let zeitreihentyp = merkmale()
        .find(|c| {
            c.cci.category.as_deref() == Some("15")
                && c.cci.characteristic_id.as_deref() == Some("Z21")
        })
        .and_then(|c| c.values.first())
        .and_then(|v| v.value_code.clone())
        .unwrap_or_default();
    let qualifier = match vorgang {
        ZpVorgang::Aktivierung => dtm::BILANZIERUNGSBEGINN,
        ZpVorgang::Deaktivierung => dtm::BILANZIERUNGSENDE,
    };
    let datum =
        prozesstag(sg4_dtm(tx, qualifier), mako_fristen::Sparte::Strom)?.unwrap_or_default();
    let nachrichtendatum = u
        .dtm()
        .iter()
        .find(|d| d.is_document_date())
        .and_then(|d| parse_dtm_datetime(d.value.as_deref()?, d.format.as_deref()))
        .map(|instant| {
            let tag = mako_fristen::berlin_date(instant);
            format!("{:04}{:02}{:02}", tag.year(), tag.month() as u8, tag.day())
        })
        .unwrap_or_default();
    Ok(ZuordnungsermaechtigungAngaben {
        lf_mp_id: tx
            .and_then(|t| t.party(nad::ZUGEHOERIGE_PARTEI))
            .and_then(|p| p.nad.party_id.clone())
            .unwrap_or_default(),
        bilanzkreis: klasse("Z19"),
        bilanzierungsgebiet: klasse("Z20"),
        zeitreihentyp,
        datum,
        nachrichtendatum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 55071 / 55072 as UTILMD AHB Strom 2.2 Kap. 13.5 lays it out.
    fn utilmd(pid: u32, datum_qualifier: &str) -> AnyMessage {
        let raw = format!(
            "UNA:+.? 'UNB+UNOC:3+9900000000019:500+9900357000004:500+260701:1000+1'\
             UNH+1+UTILMD:D:11A:UN:S2.2'BGM+Z17+DOK1'DTM+137:202607010800?+00:303'\
             NAD+MS+9900000000019::293'NAD+MR+9900357000004::293'\
             IDE+24+VORGANG1'DTM+{datum_qualifier}:202607312200?+00:303'\
             RFF+Z13:{pid}'SEQ+Z22'CCI+Z20++11YB-TENNET----U'CCI+Z19++11XBK-LF-------9'\
             CCI+15++Z21'CAV+SLS'NAD+VY+9900357000011::293'UNT+15+1'UNZ+1+1'"
        );
        edi_energy::parse(raw.as_bytes()).expect("the 55071 parses")
    }

    fn dispatch(msg: &AnyMessage) -> ZpLifecycleCommand {
        let fv = FormatVersion::parse("FV2026-10-01").expect("known");
        registry()
            .dispatch(msg as &dyn Any, &fv)
            .expect("dispatched")
    }

    /// The four parts of the key, the BKV and the German day of `DTM+158`
    /// reach the command — 31.07. 22:00 UTC is 01.08. in Berlin.
    #[test]
    fn a_55071_reads_its_key_and_its_day() {
        let ZpLifecycleCommand::ReceiveZuordnungsermaechtigung {
            pid,
            vorgang,
            sender,
            angaben,
            ..
        } = dispatch(&utilmd(55071, "158"))
        else {
            panic!("the Zuordnungsermächtigung command");
        };
        assert_eq!(pid.as_u32(), 55071);
        assert_eq!(vorgang, ZpVorgang::Aktivierung);
        assert_eq!(sender.as_str(), "9900000000019");
        assert_eq!(
            angaben,
            ZuordnungsermaechtigungAngaben {
                lf_mp_id: "9900357000011".to_owned(),
                bilanzkreis: "11XBK-LF-------9".to_owned(),
                bilanzierungsgebiet: "11YB-TENNET----U".to_owned(),
                zeitreihentyp: "SLS".to_owned(),
                datum: "20260801".to_owned(),
                nachrichtendatum: "20260701".to_owned(),
            }
        );
    }

    /// A 55072 reads its Deaktivierungsdatum from `DTM+159`, not `158`.
    #[test]
    fn a_55072_reads_its_deaktivierungsdatum() {
        let ZpLifecycleCommand::ReceiveZuordnungsermaechtigung {
            vorgang, angaben, ..
        } = dispatch(&utilmd(55072, "159"))
        else {
            panic!("the Zuordnungsermächtigung command");
        };
        assert_eq!(vorgang, ZpVorgang::Deaktivierung);
        assert_eq!(angaben.datum, "20260801");
        assert_eq!(
            geschaeftsschluessel(&utilmd(55072, "159")),
            "zuordnungsermaechtigung|9900000000019|VORGANG1"
        );
        assert!(ist_zuordnungsermaechtigung(55072));
        assert!(!ist_zuordnungsermaechtigung(55062));
    }
}
