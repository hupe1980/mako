// Cedar ABAC policies for processd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["LF", "NB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-decisions"   — GET /api/v1/decisions (NB STP audit log)
//   "read-queue"       — GET /api/v1/queue     (LF approval queue)
//   "write-queue"      — POST /api/v1/queue/:id/approve|reject
//   "use-mcp"          — POST|GET /mcp

// Any authenticated caller may read and act on data that belongs to their tenant.
permit(
    principal,
    action in [
        Action::"read-decisions",
        Action::"read-queue",
        Action::"write-queue",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
