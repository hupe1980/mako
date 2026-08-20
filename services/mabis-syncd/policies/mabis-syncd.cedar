// Cedar ABAC policies for mabis-syncd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["NB", "UENB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// A MaBiS submission is a binding filing to the BIKO under BK6-24-174 Anlage 3:
// it settles the balance group and cannot be withdrawn once acked. The three
// action groups below are separated because they are three different powers,
// and the middle one is not the first.

// ── Reading run state ─────────────────────────────────────────────────────────
//
// Authorised within the tenant. Run history discloses which Bilanzierungsgebiete
// a tenant settles, so it is not public.
permit(
    principal,
    action in [
        Action::"read-mabis-run",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Recording what the BIKO said ──────────────────────────────────────────────
//
// `POST /api/v1/datenstatus` and `POST /api/v1/pruefmitteilung` record an
// inbound IFTSTA: they state what arrived, they never file anything. They used
// to require `trigger-mabis-run`, which meant the ingest identity that relays
// BIKO responses also held the power to send a binding Summenzeitreihe in this
// tenant's name — a strictly larger grant than the job needs.
permit(
    principal,
    action == Action::"record-biko-response",
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Filing a Summenzeitreihe ──────────────────────────────────────────────────
//
// Only NB and ÜNB. These are the roles that aggregate a Bilanzierungsgebiet and
// file it with the BIKO; no other market role has standing to send a
// Summenzeitreihe in this tenant's name.
permit(
    principal,
    action == Action::"trigger-mabis-run",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("UENB"))
};
