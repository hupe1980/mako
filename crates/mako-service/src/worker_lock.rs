//! Single-runner advisory locks for background workers.
//!
//! A mako service that runs a periodic worker — a dunning sweep, a price-change
//! notice run, an Abschlag cycle — must not run it once per replica. Per-run
//! idempotency guards (a `UNIQUE (tenant, run_date)`, an `ON CONFLICT DO
//! NOTHING`) stop the *duplicate row*, but they do not stop the duplicate
//! *side effect*: two replicas both render a document, both enqueue an outbound
//! task, both announce a CloudEvent, and only then does one of them lose the
//! insert.
//!
//! PostgreSQL session-level advisory locks are the smallest thing that fixes
//! that: the lock is held by a connection, so a replica that dies releases it
//! without a lease to expire or a heartbeat to miss.
//!
//! This lived twice — once in `accountingd`, once copied into `vertragd` — with
//! the same body and two unrelated key spaces. It lives here so a third service
//! borrows it rather than copying it again.
//!
//! # Allocating a key
//!
//! The key is a single `i64` in one global namespace shared by everything on
//! the database, so a collision between two services silently serialises two
//! unrelated workers. Give each service a distinct 16-bit prefix and number its
//! workers within it — `accountingd` uses `0x_acc0_xxxx`, `vertragd`
//! `0x_7e64_xxxx`. Keys are part of the deployment contract: changing one lets
//! an old and a new replica both run the same worker during a rollout.
//!
//! # Example
//!
//! ```no_run
//! # async fn run(pool: &sqlx::PgPool) -> anyhow::Result<()> {
//! const LOCK_MY_WORKER: i64 = 0x_1234_0001;
//!
//! if let Some(mut conn) = mako_service::worker_lock::try_worker_lock(pool, LOCK_MY_WORKER).await {
//!     // ... do the work; a second replica skipped this cycle ...
//!     mako_service::worker_lock::release_worker_lock(&mut conn, LOCK_MY_WORKER).await;
//! }
//! # Ok(())
//! # }
//! ```

use sqlx::PgPool;

/// Try to take the session-level advisory lock `key`.
///
/// Returns the connection holding it when the lock is won; `None` means another
/// replica holds it and this one must **skip the cycle**, not wait for it — a
/// periodic worker that queues up behind its own previous run turns a slow
/// cycle into an unbounded backlog.
///
/// Release it with [`release_worker_lock`] **on the same connection**: the lock
/// belongs to the session, so releasing it from another connection does
/// nothing. Dropping the connection also releases it, which is what makes a
/// crashed replica safe.
pub async fn try_worker_lock(
    pool: &PgPool,
    key: i64,
) -> Option<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    let mut conn = pool.acquire().await.ok()?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(&mut *conn)
        .await
        .ok()?;
    if got { Some(conn) } else { None }
}

/// Release the advisory lock `key` held on `conn`.
///
/// A failure here is logged rather than returned: the caller has finished its
/// work and has nothing left to decide. It is not ignored, though — the
/// connection goes back to the pool still holding the lock, and every later
/// cycle of that worker would find it taken and skip, so a silent failure here
/// looks exactly like a worker that stopped running.
pub async fn release_worker_lock(conn: &mut sqlx::PgConnection, key: i64) {
    if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(conn)
        .await
    {
        tracing::warn!(
            error = %e,
            lock_key = key,
            "worker advisory lock was not released; later cycles of this worker will skip until the connection is recycled"
        );
    }
}
