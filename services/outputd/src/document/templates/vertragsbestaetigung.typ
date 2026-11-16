// ─────────────────────────────────────────────────────────────────────────────
// mako — reference Vertragsbestätigung layout (Textform, § 126b BGB)
//
// A starting point, yours to change — the contract is the export:
//
//     #let render(bestaetigung) = ...
//
// `bestaetigung` is the VertragsbestaetigungView described in
// `document::vertragsbestaetigung`.
//
// § 312f Abs. 2 BGB owes the consumer this confirmation on a durable medium
// before supply begins, with the Art. 246a EGBGB information. The publish gate
// renders the specimen and checks the page prints the declarant, the
// Vertragsnummer, the date of the Vertragsschluss, the product, both prices,
// the Belieferungsbeginn, the Laufzeit, the Kündigungsfrist, and the
// Widerrufsbelehrung: its Frist, its text, the Muster-Widerrufsformular and the
// Wertersatz notice (Art. 246a § 1 Abs. 2 Satz 1 Nr. 1 and 3 EGBGB).
//
// Helpers are duplicated from the other templates on purpose: a template is a
// single self-contained file (the renderer serves it no imports), so every
// template carries its own formatting. Change one, check the others.
// ─────────────────────────────────────────────────────────────────────────────

#let group(digits) = {
  let out = ""
  let n = digits.len()
  for (i, c) in digits.clusters().enumerate() {
    if i > 0 and calc.rem(n - i, 3) == 0 { out = out + "." }
    out = out + c
  }
  out
}

#let num(value, min: 0) = {
  if value == none { return "" }
  let s = str(value)
  let negative = s.starts-with("-")
  if negative { s = s.slice(1) }
  let parts = s.split(".")
  let whole = parts.at(0)
  let frac = if parts.len() > 1 { parts.at(1) } else { "" }
  while frac.len() < min { frac = frac + "0" }
  let sign = if negative { sym.minus } else { "" }
  sign + group(whole) + if frac.len() > 0 { "," + frac } else { "" }
}

#let money(value) = num(value, min: 2)

#let date(iso) = {
  if iso == none { return "" }
  let p = str(iso).split("-")
  if p.len() != 3 { return str(iso) }
  p.at(2) + "." + p.at(1) + "." + p.at(0)
}

#let datetime(iso) = {
  // ISO 8601 date-time, already in German local time: `2026-11-29T00:03:37+01:00`.
  if iso == none { return "" }
  let s = str(iso)
  if s.len() < 16 { return s }
  date(s.slice(0, 10)) + ", " + s.slice(11, 16) + " Uhr"
}

#let opt(value, prefix: "", suffix: "") = {
  if value == none or value == "" { "" } else { prefix + str(value) + suffix }
}

#let render(bestaetigung) = {
  let abs = bestaetigung.absender
  let emp = bestaetigung.empfaenger
  let wr = bestaetigung.widerruf

  // The form of address: `anrede` is outputd's `[policy] anrede`, "sie" or
  // "du", set on every view. `form[Sie text][du text]` picks the phrase.
  let du = bestaetigung.anrede == "du"
  let form(sie, du-text) = if du { du-text } else { sie }

  set text(font: "Libertinus Serif", size: 10pt, lang: "de", hyphenate: false)
  set par(justify: false, leading: 0.62em)
  set page(
    paper: "a4",
    margin: (left: 25mm, right: 20mm, top: 20mm, bottom: 28mm),
    footer: context {
      set text(size: 7.5pt, fill: luma(90))
      line(length: 100%, stroke: 0.4pt + luma(180))
      v(2pt)
      grid(
        columns: (1fr, 1fr, auto),
        align: (left, center, right),
        [
          #opt(abs.name) \
          #opt(abs.line1)#opt(abs.post_code, prefix: ", ") #opt(abs.city)
        ],
        [
          #opt(abs.vat_id, prefix: "USt-IdNr. ") \
          #opt(abs.phone)#opt(abs.email, prefix: " · ")
        ],
        [Seite #counter(page).display() von #counter(page).final().first()],
      )
    },
  )

  align(right)[#text(size: 13pt, weight: "bold")[#opt(abs.name)]]
  v(2mm)

  block(height: 40mm)[
    #text(size: 7pt)[#opt(abs.name) · #opt(abs.line1) · #opt(abs.post_code) #opt(abs.city)]
    #v(4mm)
    #opt(emp.name) \
    #opt(emp.line1) \
    #opt(emp.post_code) #opt(emp.city)
    #if emp.country not in (none, "", "DE") [ \ #emp.country ]
  ]

  grid(
    columns: (1fr, auto),
    align: (left + bottom, right),
    text(size: 14pt, weight: "bold")[#form[Ihre][Deine] Vertragsbestätigung],
    table(
      columns: 2,
      stroke: none,
      inset: (x: 0pt, y: 1.5pt),
      column-gutter: 10pt,
      align: (right, right),
      [Datum], [#date(bestaetigung.datum)],
      [Vertrag], [#bestaetigung.vertragsnummer],
    ),
  )
  v(4mm)
  form[Sehr geehrte Damen und Herren,][Hallo,]
  v(2mm)
  [wir bestätigen #form[Ihnen][dir] den am #datetime(bestaetigung.vertragsschluss_am) geschlossenen Vertrag:]
  v(3mm)

  table(
    columns: (auto, 1fr),
    stroke: none,
    inset: (x: 0pt, y: 3pt),
    column-gutter: 12pt,
    [*Produkt*], [#bestaetigung.produkt#opt(bestaetigung.sparte, prefix: " (", suffix: ")")],
    [*Verbrauchsstelle*], [#bestaetigung.verbrauchsstelle#opt(bestaetigung.malo_id, prefix: " (Marktlokation ", suffix: ")")],
    [*Belieferungsbeginn*], [#date(bestaetigung.belieferungsbeginn)],
    [*Laufzeit*], [#bestaetigung.laufzeit],
    [*Kündigungsfrist*], [#bestaetigung.kuendigungsfrist],
  )
  v(3mm)
  table(
    columns: (1fr, auto, auto),
    align: (left, left, right),
    stroke: none,
    inset: (x: 4pt, y: 4pt),
    fill: (_, y) => if y == 0 { luma(240) },
    table.header([*Preisbestandteil*], [*Einheit*], [*Preis*]),
    table.hline(stroke: 0.6pt),
    ..bestaetigung.preise.map(p => ([#p.bezeichnung], [#p.einheit], [#money(p.wert)])).flatten(),
    table.hline(stroke: 0.6pt),
  )
  text(size: 8pt, fill: luma(80))[Alle Preise inklusive Umsatzsteuer.]

  if bestaetigung.lieferung_in_widerrufsfrist_verlangt {
    v(2mm)
    form[Sie haben ausdrücklich verlangt, dass die Belieferung vor Ablauf der Widerrufsfrist beginnt.][Du hast ausdrücklich verlangt, dass die Belieferung vor Ablauf der Widerrufsfrist beginnt.]
  }

  // Art. 246a § 1 Abs. 2 Satz 1 Nr. 1 and 3 EGBGB — set apart, because a
  // Belehrung the consumer does not find has not informed them. It is printed
  // as the producer sends it, in „Sie“ whatever `anrede` says: the statutory
  // Muster (Art. 246a § 1 Abs. 2 EGBGB) keeps its effect only as published.
  v(5mm)
  block(
    width: 100%,
    inset: 8pt,
    stroke: 0.8pt + luma(60),
    [
      *Widerrufsbelehrung* — Widerrufsfrist: #wr.frist \
      #wr.belehrung_text
      #if wr.widerrufsfunktion != none [ \ #wr.widerrufsfunktion ]
      \ *Wertersatz* \
      #wr.wertersatz_hinweis
    ],
  )
  v(4mm)
  block(
    width: 100%,
    inset: 8pt,
    stroke: (dash: "dashed", paint: luma(90)),
    [
      *Muster-Widerrufsformular* \
      #wr.muster_widerrufsformular
    ],
  )

  v(6mm)
  [#form[Mit freundlichen Grüßen][Viele Grüße] \ #opt(abs.name)#opt(abs.contact_name, prefix: " — ")]
}
