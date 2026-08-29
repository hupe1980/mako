# mako-redispatch

Event-sourced process engine for **Redispatch 2.0** congestion-management
workflows under §§ 13, 13a, 14 EnWG. Part of the `mako` workspace.

## Format boundary

This crate is format-agnostic: workflows and the type router know only the
domain enum `RedispatchDocumentKind` — the same layering as
`mako-gpke`/`mako-wim`/`mako-mabis`, which never depend on `edi-energy`.
The **canonical** `DocumentType → RedispatchDocumentKind` mapping lives at
the transport boundary in `makod` (`redispatch_xml_ingest::document_kind`),
the only crate that depends on both halves. The mapping is exhaustive, so a
tenth XML document type fails compilation instead of silently never
routing; makod's `tests/redispatch_xml_pipeline.rs` proves the full wire
path (raw ActivationDocument XML → namespace-checked parse → kind → router
→ `redispatch-aktivierung`) and asserts a routing decision for all nine
document types (the AcknowledgementDocument is correlation-routed by
design).

## Regulatory scope

Redispatch 2.0 is mandatory for all German grid operators (ÜNB and VNB) and
their connected asset operators (ANB), effective 2021-10-01. Suppliers (LF) and
metering operators (MSB) are out of scope.

**BK6-23-241 (Beschluss 07.05.2026) consolidated it.** Its Anlage „Bilanzieller
Ausgleich von Redispatch-Maßnahmen (BilAReM)" is the basis this crate cites, and
it repeals the earlier decisions:

| Repealed | By | With effect from |
|---|---|---|
| BK6-20-059 Tenorziffer 1 | Tenorziffer 1 | end of 30.06.2026 |
| BK6-20-060 (Netzbetreiberkoordinierung) | Tenorziffer 4 | 07.05.2026 |
| BK6-20-061 (Informationsbereitstellung) | Tenorziffer 3 | 07.05.2026 |
| BK6-20-059 Tenorziffer 2 · Anlage zur BilAReM | Tenorziffer 8 | first day the new EDI@Energy documents apply |
| MaBiS Anlage 1 Kap. 17 | Tenorziffer 5 | end of 30.09.2026 |

**Market roles in scope:** ANB (Anschlussnetzbetreiber), BTR (Betreiber der
technischen Ressource), EIV (Einsatzverantwortlicher), VNB, ÜNB, LF, BKV.

## Three-crate architecture

| Crate | Responsibility |
|---|---|
| `edi-energy` | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
| `redispatch-xml` | XML/XSD format parsing (ActivationDocument, Stammdaten, …) |
| `mako-redispatch` | Process engine — workflows, routing, deadlines |

## Workflows

| Workflow name | Document type | Direction |
|---|---|---|
| `redispatch-aktivierung` | `ActivationDocument` (ACO/ACR/AAR) | ÜNB → VNB → ANB |
| `redispatch-stammdaten` | `Stammdaten` | ANB → VNB → ÜNB |
| `redispatch-planungsdaten` | `PlannedResourceScheduleDocument` | ÜNB → VNB → ANB |
| `redispatch-verfuegbarkeit` | `UnavailabilityMarketDocument` | ANB → VNB |
| `redispatch-netzengpass` | `NetworkConstraintDocument` | ÜNB ↔ VNB |
| `redispatch-kaskade` | `Kaskade` (§ 13 Abs. 2 EnWG) | ÜNB → VNB → ANB |
| `redispatch-statusanfrage` | `StatusRequest_MarketDocument` | bidirectional |
| `redispatch-kostenblatt` | `Kostenblatt` | VNB → ÜNB |

## Deadlines

Four Redispatch deadlines are widely quoted — a 6-hour acknowledgement, a
24-hour Statusanfrage answer, a 5-minute activation response, and a Kostenblatt
due on the 15th. **Three of them no longer have a published source, and the
fourth was never 6 hours.** `mako_redispatch::fristen` splits them accordingly.

### Sourced

| Obligation | Value | Source |
|---|---|---|
| `AcknowledgementDocument` | **3 minutes**, unverzüglich | AcknowledgementDocument FB 1.0g |
| Vorab-Information, Prognosemodell | 30 minutes before validity | BilAReM Kap. 6.3.1 |
| Ausfallarbeit final or Dissens established | end of the **3rd** following month, no restart after | BilAReM Kap. 6.4.3 |
| Wetterdaten of the Anlagenbetreiber | 4th Werktag of the following month | BilAReM Kap. 3.2.1 |
| Stammdaten `gueltig_ab` | ≥ 5 or ≥ 10 Werktage ahead, ≤ 2 years | Stammdaten AWT 1.4b Fn. 27/31/32/33 |
| Überführung ins Planwertmodell | ≥ 6 months' notice, only on 01.01./04./07./10. | BilAReM Kap. 2.3.2 |
| Zuordnung einer neuen SR | ≥ 5 WT before IBN if informed ≥ 10 WT ahead, else ≤ 5 WT after | BilAReM Kap. 2.3.2 |

> **The acknowledgement is three minutes, not six hours.** The
> AcknowledgementDocument Formatbeschreibung 1.0g states „unverzüglich, jedoch
> spätestens **3 Minuten** nach Erhalt der Übertragungsdatei". Six hours is a
> batch job; three minutes has to be answered by the ingest path.

The ACK carries four protocol rules worth stating separately (same source):
exactly one per Übertragungsdatei, confirming or rejecting the file as a whole;
a **missing** ACK means the message was not processed; an ACK is never itself
acknowledged; and a **late** ACK „darf nicht zu einer Fristverletzung des
eigentlichen Geschäftsvorfalles führen".

### Operator-configured

BK6-23-241 Tenorziffer 7 obliges the ÜNB to develop bundesweit einheitliche
Prozessbeschreibungen with the industry and submit them to the Beschlusskammer,
which then publishes them. Until that happens these windows are the operator's
own — `fristen::Betreiberfristen` holds them, with the historical BK6-20-05x
figure as a documented default:

| Obligation | Historical default | Was |
|---|---|---|
| Activation (ACO) response | 5 minutes | BK6-20-060 §6.3 |
| Kostenblatt submission | 15th of the following month | BK6-20-061 §7 |
| Stammdaten forward (VNB→ÜNB) | 1 Werktag | BK6-20-060 §3.2 |

> **`StatusRequest_MarketDocument` is not a request/response pair.** Its `type`
> codes are `A60` (status request for a position independently from a specific
> process) and `Z15` Erreichbarkeitsinformation, and its `status` carries `A03`
> Deactivated / `A04` Reactivated / `A13` Withdrawn — a communication-
> availability notification about a Marktpartner. There is no answer document
> and no 24-hour window.

> **Real-time scheduling:** whatever the configured ACR/AAR window, it stays a
> real-time constraint, and the 3-minute ACK is stricter still. The `makod`
> Redispatch deadline scheduler must poll well inside it — the standard
> Werktage-based scheduler used for GPKE/WiM is not sufficient.

## EDIFACT PIDs

The complete EDIFACT half of Redispatch 2.0, from the BDEW *Anwendungsübersicht
Prüfidentifikatoren 4.0* (01.04.2026) — every row whose Prozessbeschreibung is
„Kommunikationsprozesse Redispatch":

| PID | Nachricht | Inhalt | Von → An | EBD |
|----:|-----------|--------|----------|-----|
| 13021 | MSCONS | meteorologische Daten (Ex-post) | BTR → ANB · ANB → anfNB | — |
| 13022 | MSCONS | Einzelzeitreihe Ausfallarbeit | BTR ↔ NB · anfNB → ANB | — |
| 17209 | ORDERS | Anforderung der Ausfallarbeit | anfNB → ANB | — |
| 21037 | IFTSTA | Ansicht NB | NB → BTR | `E_0902` |
| 21038 | IFTSTA | Ansicht BTR | BTR → NB | `E_0900` |

There is **no ORDRSP in this family**: the ANB answers ORDERS 17209 with MSCONS
13022 (Prozessschritt 2).

### Seven neighbouring PIDs this crate does not own

| PID | Belongs to |
|----:|------------|
| 13020 | `mako-mabis` `mabis-billing` — Ausfallarbeitsüberführungszeitreihe |
| 13023 | `mako-mabis` `mabis-billing` — Lieferantenausfallarbeitssummenzeitreihe |
| 13026 | Geschäftsprozesse für EEG-Überführungszeitreihen |
| 17210 | `mako-mabis` `mabis-anforderung` — Anforderung LF-AACL |
| 17211 | `mako-mabis` `mabis-profile` — Reklamation Profile bzw. Profilscharen (`E_0100`) |
| 19204 | `mako-mabis` `mabis-anforderung` — Ablehnung Ab-/Bestellung der Aggregationsebene |
| 19301 / 19302 | Herkunftsnachweisregister (NB ↔ RB HKN-R), `S_0092` / `S_0093` |

13020 and 13023 are MaBiS Summenzeitreihen with a full Prüfmitteilung/
Datenstatus cycle, so routing them here would leave them no settlement stream to
live in and the obligation they carry nowhere to be recorded. 19301/19302 belong
to a different market exchange entirely.

## Routing

Unlike GPKE/WiM/GeLi Gas (EDIFACT `RFF+Z13` Prüfidentifikatoren), Redispatch
2.0 XML documents are routed by `RedispatchRouter` based on XML document type,
not EDIFACT PID. The `makod` AS4 ingest sniffs XML payloads (first
non-whitespace byte `<`), parses them with
`redispatch_xml::parse_and_validate`, and maps the document type to a
`RedispatchDocumentKind` before routing.

## BilAReM

`mako_redispatch::bilarem` and `mako_redispatch::ausfallarbeit` implement the
Anlage to BK6-23-241:

- **Two Bilanzierungsmodelle.** The NB-side Ausgleich happens „ausschließlich im
  Planwertmodell" (Kap. 2.1) via korrespondierende Fahrpläne against the NB's
  single Redispatch-Bilanzkreis. In the Prognosemodell the NB does not settle at
  all until 31.12.2031 (§14 Abs. 1 S. 3 EnWG); the BKV keeps the imbalance and
  receives Aufwendungsersatz.
- **Migration is one-way**, effective only on 01.01./01.04./01.07./01.10. with
  ≥ 6 months' notice. `Zuordnungsmitteilung::validate` refuses the rest.
- **The Ausfallarbeit window is a hard stop.** Kap. 6.4.3: by the end of the
  third following month the figure stands or the Dissens is formally
  established — „Danach dürfen die Prozesse … **nicht erneut gestartet**
  werden." `bilarem::abstimmung_zulaessig` enforces it.
- **Kap. 3 Ausfallarbeit** — `P_lim` per Aufforderungs-/Duldungsfall and
  Referenzprofil, Wind Spitz- and vereinfachte Spitzabrechnung, the Wind-Bin
  method for WEA auf See, Solar Spitz and Pauschal, the nicht-fluktuierende
  variants, the Kap.-3.4 Überbauungs-cap and the §24 Abs. 3 S. 2 EEG 2023
  MaLo→TR split.

---

## Engine module

`RedispatchModule` implements `EngineModule` and is registered in `makod` when
`DeploymentRoles` contains at least one of `Marktrolle::Nb`, `Marktrolle::Unb`,
or `Marktrolle::Anb`:

```rust,ignore
if roles.contains_any(&[Marktrolle::Nb, Marktrolle::Unb, Marktrolle::Anb]) {
    builder.register(Box::new(RedispatchModule));
}
```

`RedispatchModule::configure()` wires all 8 workflows into a `RedispatchRouter`
and registers IFTSTA PIDs 21037 / 21038 into the `PidRouter`.

### AcknowledgementDocument routing

`AcknowledgementDocument` is **not** registered in the type-based router.
Inbound ACKs carry a `ReceivingDocumentIdentification` field identifying the
workflow instance they belong to. The `makod` dispatcher resolves that
correlation key against the `ProcessRegistry` and delivers the ACK directly to
the originating workflow.

### Deadline scheduler note

The 5-minute Activation (ACO) deadline requires the `DeadlineScheduler` to poll
at ≤ 30-second intervals. Use a dedicated scheduler instance for Redispatch
workflows — the standard Werktage-based GPKE/WiM scheduler is insufficient.

---

## Related crates

| Crate | Role |
|---|---|
| `redispatch-xml` | XML format layer — parse · serialize · validate (joined with this crate in `makod`) |
| `mako-redispatch` ← **this crate** | Event-sourced process engine — 8 workflows, `RedispatchRouter`, `RedispatchModule` |
| `edi-energy` | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
| `mako-engine` | Event-sourced workflow runtime (`Workflow`, `Process`, `EventStore`) |
