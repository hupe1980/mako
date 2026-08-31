//! Handler- and SQL-level tests for `einsd`, against a real PostgreSQL.
//!
//! The defects these guard against do not live in the settlement arithmetic —
//! that is covered exhaustively by the pure `eeg-billing` tests. They live in
//! the seams the pure tests cannot reach: a column named in a query but absent
//! from the schema, an `ON CONFLICT` that cannot match a partial index, a state
//! written without recording the transition, an audit field accepted from the
//! caller and then dropped. Each of those shipped and was invisible until the
//! query actually ran.
//!
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); the tests skip gracefully when Docker is unavailable:
//!
//! ```bash
//! just test-einsd-db
//! ```
//!
//! Every test provisions its own schema, so they leave nothing behind.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt as _;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// A fresh throwaway PostgreSQL with the schema applied, or `None` when Docker is
/// unavailable. The returned container guard **must** be held by the test — it
/// removes the container on drop (no leak).
async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    // Every plant needs an operator to be settled — see `regelbesteuert`.
    seed_operator(&pool, "EB-1", "REGELBESTEUERUNG").await;
    Some((pool, container))
}

/// Insert an Anlagenbetreiber directly.
async fn seed_operator(pool: &PgPool, einspeiser_id: &str, ust_status: &str) {
    seed_operator_for(pool, einspeiser_id, TENANT, ust_status).await;
}

async fn seed_operator_for(pool: &PgPool, einspeiser_id: &str, tenant: &str, ust_status: &str) {
    sqlx::query(
        "INSERT INTO einspeiser (einspeiser_id, tenant, name, ust_status,
                                 bank_iban, bank_bic, zahlungsempfaenger)
         VALUES ($1, $2, $1, $3, 'DE02120300000000202051', 'BYLADEM1001', $1)
         ON CONFLICT (einspeiser_id, tenant) DO UPDATE SET ust_status = EXCLUDED.ust_status",
    )
    .bind(einspeiser_id)
    .bind(tenant)
    .bind(ust_status)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed operator {einspeiser_id}: {e}"));
}

const TENANT: &str = "9900357000004";

fn test_config() -> einsd::config::EinsdConfig {
    einsd::config::EinsdConfig {
        database: mako_service::config::DatabaseConfig {
            url: String::new(),
            pool_size: 10,
            min_connections: 0,
            acquire_timeout_secs: 30,
            idle_timeout_secs: 600,
            max_lifetime_secs: 1_800,
        },
        port: None,
        tenant: TENANT.to_owned(),
        erp_webhook_url: None,
        erp_hmac_secret: None,
        edmd_url: None,
        edmd_api_key: None,
        alert_interval_secs: None,
        jahresmarktwert_url: None,
        jahresmarktwert_import_interval_secs: None,
        auto_settle_from_day: None,
        auto_settle_catchup_months: None,
        mcp: Default::default(),
        oidc: None,
        allow_insecure_no_auth: true,
    }
}

/// Build the real router over a test pool, with auth disabled.
///
/// `OidcVerifier::disabled` admits every caller as `dev-admin` holding every
/// market role, so these tests exercise routing, extractors and SQL rather than
/// the Cedar decision — which [`the_policy_gates_settlement_writes_by_role`]
/// covers separately, against the policy itself.
fn test_router(pool: PgPool) -> axum::Router {
    let cfg = std::sync::Arc::new(test_config());
    let http = std::sync::Arc::new(reqwest::Client::new());
    let cedar = std::sync::Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../policies/einsd.cedar"
        ))
        .expect("einsd.cedar parses"),
    );
    let oidc = mako_service::oidc::OidcVerifier::disabled(TENANT);
    let mcp_state = std::sync::Arc::new(einsd::mcp_server::EinsdMcpState {
        pool: pool.clone(),
        tenant: TENANT.to_owned(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config(&Default::default(), TENANT),
        cfg: std::sync::Arc::clone(&cfg),
        http_client: std::sync::Arc::clone(&http),
    });
    einsd::routes::build_router(
        cfg,
        http,
        cedar,
        oidc,
        pool,
        mcp_state,
        mako_service::shutdown::token(),
    )
}

/// A minimal registerable plant.
/// A regelbesteuerter Anlagenbetreiber.
///
/// Every settlement needs one: the Umsatzsteuer of a Gutschrift is the operator's
/// declared status, so `build_settle_input` takes the operator rather than
/// deriving a rate from the plant.
fn regelbesteuert() -> einsd::pg_einspeiser::Einspeiser {
    einsd::pg_einspeiser::Einspeiser {
        einspeiser_id: "EB-1".to_owned(),
        name: "Testbetreiber".to_owned(),
        mastr_akteur_id: None,
        ust_status: "REGELBESTEUERUNG".to_owned(),
        bank_iban: Some("DE02120300000000202051".to_owned()),
        bank_bic: Some("BYLADEM1001".to_owned()),
        zahlungsempfaenger: Some("Testbetreiber".to_owned()),
    }
}

fn anlage_json(tr_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tr_id": tr_id,
        "malo_id": "51238696781",
        "eeg_gesetz": 2023,
        "inbetriebnahme": "2024-06-01",
        "leistung_kwp": "9.5",
        "erzeugungsart": "SOLAR_AUFDACH",
        "verguetungssatz_ct": "8.11",
        "settlement_model": "VERGUETUNG",
        "einspeiser_id": "EB-1",
    })
}

async fn post_json(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("router responds");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn put_json(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("router responds");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router responds");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Insert a plant directly.
///
/// `foerderendedatum` is NOT NULL — the service derives it at registration from
/// the commissioning date, so a direct insert has to supply it too.
async fn seed_plant(pool: &PgPool, tr_id: &str, tenant: &str, extra_cols: &str, extra_vals: &str) {
    // The plant's operator is per tenant, so a second tenant needs its own.
    seed_operator_for(pool, "EB-1", tenant, "REGELBESTEUERUNG").await;
    let sql = format!(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            einspeiser_id{extra_cols})
         VALUES ($1, $2, '51238696781', 2023, '2024-06-01', 9.5,
                 'SOLAR_AUFDACH', 8.11, 'VERGUETUNG', '2044-12-31',
                 'EB-1'{extra_vals})"
    );
    sqlx::query(&sql)
        .bind(tr_id)
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("seed plant {tr_id}: {e}"));
}

// ── Schema ↔ code agreement ───────────────────────────────────────────────────

/// Every column a query names must exist.
///
/// `get_compliance_status` selected `kwk_max_kwh`, which is derived
/// (`kwk_foerderdauer_h × leistung_kwp`) and has never been a column. The tool
/// failed for every plant, and nothing caught it because no test ran the query.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn compliance_status_query_names_only_real_columns() {
    let Some((pool, _pg)) = test_pool("compliance_cols").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, ", kwk_foerderdauer_h", ", 30000").await;

    // The exact projection the MCP tool issues.
    let row = sqlx::query(
        r"SELECT tr_id, erzeugungsart, leistung_kwp, eeg_gesetz,
                 mastr_registriert, mastr_nummer, mastr_datum, status,
                 inbetriebnahme, foerderendedatum,
                 kwk_strom_kwh_gesamt, kwk_foerderdauer_h
          FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2",
    )
    .bind("P1")
    .bind(TENANT)
    .fetch_optional(&pool)
    .await;
    assert!(row.is_ok(), "compliance projection must run: {row:?}");
    assert!(row.unwrap().is_some());
}

/// The receipts upsert must be able to match its own index.
///
/// `sr_unique_initial` is a *partial* unique index (`WHERE is_correction =
/// false`). Postgres cannot infer a partial index from the column list alone, so
/// an `ON CONFLICT (cols)` without the predicate raises "no unique or exclusion
/// constraint matching the ON CONFLICT specification" — which is what the
/// award-expired settlement path did on every call.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn receipt_upsert_matches_the_partial_unique_index() {
    let Some((pool, _pg)) = test_pool("upsert_partial").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    let upsert = r"INSERT INTO settlement_receipts
                     (id, tr_id, tenant, billing_year, billing_month,
                      settlement_model, einspeisemenge_kwh, settlement_eur, status)
                   VALUES ($1, 'P1', $2, 2026, 7, 'VERGUETUNG', 100, 8.11, $3)
                   ON CONFLICT (tr_id, tenant, billing_year, billing_month)
                       WHERE is_correction = false DO UPDATE
                   SET status = EXCLUDED.status, settled_at = now()";

    for status in ["berechnet", "foerderung_beendet"] {
        sqlx::query(upsert)
            .bind(uuid::Uuid::new_v4())
            .bind(TENANT)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("upsert must match the partial index: {e}"));
    }

    let (count, status): (i64, String) =
        sqlx::query_as("SELECT count(*), max(status) FROM settlement_receipts WHERE tr_id = 'P1'")
            .fetch_one(&pool)
            .await
            .expect("read back");
    assert_eq!(count, 1, "second upsert must update, not insert");
    assert_eq!(status, "foerderung_beendet");
}

/// A correction and its original coexist — the index only constrains originals.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_may_coexist_with_the_receipt_it_corrects() {
    let Some((pool, _pg)) = test_pool("correction_coexist").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    let original = uuid::Uuid::new_v4();
    for (id, is_corr, corr_of) in [
        (original, false, None),
        (uuid::Uuid::new_v4(), true, Some(original)),
    ] {
        sqlx::query(
            "INSERT INTO settlement_receipts
               (id, tr_id, tenant, billing_year, billing_month, settlement_model,
                einspeisemenge_kwh, settlement_eur, status, is_correction,
                correction_of, correction_reason)
             VALUES ($1, 'P1', $2, 2026, 7, 'VERGUETUNG', 100, 8.11,
                     'berechnet', $3, $4, $5)",
        )
        .bind(id)
        .bind(TENANT)
        .bind(is_corr)
        .bind(corr_of)
        .bind(if is_corr {
            Some("Messwertkorrektur: Zaehlerstand revidiert")
        } else {
            None
        })
        .execute(&pool)
        .await
        .expect("both receipts must be storable");
    }

    // § 147 AO / GoBD: the audit trail must say why the original was superseded.
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT correction_reason FROM settlement_receipts WHERE is_correction = true",
    )
    .fetch_one(&pool)
    .await
    .expect("read correction");
    assert_eq!(
        reason.as_deref(),
        Some("Messwertkorrektur: Zaehlerstand revidiert"),
        "the stated reason must survive to the audit trail"
    );
}

/// State changes must leave a transition row.
///
/// The settlement path updated `eeg_anlagen.settlement_state` in place, so the
/// prior state was unrecoverable and `get_settlement_state_history` always
/// returned empty. The CTE below is the one the service now issues.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_state_change_records_the_transition_it_came_from() {
    let Some((pool, _pg)) = test_pool("state_transition").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, ", settlement_state", ", 'aktiv'").await;

    let previous: Option<String> = sqlx::query_scalar(
        r"WITH prev AS (
              SELECT settlement_state FROM eeg_anlagen
              WHERE tr_id = $1 AND tenant = $2
              FOR UPDATE
          ), upd AS (
              UPDATE eeg_anlagen SET settlement_state = $3, updated_at = now()
              WHERE tr_id = $1 AND tenant = $2
          )
          SELECT settlement_state FROM prev",
    )
    .bind("P1")
    .bind(TENANT)
    .bind("sanktioniert")
    .fetch_optional(&pool)
    .await
    .expect("snapshot update")
    .flatten();

    assert_eq!(
        previous.as_deref(),
        Some("aktiv"),
        "the CTE must yield the pre-update state, not the new one"
    );

    let now: String =
        sqlx::query_scalar("SELECT settlement_state FROM eeg_anlagen WHERE tr_id = 'P1'")
            .fetch_one(&pool)
            .await
            .expect("read state");
    assert_eq!(
        now, "sanktioniert",
        "and the update must still have applied"
    );
}

// ── HTTP surface ──────────────────────────────────────────────────────────────

/// Registering and reading a plant through the real router.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_registered_plant_is_readable_over_http() {
    let Some((pool, _pg)) = test_pool("http_roundtrip").await else {
        return;
    };
    let app = test_router(pool);

    let (status, body) = post_json(&app, "/api/v1/anlagen", anlage_json("P-HTTP")).await;
    assert!(
        status.is_success(),
        "register must succeed: {status} {body}"
    );

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/anlagen/P-HTTP")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router responds");
    assert_eq!(res.status(), StatusCode::OK);
}

/// §25 EEG 2023: the Förderende is derived at registration, not left to the caller.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn registration_derives_the_foerderende_from_the_commissioning_date() {
    let Some((pool, _pg)) = test_pool("foerderende").await else {
        return;
    };
    let app = test_router(pool.clone());
    let (status, body) = post_json(&app, "/api/v1/anlagen", anlage_json("P-FD")).await;
    assert!(status.is_success(), "{status} {body}");

    let ende: Option<time::Date> =
        sqlx::query_scalar("SELECT foerderendedatum FROM eeg_anlagen WHERE tr_id = 'P-FD'")
            .fetch_one(&pool)
            .await
            .expect("read foerderendedatum");

    // §25 Abs. 1: 20 years plus the remainder of the commissioning year.
    assert_eq!(
        ende,
        Some(time::macros::date!(2044 - 12 - 31)),
        "commissioned 2024-06-01 → end of the 20th following year"
    );
}

/// The feed-in Gutschrift VAT is the *operator's* declared § 19 UStG election,
/// held once on `einspeiser` and never per plant. Switching it reaches every one
/// of the operator's plants in the same call: two plants of one operator can
/// never bill two different VAT rates. § 12 Abs. 3 UStG (hardware supply) never
/// applies to feed-in.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_operators_ust_status_drives_the_gutschrift_vat_for_all_its_plants() {
    let Some((pool, _pg)) = test_pool("ust_status").await else {
        return;
    };
    let app = test_router(pool.clone());

    // One operator, two plants.
    let (status, body) = post_json(&app, "/api/v1/anlagen", anlage_json("P-A")).await;
    assert!(status.is_success(), "register P-A: {status} {body}");
    let (status, body) = post_json(&app, "/api/v1/anlagen", anlage_json("P-B")).await;
    assert!(status.is_success(), "register P-B: {status} {body}");

    // (1) Regelbesteuerung — 500 kWh x 8.11 ct = 40.55 EUR net x 19 % = 7.70 EUR.
    for tr_id in ["P-A", "P-B"] {
        let steuer = settle_and_read_steuer(&app, tr_id, 7).await;
        assert_eq!(
            steuer.round_dp(2),
            rust_decimal::Decimal::new(770, 2),
            "{tr_id}: Regelbesteuerung bills 19 %"
        );
    }

    // (2) The operator declares § 19 Kleinunternehmer. One PUT, both plants.
    let (status, body) = put_json(
        &app,
        "/api/v1/einspeiser/EB-1",
        serde_json::json!({
            "name": "Testbetreiber",
            "ust_status": "KLEINUNTERNEHMER",
        }),
    )
    .await;
    assert!(status.is_success(), "switch to §19: {status} {body}");

    for tr_id in ["P-A", "P-B"] {
        let steuer = settle_and_read_steuer(&app, tr_id, 8).await;
        assert!(
            steuer.is_zero(),
            "{tr_id}: §19 Kleinunternehmer bills no USt, got {steuer}"
        );
    }

    // (3) The election is stored once, not copied onto the plants.
    let copies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns
         WHERE table_name = 'eeg_anlagen' AND column_name = 'ust_status'",
    )
    .fetch_one(&pool)
    .await
    .expect("introspect");
    assert_eq!(copies, 0, "the plant table must not carry a VAT status");
}

/// Settle one plant for `month` 2026 and return the Gutschrift's USt amount.
async fn settle_and_read_steuer(
    app: &axum::Router,
    tr_id: &str,
    month: u8,
) -> rust_decimal::Decimal {
    let (status, body) = post_json(
        app,
        &format!("/api/v1/anlagen/{tr_id}/settle/2026/{month}"),
        serde_json::json!({ "einspeisemenge_kwh": "500" }),
    )
    .await;
    assert!(status.is_success(), "settle {tr_id}: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    serde_json::from_value(v["gutschrift_steuer_eur"].clone()).expect("steuer present")
}

/// A plant cannot exist without an operator.
///
/// § 7 Abs. 1 EEG 2023 puts the payment on the Netzbetreiber, so a plant nobody
/// can be paid for is not one this service can act on. Refusing at registration
/// rather than at settlement means the Gutschrift path has no VAT-less branch
/// to guess down.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_plant_cannot_be_registered_without_an_operator() {
    let Some((pool, _pg)) = test_pool("no_operator").await else {
        return;
    };
    let app = test_router(pool.clone());

    let mut body = anlage_json("P-ORPHAN");
    body.as_object_mut().unwrap().remove("einspeiser_id");
    let (status, resp) = post_json(&app, "/api/v1/anlagen", body).await;
    assert!(!status.is_success(), "no einspeiser_id: {status} {resp}");

    let mut body = anlage_json("P-GHOST");
    body["einspeiser_id"] = serde_json::json!("EB-NOBODY");
    let (status, resp) = post_json(&app, "/api/v1/anlagen", body).await;
    assert!(
        !status.is_success(),
        "unknown einspeiser_id: {status} {resp}"
    );
    assert!(
        resp.contains("einspeiser_id"),
        "the refusal must name the field, got {resp}"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn settling_an_unknown_plant_is_not_found() {
    let Some((pool, _pg)) = test_pool("settle_404").await else {
        return;
    };
    let app = test_router(pool);
    let (status, _) = post_json(
        &app,
        "/api/v1/anlagen/NOPE/settle/2026/7",
        serde_json::json!({ "einspeisemenge_kwh": "100" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Authorization ─────────────────────────────────────────────────────────────

/// The Cedar policy is what actually decides; these assert its shape.
///
/// Every REST route was open for the whole of the service's life while the
/// README advertised OIDC and Cedar. The policy now exists — these pin the
/// decisions it must make, so a later edit that widens them fails here.
#[test]
fn the_policy_gates_settlement_writes_by_role() {
    use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

    let enforcer = CedarEnforcer::from_policy_str(include_str!("../policies/einsd.cedar"))
        .expect("einsd.cedar parses");

    let with_roles = |roles: &[&str]| CedarPrincipal {
        sub: "user-1".to_owned(),
        tenant: TENANT.to_owned(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    };

    // Reads are open to any authenticated caller inside the tenant.
    for action in ["read-anlage", "read-settlement", "read-marktdaten"] {
        assert!(
            enforcer
                .check(&with_roles(&["MSB"]), action, TENANT)
                .is_ok(),
            "{action} must be readable without a market write role"
        );
    }

    // Settling obliges the operator to pay the Anlagenbetreiber, so it is held
    // to the roles that carry that obligation.
    assert!(
        enforcer
            .check(&with_roles(&["NB"]), "run-settlement", TENANT)
            .is_ok(),
        "NB must be able to settle"
    );
    assert!(
        enforcer
            .check(&with_roles(&["MSB"]), "run-settlement", TENANT)
            .is_err(),
        "a metering operator must not be able to settle"
    );

    // Corrections re-open a closed period and are narrower again.
    assert!(
        enforcer
            .check(&with_roles(&["NB"]), "correct-settlement", TENANT)
            .is_ok(),
        "NB must be able to correct"
    );
    assert!(
        enforcer
            .check(&with_roles(&["LF"]), "correct-settlement", TENANT)
            .is_err(),
        "LF may settle but must not correct"
    );
}

/// Cedar is default-deny: another tenant's data is unreachable with no forbid rule.
#[test]
fn the_policy_denies_cross_tenant_access() {
    use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

    let enforcer = CedarEnforcer::from_policy_str(include_str!("../policies/einsd.cedar"))
        .expect("policy parses");
    let other_tenant = CedarPrincipal {
        sub: "user-1".to_owned(),
        tenant: "9999999999999".to_owned(),
        roles: vec!["NB".to_owned()],
    };

    for action in ["read-anlage", "run-settlement", "correct-settlement"] {
        assert!(
            enforcer.check(&other_tenant, action, TENANT).is_err(),
            "{action} must not cross a tenant boundary"
        );
    }
}

// ── Tenant isolation ──────────────────────────────────────────────────────────

/// Two tenants may register the same `tr_id` without colliding.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_same_plant_id_in_two_tenants_is_two_plants() {
    let Some((pool, _pg)) = test_pool("tenant_isolation").await else {
        return;
    };
    for tenant in ["T1", "T2"] {
        seed_plant(&pool, "SHARED", tenant, "", "").await;
    }

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM eeg_anlagen WHERE tr_id = 'SHARED'")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 2);
}

/// The tariff reference table ships seeded — a lookup against an empty table
/// would silently return no rate rather than the statutory one.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_statutory_rate_table_is_seeded_by_the_schema() {
    let Some((pool, _pg)) = test_pool("rates_seeded").await else {
        return;
    };
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM eeg_verguetungssaetze")
        .fetch_one(&pool)
        .await
        .expect("count rates");
    assert!(count > 0, "eeg_verguetungssaetze must ship seeded");
}

// ── Auction metadata (§22, §22b, §39n) ────────────────────────────────────────

/// The award facts must survive registration, and the **awarded** value must be
/// the one the settlement uses.
///
/// `AusschreibungMetadata` was constructed with `..Default::default()`, so
/// `award_ct`, `award_expired`, `innovation_auction` and `is_buergerenergie`
/// were always `None`/`false` no matter what was registered — §22b
/// Bürgerenergie and §39n Innovationsausschreibung were unreachable.
///
/// A second trap sat beside it: the plant carries two AW columns, and the
/// settlement read only `direktverm_aw_ct`. A tender plant registered with
/// `zuschlagswert_ct` — the field named after its award, which is what an
/// operator reaches for — settled at AW = 0 and was paid nothing, every month,
/// as a `calculated` result.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn auction_metadata_survives_registration() {
    let Some((pool, _pg)) = test_pool("auction_meta").await else {
        return;
    };
    let app = test_router(pool.clone());

    let mut body = anlage_json("P-AUCTION");
    body["settlement_model"] = serde_json::json!("AUSSCHREIBUNG");
    body["ausschreibungs_zuschlag_id"] = serde_json::json!("SEE-2024-001234");
    body["zuschlagswert_ct"] = serde_json::json!("7.35");
    body["zuschlag_datum"] = serde_json::json!("2024-03-01");
    body["ist_innovationsausschreibung"] = serde_json::json!(true);
    body["ist_buergerenergie"] = serde_json::json!(true);

    let (status, resp) = post_json(&app, "/api/v1/anlagen", body).await;
    assert!(status.is_success(), "{status} {resp}");

    // The awarded value is what settles the plant.
    let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "P-AUCTION")
        .await
        .expect("fetch")
        .expect("plant exists");
    let input = einsd::pg::build_settle_input(
        TENANT,
        &anlage,
        &regelbesteuert(),
        2026,
        6,
        einsd::pg::SettleOverrides {
            einspeisemenge_kwh: Some(rust_decimal::Decimal::from(100_000)),
            epex_avg_ct_kwh: Some(rust_decimal::Decimal::from(4)),
            ..Default::default()
        },
    );
    let mut tx = pool.begin().await.expect("begin");
    let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
        .await
        .expect("settle");
    tx.commit().await.expect("commit");
    assert_eq!(res.status, "calculated");
    assert!(
        res.settlement_eur
            .is_some_and(|e| e > rust_decimal::Decimal::ZERO),
        "the awarded AW of 7,35 ct against a 4,00 ct Marktwert owes a Marktprämie, got {:?}",
        res.settlement_eur
    );

    let row: (
        Option<rust_decimal::Decimal>,
        Option<time::Date>,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT zuschlagswert_ct, zuschlag_datum,
                ist_innovationsausschreibung, ist_buergerenergie
         FROM eeg_anlagen WHERE tr_id = 'P-AUCTION'",
    )
    .fetch_one(&pool)
    .await
    .expect("read award metadata");

    assert_eq!(row.0, Some(rust_decimal::Decimal::new(735, 2)));
    assert_eq!(row.1, Some(time::macros::date!(2024 - 03 - 01)));
    assert!(row.2, "§39n Innovationsausschreibung must round-trip");
    assert!(row.3, "§22b Bürgerenergie must round-trip");
}

// ── Jahresabrechnung ─────────────────────────────────────────────────────────

/// An incomplete year is reported as provisional, naming the months missing.
///
/// Summing eleven months and presenting the result as the year is the failure
/// mode worth guarding: the total looks plausible and nothing marks it short.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_incomplete_year_is_provisional_and_names_the_gaps() {
    let Some((pool, _pg)) = test_pool("ja_incomplete").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    // Settle January, February and April — March and May..December are missing.
    for month in [1i16, 2, 4] {
        sqlx::query(
            "INSERT INTO settlement_receipts
               (id, tr_id, tenant, billing_year, billing_month, settlement_model,
                einspeisemenge_kwh, settlement_eur, pflichtzahlung_eur, status)
             VALUES ($1, 'P1', $2, 2026, $3, 'VERGUETUNG', 100, 8.11, 2.50, 'berechnet')",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(TENANT)
        .bind(month)
        .execute(&pool)
        .await
        .expect("seed receipt");
    }

    let ja = einsd::pg::run_jahresabrechnung(&pool, TENANT, "P1", 2026)
        .await
        .expect("build Jahresabrechnung");

    assert_eq!(ja.months_settled, 3);
    assert_eq!(ja.status, "vorlaeufig");
    assert_eq!(ja.missing_months, vec![3, 5, 6, 7, 8, 9, 10, 11, 12]);
    assert_eq!(ja.einspeisemenge_kwh, rust_decimal::Decimal::from(300));
    assert_eq!(ja.settlement_eur, rust_decimal::Decimal::new(2433, 2));

    // §52 Pflichtzahlungen are a separate claim and are not netted into the
    // Vergütung total.
    assert_eq!(ja.pflichtzahlung_eur, rust_decimal::Decimal::new(750, 2));
}

/// A full year is final and lists no gaps.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_complete_year_is_final() {
    let Some((pool, _pg)) = test_pool("ja_complete").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    for month in 1i16..=12 {
        sqlx::query(
            "INSERT INTO settlement_receipts
               (id, tr_id, tenant, billing_year, billing_month, settlement_model,
                einspeisemenge_kwh, settlement_eur, verlaengerungsanspruch_qh, status)
             VALUES ($1, 'P1', $2, 2026, $3, 'VERGUETUNG', 100, 8.11, 4, 'berechnet')",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(TENANT)
        .bind(month)
        .execute(&pool)
        .await
        .expect("seed receipt");
    }

    let ja = einsd::pg::run_jahresabrechnung(&pool, TENANT, "P1", 2026)
        .await
        .expect("build Jahresabrechnung");

    assert_eq!(ja.months_settled, 12);
    assert_eq!(ja.status, "endgueltig");
    assert!(ja.missing_months.is_empty());
    // §51a accrues across the year: 12 × 4 quarter-hours.
    assert_eq!(ja.verlaengerungsanspruch_qh, 48);
}

/// A correction supersedes its original rather than adding to the year.
///
/// The partial unique index means the corrected month keeps one non-correction
/// receipt; counting the correction as well would double the month.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_does_not_double_count_its_month() {
    let Some((pool, _pg)) = test_pool("ja_correction").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    let original = uuid::Uuid::new_v4();
    for (id, is_corr, corr_of) in [
        (original, false, None),
        (uuid::Uuid::new_v4(), true, Some(original)),
    ] {
        sqlx::query(
            "INSERT INTO settlement_receipts
               (id, tr_id, tenant, billing_year, billing_month, settlement_model,
                einspeisemenge_kwh, settlement_eur, status, is_correction, correction_of)
             VALUES ($1, 'P1', $2, 2026, 3, 'VERGUETUNG', 100, 8.11,
                     'berechnet', $3, $4)",
        )
        .bind(id)
        .bind(TENANT)
        .bind(is_corr)
        .bind(corr_of)
        .execute(&pool)
        .await
        .expect("seed receipts");
    }

    let ja = einsd::pg::run_jahresabrechnung(&pool, TENANT, "P1", 2026)
        .await
        .expect("build Jahresabrechnung");

    assert_eq!(ja.months_settled, 1, "March counts once, not twice");
    assert_eq!(ja.einspeisemenge_kwh, rust_decimal::Decimal::from(100));
    assert_eq!(ja.correction_count, 1, "but the correction is visible");
}

/// Re-running replaces the stored statement rather than accumulating rows.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn rerunning_replaces_the_stored_statement() {
    let Some((pool, _pg)) = test_pool("ja_rerun").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    for _ in 0..2 {
        einsd::pg::run_jahresabrechnung(&pool, TENANT, "P1", 2026)
            .await
            .expect("build Jahresabrechnung");
    }

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jahresabrechnungen WHERE tr_id = 'P1'")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 1);
}

/// §51: the epex_spot_prices store round-trips ¼h prices (incl. negatives) and
/// the range fetch returns them ordered — the input to the negativpreis overlay.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn spot_prices_upsert_and_range_fetch() {
    use rust_decimal::dec;
    use time::macros::datetime;
    let Some((pool, _pg)) = test_pool("spot_prices").await else {
        return;
    };
    let base = datetime!(2026-06-01 00:00 UTC);
    let prices: Vec<einsd::pg::SpotPrice> = (0..8)
        .map(|n| einsd::pg::SpotPrice {
            delivery_start: base + time::Duration::minutes(15 * n),
            resolution_min: 15,
            // QH 2..5 are negative.
            price_ct_kwh: if (2..6).contains(&n) {
                dec!(-1.5)
            } else {
                dec!(4.0)
            },
        })
        .collect();

    let upserted = einsd::pg::upsert_spot_prices(&pool, &prices, "test")
        .await
        .expect("bulk upsert");
    assert_eq!(upserted, 8);

    // Idempotent: re-upsert updates, does not duplicate (PK on delivery_start).
    einsd::pg::upsert_spot_prices(&pool, &prices, "test")
        .await
        .expect("re-upsert");
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM epex_spot_prices")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(total, 8);

    let fetched = einsd::pg::fetch_spot_prices(&pool, base, base + time::Duration::hours(2))
        .await
        .expect("range fetch");
    assert_eq!(fetched.len(), 8);
    let negatives = fetched.iter().filter(|(_, p)| p.is_sign_negative()).count();
    assert_eq!(negatives, 4);
    // Ordered ascending.
    assert!(fetched.windows(2).all(|w| w[0].0 <= w[1].0));
}

/// §51 matches feed-in quarter-hours against a spot row by start instant, and
/// the store permits hourly rows. Returning them unexpanded matched only the
/// `:00` quarter, so three quarters of every negative hour were paid in full.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_hourly_spot_row_covers_all_four_of_its_quarter_hours() {
    use rust_decimal::dec;
    use time::macros::datetime;
    let Some((pool, _pg)) = test_pool("spot_hourly").await else {
        return;
    };
    let base = datetime!(2026-06-01 00:00 UTC);
    let prices: Vec<einsd::pg::SpotPrice> = (0..2)
        .map(|n| einsd::pg::SpotPrice {
            delivery_start: base + time::Duration::hours(n),
            resolution_min: 60,
            price_ct_kwh: if n == 0 { dec!(-1.5) } else { dec!(4.0) },
        })
        .collect();
    einsd::pg::upsert_spot_prices(&pool, &prices, "test")
        .await
        .expect("bulk upsert");

    let fetched = einsd::pg::fetch_spot_prices(&pool, base, base + time::Duration::hours(2))
        .await
        .expect("range fetch");
    assert_eq!(fetched.len(), 8, "two hours are eight quarter-hours");
    let negatives: Vec<_> = fetched
        .iter()
        .filter(|(_, p)| p.is_sign_negative())
        .map(|(t, _)| *t)
        .collect();
    assert_eq!(
        negatives,
        (0..4)
            .map(|n| base + time::Duration::minutes(15 * n))
            .collect::<Vec<_>>(),
        "the whole negative hour is negative, not just its first quarter"
    );
    // The window is half-open: an hourly row starting at the boundary must not
    // spill past it.
    let clipped = einsd::pg::fetch_spot_prices(&pool, base, base + time::Duration::hours(1))
        .await
        .expect("clipped fetch");
    assert_eq!(clipped.len(), 4);
}

/// `POST /settle` is idempotent — the receipt is an upsert — but the plant-level
/// counters are running totals over the whole Förderdauer. Accruing on every
/// settle burnt the §44b quota, over-extended the §51a Förderende and expired
/// the KWKG limit from an operator merely re-running a month.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn re_settling_a_month_does_not_accrue_the_counters_twice() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("resettle_accrual").await else {
        return;
    };
    seed_plant(&pool, "TR-RESETTLE", TENANT, "", "").await;

    let settle = |qh: u64| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-RESETTLE")
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                6,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(1000)),
                    negative_price_quarter_hours: Some(qh),
                    kwh_during_negative_epex: Some(dec!(10)),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    let first = settle(96).await;
    let qh_after_first: i64 =
        sqlx::query_scalar("SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = $1")
            .bind("TR-RESETTLE")
            .fetch_one(&pool)
            .await
            .expect("read counter");
    assert_eq!(qh_after_first, 96);

    // Same month, same numbers: nothing more is owed to the counter.
    let second = settle(96).await;
    let qh_after_second: i64 =
        sqlx::query_scalar("SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = $1")
            .bind("TR-RESETTLE")
            .fetch_one(&pool)
            .await
            .expect("read counter");
    assert_eq!(qh_after_second, 96, "re-settling must not double-accrue");
    assert_eq!(
        first.id, second.id,
        "the upsert keeps the receipt's id, and the caller must be told the real one"
    );

    // A correction that raises the claim moves the counter by the difference only.
    settle(120).await;
    let qh_after_correction: i64 =
        sqlx::query_scalar("SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = $1")
            .bind("TR-RESETTLE")
            .fetch_one(&pool)
            .await
            .expect("read counter");
    assert_eq!(qh_after_correction, 120);

    // And one that lowers it gives the difference back.
    settle(96).await;
    let qh_after_lowering: i64 =
        sqlx::query_scalar("SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = $1")
            .bind("TR-RESETTLE")
            .fetch_one(&pool)
            .await
            .expect("read counter");
    assert_eq!(qh_after_lowering, 96);
}

/// A plant settled before the ÜNB Marktwert existed gets a `price_missing`
/// receipt. Counting that as settled left it out of every later batch, so it
/// was simply never paid.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_price_missing_receipt_leaves_the_plant_unsettled() {
    let Some((pool, _pg)) = test_pool("unsettled_retry").await else {
        return;
    };
    seed_plant(&pool, "TR-RETRY", TENANT, "", "").await;
    seed_plant(&pool, "TR-DONE", TENANT, "", "").await;

    for (tr_id, status) in [("TR-RETRY", "price_missing"), ("TR-DONE", "calculated")] {
        sqlx::query(
            "INSERT INTO settlement_receipts
                 (tr_id, tenant, billing_year, billing_month, settlement_model, status)
             VALUES ($1, $2, 2026, 6, 'VERGUETUNG', $3)",
        )
        .bind(tr_id)
        .bind(TENANT)
        .bind(status)
        .execute(&pool)
        .await
        .expect("seed receipt");
    }

    let unsettled = einsd::pg::list_unsettled(&pool, TENANT, 2026, 6)
        .await
        .expect("list_unsettled");
    let ids: Vec<&str> = unsettled.iter().map(|a| a.tr_id.as_str()).collect();
    assert_eq!(ids, vec!["TR-RETRY"]);
}

/// §36h Abs. 2: recording a Standortgüte re-evaluation persists it and flags
/// reconciliation when the Gütefaktor moves more than 2 percentage points.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn wind_reevaluation_records_and_flags_reconciliation() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("wind_reeval").await else {
        return;
    };
    seed_plant(
        &pool,
        "W1",
        TENANT,
        ", wind_guetegrad, wind_korrekturfaktor",
        ", 0.90, 1.06",
    )
    .await;

    // Year-6 re-evaluation: 0.90 → 0.95 is a 5 pp move (> 2 pp) → reconcile.
    let (recon1, prev1) =
        einsd::pg::record_wind_reevaluation(&pool, TENANT, "W1", 6, dec!(0.95), None)
            .await
            .expect("record")
            .expect("plant exists");
    assert!(recon1);
    assert_eq!(prev1, Some(dec!(0.90)));

    // Year-11 re-evaluation vs the year-6 value: 0.95 → 0.96 is 1 pp → no reconcile.
    let (recon2, prev2) =
        einsd::pg::record_wind_reevaluation(&pool, TENANT, "W1", 11, dec!(0.96), None)
            .await
            .expect("record")
            .expect("plant exists");
    assert!(!recon2);
    assert_eq!(prev2, Some(dec!(0.95)));

    // Both re-evaluations persisted (one per effective year).
    let json: serde_json::Value = sqlx::query_scalar(
        "SELECT wind_guetefaktor_reevaluations FROM eeg_anlagen WHERE tr_id='W1'",
    )
    .fetch_one(&pool)
    .await
    .expect("fetch reevals");
    assert_eq!(json.as_array().map(Vec::len), Some(2));
}
/// The Postgres container guard a test holds until it ends — dropping it removes
/// the container (testcontainers cleans up on `Drop`; there is no leak and no
/// reliance on an external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a fresh throwaway `postgres:17-alpine` and return its URL plus the
/// container guard. `None` when Docker is unavailable (tests skip gracefully).
async fn pg_container() -> Option<(String, PgContainer)> {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Some((url, container))
}

/// §§53b–53c EEG 2023 — the recorded facts cut the anzulegender Wert, and the
/// schema refuses amounts the statutes do not provide for.
///
/// The pure engine already proves the arithmetic. What only a real database can
/// prove is the seam: that the settlement path finds these rows at all, that the
/// deduction amounts come from the statute rather than from a column, and that
/// the CHECKs stop a data-entry error from inventing a reduction.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn statutory_aw_cuts_are_driven_by_recorded_facts_not_stored_rates() {
    let Some((pool, _pg)) = test_pool("aw_cuts").await else {
        return;
    };
    let app = test_router(pool.clone());

    // Baseline: 500 kWh × 8.11 ct = 40.55 EUR.
    let (status, body) = post_json(&app, "/api/v1/anlagen", anlage_json("P-AW")).await;
    assert!(status.is_success(), "register: {status} {body}");
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-AW/settle/2026/7",
        serde_json::json!({ "einspeisemenge_kwh": "500" }),
    )
    .await;
    assert!(status.is_success(), "baseline settle: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let baseline: rust_decimal::Decimal =
        serde_json::from_value(v["settlement_eur"].clone()).expect("settlement present");
    assert_eq!(baseline, rust_decimal::Decimal::new(4055, 2));

    // §53b: a Regionalnachweis is on file for the period → AW 8.11 → 8.01.
    sqlx::query(
        "INSERT INTO eeg_regionalnachweise (tr_id, tenant, nachweis_ref, effective_from)
         VALUES ('P-AW', $1, 'HKNR-RN-2026-0001', '2026-01-01')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("record the Regionalnachweis");

    // §53c: a granted Stromsteuerbefreiung of 2.05 ct/kWh → AW 8.01 → 5.96.
    sqlx::query(
        "INSERT INTO eeg_stromsteuerbefreiungen
             (tr_id, tenant, befreiung_ct_kwh, rechtsgrundlage, effective_from)
         VALUES ('P-AW', $1, 2.05, '§9 Abs. 1 Nr. 1 StromStG', '2026-01-01')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("record the Stromsteuerbefreiung");

    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-AW/settle/2026/8",
        serde_json::json!({ "einspeisemenge_kwh": "500" }),
    )
    .await;
    assert!(status.is_success(), "reduced settle: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let reduced: rust_decimal::Decimal =
        serde_json::from_value(v["settlement_eur"].clone()).expect("settlement present");
    // 500 kWh × 5.96 ct = 29.80 EUR
    assert_eq!(
        reduced,
        rust_decimal::Decimal::new(2980, 2),
        "§53b (0,1 ct) and §53c (2,05 ct) must both cut the AW"
    );

    // The §53c cap is the full §3 StromStG rate — an exemption cannot exceed the
    // tax it exempts from.
    let over_cap = sqlx::query(
        "INSERT INTO eeg_stromsteuerbefreiungen
             (tr_id, tenant, befreiung_ct_kwh, rechtsgrundlage, effective_from)
         VALUES ('P-AW', $1, 3.00, 'bogus', '2026-01-01')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await;
    assert!(
        over_cap.is_err(),
        "a Stromsteuerbefreiung above 2,05 ct/kWh must be rejected by the CHECK"
    );

    // A §54 row that records no defect deducts nothing and is a data-entry error.
    let no_defect = sqlx::query(
        "INSERT INTO eeg_sect54_solar_defekte (tr_id, tenant, effective_from)
         VALUES ('P-AW', $1, '2026-01-01')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await;
    assert!(
        no_defect.is_err(),
        "a §54 row with no defect set must be rejected by the CHECK"
    );

    // A reversed validity period is not a period.
    let reversed = sqlx::query(
        "INSERT INTO eeg_regionalnachweise
             (tr_id, tenant, nachweis_ref, effective_from, effective_until)
         VALUES ('P-AW', $1, 'HKNR-RN-BAD', '2026-06-01', '2026-01-01')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await;
    assert!(
        reversed.is_err(),
        "effective_until < effective_from must be rejected"
    );
}

/// §24 Abs. 1 EEG 2023 — a merge the statute does not support is refused.
///
/// Zusammenlegung changes the plant size that §21 Abs. 1 / §22 read, so a merge
/// of two plants §24 keeps apart moves the survivor into a tariff band and past
/// a tender threshold it never qualified for — for the rest of its Förderdauer,
/// and indistinguishably from a legitimate merge once written.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_merger_outside_sect24_is_refused() {
    let Some((pool, _pg)) = test_pool("sect24").await else {
        return;
    };
    let app = test_router(pool.clone());

    // Two rooftop PV plants, same site, three months apart → §24 fuses them.
    for (tr, ibn) in [("P-24-A", "2024-01-15"), ("P-24-B", "2024-03-15")] {
        let mut a = anlage_json(tr);
        a["inbetriebnahme"] = serde_json::json!(ibn);
        let (status, body) = post_json(&app, "/api/v1/anlagen", a).await;
        assert!(status.is_success(), "register {tr}: {status} {body}");
    }
    sqlx::query("UPDATE eeg_anlagen SET standort_id = 'FLST-1' WHERE tenant = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .expect("set the shared site");

    // A third plant on a different site, well outside the twelve-month window.
    let mut c = anlage_json("P-24-C");
    c["inbetriebnahme"] = serde_json::json!("2020-05-01");
    let (status, body) = post_json(&app, "/api/v1/anlagen", c).await;
    assert!(status.is_success(), "register C: {status} {body}");
    sqlx::query("UPDATE eeg_anlagen SET standort_id = 'FLST-9' WHERE tr_id = 'P-24-C'")
        .execute(&pool)
        .await
        .expect("set the other site");

    // Different site, no proximity asserted → Satz 1 Nr. 1 refuses.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-24-C/zusammenlegen",
        serde_json::json!({ "parent_tr_id": "P-24-A" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a merge across sites must be refused: {body}"
    );
    assert!(
        body.contains("Satz1Nr1StandortVerschieden"),
        "the refusal must name the rule that decided: {body}"
    );

    // Proximity asserted, but still six years apart → Satz 1 Nr. 4 refuses.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-24-C/zusammenlegen",
        serde_json::json!({
            "parent_tr_id": "P-24-A",
            "unmittelbare_raeumliche_naehe": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body.contains("Satz1Nr4AusserhalbZwoelfMonatsfenster"),
        "the twelve-month window must be the stated reason: {body}"
    );

    // The child must still be settleable — a refused merge changes nothing.
    let status: String =
        sqlx::query_scalar("SELECT status FROM eeg_anlagen WHERE tr_id = 'P-24-C'")
            .fetch_one(&pool)
            .await
            .expect("read status");
    assert_eq!(
        status, "aktiv",
        "a refused merge must not deregister the child"
    );

    // Same site, three months apart → permitted.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-24-B/zusammenlegen",
        serde_json::json!({ "parent_tr_id": "P-24-A", "combined_leistung_kwp": "19.0" }),
    )
    .await;
    assert!(
        status.is_success(),
        "a §24 merger must be permitted: {status} {body}"
    );
    let merged: String =
        sqlx::query_scalar("SELECT status FROM eeg_anlagen WHERE tr_id = 'P-24-B'")
            .fetch_one(&pool)
            .await
            .expect("read status");
    assert_eq!(merged, "abgemeldet");
}

/// The §§53b–54 facts are recordable and inspectable through the API, and
/// §54 Abs. 3 Satz 2/3 lapses by closing the period rather than deleting it.
///
/// These rows silently shrink a Gutschrift, so the seam that matters is whether
/// a settlement run picks up exactly what the GET reports — and whether a late
/// Nachweis stops the deduction going forward without erasing that the plant was
/// ever short.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn aw_reduction_facts_are_recorded_inspected_and_lapse_correctly() {
    let Some((pool, _pg)) = test_pool("aw_api").await else {
        return;
    };
    let app = test_router(pool.clone());

    let (status, body) = post_json(&app, "/api/v1/anlagen", anlage_json("P-API")).await;
    assert!(status.is_success(), "register: {status} {body}");

    // Nothing on file → nothing cutting the AW.
    let (status, body) = get_json(&app, "/api/v1/anlagen/P-API/aw-reduktionen?on=2026-07-01").await;
    assert!(status.is_success(), "empty GET: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        v["reduktionen"].as_array().expect("array").is_empty(),
        "a plant with no facts on file has no AW cuts: {body}"
    );

    // §53b — record a Regionalnachweis. No amount is accepted; it is statutory.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-API/aw-reduktionen/regionalnachweis",
        serde_json::json!({ "nachweis_ref": "HKNR-RN-2026-0007", "effective_from": "2026-01-01" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "record §53b: {body}");
    assert!(
        body.contains("0.1"),
        "the response must state the statutory rate: {body}"
    );

    // §54 Abs. 3 — record the missing Agri-PV Nutzungsnachweis.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-API/aw-reduktionen/sect54-defekt",
        serde_json::json!({
            "agri_nutzungsnachweis_fehlt": true,
            "effective_from": "2026-01-01",
            "bnetza_ref": "BNetzA-54-2026-PV-004"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "record §54: {body}");
    let created: serde_json::Value = serde_json::from_str(&body).unwrap();
    let defekt_id = created["id"].as_str().expect("id").to_owned();

    // A §54 row recording no defect deducts nothing and is refused.
    let (status, _) = post_json(
        &app,
        "/api/v1/anlagen/P-API/aw-reduktionen/sect54-defekt",
        serde_json::json!({ "effective_from": "2026-01-01" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a defect-free §54 row must be refused"
    );

    // A reversed period is refused with a reason, not a constraint name.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-API/aw-reduktionen/regionalnachweis",
        serde_json::json!({
            "nachweis_ref": "HKNR-RN-BAD",
            "effective_from": "2026-06-01",
            "effective_until": "2026-01-01"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.contains("before effective_from"),
        "explain why: {body}"
    );

    // Both cuts are now visible, with their statutory amounts.
    let (status, body) = get_json(&app, "/api/v1/anlagen/P-API/aw-reduktionen?on=2026-07-01").await;
    assert!(status.is_success(), "{body}");
    assert!(
        body.contains("§53b") && body.contains("§54 Abs. 3"),
        "both cuts listed: {body}"
    );

    // …and the settlement agrees: AW 8.11 − 0.1 (§53b) − 2.5 (§54 Abs. 3) = 5.51.
    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen/P-API/settle/2026/7",
        serde_json::json!({ "einspeisemenge_kwh": "1000" }),
    )
    .await;
    assert!(status.is_success(), "settle: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let eur: rust_decimal::Decimal =
        serde_json::from_value(v["settlement_eur"].clone()).expect("settlement");
    assert_eq!(
        eur,
        rust_decimal::Decimal::new(5510, 2),
        "the settlement must apply exactly the cuts the GET reported"
    );

    // §54 Abs. 3 Satz 2/3 — the Nachweis arrives; the deduction lapses.
    let (status, body) = post_json(
        &app,
        &format!(
            "/api/v1/anlagen/P-API/aw-reduktionen/sect54-defekt/{defekt_id}/nachweis-erbracht"
        ),
        serde_json::json!({ "effective_until": "2026-07-31" }),
    )
    .await;
    assert!(status.is_success(), "close the §54 period: {status} {body}");

    // August: only §53b remains.
    let (status, body) = get_json(&app, "/api/v1/anlagen/P-API/aw-reduktionen?on=2026-08-01").await;
    assert!(status.is_success(), "{body}");
    assert!(
        body.contains("§53b"),
        "§53b is open-ended and stays: {body}"
    );
    assert!(
        !body.contains("§54 Abs. 3"),
        "a supplied Nachweis must stop the §54 deduction going forward: {body}"
    );

    // July still carries it — the row was closed, not deleted, so the §147 AO
    // trail still shows the plant was short then.
    let (_, body) = get_json(&app, "/api/v1/anlagen/P-API/aw-reduktionen?on=2026-07-01").await;
    assert!(
        body.contains("§54 Abs. 3"),
        "closing a period must not erase the past: {body}"
    );
}

// ── §51 — the regime is the commissioning date ────────────────────────────────

/// Two plants, identical but for their commissioning date, settle differently.
///
/// The Solarspitzengesetz took effect on 25.02.2025, inside a calendar year and
/// inside the EEG 2023 range. A 200 kWp plant commissioned in 2024 is under the
/// exemption of the Fassung that governs it (§51 Abs. 2 EEG 2023 as enacted:
/// below 400 kW) and must be paid in full; the same plant commissioned in June
/// 2025 is above the 100-kW exemption and is reduced. Reading §51 off the law
/// *year* collapsed the two and under-paid the 2024 plant every month it fed in
/// at a negative price.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_sect51_regime_follows_the_commissioning_date() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("sect51_regime").await else {
        return;
    };

    let settle = |tr_id: &'static str, ibn: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO eeg_anlagen
                   (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
                    erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum, einspeiser_id)
                 VALUES ($1, $2, '51238696781', 2023, $3::date, 200,
                         'SOLAR_FREIFLAECHE', 8.11, 'VERGUETUNG', '2045-12-31', 'EB-1')",
            )
            .bind(tr_id)
            .bind(TENANT)
            .bind(ibn)
            .execute(&pool)
            .await
            .expect("seed");

            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, tr_id)
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                6,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(1000)),
                    kwh_during_negative_epex: Some(dec!(50)),
                    negative_price_quarter_hours: Some(96),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    let alt = settle("TR-51-2024", "2024-06-01").await;
    assert_eq!(
        alt.einspeisemenge_kwh,
        Some(dec!(1000)),
        "a 200 kWp plant commissioned in 2024 is inside the 400 kW exemption"
    );

    let neu = settle("TR-51-2025", "2025-06-01").await;
    assert_eq!(
        neu.einspeisemenge_kwh,
        Some(dec!(950)),
        "the same plant under the Solarspitzengesetz is reduced from 100 kW up"
    );

    // §51a follows §51: only the reduced plant accrues an extension claim.
    let qh: i64 = sqlx::query_scalar(
        "SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2",
    )
    .bind("TR-51-2025")
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .expect("read counter");
    assert_eq!(qh, 96);
}

/// A correction that carries no §51 figures must not hand back the extension.
///
/// The accrual row holds the period's absolute contribution, so `None` has to mean
/// "unchanged". Recording it as "this period had no negative-price quarter-hours"
/// would reverse the §51a Förderende extension the original settlement earned.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_without_sect51_figures_keeps_the_accrual() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("correction_accrual").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum, einspeiser_id)
         VALUES ('TR-KORR', $1, '51238696781', 2023, '2025-06-01', 500,
                 'SOLAR_FREIFLAECHE', 8.11, 'VERGUETUNG', '2045-12-31', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let run = |overrides: einsd::pg::SettleOverrides| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-KORR")
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                6,
                overrides,
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    run(einsd::pg::SettleOverrides {
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(50)),
        negative_price_quarter_hours: Some(192),
        ..Default::default()
    })
    .await;

    let qh_after_initial: i64 = sqlx::query_scalar(
        "SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = 'TR-KORR'",
    )
    .fetch_one(&pool)
    .await
    .expect("read counter");
    assert_eq!(qh_after_initial, 192);

    // A meter-reading correction: the kWh changed, §51 was not re-derived.
    let korr = run(einsd::pg::SettleOverrides {
        einspeisemenge_kwh: Some(dec!(900)),
        correction: Some(einsd::pg::Korrektur {
            original_id: None,
            reason: eeg_billing::scheme::CorrectionReason::MeterDataCorrected,
            detail: Some("Ersatzwert durch Messwert ersetzt".to_owned()),
        }),
        ..Default::default()
    })
    .await;
    assert_eq!(korr.status, "calculated");

    let qh_after_correction: i64 = sqlx::query_scalar(
        "SELECT negative_price_qh_gesamt FROM eeg_anlagen WHERE tr_id = 'TR-KORR'",
    )
    .fetch_one(&pool)
    .await
    .expect("read counter");
    assert_eq!(
        qh_after_correction, 192,
        "a correction silent about §51 must leave the §51a claim standing"
    );

    // The chain is visible: the history lists both rows and says which is which.
    let receipts = einsd::pg::list_settlement_receipts(&pool, TENANT, "TR-KORR", 10)
        .await
        .expect("list receipts");
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .any(|r| r["is_correction"].as_bool() == Some(true)),
        "the correction must be marked as one: {receipts:?}"
    );
    assert!(
        receipts.iter().any(|r| r["correction_reason"]
            .as_str()
            .is_some_and(|s| s.contains("Ersatzwert"))),
        "the stated reason belongs in the § 147 AO audit trail: {receipts:?}"
    );
}

/// The §48 Abs. 2a Volleinspeisung rates must actually be in the table.
///
/// `verguetungsform` is part of the key on both sides. Without it the
/// Volleinspeisung rows collide with the Überschuss rows on
/// `(erzeugungsart, leistung_min_kwp, billing_start)`, the seed's
/// `ON CONFLICT DO NOTHING` drops every one of them, the migration still
/// reports success — and the lookup could not find them in any case.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn volleinspeisung_rates_survive_the_seed_and_are_selectable() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("volleinspeisung").await else {
        return;
    };

    let ueberschuss = einsd::pg::lookup_verguetungssatz(
        &pool,
        "SOLAR_AUFDACH",
        "UEBERSCHUSS",
        dec!(9.5),
        "2024-06-01",
    )
    .await
    .expect("lookup");
    assert_eq!(ueberschuss, Some(dec!(8.1100)), "§48 Abs. 1 Nr. 1a");

    let voll = einsd::pg::lookup_verguetungssatz(
        &pool,
        "SOLAR_AUFDACH",
        "VOLLEINSPEISUNG",
        dec!(9.5),
        "2024-06-01",
    )
    .await
    .expect("lookup");
    assert_eq!(
        voll,
        Some(dec!(12.9100)),
        "§48 Abs. 2a — the Volleinspeisung bonus row must exist and be reachable"
    );
}

/// A KWKG plant capped in Vollbenutzungsstunden must not be given an EEG
/// twenty-year Förderende.
///
/// §8 KWKG 2023 caps a plant above 2 MW in full-load hours, with a fifteen
/// calendar-year backstop (Abs. 4) — not the EEG's `inbetriebnahme + 20 years`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_hour_capped_kwkg_plant_gets_the_kwkg_backstop() {
    let Some((pool, _pg)) = test_pool("kwkg_foerderende").await else {
        return;
    };
    let app = test_router(pool.clone());
    // Every plant names its Anlagenbetreiber (§ 7 Abs. 1 EEG 2023: the claim is
    // the operator's, not the installation's), so the register refuses one
    // without an `einspeiser_id`.
    seed_operator(&pool, "EB-1", "REGELBESTEUERUNG").await;

    let (status, body) = post_json(
        &app,
        "/api/v1/anlagen",
        serde_json::json!({
            "tr_id": "TR-KWK-H",
            "malo_id": "51238696781",
            "einspeiser_id": "EB-1",
            "eeg_gesetz": 0,
            "inbetriebnahme": "2024-06-01",
            "leistung_kwp": "5000",
            "erzeugungsart": "KWKG",
            "verguetungssatz_ct": "3.00",
            "verguetungsform": "KWK_ZUSCHLAG",
            "settlement_model": "KWKG_ZUSCHLAG",
            "kwk_foerderdauer_h": 30000,
        }),
    )
    .await;
    assert!(status.is_success(), "register: {status} {body}");

    let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-KWK-H")
        .await
        .expect("fetch")
        .expect("plant exists");
    assert_eq!(
        anlage.foerderendedatum,
        time::macros::date!(2039 - 06 - 01),
        "§8 Abs. 4 KWKG 2023: fifteen calendar years, not the EEG twenty"
    );
}

/// The 180-day Förderende alert fires once per plant, not once per sweep.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_foerderende_alert_is_emitted_once_per_plant() {
    let Some((pool, _pg)) = test_pool("alert_once").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum, einspeiser_id)
         VALUES ('TR-ALERT', $1, '51238696781', 2023, '2005-06-01', 9.5,
                 'SOLAR_AUFDACH', 8.11, 'VERGUETUNG', heute() + 30, 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let first = einsd::pg::list_expiring_unalerted(&pool, TENANT, 180)
        .await
        .expect("list");
    assert_eq!(first.len(), 1, "the plant is inside the window");

    einsd::pg::mark_foerderung_alert_sent(&pool, TENANT, "TR-ALERT")
        .await
        .expect("mark");

    let second = einsd::pg::list_expiring_unalerted(&pool, TENANT, 180)
        .await
        .expect("list");
    assert!(
        second.is_empty(),
        "the next sweep must not re-emit the same alert"
    );

    // The dashboard view is deliberately unfiltered — it shows the whole window.
    let all = einsd::pg::list_expiring(&pool, TENANT, 180)
        .await
        .expect("list");
    assert_eq!(all.len(), 1);
}

/// §100 EEG — the Solarspitzengesetz opt-in only starts running once the plant
/// has an iMSys, and then it changes both the §51 regime and the rate.
///
/// The operator declares in Textform that §§ 51 and 51a shall apply; the
/// declaration takes effect at the earliest at the end of the calendar year in
/// which the iMSys goes in. From then the Bestandsanlage forgoes payment during
/// negative prices and is paid 0,6 ct/kWh more for everything else.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_sect100_optin_starts_running_after_the_imsys_year() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("sect100_optin").await else {
        return;
    };
    // A 2019 plant: EEG 2017 regime, 6-hour rule, 500 kW exemption. At 300 kWp it
    // would never be reduced — until it opts in.
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            imesys_rollout_datum, sect51_optin_erklaert_am, einspeiser_id)
         VALUES ('TR-OPTIN', $1, '51238696781', 2017, '2019-06-01', 300,
                 'SOLAR_FREIFLAECHE', 8.00, 'VERGUETUNG', '2039-12-31',
                 '2026-09-01', '2026-03-01', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let settle = |year: i16, month: i16| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-OPTIN")
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                year,
                month,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(1000)),
                    kwh_during_negative_epex: Some(dec!(100)),
                    negative_price_quarter_hours: Some(96),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    // December 2026 — the iMSys is in, but the declaration does not run until the
    // turn of the year.
    let vorher = settle(2026, 12).await;
    assert_eq!(
        vorher.einspeisemenge_kwh,
        Some(dec!(1000)),
        "still the EEG 2017 regime: 300 kWp is under the 500 kW exemption"
    );
    assert_eq!(
        vorher.settlement_eur,
        Some(dec!(80.00)),
        "and still the unmodified rate"
    );

    // January 2027 — the opt-in is in force.
    let nachher = settle(2027, 1).await;
    assert_eq!(
        nachher.einspeisemenge_kwh,
        Some(dec!(900)),
        "§51 now bites from the first negative quarter-hour"
    );
    assert_eq!(
        nachher.settlement_eur,
        Some(dec!(77.40)),
        "900 kWh × (8,00 + 0,60) ct — the §100 uplift is on the AW"
    );
}

// ── §9 / §52 — compliance the registry can actually see ──────────────────────

/// §9 Abs. 2 Nr. 2 EEG — a 50 kW plant on the 60 % Leistungsbegrenzung is
/// compliant, and must not be charged a §52 Abs. 1 Nr. 1 Pflichtzahlung.
///
/// §9 is staged: from 100 kW only Fernsteuerbarkeit satisfies it, the 25–100 kW
/// band may take the 60 % cap instead, and below 25 kW the cap alone is enough.
/// The old check was a flat "≥ 25 kW without a Fernsteuerbarkeit date", which
/// billed 10 €/kW/month — 500 € here — to a plant that had done exactly what the
/// statute offers it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_sixty_percent_cap_is_not_a_sect52_violation() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("sect9_bands").await else {
        return;
    };

    let settle = |tr_id: &'static str, kwp: &'static str, sect9: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO eeg_anlagen
                   (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
                    erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
                    sect9_erfuellung, einspeiser_id)
                 VALUES ($1, $2, '51238696781', 2023, '2024-06-01', $3::numeric,
                         'SOLAR_FREIFLAECHE', 8.11, 'VERGUETUNG', '2044-12-31', $4, 'EB-1')",
            )
            .bind(tr_id)
            .bind(TENANT)
            .bind(kwp)
            .bind(sect9)
            .execute(&pool)
            .await
            .expect("seed");

            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, tr_id)
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                6,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(1000)),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    // 50 kW on the 60 % cap — compliant.
    settle("TR-S9-CAP", "50", "LEISTUNGSBEGRENZUNG_60").await;
    let pflicht: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT pflichtzahlung_eur FROM settlement_receipts WHERE tr_id = 'TR-S9-CAP'",
    )
    .fetch_one(&pool)
    .await
    .expect("read receipt");
    assert!(
        pflicht.is_none_or(|p| p.is_zero()),
        "a 50 kW plant on the 60 % Leistungsbegrenzung owes nothing under §52, got {pflicht:?}"
    );

    // 50 kW with nothing installed — a real violation.
    settle("TR-S9-NONE", "50", "KEINE").await;
    let pflicht: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT pflichtzahlung_eur FROM settlement_receipts WHERE tr_id = 'TR-S9-NONE'",
    )
    .fetch_one(&pool)
    .await
    .expect("read receipt");
    assert!(
        pflicht.is_some_and(|p| p > dec!(0)),
        "a plant that satisfies §9 by no route at all does owe the Nr. 1 charge"
    );

    // 150 kW on the 60 % cap — above 100 kW the alternative is gone (Abs. 2 Nr. 1).
    settle("TR-S9-BIG", "150", "LEISTUNGSBEGRENZUNG_60").await;
    let pflicht: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT pflichtzahlung_eur FROM settlement_receipts WHERE tr_id = 'TR-S9-BIG'",
    )
    .fetch_one(&pool)
    .await
    .expect("read receipt");
    assert!(
        pflicht.is_some_and(|p| p > dec!(0)),
        "from 100 kW the 60 % route no longer satisfies §9"
    );
}

/// §21 Abs. 1 Satz 1 Nr. 3 EEG — the Ausfallvergütung is the ordinary rate minus
/// 20 % (§53 Abs. 3), and running past its Höchstdauern is a §52 Abs. 1 Nr. 5
/// Pflichtverstoß.
///
/// Neither was implemented: the scheme existed, the caller was expected to store
/// a pre-reduced rate and never did, and nothing counted the months. A plant
/// parked on the Ausfallvergütung was paid 25 % more than the statute allows,
/// indefinitely.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_ausfallverguetung_is_reduced_and_time_limited() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("ausfallverguetung").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            sect9_erfuellung, einspeiser_id)
         VALUES ('TR-AV', $1, '51238696781', 2023, '2024-06-01', 500,
                 'SOLAR_FREIFLAECHE', 10.00, 'AUSFALLVERGUETUNG', '2044-12-31',
                 'FERNSTEUERBARKEIT', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let settle = |month: i16| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-AV")
                .await
                .expect("fetch")
                .expect("plant exists");
            let mut tx = pool.begin().await.expect("begin");
            let nutzung =
                einsd::pg::ausfallverguetung_nutzung(&mut tx, "TR-AV", TENANT, 2026, month)
                    .await
                    .expect("usage");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                month,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(1000)),
                    ausfallverguetung: nutzung,
                    ..Default::default()
                },
            );
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    // §53 Abs. 3: 10,00 ct − 20 % = 8,00 ct → 1000 kWh = 80,00 EUR.
    let m1 = settle(1).await;
    assert_eq!(
        m1.settlement_eur,
        Some(dec!(80.00)),
        "the Ausfallvergütung is the ordinary rate reduced by 20 %"
    );

    // Months 1–3 are within the Höchstdauer — no Pflichtzahlung yet.
    settle(2).await;
    let m3 = settle(3).await;
    let pflicht3: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT pflichtzahlung_eur FROM settlement_receipts
          WHERE tr_id = 'TR-AV' AND billing_month = 3",
    )
    .fetch_one(&pool)
    .await
    .expect("read receipt");
    assert!(
        pflicht3.is_none_or(|p| p.is_zero()),
        "three consecutive months is exactly the limit, not past it"
    );
    assert_eq!(m3.settlement_eur, Some(dec!(80.00)));

    // The fourth consecutive month exceeds §21 Abs. 1 Satz 1 Nr. 3.
    settle(4).await;
    let pflicht4: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT pflichtzahlung_eur FROM settlement_receipts
          WHERE tr_id = 'TR-AV' AND billing_month = 4",
    )
    .fetch_one(&pool)
    .await
    .expect("read receipt");
    assert!(
        pflicht4.is_some_and(|p| p > dec!(0)),
        "a fourth consecutive month is a §52 Abs. 1 Nr. 5 Pflichtverstoß, got {pflicht4:?}"
    );
}

/// §52 Abs. 1 Nr. 9 EEG — a Veräußerungsform switch that was never notified.
///
/// The registry has carried an index for exactly this predicate since the column
/// was added, and nothing ever queried it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unnotified_veraeusserungsform_switch_is_charged() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("sect21c_unnotified").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            sect9_erfuellung, last_veraeusserungsform_switch, einspeiser_id)
         VALUES ('TR-21C', $1, '51238696781', 2023, '2024-06-01', 50,
                 'SOLAR_FREIFLAECHE', 8.11, 'VERGUETUNG', '2044-12-31',
                 'FERNSTEUERBARKEIT', '2026-05-01', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-21C")
        .await
        .expect("fetch")
        .expect("plant exists");
    let verstoesse = einsd::sect52::derive_pflichtverstoesse(
        &anlage,
        einsd::sect52::Sect52Context {
            billing_date: time::macros::date!(2026 - 06 - 01),
            ausfallverguetung: einsd::sect52::AusfallverguetungNutzung::default(),
        },
    );
    assert!(
        verstoesse
            .iter()
            .any(|v| v.typ == eeg_billing::SanktionsTyp::ZuordnungsWechselNichtGemeldet),
        "the unnotified switch must be detected: {verstoesse:?}"
    );

    // Once the notification is recorded, the violation lapses.
    sqlx::query(
        "UPDATE eeg_anlagen SET veraeusserungsform_notification_sent_at = now()
          WHERE tr_id = 'TR-21C'",
    )
    .execute(&pool)
    .await
    .expect("notify");
    let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-21C")
        .await
        .expect("fetch")
        .expect("plant exists");
    let verstoesse = einsd::sect52::derive_pflichtverstoesse(
        &anlage,
        einsd::sect52::Sect52Context {
            billing_date: time::macros::date!(2026 - 06 - 01),
            ausfallverguetung: einsd::sect52::AusfallverguetungNutzung::default(),
        },
    );
    assert!(verstoesse.is_empty(), "got {verstoesse:?}");
    let _ = dec!(0);
}

/// §52 Abs. 1 Nr. 4 EEG — a plant above 100 kW settled on an Einspeisevergütung
/// model. The MCP tool reported this and the settlement ignored it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_direktvermarktungspflicht_breach_reaches_the_settlement() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("sect10b_breach").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            sect9_erfuellung, einspeiser_id)
         VALUES ('TR-10B', $1, '51238696781', 2023, '2024-06-01', 250,
                 'SOLAR_FREIFLAECHE', 8.11, 'VERGUETUNG', '2044-12-31',
                 'FERNSTEUERBARKEIT', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-10B")
        .await
        .expect("fetch")
        .expect("plant exists");
    let input = einsd::pg::build_settle_input(
        TENANT,
        &anlage,
        &regelbesteuert(),
        2026,
        6,
        einsd::pg::SettleOverrides {
            einspeisemenge_kwh: Some(dec!(1000)),
            ..Default::default()
        },
    );
    let mut tx = pool.begin().await.expect("begin");
    einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
        .await
        .expect("settle");
    tx.commit().await.expect("commit");

    let pflicht: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT pflichtzahlung_eur FROM settlement_receipts WHERE tr_id = 'TR-10B'",
    )
    .fetch_one(&pool)
    .await
    .expect("read receipt");
    assert!(
        pflicht.is_some_and(|p| p > dec!(0)),
        "a 250 kW plant on the Einspeisevergütung owes the §52 Abs. 1 Nr. 4 charge, got {pflicht:?}"
    );
}

/// §36e / §37e / §39e EEG 2023 — a lapsed Zuschlag stops the settlement.
///
/// The branch that answers "the award has lapsed, nothing left to settle" was
/// read by `run_settlement` and written by nothing: `award_expired` was a stored
/// flag no endpoint or worker ever set, so it was unreachable. The expiry is now
/// derived from the date the award actually lapses.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_lapsed_zuschlag_stops_the_settlement_on_its_date() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("zuschlag_erloeschen").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            sect9_erfuellung, ausschreibungs_zuschlag_id, direktverm_aw_ct,
            zuschlag_erloeschen_datum, einspeiser_id)
         VALUES ('TR-ZUSCHLAG', $1, '51238696781', 2023, '2024-06-01', 2000,
                 'SOLAR_FREIFLAECHE', 0, 'AUSSCHREIBUNG', '2044-12-31',
                 'FERNSTEUERBARKEIT', 'BNETZA-2024-0815', 7.00,
                 '2026-07-01', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let settle = |month: i16| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-ZUSCHLAG")
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                month,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(100000)),
                    epex_avg_ct_kwh: Some(dec!(4.00)),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    // June — the award still stands, so the Marktprämie is paid.
    let juni = settle(6).await;
    assert_eq!(juni.status, "calculated");
    assert!(juni.settlement_eur.is_some_and(|e| e > dec!(0)));

    // July — the award has lapsed; nothing is settled and no Gutschrift is issued.
    let juli = settle(7).await;
    assert_eq!(juli.status, "foerderung_beendet");
    assert_eq!(juli.settlement_eur, Some(dec!(0)));
    assert_eq!(juli.gutschrift_nummer, None);
}

/// §51 Abs. 3 EEG — an Ausfallvergütung claim falls 5 % per calendar day of an
/// unreported negative-price period.
///
/// The operator must report, with the §71 Abs. 1 Nr. 1 data, what it fed in while
/// the Spotmarktpreis was continuously negative. Where nothing establishes that
/// quantity, the month's claim is cut per day such a period touched. A figure
/// derived from the NB's own metering counts as established.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unreported_negative_period_cuts_the_ausfallverguetung() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("sect51_abs3").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            sect9_erfuellung, einspeiser_id)
         VALUES ('TR-513', $1, '51238696781', 2023, '2024-06-01', 500,
                 'SOLAR_FREIFLAECHE', 10.00, 'AUSFALLVERGUETUNG', '2044-12-31',
                 'FERNSTEUERBARKEIT', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    // Two calendar days (Berlin) carry a negative quarter-hour.
    let prices: Vec<einsd::pg::SpotPrice> = ["2026-06-10T12:00:00Z", "2026-06-11T12:00:00Z"]
        .iter()
        .map(|t| einsd::pg::SpotPrice {
            delivery_start: time::OffsetDateTime::parse(
                t,
                &time::format_description::well_known::Rfc3339,
            )
            .expect("parse"),
            resolution_min: 15,
            price_ct_kwh: dec!(-1.5),
        })
        .collect();
    einsd::pg::upsert_spot_prices(&pool, &prices, "test")
        .await
        .expect("load prices");

    let from = time::macros::datetime!(2026-05-31 22:00 UTC);
    let to = time::macros::datetime!(2026-06-30 22:00 UTC);
    let tage = einsd::pg::negative_price_calendar_days(&pool, from, to)
        .await
        .expect("count days");
    assert_eq!(tage, 2, "two calendar days carry a negative price");

    let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-513")
        .await
        .expect("fetch")
        .expect("plant exists");
    let input = einsd::pg::build_settle_input(
        TENANT,
        &anlage,
        &regelbesteuert(),
        2026,
        6,
        einsd::pg::SettleOverrides {
            einspeisemenge_kwh: Some(dec!(1000)),
            sect51_abs3_unreported_days: tage,
            ..Default::default()
        },
    );
    let mut tx = pool.begin().await.expect("begin");
    let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
        .await
        .expect("settle");
    tx.commit().await.expect("commit");

    // §53 Abs. 3 first: 10,00 ct − 20 % = 8,00 ct → 80,00 EUR.
    // §51 Abs. 3 then: two days → −10 % → 72,00 EUR.
    assert_eq!(res.settlement_eur, Some(dec!(72.00)));
}

/// A registration the settlement could not honestly act on is refused.
///
/// The worst of them was a Marktprämie model with no anzulegender Wert: the
/// formula is `max(0, AW − Marktwert)`, so every month settled to EUR 0 with
/// status `calculated` and emitted a payout CloudEvent for that amount —
/// indistinguishable downstream from a month in which nothing was owed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unsettleable_registration_is_refused_with_the_field_named() {
    let Some((pool, _pg)) = test_pool("registration_validation").await else {
        return;
    };
    let app = test_router(pool.clone());

    let cases: [(&str, serde_json::Value, &str); 5] = [
        (
            "Direktvermarktung without an AW",
            serde_json::json!({ "settlement_model": "DIREKTVERMARKTUNG" }),
            "direktverm_aw_ct",
        ),
        (
            "zero capacity",
            serde_json::json!({ "leistung_kwp": "0" }),
            "leistung_kwp",
        ),
        (
            "Mieterstrom without its Zuschlag",
            serde_json::json!({ "settlement_model": "MIETERSTROM" }),
            "mieter_zuschlag_ct",
        ),
        (
            // Complete as a KWK registration, so the statute-coherence check is
            // the one that has to catch it rather than a missing field.
            "a solar plant on the KWKG model",
            serde_json::json!({
                "settlement_model": "KWKG_ZUSCHLAG",
                "kwk_foerderdauer_years": 10,
            }),
            "disagree",
        ),
        (
            "an EEG plant marked as KWKG",
            serde_json::json!({ "eeg_gesetz": 0 }),
            "eeg_gesetz",
        ),
    ];

    for (name, patch, expected) in cases {
        let mut body = anlage_json("P-INVALID");
        for (k, v) in patch.as_object().expect("patch is an object") {
            body[k] = v.clone();
        }
        let (status, resp) = post_json(&app, "/api/v1/anlagen", body).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name} must be refused, got {status}: {resp}"
        );
        assert!(
            resp.contains(expected),
            "{name}: the message must name `{expected}`, got {resp}"
        );
    }

    // Nothing was stored by any of them.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM eeg_anlagen WHERE tr_id = 'P-INVALID'")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(n, 0);
}

/// The annual statement must equal what was actually paid.
///
/// It summed only the non-correction receipts, on the reasoning that a correction
/// must not be *added* to its month. True — but a correction does not supersede
/// its original in place either: it is a separate row and the original stays
/// exactly as it was. The statement therefore reported the superseded amounts,
/// and the one artifact whose stated purpose is to agree with the receipts
/// disagreed with precisely the ones that were paid.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_annual_statement_follows_the_corrections() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("jahresabrechnung_korrektur").await else {
        return;
    };
    // Commissioned in November, so only November and December are owed.
    sqlx::query(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum,
            sect9_erfuellung, einspeiser_id)
         VALUES ('TR-JA', $1, '51238696781', 2023, '2026-11-01', 9.5,
                 'SOLAR_AUFDACH', 10.00, 'VERGUETUNG', '2046-12-31', 'FERNSTEUERBARKEIT', 'EB-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed");

    let settle = |month: i16, kwh: rust_decimal::Decimal, korrektur: bool| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-JA")
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                month,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(kwh),
                    correction: korrektur.then(|| einsd::pg::Korrektur {
                        original_id: None,
                        reason: eeg_billing::scheme::CorrectionReason::MeterDataCorrected,
                        detail: Some("Ersatzwert ersetzt".to_owned()),
                    }),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
        }
    };

    settle(11, dec!(1000), false).await; // 100,00 EUR
    settle(12, dec!(1000), false).await; // 100,00 EUR
    settle(11, dec!(1200), true).await; // corrected to 120,00 EUR

    let ja = einsd::pg::run_jahresabrechnung(&pool, TENANT, "TR-JA", 2026)
        .await
        .expect("statement");

    assert_eq!(
        ja.settlement_eur,
        dec!(220.00),
        "November must contribute its corrected 120,00 EUR, not the superseded 100,00"
    );
    assert_eq!(ja.einspeisemenge_kwh, dec!(2200));
    assert_eq!(ja.correction_count, 1);
    assert_eq!(ja.months_settled, 2);

    // A plant commissioned in November is not missing January through October.
    assert!(
        ja.missing_months.is_empty(),
        "only entitled months can be missing, got {:?}",
        ja.missing_months
    );
    assert_eq!(
        ja.status, "endgueltig",
        "both entitled months are settled, so the year is final"
    );
}

/// A corrected-away KWKG month has to give the Vollbenutzungsstunden back.
///
/// The cumulative `kwk_strom_kwh_gesamt` is what §8 KWKG 2023 measures the 30 000
/// Vollbenutzungsstunden against, so kWh a correction removed must leave it again.
/// The accrual has to be guarded on the *delta*: guarding on the period's new
/// contribution drops the negative delta of a month corrected to zero, leaving the
/// counter burning kWh the plant was never paid for.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_corrected_away_kwkg_month_releases_the_counter() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("kwkg_release").await else {
        return;
    };
    seed_plant(
        &pool,
        "TR-KWK",
        TENANT,
        ", kwk_foerderdauer_h, verguetungsform",
        ", 30000, 'KWK_ZUSCHLAG'",
    )
    .await;
    sqlx::query(
        "UPDATE eeg_anlagen
            SET erzeugungsart = 'KWKG', settlement_model = 'KWKG_ZUSCHLAG', eeg_gesetz = 0
          WHERE tr_id = 'TR-KWK'",
    )
    .execute(&pool)
    .await
    .expect("make it a KWKG plant");

    let settle = |kwh: rust_decimal::Decimal| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-KWK")
                .await
                .expect("fetch")
                .expect("plant exists");
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                6,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(kwh),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    let counter = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<rust_decimal::Decimal>>(
                "SELECT kwk_strom_kwh_gesamt FROM eeg_anlagen WHERE tr_id = 'TR-KWK'",
            )
            .fetch_one(&pool)
            .await
            .expect("read counter")
            .unwrap_or_default()
        }
    };

    settle(dec!(50000)).await;
    assert_eq!(counter().await, dec!(50000));

    // A correction that lowers the month gives the difference back.
    settle(dec!(20000)).await;
    assert_eq!(counter().await, dec!(20000), "a lowered month must release");

    // And one that removes the month entirely releases all of it — this is the
    // case the `period.kwk_kwh > 0` guard silently dropped.
    settle(dec!(0)).await;
    assert_eq!(
        counter().await,
        dec!(0),
        "a month corrected away must not keep burning the §8 KWKG limit"
    );
}

/// Two overlapping settlements of the same plant must not each spend the same
/// remaining §8 KWKG contingent.
///
/// The cumulative counters — `kwk_strom_kwh_gesamt` (§8 KWKG Vollbenutzungs-
/// stunden), `biogas_quota_kwh_ytd` (§44b) and `negative_price_qh_gesamt` (§51a)
/// — are read from the plant row and *also* written from it, so they must be
/// re-read inside the settling transaction. Computed from a snapshot taken
/// outside it, two overlapping runs spend the same remaining contingent twice.
///
/// The receipt upsert only serialises runs for the *same* month; a catch-up
/// sweep, a correction and an operator re-settle all overlap across months.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn overlapping_settlements_cannot_overspend_the_kwkg_contingent() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("kwkg_overspend").await else {
        return;
    };
    seed_plant(
        &pool,
        "TR-RACE",
        TENANT,
        ", kwk_foerderdauer_h, verguetungsform",
        ", 30000, 'KWK_ZUSCHLAG'",
    )
    .await;
    // 30 000 h × 9,5 kW = 285 000 kWh; 280 000 already paid leaves 5 000.
    sqlx::query(
        "UPDATE eeg_anlagen
            SET erzeugungsart = 'KWKG', settlement_model = 'KWKG_ZUSCHLAG', eeg_gesetz = 0,
                kwk_strom_kwh_gesamt = 280000
          WHERE tr_id = 'TR-RACE'",
    )
    .execute(&pool)
    .await
    .expect("make it a KWKG plant near its limit");

    // Both runs take their snapshot of the plant before either commits — the
    // interleaving any two overlapping settlements produce.
    let snapshot_a = einsd::pg::fetch_anlage(&pool, TENANT, "TR-RACE")
        .await
        .expect("fetch")
        .expect("plant exists");
    let snapshot_b = einsd::pg::fetch_anlage(&pool, TENANT, "TR-RACE")
        .await
        .expect("fetch")
        .expect("plant exists");

    let settle = |anlage: einsd::pg::AnlageRow, month: i16| {
        let pool = pool.clone();
        async move {
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                month,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(dec!(4000)),
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    let first = settle(snapshot_a, 5).await;
    let second = settle(snapshot_b, 6).await;

    let paid: rust_decimal::Decimal = first.einspeisemenge_kwh.unwrap_or_default()
        + second.einspeisemenge_kwh.unwrap_or_default();
    assert_eq!(
        paid,
        dec!(5000),
        "only the 5 000 kWh left under the §8 KWKG limit may be settled, got {paid}"
    );

    let counter: rust_decimal::Decimal = sqlx::query_scalar::<_, Option<rust_decimal::Decimal>>(
        "SELECT kwk_strom_kwh_gesamt FROM eeg_anlagen WHERE tr_id = 'TR-RACE'",
    )
    .fetch_one(&pool)
    .await
    .expect("read counter")
    .unwrap_or_default();
    assert_eq!(
        counter,
        dec!(285000),
        "the cumulative counter must not exceed the statutory maximum"
    );
}

/// A correction overwrites nothing, so it must not append a snapshot of a
/// receipt that is still there.
///
/// `settlement_receipt_history` exists because a re-settle of a month overwrites
/// the initial receipt in place. A **correction** is a separate row — the
/// original stays live in `settlement_receipts` — so snapshotting it copies a
/// row nobody is about to lose. `ON CONFLICT DO NOTHING` does not dedupe them
/// either: the table's only unique key is its own surrogate `id`. Three
/// corrections therefore left three identical snapshots of one untouched
/// receipt in the § 147 AO / GoBD trail.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn corrections_do_not_duplicate_the_untouched_original_in_the_audit_trail() {
    use rust_decimal::dec;
    let Some((pool, _pg)) = test_pool("history_dupes").await else {
        return;
    };
    seed_plant(&pool, "TR-HIST", TENANT, "", "").await;

    let settle = |kwh: rust_decimal::Decimal, correction: bool| {
        let pool = pool.clone();
        async move {
            let anlage = einsd::pg::fetch_anlage(&pool, TENANT, "TR-HIST")
                .await
                .expect("fetch")
                .expect("plant exists");
            let korrektur = correction.then_some(einsd::pg::Korrektur {
                original_id: None,
                reason: eeg_billing::scheme::CorrectionReason::MeterDataCorrected,
                detail: None,
            });
            let input = einsd::pg::build_settle_input(
                TENANT,
                &anlage,
                &regelbesteuert(),
                2026,
                6,
                einsd::pg::SettleOverrides {
                    einspeisemenge_kwh: Some(kwh),
                    correction: korrektur,
                    ..Default::default()
                },
            );
            let mut tx = pool.begin().await.expect("begin");
            let res = einsd::pg::run_settlement(&mut tx, input.expect("build settle input"))
                .await
                .expect("settle");
            tx.commit().await.expect("commit");
            res
        }
    };

    let history = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM settlement_receipt_history WHERE tr_id = 'TR-HIST'",
            )
            .fetch_one(&pool)
            .await
            .expect("count history")
        }
    };

    // The initial settlement overwrites nothing yet.
    settle(dec!(1000), false).await;
    assert_eq!(history().await, 0, "nothing was superseded yet");

    // Re-settling the month overwrites the initial receipt in place — that is
    // exactly what the history table is for.
    settle(dec!(1100), false).await;
    assert_eq!(history().await, 1, "the overwritten calculation is kept");

    // Corrections are separate rows. The initial receipt stays live and
    // untouched, so no further snapshot is owed.
    settle(dec!(1200), true).await;
    settle(dec!(1300), true).await;
    assert_eq!(
        history().await,
        1,
        "a correction supersedes nothing in place and must not re-snapshot the original"
    );

    // And the original really is still there, unchanged by the corrections.
    let initial: rust_decimal::Decimal = sqlx::query_scalar(
        "SELECT einspeisemenge_kwh FROM settlement_receipts
          WHERE tr_id = 'TR-HIST' AND is_correction = false",
    )
    .fetch_one(&pool)
    .await
    .expect("initial receipt still present");
    assert_eq!(initial, dec!(1100));
}
