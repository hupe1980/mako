//! The CONTRL Syntaxfehlermeldung — the half of the CONTRL both Sparten owe.
//!
//! CONTRL AHB 1.0 §2.3 uses the message two ways in Gas (Empfangsbestätigung and
//! Syntaxfehlermeldung) and §2.4 one way in Strom: „In der Sparte Strom wird die
//! CONTRL **ausschließlich** als Syntaxfehlermeldung eingesetzt." Sending it says
//! two things at once (§2.3.2 / §2.4.2): the Übertragungsdatei arrived, and it
//! „wird nicht weiterbearbeitet".
//!
//! These tests drive the service that produces it and the renderer that puts it
//! on the wire, so the `UCI` DE 0083 / DE 0085 pair is asserted as bytes rather
//! than as a struct field.

use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::store_slatedb::SlateDbStore;
use makod::contrl_ack::{ContrlAckService, SyntaxFehler};
use makod::party_registry::MpIdRegistry;

/// A Strom own party, so §2.4 applies.
const OWN_STROM: &str = "9900357000004";
const COUNTERPARTY: &str = "9900123456789";

fn registry(mp_id: &str, role: &str) -> Arc<MpIdRegistry> {
    Arc::new(
        MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: vec![role.to_owned()],
            primary: true,
            agency: None,
        }])
        .expect("valid registry"),
    )
}

/// A UTILMD Strom interchange whose body is unreadable but whose envelope is not.
fn broken_utilmd() -> Vec<u8> {
    format!(
        "UNB+UNOC:3+{COUNTERPARTY}:500+{OWN_STROM}:500+260912:0900+IC4711'\
UNH+1+UTILMD:D:11B:UN:S2.1'BGM+E01'UNT+3+1'UNZ+1+IC4711'"
    )
    .into_bytes()
}

async fn service(own: &str, role: &str) -> (ContrlAckService, SlateDbStore, TenantId) {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(own);
    let svc = ContrlAckService::new(Arc::new(store.clone()), tenant, registry(own, role));
    (svc, store, tenant)
}

/// The Syntaxfehlermeldung is owed in **Strom**, where no Empfangsbestätigung is.
///
/// This is the case that reached the counterparty as silence: a Strom sender
/// whose file did not parse got no CONTRL at all, and §2.4 gives the CONTRL no
/// other use in that Sparte.
#[tokio::test]
async fn a_strom_interchange_that_did_not_parse_is_answered() {
    let (svc, store, _tenant) = service(OWN_STROM, "NB").await;

    svc.emit_syntax_error(
        &broken_utilmd(),
        "IC4711",
        OWN_STROM,
        COUNTERPARTY,
        SyntaxFehler::UngueltigerWert,
    )
    .await
    .expect("the Syntaxfehlermeldung must be enqueued");

    let queued = store.pending_now(10).await.expect("read the outbox");
    let contrl = queued
        .iter()
        .find(|m| m.message_type.as_ref() == "CONTRL")
        .expect("a CONTRL is queued");

    assert_eq!(
        contrl.payload["accepted"],
        serde_json::json!(false),
        "a Syntaxfehlermeldung is UCI DE 0083 = 4, never 7"
    );
    assert_eq!(
        contrl.payload["syntax_error"],
        serde_json::json!("12"),
        "DE 0085 „Ungültiger Wert\" is the catch-all the AHB admits on the UCI"
    );
    assert_eq!(
        contrl.recipient.as_ref(),
        COUNTERPARTY,
        "the CONTRL goes back to the sender of the rejected file"
    );
}

/// §2.4.1 gives a Strom UTILMD or ORDERS its own window — 15 minutes, and six
/// hours when it arrived on a Saturday — and the message type is read off the
/// `UNH` of the file that did not parse.
///
/// The deadline is compared against `contrl_due_at` rather than against a
/// number, so what is pinned is that the **Anlass** was selected: a test
/// asserting 15 minutes passes Monday to Friday and fails every Saturday.
#[tokio::test]
async fn a_strom_utilmd_earns_its_own_window() {
    let (svc, store, _tenant) = service(OWN_STROM, "NB").await;
    let before = time::OffsetDateTime::now_utc();

    svc.emit_syntax_error(
        &broken_utilmd(),
        "IC4711",
        OWN_STROM,
        COUNTERPARTY,
        SyntaxFehler::UngueltigerWert,
    )
    .await
    .expect("enqueued");

    let queued = store.pending_now(10).await.expect("read the outbox");
    let contrl = queued
        .iter()
        .find(|m| m.message_type.as_ref() == "CONTRL")
        .expect("a CONTRL is queued");
    let deadlines = store
        .as_deadline_store()
        .for_stream(&contrl.stream_id)
        .await
        .expect("read deadlines");
    let window = deadlines
        .iter()
        .find(|d| d.label() == mako_fristen::CONTRL_FRIST_LABEL)
        .expect("the CONTRL delivery window is registered");

    let expected =
        mako_fristen::contrl_due_at(before, mako_fristen::ContrlAnlass::StromUtilmdOderOrders);
    let drift = (window.due_at() - expected).abs();
    assert!(
        drift < time::Duration::seconds(5),
        "the window must be the one §2.4.1 gives a Strom UTILMD — expected \
         {expected}, got {}",
        window.due_at()
    );
}

/// §2.2.2.2: „Als Antwort auf eine empfangene CONTRL-Nachricht darf weder eine
/// CONTRL-Nachricht noch eine andere UN/EDIFACT-Nachricht gesendet werden."
#[tokio::test]
async fn a_broken_contrl_is_not_answered_with_a_contrl() {
    let (svc, store, _tenant) = service(OWN_STROM, "NB").await;
    let raw = format!(
        "UNB+UNOC:3+{COUNTERPARTY}:500+{OWN_STROM}:500+260912:0900+IC1'\
UNH+1+CONTRL:D:3:UN:2.0b'UCI+IC0'UNT+3+1'UNZ+1+IC1'"
    );

    svc.emit_syntax_error(
        raw.as_bytes(),
        "IC1",
        OWN_STROM,
        COUNTERPARTY,
        SyntaxFehler::UngueltigerWert,
    )
    .await
    .expect("the call must not fail");

    assert!(
        store
            .pending_now(10)
            .await
            .expect("read the outbox")
            .is_empty(),
        "no CONTRL-on-CONTRL, however broken the CONTRL"
    );
}

/// §2.2.2.1: without a readable `UNB` the CONTRL's Muss-Datenelemente cannot be
/// filled, so „der Fehler muss dann durch andere Mittel als durch die CONTRL
/// mitgeteilt werden." The sender is what the envelope would have supplied.
#[tokio::test]
async fn no_contrl_without_a_sender() {
    let (svc, store, _tenant) = service(OWN_STROM, "NB").await;

    svc.emit_syntax_error(
        &broken_utilmd(),
        "IC4711",
        OWN_STROM,
        "",
        SyntaxFehler::UngueltigerWert,
    )
    .await
    .expect("the call must not fail");

    assert!(
        store
            .pending_now(10)
            .await
            .expect("read the outbox")
            .is_empty(),
        "a CONTRL naming no recipient is not a CONTRL"
    );
}

/// `UNB` DE 0035 = `1` on a production endpoint is DE 0085 **25**
/// „Test-Kennzeichen nicht unterstützt" (CONTRL AHB 1.0 Kap. 3).
#[tokio::test]
async fn a_test_interchange_is_refused_with_its_own_code() {
    let (svc, store, _tenant) = service(OWN_STROM, "NB").await;

    svc.emit_syntax_error(
        &broken_utilmd(),
        "IC4711",
        OWN_STROM,
        COUNTERPARTY,
        SyntaxFehler::TestKennzeichen,
    )
    .await
    .expect("enqueued");

    let queued = store.pending_now(10).await.expect("read the outbox");
    let contrl = queued
        .iter()
        .find(|m| m.message_type.as_ref() == "CONTRL")
        .expect("a CONTRL is queued");
    assert_eq!(contrl.payload["syntax_error"], serde_json::json!("25"));
}
