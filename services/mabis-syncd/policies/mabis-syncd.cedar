// Cedar ABAC policies for mabis-syncd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["NB", "UENB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-mabis-run"    — GET /api/v1/runs, GET /api/v1/runs/{id}
//   "trigger-mabis-run" — POST /api/v1/sync, PUT /api/v1/runs/{id}/retry
//
// A MaBiS submission is a binding filing to the BIKO under BK6-22-024
// Anlage 3: it settles the balance group and cannot be withdrawn once acked.
// Triggering one is therefore separated from reading run status, and is
// restricted to the grid roles that are entitled to submit a Summenzeitreihe.

// Any authenticated caller may read run status within its own tenant.
permit(
    principal,
    action in [
        Action::"read-mabis-run",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// Only NB and ÜNB may trigger or retry a submission. These are the roles that
// aggregate a Bilanzierungsgebiet and file it with the BIKO; no other market
// role has standing to send a Summenzeitreihe in this tenant's name.
permit(
    principal,
    action == Action::"trigger-mabis-run",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("UENB"))
};
