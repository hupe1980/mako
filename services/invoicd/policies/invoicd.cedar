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
// says only "forbidden". `tests/it/cedar_actions.rs` pins the two lists together.

// ── Read ──────────────────────────────────────────────────────────────────────
//
// Any authenticated caller may read the receipt store within its own tenant.
permit(
    principal,
    action in [
        Action::"read-receipt",
        Action::"read-disputes",
        Action::"read-overdue-remadv",
        Action::"use-mcp",
        Action::"read-policy",
        Action::"read-config"
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
    action == Action::"write-receipt",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};

// ── Statements for the Prüfschritte no INVOIC answers ─────────────────────────
//
// A Leistungsnachweis (Prüfschritt 310) and the answers stated on a held
// invoice (310, 320, 330, 80) decide whether that invoice is paid or refused,
// so they are the recipient's own: the roles that receive a PID 31009 — the
// ESA, the LF and the NB.
permit(
    principal,
    action == Action::"record-feststellung",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["ESA", "LF", "NB"])
};

// ── Webhooks: an operator capability ─────────────────────────────────────────
//
// An endpoint receives every receipt event that matches its filter — invoice
// outcomes, Zahlungsziele, the market partners — so registering one is an
// outbound data-export decision, and a test delivery reports the target's HTTP
// status, which makes it a network probe. The event feed
// (`GET /api/v1/events`) discloses the same. ADMIN alone.
permit(
    principal,
    action in [
        Action::"manage-subscription",
        Action::"read-events"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("ADMIN")
};
