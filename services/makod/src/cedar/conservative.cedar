// Conservative MaKo authorization policy — least-privilege baseline.
//
// This file is NOT loaded automatically.  Enable it by passing BOTH
//
//   --cedar-policy-dir <dir>      (this file, or a symlink, lives in <dir>)
//   --cedar-no-default-policy     (drops the built-in permit-all baseline)
//
// Both are required.  `default.cedar` is compiled into the binary and its
// catch-all `permit(principal is MaKo::Principal, action, resource)` is part of
// the policy set unless the second flag omits it.  A Cedar request is allowed
// when *any* permit matches and no forbid does, so with the baseline present
// every grant below is redundant and every principal not named here still has
// full access — the restriction this file describes would not exist.
//
// Design intent
// ─────────────
// The default.cedar "permit all authenticated principals" policy is suitable
// for development and single-tenant deployments where all API keys are trusted
// equally.  In production, a leaked or compromised key should not grant
// full admin access.  Run with `--cedar-no-default-policy` and this file
// becomes the entire policy set: four named role grants, each covering only
// the actions needed for that role, and nothing else reaches an allow.
//
// Roles (add further specialised principals as needed)
// ─────────────────────────────────────────────────────
//   erp-operator    — full ERP integration: submit commands + read/write MaLo
//   edifact-ingest  — AS4 gateway: ingest EDIFACT messages only
//   admin-readonly  — read-only admin: MaLo stats + partner list
//   mcp-agent       — LLM / MCP access: submit commands + MaLo read + MCP tools
//
// MCP tools evaluate the same per-resource actions as their REST equivalents,
// so `UseMcp` alone reaches the endpoint and nothing else.  The `mcp-agent`
// grants below are therefore exactly its reach through an agent: commands,
// MaLo reads and process reads — and deliberately *not* the partner directory,
// which needs AdminPartnerRead.
//
// Tenancy
// ───────
// All permits are implicitly scoped to the tenant configured for this makod
// instance.  Cross-tenant access is never permitted because the resource
// `tenant` attribute always equals the instance-level tenant ID and the
// Cedar context `tenant` field is validated server-side.
//
// Usage
// ─────
// Copy this file into your policy directory, then add/remove `principal ==`
// clauses to match the key names configured in `[http] auth_keys` (or
// `--auth-key NAME=TOKEN`).  Use the action groups defined in mako.cedarschema
// (AdminMalo, AdminPartner) to grant multiple related actions in a single
// permit statement.
//
// IMPORTANT: do NOT add `permit(principal is MaKo::Principal, action, resource)`
// (the default.cedar catch-all) alongside this file, and do not drop
// `--cedar-no-default-policy` — either one restores the catch-all and defeats
// this baseline entirely.

// ── erp-operator: submit workflow commands + full MaLo admin ─────────────────
permit(
  principal == MaKo::Principal::"erp-operator",
  action    == MaKo::Action::"SubmitCommand",
  resource  is MaKo::Command
);

permit(
  principal == MaKo::Principal::"erp-operator",
  action    in MaKo::Action::"AdminMalo",
  resource  is MaKo::AdminMaloRecord
);

permit(
  principal == MaKo::Principal::"erp-operator",
  action    in MaKo::Action::"AdminPartner",
  resource  is MaKo::AdminPartnerRecord
);

// ── edifact-ingest: EDIFACT ingest only (AS4 gateway service account) ─────────
permit(
  principal == MaKo::Principal::"edifact-ingest",
  action    == MaKo::Action::"IngestEdifact",
  resource  is MaKo::EdifactIngest
);

// ── admin-readonly: read MaLo stats + list partners (monitoring / BI) ─────────
permit(
  principal == MaKo::Principal::"admin-readonly",
  action    == MaKo::Action::"AdminMaloStats",
  resource  is MaKo::AdminMaloRecord
);

permit(
  principal == MaKo::Principal::"admin-readonly",
  action    == MaKo::Action::"AdminPartnerRead",
  resource  is MaKo::AdminPartnerRecord
);

permit(
  principal == MaKo::Principal::"admin-readonly",
  action    == MaKo::Action::"ReadMetrics",
  resource  is MaKo::MetricsEndpoint
);

// ── mcp-agent: LLM / MCP assistant (read MaLo + submit commands + MCP tools) ──
permit(
  principal == MaKo::Principal::"mcp-agent",
  action    == MaKo::Action::"UseMcp",
  resource  is MaKo::McpEndpoint
);

permit(
  principal == MaKo::Principal::"mcp-agent",
  action    == MaKo::Action::"SubmitCommand",
  resource  is MaKo::Command
);

permit(
  principal == MaKo::Principal::"mcp-agent",
  action    == MaKo::Action::"AdminMaloRead",
  resource  is MaKo::AdminMaloRecord
);

// ── Rechnung reads: ERP integration and read-only operators ───────────────────
permit(
  principal == MaKo::Principal::"erp-operator",
  action    == MaKo::Action::"ReadRechnung",
  resource  is MaKo::RechnungEndpoint
);

permit(
  principal == MaKo::Principal::"admin-readonly",
  action    == MaKo::Action::"ReadRechnung",
  resource  is MaKo::RechnungEndpoint
);

// ── Migrations: a deliberate operator action, granted to no standing principal.
// Grant MaKo::Action::"AdminMigrations" to a dedicated break-glass principal
// in your site policy for the FV-cutover window, then remove it.

// ── Process reads: workflow-scoped (§9 EnWG Informatorisches Unbundling) ──────
// The context carries the process's workflow name, so a VIU site running with
// `--cedar-no-default-policy` can split process reads into role-scoped
// principals — an NB operator sees grid-side workflows and nothing else:
//
//   permit(principal == MaKo::Principal::"nb-operator",
//          action == MaKo::Action::"ReadProcess", resource)
//   when { context.workflow like "gpke-sperrung*" };
//
// This governs both MCP `get_process` and `list_overdue_deadlines`, which name
// workflows and their missed regulatory windows.  Without the flag the grant
// above adds nothing and both tools stay fully readable by every principal.
permit(
  principal == MaKo::Principal::"mcp-agent",
  action    == MaKo::Action::"ReadProcess",
  resource  is MaKo::ProcessRecord
);

permit(
  principal == MaKo::Principal::"admin-readonly",
  action    == MaKo::Action::"ReadProcess",
  resource  is MaKo::ProcessRecord
);

// ── API-Webdienste (:8090): a dedicated client principal ──────────────────────
permit(
  principal == MaKo::Principal::"api-webdienste-client",
  action    == MaKo::Action::"UseWebdienste",
  resource  is MaKo::WebdiensteEndpoint
);
