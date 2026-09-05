// Cedar ABAC policies for vertragd (retail contract & customer management, LF role)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — this deployment's tenant, passed by the handler
//
// ── Two kinds of principal reach this service ────────────────────────────────
//
// **Operators** — the supplier's own staff and the peer services (`portald`,
// `processd`, `billingd`, `makod`) that call with a service credential. Their
// token carries a market role in `mako_roles`: LF, MSB, ESA, or ADMIN.
//
// **Customers** — an end customer of this supplier, holding a token from the
// portal realm. `portald` forwards that token unchanged to the two routes whose
// answer is *about the token's own subject*, because the question those routes
// answer ("does this identity own this Marktlokation?") can only be asked of a
// proven identity. A customer token carries no market role.
//
// The role test is therefore the split: every action that reads or changes a
// customer's record, contracts or master data requires a market role, so a
// customer token reaches `authenticate-portal-identity` and its own `by-sub`
// lookup and nothing else. **The portal realm must not issue `mako_roles`**; an
// IdP that puts a market role on a customer token makes that customer an
// operator of this service.
//
// ── Actions ─────────────────────────────────────────────────────────────────
//
// Customer-scoped (the caller's own subject is the resource):
//   "authenticate-portal-identity" — GET /api/v1/kunden/authenticate
//   "read-own-portal-identity"     — GET /api/v1/kunden/by-sub/{sub}, sub == token subject
//
// Operator, Kunden:
//   "read-kunde"                   — GET /api/v1/kunden[/{id}], /{id}/person,
//                                    /{id}/portfolio, /by-sub/{sub} for a foreign subject
//   "write-kunde"                  — POST /api/v1/kunden, PUT /{id}, PUT /{id}/person
//   "read-zahlungsinformation"     — GET  /api/v1/kunden/{id}/zahlungsinformation
//   "write-zahlungsinformation"    — PUT  /api/v1/kunden/{id}/zahlungsinformation
//   "read-portal-identitaeten"     — GET  /api/v1/kunden/{id}/identitaeten
//   "manage-portal-identitaeten"   — POST /api/v1/kunden/{id}/identitaeten,
//                                    DELETE /api/v1/kunden/{id}/identitaeten/{sub}
//   "export-kunde"                 — GET  /api/v1/kunden/{id}/export (DSGVO Art. 15/20)
//   "anonymize-kunde"              — POST /api/v1/kunden/{id}/anonymize (DSGVO Art. 17)
//
// Operator, Verträge:
//   "read-vertrag"                 — every contract GET, incl. /malo/{id}/produkte,
//                                    billing-candidates, expiring, Kündigungsfrist,
//                                    Preisgarantie
//   "create-vertrag"               — POST /api/v1/kunden/{id}/vertraege
//   "kuendigen-vertrag"            — POST /api/v1/vertraege/{id}/kuendigen and
//                                    /widerruf-kuendigung
//   "stornieren-vertrag"           — POST /api/v1/vertraege/{id}/stornieren
//   "tarifwechsel-vertrag"         — POST /api/v1/vertraege/{id}/tarifwechsel
//   "write-preisgarantie"          — PUT  /api/v1/vertraege/{id}/preisgarantie
//
// Operator, Rahmenverträge:
//   "read-rahmenvertrag"           — GET  /api/v1/rahmenvertraege[/{id}[/malos]],
//                                    GET /api/v1/kunden/{id}/rahmenvertraege
//   "create-rahmenvertrag"         — POST /api/v1/kunden/{id}/rahmenvertraege
//   "kuendigen-rahmenvertrag"      — POST /api/v1/rahmenvertraege/{id}/kuendigen
//
// Operator, Stammdaten & Betrieb:
//   "read-stammdaten"              — GET  /api/v1/ggv/{id}/betreiber,
//                                    /aggregatorvertraege,
//                                    /messstellenvertraege/{melo}/{msb}
//   "write-stammdaten"             — PUT  of the same three
//   "read-outbound-tasks"          — GET  /api/v1/outbound/dead
//   "retry-outbound-task"          — POST /api/v1/outbound/dead/{id}/retry
//
// Operator, MCP:
//   "use-mcp"                      — the whole `/mcp` surface, gated once by the
//                                    shared `McpAuth` middleware rather than per
//                                    tool. It is read-only by construction, but
//                                    it reads customer profiles and bank details
//                                    across the tenant, so it is an operator's.
//
// The two inbound webhooks (`POST /api/v1/events`, `POST /api/v1/webhooks/angebot`)
// are authenticated by the `inbound_secret` HMAC and carry no token, so they
// carry no Cedar action.

// ── Customer-scoped ──────────────────────────────────────────────────────────
// Both answers are about the token's own subject: `authenticate` reads the sub
// off the verified token and asks whether that customer owns the Marktlokation
// named in the query; the `by-sub` route is admitted here only when the handler
// has established that the requested subject *is* the caller's. Neither can be
// pointed at another customer, so neither needs a role.
permit(
    principal,
    action in [
        Action::"authenticate-portal-identity",
        Action::"read-own-portal-identity"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Operator reads ───────────────────────────────────────────────────────────
// A customer's profile, their bank details, who holds portal logins on their
// account, their contracts and the operator's own master data. Every one of
// them is a read *across* customers, so it is the operator's, not a customer's.
permit(
    principal,
    action in [
        Action::"read-kunde",
        Action::"read-zahlungsinformation",
        Action::"read-portal-identitaeten",
        Action::"read-vertrag",
        Action::"read-rahmenvertrag",
        Action::"read-stammdaten",
        Action::"read-outbound-tasks",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("ESA") ||
     context.principal_roles.contains("ADMIN"))
};

// ── Operator writes ──────────────────────────────────────────────────────────
// Creating and terminating supply, changing a tariff under § 41 Abs. 5 EnWG,
// granting portal access to a customer's account, storing an IBAN a direct
// debit will be drawn against, and erasing a data subject under DSGVO Art. 17.
// All of them act on a customer's record; none of them is ever a customer's own
// act, which is why the portal calls them with a service credential or not at
// all.
permit(
    principal,
    action in [
        Action::"write-kunde",
        Action::"write-zahlungsinformation",
        Action::"manage-portal-identitaeten",
        Action::"export-kunde",
        Action::"anonymize-kunde",
        Action::"create-vertrag",
        Action::"kuendigen-vertrag",
        Action::"stornieren-vertrag",
        Action::"tarifwechsel-vertrag",
        Action::"write-preisgarantie",
        Action::"create-rahmenvertrag",
        Action::"kuendigen-rahmenvertrag",
        Action::"write-stammdaten",
        Action::"retry-outbound-task"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("ESA") ||
     context.principal_roles.contains("ADMIN"))
};

// ── On a finer operator split, and why it is not here ────────────────────────
// A stricter design would separate the DSGVO Art. 17 erasure and the IBAN write
// from ordinary contract work, so a service credential that creates contracts
// cannot also destroy a customer record. That needs a job-function axis, and
// `mako_roles` carries **market** roles only: no IdP in this platform issues a
// DATENSCHUTZ or BUCHHALTUNG role, and a policy naming one would deny every
// caller. `anonymization_log` records who erased what and why, and
// `preisgarantie_override_log` who bypassed a price lock, so those acts are
// attributable where they are not separately authorised.
