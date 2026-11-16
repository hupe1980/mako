+++
title = "Loslegen"
description = "Starte die NB-Dunkelverarbeitungs-Demo von mako in etwa fünf Minuten: Schick eine UTILMD-Anmeldung 55001 und erhalte die Bestätigung 55002 zurück."
weight = 2

[extra]
translated_from = "content/docs/guide/getting-started.md"
source_sha256 = "7af26af3930a744071824df356d44b3183046f704caf870f5896ccc6f678c0c7"
+++
Fang mit der Konfigurationsprüfung an — sie braucht weder eine Datenbank noch
einen laufenden Dienst:

```bash
just setup
```

Der Befehl führt `--check-config` jedes Daemons gegen seine Dev-Konfiguration
aus und gibt eine Checkliste aus — Identitäten, Zertifikate, Bilanzkreise,
Zugriff, Erreichbarkeit, Formatabdeckung —, jeden Befund mit dem Pfad der
Einstellung und seiner Quelle. Solange etwas rot ist, endet er mit einem
Exit-Code ungleich null. [Einrichtung](@/docs/guide/einrichtung.de.md) erklärt
die Checkliste, wie du ein laufendes Deployment abfragst und wie
die Einrichtungsbefragung die Konfiguration vorschlägt, die sie grün macht.
`just doctor` gibt eine Zeile pro Daemon aus; `just erstlauf` führt ein erstes
Deployment von leer bis zu einem bestandenen Probelauf durch und misst jeden
Schritt.

Danach startet diese Anleitung den NB-STP-Demo-Stack lokal: UTILMD 55001 →
automatische NB-Entscheidung → UTILMD 55002.

## Was du startest

| Dienst | Port | Aufgabe |
|---|---|---|
| `postgres` | `5432` | PostgreSQL — eine Datenbank pro Dienst |
| `webhook` | `8000` | Demo-Empfänger für ERP-Events (Python, im Speicher) |
| `marktd` | `8180` | Market Data Hub — MaLo/MeLo/NeLo/TR, VersorgungsStatus, dauerhafter Fan-out, Replay über `event_log` |
| `processd` | `8580` | NB-STP-Autoresponder — `mako-pruefung` (`E_0622`/`G_0011`), LF-Antworten auf 55007/55010 innerhalb ihrer Frist je PID |
| `makod` | `8080` | EDIFACT-Prozess-Engine — GPKE/WiM/GeLi Gas, im Speicher |

```mermaid
sequenceDiagram
    participant LF as LF 4012345000023
    participant makod as makod :8080
    participant marktd as marktd :8180
    participant processd as processd :8580
    participant webhook as webhook :8000

    LF->>makod: POST /edifact<br/>UTILMD 55001
    makod-->>webhook: APERAK BGM+312<br/>(45-min deadline, auto)
    makod->>marktd: de.mako.process.initiated<br/>HMAC POST /api/v1/events (demo override)
    marktd->>processd: de.mako.process.initiated<br/>HMAC POST /webhook
    marktd-->>webhook: de.mako.process.initiated<br/>(ERP subscription)
    processd->>marktd: GET /api/v1/versorgung/{malo_id}
    processd->>marktd: GET /api/v1/malos/{malo_id}/grid
    processd->>marktd: GET /api/v1/partners/{lf_mp_id}
    Note over processd: `mako-pruefung`: E_0622 → Accept (A51)
    processd->>makod: gpke.lieferbeginn.bestaetigen
    makod-->>webhook: UTILMD 55002 Bestätigung
```

---

## Voraussetzungen

| Werkzeug | Version | Installation |
|---|---|---|
| Docker | 24+ mit Compose v2 | https://docs.docker.com/get-docker/ |
| `curl` | beliebig | Paketmanager des Betriebssystems |
| `jq` | beliebig | Paketmanager des Betriebssystems |

---

## Schritt 1 — Klonen und bauen

```bash
git clone https://github.com/hupe1980/mako.git
cd mako

just build-demo
```

`just build-demo` sind die drei Builds, die die Compose-Datei braucht, so
getaggt, wie sie sie erwartet:

```bash
docker build --target runtime          -t makod:dev     .
docker build --target marktd-runtime   -t marktd:dev    .
docker build --target processd-runtime -t processd:dev  .
```

`docker buildx bake` ist der Push-Pfad der CI und erzeugt die `:dev`-Tags
nicht. Die Stage `processd-runtime` baut mit `--features integrated` (die
NB- und LF-Module). Alle drei kommen aus einer Builder-Stage ohne
Iceberg/DataFusion, ein kalter Build dauert also ein paar Minuten.

---

## Schritt 2 — Demo-Stack starten

```bash
cd demos/nb-stp
docker compose up -d
docker compose ps   # wait until all containers are running
```

Erwartet:

```
NAME                 IMAGE              STATUS         PORTS
nb-stp-postgres-1    postgres:17-alpine Up (healthy)   5432/tcp
nb-stp-webhook-1     python:3.12-alpine Up             0.0.0.0:8000->8000/tcp
nb-stp-marktd-1      marktd:dev         Up             0.0.0.0:8180->8180/tcp
nb-stp-processd-1    processd:dev       Up             0.0.0.0:8580->8580/tcp
nb-stp-makod-1       makod:dev          Up             0.0.0.0:8080->8080/tcp
```

`processd` meldet beim Start sein Fan-out-Abonnement bei `marktd` an, und beide
führen beim ersten Start ihre Migrationen aus (`init-db.sh` legt die
Datenbanken an).

---

## Schritt 3 — Health prüfen

```bash
curl -s http://localhost:8080/health | jq .
# → {"status":"ok","instance_id":"..."}

curl -s http://localhost:8180/health | jq .
# → {"status":"ok"}

curl -s http://localhost:8580/health/ready
# → 200 OK
```

---

## Schritt 4 — Stammdaten anlegen

`mako-pruefung` in `processd` braucht drei Einträge in `marktd`, um zu einer
`Accept`-Entscheidung zu kommen. Jeder Pfad unten ist relativ zu
`demos/nb-stp`, dem Verzeichnis, in das du in
[Schritt 2](#schritt-2-demo-stack-starten) gewechselt bist.

### 4a — Preisblatt

```bash
curl -s -X PUT http://localhost:8180/api/v1/preisblaetter/9900357000004 \
  -H "Content-Type: application/json" \
  --data-binary @fixtures/preisblatt-nb.json \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

### 4b — MaLo + Netzdatensatz der MaLo

```bash
MALO_ID=51238696012

# MaLo (NB=9900357000004, no active LF)
curl -s -X PUT "http://localhost:8180/api/v1/malos/$MALO_ID" \
  -H "Content-Type: application/json" \
  --data-binary "$(jq --arg m "$MALO_ID" '.data.marktlokationsId=$m' fixtures/malo-nb.json)" \
  -w "\nHTTP %{http_code}\n"
# → HTTP 201

# MaLo grid record — without it the Anmeldung escalates instead of being answered
curl -s -X PUT "http://localhost:8180/api/v1/malos/$MALO_ID/grid" \
  -H "Content-Type: application/json" \
  -d '{"nb_mp_id":"9900357000004","bilanzierungsgebiet":"11YN0------0STXG","netzgebiet":"DEMO-NZ-001","sparte":"STROM","source":"manual"}' \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

Ohne den Netzdatensatz wird die Anmeldung eskaliert, statt beantwortet zu
werden.

### 4c — LF als Marktpartner (E_0622 Prüfschritt 60)

```bash
# Register in marktd partner directory
curl -s -X PUT http://localhost:8180/api/v1/partners/4012345000023 \
  -H "Content-Type: application/json" \
  -d '{"display_name":"Demo LF","marktrolle":"LF","sparte":"STROM","makoadresse":["https://as4.example.com/as4/receive"]}' \
  -w "\nHTTP %{http_code}\n"
# → HTTP 200

# Register in makod for EDIFACT routing
curl -s -X PUT http://localhost:8080/admin/partners/4012345000023 \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  --data-binary @fixtures/partner-lf.json | jq '.'
```

Der erste Aufruf trägt den LF ins Partnerverzeichnis von `marktd` ein, der
zweite in `makod` für das EDIFACT-Routing.

> **Zwei Verzeichnisse, zwei Formen.** `marktd` hält den BO4E-`Geschaeftspartner`
> des Partners und seine AS4-Endpunkte (`makoadresse`); `makod` hält den
> PARTIN-Datensatz (`channels`, `roles`, `contacts`). Beide lehnen ein Feld ab,
> das sie nicht kennen — ein Body an das falsche Verzeichnis ist also ein `422`,
> der das Feld nennt.

### 4d — Die Zuordnungsermächtigung des BKV (E_0622 Prüfschritt 60)

Der NB bestätigt eine Anmeldung nur in einen Bilanzkreis, für den ein BKV den
LF ermächtigt hat — „je ZRT, BG, BK und LF" (BK6-24-174 Anlage 3 Kap. 10.2.1).
Der BKV tut das mit einer UTILMD 55071; `marktd` speichert sie, und `processd`
liest sie. Die Fixture ermächtigt den Bilanzkreis `11XBK-STD------Y` der 55001
im Bilanzierungsgebiet des Netzdatensatzes ab dem 01.08.2026.

```bash
curl -s -X PUT http://localhost:8080/admin/partners/9900000000019 \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  --data-binary @fixtures/partner-bkv.json | jq '.'

curl -s -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary @fixtures/utilmd-55071.edi | jq '.messages[0].status'
# → "routed"

curl -s http://localhost:8180/api/v1/zuordnungsermaechtigungen/4012345000023 | jq .
```

Ohne sie wird die 55001 unten in Prüfschritt 60 mit `A05` abgelehnt.

---

## Schritt 5 — Eine UTILMD 55001 einreichen

```bash
curl -s -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary @fixtures/utilmd-55001.edi | jq .
```

Erwartete Antwort:

```json
{
  "accepted": 1,
  "rejected": 0,
  "messages": [{
    "message_type": "UTILMD",
    "pid": 55001,
    "workflow": "gpke-supplier-change",
    "status": "routed",
    "process_id": "...",
    "malo_id": "51238696012"
  }]
}
```

---

## Schritt 6 — Automatische NB-Entscheidung

`processd` erhält das Event `de.mako.process.initiated` über den Fan-out von
`marktd` und durchläuft den Entscheidungsbaum E_0622 (`mako-pruefung`) gegen
die Stammdaten aus Schritt 4.

```bash
# Check the decision log
curl -s http://localhost:8580/api/v1/decisions | jq '.[] | {
  malo_id, decision, antwortcode, decided_at
}'
# → {"malo_id":"51238696012","decision":"Accept","antwortcode":"A51","decided_at":"..."}
```

Mit `[nb] auto_accept = true` (gesetzt in der `processd.toml` der Demo) schickt
`Accept` automatisch `gpke.lieferbeginn.bestaetigen` an `makod`, und `makod`
stellt die ausgehende **UTILMD 55002** (Bestätigung Anmeldung verb. MaLo) in die
Outbox:

```bash
curl -s http://localhost:8000/events | jq '[.[] |
  select(.body.makomessagetype=="UTILMD")
  | {edifact: .body.data.edifact}
]'
```

---

## Schritt 7 — Den automatischen Smoke-Test ausführen

`smoke.sh` führt alles oben Beschriebene aus und prüft jeden Schritt:

```bash
bash smoke.sh
```

`MARKTD_URL` und `WEBHOOK_URL` stehen standardmäßig auf dem, was
`docker-compose.yml` veröffentlicht. Setz eine der beiden auf den leeren String,
um `makod` allein zu testen.

Die Ausgabe endet mit:

```
✓ processd NB auto-responder dispatched bestaetigen → UTILMD 55002 already arrived
✓ POST /api/v1/commands → HTTP 409 (duplicate bestaetigen correctly rejected — AntwortGesendet guard confirmed)
✓ UTILMD 55002 was already verified in step 6c (auto-responder path)
All smoke tests passed.
  Wechselprozess auto-responder: ENABLED
  Flow: UTILMD 55001 → makod → marktd ingest → validate → bestaetigen → UTILMD 55002
```

Das `HTTP 409` ist die **Zustandsprüfung des Workflows**, keine Idempotenz:
`processd` hat den Workflow bereits nach `AntwortGesendet` überführt, ein
zweites `bestaetigen` ist also ein Befehl außer der Reihe und wird als
`invalid_state` abgelehnt.

---

## Schritt 8 — Die APIs erkunden

| Schnittstelle | URL |
|---|---|
| OpenAPI-Dokument von makod | http://localhost:8080/api/v1/openapi.json |
| MCP-Server von makod | http://localhost:8080/mcp |
| OpenAPI-Dokument von marktd | http://localhost:8180/api/v1/openapi.json |
| DLQ-Verwaltung von marktd | http://localhost:8180/admin/fanout/dlq |
| Metriken von marktd | http://localhost:8180/metrics |
| Entscheidungen von processd | http://localhost:8580/api/v1/decisions |
| Freigabe-Warteschlange von processd | http://localhost:8580/api/v1/queue |
| Event-Log des ERP-Webhooks | http://localhost:8000/events |

---

## Anhalten und aufräumen

```bash
docker compose down      # keep database volumes
docker compose down -v   # destroy all volumes (full reset)
```

`docker compose down` behält die Datenbank-Volumes; `docker compose down -v`
löscht alle Volumes und setzt alles vollständig zurück.

---

## Die zweite Demo — EEG-Einspeiseabrechnung

`demos/eeg-billing` zeigt die andere Hälfte der Plattform: gar kein EDIFACT,
dafür der Abrechnungsweg, den ein NB einem Anlagenbetreiber schuldet. `einsd`
hält das Anlagenregister und berechnet die Einspeisevergütung nach § 21 EEG
2023; `edmd` hält die viertelstündliche Einspeisemenge, gegen die abgerechnet
wird.

| Dienst | Port | Aufgabe |
|---|---|---|
| `marktd` | `8180` | Market Data Hub — die MaLo, in die die Anlage einspeist |
| `edmd` | `8380` | Energiedatenmanagement — Viertelstundenwerte, Abrechnungszeiträume |
| `einsd` | `9180` | EEG/KWKG-Abrechnung — Anlagenregister, monatliche Vergütung |

```bash
# from the repo root
just build-demo-eeg   # marktd, edmd, einsd

cd demos/eeg-billing
docker compose up -d
bash smoke.sh
```

`edmd` und `einsd` kopieren aus der vollständigen `builder`-Stage, die jeden
Dienst in einem `cargo build` kompiliert, Iceberg/DataFusion eingeschlossen:
Plane kalt **20–45 Minuten** ein, mit warmem BuildKit-Cache ein paar Minuten.

Der Test legt den Anlagenbetreiber und eine Dachanlage mit 9,8 kWp an, schickt
einen Monat Viertelstundenwerte unter OBIS `1-0:2.8.0`, rechnet den Monat mit
233,568 € ab und prüft das CloudEvent `de.eeg.verguetung.berechnet`, das das
ERP erhält. Der Betrag allein ist kein Rechtsdokument: Im Gutschriftverfahren
(§ 14 Abs. 2 Satz 2 UStG) stellt der Netzbetreiber die Gutschrift aus, also
rendert `einsd` sie als BO4E-`Rechnung`, deren Umsatzsteuer dem erklärten
`ust_status` des Betreibers folgt — die Fixture ist ein Kleinunternehmer (§ 19
UStG) und trägt deshalb 0 % USt.

---

## Die dritte Demo — Order-to-Cash

`demos/o2c` zeigt den Geldweg im Vertrieb in den eigenen Büchern des
Lieferanten — Tarif, Vertrag, Rechnung, Dokument, Forderung — ohne EDIFACT.

| Dienst | Port | Aufgabe |
|---|---|---|
| `productd` | `9080` | Produkt- und Tarifkatalog — die einzige Preisquelle |
| `vertragd` | `9780` | Kunden, Verträge, Kündigungsfristen |
| `billingd` | `9280` | Abrechnungs-Engine für mehrere Produkte (EN 16931) |
| `outputd` | `9880` | Dokumentablage und Versand |
| `accountingd` | `9380` | Massenkontokorrent — das doppelte Hauptbuch |
| `webhook` | `8001` | Empfänger der ERP-CloudEvents, damit dieser Stack neben `demos/nb-stp` läuft |

`marktd`, `edmd` und `makod` fehlen: Der Smoke-Test nennt den Netzbetreiber und
liefert den Zählerstand selbst, und ihre URLs zeigen auf Hostnamen, die nicht
auflösen — eine fallengelassene Überschreibung schlägt also fehl, statt null zu
berechnen.

```bash
# from the repo root
just build-demo-o2c

cd demos/o2c
docker compose up -d
bash smoke.sh
```

Der Test veröffentlicht ein Tarifpreisblatt, legt einen Kunden als **BO4E-
`Geschaeftspartner`** und einen Versorgungsvertrag auf einer Marktlokation an,
rechnet einen Monat gegen diesen Tarif ab, legt das Dokument in `outputd` für
die acht Jahre nach § 147 AO ab, sieht zu, wie die Rechnung in `accountingd` als
Offener Posten ankommt, und schließt ihn mit einem Zahlungsimport.

§ 14 Abs. 4 Nr. 1 UStG macht den vollständigen Namen und die Anschrift des
Leistungsempfängers zum Bestandteil der Rechnung, deshalb liest Schritt 3 den
Empfänger aus **beiden** zurück — der BO4E-`Rechnung` und dem EN-16931-Modell.
Ein flacher Body mit `vorname`/`strasse`/`plz` ist ein `422`, der das Feld
nennt.

`just test-demo-payloads` deserialisiert jeden Request-Body, den die Demos
senden, in den echten Request-Typ, und `just test-demo-configs` lädt jede
`*.toml`, mit der die Stacks starten — ohne Docker, in Sekunden.

Der Smoke-Test rechnet den erwarteten Betrag selbst aus: 20 ct/Tag × 31 Tage,
32 ct/kWh × 250 kWh, die Stromsteuer nach § 3 StromStG mit 2,05 ct/kWh und ein
Netznutzungsentgelt von 8,00 ct/kWh aus der `grid`-Überschreibung, jede Zeile
auf den Cent gerundet — 111,33 € netto, 132,48 € brutto bei 19 % USt.

---

## Aus dem Quellcode — ein Dienst, keine Images

Die Demos laufen auf gebauten Images. Wenn du an einem Dienst arbeitest, starte
ihn stattdessen aus dem Checkout; dafür brauchst du Rust (von `rustup` aus
`rust-toolchain.toml` installiert), [`just`](https://github.com/casey/just) und
Docker für PostgreSQL:

```bash
just dev marktd
curl -s http://localhost:8180/health/ready
```

`just dev` startet PostgreSQL aus `dev/docker-compose.yml`, wenn es nicht läuft,
und startet den Dienst mit `dev/config/marktd.toml`; jeder Dienst hat eine
solche Datei, auf seinem Port aus dem [Dienste-Index](@/docs/services/_index.md).
`makod` braucht keine Datenbank — sein Dev-Event-Store liegt im Speicher —, und
`agentd` startet zuletzt, weil es sich beim Start mit dem MCP-Endpunkt jedes
anderen Dienstes verbindet.

Ein Konfigurationsproblem wird auf einmal gemeldet — die gelesene Datei, das
Präfix der Umgebungsvariablen (`MARKTD_`, verschachtelte Schlüssel mit `__`
verbunden) und jeder fehlende, unbekannte oder ungültige Schlüssel. Zwei Flags
helfen vor dem ersten Start:

```bash
cargo run -p marktd -- --help          # usage and config conventions, reads no config
cargo run -p marktd -- --print-config  # the merged file + environment, secrets redacted
cargo run -p marktd -- --check-config  # every finding with its path; exit 1 on a blocking one
```

`--help` zeigt Aufruf und Konfigurationskonventionen und liest keine
Konfiguration; `--print-config` gibt Datei und Umgebung zusammengeführt aus,
Geheimnisse geschwärzt; `--check-config` nennt jeden Befund mit seinem Pfad und
endet bei einem blockierenden mit Exit-Code 1.

`just smoke-dev` startet jeden Dienst auf diese Weise und wartet auf
`/health/ready`.
[CONTRIBUTING.md](https://github.com/hupe1980/mako/blob/main/CONTRIBUTING.md)
beschreibt das Bauen, das Testen eines einzelnen Crates und das Ausführen des
Gates.

---

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
| Das SDK mako-service | [mako-service README](https://github.com/hupe1980/mako/tree/main/crates/mako-service) |
