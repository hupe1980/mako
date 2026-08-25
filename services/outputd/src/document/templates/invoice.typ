// ─────────────────────────────────────────────────────────────────────────────
// mako — reference invoice layout
//
// This is a starting point, not a fixed form: everything below is yours to
// change. What you may NOT change is the contract — this file must export
//
//     #let render(invoice) = ...
//
// and `invoice` is the DocumentView described in outputd's `document::view` —
// projected there from the EN 16931 model the issuing service sends.
// The CII XML embedded beside this page is produced by mako from the EN 16931
// model and is not reachable from here; nothing you write can make the page and
// the XML disagree about what the invoice says.
//
// Fonts: the bundled Typst set — "Libertinus Serif", "New Computer Modern" and
// "DejaVu Sans Mono". There is deliberately no way to load your own: the
// renderer has no filesystem, so an invoice re-rendered in 2034 typesets with
// the same faces it did the day it was issued.
//
// § 14 Abs. 4 UStG requires all of the following on the page, and they are all
// laid out below: full name and address of both parties (1), the seller's
// USt-IdNr. or Steuernummer (2), the issue date (3), a unique invoice number
// (4), quantity and description per position (5), the period of supply (6), the
// net amount per VAT rate (7), the rate and the tax amount (8), and — where it
// applies — the reason for an exemption.
// ─────────────────────────────────────────────────────────────────────────────

// ── Formatting helpers ───────────────────────────────────────────────────────
//
// Amounts arrive as exact decimal strings ("376.50", "0.3012"). They are never
// floats, and they must never be rounded here: a unit price legitimately has
// four decimals while money has two, so these helpers PAD to a minimum and
// never truncate. Truncating would put a number on the page that the embedded
// XML does not contain.

// Group an integer part with thousands separators: "1234567" → "1.234.567"
#let group(digits) = {
  let out = ""
  let n = digits.len()
  for (i, c) in digits.clusters().enumerate() {
    if i > 0 and calc.rem(n - i, 3) == 0 { out = out + "." }
    out = out + c
  }
  out
}

// German number formatting with at least `min` decimals. Never rounds.
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

// Money: always at least two decimals.
#let money(value) = num(value, min: 2)

// An ISO date as a German one: "2026-03-01" → "01.03.2026"
#let date(iso) = {
  if iso == none { return "" }
  let p = str(iso).split("-")
  if p.len() != 3 { return str(iso) }
  p.at(2) + "." + p.at(1) + "." + p.at(0)
}

// A field that may be absent. `none` is not "", and printing it raises.
#let opt(value, prefix: "", suffix: "") = {
  if value == none or value == "" { "" } else { prefix + str(value) + suffix }
}

// UN/ECE Rec 20 unit codes, in the spelling a customer reads.
#let units = (
  KWH: "kWh", MWH: "MWh", MTQ: "m³", MON: "Monat", DAY: "Tag",
  ANN: "Jahr", C62: "Stück", HUR: "Stunde", KWT: "kW", MAW: "MW",
)
#let unit(code) = units.at(str(code), default: str(code))

// VAT category codes (UNCL 5305), for the breakdown table.
#let categories = (
  S: "Regelsatz", Z: "Nullsatz", E: "steuerbefreit",
  AE: "Reverse Charge", K: "innergemeinschaftlich", G: "Ausfuhr", O: "nicht steuerbar",
)
#let category(code) = categories.at(str(code), default: str(code))

// ── The document ─────────────────────────────────────────────────────────────

#let render(invoice) = {
  let seller = invoice.seller
  let buyer = invoice.buyer

  set text(font: "Libertinus Serif", size: 10pt, lang: "de", hyphenate: false)
  set par(justify: false, leading: 0.62em)

  // DIN 5008 Form B margins, so a window envelope works.
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
          #opt(seller.name) \
          // No markup space between the calls: Typst renders one, and a space
          // before a comma ("Musterstraße 1 , 12345") is how it shows up.
          #opt(seller.line1)#opt(seller.post_code, prefix: ", ") #opt(seller.city)
        ],
        [
          // § 14 Abs. 4 Nr. 2 UStG — the USt-IdNr. or the Steuernummer,
          // both when the operator holds both.
          #opt(seller.vat_id, prefix: "USt-IdNr. ")#opt(
            seller.tax_number,
            prefix: if seller.vat_id == none { "St.-Nr. " } else { " · St.-Nr. " },
          ) \
          #opt(seller.phone)#opt(seller.email, prefix: " · ")
        ],
        [Seite #counter(page).display() von #counter(page).final().first()],
      )
    },
  )

  // Briefkopf. Replace with your logo: #image("...") is not available — the
  // renderer has no filesystem — so draw it, or set the name in your type.
  align(right)[
    #text(size: 13pt, weight: "bold")[#opt(seller.name)]
  ]
  v(2mm)

  // Anschriftenfeld: the small return line, then the recipient.
  block(height: 40mm)[
    #text(size: 7pt)[
      #opt(seller.name) · #opt(seller.line1) · #opt(seller.post_code) #opt(seller.city)
    ]
    #v(4mm)
    #opt(buyer.name) \
    #opt(buyer.line1) \
    #opt(buyer.post_code) #opt(buyer.city)
    // Only a foreign address names its country — DIN 5008 omits it at home,
    // and BT-55 is always present because EN 16931 requires it.
    #if buyer.country not in (none, "", "DE") [ \ #buyer.country ]
  ]

  // Title and the document's identifying terms.
  grid(
    columns: (1fr, auto),
    align: (left + bottom, right),
    text(size: 14pt, weight: "bold")[Rechnung],
    table(
      columns: 2,
      stroke: none,
      inset: (x: 0pt, y: 1.5pt),
      column-gutter: 10pt,
      align: (right, right),
      [Rechnungsnummer], [#opt(invoice.number)],
      [Rechnungsdatum], [#date(invoice.issue_date)],
      ..if invoice.due_date != none { ([Fällig am], [#date(invoice.due_date)]) } else { () },
      ..if invoice.buyer_reference != none {
        ([Leitweg-ID], [#opt(invoice.buyer_reference)])
      } else { () },
    ),
  )

  // § 14 Abs. 4 Nr. 6 UStG — the period of supply.
  if invoice.period_start != none {
    v(3mm)
    text(size: 9.5pt)[
      Abrechnungszeitraum: #date(invoice.period_start) – #date(invoice.period_end)
    ]
  }

  v(5mm)

  // ── Positions ──────────────────────────────────────────────────────────────
  table(
    columns: (auto, 1fr, auto, auto, auto, auto),
    align: (right, left, right, right, right, right),
    stroke: none,
    inset: (x: 4pt, y: 4pt),
    fill: (_, y) => if y == 0 { luma(240) },
    table.header(
      [*Pos.*], [*Bezeichnung*], [*Menge*], [*Einheit*], [*Einzelpreis*], [*Betrag*],
    ),
    table.hline(stroke: 0.6pt),
    ..invoice.lines.map(l => (
      [#l.id],
      [
        #l.name
        #if l.description != none [ \ #text(size: 8.5pt, fill: luma(80))[#l.description] ]
        #text(size: 8pt, fill: luma(110))[
          #h(0pt) (#category(l.vat_category)#opt(l.vat_rate, prefix: " ", suffix: " %"))
        ]
      ],
      [#num(l.quantity)],
      [#unit(l.unit)],
      [#money(l.unit_price) €],
      [#money(l.net_amount) €],
    )).flatten(),
    table.hline(stroke: 0.6pt),
  )

  v(4mm)

  // ── Totals and the VAT breakdown ───────────────────────────────────────────
  align(right)[
    #table(
      columns: (auto, auto),
      stroke: none,
      align: (right, right),
      inset: (x: 5pt, y: 2.5pt),
      [Summe netto], [#money(invoice.totals.line_total) €],
      // BG-20 — document-level allowances, each with its own VAT terms. A
      // Restrechnung deducts every advance this way (§ 14 Abs. 5 Satz 2 UStG),
      // which is what makes BT-106 and BT-109 differ. Printing the net sum and
      // then a VAT breakdown on a smaller base, with nothing between them,
      // shows a page that does not add up while the embedded XML is correct.
      ..invoice.allowances.map(a => (
        [
          #if a.reason != none [#a.reason] else [Abzug]
          #if a.vat_rate != none and a.vat_rate != "0" [ (#num(a.vat_rate) % USt)]
        ],
        [#sym.minus#money(a.amount) €],
      )).flatten(),
      ..invoice.charges.map(c => (
        [
          #if c.reason != none [#c.reason] else [Zuschlag]
          #if c.vat_rate != none and c.vat_rate != "0" [ (#num(c.vat_rate) % USt)]
        ],
        [#money(c.amount) €],
      )).flatten(),
      // Restate the base the VAT is actually computed on, but only where it
      // differs — on an ordinary invoice a second identical line is noise.
      ..if invoice.totals.line_total != invoice.totals.taxable_total {
        ([Summe netto nach Abzügen], [#money(invoice.totals.taxable_total) €])
      } else { () },
      // Highest rate first, the exempt/zero categories last — the order a
      // German reader expects (19 % before 7 % before "steuerbefreit"). The
      // model's order is the reconciler's grouping order, which is not a
      // presentation decision; sorting here keeps the two concerns apart.
      ..invoice.vat_breakdown.sorted(key: v => {
        if v.rate == none { 1.0 } else { -float(v.rate) }
      }).map(v => (
        [
          // A rate of `none` and a rate of 0 both mean "no VAT is charged here",
          // and both happen: BR-48 makes BT-119 optional when the invoice is not
          // subject to VAT, while XRechnung's BR-DE-14 requires it regardless —
          // so an exempt position reaches this template as `0` from one producer
          // and as `none` from another. Naming the category reads correctly for
          // both; "zzgl. 0 % USt" reads like a rate that happens to be zero.
          #if v.rate != none and v.rate != "0" [
            zzgl. #num(v.rate) % USt auf #money(v.taxable_amount) €
          ] else [
            #category(v.category) auf #money(v.taxable_amount) €
          ]
        ],
        [#money(v.tax_amount) €],
      )).flatten(),
      table.hline(stroke: 0.6pt),
      ..if invoice.totals.paid != none {
        ([Bruttobetrag], [#money(invoice.totals.gross_total) €],
         [bereits gezahlt], [#sym.minus#money(invoice.totals.paid) €])
      } else { () },
      text(weight: "bold")[Zahlbetrag],
      text(weight: "bold")[#money(invoice.totals.due) €],
    )
  ]

  // § 14 Abs. 4 Nr. 8 UStG — an exemption must say why.
  let reasons = invoice.vat_breakdown.filter(v => v.exemption_reason != none)
  if reasons.len() > 0 {
    v(3mm)
    for v in reasons {
      text(size: 8.5pt)[#v.exemption_reason \ ]
    }
  }

  if invoice.payment_terms != none {
    v(5mm)
    text(size: 9.5pt)[#invoice.payment_terms]
  }

  if invoice.notes.len() > 0 {
    v(4mm)
    for note in invoice.notes {
      text(size: 9pt)[#note \ ]
    }
  }
}
