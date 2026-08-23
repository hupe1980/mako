//! **What the journal is worth as evidence, asserted against a real store.**
//!
//! `signing` compiles whether or not anything calls `signing_as`, and an
//! unsigned journal passes a signature test cleanly unless the verifier is asked
//! to *require* one. So this file builds the plane the way `main.rs` builds it,
//! runs a specialist end to end, and verifies the records the way an auditor
//! would: with a public key, and with `require_signature` **on**.
//!
//! The three questions, and why each needs a test rather than a sentence:
//!
//! | Question | What its absence would hide |
//! |---|---|
//! | Are records signed by the configured key? | A feature flag that is enabled and wired to nothing |
//! | Does a *different* key reject them? | A verifier that accepts anything, which reads identically to one that works |
//! | Is an unattested plane honestly unattested? | An auditor unable to tell "not signed" from "signed and I lack the key" |

use std::sync::Arc;

use agentplane::core::Digest;
use agentplane::journal::Record;
use agentplane::model::{Completion, Usage};
use agentplane::policy::{Ed25519Signer, Ed25519Verifier};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

const AGENT: &str = "gabi-gas-agent";
const EVENT_TYPE: &str = "de.gabi.imbalance.notified";
const TENANT: &str = "9900357000004";
const KEY_ID: &str = "spiffe://mako/agentd";

fn ce<'a>(id: &'a str) -> Envelope<'a> {
    Envelope {
        id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type: EVENT_TYPE,
    }
}

#[derive(Debug, Default)]
struct Tools;

#[async_trait::async_trait]
impl ToolClient for Tools {
    async fn call(
        &self,
        _tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        Ok(json!({ "items": [] }))
    }
}

/// The plane, wired as `main.rs` wires it — origin and signer on the **store**.
///
/// That placement is the whole point of the test. A signer given only to
/// `RuntimeBuilder` signs the plane's outward claims and leaves every record
/// unsigned, and the two are one method name apart.
fn plane(provider: &Arc<FakeProvider>, signer: Option<Arc<Ed25519Signer>>) -> Plane {
    let tenant = agentplane::core::TenantId::new(TENANT).expect("a usable key scope");
    let mut store = RedbStore::open_in_memory()
        .expect("store")
        .origin(agentd::plane::attest::ORIGIN);
    let record_signer = signer
        .as_ref()
        .map(|s| Arc::clone(s) as Arc<dyn agentplane::core::Signer>);
    if let Some(s) = record_signer.clone() {
        store = store.signing_as(s);
    }

    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let servers = ["makod", "marktd", "netzbilanzd"]
        .into_iter()
        .map(|name| (name.to_owned(), Arc::new(Tools) as Arc<dyn ToolClient>))
        .collect();

    Plane::new(
        Stores::redb(store, &tenant),
        PlaneConfig {
            owner: "agentd-test",
            tenant: &tenant,
            activated: &Activation::named(vec![AGENT.to_owned()]),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
            outbox: None,
            signer: record_signer,
            quota: agentplane::quota::TenantQuota::default(),
        },
    )
    .expect("the plane assembles")
}

fn event() -> Value {
    json!({
        "malo_id": "51238696012",
        "bilanzkreis_id": "THE0BFH012345678",
        "gas_day": "2026-08-06",
    })
}

fn completion(structured: Value) -> Completion {
    Completion {
        text: String::new(),
        structured: Some(structured),
        tool_calls: Vec::new(),
        usage: Usage::default(),
        stop_reason: Some("end_turn".to_owned()),
        truncated: false,
        continuation: None,
    }
}

fn script(provider: &FakeProvider) {
    let imbalance = ToolId::new("netzbilanzd", "get_gas_imbalance").wire_name();
    provider.will_answer(completion(json!({
        "steps": [
            { "tool": imbalance, "args": {
                "bilanzkreis": "$input/bilanzkreis_id",
                "gas_day": "$input/gas_day"
            }},
            { "parse": { "from": "$step0", "schema": {
                "type": "object",
                "required": [
                    "gas_day", "imbalance_status", "allocation_version",
                    "deadline_compliant", "action", "legal_basis"
                ],
                "properties": {
                    "gas_day":            { "type": "string" },
                    "imbalance_status":   { "type": "string" },
                    "allocation_version": { "type": "string" },
                    "deadline_compliant": { "type": "boolean" },
                    "action":             { "type": "string" },
                    "legal_basis":        { "type": "string" }
                }
            }}}
        ],
        "answer": "$step1"
    })));
    provider.will_answer(completion(json!({
        "gas_day": "2026-08-06",
        "imbalance_status": "MINDER",
        "allocation_version": "Initial",
        "deadline_compliant": true,
        "action": "REQUEST_CORRECTION",
        "legal_basis": "KoV §6.4 Abs. 3",
        "have_enough_information": true
    })));
}

/// Every record one run wrote, in order.
async fn records_of(plane: &Plane, run_id: &str) -> Vec<Record> {
    let run = agentplane::core::RunId::parse(run_id).expect("a run id");
    plane
        .runtime()
        .journal()
        .read(run, 0)
        .await
        .expect("the run's records are readable")
}

// ── The question the service could not answer ─────────────────────────────

/// **Records are signed, and an auditor with the public key can prove it.**
///
/// `require_signature: true` is the parameter that matters: with it `false` an
/// unsigned journal passes this test cleanly, which is exactly how the gap
/// survived. The verifier holds only the public half, as an auditor would.
#[tokio::test]
async fn an_attested_plane_writes_records_an_auditor_can_verify() {
    let seed = [7_u8; 32];
    let signer = Arc::new(Ed25519Signer::new(KEY_ID, &seed));
    let public = signer.verifying_key();

    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider, Some(Arc::clone(&signer)));

    let decision = plane
        .dispatch_one(AGENT, ce("ce-attested"), event())
        .await
        .expect("dispatched");
    let records = records_of(&plane, &decision.run_id).await;
    assert!(
        !records.is_empty(),
        "a run that produced no records proves nothing either way"
    );
    assert!(
        records.iter().all(|r| r.attestation.is_some()),
        "a record with no attestation: signing is wired on the runtime and not on the store"
    );

    let verifier = Ed25519Verifier::new()
        .trust(KEY_ID, &public)
        .expect("a valid public key");
    Record::verify_attested(&records, Digest::ZERO, &verifier, true)
        .expect("every record must verify against the workload's published key");
}

/// **A different key rejects them.**
///
/// Without this the test above passes against a verifier that accepts anything,
/// and the two read identically in a review.
#[tokio::test]
async fn a_record_does_not_verify_under_a_key_that_did_not_write_it() {
    let signer = Arc::new(Ed25519Signer::new(KEY_ID, &[7_u8; 32]));
    let impostor = Ed25519Signer::new(KEY_ID, &[8_u8; 32]);

    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider, Some(signer));

    let decision = plane
        .dispatch_one(AGENT, ce("ce-impostor"), event())
        .await
        .expect("dispatched");
    let records = records_of(&plane, &decision.run_id).await;

    // Same key *id*, different key material — the case a key-id check alone
    // would wave through, and the one an attacker would produce.
    let verifier = Ed25519Verifier::new()
        .trust(KEY_ID, &impostor.verifying_key())
        .expect("a valid public key");
    Record::verify_attested(&records, Digest::ZERO, &verifier, true)
        .expect_err("a signature from another key must not verify");
}

/// **An unattested plane is honestly unattested.**
///
/// It still runs — that is the documented relaxation, and it warns at startup.
/// What it must not do is present as attested: an auditor has to be able to
/// tell *not signed* from *signed and I do not hold the key*, and
/// `require_signature` is where that distinction lives.
#[tokio::test]
async fn an_unattested_plane_fails_a_verification_that_requires_signatures() {
    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider, None);

    let decision = plane
        .dispatch_one(AGENT, ce("ce-unattested"), event())
        .await
        .expect("dispatched");
    let records = records_of(&plane, &decision.run_id).await;

    assert!(
        records.iter().all(|r| r.attestation.is_none()),
        "an unconfigured plane must not mint an identity of its own"
    );
    let verifier = Ed25519Verifier::new();
    Record::verify_attested(&records, Digest::ZERO, &verifier, true)
        .expect_err("unsigned records must fail a verification that requires signatures");
    // The chain is still sound, which is the honest statement of what an
    // unattested plane gives you: tamper-evidence without attribution.
    Record::verify_attested(&records, Digest::ZERO, &verifier, false)
        .expect("the hash chain holds whether or not anybody signed it");
}

/// **The checkpoint names mako's log, not the crate's default.**
///
/// The checkpoint is the one artifact that has to leave the operator's control —
/// handed to an auditor, posted to a witness — and it arrives under whatever
/// name the store was given. Unset, that is `agentplane/<tenant>`, which says
/// nothing about whose log it is. A witness additionally holds every later
/// checkpoint to the first one it saw under a name, so this value can never be
/// changed once submitted: it is a constant in mako's source for that reason,
/// and this pins the composition with the tenant.
#[tokio::test]
async fn the_checkpoint_carries_makos_own_origin() {
    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider, None);

    let checkpoint = plane
        .runtime()
        .journal()
        .checkpoint()
        .await
        .expect("a checkpoint");
    assert_eq!(
        checkpoint.origin,
        format!("{}/{TENANT}", agentd::plane::attest::ORIGIN),
        "the store appends the tenant to the origin it was given"
    );
    assert!(
        agentplane::journal::SignedNote::new(checkpoint.to_note()).is_ok(),
        "the checkpoint must serialise as a signed-note body a witness can read"
    );
}

/// One key, one identity, on both seams.
///
/// The store signs *records*; the runtime signs the plane's *outward claims* to
/// a tool server. They are separate settings upstream, and mako gives both the
/// same key deliberately — so an auditor reading a record and a server reading a
/// provenance block see one workload rather than two that have to be reconciled.
#[test]
fn the_record_signer_and_the_claim_signer_are_one_identity() {
    let signer = Ed25519Signer::new(KEY_ID, &[7_u8; 32]);
    assert_eq!(agentplane::core::Signer::key_id(&signer).as_str(), KEY_ID);
    assert_eq!(
        <Ed25519Signer as agentplane::core::CheckpointSigner>::key_id(&signer).as_str(),
        KEY_ID,
        "the checkpoint a witness cosigns is signed under the identity on the records"
    );
}

// ── What a receiver of `de.agent.decision.made` is told ───────────────────

/// **A failed run's delivery says *why*.**
///
/// This is mako's own upstream report, pinned in mako's tree.
/// `RecordKind::RunSealed` gained a `reason` so that why a run failed outlives
/// the process that wrote it — and the projection that turns a sealed record
/// into `de.agent.decision.made` destructured it away behind `..`, so a receiver
/// got `outcome: "failed"` and nothing else: the exact state the field was added
/// to end, one layer further out. agentplane 0.22 carries it, and destructures
/// every field of the seal so the next field added has to ask deliver-or-not at
/// the build.
///
/// It is asserted here rather than trusted because the failure mode is silence:
/// a delivery with no `reason` is a well-formed CloudEvent that verifies, parses
/// and tells an operator nothing. Nothing upstream or downstream would go red.
#[tokio::test]
async fn a_failed_runs_delivery_carries_the_reason_it_failed() {
    use agentplane::journal::RecordKind;
    use agentplane::push::{Projection as _, RunCompleted};

    // No scripted answers: the privileged call has nothing to return, so the
    // run fails with a reason rather than an empty conclusion.
    let provider = FakeProvider::new();
    let plane = plane(&provider, None);

    let decision = plane
        .dispatch_one(AGENT, ce("ce-failed"), event())
        .await
        .expect("dispatched");
    assert_eq!(decision.outcome, "failed", "{decision:#?}");

    let sealed = records_of(&plane, &decision.run_id)
        .await
        .into_iter()
        .find(|r| matches!(r.kind(), RecordKind::RunSealed { .. }))
        .expect("a concluded run seals");

    let projection = RunCompleted::new("urn:mako:test:agentd").event_type("de.agent.decision.made");
    let messages = projection
        .messages(&sealed)
        .await
        .expect("the seal projects to a delivery");
    let data = &messages
        .first()
        .expect("one message per sealed run")
        .payload["data"];

    assert_eq!(data["outcome"], "failed");
    let reason = data["reason"]
        .as_str()
        .expect("a failed run's delivery must carry the reason the record holds");
    assert!(
        !reason.trim().is_empty(),
        "an empty reason reads as \"the agent said nothing\", which is the state this \
         field exists to end"
    );
    // The chain head is what a receiver hands back to ask this plane to prove
    // the run it was told about.
    assert!(data["chain_head"].is_string());
}

/// A successful run carries **no** `reason` key at all.
///
/// Absent, not `null`. A `null` there reads as a failure with no explanation,
/// which is worse than either true state — and a receiver written against
/// `if (data.reason)` would be right to treat it as one.
#[tokio::test]
async fn a_successful_runs_delivery_carries_no_reason_field() {
    use agentplane::journal::RecordKind;
    use agentplane::push::{Projection as _, RunCompleted};

    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider, None);

    let decision = plane
        .dispatch_one(AGENT, ce("ce-ok"), event())
        .await
        .expect("dispatched");
    assert_eq!(decision.outcome, "completed", "{decision:#?}");

    let sealed = records_of(&plane, &decision.run_id)
        .await
        .into_iter()
        .find(|r| matches!(r.kind(), RecordKind::RunSealed { .. }))
        .expect("a concluded run seals");

    let projection = RunCompleted::new("urn:mako:test:agentd").event_type("de.agent.decision.made");
    let messages = projection.messages(&sealed).await.expect("projects");
    let data = &messages.first().expect("one message").payload["data"];

    assert_eq!(data["outcome"], "succeeded", "{data:#}");
    assert!(
        data.get("reason").is_none(),
        "a success must omit `reason` rather than send null: {data:#}"
    );
}

/// **The run's answer does not travel.**
///
/// `de.agent.decision.made` carries the *conclusion*, not the output. The output
/// is domain data with a label on it, and shipping it by default would make an
/// egress decision nobody declared: whatever the run happened to hold, to
/// whatever destination the operator happened to configure, under no ceiling.
/// The reasoning lives behind `/api/v1/oversight/runs/{run}`, which is
/// authenticated and authorized.
#[tokio::test]
async fn a_delivery_carries_the_conclusion_and_not_the_answer() {
    use agentplane::journal::RecordKind;
    use agentplane::push::{Projection as _, RunCompleted};

    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider, None);

    let decision = plane
        .dispatch_one(AGENT, ce("ce-egress"), event())
        .await
        .expect("dispatched");
    let sealed = records_of(&plane, &decision.run_id)
        .await
        .into_iter()
        .find(|r| matches!(r.kind(), RecordKind::RunSealed { .. }))
        .expect("seals");

    let projection = RunCompleted::new("urn:mako:test:agentd").event_type("de.agent.decision.made");
    let body = projection.messages(&sealed).await.expect("projects")[0]
        .payload
        .to_string();

    for leaked in ["REQUEST_CORRECTION", "MINDER", "51238696012"] {
        assert!(
            !body.contains(leaked),
            "the delivery leaked `{leaked}` from the run's answer: {body}"
        );
    }
}
