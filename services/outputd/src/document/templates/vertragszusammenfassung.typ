// ─────────────────────────────────────────────────────────────────────────────
// mako — reference Vertragszusammenfassung layout (Textform, § 126b BGB)
//
// A starting point, yours to change — the contract is the export:
//
//     #let render(zusammenfassung) = ...
//
// `zusammenfassung` is the VertragszusammenfassungView described in
// `document::vertragszusammenfassung`.
//
// § 41 Abs. 4 Satz 2 EnWG names what the summary contains. The publish gate
// renders this template against the specimen and checks the page prints each
// item: the supplier's name and Kontaktdaten (Nr. 1), the Verbrauchsstelle
// (Nr. 2), both prices (Nr. 3), the Belieferungsbeginn (Nr. 4), the
// Kündigungsfrist (Nr. 5), the Bonus and the Mindestvertragslaufzeit (Nr. 6).
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

#let render(zusammenfassung) = {
  let abs = zusammenfassung.absender
  let emp = zusammenfassung.empfaenger

  // The form of address: `anrede` is outputd's `[policy] anrede`, "sie" or
  // "du", set on every view. `form[Sie text][du text]` picks the phrase.
  let du = zusammenfassung.anrede == "du"
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
    text(size: 14pt, weight: "bold")[Vertragszusammenfassung],
    table(
      columns: 2,
      stroke: none,
      inset: (x: 0pt, y: 1.5pt),
      column-gutter: 10pt,
      align: (right, right),
      [Datum], [#date(zusammenfassung.datum)],
      [Vertrag], [#zusammenfassung.vertragsnummer],
    ),
  )
  v(4mm)
  [Die wichtigsten Bedingungen #form[Ihres][deines] Vertrags#opt(zusammenfassung.sparte, prefix: " (", suffix: ")") auf einen Blick:]
  v(3mm)

  table(
    columns: (auto, 1fr),
    stroke: none,
    inset: (x: 0pt, y: 3pt),
    column-gutter: 12pt,
    [*#form[Ihr][Dein] Lieferant*], [
      #opt(abs.name) \
      #opt(abs.line1)#opt(abs.post_code, prefix: ", ") #opt(abs.city) \
      #opt(abs.phone, prefix: "Telefon ")#opt(abs.email, prefix: " · ")
    ],
    [*Verbrauchsstelle*], [#zusammenfassung.verbrauchsstelle#opt(zusammenfassung.malo_id, prefix: " (Marktlokation ", suffix: ")")],
    [*Voraussichtlicher Belieferungsbeginn*], [#date(zusammenfassung.belieferungsbeginn)],
    [*Kündigungsfrist*], [#zusammenfassung.kuendigungsfrist],
    ..if zusammenfassung.mindestvertragslaufzeit != none {
      ([*Mindestvertragslaufzeit*], [#zusammenfassung.mindestvertragslaufzeit])
    } else { () },
    ..if zusammenfassung.bonus != none {
      ([*Bonus*], [#zusammenfassung.bonus])
    } else { () },
  )
  v(3mm)
  [*Geltende Preise*]
  table(
    columns: (1fr, auto, auto),
    align: (left, left, right),
    stroke: none,
    inset: (x: 4pt, y: 4pt),
    fill: (_, y) => if y == 0 { luma(240) },
    table.header([*Preisbestandteil*], [*Einheit*], [*Preis*]),
    table.hline(stroke: 0.6pt),
    ..zusammenfassung.preise.map(p => ([#p.bezeichnung], [#p.einheit], [#money(p.wert)])).flatten(),
    table.hline(stroke: 0.6pt),
  )
  text(size: 8pt, fill: luma(80))[Alle Preise inklusive Umsatzsteuer.]

  v(6mm)
  [#form[Mit freundlichen Grüßen][Viele Grüße] \ #opt(abs.name)#opt(abs.contact_name, prefix: " — ")]
}
