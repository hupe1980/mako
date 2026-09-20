# Demos

Three runnable stacks, each a `docker compose` file plus a `smoke.sh`. Every
assertion is on the **outcome** — the amount the tariff implies, the Antwortcode
the Entscheidungsbaum reaches, the receivable to the cent — so a run that hit the
right status code with the wrong number is red.

| Demo | Services | What it proves |
|---|---|---|
| [`nb-stp/`](nb-stp/) | `makod` · `marktd` · `processd` | A UTILMD **55001** Anmeldung arrives over the EDIFACT door, `mako-pruefung` walks `E_0622` to `A51`, and the **55002** Bestätigung goes back — automatically, inside the Frist |
| [`eeg-billing/`](eeg-billing/) | `marktd` · `edmd` · `einsd` | A month of quarter-hour Einspeisemengen settles into a § 21 EEG 2023 Vergütung and a § 14 Abs. 2 UStG Gutschrift |
| [`o2c/`](o2c/) | `productd` · `vertragd` · `billingd` · `outputd` · `accountingd` | The retail money path: a Tarifpreisblatt prices a Vertrag, the invoice becomes a stored PDF/A document and an Offener Posten, and a payment closes it |

## Running one

```bash
# from the workspace root
just build-demo        # nb-stp:      makod, marktd, processd
just build-demo-eeg    # eeg-billing: marktd, edmd, einsd
just build-demo-o2c    # o2c:         productd, vertragd, billingd, outputd, accountingd

cd demos/<name>
docker compose up -d
bash smoke.sh
```

`just build-demo` is the quick one — its three images come out of a
`demo-builder` stage that compiles only what the Lieferbeginn path needs. The
other two copy from the full `builder` stage, which compiles every service in
the workspace in one `cargo build`, so budget 20–45 minutes cold and a few
minutes on a warm BuildKit cache.

Every stack publishes on its own host ports, so two can run side by side.

## Re-running without a reset

Each demo generates the identifiers it writes — a fresh check-digit-valid
MaLo-ID per run, and the Messlokation and Zählpunktbezeichnung that go with it —
so `bash smoke.sh` can be repeated against a warm stack. `docker compose down -v`
wipes the volume when you want a clean one.

## What these are not

They run with **authentication disabled** and with demo secrets in the compose
file. These are the endpoints that set prices, create contracts and move money.
Do not deploy this configuration; the
[production guide](https://hupe1980.github.io/mako/docs/guide/getting-started/)
has the OIDC setup.

The Marktpartner-IDs are synthetic on purpose. An MP-ID that satisfies a
published check-digit procedure is an assigned code naming a real company
(Identifikatoren AWH §2.1), and a public demo must not carry one.

## What holds them to the code

A demo is shipped API surface, so the request bodies and the config files are
checked by ordinary tests rather than only by running the stack:

| | |
|---|---|
| `just test-demo-payloads` | every body a `smoke.sh` posts deserialises into the real request type — a field the endpoint does not have is a refusal, not a discarded value |
| `just test-demo-configs` | every `*.toml` a stack mounts names no key its service ignores |
| `crates/edi-energy/tests/demo_fixtures.rs` | every `.edi` fixture validates against its AHB profile |
| `crates/edi-energy/tests/party_agency_code.rs` | every fixture stamps the coding authority its MP-ID implies, in `UNB` DE 0007 and `NAD` DE 3055 alike |

Both `just` recipes are shortcuts: the tests behind them are ordinary
`#[test]`s under `services/*/tests/`, so `just ci` runs them too.
