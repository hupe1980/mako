//! PostgreSQL implementation of [`SubscriptionRepository`].

use mako_mdm::{
    error::MdmError,
    repository::{Subscription, SubscriptionRepository},
};
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

impl SubscriptionRepository for PgSubscriptionRepository {
    async fn upsert(&self, sub: Subscription) -> Result<i64, MdmError> {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM subscriptions WHERE subscriber_id = $1")
                .bind(&sub.subscriber_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = current.map_or(1, |v| v + 1);

        sqlx::query(
            r#"INSERT INTO subscriptions
                   (subscriber_id, webhook_url, webhook_secret, roles, event_types, sparten, active, version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
               ON CONFLICT (subscriber_id) DO UPDATE
               SET webhook_url    = EXCLUDED.webhook_url,
                   webhook_secret = EXCLUDED.webhook_secret,
                   roles          = EXCLUDED.roles,
                   event_types    = EXCLUDED.event_types,
                   sparten        = EXCLUDED.sparten,
                   active         = EXCLUDED.active,
                   version        = EXCLUDED.version,
                   updated_at     = now()"#,
        )
        .bind(&sub.subscriber_id)
        .bind(&sub.webhook_url)
        .bind(&sub.webhook_secret)
        .bind(&sub.roles)
        .bind(&sub.event_types)
        .bind(&sub.sparten)
        .bind(sub.active)
        .bind(new_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, subscriber_id: &str) -> Result<Option<Subscription>, MdmError> {
        let row: Option<PgRow> = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM subscriptions WHERE subscriber_id = $1"
        ))
        .bind(subscriber_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_sub))
    }

    async fn list_active(&self) -> Result<Vec<Subscription>, MdmError> {
        let rows: Vec<PgRow> = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM subscriptions WHERE active = true ORDER BY subscriber_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_sub).collect())
    }

    async fn list_matching(
        &self,
        event_type: &str,
        role: &str,
        sparte: Option<&str>,
    ) -> Result<Vec<Subscription>, MdmError> {
        // Push role and sparte filters to SQL; wildcard event_type matching
        // stays in Rust because it requires prefix-glob logic.
        let rows: Vec<PgRow> = sqlx::query(&format!(
            r#"SELECT {SELECT_COLS} FROM subscriptions
               WHERE active = true
                 AND (roles   = '{{}}' OR $1 = ANY(roles))
                 AND ($2::text IS NULL OR sparten = '{{}}' OR $2 = ANY(sparten))
               ORDER BY subscriber_id"#
        ))
        .bind(role)
        .bind(sparte)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(row_to_sub)
            .filter(|s| {
                s.event_types.is_empty()
                    || s.event_types.iter().any(|t| {
                        t == event_type
                            || (t.ends_with('*') && event_type.starts_with(t.trim_end_matches('*')))
                    })
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
