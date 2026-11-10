//! The ingest door counts every AHB-Prüfung finding by its class and refuses,
//! with the mapped Verarbeitbarkeitsfehlermeldung, the classes it is told to
//! refuse.
//!
//! * Every finding is counted per class — the rule-id family `MIG-`, `AHB-`,
//!   `FMT-` — and message type, and a Formatbedingung finding also by its
//!   number, so a class's inbound count can be explained before it is refused.
//! * A Formatbedingung finding is answered `Z35` „Format nicht eingehalten"
//!   (APERAK AHB 1.1 Kap. 4.3, S. 45: „Der im Geschäftsvorfall angegebene Wert
//!   eines Datenelements erfüllt nicht die dafür festgelegte
//!   Formatdefinition"), with the MIG name of its segment as the obligatory
//!   Ortsangabe (Kap. 2.1.2.1, S. 12; Kap. 2.1.2.2, S. 12).
//! * A refused class's message is answered at the door and reaches no
//!   workflow; a class not refused reaches its workflow as before.

use std::sync::Arc;

use edi_energy::EdiEnergyMessage as _;
use http_body_util::BodyExt as _;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, Fehlerklasse, IngestOutcome};
use tower::ServiceExt as _;

const OWN: &str = "9900357000004";
const SENDER: &str = "4012345000023";
/// The refusal fixture: a MaLo-ID whose check digit is wrong.
const WRONG_MALO: &str = "51238696782";

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

/// Recount `UNT` DE 0074 after segments were removed.
fn recount(wire: &str) -> String {
    let unt = wire
        .split('\'')
        .find(|s| s.starts_with("UNT"))
        .expect("UNT")
        .to_owned();
    let count = wire
        .split('\'')
        .filter(|s| !s.trim().is_empty() && !s.starts_with("UNB") && !s.starts_with("UNZ"))
        .count();
    wire.replace(&format!("{unt}'"), &format!("UNT+{count}+1'"))
}

/// A 55001 whose `SG5 LOC+Z16` Marktlokation fails its check digit: a
/// `[950]` finding.
fn utilmd_wrong_malo() -> String {
    let wire = skeleton(edi_energy::MessageType::Utilmd, 55001);
    let loc = wire
        .split('\'')
        .find(|s| s.starts_with("LOC+Z16+"))
        .expect("a 55001 names its Marktlokation")
        .to_owned();
    wire.replace(&format!("{loc}'"), &format!("LOC+Z16+{WRONG_MALO}'"))
}

/// A 55001 whose `DTM+92` Beginn zum is UTC midnight — no German day starts
/// there, so `[UB1]` (`[932]`/`[933]`) is violated.
fn utilmd_utc_midnight() -> String {
    let wire = skeleton(edi_energy::MessageType::Utilmd, 55001);
    let dtm = wire
        .split('\'')
        .find(|s| s.starts_with("DTM+92:"))
        .expect("a 55001 carries its Beginn zum")
        .to_owned();
    wire.replace(&format!("{dtm}'"), "DTM+92:202701010000?+00:303'")
}

/// An MSCONS 13018 whose Muss `DTM+137` is missing — a `Z29` AHB-Fehler.
fn mscons_without_nachrichtendatum() -> String {
    let wire = skeleton(edi_energy::MessageType::Mscons, 13018);
    let dtm = wire
        .split('\'')
        .find(|s| s.starts_with("DTM+137:"))
        .expect("a 13018 carries its Nachrichtendatum")
        .to_owned();
    recount(
        &wire
            .replacen(&format!("{dtm}'"), "", 1)
            .replace("LOC+172'", "LOC+172+51238696781'"),
    )
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

fn dispatcher(store: &SlateDbStore, refused: &[Fehlerklasse]) -> EdifactIngestDispatcher {
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    )
    .with_abgewiesene_klassen(refused)
}

/// `POST /edifact` routing `pid` to `workflow`, through a dispatcher that
/// refuses `refused`.
async fn post(
    edifact: String,
    pid: u32,
    workflow: &'static str,
    refused: &[Fehlerklasse],
) -> (SlateDbStore, serde_json::Value) {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let tenant = TenantId::from_party_id(OWN);
    let mut pid_router = mako_engine::pid_router::PidRouter::new();
    pid_router.register(pid, workflow);
    let state = Arc::new(makod::edifact_api::EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router,
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
        dl_sink: Arc::new(store.as_dead_letter_sink(tenant)),
        dispatcher: Some(Arc::new(dispatcher(&store, refused))),
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

/// The value of one counter label, `0` when absent.
fn count(pairs: &[(Box<str>, u64)], label: &str) -> u64 {
    pairs
        .iter()
        .find(|(l, _)| &**l == label)
        .map_or(0, |(_, n)| *n)
}

fn snapshot() -> mako_engine::metrics::MetricsSnapshot {
    mako_engine::metrics::EngineMetrics::global().snapshot()
}

/// Render an APERAK payload and run the APERAK AHB over it.
fn assert_passes_the_aperak_ahb(payload: &mako_engine::payload::AperakPayload) -> String {
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId};
    let pending = mako_engine::outbox::PendingOutbox::typed(SENDER, payload);
    let msg = mako_engine::outbox::OutboxMessage::new(
        StreamId::new("process/door-findings"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        pending.message_type,
        pending.recipient,
        pending.payload,
    );
    let registry =
        makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: OWN.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("valid registry");
    let rendered = makod::edifact_renderer::render_to_wire_bytes(&msg, &registry).expect("renders");
    let wire = String::from_utf8(rendered.bytes.clone()).expect("UTF-8");
    let report = edi_energy::parse(&rendered.bytes)
        .expect("parses")
        .validate()
        .expect("validation runs");
    assert!(
        report.is_sendable(),
        "the door's APERAK fails the APERAK AHB:\n{wire}\n{:?}",
        report.sending_refusals().collect::<Vec<_>>()
    );
    wire
}

// ── Counted ──────────────────────────────────────────────────────────────────

/// A wrong MaLo-ID check digit raises the counter for `[950]` and for the
/// `FMT` class — and, the class not being refused, the message reaches its
/// workflow.
#[tokio::test]
async fn a_wrong_malo_check_digit_is_counted_under_its_formatbedingung() {
    let before = snapshot();
    let (_, body) = post(utilmd_wrong_malo(), 55001, "gpke-supplier-change", &[]).await;
    let after = snapshot();
    assert!(
        count(&after.formatbedingung_findings, "UTILMD,950")
            > count(&before.formatbedingung_findings, "UTILMD,950"),
        "{body}\n{:?}",
        after.formatbedingung_findings
    );
    assert!(
        count(&after.inbound_findings, "FMT,UTILMD")
            > count(&before.inbound_findings, "FMT,UTILMD"),
        "{:?}",
        after.inbound_findings
    );
}

// ── Mapped to Z35 ─────────────────────────────────────────────────────────────

/// The `[950]` finding of a refused `FMT` class is answered `ERC+Z35`, with the
/// MIG name of `SG5 LOC+Z16` as Ortsangabe, and the APERAK passes the APERAK
/// AHB.
#[tokio::test]
async fn a_formatbedingung_finding_is_answered_z35_with_its_ortsangabe() {
    let msg = edi_energy::parse(utilmd_wrong_malo().as_bytes()).expect("parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    dispatcher(&store, &[Fehlerklasse::Fmt])
        .dispatch(
            &msg,
            "gpke-supplier-change",
            55001,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let all: Vec<_> = std::iter::once((
        sent[0].error_code.clone(),
        sent[0].ortsangabe.clone(),
        sent[0].reason.clone(),
    ))
    .chain(sent[0].weitere_fehler.iter().map(|f| {
        (
            Some(f.error_code.clone()),
            f.ortsangabe.clone(),
            f.reason.clone(),
        )
    }))
    .collect();
    let z35 = all
        .iter()
        .find(|(code, _, reason)| {
            code.as_deref() == Some("Z35") && reason.as_deref().is_some_and(|r| r.contains("[950]"))
        })
        .unwrap_or_else(|| panic!("a Z35 names [950]: {all:?}"));
    assert_eq!(z35.1.as_deref(), Some("Marktlokation"), "{all:?}");
    let wire = assert_passes_the_aperak_ahb(&sent[0]);
    assert!(wire.contains("ERC+Z35'"), "{wire}");
    assert!(wire.contains("FTX+Z02+++Marktlokation'"), "{wire}");
}

// ── Enforced per class ─────────────────────────────────────────────────

/// With the `AHB` class refused, a message with an AHB-Fehler is answered at
/// the door — `Z29` naming the segment — and reaches no workflow.
#[tokio::test]
async fn a_refused_class_reaches_no_workflow() {
    let msg = edi_energy::parse(mscons_without_nachrichtendatum().as_bytes()).expect("parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let outcome = dispatcher(&store, &[Fehlerklasse::Ahb])
        .dispatch(
            &msg,
            "gpke-messwerte",
            13018,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(
            outcome,
            IngestOutcome::Skipped {
                reason: makod::ingest_dispatcher::SKIP_REFUSED_AT_DOOR,
                ..
            }
        ),
        "{outcome:?}"
    );
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].error_code.as_deref(), Some("Z29"), "{sent:?}");
    assert_eq!(sent[0].ortsangabe.as_deref(), Some("Nachrichtendatum"));
    assert_passes_the_aperak_ahb(&sent[0]);
}

/// With no class refused — the shipped default — the same message reaches
/// its workflow, and the `AHB` class counter rises.
#[tokio::test]
async fn a_class_not_refused_reaches_its_workflow_and_is_counted() {
    let before = snapshot();
    let (_, body) = post(
        mscons_without_nachrichtendatum(),
        13018,
        "gpke-messwerte",
        makod::ingest_dispatcher::ABGEWIESENE_KLASSEN,
    )
    .await;
    let after = snapshot();
    assert!(
        body["messages"][0]["process_id"].is_string(),
        "the message reached its workflow: {body}"
    );
    assert!(
        count(&after.inbound_findings, "AHB,MSCONS")
            > count(&before.inbound_findings, "AHB,MSCONS"),
        "{:?}",
        after.inbound_findings
    );
}

/// No class is refused until its inbound count is explained.
#[test]
fn no_class_is_refused_by_default() {
    assert!(makod::ingest_dispatcher::ABGEWIESENE_KLASSEN.is_empty());
}

// ── The day boundary ───────────────────────────────────────────────────

/// A 55001 whose `DTM+92` is UTC midnight starts no German day. With the `FMT`
/// class refused it is answered at the door with `Z35` naming the `DTM` —
/// rather than reaching the adapter, which refuses it unanswered.
#[tokio::test]
async fn a_process_date_off_the_day_boundary_is_answered_at_the_door() {
    let wire = utilmd_utc_midnight();
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let outcome = dispatcher(&store, &[Fehlerklasse::Fmt])
        .dispatch(
            &msg,
            "gpke-supplier-change",
            55001,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("answered at the door, not refused by the adapter");
    assert!(
        matches!(
            outcome,
            IngestOutcome::Skipped {
                reason: makod::ingest_dispatcher::SKIP_REFUSED_AT_DOOR,
                ..
            }
        ),
        "{outcome:?}"
    );
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].error_code.as_deref(), Some("Z35"), "{sent:?}");
    assert!(
        sent[0]
            .reason
            .as_deref()
            .is_some_and(|r| r.starts_with("DTM ") && r.contains("2380")),
        "the Fehlerbeschreibung names the DTM and its data element: {sent:?}"
    );
    // The MIG's name for `SG4 DTM+92` (Nr 00020).
    assert_eq!(
        sent[0].ortsangabe.as_deref(),
        Some("Beginn zum"),
        "{sent:?}"
    );
    let rendered = assert_passes_the_aperak_ahb(&sent[0]);
    assert!(rendered.contains("ERC+Z35'"), "{rendered}");
    assert!(rendered.contains("FTX+Z02+++Beginn zum'"), "{rendered}");
}
