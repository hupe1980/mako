//! **A row a model wrote says so where the operator reads it, and only there.**
//!
//! Verordnung (EU) 2024/1689 Art. 50 — see `plane::MACHINE_GENERATED`. The
//! `plane::` unit test holds the manifests' triage sentences; this suite holds
//! the **served** text, the `Hinweis.text` `Plane::hinweise` reads back from
//! the task store behind `GET /api/v1/hinweise`. A disclosure that is right in
//! the manifest and lost on the way to the store — a summary assembled at
//! runtime, a rewrite on open — is invisible to the manifest check and is what
//! the operator actually sees.
//!
//! Model direction, over the catalog: every triage rule of every model-backed
//! specialist is made to fire by a `FakeProvider` answer built from the
//! manifest's own `output.schema` with the rule's condition placed into it, and
//! the row it files is read back through `Plane::hinweise`. The coded direction
//! is held on the served text by `deadline_triage` and `oversight`, and over
//! the catalog by `skills::tests`.

use std::sync::Arc;

use agentplane::manifest::{Predicate, TriageRule};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::ToolClient;
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, MACHINE_GENERATED, Plane, PlaneConfig, Stores};

use super::specialist_smoke::{StubTools, completion, event, instantiate};

fn plane(agent: &str, provider: &Arc<FakeProvider>) -> Plane {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let tools = Arc::new(StubTools::default());
    let servers: Vec<(String, Arc<dyn ToolClient>)> =
        agentd::plane::tools::servers_named_in_grants()
            .into_iter()
            .map(|name| (name, Arc::clone(&tools) as Arc<dyn ToolClient>))
            .collect();
    Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-disclosure",
            tenant: &tenant,
            activated: &Activation::named(vec![agent.to_owned()]),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
        },
    )
    .expect("the plane assembles")
}

/// The value a condition is satisfied by.
fn satisfying(predicate: &Predicate) -> Value {
    match predicate {
        Predicate::Equals(v) => v.clone(),
        Predicate::OneOf(vs) => vs
            .first()
            .cloned()
            .expect("the parser refuses an empty `in`"),
        Predicate::AtLeast(n) | Predicate::AtMost(n) => json!(n),
        Predicate::Exists(_) => json!(true),
    }
}

/// The sub-schema of an object schema's property.
fn property(schema: &Value, name: &str) -> Value {
    schema
        .get("properties")
        .and_then(|p| p.get(name))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// `answer` with `value` at the JSON pointer `path`, every missing parent
/// instantiated from `schema` as an object.
fn place(answer: &mut Value, schema: &Value, path: &str, value: Value) {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (last, parents) = segments.split_last().expect("a pointer names a field");
    let mut here = answer;
    let mut sub = schema.clone();
    for segment in parents {
        sub = property(&sub, segment);
        let obj = here.as_object_mut().expect("a parent is an object");
        let slot = obj.entry((*segment).to_owned()).or_insert(Value::Null);
        if !slot.is_object() {
            let mut as_object = sub.clone();
            as_object["type"] = json!("object");
            *slot = instantiate(&as_object);
        }
        here = slot;
    }
    here.as_object_mut()
        .expect("the field's parent is an object")
        .insert((*last).to_owned(), value);
}

/// An answer the rule matches, valid against the manifest's schema.
fn answer_firing(schema: &Value, rule: &TriageRule) -> Value {
    let mut answer = instantiate(schema);
    for condition in &rule.when {
        place(
            &mut answer,
            schema,
            &condition.path,
            satisfying(&condition.predicate),
        );
    }
    assert!(rule.matches(&answer), "{}: {answer}", rule.name);
    answer
}

/// Every row a model-backed specialist's triage rule files is served opening
/// with the disclosure.
#[tokio::test]
async fn every_model_written_hinweis_opens_with_the_disclosure() {
    let mut checked = 0usize;
    let mut undisclosed = Vec::new();
    for (name, manifest) in agentd::plane::manifests() {
        if manifest.spec.execution.is_none() {
            continue;
        }
        let Some(schema) = manifest.output_schema() else {
            continue;
        };
        let rules = manifest
            .spec
            .oversight
            .as_ref()
            .map(|o| o.triage.as_slice())
            .unwrap_or_default();
        for rule in rules {
            let provider = FakeProvider::new();
            let plane = plane(name, &provider);
            provider.will_answer(completion(answer_firing(schema, rule)));

            let decision = plane
                .dispatch_one(
                    name,
                    Envelope {
                        id: &format!("ce-disclosure-{name}-{}", rule.name),
                        source: "urn:mako:test:tenant:9900357000004",
                        event_type: "de.disclosure.test",
                    },
                    event(),
                )
                .await
                .expect("the specialist is activated");
            assert_eq!(
                decision.outcome, "completed",
                "{name}/{}: {}",
                rule.name, decision.summary
            );

            let mut roles = rule.audience.clone();
            roles.push("mako-operations".to_owned());
            let hinweise = plane
                .hinweise(&roles)
                .await
                .expect("the worklist is readable");
            let kind = rule.task_kind();
            let row = hinweise.iter().find(|h| h.art == kind).unwrap_or_else(|| {
                panic!("{name}/{}: no row served, got {hinweise:#?}", rule.name)
            });
            checked += 1;
            if !row.text.starts_with(MACHINE_GENERATED) {
                undisclosed.push(format!("{name}/{}: {}", rule.name, row.text));
            }
        }
    }
    assert!(
        checked > 10,
        "only {checked} model-written rows were served — the catalog or the shape changed \
         and this is looking at nothing"
    );
    assert!(
        undisclosed.is_empty(),
        "these rows a model wrote reach the operator without saying so:\n  {}",
        undisclosed.join("\n  ")
    );
}
