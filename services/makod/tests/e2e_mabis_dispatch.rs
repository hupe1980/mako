//! The three MaBiS workflows must actually reach their dispatch arms.
//!
//! `e2e_dispatch_coverage_guard` cannot cover them. It drives each registered
//! PID with a real fixture, and a fixture only exists where the PID has an AHB
//! profile entry — `generate-fixtures` skips the rest. All 35 PIDs behind
//! `mabis-zp-lifecycle`, `mabis-anforderung` and `mabis-listenabgleich` are in
//! `KNOWN_PROFILE_GAPS`, so that guard silently exercises none of them.
//!
//! Which means the wiring for these three workflows — adapter, ingest arm,
//! router registration — was verified by nothing. The domain state machines have
//! unit tests, but a state machine nobody can reach is not a feature. This test
//! closes that hole with hand-built interchanges, and it asserts more than the
//! coverage guard does: not only that the PID reaches an arm, but *which*
//! outcome it produces.
//!
//! When these PIDs gain AHB profiles the coverage guard will start exercising
//! them too, and this file becomes redundant rather than wrong.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const SENDER_MP: &str = "4012345000023";
const LOC: &str = "51238696012";

async fn dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(OWN_MP);
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    )
}

/// A minimal UTILMD interchange announcing `pid`.
fn utilmd(pid: u32) -> String {
    utilmd_mit_serie(pid, None)
}

/// A UTILMD announcing `pid`, optionally carrying the `SG10` pair that names
/// which Summenzeitreihe it is about.
///
/// 55062 and 55063 are shared by eleven Summenzeitreihen, so a message using
/// them **must** carry `CCI+++ZB4` / `CAV` DE 7111 and `CCI+6` — the adapter
/// refuses without them rather than guessing, and there are eleven wrong
/// answers to guess between (UTILMD AHB Strom 2.2 Kap. 13.1).
fn utilmd_mit_serie(pid: u32, serie: Option<(&str, &str)>) -> String {
    let sg10 = serie.map_or_else(String::new, |(cav, verantwortlicher)| {
        format!("CCI+++ZB4'CAV+{cav}'CCI+6++{verantwortlicher}'")
    });
    format!(
        "UNB+UNOC:3+{SENDER_MP}:14+{OWN_MP}:14+230101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+000{pid}::+9'\
DTM+137:20230101:102'\
RFF+Z13:REF001'\
NAD+MS+{SENDER_MP}::293'\
NAD+MR+{OWN_MP}::293'\
IDE+24+{LOC}'\
{sg10}\
UNT+8+1'\
UNZ+1+1'"
    )
}

/// A minimal ORDERS interchange announcing `pid`.
///
/// `function` is BGM DE1225: `9` = original (Bestellung), `1` = cancellation
/// (Abbestellung). The adapter reads it to decide the Abonnement direction.
fn orders(pid: u32, function: &str) -> String {
    format!(
        "UNB+UNOC:3+{SENDER_MP}:14+{OWN_MP}:14+230101:0000+1'\
UNH+1+ORDERS:D:01B:UN:1.1b'\
BGM+Z01+000{pid}+{function}'\
DTM+137:20230101:102'\
RFF+ON:REF001'\
NAD+MS+{SENDER_MP}::293'\
NAD+MR+{OWN_MP}::293'\
LOC+172+{LOC}'\
UNT+8+1'\
UNZ+1+1'"
    )
}

/// Dispatch `edi` as `pid` on `workflow` and return the outcome.
async fn dispatch(edi: &str, workflow: &str, pid: u32) -> IngestOutcome {
    let msg = edi_energy::parse(edi.as_bytes())
        .unwrap_or_else(|e| panic!("test interchange for {pid} does not parse: {e}"));
    dispatcher()
        .await
        .dispatch(&msg, workflow, pid)
        .await
        .unwrap_or_else(|e| panic!("dispatch of {pid} errored: {e}"))
}

/// The `SG10` pair naming `serie`, for the families that share 55062/55063.
fn cav_fuer(serie: mako_mabis::ZpSerie) -> Option<(&'static str, &'static str)> {
    use mako_mabis::ZpSerie as S;
    Some(match serie {
        S::NetzzeitreiheNachbarNb | S::NetzzeitreiheBiko => ("ZA5", "ZA8"),
        S::LieferantensummenzeitreiheNb => ("ZA1", "ZA8"),
        S::LieferantensummenzeitreiheUenb => ("ZA3", "ZA9"),
        S::Bilanzierungsgebietssummenzeitreihe => ("Z95", "ZA9"),
        S::BilanzkreissummenzeitreiheNb => ("Z97", "ZA8"),
        S::BilanzkreissummenzeitreiheUenb => ("Z99", "ZA9"),
        S::Deltazeitreihenuebertrag => ("ZA4", "ZA9"),
        S::Abrechnungssummenzeitreihe => ("ZA6", "ZB7"),
        S::TaeglicheBgSzr => ("Z96", "ZA9"),
        S::TaeglicheBkSzr => ("ZA0", "ZA9"),
        // These have their own Anfrage PIDs and carry no CAV code.
        S::Zuordnungsermaechtigung
        | S::TaeglicheAauez
        | S::LfAaszr
        | S::MonatlicheAauezBkvLf
        | S::MonatlicheAauezBkvAnfNb => return None,
    })
}

/// A shared Anfrage code without the SG10 pair is refused, not guessed.
#[tokio::test]
async fn a_shared_anfrage_pid_without_its_series_is_refused() {
    let msg = edi_energy::parse(utilmd(55062).as_bytes()).expect("parses");
    let err = dispatcher()
        .await
        .dispatch(&msg, "mabis-zp-lifecycle", 55062)
        .await
        .expect_err("55062 alone does not say what was activated");
    assert!(
        format!("{err}").contains("Summenzeitreihen"),
        "the error must say why: {err}"
    );
}

/// `true` when the dispatcher had no arm for the PID and dropped the message.
fn was_dropped(outcome: &IngestOutcome) -> bool {
    matches!(outcome, IngestOutcome::Skipped { reason, .. } if reason.starts_with("pid_not_in_"))
}

#[tokio::test]
async fn every_zp_lifecycle_anfrage_reaches_its_arm() {
    for familie in mako_mabis::ZP_FAMILIEN {
        // A shared Anfrage code needs the SG10 pair; a code used by exactly one
        // series does not.
        let serie = (mako_mabis::serien_fuer_pid(familie.anfrage).len() > 1)
            .then(|| cav_fuer(familie.serie))
            .flatten();
        let outcome = dispatch(
            &utilmd_mit_serie(familie.anfrage, serie),
            "mabis-zp-lifecycle",
            familie.anfrage,
        )
        .await;
        assert!(
            !was_dropped(&outcome),
            "Anfrage {} was silently dropped — the ingest arm does not cover it",
            familie.anfrage
        );
    }
}

#[tokio::test]
async fn zp_lifecycle_answer_pids_resume_rather_than_spawn() {
    // 55064 answers both 55062 and 55063. Spawning a process on it would
    // answer an answer; it must resume a process this side already started.
    let outcome = dispatch(&utilmd(55064), "mabis-zp-lifecycle", 55064).await;
    match outcome {
        IngestOutcome::Skipped { reason, .. } => assert_eq!(
            reason, "answer_pid_resumes_only",
            "an answer PID must be recognised as resume-only, not dropped"
        ),
        other => panic!("expected a resume-only skip for 55064, got {other:?}"),
    }
}

#[tokio::test]
async fn every_anforderung_pid_reaches_its_arm() {
    for &pid in mako_mabis::ANFORDERUNG_PIDS {
        let outcome = dispatch(&orders(pid, "9"), "mabis-anforderung", pid).await;
        assert!(
            !was_dropped(&outcome),
            "Anforderung {pid} was silently dropped — the ingest arm does not cover it"
        );
    }
}

#[tokio::test]
async fn an_abbestellung_reaches_the_arm_and_is_decided_by_the_domain() {
    // BGM function `1` marks a cancellation, and the adapter reads it (verified
    // at the unit level). What this test can assert end-to-end is only that the
    // message reaches its arm.
    //
    // The refusal itself is *not* observable here. `handle` checks AHB
    // validation before the Abonnement rule, and these PIDs have no profile
    // entry, so a hand-built interchange fails MIG validation first and
    // legitimately spawns a `ValidationFailed` process — the Abbestellung branch
    // is never reached. That ordering is correct (an invalid message should not
    // be reasoned about further), which is why the one-shot refusal is pinned by
    // `anforderung::tests::an_inbound_abbestellung_on_a_one_shot_code_is_rejected`
    // against the workflow directly, where validation can be held true.
    //
    // Once 17201–17208 gain AHB profiles this becomes assertable end to end.
    let outcome = dispatch(&orders(17205, "1"), "mabis-anforderung", 17205).await;
    assert!(
        !was_dropped(&outcome),
        "a cancellation must still reach its arm: {outcome:?}"
    );
}

#[tokio::test]
async fn every_listenabgleich_list_reaches_its_arm() {
    for familie in mako_mabis::LISTEN_FAMILIEN {
        let outcome = dispatch(
            &utilmd(familie.liste),
            "mabis-listenabgleich",
            familie.liste,
        )
        .await;
        assert!(
            !was_dropped(&outcome),
            "list {} was silently dropped — the ingest arm does not cover it",
            familie.liste
        );
    }
}

#[tokio::test]
async fn listenabgleich_reply_pids_resume_rather_than_spawn() {
    for familie in mako_mabis::LISTEN_FAMILIEN {
        let outcome = dispatch(
            &utilmd(familie.antwort),
            "mabis-listenabgleich",
            familie.antwort,
        )
        .await;
        match outcome {
            IngestOutcome::Skipped { reason, .. } => assert_eq!(
                reason, "reply_pid_resumes_only",
                "reply PID {} must be resume-only",
                familie.antwort
            ),
            other => panic!(
                "expected a resume-only skip for {}, got {other:?}",
                familie.antwort
            ),
        }
    }
}

#[tokio::test]
async fn the_three_workflows_do_not_claim_each_others_pids() {
    // Each workflow must reject a PID belonging to a sibling, rather than
    // accepting it into the wrong state machine.
    let cases = [
        (55062, "mabis-anforderung"),
        (17205, "mabis-zp-lifecycle"),
        (55195, "mabis-zp-lifecycle"),
        (55062, "mabis-listenabgleich"),
    ];
    for (pid, workflow) in cases {
        let edi = if (17000..18000).contains(&pid) {
            orders(pid, "9")
        } else {
            utilmd(pid)
        };
        let outcome = dispatch(&edi, workflow, pid).await;
        assert!(
            !matches!(outcome, IngestOutcome::Spawned { .. }),
            "{workflow} accepted PID {pid}, which belongs to a sibling workflow: {outcome:?}"
        );
    }
}
