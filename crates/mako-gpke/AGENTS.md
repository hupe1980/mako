<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-gpke

- Governed by **BK6-24-174** (Teil 1–3, eff. 2025-06-06) and **BK6-22-024** (Teil 4 Konfiguration).
- `GpkeModule::workflow_names()` (`src/lib.rs`) is the authoritative list of the
  workflows this crate registers — 23 of them, each entry the owning module's own
  `WORKFLOW_NAME` constant. Read it there; a list restated here drifts, and
  `EngineBuilder::build` only checks `workflow_names()` against what
  `register_pids_with_roles` routes to.
- A GPKE process keeps its creation `WorkflowId`; the inbound message's own FV
  picks the adapter. Every registry must cover every known FV — there is no
  per-workflow acceptance policy to set.

## Cross-crate rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Every domain crate depends on `mako-engine`. The EDIFACT ones also carry
  `edi-energy` — `mako-gpke`, `mako-wim`, `mako-geli-gas` and `mako-mabis` as a
  dev-dependency for the parse → validate → execute integration tests, while
  production code uses `mako-engine`'s `ProfileRequirement`. `mako-gabi-gas`
  carries `dvgw-edi` instead; `mako-redispatch` and `mako-emob` carry neither.
- Shared decision and state-machine crates may be depended on: `mako-fristen`,
  `mako-pruefung`, `mako-invoic`. Sibling **workflow** crates may not.

The authoritative PID-ownership and APERAK-Fristen tables are in the
root `AGENTS.md` under *Domain Rules*.
