# makotest

**Test & simulation toolkit for German market-communication (MaKo) platforms.**

Generates regulator-conformant inputs — EDIFACT, EPEX price curves, meter data —
simulates the external counterparties a MaKo platform talks to, and asserts on
the result.

`makotest` targets [mako](https://github.com/hupe1980/mako) first but is **not
mako-specific**: everything it drives is a public wire contract (EDIFACT over
AS4, REST, CloudEvents), so it can exercise any MaKo implementation.

```python
from makotest import malo_from_base, add_werktage, validate_edifact

malo_from_base("5123869678")      # '51238696780' — BDEW check digit applied
add_werktage("2026-12-24", 2)     # '2026-12-29' — skips holidays and the weekend

report = validate_edifact(utilmd_bytes, "2026-10-01")
report.is_valid                   # MIG + AHB + semantic rules, on that FV
```

## Why

The regulated domain makes this sharper than ordinary integration testing:
deadlines are legal obligations, message content is defined by AHB rule tables,
and a wrong Prüfidentifikator is a compliance defect rather than a bug. Tests
need to express *"this is what the counterparty is entitled to send, and this is
what we owe them by when"* — which a `curl` script cannot.

## Design

**One source of truth for wire formats.** EDIFACT validation, identifier check
digits and the Werktag calendar come from the same Rust crates the platform
runs, through PyO3 bindings. They are never reimplemented in Python: a second
implementation would drift from the BDEW profiles at the first Formatumstellung,
and a harness that disagrees with production about validity is worse than none.

The rule for what goes where: **anything a regulator defines in a table is
Rust; anything shaped by test ergonomics is Python.**

| Concern | Home |
|---|---|
| EDIFACT MIG/AHB/semantic validation | Rust (`edi-energy`) |
| MaLo/MeLo check digits and formats | Rust (`rubo4e`) |
| Werktag / Feiertag calendar | Rust (`mako-engine::fristen`) |
| EPEX curves, load profiles | Python |
| Counterparty behaviour, fixtures, DSL | Python |

**A consequence worth stating:** because validation runs the platform's own AHB
engine, `makotest` proves *process and integration* behaviour — it is not an
independent check of mako's format conformance. The BDEW reference examples
remain the authority for that.

**Deterministic by construction.** Every generator takes a seed; every clock is
injected. A test that passes on Tuesday and fails on a Feiertag is not a test of
the system.

**Framework-agnostic core, pytest on top.** The generators and simulators are
plain objects usable from a script or a notebook. Only `makotest.plugin` imports
pytest, so a demo and a CI test drive the same code path.

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

## Install

```bash
pip install makotest                  # core: identifiers, Fristen, EDIFACT
pip install 'makotest[data]'          # + EPEX / Lastgang generators (numpy, pandas)
pip install 'makotest[bo4e]'          # + BO4E business objects
pip install 'makotest[hypothesis]'    # + property-based strategies
```

## pytest usage

The plugin registers itself through the `pytest11` entry point — no `conftest.py`
wiring needed.

```python
def test_frist_is_met(frozen_clock, epex_sim):
    frozen_clock.advance_werktage(4)     # BDEW calendar, not naive +4 days
    assert frozen_clock.date == "2026-11-09"
```

Fixtures: `epex_sim`, `frozen_clock`, `makotest_seed`, `mako_endpoint`.
Markers: `@pytest.mark.regulatory("GPKE Teil 2")`, `@pytest.mark.requires_docker`.
Options: `--mako-endpoint URL` (run against a live deployment),
`--makotest-seed N` (reproduce a failing run exactly).

## BO4E generation

`makotest` builds business objects from BO4E **202607** — the same release
mako's `rubo4e` generates from. Assert it once per session; testing v202607
objects against a platform on another generation produces passes that mean
nothing:

```python
from makotest.assertions import assert_bo4e_generation_matches
assert_bo4e_generation_matches(platform.bo4e_version)
```

## Development

`makotest` is a member of the mako Cargo workspace and builds with
[maturin](https://www.maturin.rs/):

```bash
cd makotest
maturin develop --extras dev
pytest
```

`pyo3/extension-module` is deliberately **not** a Cargo feature of this crate —
maturin enables it at build time. Declaring it would make `cargo test
--workspace --all-features` link the test harness against it and fail on
undefined Python symbols.

Design rationale and the roadmap for the simulator suite (market partner over
AS4, BIKO, iMSys/SMGW) live in `concepts/MAKOTEST.md`.

## Status

Pre-1.0 and incomplete. Shipping today: the Rust core (identifiers, Fristen,
EDIFACT build + interchange envelope + validation), the EPEX generator, domain
assertions, and the pytest plugin. The counterparty simulators are specified but
not yet built.

The package version tracks `workspace.package.version` through Cargo.toml
(`dynamic = ["version"]`), so the wheel and the crates it binds can never report
different versions.

## Licence

MIT OR Apache-2.0, matching mako.
