+++
title = "Für Stadtwerke"
description = "mako für dein Stadtwerk: welche Aufgaben es übernimmt, welche Marktrollen es spielt, wie du es ohne Risiko prüfst und was der Betrieb braucht."
weight = 1

[extra]
translated_from = "content/docs/guide/for-utilities.md"
source_sha256 = "3875cb271643825ff9478563256eb16358268186b4db5d88b128e0c57c78c29b"

+++
Ein Stadtwerk spielt mehrere Marktrollen zugleich — Netzbetreiber, Lieferant,
Messstellenbetreiber —, und jede davon schuldet ihren Marktpartnern regulierte
Nachrichten innerhalb einer Frist. Diese Seite ist für dich, wenn du
entscheidest, ob mako diese Arbeit tragen kann: was es abdeckt, wie du es mit
deinen eigenen Daten ausprobierst, ohne etwas zu versenden, und was eine
Installation braucht.

> [!WARNING]
> mako ist in Entwicklung. Die erste offizielle Plattformversion steht noch aus. Schnittstellen können sich ändern. Teste den Austausch mit deinen Marktpartnern vor dem Produktiveinsatz.

## Was mako abdeckt

| Aufgabe | In mako | Einstieg (englisch) |
|---|---|---|
| Gateway der Marktkommunikation — AS4-Transport, EDIFACT lesen und bauen, MIG-/AHB-Prüfung, CONTRL und APERAK | `makod` | [makod](@/docs/services/makod.md) |
| Die Prozesse selbst — GPKE, WiM, GeLi Gas, MaBiS, beantwortet nach den veröffentlichten Entscheidungsbäumen innerhalb ihrer Frist | `makod` · `processd` | [processd](@/docs/services/processd.md) |
| Stammdaten — MaLo/MeLo, Versorgungsstatus, Partner- und Netzverzeichnisse | `marktd` | [marktd](@/docs/services/marktd.md) |
| Messwesen — MSCONS, Zählerstandsgang, Ersatzwerte, Lebenszyklus des Smart-Meter-Gateways | `edmd` | [edmd](@/docs/services/edmd.md) |
| Netzabrechnung — NNE, Mehr-/Mindermengen, MaBiS-Summenzeitreihen; EEG-/KWKG-Einspeisung | `netzbilanzd` · `einsd` | [netzbilanzd](@/docs/services/netzbilanzd.md) |
| Vertrieb — Verträge, Tarife, Abrechnung, E-Rechnungen nach EN 16931, Hauptbuch, Mahnwesen, Dokumente, Kundenportal | `vertragd` · `productd` · `billingd` · `invoicd` · `accountingd` · `outputd` · `portald` | [billingd](@/docs/services/billingd.md) |
| Die Arbeitsoberfläche deiner Sachbearbeitung — jede Frist in Berliner Zeit, jede Freigabe in einem Eingang | der Desk | [Der Desk](@/docs/guide/desk/index.de.md) |

Jeder Dienst hält seine eigenen Daten und spricht mit den anderen über
typisiertes REST und CloudEvents. Eine Installation kann deshalb zuerst Gateway
und Prozessebene übernehmen und ihre bestehende Abrechnung behalten — oder
umgekehrt. Die [Übersicht der Dienste](@/docs/services/_index.md) nennt jeden
Dienst mit seiner Rolle.

## Welche Marktrollen

`makod` registriert die Prozesse der Rollen, die `[deployment] rollen` erklärt,
und keine anderen:

| Rolle | `rollen`-Werte |
|---|---|
| Netzbetreiber (NB) | `nb-strom` · `nb-gas` |
| Lieferant (LF) | `lf-strom` · `lf-gas` |
| Messstellenbetreiber (MSB) | `msb-strom` · `msb-gas` |
| Energieserviceanbieter (ESA) | `esa-strom` |
| Bilanzkreisverantwortlicher (BKV) | `bkv-strom` |
| Einsatzverantwortlicher (EIV, Redispatch 2.0) | `eiv-strom` |

Jede Rolle wird unter genau einer MP-ID je Sparte gespielt — „Für jede Sparte
(Gas bzw. Strom) und jede Rolle hat ein Markteilnehmer genau eine MP-ID"
(Identifikatoren AWH V1.2 Kap. 2.1). Wer die Prozesse welcher Rolle lesen darf,
regelt die Cedar-Policy deiner Installation. Welche Module jeder Wert
registriert, steht in [makod — Deployment roles](@/docs/services/makod.md#deployment-roles).

## Ohne Risiko prüfen

Drei Schritte, von denen keiner eine Nachricht an einen Marktpartner sendet:

1. **Die Demo starten.** `just demo` startet jeden Dienst und den Desk in
   Docker, mit einem eingespielten Stadtwerk — eine Anmeldung mit 55002
   beantwortet, eine wartet auf eine Freigabe, ein Klärfall, eine EEG-Anlage
   auf den Cent abgerechnet, ein Haushalt abgerechnet, die Rechnung zugestellt
   und bezahlt —, und `mako-seed --verify` prüft jedes Ergebnis: den
   Antwortcode, den Betrag, die Forderung auf den Cent. Melde dich als eine
   beliebige Persona an und sieh, wie Cedar jeder anders antwortet.
   [Loslegen](@/docs/guide/getting-started.de.md) geht sie Schritt für Schritt
   durch.
2. **Deine Konfiguration prüfen.** `just setup` führt `--check-config` jedes
   Daemons aus und fasst die Befunde zu einer Checkliste zusammen — Identitäten,
   Zertifikate, Bilanzkreise, Zugriff, Erreichbarkeit, Formatabdeckung —, jeden
   mit dem Pfad der Einstellung und seiner Quelle. Die Einrichtungsbefragung
   schlägt die Konfiguration vor, die die Liste grün macht. Siehe
   [Einrichtung](@/docs/guide/einrichtung.de.md).
3. **Eine Nachricht proben.** Der Probelauf im Desk baut eine Anmeldung,
   Abmeldung oder Kündigung durch dieselbe Tür wie eine echte, hält sie gegen
   ihr Profil und die Prüfung vor dem Versand und sendet und speichert nichts.
   Siehe [Der Desk — Eine Nachricht vor dem Produktivstart proben](@/docs/guide/desk/index.de.md#eine-nachricht-vor-dem-produktivstart-proben).

`just erstlauf` führt den zweiten und dritten Schritt von einem leeren `makod`
aus durch und misst jeden.

**Spiele den Verlust einer Empfangsbestätigung durch.** Mit `just recovery-drill` stellst du die Rechnung und das offene Guthaben des Lieferanten wieder her. Prüfe, welcher Nachweis den erneuten Versand freigibt, wenn der Empfänger die erste Nachricht bereits verarbeitet hat. Bei einer Sperre siehst du, welchen Nachweis du vor dem nächsten Versuch brauchst. [Anleitung zur Wiederherstellung (Englisch)](@/docs/reference/recovery.md)

## Einen Schattenbetrieb vorbereiten

Prüfe vor dem Vergleich der Antworten, welche Nachrichten im Mitschnitt angekommen sind. Exportiere für eine Marktrolle und ein Zeitfenster ein abgeschlossenes Journal aus dem Quellsystem. Erstelle unabhängig davon ein Verzeichnis der mitgeschnittenen Dateien. Mit `makotest shadow reconcile` gleichst du jede erfasste Nachricht und ihren Dateiinhalt lokal ab. Du siehst, welche Nachrichten fehlen oder doppelt vorkommen und ob ein Zeitraum nachweislich leer ist. Zum Ausprobieren liegt ein synthetischer Mitschnitt im Repository. [Dateiformat und Beispiel](@/docs/reference/makotest.md#reconcile-a-shadow-capture)

Mit dem synthetischen Beispiel kannst du eine Anmeldung als Strom-NB beantworten und die Ablehnung mit einer unabhängigen Erwartung vergleichen. Von zwei erfassten Anmeldungen ist eine geprüft. [Beispiel ausführen](@/docs/reference/makotest.md#rehearse-a-strom-nb-refusal)

Für eine Probe mit dem Daemon ergänze `--makod-workspace <Verzeichnis>`. Halte den Mitschnitt und den privaten Probelauf getrennt. Nach einem Neustart liest du dieselbe erfasste Antwort. Die sendende Installation behält die Verantwortung für offene Quittungen. Gleiche vor einer Übergabe auch alle seit dem Mitschnitt hinzugekommenen Nachrichten und Buchungen ab. [Probelauf mit makod](@/docs/services/makod.md#shadow-capture)

Öffne in der isolierten Prüfinstallation als Netzbetreiber **System → Schattenbetrieb**. Jeder erfasste Eingang hat eine eigene Zeile. Im Beispieldatensatz ist eine von zwei Antworten geprüft. Wähle den zweiten Eingang, um den fehlenden Vergleich zu sehen. Mit Schreibberechtigung kannst du eine Abweichung mit Belegreferenz und Begründung bestätigen. Ändern sich die Belege, prüfst du den neuen Stand. [Prüfinstallation einrichten (englisch)](@/docs/services/processd.md#isolated-shadow-review)

## Was der Betrieb braucht

- **Container.** Jeder Dienst ist ein Image auf GHCR, für mehrere Architekturen,
  mit seiner Version getaggt; in Produktion pinnst du eine Version
  ([makod — Docker Deployment](@/docs/services/makod.md#docker-deployment)).
- **PostgreSQL.** Ein Schema je Dienst. `makod` hält seinen Prozesszustand
  stattdessen in einem eigenen Speicher auf einem Volume.
- **Identität und Zugriff.** Ein OIDC-Identity-Provider für jede Route und den
  Desk, und Cedar-Policies, die entscheiden, wer was lesen und tun darf.
- **AS4-Schlüsselmaterial.** TLS-, Signatur- und Verschlüsselungszertifikate
  einer SM-PKI-CA mit deiner MP-ID im Subject, für jede Rolle, die du spielst
  ([makod — AS4](@/docs/services/makod.md#as4-as4-ebms3-inbound-and-outbound)).
- **Veröffentlichte MP-IDs.** Verwende im Strombereich eine BDEW-Codenummer oder GLN und im Gasbereich eine DVGW-Codenummer oder GLN, jeweils veröffentlicht im zugehörigen Verzeichnis. [BDEW, Allgemeine Festlegungen 6.1d, Kap. 2.13–2.14, S. 17–18](https://www.bdew-mako.de/api/downloadFile/12145)
- **Deine eigenen Tests.** Gegen deine eigenen Marktpartner, vor der ersten
  echten Nachricht.
