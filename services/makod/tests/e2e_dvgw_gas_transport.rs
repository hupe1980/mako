//! The DVGW gas-transport workflows must actually be reachable.
//!
//! They were not. `makod` never called `DvgwPlatform` at all: the ingest
//! dispatcher took `&edi_energy::AnyMessage`, and the GaBi Gas adapters
//! downcast to a DVGW type that an `AnyMessage` can never be. Both workflows
//! were registered against PIDs 70001–70039 and could not receive a single
//! message — a state machine nobody can reach is not a feature.
//!
//! `e2e_dispatch_coverage_guard` cannot cover this: it drives BDEW PIDs from
//! AHB-profile fixtures, and DVGW has no profile layer. So the wiring — sniff,
//! parse, correlate, dispatch — is verified here, from real wire bytes.
//!
//! The fixtures are the specification's own examples (DVGW-Nachrichtenbeschreibung
//! ALOCAT 5.11a §3.2, NOMINT 4.6 §3.2), not messages shaped to suit the parser.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const NB_MP: &str = "9870012345678";
const MGV_MP: &str = "9800505300009";

async fn dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(MGV_MP);
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    )
}

/// An ingest state wired the way `main.rs` wires it, with the DVGW PIDs routed.
async fn ingest_state() -> makod::edifact_api::EdifactApiState {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(MGV_MP);
    let dispatcher = Arc::new(EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    ));
    let mut pid_router = mako_engine::pid_router::PidRouter::new();
    for info in dvgw_edi::catalogue() {
        let workflow = match info.message_type {
            dvgw_edi::DvgwMessageType::Alocat => "gabi-gas-allocation",
            _ => "gabi-gas-nomination",
        };
        pid_router.register(info.pid, workflow);
    }
    makod::edifact_api::EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router,
        mp_id_registry: Arc::new(
            makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
                mp_id: MGV_MP.to_owned(),
                // The DVGW formats are the gas transport layer, so the party
                // receiving them is a Gas one.
                roles: vec!["NB".to_owned()],
                primary: true,
                agency: None,
            }])
            .expect("valid registry"),
        ),
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        max_body_bytes: 1024 * 1024,
        partner_store: None,
        tenant_id: tenant,
        dl_sink: Arc::new(mako_engine::dead_letter::LogDeadLetterSink),
        dispatcher: Some(dispatcher),
        contrl_ack: None,
    }
}

/// ALOCAT 5.11a §3.2, wrapped in the interchange the spec example omits.
fn alocat(pid: u32, gas_day: &str, clearing: &str) -> String {
    format!(
        "UNB+UNOA:3+{NB_MP}:502+{MGV_MP}:502+180101:1200+IC1'\
UNH+123456+ORDRSP:D:07A:UN:5.11a'\
BGM+X1G::332+ALOCAT123456'\
DTM+Z05:0:805'\
DTM+137:201801011200:203'\
DTM+Z01:{gas_day}:719'\
RFF+ANX:{clearing}'\
RFF+Z13:{pid}'\
NAD+MS+{NB_MP}::332'\
NAD+MR+{MGV_MP}::332'\
LIN+1++:Z01::332'\
LOC+Z99'\
DTM+2:{gas_day}:719'\
QTY+Z03:4000:KW1'\
STS+09G::332'\
NAD+ZEU+THE0BFH123456789::332'\
NAD+ZSH+THE0NKH712345678::332'\
UNS+S'\
UNT+17+123456'\
UNZ+1+IC1'"
    )
}

/// NOMINT 4.6 §3.2 / NOMRES 4.7, sharing the business key that pairs them.
fn nomination(carrier: &str, document: &str, pid: u32, gas_day: &str) -> String {
    format!(
        "UNB+UNOA:3+{NB_MP}:502+{MGV_MP}:502+180104:2056+IC2'\
UNH+1+{carrier}:D:07A:UN:DVGW17'\
BGM+{document}::332+NOMINT00052'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:{gas_day}:719'\
RFF+Z13:{pid}'\
NAD+MS+{NB_MP}::332'\
NAD+MR+{MGV_MP}::332'\
LIN+1'\
LOC+Z19+ABCD1234::332'\
DTM+2:{gas_day}:719'\
QTY+Z03:6782:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-2::332'\
UNS+S'\
UNT+16+1'\
UNZ+1+IC2'"
    )
}

/// A winter gas day: 05:00 UTC → 05:00 UTC is the 06:00 CET boundary.
const GAS_DAY: &str = "201801010500201801020500";
const GAS_DAY_2: &str = "201801020500201801030500";

async fn dispatch(edi: &str, workflow: &str, pid: u32) -> IngestOutcome {
    let msg = dvgw_edi::DvgwPlatform::default()
        .parse(edi.as_bytes())
        .unwrap_or_else(|e| panic!("test interchange for {pid} does not parse: {e}"));
    dispatcher()
        .await
        .dispatch_dvgw(&msg, workflow, pid)
        .await
        .unwrap_or_else(|e| panic!("dispatch of {pid} failed: {e}"))
}

// ── The path that did not exist ──────────────────────────────────────────────

/// An ALOCAT reaches the allocation workflow and spawns a process.
#[tokio::test]
async fn an_alocat_reaches_the_allocation_workflow() {
    let outcome = dispatch(
        &alocat(70_001, GAS_DAY, "CLR-2018-001"),
        "gabi-gas-allocation",
        70_001,
    )
    .await;
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "PID 70001 must spawn an allocation process, got {outcome:?}"
    );
}

/// A NOMINT reaches the nomination workflow.
#[tokio::test]
async fn a_nomint_reaches_the_nomination_workflow() {
    let outcome = dispatch(
        &nomination("ORDERS", "01G", 70_030, GAS_DAY),
        "gabi-gas-nomination",
        70_030,
    )
    .await;
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "PID 70030 must spawn a nomination process, got {outcome:?}"
    );
}

/// Every routed Prüfidentifikator must reach an arm.
///
/// The registry registers 70001–70039; a code with no arm is a silent drop, and
/// `dispatch_dvgw` reports it as `pid_not_in_dispatch_table`.
#[tokio::test]
async fn every_registered_dvgw_pid_reaches_a_dispatch_arm() {
    for info in dvgw_edi::catalogue() {
        let (workflow, edi) = match info.message_type {
            dvgw_edi::DvgwMessageType::Alocat => (
                "gabi-gas-allocation",
                alocat(info.pid, GAS_DAY, "CLR-2018-001"),
            ),
            dvgw_edi::DvgwMessageType::Nomint => (
                "gabi-gas-nomination",
                nomination("ORDERS", "01G", info.pid, GAS_DAY),
            ),
            dvgw_edi::DvgwMessageType::Nomres => (
                "gabi-gas-nomination",
                nomination("ORDRSP", "08G", info.pid, GAS_DAY),
            ),
        };
        let outcome = dispatch(&edi, workflow, info.pid).await;
        assert!(
            !matches!(
                &outcome,
                IngestOutcome::Skipped {
                    reason: "pid_not_in_dispatch_table" | "no_correlation_key",
                    ..
                }
            ),
            "PID {} ({}) is registered but reached no dispatch arm: {outcome:?}",
            info.pid,
            info.description
        );
    }
}

// ── Correlation ──────────────────────────────────────────────────────────────

/// Two gas days of the same object are two processes; a correction to one gas
/// day resumes that day's.
///
/// The published `ZO-T3` tuple is (Bilanzkreis, Netzkonto, Zeitreihentyp) and
/// stops there — it identifies an *object*, so it is the same for every day of
/// the month. `AllocationState::Recorded` holds one gas day's record and one
/// §6.4 deadline, so a key without the gas day would let the second day
/// overwrite both of the first's.
#[tokio::test]
async fn each_gas_day_of_an_object_is_its_own_process() {
    let d = dispatcher().await;
    let parse = |edi: &str| {
        dvgw_edi::DvgwPlatform::default()
            .parse(edi.as_bytes())
            .expect("parses")
    };

    let day_one = parse(&alocat(70_001, GAS_DAY, "CLR-A"));
    let day_two = parse(&alocat(70_001, GAS_DAY_2, "CLR-B"));
    // Correction to day one: same document code, same gas day.
    let day_one_correction = parse(&alocat(70_006, GAS_DAY, "CLR-A"));

    // The published tuple is identical for all three — as specified.
    assert_eq!(day_one.correlation_key(), day_two.correlation_key());
    // The process key separates the gas days, and keeps the correction with its
    // own day.
    assert_ne!(day_one.process_key(), day_two.process_key());
    assert_eq!(day_one.process_key(), day_one_correction.process_key());

    assert!(matches!(
        d.dispatch_dvgw(&day_one, "gabi-gas-allocation", 70_001)
            .await
            .expect("day one"),
        IngestOutcome::Spawned { .. }
    ));
    assert!(
        matches!(
            d.dispatch_dvgw(&day_two, "gabi-gas-allocation", 70_001)
                .await
                .expect("day two"),
            IngestOutcome::Spawned { .. }
        ),
        "a different gas day must not resume the first day's process"
    );
    assert!(
        matches!(
            d.dispatch_dvgw(&day_one_correction, "gabi-gas-allocation", 70_006)
                .await
                .expect("correction"),
            IngestOutcome::Dispatched { .. }
        ),
        "a correction must resume the gas day it corrects"
    );
}

/// A clearing message assigns to its Clearingfall, not to the allocation stream.
#[tokio::test]
async fn a_clearing_message_keys_on_its_clearingnummer() {
    let stream = dvgw_edi::DvgwPlatform::default()
        .parse(alocat(70_001, GAS_DAY, "CLR-A").as_bytes())
        .expect("parses");
    let clearing = dvgw_edi::DvgwPlatform::default()
        .parse(alocat(70_008, GAS_DAY, "CLR-A").as_bytes())
        .expect("parses");

    let stream_key = stream.correlation_key().unwrap();
    let clearing_key = clearing.correlation_key().unwrap();
    assert_eq!(stream_key.zuordnung, dvgw_edi::Zuordnung::ZoT3);
    assert_eq!(clearing_key.zuordnung, dvgw_edi::Zuordnung::ZgT1);
    assert_ne!(
        stream_key.to_string(),
        clearing_key.to_string(),
        "a clearing correction must not merge into the stream it corrects"
    );
}

/// A Prüfidentifikator with no published Zuordnung is skipped loudly rather than
/// attached to a guessed key.
#[tokio::test]
async fn an_unassigned_pid_is_refused_rather_than_guessed() {
    let outcome = dispatch(
        &alocat(70_500, GAS_DAY, "CLR-A"),
        "gabi-gas-allocation",
        70_500,
    )
    .await;
    assert!(
        matches!(
            outcome,
            IngestOutcome::Skipped {
                reason: "no_correlation_key",
                ..
            }
        ),
        "70500 has no published Zuordnung and must not reach a process: {outcome:?}"
    );
}

// ── Family separation ────────────────────────────────────────────────────────

/// A BDEW interchange must not be claimed by the DVGW path, and vice versa.
///
/// This is the whole reason the sniff exists: `ORDRSP` is a real BDEW message
/// type *and* the carrier for ALOCAT, so keying on `UNH` would route both the
/// same way.
#[test]
fn the_two_families_are_separated_by_the_document_code() {
    let dvgw = alocat(70_001, GAS_DAY, "CLR-A");
    assert!(
        dvgw_edi::sniff(dvgw.as_bytes()).is_some(),
        "an ALOCAT must be claimed by the DVGW path"
    );

    // A BDEW ORDRSP on the same carrier.
    let bdew = format!(
        "UNB+UNOC:3+{NB_MP}:500+{MGV_MP}:500+260804:1045+REF1'\
UNH+1+ORDRSP:D:07B:UN:1.1c'BGM+231+19110'DTM+137:20260804:102'\
NAD+MS+{NB_MP}::293'NAD+MR+{MGV_MP}::293'UNT+6+1'UNZ+1+REF1'"
    );
    assert!(
        dvgw_edi::sniff(bdew.as_bytes()).is_none(),
        "a BDEW ORDRSP must fall through to the BDEW path"
    );

    // The hazard is subtler than a mis-route. The BDEW parser accepts these
    // bytes *and* reads 70001 out of `RFF+Z13`, because that is exactly where it
    // looks for a Prüfidentifikator and 70001 is a plausible five-digit code. So
    // the message routes to the right workflow and then arrives as the wrong
    // type: an `AnyMessage::Ordrsp` with no document code, no gas day, and no
    // positions. The sniff is what makes the *content* usable, not what makes
    // the routing correct.
    let as_bdew = edi_energy::parse(dvgw.as_bytes()).expect("valid EDIFACT either way");
    assert_eq!(
        edi_energy::EdiEnergyMessage::detect_pruefidentifikator(&as_bdew)
            .ok()
            .map(|p| p.as_u32()),
        Some(70_001),
        "the BDEW parser reads the same code — which is why the family cannot be \
         told apart by the Prüfidentifikator either"
    );
    assert!(
        matches!(
            as_bdew.try_message_type(),
            Some(edi_energy::MessageType::Ordrsp)
        ),
        "…and calls it an ORDRSP, which is the carrier rather than the message"
    );

    // Parsed as DVGW, the same bytes yield what the workflow actually needs.
    let as_dvgw = dvgw_edi::DvgwPlatform::default()
        .parse(dvgw.as_bytes())
        .expect("parses as DVGW");
    assert_eq!(as_dvgw.document.code(), "X1G");
    assert!(as_dvgw.gas_day().is_some());
    assert_eq!(as_dvgw.quantities().count(), 1);
}

/// The gas day comes from `DTM+Z01`, and a message without a usable one is
/// refused rather than booked against today.
#[tokio::test]
async fn a_message_without_a_usable_gas_day_is_refused() {
    // `DTM+Z01` present but not a gas day: one hour instead of ~24.
    let one_hour = alocat(70_001, "201801010500201801010600", "CLR-A");
    let msg = dvgw_edi::DvgwPlatform::default()
        .parse(one_hour.as_bytes())
        .expect("parses — the defect is semantic, not syntactic");
    let err = dispatcher()
        .await
        .dispatch_dvgw(&msg, "gabi-gas-allocation", 70_001)
        .await
        .expect_err("a one-hour period is not a gas day");
    assert!(
        err.to_string().contains("Gültigkeitszeitraum"),
        "the refusal must name what is wrong: {err}"
    );
}

// ── Quantities ───────────────────────────────────────────────────────────────

/// A NOMRES that confirms less than was nominated must reach the workflow as a
/// curtailment, from wire bytes.
///
/// Both halves have to be right for this to work: the energy must be integrated
/// over the period (a `QTY` is a rate), and the NOMRES must be scoped to the
/// recipient's own side of the match — `IMD` `18G` is the counterparty's mirror,
/// and adding it would turn a shortfall into a surplus.
#[tokio::test]
async fn a_curtailed_nomination_is_recorded_as_curtailed() {
    let d = dispatcher().await;
    let parse = |edi: &str| {
        dvgw_edi::DvgwPlatform::default()
            .parse(edi.as_bytes())
            .expect("parses")
    };

    // Nominate 100 kWh/h for the whole gas day = 2 400 kWh.
    let nomint = parse(&nomination_of(
        "ORDERS", "01G", 70_030, GAS_DAY, "100", None,
    ));
    assert!(matches!(
        d.dispatch_dvgw(&nomint, "gabi-gas-nomination", 70_030)
            .await
            .expect("nomint"),
        IngestOutcome::Spawned { .. }
    ));

    // The MGV confirms 75 kWh/h — 1 800 kWh — and states it as a Bestätigung.
    // It also mirrors the counterparty's 100 under `18G`, which must be ignored.
    let nomres = parse(&nomres_two_sided(70_036, GAS_DAY, "75", "100"));
    assert!(matches!(
        d.dispatch_dvgw(&nomres, "gabi-gas-nomination", 70_036)
            .await
            .expect("nomres"),
        IngestOutcome::Dispatched { .. }
    ));

    // The recipient's own side integrates to 1 800 kWh, not 4 200.
    assert_eq!(
        nomres
            .single_energy_kwh(|item| matches!(
                item.description_code(),
                Some(dvgw_edi::model::imd::NOMINIERT) | None
            ))
            .map(|d| d.to_string()),
        Some("1800".to_owned()),
        "the counterparty's mirrored quantity must not be added in"
    );
}

/// The allocated quantity is the integral, and it reaches the command.
#[tokio::test]
async fn the_allocated_quantity_is_the_integral_of_the_rate() {
    let msg = dvgw_edi::DvgwPlatform::default()
        .parse(alocat(70_001, GAS_DAY, "CLR-A").as_bytes())
        .expect("parses");
    // 4 000 kWh/h across a 24-hour gas day.
    assert_eq!(
        msg.single_energy_kwh(|_| true).map(|d| d.to_string()),
        Some("96000".to_owned())
    );

    let outcome = dispatcher()
        .await
        .dispatch_dvgw(&msg, "gabi-gas-allocation", 70_001)
        .await
        .expect("dispatch");
    assert!(matches!(outcome, IngestOutcome::Spawned { .. }));
}

/// A message mixing entry and exit yields no single total.
#[test]
fn a_two_direction_message_has_no_single_total() {
    let both = nomination_two_directions(70_031, GAS_DAY);
    let msg = dvgw_edi::DvgwPlatform::default()
        .parse(both.as_bytes())
        .expect("parses");
    assert_eq!(msg.energy_by_qualifier().len(), 2);
    assert_eq!(
        msg.single_energy_kwh(|_| true),
        None,
        "one scalar across Z02 and Z03 would be a net position, not a total"
    );
}

// ── Fixture builders ─────────────────────────────────────────────────────────

/// A one-position nomination at `rate` kWh/h, optionally labelled with an `IMD`.
fn nomination_of(
    carrier: &str,
    document: &str,
    pid: u32,
    gas_day: &str,
    rate: &str,
    imd: Option<&str>,
) -> String {
    let imd_seg = imd
        .map(|c| format!("IMD++05G+{c}::332'"))
        .unwrap_or_default();
    format!(
        "UNB+UNOA:3+{NB_MP}:502+{MGV_MP}:502+180104:2056+IC5'\
UNH+1+{carrier}:D:07A:UN:DVGW17'\
BGM+{document}::332+NOMINT00052'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:{gas_day}:719'\
RFF+Z13:{pid}'\
NAD+MS+{NB_MP}::332'\
NAD+MR+{MGV_MP}::332'\
LIN+1'\
{imd_seg}\
LOC+Z19+ABCD1234::332'\
DTM+2:{gas_day}:719'\
QTY+Z02:{rate}:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-2::332'\
UNS+S'\
UNT+99+1'\
UNZ+1+IC5'"
    )
}

/// A NOMRES stating the recipient's confirmed side (`17G`) and the
/// counterparty's mirror (`18G`).
fn nomres_two_sided(pid: u32, gas_day: &str, own_rate: &str, other_rate: &str) -> String {
    format!(
        "UNB+UNOA:3+{MGV_MP}:502+{NB_MP}:502+180104:2056+IC6'\
UNH+1+ORDRSP:D:07A:UN:DVGW17'\
BGM+08G::332+NOMRES00052'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:{gas_day}:719'\
RFF+Z13:{pid}'\
NAD+MS+{MGV_MP}::332'\
NAD+MR+{NB_MP}::332'\
LIN+1'\
IMD++05G+17G::332'\
LOC+Z19+ABCD1234::332'\
DTM+2:{gas_day}:719'\
QTY+Z02:{own_rate}:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-2::332'\
LIN+2'\
IMD++05G+18G::332'\
LOC+Z19+ABCD1234::332'\
DTM+2:{gas_day}:719'\
QTY+Z02:{other_rate}:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-2::332'\
UNS+S'\
UNT+99+1'\
UNZ+1+IC6'"
    )
}

/// A VHP nomination stating a purchase (`Z02`) and a sale (`Z03`).
fn nomination_two_directions(pid: u32, gas_day: &str) -> String {
    format!(
        "UNB+UNOA:3+{NB_MP}:502+{MGV_MP}:502+180104:2056+IC7'\
UNH+1+ORDERS:D:07A:UN:DVGW17'\
BGM+55G::332+NOMINT00099'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:{gas_day}:719'\
RFF+Z13:{pid}'\
NAD+MS+{NB_MP}::332'\
NAD+MR+{MGV_MP}::332'\
LIN+1'\
LOC+Z19+ABCD1234::332'\
DTM+2:{gas_day}:719'\
QTY+Z02:100:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-2::332'\
LIN+2'\
LOC+Z19+ABCD1234::332'\
DTM+2:{gas_day}:719'\
QTY+Z03:20:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-3::332'\
UNS+S'\
UNT+99+1'\
UNZ+1+IC7'"
    )
}

/// A BDEW interchange must reach the BDEW path untouched.
///
/// The DVGW sniff runs first on every request, so a false positive would divert
/// a UTILMD into a gas-transport parser.
#[test]
fn a_bdew_interchange_is_not_claimed_by_the_dvgw_path() {
    // One representative of each carrier a DVGW format also uses, plus a type
    // that shares nothing.
    for wire in [
        // ORDERS — NOMINT's carrier.
        format!(
            "UNB+UNOC:3+{NB_MP}:500+{MGV_MP}:500+260804:1045+R1'\
UNH+1+ORDERS:D:01B:UN:1.1b'BGM+Z01+17115+9'DTM+137:20260804:102'\
NAD+MS+{NB_MP}::293'NAD+MR+{MGV_MP}::293'UNT+6+1'UNZ+1+R1'"
        ),
        // ORDRSP — ALOCAT's and NOMRES's carrier.
        format!(
            "UNB+UNOC:3+{NB_MP}:500+{MGV_MP}:500+260804:1045+R2'\
UNH+1+ORDRSP:D:07B:UN:1.1c'BGM+231+19110'DTM+137:20260804:102'\
NAD+MS+{NB_MP}::293'NAD+MR+{MGV_MP}::293'UNT+6+1'UNZ+1+R2'"
        ),
        // UTILMD — no overlap at all.
        format!(
            "UNB+UNOC:3+{NB_MP}:500+{MGV_MP}:500+260804:1045+R3'\
UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E01+55001+9'DTM+137:20260804:102'\
NAD+MS+{NB_MP}::293'NAD+MR+{MGV_MP}::293'UNT+6+1'UNZ+1+R3'"
        ),
    ] {
        assert_eq!(
            dvgw_edi::sniff(wire.as_bytes()),
            None,
            "a BDEW interchange must fall through to the BDEW path: {wire}"
        );
    }
}

// ── CONTRL obligation ────────────────────────────────────────────────────────

/// The report must carry what the CONTRL Empfangsbestätigung needs.
///
/// CONTRL AHB 1.0 §2.3.1 binds the receiver of every inbound *Gas* interchange to
/// a CONTRL within six wall-clock hours, and a DVGW interchange is Gas by
/// definition. The AS4 `eb:Receipt` is a protocol acknowledgement and does not
/// discharge it, so the transport needs the sender, the recipient and the UNB
/// control reference — without re-parsing the interchange to get them.
#[tokio::test]
async fn the_report_carries_what_the_contrl_acknowledgement_needs() {
    let state = ingest_state().await;
    let wire = alocat(70_001, GAS_DAY, "CLR-A");

    let report = makod::dvgw_ingest::try_ingest(&state, wire.as_bytes())
        .await
        .expect("the sniff must claim a DVGW interchange");

    assert_eq!(report.sender_mp_id.as_deref(), Some(NB_MP), "NAD+MS");
    assert_eq!(report.recipient_mp_id, MGV_MP, "UNB DE 0010");
    assert_eq!(report.interchange_ref, "IC1", "UNB DE 0020");
}

/// A BDEW interchange must not be claimed, so its CONTRL stays on the BDEW path.
#[tokio::test]
async fn a_bdew_interchange_produces_no_dvgw_report() {
    let state = ingest_state().await;
    let bdew = format!(
        "UNB+UNOC:3+{NB_MP}:500+{MGV_MP}:500+260804:1045+R9'\
UNH+1+ORDRSP:D:07B:UN:1.1c'BGM+231+19110'DTM+137:20260804:102'\
NAD+MS+{NB_MP}::293'NAD+MR+{MGV_MP}::293'UNT+6+1'UNZ+1+R9'"
    );
    assert!(
        makod::dvgw_ingest::try_ingest(&state, bdew.as_bytes())
            .await
            .is_none()
    );
}

/// Accepted/rejected follow the same rule the BDEW path documents: only a parse
/// failure or a missing Prüfidentifikator is a rejection.
#[tokio::test]
async fn an_unroutable_message_is_still_accepted_and_dead_lettered() {
    let state = ingest_state().await;
    // In range, so it parses and carries a PID — but no workflow claims it.
    let wire = alocat(70_500, GAS_DAY, "CLR-A");
    let report = makod::dvgw_ingest::try_ingest(&state, wire.as_bytes())
        .await
        .expect("DVGW");
    assert_eq!(
        report.accepted(),
        1,
        "a routable-shaped message is accepted"
    );
    assert_eq!(report.rejected(), 0);

    // No Prüfidentifikator at all is a rejection: nothing can be routed or
    // acknowledged.
    let no_pid = wire.replace("RFF+Z13:70500'", "");
    let report = makod::dvgw_ingest::try_ingest(&state, no_pid.as_bytes())
        .await
        .expect("DVGW");
    assert_eq!(report.rejected(), 1);
    assert_eq!(report.accepted(), 0);
}

// ── The Matching-Benachrichtigung must not close the nomination ──────────────

/// A `07G`/`19G` Matching-Benachrichtigung states no acceptance.
///
/// It reports the state of the match. Feeding it to the workflow as an answer
/// drove the process to a terminal `Rejected`, and the Bestätigung that followed
/// then failed with `invalid_state` — leaving a nomination the counterparty had
/// confirmed permanently on file as rejected.
#[tokio::test]
async fn a_matching_notification_leaves_the_nomination_open_for_its_confirmation() {
    let d = dispatcher().await;
    let parse = |edi: &str| {
        dvgw_edi::DvgwPlatform::default()
            .parse(edi.as_bytes())
            .expect("parses")
    };

    let nomint = parse(&nomination_of(
        "ORDERS", "01G", 70_030, GAS_DAY, "100", None,
    ));
    assert!(matches!(
        d.dispatch_dvgw(&nomint, "gabi-gas-nomination", 70_030)
            .await
            .expect("nomint"),
        IngestOutcome::Spawned { .. }
    ));

    // The Matching-Benachrichtigung arrives first, as it does in practice.
    let matching = parse(&nomination_of(
        "ORDRSP",
        "07G",
        70_035,
        GAS_DAY,
        "100",
        Some("17G"),
    ));
    assert!(
        matches!(
            d.dispatch_dvgw(&matching, "gabi-gas-nomination", 70_035)
                .await
                .expect("matching notification"),
            IngestOutcome::Skipped {
                reason: "matching_notification_states_no_acceptance",
                ..
            }
        ),
        "a matching notification must not decide the nomination"
    );

    // The Bestätigung must still land.
    let bestaetigung = parse(&nomination_of(
        "ORDRSP",
        "08G",
        70_036,
        GAS_DAY,
        "100",
        Some("17G"),
    ));
    assert!(
        matches!(
            d.dispatch_dvgw(&bestaetigung, "gabi-gas-nomination", 70_036)
                .await
                .expect("bestätigung"),
            IngestOutcome::Dispatched { .. }
        ),
        "the nomination must still be open for its Bestätigung"
    );
}

/// `17G` and `16G` name the same quantity twice; only one may be counted.
///
/// A NOMRES may state the nominated figure and the matched one for the same
/// position. Selecting both sums them, which is the same double-count as
/// including the counterparty's `18G` — a curtailment then reads as an
/// over-confirmation.
#[test]
fn a_nomres_labelling_both_its_own_views_is_not_double_counted() {
    let wire = format!(
        "UNB+UNOA:3+{MGV_MP}:502+{NB_MP}:502+180104:2056+IC8'\
UNH+1+ORDRSP:D:07A:UN:DVGW17'\
BGM+08G::332+NOMRES1'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:{GAS_DAY}:719'\
RFF+Z13:70036'\
NAD+MS+{MGV_MP}::332'\
NAD+MR+{NB_MP}::332'\
LIN+1'IMD++05G+17G::332'LOC+Z19+P::332'DTM+2:{GAS_DAY}:719'QTY+Z02:100:KW1'\
NAD+ZEU+BK1::332'NAD+ZES+BK2::332'\
LIN+2'IMD++05G+16G::332'LOC+Z19+P::332'DTM+2:{GAS_DAY}:719'QTY+Z02:75:KW1'\
NAD+ZEU+BK1::332'NAD+ZES+BK2::332'\
LIN+3'IMD++05G+18G::332'LOC+Z19+P::332'DTM+2:{GAS_DAY}:719'QTY+Z02:100:KW1'\
NAD+ZEU+BK1::332'NAD+ZES+BK2::332'\
UNS+S'UNT+99+1'UNZ+1+IC8'"
    );
    let msg = dvgw_edi::DvgwPlatform::default()
        .parse(wire.as_bytes())
        .expect("parses");

    let matched =
        |item: &dvgw_edi::LineItem| item.description_code() == Some(dvgw_edi::model::imd::GEMATCHT);
    // The matched quantity alone: 75 kWh/h × 24 h.
    assert_eq!(
        msg.single_energy_kwh(matched).map(|d| d.to_string()),
        Some("1800".to_owned())
    );
    // Every position together would be 100 + 75 + 100 = 275 kWh/h × 24 h —
    // nearly four times the figure that will actually flow.
    assert_eq!(
        msg.single_energy_kwh(|_| true).map(|d| d.to_string()),
        Some("6600".to_owned()),
        "this is the number the adapter must not use"
    );
}

// ── Test-indicator guard ─────────────────────────────────────────────────────

/// A DVGW interchange flagged `UNB DE 0035 = 1` must be refused.
///
/// # Why this is a test
///
/// Allgemeine Festlegungen V6.1d §3 forbids processing a test interchange as
/// production, and both BDEW doors — `POST /edifact` and the AS4 handler —
/// already refuse one and record a `TestMessage` dead letter. The DVGW door did
/// not read the field at all, and DVGW rides the same `UNB` envelope: a
/// counterparty's test ALOCAT allocated real gas quantities against a real gas
/// day, and a test NOMINT spawned a nomination process with a live NOMRES
/// deadline.
#[tokio::test]
async fn a_dvgw_test_interchange_is_refused() {
    let state = ingest_state().await;
    let wire = alocat(70_001, GAS_DAY, "CLR-T");
    // DE 0035 is element 10 of UNB and the fixture ends its UNB at DE 0020
    // (element 4), so six separators carry the flag into position.
    let test_wire = wire.replacen("+IC1'", "+IC1++++++1'", 1);
    assert_ne!(
        test_wire, wire,
        "the fixture's UNB must have been rewritten"
    );

    let report = makod::dvgw_ingest::try_ingest(&state, test_wire.as_bytes())
        .await
        .expect("the sniff must still claim a DVGW interchange");

    assert_eq!(
        report.accepted(),
        0,
        "a test interchange must not be accepted"
    );
    assert_eq!(report.rejected(), 1);
    assert!(
        report.messages[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("DE0035")),
        "the rejection must name the field: {:?}",
        report.messages[0].error
    );
}

/// The same interchange without the flag is processed, so the test above is
/// about the flag and not about the rewritten `UNB` failing to parse.
#[tokio::test]
async fn the_same_interchange_without_the_test_flag_is_processed() {
    let state = ingest_state().await;
    let wire = alocat(70_001, GAS_DAY, "CLR-T");
    let report = makod::dvgw_ingest::try_ingest(&state, wire.as_bytes())
        .await
        .expect("DVGW");
    assert_eq!(report.accepted(), 1);
    assert_eq!(report.rejected(), 0);
}
