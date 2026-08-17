+++
title = "makotest (Python)"
description = "Python test & simulation toolkit for MaKo platforms: BDEW identifier check digits, Werktag arithmetic and deadline instants, AHB-validated EDIFACT, seeded EPEX curves, and a pytest plugin — over the same Rust core the platform runs."
weight = 18
+++

# makotest — Python test & simulation toolkit

`makotest` generates regulator-conformant inputs, simulates the counterparties a
MaKo platform talks to, and asserts on the result — from Python.

It is **not mako-specific**. Everything it drives is a public wire contract
(EDIFACT over AS4, REST, CloudEvents), so it can exercise any MaKo
implementation.

```python
from makotest import malo_from_base, deadline_at_werktage, validate_edifact

malo_from_base("5123869678")    # '51238696012' — BDEW check digit applied
deadline_at_werktage("2026-12-30T09:00:00Z", 1)
# '2027-01-04T17:00:00+01:00' — one Werktag, five calendar days

report = validate_edifact(utilmd_bytes, "2026-10-01")
report.is_valid                 # MIG + AHB + semantic rules, on that format version
```

---

## The binding boundary

The toolkit is a PyO3 extension over the same Rust crates the platform runs.
Wire-format concerns are **never** reimplemented in Python: a second
implementation would drift from the BDEW profiles at the first Formatumstellung,
and a harness that disagrees with production about validity is worse than none.

The rule: **anything a regulator defines in a table is Rust; anything shaped by
test ergonomics is Python.**

| Concern | Home |
|---|---|
| EDIFACT MIG/AHB/semantic validation | Rust — `edi-energy` |
| MaLo/MeLo check digits and formats | Rust — `rubo4e::identifiers` |
| Werktag calendar and deadline instants | Rust — `mako-engine::fristen` |
| WiM per-PID Antwortfristen | Rust — `mako-wim` |
| EPEX curves, load profiles | Python |
| Counterparty behaviour, fixtures | Python |

Because validation runs the platform's own AHB engine, `makotest` proves
*process and integration* behaviour. It is **not** an independent check of
format conformance — the BDEW reference examples remain the authority there.

---

## Install

```bash
pip install makotest                  # core: identifiers, Fristen, EDIFACT
pip install 'makotest[data]'          # + EPEX / Lastgang generators
pip install 'makotest[bo4e]'          # + BO4E business objects
pip install 'makotest[hypothesis]'    # + property-based strategies
```

Wheels are **abi3** (`abi3-py311`) — one wheel serves Python 3.11 and later.

---

## Identifiers

A random 11-digit string is almost never a valid Marktlokations-ID, so a test
that invents one silently exercises the rejection path instead of the happy
path. Generate them instead:

```python
from makotest import malo_from_base, malo_check_digit, malo_is_valid, melo_is_valid

malo_from_base("5123869678")     # '51238696012'
malo_check_digit("5123869678")   # 0
malo_is_valid("51238696012")     # True
malo_is_valid("51238696781")     # False — wrong check digit

melo_is_valid("DE00014559929E00856996N5139699L01")   # True (33 chars)
melo_is_valid("51238696012")                         # False — that is a MaLo
```

---

## Fristen and deadlines

Regulated processes are deadline-driven, so time is an input, never ambient.
The calendar is BDEW's **conservative-inclusive** one: a day observed as a
holiday in *any* German state is a non-Werktag, so no Frist is ever computed
shorter than the AHB requires for some participant.

Two different questions live here, and mixing them up is a recurring source of
wrong deadlines.

### Which date — calendar arithmetic

```python
from makotest import add_werktage, is_werktag, next_werktag

is_werktag("2026-01-06")       # False — Heilige Drei Könige (BY, BW, ST only)
add_werktage("2026-12-24", 2)  # '2026-12-29' — 25/26 holidays, 27/28 weekend
next_werktag("2026-11-07")     # '2026-11-09' — Saturday rolls to Monday
```

### Which moment — the deadline the platform registers

A Werktage Frist expires at **17:00 Europe/Berlin** on the due Werktag. That
instant, not the date, is the obligation.

```python
from makotest import deadline_at_werktage

deadline_at_werktage("2026-03-02T09:00:00Z", 3)   # '2026-03-05T17:00:00+01:00'
deadline_at_werktage("2026-07-13T09:00:00Z", 1)   # '…+02:00' — CEST
```

The offset follows the CET/CEST transition; rendering the result in UTC hides
the hour that makes it correct. And no calendar-day approximation is sound —
**one** Werktag from Wednesday 30.12.2026 expires Monday 04.01.2027, five
calendar days later, because 31.12. and 01.01. are non-Werktage and 02./03.01.
is a weekend.

| Function | Window | Basis |
|---|---|---|
| `deadline_at_werktage(received, n)` | *n* Werktage → 17:00 Berlin | BDEW MaKo calendar |
| `add_hours(received, h)` | wall-clock hours (GPKE 24 h) | runs through weekends |
| `contrl_due_at(received)` | 6 hours | CONTRL |
| `aperak_strom_due_at(received)` | 45 minutes on a weekday | APERAK AHB §2.4.1 |
| `aperak_gas_folgeprozess_due_at(received)` | next Werktag 12:00 | GeLi Gas |
| `aperak_gas_initialprozess_due_at(received)` | 3 Werktage | GeLi Gas |
| `wim_antwort_frist_werktage(pid)` | 3 / 5 / 7 / 1 WT, per PID | BK6-22-024 WiM Teil 1 |

The **APERAK acknowledgement and the business answer are separate clocks** —
45 minutes versus days for Strom. Conflating them is the classic WiM error.

### WiM MSB-Wechsel is per process

```python
from makotest import assert_deadline_is, wim_antwort_frist_werktage

wim_antwort_frist_werktage(55039)   # 3 — Kündigung             (Kap. 2.2.2 Nr. 2)
wim_antwort_frist_werktage(55042)   # 5 — Beginn                (Kap. 2.3.2 Nr. 2)
wim_antwort_frist_werktage(55051)   # 7 — Ende                  (Kap. 2.4.2 Nr. 2)
wim_antwort_frist_werktage(55168)   # 1 — Verpflichtungsanfrage (Kap. 2.5.2 Nr. 4)

assert_deadline_is(
    response["deadline"],
    received="2026-03-02T09:00:00Z",
    werktage=wim_antwort_frist_werktage(55051),
)
```

`assert_deadline_is` computes the expectation with the platform's own
arithmetic, so the assertion measures the same instant the platform registered
rather than a re-derivation. On failure it prints both instants and the rule.

---

## EDIFACT validation

```python
from makotest.assertions import assert_edifact_valid, assert_rule_fires

assert_edifact_valid(mscons_bytes, on="2025-10-01")
assert_rule_fires(bad_bytes, "SEM-MSCONS-LOCATION-FORMAT", on="2025-10-01")
```

Pass the date the message would really be sent. A message valid under
FV2025-10-01 can be invalid under FV2026-10-01, and defaulting to "today"
silently hides that.

`assert_rule_fires` is the negative-case counterpart: it proves the rule you
think catches a defect is the one that actually caught it, rather than some
unrelated error further up the stack.

---

## Building EDIFACT

`makotest` builds messages through the same Rust builders the platform uses, so
a message it constructs is one the platform would accept.

```python
from makotest import UtilmdTransaction, build_utilmd, build_interchange

msg = build_utilmd(
    pruefidentifikator=55001,
    sender="4012345000023",
    receiver="9900357000004",
    release="S2.1",
    document_date="20251101",
    transactions=[
        UtilmdTransaction(
            object_type="melo",
            object_id="DE00014559929E00856996N5139699L01",
            process_dates=[("163", "20251101")],   # delivery start
            references=[("Z13", "55001")],
        )
    ],
)
```

`build_utilmd` and `build_mscons` return a **message** (`UNH`…`UNT`). That is not
sendable on its own — the wire unit a market partner receives is an
**interchange**:

```python
wire = build_interchange(
    sender="4012345000023",
    receiver="9900357000004",
    dar="REF001",
    messages=[msg],
    date="260802", time="0915",
)
# UNB+UNOC:3+4012345000023:14+9900357000004:500+260802:0915+REF001'...UNZ+1+REF001'
```

Building and validation are deliberately **separate steps**: a test must be able
to construct a knowingly-invalid message and assert that the right rule rejects
it. Pass the result to `assert_edifact_valid` when you want the happy path.

---

## pytest plugin

The plugin registers through the standard `pytest11` entry point — no
`conftest.py` wiring.

```python
def test_frist_is_met(frozen_clock, epex_sim):
    frozen_clock.advance_werktage(4)   # BDEW calendar, not naive +4 days
    assert frozen_clock.date == "2026-11-09"
```

| | |
|---|---|
| **Fixtures** | `epex_sim`, `nb_sim`, `biko_sim`, `imsys_sim`, `frozen_clock`, `makotest_seed`, `mako_endpoint` |
| **Markers** | `@pytest.mark.regulatory("GPKE Teil 2")`, `@pytest.mark.requires_docker` |
| **Options** | `--mako-endpoint URL`, `--makotest-seed N` |

Only `makotest.plugin` imports pytest. The generators and simulators are plain
objects usable from a script or notebook, so a demo and a CI test drive the same
code path.

---

## Counterparty simulators

Each simulator models what a counterparty *does* — including what it does not
do. Silence is the mode worth having: a platform that never sees it is never
tested against its own Fristen, and that is where regulated processes fail.

```python
def test_nb_bestaetigt(nb_sim):
    nb_sim.on(55001).bestaetigung(zuordnungsbeginn="2026-11-01")
    assert nb_sim.receive(interchange)["pid"] == 55002

def test_frist_faellt(nb_sim):
    nb_sim.on(55001).timeout()          # no answer, not even a CONTRL
    assert nb_sim.receive(interchange) is None
```

An unconfigured partner is a **silent** one. Forgetting to bind an answer
exercises the deadline path rather than quietly passing on a response the test
never asked for.

### The answer PIDs are not guessed

`MarktpartnerSim` resolves its answer from the AHB table in `edi-energy` — the
same table `mako-gpke` and `mako-geli-gas` derive their outbound response PID
from, pinned by conformance tests on both sides. A simulator that computed
`Anfrage + 1` would be wrong twice over:

| Anfrage | Bestätigung | Ablehnung | Why |
|---|---|---|---|
| 55001 | 55002 | 55003 | the regular pattern |
| 55077 | 55078 | **55080** | 55079 is unassigned |
| 44020 | 44021 | **none** | confirmable, never rejectable |
| 44019 | none | none | neither answer exists |

`.antwort(pid=...)` bypasses the table when the *point* of the test is an
adversarial answer — a counterparty replying with the wrong PID is a thing that
happens, and a platform should reject it.

### BIKO and iMSys

`BikoSim` receives Abrechnungssummenzeitreihen and answers with an acceptance or
a Klärfall. Rejections are **queued rather than sticky**, so the re-submission
after Clearing can be asserted — the accept-only path proves little on its own.

`ImsysSim` models the SMGW compliance surface a platform has to react to: TAF
profile, CLS channel state, certificate expiry and revocation, and
Zählerstandsgang gaps that force Ersatzwertbildung. It does not reimplement BSI
TR-03109 crypto — that would be a second implementation of something the gateway
owns, and getting it subtly wrong would make tests disagree with reality exactly
where they must not.

makotest ships no MaStR or UBA simulator: neither integration exists in mako, and
a simulator without a consumer encodes guesses about an interface nobody
implements.

---

## Property-based testing

```python
from hypothesis import given
from makotest.strategies import malo_ids, pruefidentifikatoren

@given(malo=malo_ids(), pid=pruefidentifikatoren(message_type="UTILMD"))
def test_every_utilmd_roundtrips(malo, pid):
    ...
```

Needs the `hypothesis` extra. Every strategy draws through the Rust core, which
is the point: a random 11-digit string is almost never a valid MaLo, so a
hand-rolled strategy exercises the rejection path and proves nothing.

`pruefidentifikatoren()` yields only PIDs the compiled profiles carry AHB rules
for. A PID without rules validates **vacuously** — `is_valid` comes back true
having checked nothing — so generating one produces a test that cannot fail.

| Strategy | Draws |
|---|---|
| `malo_ids()` | check-digit-valid 11-digit Marktlokations-IDs |
| `melo_ids(country=...)` | 33-character Messlokations-IDs |
| `marktpartner_ids(kind=...)` | BDEW (`99…`), DVGW (`98…`) or GLN codes — the prefix decides the UNB qualifier |
| `bilanzierungsgebiete()` | 16-character EIC codes (check character not computed) |
| `pruefidentifikatoren(message_type=…, sparte=…)` | PIDs with real AHB rules |
| `werktage()` | dates that are Werktage under the BDEW calendar |
| `zeitreihen(periods=…)` | kWh series, one value per MTU |

`message_types_of(pid)` returns a **list**: a Prüfidentifikator does not
identify one message type — APERAK and COMDIS both declare 29001 and 29002.
It resolves against the compiled profiles rather than a PID-band table.

---

## EPEX curves

Deterministic day-ahead curves in the MTU-keyed shape a platform ingests, with
**negative prices supported** — load-bearing for §51 EEG Vergütungsausfall and
§41a EnWG dynamic-tariff caps.

```python
from makotest.generators import EpexSim

sim = EpexSim(seed=42)
list(sim.day("2026-11-01", profile="winter_peak"))
list(sim.day("2026-06-21", profile="solar_glut", negative_hours=6))
```

Profiles: `flat`, `winter_peak`, `solar_glut`, `volatile`. Same seed reproduces
the curve exactly, and day order does not affect output.

Curves are synthetic test fixtures — never present them as market data.

---

## BO4E generation

`makotest` builds business objects from BO4E **202607**, the release mako's
`rubo4e` generates from. Assert it once per session; testing v202607 objects
against a platform on another generation produces passes that mean nothing.

```python
from makotest.assertions import assert_bo4e_generation_matches
assert_bo4e_generation_matches(platform.bo4e_version)
```

---

## Development

`makotest` is a member of the mako Cargo workspace and builds with
[maturin](https://www.maturin.rs/):

```bash
just test-makotest      # maturin develop + pytest
just build-makotest     # release wheel
```

`pyo3/extension-module` is deliberately **not** a Cargo feature — maturin
enables it at build time. Declaring it would make `cargo test --workspace
--all-features` link the Rust test harness against it and fail on undefined
Python symbols. CI exercises both paths.

---

## Scope

makotest covers the Rust core (identifiers, Fristen, Prüfidentifikator
introspection, the AHB answer table, EDIFACT build + interchange + validation),
the EPEX generator, the Marktpartner / BIKO / iMSys simulators, hypothesis
strategies, domain assertions and the pytest plugin.

The Marktpartner simulator is a plain object with `receive()` and carries no AS4
transport of its own: a transport layers on top of it, so it is never a
dependency of the many tests that do not need one.

The package version tracks `workspace.package.version` through Cargo.toml, so
the wheel and the crates it binds can never report different versions.
