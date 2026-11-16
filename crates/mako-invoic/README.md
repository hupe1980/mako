# mako-invoic

[![Crates.io](https://img.shields.io/crates/v/mako-invoic?label=mako-invoic&color=f59e0b&logo=rust)](https://crates.io/crates/mako-invoic)

The INVOIC settle/dispute state machine of German market communication, written
once and shared by every billing family. An invoice is issued; the recipient
settles or disputes it with a REMADV; the issuer may refuse that REMADV with a
COMDIS. GPKE, WiM, GaBi Gas and GeLi Gas each register a family here instead of
implementing the process; the checks an inbound invoice runs are
[`invoic-checker`](https://crates.io/crates/invoic-checker)'s.

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
    const SENDS_INVOIC: bool = true;   // this deployment issues invoices
    const ANSWERS_COMDIS: bool = true; // the family exchanges COMDIS 29001
}

pub type GpkeAbrechnungWorkflow = InvoicWorkflow<GpkeAbrechnung>;
```

## What it covers

| Family | Crate | PIDs | Issuer | COMDIS |
|---|---|---|---|---|
| `GpkeAbrechnung` | `mako-gpke` | 31001, 31002, 31005, 31006 | ✅ | ✅ |
| `WimInvoic` | `mako-wim` | 31003, 31004, 31009 | ✅ | ✅ |
| `GaBiGasInvoic` | `mako-gabi-gas` | 31007, 31008, 31010 | — | ✅ |
| `GeliGasSperrprozesseInvoic` | `mako-geli-gas` | 31011 | ✅ | — |

```text
── Recipient (payer) ────────────────────────────────────────────────
New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed
                                       ╰─[invalid]──► Rejected
ValidationPassed ──SettleInvoice──► Settled     ⇢ REMADV 33001 Zahlungsavis
                 ╰─DisputeInvoice──► Disputed   ⇢ REMADV 33002 / 33003 / 33004

── Issuer ───────────────────────────────────────────────────────────
New ──SendInvoic──► InvoicSent   ⇢ INVOIC (the message SendInvoic carries)
InvoicSent ──ReceiveRemadv 33001──► PaymentConfirmed
           ╰─ReceiveRemadv 33002/3/4──► PaymentDisputed

── Payer, after its REMADV was refused ──────────────────────────────
any settled/sent state ──ReceiveComdis 29001──► ComdisRejected

InvoicReceived / ValidationPassed ──settlement window lapses──► FristAbgelaufen
```

- Settlement is all or nothing: only 33001 confirms, and 33002/33003/33004 are
  Abweisungen. A dispute carries a `RemadvAntwort` (`SG7 AJT`: code +
  Entscheidungsbaum) whose shape picks the PID.
- Deadlines are never cancelled: a window firing after the answer is absorbed;
  one firing on an unanswered invoice records `FristAbgelaufen`, and the invoice
  still accepts its REMADV.
- `SendInvoic` carries the INVOIC the transport boundary built and validated;
  the workflow builds nothing and enqueues it in the write that records
  `InvoicSent`.
- `InvoicData` keeps `bestellung_ref` (`SG1 RFF+ACE`) and `rechnungstyp`
  (`IMD+7081`) beside the BO4E `Rechnung`.

## Sources

- INVOIC AHB 1.0
- REMADV AHB 1.0a Kap. 3
- COMDIS AHB 1.0

Operator reference: [invoicd](https://hupe1980.github.io/mako/docs/services/invoicd/) ·
API: <https://docs.rs/mako-invoic>

License: MIT OR Apache-2.0
