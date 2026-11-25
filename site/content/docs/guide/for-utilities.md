+++
title = "For utilities"
description = "mako for a Stadtwerk: which jobs it covers, which Marktrollen it plays, how to evaluate it without risk, and what running it takes."
weight = 1
+++
A Stadtwerk plays several Marktrollen at once — Netzbetreiber, Lieferant,
Messstellenbetreiber — and each of them owes its counterparties regulated
messages inside a Frist. This page is for the person deciding whether mako can
carry that work: what it covers, how to try it on your own data without
sending anything, and what a deployment needs.

> [!WARNING]
> mako is in development ahead of its first official platform release. Interfaces can change. Test with your own counterparties before production use.

## What mako covers

| Job | In mako | Starts at |
|---|---|---|
| Market-communication gateway — AS4 transport, EDIFACT parsing and building, MIG/AHB checks, CONTRL and APERAK | `makod` | [makod](@/docs/services/makod.md) |
| The processes themselves — GPKE, WiM, GeLi Gas, MaBiS, answered from the published Entscheidungsbäume inside their Frist | `makod` · `processd` | [processd](@/docs/services/processd.md) |
| Master data — MaLo/MeLo, Versorgungsstatus, partner and grid registries | `marktd` | [marktd](@/docs/services/marktd.md) |
| Metering — MSCONS, Zählerstandsgang, substitute values, smart-meter gateway lifecycle | `edmd` | [edmd](@/docs/services/edmd.md) |
| Grid settlement — NNE, Mehr-/Mindermengen, MaBiS Summenzeitreihen; EEG/KWKG feed-in | `netzbilanzd` · `einsd` | [netzbilanzd](@/docs/services/netzbilanzd.md) |
| Retail — contracts, tariffs, billing, EN 16931 e-invoices, ledger, dunning, documents, customer portal | `vertragd` · `productd` · `billingd` · `invoicd` · `accountingd` · `outputd` · `portald` | [billingd](@/docs/services/billingd.md) |
| The operator's work surface — every Frist in Berlin time, every Freigabe in one inbox | the Desk | [The Desk](@/docs/guide/desk/index.md) |

Each service owns its own data and talks to the others over typed REST and
CloudEvents, so a deployment can take the gateway and process layer first and
keep its existing billing, or the reverse. The
[services index](@/docs/services/_index.md) lists every service with its role.

## Which Marktrollen

`makod` registers the processes of the roles its `[deployment] rollen`
declares, and nothing else:

| Rolle | `rollen` tokens |
|---|---|
| Netzbetreiber (NB) | `nb-strom` · `nb-gas` |
| Lieferant (LF) | `lf-strom` · `lf-gas` |
| Messstellenbetreiber (MSB) | `msb-strom` · `msb-gas` |
| Energieserviceanbieter (ESA) | `esa-strom` |
| Bilanzkreisverantwortlicher (BKV) | `bkv-strom` |
| Einsatzverantwortlicher (EIV, Redispatch 2.0) | `eiv-strom` |

Each role is played under exactly one MP-ID per Sparte — „Für jede Sparte (Gas
bzw. Strom) und jede Rolle hat ein Markteilnehmer genau eine MP-ID"
(Identifikatoren AWH V1.2 Kap. 2.1). Which person may read which role's
processes is the deployment's Cedar policy. The table of modules each token
registers is in [makod — Deployment roles](@/docs/services/makod.md#deployment-roles).

## Evaluate it without risk

Three steps, none of which sends a message to a market partner:

1. **Run the demo.** `just demo` starts every service and the Desk in Docker
   with one Stadtwerk seeded — an Anmeldung answered with 55002, one waiting
   for a Freigabe, a Klärfall, an EEG plant settled to the cent, a household
   invoiced, issued and paid — and `mako-seed --verify` asserts each outcome:
   the Antwortcode, the amount, the receivable to the cent. Sign in as any
   persona and see Cedar answer each one differently.
   [Getting started](@/docs/guide/getting-started.md) walks it.
2. **Check your configuration.** `just setup` runs every daemon's
   `--check-config` and folds the findings into one checklist — identities,
   certificates, Bilanzkreise, access, reachability, format coverage — each
   with its setting path and source. The setup interview proposes the
   configuration that turns it green. See [Einrichtung](@/docs/guide/einrichtung.md).
3. **Rehearse a message.** The Desk's Probelauf builds an Anmeldung,
   Abmeldung or Kündigung through the same door a real one takes, holds it
   against its profile and the pre-send check, and sends and stores nothing.
   See [The Desk — Rehearse a message](@/docs/guide/desk/index.md#rehearse-a-message-before-go-live).

`just erstlauf` runs the second and third steps end to end from an empty
`makod` and times each.

**Rehearse a lost delivery acknowledgement.** Run `just recovery-drill` to restore the LF’s invoice and open credit. Check which evidence releases the queued financial event after the recipient has already accepted it. Each refusal names the evidence to recover before retrying. [Cold restore guide](@/docs/reference/recovery.md)

## Preparing a Schattenbetrieb

Before comparing answers, establish which traffic reached the mirror. Export a closed source journal for one Marktrolle and time window. Independently inventory the captured files. Run `makotest shadow reconcile` locally. It checks each occurrence and its bytes, reports duplicates and gaps, and distinguishes a confirmed quiet interval from missing evidence. You can try it first with the synthetic capture included in the checkout. [Capture format and worked example](@/docs/reference/makotest.md#reconcile-a-shadow-capture)

Rehearse one Strom NB refusal with the synthetic capture and compare it with an independent expected answer. The example assesses one of two inbound Anmeldungen. [Run the rehearsal](@/docs/reference/makotest.md#rehearse-a-strom-nb-refusal)

Add `--makod-workspace <dir>` to exercise the daemon. Keep the mirror and the private rehearsal store separate. After a restart, retrieve the same captured reply. The sending installation retains responsibility for outstanding acknowledgements. Before handing over sending authority, reconcile messages and postings added since the snapshot. [Daemon rehearsal](@/docs/services/makod.md#shadow-capture)

Open **System → Schattenbetrieb** in the NB Desk of the isolated review installation. Every captured inbound occurrence has its own row. In the synthetic window one of two answers is assessed. Select the other row to see which evidence is missing. A named resolver can acknowledge a discrepancy with an archive reference and reason. Changed evidence requires a fresh decision. [Configure the review installation](@/docs/services/processd.md#isolated-shadow-review)

## What running it takes

- **Containers.** Every service is an image on GHCR, multi-arch, tagged with
  its version; pin one in production
  ([makod — Docker Deployment](@/docs/services/makod.md#docker-deployment)).
- **PostgreSQL.** One schema per service. `makod` keeps its process state in its
  own store on a volume instead.
- **Identity and access.** An OIDC identity provider for every route and the
  Desk, and Cedar policies deciding who may read and do what.
- **AS4 key material.** TLS, signing and encryption certificates from an
  SM-PKI CA with your MP-ID in the subject, for every role you play
  ([makod — AS4](@/docs/services/makod.md#as4-as4-ebms3-inbound-and-outbound)).
- **Published MP-IDs.** Use a BDEW-Codenummer or GLN for Strom and a DVGW-Codenummer or GLN for Gas, published in the respective register. [BDEW, Allgemeine Festlegungen 6.1d, Kap. 2.13–2.14, S. 17–18](https://www.bdew-mako.de/api/downloadFile/12145)
- **Your own tests.** Against your own counterparties, before the first live
  message.
