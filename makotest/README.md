# makotest

Test and simulation toolkit for German market-communication (MaKo) platforms.
It builds regulator-conformant EDIFACT, simulates the counterparties a platform
talks to — answering in EDIFACT, late, or not at all — and asserts on both wire
contracts: the messages and the CloudEvents stream. It targets
[mako](https://github.com/hupe1980/mako) but drives only public interfaces, so
it can exercise any MaKo implementation.

```bash
pip install makotest                  # no runtime dependencies
pip install 'makotest[hypothesis]'    # + property-based strategies
```

Wheels are abi3 (`abi3-py311`): one wheel serves Python 3.11 and later, with no
runtime dependencies.

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
    platform.ingest(reply.business)  # a rendered interchange, not a dict


def test_frist_faellt(nb_sim, anmeldung):
    nb_sim.on(55001).timeout()  # no answer, not even a CONTRL
    assert not nb_sim.receive(anmeldung)


def test_verspaetete_antwort(nb_sim, anmeldung):
    nb_sim.on(55001).bestaetigung(delay_werktage=3)  # right message, wrong day
    reply = nb_sim.receive(anmeldung, received_at="2026-03-02T09:00:00Z")
    assert reply.answered_at > reply.due_at
```

From a shell:

```console
$ makotest validate inbound.edi --on 2026-04-01
$ makotest frist 55001 --received 2026-03-02T09:00:00Z
$ makotest id 9900357000004      # → satisfies NEITHER check-digit procedure
$ makotest codes --pid 55001     # what a counterparty may answer with
```

## Design

Everything a regulator defines in a table is the platform's own Rust, bound
through PyO3 — never reimplemented in Python:

| Concern | Home |
|---|---|
| EDIFACT build + MIG/AHB/semantic validation, release per format version | Rust — `edi-energy` |
| Identifier check digits (MaLo, MP-ID, EIC, §8.2 resources) | Rust — `rubo4e` |
| Werktag calendar, acknowledgement clocks, answer Fristen | Rust — `mako-fristen` |
| Antwortcodes per Entscheidungsbaum | Rust — `mako-pruefung` |
| CloudEvents type catalog and subscription matcher | Rust — `mako-events` |
| Counterparty behaviour, EPEX curves, fixtures | Python |

Generators are seeded, clocks injected and the format version is an argument,
so two runs produce byte-identical EDIFACT. Every validation report carries
`rules_applied`, and every event assertion resolves its type against the
platform's catalogue first — a check that matched nothing fails instead of
passing. `AssertionError` means the system under test is wrong; `ValueError`
means the test is.

## pytest plugin

Registered through the `pytest11` entry point:

| | |
|---|---|
| **Fixtures** | `nb_sim`, `biko_sim`, `imsys_sim`, `epex`, `lastgang`, `frozen_clock`, `makotest_seed`, `makotest_on`, `mako_endpoint` |
| **Markers** | `@pytest.mark.regulatory("GPKE Teil 2")`, `@pytest.mark.requires_docker` |
| **Options** | `--makotest-on ISO_DATE`, `--makotest-seed N`, `--mako-endpoint URL` |

## Development

`makotest` is a member of the mako Cargo workspace, built with maturin:

```bash
python3.11 -m venv .venv    # PYO3_PYTHON points here (.cargo/config.toml)
just test-makotest          # maturin develop + pytest
just lint-makotest          # ruff
just build-makotest         # release wheel
```

`pyo3/extension-module` is not a Cargo feature (maturin enables it), so
`cargo test --workspace --all-features` still links.

The full reference — every builder, assertion, strategy and simulator, with the
Fundstelle behind each rule — is
[makotest in the mako documentation](https://hupe1980.github.io/mako/docs/reference/makotest/).

## Licence

MIT OR Apache-2.0, matching mako.
