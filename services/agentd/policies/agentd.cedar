// Cedar ABAC policy for agentd's read-only operator surface.
//
// agentd authorizes its runs and its agents through the agent plane's own
// policy (`[policy] path`); this file covers only the two reports the runner
// built at boot:
//
//   "read-policy" — GET /api/v1/policy (the operator policy report)
//   "read-config" — GET /admin/einrichtung (the configuration findings)
//
// Both are read-only and carry no secret, so any authenticated caller of the
// tenant may read them.
permit(
    principal,
    action in [Action::"read-policy", Action::"read-config"],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};
