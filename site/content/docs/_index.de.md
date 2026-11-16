+++
title = "Dokumentation"
description = "Die deutsche Auswahl der mako-Dokumentation: Einstieg, Einrichtung, der Desk und mako für Stadtwerke. Alles Weitere ist auf Englisch."
sort_by = "weight"
template = "section.html"

[extra]
translated_from = "content/docs/_index.md"
source_sha256 = "1d1471760158167aa29f26d10115fb67a0e6712130dfc093a1008ec5a480b612"
+++

Auf Deutsch gibt es eine Auswahl: den Einstieg, die Einrichtung, den Desk und
die Seite für Stadtwerke. Die Betriebshandbücher der Dienste, die Referenz und
die regulatorische Zuordnung sind auf Englisch.

## Hier anfangen

| Wenn du … | Dann lies |
|---|---|
| entscheiden willst, ob mako für dein Stadtwerk passt | [Für Stadtwerke](@/docs/guide/for-utilities.de.md) |
| es in wenigen Minuten von Ende zu Ende laufen sehen willst | [Loslegen](@/docs/guide/getting-started.de.md) |
| die Konfiguration einer Installation prüfen willst | [Einrichtung](@/docs/guide/einrichtung.de.md) |
| mit der Arbeitsoberfläche deiner Sachbearbeitung arbeiten willst | [Der Desk](@/docs/guide/desk/index.de.md) |
| verstehen willst, wie aus einer Nachricht ein Prozess wird | [Architecture](@/docs/architecture/_index.md) (englisch) |
| einen Dienst produktiv betreiben willst | [Services](@/docs/services/_index.md) (englisch) |
| EDIFACT selbst lesen, prüfen oder bauen willst | [Reference](@/docs/reference/_index.md) (englisch) |
| eine Regel bis zu ihrer Festlegung oder ihrem § zurückverfolgen willst | [Regulatory](@/docs/regulatory/_index.md) (englisch) |
| ein neues BDEW-Formatrelease übernehmen willst | [Release & compliance](@/docs/compliance/_index.md) (englisch) |

## Nach Marktrolle

`makod` registriert die Engine-Module der Rollen, die seine
`[deployment] rollen` erklären, und ein für eine Rolle gebautes `processd` kann
nicht als eine Rolle antworten, für die es nicht gebaut ist. Welcher Nutzer die
Prozesse welcher Rolle lesen darf (§ 6a Abs. 1 EnWG), regelt die Cedar-Policy
der Installation. `makod`, `marktd`, `edmd`, `processd` und `agentd` laufen in
jeder Rolle; die übrigen sind rollenspezifisch.

| Rolle | Läuft zusätzlich | Einstieg (englisch) |
|---|---|---|
| **NB** — Netzbetreiber | `processd` (mit der Sperrauftrags-Warteschlange) · `netzbilanzd` · `einsd` | [processd](@/docs/services/processd.md) — die Dunkelverarbeitung |
| **LF** — Lieferant | `processd` · `vertragd` · `productd` · `billingd` · `invoicd` · `accountingd` · `outputd` · `portald` · `einsd` | [billingd](@/docs/services/billingd.md) — Abrechnung und EN 16931 |
| **MSB** — Messstellenbetreiber | `processd` · `vertragd` | [edmd](@/docs/services/edmd.md) — Messwesen und SMGW-Lebenszyklus |
| **ESA** — Energieserviceanbieter | — | [makod](@/docs/services/makod.md#esa-messages) — die Wertebestellung nach § 34 MsbG |
