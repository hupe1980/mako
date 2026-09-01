//! agentplane runtime integration.
//!
//! Specialists are declarative manifests run by
//! [`agentplane::runtime::Runtime`], so every model call and tool call is a
//! journaled effect rather than a log line.
//!
//! What lives where, and why:
//!
//! * **The manifest** (`agents/*.yaml`) owns the prompt, the model pair, the
//!   tool grants, the ceilings and the result schema. It is digest-covered, so
//!   editing a procedure is a version bump a reviewer sees.
//! * **The runtime** owns the shape of a declarative turn — which calls happen
//!   in which order — reading the prompt and the model pair out of the manifest
//!   rather than out of Rust, so no second copy can disagree with the file about
//!   what the agent is. A specialist whose conduct *is* Rust lives in
//!   [`crate::skills`] and states that by declaring `models: {}`.
//!
//! The boundary this keeps: an agent may prepare and may wait, the
//! deterministic engine still dispatches.

pub mod attest;
pub mod calendar;
pub mod keys;
pub mod label;
pub mod metrics;
pub mod oversight;
pub mod policy;
pub mod providers;
pub mod readiness;
pub mod runtime;
pub mod sweep;
pub mod tools;
pub mod witness;
pub use runtime::{
    Accepted, Activation, Admitted, AgentDecision, Envelope, Plane, PlaneConfig, Reception, Route,
    Router, Stores,
};

use std::collections::BTreeMap;

use agentplane::manifest::Manifest;

/// Every specialist manifest, embedded at compile time and keyed by the name the
/// document declares.
///
/// `agentplane::manifests!` is `include_str!` per path handed to
/// `Manifest::parse_each`, with the path literal kept as the origin for
/// diagnostics. There is no glob, deliberately: a macro expanding a directory
/// listing would make the set of agents a plane runs depend on what is on disk
/// rather than on what a reviewer reads.
///
/// The name is read from the document rather than typed beside the path: one
/// fact in one place, and the macro refuses a duplicate name (naming both
/// paths) rather than registering one agent twice while another goes absent.
///
/// # Panics
///
/// If a manifest fails to parse or two files declare one agent. Both are
/// build-time artefacts failing their own schema, and there is no runtime state
/// in which continuing is better than stopping.
pub fn manifests() -> &'static BTreeMap<String, Manifest> {
    static PARSED: std::sync::OnceLock<BTreeMap<String, Manifest>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        agentplane::manifests![
            "../../agents/billing-agent.yaml",
            "../../agents/billing-anomaly-agent.yaml",
            "../../agents/billing-regulatory-guard-agent.yaml",
            "../../agents/compliance-agent.yaml",
            "../../agents/deadline-alert-agent.yaml",
            "../../agents/eeg-agent.yaml",
            "../../agents/eeg-compliance-agent.yaml",
            "../../agents/einsd-batch-agent.yaml",
            "../../agents/gabi-gas-agent.yaml",
            "../../agents/grid-anomaly-agent.yaml",
            "../../agents/invoice-reconciliation-agent.yaml",
            "../../agents/jahresabrechnung-agent.yaml",
            "../../agents/mabis-syncd-agent.yaml",
            "../../agents/mako-agent.yaml",
            "../../agents/meter-data-agent.yaml",
            "../../agents/msb-history-agent.yaml",
            "../../agents/netzbilanz-agent.yaml",
            "../../agents/payment-reconciliation-agent.yaml",
            "../../agents/portald-agent.yaml",
            "../../agents/processd-agent.yaml",
            "../../agents/regulatory-reporting-agent.yaml",
            "../../agents/replacement-value-agent.yaml",
            "../../agents/smgw-diagnostics-agent.yaml",
            "../../agents/sperrd-agent.yaml",
            "../../agents/productd-agent.yaml",
            "../../agents/tariff-optimization-agent.yaml",
            "../../agents/vertragd-agent.yaml",
            "../../agents/vpp-billing-agent.yaml",
        ]
        .unwrap_or_else(|e| panic!("embedded specialist manifests: {e}"))
    })
}

/// The embedded specialist declaring `name`.
#[must_use]
pub fn find_manifest(name: &str) -> Option<&'static Manifest> {
    manifests().get(name)
}

/// Which revision of a specialist this binary embeds.
///
/// The digest is over the manifest's canonical bytes, so key order and
/// formatting cannot change it: two files that declare the same thing share
/// one, and a file that declares something different cannot. agentplane records
/// the same identity on every run it admits, which is what makes *"which
/// declaration governed this decision"* answerable months later.
///
/// This is the live half: the same identity read off a **running** plane, so a
/// reviewer who approved a file in a diff can check the process against it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Declaration {
    /// `metadata.version` — the human-readable bump a reviewer sees.
    pub version: String,
    /// The digest over the canonical manifest, hex.
    ///
    /// `None` where the manifest cannot be canonicalised. Reported as absent
    /// rather than as an empty string, for the reason agentplane records an
    /// absent identity rather than a false one: a run governed by a declaration
    /// that cannot name itself is not a run governed by nothing.
    pub digest: Option<String>,
}

/// The embedded revision of one specialist, for the inventory endpoints.
#[must_use]
pub fn declaration(name: &str) -> Option<Declaration> {
    let m = find_manifest(name)?;
    Some(Declaration {
        version: m.metadata.version.clone(),
        digest: m.digest().ok().map(|d| d.to_hex()),
    })
}

/// Parse a manifest, failing loudly — a manifest that does not load is a
/// deployment error, not a runtime condition.
///
/// # Errors
///
/// Returns [`agentplane::manifest::ManifestError`] when the YAML is malformed,
/// names an unknown field (the parser is `deny_unknown_fields`), or omits a
/// required block such as `budgets`.
pub fn parse_manifest(src: &str) -> Result<Manifest, agentplane::manifest::ManifestError> {
    Manifest::parse(src)
}

/// A model-backed specialist needs no Rust: `spec.execution` makes the runtime
/// supply the behaviour, and [`Plane::new`] registers each manifest with
/// [`Agent::new`](agentplane::runtime::Agent::new).
///
/// Coded specialists live in [`crate::skills`](crate::skills): a manifest that
/// omits `execution` and declares `models: {}` is one whose conduct is Rust
/// because the work is computation rather than judgement.
mod _coded_specialists_live_in_the_skills_module {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest loads, and the fields the skill depends on are present.
    ///
    /// `deny_unknown_fields` makes this a real check: a typo in the YAML is a
    /// hard failure here rather than a silently missing ceiling in production.
    /// Every specialist manifest parses.
    ///
    /// The parser is `deny_unknown_fields` and refuses several absences outright
    /// (`budgets`, `capabilities`, an `oversight` without a `deadline`), so this
    /// is a real check rather than a smoke test: a generated field name that
    /// does not exist fails here rather than at first dispatch.
    #[test]
    fn every_specialist_manifest_parses() {
        assert_eq!(manifests().len(), 28, "one manifest per specialist");
        for embedded in manifests() {
            let (name, m) = embedded;
            {
                {
                    assert_eq!(
                        &m.metadata.name, name,
                        "the parsed name is the embedded name"
                    );
                    assert!(
                        m.spec.identity.is_some(),
                        "{name}: an agent without an identity has no prompt"
                    );
                    // A quarantined model is declared only where something
                    // selects it. Under `tool-calling` with no memory formation
                    // nothing does, so the declaration would read as dual-model
                    // isolation while every call went to the privileged model.
                    // agentplane refuses that outright; this states the rule on
                    // mako's side so the reason survives in our own tree.
                    let quarantined = m
                        .spec
                        .models
                        .as_ref()
                        .and_then(|x| x.quarantined.as_ref())
                        .is_some();
                    let plans = matches!(
                        m.spec.execution.as_ref().map(|e| e.kind),
                        Some(agentplane::manifest::ExecutionKind::Planned)
                    );
                    assert!(
                        !quarantined
                            || plans
                            || m.spec
                                .memory
                                .as_ref()
                                .is_some_and(|mem| mem.formation.is_some()),
                        "{name}: declares a quarantined model that nothing would select"
                    );
                }
            }
        }
    }

    /// A manifest either declares how it runs, or names a skill that does.
    ///
    /// agentplane's rule is that `spec.execution` makes an agent fully
    /// declarative and its absence means the behaviour is a registered
    /// [`Skill`](agentplane::core::Skill). Nothing enforces the second half:
    /// a manifest with neither parses cleanly, registers, and fails at first
    /// dispatch with "no skill provides this capability" — in production, on the
    /// event that needed it.
    #[test]
    fn every_manifest_either_declares_execution_or_has_a_coded_skill() {
        // The specialists whose conduct is Rust. Adding one means adding its
        // registration in `Plane::new`, and this list is what keeps the two
        // from drifting apart.
        const CODED: &[&str] = &[
            crate::skills::DeadlineTriage::NAME,
            crate::skills::GabiAllocationTriage::NAME,
        ];

        for embedded in manifests() {
            let (name, m) = (embedded.0.as_str(), embedded.1);
            let declarative = m.spec.execution.is_some();
            let coded = CODED.contains(&name);
            assert!(
                declarative ^ coded,
                "{name}: {}",
                if declarative {
                    "declares `execution` *and* registers a coded skill — the runtime would \
                     supply the behaviour and the skill would never run"
                } else {
                    "declares no `execution` and registers no skill, so a run addressed to its \
                     capability finds nothing to invoke"
                }
            );
        }
    }

    /// A specialist that runs no model says so — and declares no token ceiling.
    ///
    /// `models: {}` is a declaration; a *missing* `models` block means "wired in
    /// code" and reads as an oversight.
    ///
    /// And it declares **no** token ceiling. `max_tokens: 0` reads as "an agent
    /// that cannot infer must not be allowed to spend" and means something
    /// else: the budget gate treats a zero ceiling as exhausted before the
    /// first effect of any kind, so even the specialist's one read-only tool
    /// call is refused. What binds a model-free agent is `max_steps`,
    /// `max_effects` and `max_wallclock_secs`.
    #[test]
    fn a_model_free_specialist_declares_it_and_carries_no_token_ceiling() {
        let m = find_manifest(crate::skills::DeadlineTriage::NAME)
            .expect("the deadline specialist is compiled in");

        let models = m.spec.models.as_ref().expect(
            "`models: {}` must be present: an absent block means \"wired in code\", which is \
             the silence this test exists to refuse",
        );
        assert!(models.privileged.is_none());
        assert!(models.quarantined.is_none());

        let budgets = m.spec.budgets.as_ref().expect("ceilings are mandatory");
        assert_eq!(
            budgets.max_tokens, None,
            "a zero token ceiling reads as parsimony and acts as a refusal of the first \
             effect — omit the ceiling on an agent with no model to spend through"
        );
        assert!(
            budgets.max_steps.is_some() && budgets.max_effects.is_some(),
            "the ceilings that bind a coded skill are steps and effects"
        );
    }

    /// A coded specialist grants exactly the tools its code calls.
    ///
    /// The cutover handed every specialist its servers' whole read surface —
    /// this one carried 34 grants across obsd, makod and marktd for a procedure
    /// that calls one tool. A coded skill's reach is legible, so there is no
    /// excuse for granting more than it takes.
    #[test]
    fn the_deadline_specialist_grants_only_the_tool_it_calls() {
        let m = find_manifest(crate::skills::DeadlineTriage::NAME).expect("compiled in");

        let refs: Vec<&str> = m.spec.tools.iter().map(|t| t.reference.as_str()).collect();
        assert_eq!(
            refs,
            vec!["tool://obsd/list_overdue_processes"],
            "least privilege is checkable when the caller is code"
        );
        assert!(
            m.output_schema().is_some(),
            "an alert nothing can parse is an alert nothing can act on"
        );
    }

    /// Every specialist returns a shape, not prose.
    ///
    /// A fenced `## OUTPUT FORMAT` block inside a prompt states the contract in
    /// the one place nothing can enforce it: every consumer becomes a parser of
    /// free text and a reworded heading is a silent break. As `output.schema`
    /// the model is held to it,
    /// the runtime puts it in the effect key (so a schema edit reports
    /// divergence on replay instead of reinterpreting a stored answer), and it
    /// is covered by the manifest digest.
    #[test]
    fn every_specialist_declares_a_machine_readable_result() {
        let mut prose_only = Vec::new();
        for embedded in manifests() {
            let (name, m) = (embedded.0.as_str(), embedded.1);
            if m.output_schema().is_none() {
                prose_only.push(name);
            }
            // And the fence must be gone: two copies of a contract disagree
            // eventually, and the prose copy is the one nobody validates.
            if let Some(identity) = m.spec.identity.as_ref() {
                assert!(
                    !identity.constraints.contains("OUTPUT FORMAT"),
                    "{name}: still carries a prose OUTPUT FORMAT fence beside its schema"
                );
            }
        }
        assert!(
            prose_only.is_empty(),
            "these specialists answer in prose nothing can act on: {prose_only:?}"
        );
    }

    /// Every answer schema is closed: `additionalProperties: false` at the top.
    ///
    /// An open schema is a contract with a hole in it. The model may pad the
    /// answer with fields nobody declared — which bloats the journal, invites
    /// consumers to depend on undeclared keys, and lets a drifting prompt grow
    /// output nobody reviews. A closed schema makes the declared shape the
    /// whole shape, which is also what keeps a triage rule's `path` total over
    /// what the model can actually return. (Out-of-band markers the runtime
    /// itself defines for `parse` steps — *not enough information* — are the
    /// runtime's business, not the schema's.)
    #[test]
    fn every_answer_schema_is_closed() {
        let mut open = Vec::new();
        for (name, m) in manifests() {
            let Some(schema) = m.output_schema() else {
                continue;
            };
            if schema.get("additionalProperties") != Some(&serde_json::Value::Bool(false)) {
                open.push(name.as_str());
            }
        }
        assert!(
            open.is_empty(),
            "these specialists declare an open answer schema — a model may pad the \
             answer with fields nobody declared: {open:?}. Add \
             `additionalProperties: false` beside the top-level `type: object`."
        );
    }

    /// **Closed all the way down**, not only at the top.
    ///
    /// The root is where the argument for closing a schema is usually made, and
    /// the holes are one level in: `findings[]`, `violations[]`,
    /// `failed_checks[]` carry what the specialist concluded, which is the part a
    /// triage rule's `path` reaches and an ERP reads.
    ///
    /// Scoped to objects that declare `properties`. One with none is a map whose
    /// keys are data — `by_partner_mp_id` counts per MP-ID, `trigger` echoes the
    /// event back — and closing it would forbid the content it exists to carry.
    #[test]
    fn every_object_in_an_answer_schema_that_names_its_fields_is_closed() {
        /// Walk a schema, collecting the JSON pointers of open objects.
        fn open_objects(node: &serde_json::Value, at: &str, out: &mut Vec<String>) {
            if let Some(map) = node.as_object() {
                if map.contains_key("properties")
                    && map.get("additionalProperties") != Some(&serde_json::Value::Bool(false))
                {
                    out.push(if at.is_empty() {
                        "<root>".to_owned()
                    } else {
                        at.to_owned()
                    });
                }
                for (key, value) in map {
                    open_objects(value, &format!("{at}/{key}"), out);
                }
            } else if let Some(items) = node.as_array() {
                for (i, value) in items.iter().enumerate() {
                    open_objects(value, &format!("{at}[{i}]"), out);
                }
            }
        }

        let mut open = Vec::new();
        for (name, m) in manifests() {
            let Some(schema) = m.output_schema() else {
                continue;
            };
            let mut here = Vec::new();
            open_objects(schema, "", &mut here);
            for path in here {
                open.push(format!("{name}: {path}"));
            }
        }
        assert!(
            open.is_empty(),
            "these answer-schema objects name their fields and then accept any others — \
             a model may pad exactly the part a triage rule reads: {open:#?}. Add \
             `additionalProperties: false` beside each `properties`."
        );
    }

    /// Knowledge is granted, not copied.
    ///
    /// mako's MCP servers publish 57 step-by-step prompts for their own
    /// procedures. A manifest names the prompt it needs; a hand-typed paraphrase
    /// in `constraints` drifts from the server's prompt the first time either
    /// changes.
    #[test]
    fn specialists_grant_the_knowledge_their_service_publishes() {
        // Coded specialists have no model to read a prompt. Billingd currently
        // publishes no VPP-specific prompt; importing its EEG prompt is worse
        // than keeping the VPP procedure explicit and was removed by audit.
        const NO_PROMPT_NEEDED: &[&str] = &[
            "deadline-alert-agent",
            "gabi-gas-agent",
            "vpp-billing-agent",
        ];

        let mut ungranted = Vec::new();
        for embedded in manifests() {
            let (name, m) = (embedded.0.as_str(), embedded.1);
            if NO_PROMPT_NEEDED.contains(&name) {
                continue;
            }
            if m.spec.context.prompts.is_empty() {
                ungranted.push(name);
            }
        }
        assert!(
            ungranted.is_empty(),
            "these specialists re-state a procedure their own service publishes: {ungranted:?}"
        );
    }

    /// Nobody delegates, and the file says so.
    ///
    /// `max_delegation_depth: 0` already enforced it; `topology` states it, so
    /// the arrangement is a declaration a reviewer can disagree with rather than
    /// something that has to be inferred from a numeric ceiling. MAST puts
    /// inter-agent misalignment at 36.9 % of observed multi-agent failures — a
    /// class mako does not have, because routing is Rust.
    #[test]
    fn every_specialist_declares_its_arrangement_and_hands_off_to_nobody() {
        use agentplane::manifest::{Role, TopologyMode};
        for embedded in manifests() {
            let (name, m) = (embedded.0.as_str(), embedded.1);
            let topology = m
                .spec
                .topology
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: declares no topology"));
            assert_eq!(topology.mode, TopologyMode::Single, "{name}");
            assert_eq!(
                topology.role,
                Role::Specialist,
                "{name}: mako routes in Rust, so no agent orchestrates another"
            );
            assert_eq!(
                m.spec.security.max_delegation_depth,
                Some(0),
                "{name}: a specialist that could delegate is an orchestrator nobody reviewed"
            );
        }
    }

    /// A *literal* memory subject must be operator-global; anything about a
    /// party must be a **binding**.
    ///
    /// `subject` is the unit `MemoryStore::forget_subject` erases. A literal one
    /// on a per-customer specialist therefore pools every Marktlokation's facts
    /// under one key: one customer's history is recalled into another's run, and
    /// a GDPR Art. 17 erasure naming one person cannot be satisfied without
    /// destroying everybody's.
    ///
    /// A binding (`$correlation/malo`) resolves per run, which is what makes
    /// per-customer memory safe; a literal is one fixed pile for every run.
    #[test]
    fn a_literal_memory_subject_must_be_operator_wide() {
        use agentplane::manifest::MemorySubject;

        // The only scopes that are genuinely the same for every run: the
        // operator's own regulatory posture.
        const OPERATOR_WIDE: &[&str] = &["operator-parity-posture", "bnetza-reporting-posture"];

        for (name, m) in manifests() {
            let Some(formation) = m
                .spec
                .memory
                .as_ref()
                .and_then(|mem| mem.formation.as_ref())
            else {
                continue;
            };
            match &formation.subject {
                MemorySubject::Literal(subject) => assert!(
                    OPERATOR_WIDE.contains(&subject.as_str()),
                    "{name}: files memories under the literal subject '{subject}'. A literal is \
                     one fixed pile for every run — if this is about a Marktlokation, bind it \
                     (`$correlation/malo`); if it is genuinely operator-wide, add it to \
                     OPERATOR_WIDE with the argument for why"
                ),
                // A binding resolves per run, which is the whole point. `$input`
                // would be a subject chosen by whoever supplied the field, and
                // agentplane already refuses it unless the field is trusted —
                // mako has no use for it and does not want to acquire one
                // silently.
                MemorySubject::Correlation(namespace) => assert_eq!(
                    namespace, "malo",
                    "{name}: binds to correlation namespace '{namespace}', which \
                     `plane::label` does not produce — the binding would fail the run"
                ),
                other => panic!("{name}: unexpected memory subject binding {other:?}"),
            }

            // Formation reads the agent's own answer, which is model output.
            assert!(
                m.spec
                    .models
                    .as_ref()
                    .and_then(|x| x.quarantined.as_ref())
                    .is_some(),
                "{name}: forms memories from untrusted-derived content with no quarantined \
                 model to read it"
            );
            assert!(
                formation.retention_seconds.is_some(),
                "{name}: a memory with no expiry hardens into a standing instruction"
            );
        }
    }

    /// A finding a person should see opens a worklist row.
    ///
    /// Most specialists are advisory by construction — a `tool-calling` agent's
    /// arguments come from a model completion, so the taint gate refuses them at
    /// a mutating sink. For them `approval: tools-only` gates nothing and
    /// `required` would suspend a run per finding; `triage` is the mode that
    /// fits: the run returns, and a matching answer *also* opens a row.
    ///
    /// The check is that a specialist whose schema declares a *terminal*
    /// severity has a rule for it. An agent that can report `VIOLATION` and
    /// tells nobody is the failure this test exists to prevent.
    #[test]
    fn a_specialist_that_can_report_a_breach_opens_a_task_for_it() {
        // The terminal values across mako's answer schemas: the ones that mean
        // "this will not resolve itself".
        const TERMINAL: &[&str] = &[
            "VIOLATION",
            "VIOLATIONS",
            "BLOCKED",
            "MISSING",
            "FAILED",
            "REJECTED",
            "SPERR_REQUIRED",
            "SPERR",
            "CRITICAL",
            "BREACH",
            "ESCALATED",
            "VALID_DISPUTE",
            "NB_ERROR",
            "CONFLICT",
            "ESCALATE_MISSING_DATA",
            "ESCALATE_MISSING_RATE",
        ];

        let mut silent = Vec::new();
        for (name, m) in manifests() {
            let Some(schema) = m.output_schema() else {
                continue;
            };
            // Does any enum in the answer carry a terminal value?
            let declares_terminal = serde_json::to_string(schema)
                .is_ok_and(|s| TERMINAL.iter().any(|t| s.contains(&format!("\"{t}\""))));
            if !declares_terminal {
                continue;
            }
            let triaged = m
                .spec
                .oversight
                .as_ref()
                .is_some_and(|o| !o.triage.is_empty());
            // A coded specialist cannot declare `oversight` at all — agentplane
            // refuses the block on a manifest with no `execution` — so it opens
            // its row with `StepCtx::open_task`. That was stated here as an
            // exemption and checked by nothing, and the one coded specialist did
            // not do it: `deadline-alert-agent` could report `BREACH` and told
            // nobody. `CODED_TRIAGE` is the list of coded specialists whose Rust
            // *does* open a row, asserted below rather than assumed.
            const CODED_TRIAGE: &[&str] = &[
                crate::skills::DeadlineTriage::NAME,
                crate::skills::GabiAllocationTriage::NAME,
            ];
            let coded = m.spec.execution.is_none() && CODED_TRIAGE.contains(&name.as_str());
            if !triaged && !coded {
                silent.push(name.as_str());
            }
        }
        assert!(
            silent.is_empty(),
            "these specialists can report a terminal finding and tell nobody: {silent:?}. \
             Add an `oversight.triage` rule, or explain in the file why the finding needs \
             no human"
        );
    }

    /// The coded specialist's worklist row names an audience the policy admits.
    ///
    /// A manifest's audiences are checked against `policy/agentd.cedar` by
    /// `plane::policy`; a coded specialist's is a Rust constant, so it is checked
    /// here. A row filed for a role Cedar refuses at the door reads, from the
    /// worklist, exactly like a row that was answered.
    #[test]
    fn the_coded_specialists_triage_audience_is_admitted_by_the_policy_set() {
        use agentplane::core::{PolicyDecision, PolicyRequest};

        let engine = crate::plane::policy::engine(crate::plane::policy::DEFAULT_POLICY)
            .expect("the embedded policy set compiles");
        for role in [
            crate::skills::DeadlineTriage::TRIAGE_AUDIENCE,
            crate::skills::DeadlineTriage::TRIAGE_ESCALATION,
            crate::skills::GabiAllocationTriage::TRIAGE_AUDIENCE,
            crate::skills::GabiAllocationTriage::TRIAGE_ESCALATION,
        ] {
            let context = serde_json::json!({ "roles": [role], "tenant": "9900357000004" });
            let decision = engine.authorize(&PolicyRequest {
                principal: "user:reviewer",
                action: "api:task.decide",
                resource: "*",
                context: &context,
            });
            assert!(
                matches!(decision, PolicyDecision::Permit),
                "`{role}` is named as the coded specialist's triage audience but \
                 policy/agentd.cedar refuses it the worklist"
            );
        }
    }

    /// Every manifest file is embedded, and every embedded manifest is a file.
    ///
    /// `manifests![]` lists paths rather than globbing, which is right — what a
    /// plane runs must be what a reviewer reads. It leaves one hole, in the
    /// direction nobody notices: a **file nobody added to the list** is a
    /// specialist that was written, reviewed and merged and is not in the binary
    /// at all. It has no builtin entry, so no subscription test names it, and no
    /// route, so no routing test misses it. (The other direction already fails
    /// at compile time, on `include_str!`.)
    #[test]
    fn the_agents_directory_and_the_embedded_set_are_the_same_set() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("agents");
        let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(&dir)
            .expect("the agents directory is beside the crate")
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                (path.extension()? == "yaml")
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))?
            })
            .collect();
        let embedded: std::collections::BTreeSet<String> = manifests().keys().cloned().collect();

        let unembedded: Vec<&String> = on_disk.difference(&embedded).collect();
        assert!(
            unembedded.is_empty(),
            "these manifests are on disk but absent from `manifests![]`, so they are not in \
             the binary and no test would ever mention them: {unembedded:?}"
        );
        // A file whose `metadata.name` differs from its filename embeds fine and
        // then cannot be found by anyone reading the directory.
        let misnamed: Vec<&String> = embedded.difference(&on_disk).collect();
        assert!(
            misnamed.is_empty(),
            "these specialists declare a `metadata.name` that is not their filename: \
             {misnamed:?}"
        );
    }

    /// **Every model a manifest names is one this deployment reviewed.**
    ///
    /// The model answering a regulated decision is part of what a reviewer
    /// approves, and it is declared in 28 separate strings. Two spellings of one
    /// model both resolve, so nothing fails — and a fleet answering from two
    /// models is not the fleet anybody reviewed.
    ///
    /// The list here is the second place, deliberately: moving a specialist onto
    /// a different model is a manifest edit *and* this edit, both on the
    /// reviewable path. A model id no provider serves fails here rather than at
    /// the first dispatch of the event that needed it.
    #[test]
    fn every_declared_model_is_one_this_deployment_reviewed() {
        /// The models mako's specialists run on.
        ///
        /// `privileged` reads the payload and decides; `quarantined` reads
        /// counterparty-derived content and cannot call a tool, so it is the
        /// cheaper model on purpose — it is doing extraction, not judgement.
        const REVIEWED: &[&str] = &["claude-sonnet-5", "claude-haiku-4-5"];

        let mut unreviewed = Vec::new();
        for (name, m) in manifests() {
            let Some(models) = m.spec.models.as_ref() else {
                continue;
            };
            for (which, pair) in [
                ("privileged", models.privileged.as_ref()),
                ("quarantined", models.quarantined.as_ref()),
            ] {
                if let Some(pair) = pair
                    && !REVIEWED.contains(&pair.model.as_str())
                {
                    unreviewed.push(format!("{name}: {which} = {}", pair.model));
                }
            }
        }
        assert!(
            unreviewed.is_empty(),
            "these manifests name a model that is not on this deployment's reviewed list: \
             {unreviewed:#?}. Add it to REVIEWED — deliberately — or correct the manifest."
        );
    }

    /// **Every embedded manifest can name itself.**
    ///
    /// agentplane records an [`AgentIdentity`](agentplane::journal::AgentIdentity)
    /// on every admitted run and computes the digest there, recording an *absent*
    /// identity when a manifest cannot be canonicalised rather than a false one.
    /// The right refusal upstream and a silent one here: such a specialist runs
    /// perfectly and leaves a journal that cannot say what governed it.
    #[test]
    fn every_manifest_produces_a_digest() {
        let nameless: Vec<&str> = manifests()
            .keys()
            .map(String::as_str)
            .filter(|name| declaration(name).is_none_or(|d| d.digest.is_none()))
            .collect();
        assert!(
            nameless.is_empty(),
            "these declarations cannot be canonicalised, so every run they govern is \
             journaled with no identity: {nameless:?}"
        );
    }

    /// Two files that declare different things do not share a digest.
    ///
    /// The property the whole review path rests on: editing a procedure changes
    /// the digest, so a reviewer sees a version bump rather than a silent
    /// substitution.
    #[test]
    fn a_digest_distinguishes_one_declaration_from_another() {
        let digests: std::collections::BTreeSet<String> = manifests()
            .keys()
            .filter_map(|name| declaration(name)?.digest)
            .collect();
        assert_eq!(
            digests.len(),
            manifests().len(),
            "two specialists share a digest — a declaration that cannot be told from \
             another is one an auditor cannot attribute a decision to"
        );
    }

    /// **A role build's worklist rows stay inside its own arm (§§ 6a, 7a EnWG).**
    ///
    /// The structural half is asserted elsewhere: a `role-lf` binary does not
    /// *contain* the NB specialists and does not require their MCP endpoints.
    /// This is the half that leaks the other way — not what a specialist reads,
    /// but who it hands a finding to.
    ///
    /// A worklist row carries the finding, the justification and the run it came
    /// from, so filing an NB one on a supply desk is grid operational state
    /// reaching supply people — the boundaries §§ 6a and 7a EnWG draw — and in a
    /// role build that desk may not exist to answer it.
    ///
    /// Compiled only where **exactly one** role feature is on, because that is
    /// the only build whose compiled set *is* an arm. `--all-features` turns all
    /// three on and is an all-roles build wearing role flags: the specialists are
    /// all present and no arm is excluded, so asking which desks are foreign has
    /// no answer. Gating on `any(...)` made `just test --all-features` pick the
    /// LF answer for a binary containing every specialist, and fail on eight
    /// perfectly correct audiences.
    #[cfg(any(
        all(
            feature = "role-lf",
            not(feature = "role-nb"),
            not(feature = "role-msb")
        ),
        all(
            feature = "role-nb",
            not(feature = "role-lf"),
            not(feature = "role-msb")
        ),
        all(
            feature = "role-msb",
            not(feature = "role-lf"),
            not(feature = "role-nb")
        ),
    ))]
    #[test]
    fn role_scoped_worklist_audiences_stay_inside_the_arm() {
        /// Desks that answer for the supply business.
        const SUPPLY: &[&str] = &["billing-operations", "billing-compliance", "credit-control"];
        /// Desks that answer for the grid business.
        const GRID: &[&str] = &[
            "grid-operations",
            "netzbilanz",
            "eeg-operations",
            "gas-operations",
        ];
        /// Desks that answer for metering.
        const METERING: &[&str] = &["metering"];
        /// Desks every arm shares: the plane's own operations, the MaKo desk,
        /// and the regulatory function that answers to the BNetzA for all of it.
        const CROSS_CUTTING: &[&str] = &["mako-operations", "marktkommunikation", "regulatory"];

        let forbidden: &[&[&str]] = if cfg!(feature = "role-lf") {
            &[GRID, METERING]
        } else if cfg!(feature = "role-nb") {
            &[SUPPLY, METERING]
        } else {
            &[SUPPLY, GRID]
        };

        let mut crossings = Vec::new();
        for (name, m) in manifests() {
            // Only the specialists this build compiled: `manifests![]` is not
            // role-gated, so the embedded set still carries the other arms'.
            if crate::builtin::find(name).is_none() {
                continue;
            }
            let Some(oversight) = m.spec.oversight.as_ref() else {
                continue;
            };
            let mut named: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            named.extend(oversight.approvers.iter().map(String::as_str));
            named.extend(oversight.escalate_to.iter().map(String::as_str));
            for rule in &oversight.triage {
                named.extend(rule.audience.iter().map(String::as_str));
            }
            for role in named {
                if CROSS_CUTTING.contains(&role) {
                    continue;
                }
                if forbidden.iter().any(|arm| arm.contains(&role)) {
                    crossings.push(format!("{name} → {role}"));
                }
            }
        }

        assert!(
            crossings.is_empty(),
            "these specialists hand a finding to a desk in another Marktrolle's arm, which \
             this build does not serve: {crossings:#?}. A worklist row carries the run's \
             own state across that boundary, and in a role-scoped deployment the desk may \
             not exist to answer it."
        );
    }

    /// Every desk a manifest names is classified by the check above.
    ///
    /// Runs in the default build, where every specialist is present, so a new
    /// audience cannot arrive unclassified — which would make the §§ 6a and 7a EnWG check
    /// silently skip it rather than fail.
    #[test]
    fn every_audience_a_manifest_names_belongs_to_a_known_arm() {
        const KNOWN: &[&str] = &[
            // supply
            "billing-operations",
            "billing-compliance",
            "credit-control",
            // grid
            "grid-operations",
            "netzbilanz",
            "eeg-operations",
            "gas-operations",
            // metering
            "metering",
            // cross-cutting
            "mako-operations",
            "marktkommunikation",
            "regulatory",
        ];

        let mut unclassified = Vec::new();
        for (name, m) in manifests() {
            let Some(oversight) = m.spec.oversight.as_ref() else {
                continue;
            };
            let mut named: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            named.extend(oversight.approvers.iter().map(String::as_str));
            named.extend(oversight.escalate_to.iter().map(String::as_str));
            for rule in &oversight.triage {
                named.extend(rule.audience.iter().map(String::as_str));
            }
            for role in named {
                if !KNOWN.contains(&role) {
                    unclassified.push(format!("{name} → {role}"));
                }
            }
        }
        assert!(
            unclassified.is_empty(),
            "these audiences belong to no arm this codebase knows, so the §§ 6a and 7a EnWG check \
             cannot decide whether a role build may name them: {unclassified:#?}. Add the \
             desk to its arm in `role_scoped_worklist_audiences_stay_inside_the_arm`, \
             deliberately."
        );
    }

    /// **A code a procedure tells the model to emit has somewhere to go.**
    ///
    /// An answer schema is closed, so a finding code the procedure defines and
    /// the schema does not carry cannot be returned *at all*: the model is told
    /// to report `SECT41A_IMSYS_REQUIRED`, has no field for it, and puts
    /// something adjacent in a field meant for something else — or nothing
    /// anywhere. The run completes, the answer validates, the finding is gone.
    ///
    /// Reads **emitted** codes only: a token introduced by `ERROR`, `WARNING`,
    /// `report` or `emit finding`. A code a procedure merely *reads* —
    /// `PERIOD_OVERLAP` off billingd's risk gate, `NEEDS_REVIEW` off einsd's
    /// settlement state — is an input and belongs in no answer schema.
    #[test]
    fn every_code_a_procedure_emits_exists_in_its_answer_schema() {
        /// The verbs that mark a token as this specialist's own output.
        const EMITS: &[&str] = &["ERROR", "WARNING", "report", "emit finding"];

        /// Whether `token` is a coded finding rather than a word.
        fn is_code(token: &str) -> bool {
            token.contains('_')
                && token.len() > 4
                && token
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        }

        let mut orphaned = Vec::new();
        for (name, m) in manifests() {
            let (Some(identity), Some(schema)) = (m.spec.identity.as_ref(), m.output_schema())
            else {
                continue;
            };
            let procedure = agentplane::manifest::Identity::system_prompt(identity);
            let rendered = serde_json::to_string(schema).unwrap_or_default();

            for verb in EMITS {
                let mut rest = procedure.as_str();
                while let Some(at) = rest.find(verb) {
                    rest = &rest[at + verb.len()..];
                    // **Skip the whitespace first.** A procedure is a wrapped
                    // YAML block scalar, so the code frequently lands on the
                    // next line: `— report\n   SECT42_…_STALE for those.`
                    // Reading straight from the verb found an empty token and
                    // the check passed on everything, which is the way a lint
                    // fails that nobody notices.
                    let token: String = rest
                        .trim_start()
                        .chars()
                        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                        .collect();
                    if is_code(&token) && !rendered.contains(&token) {
                        orphaned.push(format!("{name}: {token}"));
                    }
                }
            }
        }
        orphaned.sort();
        orphaned.dedup();
        assert!(
            orphaned.is_empty(),
            "these procedures tell the model to emit a code its own closed answer schema \
             cannot carry, so the finding has nowhere to go and the run completes without \
             it: {orphaned:#?}"
        );
    }

    /// **A manifest's own comments do not contradict it.**
    ///
    /// The comments in `agents/*.yaml` explain what the declaration below them
    /// does, and they sit inside the file the digest covers — so a drifted one is
    /// a false statement a reviewer reads *while* approving the thing it
    /// misdescribes. "No `oversight` block" above an `oversight:` block is the
    /// shape: one fact in two places, and the copy nobody validates is the one
    /// people read.
    ///
    /// Only absences are checked, because only an absence can be contradicted by
    /// the file itself without ambiguity.
    #[test]
    fn no_manifest_comment_claims_an_absence_the_file_contradicts() {
        /// A phrase that claims something is absent, and how to see whether it is.
        struct Claim {
            phrase: &'static str,
            present: fn(&Manifest) -> bool,
        }

        const CLAIMS: &[Claim] = &[
            Claim {
                phrase: "No `oversight` block",
                present: |m| m.spec.oversight.is_some(),
            },
            Claim {
                phrase: "no mutating tool grant",
                present: |m| m.spec.tools.iter().any(|t| t.mutates),
            },
            Claim {
                phrase: "no mutating grants here at all",
                present: |m| m.spec.tools.iter().any(|t| t.mutates),
            },
        ];

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("agents");
        let mut contradictions = Vec::new();
        for (name, m) in manifests() {
            let Ok(src) = std::fs::read_to_string(dir.join(format!("{name}.yaml"))) else {
                continue;
            };
            for claim in CLAIMS {
                if src.contains(claim.phrase) && (claim.present)(m) {
                    contradictions.push(format!("{name}: \"{}\"", claim.phrase));
                }
            }
        }
        assert!(
            contradictions.is_empty(),
            "these manifests carry a comment claiming an absence the declaration below it \
             contradicts — a false sentence a reviewer reads while approving the thing it \
             misdescribes: {contradictions:#?}"
        );
    }

    /// Every manifest points an editor at the schema it is written against.
    ///
    /// The modeline turns agentplane's published JSON Schema into autocomplete,
    /// hover documentation and inline unknown-field errors *while a manifest is
    /// being written*, rather than at `cargo test`. The URL is read off
    /// `Manifest::json_schema()` rather than typed here, so a file cannot end up
    /// pointing at a document that has moved.
    #[test]
    fn every_manifest_carries_the_schema_modeline() {
        let schema = Manifest::json_schema();
        let id = schema["$id"]
            .as_str()
            .expect("the published schema names its own origin in `$id`");
        let expected = format!("# yaml-language-server: $schema={id}");

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("agents");
        let mut wrong = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("the agents directory") {
            let path = entry.expect("a directory entry").path();
            if path.extension().is_none_or(|ext| ext != "yaml") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("a readable manifest");
            if src.lines().next() != Some(expected.as_str()) {
                wrong.push(path.file_name().map(std::ffi::OsStr::to_os_string));
            }
        }
        assert!(
            wrong.is_empty(),
            "these manifests do not open with `{expected}`, so an editor writing them has no \
             schema to check against: {wrong:?}"
        );
    }

    /// **A finding widens when nobody answers it; a dispatch fails closed.**
    ///
    /// `on_expiry` has one name and two jobs, and the right answer is opposite
    /// in each:
    ///
    /// * A **triage** row gates nothing — the run already finished, and the row
    ///   *is* the finding: a §20 EnWG parity deviation, an EEG breach, a
    ///   §§41f/41g sequence out of compliance. `deny` expires it and takes it
    ///   out of the worklist, so a breach that was correctly detected and
    ///   correctly delivered is deleted when its window closes. `escalate` is
    ///   the only disposition that keeps it findable.
    /// * An **approval** gates a real market message, where expiring closed is
    ///   the whole point: nobody looked, so nothing is sent.
    #[test]
    fn an_unanswered_finding_widens_and_an_unanswered_dispatch_fails_closed() {
        use agentplane::manifest::{Approval, Expiry};

        for (name, m) in manifests() {
            let Some(oversight) = m.spec.oversight.as_ref() else {
                continue;
            };
            let gates_a_dispatch = oversight.approval != Approval::None
                || m.spec.tools.iter().any(|grant| grant.requires_approval);

            if gates_a_dispatch {
                assert_eq!(
                    oversight.on_expiry,
                    Expiry::Deny,
                    "{name}: an approval that gates a dispatch must fail closed when its \
                     window passes — anything else sends a market message nobody reviewed"
                );
                continue;
            }

            assert_eq!(
                oversight.on_expiry,
                Expiry::Escalate,
                "{name}: its triage rows are findings, not gates, so expiring them deletes \
                 the delivery of something the agent correctly detected. Declare \
                 `on_expiry: escalate` with an `escalate_to` audience"
            );
            assert!(
                !oversight.escalate_to.is_empty(),
                "{name}: escalates to nobody"
            );
            // Widening to the audience that already has it is not a widening.
            for rule in &oversight.triage {
                assert!(
                    oversight
                        .escalate_to
                        .iter()
                        .any(|wider| !rule.audience.contains(wider)),
                    "{name}: triage rule '{}' escalates only to roles already in its \
                     audience, so the escalation adds nobody",
                    rule.name
                );
            }
        }
    }

    #[test]
    fn the_gabi_manifest_is_live_model_free_and_triaged_in_code() {
        let m = find_manifest("gabi-gas-agent").expect("the GaBi Gas specialist is compiled in");

        assert_eq!(m.metadata.name, "gabi-gas-agent");

        let identity = m.spec.identity.as_ref().expect("identity declared");
        let prompt = agentplane::manifest::Identity::system_prompt(identity);
        assert!(
            prompt.contains("de.gabi.alocat.missing")
                && prompt.contains("does not ask a model")
                && prompt.contains("cannot dispatch"),
            "the declaration must describe the emitted event and deterministic hard cut"
        );

        assert!(
            m.spec.execution.is_none(),
            "conduct is the registered Rust skill"
        );
        assert!(
            m.spec.tools.is_empty(),
            "the deterministic event needs no evidence lookup"
        );

        let models = m.spec.models.as_ref().expect("models declared");
        assert!(models.privileged.is_none() && models.quarantined.is_none());
        assert!(
            m.spec.oversight.is_none(),
            "the coded skill opens its own task"
        );

        assert!(
            m.output_schema().is_some(),
            "the OUTPUT FORMAT block became a schema; without it the result contract is prose again"
        );
    }
}
