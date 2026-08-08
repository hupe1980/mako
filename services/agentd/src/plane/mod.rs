//! agentplane runtime integration.
//!
//! The migration target for `agentd`: specialists become declarative manifests
//! run by [`agentplane::runtime::Runtime`], so every model call and tool call is
//! a journaled effect rather than a log line.
//!
//! What lives where, and why:
//!
//! * **The manifest** (`agents/*.yaml`) owns the prompt, the model pair, the
//!   tool grants, the ceilings and the result schema. It is digest-covered, so
//!   editing a procedure is a version bump a reviewer sees.
//! * **The skill** (this module) owns the shape of the turn — which calls happen
//!   in which order. It holds no prompt and names no model; it asks
//!   [`StepCtx::manifest`] for both. A skill carrying its own copy could
//!   disagree with the agent about what the agent is.
//!
//! See `concepts/AGENTD.md` for the migration plan and the boundary this keeps:
//! the agent may prepare and may wait, the deterministic engine still dispatches.

pub mod runtime;
pub use runtime::{Activation, AgentDecision, Plane, Route, Router};

use std::sync::Arc;

use agentplane::core::{Outcome, Sensitivity, Skill, SkillDescriptor, SkillError, Tainted};
use agentplane::manifest::Manifest;
use agentplane::model::{ModelCall, ModelId, ModelProvider};
use agentplane::runtime::StepCtx;
use serde_json::{Value, json};

/// Every specialist manifest, embedded at compile time.
///
/// `include_str!` rather than a runtime directory read: a manifest that fails to
/// parse is a deployment error, and embedding makes it a build-time artefact
/// that ships with the binary it was reviewed against.
pub const MANIFESTS: &[(&str, &str)] = &[
    (
        "billing-agent",
        include_str!("../../agents/billing-agent.yaml"),
    ),
    (
        "billing-anomaly-agent",
        include_str!("../../agents/billing-anomaly-agent.yaml"),
    ),
    (
        "billing-regulatory-guard-agent",
        include_str!("../../agents/billing-regulatory-guard-agent.yaml"),
    ),
    (
        "compliance-agent",
        include_str!("../../agents/compliance-agent.yaml"),
    ),
    (
        "deadline-alert-agent",
        include_str!("../../agents/deadline-alert-agent.yaml"),
    ),
    ("eeg-agent", include_str!("../../agents/eeg-agent.yaml")),
    (
        "eeg-compliance-agent",
        include_str!("../../agents/eeg-compliance-agent.yaml"),
    ),
    (
        "einsd-batch-agent",
        include_str!("../../agents/einsd-batch-agent.yaml"),
    ),
    (
        "gabi-gas-agent",
        include_str!("../../agents/gabi-gas-agent.yaml"),
    ),
    (
        "grid-anomaly-agent",
        include_str!("../../agents/grid-anomaly-agent.yaml"),
    ),
    (
        "invoice-reconciliation-agent",
        include_str!("../../agents/invoice-reconciliation-agent.yaml"),
    ),
    (
        "jahresabrechnung-agent",
        include_str!("../../agents/jahresabrechnung-agent.yaml"),
    ),
    (
        "mabis-syncd-agent",
        include_str!("../../agents/mabis-syncd-agent.yaml"),
    ),
    ("mako-agent", include_str!("../../agents/mako-agent.yaml")),
    (
        "meter-data-agent",
        include_str!("../../agents/meter-data-agent.yaml"),
    ),
    (
        "msb-history-agent",
        include_str!("../../agents/msb-history-agent.yaml"),
    ),
    (
        "netzbilanz-agent",
        include_str!("../../agents/netzbilanz-agent.yaml"),
    ),
    (
        "payment-reconciliation-agent",
        include_str!("../../agents/payment-reconciliation-agent.yaml"),
    ),
    (
        "portald-agent",
        include_str!("../../agents/portald-agent.yaml"),
    ),
    (
        "processd-agent",
        include_str!("../../agents/processd-agent.yaml"),
    ),
    (
        "regulatory-reporting-agent",
        include_str!("../../agents/regulatory-reporting-agent.yaml"),
    ),
    (
        "replacement-value-agent",
        include_str!("../../agents/replacement-value-agent.yaml"),
    ),
    (
        "smgw-diagnostics-agent",
        include_str!("../../agents/smgw-diagnostics-agent.yaml"),
    ),
    (
        "sperrd-agent",
        include_str!("../../agents/sperrd-agent.yaml"),
    ),
    (
        "tarifbd-agent",
        include_str!("../../agents/tarifbd-agent.yaml"),
    ),
    (
        "tariff-optimization-agent",
        include_str!("../../agents/tariff-optimization-agent.yaml"),
    ),
    (
        "vertragd-agent",
        include_str!("../../agents/vertragd-agent.yaml"),
    ),
    (
        "vpp-billing-agent",
        include_str!("../../agents/vpp-billing-agent.yaml"),
    ),
];

/// The GaBi Gas balancing specialist, declared in `agents/gabi-gas-agent.yaml`.
pub const GABI_GAS_MANIFEST: &str = include_str!("../../agents/gabi-gas-agent.yaml");

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

/// One consultation of a specialist.
///
/// The turn is deliberately plain: assemble the declared prompt, ask the
/// declared model for the declared shape. Everything that decides *what* is
/// asked comes from the manifest, so this struct is the same for every
/// specialist and the difference between them is a file.
#[derive(Debug)]
pub struct Specialist {
    /// Capability this skill provides, matching `spec.capabilities.provides`.
    capability: &'static str,
    /// Skill name for the descriptor.
    name: &'static str,
    /// The model driver. `agentplane::model::{openai, anthropic, bedrock}` in
    /// production; `testkit::FakeProvider` under test.
    provider: Arc<dyn ModelProvider>,
}

impl Specialist {
    /// Build a specialist bound to a capability and a model driver.
    #[must_use]
    pub fn new(
        name: &'static str,
        capability: &'static str,
        provider: Arc<dyn ModelProvider>,
    ) -> Self {
        Self {
            name,
            capability,
            provider,
        }
    }
}

#[async_trait::async_trait]
impl Skill for Specialist {
    fn descriptor(&self) -> SkillDescriptor {
        SkillDescriptor::new(self.name).provides(self.capability)
    }

    async fn invoke(
        &self,
        cx: &mut StepCtx<'_>,
        input: Tainted<Value>,
    ) -> Result<Outcome, SkillError> {
        // Read the declaration into owned values before any effect runs, so
        // nothing borrows the agent across an await.
        let (system, model, schema) = {
            let manifest = cx
                .manifest()
                .ok_or_else(|| SkillError::Other("specialist runs without a manifest".into()))?;
            let spec = &manifest.spec;

            let system = spec
                .identity
                .as_ref()
                .map(agentplane::manifest::Identity::system_prompt)
                .unwrap_or_default();

            let m = spec
                .models
                .as_ref()
                .and_then(|m| m.privileged.as_ref())
                .ok_or_else(|| SkillError::Other("manifest declares no privileged model".into()))?;

            (
                system,
                ModelId::new(&m.provider, &m.model),
                manifest.output_schema().cloned(),
            )
        };

        // The event payload is untrusted: it derives from inbound counterparty
        // EDIFACT. It goes in as *content*, never into `/system`.
        let prompt = Tainted::object([
            ("system".to_owned(), Tainted::trusted(json!(system))),
            ("event".to_owned(), input),
        ]);

        let mut call = ModelCall::new(Arc::clone(&self.provider), model, prompt.peek().clone())
            .with_max_sensitivity(Sensitivity::Internal);
        if let Some(schema) = schema {
            // Into the effect key: editing the schema makes a replay report
            // divergence rather than reinterpreting a stored answer.
            call = call.expecting(schema);
        }

        let answer = cx.sink(call, &prompt).await?;
        Ok(Outcome::done(answer.map(|c| {
            c.structured.unwrap_or_else(|| json!({ "text": c.text }))
        })))
    }
}

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
        assert_eq!(MANIFESTS.len(), 28, "one manifest per specialist");
        let mut failures = Vec::new();
        for (name, src) in MANIFESTS {
            match parse_manifest(src) {
                Ok(m) => {
                    assert_eq!(&m.metadata.name, name, "manifest name matches its file");
                    assert!(
                        m.spec.identity.is_some(),
                        "{name}: an agent without an identity has no prompt"
                    );
                    assert!(
                        m.spec
                            .models
                            .as_ref()
                            .and_then(|x| x.quarantined.as_ref())
                            .is_some(),
                        "{name}: the quarantined model is what reads counterparty text"
                    );
                }
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
        assert!(
            failures.is_empty(),
            "manifests failed to parse:\n{failures:#?}"
        );
    }

    #[test]
    fn the_gabi_manifest_parses_and_declares_what_the_skill_reads() {
        let m = parse_manifest(GABI_GAS_MANIFEST).expect("gabi-gas-agent.yaml parses");

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
