# mako-pruefung

**The decisions a German energy market participant owes on an inbound message.**

Every GPKE, GeLi Gas and WiM message that expects an answer has published rules for
what that answer must be. BDEW prints them as *Entscheidungsbaum-Diagramme und
Codelisten für die Antwortnachrichten* (4.3, 01.04.2026): each set of rules
names a **prüfende Rolle**, walks numbered **Prüfschritte**, and lands on a code
from its own Codeliste. This crate is those rules, executable.

`mako-fristen` says *when* you must answer. `mako-pruefung` says *what*.

## Roles

| Module | Prüfende Rolle | Processes |
|---|---|---|
| `nb` | Netzbetreiber | Anmeldung (`E_0622` → `E_0623` / `E_3005` → `E_3007`), Abmeldung (`E_0607` / `E_3019`), Neuanlage (`E_0608`) |
| `lf` | Lieferant | Abmeldung (`E_0609`/`E_3002`), Beendigung der Zuordnung (`E_0624`/`E_3020`), Kündigung (`E_0614`/`E_3001`), Anmeldung E/G (`E_0615`/`E_3008`) |
| `msb` | Messstellenbetreiber **und** Netzbetreiber | Anmeldung MSB (`E_0201`), Ende MSB (`E_0202`), Kündigung MSB (`E_0200`), Weiterverpflichtung (`E_0203`) |

The `msb` module is named for the process family, not one Marktrolle: WiM Teil 1
has the NB answer the Anmeldung and the Abmeldung, while the abgebender MSB
answers the Kündigung and the Weiterverpflichtung. The Kündigung never reaches
the NB at all, so its Prüfschritte ask about the MSBA's own
Messstellenbetriebsvertrag rather than about any grid registry.

The document defines around sixty trees with the LF as prüfende Rolle. The ones
here are the **process** answers — the messages that move a Marktlokation
between suppliers. The rest (Rechnungsprüfung `E_0406` / `E_0519`,
Stammdatenänderung `E_0408`, MaBiS `E_0004`, …) are separate obligations that
belong beside the service owning their data.

Each role module is split **by process**, with the Strom tree and its Gas
counterpart together — they are the same business decision expressed in two
documents.

The `role-nb`, `role-lf` and `role-msb` Cargo features compile only their own
rules, so a role-gated binary carries only the decisions it is licensed to make
(§ 7 EnWG).

## A code has no meaning without its tree

`A02` is

- „Vorlauffrist nicht eingehalten" in `E_0607`,
- „Marktlokation nimmt nicht an der Marktkommunikation teil" in `E_0622`,
- „Lieferende zum Abmeldedatum wurde bereits bestätigt" in `E_0609`,
- „Änderung kann durchgeführt werden" — a *Zustimmung* — in `E_0249`,

and a combined NB+LF+MSB deployment runs all four. The WiM MSB-Wechsel trees
share no code with the GPKE ones at all: a rejection there is `ZC9`, `Z29`,
`Z34`, `E11`, `E17` or `Z09`. `codes::lookup(ebd, code)`
resolves a code **within** its tree, and the code's published `Cluster` is what
selects the answer PID. A caller never passes an `accepted: bool` alongside a
code, because the two can disagree.

Not every tree clusters on agreement. `E_0595` („Bestellung prüfen", the
Bearbeitungsstand on Abrechnungsdaten) splits its codes into „Änderung der Daten"
/ „keine Änderung der Daten" — whether a Stammdatenänderung follows, which both
IFTSTA 21047 answers carry. `ist_zustimmung()` returns `None` there rather than
reading „keine Änderung" as a refusal, and `sendet_stammdatenaenderung()` is the
question that tree does answer.

The split runs deeper than one code. `E_0622` Prüfschritt 10 divides Strom into a
verbrauchende/ruhende branch and an erzeugende one that share **nothing**: „andere
Anmeldung in Bearbeitung" is `A06` in the first and `A45` in the second. Gas
answers from a different alphabet again — `ZC5` for the same question, `A16`
where Strom says `A02`, `E17` where it says `A07`, `E13` where it says `A05`. A
Strom code on a 44003 is not a wrong reason; it is undefined.

`E_0608` (Neuanlage) has a **third outcome** on top of the usual three:
Prüfschritte 110 / 590 loop, so a Marktlokation the NB cannot yet identify is
re-checked daily for 60 Werktage before a refusal is admissible.
`NeuanlageEntscheidung::Vertagen` is that state — the NB answers nothing at all
that day, which no two-outcome engine can express.

A **Vorprüfung** publishes only Ablehnungen. `E_0622` and `E_3005` can refuse a
message but never agree to one — a survivor is confirmed out of `E_0623` / `E_3007`,
and `VORPRUEFUNG_TREES` names the pair. `SG4 STS+E01` is Muss on every
Antwortnachricht, so `NbEntscheidung::Accept` carries `A51` / `A58` / `E15`
rather than nothing.

These codes travel in `SG4 STS+E01++<code>:<ebd>` of the answering UTILMD. They
are **not** ERC codes: `ERC` is the APERAK/CONTRL segment for processability
errors, with its own catalogue in `mako_engine::erc`.

## Never guess

A Prüfschritt the caller's records cannot answer produces an escalation naming
that Prüfschritt — never a plausible code. `Bekannt::Unbekannt` is what carries
"we have no record either way" into the walk; collapsing it to `false` is how a
supplier ends up silently agreeing to release a customer it still has under
contract.

## Design constraints

- **No I/O** — every input is a function argument.
- **No clock** — the current instant is passed in.
- **Deterministic** — same inputs, same output.
- **No async.**

## Usage

```rust,ignore
use mako_pruefung::{Bekannt, LfAnfrage, LfVertragslage, pruefe_beendigung_zuordnung};

let entscheidung = pruefe_beendigung_zuordnung(&anfrage, &lage);

match entscheidung.as_antwort() {
    Some(a) => println!("{} ({}) — {}", a.code, if a.zustimmung { "Zustimmung" } else { "Ablehnung" }, a.bedeutung),
    None => println!("operator decision required"),
}
```
