+++
title = "Redispatch 2.0"
description = "Redispatch 2.0 in mako: XML document types, 8 event-sourced workflows, the BilAReM regime under BK6-23-241, which deadlines still have a published source, IFTSTA EDIFACT integration, and RedispatchModule deployment."
weight = 16
[extra]
mermaid = true
+++
# Redispatch 2.0

Redispatch 2.0 is the mandatory German grid-congestion management protocol
under §§ 13, 13a, 14 EnWG, effective 1 October 2021 (NABEG). It requires all
TSOs (ÜNB) and DSOs (VNB) to coordinate controllable generation units across
transmission and distribution networks using structured XML documents.

Unlike GPKE / WiM / GeLi Gas, which use EDIFACT `RFF+Z13` Prüfidentifikatoren
for routing, Redispatch 2.0 uses **CIM/IEC 62325-based XML** for primary data
exchange and **IFTSTA (EDIFACT)** only for final status confirmations.

---

## Regulatory basis

| BNetzA decision | Topic | Effective |
|---|---|---|
| BK6-20-059 | Datenformate und Übermittlungswege | 2021-10-01 — **TZ 1 repealed** with the end of 30.06.2026; TZ 2 survives until the new EDI@Energy documents apply |
| BK6-20-060 | Netzbetreiberkoordinierung (Stammdaten forwarding, Activation response) | 2021-10-01 — **repealed** by BK6-23-241 TZ 4 |
| BK6-20-061 | Informationsbereitstellung (`Kostenblatt`) | 2021-10-01 — **repealed** by BK6-23-241 TZ 3 |
| **BK6-23-241** | **BilAReM** — Bilanzieller Ausgleich von Redispatch-Maßnahmen: Planwertmodell (NB-side Ausgleich via korrespondierende Fahrpläne against a dedicated Redispatch-Bilanzkreis) alongside the Prognosemodell (BKV keeps the imbalance, §14 Abs. 1 S. 3/1b EnWG, until 31.12.2031); one-way SR migration at quarter boundaries with ≥6-month notice (soll-target 01.01.2031); Pauschal-Abrechnung grandfathering ends 31.12.2028; MaBiS Anlage 1 Kap. 17 revoked 30.09.2026 (survivors continue as Anlage zur BilAReM) | 2026-07-01 (ÜNB); formats follow the EDI@Energy expert group on relative deadlines |

NABEG 2019 and the above BNetzA decisions implement the legal obligation.
Absence of a conformant implementation is a regulatory violation under § 14 EnWG.

The BilAReM domain layer spans three seams:

- `mako_redispatch::bilarem` — `Bilanzierungsmodell`, `Abrechnungsverfahren`
  admissibility, quarter-boundary + 6-month-notice `Zuordnungsmitteilung`
  validation, all key dates as constants.
- `mako_redispatch::ausfallarbeit` — the full Kap.-3 Ausfallarbeit engine per
  the final Anlage (Beschluss 07.05.2026): `P_lim` determination
  (Aufforderungs-/Duldungsfall, Referenzprofil/beidseitige Fixierung), Wind
  Spitz-/vereinfachte Spitzabrechnung (`KF = P_VZ,ist/P_VZ,theo`, Nennleistung
  cap), the Wind-Bin-Verfahren for WEA auf See (`KF_Bin = KF_LBin × KF_V`,
  0,5-m/s bins per DIN EN 61400-12-1, `m ≥ 3`, Ersatzwert chain
  Vormonat → Folgemonat → 12-Monats-Mittel → 1, `KF_V ∈ ]0;1[`), Solar
  Spitz (irradiation-scaled, `P_WR` bound) and Pauschal (Anlagenfaktor table,
  UTC+1), the grandfathered Wind-/Solar-Pauschal-Fortschreibung,
  nicht-fluktuierende Spitz-/Pauschal-Abrechnung, the Kap.-3.4 Überbauungs-cap
  (`Σ W_A ≤ P_anschl × ¼ h − Einspeisung`, pro-rata Kürzung by installed
  capacity with clamp-and-redistribute), and the § 24 Abs. 3 S. 2 EEG 2023
  MaLo→TR split.
- `grid_billing::bilarem_finanzielle_korrektur`
  (`Korr_fin = (W_A − W_Ausgl)/1000 × ID-AEP` — the financial-only residual
  settlement for fluctuating plants in the Planwertmodell).

`netzbilanzd` exposes the engine as stateless, schema-validated compute
endpoints:

| Endpoint | Computes |
|---|---|
| `POST /api/v1/redispatch/ausfallarbeit/compute` | Per-TR `W_A` series + sum, for every Abrechnungsvariante |
| `POST /api/v1/redispatch/ausfallarbeit/ueberbauung` | Kap.-3.4 cap across the TR of one Netzlokation |
| `POST /api/v1/redispatch/ausfallarbeit/kf-bin` | Kap.-3.2.3.2 `KF_Bin` for one 0,5-m/s bin — feed the result back as `kf` on a `wind_spitz` request |
| `POST /api/v1/redispatch/ausfallarbeit/malo-split` | § 24 Abs. 3 S. 2 EEG 2023 — splits one marktlokationsscharfer Wert onto the TR behind the MaLo, pro rata by installed capacity |

An underoccupied bin is not an error on the `kf-bin` route: Kap. 3.2.3.2
prescribes a binding Ersatzwert order, and the response names which step
supplied the value (`monat` / `vormonat` / `folgemonat` /
`zwoelf_monats_mittel` / `standard`) so the operator can evidence it. A `KF_V`
outside `]0;1[` *is* rejected — that is a data error, not a value to clamp.

The caller supplies the quarter-hour input series — SCADA/edmd/DWD sourcing stays
operator-side. BDEW has not published the EDI@Energy wire formats for this
exchange; the Festlegung sets go-live at ≤ 6 months after their publication.

---

## Market roles in scope

| Abbrev. | Role |
|---|---|
| **ÜNB** | Übertragungsnetzbetreiber — Transmission System Operator (TSO) |
| **VNB** | Verteilnetzbetreiber — Distribution System Operator (DSO) |
| **ANB** | Anlagenbetreiber — generation / storage asset operator |
| **DV** | Direktvermarkter — direct marketer |
| **BKV** | Bilanzkreisverantwortlicher — balance responsible party |

Suppliers (LF) and metering-point operators (MSB) are **not** in scope for
Redispatch 2.0. Register `RedispatchModule` only when `DeploymentRoles`
contains at least one of `Marktrolle::Nb`, `Marktrolle::Unb`, or
`Marktrolle::Anb`.

---

## Three-crate architecture

```mermaid
graph LR
    subgraph "Transport boundary"
        AS4["AS4/ebMS3<br/>(SOAP/MTOM)<br/>XML sniff: first byte &lt;"]
    end

    subgraph "redispatch-xml"
        PARSE["parse_and_validate(bytes)<br/>→ Document enum"]
    end

    subgraph "edi-energy"
        IFTSTA["parse IFTSTA<br/>PID 21037 / 21038"]
    end

    subgraph "mako-redispatch"
        ROUTER["RedispatchRouter<br/>(XML document-type routing)"]
        PIDR["PidRouter<br/>(IFTSTA 21037/21038)"]
        WF1["redispatch-stammdaten"]
        WF2["redispatch-aktivierung"]
        WF3["redispatch-verfuegbarkeit"]
        WF4["redispatch-netzengpass"]
        WF5["redispatch-kaskade"]
        WF6["redispatch-planungsdaten"]
        WF7["redispatch-statusanfrage"]
        WF8["redispatch-kostenblatt"]
        ROUTER --> WF1 & WF2 & WF3 & WF4 & WF5 & WF6 & WF7 & WF8
        PIDR --> WF2
    end

    AS4 --> PARSE
    PARSE --> ROUTER
    AS4 --> IFTSTA --> PIDR
```

---

## XML document types

All nine document types are CIM/IEC 62325-based XML — **not** EDIFACT. The
`redispatch-xml` crate handles all parsing, serialization, and validation.

| Document type | XSD version | Sender → Receiver | Handled by workflow |
|---|---|---|---|
| `ActivationDocument` | 1.1f | ÜNB → VNB → ANB | `redispatch-aktivierung` |
| `PlannedResourceScheduleDocument` | 1.0f | ÜNB → VNB → ANB | `redispatch-planungsdaten` |
| `AcknowledgementDocument` | 1.0g | any → sender of referenced doc | correlation routing (ProcessRegistry) |
| `Stammdaten` (master data) | 1.4b | ANB → VNB → ÜNB | `redispatch-stammdaten` |
| `StatusRequest_MarketDocument` | 1.1 | bidirectional | `redispatch-statusanfrage` |
| `Unavailability_MarketDocument` | 1.1b | ANB → VNB | `redispatch-verfuegbarkeit` |
| `Kaskade` | 1.0 | ÜNB → VNB → ANB | `redispatch-kaskade` |
| `NetworkConstraintDocument` | 1.1b | ÜNB ↔ VNB | `redispatch-netzengpass` |
| `Kostenblatt` | 1.0d | VNB → ÜNB | `redispatch-kostenblatt` |

XSD schemas and application guidelines are published by BDEW at
[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25 — XML-Datenformate Redispatch 2.0).

### AcknowledgementDocument routing

`AcknowledgementDocument` is **not** registered in the document-type router.
Every ACK carries a `ReceivingDocumentIdentification` field that identifies
the workflow instance it belongs to. The `makod` dispatcher resolves that
correlation key against the `ProcessRegistry` and delivers the ACK directly to
the originating workflow without routing by type.

---

## IFTSTA EDIFACT integration

Status messages are the only EDIFACT component of Redispatch 2.0. The
`edi-energy` crate handles IFTSTA parsing; `mako-redispatch` registers the
two PIDs in the `PidRouter`:

| PID | Nachricht | Inhalt | Von → An | EBD |
|----:|-----------|--------|----------|-----|
| 13021 | MSCONS | meteorologische Daten (Ex-post) | BTR → ANB · ANB → anfNB | — |
| 13022 | MSCONS | Einzelzeitreihe Ausfallarbeit | BTR ↔ NB · anfNB → ANB | — |
| 17209 | ORDERS | Anforderung der Ausfallarbeit | anfNB → ANB | — |
| **21037** | IFTSTA | Ansicht NB | NB → BTR | `E_0902` |
| **21038** | IFTSTA | Ansicht BTR | BTR → NB | `E_0900` |

`E_0902` and `E_0901` are executable in `mako-pruefung` (`role-mabis`); `E_0900`
is the **Betreiber's** tree and is deliberately not, because mako does not play
that role.

Two properties of `E_0902` that a plain accept/reject would lose:

- It is published once but applies **to both** the Ausfallarbeitszeitreihe and
  the Fahrplananteilzeitreihe, and BDEW states the two runs can reach different
  results — so it is decided per series, not once per message.
- Its two Ablehnungen state the same reason and differ in what the NB owes
  next: `A02` carries a **Gegenvorschlag** (the NB states its own figures),
  `A03` a **Korrekturanforderung** (the Betreiber must resend). Both require a
  written Erläuterung.

`E_0901` („Gegenvorschlag prüfen") then bounds the counter-proposal leg:
exactly **one** Gegenvorschlag is admissible per Ausfallarbeitszeitreihe
(`A03`), and none at all once the series has been confirmed (`A01`).

That is the complete EDIFACT half — every row in the BDEW *Anwendungsübersicht
Prüfidentifikatoren 4.0* whose Prozessbeschreibung is „Kommunikationsprozesse
Redispatch". There is **no ORDRSP in this family**: the ANB answers ORDERS 17209
with MSCONS 13022 (Prozessschritt 2). All of them route to the
`redispatch-aktivierung` workflow via conversation-ID lookup.

### Seven PIDs that look like Redispatch and are not

Their subject is the Ausfallarbeit, which is why they get filed here, but the
PID overview puts them under a different Prozessbeschreibung:

| PID | Belongs to |
|----:|------------|
| 13020 | `mako-mabis` `mabis-billing` — Ausfallarbeitsüberführungszeitreihe |
| 13023 | `mako-mabis` `mabis-billing` — Lieferantenausfallarbeitssummenzeitreihe |
| 13026 | Geschäftsprozesse für EEG-Überführungszeitreihen |
| 17210 | `mako-mabis` `mabis-anforderung` — Anforderung LF-AACL |
| 17211 | `mako-mabis` `mabis-profile` — Reklamation Profile bzw. Profilscharen (`E_0100`) |
| 19204 | `mako-mabis` `mabis-anforderung` — Ablehnung Ab-/Bestellung der Aggregationsebene |
| 19301 / 19302 | Herkunftsnachweisregister (NB ↔ RB HKN-R), `S_0092` / `S_0093` |

13020 and 13023 are MaBiS Summenzeitreihen carrying a full Prüfmitteilung/
Datenstatus cycle (IFTSTA 21000, 21002–21005); routing them to an activation
workflow gave them no settlement stream to live in, so the obligation they carry
had nowhere to be recorded.

---

## Deadlines

Four Redispatch deadlines are widely quoted — a 6-hour acknowledgement, a
24-hour Statusanfrage answer, a 5-minute activation response, and a Kostenblatt
due on the 15th. **Three of them no longer have a published source, and the
fourth was never 6 hours.**

BK6-23-241 repealed the decisions they came from (Tenorziffern 1, 3 and 4), and
what replaces them is not a new table of Fristen: Tenorziffer 7 obliges the ÜNB
to *develop* bundesweit einheitliche Prozessbeschreibungen with the industry and
submit them to the Beschlusskammer, which then publishes them. Until that
happens the concrete windows are a matter of the operator's own
Prozessbeschreibung. `mako_redispatch::fristen` splits the two cases, because
citing a repealed paragraph for a hard-coded number reads as authority and stops
anyone re-checking it.

### Sourced

| Obligation | Value | Clock | Source |
|---|---|---|---|
| `AcknowledgementDocument` | **3 minutes**, unverzüglich | UTC | AcknowledgementDocument FB 1.0g |
| Vorab-Information, Prognosemodell | 30 minutes before validity | UTC | BilAReM Kap. 6.3.1 |
| Ausfallarbeit final or Dissens established | end of the **3rd** following month | German local time | BilAReM Kap. 6.4.3 |
| Wetterdaten of the Anlagenbetreiber | 4th Werktag of the following month | German local time | BilAReM Kap. 3.2.1 |
| `Stammdaten` `gueltig_ab` | ≥ 5 or ≥ 10 Werktage ahead, ≤ 2 years | German local time | Stammdaten AWT 1.4b Fn. 27/31/32/33 |
| Überführung ins Planwertmodell | ≥ 6 months' notice, only on 01.01./04./07./10. | German local time | BilAReM Kap. 2.3.2 |

> **The acknowledgement is three minutes, not six hours.** The
> AcknowledgementDocument Formatbeschreibung 1.0g, section „Fristen zur
> Übermittlung der AcknowledgementDocument-Nachricht":
>
> > „Der Empfänger der Übertragungsdatei teilt dem Absender **unverzüglich,
> > jedoch spätestens 3 Minuten** nach Erhalt der Übertragungsdatei das Ergebnis
> > seiner syntaktischen Prüfung mittels der AcknowledgementDocument-Nachricht
> > mit."
>
> Six hours and three minutes are not the same obligation with a different
> number: six hours is something a batch job satisfies, three minutes has to be
> answered by the receiving process itself.

Four protocol rules ride with it (same source), and each one is a branch an
implementation either has or silently does not:

1. **Exactly one ACK per Übertragungsdatei**, confirming or rejecting the file
   as a whole. A positive ACK is `A01`; a negative one is `A02` plus `Z12`
   (XSD did not validate) or one of `Z13`–`Z18` (valid but not processable).
2. **No ACK means not processed** — „Eine nicht empfangene
   AcknowledgementDocument-Nachricht bedeutet, dass die Ursprungsnachricht beim
   Empfänger **nicht bearbeitet** wird." A sender that reads silence as success
   loses the message.
3. **Never acknowledge an acknowledgement.**
4. **A late ACK does not breach the business Frist** — „Syntaxfehlermeldungen,
   welche außerhalb der Frist beim Absender … eingehen, dürfen nicht zu einer
   Fristverletzung des eigentlichen Geschäftsvorfalles führen." The transport
   clock and the process clock are separate.

### Operator-configured

`fristen::Betreiberfristen` holds these, with the historical BK6-20-05x figure
as a documented default. They are **not** binding — the decisions that set them
are repealed:

| Obligation | Historical default | Was |
|---|---|---|
| Activation (ACO) response | 5 minutes | BK6-20-060 §6.3 |
| `Kostenblatt` submission | 15th of the following month | BK6-20-061 §7 |
| `Stammdaten` forward (VNB→ÜNB) | 1 Werktag | BK6-20-060 §3.2 |

BilAReM Kap. 6.2.1.1 keeps the Stammdaten *obligation* — the responsible
Marktpartner sends a changed value „unverzüglich nach Bekanntwerden" — but
attaches no countable window.

### `StatusRequest_MarketDocument` is not a request/response pair

Its `type` codes are `A60` (status request for a position independently from a
specific process) and `Z15` Erreichbarkeitsinformation, and its `status` carries
`A03` Deactivated / `A04` Reactivated / `A13` Withdrawn — a
communication-availability notification about a Marktpartner
(StatusRequest_MarketDocument FB 1.1). There is no answer document and no
24-hour window; the acknowledgement is the only thing owed back.

### Real-time scheduling

Whatever ACR/AAR window the operator configures, it stays a real-time
constraint, and the 3-minute ACK is stricter still. `makod` must be configured
with a dedicated `DeadlineScheduler` instance for Redispatch workflows; the
standard Werktage-based GPKE/WiM scheduler (which typically polls every few
minutes) is **not** sufficient and must not be shared with it.

```
GPKE/WiM deadline scheduler   →  polls every few minutes  (Werktage arithmetic)
Redispatch deadline scheduler →  polls every 30 s         (UTC, 3-minute ACK window)
```

---

## Aufforderungsfall vs Duldungsfall

The central Redispatch 2.0 case split (BilAReM Kap. 1) is a behavioural branch
in the Aktivierung workflow, not just master data. In the **Aufforderungsfall**
the anweisende Netzbetreiber asks the EIV to change the Wirkleistung and the EIV
steers; in the **Duldungsfall** the anweisende Netzbetreiber steers the SR
itself. Kap. 3.1 hangs the Ausfallarbeit directly off the distinction:
`P_lim = min{P_ist; P_min}` / `max{P_ist; P_max}` in the Aufforderungsfall and
`P_lim = P_ist` in the Duldungsfall.

| | Aufforderungsfall | Duldungsfall |
|---|---|---|
| Who steers | EIV/BTR per transmitted schedule (`AbrufartAufforderungsfall`: Z01 Delta / Z02 Sollwert) | The NB directly via the technical Steuerkanal (marktd `nelo.steuerkanal`) |
| ACR/AAR response window | **Enforced** — the process expires when no ACR/AAR arrives | **Not applicable** — no counterparty response is awaited; a mistakenly scheduled window is ignored |
| §13a settlement basis | Transmitted schedule | Measured vs. reference Lastgang |

`AktivierungCommand::ReceiveAco` carries the case (`Abwicklung`), resolved by
the transport layer from the resource's Stammdaten.

## §13a EnWG compensation

`grid_billing::redispatch_verguetung` computes the angemessene Vergütung
(§13a Abs. 2 EnWG): entgangene Einnahmen (for EEG/KWKG plants via
`eeg_entgangene_einnahmen` from the anzulegender Wert — Nr. 5; for others the
proven lost revenue — Nr. 3) plus zusätzliche Aufwendungen (Nr. 1/2/4) minus
ersparte Aufwendungen (Satz 4 — reimbursed to the NB; the net may be
negative). netzbilanzd exposes it as
`POST /api/v1/redispatch/verguetung/{activation_id}/compute`. Calculation
endpoint only — the payment run is the operator's ERP.

**The case selects the counterfactual, and the request must say which.**
`abwicklung` is required, and it picks the `AusfallarbeitBasis`:

| `abwicklung` | Ausfallarbeit from | Why |
|---|---|---|
| `DULDUNGSFALL` | the edmd 15-min Lastgang window (`P_lim = P_ist`, BilAReM Kap. 3.1) | the NB steered the resource, so what the plant would have produced was never transmitted |
| `AUFFORDERUNGSFALL` | `ausfallarbeit_kwh_override`, taken from the transmitted schedule — **required**, `422` without it | the EIV steered to that schedule, and the schedule *is* the counterfactual |

Resolving both from the Lastgang would settle an Aufforderungsfall against what
happened rather than against what was instructed — a money error in whichever
direction the plant deviated, and one nothing downstream detects. The chosen
basis travels into the result and its calculation trace, so an audit can see
which counterfactual a figure rests on.

## Workflow overview

The `mako-redispatch` crate provides 8 fully implemented workflows, all backed
by the same `mako-engine` `Workflow` + `Process` infrastructure.

| Workflow | Document type | Direction | Key deadline |
|---|---|---|---|
| `redispatch-stammdaten` | `Stammdaten` | ANB → VNB → ÜNB | 3-min ACK; forwarding window operator-configured |
| `redispatch-aktivierung` | `ActivationDocument` + IFTSTA | ÜNB → VNB → ANB | 3-min ACK; ACR/AAR window operator-configured |
| `redispatch-verfuegbarkeit` | `UnavailabilityMarketDocument` | ANB → VNB | 3-min ACK |
| `redispatch-netzengpass` | `NetworkConstraintDocument` | ÜNB ↔ VNB | 3-min ACK |
| `redispatch-kaskade` | `Kaskade` (§ 13 Abs. 2 EnWG) | ÜNB → VNB → ANB | 3-min ACK |
| `redispatch-planungsdaten` | `PlannedResourceScheduleDocument` | ÜNB → VNB → ANB | 3-min ACK |
| `redispatch-statusanfrage` | `StatusRequest_MarketDocument` | notification | 3-min ACK — no answer document |
| `redispatch-kostenblatt` | `Kostenblatt` | VNB → ÜNB | 3-min ACK; submission day operator-configured |

Each workflow uses a dedicated event-type newtype (e.g., `VerfuegbarkeitEvent`,
`NetzengpassEvent`) to prevent cross-workflow event-type collisions in the
shared `EventStore`.

---

## RedispatchModule

`RedispatchModule` implements `mako_engine::builder::EngineModule` and is the
single registration point for all Redispatch 2.0 handling in `makod`.

```rust,no_run
use mako_redispatch::RedispatchModule;
use mako_engine::builder::EngineBuilder;

// Register conditionally — only for NB/ÜNB/ANB deployments:
if roles.contains_any(&[Marktrolle::Nb, Marktrolle::Unb, Marktrolle::Anb]) {
    builder.register(Box::new(RedispatchModule));
}
```

`RedispatchModule::configure()` wires:
1. All 8 workflows into a `RedispatchRouter` (XML document-type routing)
2. IFTSTA PIDs 21037 and 21038 into the `PidRouter` (EDIFACT routing)

---

## Quick start — parsing a Redispatch XML document

```rust
use redispatch_xml::{parse_and_validate, Document};

// Recommended: parse + validate in one step
let doc = parse_and_validate(&xml_bytes)?;

// Access common fields on any document type
println!("mRID: {}", doc.mrid());
println!("Sender: {}", doc.sender_id());
println!("Receiver: {}", doc.receiver_id());

// Pattern match on the variant to access type-specific fields
match &doc {
    Document::Activation(a) => {
        println!("Activation period: {}", a.time_interval);
    }
    Document::Stammdaten(s) => {
        println!("Asset count: {}", s.controllable_units.len());
    }
    _ => {}
}
```

### Validation details

```rust
use redispatch_xml::{parse, validate};

let doc = parse(&xml_bytes)?;
let result = validate(&doc);

if result.is_valid() {
    // Zero errors — proceed with processing
} else {
    // All errors, not just the first:
    let all_errors = result.into_errors().unwrap_err();
    for e in &all_errors {
        eprintln!("Validation error: {}", e);
    }
}
```

---

## Integration with `makod`

Both transport legs are wired end-to-end:

1. **EDIFACT leg.** IFTSTA 21037/21038 Vollzugsmeldungen
   and the MSCONS (13020–13023, 13026) / ORDERS (17209–17211) / ORDRSP
   (19204, 19301, 19302) Ausfallarbeit family resolve via the `PidRouter` to
   `redispatch-aktivierung` and are executed on the activation process by the
   ingest dispatcher — spawned when none exists yet, so no Redispatch market
   message is silently dropped. Correlation key: MaLo where the message
   carries one, else the BGM document reference.
2. **XML leg.** The AS4 ingest sniffs XML payloads (first non-whitespace
   byte `<` — EDIFACT interchanges start with `UNA`/`UNB`) and hands them to
   `redispatch_xml_ingest::dispatch_redispatch_xml`: `redispatch-xml`
   parses, namespace-checks and validates the document, the canonical
   `document_kind` mapping (exhaustive — enum drift fails compilation)
   picks the workflow, and the dispatcher spawns/resumes the process with
   the regulatory deadlines registered **atomically with the first events**:
   - `ActivationDocument` → `ReceiveAco` with the ACR/AAR response window
     and the **3-minute ACK window**. The Abwicklung defaults to
     Aufforderungsfall/Sollwert — the strict case; resolving a Duldungsfall
     from the resource's Stammdaten relaxes it, never the reverse.
   - `Stammdaten` → 3-minute ACK window + forward window.
   - The six ack-forward document types → their 3-minute ACK windows.
   - `AcknowledgementDocument` is delivered by **correlation**
     (`ReceivingDocumentIdentification` → the process registered under the
     acknowledged document's MRID), never type-routed.

   A parse/validation failure or unroutable document is rejected without an
   AS4 receipt (the receipt would assert successful reception), so the
   sender corrects and retransmits. Deadlines fire through
   `deadline_dispatch` (all 8 workflows covered). The
   `redispatch_xml_pipeline` integration test in makod proves parse → kind
   → route for all nine document types.

### Startup coverage check

`deadline_dispatch::assert_dispatch_coverage` panics at startup when a
registered Redispatch workflow lacks a deadline-dispatch entry — a deadline
that can be scheduled but never fired would otherwise fail silently.
`RedispatchModule` itself is registered for NB/ÜNB deployments (default
feature set or `role-nb-strom`).

---

## Key invariants

- `Workflow::handle` and `Workflow::apply` are **pure functions**: no I/O, no
  clock access, no global state mutation.
- Events and `AcknowledgementDocument` outbox entries are always written in a
  **single `WriteBatch`** via `AtomicAppend::append_with_outbox`. Separate
  writes are not permitted — a crash between them produces a lost ACK with no
  recovery path (regulatory violation).
- The ACK and Activation windows use **UTC nanosecond precision**; do not
  convert to local time before comparing. Only the BilAReM Werktag obligations
  (Wetterdaten, Stammdaten-Gültigkeit, Planwert-Überführung) run on German local
  time.
- **A late `AcknowledgementDocument` never fails the Geschäftsvorfall it
  carried** (AcknowledgementDocument FB 1.0g). The transport clock and the
  process clock are separate, and conflating them turns a slow counterparty
  into a regulatory breach that did not happen.

---

## See also

- [`redispatch-xml` crate](https://crates.io/crates/redispatch-xml) — XML format layer
- [Process Engine Guide](@/docs/architecture/engine.md) — `Workflow`, `Process`, `EventStore`
- [PID Reference — Redispatch section](@/docs/regulatory/pid-reference.md#redispatch-2-0-xml-document-types-not-edifact-pids)
- [BNetzA Regulatory Reference](@/docs/regulatory/bnetza.md) — BK6-23-241 (BilAReM) and the BK6-20-05x decisions it replaced
