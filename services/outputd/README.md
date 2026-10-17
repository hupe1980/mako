# outputd

`outputd` (`:9880`) renders the documents a customer receives and delivers them.
It owns how documents look — the operator's Typst templates, the ZUGFeRD PDF/A-3
carrier, the publish gates and the append-only template store — and whether
they arrived. What a document says stays with the issuing service (`billingd`,
`accountingd`, `vertragd`); `outputd` never recomputes a number.

## Run it

```bash
just dev outputd       # dev/config/outputd.toml against the dev PostgreSQL
```

## At a glance

| | |
|---|---|
| **Kinds** | `INVOICE` (the caller sends the EN 16931 model; outputd projects the view), `MAHNUNG` (§ 126b BGB) from `accountingd`, `PREISANPASSUNG` (§ 41 Abs. 5 EnWG) from `vertragd` |
| **Template store** | content-addressed per tenant, append-only; issuers pin the hash beside each document (`X-Mako-Template-Hash`). `document_template_current` is the one mutable pointer |
| **Publish gate** | renders the candidate against a specimen, enforces PDF/A, reads the Factur-X payload back byte-identical and requires the § 14 Abs. 4 UStG terms on the page before a row is written |
| **Typst sandbox** | no filesystem, network, packages or clock; renders capped at cores − 1, 20 s budget |
| **Delivery** | one track per (document, channel): `PORTAL`, `EMAIL`, `POST`, `ERP`, with backoff and `max_attempts`. `SENT` ≠ `DELIVERED` — arrival is reported back |
| **Auth** | OIDC + Cedar `policies/outputd.cedar`: reads and previews for the tenant; publish, rollout, render, issue and delivery reports for `LF`/`MSB`/`ESA` |
| **Errors** | one envelope with a stable code; a template that does not compile returns its diagnostics as a list (`TEMPLATE_DID_NOT_COMPILE`) |

## Routes

| Route | Purpose |
|---|---|
| `POST /api/v1/render/{kind}` | stateless render → PDF + template hash |
| `POST /api/v1/documents/issue/{kind}` | render, record and queue for delivery; idempotent on `subject_ref` |
| `GET /api/v1/documents` · `/{document_id}` · `/{document_id}/content` | the inbox; content is the bytes as issued, never a re-render |
| `GET\|POST /api/v1/templates`, `/templates/preview`, `/templates/{kind}/current`, `/templates/by-hash/{hash}`, `/templates/reference/{kind}` | template store |
| `POST /api/v1/deliveries/{delivery_id}/status` · `/read`, `GET /api/v1/spool` | delivery evidence; print services may pull the spool |

## Configuration

```toml
# outputd.toml
tenant = "9900357000004"   # the operator's MP-ID; every template row is scoped to it

[http]
addr = "0.0.0.0:9880"

[database]
url = "postgresql://outputd:secret@db:5432/outputd"

# Required unless `allow_insecure_no_auth = true` (dev only).
[oidc]
issuer   = "https://auth.example.de/realms/mako"
audience = "outputd"

# Each channel is an HTTP relay answering 2xx on acceptance; PORTAL needs none.
# Each `*_relay_url` requires its `*_api_key` (bearer + Standard Webhooks key).
[delivery]
enabled            = true
email_relay_url    = "http://mail-relay:8000/send"
email_relay_api_key = "env:OUTPUTD_EMAIL_RELAY_KEY"
# Without postal_relay_url, POST deliveries wait in GET /api/v1/spool.
# postal_relay_url = "http://print:8000/jobs"
erp_webhook_url    = "http://erp:8000/documents"
from_address       = "rechnung@example.de"
max_attempts       = 8                       # ~half a day with the doubling backoff

[delivery.subjects]
MAHNUNG = "Zahlungserinnerung"
```

## Tests

```bash
cargo test -p outputd       # gates, carrier, XMP, reproducibility, authz
just test-outputd-db        # template and document stores against PostgreSQL (Docker)
just zugferd-verify         # veraPDF + Mustang, containerized
```

The [operator guide](https://hupe1980.github.io/mako/docs/services/outputd/)
covers templates, the view contracts, delivery and the full configuration
reference.
