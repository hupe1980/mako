+++
title = "mako"
description = "Open-Source-Plattform in Rust für die deutsche Marktkommunikation: BDEW-EDIFACT, ereignisbasierte MaKo-Prozesse, AS4, Abrechnung und Fakturierung."
template = "home.html"

[extra]
translated_from = "templates/index.html"
source_sha256 = "c6ac9f2bac286a8b0b84ae04a07540f7f9d1026f6ea57e0aaaf04c97eedc527d"

eyebrow = "Rust · MIT oder Apache-2.0 · BDEW EDI@Energy"
hero_title = "Marktkommunikation für Strom und Gas,<br>von rohem EDIFACT bis zur Produktion — KI-first, in Rust."
hero_sub = """Jeder Marktpartner im deutschen Strom- und Gasmarkt muss mit seinen Gegenübern regulierte \
Nachrichten austauschen — die *Marktkommunikation*, kurz MaKo. Das sind einige hundert \
vorgeschriebene Prozesse über EDIFACT und AS4, jeder mit veröffentlichtem Entscheidungsbaum \
und einer Frist. **mako setzt sie um**: Die BDEW-Formate werden als Daten gelesen, jeder Prozess \
läuft als ereignisbasierter Workflow, und Abrechnung, Fakturierung und Hauptbuch stehen auf \
denselben Ereignissen. **KI-first**: Die Dienste sprechen MCP, und der Desk für deine \
Sachbearbeitung liest dieselben typisierten Werkzeuge, die auch die Agenten bekommen — ein \
Modell liest, erklärt und schlägt vor; jede Frist, jeder Betrag und jede Entscheidung wird von \
Code berechnet."""

scope_label = "Umfang"
scope_note = """Wie viele Prüfidentifikatoren mako importiert und routet, wie viele Workflows, \
Nachrichtentypen und Dienste es gibt, steht mit seiner Herleitung auf der \
[englischen Startseite](@/_index.md) — dort hält der Build jede Zahl gegen den Code fest, \
und eine Version, die eine davon verkleinern würde, baut nicht."""

roles_title = "Wer das betreibt"
roles_lead = """Jede MaKo-Nachricht nennt an beiden Enden eine Marktrolle. Diese Rollen tragen die \
Massenprozesse und ihre Bilanzierung. Eine Installation erklärt, welche Rollen sie spielt, und \
registriert nur deren Prozesse; welcher Nutzer die Prozesse welcher Rolle lesen darf, regelt \
eine Cedar-Policy."""

cards_title = "Was du bekommst"
why_title = "Gebaut für das regulatorische Tempo"
why_lead = """Die Marktkommunikation ändert sich schnell — LFW24, § 14a, dynamische Tarife nach \
§ 41a, § 42b Gebäudeversorgung und § 42c Energy Sharing, die API-Webdienste neben EDIFACT, \
der MaBiS-Hub. mako trennt die Fachlogik bewusst von Transport und Format: Wenn sich der Markt \
bewegt, zieht die Plattform mit einem Import nach — nicht mit einer Neuentwicklung."""

[[extra.cta]]
label = "Loslegen"
path = "@/docs/guide/getting-started.md"
lang = "de"

[[extra.cta]]
label = "Für Stadtwerke"
path = "@/docs/guide/for-utilities.md"
lang = "de"

[[extra.cta]]
label = "Zur Doku"
path = "@/docs/_index.md"
lang = "de"

[[extra.roles]]
abbr = "NB"
name = "Netzbetreiber"
text = """Beantwortet An- und Abmeldungen von Lieferanten nach den veröffentlichten \
Entscheidungsbäumen, rechnet die Netznutzung (NNE) ab, rechnet Mehr-/Mindermengen ab, \
meldet MaBiS-Summenzeitreihen und führt Sperraufträge aus."""
path = "@/docs/services/processd.md"
lang = "en"
link = "processd · netzbilanzd →"

[[extra.roles]]
abbr = "LF"
name = "Lieferant"
text = """Gewinnt und wechselt Kunden, bepreist Verträge gegen den Tarifkatalog, rechnet den \
Verbrauch ab, stellt E-Rechnungen nach EN 16931 aus (XRechnung, ZUGFeRD) und zieht per SEPA \
ein."""
path = "@/docs/services/billingd.md"
lang = "en"
link = "billingd · vertragd · invoicd →"

[[extra.roles]]
abbr = "MSB"
name = "Messstellenbetreiber"
text = """Führt die WiM-Geräteprozesse — Einbau, Wechsel des Messstellenbetreibers, \
Störungen —, verwaltet den Lebenszyklus des Smart-Meter-Gateways und liefert Messwerte an \
alle, die sie bekommen dürfen."""
path = "@/docs/services/edmd.md"
lang = "en"
link = "edmd · makod →"

[[extra.roles]]
abbr = "ESA"
name = "Energieserviceanbieter"
text = """Bestellt Messwerte über die Wertebestellung nach § 34 MsbG, bietet eigene \
Messprodukte an und rechnet die Leistung gegen das Angebot ab, das das Gegenüber angenommen \
hat."""
path = "@/docs/services/makod.md"
lang = "en"
link = "makod — ESA-Nachrichten →"

[[extra.roles]]
abbr = "BKV"
name = "Bilanzkreisverantwortlicher"
text = """Empfängt seine Bilanzkreissummenzeitreihen vom BIKO, beantwortet sie mit einer \
Prüfmitteilung, nimmt den Datenstatus entgegen und fordert unter MaBiS seine \
Bilanzkreiszuordnungsliste und die Clearingliste BAS an."""
path = "@/docs/services/makod.md"
lang = "en"
link = "makod — MaBiS-Nachrichten →"

[[extra.cards]]
title = "Protokoll und Prozess"
text = """Alle EDI@Energy-EDIFACT-Typen und die DVGW-Gasformate lesen, prüfen und bauen, und \
jeden gerouteten MaKo-Prozess als ereignisbasierten, wiederholbar abspielbaren Workflow über \
AS4/ebMS3 ausführen — BDEW-AS4-Profil, signiert und verschlüsselt, signierte Quittungen, \
Wiederholung mit Duplikaterkennung."""
path = "@/docs/services/makod.md"
lang = "en"
link = "makod · processd →"

[[extra.cards]]
title = "Marktdaten"
text = """Ein typisierter BO4E-Marktdaten-Hub — MaLo/MeLo, Verträge, Versorgungsstatus, \
Gerätekonfiguration, Netzbetreiberverzeichnis — mit dauerhafter Verteilung per CloudEvents \
und OIDC/JWT auf jeder Route."""
path = "@/docs/services/marktd.md"
lang = "en"
link = "marktd · vertragd · productd →"

[[extra.cards]]
title = "Messwesen und Abrechnung"
text = """Viertelstündliche Messwerte mit Ersatzwerten nach § 60 MsbG und dem \
SMGW-Lebenszyklus; rollenneutrale Netzabrechnung (NNE/MMM/MSB), EEG-/KWKG-Einspeisung und \
MaBiS-Summenzeitreihen — jede Position mit einem Rechenweg, der seine Rechtsgrundlage nennt."""
path = "@/docs/services/edmd.md"
lang = "en"
link = "edmd · netzbilanzd · einsd →"

[[extra.cards]]
title = "Fakturierung und Hauptbuch"
text = """Abrechnung für Endkunden mit E-Rechnung nach EN 16931 (XRechnung, ZUGFeRD), \
SEPA-Einzug und Rücklastschriften, ein hashverkettetes FI-CA-Hauptbuch mit Periodensiegeln \
nach § 147 AO und Merkle-Inklusionsbeweisen, und die Dokumente selbst — aus deinen Vorlagen \
gerendert und mit Nachweis je Kanal zugestellt."""
path = "@/docs/services/billingd.md"
lang = "en"
link = "billingd · invoicd · accountingd · outputd →"

[[extra.cards]]
title = "Betrieb und Aufsicht"
text = """Der [Desk](@/docs/guide/desk/index.de.md) für deine Sachbearbeitung: jede Frist in \
Berliner Zeit, jeder Antwortcode im Wortlaut des BDEW und ein Assistent, der für jeden Satz \
seine Quelle nennt. Dazu BNetzA-Kennzahlen, ein Kundenportal und eine Agentenebene, deren \
Spezialisten nur beraten: Kein aktives Manifest gewährt ein schreibendes Werkzeug."""
path = "@/docs/guide/desk/index.md"
lang = "de"
link = "Der Desk →"

[[extra.cards]]
title = "Aus Python testbar"
text = """[makotest](@/docs/reference/makotest.md) bindet denselben Rust-Kern, den die \
Plattform ausführt — BDEW-Prüfziffern, den Werktagskalender, die veröffentlichten \
Antwortfristen, EDIFACT-Bau und AHB-Prüfung —, damit Test und Produktion sich nie darüber \
uneinig sind, was „gültig“ heißt und wann eine Frist abläuft."""
path = "@/docs/reference/makotest.md"
lang = "en"
link = "makotest →"

[[extra.why]]
title = "KI-first, der Code entscheidet"
text = "Agenten und Menschen arbeiten auf einer typisierten Werkzeugoberfläche; kein Modell sitzt in einem Rechenweg, und kein Agent versendet eine Marktnachricht."

[[extra.why]]
title = "Prüfbar von Grund auf"
text = "ereignisbasierte Prozesse, wiederherstellbarer Zustand, hashverkettetes Hauptbuch mit Periodensiegeln nach § 147 AO."

[[extra.why]]
title = "Viele Marktrollen, eine Engine"
text = "NB, LF, MSB und ESA antworten nach den veröffentlichten Entscheidungsbäumen, der BKV übernimmt seinen MaBiS-Teil, und eine Installation registriert nur die Prozesse der Rollen, die sie erklärt."

[[extra.why]]
title = "Prüfung, die laut scheitert"
text = "Ein nicht registrierter Prüfidentifikator wird gemeldet, nie stillschweigend angenommen."

[[extra.why]]
title = "Fristen, die zueinander passen"
text = "ein BDEW-Werktagskalender, von der APERAK-Uhr bis zum Freigabefenster eines Agenten."
+++

## In Minuten starten

```bash
# EDIFACT-Übertragungsdatei lesen und prüfen
cargo add edi-energy

# Das ganze System starten, befüllt mit einem Stadtwerk, mit dem Desk
just doctor && just dev

# Aus Python steuern — Generatoren, Simulatoren, pytest-Plugin
pip install makotest
```

[Zur vollständigen Anleitung →](@/docs/guide/getting-started.de.md)

Experimentell, vor 1.0: Schnittstellen können sich zwischen Versionen ändern, und
ein produktiver Betrieb braucht deine eigenen Tests gegen deine eigenen
Marktpartner. Lizenziert unter MIT oder Apache 2.0.
