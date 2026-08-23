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
pub mod oversight;
pub mod policy;
pub mod providers;
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
            "../../agents/tarifbd-agent.yaml",
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
        const CODED: &[&str] = &[crate::skills::DeadlineTriage::NAME];

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

    /// Knowledge is granted, not copied.
    ///
    /// mako's MCP servers publish fifty step-by-step prompts for their own
    /// procedures. Before this, no manifest reached one: each specialist carried
    /// a hand-typed paraphrase in `constraints`, so the server's prompt and the
    /// agent's copy drifted apart the first time either changed.
    #[test]
    fn specialists_grant_the_knowledge_their_service_publishes() {
        // The two whose procedure is code or a plan need no prompt: one has no
        // model to read it, the other's control flow is fixed before anything
        // untrusted is read.
        const NO_PROMPT_NEEDED: &[&str] = &["deadline-alert-agent", "gabi-gas-agent"];

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
            // A coded specialist has `StepCtx::open_task` instead; oversight
            // without `execution` is refused, so it cannot declare triage.
            let coded = m.spec.execution.is_none();
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
    fn the_gabi_manifest_parses_and_declares_what_the_skill_reads() {
        let m = find_manifest("gabi-gas-agent").expect("the GaBi Gas specialist is compiled in");

        assert_eq!(m.metadata.name, "gabi-gas-agent");

        let identity = m.spec.identity.as_ref().expect("identity declared");
        let prompt = agentplane::manifest::Identity::system_prompt(identity);
        assert!(
            prompt.contains("kWh_Hs"),
            "the procedure must reach the prompt — it is the unit rule that DVGW G 685 turns on"
        );

        let models = m.spec.models.as_ref().expect("models declared");
        assert!(
            models.privileged.is_some(),
            "a privileged model is required"
        );
        assert!(
            models.quarantined.is_some(),
            "the quarantined model is the point: counterparty text must be read by a model \
             that cannot call a tool"
        );

        assert!(
            m.output_schema().is_some(),
            "the OUTPUT FORMAT block became a schema; without it the result contract is prose again"
        );
    }
}
