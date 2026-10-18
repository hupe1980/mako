//! A partly broken interchange is answered by one CONTRL naming each broken
//! message.
//!
//! CONTRL AHB 1.0 (Stand 11.12.2025) Kap. 1.3: „Auf eine Übertragungsdatei ist
//! vom Empfänger genau eine CONTRL an den Absender der Übertragungsdatei zu
//! senden. In der CONTRL wird entweder eine Übertragungsdatei bestätigt oder die
//! gesamte Übertragungsdatei zurückgewiesen." Kap. 3 gives the column
//! „Syntaxfehlermeldung in der Nachricht": `UCI` DE 0083 = `4` without DE 0085,
//! an `SG1 UCM` per broken message with DE 0083 = `4`, and `SG2 UCS` / `UCD`
//! naming the segment (DE 0096, `UNH` = 1) and the data element (DE 0098, the
//! segment tag = 1; CONTRL MIG 2.0b). Both Sparten owe it: in Gas it replaces
//! the Empfangsbestätigung, in Strom it is the CONTRL's only use (Kap. 2.4).
//!
//! Kap. 1.2: „bei Syntaxfehlern [werden] alle in der Übertragungsdatei
//! enthaltenen Geschäftsvorfälle vom Empfänger nicht verarbeitet" — a readable
//! message beside a broken one is rejected with the file.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::store_slatedb::SlateDbStore;
use makod::contrl_ack::{ContrlAckService, Fehlerort, Syntaxpruefung, syntaxpruefung};
use makod::party_registry::MpIdRegistry;

const OWN_STROM: &str = "9900357000004";
const OWN_GAS: &str = "9870012345678";
const COUNTERPARTY: &str = "4012345000023";

fn registry(mp_id: &str, role: &str) -> Arc<MpIdRegistry> {
    Arc::new(
        MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: vec![role.to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("valid registry"),
    )
}

/// One skeleton message of `pid`, `UNH` through `UNT`, with reference `msg_ref`.
fn message(pid: u32, receiver: &str, msg_ref: &str) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Utilmd)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a UTILMD profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: COUNTERPARTY.to_owned(),
                receiver: receiver.to_owned(),
            },
        )
        .expect("skeleton");
    let text = String::from_utf8(bytes).expect("ASCII");
    let start = text.find("UNH+").expect("UNH");
    let end = text.find("UNZ+").expect("UNZ");
    // The skeleton's message reference is `1`, in `UNH` and in the closing `UNT`.
    let body = text[start..end].replacen("UNH+1+", &format!("UNH+{msg_ref}+"), 1);
    let trimmed = body
        .strip_suffix("+1'")
        .expect("UNT closes with the reference");
    format!("{trimmed}+{msg_ref}'")
}

/// Two messages; the second's `BGM` tag is broken.
fn interchange(pid: u32, receiver: &str, agency: &str) -> String {
    let good = message(pid, receiver, "1");
    let bad = message(pid, receiver, "2").replacen("BGM+", "BGM", 1);
    format!(
        "UNB+UNOC:3+{COUNTERPARTY}:{agency}+{receiver}:{agency}+261001:0700+IC9'{good}{bad}UNZ+2+IC9'"
    )
}

fn parse(wire: &str) -> Vec<Result<edi_energy::AnyMessage, edi_energy::Error>> {
    edi_energy::Platform::with_all_profiles()
        .parse_interchange(std::io::Cursor::new(wire.as_bytes()))
        .collect()
}

#[test]
fn the_report_names_the_broken_message_its_segment_and_its_element() {
    let wire = interchange(55001, OWN_STROM, "500");
    let results = parse(&wire);
    assert_eq!(results.len(), 2, "one result per message");
    assert!(results[0].is_ok() && results[1].is_err(), "{results:?}");

    let Syntaxpruefung::Nachrichten(fehler) = syntaxpruefung(wire.as_bytes(), &results) else {
        panic!("a locatable fault is reported per message");
    };
    assert_eq!(fehler.len(), 1, "only the broken message is named");
    let f = &fehler[0];
    assert_eq!(f.message_ref, "2");
    assert_eq!(f.message_type, "UTILMD");
    // BGM is the second segment, counting UNH as 1 (CONTRL MIG 2.0b, DE 0096);
    // the broken part is its tag, which DE 0098 counts as position 1.
    let Fehlerort::Segment {
        position,
        code,
        datenelement,
    } = &f.fehler
    else {
        panic!("the fault is inside the message, not in UNH/UNT: {f:?}");
    };
    assert_eq!(*position, 2);
    assert_eq!(*code, None, "UCS DE 0085 only when no UCD can be given");
    let de = datenelement.as_ref().expect("the element is known");
    assert_eq!(de.code, "12");
    assert_eq!(de.position, 1);
}

#[test]
fn a_clean_interchange_is_fehlerfrei_and_an_unreadable_one_is_the_whole_file() {
    let good = format!(
        "UNB+UNOC:3+{COUNTERPARTY}:500+{OWN_STROM}:500+261001:0700+IC9'{}UNZ+1+IC9'",
        message(55001, OWN_STROM, "1")
    );
    assert!(matches!(
        syntaxpruefung(good.as_bytes(), &parse(&good)),
        Syntaxpruefung::Fehlerfrei
    ));

    let garbage = "\u{0}\u{1}not edifact";
    assert!(matches!(
        syntaxpruefung(garbage.as_bytes(), &parse(garbage)),
        Syntaxpruefung::Uebertragungsdatei(_)
    ));
}

async fn rendered(own: &str, role: &str, wire: &str) -> String {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let registry = registry(own, role);
    let svc = ContrlAckService::new(
        Arc::new(store.clone()),
        TenantId::from_party_id(own),
        Arc::clone(&registry),
    );
    let Syntaxpruefung::Nachrichten(fehler) = syntaxpruefung(wire.as_bytes(), &parse(wire)) else {
        panic!("per-message report expected");
    };
    svc.emit_message_syntax_errors(wire.as_bytes(), "IC9", own, COUNTERPARTY, &fehler)
        .await
        .expect("enqueued");
    let queued = store.pending_now(10).await.expect("outbox");
    let contrl = queued
        .iter()
        .find(|m| m.message_type.as_ref() == "CONTRL")
        .expect("one CONTRL is queued");
    let bytes = makod::edifact_renderer::render_to_wire_bytes(contrl, &registry)
        .expect("renders")
        .bytes;
    String::from_utf8(bytes).expect("ASCII")
}

fn assert_per_message(wire: &str) {
    assert!(wire.contains("UNH+"), "{wire}");
    assert!(wire.contains("+CONTRL:D:3:UN:2.0b'"), "BDEW UNH: {wire}");
    let uci = wire
        .split('\'')
        .find(|s| s.starts_with("UCI+"))
        .expect("UCI");
    assert!(
        uci.ends_with("+4"),
        "UCI DE 0083 = 4 and no DE 0085 in the „in der Nachricht\" column: {uci}"
    );
    // CONTRL MIG 2.0b, UCI Bemerkung (S. 7): S002 / S003 copy the received
    // UNB DE 0004 / DE 0010 — the counterparty first, although the CONTRL
    // itself travels from us to it.
    assert!(
        uci.starts_with(&format!("UCI+IC9+{COUNTERPARTY}:")),
        "S002 is the acknowledged interchange's sender: {uci}"
    );
    assert!(
        wire.contains("'UCM+2+UTILMD:D:11A:UN:"),
        "SG1 UCM names the broken message: {wire}"
    );
    let ucm = wire
        .split('\'')
        .find(|s| s.starts_with("UCM+"))
        .expect("UCM");
    assert!(ucm.ends_with("+4"), "UCM DE 0083 = 4: {ucm}");
    assert!(
        !wire.contains("UCM+1+"),
        "the readable message is not named"
    );
    assert!(wire.contains("'UCS+2'"), "SG2 UCS DE 0096: {wire}");
    assert!(
        wire.contains("'UCD+12+1'"),
        "SG2 UCD DE 0085 / DE 0098: {wire}"
    );
}

#[tokio::test]
async fn strom_gets_the_per_message_syntaxfehlermeldung() {
    let wire = rendered(OWN_STROM, "NB", &interchange(55001, OWN_STROM, "500")).await;
    assert_per_message(&wire);
}

#[tokio::test]
async fn gas_gets_the_per_message_syntaxfehlermeldung_not_an_empfangsbestaetigung() {
    let wire = rendered(OWN_GAS, "NB", &interchange(44001, OWN_GAS, "502")).await;
    assert_per_message(&wire);
}

/// `POST /edifact` answers a partly broken file as the AS4 door does: nothing
/// of it is dispatched, and the sender is owed the per-message CONTRL.
#[tokio::test]
async fn the_rest_door_rejects_the_whole_file_and_owes_the_contrl() {
    use tower::ServiceExt as _;

    let store = SlateDbStore::open_in_memory().await.expect("store");
    let tenant = TenantId::from_party_id(OWN_STROM);
    let registry = registry(OWN_STROM, "NB");
    let dispatcher = Arc::new(makod::ingest_dispatcher::EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    ));
    let mut pid_router = mako_engine::pid_router::PidRouter::new();
    pid_router.register(55001, "gpke-supplier-change");
    let state = Arc::new(makod::edifact_api::EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router,
        mp_id_registry: Arc::clone(&registry),
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        max_body_bytes: 1024 * 1024,
        partner_store: None,
        tenant_id: tenant,
        dl_sink: Arc::new(mako_engine::dead_letter::LogDeadLetterSink),
        dispatcher: Some(dispatcher),
        contrl_ack: Some(Arc::new(ContrlAckService::new(
            Arc::new(store.clone()),
            tenant,
            Arc::clone(&registry),
        ))),
    });

    let response = makod::edifact_api::router(state)
        .oneshot(
            axum::http::Request::post("/edifact")
                .header("content-type", "application/edifact")
                .body(axum::body::Body::from(interchange(55001, OWN_STROM, "500")))
                .expect("request"),
        )
        .await
        .expect("infallible");
    let status = response.status();
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let queued = store.pending_now(10).await.expect("outbox");
    assert!(
        queued.iter().all(|m| m.message_type.as_ref() == "CONTRL"),
        "the readable message is not processed either: {:?}",
        queued
            .iter()
            .map(|m| m.message_type.as_ref())
            .collect::<Vec<_>>()
    );
    let contrl = queued
        .first()
        .unwrap_or_else(|| panic!("the CONTRL is owed: {}", String::from_utf8_lossy(&body)));
    assert_eq!(contrl.payload["messages"][0]["message_ref"], "2");
}
