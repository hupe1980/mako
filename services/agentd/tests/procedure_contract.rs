//! **What the model is actually asked.**
//!
//! Every other suite checks what a run *did*. This one checks what it was
//! *offered*, for every model-backed specialist, on the production wiring — and
//! it exists because that is where a declaration and a runtime drift apart
//! without anything failing.
//!
//! The manifest declares a procedure, a model, a tool surface, ceilings and an
//! answer schema. All five are supposed to reach the model exactly as written.
//! Nothing proved any of them past one specialist:
//!
//! * A **tool surface wider than the grants** is the failure this project spent
//!   a release inside: every specialist held its servers' whole read surface, and
//!   no test could see it because the runtime dutifully offered whatever the
//!   catalogue held. Now that the grants are narrow, the thing worth pinning is
//!   that the narrowing *reaches the model* — a runtime that offered the whole
//!   catalogue anyway would look identical from the outside.
//! * A **model other than the declared one** would make "moving a regulated
//!   decision onto a different model is a manifest edit" false, silently: the
//!   digest would be unchanged and the answer would come from somewhere else.
//! * A **procedure that does not reach the prompt** turns the digest-covered
//!   file into documentation. `plane_golden_run` asserts this for one specialist
//!   by grepping for two phrases; the other 25 were unchecked.
//! * A **schema not requested** means the closed answer shape is enforced by
//!   nothing at the one moment it could be.
//! * A **`max_output_tokens` above the manifest's ceiling** is a budget that
//!   reads as configured and bounds nothing.
//!
//! `FakeProvider::asked()` records the exact ask, so all five are deterministic
//! and free.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

fn ce<'a>(id: &'a str) -> Envelope<'a> {
    Envelope {
        id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type: "de.smoke.test",
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
/// The same generator `specialist_smoke` uses — the fake provider validates a
/// scripted completion against the requested schema, so an answer has to fit.
fn instantiate(schema: &Value) -> Value {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values.first().cloned().unwrap_or(Value::Null);
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.clone(),
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

/// A payload with one of everything the labeller promotes.
fn event() -> Value {
    json!({
        "malo_id": "51238696012",
        "melo_id": "DE0001234567890123456789012345678",
        "mp_id": "9900357000004",
        "pid": "13013",
        "process_id": "123e4567-e89b-12d3-a456-426614174000",
        "gas_day": "2026-08-06",
        "note": "contract",
    })
}

/// The tool names a manifest grants, **as the model sees them**.
///
/// agentplane renders `tool://server/name` to the model as `server__name` — the
/// injective spelling that is why a server key may not contain `-`. Comparing
/// bare tool names would pass while the runtime offered a different server's
/// tool of the same name, which is the collision that rendering exists to
/// prevent.
fn granted(manifest: &agentplane::manifest::Manifest) -> std::collections::BTreeSet<String> {
    manifest
        .spec
        .tools
        .iter()
        .filter_map(|g| agentplane::tools::ToolId::parse(&g.reference))
        .map(|id| format!("{}__{}", id.server, id.tool))
        .collect()
}

/// **Every model-backed specialist is asked exactly what its manifest declares.**
///
/// One dispatch each, on `Plane::new` with the real Cedar set and the real
/// manifests, then five assertions against the recorded ask.
#[tokio::test]
async fn the_ask_matches_the_manifest_for_every_specialist() {
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
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-contract",
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

    let mut findings = Vec::new();
    for (name, manifest) in agentd::plane::manifests() {
        let Some(models) = manifest.spec.models.as_ref() else {
            continue;
        };
        // `models: {}` — the coded skill asks nobody, which is its own contract
        // and is pinned by `deadline_triage.rs`.
        let Some(declared) = models.privileged.as_ref() else {
            continue;
        };
        let Some(schema) = manifest.output_schema() else {
            continue;
        };

        // Where this dispatch's asks begin. `asked()` accumulates across the
        // whole suite, and **the last ask is not this specialist's answer**: a
        // memory-forming specialist asks its *quarantined* model afterwards to
        // write the memory, so `.last()` reports the wrong model, the wrong
        // schema and no tools at all. Seven specialists form memories, and every
        // one of them would have failed on the runtime doing exactly what it
        // was told to.
        let before = provider.asked().len();

        provider.will_answer(completion(instantiate(schema)));
        let decision = plane
            .dispatch_one(name, ce(&format!("ce-contract-{name}")), event())
            .await
            .expect("every compiled specialist is activated");
        assert_eq!(
            decision.outcome, "completed",
            "{name}: {}",
            decision.summary
        );

        let asks = provider.asked();
        let Some(ask) = asks.get(before) else {
            findings.push(format!("{name}: the model was never asked"));
            continue;
        };

        // 1. The model that answered is the model the manifest names.
        if ask.model.provider != declared.provider || ask.model.model != declared.model {
            findings.push(format!(
                "{name}: manifest declares {}/{} and the runtime asked {}/{}",
                declared.provider, declared.model, ask.model.provider, ask.model.model
            ));
        }

        // 2. The tool surface offered is exactly the grants — the runtime-side
        //    proof that narrowing the manifests actually narrowed what a model
        //    sees, rather than only what a guard reads.
        let offered: std::collections::BTreeSet<String> =
            ask.tools.iter().map(|t| t.name.clone()).collect();
        let want = granted(manifest);
        let extra: Vec<&String> = offered.difference(&want).collect();
        let absent: Vec<&String> = want.difference(&offered).collect();
        if !extra.is_empty() {
            findings.push(format!(
                "{name}: offered {} tool(s) the manifest does not grant: {extra:?}",
                extra.len()
            ));
        }
        if !absent.is_empty() {
            findings.push(format!(
                "{name}: granted {} tool(s) the model was never offered: {absent:?}",
                absent.len()
            ));
        }

        // 3. The answer schema is requested, so the closed shape is enforced at
        //    the one moment it can be.
        match ask.schema.as_ref() {
            Some(asked) if asked == schema => {}
            Some(_) => findings.push(format!(
                "{name}: the schema requested is not the manifest's `output.schema`"
            )),
            None => findings.push(format!(
                "{name}: no schema was requested — the answer shape is enforced by nothing"
            )),
        }

        // 4. The procedure reaches the prompt, verbatim. Checked on the
        //    manifest's own text rather than on phrases a test author picked,
        //    so it cannot pass on a prompt assembled from something else.
        let procedure = manifest
            .spec
            .identity
            .as_ref()
            .map(agentplane::manifest::Identity::system_prompt)
            .unwrap_or_default();
        let rendered = ask.prompt.to_string();
        // One distinctive line: the whole block is re-wrapped by the serializer,
        // so a substring of the *rendered* prompt is what a subset test can use.
        let marker = procedure
            .lines()
            .find(|l| {
                l.trim_start().starts_with("## STEP-BY-STEP")
                    || l.trim_start().starts_with("## PROCEDURE")
            })
            .map(str::trim)
            .unwrap_or("## TRIGGERED BY");
        if !rendered.contains(marker) {
            findings.push(format!(
                "{name}: the manifest's procedure does not reach the prompt (`{marker}` absent)"
            ));
        }

        // 5. The turn cannot be asked for more output than the manifest allows.
        if let Some(ceiling) = manifest.spec.budgets.as_ref().and_then(|b| b.max_tokens)
            && u64::from(ask.max_output_tokens) > ceiling
        {
            findings.push(format!(
                "{name}: asked for {} output tokens against a declared ceiling of {ceiling}",
                ask.max_output_tokens
            ));
        }
    }

    assert!(
        findings.is_empty(),
        "the ask and the manifest disagree for these specialists:\n  {}",
        findings.join("\n  ")
    );
}
