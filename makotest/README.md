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
| **Rust core** | `edi-energy` (EDIFACT, MIG/AHB validation), `rubo4e` (check digits), `mako-fristen` (Werktage, Fristen), `mako-pruefung` (Antwortcodes and the NB participation check), `mako-events` (CloudEvent types) |
| **pytest** | plugin via `pytest11`: simulator, clock and seed fixtures, `@pytest.mark.regulatory` |
| **Fixtures** | `nb_sim`, `biko_sim`, `mgv_sim`, `imsys_sim`, `epex`, `lastgang`, `frozen_clock`, `makotest_seed`, `makotest_on`, `mako_endpoint` |
| **Options** | `--makotest-on ISO_DATE`, `--makotest-seed N`, `--mako-endpoint URL` |
| **CLI** | `makotest validate`, `frist`, `id`, `pids`, `codes`, `shadow reconcile`, `shadow rehearse`, … |

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

## Reconcile a shadow capture

From the repository root, run `just test-makotest` to install the development build, then compare the two independently prepared inputs and read the mirror on disk.

```sh
capture=makotest/tests/fixtures/shadow
makotest/.venv/bin/makotest shadow reconcile --journal "$capture/journal.json" \
  --inventory "$capture/inventory.json" --mirror-root "$capture/mirror"
```

The fixture contains two synthetic Strom Anmeldungen received by an NB and a closed outbound interval with zero traffic. Exit 0 means the capture is complete or confirmed empty. Exit 1 identifies differences in sealed inputs. Exit 2 means coverage could not be established. Read `eligible_for_comparison` before comparing business outcomes. The report contains fingerprints, counts and reason codes and can be redirected to a file **outside** the mirror. [Capture format and recovery steps](https://hupe1980.github.io/mako/docs/reference/makotest/#reconcile-a-shadow-capture)

Replay one occurrence against the independent expected reply. The synthetic fixture yields two eligible Anmeldungen, one matching response and one unassessed occurrence.

```sh
capture=makotest/tests/fixtures/shadow
makotest/.venv/bin/makotest shadow rehearse \
  --journal "$capture/journal.json" --inventory "$capture/inventory.json" \
  --mirror-root "$capture/mirror" --coverage "$capture/reports/complete.json" \
  --case "$capture/case.json" --expected "$capture/expected.json"
```

Add `--emit summary` to see the counts and the two answers. With `makod` on `PATH`, add `--makod-workspace "$(mktemp -d)"` to compare a reply from the daemon’s own workflow and capture its outbox locally.

For the isolated review installation, add `--emit review --review-nb 9900357000003` to the daemon command and redirect stdout to a private file outside the mirror. Every inbound envelope must address that NB. Use `--also CASE EXPECTED [OBSERVATION]` for each additional assessment. Exit 1 produces a reviewable difference. Exit 2 withholds the private file contents. [Review configuration](https://hupe1980.github.io/mako/docs/services/processd/#isolated-shadow-review)

Compare a response present on only one side with `makotest shadow pair --incumbent DIR --shadow DIR --case FILE`. Both archives must cover the same inbound and outbound window. Assess an ESA invoice using `makotest shadow invoice --invoice FILE --reference FILE`. The bundled settlement example returns EUR 13.95 for the position and REMADV 33001. [Capture and invoice contracts](https://hupe1980.github.io/mako/docs/reference/makotest/#compare-an-esa-invoice)

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
