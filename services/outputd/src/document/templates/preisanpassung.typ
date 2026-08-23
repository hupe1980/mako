// ─────────────────────────────────────────────────────────────────────────────
// mako — reference Preisanpassung layout (Textform, § 126b BGB)
//
// A starting point, yours to change — the contract is the export:
//
//     #let render(anzeige) = ...
//
// `anzeige` is the PreisanpassungView described in `document::preisanpassung`.
//
// § 41 Abs. 5 EnWG makes this letter's *content* a form requirement, not a
// stylistic choice. The publish gate renders this template against the
// specimen and checks the page prints: the declarant, the date the new prices
// apply, **both** changed prices (including the one that goes down), and the
// § 41 Abs. 5 Satz 4 Sonderkündigungsrecht. A notice that announces the price
// and omits the termination right is not a valid Preisänderungsanzeige.
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

#let render(anzeige) = {
  let abs = anzeige.absender
  let emp = anzeige.empfaenger

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
    text(size: 14pt, weight: "bold")[Änderung Ihrer Preise],
    table(
      columns: 2,
      stroke: none,
      inset: (x: 0pt, y: 1.5pt),
      column-gutter: 10pt,
      align: (right, right),
      [Datum], [#date(anzeige.datum)],
      ..if anzeige.vertragsnummer != none {
        ([Vertrag], [#anzeige.vertragsnummer])
      } else { () },
      ..if anzeige.malo_id != none {
        ([Marktlokation], [#anzeige.malo_id])
      } else { () },
    ),
  )
  v(4mm)

  // § 41 Abs. 5 Satz 1 — the change, when it takes effect, and its Anlass.
  [Sehr geehrte Damen und Herren,]
  v(2mm)
  [wir passen die Preise Ihrer#opt(anzeige.sparte, prefix: " ")belieferung zum
   *#date(anzeige.wirksam_ab)* an. Anlass: #anzeige.anlass]
  v(2mm)
  [Die Änderung wird Ihnen mit einer Frist von #anzeige.ankuendigungsfrist
   angekündigt.]

  // § 41 Abs. 5 Satz 1 — the Umfang, line by line. A single sentence cannot
  // state it: a customer whose Arbeitspreis rises while their Grundpreis falls
  // has to be able to see both.
  if anzeige.positionen.len() > 0 {
    v(4mm)
    table(
      columns: (1fr, auto, auto, auto),
      align: (left, left, right, right),
      stroke: none,
      inset: (x: 4pt, y: 4pt),
      fill: (_, y) => if y == 0 { luma(240) },
      table.header([*Preisbestandteil*], [*Einheit*], [*bisher*], [*neu*]),
      table.hline(stroke: 0.6pt),
      ..anzeige.positionen.map(p => (
        [#p.bezeichnung],
        [#p.einheit],
        [#money(p.bisher)],
        [#text(weight: "bold")[#money(p.neu)]],
      )).flatten(),
      table.hline(stroke: 0.6pt),
    )
    v(1mm)
    text(size: 8pt, fill: luma(80))[Alle Preise inklusive Umsatzsteuer.]
  }

  // § 41 Abs. 5 Satz 4 — the termination right, which Satz 1 obliges us to
  // state in this same notice. Set apart deliberately: a right buried in body
  // text is a right the customer does not read, and the gate checks it is on
  // the page at all.
  v(4mm)
  block(
    width: 100%,
    inset: 8pt,
    stroke: 0.8pt + luma(60),
    [
      *Ihr Sonderkündigungsrecht* \
      Sie können den Vertrag ohne Einhaltung einer Kündigungsfrist zum
      *#date(anzeige.sonderkuendigung.wirksam_zum)* kündigen
      (#anzeige.sonderkuendigung.rechtsgrundlage).
      #if anzeige.sonderkuendigung.entgeltfrei [
        Die Kündigung ist für Sie kostenfrei.
      ]
      \ Kündigen Sie nicht, gelten die neuen Preise ab dem genannten Termin.
    ],
  )

  if anzeige.hinweis != none {
    v(4mm)
    [#anzeige.hinweis]
  }

  v(6mm)
  [Mit freundlichen Grüßen \ #opt(abs.name)#opt(abs.contact_name, prefix: " — ")]
}
