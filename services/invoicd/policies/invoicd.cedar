// Cedar ABAC policies for invoicd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["LF", "NB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-receipt"                — GET /api/v1/receipts, GET /api/v1/receipts/:id
//   "read-disputes"               — GET /api/v1/disputes
//   "read-overdue-remadv"         — GET /api/v1/overdue-remadv
//   "dispatch-selbstausstellen"   — POST /api/v1/selbstausstellen/{malo_id}

// Any authenticated caller may read receipts and manage billing in their own tenant.
permit(
    principal,
    action in [
        Action::"read-receipt",
        Action::"read-disputes",
        Action::"read-overdue-remadv",
        Action::"dispatch-selbstausstellen",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
