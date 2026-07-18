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
//   "write-meter-reads"   — POST /api/v1/meter-reads/rlm/:malo_id (direct push)
//   "read-corrections"    — GET /api/v1/corrections/:malo_id
//   "write-corrections"   — POST /api/v1/corrections/:malo_id (§22 MessZV)
//   "write-gdpr-erasure"  — DELETE /api/v1/gdpr/erasure/:malo_id (GDPR Art. 17)
//   "use-mcp"             — /mcp (Streamable HTTP MCP server)

// Same-tenant read: any authenticated caller may read their own meter data.
permit(
    principal,
    action in [
        Action::"read-timeseries",
        Action::"read-imbalance",
        Action::"read-billing-period",
        Action::"read-corrections",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// Same-tenant write: MSB, LF, or admin roles may push meter reads and corrections.
permit(
    principal,
    action in [
        Action::"write-meter-reads",
        Action::"write-corrections"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// GDPR erasure: operator roles only.
permit(
    principal,
    action == Action::"write-gdpr-erasure",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
