//! End-to-end test: LF → NB GPKE Neuanlage (PID 55600 / 55601).
//!
//! The Lieferant (LF) triggers a new Marktlokation registration; the
//! Netzbetreiber (NB) receives the UTILMD 55600/55601 Anfrage, validates it,
//! and confirms acceptance (55602/55603) or rejection (55604/55605).
//!
//! # Protocol trace (PID 55600 accept path)
//!
//! ```text
//!   LF ERP (wire fixture)                      NB ERP (MockNb)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD 55600 ────►
//!                                               receive_anmeldung(wire)
//!                                                 → adapter: ReceiveAnmeldung
//!                                                 → state: ValidationPassed
//!                                               send_antwort(accepted=true)
//!                                                 → state: AntwortGesendet
//!                                               aktivieren()
//!                                                 → state: Aktiviert
//!   ──────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 55600**: Neue verbindliche Marktlokation (NB → LF response: 55602/55604)
//! - **PID 55601**: Neue erzeugende Marktlokation (NB → LF response: 55603/55605)
//! - **Deadline**: the Neuanlage answer window, **00:00 Uhr des 61. Werktags
//!   nach dem ÜT** (BK6-24-174 GPKE Teil 2 Kap. 2.2.2) — `E_0608` makes the
//!   identification of a newly commissioned MaLo a daily re-check for 60 WT
//! - **Regulatory basis**: BNetzA BK6-24-174 Anlage 1b Kap. 2.2 (Neuanlage)
//!
//! AHB validation is bypassed for inbound `ReceiveAnmeldung` because the
//! hand-crafted fixture does not satisfy all S2.1 profile rules.

use mako_engine::process::RunOptions;
use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::MessageRef,
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{GpkeNeuanlageWorkflow, NeuanlageCommand, NeuanlageState};
use makod::adapters::gpke_neuanlage_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const LF_ID: &str = "4012345000023"; // Lieferant (new-MaLo requester)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Wire fixtures ─────────────────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD S2.1 messages.  The BGM+ qualifier is E01 for all
// UTILMD variants; the Prüfidentifikator is carried in BGM+E01:::+00055600::
// Note: process date (DTM+92) is embedded in the transaction group.

/// PID 55600: Neue verbrauchende Marktlokation (Anmeldung von LF an NB).
const UTILMD_55600_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:500+250115:0800+NANLG-2025-001'\
UNH+MSG-NANLG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055600::+9'\
DTM+137:202501150800?+00:303'\
RFF+Z13:NANLG-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+24+VORGANG-0001'\
LOC+Z16+51238696781'\
DTM+92:202503312200?+00:303'\
UNT+10+MSG-NANLG-001'\
UNZ+1+NANLG-2025-001'";

/// PID 55601: Neue erzeugende Marktlokation (Anmeldung von LF an NB).
const UTILMD_55601_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:500+250115:0800+NANLG-2025-002'\
UNH+MSG-NANLG-002+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055601::+9'\
DTM+137:202501150800?+00:303'\
RFF+Z13:NANLG-REF-002'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+24+VORGANG-0001'\
LOC+Z16+51238696781'\
DTM+92:202503312200?+00:303'\
UNT+10+MSG-NANLG-002'\
UNZ+1+NANLG-2025-002'";

// ── Mock NB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber ERP** receiving and acting on a Neuanlage
/// request.
///
/// Owns a single `GpkeNeuanlageWorkflow` process backed by an in-memory store.
struct MockNb {
    process: Process<GpkeNeuanlageWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("gpke-neuanlage", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive LF's UTILMD 55600/55601 wire bytes, adapt,
    /// and execute.
    ///
    /// `validation_passed` is forced to `true` so the minimal wire fixture
    /// advances to `ValidationPassed` without requiring full S2.1 compliance.
    async fn receive_anmeldung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse LF UTILMD Neuanlage wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = gpke_neuanlage_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt UTILMD Neuanlage to NeuanlageCommand");

        // Override validation_passed to bypass AHB profile rules for
        // this minimal fixture.
        let cmd = match cmd {
            NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                ..
            } => {
                assert!(
                    pid.as_u32() == 55600 || pid.as_u32() == 55601,
                    "adapter must extract PID 55600 or 55601 from wire; got: {}",
                    pid.as_u32()
                );
                assert_eq!(
                    sender.as_str(),
                    LF_ID,
                    "adapter must extract sender GLN (LF) from NAD+MS"
                );
                assert_eq!(
                    location_id.as_str(),
                    MALO_ID,
                    "adapter must extract MaLo from IDE+24"
                );
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref"
                );
                NeuanlageCommand::ReceiveAnmeldung {
                    pid,
                    sender,
                    receiver,
                    location_id,
                    document_date,
                    process_date,
                    message_ref,
                    validation_passed: true,
                    validation_errors: vec![],
                    vorgang: Box::default(),
                    produktpaket_id: None,
                }
            }
            _ => panic!("expected NeuanlageCommand::ReceiveAnmeldung"),
        };

        self.process
            .run(cmd, RunOptions::new())
            .await
            .expect("NB: execute ReceiveAnmeldung");
    }

    /// ERP action: send the response (accept or reject the Neuanlage request).
    async fn send_antwort(&self, accepted: bool, reason: Option<&str>) {
        self.process
            .run(
                NeuanlageCommand::SendAntwort {
                    antwort: neuanlage_antwort(accepted, reason),
                    marktlokation: None,
                    tranche: None,
                },
                RunOptions::new(),
            )
            .await
            .expect("NB: execute SendAntwort");
    }

    /// ERP action: activate the new Marktlokation in the grid.
    async fn aktivieren(&self) {
        self.process
            .run(NeuanlageCommand::Aktivieren, RunOptions::new())
            .await
            .expect("NB: execute Aktivieren");
    }

    async fn state(&self) -> NeuanlageState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Neuanlage PID 55600 — accept path → Aktiviert.
///
/// NB receives 55600, confirms acceptance (→ 55602 outbound), and activates
/// the new verbindliche Marktlokation.
#[tokio::test]
async fn e2e_neuanlage_55600_accepted_and_activated() {
    let nb = MockNb::new();

    // ── NB: receive 55600 Anfrage ─────────────────────────────────────────────
    nb.receive_anmeldung(UTILMD_55600_BYTES).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveAnmeldung"
    );

    // ── NB: accept the request (→ 55602 Bestätigung) ─────────────────────────
    nb.send_antwort(true, None).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::AntwortGesendet { .. }),
        "NB must be AntwortGesendet after SendAntwort(accepted)"
    );

    // ── NB: activate the new MaLo in the grid ────────────────────────────────
    nb.aktivieren().await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Aktiviert(_)),
        "NB must be Aktiviert after Aktivieren; got: {final_state:?}"
    );
    if let NeuanlageState::Aktiviert(data) = final_state {
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.sender.as_str(), LF_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55600,
            "persisted data must carry PID 55600"
        );
    }
}

/// Neuanlage PID 55601 — accept path → Aktiviert.
///
/// Same flow as above but for neue erzeugende MaLo (PID 55601).
#[tokio::test]
async fn e2e_neuanlage_55601_accepted_and_activated() {
    let nb = MockNb::new();

    nb.receive_anmeldung(UTILMD_55601_BYTES).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveAnmeldung (55601)"
    );

    nb.send_antwort(true, None).await;
    nb.aktivieren().await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Aktiviert(_)),
        "NB must be Aktiviert after Aktivieren (55601); got: {final_state:?}"
    );
    if let NeuanlageState::Aktiviert(data) = final_state {
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55601,
            "persisted data must carry PID 55601"
        );
    }
}

/// Neuanlage PID 55600 — rejection path → Rejected.
///
/// NB receives 55600 but denies the request (→ 55604 Ablehnung outbound).
#[tokio::test]
async fn e2e_neuanlage_55600_rejected() {
    let nb = MockNb::new();

    nb.receive_anmeldung(UTILMD_55600_BYTES).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::ValidationPassed(_)),
        "NB must be ValidationPassed before rejection"
    );

    // NB rejects: duplicate MaLo, grid capacity exceeded, etc.
    nb.send_antwort(false, Some("MaLo already registered in grid"))
        .await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Rejected { .. }),
        "NB must be Rejected after SendAntwort(rejected); got: {final_state:?}"
    );
}

/// Neuanlage — validation failure path: AHB check fails → Rejected.
///
/// A UTILMD with a known-bad wire (non-trivial validation failure) is
/// submitted without bypassing validation; the process should immediately
/// advance to `Rejected` without waiting for SendAntwort.
#[tokio::test]
async fn e2e_neuanlage_validation_failure_rejects() {
    use mako_engine::types::{MaLo, MarktpartnerCode, Pruefidentifikator};

    let nb = MockNb::new();

    // Inject ReceiveAnmeldung with validation_passed = false directly
    // (bypasses wire parsing to keep the fixture minimal).
    let cmd = NeuanlageCommand::ReceiveAnmeldung {
        pid: Pruefidentifikator::new(55600).unwrap(),
        sender: MarktpartnerCode::new(LF_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250401".to_owned(),
        message_ref: MessageRef::new("MSG-BAD-001"),
        validation_passed: false,
        validation_errors: vec!["BGM qualifier missing".to_owned()],
        vorgang: Box::default(),
        produktpaket_id: None,
    };

    nb.process
        .run(cmd, RunOptions::new())
        .await
        .expect("execute validation failure");

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Rejected { .. }),
        "NB must be Rejected after validation failure; got: {final_state:?}"
    );
}

/// The NB's `E_0608` answer: `A09` for a verbrauchende Bestätigung, `A03`
/// („Keine Neuanlage, falscher Anwendungsfall") for a refusal. `SG4 STS+E01` is
/// Muss on every Antwortnachricht, and the published Cluster picks the PID.
fn neuanlage_antwort(accepted: bool, reason: Option<&str>) -> mako_gpke::LfAntwort {
    let mut a = if accepted {
        mako_gpke::LfAntwort::zustimmung("A09", "E_0608")
    } else {
        mako_gpke::LfAntwort::ablehnung("A03", "E_0608")
    };
    a.bemerkung = reason.map(ToOwned::to_owned);
    a
}

// ── The door: `gpke.neuanlage.bestaetigen` ─────────────────────────────────────

/// An NB deployment holding the process an inbound Anmeldung neue
/// Marktlokation `pid` opened, and the door that answers it.
struct NeuanlageDoor {
    store: std::sync::Arc<mako_engine::store_slatedb::SlateDbStore>,
    state: makod::commands_api::CommandsApiState,
    /// The process the Anmeldung opened — it names no Marktlokation, so the
    /// answer finds it by this.
    process_id: String,
}

/// The Marktlokations-ID the NB identified for the new Marktlokation.
const IDENTIFIZIERT: &str = "51238696012";

impl NeuanlageDoor {
    async fn after(pid: u32) -> Self {
        use edi_energy::profile::SkeletonParties;
        let profile = edi_energy::ReleaseRegistry::global()
            .profile_for_date_and_track(
                edi_energy::MessageType::Utilmd,
                mako_fristen::heute(),
                edi_energy::ReleaseTrack::Strom,
            )
            .expect("the UTILMD Strom profile in force");
        let anmeldung = profile
            .skeleton_interchange(
                profile.anwendungsfall(pid).expect("the Anmeldung's column"),
                &SkeletonParties {
                    sender: LF_ID.to_owned(),
                    receiver: NB_ID.to_owned(),
                },
            )
            .expect("the skeleton");
        Self::receiving(&anmeldung).await
    }

    /// The NB deployment after the Anmeldung `anmeldung` arrived.
    async fn receiving(anmeldung: &[u8]) -> Self {
        use mako_engine::store_slatedb::SlateDbStore;
        use std::sync::Arc;
        let store = Arc::new(SlateDbStore::open_in_memory().await.expect("store"));
        let tenant_id = TenantId::from_party_id(NB_ID);
        let msg = edi_energy::parse(anmeldung).expect("the Anmeldung parses");
        let pid = msg
            .detect_pruefidentifikator()
            .expect("the Anmeldung names its PID")
            .as_u32();
        let outcome = makod::ingest_dispatcher::EdifactIngestDispatcher::new(
            Arc::clone(&store),
            store.as_snapshot_store(),
            100,
            tenant_id,
        )
        .dispatch(&msg, "gpke-neuanlage", pid, time::OffsetDateTime::now_utc())
        .await
        .expect("the Anmeldung dispatches");
        let makod::ingest_dispatcher::IngestOutcome::Spawned { process_id, .. } = outcome else {
            panic!("the Anmeldung spawns its process: {outcome:?}");
        };
        let state = makod::commands_api::CommandsApiState {
            tenant_id,
            sender_party_id: NB_ID.to_owned(),
            configured_marktrollen: vec!["NB".to_owned()],
            max_body_bytes: 1_048_576,
            snapshot_interval: 100,
            cedar: Arc::new(
                makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
            ),
            snapshot_store: store.as_snapshot_store(),
            malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new(
                (*store).clone(),
                tenant_id,
            )),
            maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
            store: Arc::clone(&store),
            marktd_client: None,
        };
        Self {
            store,
            state,
            process_id: process_id.to_string(),
        }
    }

    async fn bestaetigen(
        &self,
        body: serde_json::Value,
    ) -> Result<(), makod::commands_api::DispatchError> {
        makod::commands_api::dispatch_command(&self.state, "gpke.neuanlage.bestaetigen", &body)
            .await
            .map(|_| ())
    }

    /// The UTILMD answers written, rendered.
    async fn antworten(&self) -> Vec<String> {
        use mako_engine::outbox::OutboxStore as _;
        let registry =
            makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
                mp_id: NB_ID.to_owned(),
                roles: vec!["NB".to_owned()],
                primary: true,
                agency: None,
                as4: None,
            }])
            .expect("registry");
        self.store
            .pending(
                100,
                time::OffsetDateTime::now_utc() + time::Duration::days(1),
            )
            .await
            .expect("pending outbox")
            .into_iter()
            .filter(|om| &*om.message_type == "UTILMD")
            .map(|om| {
                let wire = makod::edifact_renderer::render_to_wire_bytes(&om, &registry)
                    .expect("the answer renders");
                let report = edi_energy::parse(&wire.bytes)
                    .expect("parses")
                    .validate_on_date(mako_fristen::heute())
                    .expect("validation runs");
                assert!(
                    report.is_sendable(),
                    "{:#?}",
                    report.sending_refusals().collect::<Vec<_>>()
                );
                String::from_utf8(wire.bytes).expect("UNOC text")
            })
            .collect()
    }
}

/// The grundzuständige MSB of the new Marktlokation and its Messlokation.
fn msb() -> serde_json::Value {
    serde_json::json!({ "mp_id": NB_ID, "rolle": "Z39", "grundlage": "Z19", "gmsb_mp_id": NB_ID })
}

const MELO: &str = "DE00056266802AO6G56M11SN51G21M24S";

/// A Bestätigung 55602 names how the new Marktlokation is metered: `ZW6`
/// „Pauschale Marktlokation" or `ZW7` „Gemessene Marktlokation" (UTILMD AHB
/// Strom 2.2 Kap. 8.4, S. 97), and behind `ZW7` the Messlokationen (`[483]`,
/// S. 100) with the `SG8 SEQ+Z98` of their MSB (S. 105). Only the NB knows
/// which: a body that states neither takes it from the NB's record in marktd
/// and is refused where there is none to read, and one that states it goes
/// out complete.
#[tokio::test]
async fn a_55602_states_its_classification_or_is_refused() {
    let door = NeuanlageDoor::after(55600).await;
    let ohne = serde_json::json!({ "process_id": door.process_id, "malo_id": IDENTIFIZIERT, "antwort_code": "A09" });
    match door.bestaetigen(ohne).await {
        Err(makod::commands_api::DispatchError::MarktdUnavailable(_)) => {}
        other => {
            panic!("a 55602 that classifies nothing and has no marktd is refused, got {other:?}")
        }
    }
    assert!(door.antworten().await.is_empty(), "nothing is written");

    let mit = serde_json::json!({
        "process_id": door.process_id, "malo_id": IDENTIFIZIERT,
        "antwort_code": "A09",
        "transaktionsgrund_ergaenzung": "ZW7",
        "messlokationen": [{ "melo_id": MELO, "msb": msb() }],
        "malo_msb": msb(),
    });
    door.bestaetigen(mit).await.expect("the 55602 is written");
    let [wire] = door.antworten().await.try_into().expect("one answer");
    for place in [
        &format!("LOC+Z16+{IDENTIFIZIERT}'"),
        "RFF+Z13:55602'",
        "+ZW7'",
        "RFF+Z60:",
        &format!("LOC+Z17+{MELO}'"),
        "SEQ+Z98'",
        "SEQ+ZF3'",
    ] {
        assert!(wire.contains(place), "{place} in {wire}");
    }
}

/// The body is typed: a key it does not declare is refused, not ignored.
#[tokio::test]
async fn a_neuanlage_body_with_an_unknown_key_is_refused() {
    let door = NeuanlageDoor::after(55600).await;
    let body = serde_json::json!({
        "process_id": door.process_id, "malo_id": IDENTIFIZIERT,
        "antwort_code": "A09",
        "transaktionsgrund_ergaenzung": "ZW6",
        "zustimmung": true,
    });
    match door.bestaetigen(body).await {
        Err(makod::commands_api::DispatchError::InvalidPayload(m)) => {
            assert!(m.contains("zustimmung"), "{m}");
        }
        other => panic!("an undeclared key is refused, got {other:?}"),
    }
}

/// A Bestätigung 55603 restates the Anmeldung's Geschäftsvorfall (UTILMD AHB
/// Strom 2.2 Kap. 8.5, S. 111) and always names the Messlokationen (`[623]`,
/// S. 114) with the `SG8 SEQ+Z98` of their MSB (S. 119).
#[tokio::test]
async fn a_55603_restates_the_geschaeftsvorfall_and_names_its_messlokationen() {
    let door = NeuanlageDoor::after(55601).await;
    let body = serde_json::json!({
        "process_id": door.process_id, "malo_id": IDENTIFIZIERT,
        "antwort_code": "A18",
        "messlokationen": [{ "melo_id": MELO, "msb": msb() }],
        "malo_msb": msb(),
    });
    // Geschäftsvorfall 3 names its Tranche (`LOC+Z21`, Muss behind `ZW2`,
    // Bedingung `[478]`, S. 114), which the body does not carry.
    let mut tranche = body.clone();
    tranche["transaktionsgrund_ergaenzung"] = serde_json::json!("ZW2");
    assert!(
        matches!(
            door.bestaetigen(tranche).await,
            Err(makod::commands_api::DispatchError::InvalidPayload(_))
        ),
        "a ZW2 Bestätigung without its Tranche is refused"
    );
    door.bestaetigen(body).await.expect("the 55603 is written");
    let [wire] = door.antworten().await.try_into().expect("one answer");
    for place in [
        "RFF+Z13:55603'",
        "STS+7++E02+ZW0'",
        "RFF+Z60:",
        &format!("LOC+Z17+{MELO}'"),
        "SEQ+Z98'",
    ] {
        assert!(wire.contains(place), "{place} in {wire}");
    }
}

// ── The Lieferant's door: `gpke.neuanlage.anmelden` ───────────────────────────

/// A Bilanzkreis EIC of the Lieferant.
const BILANZKREIS: &str = "11XBK-LF-------9";

/// An LF deployment and its door for the Anmeldung neue Marktlokation.
struct LfNeuanlage {
    store: std::sync::Arc<mako_engine::store_slatedb::SlateDbStore>,
    state: makod::commands_api::CommandsApiState,
}

impl LfNeuanlage {
    async fn new() -> Self {
        use mako_engine::store_slatedb::SlateDbStore;
        use std::sync::Arc;
        let store = Arc::new(SlateDbStore::open_in_memory().await.expect("store"));
        let state = makod::commands_api::CommandsApiState {
            tenant_id: TenantId::from_party_id(LF_ID),
            sender_party_id: LF_ID.to_owned(),
            configured_marktrollen: vec!["LF".to_owned()],
            max_body_bytes: 1_048_576,
            snapshot_interval: 100,
            cedar: Arc::new(
                makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
            ),
            snapshot_store: store.as_snapshot_store(),
            malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new(
                (*store).clone(),
                TenantId::from_party_id(LF_ID),
            )),
            maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
            store: Arc::clone(&store),
            marktd_client: None,
        };
        Self { store, state }
    }

    async fn anmelden(
        &self,
        body: &serde_json::Value,
    ) -> Result<String, makod::commands_api::DispatchError> {
        match makod::commands_api::dispatch_command(&self.state, "gpke.neuanlage.anmelden", body)
            .await?
        {
            makod::commands_api::DispatchOutcome::Spawned { process_id } => {
                Ok(process_id.to_string())
            }
            other => panic!("the Anmeldung spawns its process: {other:?}"),
        }
    }

    /// The outbox entries of `message_type`, as the outbox worker reads them.
    async fn outbox(&self, message_type: &str) -> Vec<mako_engine::outbox::OutboxMessage> {
        use mako_engine::outbox::OutboxStore as _;
        self.store
            .pending(
                100,
                time::OffsetDateTime::now_utc() + time::Duration::days(1),
            )
            .await
            .expect("pending outbox")
            .into_iter()
            .filter(|om| &*om.message_type == message_type)
            .collect()
    }

    /// The one UTILMD written, rendered as the LF sends it and held against
    /// its profile.
    async fn gesendet(&self) -> Vec<u8> {
        let registry =
            makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
                mp_id: LF_ID.to_owned(),
                roles: vec!["LF".to_owned()],
                primary: true,
                agency: None,
                as4: None,
            }])
            .expect("registry");
        let [om] = self
            .outbox("UTILMD")
            .await
            .try_into()
            .expect("one Anmeldung");
        assert_eq!(&*om.recipient, NB_ID, "the Anmeldung goes to the NB named");
        let wire = makod::edifact_renderer::render_to_wire_bytes(&om, &registry)
            .expect("the Anmeldung renders");
        let report = edi_energy::parse(&wire.bytes)
            .expect("parses")
            .validate_on_date(mako_fristen::heute())
            .expect("validation runs");
        assert!(
            report.is_sendable(),
            "{:#?}\n{}",
            report.sending_refusals().collect::<Vec<_>>(),
            String::from_utf8_lossy(&wire.bytes)
        );
        wire.bytes
    }

    /// Hand the NB's answer to the LF's ingest.
    async fn empfangen(&self, antwort: &[u8]) -> makod::ingest_dispatcher::IngestOutcome {
        let msg = edi_energy::parse(antwort).expect("the answer parses");
        let pid = msg
            .detect_pruefidentifikator()
            .expect("the answer names its PID")
            .as_u32();
        makod::ingest_dispatcher::EdifactIngestDispatcher::new(
            std::sync::Arc::clone(&self.store),
            self.store.as_snapshot_store(),
            100,
            self.state.tenant_id,
        )
        .dispatch(
            &msg,
            "gpke-lf-anmeldung",
            pid,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("the answer dispatches")
    }
}

/// An Anmeldung neue erzeugende Marktlokation as the Lieferant states it.
fn neue_erzeugende(geschaeftsvorfall: &str) -> serde_json::Value {
    serde_json::json!({
        "pid": 55601,
        "nb_mp_id": NB_ID,
        "vertragsbeginn": "2026-12-01",
        "transaktionsgrund_ergaenzung": geschaeftsvorfall,
        "bilanzkreis": BILANZKREIS,
    })
}

/// The Lieferant sends the Anmeldung neue erzeugende Marktlokation 55601 (LF
/// an NB, UTILMD AHB Strom 2.2 Kap. 8.5, S. 109) to the Netzbetreiber the
/// body names — the new Marktlokation has no record to resolve him from — and
/// it passes its column: `STS+7` `E02` with the Geschäftsvorfall (S. 110),
/// the Produktpaket (S. 114), and no `LOC+Z16` where the MaLo-ID is not yet
/// known (Soll „Wenn bekannt", `[165]`, S. 112).
#[tokio::test]
async fn posting_a_55601_renders_a_profile_conformant_anmeldung() {
    let lf = LfNeuanlage::new().await;
    lf.anmelden(&neue_erzeugende("ZW0"))
        .await
        .expect("the 55601 is written");
    let wire = String::from_utf8_lossy(&lf.gesendet().await).into_owned();
    for place in ["RFF+Z13:55601'", "STS+7++E02+ZW0'", "SEQ+Z79+"] {
        assert!(wire.contains(place), "{place} in {wire}");
    }
    assert!(
        !wire.contains("LOC+Z16"),
        "no Marktlokation is named: {wire}"
    );
}

/// The body is typed and the column's facts are Muss: a key it does not
/// declare, a 55601 without its Geschäftsvorfall, a `ZW1` the column does not
/// list (S. 110) and a `ZW2` without its Tranchengröße (Codeliste der
/// Konfigurationen 1.4 Kap. 6.1.1) are each refused, and nothing is written.
#[tokio::test]
async fn a_55601_without_its_column_facts_is_refused() {
    let lf = LfNeuanlage::new().await;
    let mut unbekannt = neue_erzeugende("ZW0");
    unbekannt["malo"] = serde_json::json!(IDENTIFIZIERT);
    let mut ohne = neue_erzeugende("ZW0");
    ohne.as_object_mut()
        .expect("an object")
        .remove("transaktionsgrund_ergaenzung");
    for (body, named) in [
        (unbekannt, "malo"),
        (ohne, "transaktionsgrund_ergaenzung"),
        (neue_erzeugende("ZW1"), "ZW0/ZW2"),
        (neue_erzeugende("ZW2"), "Tranchengröße"),
    ] {
        match lf.anmelden(&body).await {
            Err(makod::commands_api::DispatchError::InvalidPayload(m)) => {
                assert!(m.contains(named), "{named}: {m}");
            }
            other => panic!("{body} is refused, got {other:?}"),
        }
    }
    assert!(lf.outbox("UTILMD").await.is_empty(), "nothing is written");
}

/// The Anmeldung neue verbrauchende Marktlokation 55600 names the Kunde des
/// Lieferanten and his Gruppenzuordnung, both Muss (UTILMD AHB Strom 2.2 Kap.
/// 8.4, S. 104, 106–107): without them it is refused, with them it passes.
#[tokio::test]
async fn a_55600_names_its_kunde() {
    let lf = LfNeuanlage::new().await;
    let mut body = serde_json::json!({
        "pid": 55600,
        "nb_mp_id": NB_ID,
        "vertragsbeginn": "2026-12-01",
        "bilanzkreis": BILANZKREIS,
    });
    assert!(
        matches!(
            lf.anmelden(&body).await,
            Err(makod::commands_api::DispatchError::InvalidPayload(m)) if m.contains("kunde")
        ),
        "a 55600 without its Kunde is refused"
    );
    body["haushaltskunde"] = serde_json::json!(true);
    body["kunde"] = serde_json::json!({
        "vorname": "Erika", "nachname": "Mustermann",
        "anschrift": {
            "strasse": "Hauptstraße", "hausnummer": "1",
            "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
        }
    });
    lf.anmelden(&body).await.expect("the 55600 is written");
    let wire = String::from_utf8_lossy(&lf.gesendet().await).into_owned();
    for place in ["RFF+Z13:55600'", "STS+7++E02+ZW4'", "NAD+Z09+", "NAD+Z04+"] {
        assert!(wire.contains(place), "{place} in {wire}");
    }
}

/// The NB's Bestätigung finds the Lieferant's process by the Vorgangsnummer
/// it restates in `SG6 RFF+TN` (Muss on 55603, S. 114) — the Anmeldung named
/// no Marktlokation to find it by — and tells it the MaLo-ID the NB
/// identified (`LOC+Z16`, Muss, S. 112), which the process reports on.
#[tokio::test]
async fn an_inbound_55603_resumes_the_lfs_process() {
    let lf = LfNeuanlage::new().await;
    let process_id = lf
        .anmelden(&neue_erzeugende("ZW0"))
        .await
        .expect("the 55601 is written");
    let nb = NeuanlageDoor::receiving(&lf.gesendet().await).await;
    nb.bestaetigen(serde_json::json!({
        "process_id": nb.process_id, "malo_id": IDENTIFIZIERT,
        "antwort_code": "A18",
        "messlokationen": [{ "melo_id": MELO, "msb": msb() }],
        "malo_msb": msb(),
    }))
    .await
    .expect("the 55603 is written");
    let [antwort] = nb.antworten().await.try_into().expect("one answer");
    match lf.empfangen(antwort.as_bytes()).await {
        makod::ingest_dispatcher::IngestOutcome::Dispatched {
            process_id: resumed,
            ..
        } => assert_eq!(resumed.to_string(), process_id),
        other => panic!("the 55603 resumes the LF's process, got {other:?}"),
    }
    let [completed] = lf
        .outbox("ProcessCompleted")
        .await
        .try_into()
        .expect("one completion");
    assert_eq!(completed.payload["pid"], 55603);
    assert_eq!(completed.payload["accepted"], true);
    assert_eq!(completed.payload["malo_id"], IDENTIFIZIERT);
}

/// The Tranche the NB forms for Geschäftsvorfall 3 — a Marktlokations-ID
/// of its own (`[950]`).
const TRANCHE: &str = "51238696020";

/// Geschäftsvorfall 3 builds a new Tranche: the Lieferant's 55601 `ZW2`
/// carries its Tranchengröße (Codeliste der Konfigurationen 1.4 Kap. 6.1.1),
/// and the NB's Bestätigung 55603 names the Tranche it formed — `SG5 LOC+Z21`
/// and the `SG8 SEQ+Z15` „Daten der Tranche" with its `QTY+11` share and
/// `CCI+Z37` basis, Muss behind `ZW2` (Bedingung `[478]`, UTILMD AHB Strom 2.2
/// S. 113, 119–120). A Bestätigung without it is refused naming it; with it
/// the answer passes its column and resumes the Lieferant's process.
#[tokio::test]
async fn a_zw2_55603_names_its_tranche() {
    let lf = LfNeuanlage::new().await;
    let mut anmeldung = neue_erzeugende("ZW2");
    anmeldung["tranchengroesse"] = serde_json::json!({ "art": "prozent", "wert": "40" });
    let process_id = lf.anmelden(&anmeldung).await.expect("the 55601 is written");
    let nb = NeuanlageDoor::receiving(&lf.gesendet().await).await;
    let mut body = serde_json::json!({
        "process_id": nb.process_id, "malo_id": IDENTIFIZIERT,
        "antwort_code": "A18",
        "messlokationen": [{ "melo_id": MELO, "msb": msb() }],
        "malo_msb": msb(),
    });
    match nb.bestaetigen(body.clone()).await {
        Err(makod::commands_api::DispatchError::InvalidPayload(m)) => {
            assert!(m.contains("tranche"), "{m}");
        }
        other => panic!("a ZW2 Bestätigung without its Tranche is refused, got {other:?}"),
    }
    body["tranche"] = serde_json::json!({ "tranche_id": TRANCHE, "basis": "ZD1", "prozent": "40" });
    nb.bestaetigen(body).await.expect("the 55603 is written");
    let [antwort] = nb.antworten().await.try_into().expect("one answer");
    for place in [
        "STS+7++E02+ZW2'",
        &format!("LOC+Z21+{TRANCHE}'"),
        "SEQ+Z15'",
        "QTY+11:40:P1'",
        "CCI+Z37++ZD1'",
    ] {
        assert!(antwort.contains(place), "{place} in {antwort}");
    }
    match lf.empfangen(antwort.as_bytes()).await {
        makod::ingest_dispatcher::IngestOutcome::Dispatched {
            process_id: resumed,
            ..
        } => assert_eq!(resumed.to_string(), process_id),
        other => panic!("the 55603 resumes the LF's process, got {other:?}"),
    }
}

/// processd's automatic Bestätigung states the code, the process and the
/// Marktlokation the identification found, nothing of what the NB knows of
/// it. makod's door reads that from marktd — the NB's record — on the
/// Anmeldung's Vertragsbeginn: the Messlokation with its MSB (`SG5 LOC+Z17`,
/// `SG8 SEQ+Z98` / `SEQ+ZF3`) and, on a 55602, the classification `ZW7`
/// that follows from a Messlokation being assigned (UTILMD AHB Strom 2.2
/// Kap. 8.4, S. 97, 100, 105). Without a marktd record the answer is refused,
/// and nothing is written.
#[tokio::test]
async fn an_automatic_bestaetigung_reads_its_lokationsdaten_from_marktd() {
    let mut door = NeuanlageDoor::after(55600).await;
    let body = serde_json::to_value(mako_markt::commands::GpkeNeuanlageBeantworten {
        malo_id: Some(IDENTIFIZIERT.to_owned()),
        process_id: Some(door.process_id.clone()),
        antwort_code: "A09".to_owned(),
        antwort_codeliste: Some("E_0608".to_owned()),
        bemerkung: None,
        transaktionsgrund_ergaenzung: None,
        geplantes_produktpaket: None,
        malo_msb: None,
        messlokationen: Vec::new(),
        tranche: None,
    })
    .expect("the body serialises");

    door.state.marktd_client = Some(std::sync::Arc::new(crate::support::stub_marktd(None).await));
    assert!(
        door.bestaetigen(body.clone()).await.is_err(),
        "no marktd record — refused"
    );
    assert!(door.antworten().await.is_empty(), "nothing is written");

    door.state.marktd_client = Some(std::sync::Arc::new(
        crate::support::stub_marktd(Some(crate::support::strom_stammdaten(IDENTIFIZIERT, NB_ID)))
            .await,
    ));
    door.bestaetigen(body).await.expect("the 55602 is written");
    let [wire] = door.antworten().await.try_into().expect("one answer");
    for place in [
        &format!("LOC+Z16+{IDENTIFIZIERT}'"),
        "STS+7++E02+ZW7'",
        &format!("LOC+Z17+{}'", crate::support::STROM_MELO),
        "SEQ+Z98'",
        "SEQ+ZF3'",
    ] {
        assert!(wire.contains(place), "{place} in {wire}");
    }
}
