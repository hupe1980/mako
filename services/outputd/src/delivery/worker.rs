//! The relay workers: one `mako_service::outbox` command worker per pushed
//! channel, and the sweep that marks a delivery the outbox gave up on `FAILED`.
//!
//! A relay is an HTTP endpoint an operator runs — a mail gateway, a print
//! service, their ERP. It receives `POST {url}` with the JSON body
//! [`super::store`] built at issue (the delivery id, the document
//! base64-encoded), authenticated with the relay credential as bearer token and
//! signed with it under [Standard Webhooks]: the body carries the customer's
//! document, their name, e-mail address, MaLo and Kundennummer, so the relay has
//! to tell a mako push from anything else that reached its URL. The
//! `webhook-id` is the delivery, so a retry is the same message. It answers 2xx
//! with an optional receipt (`message_id`, `delivered`), recorded as the
//! evidence in the transaction that marks the command delivered.
//!
//! Retries, backoff and the give-up follow the shared outbox policy.
//!
//! [Standard Webhooks]: https://www.standardwebhooks.com/

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use super::channel::{Channel, RelayReceipt};
use super::store;
use crate::config::OutputdConfig;

/// How often the give-up sweep runs.
const SWEEP: Duration = Duration::from_secs(60);

/// The outbox worker delivering `channel`'s pushes to `url`.
///
/// The relay's receipt becomes the delivery's evidence in the transaction that
/// marks the command delivered: `DELIVERED` when the relay says it observed
/// arrival, `SENT` otherwise.
#[must_use]
pub fn relay_worker(
    pool: PgPool,
    tenant: String,
    channel: Channel,
    url: impl Into<String>,
    api_key: Option<secrecy::SecretString>,
) -> mako_service::outbox::OutboxWorker {
    let tenant = Arc::new(tenant);
    mako_service::outbox::OutboxWorker::commands(pool, store::upstream(channel), url, api_key)
        .signed()
        .on_delivered(Arc::new(move |conn, delivered| {
            let tenant = Arc::clone(&tenant);
            Box::pin(async move {
                let Some(id) = delivered
                    .subject
                    .as_deref()
                    .and_then(store::delivery_of_key)
                else {
                    return Ok(());
                };
                let receipt: RelayReceipt = delivered
                    .reply
                    .clone()
                    .and_then(|r| serde_json::from_value(r).ok())
                    .unwrap_or_default();
                store::record_success(
                    conn,
                    &tenant,
                    id,
                    receipt.delivered.unwrap_or(false),
                    Some(serde_json::json!({ "message_id": receipt.message_id })),
                )
                .await
                .map_err(|e| sqlx::Error::Protocol(format!("{e:#}")))
            })
        }))
}

/// Mark every delivery the outbox gave up on `FAILED`, logging each.
///
/// # Errors
///
/// Propagates database errors.
pub async fn sweep(pool: &PgPool, tenant: &str) -> anyhow::Result<usize> {
    let gave_up = store::fail_dead_lettered(pool, tenant).await?;
    for d in &gave_up {
        // The outcome an operator must not have to go looking for: the
        // platform believes it communicated something and the customer never
        // received it. A § 41f notice in this state leaves the sequence
        // resting on it with no basis.
        tracing::error!(
            delivery_id = %d.delivery_id, channel = %d.channel, attempts = d.attempts,
            error = %d.error,
            "outputd: document delivery FAILED permanently — the recipient did not \
             receive it and no further attempt will be made"
        );
    }
    Ok(gave_up.len())
}

/// Start a relay worker per configured channel and the give-up sweep.
pub fn spawn(
    pool: PgPool,
    service: Arc<OutputdConfig>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let cfg = &service.delivery;
    if !cfg.enabled {
        tracing::info!(
            "outputd: document delivery is disabled ([delivery] enabled = false) — documents \
             are still stored and served, and every queued delivery stays PENDING"
        );
        return;
    }
    for (channel, url, key) in [
        (
            Channel::Email,
            &cfg.email_relay_url,
            &cfg.email_relay_api_key,
        ),
        (
            Channel::Post,
            &cfg.postal_relay_url,
            &cfg.postal_relay_api_key,
        ),
        (Channel::Erp, &cfg.erp_webhook_url, &cfg.erp_api_key),
    ] {
        if let Some(url) = url.as_deref().filter(|u| !u.is_empty()) {
            tokio::spawn(
                relay_worker(
                    pool.clone(),
                    service.tenant.clone(),
                    channel,
                    url,
                    key.clone(),
                )
                .run(shutdown.clone()),
            );
        }
    }
    tracing::info!(
        email_relay = cfg.email_relay_url.is_some(),
        postal_relay = cfg.has_postal_relay(),
        erp_webhook = cfg.erp_webhook_url.is_some(),
        "outputd: document delivery started"
    );
    let tenant = service.tenant.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(SWEEP) => {}
            }
            if let Err(e) = sweep(&pool, &tenant).await {
                tracing::warn!(error = %e, "outputd: delivery give-up sweep failed");
            }
        }
    });
}
