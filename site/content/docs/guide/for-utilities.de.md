+++
title = "Für Stadtwerke"
description = "mako für dein Stadtwerk: welche Aufgaben es übernimmt, welche Marktrollen es spielt, wie du es ohne Risiko prüfst und was der Betrieb braucht."
weight = 1

[extra]
translated_from = "content/docs/guide/for-utilities.md"
source_sha256 = "8697da7f29254299470617512572981e9d97e163bc1270c2b557d5ab324c0a9d"
+++
Ein Stadtwerk spielt mehrere Marktrollen zugleich — Netzbetreiber, Lieferant,
Messstellenbetreiber —, und jede davon schuldet ihren Marktpartnern regulierte
Nachrichten innerhalb einer Frist. Diese Seite ist für dich, wenn du
entscheidest, ob mako diese Arbeit tragen kann: was es abdeckt, wie du es mit
deinen eigenen Daten ausprobierst, ohne etwas zu versenden, und was eine
Installation braucht.

> [!WARNING]
> mako ist experimentell und vor 1.0. Schnittstellen ändern sich zwischen
> Versionen, und ein produktiver Betrieb braucht deine eigenen Tests gegen deine
> eigenen Marktpartner.

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

Jede Rolle wird unter genau einer MP-ID je Sparte gespielt — „Für jede Sparte
(Gas bzw. Strom) und jede Rolle hat ein Markteilnehmer genau eine MP-ID"
(Identifikatoren AWH V1.2 Kap. 2.1). Wer die Prozesse welcher Rolle lesen darf,
regelt die Cedar-Policy deiner Installation. Welche Module jeder Wert
registriert, steht in [makod — Deployment roles](@/docs/services/makod.md#deployment-roles).

## Ohne Risiko prüfen

Drei Schritte, von denen keiner eine Nachricht an einen Marktpartner sendet:

1. **Eine Demo starten.** Jede Demo ist ein `docker compose`-Stack mit einem
   `smoke.sh`, das das Ergebnis prüft — den Antwortcode, den Betrag, die
   Forderung auf den Cent. `nb-stp` beantwortet eine UTILMD-55001-Anmeldung mit
   55002, `eeg-billing` rechnet einen Monat Einspeisung ab, `o2c` führt einen
   Tarif bis zur bezahlten Rechnung. [Loslegen](@/docs/guide/getting-started.de.md)
   geht die erste Schritt für Schritt durch.
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

## Umstieg: ein Schattenbetrieb

Der Weg, eine laufende Marktrolle auf mako umzustellen, ist ein
Schattenbetrieb: mako empfängt dieselben eingehenden Nachrichten wie dein
produktives System, entscheidet sie, und seine Antworten werden mit den
tatsächlich gesendeten verglichen — bevor mako selbst etwas sendet. Einen
Schattenmodus hat mako heute nicht: Es spiegelt nicht den Verkehr eines
bestehenden Systems und vergleicht keine Antworten. Was es für diesen Weg gibt,
ist das, was die Prüfung oben nutzt — die Konfigurationsprüfung, den Probelauf
und [makotest](@/docs/reference/makotest.md) mit Marktpartnern, die innerhalb
der veröffentlichten Fristen antworten.

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
- **Veröffentlichte MP-IDs.** In produktiven Nachrichten darf nur eine im
  Codeverzeichnis veröffentlichte BDEW- oder DVGW-Codenummer stehen (Allgemeine
  Festlegungen V6.1d Kap. 2.14).
- **Deine eigenen Tests.** Gegen deine eigenen Marktpartner, vor der ersten
  echten Nachricht.
