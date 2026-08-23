// Cedar ABAC policies for outputd (customer-document rendering)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data
//
// Actions:
//   "read-template"     — GET  /api/v1/templates, /{kind}/current,
//                         /by-hash/{hash}, /reference/{kind}
//   "preview-template"  — POST /api/v1/templates/preview — renders a candidate
//                         against the kind's specimen and stores nothing
//   "publish-template"  — POST /api/v1/templates — writes a row that can never
//                         be deleted (§ 147 AO: 8 years)
//   "rollout-template"  — PUT  /api/v1/templates/{kind}/current — decides what
//                         *every* document issued from now on looks like
//   "render-document"   — POST /api/v1/render/{kind} — produces a PDF under the
//                         operator's Briefkopf, and stores nothing
//   "issue-document"    — POST /api/v1/documents/{kind} — the same render,
//                         recorded for 8 years and queued for delivery to a
//                         named recipient
//   "read-document"     — GET  /api/v1/documents, /{id}, /{id}/content,
//                         /api/v1/spool, and POST /deliveries/{id}/read —
//                         reads an issued document and its delivery evidence
//   "report-delivery"   — POST /api/v1/deliveries/{id}/status — a channel
//                         reporting whether a document arrived
//
// outputd shipped with the `cedar` feature enabled and **no policy file at
// all**. Authentication established who was calling and nothing established
// what they could do, so any token the OIDC verifier accepted could roll out a
// new layout for every invoice and Mahnung the tenant issues, or render
// arbitrary content on the operator's letterhead. This is the same defect
// `billingd` carried; the blast radius here is larger, because a template is
// not one document but the shape of all of them.
//
// Roles are the platform's **market** roles (`mako_roles`): NB, LF, MSB, ESA,
// UENB — see the note at the bottom on why there is no job-function axis.

// ── Reads and previews ────────────────────────────────────────────────────────
// Any authenticated caller in the tenant may see which layouts exist and render
// a candidate against the specimen. A preview writes nothing, moves nothing and
// renders no customer's data — it renders mako's own specimen — so it is a read
// with a CPU cost, not a decision.
permit(
    principal,
    action in [
        Action::"read-template",
        Action::"preview-template"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Publishing, rolling out and rendering ─────────────────────────────────────
// Everything below either changes what customers will receive or produces
// something they receive.
//
// `publish-template` is append-only and irreversible: the row is kept for eight
// years whether or not anyone ever rolls it out. `rollout-template` is the one
// that reaches customers — it re-points every subsequent render, so a careless
// PUT changes the appearance of an entire month's invoicing. `render-document`
// puts the operator's Briefkopf on a page whose content the caller supplied.
//
// Restricted to the roles that operate the document-issuing services: LF is the
// retail supplier (billingd), MSB bills metering services, ESA sells
// Energiedienstleistungen. A Netzbetreiber token reaching this service is not
// the party whose letterhead this is.
permit(
    principal,
    action in [
        Action::"publish-template",
        Action::"rollout-template",
        Action::"render-document",
        Action::"issue-document",
        Action::"report-delivery"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("ESA"))
};

// ── Reading issued documents ──────────────────────────────────────────────────
// A read within the tenant, on the same footing as `read-template`. The scope
// that matters here is not the role but the *query*: `GET /api/v1/documents`
// refuses to answer without a `malo_id` or a `kunden_nr`, so a token cannot ask
// for the portfolio. `portald` is the caller that turns a customer's OIDC
// identity into that scope, and it resolves the scope from `vertragd` — the one
// service in the platform that maps identities to Marktlokationen.
//
// Deliberately not restricted to LF/MSB/ESA: a Netzbetreiber deployment issues
// no retail documents at all, so the rule would never fire, while an operator's
// own back-office tooling legitimately reads them.
permit(
    principal,
    action == Action::"read-document",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── On separating rollout from publish, and why it is not here ────────────────
// Publishing and rolling out are separate *acts* — that is the whole shape of
// this API — but they are not separately *authorised*: both need the same
// market roles. A stricter design would let a designer publish and require an
// operations role to roll out, which is the four-eyes control the append-only
// store is otherwise built for.
//
// It is not implemented for the same reason billingd's is not: `mako_roles`
// carries market roles, not job functions, and no IdP in this platform issues a
// GESTALTUNG or a BETRIEB. A policy naming one would deny every caller, and an
// endpoint nobody can reach is worse than one too many can. `published_by`
// records who published what in the meantime, so the act stays attributable.
// A function-role axis is tracked in ROADMAP.md.
