+++
title = "MaLo-ID check — is this ID valid?"
description = "Check a MaLo-ID, a Marktpartner-ID (BDEW, DVGW, GLN), an EIC or a MeLo-ID: whether the check digit holds, and the right one if it does not."
template = "validator-tool.html"
path = "validator/ids"

[extra]
tool = "ids"

[extra.ui]
privacy = "Runs in your browser · nothing leaves this tab"
no_js = "This tool runs in your browser, so it needs JavaScript."
no_wasm = "This browser cannot run WebAssembly, which this tool needs."
loading = "Loading …"
load_failed = "The tool did not load"
no_match = "Nothing matches."
title = "Is this ID valid?"
lede = "Paste a MaLo-ID, a Marktpartner-ID, an EIC or a MeLo-ID. The check digit is verified by the procedure its kind prescribes — and for a wrong one, the right digit is shown."
id_label = "ID"
check_ok = "✓ The check digit holds."
check_bad = "✗ The check digit does not hold."
would_be = "With the correct check digit:"
mp_checks = "BDEW procedure {bdew} · GS1/GLN procedure {gln}"
assigned_note = "A valid check digit does not mean the number is assigned; the BDEW and DVGW registers say that."

[extra.ui.kinds]
malo = "Marktlokations-ID"
mpid_bdew = "Marktpartner-ID · BDEW-Codenummer"
mpid_dvgw = "Marktpartner-ID · DVGW-Codenummer"
mpid_gln = "Marktpartner-ID · GLN"
eic = "EIC (Energy Identification Code)"
melo = "Messlokations-ID"
+++
