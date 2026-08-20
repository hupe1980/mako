// Cedar ABAC policies for accountingd (Massenkontokorrent / customer ledger, LF role)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data row
//
// Authentication says who is calling; this says what they may do with the most
// sensitive data the platform holds. Reads are split three ways because a
// customer's balance, their IBAN, and the whole book's aging list leak very
// different things.
//
// Actions
// -------
//   "read-account"       — GET  a customer's account, balance, Kontokorrent,
//                          Kontoauszug, open items, Vorauszahlung, payment plans,
//                          interest charges, business-partner aggregates
//   "read-banking"       — GET  a SEPA mandate or Zahlungsinformation, the
//                          collection entries of a run, an EEG payout and its
//                          pain.001 XML. Split from "read-account" because these
//                          expose the IBAN and the instructions that move money.
//   "read-books"         — GET  the trial balance, aging, the dunning list, the
//                          period seals and the audit proofs. Book-level reads
//                          that span the whole tenant rather than one customer.
//   "write-account"      — PUT  account master data, Abschlag, Vorauszahlung,
//                          Zahlungsinformation, business-partner link
//   "post-entry"         — POST a manual ledger booking, a clearing, a clearing
//                          reset, an interest charge, a reconcile
//   "manage-dunning"     — POST Mahnstufe escalation/resolution, the §41g Abs. 1
//                          S. 2 Abwendungsvereinbarung offer, placing and lifting
//                          a Mahnsperre (dunning lock), and recording or closing
//                          a §41f Abs. 3 S. 3-5 Forderungseinwand
//   "manage-sepa"        — POST a mandate, a collection run, a pain.002 import,
//                          a pain.007 reversal; DELETE a mandate
//   "import-payments"    — POST camt.052/053/054 and the flat payment import
//   "run-payout"         — POST an EEG payout run, PUT its pain.002 status
//   "close-period"       — POST the Jahresabschluss and the Festschreibung seal
//   "erase-pii"          — POST the GDPR Art. 17 anonymization
//
// The MCP surface is authenticated separately by `[mcp]` (McpAuth) and is
// read-only by construction, so it carries no Cedar action. The inbound
// `/webhook` is HMAC-authenticated, not OIDC, and likewise carries none.
//
// Roles are the platform's **market** roles (`mako_roles`). accountingd is an LF
// service: the retail supplier keeps the Kundenkonto. MSB is admitted because a
// Messstellenbetreiber runs the same receivable machinery against its own
// Messentgelt invoices.

// ── Reads: one customer ───────────────────────────────────────────────────────
// Any authenticated caller may read their own tenant's customer accounts.
permit(
    principal,
    action == Action::"read-account",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Reads: bank details and payment instructions ──────────────────────────────
// An IBAN, a mandate and a pain.001 are together everything needed to collect
// from — or pay out to — a bank account, so they are held to the roles that
// operate the service rather than to any authenticated caller.
permit(
    principal,
    action == Action::"read-banking",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["LF", "MSB"])
};

// ── Reads: the books ──────────────────────────────────────────────────────────
// Tenant-wide rather than per-customer: the trial balance and the aging list are
// the whole receivable position, and the seals and proofs are the § 147 AO
// evidence chain.
permit(
    principal,
    action == Action::"read-books",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["LF", "MSB"])
};

// ── Writes that move money or change what a customer owes ─────────────────────
permit(
    principal,
    action in [
        Action::"write-account",
        Action::"post-entry",
        Action::"manage-sepa",
        Action::"import-payments",
        Action::"run-payout"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.containsAny(["LF", "MSB"])
};

// ── Dunning and the §§41f/41g disconnection sequence ──────────────────────────
// Every act here moves a household customer toward, or away from, having their
// electricity cut off: a Mahnstufe escalation feeds the sequence, a Mahnsperre
// stops it, lifting one for `vereinbarung_gebrochen` (§41g Abs. 1 S. 11) starts
// it again, and a Forderungseinwand takes a disputed amount out of the arrears
// the §41f Abs. 3 threshold is measured against. Restricted to the supplier role
// that carries the §41f obligations.
permit(
    principal,
    action == Action::"manage-dunning",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};

// ── Closing the books ─────────────────────────────────────────────────────────
// A Festschreibung is irreversible: it commits to the closing balances of a
// period and sets a watermark that refuses every later booking dated into it
// (GoBD / § 146 AO / § 239 HGB). The Jahresabschluss settles a customer's year.
permit(
    principal,
    action == Action::"close-period",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};

// ── GDPR Art. 17 erasure ──────────────────────────────────────────────────────
// Irreversible destruction of personal data, deliberately narrower than the other
// writes: it is a data-protection act, not an accounting one, and the financial
// records it leaves behind must survive it (§ 238 HGB).
permit(
    principal,
    action == Action::"erase-pii",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};

// Note on the role axis
// ---------------------
// There is deliberately no job-function axis (clerk / accountant / admin) here.
// The platform's JWTs carry market roles, and inventing a second vocabulary this
// service alone understands would put an authorization decision behind a claim
// nothing else issues. Finer separation of duties belongs in the IdP.
