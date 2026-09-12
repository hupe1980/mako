<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-gpke

- Governed by **BK6-24-174** (Teil 1–3, eff. 2025-06-06) and **BK6-22-024** (Teil 4 Konfiguration).
- Source modules: `wechselprozesse`, `lf_anmeldung`, `lf_abmeldung`, `neuanlage`, `abrechnung`, `sperrung`, `konfiguration`, `post_acceptance`.
- The `ForwardCompatible` version policy is mandatory for all GPKE workflows.

## Cross-crate rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Each crate depends on `mako-engine` and `edi-energy`; domain crates must not depend on each other.

The authoritative PID-ownership and APERAK-Fristen tables are in the
root `AGENTS.md` under *Domain Rules*.
