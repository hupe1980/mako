// Cedar ABAC policies for invoicd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["LF", "NB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// # Every action the code checks must appear here
//
// Cedar is deny-by-default, so an action a handler names and this file does not
// permit is a permanent 403 that no configuration can lift, and the response
// says only "forbidden". `tests/cedar_actions.rs` pins the two lists together.

// ── Read ──────────────────────────────────────────────────────────────────────
//
// Any authenticated caller may read the receipt store within its own tenant.
permit(
    principal,
    action in [
        Action::"read-receipt",
        Action::"read-disputes",
        Action::"read-overdue-remadv",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Write ─────────────────────────────────────────────────────────────────────
//
// Confirming a payment, re-dispatching a REMADV and closing a dispute all
// change the § 147 AO audit trail or send a market message in this tenant's
// name. They are the LF's own decisions, so they are restricted to the LF role
// rather than granted to every authenticated reader.
permit(
    principal,
    action in [
        Action::"write-receipt",
        Action::"dispatch-selbstausstellen"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};
