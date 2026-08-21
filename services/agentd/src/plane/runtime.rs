//! The plane: event in, journaled agent run out.
//!
//! `agentplane` owns the session loop, the registry, durable execution, the
//! model providers and the MCP client. What agentd owns is the part agentplane
//! deliberately does not do: routing a CloudEvent type to the specialists that
//! subscribe to it.
//!
//! ## What runs where
//!
//! * **Routing** — [`Router`] matches an event type against each specialist's
//!   trigger patterns. agentplane has no notion of an event bus; this is mako's
//!   bridge from a `de.*` CloudEvent to `runtime.run(capability, payload)`.
//! * **The turn** — the runtime's tool-calling loop, driven entirely by the
//!   manifest: prompt, model pair, tool grants, ceilings and result schema.
//! * **Durability** — every model and tool call is a journaled effect, so a
//!   crash resumes from the last completed one. There is no dead-letter queue:
//!   a failed run has somewhere to go, which is back into itself.
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

use agentplane::case::{CaseStore, EventStore, TaskStore, TimerStore};
use agentplane::core::{PolicyEngine, TenantId};
use agentplane::journal::JournalStore;
use agentplane::keyring::KeyRing;
use agentplane::runtime::{Admission, Agent, RunStatus, Runtime};
use serde_json::Value;
use tracing::{info, warn};

use crate::plane::{find_manifest, manifests};

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
            let Some(embedded) = find_manifest(def.name) else {
                problems.push(format!("{}: no manifest", def.name));
                continue;
            };
            let m = embedded;
            match m.spec.capabilities.provides.first() {
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

    /// Whether any activated specialist subscribes to this event type.
    ///
    /// This is the **only** admission filter on `POST /webhook`. A second
    /// event-type list — one that must agree with the manifests and that nothing
    /// checks — is a subscription table with a second opinion, and an event it
    /// omits is answered `204 No Content` with nothing in any log distinguishing
    /// that from "no specialist subscribes".
    #[must_use]
    pub fn accepts(&self, event_type: &str) -> bool {
        self.routes.iter().any(|r| {
            r.triggers
                .iter()
                .any(|p| mako_events::matches(p, event_type))
        })
    }
}

/// The identity a redelivery of one CloudEvent keeps.
///
/// `POST /webhook` is at-least-once: an emitter retries until it sees a 2xx, so
/// the same message arrives more than once and must not start a second fan-out.
/// What makes two deliveries *the same message* is the CloudEvents pair
/// `(source, id)` — the standard's own uniqueness rule, and the one mako's
/// emitters hold stable across attempts (`mako_service`'s sender puts the
/// CloudEvent id in `webhook-id` for exactly this reason).
///
/// Borrowed rather than owned because every use is one dispatch's worth: this
/// exists to stop three `&str` parameters being transposed at a call site, not
/// to be stored.
#[derive(Debug, Clone, Copy)]
pub struct Envelope<'a> {
    /// CloudEvents `id`. Never empty — the door refuses an event without one,
    /// because an unset id arrives as `""` and `""` is a perfectly good
    /// admission key: the first message claims it and every later message is
    /// answered with the first one's run.
    pub id: &'a str,
    /// CloudEvents `source`. The producer half of the identity: an id is unique
    /// only within one emitter, so a bare id lets two services swallow each
    /// other's messages as apparent retries.
    pub source: &'a str,
    /// CloudEvents `type`, which the router matches against.
    pub event_type: &'a str,
}

impl Envelope<'_> {
    /// The admission key for **one specialist's** run of this event.
    ///
    /// Per specialist, not per event: one event fans out to several independent
    /// runs, so an event-wide key would admit the first specialist and answer
    /// every other one with its run.
    ///
    /// The `(source, id)` half is spelled the way agentplane spells it
    /// (`InboundEvent::dedup_key` — U+001F-joined), so the identity the buffer
    /// would use and the identity admission uses are the same string. They are
    /// deliberately not the same *table*: an event nobody subscribes to is
    /// dead-lettered by the sweep, and using the buffer as an idempotency
    /// ledger would spend that signal to buy deduplication.
    #[must_use]
    pub fn admission_key(&self, agent: &str) -> String {
        format!("{}\u{1f}{}\u{1f}{agent}", self.source, self.id)
    }
}

/// Whether this dispatch is the one that admitted the run.
///
/// Recorded rather than inferred: "the same decision twice in the log" and "a
/// redelivery answered with the original run" look identical without it, and
/// only the first is a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Admitted {
    /// This call is the one that started the run.
    Fresh,
    /// The key already admitted a run that has rested. Its answer, again.
    Replayed,
    /// The key already admitted a run that is still executing.
    InFlight,
}

/// What one specialist concluded.
///
/// Every field is read off the run's own outcome. There is deliberately no
/// per-turn count and no handoff target: the runtime drives the turn loop and
/// no specialist delegates, so either field would be structurally empty — and a
/// zero that reads as "this agent called no tools" is worse than no field.
///
/// **This is not the shape an ERP subscriber receives.** `de.agent.decision.made`
/// is delivered by the journal-backed outbox from the run's own sealed record —
/// see [`crate::plane::sweep::spawn_delivery`]. This is the operator's
/// "what just happened" view and the answer to a manual run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentDecision {
    pub agent_name: String,
    /// The run id — the journal key, and what `GET /api/v1/oversight/runs/{id}`
    /// takes. An operator can look up every effect behind this decision.
    pub run_id: String,
    pub event_id: String,
    pub event_type: String,
    /// `completed` · `failed` · `suspended` · `exhausted` · `quarantined` ·
    /// `replanning` · `cancelled` · `running` · `not-admitted`
    pub outcome: String,
    pub summary: String,
    /// Whether this dispatch admitted the run or met one already admitted.
    ///
    /// Absent when nothing was admitted at all — a refused envelope, a dispatch
    /// that timed out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admitted: Option<Admitted>,
    /// What a suspended run is waiting for — an approval, a message, an instant.
    ///
    /// "Suspended" tells an operator a run is stuck; it does not tell them
    /// whether to approve something, chase a counterparty, or wait. Present
    /// only when the run is suspended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    /// Model tokens this run consumed, as the journal metered them.
    pub tokens: u64,
}

/// Everything the plane persists to.
///
/// One backend supplies all five: the journal is the run's record, and the case
/// layer is where a matter, its obligations, its buffered events and its human
/// tasks live. Bundling them is not a convenience — a plane whose journal and
/// case store were different backends could resume a run whose case it cannot
/// read, and the failure would appear as an approval that never opens.
pub struct Stores {
    pub journal: Arc<dyn JournalStore>,
    pub cases: Arc<dyn CaseStore>,
    pub tasks: Arc<dyn TaskStore>,
    pub timers: Arc<dyn TimerStore>,
    pub events: Arc<dyn EventStore>,
    /// Where an outbox registration and its cursor live.
    pub push: Arc<dyn agentplane::push::PushStore>,
    /// Where `memory_formation` writes and `Recall` reads.
    ///
    /// Not optional: seven specialists declare `memory_formation`, and a
    /// memory-forming agent on a plane without a memory store is a **build
    /// refusal** — which is exactly how its absence was found. Every earlier
    /// suite activated only `gabi-gas-agent` (no memory block), so the plane
    /// assembled in tests and `enable_all` — the documented default — could
    /// not boot. The all-specialist smoke test exists so a seam a manifest
    /// declares can never again ship unwired.
    pub memory: Arc<dyn agentplane::memory::MemoryStore>,
}

impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stores").finish_non_exhaustive()
    }
}

impl Stores {
    /// The embedded backend: one redb file holds all five tables.
    ///
    /// The tenant is part of every key rather than a predicate somebody
    /// remembers to add, so it is bound to the *store* and not only to the
    /// runtime. agentplane refuses to build when the two disagree — a plane
    /// whose runs land in another tenant's keyspace while every erasure and
    /// policy request names this one is the failure that does not surface at
    /// runtime.
    #[must_use]
    pub fn redb(store: agentplane::store::RedbStore, tenant: &TenantId) -> Self {
        Self::from_arc(Arc::new(store.for_tenant(tenant.clone())))
    }

    /// The shared backend: several agentd instances on one database.
    ///
    /// The topology an embedded store cannot serve — fencing and exactly-once
    /// are arbitrated by Postgres rather than by hoping the writers agree.
    #[must_use]
    pub fn postgres(store: agentplane::store::PostgresStore, tenant: &TenantId) -> Self {
        Self::from_arc(Arc::new(store.for_tenant(tenant.clone())))
    }

    /// One backend, seven seams.
    fn from_arc<S>(store: Arc<S>) -> Self
    where
        S: JournalStore
            + CaseStore
            + TaskStore
            + TimerStore
            + EventStore
            + agentplane::push::PushStore
            + agentplane::memory::MemoryStore
            + 'static,
    {
        Self {
            journal: Arc::clone(&store) as Arc<dyn JournalStore>,
            cases: Arc::clone(&store) as Arc<dyn CaseStore>,
            tasks: Arc::clone(&store) as Arc<dyn TaskStore>,
            timers: Arc::clone(&store) as Arc<dyn TimerStore>,
            events: Arc::clone(&store) as Arc<dyn EventStore>,
            push: Arc::clone(&store) as Arc<dyn agentplane::push::PushStore>,
            memory: store as Arc<dyn agentplane::memory::MemoryStore>,
        }
    }
}

/// What a deployment decides about its plane, beyond the stores.
///
/// A struct rather than eight positional arguments: every field here is
/// something an operator configured, and a call site that transposes two
/// `Arc<dyn …>` parameters compiles.
pub struct PlaneConfig<'a> {
    /// Identifies this *process* for lease fencing — not the agent.
    pub owner: &'a str,
    /// Scopes the data keys, so one operator's erasure cannot reach another's.
    ///
    /// The same value the stores were built with — agentplane refuses the build
    /// if they disagree.
    pub tenant: &'a TenantId,
    /// Which compiled specialists this deployment runs.
    pub activated: &'a Activation,
    /// Model drivers, under the names the manifests use.
    pub providers: Vec<(String, Arc<dyn agentplane::model::ModelProvider>)>,
    /// One transport per MCP server named in the grants.
    pub tool_servers: Vec<(String, Arc<dyn agentplane::tools::ToolClient>)>,
    /// The authorization engine. Not optional: agentplane ships no `AllowAll`,
    /// and its operator surface refuses to open on an ungoverned plane.
    pub policy: Arc<dyn PolicyEngine>,
    /// Envelope encryption for everything written down, when configured.
    pub keyring: Option<Arc<dyn KeyRing>>,
    /// Where a completed run's decision is delivered, durably.
    ///
    /// `None` leaves runs unwatched — the decision is still in the journal, but
    /// nothing announces it. Configuring `audit_webhook_url` is what turns this
    /// on.
    pub outbox: Option<Arc<agentplane::push::Outbox>>,
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
    /// The case layer is wired unconditionally, and it is not optional in
    /// practice: asking a human needs a case to hold the task, a calendar to
    /// resolve the deadline and a timer to expire it. A plane without them
    /// assembles cleanly and then fails every `requires_approval` call at
    /// dispatch — which is the one path a test suite is least likely to reach.
    ///
    /// # Errors
    ///
    /// Returns an error when the routing table cannot be built, a manifest fails
    /// to parse, or the runtime refuses to build — all are deployment errors,
    /// surfaced at startup. In particular a declarative agent whose tool servers
    /// are unreachable is refused here rather than failing identically on every
    /// run.
    pub fn new(stores: Stores, cfg: PlaneConfig<'_>) -> Result<Self, String> {
        let router = Router::build(cfg.activated)?;

        let mut builder = Runtime::builder(stores.journal)
            .owner(cfg.owner.to_owned())
            // The erasure unit is the case, and the key that opens it is scoped
            // by tenant. A plane that left this at the default would write one
            // operator's runs into another's keyspace.
            .tenant(cfg.tenant.clone())
            // The matter a run belongs to, the humans it may have to ask, the
            // instants it waits for, and the messages that wake it.
            .cases(stores.cases)
            .tasks(stores.tasks)
            .timers(stores.timers)
            .events(stores.events)
            // Where the seven memory-forming specialists write and recall.
            // A manifest declaring `memory_formation` on a plane without this
            // is refused at build — the refusal that found the seam missing.
            .memory(stores.memory)
            // Werktage, resolved through mako's own BDEW holiday table — so an
            // agent's approval window and the regulatory window it guards
            // cannot disagree about when Karfreitag is.
            .calendar(Arc::new(super::calendar::MakoCalendar))
            // No `AllowAll` exists, and none is wanted: this is what every
            // effect is checked against.
            .policy(cfg.policy);
        for (name, driver) in cfg.providers {
            builder = builder.provider(name, driver);
        }
        for (name, client) in cfg.tool_servers {
            builder = builder.tool_server(name, client);
        }
        // Registrations are made at **admission**, so no run exists unwatched,
        // and delivery reads the run's own journal records past a cursor that
        // advances only on 2xx. This replaces a fire-and-forget POST at request
        // time: agentd was the one mako service emitting an event without
        // persist-before-dispatch, in a system whose whole argument is that the
        // journal is the plan of record.
        if let Some(outbox) = cfg.outbox {
            builder = builder.outbox(outbox);
        }
        if let Some(keys) = cfg.keyring {
            // Seals the journal, case state, events and task proposals — done at
            // `build`, so registration order cannot lose the guarantee.
            builder = builder.keyring(keys);
        }
        // Only *compiled and activated* specialists are registered. An agent the
        // operator did not enable is not merely unrouted — it has no declaration
        // in the runtime, so a run cannot address its capability by any other
        // path. The compiled-in filter matters in a role-scoped build: the
        // `manifests![]` embedding is not role-gated, so without it an
        // `enable_all` deployment of a `role-lf` binary would register the NB
        // and MSB specialists as addressable capabilities — unrouted, but
        // declared, with their grants counted as required wiring (§ 9 EnWG).
        for (name, declaration) in manifests().iter().filter(|(name, _)| {
            crate::builtin::find(name).is_some() && cfg.activated.includes(name)
        }) {
            let name = name.as_str();
            let m = Arc::new(declaration.clone());
            let mut agent = Agent::new(&m);
            // A specialist whose work is computation carries a coded skill
            // instead of an `execution` block. The manifest still governs it —
            // same grants, same ceilings, same digest — but the conduct is Rust,
            // so the thresholds are testable and no model is asked to subtract.
            if name == crate::skills::DeadlineTriage::NAME {
                // No wiring: `StepCtx::call_tool` dispatches through the plane's
                // own catalogue, which `try_build` derived from these manifests
                // and checked against them. A skill that carried its own could
                // grant itself reach the declaration never described.
                agent = agent.skill(crate::skills::DeadlineTriage::new());
            }
            builder = builder.agent(agent);
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

    /// The runtime itself — for the operator surface and the sweeper.
    ///
    /// Handed out rather than wrapped: the worklist, run views and event
    /// delivery are agentplane's own HTTP surface, and re-implementing them
    /// here would be a second copy of an authorization rule.
    #[must_use]
    pub fn runtime(&self) -> Arc<Runtime> {
        Arc::clone(&self.runtime)
    }

    // Inbound-message delivery — waking a run suspended on `await_event` — is
    // deliberately *not* wrapped here. It is `POST /api/v1/oversight/events` on
    // agentplane's own surface, authenticated and authorized like every other
    // operation there. A second door into the same store would be a second
    // place to get the authorization wrong, and delivering every webhook event
    // blindly would buffer messages nobody waits for until the sweeper
    // dead-letters them, turning a healthy signal into noise.

    /// Run every specialist subscribing to this event.
    ///
    /// Each match is its own run on its own journal — independent opinions, not
    /// one run's internal concurrency. Returns one decision per specialist, in
    /// routing order; an empty result means nothing subscribed.
    ///
    /// Admission is keyed, so a redelivery of the same `(source, id)` is
    /// answered with the runs it already started rather than starting more.
    pub async fn dispatch(&self, event: Envelope<'_>, payload: Value) -> Vec<AgentDecision> {
        let matched = self.router.matching(event.event_type);
        if matched.is_empty() {
            info!(
                event_type = event.event_type,
                "no specialist subscribes to this event"
            );
            return Vec::new();
        }

        let mut decisions = Vec::with_capacity(matched.len());
        for route in matched {
            decisions.push(self.run_one(route, event, payload.clone()).await);
        }
        decisions
    }

    /// Run one named specialist directly, bypassing routing.
    ///
    /// Returns `None` when no specialist by that name exists.
    pub async fn dispatch_one(
        &self,
        name: &str,
        event: Envelope<'_>,
        payload: Value,
    ) -> Option<AgentDecision> {
        let route = self.router.routes.iter().find(|r| r.name == name)?;
        Some(self.run_one(route, event, payload).await)
    }

    /// A decision recording that the plane declined to start a run.
    ///
    /// Not a failure of the agent — the agent never ran. It is on the record
    /// because a specialist that silently receives nothing is indistinguishable
    /// from one that ran and found nothing.
    ///
    /// It spends no admission key either, which is the point of admitting late:
    /// a refusal here leaves the key unclaimed, so a corrected redelivery is
    /// admitted rather than answered with the refusal.
    fn did_not_run(route: &Route, event: Envelope<'_>, why: &str) -> AgentDecision {
        AgentDecision {
            agent_name: route.name.to_owned(),
            run_id: String::new(),
            event_id: event.id.to_owned(),
            event_type: event.event_type.to_owned(),
            outcome: "not-admitted".to_owned(),
            summary: why.to_owned(),
            admitted: None,
            waiting_for: None,
            tokens: 0,
        }
    }

    async fn run_one(&self, route: &Route, event: Envelope<'_>, payload: Value) -> AgentDecision {
        let (event_id, event_type) = (event.id, event.event_type);
        // Read the business keys before the payload is consumed by labelling:
        // which case this run joins is a question of fact about the message,
        // decided the same way for every specialist that receives it.
        let correlation = super::label::correlation(event_id, &payload);

        // Admission is where mako's trust boundary is drawn, and 0.10 made the
        // label part of the value rather than part of the method name: every
        // door takes a `Tainted<Value>`. Almost nothing in a CloudEvent payload
        // is trusted — a MaLo came out of a counterparty's UTILMD, a `reference`
        // is text they wrote — so `plane::label` carries the real labels in.
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
                        event,
                        "The event carried no identifier this plane could re-validate, so a \
                         planned specialist had no trusted input to compile a plan from.",
                    );
                }
            },
            false => super::label::admit(event_type, payload),
        };

        // Correlated **and keyed**, neither alone.
        //
        // Correlated: a run outside a case cannot register an obligation or open
        // a task, so `requires_approval` would fail at dispatch instead of
        // asking a human. The keys are the re-validated identifiers, which makes
        // the case the erasure unit for one Marktlokation.
        //
        // Keyed: correlation decides *which case*, the key decides *whether at
        // all*. Correlation alone would join a redelivery to the right case and
        // start a second run inside it — and since the one dispatching grant
        // suspends on a four-eyes decision, that second run puts a second
        // identical Freigabe in front of a reviewer. The store claims the key
        // inside the transaction that appends the run's first record, so this
        // holds across instances and across a restart.
        let admitted = self
            .runtime
            .run_correlated_once(
                &route.capability,
                input,
                correlation.kind,
                &correlation.keys,
                &event.admission_key(route.name),
            )
            .await;

        // The answer when there is one; the *reason* when there is not. A failed
        // run with an empty summary reads as "the agent said nothing", when the
        // truth is "the runtime refused, and said why" — and the why is the only
        // actionable part.
        let told = |o: &agentplane::runtime::RunOutcome| {
            o.output
                .as_ref()
                .map(|t| t.peek().to_string())
                .or_else(|| o.reason().map(|r| r.into_owned()))
                .unwrap_or_default()
        };

        let (run_id, status, summary, tokens, admission) = match admitted {
            Ok(Admission::Fresh(o)) => (
                o.run_id.to_string(),
                o.status.clone(),
                told(&o),
                o.spend.tokens,
                Admitted::Fresh,
            ),
            // A duplicate is answered, not refused: a caller that retried wants
            // the original run. `Replayed` carries no output — a step's result
            // is reconstructed by replay rather than stored — so the summary is
            // the conclusion's reason, and its absence means the original run
            // succeeded and its answer is under `/oversight/runs/{id}`.
            Ok(Admission::Replayed(o)) => {
                info!(
                    agent = route.name,
                    event_id,
                    run_id = %o.run_id,
                    "this event was already admitted — answering with the original run"
                );
                let summary = o.reason().map_or_else(
                    || {
                        format!(
                            "This event was already admitted for {}. The original run's answer \
                             stands; read it at /api/v1/oversight/runs/{}.",
                            route.name, o.run_id
                        )
                    },
                    std::borrow::Cow::into_owned,
                );
                (
                    o.run_id.to_string(),
                    o.status.clone(),
                    summary,
                    o.spend.tokens,
                    Admitted::Replayed,
                )
            }
            // Still executing. The honest answer is *accepted, already in
            // progress*, naming the run — never a failure, because a
            // retry-provoking answer here is how a redelivery storm starts.
            Ok(Admission::InFlight(run)) => {
                info!(
                    agent = route.name,
                    event_id,
                    run_id = %run,
                    "this event is already being handled"
                );
                return AgentDecision {
                    agent_name: route.name.to_owned(),
                    run_id: run.to_string(),
                    event_id: event_id.to_owned(),
                    event_type: event_type.to_owned(),
                    outcome: "running".to_owned(),
                    summary: format!(
                        "This event was already admitted for {} and that run has not \
                         concluded yet; follow it at /api/v1/oversight/runs/{run}.",
                        route.name
                    ),
                    admitted: Some(Admitted::InFlight),
                    waiting_for: None,
                    tokens: 0,
                };
            }
            Err(e) => {
                warn!(agent = route.name, error = %e, "agent run failed to start");
                return AgentDecision {
                    agent_name: route.name.to_owned(),
                    run_id: String::new(),
                    event_id: event_id.to_owned(),
                    event_type: event_type.to_owned(),
                    outcome: "failed".to_owned(),
                    summary: e.to_string(),
                    // A refused admission spends no key — the key and the
                    // records commit together or not at all — so a corrected
                    // redelivery is admitted rather than answered with this.
                    admitted: None,
                    waiting_for: None,
                    tokens: 0,
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

        // Why it is waiting, not merely that it is. A run suspended on an
        // approval needs a reviewer; one waiting for an APERAK needs patience
        // or a counterparty chased. Both read as "suspended" without this.
        let waiting_for = match &status {
            RunStatus::Suspended(reason) => Some(reason.to_string()),
            _ => None,
        };

        // A replayed conclusion is old news: warning on it would page somebody
        // about a run they already looked at.
        if !matches!(status, RunStatus::Succeeded) && admission == Admitted::Fresh {
            warn!(
                agent = route.name,
                run_id,
                outcome = outcome_label,
                waiting_for = waiting_for.as_deref().unwrap_or(""),
                "agent run did not complete"
            );
        }

        AgentDecision {
            agent_name: route.name.to_owned(),
            run_id,
            event_id: event_id.to_owned(),
            event_type: event_type.to_owned(),
            outcome: outcome_label.to_owned(),
            summary,
            admitted: Some(admission),
            waiting_for,
            tokens,
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

    /// The webhook's admission filter is the routing table itself.
    ///
    /// Without this, an event a specialist subscribes to can be dropped at the
    /// door by a filter nobody reconciled with the manifests.
    #[test]
    fn the_admission_filter_accepts_exactly_what_a_specialist_subscribes_to() {
        let router = Router::build(&Activation::all()).expect("routes");

        // Every pattern any specialist declared is admitted. Concrete types are
        // used where the pattern is one; a glob is exercised through a member.
        for def in crate::builtin::all() {
            for pattern in def.trigger_patterns {
                let sample = mako_events::all()
                    .iter()
                    .find(|ev| mako_events::matches(pattern, ev))
                    .copied()
                    // A pattern with no emitter yet (UNEMITTED_PATTERNS) still
                    // has to be admitted, or wiring the emitter would not be
                    // enough to wake the specialist.
                    .unwrap_or(pattern);
                assert!(
                    router.accepts(sample),
                    "{}: subscribes to `{pattern}` but the webhook would drop `{sample}`",
                    def.name
                );
            }
        }

        assert!(
            !router.accepts("de.nobody.listens.here"),
            "an event nothing subscribes to is not admitted"
        );
    }

    /// Deactivating a specialist closes the door its subscriptions opened.
    #[test]
    fn an_unactivated_specialists_events_are_not_admitted() {
        let only = Activation::named(vec!["mako-agent".to_owned()]);
        let router = Router::build(&only).expect("routes");
        assert!(router.accepts(mako_events::mako::PROCESS_FAILED));
        assert!(
            !router.accepts(mako_events::obs::DEADLINE_APPROACHING),
            "deadline-alert-agent is not activated, so its events are not admitted"
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
