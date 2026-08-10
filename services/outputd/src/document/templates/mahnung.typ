// ─────────────────────────────────────────────────────────────────────────────
// mako — reference Mahnung layout (Textform, § 126b BGB)
//
// A starting point, yours to change — the contract is the export:
//
//     #let render(mahnung) = ...
//
// `mahnung` is the MahnungView described in billingd's `document::mahnung`.
// Textform requires: readable, durable medium, declarant named. The publish
// gate checks the declarant, the Gesamtforderung and the Zahlungsfrist are
// actually printed.
//
// Helpers are duplicated from the invoice template on purpose: a template is a
// single self-contained file (the renderer serves it no imports), so every
// template carries its own formatting. Change one, check the other.
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

#let stufen-titel = ("1": "Zahlungserinnerung", "2": "Mahnung", "3": "Letzte Mahnung")

// An IBAN in 4-character groups — the way a bank prints it and a reader
// transcribes it. The value arrives ungrouped; grouping is presentation.
#let iban(s) = {
  if s == none { return "" }
  let out = ""
  for (i, c) in str(s).clusters().enumerate() {
    if i > 0 and calc.rem(i, 4) == 0 { out = out + " " }
    out = out + c
  }
  out
}

#let render(mahnung) = {
  let abs = mahnung.absender
  let emp = mahnung.empfaenger

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
    text(size: 14pt, weight: "bold")[
      #stufen-titel.at(str(mahnung.stufe), default: "Mahnung")
    ],
    table(
      columns: 2,
      stroke: none,
      inset: (x: 0pt, y: 1.5pt),
      column-gutter: 10pt,
      align: (right, right),
      [Datum], [#date(mahnung.datum)],
      [Zahlbar bis], [*#date(mahnung.zahlungsfrist)*],
    ),
  )
  v(4mm)

  [Trotz Fälligkeit sind die folgenden Beträge noch offen:]
  v(2mm)

  table(
    columns: (1fr, auto, auto, auto),
    align: (left, right, right, right),
    stroke: none,
    inset: (x: 4pt, y: 4pt),
    fill: (_, y) => if y == 0 { luma(240) },
    table.header([*Rechnung*], [*vom*], [*fällig am*], [*offen*]),
    table.hline(stroke: 0.6pt),
    ..mahnung.posten.map(p => (
      [#p.rechnungsnummer],
      [#date(p.rechnungsdatum)],
      [#date(p.faellig_am)],
      [#money(p.offener_betrag) €],
    )).flatten(),
    table.hline(stroke: 0.6pt),
  )

  v(3mm)
  align(right)[
    #table(
      columns: (auto, auto),
      stroke: none,
      align: (right, right),
      inset: (x: 5pt, y: 2.5pt),
      ..if mahnung.mahngebuehr != none {
        ([Mahngebühr], [#money(mahnung.mahngebuehr) €])
      } else { () },
      ..if mahnung.verzugszinsen != none {
        ([Verzugszinsen#opt(mahnung.zins_grundlage, prefix: " — ")],
         [#money(mahnung.verzugszinsen) €])
      } else { () },
      table.hline(stroke: 0.6pt),
      text(weight: "bold")[Gesamtforderung],
      text(weight: "bold")[#money(mahnung.gesamtforderung) €],
    )
  ]

  v(4mm)
  [Bitte überweisen Sie den Gesamtbetrag bis zum
   *#date(mahnung.zahlungsfrist)*#if mahnung.iban != none [
     #h(0pt) auf das Konto IBAN #iban(mahnung.iban)].
   Sollten Sie zwischenzeitlich gezahlt haben, betrachten Sie dieses Schreiben
   als gegenstandslos.]

  // § 41f Abs. 1 EnWG — the Stufe-3 threat block. Visually set apart: a
  // disconnection threat buried in body text fails its purpose and, arguably,
  // its lawfulness.
  if mahnung.sperrandrohung != none {
    v(4mm)
    block(
      width: 100%,
      inset: 8pt,
      stroke: 0.8pt + luma(60),
      [
        *Androhung der Versorgungsunterbrechung* \
        #mahnung.sperrandrohung
        #if mahnung.geplantes_sperrdatum != none [
          \ Frühester Sperrtermin: *#date(mahnung.geplantes_sperrdatum)*.
        ]
        \ Die Unterbrechung unterbleibt, wenn Sie den rückständigen Betrag
        begleichen oder eine Abwendungsvereinbarung (§ 41g EnWG) mit uns
        treffen. Bitte wenden Sie sich dazu umgehend an
        #opt(abs.contact_name, prefix: "unser ")#opt(abs.phone, prefix: ", Telefon ").
      ],
    )
  }

  v(6mm)
  [Mit freundlichen Grüßen \ #opt(abs.name)#opt(abs.contact_name, prefix: " — ")]
}
