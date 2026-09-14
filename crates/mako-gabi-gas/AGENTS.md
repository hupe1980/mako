<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-gabi-gas

- Governed by **BK7-24-01-008** (GaBi Gas 2.1, Bundesnetzagentur).
- Scope: `GaBiGasModule` registers five workflows — `gabi-gas-invoic`
  (INVOIC **31010** Kapazitätsrechnung FNB/VNB → BKV, **31007**/**31008**
  aggregierte MMM-Rechnung Gas NB → MGV, REMADV **33001** Zahlungsavis and
  COMDIS **29001** Ablehnung REMADV), `gabi-gas-mmma` (MSCONS **13013**,
  marktlokationsscharfe Allokationsliste Gas NB → LF), and the three DVGW
  carriers `gabi-gas-allocation` (ALOCAT, PIDs 70001–70023),
  `gabi-gas-nomination` (NOMINT/NOMRES, 70030–70039) and
  `gabi-gas-mehr-mindermengen` (SSQNOT, 70095–70096). `workflow_names()` in
  `src/lib.rs` is the list; the module docs carry the per-PID routing.
- ORDERS **17110** / ORDRSP **19110** (Anfrage und Ablehnung der Allokationsliste
  Gas) open the exchange MSCONS 13013 answers, but are **routed by `mako-gpke`**
  under `gpke-allokationsliste`, which carries the Strom pair beside them. Named
  here, registered there — one PID, one crate.
- The DVGW formats `dvgw-edi` cannot parse — SCHEDL, IMBNOT, TRANOT,
  DELORD/DELRES, CHACAP, NUEVOR, SLPASP, TSIMSG — have no workflow and no PID
  here: registering one would overstate what the router handles.
- GaBi Gas = gas balancing. Key roles: FNB, VNB, BKV, MGV. The BKV pays the FNB/VNB for transmission capacity.
- PID 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas) belongs to `mako-geli-gas`. Direction NB → LF (not NB → BKV) confirms this is NOT a GaBi Gas process.

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
