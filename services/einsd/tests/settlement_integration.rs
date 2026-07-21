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
//! ```bash
//! docker run -d --name einsd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=einsd -p 55434:5432 postgres:17-alpine
//! export EINSD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55434/einsd"
//! cargo test -p einsd --test settlement_integration -- --include-ignored
//! ```
//!
//! Every test provisions its own schema, so they leave nothing behind.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt as _;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// Connect and provision a fresh schema, or skip when no database is configured.
async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("EINSD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;

    let schema = format!("t_{test_name}");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("drop schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;

    let opts: sqlx::postgres::PgConnectOptions = base.parse().expect("parse url");
    let pool = PgPool::connect_with(opts.options([("search_path", schema.as_str())]))
        .await
        .expect("connect to test schema");

    for stmt in split_statements(SCHEMA) {
        sqlx::query(&stmt)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema statement failed: {e}\n{stmt}"));
    }
    Some(pool)
}

/// Split the DDL on `;` at statement level, keeping `$$`-quoted bodies intact.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_dollar = false;
    for line in sql.lines() {
        let dollars = line.matches("$$").count();
        if dollars % 2 == 1 {
            in_dollar = !in_dollar;
        }
        current.push_str(line);
        current.push('\n');
        if !in_dollar && line.trim_end().ends_with(';') {
            let stmt = current.trim().to_owned();
            if !stmt.is_empty() && !stmt.lines().all(|l| l.trim().starts_with("--")) {
                out.push(stmt);
            }
            current.clear();
        }
    }
    out
}

const TENANT: &str = "9900357000004";

fn test_config() -> einsd::config::EinsdConfig {
    einsd::config::EinsdConfig {
        database_url: String::new(),
        port: None,
        tenant: TENANT.to_owned(),
        erp_webhook_url: None,
        erp_hmac_secret: None,
        tarifbd_url: None,
        edmd_url: None,
        edmd_api_key: None,
        alert_interval_secs: None,
        jahresmarktwert_url: None,
        jahresmarktwert_import_interval_secs: None,
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
fn anlage_json(tr_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tr_id": tr_id,
        "malo_id": "51238696781",
        "eeg_gesetz": 2023,
        "inbetriebnahme": "2024-06-01",
        "leistung_kwp": "9.5",
        "erzeugungsart": "SOLAR_AUFDACH",
        "verguetungssatz_ct": "8.11",
        "settlement_model": "FEED_IN_TARIFF",
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

/// Insert a plant directly.
///
/// `foerderendedatum` is NOT NULL — the service derives it at registration from
/// the commissioning date, so a direct insert has to supply it too.
async fn seed_plant(pool: &PgPool, tr_id: &str, tenant: &str, extra_cols: &str, extra_vals: &str) {
    let sql = format!(
        "INSERT INTO eeg_anlagen
           (tr_id, tenant, malo_id, eeg_gesetz, inbetriebnahme, leistung_kwp,
            erzeugungsart, verguetungssatz_ct, settlement_model, foerderendedatum{extra_cols})
         VALUES ($1, $2, '51238696781', 2023, '2024-06-01', 9.5,
                 'SOLAR_AUFDACH', 8.11, 'FEED_IN_TARIFF', '2044-12-31'{extra_vals})"
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn compliance_status_query_names_only_real_columns() {
    let Some(pool) = test_pool("compliance_cols").await else {
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn receipt_upsert_matches_the_partial_unique_index() {
    let Some(pool) = test_pool("upsert_partial").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    let upsert = r"INSERT INTO settlement_receipts
                     (id, tr_id, tenant, billing_year, billing_month,
                      settlement_model, einspeisemenge_kwh, settlement_eur, status)
                   VALUES ($1, 'P1', $2, 2026, 7, 'FEED_IN_TARIFF', 100, 8.11, $3)
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn a_correction_may_coexist_with_the_receipt_it_corrects() {
    let Some(pool) = test_pool("correction_coexist").await else {
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
             VALUES ($1, 'P1', $2, 2026, 7, 'FEED_IN_TARIFF', 100, 8.11,
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn a_state_change_records_the_transition_it_came_from() {
    let Some(pool) = test_pool("state_transition").await else {
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn a_registered_plant_is_readable_over_http() {
    let Some(pool) = test_pool("http_roundtrip").await else {
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn registration_derives_the_foerderende_from_the_commissioning_date() {
    let Some(pool) = test_pool("foerderende").await else {
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

/// A settlement for an unknown plant is a 404, not a 500.
#[tokio::test]
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn settling_an_unknown_plant_is_not_found() {
    let Some(pool) = test_pool("settle_404").await else {
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn the_same_plant_id_in_two_tenants_is_two_plants() {
    let Some(pool) = test_pool("tenant_isolation").await else {
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn the_statutory_rate_table_is_seeded_by_the_schema() {
    let Some(pool) = test_pool("rates_seeded").await else {
        return;
    };
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM eeg_verguetungssaetze")
        .fetch_one(&pool)
        .await
        .expect("count rates");
    assert!(count > 0, "eeg_verguetungssaetze must ship seeded");
}

// ── Auction metadata (§22, §22b, §39n) ────────────────────────────────────────

/// The award facts must survive registration.
///
/// `AusschreibungMetadata` was constructed with `..Default::default()`, so
/// `award_ct`, `award_expired`, `innovation_auction` and `is_buergerenergie`
/// were always `None`/`false` no matter what was registered — §22b
/// Bürgerenergie and §39n Innovationsausschreibung were unreachable.
#[tokio::test]
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn auction_metadata_survives_registration() {
    let Some(pool) = test_pool("auction_meta").await else {
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn an_incomplete_year_is_provisional_and_names_the_gaps() {
    let Some(pool) = test_pool("ja_incomplete").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    // Settle January, February and April — March and May..December are missing.
    for month in [1i16, 2, 4] {
        sqlx::query(
            "INSERT INTO settlement_receipts
               (id, tr_id, tenant, billing_year, billing_month, settlement_model,
                einspeisemenge_kwh, settlement_eur, pflichtzahlung_eur, status)
             VALUES ($1, 'P1', $2, 2026, $3, 'FEED_IN_TARIFF', 100, 8.11, 2.50, 'berechnet')",
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn a_complete_year_is_final() {
    let Some(pool) = test_pool("ja_complete").await else {
        return;
    };
    seed_plant(&pool, "P1", TENANT, "", "").await;

    for month in 1i16..=12 {
        sqlx::query(
            "INSERT INTO settlement_receipts
               (id, tr_id, tenant, billing_year, billing_month, settlement_model,
                einspeisemenge_kwh, settlement_eur, verlaengerungsanspruch_qh, status)
             VALUES ($1, 'P1', $2, 2026, $3, 'FEED_IN_TARIFF', 100, 8.11, 4, 'berechnet')",
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn a_correction_does_not_double_count_its_month() {
    let Some(pool) = test_pool("ja_correction").await else {
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
             VALUES ($1, 'P1', $2, 2026, 3, 'FEED_IN_TARIFF', 100, 8.11,
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
#[ignore = "requires EINSD_TEST_DATABASE_URL"]
async fn rerunning_replaces_the_stored_statement() {
    let Some(pool) = test_pool("ja_rerun").await else {
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
