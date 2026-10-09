# mako-invoic

[![Crates.io](https://img.shields.io/crates/v/mako-invoic?label=mako-invoic&color=f59e0b&logo=rust)](https://crates.io/crates/mako-invoic)

The INVOIC settle/dispute state machine of German market communication, written
once and shared by every billing family. An invoice is issued; the recipient
settles or disputes it with a REMADV; the issuer may refuse that REMADV with a
COMDIS. GPKE, WiM, GaBi Gas and GeLi Gas each register a family here instead of
implementing the process.

## Example

A family is a handful of constants; the workflow is generic over it:

```rust
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

## Key types

| Item | Meaning |
|---|---|
| `WORKFLOW_NAME` | the name registered in the process engine and stored on every stream |
| `DEADLINE_LABEL` | the settlement response window's label |
| `INVOIC_PIDS` | the Prüfidentifikatoren (PIDs — the five-digit BDEW codes naming an Anwendungsfall) this family accepts, inbound and outbound |
| `SENDS_INVOIC` | whether this deployment plays the **issuer** role |
| `ANSWERS_COMDIS` | whether the family exchanges COMDIS 29001 |

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

InvoicReceived / ValidationPassed ──settlement window lapses──► FristAbgelaufen
```

Deadlines are never cancelled, so a window that fires after the answer is
absorbed; one that fires on an unanswered invoice records `FristAbgelaufen` and
the invoice still accepts its REMADV. A dispute carries a `RemadvAntwort`
(`SG7 AJT`: code + Entscheidungsbaum), and its shape picks 33002, 33003 or 33004
(REMADV AHB 1.0a § 3). `InvoicData` keeps `bestellung_ref` (`SG1 RFF+ACE`) and
`rechnungstyp` (`IMD+7081`) beside the BO4E `Rechnung`.

API documentation: <https://docs.rs/mako-invoic>. The checks an inbound invoice
runs are [`invoic-checker`](../invoic-checker/)'s; the service is
[`invoicd`](https://hupe1980.github.io/mako/docs/services/invoicd/).
