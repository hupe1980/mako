//! One line per check, and what the kit cannot check.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

use crate::profile::{Probe, Profile};
use std::sync::Mutex;

/// Which runner a check belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    /// Refusals, statuses and bodies against the OpenAPI document.
    Rest,
    /// Signed, schema-valid deliveries and inbound refusals.
    Events,
    /// The MCP tool list and refusals.
    Mcp,
}

impl Runner {
    /// The name `--only` takes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::Events => "events",
            Self::Mcp => "mcp",
        }
    }

    /// The runner `--only` names.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Rest, Self::Events, Self::Mcp]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}

/// How a check came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The contract held.
    Held,
    /// It did not: what was seen, and why it matters.
    Broken(String),
    /// It could not be checked here, and why — never counted as held.
    Skipped(String),
}

/// One check.
#[derive(Debug, Clone)]
pub struct Check {
    /// The runner that made it.
    pub runner: Runner,
    /// What was checked.
    pub name: String,
    /// How it came out.
    pub outcome: Outcome,
}

#[derive(Debug, Clone)]
struct Observation {
    probe: Probe,
    reason: Option<Reason>,
    outcome: Outcome,
}

/// Every observation and the immutable reference used for this run.
#[derive(Debug)]
pub struct Report {
    profile: Profile,
    checks: Mutex<Vec<Observation>>,
}

impl Report {
    pub(crate) fn new(profile: Profile) -> Self {
        Self {
            profile,
            checks: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, probe: Probe, reason: Option<Reason>, outcome: Outcome) {
        self.checks
            .lock()
            .expect("the report lock is never poisoned")
            .push(Observation {
                probe,
                reason,
                outcome,
            });
    }

    pub(crate) fn held(&self, probe: Probe) {
        self.record(probe, None, Outcome::Held);
    }

    pub(crate) fn broken(&self, probe: Probe, why: impl Into<String>) {
        self.record(
            probe,
            Some(Reason::ContractMismatch),
            Outcome::Broken(why.into()),
        );
    }

    pub(crate) fn skipped(&self, probe: Probe, reason: Reason) {
        self.record(
            probe,
            Some(reason),
            Outcome::Skipped(reason.description().into()),
        );
    }

    pub(crate) fn required(&self, probe: &Probe) -> bool {
        self.profile.expected.get(probe) == Some(&true)
    }

    /// Every diagnostic check, in the order it was observed.
    #[must_use]
    pub fn checks(&self) -> Vec<Check> {
        self.checks
            .lock()
            .expect("the report lock is never poisoned")
            .iter()
            .map(|o| Check {
                runner: o.probe.runner(),
                name: o.probe.name(),
                outcome: o.outcome.clone(),
            })
            .collect()
    }

    /// Close the selected inventory against the observations and bind it to the asserted build.
    ///
    /// # Errors
    ///
    /// When the label exceeds the portable build identifier vocabulary.
    #[must_use = "the assessment may be incomplete or failed"]
    pub fn acceptance(&self, label: &str) -> anyhow::Result<Evidence> {
        anyhow::ensure!(
            valid_target_label(label),
            "target build must be 1–64 ASCII letters, digits, periods, underscores or hyphens"
        );
        let observations = self
            .checks
            .lock()
            .expect("the report lock is never poisoned");
        Ok(Evidence::reduce(&self.profile, &observations, label))
    }

    /// The broken checks.
    #[must_use]
    pub fn gaps(&self) -> Vec<Check> {
        self.checks()
            .into_iter()
            .filter(|c| matches!(c.outcome, Outcome::Broken(_)))
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let checks = self.checks();
        for c in &checks {
            match &c.outcome {
                Outcome::Held => writeln!(f, "✓ [{}] {}", c.runner.as_str(), c.name)?,
                Outcome::Broken(why) => {
                    writeln!(f, "✗ [{}] {}\n    {why}", c.runner.as_str(), c.name)?;
                }
                Outcome::Skipped(why) => {
                    writeln!(f, "– [{}] {} — {why}", c.runner.as_str(), c.name)?;
                }
            }
        }
        let count = |m: fn(&Outcome) -> bool| checks.iter().filter(|c| m(&c.outcome)).count();
        writeln!(
            f,
            "\n{} held, {} broken, {} not checked.",
            count(|o| matches!(o, Outcome::Held)),
            count(|o| matches!(o, Outcome::Broken(_))),
            count(|o| matches!(o, Outcome::Skipped(_))),
        )?;
        write!(f, "{LIMIT}")
    }
}

/// What a passing run does not mean.
pub const LIMIT: &str = "\
Conformance is the contract's shape — routes, refusals, bodies, signed and \
schema-valid events, MCP tools. It is not mako's regulatory behaviour: a \
replacement that passes may still send an Anmeldung a held Kündigung forbids \
or bill past a Zuordnungsende. Those rules are proven by mako's own tests.\n";

/// Whether a caller-asserted, non-secret build identifier can be exported.
#[must_use]
pub fn valid_target_label(label: &str) -> bool {
    (1..=64).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    ContractMismatch,
    DuplicateObservation,
    UnexpectedObservation,
    MissingObservation,
    MissingRefusedCredential,
    MissingInboundSecret,
    DeliveryUnobserved,
    SchemaUnavailable,
    UnobservedBody,
    AuthNotReached,
    NoReferenceBody,
    PartialScope,
    EmptyRequiredSet,
}

impl Reason {
    fn description(self) -> &'static str {
        match self {
            Self::ContractMismatch => "Observed behavior differs from the reference contract",
            Self::DuplicateObservation => "Duplicate observation for a declared check",
            Self::UnexpectedObservation => "Observation outside the reference inventory",
            Self::MissingObservation => "Required check has no observation",
            Self::MissingRefusedCredential => "Missing refused-role credential",
            Self::MissingInboundSecret => "Missing inbound signing secret",
            Self::DeliveryUnobserved => "Required delivery was not observed",
            Self::SchemaUnavailable => "Published event schema is missing",
            Self::UnobservedBody => "No successful response with the required body schema",
            Self::AuthNotReached => "Request validation ran before the credential check",
            Self::NoReferenceBody => "Reference declares no successful JSON body",
            Self::PartialScope => "Only one runner was selected",
            Self::EmptyRequiredSet => "The selected profile contains no required checks",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::MissingRefusedCredential => {
                "Supply --refused-token for a test principal the service refuses, then rerun full acceptance."
            }
            Self::MissingInboundSecret => {
                "Supply --inbound-secret for the test deployment, then rerun full acceptance."
            }
            Self::DeliveryUnobserved => {
                "Check the test listener, callback and seed, then rerun full acceptance."
            }
            Self::SchemaUnavailable => {
                "Add the published schema to the reference kit, then rerun full acceptance."
            }
            Self::PartialScope => "Remove --only and rerun full acceptance.",
            Self::EmptyRequiredSet => "Select a reference suite with required checks.",
            Self::DuplicateObservation | Self::UnexpectedObservation => {
                "Check the runner against the reference inventory, then rerun full acceptance."
            }
            Self::AuthNotReached => {
                "Supply a valid test request for this operation, then rerun full acceptance."
            }
            _ => {
                "Inspect the named check on the isolated test deployment, then rerun full acceptance."
            }
        }
    }
}

/// Full-scope interface verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every required check held.
    Passed,
    /// A contract check or inventory integrity check failed.
    Failed,
    /// Required evidence is missing.
    Incomplete,
}

/// The required partition, plus reference-declared exceptions.
#[derive(Debug, Default, Serialize)]
pub struct Counts {
    /// Selected checks requiring evidence.
    pub required: usize,
    /// Required checks with positive evidence.
    pub held: usize,
    /// Required checks with negative or duplicate evidence.
    pub broken: usize,
    /// Required checks awaiting evidence.
    pub unknown: usize,
    /// Selected checks excluded by the reference document.
    pub reference_inapplicable: usize,
    /// Observations outside the selected required inventory; excluded from its denominator.
    pub unexpected: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Held,
    Broken,
    Unknown,
    ReferenceInapplicable,
}

#[derive(Debug, Serialize)]
struct Finding {
    id: String,
    state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<Reason>,
}

#[derive(Debug, Serialize)]
struct TargetBuild {
    label: String,
    origin: &'static str,
}

/// Deterministic, shareable representation with no transport diagnostics.
#[derive(Debug, Serialize)]
pub struct Evidence {
    format_version: u8,
    claim: &'static str,
    kit_version: &'static str,
    service: String,
    reference_fingerprint: String,
    reference_inputs: BTreeMap<String, Option<String>>,
    target_build: TargetBuild,
    scope: &'static str,
    /// Full-scope verdict.
    pub verdict: Verdict,
    /// Partition of the selected reference inventory.
    pub counts: Counts,
    checks: Vec<Finding>,
    assessment_findings: Vec<Finding>,
}

impl Evidence {
    fn reduce(profile: &Profile, observations: &[Observation], label: &str) -> Self {
        let mut counted = BTreeMap::<&Probe, Vec<&Observation>>::new();
        let mut unexpected = 0;
        for observation in observations {
            if profile.expected.get(&observation.probe) == Some(&true) {
                counted
                    .entry(&observation.probe)
                    .or_default()
                    .push(observation);
            } else {
                unexpected += 1;
            }
        }
        let mut counts = Counts::default();
        let mut checks = Vec::new();
        for (probe, required) in &profile.expected {
            let (state, reason) = if *required {
                counts.required += 1;
                match counted.get(probe).map(Vec::as_slice).unwrap_or_default() {
                    [] => (State::Unknown, Some(Reason::MissingObservation)),
                    [observation] => (
                        match observation.outcome {
                            Outcome::Held => State::Held,
                            Outcome::Broken(_) => State::Broken,
                            Outcome::Skipped(_) => State::Unknown,
                        },
                        observation.reason,
                    ),
                    _ => (State::Broken, Some(Reason::DuplicateObservation)),
                }
            } else {
                (State::ReferenceInapplicable, Some(Reason::NoReferenceBody))
            };
            match state {
                State::Held => counts.held += 1,
                State::Broken => counts.broken += 1,
                State::Unknown => counts.unknown += 1,
                State::ReferenceInapplicable => counts.reference_inapplicable += 1,
            }
            checks.push(Finding {
                id: probe.id(),
                state,
                reason,
            });
        }
        checks.sort_by(|a, b| a.id.cmp(&b.id));
        let mut assessment_findings = Vec::new();
        counts.unexpected = unexpected;
        if unexpected > 0 {
            assessment_findings.push(Finding {
                id: "inventory.unexpected".into(),
                state: State::Broken,
                reason: Some(Reason::UnexpectedObservation),
            });
        }
        if profile.only.is_some() {
            assessment_findings.push(Finding {
                id: "scope.partial".into(),
                state: State::Unknown,
                reason: Some(Reason::PartialScope),
            });
        }
        if counts.required == 0 {
            assessment_findings.push(Finding {
                id: "scope.empty".into(),
                state: State::Unknown,
                reason: Some(Reason::EmptyRequiredSet),
            });
        }
        assessment_findings.sort_by(|a, b| a.id.cmp(&b.id));
        let verdict = if counts.broken > 0 || unexpected > 0 {
            Verdict::Failed
        } else if counts.unknown > 0 || !assessment_findings.is_empty() {
            Verdict::Incomplete
        } else {
            Verdict::Passed
        };
        Self {
            format_version: 1,
            claim: "declared_interface_checks",
            kit_version: env!("CARGO_PKG_VERSION"),
            service: profile.service.clone(),
            reference_fingerprint: profile.fingerprint.clone(),
            reference_inputs: profile.materials.clone(),
            target_build: TargetBuild {
                label: label.into(),
                origin: "caller_asserted",
            },
            scope: profile.only.map_or("full", Runner::as_str),
            verdict,
            counts,
            checks,
            assessment_findings,
        }
    }

    /// The published acceptance CLI status.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self.verdict {
            Verdict::Passed => 0,
            Verdict::Failed => 1,
            Verdict::Incomplete => 3,
        }
    }
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = match self.verdict {
            Verdict::Passed => "PASSED",
            Verdict::Failed => "FAILED",
            Verdict::Incomplete => "INCOMPLETE",
        };
        let c = &self.counts;
        writeln!(
            f,
            "{verdict}  {}  |  {} required, {} held, {} broken, {} unknown, {} reference-inapplicable",
            self.service, c.required, c.held, c.broken, c.unknown, c.reference_inapplicable
        )?;
        let level = match self.verdict {
            Verdict::Failed => State::Broken,
            Verdict::Incomplete => State::Unknown,
            Verdict::Passed => return Ok(()),
        };
        if let Some(blocker) = self
            .checks
            .iter()
            .chain(&self.assessment_findings)
            .filter(|c| c.state == level)
            .min_by_key(|c| &c.id)
            && let Some(reason) = blocker.reason
        {
            writeln!(f, "{}  |  {}", blocker.id, reason.description())?;
            write!(f, "Next step: {}", reason.hint())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(items: &[(Probe, bool)]) -> Profile {
        Profile {
            service: "fixture".into(),
            only: None,
            expected: items.iter().cloned().collect(),
            fingerprint: "fixture".into(),
            materials: BTreeMap::new(),
        }
    }

    fn fixture(only: Option<Runner>, items: &[(Probe, bool)]) -> Report {
        let mut p = profile(items);
        p.only = only;
        Report::new(p)
    }

    #[test]
    fn acceptance_requires_a_complete_nonempty_full_scope() {
        let a = Probe::McpAuth;
        let b = Probe::Tools;
        let p = [(a.clone(), true), (b.clone(), false)];
        let full = fixture(None, &p);
        full.held(a.clone());
        let held = full.acceptance("build-2").unwrap();
        assert_eq!(held.verdict, Verdict::Passed);
        assert_eq!(
            (held.counts.held, held.counts.reference_inapplicable),
            (1, 1)
        );
        for partial in [
            fixture(Some(Runner::Mcp), &p),
            fixture(None, &[]),
            fixture(None, &[(b, false)]),
        ] {
            if partial.required(&a) {
                partial.held(a.clone());
            }
            assert_eq!(partial.acceptance("build-2").unwrap().exit_code(), 3);
        }
        let missing = fixture(None, &p);
        let e = missing.acceptance("fixture").unwrap();
        assert_eq!(e.counts.unknown, 1);
        assert_eq!(e.exit_code(), 3);
        missing.skipped(a.clone(), Reason::MissingRefusedCredential);
        assert!(
            missing
                .acceptance("fixture")
                .unwrap()
                .to_string()
                .contains("--refused-token")
        );
    }

    #[test]
    fn reordered_mixed_observations_cannot_leak_target_metadata() {
        let a = Probe::McpAuth;
        let b = Probe::Tools;
        let c = Probe::Tool("declared".into());
        let p = [(a.clone(), true), (b.clone(), true), (c, true)];
        let mut encodings = Vec::new();
        for reversed in [false, true] {
            let report = fixture(None, &p);
            let garbage = Probe::Tool("https://marker-secret.local/private-customer".into());
            if reversed {
                report.held(garbage.clone());
            }
            report.broken(a.clone(), "https://marker-secret.local/private-customer");
            report.skipped(b.clone(), Reason::MissingRefusedCredential);
            if !reversed {
                report.held(garbage);
            }
            let evidence = report.acceptance("build_2026.11").unwrap();
            assert_eq!(evidence.exit_code(), 1);
            assert_eq!(
                (
                    evidence.counts.required,
                    evidence.counts.broken,
                    evidence.counts.unknown
                ),
                (3, 1, 2)
            );
            let bytes = serde_json::to_string(&evidence).unwrap();
            assert!(!bytes.contains("marker-secret"));
            assert!(!bytes.contains("private-customer"));
            assert!(!evidence.to_string().contains("marker-secret"));
            assert!(bytes.contains("unexpected_observation"));
            encodings.push(bytes);
        }
        assert_eq!(encodings[0], encodings[1]);
    }

    #[test]
    fn duplicate_observations_fail_even_when_both_held() {
        let report = fixture(None, &[(Probe::Seed, true)]);
        report.held(Probe::Seed);
        report.held(Probe::Seed);
        let evidence = report.acceptance("fixture").unwrap();
        assert_eq!(evidence.exit_code(), 1);
        assert_eq!(evidence.counts.broken, 1);
        assert!(evidence.to_string().contains("Duplicate observation"));
    }

    #[test]
    fn build_label_has_an_ascii_vocabulary_and_length_limit() {
        for label in [
            "",
            "build 4",
            "https://example.test",
            "../release",
            "Über",
            &"a".repeat(65),
        ] {
            assert!(!valid_target_label(label));
        }
        assert!(valid_target_label(&"9".repeat(64)));
    }
}
