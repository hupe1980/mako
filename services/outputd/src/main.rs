//! `outputd` — Customer-Communications daemon.
//!
//! Port: `:9880`
//!
//! The document engine renders what other services computed and owns nothing
//! about *what* a document says — see `outputd::document` for the layering and `document::mahnung` for why the
//! view contracts live with the renderer. `outputd::delivery` owns the other
//! half: the documents actually issued, the channels they go out on (portal
//! inbox, e-mail, print spool, ERP) and the per-document evidence that they
//! arrived.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/render/{kind}` | Render a view with the current or a pinned template; `X-Mako-Template-Hash` names the layout used. Stores nothing |
//! | `POST` | `/api/v1/documents/issue/{kind}` | Render, **record** and queue for delivery; idempotent on `subject_ref` |
//! | `GET` | `/api/v1/documents` | A customer's documents (`?malo_id=` or `?kunden_nr=`, `&kind=`) — the portal inbox |
//! | `GET` | `/api/v1/documents/{id}` | One document with every delivery track |
//! | `GET` | `/api/v1/documents/{id}/content` | The bytes as issued (§ 147 AO reproduction, never a re-render) |
//! | `POST` | `/api/v1/deliveries/{id}/read` | The customer opened it in the portal |
//! | `POST` | `/api/v1/deliveries/{id}/status` | A channel reports arrival or a bounce |
//! | `GET` | `/api/v1/spool` | What a print service collects |
//! | `GET` | `/api/v1/templates` | Every template this tenant published (`?kind=&limit=`) |
//! | `POST` | `/api/v1/templates` | Prove a template, then store it forever |
//! | `POST` | `/api/v1/templates/preview` | Render a candidate against the kind's specimen; stores nothing |
//! | `GET` | `/api/v1/templates/reference/{kind}` | The reference layout mako ships (INVOICE, MAHNUNG) |
//! | `GET`/`PUT` | `/api/v1/templates/{kind}/current` | Which template is rolled out |
//! | `GET` | `/api/v1/templates/by-hash/{hash}` | Resolve the layout an issued document used |
//! | `GET` | `/health` · `/health/ready` | Liveness · readiness |

#![deny(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<outputd::Outputd>().await
}
