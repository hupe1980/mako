<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-wim

- Governed by **WiM (Az. BK6-09-034)**, whose Anlage 1 — Teil 1 „Fokus
  Basis-Prozesse" und Teil 2 „Fokus Übermittlung von Werten" — is amended by
  **BK6-24-174 Anlagen 2a/2b** (Tenorziffer 2), the amendment before it being
  BK6-22-024. Cite BK6-24-174 plus the Teil and the chapter; the Fristen tables
  are unchanged from the BK6-22-024 edition, the definitions are not. Gas follows the **AWH WiM Gas 2.0** (gültig ab 01.10.2026).
- **One crate, both Sparten.** The Gas UTILMD PIDs 44039/44042/44051/44168/44183
  run the same workflows as their Strom twins; the Sparte picks the
  Entscheidungsbaum (`E_2000`…`E_2006` against `E_0200`…`E_0240`), the Codeliste
  (`G_00xx` against `S_00xx`), the APERAK regime and the Zuordnungszeitpunkt
  (06:00 Uhr Gastag against 00:00).
- Antwortfristen are 3 / 5 / 7 / 1 Werktage per PID **in both Sparten** — never a
  flat window, and never the APERAK clock.
- `SG4 STS+E01` DE 1131 and `SG2 AJT` DE 1082 carry the **Codeliste**, not the
  EBD number: ask `AntwortCode::wire_codeliste()`.
- Includes the WiM-Rechnung INVOIC workflow (`wim-invoic`): 31009
  Messstellenbetrieb (Strom), 31003 Dienstleistungen (**both Sparten**) and the
  Sparte-neutral Stornorechnung 31004. The answer is due *zum* Zahlungsziel
  (`SG8 DTM+265`), except 31009 to a **NB**, which is the **4. WT davor**
  (Kap. 6.2 Nr. 2) — `mako_fristen::vorlauf::rechnung_antwort_spaetester_uet`.
- `wim-insrpt` hosts **both sides** of the Störungsbehebung. Its two windows
  branch on the Messtechnik, which no message carries; the Weiterleitung
  23011/23012 stays due after the Ergebnisbericht has closed the Use-Case.

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
