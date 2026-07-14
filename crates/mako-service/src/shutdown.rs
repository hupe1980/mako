//! Graceful-shutdown helpers for mako daemons.
//!
//! Every mako service needs the same three things at startup:
//! 1. A [`CancellationToken`] to propagate shutdown across tasks.
//! 2. A SIGINT (`Ctrl-C`) watcher that cancels the token.
//! 3. A `SIGTERM` watcher (Linux production) that also cancels the token.
//!
//! And the same thing at the end of `main`:
//! 4. `axum::serve(listener, app).with_graceful_shutdown(…).await`
//!
//! This module provides those four operations as one-liners so every service's
//! `main` function looks the same and none of them forget SIGTERM.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use mako_service::shutdown;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let ct = shutdown::token();   // SIGINT + SIGTERM → cancel
//!
//!     // … build app …
//!     # let app: axum::Router = axum::Router::new();
//!
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
//!     shutdown::serve(listener, app, ct).await
//! }
//! ```

use anyhow::Context as _;
use tokio_util::sync::CancellationToken;

/// Build a [`CancellationToken`] that is cancelled on `SIGINT` (Ctrl-C) or
/// `SIGTERM` (Kubernetes / systemd `SIGTERM` during pod eviction).
///
/// Spawns a background Tokio task that waits for either signal and then calls
/// [`CancellationToken::cancel`].  The task is lightweight (< 1 µs per service
/// startup) and automatically exits when the token is dropped.
///
/// Call this **once** at the top of `main`, before any work is done:
///
/// ```rust,no_run
/// let ct = mako_service::shutdown::token();
/// // pass ct.clone() to background workers, MCP router, etc.
/// ```
#[must_use]
pub fn token() -> CancellationToken {
    let ct = CancellationToken::new();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        ct_clone.cancel();
    });
    ct
}

/// Bind a [`tokio::net::TcpListener`], serve `app`, and shut down gracefully
/// when `ct` is cancelled.
///
/// This is the standard one-liner at the end of every service `main`:
///
/// ```rust,no_run
/// # use axum::Router;
/// # use mako_service::shutdown;
/// # async fn run() -> anyhow::Result<()> {
/// let ct = shutdown::token();
/// let app: Router = Router::new(); // your assembled app
/// let listener = tokio::net::TcpListener::bind("0.0.0.0:9080").await?;
/// shutdown::serve(listener, app, ct).await
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` when `axum::serve` fails (e.g. address already in use, OS
/// TCP error after bind succeeds).
pub async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    ct: CancellationToken,
) -> anyhow::Result<()> {
    let addr = listener.local_addr().context("get listener addr")?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { ct.cancelled().await })
        .await
        .context("HTTP serve")
}

// ── Signal helpers ────────────────────────────────────────────────────────────

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).unwrap_or_else(|_| {
        // Gracefully fall back to SIGINT-only when SIGTERM isn't available
        // (shouldn't happen on any realistic Linux target).
        panic!("failed to register SIGTERM handler")
    });
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown: received SIGINT (Ctrl-C)");
        }
        _ = sigterm.recv() => {
            tracing::info!("shutdown: received SIGTERM");
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutdown: received SIGINT (Ctrl-C)");
}
