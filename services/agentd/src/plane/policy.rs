//! The authorization gate, loaded and compiled at startup.
//!
//! agentplane has no `AllowAll` — a permissive engine and no engine are the same
//! behaviour, so it ships neither — and its HTTP surface refuses to start on an
//! ungoverned plane. That makes this module load-bearing twice: it is what an
//! agent's effects are checked against, and it is what makes the operator
//! worklist openable at all.
//!
//! ## Where the rules come from
//!
//! [`DEFAULT_POLICY`] is embedded from `policy/agentd.cedar`, so the rules ship
//! with the binary they were reviewed against and a deployment that forgets to
//! mount a policy file is governed rather than open. An operator may point
//! `[policy] path` at their own file to replace it — *replace*, not extend: two
//! policy sets that both permit are the union of their permits, and a
//! least-privilege file that silently inherited a broader one is the failure
//! mode this avoids.

use std::sync::Arc;

use agentplane::core::{PolicyDecision, PolicyEngine, PolicyRequest};
use agentplane::policy::CedarEngine;

/// mako's own rules, embedded at compile time.
pub const DEFAULT_POLICY: &str = include_str!("../../policy/agentd.cedar");

/// The verbs **agentd's own** doors ask about.
///
/// agentplane's operator surface enumerates its verbs in
/// [`agentplane::api::action::ALL`] and asks the same engine about each. The
/// three routes agentd serves itself — starting a run by hand, and the two
/// inventory reads — had no verb at all: they took a `Claims` extractor and
/// stopped there, so any principal the realm would issue a token to could start
/// any specialist on any Marktlokation, spend the tenant's model budget, and
/// read which Marktrolle a role-scoped deployment is.
///
/// Authentication is not authorization, and a door with only the first is open
/// to everyone the IDP knows.
pub mod action {
    /// `POST /api/v1/run` — start a specialist by hand.
    ///
    /// Distinct from `run:admit`, which asks whether *an agent* may be admitted
    /// at all and whose principal is the agent. This one's principal is a person
    /// (or a scheduler's service key), and it is the verb that decides whether
    /// they may spend a run.
    pub const RUN_START: &str = "api:run.start";

    /// `GET /api/v1/agents` and `/api/v1/agents/catalog` — what this deployment
    /// runs, and what it could run.
    ///
    /// In a role-scoped build the activated set *is* the deployment's Marktrolle
    /// (§§ 6a, 7a EnWG), so it is deployment detail rather than public capability
    /// advertising. The A2A Agent Cards under `/.well-known/agents/{name}` stay
    /// open: a card is what an agent is, and carries no endpoint credential.
    pub const AGENT_LIST: &str = "api:agent.list";

    /// `POST /api/v1/erasure` — destroy case keys and forget memory subjects.
    pub const ERASURE_EXECUTE: &str = "api:erasure.execute";

    /// Every verb agentd's own surface asks about.
    ///
    /// Walked by a test against the policy set for the same reason agentplane
    /// publishes its own list: a verb granted to nobody is a permanent 403 on a
    /// route, behind a policy set that compiles clean.
    pub const ALL: &[&str] = &[RUN_START, AGENT_LIST, ERASURE_EXECUTE];
}

/// Whether this caller may perform `action` on agentd's own surface.
///
/// The roles come from the verified token and nothing else — the same rule the
/// oversight surface follows, and for the same reason: a caller who can name
/// their own roles has authorization in name only.
///
/// Fails closed. A `PolicyDecision` that is not `Permit` — a denial, or a rule
/// that could not be evaluated — is a refusal.
#[must_use]
pub fn caller_may(
    engine: &dyn PolicyEngine,
    subject: &str,
    roles: &[String],
    tenant: &str,
    action: &str,
) -> bool {
    let context = serde_json::json!({ "roles": roles, "tenant": tenant });
    matches!(
        engine.authorize(&PolicyRequest {
            principal: subject,
            action,
            resource: "agentd",
            context: &context,
        }),
        PolicyDecision::Permit
    )
}

/// Compile a policy set.
///
/// `source` is the operator's file when one is configured, and [`DEFAULT_POLICY`]
/// otherwise.
///
/// # Errors
///
/// Returns the Cedar diagnostic when the source does not parse. Startup is the
/// only honest place to fail: a policy set that cannot be compiled would
/// otherwise become a plane that denies every effect for a reason no operator
/// can read off a run.
pub fn engine(source: &str) -> Result<Arc<dyn PolicyEngine>, String> {
    let engine =
        CedarEngine::new(source).map_err(|e| format!("compile the agentd policy set: {e}"))?;
    Ok(Arc::new(engine) as Arc<dyn PolicyEngine>)
}

/// Read the operator's policy file, or fall back to the embedded rules.
///
/// # Errors
///
/// Returns an error when a configured path cannot be read. A missing file is a
/// deployment fault and not a reason to quietly fall back — an operator who
/// mounted a file expects *their* rules, and silently running mako's would be a
/// plane governed by something nobody chose.
pub fn source(path: Option<&str>) -> Result<String, String> {
    match path {
        Some(p) => {
            std::fs::read_to_string(p).map_err(|e| format!("read the Cedar policy at {p}: {e}"))
        }
        None => Ok(DEFAULT_POLICY.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> Arc<dyn PolicyEngine> {
        super::engine(DEFAULT_POLICY).expect("the embedded policy set compiles")
    }

    fn permitted(d: &PolicyDecision) -> bool {
        matches!(d, PolicyDecision::Permit)
    }

    /// The shipped rules compile. Without this the first thing a deployment
    /// learns about a syntax error is that it will not boot.
    #[test]
    fn the_embedded_policy_set_compiles() {
        let _ = engine();
    }

    /// A read against one of mako's own services is permitted.
    ///
    /// The baseline has to actually work: a policy set that denied ordinary
    /// reads would be discovered as "the agents do nothing", which reads as a
    /// wiring fault rather than an authorization one.
    #[test]
    fn a_read_on_a_mako_server_is_permitted() {
        let context = json!({
            "run": "r1", "step": 0, "tenant": "9900357000004",
            "mutates": false,
            "args": { "server": "marktd", "tool": "get_malo", "arguments": {} },
            "label": { "provenance": [], "trust": "trusted", "sensitivity": "internal" },
            "delegation_depth": 0,
        });
        let d = engine().authorize(&PolicyRequest {
            principal: "gabi-gas-agent",
            action: "effect:perform",
            resource: "tool.call",
            context: &context,
        });
        assert!(permitted(&d), "an ordinary read was denied: {d:?}");
    }

    /// A mutating call with model-written arguments is **permitted here**, and
    /// the reason is the most important comment in the policy file.
    ///
    /// The tempting rule — `forbid when { context.mutates && label.trust ==
    /// "untrusted" }` — denies every mutating call a tool-calling agent will
    /// ever make, because the arguments were written by a model that had just
    /// read a counterparty's event. What binds instead is per-argument:
    /// `protected_fields` with `require_trusted` on `/malo_id`, `/pid` and
    /// `/mp_id`, plus a named human on every mutating grant.
    ///
    /// This test exists so that re-adding the rule fails here rather than in
    /// production, where it presents as agents that run, succeed, and never do
    /// anything.
    #[test]
    fn a_mutating_call_on_model_written_arguments_is_not_denied_by_policy() {
        let context = json!({
            "run": "r1", "step": 3, "tenant": "9900357000004",
            "mutates": true,
            "args": { "server": "makod", "tool": "submit_command", "arguments": {} },
            "label": {
                "provenance": ["cloudevent:de.mako.process.failed"],
                "trust": "untrusted",
                "sensitivity": "internal",
            },
            "delegation_depth": 0,
        });
        let d = engine().authorize(&PolicyRequest {
            principal: "mako-agent",
            action: "effect:perform",
            resource: "tool.call",
            context: &context,
        });
        assert!(
            permitted(&d),
            "the policy layer must not blanket-deny mutating calls — the control that \
             binds is protected_fields plus human approval: {d:?}"
        );
    }

    /// **Every server a manifest grants must be reachable through the policy.**
    ///
    /// A granted server missing from the allowlist is the quietest failure
    /// there is: startup succeeds — the wiring check compares grants against
    /// `[mcp_servers]`, not against the policy — the agent registers, routes and
    /// runs, and every one of its tool calls is denied. A specialist answering
    /// from the model alone, with no evidence behind it, is worse than one that
    /// does not run.
    ///
    /// The server list is read from the manifests rather than restated here, so
    /// a new grant fails this test until the policy set admits it.
    #[test]
    fn every_granted_server_is_reachable() {
        let engine = engine();
        let denied: Vec<String> = crate::plane::tools::servers_named_in_grants()
            .into_iter()
            .filter(|server| {
                let context = json!({
                    "run": "r1", "step": 0, "tenant": "9900357000004",
                    "mutates": false,
                    "args": { "server": server, "tool": "any_read", "arguments": {} },
                    "label": { "provenance": [], "trust": "trusted", "sensitivity": "internal" },
                });
                !permitted(&engine.authorize(&PolicyRequest {
                    principal: "mako-agent",
                    action: "effect:perform",
                    resource: "tool.call",
                    context: &context,
                }))
            })
            .collect();

        assert!(
            denied.is_empty(),
            "these MCP servers are granted by a specialist manifest but the Cedar set \
             refuses every call to them — the specialists boot, run, and can reach none of \
             their tools: {denied:?}\n\n\
             Add them to the `unless` list in policy/agentd.cedar. Remember agentplane \
             reserves `-` in a tool:// server component, so a hyphenated service is spelled \
             with an underscore."
        );
    }

    /// A tool call to a server outside mako is refused even when trusted.
    ///
    /// The failure this prevents is a mistyped `[mcp_servers]` entry: the
    /// transport would connect and the agent would talk to it happily.
    #[test]
    fn a_tool_call_to_an_unknown_server_is_denied() {
        let context = json!({
            "run": "r1", "step": 1, "tenant": "9900357000004",
            "mutates": false,
            "args": { "server": "pastebin", "tool": "get", "arguments": {} },
            "label": { "provenance": [], "trust": "trusted", "sensitivity": "public" },
            "delegation_depth": 0,
        });
        let d = engine().authorize(&PolicyRequest {
            principal: "mako-agent",
            action: "effect:perform",
            resource: "tool.call",
            context: &context,
        });
        assert!(!permitted(&d), "a foreign MCP server was reachable: {d:?}");
    }

    /// Secrets do not reach a model, whatever a manifest's egress ceiling says.
    #[test]
    fn secret_material_may_not_reach_a_model() {
        let context = json!({
            "run": "r1", "step": 0, "tenant": "9900357000004",
            "mutates": false,
            "args": {},
            "label": { "provenance": [], "trust": "trusted", "sensitivity": "secret" },
            "delegation_depth": 0,
        });
        let d = engine().authorize(&PolicyRequest {
            principal: "mako-agent",
            action: "effect:perform",
            resource: "model.complete",
            context: &context,
        });
        assert!(!permitted(&d), "a secret reached a model call: {d:?}");
    }

    /// Declassification is refused: mako has no reviewed path for it.
    #[test]
    fn declassification_is_refused() {
        let context = json!({ "run": "r1", "step": 0, "tenant": "9900357000004" });
        let d = engine().authorize(&PolicyRequest {
            principal: "mako-agent",
            action: "data:release",
            resource: "label.release",
            context: &context,
        });
        assert!(!permitted(&d), "an untrusted value was relabelled: {d:?}");
    }

    /// Every role a manifest names can actually reach the worklist.
    ///
    /// A triage rule whose audience this set does not admit opens worklist rows
    /// nobody can see or decide. An oversight control that reads as configured
    /// and cannot be exercised is worse than none, because review sees it and
    /// stops asking.
    ///
    /// The roles are read from the embedded manifests, not restated here: a new
    /// audience in a manifest fails this test until the Cedar set admits it.
    #[test]
    fn every_role_the_manifests_name_can_reach_the_worklist() {
        let mut roles: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for m in crate::plane::manifests().values() {
            let Some(oversight) = m.spec.oversight.as_ref() else {
                continue;
            };
            roles.extend(oversight.approvers.iter().cloned());
            // The escalation audience counts. It is the one that arrives late
            // and unannounced: a role named only here is handed a row by the
            // sweeper, hours after the fact, and a Cedar set that does not
            // admit it turns escalation into a widening to nobody — which
            // reads, from the worklist, exactly like the row having been
            // answered.
            roles.extend(oversight.escalate_to.iter().cloned());
            for rule in &oversight.triage {
                roles.extend(rule.audience.iter().cloned());
            }
        }
        assert!(
            roles.len() >= 10,
            "the manifests name a spread of audiences; parsing found only {roles:?}"
        );

        let engine = engine();
        for role in &roles {
            let context = json!({ "roles": [role], "tenant": "9900357000004" });
            for action in [
                "api:task.list",
                "api:task.read",
                "api:task.claim",
                "api:task.release",
                "api:task.decide",
                "api:run.read",
                "api:case.read",
            ] {
                let d = engine.authorize(&PolicyRequest {
                    principal: "user:reviewer",
                    action,
                    resource: "*",
                    context: &context,
                });
                assert!(
                    permitted(&d),
                    "role `{role}` is named by a manifest (approver or triage audience) but \
                     the Cedar set refuses it `{action}` — its worklist rows exist and \
                     nobody can ever see or decide them"
                );
            }
        }

        // The narrowing still narrows: a market role is not a reviewer, and
        // per-task eligibility stays with the task store's candidate_roles.
        let lf = json!({ "roles": ["LF"], "tenant": "9900357000004" });
        let d = engine.authorize(&PolicyRequest {
            principal: "user:mallory",
            action: "api:task.decide",
            resource: "*",
            context: &lf,
        });
        assert!(!permitted(&d), "a market role must stay refused");
    }

    /// The worklist answers an operator and refuses everybody else.
    ///
    /// Both halves matter: a surface nobody can reach is an oversight control
    /// that reads as configured and cannot be exercised, and one anybody can
    /// reach is not a control at all.
    #[test]
    fn deciding_a_task_needs_an_operations_role() {
        let allowed = json!({ "roles": ["mako-operations"], "tenant": "9900357000004" });
        let refused = json!({ "roles": ["LF"], "tenant": "9900357000004" });

        let d = engine().authorize(&PolicyRequest {
            principal: "user:anna",
            action: "api:task.decide",
            resource: "task-1",
            context: &allowed,
        });
        assert!(permitted(&d), "an operator could not decide: {d:?}");

        let d = engine().authorize(&PolicyRequest {
            principal: "user:mallory",
            action: "api:task.decide",
            resource: "task-1",
            context: &refused,
        });
        assert!(!permitted(&d), "a market role could decide: {d:?}");
    }

    /// Cancelling a run is narrower than reading one.
    #[test]
    fn cancelling_a_run_is_operations_only() {
        let gas = json!({ "roles": ["gas-operations"], "tenant": "9900357000004" });
        let d = engine().authorize(&PolicyRequest {
            principal: "user:jan",
            action: "api:run.read",
            resource: "run-1",
            context: &gas,
        });
        assert!(permitted(&d), "gas operations must be able to read: {d:?}");

        let d = engine().authorize(&PolicyRequest {
            principal: "user:jan",
            action: "api:run.cancel",
            resource: "run-1",
            context: &gas,
        });
        assert!(!permitted(&d), "cancel is not a gas-operations verb: {d:?}");
    }

    /// **Every verb the oversight surface asks about is granted to somebody.**
    ///
    /// mako's pinned authorization failure class, in the tier where it is
    /// quietest: a Cedar action in the runtime's route table but in no rule here
    /// is a **permanent 403** on that route, with a policy set that compiles
    /// clean and nothing anywhere reporting it — no test, no startup check, no
    /// log line.
    ///
    /// The list is read from `action::ALL` rather than restated, so a verb
    /// agentplane adds fails here on the first `cargo test` after the bump
    /// rather than on a compliance function's first visit.
    #[test]
    fn every_action_the_oversight_surface_asks_about_is_granted_to_somebody() {
        // Every role the Cedar set names, plus the umbrella. A verb granted to
        // none of these is granted to nobody: the authenticator mints exactly
        // these from `mako_roles`.
        const ROLES: &[&str] = &[
            "mako-operations",
            "gas-operations",
            "billing-operations",
            "billing-compliance",
            "credit-control",
            "eeg-operations",
            "grid-operations",
            "marktkommunikation",
            "metering",
            "netzbilanz",
            "regulatory",
            "mako-service",
        ];

        let engine = engine();
        let orphaned: Vec<&str> = agentplane::api::action::ALL
            .iter()
            .copied()
            .filter(|action| {
                !ROLES.iter().any(|role| {
                    let context = json!({ "roles": [role], "tenant": "9900357000004" });
                    permitted(&engine.authorize(&PolicyRequest {
                        principal: "user:reviewer",
                        action,
                        resource: "*",
                        context: &context,
                    }))
                })
            })
            .collect();

        assert!(
            orphaned.is_empty(),
            "the oversight surface asks the policy engine about these actions and \
             `policy/agentd.cedar` grants them to no role, so the routes behind them answer \
             403 to every caller forever: {orphaned:?}. Add a `permit` naming the audience \
             each belongs to — deliberately: a verb agentplane split out is one it expects \
             to be granted separately."
        );
    }

    /// The narrower verb stays narrow.
    ///
    /// `api:obligation.list` is its own action so *what did we miss* can be
    /// granted without *the contents of every matter*. Granting it to the whole
    /// read audience would satisfy the test above while discarding the
    /// distinction it exists for.
    #[test]
    fn listing_breached_obligations_is_narrower_than_reading_cases() {
        let engine = engine();
        let ask = |role: &str, action: &str| {
            let context = json!({ "roles": [role], "tenant": "9900357000004" });
            permitted(&engine.authorize(&PolicyRequest {
                principal: "user:reviewer",
                action,
                resource: "*",
                context: &context,
            }))
        };

        for role in ["mako-operations", "regulatory"] {
            assert!(
                ask(role, "api:obligation.list"),
                "`{role}` answers for what this plane missed and cannot list it"
            );
        }
        for role in ["metering", "credit-control", "gas-operations"] {
            assert!(
                ask(role, "api:case.read"),
                "`{role}` must still be able to read its own matters"
            );
            assert!(
                !ask(role, "api:obligation.list"),
                "`{role}` was handed every domain's missed Fristen — the route does not \
                 narrow by domain, so a broad grant is a cross-domain disclosure"
            );
        }
    }

    /// **agentd's own doors are governed too.**
    ///
    /// `POST /api/v1/run` and the two inventory reads took a `Claims` extractor
    /// and nothing else, which authenticates a caller and authorizes nobody. The
    /// consequence was not subtle: any principal the realm would issue a token
    /// to could start any specialist on any Marktlokation and spend the tenant's
    /// model budget doing it.
    ///
    /// Walked over [`action::ALL`] rather than a hand-written pair, for the same
    /// reason agentplane publishes its own list: a verb this file forgets is a
    /// verb nobody checks is closed.
    #[test]
    fn agentds_own_verbs_are_granted_to_an_audience_and_refused_to_everyone_else() {
        let engine = engine();
        let may = |roles: &[&str], action: &str| {
            caller_may(
                engine.as_ref(),
                "user:someone",
                &roles.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>(),
                "9900357000004",
                action,
            )
        };

        for action in action::ALL {
            assert!(
                may(&["mako-operations"], action),
                "{action}: operations cannot reach agentd's own door"
            );
            if *action == action::ERASURE_EXECUTE {
                assert!(
                    may(&["regulatory"], action),
                    "privacy/compliance operations cannot execute erasure"
                );
                assert!(
                    !may(&["mako-service"], action),
                    "a scheduler must not hold irreversible erasure authority"
                );
            } else {
                // The two manual-only specialists exist because no CloudEvent
                // marks "the reporting period ended", so a scheduler is a
                // first-class caller of the manual-run door.
                assert!(
                    may(&["mako-service"], action),
                    "{action}: a scheduler cannot reach the run/inventory door"
                );
            }
            assert!(
                !may(&[], action),
                "{action}: a token with no roles was admitted — this is the whole finding"
            );
            assert!(
                !may(&["portal-customer"], action),
                "{action}: a role with no business on this plane was admitted"
            );
        }
    }

    /// An unauthenticated-shaped request — no roles at all — reaches nothing.
    ///
    /// Over `action::ALL` rather than a hand-written list, for the same reason
    /// as above: a verb this file forgets is a verb nobody checks is closed.
    #[test]
    fn a_caller_with_no_roles_is_refused_everywhere() {
        let none = json!({ "roles": [], "tenant": "9900357000004" });
        for action in agentplane::api::action::ALL.iter().chain(action::ALL) {
            let d = engine().authorize(&PolicyRequest {
                principal: "user:nobody",
                action,
                resource: "*",
                context: &none,
            });
            assert!(!permitted(&d), "{action} was open to a roleless caller");
        }
    }
}
