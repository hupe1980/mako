//! Dispatch-coverage guard for **reply-PID families**.
//!
//! Regression guard for the "registered-but-not-dispatched" bug class: a reply
//! PID (ORDRSP / ORDCHG / IFTSTA / REMADV / COMDIS / QUOTES) is registered in the
//! PID router (so `resolve_workflow` returns its workflow) but the ingest
//! `match pid` arm has no branch for it, so the inbound reply is silently dropped
//! (`Skipped { pid_not_in_* }`).
//!
//! For every reply-range PID each domain module registers, this test dispatches a
//! parseable message of the right type and asserts the outcome is **not**
//! `pid_not_in_*`. The dispatch arms match on the passed PID (not the message
//! content), so a minimal message suffices; `process_not_found` / `no_malo_id` /
//! adapter `Err` all mean "the PID reached its arm" — only `pid_not_in_*` is the bug.
//!
//! Router is built **per module** (a combined all-roles router would panic on the
//! deliberate geli-gas ↔ wim-gas 44022–44024 `register_with_module` conflict).

use std::sync::Arc;

use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const LOC: &str = "51238696780";

/// A parseable message of the reply type implied by `pid`'s range, or `None` if
/// `pid` is not a reply-family PID. All carry a LOC and an `RFF+ON`/`Z13` so both
/// the MaLo and order-ref/reference correlation paths have something to read.
fn reply_message(pid: u32) -> Option<String> {
    let msg = match pid {
        15000..=15999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+QUOTES:D:10A:UN:1.3c'BGM+310+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'UNT+8+1'UNZ+1+1'"
        ),
        19000..=19999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+ORDRSP:D:10A:UN:1.4c'BGM+7+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'UNT+8+1'UNZ+1+1'"
        ),
        21000..=21999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+IFTSTA:D:18A:UN:2.1'BGM+Z03+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'STS+Z21+Z05'UNT+9+1'UNZ+1+1'"
        ),
        29000..=29999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+250101:0000+1'\
UNH+1+COMDIS:D:17A:UN:1.0g'BGM+739+ABL{pid}'RFF+Z13:{pid}'DTM+137:20250101:102'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'AJT+Z01'UNT+8+1'UNZ+1+1'"
        ),
        33000..=33999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+250101:0000+1'\
UNH+1+REMADV:D:05A:UN:2.9f'BGM+239+000{pid}'DTM+137:20250101:102'RFF+Z13:REF'\
NAD+MS+4012345000023::293'CUX+2:EUR:9'MOA+9:100.00:EUR'UNS+D'MOA+9:100.00:EUR'AJT+Z01'UNT+11+1'UNZ+1+1'"
        ),
        39000..=39999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+ORDCHG:D:20B:UN:1.1'BGM+Z51+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'UNS+D'UNT+8+1'UNZ+1+1'"
        ),
        _ => return None,
    };
    Some(msg)
}

async fn make_dispatcher() -> EdifactIngestDispatcher {
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

#[tokio::test]
async fn every_registered_reply_pid_has_a_dispatch_arm() {
    let modules: Vec<Box<dyn EngineModule>> = vec![
        Box::new(mako_gpke::GpkeModule),
        Box::new(mako_wim::WimModule),
        Box::new(mako_geli_gas::GeliGasModule),
        Box::new(mako_wim_gas::WimGasModule),
        Box::new(mako_gabi_gas::GaBiGasModule),
        Box::new(mako_mabis::MabisModule),
        Box::new(mako_redispatch::RedispatchModule),
    ];
    let roles = DeploymentRoles::all();
    let dispatcher = make_dispatcher().await;

    // Collect (pid, workflow) reply-family registrations across all modules, each
    // module into its own router to avoid the deliberate cross-module conflict.
    let mut pairs: Vec<(u32, String)> = Vec::new();
    for m in &modules {
        let mut router = PidRouter::new();
        m.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if reply_message(pid).is_some()
                && let Some(wf) = router.route(pid)
            {
                pairs.push((pid, wf.to_owned()));
            }
        }
        for (pid, _sparte, wf) in router.registered_commodity_entries() {
            if reply_message(pid).is_some() {
                pairs.push((pid, wf.to_owned()));
            }
        }
    }
    assert!(
        !pairs.is_empty(),
        "expected some reply-family PIDs to be registered"
    );

    let mut gaps: Vec<String> = Vec::new();
    for (pid, wf) in pairs {
        let edi = reply_message(pid).expect("checked above");
        let Ok(msg) = edi_energy::parse(edi.as_bytes()) else {
            // A template that fails to parse is a test-fixture issue, not a
            // coverage gap — skip (the arm-coverage assertion is what matters).
            continue;
        };
        if let Ok(IngestOutcome::Skipped { reason, .. }) = dispatcher.dispatch(&msg, &wf, pid).await
            && reason.starts_with("pid_not_in_")
        {
            gaps.push(format!("PID {pid} → {wf} ({reason})"));
        }
    }

    assert!(
        gaps.is_empty(),
        "reply PIDs registered but not dispatched (registered-but-not-dispatched bug):\n  {}",
        gaps.join("\n  ")
    );
}
