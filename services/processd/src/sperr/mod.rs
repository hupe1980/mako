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
//! | [`worker`](crate::sperr::worker) | the IFTSTA escalation and execution-window sweep |
//! | [`pg`](crate::sperr::pg) | persistence |

use std::sync::Arc;

use axum::{Extension, Router, routing::get};

pub mod events;
pub mod handlers;
pub mod ingest;
pub(crate) mod mcp;
pub mod model;
pub mod pg;
pub mod worker;

/// The outbox worker that delivers the IFTSTA 21039 commands to makod, and
/// stamps each order's `iftsta_dispatched_at` in the transaction that marks its
/// command delivered.
#[must_use]
pub fn iftsta_worker(
    pool: sqlx::PgPool,
    makod_url: impl Into<String>,
    makod_api_key: secrecy::SecretString,
) -> mako_service::outbox::OutboxWorker {
    mako_service::outbox::OutboxWorker::commands(
        pool,
        pg::IFTSTA_UPSTREAM,
        makod_url,
        Some(makod_api_key),
    )
    .on_delivered(Arc::new(|conn, delivered| {
        Box::pin(pg::record_iftsta_delivered(conn, delivered))
    }))
}

/// Newtype for the tenant string injected as an Axum `Extension`.
///
/// Using a newtype avoids accidental collisions with other `Extension<String>`
/// values.
#[derive(Clone, Debug)]
pub struct Tenant(pub String);

/// `[policy] msb_vorlauf_lesart`, injected as an Axum `Extension` so an order
/// view states the earliest Sperrtermin under the reading in force.
#[derive(Clone, Copy, Debug)]
pub struct Lesart(pub mako_service::policy::Policy<crate::config::MsbVorlaufLesart>);

/// The Sperrauftrag routes, carrying the tenant they work under and the
/// Sperrtermin reading. The caller adds the pool, OIDC
/// and Cedar layers it shares with every other processd route.
pub fn router(tenant: String, lesart: Lesart) -> Router {
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
        .layer(Extension(Tenant(tenant)))
        .layer(Extension(lesart))
}
