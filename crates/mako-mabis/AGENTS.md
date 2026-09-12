<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-mabis

- Governed by **BK6-24-174**.
- MSCONS **13003 + 13010–13012** (Bilanzkreisabrechnung Strom, BKV↔ÜNB/BIKO) via
  `mabis-billing`; UTILMD Clearinglisten **55065/55069/55070** via `mabis-clearingliste`.
- The remaining 130xx Messwesen PIDs do **not** belong here.

## Cross-crate rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Each crate depends on `mako-engine` and `edi-energy`; domain crates must not depend on each other.

The authoritative PID-ownership and APERAK-Fristen tables are in the
root `AGENTS.md` under *Domain Rules*.
