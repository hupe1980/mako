//! A master-data write and the CloudEvent that announces it commit together.
//!
//! processd, makod and the ERP learn of a change only through the event, so a
//! write that commits while its event is lost is a change nobody downstream
//! ever sees — and the caller, answered `500`, retries a write that already
//! happened. Each test makes the event INSERT fail (`event_log` renamed away,
//! as a lost event looks from inside the transaction), drives the handler over
//! HTTP and checks that the business write rolled back with it.
//!
//! The inbound CloudEvent ingest has the converse shape: its idempotency marker
//! must not commit without the Stammdatenänderung it reports, or makod's
//! redelivery is deduplicated away and the change is never applied.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use marktd::daemon::Marktd;
use sqlx::PgPool;

const TENANT: &str = "9900357000004";

struct Harness {
    pool: PgPool,
    base: String,
    http: reqwest::Client,
    _pg: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
}

async fn harness() -> Harness {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start postgres");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPool::connect(&url).await.expect("connect");
    let mut cfg = super::router_builds::load("../../dev/config/marktd.toml");
    cfg.database.url = url;
    let app = mako_service::build_for_test_on::<Marktd>(cfg, pool.clone())
        .await
        .expect("the router builds on the database");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Harness {
        pool,
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        _pg: pg,
    }
}

impl Harness {
    async fn sql(&self, stmt: &'static str) {
        sqlx::raw_sql(stmt)
            .execute(&self.pool)
            .await
            .unwrap_or_else(|e| panic!("{stmt}: {e}"));
    }

    /// `stmt` with `$1` bound to the tenant.
    async fn sql_tenant(&self, stmt: &'static str) {
        sqlx::query(stmt)
            .bind(TENANT)
            .execute(&self.pool)
            .await
            .unwrap_or_else(|e| panic!("{stmt}: {e}"));
    }

    async fn scalar(&self, q: &'static str) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>(q)
            .fetch_optional(&self.pool)
            .await
            .unwrap_or_else(|e| panic!("{q}: {e}"))
            .flatten()
    }

    async fn lose_events(&self) {
        self.sql("ALTER TABLE event_log RENAME TO event_log_away")
            .await;
    }

    async fn restore_events(&self) {
        self.sql("ALTER TABLE event_log_away RENAME TO event_log")
            .await;
    }

    async fn events(&self, ce_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM event_log WHERE ce_type = $1")
            .bind(ce_type)
            .fetch_one(&self.pool)
            .await
            .expect("count events")
    }

    /// Send with the event INSERT failing; the answer must be a `500` with an
    /// `error_id`, never a bare status.
    async fn send_losing_the_event(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) {
        self.lose_events().await;
        let mut req = self.http.request(method, format!("{}{path}", self.base));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.expect("answered");
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        self.restore_events().await;
        assert_eq!(status, 500, "{path}: {text}");
        let problem: serde_json::Value = serde_json::from_str(&text).expect("a problem body");
        assert!(
            problem["error_id"].as_str().is_some(),
            "{path}: a failed enqueue answers with an error_id: {text}"
        );
    }
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_konfigurationsprodukte_put_rolls_back_without_its_event() {
    let h = harness().await;
    h.sql_tenant("INSERT INTO steuerbare_ressourcen (sr_id, tenant) VALUES ('C1234567890', $1)")
        .await;
    h.send_losing_the_event(
        reqwest::Method::PUT,
        "/api/v1/steuerbare-ressourcen/C1234567890/konfigurationsprodukte",
        Some(serde_json::json!({
            "konfigurationsprodukte": [{ "produktcode": "FLEX-PRODUCT-001" }]
        })),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT konfigurationsprodukte::text FROM steuerbare_ressourcen")
            .await,
        None,
        "the Konfigurationsprodukte must not commit without their event"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_konfigurationsprodukt_delete_rolls_back_without_its_event() {
    let h = harness().await;
    h.sql_tenant(
        "INSERT INTO steuerbare_ressourcen (sr_id, tenant, konfigurationsprodukte) \
         VALUES ('C1234567890', $1, '[{\"produktcode\": \"FLEX-PRODUCT-001\"}]')",
    )
    .await;
    h.send_losing_the_event(
        reqwest::Method::DELETE,
        "/api/v1/steuerbare-ressourcen/C1234567890/konfigurationsprodukte/FLEX-PRODUCT-001",
        None,
    )
    .await;
    assert_eq!(
        h.scalar(
            "SELECT jsonb_array_length(konfigurationsprodukte)::text FROM steuerbare_ressourcen"
        )
        .await
        .as_deref(),
        Some("1"),
        "the removal must not commit without its event"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_geraet_konfiguration_put_rolls_back_without_its_event() {
    let h = harness().await;
    h.sql_tenant("INSERT INTO geraete (geraet_id, tenant, zaehler_id) VALUES ('G-1', $1, 'Z-1')")
        .await;
    h.send_losing_the_event(
        reqwest::Method::PUT,
        "/api/v1/zaehler/Z-1/geraete/G-1/konfigurationen",
        Some(serde_json::json!({
            "konfigurationen": [{ "parameter": "FIRMWARE_VERSION", "wert": "1.2.3", "notiz": null }]
        })),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT jsonb_array_length(geraet_konfigurationen)::text FROM geraete")
            .await
            .as_deref(),
        Some("0"),
        "the Konfiguration must not commit without its event"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_melo_put_rolls_back_without_its_event() {
    let h = harness().await;
    h.send_losing_the_event(
        reqwest::Method::PUT,
        "/api/v1/melos/DE0000000000000000000000000000042",
        Some(serde_json::json!({
            "malo_id": null,
            "data": { "messlokationsId": "DE0000000000000000000000000000042" }
        })),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT melo_id FROM melo").await,
        None,
        "the MeLo must not commit without its event"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_partner_put_rolls_back_without_its_event() {
    let h = harness().await;
    h.send_losing_the_event(
        reqwest::Method::PUT,
        "/api/v1/partners/9900357000004",
        Some(serde_json::json!({ "display_name": "Demo NB", "marktrolle": "NB" })),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT mp_id FROM partners").await,
        None,
        "the partner must not commit without its event"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_nb_contract_put_rolls_back_without_its_event() {
    let h = harness().await;
    h.sql("INSERT INTO malo (malo_id, sparte, data) VALUES ('51238696012', 'STROM', '{}')")
        .await;
    h.send_losing_the_event(
        reqwest::Method::PUT,
        "/api/v1/nb-contracts/NNV-1",
        Some(serde_json::json!({
            "malo_id": "51238696012",
            "nb_mp_id": TENANT,
            "sparte": "STROM",
            "netzebene": "NSP",
            "bilanzierungsmethode": "SLP",
            "billing_schedule": "MONTHLY",
            "netznutzer_mp_id": "9900357000004",
            "valid_from": "2026-01-01"
        })),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT contract_id FROM nb_contracts").await,
        None,
        "the contract must not commit without its event"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_netzzugang_antrag_and_its_status_roll_back_without_their_event() {
    let h = harness().await;
    let antrag = serde_json::json!({
        "antrag_typ": "zaehlpunktanordnung",
        "aktion": "bestellung",
        "netzanschluss_id": "NA-1",
        "nb_mp_id": TENANT,
        "antragsteller_ref": "REF-1"
    });
    h.send_losing_the_event(
        reqwest::Method::PUT,
        "/api/v1/netzzugang/antraege",
        Some(antrag.clone()),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT id::text FROM netzzugang_antraege").await,
        None,
        "the Antrag must not commit without its event"
    );

    // The status change has the same shape.
    let resp = h
        .http
        .put(format!("{}/api/v1/netzzugang/antraege", h.base))
        .json(&antrag)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
    let id = h
        .scalar("SELECT id::text FROM netzzugang_antraege")
        .await
        .unwrap();
    h.send_losing_the_event(
        reqwest::Method::PATCH,
        &format!("/api/v1/netzzugang/antraege/{id}/status"),
        Some(serde_json::json!({ "status": "uebermittelt" })),
    )
    .await;
    assert_eq!(
        h.scalar("SELECT status FROM netzzugang_antraege")
            .await
            .as_deref(),
        Some("erfasst"),
        "the status must not commit without its event"
    );
}

/// The ingest's idempotency marker commits with the Stammdatenänderung it
/// carries: a failed apply answers `500`, so makod redelivers and the
/// redelivery applies it — instead of a `202` whose marker swallows every
/// redelivery as a duplicate.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_failed_stammdaten_apply_leaves_the_event_redeliverable() {
    let h = harness().await;
    h.sql_tenant(
        "INSERT INTO nelo (nelo_id, tenant, sparte, nb_mp_id) \
         VALUES ('E1234567890', $1, 'STROM', $1)",
    )
    .await;
    let event = serde_json::json!({
        "specversion": "1.0",
        "id": "evt-stammdaten-1",
        "source": "/makod/test",
        "time": "2026-11-10T08:00:00Z",
        "type": mako_events::mako::PROCESS_COMPLETED,
        "subject": "6f1c1e8e-0000-4000-8000-000000000001",
        "makopid": 55_175,
        "data": {
            "malo_id": "E1234567890",
            "objekt": "NETZLOKATION",
            "stammdaten_patch": { "netzebene": "MSP" }
        }
    });
    let body = event.to_string();
    let post = || async {
        let mut req = h
            .http
            .post(format!("{}/api/v1/events", h.base))
            .header("content-type", "application/cloudevents+json");
        // Signed with the dev configuration's inbound secret, freshly stamped.
        for (name, value) in mako_service::webhook::headers(
            b"dev-webhook-secret",
            "evt-stammdaten-1",
            time::OffsetDateTime::now_utc().unix_timestamp(),
            body.as_bytes(),
        ) {
            req = req.header(name, value);
        }
        req.body(body.clone()).send().await.expect("answered")
    };

    h.sql("ALTER TABLE nelo RENAME TO nelo_away").await;
    let first = post().await;
    let first_status = first.status();
    h.sql("ALTER TABLE nelo_away RENAME TO nelo").await;
    assert_eq!(
        first_status, 500,
        "a Stammdatenänderung that could not be applied must be redelivered"
    );

    let second = post().await;
    assert_eq!(second.status(), 202);
    assert_eq!(
        h.scalar("SELECT netzebene FROM nelo").await.as_deref(),
        Some("MSP"),
        "the redelivery applies the change"
    );
    assert_eq!(h.events(mako_events::markt::STAMMDATEN_GEAENDERT).await, 1);
}
