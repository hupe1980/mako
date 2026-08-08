//! The plane: event in, journaled agent run out.
//!
//! Replaces the hand-rolled orchestrator, session loop, registry, dead-letter
//! queue, model providers and MCP client with `agentplane`. What survives from
//! the old design is the part agentplane deliberately does not do: routing a
//! CloudEvent type to the specialists that subscribe to it.
//!
//! ## What runs where
//!
//! * **Routing** — [`Router`] matches an event type against each specialist's
//!   trigger patterns. agentplane has no notion of an event bus; this is mako's
//!   bridge from a `de.*` CloudEvent to `runtime.run(capability, payload)`.
//! * **The turn** — the runtime's tool-calling loop, driven entirely by the
//!   manifest: prompt, model pair, tool grants, ceilings and result schema.
//! * **Durability** — every model and tool call is a journaled effect. A crash
//!   resumes from the last completed effect; the old `dlq` existed because a
//!   failed session had nowhere to go.
//!
//! ## Fan-out
//!
//! When several specialists match one event they are independent opinions — a
//! billing event runs an anomaly check *and* a regulatory guard — so each gets
//! its own run and its own journal rather than sharing one. There is no
//! first-wins mode: abandoning an in-flight branch leaves a started effect with
//! no terminal record, which for a mutating tool call is an unrecoverable
//! unknown outcome.

use std::sync::Arc;

use agentplane::journal::JournalStore;
use agentplane::runtime::{Agent, RunStatus, Runtime};
use serde_json::Value;
use tracing::{info, warn};

use crate::plane::{MANIFESTS, parse_manifest};

/// Which compiled specialists a deployment activates.
///
/// Built from `[bundled_agents]`. Kept separate from the config type so the
/// router can be tested without constructing a whole `AgentdConfig`.
#[derive(Debug, Clone, Default)]
pub struct Activation {
    all: bool,
    names: Vec<String>,
}

impl Activation {
    /// Activate every specialist compiled into this binary.
    #[must_use]
    pub fn all() -> Self {
        Self {
            all: true,
            names: Vec::new(),
        }
    }

    /// Activate only the named specialists.
    #[must_use]
    pub fn named(names: Vec<String>) -> Self {
        Self { all: false, names }
    }

    /// Read the operator's choice from config.
    #[must_use]
    pub fn from_config(cfg: &crate::config::BundledAgentsConfig) -> Self {
        if cfg.enable_all {
            Self::all()
        } else {
            Self::named(cfg.enable.clone())
        }
    }

    /// Whether this specialist runs in this deployment.
    #[must_use]
    pub fn includes(&self, name: &str) -> bool {
        self.all || self.names.iter().any(|n| n == name)
    }

    /// Names enabled by the operator that match no compiled specialist.
    ///
    /// Returned rather than ignored: a typo, or a name from another role's
    /// build, would otherwise present as a specialist that simply never fires.
    fn unknown_names(&self) -> Vec<&str> {
        self.names
            .iter()
            .map(String::as_str)
            .filter(|n| !crate::builtin::all().any(|d| d.name == *n))
            .collect()
    }
}

/// One specialist's routing entry.
#[derive(Debug, Clone)]
pub struct Route {
    /// Specialist name, matching its manifest's `metadata.name`.
    pub name: &'static str,
    /// Capability the manifest provides — what `Runtime::run` is given.
    pub capability: String,
    /// CloudEvent type globs this specialist subscribes to.
    pub triggers: &'static [&'static str],
    /// Whether the manifest declares `execution.kind: planned`.
    ///
    /// It decides how the payload is admitted, so it is read once at startup
    /// rather than re-parsed per event: a planned agent receives only the
    /// re-validated routing envelope, a tool-calling one the whole payload with
    /// per-field labels.
    pub plans: bool,
}

/// Event type → specialists.
///
/// Built from the compiled trigger table and the embedded manifests, so a
/// specialist that subscribes to an event it has no manifest for is a startup
/// failure rather than an event that silently routes nowhere.
#[derive(Debug, Clone)]
pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    /// Build the routing table, pairing each activated specialist with its
    /// manifest.
    ///
    /// `activated` decides which compiled specialists this deployment runs. A
    /// specialist that is compiled in but not activated is absent from the table
    /// and receives no events.
    ///
    /// # Errors
    ///
    /// Returns an error naming every specialist whose manifest is missing or
    /// unparseable, that declares no capability, or that is named in `enable`
    /// but compiled into no build — the last is usually a name belonging to
    /// another Marktrolle's role-scoped binary.
    pub fn build(activated: &Activation) -> Result<Self, String> {
        let mut routes = Vec::new();
        let mut problems = Vec::new();

        for name in activated.unknown_names() {
            problems.push(format!(
                "{name}: enabled in [bundled_agents] but not compiled into this binary"
            ));
        }

        for def in crate::builtin::all().filter(|d| activated.includes(d.name)) {
            let Some((_, src)) = MANIFESTS.iter().find(|(n, _)| *n == def.name) else {
                problems.push(format!("{}: no manifest", def.name));
                continue;
            };
            match parse_manifest(src) {
                Ok(m) => match m.spec.capabilities.provides.first() {
                    Some(cap) => routes.push(Route {
                        name: def.name,
                        capability: cap.to_string(),
                        triggers: def.trigger_patterns,
                        plans: matches!(
                            m.spec.execution.as_ref().map(|e| e.kind),
                            Some(agentplane::manifest::ExecutionKind::Planned)
                        ),
                    }),
                    None => problems.push(format!("{}: manifest provides no capability", def.name)),
                },
                Err(e) => problems.push(format!("{}: {e}", def.name)),
            }
        }

        if problems.is_empty() {
            Ok(Self { routes })
        } else {
            Err(problems.join("; "))
        }
    }

    /// Specialists subscribing to `event_type`.
    #[must_use]
    pub fn matching(&self, event_type: &str) -> Vec<&Route> {
        self.routes
            .iter()
            .filter(|r| {
                r.triggers
                    .iter()
                    .any(|p| mako_events::matches(p, event_type))
            })
            .collect()
    }

    /// Every route, for health and inventory endpoints.
    #[must_use]
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
}

/// What one specialist concluded, in the shape the CloudEvent carries.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentDecision {
    pub agent_name: String,
    /// The run id — the journal key. Replaces the old opaque session UUID with
    /// something an operator can actually look up.
    pub session_id: String,
    pub event_id: String,
    pub event_type: String,
    /// `completed` · `failed` · `suspended` · `exhausted` · `quarantined`
    pub outcome: String,
    pub summary: String,
    pub tool_calls: usize,
    pub turns: u32,
    pub handoff_to: Option<String>,
}

impl AgentDecision {
    /// The CloudEvent an ERP subscriber receives.
    #[must_use]
    pub fn to_cloud_event(&self, tenant: &str) -> mako_service::CloudEvent {
        mako_service::CloudEvent::new(
            mako_service::source("agentd", tenant),
            mako_events::agent::DECISION_MADE,
            self.event_id.clone(),
            serde_json::to_value(self).unwrap_or(Value::Null),
        )
    }
}

/// The runtime and its routing table.
pub struct Plane {
    runtime: Arc<Runtime>,
    router: Router,
}

impl std::fmt::Debug for Plane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plane")
            .field("routes", &self.router.routes.len())
            .finish_non_exhaustive()
    }
}

impl Plane {
    /// Assemble the runtime from the embedded manifests.
    ///
    /// Every activated manifest is registered as an agent, so a run addressed to
    /// a capability finds the declaration that governs it.
    ///
    /// `tenant` scopes the **data keys**, so one operator's cryptographic
    /// erasure cannot reach another's bytes. `tool_servers` supplies one
    /// transport per MCP server named in the grants; the catalogue is derived
    /// from the declarations themselves, so a grant, its ceiling and its
    /// protected fields are stated once.
    ///
    /// # Errors
    ///
    /// Returns an error when the routing table cannot be built, a manifest fails
    /// to parse, or the runtime refuses to build — all are deployment errors,
    /// surfaced at startup. In particular a declarative agent whose tool servers
    /// are unreachable is refused here rather than failing identically on every
    /// run.
    pub fn new(
        store: Arc<dyn JournalStore>,
        owner: &str,
        tenant: &str,
        activated: &Activation,
        providers: Vec<(String, Arc<dyn agentplane::model::ModelProvider>)>,
        tool_servers: Vec<(String, Arc<dyn agentplane::tools::ToolClient>)>,
        keyring: Option<Arc<dyn agentplane::keyring::KeyRing>>,
    ) -> Result<Self, String> {
        let router = Router::build(activated)?;

        let mut builder = Runtime::builder(store)
            .owner(owner.to_owned())
            // The erasure unit is the case, and the key that opens it is scoped
            // by tenant. A plane that left this at the default would write one
            // operator's runs into another's keyspace.
            .tenant(
                agentplane::core::TenantId::new(tenant)
                    .map_err(|e| format!("tenant `{tenant}` is not a usable key scope: {e}"))?,
            );
        for (name, driver) in providers {
            builder = builder.provider(name, driver);
        }
        for (name, client) in tool_servers {
            builder = builder.tool_server(name, client);
        }
        if let Some(keys) = keyring {
            // Seals the journal, case state, events and task proposals — done at
            // `build`, so registration order cannot lose the guarantee.
            builder = builder.keyring(keys);
        }
        // Only activated specialists are registered. An agent the operator did
        // not enable is not merely unrouted — it has no declaration in the
        // runtime, so a run cannot address its capability by any other path.
        for (name, src) in MANIFESTS.iter().filter(|(n, _)| activated.includes(n)) {
            let m = Arc::new(parse_manifest(src).map_err(|e| format!("{name}: {e}"))?);
            builder = builder.agent(Agent::new(&m));
        }

        // `try_build`, not `build`: `build` panics on a wiring fault, and a
        // declarative agent whose tool servers are unreachable is exactly that.
        // A daemon should refuse to start with a diagnostic, not abort.
        let runtime = builder
            .try_build()
            .map_err(|e| format!("assemble the agent runtime: {e}"))?;

        Ok(Self { runtime, router })
    }

    /// Routing table, for health and inventory endpoints.
    #[must_use]
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// Run every specialist subscribing to this event.
    ///
    /// Each match is its own run on its own journal — independent opinions, not
    /// one run's internal concurrency. Returns one decision per specialist, in
    /// routing order; an empty result means nothing subscribed.
    pub async fn dispatch(
        &self,
        event_id: &str,
        event_type: &str,
        payload: Value,
    ) -> Vec<AgentDecision> {
        let matched = self.router.matching(event_type);
        if matched.is_empty() {
            info!(event_type, "no specialist subscribes to this event");
            return Vec::new();
        }

        let mut decisions = Vec::with_capacity(matched.len());
        for route in matched {
            decisions.push(
                self.run_one(route, event_id, event_type, payload.clone())
                    .await,
            );
        }
        decisions
    }

    /// Run one named specialist directly, bypassing routing.
    ///
    /// Returns `None` when no specialist by that name exists.
    pub async fn dispatch_one(
        &self,
        name: &str,
        event_id: &str,
        event_type: &str,
        payload: Value,
    ) -> Option<AgentDecision> {
        let route = self.router.routes.iter().find(|r| r.name == name)?;
        Some(self.run_one(route, event_id, event_type, payload).await)
    }

    /// A decision recording that the plane declined to start a run.
    ///
    /// Not a failure of the agent — the agent never ran. It is on the record
    /// because a specialist that silently receives nothing is indistinguishable
    /// from one that ran and found nothing.
    fn did_not_run(route: &Route, event_id: &str, event_type: &str, why: &str) -> AgentDecision {
        AgentDecision {
            agent_name: route.name.to_owned(),
            session_id: String::new(),
            event_id: event_id.to_owned(),
            event_type: event_type.to_owned(),
            outcome: "not-admitted".to_owned(),
            summary: why.to_owned(),
            tool_calls: 0,
            turns: 0,
            handoff_to: None,
        }
    }

    async fn run_one(
        &self,
        route: &Route,
        event_id: &str,
        event_type: &str,
        payload: Value,
    ) -> AgentDecision {
        // Admission is where mako's trust boundary is drawn. `Runtime::run`
        // would label the whole payload trusted, and almost nothing in it is:
        // a MaLo came out of a counterparty's UTILMD, a `reference` is text they
        // wrote. `run_tainted` carries the real labels in.
        let input = match route.plans {
            // A `planned` agent refuses untrusted input — the plan it compiles
            // is the authorization graph. It gets the re-validated identifiers
            // and reaches the rest through its granted tools.
            true => match super::label::routing_envelope(&payload) {
                Some(envelope) => envelope,
                None => {
                    warn!(
                        agent = route.name,
                        event_type,
                        "no re-validated identifier in the payload — a planned specialist \
                         has nothing to plan from"
                    );
                    return Self::did_not_run(
                        route,
                        event_id,
                        event_type,
                        "The event carried no identifier this plane could re-validate, so a \
                         planned specialist had no trusted input to compile a plan from.",
                    );
                }
            },
            false => super::label::admit(event_type, payload),
        };

        let outcome = self.runtime.run_tainted(&route.capability, input).await;

        let (run_id, status, summary) = match outcome {
            Ok(o) => {
                let summary = o
                    .output
                    .as_ref()
                    .map(|t| t.peek().to_string())
                    .unwrap_or_default();
                (o.run_id.to_string(), o.status, summary)
            }
            Err(e) => {
                warn!(agent = route.name, error = %e, "agent run failed to start");
                return AgentDecision {
                    agent_name: route.name.to_owned(),
                    session_id: String::new(),
                    event_id: event_id.to_owned(),
                    event_type: event_type.to_owned(),
                    outcome: "failed".to_owned(),
                    summary: e.to_string(),
                    tool_calls: 0,
                    turns: 0,
                    handoff_to: None,
                };
            }
        };

        // A suspended run is not a failure: it is waiting for a human decision
        // or an inbound event, and it costs a database row until then.
        let outcome_label = match &status {
            RunStatus::Succeeded => "completed",
            RunStatus::Failed(_) => "failed",
            RunStatus::Suspended(_) => "suspended",
            RunStatus::Exhausted(_) => "exhausted",
            RunStatus::Quarantined(_) => "quarantined",
            // A replan is in-flight, not an outcome; a cancellation was asked
            // for by an operator. Neither is a fault, and neither is success.
            RunStatus::Replanning(_) => "replanning",
            RunStatus::Cancelled { .. } => "cancelled",
        };

        if !matches!(status, RunStatus::Succeeded) {
            warn!(
                agent = route.name,
                run_id,
                outcome = outcome_label,
                "agent run did not complete"
            );
        }

        AgentDecision {
            agent_name: route.name.to_owned(),
            session_id: run_id,
            event_id: event_id.to_owned(),
            event_type: event_type.to_owned(),
            outcome: outcome_label.to_owned(),
            summary,
            tool_calls: 0,
            turns: 0,
            handoff_to: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every specialist that subscribes to an event has a manifest to run.
    ///
    /// The failure this prevents is silent: an event arrives, matches a trigger,
    /// and routes to a capability no agent provides. Building the table at
    /// startup turns that into a refusal to boot.
    #[test]
    fn the_routing_table_pairs_every_specialist_with_its_manifest() {
        let router =
            Router::build(&Activation::all()).expect("every specialist has a parseable manifest");
        assert_eq!(
            router.routes().len(),
            crate::builtin::all().count(),
            "one route per compiled specialist"
        );
        assert!(
            router.routes().iter().all(|r| !r.capability.is_empty()),
            "a route with no capability would run nothing"
        );
    }

    /// Fan-out is by subscription, and several specialists may share an event.
    #[test]
    fn one_event_can_match_several_specialists() {
        let router = Router::build(&Activation::all()).expect("routes");
        let matched = router.matching("de.mako.process.failed");
        assert!(
            !matched.is_empty(),
            "a process failure must reach at least one specialist"
        );
    }

    /// An event nothing subscribes to routes nowhere rather than to everything.
    #[test]
    fn an_unsubscribed_event_matches_nothing() {
        let router = Router::build(&Activation::all()).expect("routes");
        assert!(router.matching("de.nobody.listens.here").is_empty());
    }

    /// A specialist the operator did not enable receives nothing.
    ///
    /// The failure this prevents is the one the cutover introduced: routing
    /// built straight from the compiled table, so `[bundled_agents] enable`
    /// silently ran all 28 specialists in a deployment that asked for one.
    #[test]
    fn an_unactivated_specialist_is_absent_from_the_routing_table() {
        let only = Activation::named(vec!["mako-agent".to_owned()]);
        let router = Router::build(&only).expect("routes");

        assert_eq!(router.routes().len(), 1, "exactly the enabled specialist");
        assert_eq!(router.routes()[0].name, "mako-agent");

        // `processd-agent` also subscribes to this event, and must not appear.
        let matched = router.matching("de.mako.process.failed");
        assert!(
            matched.iter().all(|r| r.name == "mako-agent"),
            "an unenabled specialist matched anyway: {:?}",
            matched.iter().map(|r| r.name).collect::<Vec<_>>()
        );
    }

    /// A name that matches no compiled specialist refuses to boot.
    ///
    /// In a role-scoped build this is the common operator error — enabling an
    /// agent that exists only in another Marktrolle's binary. Failing at
    /// startup names it; ignoring it presents as an agent that never fires.
    #[test]
    fn enabling_an_unknown_specialist_is_a_startup_failure() {
        let err = Router::build(&Activation::named(vec!["no-such-agent".to_owned()]))
            .expect_err("an unknown name must not boot");
        assert!(err.contains("no-such-agent"), "the error names it: {err}");
    }

    /// Enabling nothing routes nothing, rather than defaulting to everything.
    #[test]
    fn activating_no_specialist_routes_no_events() {
        let router = Router::build(&Activation::named(Vec::new())).expect("routes");
        assert!(router.routes().is_empty());
        assert!(router.matching("de.mako.process.failed").is_empty());
    }
}
