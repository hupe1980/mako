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
