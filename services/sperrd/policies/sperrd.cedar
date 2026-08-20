// Cedar ABAC policies for sperrd (Sperr-/Entsperrauftrag execution, NB role)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target order
//
// Actions
// -------
//   "read-sperr-order"    — GET the queue, a single order, or /stats
//   "create-sperr-order"  — POST a new order. Schedules a physical visit.
//   "execute-sperr-order" — PUT .../execute or .../fail. Reports the outcome to
//                           the Lieferant as IFTSTA 21039; the message cannot be
//                           taken back, only corrected by another one.
//   "cancel-sperr-order"  — PUT .../cancel. Withdraws a pending order.
//
// The `/webhook` market ingest is authenticated by the inbound X-Mako-Signature
// HMAC rather than by a bearer token, so it carries no Cedar action. The MCP
// surface is authenticated by `[mcp]` (McpAuth) and is read-only by
// construction, so it carries none either.
//
// Roles are the platform's market roles (`mako_roles`). This is a Netzbetreiber
// service throughout: the NB receives the Sperrauftrag, sends the field team,
// and reports the Auftragsstatus.

// ── Reads ─────────────────────────────────────────────────────────────────────
// The queue is dispatch data — which household is to be disconnected, and where
// the technician should meet them — so it is not open to every authenticated
// caller the way a reference list would be.
permit(
    principal,
    action == Action::"read-sperr-order",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};

// ── Creating and executing ────────────────────────────────────────────────────
// Creating an order schedules a visit that ends with someone's electricity off.
// Executing one both records that it happened and puts an IFTSTA 21039 on the
// market — a message that cannot be withdrawn, only corrected by a later one.
permit(
    principal,
    action in [
        Action::"create-sperr-order",
        Action::"execute-sperr-order"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};

// ── Cancelling ────────────────────────────────────────────────────────────────
// Only ever *prevents* a disconnection, and dispatches nothing to the market —
// but an order cancelled without telling the Lieferant still leaves their
// process waiting, so it is held to the same role.
permit(
    principal,
    action == Action::"cancel-sperr-order",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};
