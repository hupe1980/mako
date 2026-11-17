+++
title = "Loslegen"
description = "Ein Befehl bis zu einem laufenden, befüllten mako: `just dev` baut und startet jeden Dienst mit dem Desk und befüllt ein Stadtwerk; `just demo` startet die veröffentlichten Images ohne Rust."
weight = 2

[extra]
translated_from = "content/docs/guide/getting-started.md"
source_sha256 = "e4589a7d0e3889aab235eaf0450ed4de620a9d8b0be9e720330a31a1bc1eb907"

+++
Zwei Befehle starten mako. Beide fahren dieselbe Welt hoch — die **Stadtwerke
Musterstadt**, Netzbetreiber, Lieferant und Messstellenbetreiber in Strom und
Gas in einem Mandanten — und beide befüllen sie über die öffentlichen APIs der Dienste, damit
der Desk mit Arbeit aufgeht.

| | Braucht | Was läuft |
|---|---|---|
| `just dev` | Docker, Rust (`rustup`), Node für den Desk | PostgreSQL in Docker; jeder Dienst von cargo gebaut und auf deinem Rechner gestartet; der Desk unter Vite |
| `just demo` | Docker | Die veröffentlichten Images aller Dienste und des Desks, ein veröffentlichter Port |

## Prüf deinen Rechner

```bash
git clone https://github.com/hupe1980/mako.git
cd mako
just doctor
```

`just doctor` prüft, was `just dev` braucht, und nennt jede Lücke mit dem
Befehl, der sie schließt: Docker läuft mit Compose v2, die Toolchain, die
`rust-toolchain.toml` festlegt, `curl`, `perl` und `python3`, Node für den Desk
(optional), freie Ports aus `deploy/ports.toml` plus `5432` und `5173` und der
Plattenplatz — `target/` wächst auf rund 100 GB, darunter warnt er. Danach gibt
er eine Zeile pro Daemon-Konfiguration aus, bereit oder blockiert
([Einrichtung](@/docs/guide/einrichtung.de.md) erklärt die Befunde).

## `just dev` — das ganze System auf deinem Rechner

```bash
just dev
```

1. PostgreSQL startet in Docker (`deploy/dev/compose.yaml`), eine Datenbank pro
   Dienst, angelegt von `deploy/postgres/init.sql`.
2. Jeder Dienst und der Seeder werden mit cargo gebaut. Der erste Build dauert
   15–30 Minuten, spätere Sekunden.
3. Jeder Dienst läuft mit seiner Konfiguration `deploy/demo/<dienst>.toml`,
   umgeschrieben nach `target/dev/`, damit er seine Nachbarn und seine Datenbank
   unter `127.0.0.1` findet. Jede Logzeile trägt den farbigen Namen des
   Dienstes, und jedes Log landet zusätzlich in `target/dev/<dienst>.log`.
   `marktd` startet zuerst, `agentd` zuletzt, weil er sich beim Start mit dem
   MCP-Endpunkt jedes anderen Dienstes verbindet.
4. Der Seeder befüllt die Stadtwerke Musterstadt (siehe unten).
5. Der Desk startet unter Vite auf <http://localhost:5173> gegen die laufenden
   Dienste.

Am Ende stehen die URLs, der Token-Befehl und die benötigte Zeit. **Strg-C**
hält die Dienste an und behält die Daten; das nächste `just dev` baut
inkrementell und befüllt nichts doppelt. `just dev-down` löscht die Datenbank.

| Option | Wirkung |
|---|---|
| `just dev --no-desk` | ohne den Desk |
| `just dev --no-seed` | ein leeres System |
| `just dev --ci` | ohne Oberfläche: starten, befüllen, prüfen, beenden — das, was der CI-Job ausführt |

Wird ein Dienst nicht bereit, bricht der Lauf mit den letzten Zeilen seines Logs
ab. Hast du einen Dienst geändert, reichen Strg-C und `just dev`.

### Das befüllte Stadtwerk

`tools/mako-seed` schreibt die Welt über dieselben REST-Routen und dieselbe
EDIFACT-Tür, die eine Integration benutzt, geprüft von denselben Regeln und
Cedar-Policies. Jeder Schritt hängt an einem fachlichen Schlüssel, darum kannst
du `just seed` erneut ausführen, ohne dass sich etwas ändert.

| Was | Wo du es siehst |
|---|---|
| Marktlokationen mit ihren Messlokationen, dem MSB und dem Netzdatensatz, dazu das Preisblatt des Netzbetreibers | `marktd` |
| Eine UTILMD 55001 von Nordlicht Energie, entschieden mit `Accept` (`E_0622` → `A51`) und beantwortet mit der 55002 | `processd`, `makod` |
| Eine 55001 von Musterstadt Energievertrieb, dem eigenen Vertrieb des Stadtwerks, zurückgehalten für einen Menschen (Gleichbehandlung nach § 20 Abs. 1 EnWG) | `processd` — die Freigaben im Desk |
| Eine 55001 auf eine Marktlokation ohne Belieferungsdatensatz, als Klärfall eskaliert (`E_0622` Prüfschritt 50) | `processd` — „Heute“ im Desk |
| Eine Aufdach-Anlage mit 9,8 kWp und ihre Einspeisung im Juni 2026, 2.880 kWh, abgerechnet mit 233,568 EUR nach § 21 EEG 2023 | `einsd`, `edmd` |
| Der Haushaltsvertrag von Erika Mustermann, ihre Januarrechnung (111,33 netto, 132,48 brutto), das ausgestellte PDF, der Offene Posten und die Zahlung, die ihn ausgleicht | `vertragd`, `billingd`, `outputd`, `accountingd` |
| Die Online-Bestellung von Clara Sommer: das eingefrorene Endkunden-Angebot, die Rechtsdokumente und der Vertrag mit seinem Bestellnachweis | `productd`, `vertragd` |
| Zwölf weitere Haushalte mit Verträgen und den täglichen Messwerten des Vormonats; eine ihrer Rechnungen hält das Risiko-Gate zur Prüfung an | `vertragd`, `edmd`, `billingd` |
| Das Stadtwerk im Gas unter seiner eigenen DVGW-Codenummer: sechs Gas-Marktlokationen mit Messlokation, MSB und Belieferungsdatensatz, dazu der Gasvertrag von Hannah Brandt | `marktd`, `productd`, `vertragd` |
| Eine NN-Rechnung Gas (INVOIC 31002) von Gasnetz Nachbarstadt zu 1,42 ct/kWh, wo ihr Preisblatt 1,12 ct/kWh nennt, beanstandet | `invoicd` — die Rechnungsdispute im Desk |
| Eine UTILMD mit einem Prüfidentifikator, den keine Anwendungsübersicht veröffentlicht, als Dead Letter | `makod` — die Dead Letters im Desk |
| Ein KAV-Grenzpreis für 2024, synthetische Demodaten mit der Seed-Datei als Quelle — nie ein veröffentlichter Wert | `marktd` Referenzreihen |

```bash
just seed --verify
```

`--verify` prüft die Ergebnisse, nicht Statuscodes — die Entscheidung und ihren
Antwortcode, die kWh und die Vergütung, jede Rechnungsposition auf den Cent, das
Dokument, die ausgeglichene Forderung — und dass jede Ansicht des Desks, die der
Seeder befüllt, Zeilen hat.

### Personas und Tokens

Die Demo-Konfigurationen setzen `[oidc] mode = "demo"`: Ein Dienst nimmt Tokens
an, die mit dem veröffentlichten Entwicklungsschlüssel signiert sind, und seine
Konfigurationsprüfung meldet das als Warnung. Im Kontomenü des Desks wechselst
du zwischen den Personas; auf der Kommandozeile gibt `just token` eine aus:

| Persona | Marktrollen im Token |
|---|---|
| `nb` — Sachbearbeitung Netz | `NB` |
| `lf` — Sachbearbeitung Vertrieb | `LF` |
| `admin` — Administration (Standard) | `NB`, `LF`, `MSB`, `ESA`, `ADMIN` |
| `agent` — Agent | `NB`, `LF`, `MSB` |

```bash
TOKEN=$(just token lf)
curl -s http://127.0.0.1:9780/api/v1/vertraege -H "Authorization: Bearer $TOKEN"
```

Jede Anfrage entscheiden dieselben Cedar-Policies wie in Produktion. Eine
Konfiguration ohne `mode = "demo"` verweigert den Start gegen einen Issuer, der
den Entwicklungsschlüssel veröffentlicht. `makod` erreichst du mit seinem
API-Schlüssel (`deploy/demo/makod.toml`, `[http] auth_keys`).

### Die Ereignisse mitlesen

```bash
just listen
```

abonniert das Fan-out von `marktd` — jedes CloudEvent des Systems läuft dort
durch —, prüft die Standard-Webhooks-Signatur jeder Zustellung und gibt sie aus,
sobald sie ankommt. Strg-C entfernt das Abonnement.

### Jeder Dienst

Jeder Dienst antwortet auf seinem Port aus `deploy/ports.toml` unter
`http://127.0.0.1:<port>`: `/health/ready`, `/api/v1/openapi.json`, `/mcp`.

| Dienst | Port | | Dienst | Port |
|---|---|---|---|---|
| `makod` | 8080 | | `productd` | 9080 |
| `marktd` | 8180 | | `einsd` | 9180 |
| `invoicd` | 8280 | | `billingd` | 9280 |
| `edmd` | 8380 | | `accountingd` | 9380 |
| `processd` | 8580 | | `portald` | 9480 |
| `netzbilanzd` | 8680 | | `agentd` | 9580 |
| `vertragd` | 9780 | | `outputd` | 9880 |

PostgreSQL erreichst du unter `postgres://mako:mako@127.0.0.1:5432/<dienst>`.

Ein Konfigurationsproblem wird auf einmal gemeldet — die gelesene Datei, das
Präfix der Umgebungsvariablen (`MARKTD_`, verschachtelte Schlüssel mit `__`
verbunden) und jeder fehlende, unbekannte oder ungültige Schlüssel:

```bash
cargo run -p marktd -- --help                                              # Aufruf, liest keine Konfiguration
MARKTD_CONFIG=deploy/demo/marktd.toml cargo run -p marktd -- --check-config  # jeder Befund mit seinem Pfad
```

## `just demo` — ohne Rust

```bash
just demo
```

startet die veröffentlichten Images der Version, die dieser Checkout trägt
(`ghcr.io/hupe1980/mako-<dienst>`): PostgreSQL, jeden Dienst, den Desk und einen
Seed-Container, der dasselbe Stadtwerk befüllt und prüft. Ein Port ist
veröffentlicht: Öffne <http://localhost:8000>. Der Rand des Desks leitet jedes
`/svc/<dienst>/` an seinen Dienst weiter, mit dem Token der Persona, die du im
Kontomenü gewählt hast; `just token <persona>` gibt dieselben Tokens für `curl`
aus.

`just demo --build` baut die Images aus diesem Checkout, statt sie zu laden —
rechne kalt mit 20–45 Minuten. `just demo-down` hält die Demo an und löscht ihre
Daten. `just listen` abonniert über den Rand der Demo, wenn sie läuft.

## Nur der Desk

```bash
just dev-ui
```

startet den Desk auf seinen aufgezeichneten Antworten (`desk/fixtures`) — nur
Node, kein Rust, kein Docker.

## Nächste Schritte

| Thema | Anleitung (englisch, wo nicht anders genannt) |
|---|---|
| Konfigurationsbefunde und die Deployment-Checkliste | [Einrichtung](@/docs/guide/einrichtung.de.md) |
| EDIFACT parsen und validieren | [Parsing guide](@/docs/reference/parsing.md) |
| ERP-Anbindung — CloudEvents, HMAC | [ERP integration](@/docs/architecture/erp-integration.md) |
| Betriebsreferenz für makod | [makod guide](@/docs/services/makod.md) |
| Betriebsreferenz für marktd | [marktd guide](@/docs/services/marktd.md) |
| processd — NB-STP und automatische LF-Antworten | [processd guide](@/docs/services/processd.md) |
| INVOIC-Plausibilisierung, § 147 AO / GoBD | [invoicd guide](@/docs/services/invoicd.md) |
| Energiedaten, Ungleichgewichte (Imbalance), Abrechnungszeiträume | [edmd guide](@/docs/services/edmd.md) |
| Prozess-Lesemodell, Gleichbehandlung (§ 7a Abs. 5 EnWG) | [processd guide — process read model](@/docs/services/processd.md#process-read-model) |
| Die Architektur des ganzen Systems | [Architecture](@/docs/architecture/_index.md) |
| Prozesskatalog (GPKE, WiM, …) | [Processes](@/docs/reference/processes.md) |
| Bauen, Testen und das Gate | [CONTRIBUTING.md](https://github.com/hupe1980/mako/blob/main/CONTRIBUTING.md) |
