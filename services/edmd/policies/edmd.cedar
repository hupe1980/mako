// Cedar ABAC policies for edmd
//
// Context fields:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim
//   context.resource_tenant   — owning tenant of the target data row
//
// Actions:
//   "read-timeseries"     — GET /api/v1/deliveries/:malo_id
//   "read-imbalance"      — GET /api/v1/imbalance/:malo_id/:year/:month
//   "read-billing-period" — GET /api/v1/billing-period/:malo_id (M15)

// Same-tenant read: any authenticated caller may read their own meter data.
permit(
    principal,
    action in [
        Action::"read-timeseries",
        Action::"read-imbalance",
        Action::"read-billing-period",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
