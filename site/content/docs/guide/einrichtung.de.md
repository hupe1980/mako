+++
title = "Einrichtung"
description = "Prüfe die Konfiguration einer Installation, bevor sie startet: jeder Befund jedes Daemons, mit Pfad und Quelle, im Terminal und über HTTP."
weight = 4

[extra]
translated_from = "content/docs/guide/einrichtung.md"
source_sha256 = "68c3550bdd1fa6949da5ebf78084b5d11d500591c1c381f5d1b18218176a8f41"

+++
Jeder Daemon prüft seine eigene Konfiguration und meldet **jeden** Befund,
nicht nur den ersten. Jeder Befund nennt, was falsch ist, den Pfad der
Einstellung in der Konfigurationsdatei und — wo ein Dokument die Regel
festlegt — das Dokument und das Kapitel. Ein blockierender Befund verhindert
den Start; eine Warnung wird bei jedem Start geloggt. Der Start, der
Prüfbefehl und die HTTP-Route lesen dieselbe Liste und können sich deshalb
nicht widersprechen.

## Die Checkliste

```bash
just setup
```

baut jeden Daemon mit einer Konfiguration unter `deploy/demo/`, führt ihn mit
`--check-config --json` gegen sie aus und fasst die Berichte zu einer Checkliste
zusammen. Der Befehl endet mit einem Fehlercode, solange etwas rot ist oder ein
Daemon nicht gefragt werden konnte.

Gegen eine laufende Installation fragst du makod, das die Berichte der Daemons
aus seiner Tabelle `[[einrichtung.dienst]]` zusammenfasst:

```bash
just setup --url https://makod.example:8080 --token-env MAKOD_TOKEN
```

| Punkt | Frage |
|---|---|
| `identitaeten` | Jede Rolle hat genau eine MP-ID je Sparte, mit der Codenummer, die sie verlangt (`99…` Strom, `98…` Gas) |
| `zertifikate` | Jedes AS4-Zertifikat ist lesbar, passt zu seinem Schlüssel und läuft nicht demnächst ab |
| `bilanzierung` | Jeder Bilanzkreis ist eindeutig seiner Sparte zugeordnet |
| `zugriff` | Jede Tür ist authentifiziert und autorisiert |
| `erreichbarkeit` | Jeder Daemon und jeder Marktpartner ist erreichbar |
| `formatabdeckung` | Jede geltende Formatversion ist durch ein Profil und einen Adapter abgedeckt |
| `konfiguration` | Jede übrige Einstellung ist gültig |

Ein Punkt ist **grün** ohne Befund, **gelb** mit nur Warnungen, **rot** mit
einem blockierenden Befund und **unbekannt**, wenn ein Daemon nicht gefragt
werden konnte — ein unerreichbarer Daemon ist weder bereit noch fehlerhaft.
Nennen zwei Daemons für dieselbe Rolle und Sparte verschiedene MP-IDs, ist das
ein eigener Befund, den kein einzelner Daemon treffen kann.

`just setup --tabelle` gibt dieselben Berichte als eine Zeile je Daemon aus —
bereit oder blockiert, und wie viele Befunde blockieren oder warnen. `just
doctor` gibt sie aus, nachdem er deinen Rechner für `just dev` geprüft hat.

## Der Vorschlag

Die Einrichtungsbefragung fragt nur, was die Konfiguration ändert: die Rollen
und die MP-ID, unter der jede gespielt wird, die primäre MP-ID, wo es mehrere
gibt, die Bilanzkreise eines Lieferanten je Sparte, die verbundenen
Lieferanten, das AS4-Schlüsselmaterial als Pfade und wo die anderen Daemons
laufen. Die angebotenen Rollen sind der Typ, in den makod `[deployment] rollen`
einliest (`mako_service::einrichtung::befragung::Rolle`). Jede Rolle, die makod
lädt — darunter `bkv-strom` —, nennt also auch die Befragung, und der Vorschlag
behält sie. Aus den Antworten schlägt `mako_service::einrichtung::vorschlag`
eine Datei je Daemon vor:

- jede Zeile markiert gegen das, was die Daemons als geladen melden — neu,
  unverändert oder zu entfernen —, mit der Regel oder dem Dokument, das sie
  erfüllt (`# Quelle:` in der Datei);
- jede Angabe, die niemand gemacht hat, als `# FEHLT:`-Lücke, sodass die Datei
  weiter lesbar bleibt und der Daemon weiter ablehnt, bis die Lücke gefüllt ist;
- kein Geheimnis: Ein Zertifikat, ein Schlüssel oder ein API-Key wird über
  seinen Pfad genannt, und eine Antwort, die Schlüsselmaterial ist, wird
  abgelehnt;
- eine Antwort, die eine Regel verletzt, abgelehnt mit ihrer Quelle — eine
  Gas-Rolle unter einer `99…`-Codenummer, eine Strom-Rolle unter `98…`
  (Identifikatoren AWH V1.2, Kap. 2.2), eine Codenummer für beide Sparten
  (Allgemeine Festlegungen V6.1d, Kap. 2.13).

Dieselbe Funktion beantwortet den **Vorschlag** im Desk, makods
`POST /admin/einrichtung/vorschlag`, das MCP-Tool `einrichtung_vorschlag` und
das Terminal:

```bash
just setup --vorschlag antworten.toml                 # the diff per daemon
just setup --vorschlag antworten.toml --schreiben out # and the files in out/
```

Der erste Befehl zeigt den Unterschied je Daemon, der zweite schreibt zusätzlich
die Dateien nach `out/`.

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

Der Befehl endet mit einem Fehlercode, solange der Vorschlag eine Lücke oder
eine abgelehnte Antwort enthält. Auf einen Server wird nichts geschrieben: Du
spielst jede Datei selbst ein. Im Desk kann ein einziger Satz die Befragung
vorausfüllen; agentd behält eine MP-ID oder einen Bilanzkreis, den es
vorschlägt, nur, wenn der Satz ihn enthält, und jede Antwort wartet darauf, dass
du sie übernimmst.

## Der erste Lauf

```bash
just erstlauf
```

beginnt mit makods Betriebsrahmen allein — abgelehnt, ohne Rolle und ohne
Identität —, beantwortet die Befragung für einen Lieferanten Strom und Gas,
hängt die vorgeschlagene `makod.toml` an, prüft sie auf Bereitschaft, startet
makod, legt die Beispiel-Marktlokation in seinen Cache und probt die
Strom-Anmeldung. Jeder Schritt wird gemessen; der Lauf schlägt bei jedem
fehlgeschlagenen Schritt fehl, oder wenn er länger als 30 Minuten dauert.

## Ein Daemon

| Befehl | Ausgabe |
|---|---|
| `<daemon> --check-config` | Jeder Befund, einer je Zeile; Exit-Code 1 bei einem blockierenden |
| `<daemon> --check-config --json` | Der Bericht, den `GET /admin/einrichtung` ausliefert |
| `<daemon> --print-config` | Die zusammengeführte Konfiguration (Geheimnisse geschwärzt), die Befunde und die Policy |

Eine Konfiguration, die sich nicht laden lässt, meldet jedes Ladeproblem — einen
fehlenden, unbekannten oder ungültigen Schlüssel — als blockierenden Befund an
seinem Pfad. Nichts wird gebunden, geöffnet oder geschrieben.

## Über HTTP

| Route | Cedar-Aktion | Inhalt |
|---|---|---|
| `GET /admin/einrichtung` | `read-config` (makod: `AdminEinrichtung`) | der Bericht des Daemons |
| `GET /api/v1/policy` | `read-policy` (makod: `AdminEinrichtung`) | seine [Betreiber-Policy](@/docs/reference/operator-policy.md) (englisch) |
| `GET /admin/einrichtung/deployment` (makod) | `AdminEinrichtung` | die Checkliste über alle Daemons |

Der Bericht:

```json
{
  "dienst": "processd",
  "version": "0.34.0",
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

`schwere` ist `blockiert` oder `warnung`. `quelle` steht dort, wo ein Dokument
die Regel festlegt, `referenz` dort, wo die Einstellung eine
Betreiber-Policy-Einstellung ist.

## makod

makods Bericht umfasst `[[party]]` (eine MP-ID je Rolle, die Codenummer, die
jede Sparte verlangt), `[deployment] rollen`, die Adapterabdeckung je
Formatversion, das AS4-Schlüsselmaterial und die Zertifikate der Marktpartner,
Authentifizierung und Cedar sowie die Callback- und marktd-Endpunkte. Ein
abgelaufenes Zertifikat blockiert; eines, das innerhalb von
`[as4] zertifikat_warnfenster_tage` abläuft (Standard 30, mindestens 14), warnt
mit seinem Ablaufdatum.

```toml
[[einrichtung.dienst]]
name         = "processd"
url          = "http://processd:8580"
api_key_file = "/run/secrets/processd-read-config"
```

Jeder Eintrag wird mit diesem Schlüssel als Bearer-Token nach
`GET /admin/einrichtung` gefragt; der Schlüssel braucht dort also
`read-config`.

Die Ansicht **Einrichtung** im Desk zeigt dieselbe Checkliste, gelesen über
makods MCP-Tool `einrichtung_checkliste`, und fragt alle 15 Sekunden neu. `deploy/demo/makod.toml` nennt jeden Daemon der Installation, sodass ein
Daemon, der nicht läuft, als unerreichbar erscheint.
