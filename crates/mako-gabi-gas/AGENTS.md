<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-gabi-gas

- Governed by **BK7-24-01-008** (GaBi Gas 2.1, Bundesnetzagentur).
- Scope: INVOIC **31010** only (Kapazitätsrechnung, FNB/VNB → BKV).
- GaBi Gas = gas balancing. Key roles: FNB, VNB, BKV, MGV. The BKV pays the FNB/VNB for transmission capacity.
- PID 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas) belongs to `mako-geli-gas`. Direction NB → LF (not NB → BKV) confirms this is NOT a GaBi Gas process.

## Cross-crate rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Each crate depends on `mako-engine` and `edi-energy`; domain crates must not depend on each other.

The authoritative PID-ownership and APERAK-Fristen tables are in the
root `AGENTS.md` under *Domain Rules*.
