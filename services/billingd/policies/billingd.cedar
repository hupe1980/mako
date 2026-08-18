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
//   "preview-billing"    — POST /api/v1/billing/{malo_id}/preview (no side effects)
//   "run-billing"        — POST /api/v1/billing/{malo_id}/calculate, /tarifwechsel,
//                          /sammelrechnung/{rv_id}, /ggv/{ggv_id} — issues invoices
//   "settle-flexibility" — POST /api/v1/billing/vpp/{vpp_id} — issues a §41e Gutschrift
//   "correct-billing"    — POST /api/v1/billing/{id}/correction (§147 AO / GoBD Storno)
//   "release-billing"    — POST /api/v1/billing/{id}/release — overrides the risk gate
//   "submit-b2g"         — POST /api/v1/billing/{id}/submit-b2g
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
        Action::"preview-billing"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
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
        Action::"submit-b2g"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("ESA"))
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
// Adding a function-role axis is tracked in ROADMAP.md.
