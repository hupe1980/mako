// Cedar ABAC policies for obsd
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-process"    — GET /obs/processes/:id, GET /obs/processes (list)
//   "read-kpi"        — GET /obs/kpis
//   "read-overdue"    — GET /obs/overdue
//   "use-mcp"         — POST|GET /mcp (MCP Streamable HTTP)

// Same-tenant read: any authenticated caller may read their own process projections.
permit(
    principal,
    action in [
        Action::"read-process",
        Action::"read-kpi",
        Action::"read-overdue",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
