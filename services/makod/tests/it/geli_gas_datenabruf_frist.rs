//! The GNB side of the GeLi Gas Datenabruf registers the window its Festlegung
//! publishes, from the instant the Anfrage arrived.
//!
//! - ORDERS 17103 (Anfrage Brennwert / Zustandszahl): „spätestens 10 Werktage
//!   nach Eingang der Anfrage des Lieferanten, aber nicht vor dem 10. Werktag
//!   des Monats der auf den Monat folgt, in den das Ende der … Zeitspanne fällt"
//!   (BK7-24-01-009 GeLi Gas 3.0 Kap. 4.2, S. 13). The Zeitspanne's end is the
//!   ORDERS `SG29 DTM+164` (ORDERS AHB 1.1b Kap. 4.1.3).
//!
//! The 17104 window (GPKE Teil 4 Kap. 3.2 Nr. 2) is pinned beside
//! `ingest_dispatcher::geli_gas::datenabruf_antwort_due`.
//!
//! A flat ten Werktage reports an Anfrage about the running month due before
//! the GNB may answer it at all.

use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::ids::{StreamId, TenantId};
use mako_engine::store_slatedb::SlateDbStore;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
use time::macros::datetime;

const OWN: &str = "9800357000005";
const LF: &str = "9800555000006";

/// Friday 2026-10-23, 12:00 MESZ — two days before summer time ends.
const RECEIVED_AT: time::OffsetDateTime = datetime!(2026-10-23 10:00 UTC);

/// The skeleton ORDERS of `pid`.
fn skeleton(pid: u32) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Orders)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    let wire = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: LF.to_owned(),
                receiver: OWN.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8(wire).expect("UTF-8")
}

/// The `DTM+164` segment of a 17103 skeleton as `(start, end)` byte offsets,
/// the terminator included.
fn dtm_164(wire: &str) -> (usize, usize) {
    let at = wire
        .find("DTM+164:")
        .expect("the 17103 skeleton has a DTM+164");
    (
        at,
        at + wire[at..].find('\'').expect("segment terminator") + 1,
    )
}

/// The skeleton ORDERS of `pid`, with its `DTM+164` replaced by `ende` where
/// one is given.
fn anfrage(pid: u32, ende: Option<&str>) -> edi_energy::AnyMessage {
    let mut wire = skeleton(pid);
    if let Some(ende) = ende {
        let (at, end) = dtm_164(&wire);
        wire = format!("{}DTM+164:{ende}:303'{}", &wire[..at], &wire[end..]);
    }
    edi_energy::parse(wire.as_bytes()).expect("the Anfrage parses")
}

/// The answer window registered when `msg` arrives at [`RECEIVED_AT`].
async fn registered_due(msg: &edi_energy::AnyMessage, pid: u32) -> time::OffsetDateTime {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let tenant = TenantId::from_party_id(OWN);
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    );
    let outcome = d
        .dispatch(
            msg,
            mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
            pid,
            RECEIVED_AT,
        )
        .await
        .expect("the Anfrage is dispatched");
    let IngestOutcome::Spawned { process_id, .. } = outcome else {
        panic!("the {pid} spawns: {outcome:?}");
    };
    let deadlines = store
        .as_deadline_store()
        .for_stream(&StreamId::for_process(tenant, &process_id))
        .await
        .expect("read deadlines");
    let answer: Vec<_> = deadlines
        .iter()
        .filter(|d| d.label() == mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL)
        .collect();
    assert_eq!(answer.len(), 1, "{deadlines:?}");
    answer[0].due_at()
}

/// A Zeitspanne ending 22.10.2026: its Brennwert exists from the 10. Werktag of
/// November (13.11.), later than the ten Werktage after Eingang (06.11.). The
/// window runs over the end of summer time, so it closes at 23:59:59 MEZ.
#[tokio::test]
async fn a_brennwert_anfrage_waits_for_the_tenth_werktag_of_the_following_month() {
    // DTM+164: the Gastag after the last one begins 23.10.2026 06:00 MESZ.
    let due = registered_due(&anfrage(17103, Some("202610230400?+00")), 17103).await;
    assert_eq!(
        due,
        datetime!(2026-11-13 23:59:59.999_999_999 +01:00),
        "the 10. Werktag of November, end of day MEZ"
    );
}

/// A Zeitspanne long past: the ten Werktage after Eingang bind — 06.11.2026,
/// with the Reformationstag (31.10.) a Saturday and no other holiday between.
#[tokio::test]
async fn a_brennwert_anfrage_about_an_old_zeitspanne_is_due_ten_werktage_after_eingang() {
    // Zeitspanne ending 31.08.2026: DTM+164 is 01.09.2026 06:00 MESZ.
    let due = registered_due(&anfrage(17103, Some("202609010400?+00")), 17103).await;
    assert_eq!(due, datetime!(2026-11-06 23:59:59.999_999_999 +01:00));
}

/// An Ende on the Strom day boundary (00:00) names the same last Gastag.
#[tokio::test]
async fn an_ende_at_midnight_names_the_same_gastag() {
    // 23.10.2026 00:00 MESZ — the last Gastag is 22.10. either way.
    let due = registered_due(&anfrage(17103, Some("202610222200?+00")), 17103).await;
    assert_eq!(due, datetime!(2026-11-13 23:59:59.999_999_999 +01:00));
}

/// Without its `DTM+164` the window cannot be computed: the Anfrage is
/// refused, never given the ten Werktage alone.
#[tokio::test]
async fn a_brennwert_anfrage_without_its_ende_is_refused() {
    let wire = skeleton(17103);
    let (at, end) = dtm_164(&wire);
    let wire = format!("{}{}", &wire[..at], &wire[end..]);
    let msg = edi_energy::parse(wire.as_bytes()).expect("still parses");
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    let refused = d
        .dispatch(
            &msg,
            mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
            17103,
            RECEIVED_AT,
        )
        .await;
    assert!(
        matches!(refused, Err(ref e) if e.to_string().contains("DTM+164")),
        "{refused:?}"
    );
}

/// An Anfrage des MSB Gas an den NB Strom may name its Messlokation by address
/// alone: the Meldepunkt `LOC+172` is conditional (`[64]`, „Wenn SG2 NAD+Z03
/// nicht vorhanden") and the Messlokationsadresse its alternative (ORDERS AHB
/// 1.1b Kap. 4.1.4, S. 20). Such a 17104 spawns under the sender and its
/// Dokumentennummer and is due the next Werktag — here across the end of
/// summer time.
#[tokio::test]
async fn an_address_only_anfrage_vom_msb_gas_spawns_under_its_dokumentennummer() {
    let mut wire = skeleton(17104);
    if let Some(at) = wire.find("LOC+172") {
        let end = at + wire[at..].find('\'').expect("segment terminator") + 1;
        wire = format!("{}{}", &wire[..at], &wire[end..]);
    }
    if !wire.contains("NAD+Z03") {
        let at = wire.find("UNS+S'").expect("the skeleton has its UNS");
        wire = format!(
            "{}NAD+Z03++Musterstrasse 1:+++Musterstadt++12345+DE'{}",
            &wire[..at],
            &wire[at..]
        );
    }
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    assert!(!wire.contains("LOC+172"), "{wire}");
    let due = registered_due(&msg, 17104).await;
    assert_eq!(due, datetime!(2026-10-26 23:59:59.999_999_999 +01:00));
}

/// Two Anfragen from one MSB Gas are two processes: the key is the Anfrage,
/// not the Messlokation, and the Messlokation a 17104 names is indexed beside it.
#[tokio::test]
async fn every_anfrage_vom_msb_gas_is_its_own_process() {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let tenant = TenantId::from_party_id(OWN);
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    );
    let wire = skeleton(17104);
    let bgm = wire.find("BGM+Z14+").expect("the skeleton has its BGM");
    let bgm_end = bgm + wire[bgm..].find('\'').expect("terminator") + 1;
    for nummer in ["ANF-A", "ANF-B"] {
        let wire = format!("{}BGM+Z14+{nummer}'{}", &wire[..bgm], &wire[bgm_end..]);
        let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
        let outcome = d
            .dispatch(
                &msg,
                mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
                17104,
                RECEIVED_AT,
            )
            .await
            .expect("dispatched");
        assert!(
            matches!(outcome, IngestOutcome::Spawned { .. }),
            "{nummer}: {outcome:?}"
        );
    }
}
