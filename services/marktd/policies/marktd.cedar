// marktd authorization policies — FV2025-10-01+
//
// Principal attributes (from JWT):
//   context.principal_tenant  — `mako_tenant` custom claim (operator MP-ID)
//   context.principal_roles   — `mako_roles` custom claim (["NB", "LF", ...])
//
// Resource attribute:
//   context.resource_tenant   — owning tenant passed by the handler
//                                (= cfg.makod.tenant_id for this instance)
//
// Isolation model:
//   A token issued by the IDP carries `mako_tenant = "<operator-MP-ID>".
//   The running marktd instance is configured with the same MP-ID in `tenant_id`.
//   Access is permitted only when the two match.  This prevents cross-tenant
//   data leakage even if a valid token for another tenant reaches this instance.
//
// Role model:
//   Any authenticated user can read resources in their own tenant.
//   Any authenticated user can write MaLo/MeLo/contract/partner data.
//   Only the NB (Netzbetreiber) role may write price sheets (INVOIC billing).

// ── Read access — any role, same tenant ──────────────────────────────────────

permit(
    principal,
    action in [
        Action::"read-malo",
        Action::"read-melo",
        Action::"read-contract",
        Action::"read-nb-contract",
        Action::"read-partner",
        Action::"read-preisblatt",
        Action::"read-versorgungsstatus",
        Action::"read-nelo",
        Action::"read-malo-grid",
        Action::"read-device",
        Action::"read-sr",
        Action::"read-sharing-eligibility",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Write access — any role, same tenant ─────────────────────────────────────

permit(
    principal,
    action in [
        Action::"write-malo",
        Action::"write-melo",
        Action::"write-contract",
        Action::"write-nb-contract",
        Action::"write-partner",
        Action::"write-versorgungsstatus",
        Action::"write-malo-grid",
        Action::"write-device",
        Action::"write-sr"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Price-sheet writes — NB role required ────────────────────────────────────
//
// The NB (Netzbetreiber / Gasnetzbetreiber) is the billing party in INVOIC
// processes.  Only NB operators create and update PreisblattNetznutzung records
// used by invoic-checker to validate INVOIC plausibility (§22 MessZV).

permit(
    principal,
    action == Action::"write-preisblatt",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};

// ── NeLo writes — NB role required ───────────────────────────────────────────
//
// Netz-Element-Lokationen (NeLo) for Redispatch 2.0 are owned and managed by
// the Netzbetreiber.  Reads are open to all authenticated users in the tenant
// (see read-access block above); writes require the NB role to prevent
// unauthorized modification of network topology data.

permit(
    principal,
    action == Action::"write-nelo",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};
