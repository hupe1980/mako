+++
title = "The Desk"
description = "mako Desk, the operator's web UI: open a process by its business key, see every Frist in Berlin time, work one market role at a time — on the same typed MCP tools the agents use."
weight = 3
+++
**mako Desk** is the operator's work surface. It reads the services' MCP
tools — the same typed read surface the agents are granted — and renders what
they answer. It computes nothing that decides: every Frist, amount and verdict
on screen is the one a service returned.

[![Prozesse: one MaLo, two processes, the earliest Frist first](prozesse-light.png)](prozesse-light.png)
[![Prozesse: one MaLo, two processes, the earliest Frist first](prozesse-dark.png)](prozesse-dark.png)

## Run it

```sh
just desk
```

starts makod and the Desk with hot reload, and opens `http://localhost:5173`.

In production the Desk is the `mako-desk` image — the static bundle on port
8080 — behind [oauth2-proxy](https://oauth2-proxy.github.io/oauth2-proxy/) as
the gateway. The browser talks to one origin and holds no service token: the
gateway signs the person in by OIDC, serves the Desk, and forwards
`/svc/<service>/` to each service the Desk reads — makod, processd, vertragd,
invoicd, billingd, edmd, netzbilanzd and agentd — with the person's access
token as `Authorization: Bearer`. `desk/deploy/gateway.yaml` is the
gateway's routing, ready to adapt.

Each service verifies that token as it does any other, so its
`[oidc] audience` must be in the token's `aud` — an audience mapper at the
identity provider, or one audience shared by all of them. Cedar decides what the
person may do; the Desk keeps no role map. Its CSP allows the bundle's own
script and stylesheet only, plus a per-request nonce for the styles React
Aria injects.

## A process, by its business key

Type a MaLo-ID, MeLo-ID or Vorgangsnummer on **Prozesse**, or anywhere press
<kbd>⌘K</kbd> and type it: the palette offers to open it directly.

[![The command palette offering to open a process by its MaLo-ID](palette-light.png)](palette-light.png)
[![The command palette offering to open a process by its MaLo-ID](palette-dark.png)](palette-dark.png)

A key can name several processes — a Lieferantenwechsel beside an
Anfrage/Bestellung. Each is a card, ordered by its earliest pending Frist,
leaving out any process the person's role may not read.

Each card is titled the way an operator says it — „Lieferantenwechsel · Strom"
— with the workflow and its ids under „Technische Angaben".

Its **Verlauf** threads the market messages: received on the left, sent on the
right, each with PID, Berlin time and document number, as the workflow
recorded them. An answer also states its code and the BDEW's wording from the
list that defines it („A02 — Marktlokation … nimmt nicht an der
Marktkommunikation teil"); where that list publishes no wording for the code,
the line names the list alone. Steps between two messages fold into one quiet
row („2 interne Schritte"). Pending Fristen close the thread, each named for what it is — an
„Eingangsbestätigung (APERAK)", a „Syntaxquittung (CONTRL)" or a „Frist des
Prozesses".

The **Frist-Uhr** says how far a deadline is: red inside two hours, amber
inside a day, solid red once it has lapsed. The time beside it is Berlin time;
hover or focus shows the instant in UTC as the wire carries it.

A key with no process gets the Marktkommunikation's own answer, said calmly:

[![The answer for a key that names no process](refusal-light.png)](refusal-light.png)
[![The answer for a key that names no process](refusal-dark.png)](refusal-dark.png)

## Ask the assistant

Under a customer, a MaLo's processes and a Freigabe, the Desk asks the
assistant — type a question, take a suggestion, or press <kbd>⌘J</kbd>. It
reads the same services the screen does, names the model that answers, and
ticks off each service as it reads it. The answer opens with „AI-generated.";
the assistant reads and never acts.

It answers in at most five sentences, each with the read it rests on, and each
checked before it is shown: every MaLo-ID, date, time and amount in it must
appear in that read's answer. One that does not is marked **nicht belegt** —
typically a figure the model worked out itself, such as „noch 25 Stunden".
The check says the figures are in what it read, not that its reasoning is
right.

If it finds no answer, „Nochmal fragen" asks again. When the assistant is not
running, the panel is not shown and the rest of the Desk works as before.

[![The assistant's answer about a customer, one sentence marked „nicht belegt"](fragen-light.png)](fragen-light.png)
[![The assistant's answer about a customer, one sentence marked „nicht belegt"](fragen-dark.png)](fragen-dark.png)

What a skill noticed on its own — a Frist that passed, an Anmeldung that
stalled — arrives as a **Hinweis**: at the top of Heute and the Überblick, and
under the customer or MaLo it concerns, in the skill's own words (a
model-written one opens with „AI-generated."). A skill can be muted in one
click and shown again in one; muting is this browser's and deletes nothing.

**Assistent** in the sidebar is each skill's package leaflet, read off the
reviewed description agentd runs: what it is for, whether it is active here,
what starts it — a question or an event — which model it uses or that it uses
none, and which services it may read, each tool's reason on hover. Every skill
only reads: none can send a market message, book or change master data.

Each active skill's card also carries its record over the last 30 days, as
agentd counts it: how many runs it made, how often a person found its answer
„hilfreich", and what became of its proposals — **übernommen** (sent as
proposed), **angepasst** (changed before sending) and **verworfen**. A share
is shown only from five verdicts up; below that the card gives the count alone
(„2 Bewertungen"), and a skill nobody has rated yet says „Noch keine
Bewertungen", never 0 %. The record informs your judgement of a proposal; it
never decides one.

[![The Assistent page: one card per skill](assistent-light.png)](assistent-light.png)
[![The Assistent page: one card per skill](assistent-dark.png)](assistent-dark.png)

## Freigaben

Some answers the Prozessüberwachung does not send on its own: an Anmeldung of
an affiliated supplier held for a person, or a case its Entscheidungsbaum
could not settle. **Freigaben** lists them for the Rolle you wear, the
earliest Frist first; the sidebar shows how many wait.

Each opens with its reason and its Frist. A held answer says which Antwortcode
it will send; an open one asks you for the code, and the form offers only the
codes its Entscheidungsbaum defines, each with its published wording. The
process's Verlauf is beside it. Every decision sends a market message, so the
Desk asks once more before it goes, and it is recorded under your name.

<kbd>J</kbd> and <kbd>K</kbd> step through the list, <kbd>A</kbd> chooses
Freigeben and <kbd>R</kbd> Ablehnen; <kbd>↵</kbd> confirms the send. When a
colleague decided the same Freigabe first, the Desk says who and when, and
yours is not sent. Any other refusal is shown in the Prozessüberwachung's own
words; one its policy makes stands where the controls were until the tab is
closed.

The assistant has usually proposed a decision before you open one: once for
every Freigabe, as it is queued — an action, a code and one sentence why,
marked „AI-generated." and checked like every sentence it writes. It stands
at the top of the decision; <kbd>U</kbd> or „In die Entscheidung übernehmen"
fills the form and sends nothing; you check it and send it yourself. Where it
has not proposed one, ask it „Was schlägst du vor?". Whether you sent the proposal as it was, changed it
or dismissed it is kept beside the assistant's run, which is how its proposals
are measured — and every proposal card, on a Freigabe, a Rechnungsdisput or a
geparkte Buchung, ends with one line of that record: „In 30 Tagen: 14 von 17
Vorschlägen übernommen" (from five decided proposals; below, the count alone).

[![A Freigabe with the assistant's proposal taken into the form, unsent](freigaben-light.png)](freigaben-light.png)
[![A Freigabe with the assistant's proposal taken into the form, unsent](freigaben-dark.png)](freigaben-dark.png)

## Taking a decision back

A routine decision — „Hilfreich?" under an answer or a Hinweis, „Verwerfen"
on a proposal — shows at once and leaves the browser five seconds later. Until
then a toast at the foot of the window offers „Rückgängig", and
<kbd>⌘Z</kbd> takes back the newest; nothing was sent, so there is nothing to
reverse. Closing the tab sends whatever is still held. A decision that sends a
market message is never held: it asks first and goes at once.

## Überblick

A few numbers per role for whoever answers for it — each one as its service
returns it, never a sum the Desk works out, and each tile opens the list it
counts. A tile turns red only where the service marks a breach; a
number the service capped shows as a floor („500+").

**Fristen überschritten** lists every open process past its Antwortfrist, as
the Prozessüberwachung has it, each with the Festlegung its deadline came from.

**Dead Letters** (Netz) counts every message the Marktkommunikation could not
take or deliver and lists the most recent. The Lieferant also sees
**Rechnungsprüfung**, **Rechnungsdispute** and **Geparkte Buchungen**, the
Messstelle **Ableseaufträge überfällig** — each the same read as its row in
Heute, which a click opens.

Every Rolle sees **Assistent**: the share of the desk-assistant's answers
people found „hilfreich" over the last 30 days — the ratio of agentd's two
counts, and only from five verdicts; below, the count — with what became of
its proposals beneath. It opens the Beipackzettel.

[![The Überblick](ueberblick-light.png)](ueberblick-light.png)
[![The Überblick](ueberblick-dark.png)](ueberblick-dark.png)
[![Overdue processes](fristen-light.png)](fristen-light.png)
[![Overdue processes](fristen-dark.png)](fristen-dark.png)

## Heute

Every Rolle opens on **Heute**: everything that waits on you, in one list.
Each item is something a service holds open for this Rolle:

| Rolle | What Heute lists |
|---|---|
| Netz | Freigaben, overdue processes, Korrekturbedarf after a negative Prüfmitteilung |
| Lieferant | Anmeldungen waiting on the Netzbetreiber, Freigaben, overdue processes, disputed invoices, invoices held for review, bookings the ledger refused, Anmeldungen not yet sent |
| Messstelle | Freigaben, overdue processes, overdue reading orders |
| Energieservice | Freigaben, overdue processes |

The list is read in bands — **Überfällig**, **Heute**, **Nächste 7 Tage**,
**Später**, **Ohne Frist** — by the Frist each service returned; the Desk
computes none. A row names the customer where one is known, says in the
service's words why it waits, and opens what it is about.

A Freigabe is decided where it stands: <kbd>↵</kbd> opens the decision under
the row — <kbd>A</kbd> or <kbd>R</kbd>, the code, the confirm — and Heute
stays on screen. The assistant's proposal waits under the row;
<kbd>U</kbd> opens the decision with it taken in, unsent.

A disputed invoice is decided there too. Its row names the Rechnungssteller
as the Marktstammdaten register it, with the Rechnungsnummer beneath, and the
decision lists the checker's findings by their German names. **Beilegen**
closes the dispute with your Bemerkung and sends nothing, so it is held for
five seconds behind „Rückgängig"; while the refusal (REMADV) has
not gone out, **Ablehnung senden** sends it after the confirm. The assistant
proposes one of the two with the finding it rests on, „nicht belegt" when the
findings it read do not name it.

An invoice the Abrechnung's risk check put aside is decided in place as well:
a **held** one is released after the confirm, since releasing issues it; one
**under review** went out already, and **Geprüft** takes it off the list,
held for undo. The assistant proposes neither — the risk check's findings are
the evidence, listed in the decision.

A booking the Kundenkonten' ledger refused for good — a sealed month, an
account not yet open — is a **geparkte Buchung**: its row names the customer,
the amount and the refusal, the decision says what to do about it. **Neu
buchen** books it on the date you enter, filled in with the first open day
where the ledger names one; **Verwerfen** drops it with the Grund you give.
Both are final and recorded under your name, so each asks once more first; a
refusal is shown in the ledger's words.

The assistant proposes one of the two, with the read it rests on:
**Verwerfen** when the customer's account already carries the same Beleg and
amount — the Grund names that booking — otherwise **Neu buchen** on the first
open day. Opening the decision takes the proposal in, marked as the
assistant's; nothing is booked until you confirm. Where the reads do not settle
it, no proposal is shown.

A Korrekturbedarf opens nothing: the Desk has no Netzentgelte view.

[![A disputed invoice decided in Heute: its finding, the assistant's proposal resting on it, Beilegen and Ablehnung senden](heute-disput-light.png)](heute-disput-light.png)
[![A disputed invoice decided in Heute: its finding, the assistant's proposal resting on it, Beilegen and Ablehnung senden](heute-disput-dark.png)](heute-disput-dark.png)

The assistant works in the same list. Its Hinweis on a MaLo or process sits
under that item, „Hilfreich?" beside it while the item has focus; one no item
carries is an item of its own. <kbd>J</kbd> and
<kbd>K</kbd> or the arrows move, <kbd>↵</kbd> opens, and <kbd>⌘J</kbd> asks the
assistant about the item you are on.

**Warum?** — <kbd>?</kbd> on the selected row, or the small button that shows
on it — asks the assistant „Warum steht das auf meiner Liste, und was
entscheidet es?" about that item and answers under it, without leaving Heute:
at most five sentences, each with its source and its check, „AI-generated."
first and „Hilfreich?" beneath; <kbd>?</kbd> again puts it away. It is offered
wherever the assistant reads the record that put the item there — a Freigabe,
a process, an Anmeldung, a Rechnungsdisput, a geparkte Buchung, a customer, a
Hinweis. A Korrekturbedarf, an overdue reading order and an invoice under
review do not offer it: the assistant reads neither the Netzentgelte, the
reading orders nor the Abrechnung's risk check, so it would answer about
something else. While the assistant is unreachable, „Warum?" is not offered.

Under the list each source says how many it returned, or why it did not
answer — one unreachable service never empties Heute. As Lieferant the
**Lage** heads the page: the day in the services' own counts, no model in it.

[![Heute as Lieferant: the Lage, then the items in Frist bands, a Hinweis of the assistant under the item it concerns](heute-light.png)](heute-light.png)
[![Heute as Lieferant: the Lage, then the items in Frist bands, a Hinweis of the assistant under the item it concerns](heute-dark.png)](heute-dark.png)

## The Lieferant starts from the customer

**Kunden** finds a customer: type the start of any word of a name, a
Kundennummer or the MaLo-ID of a Lieferstelle, and the Kundenverwaltung answers from an
index as you type — at most 20 customers; nothing is searched in the browser.

[![Kunden: three customers matching „Muster“](kunden-light.png)](kunden-light.png)
[![Kunden: three customers matching „Muster“](kunden-dark.png)](kunden-dark.png)

A customer shows its Lieferstellen, newest first, with the state of its
supply — Anmeldung läuft, beliefert seit, beendet zum — and, where a process
runs, the Frist-Uhr of its earliest deadline, all from one read. A
customer with more than 200 shows the newest 200 and the total; ⌘K finds any
other by its MaLo-ID. Each opens the processes of its MaLo.
Payment data is not part of the view.

[![A customer with three Lieferstellen in different states](kunde-light.png)](kunde-light.png)
[![A customer with three Lieferstellen in different states](kunde-dark.png)](kunde-dark.png)

<kbd>⌘K</kbd> finds customers too:

[![The command palette listing customers for „Muster“](palette-kunden-light.png)](palette-kunden-light.png)
[![The command palette listing customers for „Muster“](palette-kunden-dark.png)](palette-kunden-dark.png)

A question typed into the palette goes to the assistant on the page you are
on — under a process, a customer or a Freigabe — and is answered there.

[![The command palette handing a question to the assistant](palette-fragen-light.png)](palette-fragen-light.png)
[![The command palette handing a question to the assistant](palette-fragen-dark.png)](palette-fragen-dark.png)

## One role at a time

Holding several market roles, a person wears one at a time — **Netz** (NB),
**Lieferant** (LF), **Messstelle** (MSB), **Energieservice** (ESA) — in the
toolbar or with <kbd>⌃1</kbd>–<kbd>⌃4</kbd>. Each has its own accent and start
page; switching shows no data of the role left. What a role may read is each
service's Cedar policy.

[![The role switcher in the toolbar](rolle-light.png)](rolle-light.png)
[![The role switcher in the toolbar](rolle-dark.png)](rolle-dark.png)

## You and your keys

Your name sits at the foot of the sidebar. Its menu shows the market roles you
hold and sets the appearance: System, Hell or Dunkel, kept in this browser.
<kbd>⌘/</kbd> lists every key the Desk answers to, from anywhere.

[![The account menu](konto-light.png)](konto-light.png)
[![The account menu](konto-dark.png)](konto-dark.png)
[![The keyboard shortcuts](tastatur-light.png)](tastatur-light.png)
[![The keyboard shortcuts](tastatur-dark.png)](tastatur-dark.png)

## What the Desk will not do

- **Compute a Frist, Werktag, amount or verdict** — every one on screen is the
  service's.
- **Round an amount or shift a date** — both stay exactly as the service sent
  them, in Berlin time.
- **Show data it cannot type** — a view reads only tools whose answer has a
  declared shape.
- **Decide who may act** — it offers every action and the service's policy
  answers.

## The design system

`/dev/gallery` (development builds) shows every control in every state, light
and dark side by side — the page a design change is reviewed on.

[![The design system gallery, light and dark side by side](gallery.png)](gallery.png)

## For developers

| Command | What it does |
|---|---|
| `just desk` | makod + the Desk with hot reload |
| `just desk-gen` | refresh `services/*/mcp.snapshot.json` and regenerate the Desk's types |
| `just desk-check` | generated types current, `tsc`, Biome, Vitest — part of `just ci` |
| `just desk-screenshots` | regenerate the screenshots on this page |

`cargo xtask check-desk` holds the first two rules, and each service's
`mcp.snapshot.json` the third. The screenshots are produced by
`desk/scripts/screenshots.ts`: Vite serves the
Desk, makod's answers are recorded responses in the schema makod declares, the
browser clock is fixed and the zone is Europe/Berlin, so every run draws the
same pictures. `desk/AGENTS.md` holds the rules for working on the Desk.
