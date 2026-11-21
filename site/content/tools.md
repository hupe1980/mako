+++
title = "Tools — EDIFACT validator and energy calculators"
description = "Free tools for the German energy market, running in your browser: check an EDIFACT message, look up an Antwortcode or a Frist, verify an ID, and calculate smart-meter caps, EEG rates, 2027 levies and § 14a modules."
template = "tools.html"
path = "tools"

[extra]
privacy = "Every tool runs in your browser · nothing leaves the tab"
heading = "Tools"
lede = "The rules mako settles with, as **small tools in your browser** — the same code, compiled to WebAssembly. Each answer names its source."

[[extra.families]]
title = "Validator"
lede = "For market communication: messages, codes, Fristen, identifiers."

[[extra.families.tools]]
page = "@/validator.md"
icon = "✉️"
title = "Check a message"
text = "Paste an EDIFACT message: segments, AHB findings, its Frist and who answers."

[[extra.families.tools]]
page = "@/validator-codes.md"
icon = "🔎"
title = "Antwortcode"
text = "What A02 means — in the Entscheidungsbaum that publishes it."

[[extra.families.tools]]
page = "@/validator-fristen.md"
icon = "⏱️"
title = "Frist"
text = "When a message must be answered, from the moment it arrived."

[[extra.families.tools]]
page = "@/validator-ids.md"
icon = "✓"
title = "ID check"
text = "MaLo, MeLo, MP-ID or EIC: valid, and which check digit it needs."

[[extra.families]]
title = "Calculators"
lede = "For customers and the people advising them: what the statute fixes."

[[extra.families.tools]]
page = "@/rechner-pog.md"
icon = "📟"
title = "Smart meter"
text = "What an intelligentes Messsystem may cost you per year (§ 30 MsbG)."

[[extra.families.tools]]
page = "@/rechner-eeg.md"
icon = "☀️"
title = "EEG rate"
text = "What a PV system earns per kWh, and until when (EEG 2023)."

[[extra.families.tools]]
page = "@/rechner-preise.md"
icon = "📈"
title = "Price 2027"
text = "The Umlagen and taxes in a kWh, 2026 against 2027."

[[extra.families.tools]]
page = "@/rechner-14a.md"
icon = "🔌"
title = "§ 14a module"
text = "Pauschale or an Arbeitspreis cut to 40 %: which saves more."
+++
