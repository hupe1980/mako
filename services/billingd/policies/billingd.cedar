// Cedar ABAC policies for billingd (multi-product retail billing, LF role)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-billing"       — GET  /api/v1/billing[/{id}], /review-queue, and the
//                          rendered documents (/xrechnung, /ubl, /pdf)
//   "read-billing-by-token" — POST /api/v1/billing/nachrechnen — portald alone
//   "preview-billing"    — POST /api/v1/billing/{malo_id}/preview (no side effects)
//   "run-billing"        — POST /api/v1/billing/{malo_id}/calculate, /tarifwechsel,
//                          /sammelrechnung/{rv_id}, /ggv/{ggv_id} — issues invoices
//   "settle-flexibility" — POST /api/v1/billing/vpp/{vpp_id} — issues a §41e Gutschrift
//   "correct-billing"    — POST /api/v1/billing/{id}/correction (§147 AO / GoBD Storno)
//   "release-billing"    — POST /api/v1/billing/{id}/release — overrides the risk gate
//   "review-billing"     — POST /api/v1/billing/{id}/reviewed — records that a REVIEW
//                          record was looked at; issues and sends nothing
//   "submit-b2g"         — POST /api/v1/billing/{id}/submit-b2g
//   "issue-billing"      — POST /api/v1/billing/{id}/versenden — records the
//                          rendered document for 8 years and sends it to the
//                          customer on their channels
//   "read-policy"        — GET  /api/v1/policy — the operator policy report
//   "read-config"        — GET  /admin/einrichtung — the configuration findings
//   "manage-subscription" — /api/v1/webhooks/** — webhook endpoints, deliveries,
//                          replay; the `list_webhook_*` MCP tools
//   "read-events"        — GET  /api/v1/events — the emitted events as a feed
//
// The MCP surface is authenticated separately by `[mcp]` (McpAuth) and is
// read-only by construction, so it carries no Cedar action. The VPP dispatch
// webhook is HMAC-authenticated, not OIDC, and likewise carries none.
//
// Roles are the platform's **market** roles (`mako_roles`): NB, LF, MSB, ESA,
// UENB. There is deliberately no job-function axis here — see the note at the
// bottom.

// ── Reads ─────────────────────────────────────────────────────────────────────
// Any authenticated caller may read their own tenant's invoices and preview a
// calculation. A preview persists nothing and emits nothing, so it is a read.
permit(
    principal,
    action in [
        Action::"read-billing",
        Action::"preview-billing",
        Action::"read-policy",
        Action::"read-config"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── An invoice by its verify token: portald alone ─────────────────────────────
// The token printed on the invoice is the customer's credential for the public
// page. Only portald's service principal (`[[oidc.service_keys]] name =
// "portald"`) relays it; an operator reads an invoice by its id under
// `read-billing` and has no use for a lookup by token. The demo configures the
// same service key, so no synthetic principal is admitted here.
permit(
    principal,
    action == Action::"read-billing-by-token",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    principal == User::"portald"
};

// ── Issuing, correcting and releasing documents ───────────────────────────────
// Everything below puts a document in front of a customer, or takes one back.
// Issuing a Rechnung creates a payment obligation and a § 147 AO record that
// only a Stornorechnung can undo; a Storno reverses a document the customer
// already received and re-opens its period; a release overrides the risk gate.
//
// All are restricted to the roles that actually operate this service: LF is the
// retail supplier, MSB bills metering services, ESA sells Energiedienstleistungen.
permit(
    principal,
    action in [
        Action::"run-billing",
        Action::"settle-flexibility",
        Action::"correct-billing",
        Action::"release-billing",
        Action::"review-billing",
        Action::"submit-b2g",
        Action::"issue-billing"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("ESA"))
};

// ── Webhooks: an operator capability ─────────────────────────────────────────
// An endpoint receives every invoice event that matches its filter — amounts,
// Marktlokationen, Lieferanten — so registering one is an outbound data-export
// decision, and a test delivery reports the target's HTTP status, which makes
// it a network probe. The event feed discloses the same. ADMIN alone.
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

// ── On four-eyes, and why it is not here ──────────────────────────────────────
// A stricter design would separate "may run billing" from "may release an
// invoice the risk gate held", so the automation identity that produces held
// invoices cannot also wave them through. That is a real control, and this
// policy does not implement it.
//
// The reason is that `mako_roles` carries **market** roles, not job functions:
// there is no BUCHHALTUNG or CONTROLLING in the vocabulary any IdP in this
// platform issues, and a policy naming one would deny every caller — an
// endpoint that cannot be reached is worse than one that is reachable by too
// many. `released_by`/`released_at` record who released what in the meantime,
// so the action is attributable even where it is not separately authorised.

// ── Speaking MCP at all ───────────────────────────────────────────────────────
// `use-mcp` is the blanket gate the shared MCP middleware applies to **every**
// frame before any tool name is looked at — `mako_service::mcp_auth` checks it
// whenever a Cedar enforcer is wired, and `CedarEnforcer::check` is
// default-deny. A policy that does not name it therefore does not merely leave
// the surface ungoverned: it returns 403 to every caller, and the whole tool and
// prompt surface is unreachable.
//
// It is tenant-scoped and nothing more. What a frame may then *do* is decided by
// the per-action rules above; this one only says the caller may open its mouth.
permit(
    principal,
    action == Action::"use-mcp",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
