+++
title = "MaLo-ID prüfen — ist diese ID gültig?"
description = "Eine MaLo-ID, Marktpartner-ID (BDEW, DVGW, GLN), einen EIC oder eine MeLo-ID prüfen: ob die Prüfziffer stimmt, und die richtige, wenn nicht."
template = "validator-tool.html"
path = "de/validator/ids"

[extra]
tool = "ids"
translated_from = "content/validator-ids.md"
source_sha256 = "151e55c2edd905d549f0332079137458376a981a412e80a5164dfa64837074b8"

[extra.ui]
privacy = "Läuft in deinem Browser · nichts verlässt diesen Tab"
no_js = "Dieses Werkzeug läuft in deinem Browser und braucht dafür JavaScript."
no_wasm = "Dieser Browser kann kein WebAssembly ausführen, das dieses Werkzeug braucht."
loading = "Wird geladen …"
load_failed = "Das Werkzeug ließ sich nicht laden"
no_match = "Nichts passt."
title = "Ist diese ID gültig?"
lede = "Füg eine MaLo-ID, Marktpartner-ID, einen EIC oder eine MeLo-ID ein. Die Prüfziffer wird nach dem Verfahren geprüft, das ihre Art vorschreibt — und bei einer falschen steht die richtige daneben."
id_label = "ID"
check_ok = "✓ Die Prüfziffer stimmt."
check_bad = "✗ Die Prüfziffer stimmt nicht."
would_be = "Mit richtiger Prüfziffer:"
mp_checks = "BDEW-Verfahren {bdew} · GS1/GLN-Verfahren {gln}"
assigned_note = "Eine gültige Prüfziffer heißt nicht, dass die Nummer vergeben ist; das sagen die Register von BDEW und DVGW."

[extra.ui.kinds]
malo = "Marktlokations-ID"
mpid_bdew = "Marktpartner-ID · BDEW-Codenummer"
mpid_dvgw = "Marktpartner-ID · DVGW-Codenummer"
mpid_gln = "Marktpartner-ID · GLN"
eic = "EIC (Energy Identification Code)"
melo = "Messlokations-ID"
+++
