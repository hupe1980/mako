// Cedar ABAC policies for einsd (EEG/KWKG feed-in settlement)
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-anlage"        — GET  /api/v1/anlagen[/{tr_id}], /foerderung-auslaufend
//   "write-anlage"       — POST/PUT/DELETE /api/v1/anlagen[/{tr_id}]
//   "read-settlement"    — GET  /api/v1/anlagen/{tr_id}/settlements
//   "run-settlement"     — POST /api/v1/anlagen/{tr_id}/settle/... and /api/v1/settle/...
//   "correct-settlement" — POST .../settlements/{year}/{month}/correction (§22 MessZV)
//   "manage-lifecycle"   — repowering (§22), zusammenlegen (§24), mastr-registrierung,
//                          switch-veraeusserungsform (§21b)
//   "read-marktdaten"    — GET  EPEX / Jahresmarktwert / Vergütungssatz lookup
//   "write-marktdaten"   — PUT  EPEX / Jahresmarktwert
//   "use-mcp"            — /mcp (Streamable HTTP MCP server)

// ── Reads ─────────────────────────────────────────────────────────────────────
// Any authenticated caller may read their own tenant's plants and settlements.
permit(
    principal,
    action in [
        Action::"read-anlage",
        Action::"read-settlement",
        Action::"read-marktdaten",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Writes ────────────────────────────────────────────────────────────────────
// Registering a plant or running a settlement creates a payment obligation to
// the Anlagenbetreiber, so it is restricted to the roles that carry it: the
// Netzbetreiber owes the EEG payment, the Lieferant the Direktvermarktung one.
permit(
    principal,
    action in [
        Action::"write-anlage",
        Action::"run-settlement",
        Action::"manage-lifecycle",
        Action::"write-marktdaten"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("LF") ||
     context.principal_roles.contains("UENB"))
};

// ── Corrections (§22 MessZV) ──────────────────────────────────────────────────
// A correction supersedes a settlement already sent to the Anlagenbetreiber and
// re-opens a closed accounting period, so it is held to a narrower role set than
// an ordinary settlement run.
permit(
    principal,
    action == Action::"correct-settlement",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("UENB"))
};

// Cross-tenant access is denied by the absence of any permitting policy: Cedar
// is default-deny, so no forbid clause is needed for it.
