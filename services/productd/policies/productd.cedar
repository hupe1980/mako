// Cedar ABAC policies for productd (the retail product & tariff catalogue, LF role)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — this deployment's tenant, passed by the handler
//
// ── What this service is, and why a token alone is not enough ────────────────
//
// productd is the single source of truth for what the supplier charges. Every
// price `billingd` puts on an invoice is read from a row here, and nothing
// downstream re-checks it: a `PUT /api/v1/products/{lf_mp_id}/{product_code}`
// rewrites the Arbeitspreis, the Grundpreis and the Preisgarantie of a live
// tariff, and the next invoice run bills the new number. The § 41a EPEX and
// nEHS imports are the same act by another route — a dynamic tariff's billed
// price *is* the imported day-ahead curve.
//
// Before this policy the service authenticated and did not authorize: every
// route extracted `Claims` and, at most, compared the path's `lf_mp_id` with
// the token's tenant. That check answers "is this my tenant's catalogue?", not
// "may this caller change its prices" — so any token the verifier accepted for
// the tenant could rewrite the whole price sheet.
//
// ── Actions ─────────────────────────────────────────────────────────────────
//
// Catalogue:
//   "read-product"        — GET  /api/v1/products/{lf_mp_id}[/{code}
//                               [/history|/energiemix]],
//                           POST /api/v1/products/{lf_mp_id}/resolve — the
//                           batch lookup `billingd` runs once per invoice
//   "write-product"       — PUT/DELETE /api/v1/products/{lf_mp_id}/{code} and
//                           its /energiemix sub-resource (§ 42 EnWG
//                           Stromkennzeichnung). This is the money: the
//                           Tarifpreisblatt these routes store is what the next
//                           invoice is calculated from
//
// Market price series:
//   "read-marktpreise"    — GET  /api/v1/epex-prices/{date}/quarter-hourly,
//                           /api/v1/epex-prices/{year}/{month}/average,
//                           /api/v1/nehs-prices/latest
//   "write-marktpreise"   — PUT  /api/v1/epex-prices/{date},
//                           /api/v1/nehs-prices/{date} — the § 41a dynamic
//                           tariff bills the imported EPEX curve, and the BEHG
//                           nEHS certificate price is a per-kWh surcharge, so
//                           an import is a price change for every customer on
//                           those products at once
//
// Angebot (B2B quotation) lifecycle:
//   "read-angebot"        — GET  /api/v1/angebote[/{id}[/comparison]]
//   "write-angebot"       — POST /api/v1/angebote, PUT /api/v1/angebote/{id}
//   "versenden-angebot"   — POST /api/v1/angebote/{id}/versenden — the
//                           quotation leaves the house to the prospect
//   "entscheiden-angebot" — POST /api/v1/angebote/{id}/annehmen, /ablehnen —
//                           acceptance emits `de.tarif.angebot.angenommen`,
//                           which `vertragd` turns into a supply contract at
//                           the accepted price
//   "expire-angebote"     — POST /api/v1/angebote/expire — the tenant-wide
//                           sweep that marks every open quotation past its
//                           `gueltig_bis` as ABGELAUFEN. Separated from
//                           `write-angebot` because it is a bulk state change
//                           over the whole book, not an edit of one offer
//
// MCP:
//   "use-mcp"             — the whole `/mcp` surface, gated once by the shared
//                           `McpAuth` middleware rather than per tool. Its
//                           thirteen tools are reads, but three of them
//                           (`list_angebote`, `get_angebot`,
//                           `get_angebot_summary`) read named prospects and the
//                           bespoke prices quoted to them, so it is held to the
//                           same role the Angebot routes are
//
// ── The two routes with no action, and why ──────────────────────────────────
//
// `GET /api/v1/comparison-feed` and `GET /api/v1/comparison-feed/bo4e` take no
// `Claims` and carry no Cedar action. That is deliberate and statutory: § 41c
// EnWG obliges the supplier to make its tariff data available to independent
// comparison instruments (Verivox, Check24, the BNetzA Markttransparenzstelle),
// and an obligation to publish that is only discharged behind a bearer token is
// not discharged. Requiring a credential would put the operator in breach.
//
// The exemption is bounded by the query rather than by the caller: both routes
// serve `fetch_comparison_feed`, whose `WHERE` clause pins
// `product_status = 'PUBLISHED'` and restricts the rows to the `FEED_CATEGORIES`
// allowlist. A draft tariff, a withdrawn one and an Angebot are unreachable
// through them. `tests/authorization_guard.rs` pins both facts, so the
// exemption's justification stays checkable rather than remaining a claim in a
// comment.
//
// ── Roles ───────────────────────────────────────────────────────────────────
//
// `mako_roles` carries the platform's **market** roles. productd is catalogued
// as an LF service, but its product categories are not the LF's alone:
// ENERGIEDIENSTLEISTUNG covers Messstellenbetrieb and smart-meter services an
// MSB sells, and an ESA sells the same catalogue of Energiedienstleistungen. A
// policy naming LF alone would 403 an MSB or ESA deployment out of its own
// price sheet, so the operator set is LF, MSB, ESA and ADMIN — the same set
// `vertragd` admits, which is what the peer service credentials on this path
// (`billingd`, `vertragd`, `portald`, `makod`) already carry.

// ── Reads that change nothing ────────────────────────────────────────────────
// Any authenticated caller of this tenant may read the catalogue and the market
// price series it bills against. Reading is deliberately a different action
// from every write below, so a role-less token — an auditor's, or the narrow
// service credential `billingd` resolves products with — reaches the catalogue
// and cannot move a single price.
//
// The published subset of exactly this data already leaves the house
// unauthenticated under § 41c; what a role-less token adds is the unpublished
// draft and the history, neither of which is a price anyone is billed at.
permit(
    principal,
    action in [
        Action::"read-product",
        Action::"read-marktpreise"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Writing the price sheet ──────────────────────────────────────────────────
// Everything that decides what a customer is charged: the Tarifpreisblatt, the
// Energiemix label that goes with it, the EPEX day-ahead curve a § 41a dynamic
// tariff is settled against, and the BEHG nEHS certificate price. None of them
// is reversible by the customer, and none of them is re-checked downstream —
// `billingd` bills what it reads.
permit(
    principal,
    action in [
        Action::"write-product",
        Action::"write-marktpreise"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["LF", "MSB", "ESA", "ADMIN"])
};

// ── The Angebot lifecycle ────────────────────────────────────────────────────
// A quotation names a prospect and the bespoke price offered to them, so even
// reading one is an operator's act rather than an auditor's. Sending it commits
// that price to a third party, and accepting it emits the CloudEvent `vertragd`
// turns into a supply contract — after which the price is a contractual one.
permit(
    principal,
    action in [
        Action::"read-angebot",
        Action::"write-angebot",
        Action::"versenden-angebot",
        Action::"entscheiden-angebot",
        Action::"expire-angebote"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["LF", "MSB", "ESA", "ADMIN"])
};

// ── MCP ──────────────────────────────────────────────────────────────────────
// The blanket gate the shared middleware applies
// (`mako_service::mcp_auth::McpAuth::authenticate`). The tools are reads, but
// they include the Angebot register, so the gate carries the Angebot roles
// rather than the role-less catalogue read. An API-key caller is a
// deployment-trusted boundary the middleware handles on its own.
permit(
    principal,
    action == Action::"use-mcp",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["LF", "MSB", "ESA", "ADMIN"])
};

// ── On a finer split, and why it is not here ─────────────────────────────────
// The control this catalogue really wants is four-eyes on a price change: the
// identity that drafts a Tarifpreisblatt should not be the one that publishes
// it. That needs a job-function axis, and `mako_roles` carries market roles
// only — no IdP in this platform issues a PRICING or FREIGABE role, and a
// policy naming one would deny every caller. What exists instead is
// attribution: `product_history` keeps every superseded version of a product
// row, and a denial is logged with the caller's `sub` and the action.
