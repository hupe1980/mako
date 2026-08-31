# makotest

**Test & simulation toolkit for German market-communication (MaKo) platforms.**

Builds regulator-conformant EDIFACT, simulates the counterparties a MaKo
platform talks to — in EDIFACT, so a test can feed the answer back — and asserts
on both wire contracts it exposes: the messages and the event stream.

`makotest` targets [mako](https://github.com/hupe1980/mako) first but is **not
mako-specific**. Everything it drives is public (EDIFACT over AS4, REST,
CloudEvents), so it can exercise any MaKo implementation.

```python
from makotest import antwort_obligation, malo_from_base, validate_edifact

malo_from_base("5123869601")  # '51238696012' — BDEW check digit applied

o = antwort_obligation(55001)  # what a Netzbetreiber owes on an Anmeldung
o.clock_time  # '11:00' — a clock time, not n × 24 h
o.due_at("2026-03-02T09:00:00Z")  # '2026-03-03T11:00:00+01:00'

validate_edifact(utilmd_bytes, "2026-10-01").is_valid  # MIG + AHB + semantic
```

```bash
pip install makotest                  # no runtime dependencies
pip install 'makotest[hypothesis]'    # + property-based strategies
```

Nothing is pinned because nothing needs to be: everything regulated is compiled
into the extension, and the Europe/Berlin day comes from its tz database — so
there is no `tzdata` requirement on Windows, where `zoneinfo` ships no data.

Wheels are **abi3** (`abi3-py311`) — one wheel serves Python 3.11 and later.

The same answers are reachable from a shell, for whoever is holding a real
message rather than writing a test:

```console
$ makotest validate inbound.edi --on 2026-04-01
$ makotest frist 55001 --received 2026-03-02T09:00:00Z
$ makotest id 9900357000004      # → satisfies NEITHER check-digit procedure
$ makotest codes --pid 55001     # what a counterparty may answer with
```

---

## Why

Deadlines are legal obligations, message content is defined by AHB rule tables,
and a wrong Prüfidentifikator is a compliance defect rather than a bug. A test
has to express *"this is what the counterparty is entitled to send, and this is
what we owe them by when"* — which a `curl` script cannot.

Two failure modes shape the whole design, because both produce a **green suite
that proves nothing**:

- a message whose Prüfidentifikator has no AHB rules *validates* — having
  checked nothing;
- an assertion naming an event type the platform does not declare finds no such
  event — forever.

Every validation report therefore carries `rules_applied`, and every event
assertion resolves the type against the platform's own catalog first.

## Design

**One source of truth.** EDIFACT construction and validation, identifier check
digits, the Werktag calendar, the published answer Fristen and the CloudEvents
catalog come from the same Rust crates the platform runs, through PyO3 bindings.
None is reimplemented in Python: a second implementation drifts from the BDEW
documents at the first Formatumstellung, and a harness that disagrees with the
system under test about what is valid — or about when a Frist expires — is worse
than none.

The rule: **anything a regulator defines in a table is Rust; anything shaped by
test ergonomics is Python.**

| Concern | Home |
|---|---|
| EDIFACT build + MIG/AHB/semantic validation, release per format version | Rust — `edi-energy` |
| Identifier check digits (MaLo, MP-ID, EIC, §8.2 resources) | Rust — `rubo4e` |
| Werktag calendar, acknowledgement clocks, answer Fristen | Rust — `mako-fristen` |
| Antwortcodes per Entscheidungsbaum | Rust — `mako-pruefung` |
| CloudEvents type catalog and subscription matcher | Rust — `mako-events` |
| Counterparty behaviour, EPEX curves, fixtures | Python |

Because validation runs the platform's own AHB engine, `makotest` proves
*process and integration* behaviour — not format conformance. The BDEW reference
examples remain the authority for that.

**Deterministic by construction.** Every generator is seeded, every clock is
injected, and the format version is an argument rather than a read of today's
date. Two runs of the same scenario produce byte-identical EDIFACT.

**Framework-agnostic core, pytest on top.** Only `makotest.plugin` imports
pytest, so a demo and a CI test drive the same code path.

**Two failures, two exceptions.** `AssertionError` means the system under test is
wrong; `ValueError` means the test is.

---

## A tour

**Five families, four Frist shapes, and Gas differs on the wire too** — a Gas
answer names no Codeliste in `SG4 STS+E01` DE 1131 where a GPKE answer names its
EBD, and UTILMD runs a parallel `G…` release track on the same date as `S…`.

**Fristen have four shapes, so ask the table.** "A Werktage Frist expires at
17:00 Berlin" is true of the WiM MSB-Wechsel windows and of nothing else — GPKE
states a clock time on the n-th Werktag after the ÜT, or on the ÜT itself, and
GeLi Gas the *end* of the n-th Werktag. The two GPKE shapes share a clock time
and land a day apart.

```python
assert_deadline_is(response["deadline"], received=received, pid=55001)
assert_frist_met(55001, received=received, answered_at=answer["sent_at"])
```

**The send date picks the format version.** Pinning a release by hand and
validating on a date where another is in force reports the mismatch rather than
the message.

```python
msg = build_utilmd(
    55001,
    sender=LF,
    receiver=NB,
    on="2026-04-01",
    transactions=[
        UtilmdTransaction("VORGANG-1", locations=[("melo", melo)], dates=[("92", start)])
    ],
)
wire = build_interchange(
    sender=LF, receiver=NB, dar="REF1", messages=[msg], on="2026-04-01"
)
assert_edifact_valid(wire, on="2026-04-01")
```

**Counterparties answer in EDIFACT — or badly, or not at all.** The unhappy modes
are the ones worth having: a platform that only ever sees a punctual, conformant
partner has never had its Fristüberwachung exercised.

```python
def test_nb_bestaetigt(nb_sim, anmeldung):
    nb_sim.on(55001).bestaetigung(antwort_code="A51", ebd="E_0623")
    reply = nb_sim.receive(anmeldung, received_at="2026-03-02T09:00:00Z")

    assert reply.pid == 55002  # the AHB answer PID — never Anfrage + 1
    platform.ingest(reply.business)  # a rendered interchange, not a dict


def test_frist_faellt(nb_sim, anmeldung):
    nb_sim.on(55001).timeout()  # no answer, not even a CONTRL
    assert not nb_sim.receive(anmeldung)


def test_verspaetete_antwort(nb_sim, anmeldung):
    nb_sim.on(55001).bestaetigung(delay_werktage=3)  # right message, wrong day
    reply = nb_sim.receive(anmeldung, received_at="2026-03-02T09:00:00Z")
    assert reply.answered_at > reply.due_at
```

An interchange carries several messages, each its own Vorgang — the reply answers
all of them, and `reply.pid` raises rather than speaking for one.

**A counterparty remembers what it accepted.** A Netzbetreiber holding an open
Vorgang for a Marktlokation does not answer a second Anmeldung the way it
answered the first — `E_0622` publishes `A06` „Andere Anmeldung in Bearbeitung"
for exactly that, and a platform re-sending a confirmed request is otherwise
never contradicted.

```python
nb_sim.on(55001).bestaetigung(antwort_code="A51", ebd="E_0623")
nb_sim.on(55001).bei_offenem_vorgang().ablehnung(antwort_code="A06", ...)
```

Only a Bestätigung opens a Vorgang, and the register is keyed on the **Lokation**
— the Vorgangsnummer is the sender's reference, and a duplicate carries a new one.

**Answers are read back structurally**, not by matching bytes: `LOC+Z16` and
`LOC+Z17` differ by one character, and a substring check passes on the wrong one.

```python
v = validate_edifact(reply.business, on).messages[0].vorgaenge[0]
v.location("melo"), v.iso_date("92"), v.antwort_code
```

**An answer states a code its Entscheidungsbaum publishes.** `SG4 STS+E01` is
AHB-Muss on every Antwortnachricht, and a code means nothing outside its tree —
`A02` is „Vorlauffrist nicht eingehalten" in `E_0607` and something else in
`E_0622`. The catalogue decides: a code the tree has no leaf for, or one whose
Cluster contradicts the answer PID it would ride, is refused at binding time.

```python
antwort_code("E_0622", "A06").bedeutung  # what the BDEW says it means
antwort_codes_for_pid(55001)  # the tree's whole outcome space
assert_antwort_code(reply.antwort_code, ebd="E_0622", accepted=False)
```

The catalogue serves three wires — `SG4 STS+E01` on a UTILMD, `AJT` on a REMADV
and an ORDRSP — with one lookup for all of them. That is what makes **55 of the
58** published answer obligations answerable: a Frist and an Antwortcode are of
no use if the answer's message type cannot be built.

DE 1131 carries the *Codeliste*, which for every WiM tree is an `S_xxxx` and not
the EBD number — `E_0200` names `S_0090` on a Zustimmung and `S_0054` on an
Ablehnung.

**A Bilanzkreis is a segment group, named once.** `bilanzkreis="11XBK-EEG-----1"`
on a transaction renders the whole `SG8 SEQ+Z79` Produktpaket — the Produkt-Code
and CAV qualifier are AHB constants a test should not be transcribing.

**Events are the other wire contract.**

```python
assert_event_emitted(webhook_bodies, "de.mako.process.*", subject=malo)
assert_no_event_emitted(webhook_bodies, "de.mako.aperak.timeout")
```

**Strategies draw values the platform accepts.** A random 11-digit string is a
valid MaLo one time in ten and a random 16-character string is essentially never
a valid EIC, so a hand-rolled strategy spends its budget on the rejection path.

```python
@given(malo=malo_ids(), pid=pruefidentifikatoren(message_type="UTILMD"))
def test_every_utilmd_roundtrips(malo, pid): ...
```

**Lastgang curves are synthetic, and say so.** Four shapes — household, Gewerbe,
Wärmepumpe, PV feed-in — scaled to an annual quantity with a seasonal weight.
They are deliberately *not* Standardlastprofile: the BDEW coefficient tables are
not in this build, and a generator calling itself `H0` while inventing them
would make every settlement asserted against it look authoritative and be wrong.

```python
gang = smgw.deliver("2026-10-25", werte=LastgangGenerator(seed=42).day("2026-10-25"))
gang.as_mscons(pruefidentifikator=13025, ...)   # one QTY per interval, with its period
```

Interval data carries its **measurement period**. A bare `QTY` states a magnitude
with no time reference — a receiver cannot settle against it, and the AHB does
not reject it, so the flat form validates while being unusable.

**EPEX days are Europe/Berlin days**, so the DST days carry 92 and 100
quarter-hourly MTUs rather than 96 — and negative prices are supported, which
§51 EEG and §41a EnWG dynamic tariffs both need.

```python
EpexGenerator(seed=42).day("2026-06-21", profile="solar_glut", negative_hours=6)
```

The full reference — every builder, assertion, strategy and simulator, with the
Fundstelle behind each rule — is in the
[mako documentation](https://hupe1980.github.io/mako/docs/reference/makotest/).

---

## pytest plugin

Registered through the `pytest11` entry point — no `conftest.py` wiring.

| | |
|---|---|
| **Fixtures** | `nb_sim`, `biko_sim`, `imsys_sim`, `epex`, `lastgang`, `frozen_clock`, `makotest_seed`, `makotest_on`, `mako_endpoint` |
| **Markers** | `@pytest.mark.regulatory("GPKE Teil 2")`, `@pytest.mark.requires_docker` |
| **Options** | `--makotest-on ISO_DATE`, `--makotest-seed N`, `--mako-endpoint URL` |

`--makotest-on` pins the format version for the whole suite, so re-running on a
future date shows what the next Formatumstellung breaks. Every dated fixture
takes it, so one test is never about two days. `--mako-endpoint` names a running
deployment; bring your own HTTP client.

`--hypothesis-profile=makotest` selects a registered profile with `deadline=None`
and `derandomize=True` — a strategy here draws through the Rust core, and
Hypothesis' 200 ms per-example deadline is written for pure functions.

---

## Development

`makotest` is a member of the mako Cargo workspace and builds with
[maturin](https://www.maturin.rs/):

```bash
just test-makotest      # maturin develop + pytest
just lint-makotest      # ruff check + format check
just build-makotest     # release wheel
```

`pyo3/extension-module` is deliberately **not** a Cargo feature — maturin enables
it at build time. Declaring it would make `cargo test --workspace --all-features`
link the Rust test harness against it and fail on undefined Python symbols. CI
exercises both paths.

`abi3-py311` sets a floor of Python 3.11, so *every* workspace-wide cargo command
builds this crate against an interpreter. The workspace pins `PYO3_PYTHON` to
`.venv/bin/python` in [`.cargo/config.toml`](../.cargo/config.toml); create that
venv once and the rest of the workspace builds:

```bash
python3.11 -m venv .venv    # or any ≥ 3.11
```

Without it PyO3 falls back to the first `python3` on `PATH` — still 3.9 on macOS
— and the whole workspace fails to build on a message about a crate nobody was
working on.

`py.typed` ships with the wheel, so `_native.pyi` is the only thing a consumer's
type checker sees; a test pins it against the compiled module in both directions.

## Scope

The Marktpartner simulator carries no AS4 transport of its own: it is a plain
object with `receive()`, so a transport layers on top instead of being a
dependency of the many tests that do not need one.

A counterparty is modelled once it has a consumer. One written ahead of its
consumer encodes guesses about an interface nobody has implemented, and to the
next reader those guesses are indistinguishable from requirements.

The package version tracks `workspace.package.version` through Cargo.toml, so the
wheel and the crates it binds can never report different versions.

## Licence

MIT OR Apache-2.0, matching mako.
