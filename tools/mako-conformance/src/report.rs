//! One line per check, and what the kit cannot check.

use std::fmt;
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

/// Every check of one run.
#[derive(Debug, Default)]
pub struct Report {
    checks: Mutex<Vec<Check>>,
}

impl Report {
    /// Record a check.
    pub fn push(&self, runner: Runner, name: impl Into<String>, outcome: Outcome) {
        self.checks
            .lock()
            .expect("the report lock is never poisoned")
            .push(Check {
                runner,
                name: name.into(),
                outcome,
            });
    }

    /// Record a check that held.
    pub fn held(&self, runner: Runner, name: impl Into<String>) {
        self.push(runner, name, Outcome::Held);
    }

    /// Record a gap.
    pub fn broken(&self, runner: Runner, name: impl Into<String>, why: impl Into<String>) {
        self.push(runner, name, Outcome::Broken(why.into()));
    }

    /// Record a check that could not be made.
    pub fn skipped(&self, runner: Runner, name: impl Into<String>, why: impl Into<String>) {
        self.push(runner, name, Outcome::Skipped(why.into()));
    }

    /// Every check, in the order they were made.
    #[must_use]
    pub fn checks(&self) -> Vec<Check> {
        self.checks
            .lock()
            .expect("the report lock is never poisoned")
            .clone()
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
