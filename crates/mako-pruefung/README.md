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

The process walks — `nb::evaluate`, `lf::pruefe_beendigung_zuordnung`,
`msb::…`, `esa::…`, `rechnung::…`, `mabis::…`, `emob::…` — take the message's
facts plus the caller's records and return a decision carrying its code and tree.

## Roles

| Module | Prüfende Rolle | Processes |
|---|---|---|
| `nb` | Netzbetreiber | Anmeldung (`E_0622` → `E_0623` / `E_3005` → `E_3007`), Abmeldung (`E_0607` / `E_3019`), Neuanlage (`E_0608`) |
| `lf` | Lieferant | Abmeldung (`E_0609`/`E_3002`), Beendigung der Zuordnung (`E_0624`/`E_3020`), Kündigung (`E_0614`/`E_3001`), Anmeldung E/G (`E_0615`/`E_3008`), Zuordnung LF (`E_0603`–`E_0606`, Strom only) |
| `msb` | Messstellenbetreiber **und** Netzbetreiber | Anmeldung MSB (`E_0201`), Ende MSB (`E_0202`), Kündigung MSB (`E_0200`), Weiterverpflichtung (`E_0203`), Ersteinbau iMS (`E_0233`), Änderung der Technik (`E_0278`·`E_0281`, `E_0279`·`E_0283`, `E_0286`), Rechnungsabwicklung über den LF (`E_0205`–`E_0209`), Preisblatt-B-Abrechnung (`E_0270`–`E_0277`) |
| `esa` | MSB (Bestellung) **und** Energieserviceanbieter (Abrechnung) | Wertebestellung (`E_0252`, `E_0256`, `E_0257`, `E_0254`); the Kap. 4.5 Abrechnung (`E_0264`–`E_0267`) is `rechnung::ESA` bound |
| `rechnung` (ungated) | ESA, LF **und** NB | One invoice walk, three families: `E_0264`–`E_0267` (ESA), `E_0270`–`E_0272`/`E_0276` (Preisblatt B ↔ LF), `E_0273`–`E_0275`/`E_0277` (↔ NB) |
| `emob` | NB (VNB) **und** LF | NZR-EMob / Modell 2 (`E_0510`–`E_0513`): Anmeldung, Beendigung der Zuordnung, Abmeldung |
| `mabis` | NB, LF **und** BKV | Summenzeitreihen (`E_0007`, `E_0040`–`E_0041`, `E_0062`–`E_0065`, `E_0093`, `E_0098`/`E_0099`), Listenabgleich (`E_0004`, `E_0014`, `E_0017`, `E_0047`, `E_0049`, `E_0052`, `E_0070`, `E_0096`, `E_0097`), MaBiS-ZP (`E_0010`, `E_0020`, `E_0102`, `E_0103`), MaBiS-ZP für die monatliche AAÜZ (`E_0071`/`E_0072` NB↔BKV(LF), `E_0078`/`E_0079` NB↔anfNB), Profile (`E_0100`), Einzelanforderung/Listeninhalt (`E_0068`, `E_0104`), Redispatch-Ausfallarbeit (`E_0901`, `E_0902`) — 32 trees in `mabis::codes::MABIS_TREES` |

The `role-nb`, `role-lf`, `role-msb`, `role-esa`, `role-mabis` and `role-emob`
features (all on by default) compile only their own rules, so a role-gated
binary cannot form a decision it is not entitled to make (§ 6a EnWG
informatorische Entflechtung). `role-msb` implies `role-esa`.

## Rules the crate keeps

- **Resolve within the tree.** `codes::lookup(ebd, code)`; Strom and Gas use
  disjoint alphabets, and `E_0622`'s two branches share no code.
- **Never guess.** A Prüfschritt the caller's records cannot answer
  (`Bekannt::Unbekannt`) escalates naming that Prüfschritt; a Vorgang that fits
  no edge is refused, never walked through to a Zustimmung.
- **More than two outcomes.** `E_0608` can answer `NeuanlageEntscheidung::Vertagen`
  (re-checked daily for 60 Werktage), and MaBiS adds clusters off the agreement
  axis:

| Cluster | What it means | Consequence |
|---|---|---|
| `Zustimmung` / `Ablehnung` | the agreement axis | picks the answer PID |
| `Abweisung` | refused **before** it was assessed | its Prüfmitteilung is **not forwarded** (MaBiS Kap. 9.8.2 Nr. 2) |
| `AblehnungDerGesamtenListe` | the whole list is refused | carries **no** positions; resend a whole list |
| `KorrekturlisteWegenAblehnung` | individual positions are disputed | the answer **is** a list, one entry per Marktlokation |
| `Reklamation` | the tree publishes only complaints | there is no Zustimmung; an acceptable profile is answered with silence |
| `AenderungDerDaten` / `KeineAenderungDerDaten` | `E_0595` — whether data follows | off the agreement axis |

These codes travel in `SG4 STS+E01++<code>:<ebd>`; they are not ERC codes (those
are `mako_engine::erc`).

API documentation: <https://docs.rs/mako-pruefung>.
