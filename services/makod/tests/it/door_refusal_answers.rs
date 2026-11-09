//! A message the door refuses is answered with its Verarbeitbarkeitsfehler-
//! meldung where APERAK AHB 1.1 names the code, in the same write as the
//! refusal's record.
//!
//! * A Prüfidentifikator this deployment's Marktrollen do not process:
//!   `Z31` „Geschäftsvorfall wird vom Empfänger zurückgewiesen" (Kap. 4.3,
//!   S. 42) — „Entsprechend seiner Marktrolle verarbeitet der Empfänger
//!   Geschäftsvorfälle mit dem angegebenen Prüfidentifikator nicht". Owed in
//!   both Sparten: a Verarbeitbarkeitsfehler is the one result Gas reports
//!   (§ 2.3, S. 21).
//! * An AHB-Fehler (§ 2.1.1, S. 10): a missing Muss element `Z29`, a code
//!   outside the permitted ones `Z39`, each with its Ortsangabe (§ 2.1.2.2,
//!   S. 12) — instead of the Anerkennungsmeldung.

use std::sync::Arc;

use http_body_util::BodyExt as _;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
use tower::ServiceExt as _;

const OWN: &str = "9900357000004";
const SENDER: &str = "4012345000023";

fn skeleton(mt: edi_energy::MessageType, pid: u32) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(mt)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: SENDER.to_owned(),
                receiver: OWN.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8(bytes).expect("UTF-8")
}

/// The APERAKs in the outbox, as typed payloads.
async fn aperaks(store: &SlateDbStore) -> Vec<mako_engine::payload::AperakPayload> {
    store
        .pending_now(50)
        .await
        .expect("outbox")
        .into_iter()
        .filter(|m| &*m.message_type == "APERAK")
        .map(|m| serde_json::from_value(m.payload).expect("an APERAK payload"))
        .collect()
}

/// `POST /edifact` with a router that takes no Prüfidentifikator at all.
async fn post_unrouted(edifact: String) -> (SlateDbStore, serde_json::Value) {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let tenant = TenantId::from_party_id(OWN);
    let state = Arc::new(makod::edifact_api::EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router: mako_engine::pid_router::PidRouter::new(),
        mp_id_registry: Arc::new(
            makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
                mp_id: OWN.to_owned(),
                roles: vec!["NB".to_owned()],
                primary: true,
                agency: None,
                as4: None,
            }])
            .expect("valid registry"),
        ),
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        max_body_bytes: 1024 * 1024,
        partner_store: None,
        tenant_id: tenant,
        dl_sink: Arc::new(store.as_dead_letter_sink()),
        dispatcher: None,
        contrl_ack: None,
    });
    let response = makod::edifact_api::router(state)
        .oneshot(
            axum::http::Request::post("/edifact")
                .header("content-type", "application/edifact")
                .body(axum::body::Body::from(edifact))
                .expect("request"),
        )
        .await
        .expect("infallible");
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("JSON");
    (store, body)
}

/// A PID no Marktrolle of this deployment processes is refused with `Z31`,
/// and the refusal is recorded beside it.
#[tokio::test]
async fn an_unprocessed_pid_is_answered_z31() {
    let (store, body) = post_unrouted(skeleton(edi_energy::MessageType::Utilmd, 55001)).await;
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{body}");
    assert_eq!(sent[0].pid, mako_engine::payload::APERAK_PID_FEHLER);
    assert_eq!(sent[0].error_code.as_deref(), Some("Z31"));
    assert_eq!(sent[0].sender, OWN);
    assert_eq!(sent[0].receiver.as_deref(), Some(SENDER));
    assert_eq!(
        store
            .list_dead_letters(10)
            .await
            .expect("dead letters")
            .len(),
        1,
        "the refusal is recorded"
    );
}

/// An APERAK is never answered with an APERAK (§ 2, S. 9), routed or not.
#[tokio::test]
async fn an_unrouted_aperak_gets_no_aperak() {
    let (store, _) = post_unrouted(skeleton(edi_energy::MessageType::Aperak, 29002)).await;
    assert!(aperaks(&store).await.is_empty());
}

/// An MSCONS whose Muss `DTM+137` Nachrichtendatum is missing is an AHB-Fehler:
/// `Z29` with the MIG name of the segment as Ortsangabe, not an Anerkennungsmeldung.
#[tokio::test]
async fn an_ahb_fehler_is_answered_z29() {
    let wire = skeleton(edi_energy::MessageType::Mscons, 13018)
        .replace("DTM+137:202609302200?+00:303'", "")
        .replace("LOC+172'", "LOC+172+51238696781'")
        .replace("UNT+17+1'", "UNT+16+1'");
    let msg = edi_energy::parse(wire.as_bytes()).expect("still parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    let outcome = d
        .dispatch(
            &msg,
            "gpke-messwerte",
            13018,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].error_code.as_deref(), Some("Z29"), "{sent:?}");
    assert_eq!(
        sent[0].ortsangabe.as_deref(),
        Some("Nachrichtendatum"),
        "{sent:?}"
    );
}

/// In Gas a Verarbeitbarkeitsfehler is reported too (§ 2.3, S. 21), and its
/// window watches the Fehlermeldung's delivery.
#[tokio::test]
async fn a_gas_ahb_fehler_is_answered_and_watched() {
    use mako_engine::deadline::DeadlineStore as _;
    let wire = skeleton(edi_energy::MessageType::Mscons, 13008);
    let unt = wire
        .split('\'')
        .find(|s| s.starts_with("UNT"))
        .expect("UNT")
        .to_owned();
    let without = wire.replacen("DTM+137:202609302200?+00:303'", "", 1);
    // `UNT` counts `UNH` through `UNT`.
    let count = without
        .split('\'')
        .filter(|s| !s.is_empty() && !s.starts_with("UNB") && !s.starts_with("UNZ"))
        .count();
    let without = without
        .replace(&format!("{unt}'"), &format!("UNT+{count}+1'"))
        .replace("LOC+172'", "LOC+172+51238696781'");
    let msg = edi_energy::parse(without.as_bytes()).expect("still parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    let received = time::macros::datetime!(2026-07-07 10:00 UTC);
    d.dispatch(&msg, "geli-gas-mscons", 13008, received)
        .await
        .expect("dispatch");
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].error_code.as_deref(), Some("Z29"));
    let windows: Vec<String> = store
        .as_deadline_store()
        .due_now(100)
        .await
        .expect("deadlines")
        .deadlines
        .iter()
        .filter(|d| mako_fristen::is_delivery_window(d.label()))
        .map(|d| d.label().to_owned())
        .collect();
    assert_eq!(
        windows,
        vec![mako_fristen::APERAK_GAS_FOLGEPROZESS_LABEL.to_owned()]
    );
}

/// Every AHB-Fehler of the Geschäftsvorfall is named (§ 2.1.1, S. 10), each in
/// its own `SG4 ERC` group with its own Ortsangabe — the MIG lets `SG4` repeat
/// 99999 times.
#[tokio::test]
async fn every_ahb_fehler_gets_its_own_erc_group() {
    let wire = skeleton(edi_energy::MessageType::Mscons, 13018);
    let unt = wire
        .split('\'')
        .find(|s| s.starts_with("UNT"))
        .expect("UNT")
        .to_owned();
    let without = wire
        .replacen("DTM+137:202609302200?+00:303'", "", 1)
        .replacen("QTY+220:100'", "", 1);
    let count = without
        .split('\'')
        .filter(|s| !s.is_empty() && !s.starts_with("UNB") && !s.starts_with("UNZ"))
        .count();
    let without = without
        .replace(&format!("{unt}'"), &format!("UNT+{count}+1'"))
        .replace("LOC+172'", "LOC+172+51238696781'");
    let msg = edi_energy::parse(without.as_bytes()).expect("still parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    d.dispatch(
        &msg,
        "gpke-messwerte",
        13018,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("dispatch");
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "one APERAK names them all: {sent:?}");
    let named = 1 + sent[0].weitere_fehler.len();
    assert!(named >= 2, "both missing segments are named: {sent:?}");
    let orte: Vec<_> = std::iter::once(sent[0].ortsangabe.clone())
        .chain(sent[0].weitere_fehler.iter().map(|f| f.ortsangabe.clone()))
        .collect();
    assert!(
        orte.contains(&Some("Nachrichtendatum".to_owned())),
        "{orte:?}"
    );
    assert!(
        orte.iter()
            .any(|o| o.as_deref() == Some("Mengen- und Statusangaben")),
        "{orte:?}"
    );
}

/// A follow-up whose preceding Geschäftsvorfall is not here is answered `Z33`
/// „Referenziertes Geschäftsvorfall-Tupel nicht vorhanden" (Kap. 4.3, S. 43)
/// — the PID-Übersicht marks 55002's „Stelle der Zuordnungsprüfung" `V`, so
/// the check belongs to the Verarbeitbarkeitsprüfung (§ 2.1.3, S. 14).
#[tokio::test]
async fn an_orphan_follow_up_is_answered_z33() {
    let wire = skeleton(edi_energy::MessageType::Utilmd, 55002);
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    let outcome = d
        .dispatch(
            &msg,
            "gpke-lf-anmeldung",
            55002,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(
            outcome,
            IngestOutcome::Skipped {
                reason: "process_not_found",
                ..
            }
        ),
        "{outcome:?}"
    );
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].error_code.as_deref(), Some("Z33"));
    assert_eq!(sent[0].receiver.as_deref(), Some(SENDER));
}

/// The column, read from the tracked `pid-overview.json`, agrees with the
/// Anwendungsübersicht for the rows this suite relies on.
#[test]
fn the_zuordnungspruefung_column_is_carried() {
    assert!(makod::ingest_dispatcher::z33_owed(55002));
    // 55001 opens a Vorgang: no Geschäftsvorfall precedes it.
    assert!(!makod::ingest_dispatcher::z33_owed(55001));
    // Not published at all.
    assert!(!makod::ingest_dispatcher::z33_owed(99999));
}

/// Held for a person (`[policy.ingest] unbekannter_vorgang = "zur_pruefung"`,
/// a cutover from another MaKo system): the orphan gets no `Z33` and is
/// recorded as a dead letter instead.
#[tokio::test]
async fn a_held_orphan_follow_up_is_dead_lettered_not_refused() {
    use makod::config::UnbekannterVorgang;
    let wire = skeleton(edi_energy::MessageType::Utilmd, 55002);
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    )
    .with_unbekannter_vorgang(UnbekannterVorgang::ZurPruefung);
    d.dispatch(
        &msg,
        "gpke-lf-anmeldung",
        55002,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("dispatch");
    assert!(aperaks(&store).await.is_empty(), "no Z33 while held");
    assert_eq!(
        store
            .list_dead_letters(10)
            .await
            .expect("dead letters")
            .len(),
        1,
        "the held message is recorded for a person"
    );
}
