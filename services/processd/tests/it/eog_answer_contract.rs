//! The E/G Lieferant's automatic Bestätigung EoG Gas 44014, read through the
//! request type makod's `geli.eog.bestaetigen` door reads it as
//! ([`mako_service::json_from_value`] refuses an undeclared key exactly as
//! makod does).
//!
//! UTILMD AHB Gas 1.2 Kap. 5.9 makes the LF's Bilanzkreis (`SG10 CCI+Z19`)
//! and its Abrechnungsintervall (`SG6 DTM+Z20`) Muss on the 44014, and makod
//! refuses a Bestätigung without either. The body has to carry both from
//! processd's own configuration; a key the door does not declare — the Strom
//! 55014's `versorgungsart` — is refused the same way.

use mako_markt::commands::{GELI_EOG_BESTAETIGEN, GeliEogBestaetigen};
use mako_markt::domain::Sparte;
use processd::config::BilanzkreisEintrag;
use processd::lf_module::{
    ANMELDUNG_EOG_GAS, EogFakten, LfAnfragePayload, LfModuleConfig, answer_payload,
};
use secrecy::SecretString;

const MALO: &str = "51238696012";
/// The Marktlokation's Bilanzierungsgebiet, as marktd holds it.
const BG: &str = "11YN-BG-EON----A";
/// Gas Bilanzkreis codes as the LF's own facts — synthetic, no assignment.
const BK_BG: &str = "BKGAS0001";
const BK_FALLBACK: &str = "BKGAS0002";
/// The same supplier's Strom Bilanzkreis — never on a Gas answer.
const BK_STROM: &str = "BKSTROM0001";

fn config(intervall: Option<u8>) -> LfModuleConfig {
    LfModuleConfig {
        marktd_url: String::new(),
        marktd_api_key: SecretString::from(String::new()),
        vertragd_url: None,
        vertragd_api_key: None,
        own_mp_id: "9900357000004".to_owned(),
        tenant: "t".to_owned(),
        auto_respond: true,
        bilanzkreise: vec![
            // A deployment supplying Strom as well: its Strom rows name the
            // same Bilanzierungsgebiet and a fallback, and a Gas answer must
            // read neither.
            BilanzkreisEintrag {
                bilanzierungsgebiet: Some(BG.to_owned()),
                standard: vec![BK_STROM.to_owned()],
                ..BilanzkreisEintrag::leer(Sparte::Strom)
            },
            BilanzkreisEintrag {
                standard: vec![BK_STROM.to_owned()],
                ..BilanzkreisEintrag::leer(Sparte::Strom)
            },
            BilanzkreisEintrag {
                bilanzierungsgebiet: Some(BG.to_owned()),
                standard: vec![BK_BG.to_owned()],
                ..BilanzkreisEintrag::leer(Sparte::Gas)
            },
            BilanzkreisEintrag {
                standard: vec![BK_FALLBACK.to_owned()],
                ..BilanzkreisEintrag::leer(Sparte::Gas)
            },
        ],
        grundversorgungs_netzgebiete: vec!["9900000000001".to_owned()],
        eog_auto_respond: true,
        // Configured for the Strom 55014; the Gas answer must not carry it.
        eog_versorgungsart: Some("ZD0".to_owned()),
        eog_abrechnungsintervall_monate: intervall,
    }
}

fn anmeldung() -> LfAnfragePayload {
    LfAnfragePayload::parse(&serde_json::json!({
        "makopid": 44_013,
        "subject": uuid::Uuid::nil().to_string(),
        "time": "2026-08-20T09:00:00Z",
        "data": { "malo_id": MALO },
    }))
    .expect("a 44013 is routed in a role-lf-gas build")
}

/// The E_3008 Zustimmung `E15` as the walk returns it.
fn zustimmung() -> mako_pruefung::LfAntwort {
    let entry = mako_pruefung::codes::lookup(mako_pruefung::codes::EBD_ANMELDUNG_EOG_GAS, "E15")
        .expect("E_3008 publishes E15");
    assert_eq!(entry.ist_zustimmung(), Some(true));
    mako_pruefung::LfAntwort {
        code: "E15".to_owned(),
        ebd: None,
        zustimmung: true,
        bedeutung: entry.bedeutung.to_owned(),
        bemerkung: None,
        bilanzkreis: None,
        termin: None,
        pruefschritt: 90,
    }
}

#[test]
fn the_automatic_44014_is_the_body_makods_door_reads() {
    let p = anmeldung();
    let cfg = config(Some(12));
    let fakten = EogFakten::fuer(&cfg, &ANMELDUNG_EOG_GAS, Some(BG));
    assert_eq!(fakten.fehlt(), None, "every fact resolves: {fakten:?}");

    let (command, body) = answer_payload(p.process, &p.anfrage, &zustimmung(), Some(&fakten));
    assert_eq!(command, GELI_EOG_BESTAETIGEN);
    let body: GeliEogBestaetigen =
        mako_service::json_from_value(body).expect("geli.eog.bestaetigen body");
    assert_eq!(body.malo_id, MALO);
    assert_eq!(body.antwort_code, "E15");
    assert_eq!(
        body.bilanzkreis.as_deref(),
        Some(BK_BG),
        "the standard entry of the MaLo's Bilanzierungsgebiet, not the fallback"
    );
    assert_eq!(body.abrechnungsintervall_monate, Some(12));
}

/// A Marktlokation in no listed Bilanzierungsgebiet takes the fallback row.
#[test]
fn an_unlisted_bilanzierungsgebiet_takes_the_fallback_row() {
    let fakten = EogFakten::fuer(
        &config(Some(1)),
        &ANMELDUNG_EOG_GAS,
        Some("11YN-BG-OTHER--3"),
    );
    assert_eq!(fakten.bilanzkreis.as_deref(), Some(BK_FALLBACK));
}

/// Without a Bilanzkreis or an Abrechnungsintervall the Zustimmung cannot go
/// out, and the module queues it naming what is missing.
#[test]
fn a_missing_fact_is_named_rather_than_sent() {
    // Only Strom rows: nothing a Gas answer may read.
    let mut cfg = config(Some(12));
    cfg.bilanzkreise.retain(|r| r.sparte == Sparte::Strom);
    let fehlt = EogFakten::fuer(&cfg, &ANMELDUNG_EOG_GAS, Some(BG))
        .fehlt()
        .expect("no Bilanzkreis resolves");
    assert!(fehlt.contains("CCI+Z19"), "{fehlt}");

    let fehlt = EogFakten::fuer(&config(None), &ANMELDUNG_EOG_GAS, Some(BG))
        .fehlt()
        .expect("no Abrechnungsintervall is configured");
    assert!(fehlt.contains("eog_abrechnungsintervall_monate"), "{fehlt}");
}

/// The Ablehnung 44015 carries none of the Zustimmung's facts.
#[test]
fn the_ablehnung_carries_no_eog_facts() {
    let p = anmeldung();
    let cfg = config(Some(12));
    let fakten = EogFakten::fuer(&cfg, &ANMELDUNG_EOG_GAS, Some(BG));
    let ablehnung = mako_pruefung::LfAntwort {
        code: "A02".to_owned(),
        zustimmung: false,
        ..zustimmung()
    };
    let (_, body) = answer_payload(p.process, &p.anfrage, &ablehnung, Some(&fakten));
    for key in [
        "bilanzkreis",
        "abrechnungsintervall_monate",
        "versorgungsart",
    ] {
        assert!(body.get(key).is_none(), "{key} on a 44015: {body}");
    }
}

/// A Gas row is never read by a Strom answer: with Gas rows only, a 55014
/// resolves no Bilanzkreis.
#[test]
fn a_strom_answer_never_reads_a_gas_row() {
    let mut cfg = config(Some(12));
    cfg.bilanzkreise.retain(|r| r.sparte == Sparte::Gas);
    assert_eq!(
        processd::lf_module::bilanzkreis_fuer(
            &cfg.bilanzkreise,
            Sparte::Strom,
            mako_pruefung::Bilanzkreisart::Standard,
            Some(BG),
        ),
        None
    );
}
