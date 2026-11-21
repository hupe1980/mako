# makotest

Test and simulation toolkit for German market-communication (MaKo) platforms.
It builds regulator-conformant EDIFACT, simulates the counterparties a platform
talks to — answering in EDIFACT, late, or not at all — and asserts on the
messages and the CloudEvents stream. It drives only public interfaces, so it can
exercise any MaKo implementation, not only
[mako](https://github.com/hupe1980/mako).

| | |
|---|---|
| **Package** | `pip install makotest` (no runtime dependencies); `makotest[hypothesis]` adds property-based strategies |
| **Build** | PyO3 extension over mako's Rust crates, built with maturin; one abi3 wheel for Python ≥ 3.11 |
| **Rust core** | `edi-energy` (EDIFACT, MIG/AHB validation), `rubo4e` (check digits), `mako-fristen` (Werktage, Fristen), `mako-pruefung` (Antwortcodes), `mako-events` (CloudEvent types) |
| **pytest** | plugin via `pytest11`: simulator, clock and seed fixtures, `@pytest.mark.regulatory` |
| **Fixtures** | `nb_sim`, `biko_sim`, `mgv_sim`, `imsys_sim`, `epex`, `lastgang`, `frozen_clock`, `makotest_seed`, `makotest_on`, `mako_endpoint` |
| **Options** | `--makotest-on ISO_DATE`, `--makotest-seed N`, `--mako-endpoint URL` |
| **CLI** | `makotest validate`, `frist`, `id`, `pids`, `codes`, … |

## Example

```python
from makotest import antwort_obligation, malo_from_base, validate_edifact

malo_from_base("5123869601")  # '51238696012' — BDEW check digit applied

o = antwort_obligation(55001)  # what a Netzbetreiber owes on an Anmeldung
o.clock_time  # '11:00' — a clock time, not n × 24 h
o.due_at("2026-03-02T09:00:00Z")  # '2026-03-03T11:00:00+01:00'

validate_edifact(utilmd_bytes, "2026-10-01").is_valid  # MIG + AHB + semantic
```

```python
def test_nb_bestaetigt(nb_sim, anmeldung):
    nb_sim.on(55001).bestaetigung(antwort_code="A51", ebd="E_0623")
    reply = nb_sim.receive(anmeldung, received_at="2026-03-02T09:00:00Z")
    assert reply.pid == 55002  # the AHB answer PID — never Anfrage + 1
```

Generators are seeded, clocks injected and the format version is an argument,
so two runs produce byte-identical EDIFACT. `AssertionError` means the system
under test is wrong; `ValueError` means the test is.

## Run

`makotest` is a member of the mako Cargo workspace:

```bash
python3.11 -m venv .venv    # at the workspace root; PYO3_PYTHON points here (.cargo/config.toml)
just test-makotest          # maturin develop + pytest
just lint-makotest          # ruff
just build-makotest         # release wheel
```

`pyo3/extension-module` is not a Cargo feature (maturin enables it), so
`cargo test --workspace --all-features` still links.

Licence: MIT OR Apache-2.0, matching mako.

The full reference — every builder, assertion, strategy and simulator, with the
Fundstelle behind each rule — is
[makotest in the mako documentation](https://hupe1980.github.io/mako/docs/reference/makotest/).
