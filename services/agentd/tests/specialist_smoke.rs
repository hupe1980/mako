//! **Every specialist can actually run.**
//!
//! The golden run, the oversight suite and the regulatory suite all exercise
//! one specialist. The other twenty-seven were asserted only statically — the
//! manifest parses, the grants resolve, the schema exists. None of that proves
//! a run *completes*: a schema no answer can satisfy, a prompt the runtime
//! refuses to assemble, or a formation block that fails after the answer all
//! parse cleanly and die at first dispatch, in production, on the event that
//! needed them.
//!
//! This suite runs each specialist once on the production wiring
//! (`Plane::new`, real Cedar set, real manifests) with a `FakeProvider`
//! scripted to a **minimal schema-valid answer generated from the manifest's
//! own `output.schema`**. The fake is as strict as a real driver — it validates
//! scripted completions against the schema the runtime requests — so a schema
//! that contradicts itself, or one this generator cannot satisfy from its
//! declared types alone, fails here rather than in production.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

/// One inbound event's identity, as `POST /webhook` would have read it off the
/// CloudEvent envelope.
///
/// `source` is the producer half of the admission key: an id is unique only
/// within one emitter, so the key `run_correlated_once` claims carries both.
fn ce<'a>(id: &'a str, event_type: &'a str) -> Envelope<'a> {
    Envelope {
        id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type,
    }
}

/// Answers every read with an empty-but-shaped result.
#[derive(Debug, Default)]
struct StubTools {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ToolClient for StubTools {
    async fn call(
        &self,
        _tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!({ "items": [], "overdue": [], "count": 0 }))
    }
}

/// A minimal instance of a JSON Schema, from its declared types alone.
///
/// Deliberately naive: enum → first value, string → a date-shaped literal,
/// numbers → the minimum or zero, unions with null → null, objects → required
/// properties only. If a schema needs more than this to be satisfiable — a
/// pattern, a format the validator enforces, an implicit cross-field rule —
/// the specialist fails this suite, and that is a finding about the schema:
/// the model gets no more guidance than the schema carries.
fn instantiate(schema: &Value) -> Value {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values.first().cloned().unwrap_or(Value::Null);
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.clone(),
        // A union type: null is the cheapest member when present.
        Some(Value::Array(ts)) => {
            if ts.iter().any(|t| t == "null") {
                return Value::Null;
            }
            ts.first()
                .and_then(Value::as_str)
                .unwrap_or("object")
                .to_owned()
        }
        _ => "object".to_owned(),
    };
    match ty.as_str() {
        "string" => json!("2026-08-06"),
        "integer" | "number" => schema.get("minimum").cloned().unwrap_or(json!(0)),
        "boolean" => json!(false),
        "array" => json!([]),
        "null" => Value::Null,
        _ => {
            // Object: required properties only — every schema is closed
            // (`additionalProperties: false`), so less is safer than more.
            let mut out = serde_json::Map::new();
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let props = schema.get("properties").and_then(Value::as_object);
            for name in required {
                let sub = props
                    .and_then(|p| p.get(name))
                    .cloned()
                    .unwrap_or(json!({}));
                out.insert(name.to_owned(), instantiate(&sub));
            }
            Value::Object(out)
        }
    }
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

/// A payload with one of everything the labeller can promote, so correlation
/// and `$correlation/malo` memory subjects resolve for every specialist.
fn event() -> Value {
    json!({
        "malo_id": "51238696012",
        "melo_id": "DE0001234567890123456789012345678",
        "mp_id": "9900357000004",
        "pid": "13013",
        "process_id": "123e4567-e89b-12d3-a456-426614174000",
        "gas_day": "2026-08-06",
        "note": "smoke",
    })
}

/// Every specialist completes a run and returns a schema-valid answer.
#[tokio::test]
async fn every_specialist_completes_a_run() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");

    let tools = Arc::new(StubTools::default());
    let servers: Vec<(String, Arc<dyn ToolClient>)> =
        agentd::plane::tools::servers_named_in_grants()
            .into_iter()
            .map(|name| (name, Arc::clone(&tools) as Arc<dyn ToolClient>))
            .collect();

    let provider = FakeProvider::new();
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            owner: "agentd-smoke",
            tenant: &tenant,
            activated: &Activation::all(),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
        },
    )
    .expect("the plane assembles with every specialist");

    // The planned specialist is exercised end to end by the golden-run,
    // oversight and regulatory suites; its scripted plan does not fit the
    // generic loop below and duplicating it here would be a second copy.
    const COVERED_ELSEWHERE: &[&str] = &["gabi-gas-agent"];

    let mut failures = Vec::new();
    for (name, manifest) in agentd::plane::manifests() {
        if COVERED_ELSEWHERE.contains(&name.as_str()) {
            continue;
        }

        // Script an answer only for specialists that will ask a model for one.
        // The coded skill (`models: {}`) computes its answer in Rust: queueing
        // for it leaves a stale completion that the *next* specialist consumes,
        // and every answer after that arrives one agent late.
        let asks_a_model = manifest
            .spec
            .models
            .as_ref()
            .is_some_and(|m| m.privileged.is_some());
        if asks_a_model && let Some(schema) = manifest.output_schema() {
            provider.will_answer(completion(instantiate(schema)));
        }

        let decision = plane
            .dispatch_one(
                name,
                ce(&format!("ce-smoke-{name}"), "de.smoke.test"),
                event(),
            )
            .await
            .expect("every compiled specialist is activated");

        if decision.outcome != "completed" {
            failures.push(format!(
                "{name}: outcome `{}` — {}",
                decision.outcome, decision.summary
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "these specialists cannot complete a run on their own declarations:\n  {}",
        failures.join("\n  ")
    );
}
