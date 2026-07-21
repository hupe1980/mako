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

// Meter-reading ingest: the MSB delivers readings (iMSys/SMGW direct push,
// gas push, IoT push, SMGW registry). Defense in depth on top of the tenant
// check — an LF-role service account of the same tenant (e.g. a portal
// integration) must not be able to write measurement data.
permit(
    principal,
    action == Action::"write-meter-reads",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("admin"))
};

// Stored-series mutation: bulk import, §22 MessZV corrections, §17 MessZV
// substitute values, virtual-meter configuration, quality rescore. Operated
// by the metering-side roles (MSB field data, NB grid operations) or an
// operator account.
permit(
    principal,
    action in [
        Action::"write-corrections",
        Action::"write-timeseries",
        Action::"write-quality-rescore"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("NB") ||
     context.principal_roles.contains("admin"))
};

// Reading orders (Ablesesteuerung). Reads are open to the tenant; creating,
// completing, or cancelling an order — and launching the §40 Abs. 2 EnWG
// Jahresablesung campaign — dispatches field work, so it needs the NB or MSB
// role.
permit(
    principal,
    action == Action::"read-reading-order",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

permit(
    principal,
    action == Action::"write-reading-order",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("admin"))
};

// GDPR erasure (Art. 17 DSGVO): destructive — operator roles only. An
// LF-role token of the right tenant is refused.
permit(
    principal,
    action == Action::"write-gdpr-erasure",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("admin"))
};
