//! The NB answers a Geschäftsdatenanfrage Stammdaten Strom with the
//! Lokationsbündel marktd holds.
//!
//! A LF's ORDERS 17132 asks the NB for a Marktlokation's Stammdaten (GPKE Teil
//! 4 Kap. 3); the NB answers UTILMD 55035 for a verbrauchende Marktlokation,
//! whose column (UTILMD AHB Strom 2.2 Kap. 9.5, S. 599–665) is the
//! Lokationsbündel: `SG5 LOC+Z16`/`Z17`, `SG8 SEQ+ZD5` the
//! Lokationsbündelstruktur, `SEQ+ZD6` each object's Objektcode, then the
//! Informative Daten of the Marktlokation, its Messlokation and Zähler, and
//! `SG12 NAD+Z63`. `gpke.geschaeftsdatenanfrage.beantworten` names only the
//! Anfrage and the answer; makod reads the rest from a stand-in marktd, and
//! the rendered UTILMD is parsed back and held against its AHB profile as the
//! pre-send gate holds it.

use std::sync::Arc;

use mako_engine::event_store::CorrelationEntry;
use mako_engine::outbox::OutboxStore;
use mako_engine::process::{Process, RunOptions};
use mako_engine::types::{MarktpartnerCode, MessageRef, Pruefidentifikator};
use mako_engine::version::WorkflowId;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use mako_gpke::GpkeGeschaeftsdatenanfrageWorkflow;
use mako_gpke::geschaeftsdatenanfrage::netzbetreiber_key;
use mako_gpke::{GDA_WORKFLOW_NAME, GdaAnfrage, GdaCommand, GdaState};
use mako_markt::domain::Lokationstyp;
use mako_markt::lokationsbuendel::{
    BuendelAngaben, BuendelObjekt, BuendelZaehler, BuendelZuordnung, MaloLokationsbuendel,
    NnaPosition, ObisAngabe, Profilverfahren, RegisterAngabe, Wertegranularitaet,
};
use mako_markt::utilmd_stammdaten::{MaloUtilmdStammdaten, MaloZaehlwerk};
use makod::commands_api::{CommandsApiState, DispatchError, dispatch_command};

use crate::support;

const NB_MP: &str = "9900357000004";
const LF_MP: &str = "9900000000003";
const MSB_MP: &str = "9900357000004";
const MALO: &str = "51238696012";
const NUMMER: &str = "GDA-0815";
const FV: &str = "FV2026-10-01";
/// Verbrauch mit einer Messlokation (Standard) and its object codes
/// (Codeliste der Lokationsbündelstrukturen 1.0, Kap. 4.2).
const STRUKTUR: &str = "9992000000026";
const MALO_CODE: &str = "9992000001016";
const MELO_CODE: &str = "9992000001032";

/// The UTILMD Stammdaten marktd serves: the support fixture's SLP
/// Marktlokation, with its Zählwerk.
fn stammdaten() -> MaloUtilmdStammdaten {
    let mut sd = support::strom_stammdaten(MALO, MSB_MP);
    sd.zaehlwerke = vec![MaloZaehlwerk {
        obis_kennzahl: "1-1:1.8.0".to_owned(),
        konzessionsabgabe_satz: None,
        konzessionsabgabe_eur_kwh: None,
    }];
    sd
}

/// The Lokationsbündel marktd serves: the Marktlokation and its one
/// Messlokation in the Standard structure, and the NB's Angaben.
fn buendel() -> MaloLokationsbuendel {
    MaloLokationsbuendel {
        malo_id: MALO.to_owned(),
        stichtag: time::macros::date!(2026 - 12 - 01),
        energierichtung: Some(rubo4e::current::Energierichtung::Aussp),
        lokationsbuendelcodes: vec![STRUKTUR.to_owned()],
        objekte: vec![
            BuendelObjekt {
                typ: Lokationstyp::Malo,
                id: MALO.to_owned(),
                objektcode: Some(MALO_CODE.to_owned()),
            },
            BuendelObjekt {
                typ: Lokationstyp::Melo,
                id: support::STROM_MELO.to_owned(),
                objektcode: Some(MELO_CODE.to_owned()),
            },
        ],
        zuordnungen: vec![BuendelZuordnung {
            von_id: support::STROM_MELO.to_owned(),
            von_typ: Lokationstyp::Melo,
            nach_id: MALO.to_owned(),
            nach_typ: Lokationstyp::Malo,
            valid_from: Some(time::macros::date!(2026 - 01 - 01)),
            valid_to: None,
        }],
        angaben: Some(BuendelAngaben {
            valid_from: time::macros::date!(2026 - 01 - 01),
            valid_to: None,
            uenb_mp_id: Some("9911845000009".to_owned()),
            technische_einrichtungen: Some(false),
            standardlastprofil: Some("H25".to_owned()),
            profilverfahren: Some(Profilverfahren::Synthetisch),
            nna_positionen: vec![NnaPosition {
                artikel_id: "1-02-0-001".to_owned(),
                zu_abschlag_prozent: rust_decimal::Decimal::ZERO,
            }],
            obis: vec![ObisAngabe {
                obis_kennzahl: "1-1:1.8.0".to_owned(),
                verwendungszweck_nb: Some("V01".to_owned()),
                verwendungszweck_lf: Some("V02".to_owned()),
                verwendungszweck_uenb: Some("V15".to_owned()),
                wertegranularitaet: None,
            }],
            register: vec![RegisterAngabe {
                zaehler_id: "Z-STROM-1".to_owned(),
                obis_kennzahl: "1-1:1.8.0".to_owned(),
                wertegranularitaet: Wertegranularitaet::Jaehrlich,
            }],
        }),
        zaehler: vec![BuendelZaehler {
            melo_id: support::STROM_MELO.to_owned(),
            zaehler_id: "Z-STROM-1".to_owned(),
            zaehlertyp_spezifikation: Some(rubo4e::current::ZaehlertypSpezifikation::MmeStandard),
        }],
    }
}

/// A stand-in marktd serving `sd` and `lb` for whichever Marktlokation is
/// asked for; `None` answers 404.
async fn stub_marktd(
    sd: Option<MaloUtilmdStammdaten>,
    lb: Option<MaloLokationsbuendel>,
) -> mako_markt::marktd_client::MarktdClient {
    use axum::{extract::Path, response::IntoResponse, routing::get};
    let (sd, lb) = (Arc::new(sd), Arc::new(lb));
    let app = axum::Router::new()
        .route(
            "/api/v1/malos/{id}/utilmd-stammdaten",
            get(move |Path(id): Path<String>| {
                let sd = Arc::clone(&sd);
                async move {
                    match sd.as_ref() {
                        Some(r) => {
                            let mut r = r.clone();
                            r.malo_id = id;
                            axum::Json(r).into_response()
                        }
                        None => axum::http::StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        )
        .route(
            "/api/v1/malos/{id}/lokationsbuendel",
            get(move |Path(_id): Path<String>| {
                let lb = Arc::clone(&lb);
                async move {
                    match lb.as_ref() {
                        Some(r) => axum::Json(r.clone()).into_response(),
                        None => axum::http::StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stand-in marktd");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    mako_markt::marktd_client::MarktdClient::new(
        format!("http://{addr}"),
        secrecy::SecretString::from("test-key"),
        reqwest::Client::new(),
    )
}

struct Nb {
    store: Arc<SlateDbStore>,
    state: CommandsApiState,
}

/// A NB Strom whose 17132 from the LF is open and owed an answer.
async fn nb(marktd: Option<mako_markt::marktd_client::MarktdClient>) -> Nb {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant_id = TenantId::from_party_id(NB_MP);
    let state = CommandsApiState {
        tenant_id,
        sender_party_id: NB_MP.to_owned(),
        configured_marktrollen: vec!["NB".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new((*store).clone())),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store: Arc::clone(&store),
        marktd_client: marktd.map(Arc::new),
    };
    let process = Process::<GpkeGeschaeftsdatenanfrageWorkflow, _>::new(
        Arc::clone(&store),
        tenant_id,
        WorkflowId::new(GDA_WORKFLOW_NAME, FV),
    );
    let key = CorrelationEntry {
        tenant_id,
        tag: netzbetreiber_key(LF_MP, NUMMER),
        process_id: process.process_id(),
        identity: process.identity(),
    };
    process
        .run(
            GdaCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::const_new(17132),
                anfrage: GdaAnfrage {
                    anfragender: MarktpartnerCode::new(LF_MP),
                    netzbetreiber: MarktpartnerCode::new(NB_MP),
                    lokation_id: MALO.to_owned(),
                    anfrage_nummer: MessageRef::new(NUMMER),
                },
                message_ref: MessageRef::new("UNH17132"),
                validation_passed: true,
                validation_errors: vec![],
            },
            RunOptions::new().correlations(&[key]),
        )
        .await
        .expect("the 17132 opens the NB's process");
    Nb { store, state }
}

impl Nb {
    async fn beantworten(&self, antwort_pid: u32) -> Result<(), DispatchError> {
        dispatch_command(
            &self.state,
            "gpke.geschaeftsdatenanfrage.beantworten",
            &serde_json::json!({
                "anfragender_mp_id": LF_MP,
                "anfrage_nummer": NUMMER,
                "antwort_pid": antwort_pid,
                "stichtag": "2026-12-01",
            }),
        )
        .await
        .map(|_| ())
    }

    async fn pending_utilmd(&self) -> Vec<mako_engine::outbox::OutboxMessage> {
        let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        OutboxStore::pending(&*self.store, 100, now)
            .await
            .expect("pending outbox")
            .into_iter()
            .filter(|om| &*om.message_type == "UTILMD")
            .collect()
    }

    async fn status(&self) -> &'static str {
        let identity = mako_engine::registry::ProcessRegistry::lookup_correlated(
            &self.store.as_process_registry(),
            self.state.tenant_id,
            &netzbetreiber_key(LF_MP, NUMMER),
        )
        .await
        .expect("lookup")
        .into_iter()
        .find(|id| id.workflow_id.name.as_ref() == GDA_WORKFLOW_NAME)
        .expect("the process");
        let state: GdaState = Process::<GpkeGeschaeftsdatenanfrageWorkflow, _>::from_identity(
            Arc::clone(&self.store),
            identity,
        )
        .state()
        .await
        .expect("state");
        state.status_str()
    }
}

/// Render `om`, parse it back and hold it against its AHB profile, as the
/// pre-send gate does.
fn gated(om: &mako_engine::outbox::OutboxMessage) -> String {
    use edi_energy::EdiEnergyMessage as _;
    let registry =
        makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: NB_MP.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("valid registry");
    let wire = makod::edifact_renderer::render_to_wire_bytes(om, &registry)
        .unwrap_or_else(|e| panic!("render {}: {e:?}", om.message_type));
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let msg = edi_energy::parse(&wire.bytes).unwrap_or_else(|e| panic!("parse: {e:?}"));
    let report = msg
        .validate_on_date(mako_fristen::heute())
        .expect("the AHB validation runs");
    assert!(
        report.is_valid(),
        "the rendered 55035 violates its AHB profile: {:#?}\n{}",
        report
            .errors()
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        text.replace('\'', "'\n"),
    );
    assert_eq!(
        msg.detect_pruefidentifikator().map(|p| p.as_u32()).ok(),
        Some(55_035)
    );
    text
}

/// The 55035 carries marktd's Lokationsbündel: the structure, each object's
/// Objektcode, the ÜNB and MSB, the NNA-Position, the OBIS-Daten with their
/// Verwendungszwecke, the Zähleinrichtung and the Profil — and passes its
/// profile.
#[tokio::test]
async fn the_55035_is_built_from_the_buendel_marktd_holds() {
    let c = nb(Some(stub_marktd(Some(stammdaten()), Some(buendel())).await)).await;
    c.beantworten(55_035)
        .await
        .expect("the answer is dispatched");
    let utilmd = c.pending_utilmd().await;
    let [om] = utilmd.as_slice() else {
        panic!("one UTILMD is written, got {}", utilmd.len());
    };
    assert_eq!(om.recipient.as_ref(), LF_MP);
    let wire = gated(om);
    for expected in [
        "BGM+E44+",
        "STS+7++ZY7+ZW4'",
        &format!("LOC+Z16+{MALO}'"),
        &format!("LOC+Z17+{}'", support::STROM_MELO),
        &format!("RFF+AAV:{NUMMER}'"),
        &format!("SEQ+ZD5'RFF+Z31:{STRUKTUR}'"),
        &format!("SEQ+ZD6'RFF+Z18:{MALO}'RFF+Z33:{MALO_CODE}'"),
        &format!(
            "SEQ+ZD6'RFF+Z19:{}'RFF+Z33:{MELO_CODE}'",
            support::STROM_MELO
        ),
        "QTY+31:3500:KWH'",
        "CAV+Z90:9911845000009'",
        &format!("CAV+Z91:{MSB_MP}::Z39:Z19'"),
        "CAV+E06'",
        "CCI+++ZA6'CAV+E02'",
        "SEQ+ZE1'PIA+Z02+1-02-0-001:Z09'QTY+Z16:0:P1'QTY+Z37:0:P1'",
        "SEQ+ZE3'PIA+5+1-1?:1.8.0:SRW+:V01:Z16+:V02:Z16+:V15:Z16'",
        "SEQ+ZH9'CCI+Z63'CAV+ZH8'",
        &format!("CAV+ZF0:{MSB_MP}'"),
        "CAV+MME:::Z04'",
        "CAV+Z30:::1EMH0012345678'",
        "CAV+MMR'",
        "SEQ+ZF6'RFF+MG:1EMH0012345678'PIA+5+1-1?:1.8.0:SRW'CCI+++ZE4'CAV+ZD9'",
        "SEQ+ZG1'CCI+Z02++E01'CAV+H25'",
        "NAD+Z63++++Musterstr. 1+Berlin++10115+DE'",
    ] {
        assert!(
            wire.contains(expected),
            "{expected} in\n{}",
            wire.replace('\'', "'\n")
        );
    }
    assert_eq!(c.status().await, "Beantwortet");
}

/// marktd unreachable: the answer is refused `503`, nothing is written and
/// the Anfrage stays open — its 1-WT window keeps running.
#[tokio::test]
async fn an_unreadable_marktd_refuses_the_answer() {
    let c = nb(Some(support::unreachable_marktd())).await;
    let r = c.beantworten(55_035).await;
    assert!(
        matches!(r, Err(DispatchError::MarktdUnavailable(_))),
        "got {r:?}"
    );
    assert!(c.pending_utilmd().await.is_empty());
    assert_eq!(c.status().await, "Offen");
}

/// A bündel marktd holds incompletely is refused `422`, naming each missing
/// fact, and nothing is sent.
#[tokio::test]
async fn missing_facts_are_refused_by_name() {
    let mut lb = buendel();
    lb.lokationsbuendelcodes.clear();
    lb.objekte[1].objektcode = None;
    if let Some(a) = lb.angaben.as_mut() {
        a.uenb_mp_id = None;
        a.nna_positionen.clear();
        a.obis[0].verwendungszweck_uenb = None;
        a.technische_einrichtungen = None;
        a.register.clear();
    }
    let c = nb(Some(stub_marktd(Some(stammdaten()), Some(lb)).await)).await;
    let r = c.beantworten(55_035).await;
    let Err(DispatchError::StammdatenUnvollstaendig { malo_id, fehlend }) = r else {
        panic!("expected a refusal naming the facts, got {r:?}");
    };
    assert_eq!(malo_id, MALO);
    for fact in [
        "lokationsbuendelcode",
        "uenb_mp_id",
        "nna_positionen",
        "Verwendungszweck for the ÜNB",
        "technische_einrichtungen",
        "wertegranularitaet",
    ] {
        assert!(
            fehlend.iter().any(|f| f.contains(fact)),
            "{fact} is named in {fehlend:#?}"
        );
    }
    assert!(c.pending_utilmd().await.is_empty());
    assert_eq!(c.status().await, "Offen");

    // An erzeugende Marktlokation is not answered 55035.
    let mut lb = buendel();
    lb.energierichtung = Some(rubo4e::current::Energierichtung::Einsp);
    let c = nb(Some(stub_marktd(Some(stammdaten()), Some(lb)).await)).await;
    let r = c.beantworten(55_035).await;
    assert!(
        matches!(&r, Err(DispatchError::StammdatenUnvollstaendig { fehlend, .. })
            if fehlend.iter().any(|f| f.contains("55095"))),
        "got {r:?}"
    );
}

/// The Netzlokation of a bündel is named as not assembled rather than left
/// off the answer: its `SEQ+ZD7` is Muss once its `LOC+Z18` is sent.
#[tokio::test]
async fn an_object_not_assembled_is_refused_by_name() {
    let mut lb = buendel();
    lb.objekte.push(BuendelObjekt {
        typ: Lokationstyp::Nelo,
        id: "E1234567890".to_owned(),
        objektcode: Some("9992000001256".to_owned()),
    });
    let c = nb(Some(stub_marktd(Some(stammdaten()), Some(lb)).await)).await;
    let r = c.beantworten(55_035).await;
    assert!(
        matches!(&r, Err(DispatchError::StammdatenUnvollstaendig { fehlend, .. })
            if fehlend.iter().any(|f| f.contains("Netzlokationen") && f.contains("E1234567890"))),
        "got {r:?}"
    );
    assert!(c.pending_utilmd().await.is_empty());
}
