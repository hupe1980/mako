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
//   "read-parity"      — GET /api/v1/parity    (§20 EnWG affiliate parity report)
//   "use-mcp"          — POST|GET /mcp
//
// §20 EnWG Diskriminierungsfreiheitspflicht enforcement:
//   Automatic acceptance (auto_accept=true) is suppressed in the nb_module
//   when initiator_is_affiliate==true.  The Cedar policy layer reflects this
//   invariant: "read-parity" is always permitted for same-tenant callers so
//   BNetzA audit evidence is always accessible.

// Any authenticated caller may read and act on data that belongs to their tenant.
permit(
    principal,
    action in [
        Action::"read-decisions",
        Action::"read-queue",
        Action::"write-queue",
        Action::"read-parity",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
