# mako-pruefung

[![Crates.io](https://img.shields.io/crates/v/mako-pruefung?label=mako-pruefung&color=f59e0b&logo=rust)](https://crates.io/crates/mako-pruefung)

The BDEW Entscheidungsbäume as a library: which answer a German energy market
participant owes on an inbound message. Each tree in *Entscheidungsbaum-Diagramme
und Codelisten* 4.3 names a prüfende Rolle, walks numbered Prüfschritte and
lands on a code from its own Codeliste; this crate makes them executable.
`mako-fristen` says *when* to answer, `mako-pruefung` says *what*. No I/O, no
clock, no async.

## Example

```rust
use mako_pruefung::codes;

// A code has no meaning without its tree: A02 is two different answers here.
let gpke_abmeldung = codes::lookup("E_0607", "A02").expect("published code");
let gpke_anmeldung = codes::lookup("E_0622", "A02").expect("published code");
assert_ne!(gpke_abmeldung.bedeutung, gpke_anmeldung.bedeutung);

// The code's published Cluster picks the answer PID — never a separate bool.
println!("{} → {:?}", gpke_abmeldung.bedeutung, gpke_abmeldung.cluster);
```

The walks — `nb::evaluate`, `lf::pruefe_beendigung_zuordnung`, `msb::…`,
`esa::…`, `rechnung::…`, `mabis::…`, `emob::…` — take the message's facts plus
the caller's records and return a decision carrying its code and tree. The
Anmeldung walk (`E_0622`), for one, refuses a missing Zuordnungsermächtigung at
Prüfschritt 60 with `A05` and at 260 with `A25`.

## What it covers

| Module | Prüfende Rolle | Trees |
|---|---|---|
| `nb` | Netzbetreiber | Anmeldung `E_0622` → `E_0623`, Gas `E_3005` → `E_3007`; Abmeldung `E_0607` / `E_3019`; Neuanlage `E_0608` |
| `lf` | Lieferant | `E_0609`/`E_3002`, `E_0624`/`E_3020`, `E_0614`/`E_3001`, `E_0615`/`E_3008`; Strom `E_0603`–`E_0606` |
| `msb` | MSB and NB | `E_0200`–`E_0203`, Ersteinbau `E_0233`, Änderung der Technik `E_0249`/`E_0250`, `E_0278`/`E_0279`, `E_0281`, `E_0283`, `E_0286` |
| `esa` | MSB (Bestellung) | Wertebestellung `E_0252`, `E_0254`, `E_0256`, `E_0257` |
| `rechnung` (ungated) | ESA, LF and NB | one invoice walk: `E_0264`–`E_0267` (ESA), `E_0270`–`E_0277` (Preisblatt B ↔ LF / NB) |
| `emob` | NB (VNB) and LF | NZR-EMob / Modell 2, `E_0510`–`E_0513` |
| `mabis` | NB, LF and BKV | the 32 MaBiS and Redispatch-Ausfallarbeit trees in `mabis::codes::MABIS_TREES` |

Rules the crate keeps:

- **Resolve within the tree.** `codes::lookup(ebd, code)`; Strom and Gas use
  disjoint alphabets, and `E_0622`'s two Strom branches share no code.
- **Never guess.** A Prüfschritt the caller's records cannot answer
  (`Bekannt::Unbekannt`) escalates naming that Prüfschritt.
- **More than two outcomes.** `Cluster` has eight variants: besides
  `Zustimmung`/`Ablehnung`, an `Abweisung` was refused before it was assessed
  and its Prüfmitteilung is not forwarded (MaBiS Kap. 9.8.2 Nr. 2), and list,
  Reklamation and `E_0595` clusters sit off the agreement axis. `E_0608` can
  also answer `Vertagen`.
- These codes travel in `SG4 STS+E01` (UTILMD) or `AJT` (REMADV, ORDRSP); they
  are not ERC codes.

## Features

Each feature compiles only its own trees, so a role-gated build cannot answer
as a role it was not built for and a misconfiguration fails closed. All are on
by default.

| Feature | Trees |
|---|---|
| `role-nb` | `nb` |
| `role-lf` | `lf` |
| `role-msb` | `msb`; implies `role-esa` |
| `role-esa` | `esa`, without the Messstellenbetrieb trees |
| `role-mabis` | `mabis`, both NB and LF sides |
| `role-emob` | `emob`; a full Modell-2 deployment also needs `role-mabis` |

## Sources

- BDEW Entscheidungsbaum-Diagramme und Codelisten 4.3
- BK6-24-174 (GPKE, WiM Strom, MaBiS Anlage 3); BK7-24-01-009 (GeLi Gas)

Operator reference: [processd](https://hupe1980.github.io/mako/docs/services/processd/) ·
API: <https://docs.rs/mako-pruefung>

License: MIT OR Apache-2.0
