//! A shadow restart must consume pending effects locally, even with live endpoints configured.
use std::io::{ErrorKind, Write as _};
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use edi_energy::EdiEnergyMessage as _;
use mako_engine::deadline::{Deadline, DeadlineStore as _};
use mako_engine::ids::OutboxMessageId;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};
use serde_json::{Value, json};
use time::OffsetDateTime;

const RAW: &[u8] =
    include_bytes!("../../../../makotest/tests/fixtures/shadow/mirror/inbound/first.capture");
const EXPECTED: &str = include_str!("../../../../makotest/tests/fixtures/shadow/expected.json");
const COVERAGE: &str =
    include_str!("../../../../makotest/tests/fixtures/shadow/reports/complete.json");
const CASE: &str = include_str!("../../../../makotest/tests/fixtures/shadow/case.json");

struct Scenario {
    workspace: tempfile::TempDir,
    sink: TcpListener,
}

impl Scenario {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(workspace.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let sink = TcpListener::bind("127.0.0.1:0").unwrap();
        sink.set_nonblocking(true).unwrap();
        Self { workspace, sink }
    }

    fn invoke(&self, action: &str, input: Option<&Value>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_makod"));
        cmd.arg("shadow")
            .arg("--workspace")
            .arg(self.workspace.path())
            .args(["--nb", "9900357000003", action])
            .env("MAKOD_WORKER_THREADS", "invalid-live-setting")
            .env("MAKOD_CONFIG", self.workspace.path().join("absent.toml"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in [
            "MAKOD_OTEL_ENDPOINT",
            "MAKOD_S3_ENDPOINT",
            "MAKOD_VERZEICHNISDIENST_URL",
            "MAKOD_ERP_WEBHOOK_URL",
            "MAKOD_EDIFACT_OUTBOX_WEBHOOK_URL",
            "MAKOD_NETZZUGANG_ENDPOINT_URL",
            "MAKOD_MARKTD_URL",
        ] {
            cmd.env(var, format!("http://{}", self.sink.local_addr().unwrap()));
        }
        if action == "prepare" {
            cmd.arg("--scope").arg(
                serde_json::from_str::<Value>(COVERAGE).unwrap()["scope_fingerprint"]
                    .as_str()
                    .unwrap(),
            );
        }
        let mut child = cmd.spawn().unwrap();
        if let Some(value) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(value.to_string().as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }

    fn ok(&self, action: &str, input: Option<&Value>) -> Vec<u8> {
        let result = self.invoke(action, input);
        assert!(
            result.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    }

    async fn store(&self) -> SlateDbStore {
        let local = object_store::local::LocalFileSystem::new_with_prefix(
            self.workspace.path().join("occurrence"),
        )
        .unwrap();
        SlateDbStore::open("shadow-v1", Arc::new(local))
            .await
            .unwrap()
    }
}

fn packet() -> Value {
    let expected: Value = serde_json::from_str(EXPECTED).unwrap();
    let case: Value = serde_json::from_str(CASE).unwrap();
    let coverage: Value = serde_json::from_str(COVERAGE).unwrap();
    json!({"version":1,"wire":STANDARD.encode(RAW),"malo":case["malo"],"nb":case["nb"],
        "received_at":case["received_at"],"grid_member":case["grid_member"],"supply_status":case["supply_status"],
        "request_sha256":expected["request_sha256"],"decision":{"pid":expected["pid"],"tree":expected["tree"],"code":expected["code"]},
        "scope":coverage["scope_fingerprint"],"coverage":coverage["fingerprints"],"occurrence":case["occurrence"],
        "role":case["role"],"direction":"inbound","batch":expected["batch"],"revision":expected["revision"],
        "facts":"a".repeat(64),"ebd_revision":"4.3","pruefschritt":30})
}

async fn seed_open_deadline(store: &SlateDbStore, tenant: mako_engine::ids::TenantId) -> Deadline {
    use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    );
    let request =
        include_bytes!("../../../../makotest/tests/fixtures/shadow/mirror/inbound/second.capture");
    let outcome = dispatcher
        .dispatch(
            &edi_energy::parse(request).unwrap(),
            "gpke-supplier-change",
            55001,
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    let IngestOutcome::Spawned { process_id, .. } = outcome else {
        panic!("expected second Anmeldung")
    };
    let initiated = store
        .pending_now(32)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.process_id == process_id)
        .unwrap();
    let deadlines = store.as_deadline_store();
    let windows = deadlines.for_stream(&initiated.stream_id).await.unwrap();
    let window = windows
        .iter()
        .find(|d| d.label() == mako_gpke::GPKE_PROCESS_RESPONSE_LABEL)
        .unwrap();
    let due = Deadline::new(
        initiated.stream_id,
        process_id,
        tenant,
        window.workflow_id().clone(),
        window.label(),
        OffsetDateTime::UNIX_EPOCH,
    );
    deadlines.register(&due).await.unwrap();
    due
}

#[tokio::test]
async fn isolated_workers_capture_retries_deadlines_and_restarts() {
    let s = Scenario::new();
    assert_eq!(
        serde_json::from_slice::<Value>(&s.ok("check", None)).unwrap()["outbound"],
        "capture"
    );
    assert_eq!(std::fs::read_dir(s.workspace.path()).unwrap().count(), 0);
    s.ok("prepare", Some(&packet()));
    s.ok("prepare", Some(&packet()));
    let store = s.store().await;
    let live = SlateDbStore::open_local(s.workspace.path().join("occurrence"))
        .await
        .unwrap();
    assert!(live.pending_now(32).await.unwrap().is_empty());
    live.close().await.unwrap();
    let pending = store.pending_now(32).await.unwrap();
    let reply = pending
        .iter()
        .find(|m| m.message_type.as_ref() == "UTILMD")
        .expect("business answer")
        .clone();
    for channel in ["MaloIdentCallback", "NetzzugangAntrag"] {
        let mut intent = reply.clone();
        intent.message_id = OutboxMessageId::new();
        intent.message_type = channel.into();
        store.enqueue(&[intent]).await.unwrap();
    }
    let due = seed_open_deadline(&store, reply.tenant_id).await;
    store
        .reschedule(reply.message_id, OffsetDateTime::UNIX_EPOCH)
        .await
        .unwrap();
    store.close().await.unwrap();
    let report: Value = serde_json::from_slice(&s.ok("run", None)).unwrap();
    assert_eq!(report["pending"], 0);
    assert_eq!(report["captured"]["callback"], 2);
    assert!(report["captured"]["erp"].as_u64().unwrap_or_default() > 0);
    let answer = s.ok("answer", None);
    let interchange = edi_energy::Platform::with_all_profiles()
        .parse_interchange_full(&answer)
        .unwrap();
    let msg = &interchange.messages[0].message;
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 55003);
    let edi_energy::AnyMessage::Utilmd(utilmd) = msg else {
        panic!("expected UTILMD")
    };
    let tx = &utilmd.transactions()[0];
    let status = tx.antwort().unwrap();
    assert_eq!(
        (status.code.as_str(), status.codeliste.as_deref()),
        ("A02", Some("E_0622"))
    );
    assert!(
        tx.references
            .iter()
            .any(|r| r.qualifier == "TN" && r.reference.as_deref() == Some("VORGANG-1"))
    );
    let store = s.store().await;
    assert!(
        !store
            .as_deadline_store()
            .for_stream(due.stream_id())
            .await
            .unwrap()
            .iter()
            .any(|d| d.deadline_id() == due.deadline_id())
    );
    // The record exists but the queue still contains the reply if the process dies before acknowledge.
    store.enqueue(&[reply]).await.unwrap();
    let captures = KvNamespace::new("shadow/captures/");
    let first = store.kv_scan_prefix(captures).await.unwrap();
    assert!(
        first
            .iter()
            .map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).unwrap())
            .any(|record| record["type"] == mako_engine::erp::LAPSE_NOTICE
                && record["effect"]["kind"] == "erp"),
        "a missed business deadline must produce a locally captured ERP notice"
    );
    store.close().await.unwrap();
    s.ok("run", None);
    assert_eq!(s.ok("answer", None), answer);
    let store = s.store().await;
    assert_eq!(store.kv_scan_prefix(captures).await.unwrap(), first);
    assert!(store.list_dead_letters(1).await.unwrap().is_empty());
    store.close().await.unwrap();
    assert_eq!(s.sink.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
}

#[test]
fn refused_inputs_cannot_replace_a_prepared_occurrence() {
    let s = Scenario::new();
    let mut changed = packet();
    changed["request_sha256"] = Value::String("0".repeat(64));
    assert!(!s.invoke("prepare", Some(&changed)).status.success());
    assert!(!s.workspace.path().join("occurrence").exists());
    for (field, value) in [
        ("role", "LF"),
        ("direction", "outbound"),
        ("supply_status", "Aktiv"),
        ("nb", "4012345000023"),
    ] {
        changed = packet();
        changed[field] = Value::String(value.into());
        assert!(!s.invoke("prepare", Some(&changed)).status.success());
        assert!(!s.workspace.path().join("occurrence").exists());
    }
    s.ok("prepare", Some(&packet()));
    changed = packet();
    changed["scope"] = Value::String("0".repeat(64));
    assert!(!s.invoke("prepare", Some(&changed)).status.success());
}
