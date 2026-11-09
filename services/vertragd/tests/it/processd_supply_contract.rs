//! The supply bodies `vertragd` posts to `processd`, read through `processd`'s
//! own request types as its handler reads them ([`mako_service::json_from_value`]
//! refuses an undeclared key exactly as [`mako_service::Json`] does), and its
//! Bilanzkreis through `processd`'s own rule.
//!
//! The two services agree on these bodies or not at all: a key processd does
//! not declare is a 422 on every activation, and a missing required field is the
//! same. Asserting that keys are *present* would pass both.

use mako_markt::utilmd_stammdaten::{NetznutzungZahler, Netznutzungsvertrag};
use processd::{
    config::BilanzkreisEintrag,
    lf_module::anmeldung_bilanzkreis,
    server::{EndSupplyGasRequest, EndSupplyRequest, StartSupplyGasRequest, StartSupplyRequest},
};
use time::macros::date;
use uuid::Uuid;
use vertragd::domain::Lieferbeginnanlass;
use vertragd::outbound::{GasAnmeldung, lieferbeginn, lieferende};

const MALO: &str = "51238696012";
const BK: &str = "11XBK-STANDARD-4";

/// A gas supply contract's Anmeldung facts, its Kunde as vertragd stores him.
fn gas_anmeldung() -> GasAnmeldung {
    let kunde = vertragd::domain::utilmd_kunde(
        Some(&serde_json::json!({
            "_typ": "GESCHAEFTSPARTNER",
            "vorname": "Erika",
            "nachname": "Mustermann",
            "adresse": {
                "strasse": "Hauptstraße", "hausnummer": "1",
                "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
            }
        })),
        None,
        None,
    );
    GasAnmeldung::aus_vertrag(
        Some(Lieferbeginnanlass::Umzug),
        "VIERTELJAEHRLICH",
        true,
        Some(Netznutzungsvertrag::LieferantNb),
        Some(NetznutzungZahler::Lieferant),
        Some(rust_decimal::Decimal::new(150, 2)),
        kunde,
    )
    .expect("a full gas contract")
}

/// The task's endpoint and the body the worker posts.
fn call(payload: &serde_json::Value) -> (&str, serde_json::Value) {
    (
        payload["endpoint"].as_str().expect("endpoint"),
        payload["body"].clone(),
    )
}

#[test]
fn the_fixture_bilanzkreis_is_one() {
    vertragd::domain::pruefe_bilanzkreis("STROM", BK)
        .expect("an X-type EIC with its check character");
}

#[test]
fn every_lieferbeginn_body_is_the_request_processd_reads() {
    let komp = Uuid::nil();
    let strom = lieferbeginn(komp, "STROM", MALO, Some(BK), date!(2027 - 01 - 01), None).unwrap();
    let (endpoint, body) = call(&strom.payload);
    assert_eq!(endpoint, "start-supply");
    let req: StartSupplyRequest = mako_service::json_from_value(body).expect("start-supply body");
    assert_eq!(req.malo_id, MALO);
    assert_eq!(req.lieferbeginn_datum, "2027-01-01");
    assert_eq!(req.bilanzkreis.as_deref(), Some(BK));

    let gas = lieferbeginn(
        komp,
        "GAS",
        MALO,
        None,
        date!(2027 - 01 - 01),
        Some(&gas_anmeldung()),
    )
    .unwrap();
    let (endpoint, body) = call(&gas.payload);
    assert_eq!(endpoint, "start-supply-gas");
    let req: StartSupplyGasRequest =
        mako_service::json_from_value(body).expect("start-supply-gas body");
    assert_eq!(req.malo_id, MALO);
    assert_eq!(req.process_date, "20270101");
    assert_eq!(req.bilanzkreis, None);
    // Every contract fact of the Anmeldung NN 44001, as the contract states it.
    assert_eq!(req.transaktionsgrund.as_deref(), Some("E01"));
    assert_eq!(req.abrechnungsintervall_monate, Some(3));
    assert_eq!(req.haushaltskunde, Some(true));
    assert_eq!(
        req.netznutzungsvertrag,
        Some(Netznutzungsvertrag::LieferantNb)
    );
    assert_eq!(req.netznutzung_zahler, Some(NetznutzungZahler::Lieferant));
    assert_eq!(
        req.gemeinderabatt_prozent,
        Some(rust_decimal::Decimal::new(150, 2))
    );
    let kunde = req.kunde.expect("the Kunde");
    assert_eq!(kunde.nachname.as_deref(), Some("Mustermann"));
    assert!(
        kunde.anmeldung_maengel().is_empty(),
        "{:?}",
        kunde.anmeldung_maengel()
    );
    assert_eq!(req.ablesekarte.as_ref(), Some(&kunde));
    // The admitted Transaktionsgründe are processd's to read from the profile,
    // and every Anlass vertragd stores is one of them.
    let admitted = processd::server::transaktionsgruende_44001();
    for anlass in Lieferbeginnanlass::ALL {
        assert!(
            admitted.iter().any(|c| c == anlass.transaktionsgrund()),
            "{anlass:?} → {} is not a 44001 code: {admitted:?}",
            anlass.transaktionsgrund()
        );
    }
}

#[test]
fn every_lieferende_body_is_the_request_processd_reads() {
    let komp = Uuid::nil();
    let strom = lieferende(komp, "STROM", MALO, date!(2027 - 06 - 30));
    let (endpoint, body) = call(&strom.payload);
    assert_eq!(endpoint, "end-supply");
    let req: EndSupplyRequest = mako_service::json_from_value(body).expect("end-supply body");
    assert_eq!(req.lieferende_datum, "2027-06-30");

    let gas = lieferende(komp, "GAS", MALO, date!(2027 - 06 - 30));
    let (endpoint, body) = call(&gas.payload);
    assert_eq!(endpoint, "end-supply-gas");
    let req: EndSupplyGasRequest =
        mako_service::json_from_value(body).expect("end-supply-gas body");
    assert_eq!(req.malo_id, MALO);
    assert_eq!(req.process_date, "20270630");
}

/// The check is the handler's: a key it does not declare refuses the body, so
/// the tests above would fail on the MP-IDs or the routing key in it.
#[test]
fn a_body_with_a_key_processd_does_not_declare_is_refused() {
    let mut body = call(
        &lieferbeginn(
            Uuid::nil(),
            "STROM",
            MALO,
            None,
            date!(2027 - 01 - 01),
            None,
        )
        .unwrap()
        .payload,
    )
    .1;
    body["nb_mp_id"] = "9900000000001".into();
    let err = mako_service::json_from_value::<StartSupplyRequest>(body).unwrap_err();
    assert!(err.to_string().contains("`nb_mp_id`"), "{err}");
}

const STROM: mako_markt::domain::Sparte = mako_markt::domain::Sparte::Strom;

fn standard(bks: &[&str]) -> Vec<BilanzkreisEintrag> {
    vec![BilanzkreisEintrag {
        standard: bks.iter().map(|b| (*b).to_owned()).collect(),
        ..BilanzkreisEintrag::leer(mako_markt::domain::Sparte::Strom)
    }]
}

/// What processd answers an activation with, by what the contract names and
/// what the deployment declares: the contract's Bilanzkreis wins, a single
/// `standard` entry fills in, and anything else is `MISSING_BILANZKREIS`.
#[test]
fn an_activation_reaches_the_nb_with_a_bilanzkreis_or_is_refused() {
    let body = |bk| {
        let task = lieferbeginn(Uuid::nil(), "STROM", MALO, bk, date!(2027 - 01 - 01), None);
        mako_service::json_from_value::<StartSupplyRequest>(call(&task.unwrap().payload).1)
            .unwrap()
            .bilanzkreis
    };
    let second = rubo4e::identifiers::EicCode::new_from_prefix("11XBK-STANDRD2-")
        .expect("a derived check character")
        .to_string();
    let two = [BK, second.as_str()];

    // The contract names one: it reaches the NB whatever the deployment lists.
    for cfg in [standard(&[]), standard(&two), Vec::new()] {
        assert_eq!(
            anmeldung_bilanzkreis(body(Some(BK)).as_deref(), &cfg, STROM).as_deref(),
            Some(BK)
        );
    }
    // It names none: the single `standard` entry decides …
    assert_eq!(
        anmeldung_bilanzkreis(body(None).as_deref(), &standard(&[BK]), STROM).as_deref(),
        Some(BK)
    );
    // … and without exactly one, processd refuses (`MISSING_BILANZKREIS`).
    for cfg in [standard(&[]), standard(&two), Vec::new()] {
        assert_eq!(
            anmeldung_bilanzkreis(body(None).as_deref(), &cfg, STROM),
            None
        );
    }
}
