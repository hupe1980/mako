// mako's authorization rules for the agent plane.
//
// agentplane starts at `DenyAll` and ships no permissive default, so this file
// is the whole of what an agent — and an operator on the HTTP surface — may do.
// It is compiled at startup: a policy set that does not parse is a refusal to
// boot, never a plane that denies everything for a reason nobody can see.
//
// Two things to know before editing.
//
//   * Cedar allows on **any** matching permit, so a broad `permit` cannot be
//     narrowed by adding a `forbid`-shaped exception elsewhere — it must be
//     narrowed here, or written as a `forbid` (which always wins). The baseline
//     permits below are deliberately paired with the forbids that bound them.
//   * Cedar is **total**: a `when` clause reading an attribute the runtime never
//     sends is simply unsatisfied, so a rule keyed on a misspelled attribute
//     disappears silently. Every attribute used here is from agentplane's
//     documented request shape; `tests/plane_policy.rs` runs each rule against a
//     real request rather than one assembled to suit it.
//
// The request shape, for reference:
//
//   principal  Agent::"<the acting agent>"       (an operator's `sub` for api:*)
//   action     Action::"effect:perform" | "run:admit" | "data:release" | "api:*"
//   resource   Resource::"<effect kind>"         — tool.call, model.complete, …
//   context    { run, step, tenant, mutates, args, label?, owner, subject,
//                delegation_depth, scope }       — label only on `sink`
//
// ─────────────────────────────────────────────────────────────────────────────
// Runs
// ─────────────────────────────────────────────────────────────────────────────

// Which agents exist at all is decided by wiring: `Plane::new` registers only
// the manifests an operator activated, and a capability nothing provides cannot
// be addressed. So admission is permitted here and narrowed there.
//
// To freeze one exact revision — a regulated deployment that wants the reviewed
// manifest and no other — bind to the digest instead:
//
//   permit(principal, action == Action::"run:admit", resource)
//   when { context.agent.digest == "a1b2…" };
//
// Never to `context.agent.name`: a manifest is a file and its name is whatever
// its author typed, so a rule granting authority to a name grants it to anyone
// who types that name.
permit(principal, action == Action::"run:admit", resource);

// ─────────────────────────────────────────────────────────────────────────────
// Effects
// ─────────────────────────────────────────────────────────────────────────────

// The baseline every narrowing below bounds. Enumerating the permitted effect
// kinds instead was tried and is worse here: `clock.now`, `case.*`, `task.*` and
// `deadline.*` are runtime bookkeeping, and a policy that forgets one turns an
// obligation the manifest declares into a run that cannot register it.
permit(principal, action == Action::"effect:perform", resource);

// **The rule that is deliberately NOT here**, because writing it down is worth
// more than the rule would have been:
//
//   forbid(principal, action == Action::"effect:perform", resource)
//   when { context.mutates && context.label.trust == "untrusted" };
//
// It reads like the Biba invariant this whole design is built on, it appears in
// agentplane's own documentation as an illustration, and in mako it is an
// outage. `context.label` is the label of the *whole* argument bundle, and in a
// tool-calling agent those arguments were written by a model that had just read
// a counterparty's event — so the bundle is untrusted on every mutating call
// there will ever be. The rule denies all of them, the tool loop reports the
// denial back to the model, and the run ends *successfully* having done
// nothing. It passed every unit test, because a hand-written context is a
// context assembled to suit the rule; an end-to-end run is what caught it
// (`tests/oversight.rs`).
//
// What binds instead is finer-grained and structural: each mutating grant
// declares `protected_fields` with `require_trusted` on its authority-bearing
// arguments — `/malo_id`, `/pid`, `/mp_id` — so a counterparty-derived value
// cannot choose *which* MaLo a dispatch names, while the model may still fill
// in the parts it is there to decide. On top of that, every mutating grant in
// every manifest requires a named human. Trust is a per-argument question, and
// Cedar sees the call, not the fields.

// mako's own services, and nothing else. The plane reaches MCP servers over a
// deployment-configured transport, so a mis-typed `[mcp_servers]` entry pointing
// at somebody else's endpoint is a configuration error this refuses rather than
// calls. `context.args.server` is what makes this expressible without naming
// all 166 granted tools.
forbid(principal, action == Action::"effect:perform", resource == Resource::"tool.call")
unless {
  ["makod", "marktd", "processd", "obsd", "billingd", "invoicd", "accountingd",
   "edmd", "einsd", "netzbilanzd", "portald", "sperrd", "tarifbd", "vertragd"]
  .contains(context.args.server)
};

// Nothing above `confidential` reaches a model. The manifests declare
// `max_sensitivity_egress: internal`, which is stricter still; this is the floor
// that survives a manifest edit — a prompt assembled from a secret is an
// exfiltration whether or not anyone meant it.
forbid(principal, action == Action::"effect:perform", resource == Resource::"model.complete")
when { context.label.sensitivity == "secret" };

// No specialist delegates. Every manifest declares `max_delegation_depth: 0`,
// and mako's fan-out is one independent run per subscribing specialist rather
// than a chain — so a request arriving with depth is a bug or an attempt.
forbid(principal, action == Action::"effect:perform", resource)
when { context.delegation_depth >= 1 };

// ─────────────────────────────────────────────────────────────────────────────
// Declassification
// ─────────────────────────────────────────────────────────────────────────────

// `cx.release()` relabels untrusted data as trusted, with provenance and on the
// record. It is the right mechanism and mako has no reviewed use for it: every
// value our agents trust is re-validated at admission (`plane::label`), which is
// a check on the value's *shape*, not an assertion about its author.
//
// Refused outright, so that adopting declassification is a policy edit somebody
// reviews rather than a call somebody adds.
forbid(principal, action == Action::"data:release", resource);

// ─────────────────────────────────────────────────────────────────────────────
// The operator surface
// ─────────────────────────────────────────────────────────────────────────────
//
// Here the principal is a person (their OIDC `sub`), and `context.roles` are the
// `mako_roles` their IDP asserted. Four-eyes is enforced by agentplane's task
// store — a proposer cannot approve their own action — so these rules decide
// *eligibility*, not identity.

// Reading the plane: what ran, what is waiting, what is on this case. Broad on
// purpose — an operator who cannot see a stuck run pages somebody who can.
permit(
  principal,
  action in [Action::"api:run.read", Action::"api:run.list", Action::"api:task.list",
             Action::"api:task.read", Action::"api:case.read"],
  resource
)
when { context.roles.contains("mako-operations") || context.roles.contains("gas-operations") };

// Claiming and releasing a proposal: the same eligibility as reading, because a
// reservation is not a decision and a queue nobody can claim duplicates work.
permit(
  principal,
  action in [Action::"api:task.claim", Action::"api:task.release"],
  resource
)
when { context.roles.contains("mako-operations") || context.roles.contains("gas-operations") };

// Deciding: approving a mutating tool call dispatches a real market message.
// The role names are the ones the manifests name in `oversight.approvers`, so a
// reviewer eligible in the file is eligible here, and adding a role is one edit
// in each place rather than a mismatch nobody notices until an approval sits
// unanswered past its deadline.
permit(principal, action == Action::"api:task.decide", resource)
when { context.roles.contains("mako-operations") || context.roles.contains("gas-operations") };

// Stopping a run is an intervention: it is recorded with the actor's name and a
// mandatory reason. Operations only.
permit(principal, action == Action::"api:run.cancel", resource)
when { context.roles.contains("mako-operations") };

// Event delivery wakes a suspended run. mako's services authenticate with their
// own service keys, so this is the machine door and stays separate from the
// human ones above.
permit(principal, action == Action::"api:event.deliver", resource)
when { context.roles.contains("mako-service") || context.roles.contains("mako-operations") };
