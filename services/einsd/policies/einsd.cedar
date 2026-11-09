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
//   "read-einspeiser"    — GET  /api/v1/einspeiser[/{einspeiser_id}]
//   "write-einspeiser"   — PUT  /api/v1/einspeiser/{einspeiser_id}
//   "read-settlement"    — GET  /api/v1/anlagen/{tr_id}/settlements
//   "run-settlement"     — POST /api/v1/anlagen/{tr_id}/settle/... and /api/v1/settle/...
//   "correct-settlement" — POST .../settlements/{year}/{month}/correction (§147 AO / GoBD)
//   "manage-lifecycle"   — repowering (§3 Nr. 30 i.V.m. §25), zusammenlegen (§24),
//                          mastr-registrierung, switch-veraeusserungsform (§21b/§21c),
//                          §§53b–54 AW-Reduktionen, §36h Abs. 2 Wind-Reevaluierung
//   "read-marktdaten"    — GET  EPEX / Jahresmarktwert / Vergütungssatz lookup
//   "write-marktdaten"   — PUT  EPEX / Jahresmarktwert
//   "read-enfg"          — GET  /api/v1/enfg/ledger/..., /api/v1/enfg/mitteilung/...
//   "build-enfg-ledger"  — POST /api/v1/enfg/ledger/{year}/{month}
//   "use-mcp"            — /mcp (Streamable HTTP MCP server)
//   "read-policy"        — GET  /api/v1/policy (the operator policy report)
//   "read-config"        — GET  /admin/einrichtung (the configuration findings)

// ── Reads ─────────────────────────────────────────────────────────────────────
// Any authenticated caller may read their own tenant's plants and settlements.
permit(
    principal,
    action in [
        Action::"read-anlage",
        Action::"read-einspeiser",
        Action::"read-settlement",
        Action::"read-marktdaten",
        Action::"use-mcp",
        Action::"read-policy",
        Action::"read-config"
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
        Action::"write-einspeiser",
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

// ── Corrections (§ 147 AO / GoBD) ─────────────────────────────────────────────
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

// ── EnFG Belastungsausgleich ──────────────────────────────────────────────────
// The ÜNB reimburses „ihren nachgelagerten Verteilernetzbetreibern" (§ 13 Abs. 1
// EnFG) and the § 50 Nr. 1 Mitteilung is the Verteilernetzbetreiber's, so the
// ledger and the Mitteilung are the Netzbetreiber's alone. Reading them is held
// to NB as well: they total every payment of the tenant, not one plant's.
permit(
    principal,
    action in [Action::"read-enfg", Action::"build-enfg-ledger"],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};

// Cross-tenant access is denied by the absence of any permitting policy: Cedar
// is default-deny, so no forbid clause is needed for it.
