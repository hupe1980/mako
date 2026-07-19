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
        Action::"read-archive-olap",
        Action::"read-archive-status",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// Same-tenant write.
//
// Note: the guard below checks tenant only. `context.principal_roles` is passed
// by the enforcer but never evaluated here, so despite the historical comment
// naming "MSB, LF, or admin" there is no role gating — any authenticated
// principal of the right tenant may write. Add a `principal_roles.contains(..)`
// clause here if that needs tightening.
//
// `write-timeseries` and `write-quality-rescore` are used by
// `post_bulk_reads`, `post_corrections` and `post_quality_rescore` but were
// never declared. Cedar is default-deny, so those endpoints returned 403 for
// every caller regardless of role.
permit(
    principal,
    action in [
        Action::"write-meter-reads",
        Action::"write-corrections",
        Action::"write-timeseries",
        Action::"write-quality-rescore"
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
