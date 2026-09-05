// Cedar ABAC policies for netzbilanzd (the NB's outbound billing daemon)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-settlement"      — GET  /api/v1/billing/drafts[/{id}], /malo/{malo_id},
//                            /summary, /fremdkosten/{draft_id}
//   "export-audit"         — GET  /api/v1/billing/audit — the § 147 AO /
//                            § 14b UStG export: every invoice of a period with
//                            its counterparties, amounts and BO4E document
//   "run-settlement"       — POST /api/v1/billing/run, /mmm-run/{malo_id},
//                            /ggv-nne/{ggv_malo_id} — settles a period and
//                            consumes invoice numbers out of the Rechnungskreis
//   "amend-settlement"     — PUT  /api/v1/billing/fremdkosten/{draft_id},
//                            PUT  /api/v1/billing/drafts/{id}/reject — changes
//                            what a draft will bill, or discards it and reopens
//                            the period, before anything leaves the house
//   "dispatch-settlement"  — PUT  /api/v1/billing/drafts/{id}/dispatch,
//                            POST /api/v1/billing/drafts/dispatch-batch — sends
//                            the INVOIC to the market partner over AS4 and
//                            starts a payment obligation
//   "correct-settlement"   — POST /api/v1/billing/drafts/{id}/storno,
//                            /korrektur — reverses or replaces an invoice the
//                            counterparty already holds
//   "record-payment"       — PUT  /api/v1/billing/drafts/{id}/mark-paid,
//                            /mark-disputed — closes or contests a receivable
//   "read-kostenblatt"     — GET  /api/v1/redispatch/kostenblatt[/{activation_id}],
//                            /kostenblatt/gaps/{year}/{month}
//   "compute-kostenblatt"  — PUT  /api/v1/redispatch/kostenblatt/{activation_id},
//                            POST /kostenblatt/{activation_id}/compute — writes
//                            the priced Redispatch cost row
//   "submit-kostenblatt"   — POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}
//                            — files the month's cost sheet; the § 13a Abs. 1
//                            EnWG claim is what the month says
//   "compute-verguetung"   — POST /api/v1/redispatch/verguetung/{activation_id}/compute
//                            — the § 13a Abs. 2 EnWG compensation owed to a
//                            curtailed Anlagenbetreiber
//   "compute-ausfallarbeit"— POST /api/v1/redispatch/ausfallarbeit/* — the
//                            stateless BilAReM Kap.-3 calculators
//   "use-mcp"              — the blanket gate the shared MCP middleware applies
//                            (`mako_service::mcp_auth::McpAuth::authenticate`)
//
// `POST /api/v1/webhooks/remadv` carries no Cedar action: it is authenticated
// by the inbound HMAC (`inbound_secret`) rather than by a bearer token, and
// startup refuses without that secret.
//
// Roles are the platform's **market** roles (`mako_roles`): NB, LF, MSB, ESA,
// UENB. netzbilanzd issues for two of them — the NB bills Netznutzung, the
// Mehr-/Mindermengensaldo and the GeLi-Gas Handlungen; the MSB bills the
// Messstellenbetrieb (PID 31009) — and the Redispatch surface is settled
// between the NB that steered the resource and the ÜNB that requested it.

// ── Reads ─────────────────────────────────────────────────────────────────────
// Any authenticated caller of the tenant may read its invoices, its Redispatch
// cost sheets, and the § 147 AO export, and may run the stateless BilAReM
// calculators — which touch no stored data, take their whole input from the
// request body and persist nothing, so they disclose nothing about the tenant.
//
// Reading is deliberately a different action from every write below, so a token
// that carries no market role — an auditor's — reaches GET /api/v1/billing/audit
// and cannot dispatch, reverse or mark-paid a single invoice.
permit(
    principal,
    action in [
        Action::"read-settlement",
        Action::"export-audit",
        Action::"read-kostenblatt",
        Action::"compute-ausfallarbeit"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Invoicing ─────────────────────────────────────────────────────────────────
// Everything that decides what a market partner is billed, sends it, takes it
// back, or records that it was paid. An INVOIC over AS4 is a payment obligation
// the counterparty answers with a REMADV, and only a Stornorechnung undoes it.
permit(
    principal,
    action in [
        Action::"run-settlement",
        Action::"amend-settlement",
        Action::"dispatch-settlement",
        Action::"correct-settlement",
        Action::"record-payment"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("MSB"))
};

// ── Redispatch 2.0 ────────────────────────────────────────────────────────────
// Quantifying an activation, filing the month, and computing the § 13a Abs. 2
// Vergütung all set money between the NB and the ÜNB — and the Vergütung is
// paid to the Anlagenbetreiber for energy it did not get to sell.
permit(
    principal,
    action in [
        Action::"compute-kostenblatt",
        Action::"submit-kostenblatt",
        Action::"compute-verguetung"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("UENB"))
};

// ── MCP ───────────────────────────────────────────────────────────────────────
// The MCP tools are read-only by construction (see `mcp_server`), so the
// blanket gate the shared middleware applies is the whole check: a JWT caller
// of this tenant may use them, and an API-key caller is a deployment-trusted
// boundary the middleware handles on its own.
permit(
    principal,
    action == Action::"use-mcp",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── On four-eyes, and why it is not here ──────────────────────────────────────
// A stricter design would separate "may settle a period" from "may dispatch the
// invoice it produced", so the identity that runs the monthly billing job
// cannot also send its output to the market partner. That is a real control,
// and this policy does not implement it.
//
// The reason is the vocabulary: `mako_roles` carries **market** roles, not job
// functions, so there is no BUCHHALTUNG or CONTROLLING any IdP in this platform
// issues, and a policy naming one would deny every caller — an endpoint that
// cannot be reached is worse than one reachable by too many. A denial is logged
// with the caller's `sub` and the action, so who tried what is visible even
// where two roles are not separated.
