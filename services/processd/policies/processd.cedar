// Cedar ABAC policies for processd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["LF", "NB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// Completeness:
//   Cedar is default-deny, so an action checked in a handler but named in no
//   policy below makes that endpoint return 403 to every caller. The
//   `authorization_guard` test pins both directions — every action checked in
//   code is permitted here, and every action permitted here is checked in code.
//
// Why writes are role-gated:
//   processd does not merely report decisions, it *makes* them. Approving a
//   queue entry dispatches the market answer; `start-supply` / `end-supply`
//   initiate and terminate a supply relationship on the market. Those are the
//   LF's own processes, so they require the LF role rather than "any
//   authenticated caller" — an NB principal holding a token for this
//   deployment has no business starting a supply in the operator's name.

// ── Reads — any authenticated caller of the tenant ───────────────────────────
//
// The decision log, the approval queue and the EoG case log are operational
// evidence: § 20 Abs. 1 EnWG parity audits and the § 38 EnWG timer both have to
// be readable by whoever is answering for them.

permit(
    principal,
    action in [
        Action::"read-decisions",
        Action::"read-queue",
        Action::"read-eog",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Deciding a queued process — NB, LF or MSB ────────────────────────────────
//
// An approval-queue entry is decided by the role that owes the answer: the NB
// for an escalated or held-back GPKE / GeLi Gas Anmeldung, the LF for the GPKE
// Lieferende / Beendigung der Zuordnung queries, the MSB for an escalated
// MSB-Wechsel or a § 14a Steuerungsauftrag whose SteuerbareRessource could not
// be auto-checked. Approving dispatches the market message, so this is a write.
//
// Every role that enqueues must appear here: an entry whose role is absent is a
// permanent 403.

permit(
    principal,
    action == Action::"decide-queue",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB"))
};

// ── Initiating a supply process — LF only ────────────────────────────────────
//
// `start-supply` dispatches UTILMD 55001/44001 and `end-supply` 55004/44004; both
// commit the operator to a market position in the LF role. § 7 EnWG separation
// is enforced at the binary level by the `role-lf-*` Cargo features; this is
// the same boundary at the token level.

permit(
    principal,
    action == Action::"initiate-supply",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};
