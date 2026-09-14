<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-geli-gas

- Governed by **BK7-24-01-009** (GeLi Gas 3.0, Beschluss 12.09.2025). Supersedes BK7-19-001 and BK7-06-067.
- Scope: UTILMD G (PIDs 44001–44024, incl. the Stornierung shared with WiM Gas) + ORDERS Sperrung Gas (17115–17117) + PARTIN Gas (37008–37014) + **INVOIC 31011** (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF).
- PID 31011 is billed by GNB/VNB to LFN/LFA for performing AWH (Sperrung/Entsperrung). Direction is NB → LF — NOT NB → BKV. This is GeLi Gas (BK7-24-01-009), not GaBi Gas (BK7-24-01-008).
- APERAK: Gas knows only the Verarbeitbarkeitsfehlermeldung — see the table above.

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
