# outputd — Customer Communications

`outputd` renders the documents a customer receives. It owns **how documents
look**: the operator's Typst templates, the render engine, the ZUGFeRD PDF/A-3
carrier, the publish gates and the append-only template store. What a document
**says** — amounts, VAT, legal basis — stays with the issuing service (billingd
for invoices, accountingd for the Mahnwesen figures); outputd never recomputes
a number.

A separate daemon because the template system was never invoice-specific: one
brand has one template store, and a logo change must reach the invoice *and*
the Mahnung. Delivery channels (mail, e-mail, portal inbox, with per-document
evidence) are this daemon's designed growth.

Port: `:9880`

| Capability | Where |
|---|---|
| **Render API** | `POST /api/v1/render/{kind}` — `{view, template_hash?, attachment?: {xml, specification_id}, date, ident}` → PDF + `X-Mako-Template-Hash` for the caller to pin |
| **Typst sandbox** | no filesystem, no network, no packages, no clock (`datetime.today()` is the *document's* date); renders capped at cores − 1 on the blocking pool, 20 s budget |
| **ZUGFeRD carrier** | `document::facturx` — PDF/A-3 via typst-pdf, Factur-X XMP stamped by incremental update (typst-pdf has no XMP hook); profile derived from the payload's BT-24, never configured |
| **Publish gates** | `POST /api/v1/templates` renders the candidate against an awkward specimen, enforces PDF/A, stamps, then reads the finished file back with `en16931-formats::zugferd::extract` (byte-identical payload, no `Divergence`) and requires the § 14 Abs. 4 UStG terms on the page. Only then is a row written |
| **Template store** | content-addressed (SHA-256), append-only, never UPDATE/DELETE — issuing services pin the hash next to each document, and § 147 AO / GoBD keep that resolvable for 8 years. `document_template_current` is the one mutable pointer |
| **Textform kinds** | `MAHNUNG` (§ 126b BGB; `MahnungView` contract, Stufe-3 specimen gate) and `PREISANPASSUNG` share the store and the engine |
| **External validation** | `just zugferd-verify` — veraPDF + Mustang, containerized (Docker is the only host dependency); all specimens must come back valid/compliant |

The view contracts (`document::view::DocumentView`, `document::mahnung::MahnungView`)
are **normative here** — the publish gate proves templates against them; callers
serialise their own copy of the shape at the HTTP boundary, the same way other
mako wire structs are duplicated per service.

Full operator guide: `site/content/docs/services/outputd.md`.

## Tests

```bash
cargo test -p outputd                 # gates, carrier, XMP, reproducibility
just test-outputd-db                  # template store against real PostgreSQL (Docker)
just zugferd-specimen                 # write stamped specimens to target/
just zugferd-verify                   # veraPDF + Mustang, containerized
```
