//! The windows makod's Gas ingest registers come from a catalogued source, from
//! the instant the message arrived — never from a Werktage literal beside the
//! call site.
//!
//! - ORDERS 17115 / 17117 on the GNB side: „spätester ÜT ist der 1. WT nach dem
//!   ÜT von Nr. 1" (GPKE Teil 2 Kap. 3.5.1.2 / 3.5.2.2 Nr. 2), the Sparte-neutral
//!   rows of `mako_fristen::antwort` the LF side registers for the same
//!   Auftrag; GeLi Gas 3.0 holds no Sperrprozess.
//! - UTILMD 44022: no Festlegung quantifies the answer to a Stornierung, so the
//!   window is the operating convention `operator_window` names as such.
//! - INVOIC 31011 / 31010: the Zahlungsziel the invoice states in `SG8
//!   DTM+265`, Muss on both (INVOIC AHB 1.0b Kap. 3.1.5, S. 53 / Kap. 3.1.4,
//!   S. 45); an invoice without it is refused.
//!
//! The Sperrauftrag cases arrive Friday 23.10.2026, two days before summer
//! time ends, so a window counted in UTC or in the wrong offset is off by an
//! hour on Monday.

use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::ids::{StreamId, TenantId};
use mako_engine::store_slatedb::SlateDbStore;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
use time::macros::datetime;

const GNB: &str = "9800357000005";
const LF: &str = "9800555000006";

/// Friday 2026-10-23, 12:00 MESZ.
const RECEIVED_AT: time::OffsetDateTime = datetime!(2026-10-23 10:00 UTC);

/// The skeleton of `pid` in the newest profile of `message` that carries it.
fn skeleton(message: edi_energy::MessageType, pid: u32) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(message)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    let wire = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: LF.to_owned(),
                receiver: GNB.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8(wire).expect("UTF-8")
}

/// `wire` with the segment starting `prefix` replaced by `segment`, or removed
/// when `segment` is empty.
fn replace_segment(wire: &str, prefix: &str, segment: &str) -> String {
    let at = wire
        .find(prefix)
        .unwrap_or_else(|| panic!("the skeleton has a {prefix}"));
    let end = at + wire[at..].find('\'').expect("segment terminator") + 1;
    format!("{}{segment}{}", &wire[..at], &wire[end..])
}

fn parse(wire: &str) -> edi_energy::AnyMessage {
    edi_energy::parse(wire.as_bytes()).unwrap_or_else(|e| panic!("parses: {e}\n{wire}"))
}

async fn dispatcher() -> (SlateDbStore, EdifactIngestDispatcher) {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(GNB),
    );
    (store, d)
}

/// The one deadline labelled `label` that dispatching `msg` to `workflow`
/// registers.
async fn registered_due(
    msg: &edi_energy::AnyMessage,
    workflow: &str,
    pid: u32,
    label: &str,
) -> time::OffsetDateTime {
    let (store, d) = dispatcher().await;
    let outcome = d
        .dispatch(msg, workflow, pid, RECEIVED_AT)
        .await
        .expect("dispatched");
    let IngestOutcome::Spawned { process_id, .. } = outcome else {
        panic!("the {pid} spawns: {outcome:?}");
    };
    let deadlines = store
        .as_deadline_store()
        .for_stream(&StreamId::for_process(
            TenantId::from_party_id(GNB),
            &process_id,
        ))
        .await
        .expect("read deadlines");
    let labelled: Vec<_> = deadlines.iter().filter(|d| d.label() == label).collect();
    assert_eq!(labelled.len(), 1, "{deadlines:?}");
    labelled[0].due_at()
}

/// The GNB answers a Gas-Sperr- or Entsperrauftrag by the end of the next
/// Werktag — Monday 26.10.2026, MEZ — not ten Werktage later.
#[tokio::test]
async fn the_gnb_answers_a_sperrauftrag_by_the_next_werktag() {
    for pid in [17_115, 17_117] {
        let msg = parse(&skeleton(edi_energy::MessageType::Orders, pid));
        let due = registered_due(
            &msg,
            mako_geli_gas::sperrung_nb::WORKFLOW_NAME,
            pid,
            mako_geli_gas::GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL,
        )
        .await;
        assert_eq!(
            due,
            datetime!(2026-10-26 23:59:59.999_999_999 +01:00),
            "PID {pid}"
        );
        assert_eq!(
            Some(due),
            mako_fristen::antwort::antwort_deadline(pid, RECEIVED_AT),
            "PID {pid}: the row the LF side registers"
        );
    }
}

/// The answer to a Stornierung is an operating convention, and registered as
/// the one `operator_window` states — not a ten-Werktage window attributed to
/// BK7-24-01-009.
#[tokio::test]
async fn a_stornierungsanfrage_carries_the_operating_convention() {
    let msg = parse(&skeleton(edi_energy::MessageType::Utilmd, 44_022));
    let due = registered_due(
        &msg,
        mako_geli_gas::stornierung::WORKFLOW_NAME,
        44_022,
        mako_geli_gas::STORNIERUNG_RESPONSE_WINDOW_LABEL,
    )
    .await;
    let window = mako_fristen::antwort::operator_window(44_022, RECEIVED_AT);
    assert!(!window.is_regulatory, "no Festlegung quantifies it");
    assert_eq!(due, window.deadline);
}

/// An INVOIC's settlement window is the Zahlungsziel it states.
#[tokio::test]
async fn an_invoic_is_due_at_its_zahlungsziel() {
    for (pid, workflow, label) in [
        (
            31_011,
            "geli-gas-sperrprozesse-invoic",
            mako_geli_gas::SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL,
        ),
        (
            31_010,
            "gabi-gas-invoic",
            mako_gabi_gas::INVOIC_SETTLEMENT_WINDOW_LABEL,
        ),
    ] {
        let wire = replace_segment(
            &skeleton(edi_energy::MessageType::Invoic, pid),
            "DTM+265:",
            "DTM+265:202612042300?+00:303'",
        );
        let due = registered_due(&parse(&wire), workflow, pid, label).await;
        assert_eq!(due, datetime!(2026-12-04 23:00 UTC), "PID {pid}");
    }
}

/// Without its Fälligkeitsdatum the window cannot be set: the invoice is
/// refused, never given a Werktage count nobody stated.
#[tokio::test]
async fn an_invoic_without_its_zahlungsziel_is_refused() {
    for (pid, workflow) in [
        (31_011, "geli-gas-sperrprozesse-invoic"),
        (31_010, "gabi-gas-invoic"),
    ] {
        let wire = replace_segment(
            &skeleton(edi_energy::MessageType::Invoic, pid),
            "DTM+265:",
            "",
        );
        let (_store, d) = dispatcher().await;
        let refused = d.dispatch(&parse(&wire), workflow, pid, RECEIVED_AT).await;
        assert!(
            matches!(refused, Err(ref e) if e.to_string().contains("DTM+265")),
            "PID {pid}: {refused:?}"
        );
    }
}
