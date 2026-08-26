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
//   "write-esa-typ2"      — POST /api/v1/esa/typ2/:malo_id (Kapitel 4.6.2 SMGW push)
//   "read-corrections"    — GET /api/v1/corrections/:malo_id
//   "write-corrections"   — POST /api/v1/corrections/:malo_id (§ 60 Abs. 6 MsbG)
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

// Stored-series mutation: bulk import, § 60 Abs. 6 MsbG corrections, § 60 Abs. 2
// MsbG substitute values, virtual-meter configuration, quality rescore. Operated
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

// ESA "Werte nach Typ 2 aus SMGW" (Codeliste 1.4 Kapitel 4.6.2).
//
// **Its own action, not `write-meter-reads`**, and the separation runs both
// ways. The writer is the *ESA's* SM-PKI gateway — the thing the Werteanfrage's
// `FTX+Z17` Zieladresse points at — so it carries the `ESA` role and would be
// refused by a policy demanding `MSB`. And it must not gain what
// `write-meter-reads` grants: a Typ-2 value is non-authoritative by regulation
// (Codeliste 1.4 Kap. 4.6 · WiM Strom Teil 2 §4) and may never reach
// `meter_reads`. A gateway that can write one store must not be able to write
// the other.
//
// `MSB` is permitted alongside for the integrated deployment that holds both
// roles and runs the handshake against itself.
permit(
    principal,
    action == Action::"write-esa-typ2",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("ESA") ||
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
