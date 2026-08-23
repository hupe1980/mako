//! The delivery worker: drains pending deliveries with backoff and a ceiling.
//!
//! A short poll and a bounded batch; `FOR UPDATE SKIP LOCKED` makes replicas
//! safe. The decisions that matter — what counts as delivered, what is
//! retryable — live in [`super::channel`] and the schema.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use super::channel::{Channel, DeliveryOutcome, Relay, send_to_relay};
use super::store;
use crate::config::{DeliveryConfig, OutputdConfig};

/// How long a claimed delivery is held before another replica may retry it,
/// and the first retry interval — doubling per attempt up to [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(6 * 3600);
/// Deliveries claimed per tick.
const BATCH: i64 = 32;
/// How often the loop looks for work.
const POLL: Duration = Duration::from_secs(20);

fn backoff(attempts: i32) -> time::Duration {
    let shift = u32::try_from(attempts.clamp(0, 16)).unwrap_or(0);
    let secs = BASE_BACKOFF
        .as_secs()
        .saturating_mul(1u64 << shift.min(20))
        .min(MAX_BACKOFF.as_secs());
    time::Duration::seconds(i64::try_from(secs).unwrap_or(3600))
}

/// Run the delivery loop until `shutdown` fires.
pub async fn run(
    pool: PgPool,
    service: Arc<OutputdConfig>,
    http: reqwest::Client,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let tenant = service.tenant.clone();
    let cfg = &service.delivery;
    if !cfg.enabled {
        tracing::info!(
            "outputd: document delivery is disabled ([delivery] enabled = false) — documents \
             are still stored and served, and every queued delivery stays PENDING"
        );
        return;
    }
    tracing::info!(
        email_relay = cfg.email_relay_url.is_some(),
        postal_relay = cfg.postal_relay_url.is_some(),
        erp_webhook = cfg.erp_webhook_url.is_some(),
        max_attempts = cfg.max_attempts,
        "outputd: document delivery worker started"
    );
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("outputd: delivery worker shutting down");
                return;
            }
            () = tokio::time::sleep(POLL) => {}
        }
        if let Err(e) = tick(&pool, &tenant, cfg, &http).await {
            tracing::warn!(error = %e, "outputd: delivery tick failed");
        }
    }
}

async fn tick(
    pool: &PgPool,
    tenant: &str,
    cfg: &DeliveryConfig,
    http: &reqwest::Client,
) -> anyhow::Result<()> {
    let due = store::claim_due(pool, tenant, BATCH, backoff(0)).await?;
    for delivery in due {
        let result = deliver(pool, tenant, cfg, http, &delivery).await;
        match result {
            Ok(outcome) => {
                store::record_success(
                    pool,
                    tenant,
                    delivery.delivery_id,
                    outcome.delivered,
                    outcome.evidence,
                )
                .await?;
                tracing::info!(
                    delivery_id = %delivery.delivery_id,
                    channel = %delivery.channel,
                    kind = %delivery.kind,
                    subject = %delivery.subject_ref,
                    delivered = outcome.delivered,
                    "outputd: document delivered"
                );
            }
            Err(e) => {
                let give_up = delivery.attempts >= cfg.max_attempts;
                store::record_failure(
                    pool,
                    tenant,
                    delivery.delivery_id,
                    &format!("{e:#}"),
                    give_up,
                    backoff(delivery.attempts),
                )
                .await?;
                if give_up {
                    // The outcome an operator must not have to go looking for:
                    // the platform believes it communicated something and the
                    // customer never received it. A § 41f notice in this state
                    // leaves the sequence resting on it with no basis.
                    tracing::error!(
                        delivery_id = %delivery.delivery_id,
                        channel = %delivery.channel,
                        kind = %delivery.kind,
                        subject = %delivery.subject_ref,
                        attempts = delivery.attempts,
                        error = %e,
                        "outputd: document delivery FAILED permanently — the recipient did not \
                         receive it and no further attempt will be made"
                    );
                } else {
                    tracing::warn!(
                        delivery_id = %delivery.delivery_id,
                        channel = %delivery.channel,
                        attempts = delivery.attempts,
                        error = %e,
                        "outputd: delivery attempt failed, will retry"
                    );
                }
            }
        }
    }
    Ok(())
}

async fn deliver(
    pool: &PgPool,
    tenant: &str,
    cfg: &DeliveryConfig,
    http: &reqwest::Client,
    delivery: &store::PendingDelivery,
) -> anyhow::Result<DeliveryOutcome> {
    match delivery.channel {
        // Publishing *is* the delivery: the document is in the store and
        // `portald` serves it from there, which is why this channel can claim
        // arrival by itself.
        Channel::Portal => Ok(DeliveryOutcome {
            delivered: true,
            evidence: Some(serde_json::json!({
                "published": "portal-inbox",
                "document_id": delivery.document_id,
            })),
        }),
        Channel::Email => {
            let relay = relay(
                cfg.email_relay_url.as_deref(),
                cfg.email_relay_api_key.clone(),
            )
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no [delivery] email_relay_url configured — an EMAIL channel was \
                         requested and this deployment has nothing to send it with"
                )
            })?;
            let to = delivery
                .target
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("EMAIL delivery has no target address"))?;
            let body = relay_body(pool, tenant, cfg, delivery, Some(to)).await?;
            send_to_relay(http, &relay, &body).await
        }
        // A print service *pulls* from `GET /api/v1/spool`; the push is for
        // partners that offer an endpoint. With neither, the row stays PENDING
        // and shows in the spool — correctly, since the letter is unsent.
        Channel::Post => {
            let Some(relay) = relay(
                cfg.postal_relay_url.as_deref(),
                cfg.postal_relay_api_key.clone(),
            ) else {
                anyhow::bail!(
                    "no [delivery] postal_relay_url configured — this document is waiting in \
                     GET /api/v1/spool for a print service to collect"
                );
            };
            let body = relay_body(pool, tenant, cfg, delivery, None).await?;
            send_to_relay(http, &relay, &body).await
        }
        Channel::Erp => {
            let relay = relay(cfg.erp_webhook_url.as_deref(), cfg.erp_api_key.clone())
                .ok_or_else(|| anyhow::anyhow!("no [delivery] erp_webhook_url configured"))?;
            let body = relay_body(pool, tenant, cfg, delivery, None).await?;
            send_to_relay(http, &relay, &body).await
        }
    }
}

fn relay(url: Option<&str>, api_key: Option<secrecy::SecretString>) -> Option<Relay> {
    url.filter(|u| !u.is_empty()).map(|u| Relay {
        url: u.to_owned(),
        api_key,
    })
}

/// The JSON a relay receives.
///
/// Carries the delivery id, so a relay reporting asynchronously through
/// `POST /api/v1/deliveries/{id}/status` has a key to report against, and the
/// document base64-encoded, so an adapter needs nothing but a JSON parser.
async fn relay_body(
    pool: &PgPool,
    tenant: &str,
    cfg: &DeliveryConfig,
    delivery: &store::PendingDelivery,
    to: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    use base64::Engine as _;
    let (bytes, media_type) = store::content(pool, tenant, delivery.document_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("document {} has no content", delivery.document_id))?;
    let filename = format!(
        "{}-{}.{}",
        delivery.kind.to_ascii_lowercase(),
        delivery.subject_ref.replace(['/', '\\', ' '], "-"),
        if media_type == "application/pdf" {
            "pdf"
        } else {
            "bin"
        }
    );
    Ok(serde_json::json!({
        "delivery_id":    delivery.delivery_id,
        "document_id":    delivery.document_id,
        "channel":        delivery.channel.as_str(),
        "kind":           delivery.kind,
        "subject_ref":    delivery.subject_ref,
        "malo_id":        delivery.malo_id,
        "kunden_nr":      delivery.kunden_nr,
        "attempt":        delivery.attempts,
        "from":           cfg.from_address,
        "to":             to,
        "recipient_name": delivery.recipient_name,
        "subject":        cfg.subject_for(&delivery.kind),
        "filename":       filename,
        "media_type":     media_type,
        "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
    }))
}

#[cfg(test)]
mod tests {
    use super::backoff;

    /// Backoff doubles and then saturates, so a broken relay is retried at a
    /// bounded rate rather than hammered or abandoned.
    #[test]
    fn backoff_doubles_and_saturates() {
        assert_eq!(backoff(0), time::Duration::seconds(60));
        assert_eq!(backoff(1), time::Duration::seconds(120));
        assert_eq!(backoff(4), time::Duration::seconds(960));
        assert_eq!(backoff(30), time::Duration::seconds(6 * 3600));
        // Never zero: that would release the claim immediately and let two
        // replicas race on every attempt.
        assert!(backoff(-5) > time::Duration::ZERO);
    }
}
