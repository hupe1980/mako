//! PostgreSQL storage: [`PgSubscriptionRepository`].

use mako_markt::{error::MdmError, repository::Subscription};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed subscription repository.
#[derive(Clone, Debug)]
pub struct PgSubscriptionRepository {
    pool: PgPool,
}

impl PgSubscriptionRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const SELECT_COLS: &str =
    "subscriber_id, webhook_url, webhook_secret, roles, event_types, sparten, active, version";

impl PgSubscriptionRepository {
    /// Insert or update a subscription.
    ///
    /// `webhook_secret` is the HMAC signing key and is stored **in plaintext** —
    /// it is an integrity secret a subscriber uses to verify a delivery came
    /// from this hub, not a confidentiality key over customer data. Protect it
    /// with database-level controls (least-privilege grants on `subscriptions`,
    /// storage encryption); see the marktd README, "Webhook secret at rest".
    ///
    /// Returns the new version number.
    pub async fn upsert(&self, sub: Subscription) -> Result<i64, MdmError> {
        // The version is bumped in SQL and returned: a read-then-write would let
        // two concurrent PUTs settle on the same version, silently losing one.
        let new_version: i64 = sqlx::query_scalar(
            r#"INSERT INTO subscriptions
                   (subscriber_id, webhook_url, webhook_secret, roles, event_types, sparten, active, version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, 1, now())
               ON CONFLICT (subscriber_id) DO UPDATE
               SET webhook_url    = EXCLUDED.webhook_url,
                   webhook_secret = EXCLUDED.webhook_secret,
                   roles          = EXCLUDED.roles,
                   event_types    = EXCLUDED.event_types,
                   sparten        = EXCLUDED.sparten,
                   active         = EXCLUDED.active,
                   version        = subscriptions.version + 1,
                   updated_at     = now()
               RETURNING version"#,
        )
        .bind(&sub.subscriber_id)
        .bind(&sub.webhook_url)
        .bind(&sub.webhook_secret)
        .bind(&sub.roles)
        .bind(&sub.event_types)
        .bind(&sub.sparten)
        .bind(sub.active)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    /// Return a subscription by subscriber ID.
    pub async fn find(&self, subscriber_id: &str) -> Result<Option<Subscription>, MdmError> {
        let row: Option<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM subscriptions WHERE subscriber_id = $1"
        )))
        .bind(subscriber_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_sub))
    }

    /// Deactivate a subscription so it stops matching future fan-outs.
    ///
    /// A soft delete by design: `event_delivery` rows reference the subscriber
    /// and are the § 147 AO / GoBD record that a market event was (or was not)
    /// delivered, so the row itself must survive. Returns `false` when no such
    /// subscription exists.
    pub async fn deactivate(&self, subscriber_id: &str) -> Result<bool, MdmError> {
        // Bumps `version` too, so a concurrent GET/PUT round-trip observes the
        // change rather than re-writing a stale `active = true`.
        let affected = sqlx::query(
            "UPDATE subscriptions
                SET active = false, version = version + 1, updated_at = now()
              WHERE subscriber_id = $1 AND active = true",
        )
        .bind(subscriber_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .rows_affected();

        Ok(affected > 0)
    }

    /// List all active subscriptions.
    pub async fn list_active(&self) -> Result<Vec<Subscription>, MdmError> {
        let rows: Vec<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM subscriptions WHERE active = true ORDER BY subscriber_id"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_sub).collect())
    }

    /// Return all active subscriptions that match a given event type and role.
    ///
    /// Used by the fan-out worker to select delivery targets.
    ///
    /// `role` and `sparte` are both `None` when the event does not carry one.
    /// That means *not scoped by this axis* and matches every subscriber —
    /// an event with no Marktrolle is not an event for the empty Marktrolle.
    pub async fn list_matching(
        &self,
        event_type: &str,
        role: Option<&str>,
        sparte: Option<&str>,
    ) -> Result<Vec<Subscription>, MdmError> {
        // Push role and sparte filters to SQL; wildcard event_type matching
        // stays in Rust because it requires prefix-glob logic.
        let rows: Vec<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            r#"SELECT {SELECT_COLS} FROM subscriptions
               WHERE active = true
                 AND ($1::text IS NULL OR roles = '{{}}' OR $1 = ANY(roles))
                 AND ($2::text IS NULL OR sparten = '{{}}' OR $2 = ANY(sparten))
               ORDER BY subscriber_id"#
        )))
        .bind(role)
        .bind(sparte)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(row_to_sub)
            .filter(|s| {
                // Canonical shared matcher (mako_events::matches): exact,
                // trailing-`*` prefix (the historical semantics), and full
                // glob patterns all work.
                s.event_types.is_empty()
                    || s.event_types
                        .iter()
                        .any(|t| mako_events::matches(t, event_type))
            })
            .collect())
    }
}

fn row_to_sub(r: PgRow) -> Subscription {
    Subscription {
        subscriber_id: r.get("subscriber_id"),
        webhook_url: r.get("webhook_url"),
        webhook_secret: r.get("webhook_secret"),
        roles: r.get("roles"),
        event_types: r.get("event_types"),
        sparten: r.get("sparten"),
        active: r.get("active"),
        version: r.get("version"),
    }
}
