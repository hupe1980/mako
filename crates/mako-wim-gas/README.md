# mako-wim-gas

**WiM Gas — Wechselprozesse im Messwesen Gas**

Process engine workflows for the German gas metering-point operator change
processes. Governed by the BDEW WiM Gas specification and the BNetzA ruling
**BK7-24-01-009** ("GeLi Gas 3.0", same umbrella ruling as GeLi Gas supplier
switching). Current process documentation: **AWH WiM Gas V2.0**
(BDEW/VKU/GEODE/FNBGas, 2025-08-04).

## Key boundaries

### WiM Gas vs. GeLi Gas

Both live under BK7-24-01-009 but cover entirely different processes:

| Aspect | GeLi Gas (`mako-geli-gas`) | WiM Gas (`mako-wim-gas`) |
|---|---|---|
| Scope | Supplier switching (Lieferbeginn/-ende) | MSB change (Anmeldung/Kündigung gMSB) |
| Parties | LFN ↔ GNB | MSBN / MSBA ↔ NB / gMSB |
| EDIFACT PIDs | 44001–44021, 44022–44024 | 44039–44053, 44168–44170 |
| APERAK Frist | 10 Werktage | 10 Werktage |

### WiM Gas vs. WiM Strom

| Aspect | WiM Strom (`mako-wim`) | WiM Gas (`mako-wim-gas`) |
|---|---|---|
| Medium | Electricity | Gas |
| APERAK Frist | **5 Werktage** | **10 Werktage** |
| Ruling | BK6-24-174 | BK7-24-01-009 |
| EDIFACT | UTILMD S2.x | UTILMD G1.x |

> **Saturdays, Sundays and public holidays are not Werktage** for either.

## PID Inventory

| PID range | Process | Status |
|---|---|---|
| 44039–44041 | Kündigung MSB Gas (Anfrage / Bestätigung / Ablehnung) | ✅ Implemented (`WimGasKuendigungWorkflow`) |
| 44042–44044 | Anmeldung neuer MSB Gas (MSBN ↔ NB) | ✅ Implemented (`WimGasAnmeldungWorkflow`) |
| 44051–44053 | Ende MSB Gas / Vorläufige Abmeldung (NB ↔ MSBA) | ✅ Implemented (`WimGasAnmeldungWorkflow`) |
| 44168–44170 | Verpflichtungsanfrage (NB → gMSB) | ✅ Implemented (`WimGasVerpflichtungsanfrageWorkflow`) |
| 23005 | Ablehnung Störungsmeldung Gas-Variante — **Gas-only** | ✅ Implemented (`WimGasInsrptWorkflow`) |
| 23009 | Ergebnisbericht Störungsmeldung Gas-Variante — **Gas-only** | ✅ Implemented (`WimGasInsrptWorkflow`) |

### INSRPT Störungsmeldungen — Gas-only PIDs

PIDs 23005 and 23009 carry Gas-specific fault message qualifiers used exclusively
in the gas metering-point context. They are always owned by `mako-wim-gas` — even
in combined Strom+Gas deployments. The shared PIDs (23001, 23003, 23004, 23008)
are registered by `mako-wim` in a combined deployment and by this crate in a
Gas-only deployment.

| PID | Role in process | APERAK Frist | Notes |
|---|---|---|---|
| **23005** | Ablehnung (Gas-Variante) | 10 Werktage | Gas-only; always routed to `wim-gas-insrpt` |
| **23009** | Ergebnisbericht (Gas-Variante) | 10 Werktage | Gas-only; always routed to `wim-gas-insrpt` |

## EDIFACT Format Versions

| Format version | Valid from | Valid until | Profile status |
|---|---|---|---|
| `FV2025-10-01` | 2025-10-01 | 2026-09-30 | ✓ available (UTILMD AHB Gas 1.1) |
| `FV2026-10-01` | 2026-10-01 | — | ✓ available (UTILMD AHB Gas 1.2) |

## Regulatory references

- **BNetzA BK7-24-01-009** — GeLi Gas 3.0 / WiM Gas ruling,
  Beschluss 12.09.2025, abgeschlossen 24.09.2025
- **BDEW/VKU/GEODE/FNBGas AWH WiM Gas V2.0** (2025-08-04) —
  `docs/pdfs/bdew-mako/BDEW_VKU_GEODE_FNBGas_AWH_WiMGas_V2_0_20250804.pdf`
- **UTILMD AHB Gas 1.1 / 1.2** — EDIFACT message specification
