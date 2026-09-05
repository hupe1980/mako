# mako-invoic

The INVOIC settle/dispute state machine of German market communication —
written once, and shared by every billing family.

## Why one machine

Every billing process in MaKo is the same conversation. An invoice is issued;
the recipient validates it against the AHB and either settles or disputes it;
where this deployment is the *issuer*, a REMADV comes back confirming or
refusing payment, and a COMDIS may refuse that REMADV in turn.

Nothing in it is commodity-specific. The process is keyed on the invoice
reference, and the Sparte only decides which price sheet `invoic-checker`
fetches — `invoicd`'s decision, not the workflow's. So GPKE, WiM, GaBi Gas and
GeLi Gas register a family here instead of implementing the process, and there
is one arm per rule rather than four readings of it.

## What a family chooses

`InvoicFamily` is the whole of the variation:

| Item | Meaning |
|---|---|
| `WORKFLOW_NAME` | the name registered in the process engine and stored on every stream |
| `DEADLINE_LABEL` | the settlement response window's label |
| `INVOIC_PIDS` | the Prüfidentifikatoren (PIDs — the five-digit BDEW codes naming an Anwendungsfall) this family accepts, inbound and outbound |
| `SENDS_INVOIC` | whether this deployment plays the **issuer** role |
| `ANSWERS_COMDIS` | whether the family exchanges COMDIS 29001 |

The two capabilities are refusals, not decoration. A family that never issues an
invoice refuses `SendInvoic` and `ReceiveRemadv` rather than opening a state it
cannot honestly reach — accepting an inbound REMADV there inverts the direction
of the conversation, because after *receiving* an invoice this platform is the
one that sends the REMADV.

The four families that ship:

| Family | Crate | PIDs | Issuer | COMDIS |
|---|---|---|---|---|
| `GpkeAbrechnung` | `mako-gpke` | 31001, 31002, 31005, 31006 | ✅ | ✅ |
| `WimInvoic` | `mako-wim` | 31009, 31003, 31004 | ✅ | ✅ |
| `GaBiGasInvoic` | `mako-gabi-gas` | 31010, 31007, 31008 | — | ✅ |
| `GeliGasSperrprozesseInvoic` | `mako-geli-gas` | 31011 | ✅ | — |

## The process

```text
── Recipient (payer) ────────────────────────────────────────────────
New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed
                                       ╰─[invalid]──► Rejected
ValidationPassed ──SettleInvoice──► Settled     ⇢ REMADV 33001 Zahlungsavis
                 ╰─DisputeInvoice──► Disputed   ⇢ REMADV 33002 / 33003 / 33004

── Issuer ───────────────────────────────────────────────────────────
New ──SendInvoic──► InvoicSent ──ReceiveRemadv 33001──► PaymentConfirmed
                               ╰─ReceiveRemadv 33002/3/4──► PaymentDisputed

── Payer, after its REMADV was refused ──────────────────────────────
any settled/sent state ──ReceiveComdis 29001──► ComdisRejected

Any non-terminal state ──TimeoutExpired──► Rejected
```

A deadline that fires *after* the answer was given is absorbed: deadlines are
never cancelled, so they fire on the healthy path too.

## What an inbound invoice carries that BO4E does not model

`InvoicData` keeps two EDIFACT facts beside the BO4E `Rechnung`, because
`Rechnung` models the document and neither belongs to it:

| Field | Segment | Why it is kept |
|---|---|---|
| `bestellung_ref` | `SG1 RFF+ACE` | The **order this invoice answers** — the ORDERS Dokumentennummer on `IMD++KON`/`TEC`, the QUOTES on `MSB` (INVOIC AHB 1.0b segment 00020, hints `[501]`/`[508]`). `E_0264` Prüfschritt 40 („Basiert die Rechnung auf einer Bestellung?") is what compares it against the orders on record. |
| `rechnungstyp` | `IMD+7081` | The **Use-Case**. PID 31009 carries three — `KON` „Abrechnung von Konfigurationen (Universalbestellprozess)" is the ESA billing of WiM Teil 2 Kap. 4.5, `MSB` the Messstellenbetrieb toward NB or LF, `TEC` the Änderung der Technik — and they answer under different trees on different windows. |

Both ride the `ProcessInitiated` payload so `invoicd` reads them without going
back to the EDIFACT archive, exactly as the `Rechnung` does.

## Answering means two messages, not one

Settling or disputing emits **both** a `ProcessCompleted` — this operator's own
ERP notification — and the **REMADV** the invoice issuer is waiting on. Only the
second is visible to the market, and it is the one with a Frist attached: WiM
Teil 2 Kap. 4.5.2 Nr. 2 gives an ESA until the 4. Werktag before the
Zahlungsziel, Teil 1 Kap. 6.2 the same for an NB, and Kap. 3.6.3.8.2 gives the
LF until the Zahlungsziel itself.

The dispute carries a [`RemadvAntwort`]: `SG7 AJT` is **Muss** on every
Nicht-Zahlungsavis, DE 4465 the code and DE 1082 the Entscheidungsbaum that
publishes it. The tree is not a constant — PID 31009 alone carries three
Use-Cases with three different quartets — so the caller resolves it from
`mako_pruefung::codes::rechnungspruefung` and the workflow does not guess one.

The answer's **shape** picks the Prüfidentifikator, because REMADV AHB 1.0a
admits a different list of trees on each: § 3.1.1's 33002 for a tree that states
one code, § 3.1.2's 33003 („Abweisung Kopf und Summe") / 33004 („Abweisung
Position") for one that states a set.

## Registering a family

```rust
use mako_engine::types::Pruefidentifikator;
use mako_invoic::{InvoicFamily, InvoicWorkflow};

pub const WORKFLOW_NAME: &str = "gpke-abrechnung";
pub const ABRECHNUNG_WINDOW_LABEL: &str = "invoic-settlement-deadline";
pub const GPKE_INVOIC_PIDS: &[u32] = &[31001, 31002, 31005, 31006];

pub struct GpkeAbrechnung;

impl InvoicFamily for GpkeAbrechnung {
    const WORKFLOW_NAME: &'static str = WORKFLOW_NAME;
    const DEADLINE_LABEL: &'static str = ABRECHNUNG_WINDOW_LABEL;
    const INVOIC_PIDS: &'static [u32] = GPKE_INVOIC_PIDS;
    const SENDS_INVOIC: bool = true;
    const ANSWERS_COMDIS: bool = true;
}

pub type GpkeAbrechnungWorkflow = InvoicWorkflow<GpkeAbrechnung>;
```

## What downstream hears

A validated invoice emits `ProcessInitiated` naming the family, the PID, the
invoice reference and the BO4E `Rechnung`, so `invoicd` can run
`InvoicCheckEngine::check` straight off the webhook payload without going back
to the EDIFACT archive. A settled or disputed one emits `ProcessCompleted` with
the outcome and, for a dispute, its reason.

## Regulatory basis

- **INVOIC AHB 1.0** (FV2025-10-01 onwards; AHB 2.8e before) — the invoice
  message and its Prüfidentifikatoren.
- **REMADV AHB 1.0a § 3** — the payment advice. Settlement is „ganz oder gar
  nicht": there are no Teilzahlungen, so 33002/33003/33004 are all Abweisungen
  and only 33001 confirms.
- **COMDIS AHB 1.0** — the invoicer's refusal of a payer's REMADV (29001).
- **APERAK AHB 1.0 § 2.4.1** — the technical acknowledgement, 45 Minuten on a
  weekday. A different clock from the business answer this workflow runs.

## Tests

`tests/state_machine.rs` covers what the process *does*; what each family
chooses is tested in that family's own crate.

## Related crates

| Crate | Role |
|---|---|
| [`mako-invoic`](https://docs.rs/mako-invoic) ← **this crate** | The shared settle/dispute state machine and the `InvoicFamily` trait |
| [`mako-engine`](https://docs.rs/mako-engine) | Event-sourced workflow runtime — `Workflow`, `Process`, `EventStore`, deadlines |
| [`mako-gpke`](https://docs.rs/mako-gpke) | `GpkeAbrechnung` — Netznutzungsabrechnung Strom |
| [`mako-wim`](https://docs.rs/mako-wim) | `WimInvoic` — Messstellenbetrieb and ESA billing |
| [`mako-gabi-gas`](https://docs.rs/mako-gabi-gas) | `GaBiGasInvoic` — Kapazitäts- and MMM-Rechnung Gas |
| [`mako-geli-gas`](https://docs.rs/mako-geli-gas) | `GeliGasSperrprozesseInvoic` — Sperrprozesse Gas |
| [`invoic-checker`](https://docs.rs/invoic-checker) | The plausibility and tariff checks run on a received invoice |
| [`invoicd`](https://hupe1980.github.io/mako/docs/services/invoicd/) | Production daemon — runs the checks and files the § 147 AO receipt |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>

## License

MIT OR Apache-2.0
