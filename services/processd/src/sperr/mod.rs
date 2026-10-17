//! The Sperr-/Entsperrauftrag execution queue (Netzbetreiber role).
//!
//! The grid operator's work queue for the physical acts GPKE orders it to
//! perform: an ORDERS **17115 Sperrauftrag** or **17117 Entsperrauftrag** from a
//! Lieferant becomes a job for the field team, and the outcome goes back as
//! **IFTSTA 21039** (Auftragsstatus Sperren/Entsperren).
//!
//! Without the IFTSTA the Lieferant's `gpke-sperrung-lf` process never reaches a
//! terminal state and GPKE gives them no way to find out what happened but to
//! ask — so dispatching it is the queue's whole reason to exist, and the one
//! state it will not let fall silently on the floor (see [`worker`](crate::sperr::worker)).
//!
//! The trigger is the one the NB decision module reads: `makod` spawns the
//! NB-role `gpke-sperrung` workflow on an inbound ORDERS and announces it as
//! `de.mako.process.initiated`, which reaches processd's one `POST /webhook`
//! and is handed to [`handlers::ingest`](crate::sperr::handlers::ingest).
//!
//! # Endpoints
//!
//! | Method | Path | Cedar action | Description |
//! |---|---|---|---|
//! | `POST`  | `/api/v1/sperr-orders` | `create-sperr-order` | Create an order by hand |
//! | `GET`   | `/api/v1/sperr-orders` | `read-sperr-order` | The queue (`?status=&malo_id=&due=true`) |
//! | `GET`   | `/api/v1/sperr-orders/stats` | `read-sperr-order` | Counters, incl. outstanding IFTSTA |
//! | `GET`   | `/api/v1/sperr-orders/{id}` | `read-sperr-order` | One order |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/execute` | `execute-sperr-order` | Carried out → IFTSTA `Z14` |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/fail` | `execute-sperr-order` | Not carried out → IFTSTA `Z13` |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/cancel` | `cancel-sperr-order` | Withdraw a pending order |
//!
//! | Module | Purpose |
//! |---|---|
//! | [`model`](crate::sperr::model) | the typed domain — ORDERS/IFTSTA codes, not loose strings |
//! | [`ingest`](crate::sperr::ingest) | inbound ORDERS 17115/17117 → work order |
//! | [`handlers`](crate::sperr::handlers) | HTTP routes (all authenticated **and** authorized) |
//! | [`events`](crate::sperr::events) | `de.sperr.*` CloudEvents |
//! | [`worker`](crate::sperr::worker) | the IFTSTA 21039 retry queue |
//! | [`pg`](crate::sperr::pg) | persistence |

use std::sync::Arc;

use axum::{Extension, Router, routing::get};
use mako_markt::makod_client::MakodClient;

pub mod events;
pub mod handlers;
pub mod ingest;
pub(crate) mod mcp;
pub mod model;
pub mod pg;
pub mod worker;

/// Newtype for the tenant string injected as an Axum `Extension`.
///
/// Using a newtype avoids accidental collisions with other `Extension<String>`
/// values.
#[derive(Clone, Debug)]
pub struct Tenant(pub String);

/// The Sperrauftrag routes, carrying the `makod` client and the tenant they
/// dispatch under. The caller adds the pool, OIDC and Cedar layers it shares
/// with every other processd route.
pub fn router(makod: Arc<MakodClient>, tenant: String) -> Router {
    Router::new()
        .route("/api/v1/sperr-orders/stats", get(handlers::get_stats))
        .route(
            "/api/v1/sperr-orders",
            get(handlers::list_orders).post(handlers::create_order),
        )
        .route("/api/v1/sperr-orders/{id}", get(handlers::get_order))
        .route(
            "/api/v1/sperr-orders/{id}/execute",
            axum::routing::put(handlers::execute_order),
        )
        .route(
            "/api/v1/sperr-orders/{id}/fail",
            axum::routing::put(handlers::fail_order),
        )
        .route(
            "/api/v1/sperr-orders/{id}/cancel",
            axum::routing::put(handlers::cancel_order),
        )
        .layer(Extension(makod))
        .layer(Extension(Tenant(tenant)))
}
