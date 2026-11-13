+++
title = "Einrichtung"
description = "Check a deployment's configuration before it starts: every finding of every daemon, with its setting path and source, in the terminal and over HTTP."
weight = 4
+++
Every daemon checks its own configuration and reports **every** finding, not
the first. Each finding names what is wrong, the setting's path in the
configuration file, and — where a document fixes the rule — the document and
chapter. A blocking finding refuses the boot; a warning is logged at every boot.
The boot, the check command and the HTTP route read one list, so they cannot
disagree.

## The checklist

```bash
just setup
```

builds every daemon under `dev/config/`, runs each with `--check-config --json`
against its dev config, and folds the reports into one checklist. It exits
non-zero while anything is red or a daemon could not be asked.

Against a running deployment, ask makod, which folds the reports of the daemons
in its `[[einrichtung.dienst]]` table:

```bash
just setup --url https://makod.example:8080 --token-env MAKOD_TOKEN
```

| Item | Question |
|---|---|
| `identitaeten` | Every Rolle has exactly one MP-ID per Sparte, with the Codenummer it requires (`99…` Strom, `98…` Gas) |
| `zertifikate` | Every AS4 certificate is readable, matches its key and is not about to expire |
| `bilanzierung` | Every Bilanzkreis is assigned to its Sparte without ambiguity |
| `zugriff` | Every door is authenticated and authorised |
| `erreichbarkeit` | Every daemon and counterparty can be reached |
| `formatabdeckung` | Every format version in force is covered by a profile and an adapter |
| `konfiguration` | Every other setting is valid |

An item is **grün** with no finding, **gelb** with warnings only, **rot** with a
blocking finding, and **unbekannt** when a daemon could not be asked — an
unreachable daemon is neither ready nor failing. Two daemons naming different
MP-IDs for one Rolle and Sparte is a finding of its own, which no single daemon
can make.

`just doctor` prints the same reports as one line per daemon — ready or
blocked, and how many findings block or warn.

## The proposal

The setup interview asks only what changes the configuration: the Rollen and
the MP-ID each plays under, the primary MP-ID where there are several, a
Lieferant's Bilanzkreise per Sparte, the affiliated Lieferanten, the AS4 key
material as paths, and where the other daemons run. From the answers
`mako_service::einrichtung::vorschlag` proposes one file per daemon:

- every line marked against what the daemons report as loaded — new,
  unchanged, or to remove — and naming the rule or document it satisfies
  (`# Quelle:` in the file);
- every fact nobody gave a `# FEHLT:` gap, so the file still parses and the
  daemon still refuses until the gap is filled;
- no secret: a certificate, a key or an API key is named by its path, and an
  answer that is key material is refused;
- an answer that breaks a rule refused with its source — a Gas Rolle under a
  `99…` code, a Strom Rolle under `98…` (Identifikatoren AWH V1.2, Kap. 2.2),
  one Codenummer for both Sparten (Allgemeine Festlegungen V6.1d, Kap. 2.13).

The same function answers the Desk's **Vorschlag**, makod's
`POST /admin/einrichtung/vorschlag` and MCP `einrichtung_vorschlag`, and the
terminal:

```bash
just setup --vorschlag antworten.toml                 # the diff per daemon
just setup --vorschlag antworten.toml --schreiben out # and the files in out/
```

```toml
# antworten.toml
primaer = "9900357000004"
verbundene_lieferanten = []

[[rollen]]
rolle = "lf-strom"
mp_id = "9900357000004"

[[rollen]]
rolle = "lf-gas"
mp_id = "9800357000005"

[[bilanzkreise]]
sparte   = "strom"
standard = ["11XBK-STD------Y"]
```

It exits non-zero while the proposal has a gap or a refused answer. Nothing is
written to a server: the operator applies each file. In the Desk one sentence
can pre-fill the interview; agentd keeps an MP-ID or Bilanzkreis it proposes
only where the sentence contains it, and each answer waits for the person to
take it.

## The first run

```bash
just erstlauf
```

starts from makod's operating frame alone — refused, no Rolle and no
identity — answers the interview for a Lieferant Strom und Gas, appends the
proposed `makod.toml`, checks it ready, boots makod, puts the sample
Marktlokation into its cache and rehearses the Strom Anmeldung. Each step is
timed; the run fails on any step, or over 30 minutes.

## One daemon

| Command | Prints |
|---|---|
| `<daemon> --check-config` | Every finding, one per line; exit 1 on a blocking one |
| `<daemon> --check-config --json` | The report `GET /admin/einrichtung` serves |
| `<daemon> --print-config` | The merged config (secrets redacted), the findings and the policy |

A config that does not load reports each loader problem — a missing, unknown or
invalid key — as a blocking finding at its path. Nothing is bound, opened or
written.

## Over HTTP

| Route | Cedar action | Body |
|---|---|---|
| `GET /admin/einrichtung` | `read-config` (makod: `AdminEinrichtung`) | the daemon's report |
| `GET /api/v1/policy` | `read-policy` (makod: `AdminEinrichtung`) | its [operator policy](@/docs/reference/operator-policy.md) |
| `GET /admin/einrichtung/deployment` (makod) | `AdminEinrichtung` | the checklist across daemons |

The report:

```json
{
  "dienst": "processd",
  "version": "0.31.0",
  "bereit": false,
  "befunde": [
    {
      "schwere": "blockiert",
      "bereich": "bilanzierung",
      "text": "[[lf.bilanzkreise]] rows 1 and 2 both name sparte GAS …",
      "pfad": "lf.bilanzkreise[1]",
      "quelle": "…",
      "referenz": "@/docs/services/….md#policy-…"
    }
  ],
  "policy": { "entries": [] },
  "parteien": [{ "rolle": "LF", "sparte": "strom", "mp_id": "…" }]
}
```

`schwere` is `blockiert` or `warnung`. `quelle` is present where a document
fixes the rule, `referenz` where the setting is an operator policy setting.

## makod

makod's report covers `[[party]]` (one MP-ID per role, the Codenummer each
Sparte requires), `[deployment] rollen`, the adapter coverage per format
version, the AS4 key material and partner certificates, authentication and
Cedar, and the callback and marktd endpoints. A certificate that has expired
blocks; one that expires within `[as4] zertifikat_warnfenster_tage` (default
30) warns with its expiry date.

```toml
[[einrichtung.dienst]]
name         = "processd"
url          = "http://processd:8580"
api_key_file = "/run/secrets/processd-read-config"
```

Each entry is asked for `GET /admin/einrichtung` with that key as a bearer
token, so the key needs `read-config` at that daemon.

The Desk's **Einrichtung** view shows the same checklist, read through makod's
MCP tool `einrichtung_checkliste`, and asks again every 15 seconds. The dev
`makod.toml` names every dev daemon, so `just desk` shows the daemons not
started as unreachable.
