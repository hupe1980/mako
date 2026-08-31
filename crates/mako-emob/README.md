# mako-emob

**NZR-EMob / Modell 2 — the virtual Bilanzierungsgebiet**

„Bring your own supplier" at a charge point, per BNetzA **BK6-20-160 Anlage 6**
(„NZR-EMob"), the BDEW Anwendungshilfe *Zum Modell 2* V1.3 and Beschluss
**BK6-24-267**. A Ladepunktbetreiber runs a regelzonenweites
Bilanzierungsgebiet of its own and books every charging session into the
Bilanzkreis of the supplier the customer chose — quarter hour by quarter hour,
across DSO borders, with intraday supplier changes.

This crate holds the **allocation engine, its invariants, and the three
Modellwechsel state machines**: pure domain, no I/O, no wire rendering. The
decision trees live in `mako_pruefung::emob` and the answer Fristen in
`mako_fristen::antwort::EMOB`, because that is where every other market process
keeps them.

## Five things this crate exists to get right

### 1. The Deltamenge is a term in the identity, not a rounding error

```text
NGZ(t, richtung) = Σ Zuordnungen + Deltamenge        exactly, every ¼ h
```

Anlage 6 §IV.1 obliges the LPB to assign the **whole** Bilanzierungsgebiet every
quarter hour. Whatever no Marktlokation claimed is the Deltamenge, and §IV.2
books it to a Bilanzkreis the LPB names, **at the LPB's own cost**. It settles
in money, so `QuarterHourAllocation` reports it as a field and returns a
`ConservationProof` beside every row rather than asserting it away.

The arithmetic is `metering::allocation::allocate` with each claim capped at
itself — reused rather than reimplemented, because that crate's
`Σ allocated + residual = total` identity *is* this one and its residual *is*
the Deltamenge. One call covers both directions of imbalance:

| Case | Result | Delta |
|---|---|---|
| claims **under** the NGZ | every claim met in full | `NGZ − Σ claims` |
| claims **over** the NGZ | every claim cut back in proportion | zero up to the six-decimal cut, and `Ueberdeckung` is recorded |

Over-claiming is real — local generation behind the Netzanschluss feeds the
charge points — and no published rule resolves it. Proportional cut-back is this
crate's stated default and is recorded on the result, because an operator whose
claims routinely exceed the NGZ has a metering problem, not a rounding one.

### 2. The Anmeldung answer window is seven Werktage

Only the **Abmeldung** (55242 → 55243) is three — it has no LF leg to wait for.
Three cannot work for the Anmeldung: the VNB may spend 3 WT sending the 55240
and the LF has 3 WT of its own, so `E_0510` Prüfschritt 1 — „Ging innerhalb der
Antwortfrist eine Ablehnung des Lieferanten ein?" — is undecidable before the
6. WT. The windows and a `const` assertion that pins the relation live in
`mako_fristen::antwort`.

### 3. Bezug and Einspeisung never net

`Richtung` is part of the allocation key and each direction carries its own
non-negative pool. Netting them inside a quarter hour would let a V2G discharge
cancel a neighbour's draw, and both would vanish from their suppliers'
Bilanzkreise. The Zeitreihentyp the BIKO expects for an Einspeisungs-BK-SZR eMob
is still open, so a deployment holds those rows — but the direction is modelled
regardless, because the alternative is silent netting.

### 4. Two identifiers are deliberately not the market's

`VirtualMaloId::new` **refuses an eleven-digit value**. AWH Kap. 1.6.1 lets the
LPB use its Stromnetzbetreibernummer for Zählpunktbildung und BG-Beantragung and
for nothing else; nothing grants it a MaLo-ID range for the per-vehicle objects
it needs internally. A virtual Marktlokation that looked like a BDEW MaLo-ID
would collide with a real one the day a Netzbetreiber issued it, and the
collision would surface as energy booked to a stranger's Bilanzkreis.

`TokenRef` carries an opaque keyed hash, never an RFID UID or an eMAID. Those
identify a natural person's charging contract across every operator they visit;
the allocation only needs to know which virtual MaLo a session belongs to, which
is a lookup the token registry already performed.

### 5. Silence is not consent

Neither Anlage 6 nor the AWH gives an unanswered leg a default outcome. The
GPKE Beendigung der Zuordnung has one — „Verstreicht die Frist …, gilt dies als
Bestätigung" — and Modell 2 has nothing of the kind, so an expired window puts
the process in `Eskaliert` and waits for an operator. Confirming would move a
Marktlokation between Bilanzierungsgebieten on no one's say-so.

## The three legs

One state machine, three workflow names, because all three run on the **same**
Marktlokation and a process resolves by (business key, workflow name):

| Workflow | Request → Answer | Tree | Frist | Answered by |
|---|---|---|---|---|
| `emob-anmeldung` | 55238 → 55239 | `E_0513` → `E_0510` | 7 WT | NB (VNB) |
| `emob-zuordnungsende` | 55240 → 55241 | `E_0511` | 3 WT | **LF** |
| `emob-abmeldung` | 55242 → 55243 | `E_0512` | 3 WT | NB (VNB) |

The 55240 leg runs *inside* the Anmeldung's own window, which is why that
window is seven Werktage. An answer carries both its code and the tree that
produced it into `SG4 STS+E01` DE 9013/1131: `A01` means opposite things in
`E_0510` and `E_0511`, so the pair is the smallest unit that means anything.

`mako_emob::EmobModule` registers the six Prüfidentifikatoren; `makod` routes,
adapts and renders them.

## The quarter-hour grid needs no DST special case

A `Viertelstunde` is an instant plus fifteen minutes of real time, and German
local time is offset from UTC by whole hours — so a UTC-aligned quarter hour is
aligned in Europe/Berlin too, and the 92- and 100-slot days are simply days with
fewer or more instants in them. Nothing in this crate counts „96".

## Provenance rides on every value

A charge point reporting **clock-aligned meter values** every 900 s (OCPP
`AlignedDataCtrlr`) measures each quarter hour. A **CDR** reports one total and
splitting it assumes constant power, which a tapering charge curve violates —
an estimate wearing the shape of a measurement, whose error lands on whichever
supplier held the slot boundary. `Provenance` is therefore on every allocated
value, and `SessionSplit` reports what the six-decimal cut could not place
instead of dropping it.

## Prüfidentifikatoren

| PID | Message | From → To | Answered by |
|---|---|---|---|
| 55238 | Anmeldung in Modell 2 | NB (LPB) → NB (VNB) | 55239, `E_0513` → `E_0514` → `E_0510` |
| 55240 | Beendigung der Zuordnung zur MaLo | NB (VNB) → LF | 55241, `E_0511` |
| 55242 | Abmeldung aus dem Modell 2 | NB (LPB) → NB (VNB) | 55243, `E_0512` |
| 55235 / 55236 | Zuordnung / Beendigung ZP der NGZ zur NZR | verantw. NB → benachb. NB, ÜNB | 55237, `E_0102` / `E_0103` (MaBiS) |
| 13018 | MSCONS Netzgangzeitreihe | NB (VNB) → NB (LPB), ÜNB | — |

`A01` is an **Ablehnung** in `E_0510` and a **Zustimmung** in `E_0511` and
`E_0512`. The three run in the same process and a combined VNB+LF deployment
walks all of them, so every lookup is keyed on `(ebd, code)`.

The market role on the wire is always **NB** — the BDEW Rollenmodell defines no
LPB („der LPB kommuniziert aus prozessualer Sicht wie die Rolle NB", AWH
Kap. 1.4). `mako_engine::marktrolle::Marktrolle::Lpb` is a *deployment* role, the
same pattern as `Nmsb`/`Amsb` and `Lfn`/`Lfa`.

## Sources

- **BK6-20-160** (21.12.2020) Anlage 6 „NZR-EMob"; Mitteilung Nr. 4 (03.05.2022)
- **BDEW AWH „Zum Modell 2 …" V1.3** (01.04.2025)
- **BDEW AWH Ergänzung der Marktregeln … (MaBiS)** V1.0 (27.04.2022)
- **BK6-24-267** (15.05.2025), bestandskräftig
- **UTILMD AHB Strom 2.2** Kap. 11; **EBD 4.3** Kap. 17
- **MaBiS** BK6-24-174 Anlage 3, Kap. 3.8 / 3.10 / 5

## License

MIT OR Apache-2.0
