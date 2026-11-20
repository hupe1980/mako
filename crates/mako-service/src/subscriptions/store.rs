//! The endpoint and delivery tables, and the statements over them.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// DDL for the two tables. Idempotent; [`super::ensure_schema`] runs it.
///
/// An endpoint holds at most two secrets: the current one and, for a stated
/// overlap after a rotation, the previous one — both sign while both are
/// active. A delivery is one event to one endpoint; it carries the event's
/// commit position `(event_txid, event_seq)` and its ordering key, which is
/// what holds a later event of the same key back while an earlier one is
/// outstanding.
pub const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id                         UUID        PRIMARY KEY,
    tenant                     TEXT        NOT NULL,
    name                       TEXT        NOT NULL,
    url                        TEXT        NOT NULL,
    event_types                TEXT[]      NOT NULL DEFAULT '{}',
    extensions                 JSONB       NOT NULL DEFAULT '{}',
    owner                      TEXT        NOT NULL CHECK (owner IN ('config', 'api')),
    partner                    TEXT,
    active                     BOOLEAN     NOT NULL DEFAULT true,
    deactivated_reason         TEXT,
    secret                     TEXT,
    previous_secret            TEXT,
    previous_secret_expires_at TIMESTAMPTZ,
    version                    BIGINT      NOT NULL DEFAULT 1,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, name),
    -- The config URL may run unsigned in development; an API endpoint never.
    CONSTRAINT webhook_endpoints_api_signed CHECK (owner = 'config' OR secret IS NOT NULL),
    CONSTRAINT webhook_endpoints_partner_api CHECK (owner = 'api' OR partner IS NULL)
);
CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant           TEXT        NOT NULL,
    endpoint_id      UUID        NOT NULL REFERENCES webhook_endpoints (id) ON DELETE CASCADE,
    event_id         TEXT        NOT NULL,
    ce_type          TEXT        NOT NULL,
    event_txid       BIGINT      NOT NULL,
    event_seq        BIGINT      NOT NULL,
    ordering_key     TEXT,
    attempts         SMALLINT    NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at     TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    last_status      SMALLINT,
    last_error       TEXT,
    latency_ms       INTEGER,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (endpoint_id, event_id)
);
CREATE INDEX IF NOT EXISTS webhook_deliveries_due ON webhook_deliveries (event_txid, event_seq)
    WHERE delivered_at IS NULL AND dead_lettered_at IS NULL;
CREATE INDEX IF NOT EXISTS webhook_deliveries_key ON webhook_deliveries (endpoint_id, ordering_key, event_txid, event_seq)
    WHERE delivered_at IS NULL AND dead_lettered_at IS NULL AND ordering_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS webhook_deliveries_log ON webhook_deliveries (endpoint_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS event_outbox_window ON event_outbox (created_at)
    WHERE kind = 'cloudevent' AND upstream IS NULL;";

/// Who owns an endpoint.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    /// The service's config names it (`default`); listed, delivered to and
    /// replayable, never writable through the API.
    Config,
    /// Registered through the API.
    Api,
}

impl Owner {
    fn parse(s: &str) -> Self {
        if s == "config" {
            Self::Config
        } else {
            Self::Api
        }
    }
}

/// A webhook endpoint, as listed. Its secrets are never part of it.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, utoipa::ToSchema,
)]
pub struct Endpoint {
    /// The endpoint's id.
    #[schemars(extend("format" = "uuid"))]
    #[schema(format = Uuid)]
    pub id: String,
    /// Unique per service; `default` is the config URL.
    pub name: String,
    /// Where deliveries are POSTed.
    pub url: String,
    /// `mako_events::matches` patterns; empty matches every type.
    pub event_types: Vec<String>,
    /// The patterns of `event_types` that match no event type mako emits.
    /// They deliver nothing; replace them.
    pub patterns_matching_nothing: Vec<String>,
    /// CloudEvent extensions an event must carry with these values
    /// (`marktsparte`, `marktrole`).
    pub extensions: BTreeMap<String, String>,
    /// `config` or `api`.
    pub owner: Owner,
    /// The Vertriebspartner the endpoint is bound to; it receives only events
    /// stamped with that partner.
    pub partner: Option<String>,
    /// Whether deliveries are made. A `410 Gone` deactivates an endpoint.
    pub active: bool,
    /// Why the endpoint was deactivated (`gone`).
    pub deactivated_reason: Option<String>,
    /// Until when the previous secret still signs, after a rotation.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub previous_secret_expires_at: Option<String>,
    /// The version `If-Match` names.
    pub version: i64,
    /// When the endpoint was registered.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub created_at: String,
}

/// An endpoint as the fan-out and delivery read it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct EndpointRow {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub extensions: serde_json::Value,
    pub owner: String,
    pub partner: Option<String>,
    pub active: bool,
    pub deactivated_reason: Option<String>,
    pub previous_secret_expires_at: Option<OffsetDateTime>,
    pub version: i64,
    pub created_at: OffsetDateTime,
}

pub(crate) const ENDPOINT_COLS: &str = "id, name, url, event_types, extensions, owner, partner, \
     active, deactivated_reason, previous_secret_expires_at, version, created_at";

impl EndpointRow {
    pub fn extensions(&self) -> BTreeMap<String, String> {
        self.extensions
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn owner(&self) -> Owner {
        Owner::parse(&self.owner)
    }

    /// Whether `envelope` is an event this endpoint receives: a type pattern
    /// matches (or none is set), every named extension is carried with its
    /// value, and a partner endpoint's partner is the event's.
    pub fn matches(&self, envelope: &serde_json::Value) -> bool {
        let ce_type = envelope["type"].as_str().unwrap_or_default();
        let typed = self.event_types.is_empty()
            || self
                .event_types
                .iter()
                .any(|p| mako_events::matches(p, ce_type));
        let extended = self
            .extensions()
            .iter()
            .all(|(k, v)| envelope[k.as_str()].as_str() == Some(v.as_str()));
        let partnered = self
            .partner
            .as_deref()
            .is_none_or(|p| envelope[super::PARTNER_EXTENSION].as_str() == Some(p));
        typed && extended && partnered
    }

    pub fn into_endpoint(self) -> Endpoint {
        let extensions = self.extensions();
        let owner = self.owner();
        Endpoint {
            id: self.id.to_string(),
            name: self.name,
            url: self.url,
            patterns_matching_nothing: super::silent_patterns(&self.event_types),
            event_types: self.event_types,
            extensions,
            owner,
            partner: self.partner,
            active: self.active,
            deactivated_reason: self.deactivated_reason,
            previous_secret_expires_at: self.previous_secret_expires_at.map(rfc3339),
            version: self.version,
            created_at: rfc3339(self.created_at),
        }
    }
}

/// A delivery's state.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Owed: not yet attempted, or retrying.
    Pending,
    /// Accepted by the endpoint with a 2xx.
    Delivered,
    /// Given up after its attempts, or refused with `410 Gone`.
    DeadLettered,
}

/// One event to one endpoint.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, utoipa::ToSchema,
)]
pub struct Delivery {
    /// The delivery's id — what a single replay names.
    #[schemars(extend("format" = "uuid"))]
    #[schema(format = Uuid)]
    pub id: String,
    /// The endpoint delivered to.
    #[schemars(extend("format" = "uuid"))]
    #[schema(format = Uuid)]
    pub endpoint_id: String,
    /// The CloudEvent `id`, sent as `webhook-id`.
    pub event_id: String,
    /// The CloudEvent `type`.
    pub event_type: String,
    /// The aggregate the event is about; deliveries of one key arrive in order.
    pub ordering_key: Option<String>,
    /// `pending`, `delivered` or `dead_lettered`.
    pub status: DeliveryStatus,
    /// Attempts made.
    pub attempts: i16,
    /// The HTTP status of the last attempt; absent when it got no answer.
    pub last_status: Option<i16>,
    /// The last attempt's failure.
    pub last_error: Option<String>,
    /// How long the last answered attempt took, in milliseconds.
    pub latency_ms: Option<i32>,
    /// When the next attempt is due, while pending.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub next_attempt_at: Option<String>,
    /// When the delivery was created.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub created_at: String,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct DeliveryRow {
    pub id: Uuid,
    pub endpoint_id: Uuid,
    pub event_id: String,
    pub ce_type: String,
    pub ordering_key: Option<String>,
    pub attempts: i16,
    pub next_attempt_at: OffsetDateTime,
    pub delivered_at: Option<OffsetDateTime>,
    pub dead_lettered_at: Option<OffsetDateTime>,
    pub last_status: Option<i16>,
    pub last_error: Option<String>,
    pub latency_ms: Option<i32>,
    pub created_at: OffsetDateTime,
}

pub(crate) const DELIVERY_COLS: &str = "id, endpoint_id, event_id, ce_type, ordering_key, \
     attempts, next_attempt_at, delivered_at, dead_lettered_at, last_status, last_error, \
     latency_ms, created_at";

impl DeliveryRow {
    pub fn into_delivery(self) -> Delivery {
        let status = if self.delivered_at.is_some() {
            DeliveryStatus::Delivered
        } else if self.dead_lettered_at.is_some() {
            DeliveryStatus::DeadLettered
        } else {
            DeliveryStatus::Pending
        };
        Delivery {
            id: self.id.to_string(),
            endpoint_id: self.endpoint_id.to_string(),
            event_id: self.event_id,
            event_type: self.ce_type,
            ordering_key: self.ordering_key,
            status,
            attempts: self.attempts,
            last_status: self.last_status,
            last_error: self.last_error,
            latency_ms: self.latency_ms,
            next_attempt_at: (status == DeliveryStatus::Pending)
                .then(|| rfc3339(self.next_attempt_at)),
            created_at: rfc3339(self.created_at),
        }
    }
}

/// `t` as RFC 3339.
pub(crate) fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// A fresh Standard Webhooks secret: `whsec_` and 32 random bytes in base64.
pub(crate) fn new_secret() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    format!(
        "whsec_{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Write the config-owned `default` endpoint: upsert it when the config names
/// a URL, deactivate it when the config no longer does.
/// One config-owned endpoint as [`sync_config`] writes it.
pub(crate) struct ConfigOwned<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub secret: Option<&'a str>,
    pub event_types: &'a [String],
    pub extensions: serde_json::Value,
}

/// Upsert every config-owned endpoint and deactivate the config-owned ones
/// `owned` no longer names, in one transaction.
pub(crate) async fn sync_config(
    pool: &PgPool,
    tenant: &str,
    owned: &[ConfigOwned<'_>],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for e in owned {
        sqlx::query(
            "INSERT INTO webhook_endpoints (id, tenant, name, url, owner, secret, event_types, extensions)
             VALUES ($1, $2, $3, $4, 'config', $5, $6, $7)
             ON CONFLICT (tenant, name) DO UPDATE
                SET url = EXCLUDED.url, secret = EXCLUDED.secret, owner = 'config',
                    partner = NULL, event_types = EXCLUDED.event_types,
                    extensions = EXCLUDED.extensions,
                    active = true, deactivated_reason = NULL,
                    version = webhook_endpoints.version + 1, updated_at = now()
              WHERE webhook_endpoints.url IS DISTINCT FROM EXCLUDED.url
                 OR webhook_endpoints.secret IS DISTINCT FROM EXCLUDED.secret
                 OR webhook_endpoints.event_types IS DISTINCT FROM EXCLUDED.event_types
                 OR webhook_endpoints.extensions IS DISTINCT FROM EXCLUDED.extensions
                 OR webhook_endpoints.owner <> 'config'
                 OR NOT webhook_endpoints.active",
        )
        .bind(Uuid::new_v4())
        .bind(tenant)
        .bind(e.name)
        .bind(e.url)
        .bind(e.secret)
        .bind(e.event_types)
        .bind(&e.extensions)
        .execute(&mut *tx)
        .await?;
    }
    let names: Vec<&str> = owned.iter().map(|e| e.name).collect();
    sqlx::query(
        "UPDATE webhook_endpoints
            SET active = false, deactivated_reason = 'config_removed',
                version = version + 1, updated_at = now()
          WHERE tenant = $1 AND owner = 'config' AND active AND NOT (name = ANY($2))",
    )
    .bind(tenant)
    .bind(&names)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}
