//! Integration tests for [`mako_mabis::zp_register::ZpRegister`].
//!
//! The register answers „which MaBiS-Zählpunkte are activated right now", which
//! `zp_lifecycle` cannot: an Aktivierung and the Deaktivierung that ends it are
//! two processes on two streams. The question has a date attached — BK6-23-241
//! Tenorziffer 5 repeals MaBiS Kap. 17.2 with the end of 30.09.2026 — so the
//! fold is what tells an operator a tägliche AAÜZ is still switched on for a
//! series that no longer exists.
//!
//! Every test drives the real workflow and folds the events it actually
//! emitted. Hand-built envelopes would test this file's idea of the event
//! shapes rather than the shapes.
//!
//! # The tägliche AAÜZ answers nothing
//!
//! Its family (55197/55198) carries `antwort: None`, so `ReceiveAnfrage` lands
//! straight in `Erfasst` and there is no answering party to confirm anything.
//! For such a family the recorded Anfrage **is** the outcome — which is why the
//! register counts `Erfasst` as a confirmation. Leaving it out would report
//! zero active MaBiS-ZP for exactly the series the repeal date is about.
//!
//! The confirm/refuse paths are therefore exercised on `NetzzeitreiheBiko`,
//! which does answer (55062/55063 → 55064).

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    projection::ProjectionRunner,
    types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    zp_lifecycle::{MabisZpLifecycleWorkflow, ZpLifecycleCommand, ZpSerie, ZpVorgang, familie_for},
    zp_register::ZpRegister,
};

const ZP: &str = "DE0001112223334445556667778889990";

fn mp(s: &str) -> MarktpartnerCode {
    MarktpartnerCode::new(s)
}

fn process(store: InMemoryEventStore) -> Process<MabisZpLifecycleWorkflow, InMemoryEventStore> {
    Process::new(
        store,
        TenantId::new(),
        WorkflowId::new("mabis-zp-lifecycle", "FV2026-04-01"),
    )
}

fn receive(serie: ZpSerie, vorgang: ZpVorgang) -> ZpLifecycleCommand {
    let pid = familie_for(serie, vorgang).expect("in the table").anfrage;
    ZpLifecycleCommand::ReceiveAnfrage {
        pid: Pruefidentifikator::new(pid).expect("valid PID"),
        serie,
        vorgang,
        mabis_zp_id: ZP.to_owned(),
        sender: mp("9900123456789"),
        receiver: mp("9900987654321"),
        billing_period: BillingPeriod::new("2026-07"),
        document_date: "20260701".to_owned(),
        message_ref: MessageRef::new("MSG-1"),
        validation_passed: true,
        validation_errors: vec![],
    }
}

fn antwort(bestaetigt: bool) -> ZpLifecycleCommand {
    ZpLifecycleCommand::SendAntwort {
        bestaetigt,
        grund: (!bestaetigt).then(|| "Bilanzierungsgebiet nicht gültig".to_owned()),
    }
}

fn d(y: i32, m: time::Month, day: u8) -> time::Date {
    time::Date::from_calendar_date(y, m, day).expect("valid date")
}

/// Fold everything the store holds.
async fn fold(store: &InMemoryEventStore) -> ZpRegister {
    let events = store.all_events().await;
    let mut register = ZpRegister::default();
    ProjectionRunner::run(&mut register, &events);
    assert_eq!(
        register.unlesbar(),
        0,
        "every MaBiS-ZP event the workflow emitted must decode"
    );
    register
}

/// A recorded Aktivierung activates, and the repeal date then names it.
#[tokio::test]
async fn a_recorded_aktivierung_on_a_repealed_serie_is_reported() {
    let store = InMemoryEventStore::new();
    let p = process(store.clone());
    p.execute(receive(ZpSerie::TaeglicheAauez, ZpVorgang::Aktivierung))
        .await
        .expect("Anfrage accepted");

    let register = fold(&store).await;
    assert_eq!(register.aktive_anzahl(), 1);

    // While the series still exists there is nothing to report.
    assert!(
        register
            .auf_beendeter_serie(d(2026, time::Month::September, 30))
            .is_empty(),
        "30.09.2026 is the last day it exists"
    );

    let past = register.auf_beendeter_serie(d(2026, time::Month::October, 1));
    assert_eq!(past.len(), 1, "01.10.2026 is the first day it does not");
    assert_eq!(past[0].mabis_zp_id, ZP);
    assert_eq!(past[0].serie, ZpSerie::TaeglicheAauez);
    assert!(past[0].aktiv);
}

/// A refused Anfrage activates nothing — the answering party said no.
#[tokio::test]
async fn a_refused_anfrage_does_not_activate() {
    let store = InMemoryEventStore::new();
    let p = process(store.clone());
    p.execute(receive(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung))
        .await
        .expect("Anfrage accepted");
    p.execute(antwort(false)).await.expect("Ablehnung sent");

    let register = fold(&store).await;
    assert_eq!(register.aktive_anzahl(), 0);
}

/// An Anfrage still waiting for its Antwort has activated nothing yet.
///
/// Only for a family that *has* an Antwort — see the module doc.
#[tokio::test]
async fn an_unanswered_anfrage_does_not_activate() {
    let store = InMemoryEventStore::new();
    let p = process(store.clone());
    p.execute(receive(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung))
        .await
        .expect("Anfrage accepted");

    let register = fold(&store).await;
    assert_eq!(
        register.aktive_anzahl(),
        0,
        "an activation is the answering party's act, not the asker's"
    );
}

/// A family with no Antwort activates on the record alone.
#[tokio::test]
async fn a_family_that_answers_nothing_activates_on_the_record() {
    let store = InMemoryEventStore::new();
    let p = process(store.clone());
    p.execute(receive(ZpSerie::TaeglicheAauez, ZpVorgang::Aktivierung))
        .await
        .expect("Anfrage accepted");

    let register = fold(&store).await;
    assert_eq!(
        register.aktive_anzahl(),
        1,
        "55197 carries `antwort: None` — the recorded Anfrage is the outcome"
    );
}

/// A confirmed Deaktivierung on a second process ends the activation.
///
/// This is the case `zp_lifecycle` alone cannot answer: two streams, one
/// Zählpunkt.
#[tokio::test]
async fn a_confirmed_deaktivierung_on_another_stream_ends_it() {
    let store = InMemoryEventStore::new();

    let p1 = process(store.clone());
    p1.execute(receive(ZpSerie::TaeglicheAauez, ZpVorgang::Aktivierung))
        .await
        .expect("Aktivierung accepted");
    assert_eq!(fold(&store).await.aktive_anzahl(), 1);

    let p2 = process(store.clone());
    p2.execute(receive(ZpSerie::TaeglicheAauez, ZpVorgang::Deaktivierung))
        .await
        .expect("Deaktivierung accepted");

    let register = fold(&store).await;
    assert_eq!(register.aktive_anzahl(), 0);
    assert!(
        register
            .auf_beendeter_serie(d(2026, time::Month::October, 1))
            .is_empty(),
        "a deactivated MaBiS-ZP is not an operator's problem"
    );
}

/// A series no Festlegung repealed is never reported, whatever the date.
#[tokio::test]
async fn a_live_serie_is_never_reported() {
    let store = InMemoryEventStore::new();
    let p = process(store.clone());
    p.execute(receive(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung))
        .await
        .expect("Anfrage accepted");
    p.execute(antwort(true)).await.expect("Antwort sent");

    let register = fold(&store).await;
    assert_eq!(register.aktive_anzahl(), 1);
    assert!(
        register
            .auf_beendeter_serie(d(2099, time::Month::January, 1))
            .is_empty(),
        "only a repealed series is reported"
    );
}
