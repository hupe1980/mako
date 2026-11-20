//! Webhook subscriptions on the service's own `event_outbox`.
//!
//! A service that enqueues CloudEvents mounts this module with one call and
//! gets, for every integrator: endpoint CRUD with a `whsec_` secret shown
//! once, secret rotation with an overlap, a test delivery, the delivery log,
//! replay of one delivery or of a time window, and `GET /api/v1/events` — the
//! outbox as a keyset-paged feed. Every route is documented through
//! `utoipa-axum`, and requires the Cedar action `manage-subscription` (the
//! feed: `read-events`).
//!
//! Delivery is [Standard Webhooks]: `webhook-id` the CloudEvent `id` (the key a
//! receiver deduplicates on), a per-attempt `webhook-timestamp`, and one `v1,`
//! signature per active secret. Deliveries of one ordering key (the event's
//! `subject` unless the emitter named another, `outbox::enqueue_ordered`)
//! reach an endpoint in commit order; other keys never wait. A `410 Gone`
//! deactivates the endpoint. An endpoint registered through the API must be
//! `https` and must not reach a loopback, private or link-local address —
//! checked when it is registered and on the address it resolves to at each
//! delivery ([`ssrf`]).
//!
//! The URL the service's config names is the endpoint `default`
//! ([`Subscriptions::with_default_endpoint`]): listed, delivered to and
//! replayable like any other, refused for writes with `endpoint_config_owned`.
//! An in-cluster receiver — whose private address the API refuses — is a
//! `[[subscriptions.endpoints]]` entry ([`ConfigEndpoint`]), config-owned the
//! same way and deactivated when the config drops it.
//!
//! A partner credential — a request the service's authentication layer marks
//! with the [`Partner`] extension — creates endpoints bound to itself, sees
//! only those, and they receive only events whose [`PARTNER_EXTENSION`]
//! names it.
//!
//! ```rust,ignore
//! // Daemon::migrate
//! mako_service::outbox::ensure_schema(pool).await?;
//! mako_service::subscriptions::ensure_schema(pool).await?;
//!
//! // Daemon::build
//! let subs = Subscriptions::new(ctx.pool().clone(), Self::NAME, &cfg.tenant, cfg.subscriptions.clone())
//!     .with_default_endpoint(cfg.erp_webhook_url.clone(), cfg.erp_webhook_secret.clone());
//! subs.spawn(ctx.shutdown.clone()); // retries the endpoint write until the database answers
//! let api = OpenApiRouter::with_openapi(ApiDoc::openapi())
//!     .routes(routes!(handlers::get_thing))
//!     .merge(subs.router());
//! ```
//!
//! [Standard Webhooks]: https://www.standardwebhooks.com/

mod routes;
pub mod ssrf;
mod store;
mod worker;

use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use utoipa_axum::{router::OpenApiRouter, routes};

pub use routes::{
    Caller, DeliveryFilter, DeliveryQuery, EndpointCreate, EndpointCreated, EndpointUpdate,
    FeedQuery, MANAGE, MAX_OVERLAP_HOURS, MAX_WINDOW_EVENTS, READ_EVENTS, ReplayWindow, Replayed,
    RotateSecret, SecretRotated, TestDelivery,
};
pub use store::{Delivery, DeliveryStatus, Endpoint, Owner, SCHEMA};

use crate::outbox::OutboxConfig;

/// The name of the endpoint the service's config owns.
pub const DEFAULT_ENDPOINT: &str = "default";

/// The CloudEvent extension naming the Vertriebspartner an event concerns.
pub const PARTNER_EXTENSION: &str = "vertriebspartner";

/// The `data` of `de.mako.webhook.test`.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct WebhookTest {
    /// The endpoint the test was sent to.
    #[schemars(extend("format" = "uuid"))]
    pub endpoint_id: String,
    /// A fixed text, for a human looking at the receiver's log.
    pub message: String,
}

impl crate::EventPayload for WebhookTest {
    const TYPE: &'static str = mako_events::mako::WEBHOOK_TEST;
}

/// Request extension a service's authentication layer inserts for a partner
/// credential: the partner's id. Its endpoints are bound to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partner(pub String);

/// `[subscriptions]` in a service's config.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionsConfig {
    /// How long delivered events stay in the outbox for replay and the feed.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Admit `http` and loopback endpoints — `just dev`'s local receivers.
    /// Never in production: an endpoint could then reach the host itself.
    #[serde(default)]
    pub allow_loopback: bool,
    /// `[[subscriptions.endpoints]]`: receivers the operator names in the
    /// config — an in-cluster service, which the API refuses because its
    /// address is private. Config-owned like `default`.
    #[serde(default)]
    pub endpoints: Vec<ConfigEndpoint>,
}

/// One `[[subscriptions.endpoints]]` entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigEndpoint {
    /// Unique per service; `default` is the service's own ERP URL.
    pub name: String,
    /// Where deliveries are POSTed; not address-checked, the operator named it.
    pub url: String,
    /// The secret the receiver verifies with (`env:VAR` reads it from the
    /// environment); unsigned when absent.
    #[serde(default)]
    pub secret: Option<SecretString>,
    /// `mako_events::matches` patterns; empty matches every type.
    #[serde(default)]
    pub event_types: Vec<String>,
    /// CloudEvent extensions an event must carry with these values.
    #[serde(default)]
    pub extensions: std::collections::BTreeMap<String, String>,
}

const fn default_retention_days() -> u32 {
    30
}

impl Default for SubscriptionsConfig {
    fn default() -> Self {
        Self {
            retention_days: default_retention_days(),
            allow_loopback: false,
            endpoints: Vec::new(),
        }
    }
}

impl SubscriptionsConfig {
    /// The findings of `[subscriptions]` for `Daemon::validate`: a warning
    /// while `allow_loopback` is on, a blocking one for a config endpoint
    /// named `default` or twice, or with no URL.
    #[must_use]
    pub fn befunde(&self) -> Vec<crate::Befund> {
        use crate::befund::{Befund, Bereich};
        let mut out = Vec::new();
        if self.allow_loopback {
            out.push(
                Befund::warnung(
                    "subscriptions.allow_loopback",
                    "a webhook endpoint may name a loopback or private address over http — the \
                     posture of `just dev`; a deployment leaves it off",
                )
                .bereich(Bereich::Zugriff),
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, e) in self.endpoints.iter().enumerate() {
            let path = format!("subscriptions.endpoints[{i}]");
            if e.name == DEFAULT_ENDPOINT {
                out.push(Befund::blockiert(
                    format!("{path}.name"),
                    "`default` is the service's own ERP URL; name this endpoint otherwise",
                ));
            } else if e.name.trim().is_empty() || !seen.insert(e.name.as_str()) {
                out.push(Befund::blockiert(
                    format!("{path}.name"),
                    format!("endpoint name {:?} is empty or named twice", e.name),
                ));
            }
            if let Some(Err(var)) = e.secret.as_ref().map(resolved) {
                out.push(Befund::blockiert(
                    format!("{path}.secret"),
                    format!("the secret names the unset environment variable {var}"),
                ));
            }
            if e.url.trim().is_empty() {
                out.push(Befund::blockiert(
                    format!("{path}.url"),
                    "a config endpoint needs a URL",
                ));
            }
            let silent = silent_patterns(&e.event_types);
            if !silent.is_empty() {
                out.push(Befund::blockiert(
                    format!("{path}.event_types"),
                    format!(
                        "{silent:?} match no event type mako emits, so the endpoint would \
                         receive nothing — every type is `de.mako.<domain>.<fact>`"
                    ),
                ));
            }
        }
        out
    }
}

/// The patterns of `event_types` that match no catalogued type.
///
/// Such a pattern delivers nothing and says nothing about it: a subscriber
/// with an outdated type name or a typo waits for events that never come. `*`
/// matches everything and is never silent.
#[must_use]
pub fn silent_patterns(event_types: &[String]) -> Vec<String> {
    event_types
        .iter()
        .filter(|p| {
            !mako_events::all()
                .iter()
                .any(|ty| mako_events::matches(p, ty))
        })
        .cloned()
        .collect()
}

/// A config secret, `env:VAR` read from the environment.
fn resolved(secret: &SecretString) -> Result<SecretString, String> {
    crate::config::resolve_env_secret(secret.expose_secret()).map_err(|e| e.var)
}

/// Create the endpoint and delivery tables. Run it after
/// [`crate::outbox::ensure_schema`], whose table it indexes.
///
/// # Errors
///
/// A database error.
pub async fn ensure_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub(crate) struct Inner {
    pub pool: PgPool,
    pub service: String,
    pub tenant: String,
    pub cfg: SubscriptionsConfig,
    pub policy: OutboxConfig,
    pub default_url: Option<String>,
    pub default_secret: Option<SecretString>,
    /// Deliveries to the config URL: the operator named it, so no address
    /// check — an in-cluster ERP adapter is private by design.
    pub config_client: reqwest::Client,
    /// Deliveries to API endpoints: the resolved-address check.
    pub api_client: reqwest::Client,
}

/// The module, bound to one service's pool and tenant. Cheap to clone.
#[derive(Clone)]
pub struct Subscriptions {
    pub(crate) inner: Arc<Inner>,
}

impl Subscriptions {
    /// The module for `service` (its `Daemon::NAME`) and `tenant`.
    #[must_use]
    pub fn new(
        pool: PgPool,
        service: impl Into<String>,
        tenant: impl Into<String>,
        cfg: SubscriptionsConfig,
    ) -> Self {
        let api_client = ssrf::guarded_client(cfg.allow_loopback);
        Self {
            inner: Arc::new(Inner {
                pool,
                service: service.into(),
                tenant: tenant.into(),
                cfg,
                policy: OutboxConfig::default(),
                default_url: None,
                default_secret: None,
                config_client: crate::http::default_client_with(std::time::Duration::from_secs(10)),
                api_client,
            }),
        }
    }

    fn with_inner(self, f: impl FnOnce(&mut Inner)) -> Self {
        let mut inner = Arc::try_unwrap(self.inner).unwrap_or_else(|arc| Inner {
            pool: arc.pool.clone(),
            service: arc.service.clone(),
            tenant: arc.tenant.clone(),
            cfg: arc.cfg.clone(),
            policy: arc.policy.clone(),
            default_url: arc.default_url.clone(),
            default_secret: arc.default_secret.clone(),
            config_client: arc.config_client.clone(),
            api_client: arc.api_client.clone(),
        });
        f(&mut inner);
        Self {
            inner: Arc::new(inner),
        }
    }

    /// The endpoint `default`: the URL the service's config names, signed
    /// with `secret` when one is configured. `None` deactivates it.
    #[must_use]
    pub fn with_default_endpoint(self, url: Option<String>, secret: Option<SecretString>) -> Self {
        self.with_inner(|i| {
            i.default_url = url.filter(|u| !u.trim().is_empty());
            i.default_secret = secret;
        })
    }

    /// Override the delivery schedule (attempts, backoff, lease, poll).
    #[must_use]
    pub fn with_policy(self, policy: OutboxConfig) -> Self {
        self.with_inner(|i| i.policy = policy)
    }

    /// Write the config-owned endpoints and spawn the fan-out and delivery
    /// worker on `shutdown`.
    ///
    /// # Errors
    ///
    /// A database error writing the config-owned endpoints.
    pub async fn start(&self, shutdown: CancellationToken) -> Result<(), sqlx::Error> {
        self.sync_config().await?;
        tokio::spawn(self.clone().run(shutdown));
        Ok(())
    }

    /// [`Self::start`] in the background, for `Daemon::build`: the endpoint
    /// write is retried until the database answers, so a service boots while
    /// its database is briefly away; events wait in the outbox meanwhile.
    pub fn spawn(&self, shutdown: CancellationToken) {
        let subs = self.clone();
        tokio::spawn(async move {
            loop {
                match subs.sync_config().await {
                    Ok(()) => break,
                    Err(e) => tracing::warn!(service = %subs.inner.service, error = %e,
                        "subscriptions: config endpoints not written — retrying in 5 s"),
                }
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                }
            }
            subs.warn_silent_endpoints().await;
            subs.run(shutdown).await;
        });
    }

    /// Log every active endpoint with a pattern that matches no catalogued
    /// type. The endpoint keeps the pattern and its read shows it as
    /// `patterns_matching_nothing`; the log is where an operator learns of it
    /// without reading every endpoint.
    async fn warn_silent_endpoints(&self) {
        let rows: Result<Vec<(String, Vec<String>)>, _> = sqlx::query_as(
            "SELECT name, event_types FROM webhook_endpoints WHERE tenant = $1 AND active",
        )
        .bind(&self.inner.tenant)
        .fetch_all(&self.inner.pool)
        .await;
        match rows {
            Ok(rows) => {
                for (name, event_types) in rows {
                    let silent = silent_patterns(&event_types);
                    if !silent.is_empty() {
                        tracing::warn!(
                            service = %self.inner.service, endpoint = %name, patterns = ?silent,
                            "subscriptions: endpoint patterns match no event type — it receives \
                             nothing for them; every type is `de.mako.<domain>.<fact>`"
                        );
                    }
                }
            }
            Err(e) => tracing::warn!(service = %self.inner.service, error = %e,
                "subscriptions: endpoints not read for the pattern check"),
        }
    }

    /// Write the config-owned endpoints — `default` and every
    /// `[[subscriptions.endpoints]]` entry — and deactivate the config-owned
    /// ones the config no longer names.
    ///
    /// # Errors
    ///
    /// A database error.
    pub async fn sync_config(&self) -> Result<(), sqlx::Error> {
        let mut owned = Vec::with_capacity(self.inner.cfg.endpoints.len() + 1);
        if let Some(url) = self.inner.default_url.as_deref() {
            owned.push(store::ConfigOwned {
                name: DEFAULT_ENDPOINT,
                url,
                secret: self
                    .inner
                    .default_secret
                    .as_ref()
                    .map(ExposeSecret::expose_secret),
                event_types: &[],
                extensions: serde_json::json!({}),
            });
        }
        // An `env:` secret is read when it is written, so a rotated variable
        // takes effect on the next start; an unset one is a blocking finding.
        let secrets: Vec<Option<SecretString>> = self
            .inner
            .cfg
            .endpoints
            .iter()
            .map(|e| e.secret.as_ref().and_then(|s| resolved(s).ok()))
            .collect();
        for (i, e) in self.inner.cfg.endpoints.iter().enumerate() {
            owned.push(store::ConfigOwned {
                name: &e.name,
                url: &e.url,
                secret: secrets[i].as_ref().map(ExposeSecret::expose_secret),
                event_types: &e.event_types,
                extensions: serde_json::to_value(&e.extensions)
                    .unwrap_or_else(|_| serde_json::json!({})),
            });
        }
        store::sync_config(&self.inner.pool, &self.inner.tenant, &owned).await
    }

    /// The routes, documented, for the service to merge into its own
    /// `OpenApiRouter` — behind its authentication layers, which supply the
    /// `OidcVerifier`, the `CedarEnforcer` and, for a partner, [`Partner`].
    pub fn router<S: Clone + Send + Sync + 'static>(&self) -> OpenApiRouter<S> {
        OpenApiRouter::<Self>::with_openapi(<Components as utoipa::OpenApi>::openapi())
            .routes(routes!(routes::list_endpoints, routes::create_endpoint))
            .routes(routes!(
                routes::get_endpoint,
                routes::update_endpoint,
                routes::delete_endpoint
            ))
            .routes(routes!(routes::rotate_secret))
            .routes(routes!(routes::test_endpoint))
            .routes(routes!(routes::list_deliveries))
            .routes(routes!(routes::replay_window))
            .routes(routes!(routes::replay_delivery))
            .routes(routes!(routes::list_events))
            .with_state(self.clone())
    }
}

/// The schemas the routes reference only through a query parameter:
/// `IntoParams` names a field's type by `$ref` without registering it, so the
/// module registers it here.
#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(DeliveryFilter)))]
struct Components;

#[cfg(test)]
mod tests {
    use super::*;

    fn lazy() -> Subscriptions {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@127.0.0.1:1/unused")
            .expect("a lazy pool opens no connection");
        Subscriptions::new(pool, "testd", "t1", SubscriptionsConfig::default())
    }

    fn document() -> serde_json::Value {
        let (_, doc) = OpenApiRouter::<()>::new()
            .merge(lazy().router())
            .split_for_parts();
        serde_json::to_value(doc).expect("the document serializes")
    }

    /// Every route the module mounts is in the document it hands the service,
    /// with its refusals as the shared `ApiError`.
    #[tokio::test]
    async fn every_mounted_route_is_documented() {
        let doc = document();
        let mut paths: Vec<&str> = doc["paths"]
            .as_object()
            .expect("paths")
            .keys()
            .map(String::as_str)
            .collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            [
                "/api/v1/events",
                "/api/v1/webhooks/deliveries/{id}/replay",
                "/api/v1/webhooks/endpoints",
                "/api/v1/webhooks/endpoints/{id}",
                "/api/v1/webhooks/endpoints/{id}/deliveries",
                "/api/v1/webhooks/endpoints/{id}/replay",
                "/api/v1/webhooks/endpoints/{id}/rotate-secret",
                "/api/v1/webhooks/endpoints/{id}/test",
            ]
        );
        assert!(doc["components"]["schemas"]["ApiError"].is_object());
    }

    /// Every `#/components/schemas/X` the module's document references is
    /// defined in it: a service merging `router()` must not commit a document
    /// a client generator refuses on a dangling `$ref`.
    #[tokio::test]
    async fn the_document_has_no_dangling_ref() {
        fn refs(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for (k, v) in m {
                        if k == "$ref"
                            && let Some(name) = v
                                .as_str()
                                .and_then(|r| r.strip_prefix("#/components/schemas/"))
                        {
                            out.push(name.to_owned());
                        }
                        refs(v, out);
                    }
                }
                serde_json::Value::Array(a) => a.iter().for_each(|v| refs(v, out)),
                _ => {}
            }
        }
        let doc = document();
        let mut found = Vec::new();
        refs(&doc, &mut found);
        assert!(!found.is_empty(), "the document references no schema");
        let dangling: Vec<&String> = found
            .iter()
            .filter(|name| doc["components"]["schemas"][name.as_str()].is_null())
            .collect();
        assert!(dangling.is_empty(), "dangling $refs: {dangling:?}");
    }

    /// `crates/mako-service/events/` holds the schema of every payload this
    /// crate emits (`BLESS_MCP_SNAPSHOT=1` rewrites it).
    #[test]
    fn the_committed_event_schemas_are_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("events");
        let schemas = [crate::cloudevent::event_schema::<WebhookTest>()];
        if let Err(e) = crate::cloudevent::compare_event_schemas(&dir, &schemas) {
            panic!("{e}");
        }
    }

    /// A config endpoint named `default`, named twice or without a URL blocks
    /// the boot; `allow_loopback` is a warning.
    #[test]
    fn a_misnamed_config_endpoint_is_a_blocking_finding() {
        let cfg: SubscriptionsConfig = toml::from_str(
            r#"
            allow_loopback = true
            [[endpoints]]
            name = "default"
            url = "http://a"
            [[endpoints]]
            name = "x"
            url = ""
            [[endpoints]]
            name = "x"
            url = "http://b"
            "#,
        )
        .expect("parses");
        let pfade: Vec<String> = cfg.befunde().iter().map(|b| b.pfad.clone()).collect();
        assert_eq!(
            pfade,
            [
                "subscriptions.allow_loopback",
                "subscriptions.endpoints[0].name",
                "subscriptions.endpoints[1].url",
                "subscriptions.endpoints[2].name",
            ]
        );
    }

    /// A pattern naming a type mako does not emit matches nothing, so a
    /// subscriber would wait for events that never come. Config refuses it at
    /// boot; `*` and a pattern matching a catalogued type pass.
    #[test]
    fn a_pattern_matching_no_type_is_silent_and_refused_in_config() {
        let patterns = [
            "de.vertrag.*".to_owned(),
            "de.mako.vertrag.*".to_owned(),
            "*".to_owned(),
            "de.mako.engine.process.completed".to_owned(),
            "de.mako.engine.prozess.*".to_owned(),
        ];
        assert_eq!(
            silent_patterns(&patterns),
            ["de.vertrag.*", "de.mako.engine.prozess.*"]
        );
        let cfg: SubscriptionsConfig = toml::from_str(
            r#"
            [[endpoints]]
            name = "erp"
            url = "https://erp.example"
            event_types = ["de.mako.vertrag.*", "de.sperr.*"]
            "#,
        )
        .expect("parses");
        let befunde = cfg.befunde();
        assert_eq!(befunde.len(), 1, "{befunde:?}");
        assert_eq!(befunde[0].pfad, "subscriptions.endpoints[0].event_types");
        assert!(befunde[0].text.contains("de.sperr.*"), "{:?}", befunde[0]);
    }

    /// The `[subscriptions]` section refuses a key it does not declare.
    #[test]
    fn the_config_section_refuses_an_unknown_key() {
        let ok: SubscriptionsConfig = toml::from_str("retention_days = 7").expect("parses");
        assert_eq!(ok.retention_days, 7);
        assert!(!ok.allow_loopback);
        assert!(ok.endpoints.is_empty());
        assert!(toml::from_str::<SubscriptionsConfig>("retention = 7").is_err());
    }

    /// The MCP tools `list_webhook_endpoints` and `list_webhook_deliveries`
    /// are agent twins of the REST reads: a service declares each with the
    /// REST response type as its output schema.
    #[cfg(all(feature = "mcp", feature = "testing"))]
    #[tokio::test]
    async fn the_list_tools_are_twins_of_the_rest_reads() {
        let doc = document();
        let tools = [
            rmcp::model::Tool::new(
                "list_webhook_endpoints",
                "endpoints",
                Arc::new(serde_json::Map::new()),
            )
            .with_output_schema::<crate::page::Page<Endpoint>>(),
            rmcp::model::Tool::new(
                "list_webhook_deliveries",
                "deliveries",
                Arc::new(serde_json::Map::new()),
            )
            .with_output_schema::<crate::page::Page<Delivery>>(),
        ];
        crate::mcp::conformance::assert_rest_twin(
            &tools,
            "list_webhook_endpoints",
            &doc,
            "/api/v1/webhooks/endpoints",
        );
        crate::mcp::conformance::assert_rest_twin(
            &tools,
            "list_webhook_deliveries",
            &doc,
            "/api/v1/webhooks/endpoints/{id}/deliveries",
        );
    }
}
