//! The Antworten auf Anfrage of UTILMD AHB Gas 1.2 Kap. 5.12 beside the 44142:
//! each is built by `geli.stammdaten.anfrage.beantworten` from the column the
//! code selects, rendered, parsed back and held against its AHB profile.
//!
//! | Anfrage | Antwort | Data |
//! |---|---|---|
//! | 44156 LF → NB | 44157 `G_0034` | marktd's Prognosegrundlage (Kap. 5.12.2) |
//! | 44180 LF → NB | 44181 `G_0035` / 44182 `G_0036` | marktd's Messlokationen (Kap. 5.12.3, `[590]`) |
//! | 44137 NB → LF | 44138 `G_0040` | the LF's own facts, stated by the command (Kap. 5.12.4) |
//! | 44150 NB → LF | 44151 `G_0038` / 44152 `G_0039` | the LF's Bilanzkreis (Kap. 5.12.5) |
//! | 44139 / 44140 → NB | 44142 | marktd's Turnusablesung and Netznutzungsvertrag (Kap. 5.12.1); the 44140 names a Messlokation, answered from the Marktlokation marktd assigns it to |
//!
//! The NB's 44163/44164 as Verteiler of a 44162 (Kap. 5.12.7) are not
//! assembled and answer `501`.

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
use mako_engine::outbox::OutboxStore;
use mako_engine::pid_router::PidRouter;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::commands_api::{CommandsApiState, DispatchError, dispatch_command};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::EdifactIngestDispatcher;

use crate::support;

// DVGW-Codenummern: NAD DE 3055 `332`.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "9870012345678";
const MSB_MP: &str = "9800357000011";
const MALO: &str = "51238696012";
const COMMAND: &str = "geli.stammdaten.anfrage.beantworten";

struct Deployment {
    store: Arc<SlateDbStore>,
    state: CommandsApiState,
    dispatcher: EdifactIngestDispatcher,
    router: PidRouter,
    registry: makod::party_registry::MpIdRegistry,
}

/// A GNB (`lf == false`) or a Gas Lieferant deployment answering from `record`.
async fn deployment(
    lf: bool,
    record: Option<mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten>,
) -> Deployment {
    use makod::config::PartyConfig;
    let own = if lf { LF_MP } else { GNB_MP };
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant_id = TenantId::from_party_id(own);
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_id,
    );
    let state = CommandsApiState {
        tenant_id,
        sender_party_id: own.to_owned(),
        configured_marktrollen: vec![if lf { "LF" } else { "GNB" }.to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new((*store).clone())),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store: Arc::clone(&store),
        marktd_client: Some(Arc::new(support::stub_marktd(record).await)),
    };
    let rollen: BTreeSet<Rolle> = [if lf { Rolle::LfGas } else { Rolle::NbGas }]
        .into_iter()
        .collect();
    let roles = DeploymentRoles::from_roles([if lf { Marktrolle::Lf } else { Marktrolle::Nb }]);
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, &roles);
    }
    let party = |mp: &str, role: &str| PartyConfig {
        mp_id: mp.to_owned(),
        roles: vec![role.to_owned()],
        primary: mp == own,
        agency: None,
        as4: None,
    };
    let registry = makod::party_registry::MpIdRegistry::from_config(&[
        party(GNB_MP, "NB"),
        party(LF_MP, "LF"),
        party(MSB_MP, "MSB"),
    ])
    .expect("valid registry");
    Deployment {
        store,
        state,
        dispatcher,
        router,
        registry,
    }
}

/// Render `om`, parse it and hold it against its AHB profile, as the pre-send
/// gate does.
fn gated(
    om: &mako_engine::outbox::OutboxMessage,
    registry: &makod::party_registry::MpIdRegistry,
) -> (edi_energy::AnyMessage, String) {
    use edi_energy::EdiEnergyMessage as _;
    let wire = makod::edifact_renderer::render_to_wire_bytes(om, registry)
        .unwrap_or_else(|e| panic!("render {}: {e:?}", om.message_type));
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let msg = edi_energy::parse(&wire.bytes).unwrap_or_else(|e| panic!("parse: {e:?}"));
    let report = msg
        .validate_on_date(mako_fristen::heute())
        .expect("the AHB validation runs");
    assert!(
        report.is_sendable(),
        "the rendered {} violates its AHB profile: {:#?}\n{}",
        om.message_type,
        report
            .sending_refusals()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        text.replace('\'', "'\n"),
    );
    (msg, text)
}

fn seg(tag: &str, e: &[&[&str]]) -> serde_json::Value {
    serde_json::json!({
        "tag": tag,
        "elemente": e.iter().map(|c| c.to_vec()).collect::<Vec<_>>(),
    })
}

/// The Meldepunkt an Anfrage `pid` names: a Zählpunktbezeichnung where its
/// column formats it so — the 44140 and the 44162 (`[951]`, UTILMD AHB Gas
/// 1.2 Kap. 5.12.1, S. 149; Kap. 5.12.7) —, the Marktlokation otherwise.
fn meldepunkt(pid: u32) -> &'static str {
    if matches!(pid, 44140 | 44162) {
        support::GAS_MELO
    } else {
        MALO
    }
}

/// An inbound Anfrage `pid` from `sender` to `receiver`, Grund `grund`,
/// carrying `stammdaten`.
fn anfrage(
    pid: u32,
    sender: &str,
    receiver: &str,
    grund: &str,
    stammdaten: serde_json::Value,
) -> mako_engine::outbox::OutboxMessage {
    support::outbox_message(
        "UTILMD",
        receiver,
        serde_json::json!({
            "pid": pid,
            "sender": sender,
            "receiver": receiver,
            "malo": meldepunkt(pid),
            "process_date": "2026-12-01",
            "vertragsbeginn": "2026-12-01",
            "vorgangsnummer": "ANFRAGE-VG-1",
            "transaktionsgrund": grund,
            "stammdaten": stammdaten,
        }),
    )
}

impl Deployment {
    /// Deliver `om` to this deployment's ingest.
    async fn receive(&self, om: &mako_engine::outbox::OutboxMessage, pid: u32) {
        use edi_energy::EdiEnergyMessage as _;
        let (msg, _) = gated(om, &self.registry);
        assert_eq!(
            msg.detect_pruefidentifikator().map(|p| p.as_u32()).ok(),
            Some(pid)
        );
        let workflow = self.router.route(pid).expect("the Anfrage is routed");
        self.dispatcher
            .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
            .await
            .expect("the Anfrage dispatches");
    }

    async fn pending_utilmd(&self) -> Vec<mako_engine::outbox::OutboxMessage> {
        let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        OutboxStore::pending(&*self.store, 100, now)
            .await
            .expect("pending outbox")
            .into_iter()
            .filter(|om| &*om.message_type == "UTILMD")
            .collect()
    }

    async fn answer(&self, body: serde_json::Value) -> Result<(), DispatchError> {
        dispatch_command(&self.state, COMMAND, &body)
            .await
            .map(|_| ())
    }

    /// The one UTILMD written, gated, with its Prüfidentifikator.
    async fn the_antwort(&self) -> (u32, String) {
        use edi_energy::EdiEnergyMessage as _;
        let utilmd = self.pending_utilmd().await;
        let [om] = utilmd.as_slice() else {
            panic!("one UTILMD is written, got {}", utilmd.len());
        };
        let (msg, wire) = gated(om, &self.registry);
        let pid = msg
            .detect_pruefidentifikator()
            .expect("a Prüfidentifikator")
            .as_u32();
        (pid, wire)
    }
}

fn body(code: &str) -> serde_json::Value {
    serde_json::json!({ "malo_id": MALO, "antwort_code": code })
}

/// The command answering an Anfrage keyed on the Messlokation.
fn melo_body(code: &str) -> serde_json::Value {
    serde_json::json!({ "melo_id": support::GAS_MELO, "antwort_code": code })
}

fn assert_contains(wire: &str, expected: &[&str]) {
    for e in expected {
        assert!(wire.contains(e), "{e} in\n{}", wire.replace('\'', "'\n"));
    }
}

fn assert_absent(wire: &str, absent: &[&str]) {
    for a in absent {
        assert!(
            !wire.contains(a),
            "{a} not in\n{}",
            wire.replace('\'', "'\n")
        );
    }
}

// ── 44156 → 44157 ─────────────────────────────────────────────────────────────

fn anfrage_44156() -> mako_engine::outbox::OutboxMessage {
    anfrage(
        44156,
        LF_MP,
        GNB_MP,
        "ZF8",
        serde_json::json!({ "bloecke": [{ "seq": "Z01", "segmente": [
            seg("CCI", &[&[], &[], &["ZC0"]]),
            seg("CCI", &[&[], &[], &["Z17", "GABi-RLMoT"]]),
        ]}]}),
    )
}

/// `ZG2` and `ZG4` both carry marktd's Prognosegrundlage — the column's
/// `SEQ+Z01` is Muss on either (UTILMD AHB Gas 1.2 Kap. 5.12.2, S. 164), and
/// `ZG4` „übermittelt die zum angefragten Datum gültigen Stammdaten" (EBD 4.3
/// Codeliste `G_0034`, S. 743).
#[tokio::test]
async fn the_44157_carries_marktds_prognosegrundlage_on_zg2_and_zg4() {
    for code in ["ZG2", "ZG4"] {
        let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
        nb.receive(&anfrage_44156(), 44156).await;
        nb.answer(body(code))
            .await
            .expect("the 44157 is dispatched");
        let (pid, wire) = nb.the_antwort().await;
        assert_eq!(pid, 44157);
        assert_contains(
            &wire,
            &[
                &format!("STS+E01++{code}:G_0034"),
                "STS+7++ZF8",
                "RFF+TN:ANFRAGE-VG-1",
                "SEQ+Z01",
                "CCI+++ZC0",
                // marktd's Fallgruppe, not the proposed one.
                "CCI+++Z17:GABi-RLMmT",
            ],
        );
        assert_absent(&wire, &["GABi-RLMoT"]);
    }
}

/// A code `G_0034` does not publish is refused naming the list, and a
/// Prognose auf Basis von Profilen — whose Lastprofildaten marktd does not
/// hold — is refused naming them; nothing goes out.
#[tokio::test]
async fn the_44157_refuses_what_its_column_cannot_carry() {
    let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    nb.receive(&anfrage_44156(), 44156).await;
    let err = nb
        .answer(body("ZG0"))
        .await
        .expect_err("ZG0 is no G_0034 code");
    assert!(format!("{err:?}").contains("G_0034"), "{err:?}");

    let mut record = support::gas_stammdaten(MALO, GNB_MP);
    if let Some(b) = record.bilanzierung.as_mut() {
        b.prognosegrundlage = Some(rubo4e::current::Prognosegrundlage::Profile);
    }
    let nb = deployment(false, Some(record)).await;
    nb.receive(&anfrage_44156(), 44156).await;
    match nb.answer(body("ZG2")).await {
        Err(DispatchError::StammdatenUnvollstaendig { fehlend, .. }) => {
            assert!(
                fehlend.iter().any(|f| f.contains("Lastprofildaten")),
                "{fehlend:?}"
            );
        }
        other => panic!("refused as incomplete Stammdaten, got {other:?}"),
    }
    assert!(nb.pending_utilmd().await.is_empty());
}

// ── 44180 → 44181 / 44182 ─────────────────────────────────────────────────────

fn anfrage_44180() -> mako_engine::outbox::OutboxMessage {
    anfrage(
        44180,
        LF_MP,
        GNB_MP,
        "ZJ1",
        serde_json::json!({ "lokationen": [
            { "qualifier": "172", "id": support::GAS_MELO },
        ]}),
    )
}

/// Both columns name „die ID der Marktlokation … sowie alle Identifikatoren
/// der Messlokationen" (`[590]`); the code picks the PID, and the Ablehnung
/// 44182 lists no `SG4 DTM` (UTILMD AHB Gas 1.2 Kap. 5.12.3, S. 168–169).
#[tokio::test]
async fn the_marktlokationsstruktur_is_answered_44181_or_refused_44182() {
    for (code, pid, liste) in [("ZG2", 44181, "G_0035"), ("ZJ5", 44182, "G_0036")] {
        let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
        nb.receive(&anfrage_44180(), 44180).await;
        nb.answer(body(code))
            .await
            .expect("the answer is dispatched");
        let (got, wire) = nb.the_antwort().await;
        assert_eq!(got, pid);
        assert_contains(
            &wire,
            &[
                &format!("STS+E01++{code}:{liste}"),
                &format!("LOC+172+{MALO}"),
                &format!("LOC+172+{}", support::GAS_MELO),
                "RFF+TN:ANFRAGE-VG-1",
            ],
        );
        if pid == 44182 {
            assert_absent(&wire, &["DTM+92", "DTM+157"]);
        } else {
            assert_contains(&wire, &["DTM+92", "DTM+157"]);
        }
    }
}

// ── 44137 → 44138 (at the LF) ─────────────────────────────────────────────────

fn anfrage_44137() -> mako_engine::outbox::OutboxMessage {
    anfrage(
        44137,
        GNB_MP,
        LF_MP,
        "ZF3",
        serde_json::json!({
            "sg6": [
                seg("RFF", &[&["Z18", MALO]]),
                seg("DTM", &[&["Z20", "6", "802"]]),
            ],
            "bloecke": [{ "seq": "Z01", "segmente": [seg("CCI", &[&[], &[], &["Z18"]])] }],
            "sg12": [
                seg("NAD", &[&["Z09"], &[], &[], &["Neu Name", "", "", "", "", "Z01"]]),
                seg("NAD", &[
                    &["Z04"], &[], &[], &["Neu Name", "", "", "", "", "Z01"],
                    &["Neuweg 1"], &["Berlin"], &[], &["10115"], &["DE"],
                ]),
            ],
        }),
    )
}

fn lf_body(code: &str) -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "antwort_code": code,
        "abrechnungsintervall_monate": 12,
        "haushaltskunde": true,
        "kunde": {
            "vorname": "Erika",
            "nachname": "Muster",
            "anschrift": {
                "strasse": "Postweg",
                "hausnummer": "7",
                "postleitzahl": "10115",
                "ort": "Berlin",
                "landescode": "DE",
            },
        },
    })
}

/// The LF answers the NB's 44137 with its own facts for each group the
/// Anfrage filled (UTILMD AHB Gas 1.2 Kap. 5.12.4, S. 171–175, `[336]`): the
/// command states them, since they are the LF's and not marktd's.
#[tokio::test]
async fn the_lf_answers_a_44137_with_its_own_facts() {
    let lf = deployment(true, None).await;
    lf.receive(&anfrage_44137(), 44137).await;
    lf.answer(lf_body("ZG2"))
        .await
        .expect("the 44138 is dispatched");
    let (pid, wire) = lf.the_antwort().await;
    assert_eq!(pid, 44138);
    assert_contains(
        &wire,
        &[
            "STS+E01++ZG2:G_0040",
            "STS+7++ZF3",
            &format!("RFF+Z18:{MALO}"),
            "DTM+Z20:12:802",
            "CCI+++Z15",
            "NAD+Z09+++Erika Muster:::::Z01",
            "NAD+Z04+++Erika Muster:::::Z01+Postweg 7+Berlin++10115+DE",
        ],
    );
    assert_absent(&wire, &["Neu Name", "DTM+Z20:6"]);
}

/// A fact of the LF's own its Antwort must carry and the command omits is
/// refused naming it; a `ZG0` carries none; an LF field on an NB Antwort is
/// refused.
#[tokio::test]
async fn the_44138_refuses_a_missing_lf_fact_by_name() {
    let lf = deployment(true, None).await;
    lf.receive(&anfrage_44137(), 44137).await;
    let mut b = lf_body("ZG2");
    b.as_object_mut().expect("object").remove("kunde");
    let err = lf.answer(b).await.expect_err("the Kunde is missing");
    assert!(
        matches!(&err, DispatchError::InvalidPayload(m) if m.contains("kunde")),
        "{err:?}"
    );
    assert!(lf.pending_utilmd().await.is_empty());

    lf.answer(body("ZG0")).await.expect("the ZG0 is dispatched");
    let (pid, wire) = lf.the_antwort().await;
    assert_eq!(pid, 44138);
    assert_contains(&wire, &["STS+E01++ZG0:G_0040"]);
    assert_absent(&wire, &["SEQ+", "NAD+Z09", "DTM+Z20"]);

    let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    nb.receive(&anfrage_44180(), 44180).await;
    let err = nb
        .answer(serde_json::json!({ "malo_id": MALO, "antwort_code": "ZG2", "bilanzkreis": "X" }))
        .await
        .expect_err("an NB Antwort takes no LF fact");
    assert!(format!("{err:?}").contains("bilanzkreis"), "{err:?}");
}

// ── 44150 → 44151 / 44152 (at the LF) ─────────────────────────────────────────

/// A Gas Bilanzkreis code (DVGW Bilanzkreiscode, `CCI+Z19` DE 7037).
const BILANZKREIS: &str = "THE0BFH001230000";

fn anfrage_44150() -> mako_engine::outbox::OutboxMessage {
    anfrage(
        44150,
        GNB_MP,
        LF_MP,
        "ZF6",
        serde_json::json!({ "bloecke": [{ "seq": "Z01", "segmente": [
            seg("CCI", &[&["Z19"], &[], &["THE0BFH009990000"]]),
        ]}]}),
    )
}

/// The LF answers a 44150 with its Bilanzkreis on 44151 (`G_0038`), or
/// refuses it on 44152 (`G_0039`), which lists no `SG4 DTM` (UTILMD AHB Gas
/// 1.2 Kap. 5.12.5, S. 176–177).
#[tokio::test]
async fn the_lf_answers_a_44150_with_its_bilanzkreis_or_refuses_it() {
    let lf = deployment(true, None).await;
    lf.receive(&anfrage_44150(), 44150).await;
    let err = lf
        .answer(body("ZG2"))
        .await
        .expect_err("no Bilanzkreis stated");
    assert!(format!("{err:?}").contains("bilanzkreis"), "{err:?}");
    lf.answer(serde_json::json!({
        "malo_id": MALO, "antwort_code": "ZG2", "bilanzkreis": BILANZKREIS,
    }))
    .await
    .expect("the 44151 is dispatched");
    let (pid, wire) = lf.the_antwort().await;
    assert_eq!(pid, 44151);
    assert_contains(
        &wire,
        &["STS+E01++ZG2:G_0038", &format!("CCI+Z19++{BILANZKREIS}")],
    );

    let lf = deployment(true, None).await;
    lf.receive(&anfrage_44150(), 44150).await;
    lf.answer(body("ZG0"))
        .await
        .expect("the 44152 is dispatched");
    let (pid, wire) = lf.the_antwort().await;
    assert_eq!(pid, 44152);
    assert_contains(&wire, &["STS+E01++ZG0:G_0039", "RFF+TN:ANFRAGE-VG-1"]);
    assert_absent(&wire, &["DTM+92", "DTM+157", "SEQ+"]);
}

// ── 44139 / 44140 → 44142: Turnusablesung and Netznutzung ────────────────────

/// The MSB's 44140 asks for the Turnusablesung, the LF's 44139 for the
/// Netznutzung; marktd holds both now, and the 44142 states them — the
/// Turnusablesung for each Messlokation, „Der Termin der Turnusablesung wird
/// an der Marktlokation vorgegeben" (UTILMD MIG Gas G1.2 S. 60).
#[tokio::test]
async fn the_44142_states_turnusablesung_and_netznutzung() {
    let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    nb.receive(
        &anfrage(
            44140,
            MSB_MP,
            GNB_MP,
            "ZF4",
            serde_json::json!({ "sg6": [
                seg("RFF", &[&["Z19", support::GAS_MELO]]),
                seg("DTM", &[&["752", "0915", "106"]]),
            ]}),
        ),
        44140,
    )
    .await;
    nb.answer(melo_body("ZG2"))
        .await
        .expect("the 44142 is dispatched");
    let (pid, wire) = nb.the_antwort().await;
    assert_eq!(pid, 44142);
    assert_contains(
        &wire,
        &[
            // The Meldepunkt the 44140 named — `[953]` admits it on the 44142.
            &format!("LOC+172+{}", support::GAS_MELO),
            "STS+E01++ZG2:G_0069",
            &format!("RFF+Z19:{}", support::GAS_MELO),
            "DTM+752:05040504:104",
        ],
    );

    let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    nb.receive(
        &anfrage(
            44139,
            LF_MP,
            GNB_MP,
            "ZF4",
            serde_json::json!({ "bloecke": [{ "seq": "Z01", "segmente": [
                seg("CCI", &[&[], &[], &["ZB0"]]),
                seg("CCI", &[&[], &[], &["Z88"]]),
                seg("CAV", &[&["Z74", "", "", "Z09"]]),
                seg("CAV", &[&["Z73", "", "", "Z11"]]),
                seg("CAV", &[&["ZB1", "", "", "Z14"]]),
            ]}]}),
        ),
        44139,
    )
    .await;
    nb.answer(body("ZG2"))
        .await
        .expect("the 44142 is dispatched");
    let (_, wire) = nb.the_antwort().await;
    assert_contains(
        &wire,
        &[
            "STS+E01++ZG2:G_0031",
            "CCI+++Z88",
            "CAV+Z74:::Z08",
            "CAV+Z73:::Z10",
            "CAV+ZB1:::Z15",
        ],
    );
}

/// A Turnusablesung marktd does not hold is refused naming it.
#[tokio::test]
async fn a_turnusablesung_marktd_lacks_is_refused_by_name() {
    let mut record = support::gas_stammdaten(MALO, GNB_MP);
    if let Some(n) = record.netznutzung.as_mut() {
        n.turnusablesung = None;
    }
    let nb = deployment(false, Some(record)).await;
    nb.receive(
        &anfrage(
            44140,
            MSB_MP,
            GNB_MP,
            "ZF4",
            serde_json::json!({ "sg6": [
                seg("RFF", &[&["Z19", support::GAS_MELO]]),
                seg("DTM", &[&["752", "0915", "106"]]),
            ]}),
        ),
        44140,
    )
    .await;
    match nb.answer(melo_body("ZG2")).await {
        Err(DispatchError::StammdatenUnvollstaendig { fehlend, .. }) => {
            assert!(
                fehlend.iter().any(|f| f.contains("turnusablesung")),
                "{fehlend:?}"
            );
        }
        other => panic!("refused as incomplete Stammdaten, got {other:?}"),
    }
    assert!(nb.pending_utilmd().await.is_empty());
}

// ── 44162 → 44163 / 44164: not assembled ─────────────────────────────────────

/// The NB answers a 44162 as Verteiler after the MSB's 44167 to the 44165 it
/// forwards (UTILMD AHB Gas 1.2 Kap. 5.12.7, S. 206); that is not built, so
/// the command answers `501` naming the PID.
#[tokio::test]
async fn the_verteiler_antwort_is_not_implemented() {
    let nb = deployment(false, Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    nb.receive(
        &anfrage(
            44162,
            LF_MP,
            GNB_MP,
            "ZG8",
            serde_json::json!({ "sg12": [
                seg("NAD", &[
                    &["Z05"], &[], &[], &["Erika Muster", "", "", "", "", "Z01"],
                    &["Postweg 7"], &["Berlin"], &[], &["10115"], &["DE"],
                ]),
                seg("RFF", &[&["Z19", support::GAS_MELO]]),
            ]}),
        ),
        44162,
    )
    .await;
    let err = nb
        .answer(melo_body("ZG2"))
        .await
        .expect_err("not assembled");
    assert!(
        matches!(&err, DispatchError::NotImplemented(m) if m.contains("44163")),
        "{err:?}"
    );
}
