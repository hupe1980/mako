# outputd — Customer Communications

`outputd` renders the documents a customer receives and delivers them. It owns
**how documents look** — the operator's Typst templates, the render engine, the
ZUGFeRD PDF/A-3 carrier, the publish gates and the append-only template store —
and **whether they arrived**: the store of issued documents and the per-channel
delivery evidence. What a document **says** — amounts, VAT, legal basis — stays
with the issuing service (billingd for invoices, accountingd for the Mahnwesen
figures, vertragd for a price change); outputd never recomputes a number.
Projecting the model onto *what a template may print* is not recomputation and
does live here: it is the renderer's API, and the publish gate has to construct
it without the issuing service.

A separate daemon because the template system was never invoice-specific: one
brand has one template store, and a logo change must reach the invoice *and*
the Mahnung.

Port: `:9880`

| Capability | Where |
|---|---|
| **Render API** | `POST /api/v1/render/{kind}` — `{model \| view, template_hash?, attachment?: {xml, specification_id}, date, ident}` → PDF + `X-Mako-Template-Hash` for the caller to pin. An `INVOICE` sends the **EN 16931 model** and outputd projects the page view from it; a Textform kind sends its own view. Stores nothing |
| **Document API** | `POST /api/v1/documents/{kind}` — the same render, **recorded** and queued for delivery. Idempotent on `subject_ref` (a Rechnungsnummer, a dunning-case id, a slice id), so a retrying issuer cannot send a second notice. `GET /documents` is the customer's inbox; `/documents/{id}/content` reproduces the bytes as issued (§ 14 Abs. 1 UStG, § 147 AO — never a re-render) |
| **Delivery** | One track per (document, channel) with backoff, an attempt ceiling (`max_attempts`, default 8) and evidence. `PORTAL`, `EMAIL`, `POST` and `ERP`; no SMTP client and no print driver — `EMAIL`/`POST` are HTTP relays an operator points at what they already run, and the print service can also *pull* `GET /api/v1/spool`. `SENT` ≠ `DELIVERED`: arrival comes back through `POST /deliveries/{id}/status`, and a portal read through `POST /deliveries/{id}/read` |
| **Authz** | Cedar ABAC (`policies/outputd.cedar`) on every route: tenant isolation everywhere, plus a market-role gate (`LF`/`MSB`/`ESA`) on publishing, rolling out and rendering |
| **Errors** | One envelope, one stable code — and a template that does not compile returns its diagnostics as a **list**, not a blob: `{"error":{"code":"TEMPLATE_DID_NOT_COMPILE","diagnostics":["/template.typ:12:4: …"]}}` |
| **Typst sandbox** | no filesystem, no network, no packages, no clock (`datetime.today()` is the *document's* date); renders capped at cores − 1 on the blocking pool, 20 s budget |
| **ZUGFeRD carrier** | `document::facturx` — PDF/A-3 via typst-pdf, Factur-X XMP stamped by incremental update (typst-pdf has no XMP hook); profile derived from the payload's BT-24, never configured |
| **Publish gates** | `POST /api/v1/templates` renders the candidate against an awkward specimen, enforces PDF/A, stamps, then reads the finished file back with `en16931-formats::zugferd::extract` (byte-identical payload, no `Divergence`) and requires the § 14 Abs. 4 UStG terms on the page — the number (Nr. 4), both party names (Nr. 1) and the seller's USt-IdNr. **or** Steuernummer (Nr. 2, a disjunction). Only then is a row written |
| **Template store** | content-addressed per tenant (`PRIMARY KEY (tenant, hash)`), append-only, never UPDATE/DELETE — issuing services pin the hash next to each document, and § 147 AO / GoBD keep that resolvable for 8 years. `document_template_current` is the one mutable pointer |
| **Textform kinds** | `MAHNUNG` (§ 126b BGB; Stufe-3 specimen gate — declarant, Gesamtforderung, Zahlungsfrist, § 41f Sperrtermin) and `PREISANPASSUNG` (§ 41 Abs. 5 EnWG; mixed-change specimen — declarant, Wirksamkeit, **both** changed prices and the Satz-4 Sonderkündigungsrecht) share the store and the engine. Each has a view, a specimen, a gate and a reference layout, and a producer: `accountingd` and `vertragd` |
| **External validation** | `just zugferd-verify` — veraPDF + Mustang, containerized (Docker is the only host dependency); all specimens must come back valid/compliant |

## Authorization

Authentication establishes *who* is calling. `policies/outputd.cedar` decides
what they may do, and every route checks it before touching the database:

| Action | Who |
|---|---|
| `read-template`, `preview-template`, `read-document` | any authenticated caller in the tenant |
| `publish-template`, `rollout-template`, `render-document`, `issue-document`, `report-delivery` | `LF`, `MSB` or `ESA` |

Authentication alone is not enough here: without a policy, any token the OIDC
verifier accepts could roll out the layout every invoice and Mahnung of the
tenant renders with, or render arbitrary content under the operator's
Briefkopf. A template is not one document; it is the shape of all of them.
`tests/authorization.rs` pins the
decisions, including that publishing and rolling out are reachable by exactly
the same callers.

A **preview** is deliberately a read: it renders mako's own specimen, stores
nothing and moves nothing, so it reaches no customer.

`template_store::by_hash` is tenant-scoped for the same reason. It was not —
"the hash *is* the identity, and a document carrying it has already established
the right to see it" — which holds for a document and not for
`GET /templates/by-hash/{hash}`, where the caller supplies the hash and nothing
has been established. In a shared database that made one operator's complete
template source readable by another. The lock is in the query, not in a handler
check; `render_admissible`'s tenant arm is gone because there is nothing left
for it to catch.

## One projection, not two

The view contracts (`document::view::DocumentView`,
`document::mahnung::MahnungView`, `document::preisanpassung::PreisanpassungView`)
are **normative here** — the publish gate proves every operator template against
them.

An `INVOICE` render carries the **EN 16931 model**, and the projection to
`DocumentView` happens here — once, on the side that proves templates against
it. A caller projecting its own copy would be two implementations of one
contract with nothing tying them together: the gate would prove templates
against outputd's, production would feed them the caller's, and a field added to
either yields templates that pass the gate and fail in production. Both services
already depend on `en16931`, so the model is a type they share exactly as they
share `zugferd::Profile`.

The Textform kinds send their view directly: their producer has no EN 16931
model, and their view *is* the contract.

## Configuration

```toml
# outputd.toml
port   = 9880
tenant = "9900357000004"   # the operator's MP-ID; every template row is scoped to it

[database]
url = "postgresql://outputd:secret@db:5432/outputd"

# Required in production. Without it anyone can roll out the layout every
# invoice and Mahnung renders with; `allow_insecure_no_auth = true` is the
# dev-only escape hatch and says so at startup.
[oidc]
issuer   = "https://auth.example.de/realms/mako"
audience = "outputd"

# How issued documents leave. Nothing here is an SMTP client or a print driver:
# each is an HTTP relay answering 2xx on acceptance. Configure none and the
# PORTAL channel still works, which is the one § 41 Abs. 5 EnWG and § 126b BGB
# actually ask for — Textform on a durable medium, not registered post.
[delivery]
enabled            = true
email_relay_url    = "http://mail-relay:8000/send"
email_relay_api_key = "env:OUTPUTD_EMAIL_RELAY_KEY"
# Omit postal_relay_url and POST deliveries wait in GET /api/v1/spool for the
# print service to pull, which is how most Druckdienstleister integrate.
# postal_relay_url = "http://print:8000/jobs"
erp_webhook_url    = "http://erp:8000/documents"
from_address       = "rechnung@example.de"
max_attempts       = 8                       # ~half a day with the doubling backoff

[delivery.subjects]
MAHNUNG = "Zahlungserinnerung"
```

Full operator guide: `site/content/docs/services/outputd.md`.

## Tests

```bash
cargo test -p outputd                 # gates, carrier, XMP, reproducibility, authz
just test-outputd-db                  # template + document stores against real PostgreSQL (Docker)
just zugferd-specimen                 # write stamped specimens to target/
just zugferd-verify                   # veraPDF + Mustang, containerized
```
