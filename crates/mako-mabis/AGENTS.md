<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# crates/mako-mabis

- Governed by **BK6-24-174**.
- `MabisModule::workflow_names()` (`src/lib.rs`) registers six workflows:
  `mabis-billing` (MSCONS **13003 + 13010–13012**, Bilanzkreisabrechnung Strom,
  BKV↔ÜNB/BIKO), `mabis-profile`, `mabis-clearingliste` (UTILMD Clearinglisten
  **55065/55069/55070**), `mabis-zp-lifecycle`, `mabis-anforderung` and
  `mabis-listenabgleich`. Each name is the owning module's own `WORKFLOW_NAME`
  constant — read the list from the function, not from a restatement.
- The remaining 130xx Messwesen PIDs do **not** belong here.
- `mabis-zp-lifecycle` runs **both directions**: `SendAnfrage`/`ReceiveAntwort`
  where this participant activates its own MaBiS-Zählpunkte, and
  `ReceiveAnfrage`/`SendAntwort`/`SendWeiterleitung` where it answers. The
  Netzzeitreihe axis is NB → NB, so both are live in one deployment.
  `SendWeiterleitung` is reachable only from the answering leg — Prozessschritt 4
  is the BIKO's step toward the BKV.
- Prozessschritt 2 is **1 Werktag nach Erhalt** in every Use-Case of that family
  that has an Antwort, and an Ablehnung carries a Begründung (BK6-24-174
  Anlage 3, SD Nr. 2). The window is published per Anfrage PID in
  `mako_fristen::antwort::MABIS` for 55203/55206/55209/55212 only: 55062/55063
  are shared by eleven Summenzeitreihen, five of which owe no answer.
- An inbound Antwortcode is `(DE 9013, DE 1131)`, never DE 9013 alone. Resolve it
  with `zp_antwort_ist_zustimmung`; an uncatalogued tree answers `None` and must
  be refused, not defaulted to Ablehnung. `A12` is the Zustimmung of `E_0020`
  and an Ablehnung in `E_0071` — the pair is the whole discriminator.
- `zp_register::ZpRegister` folds the lifecycle streams into *which MaBiS-ZP are
  activated now*, which no single process knows: an Aktivierung and the
  Deaktivierung that ends it are two streams. A family with no Antwort PID (the
  tägliche AAÜZ is one) activates on `Erfasst` alone — counting only confirmed
  answers would report zero active Zählpunkte for exactly the series BK6-23-241
  Tenorziffer 5 repeals on 30.09.2026.

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
