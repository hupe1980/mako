// ─────────────────────────────────────────────────────────────────────────────
// mako — reference Gebührenbescheid layout
//
// A starting point, yours to change — the contract is the export:
//
//     #let render(bescheid) = ...
//
// `bescheid` is the GebuehrenbescheidView described in
// `document::gebuehrenbescheid`.
//
// The page states what § 157 Abs. 1 AO attaches to a Bescheid: each Gebühr by
// Art and Betrag, who owes it, and a Rechtsbehelfsbelehrung naming the
// Rechtsbehelf, its Frist and the Behörde. The publish gate renders this
// template against the specimen and checks the page prints each of them.
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

#let opt(value, prefix: "", suffix: "") = {
  if value == none or value == "" { "" } else { prefix + str(value) + suffix }
}

#let render(bescheid) = {
  let abs = bescheid.absender
  let emp = bescheid.schuldner

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
    text(size: 14pt, weight: "bold")[Gebührenbescheid],
    table(
      columns: 2,
      stroke: none,
      inset: (x: 0pt, y: 1.5pt),
      column-gutter: 10pt,
      align: (right, right),
      [Datum], [#date(bescheid.datum)],
      [Bescheid-Nr.], [#bescheid.bescheidnummer],
      ..if bescheid.malo_id != none {
        ([Abnahmestelle], [#bescheid.malo_id])
      } else { () },
    ),
  )
  v(4mm)

  // § 157 Abs. 1 Satz 2 AO — who owes it, and each Gebühr by Art and Betrag.
  [Für den Zeitraum #date(bescheid.zeitraum_von) bis #date(bescheid.zeitraum_bis)
   werden gegenüber #opt(emp.name) folgende Gebühren festgesetzt:]
  v(3mm)
  table(
    columns: (1fr, auto, auto, auto),
    align: (left, right, right, right),
    stroke: none,
    inset: (x: 4pt, y: 4pt),
    fill: (_, y) => if y == 0 { luma(240) },
    table.header([*Gebühr*], [*Bemessungsgrundlage*], [*Satz*], [*Betrag EUR*]),
    table.hline(stroke: 0.6pt),
    ..bescheid.positionen.map(p => (
      [#p.art],
      [#p.bemessungsgrundlage],
      [#p.satz],
      [#money(p.betrag)],
    )).flatten(),
    table.hline(stroke: 0.6pt),
    [*Festgesetzt*], [], [], [*#money(bescheid.festgesetzt)*],
  )
  v(3mm)
  [Der Betrag ist bis zum *#date(bescheid.faellig_am)* zu zahlen#opt(bescheid.iban, prefix: " auf das Konto IBAN ").]

  // § 157 Abs. 1 Satz 3 AO — the Rechtsbehelfsbelehrung: which Rechtsbehelf,
  // within which Frist, with which Behörde. Set apart, as the Bescheid's last
  // word before the signature.
  v(5mm)
  block(
    width: 100%,
    inset: 8pt,
    stroke: 0.8pt + luma(60),
    [
      *Rechtsbehelfsbelehrung* \
      Gegen diesen Bescheid kann #bescheid.rechtsbehelf.rechtsbehelf
      #bescheid.rechtsbehelf.frist erhoben werden bei:
      #bescheid.rechtsbehelf.behoerde.
    ],
  )

  v(6mm)
  [#opt(abs.name)#opt(abs.contact_name, prefix: " — ")]
}
