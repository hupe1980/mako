# outputd

`outputd` renders the documents a customer receives and delivers them. It owns
how documents look — the operator's Typst templates, the ZUGFeRD PDF/A-3
carrier, the publish gates and the append-only template store — and whether
they arrived. What a document says stays with the issuing service (`billingd`,
`accountingd`, `vertragd`); `outputd` never recomputes a number.

| | |
|---|---|
| **Port** | 9880 (`dev/ports.toml`) |
| **Database** | PostgreSQL, `public` schema — template store, issued documents, delivery tracks |
| **MCP** | none |
| **Emits** | no CloudEvents; each pushed delivery is a signed `event_outbox` command to its relay |
| **Consumes** | delivery reports at `POST /api/v1/deliveries/{delivery_id}/status` · `/read` |
| **Depends on** | none — its callers are billingd, accountingd and vertragd |

Document kinds: `INVOICE`, `MAHNUNG`, `PREISANPASSUNG`, `GEBUEHRENBESCHEID`,
`VERTRAGSZUSAMMENFASSUNG`, `VERTRAGSBESTAETIGUNG`, `WIDERRUFSBESTAETIGUNG`.

## Run

```bash
just dev outputd    # dev/config/outputd.toml against the dev PostgreSQL
OUTPUTD_CONFIG=dev/config/outputd.toml cargo run -p outputd -- --check-config
just test-outputd-db
just zugferd-verify # veraPDF + Mustang, containerized; not part of just ci
```

## API

| Route | Purpose |
|---|---|
| `POST /api/v1/render/{kind}` | Stateless render → PDF + `X-Mako-Template-Hash` |
| `POST /api/v1/documents/issue/{kind}` | Render, record and queue for delivery; idempotent on `subject_ref` |
| `GET /api/v1/documents` · `/{document_id}` · `/{document_id}/content` | The inbox; content is the bytes as issued |
| `GET /api/v1/documents/by-subject/{kind}/{subject_ref}` | One document by what it is about, with its delivery tracks |
| `GET /api/v1/deliveries?status=FAILED&kind=` | Failed deliveries — the operator's work list |
| `GET /api/v1/spool` | What a print service collects |
| `/api/v1/templates` (+ `/preview`, `/{kind}/current`, `/by-hash/{hash}`, `/reference/{kind}`) | Template store: publish behind the gate, roll out, resolve |

## Configuration

`tenant`, `[database]`, `[oidc]`, and `[delivery]`: `from_address`,
`email_relay_url`, `postal_relay_url` (unset: `POST` deliveries wait in the
spool), `erp_webhook_url` — each relay URL with its `*_api_key` — and
`[delivery.subjects]` per kind.

`[policy]`: [`anrede`](https://hupe1980.github.io/mako/docs/services/outputd/#policy-anrede)
— `sie` (default) or `du`, applied to every letter and built-in mail subject.
The Widerrufsbelehrung and the Gebührenbescheid always say „Sie". See the
[operator policy reference](https://hupe1980.github.io/mako/docs/reference/operator-policy/).

The [operator guide](https://hupe1980.github.io/mako/docs/services/outputd/)
covers the document kinds, templates and their sandbox, the publish gate, the
Factur-X carrier, delivery and the full configuration.
