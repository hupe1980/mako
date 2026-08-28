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
| `lf` | Lieferant | Abmeldung (`E_0609`/`E_3002`), Beendigung der Zuordnung (`E_0624`/`E_3020`), Kündigung (`E_0614`/`E_3001`), Anmeldung E/G (`E_0615`/`E_3008`), Zuordnung LF (`E_0603`–`E_0606`, Strom only) |
| `msb` | Messstellenbetreiber **und** Netzbetreiber | Anmeldung MSB (`E_0201`), Ende MSB (`E_0202`), Kündigung MSB (`E_0200`), Weiterverpflichtung (`E_0203`) |
| `mabis` | NB, LF **und** BKV | Summenzeitreihen (`E_0007`, `E_0040`–`E_0041`, `E_0062`–`E_0065`, `E_0093`, `E_0098`/`E_0099`), Listenabgleich (`E_0004`, `E_0014`, `E_0017`, `E_0047`, `E_0049`, `E_0052`, `E_0070`, `E_0096`, `E_0097`), MaBiS-ZP (`E_0010`, `E_0020`, `E_0102`, `E_0103`), Profile (`E_0100`), Redispatch-Ausfallarbeit (`E_0901`, `E_0902`) |

The `msb` module is named for the process family, not one Marktrolle: WiM Teil 1
has the NB answer the Anmeldung and the Abmeldung, while the abgebender MSB
answers the Kündigung and the Weiterverpflichtung. The Kündigung never reaches
the NB at all, so its Prüfschritte ask about the MSBA's own
Messstellenbetriebsvertrag rather than about any grid registry.

The `mabis` module is likewise named for the regulation (BNetzA BK6-24-174
Anlage 3) rather than one Marktrolle, because MaBiS keys its trees on the
**Summenzeitreihe** and not on who receives it: for Kategorie B the ÜNB is
aggregationsverantwortlich, so it sends the BG-SZR that the NB checks, the
LF-SZR that the LF checks and the BK-SZR that the BKV checks. Only the trees
mako can actually decide are catalogued — the BIKO, ÜNB and
Redispatch-Betreiber trees are deliberately absent, because shipping their
codes would claim decisions this platform never makes.

The document defines around sixty trees with the LF as prüfende Rolle. The ones
here are the **process** answers — the messages that move a Marktlokation
between suppliers. The rest (Rechnungsprüfung `E_0406` / `E_0519`,
Stammdatenänderung `E_0408`, …) are separate obligations that belong beside the
service owning their data.

Each role module is split **by process**, with the Strom tree and its Gas
counterpart together — they are the same business decision expressed in two
documents.

The `role-nb`, `role-lf`, `role-msb` and `role-mabis` Cargo features compile
only their own rules, so a role-gated binary carries only the decisions it is
licensed to make (§ 7 EnWG).

## Eight clusters, not two

A GPKE answer is a Zustimmung or an Ablehnung, and the cluster picks between two
PIDs. MaBiS adds four more, and every one of them is observable on the wire:

| Cluster | What it means | Consequence |
|---|---|---|
| `Zustimmung` / `Ablehnung` | the agreement axis | picks the answer PID |
| `Abweisung` | refused **before** it was assessed | its Prüfmitteilung is **not forwarded** (MaBiS Kap. 9.8.2 Nr. 2) |
| `AblehnungDerGesamtenListe` | the whole list is refused | carries **no** positions; resend a whole list |
| `KorrekturlisteWegenAblehnung` | individual positions are disputed | the answer **is** a list, one entry per Marktlokation |
| `Reklamation` | the tree publishes only complaints | there is no Zustimmung; an acceptable profile is answered with silence |
| `AenderungDerDaten` / `KeineAenderungDerDaten` | `E_0595` — whether data follows | off the agreement axis |

`ist_zustimmung()` answers `None` on the two axes that are not about agreement,
so a caller cannot read a Profil-Reklamation as a refusal of the profile — it
is not one, and the LF keeps bilanzierend with that profile until a corrected
version arrives.

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

The rule extends past the *facts* to the **message**. A walk reaches its
Zustimmung by traversing edges, so a Vorgang that fits no edge must not arrive
there by falling past every branch:

- **No Transaktionsgrundergänzung** (`SG4 STS+7` DE 9013 element 3). `E_0609`
  and `E_0624` split on it at Prüfschritt 10 and the two halves answer from
  different code ranges — `A10` where the counterparty expects `A29`. The AHB
  marks the element Muss precisely because there is no default.
- **A Transaktionsgrund outside the AHB's set.** `E_0609` branches on it at 50
  and 80; the three grounds a 55007 may carry are `Z33`, `ZQ7` and `ZT0`. A
  fourth value has no path, and reaching the terminal would *confirm* an
  Abmeldung the walk never examined.
- **The wrong date qualifier.** `DTM+93` „Ende zum" and `DTM+471` „Ende zum
  nächstmöglichen Termin" are mutually exclusive on a Kündigung, and both
  Sparten branch on which arrived. Only a **fixed** date may be refused for
  Vertragsbindung — `E_0614` Prüfschritt 60 on the Strom side, and on the Gas
  side an AHB Bedingung: `Z12` is gated on `[43] Wenn SG4 DTM+93 vorhanden`,
  `Z01` „Zustimmung mit Terminänderung" on `[41] Wenn SG4 DTM+471 vorhanden`.
  Answering `Z12` to a `DTM+471` Kündigung is not merely the wrong business
  answer; it fails AHB validation at the counterparty.
- **No `SG4 DTM+154`** on a 55010. `E_0624` Prüfschritt 5 is the tree's *first*
  question and measures its window from the ÜT der Lieferanmeldung. A message
  without one cannot be measured, and skipping the step accepts every late
  Anfrage — the one thing `A43` exists to refuse.

## Three contract dates, three questions

The Kündigungs-Prüfschritte read three different dates and the crate keeps them
apart, because collapsing any two of them answers a code that is false:

| `LfVertragslage` field | What it is | Read by |
|---|---|---|
| `vertragsende` | a **recorded termination** — somebody has cancelled, to this date | `E_0614` 40/50/80 (`A03` „bereits gekündigt", `A05` „bereits in der Zukunft beendet"), Gas `Z29`/`Z34` |
| `naechstmoeglicher_kuendigungstermin` | the next date notice could take effect, as of the Kündigungseingang | `E_0614` 70/580, the `DTM+157` on `A06`/`A15`, the `DTM+471` on a „nächstmöglich" Bestätigung, Gas `Z12`/`Z01` |
| `vertragsbindung_am_folgetag` | does the Vertragsverhältnis survive the requested date | `E_0624` 90/220 and Gas `E_3020` — **and nothing else** |

Prüfschritt 70 („kündbar unter Einhaltung der Kündigungsfrist?") compares the
second against the requested Termin, not the third: every unterminated contract
is running, so the third would refuse `A06` to every § 20a EnWG switch.

`vertrag_vorhanden` is likewise a record, not an inference — `E_0614`
Prüfschritt 500 produces `A18` only from a stated `Nein`, because a deployment
that cannot look a contract up finds nothing for *every* object.

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
