+++
title = "Werkzeuge — EDIFACT-Validator und Rechner für den Energiemarkt"
description = "Kostenlose Werkzeuge für den deutschen Energiemarkt, im Browser: EDIFACT-Nachricht prüfen, Antwortcode und Frist nachschlagen, IDs prüfen, und Preisobergrenze Smart Meter, EEG-Vergütung, Umlagen 2027 und § 14a-Module rechnen."
template = "tools.html"
path = "de/werkzeuge"

[extra]
translated_from = "content/tools.md"
source_sha256 = "7ffe81c826be327d0a54bd058b1103bb79f9edc8b1e327020cdf571b04fea428"
privacy = "Jedes Werkzeug läuft in deinem Browser · nichts verlässt den Tab"
heading = "Werkzeuge"
lede = "Die Regeln, mit denen mako abrechnet, als **kleine Werkzeuge im Browser** — derselbe Code, nach WebAssembly übersetzt. Jede Antwort nennt ihre Quelle."

[[extra.families]]
title = "Validator"
lede = "Für die Marktkommunikation: Nachrichten, Codes, Fristen, Identifikatoren."

[[extra.families.tools]]
page = "@/validator.md"
icon = "✉️"
title = "Nachricht prüfen"
text = "EDIFACT einfügen: Segmente, AHB-Befunde, die Frist und wer antwortet."

[[extra.families.tools]]
page = "@/validator-codes.md"
icon = "🔎"
title = "Antwortcode"
text = "Was A02 bedeutet — in dem Entscheidungsbaum, der ihn veröffentlicht."

[[extra.families.tools]]
page = "@/validator-fristen.md"
icon = "⏱️"
title = "Frist"
text = "Bis wann eine Nachricht beantwortet sein muss, ab ihrem Eingang."

[[extra.families.tools]]
page = "@/validator-ids.md"
icon = "✓"
title = "ID prüfen"
text = "MaLo, MeLo, MP-ID oder EIC: gültig, und welche Prüfziffer sie braucht."

[[extra.families]]
title = "Rechner"
lede = "Für Kundinnen, Kunden und alle, die sie beraten: was das Gesetz festlegt."

[[extra.families.tools]]
page = "@/rechner-pog.md"
icon = "📟"
title = "Smart Meter"
text = "Was ein intelligentes Messsystem dich im Jahr höchstens kostet (§ 30 MsbG)."

[[extra.families.tools]]
page = "@/rechner-eeg.md"
icon = "☀️"
title = "EEG-Satz"
text = "Was eine PV-Anlage je kWh verdient, und bis wann (EEG 2023)."

[[extra.families.tools]]
page = "@/rechner-preise.md"
icon = "📈"
title = "Preis 2027"
text = "Umlagen und Steuern in einer kWh, 2026 gegen 2027."

[[extra.families.tools]]
page = "@/rechner-14a.md"
icon = "🔌"
title = "§ 14a-Modul"
text = "Pauschale oder Arbeitspreis auf 40 %: was mehr spart."
+++
