// Conservative MaKo authorization policy — least-privilege baseline.
//
// This file is NOT loaded automatically.  Enable it by passing
// `--cedar-policy-dir <dir>` at startup where this file (or a symlink) is
// placed alongside the schema file.
//
// Design intent
// ─────────────
// The default.cedar "permit all authenticated principals" policy is suitable
// for development and single-tenant deployments where all API keys are trusted
// equally.  In production, a leaked or compromised key should not grant
// full admin access.  This file replaces the catch-all permit with four
// named role grants, each covering only the actions needed for that role.
//
// Roles (add further specialised principals as needed)
// ─────────────────────────────────────────────────────
//   erp-operator    — full ERP integration: submit commands + read/write MaLo
//   edifact-ingest  — AS4 gateway: ingest EDIFACT messages only
//   admin-readonly  — read-only admin: MaLo stats + partner list
//   mcp-agent       — LLM / MCP access: submit commands + MaLo read + MCP tools
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
// clauses to match the key names configured in `--api-keys`.  Use the action
// groups defined in mako.cedarschema (AdminMalo, AdminPartner) to grant
// multiple related actions in a single permit statement.
//
// IMPORTANT: do NOT add `permit(principal is MaKo::Principal, action, resource)`
// (the default.cedar catch-all) alongside this file — that would defeat the
// purpose of this conservative baseline.

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
