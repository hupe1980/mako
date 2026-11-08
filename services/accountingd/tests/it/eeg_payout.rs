//! The EEG payout a settlement receipt chain earns — through the router, the
//! way `einsd`'s CloudEvents and the operator's payout run reach it.
//!
//! A settlement period (plant, billing year, billing month) is one receipt
//! chain: the initial receipt and every § 147 AO correction after it each state
//! the **whole** amount owed for the period. What is paid for the chain is
//! therefore the difference between the current receipt and what was already
//! paid or ordered for it, never the corrected amount again; a lower correction
//! pays nothing and leaves the overpayment as a claim on the plant operator.
//!
//! A payout order also has to name its plant and billing month, or the EnFG
//! ledger `einsd` builds from `GET /api/v1/eeg/enfg-zahlungen` cannot attribute
//! it.

use testcontainers::{ContainerAsync, ImageExt as _, runners::AsyncRunner as _};
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

/// The dev configuration's tenant.
const TENANT: &str = "9900357000004";
/// The plant operator's account — a valid German IBAN.
const CREDITOR_IBAN: &str = "DE02120300000000202051";
/// The Netzbetreiber's own account the payouts are debited from.
const DEBTOR_IBAN: &str = "DE89370400440532013000";

/// The dev router with `[eeg]` set as given, over a throwaway PostgreSQL.
async fn app(auto_payout: bool) -> Option<(axum::Router, sqlx::PgPool, ContainerAsync<Postgres>)> {
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
    )]
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../dev/config/accountingd.toml"))
            .expect("the dev configuration parses");
    cfg["database"]["url"] = toml::Value::String(url);
    if let Some(db) = cfg["database"].as_table_mut() {
        db.remove("migration_url");
    }
    let mut eeg = toml::map::Map::new();
    eeg.insert("auto_payout".into(), toml::Value::Boolean(auto_payout));
    eeg.insert(
        "debtor_iban".into(),
        toml::Value::String(DEBTOR_IBAN.into()),
    );
    cfg.as_table_mut()
        .expect("table")
        .insert("eeg".into(), toml::Value::Table(eeg));
    let cfg = cfg.try_into().expect("an accountingd configuration");
    let app = mako_service::build_for_test_on::<accountingd::Accountingd>(cfg, pool.clone())
        .await
        .expect("the router builds");
    Some((app, pool, pg))
}

async fn call(
    app: &axum::Router,
    request: axum::http::Request<axum::body::Body>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt as _;
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, body)
}

fn post_json(uri: &str, body: &serde_json::Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::post(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("request")
}

/// One plant's settlement period.
struct Periode {
    tr_id: String,
    malo_id: String,
    year: i16,
    month: i16,
}

impl Periode {
    fn new() -> Self {
        let hex = Uuid::new_v4().simple().to_string();
        let digits: String = hex.chars().filter(char::is_ascii_digit).take(10).collect();
        Self {
            tr_id: format!("TR-{}", &hex[..12]),
            malo_id: format!("5{digits:0<10}"),
            year: 2026,
            month: 10,
        }
    }

    /// A `de.eeg.verguetung.berechnet` CloudEvent for `receipt_id`, as einsd
    /// emits it; `korrektur_von` names the receipt a correction supersedes.
    fn receipt(
        &self,
        ce_id: &str,
        time: &str,
        receipt_id: Uuid,
        korrektur_von: Option<Uuid>,
        settlement_eur: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "specversion": "1.0",
            "id": ce_id,
            "source": format!("urn:mako:einsd:{TENANT}"),
            "type": "de.eeg.verguetung.berechnet",
            "subject": self.tr_id,
            "time": time,
            "datacontenttype": "application/json",
            "data": {
                "receipt_id": receipt_id,
                "korrektur_von": korrektur_von,
                "tr_id": self.tr_id,
                "malo_id": self.malo_id,
                "billing_year": self.year,
                "billing_month": self.month,
                "settlement_model": "FIXED_TARIFF",
                "settlement_eur": settlement_eur,
                "status": "calculated",
                "bank_iban": CREDITOR_IBAN,
                "zahlungsempfaenger": "Anlagenbetreiber GmbH",
            }
        })
    }

    /// Every payout order of the period, oldest first: `(amount_ct, tr_id)`.
    async fn orders(&self, pool: &sqlx::PgPool) -> Vec<(i64, Option<String>)> {
        sqlx::query_as(
            "SELECT amount_ct, tr_id FROM eeg_payout_orders
              WHERE tenant = $1 AND malo_id = $2
                AND billing_year = $3 AND billing_month = $4
              ORDER BY created_at",
        )
        .bind(TENANT)
        .bind(&self.malo_id)
        .bind(self.year)
        .bind(self.month)
        .fetch_all(pool)
        .await
        .expect("read payout orders")
    }
}

/// The plant's Kontokorrent balance as the ledger booked it (negative = owed
/// to the operator).
async fn balance_ct(pool: &sqlx::PgPool, malo_id: &str) -> i64 {
    sqlx::query_scalar("SELECT balance_ct FROM accounts WHERE tenant = $1 AND malo_id = $2")
        .bind(TENANT)
        .bind(malo_id)
        .fetch_one(pool)
        .await
        .expect("account")
}

async fn deliver(app: &axum::Router, ce: &serde_json::Value) {
    let (status, body) = call(app, post_json("/webhook", ce)).await;
    assert!(status.is_success(), "{status}: {body}");
}

/// A receipt paid at 100,00 € and corrected to 110,00 € pays 10,00 € more.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_higher_correction_pays_the_difference() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let original = Uuid::new_v4();
    deliver(
        &app,
        &p.receipt(
            "ce-orig-1",
            "2026-11-02T08:00:00Z",
            original,
            None,
            "100.00",
        ),
    )
    .await;
    deliver(
        &app,
        &p.receipt(
            "ce-korr-1",
            "2026-11-05T08:00:00Z",
            Uuid::new_v4(),
            Some(original),
            "110.00",
        ),
    )
    .await;

    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(
        amounts,
        vec![10_000, 1_000],
        "the correction pays 10,00 € on top of the 100,00 € already ordered"
    );
    assert_eq!(
        balance_ct(&pool, &p.malo_id).await,
        -11_000,
        "the ledger credits the period once, at its corrected amount"
    );
}

/// A receipt paid at 100,00 € and corrected to 90,00 € pays nothing and
/// records the 10,00 € owed back.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_lower_correction_pays_nothing_and_records_the_overpayment() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let original = Uuid::new_v4();
    deliver(
        &app,
        &p.receipt(
            "ce-orig-2",
            "2026-11-02T08:00:00Z",
            original,
            None,
            "100.00",
        ),
    )
    .await;
    deliver(
        &app,
        &p.receipt(
            "ce-korr-2",
            "2026-11-05T08:00:00Z",
            Uuid::new_v4(),
            Some(original),
            "90.00",
        ),
    )
    .await;

    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![10_000], "a lower correction pays nothing");
    assert_eq!(balance_ct(&pool, &p.malo_id).await, -9_000);

    let (status, body) = call(
        &app,
        axum::http::Request::get("/api/v1/eeg/rueckforderungen")
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let row = body["rueckforderungen"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|r| r["tr_id"] == p.tr_id.as_str())
        .unwrap_or_else(|| panic!("no Rückforderung for the plant: {body}"));
    assert_eq!(row["rueckforderung_ct"], 1_000, "{row}");
    assert_eq!(row["anspruch_ct"], 9_000, "{row}");
    assert_eq!(row["geleistet_ct"], 10_000, "{row}");
    assert_eq!(row["billing_month"], p.month, "{row}");
}

/// A replay of the same CloudEvent pays nothing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_replayed_receipt_pays_nothing() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let ce = p.receipt(
        "ce-orig-3",
        "2026-11-02T08:00:00Z",
        Uuid::new_v4(),
        None,
        "100.00",
    );
    deliver(&app, &ce).await;
    deliver(&app, &ce).await;
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![10_000]);
}

/// A receipt that arrives after the correction that supersedes it — a retried
/// delivery overtaken by the next one — does not become the period's amount
/// again: the chain is ordered by when einsd issued each receipt, not by
/// arrival.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_overtaken_receipt_does_not_reopen_the_period() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let original = Uuid::new_v4();
    deliver(
        &app,
        &p.receipt(
            "ce-korr-4",
            "2026-11-05T08:00:00Z",
            Uuid::new_v4(),
            Some(original),
            "110.00",
        ),
    )
    .await;
    deliver(
        &app,
        &p.receipt(
            "ce-orig-4",
            "2026-11-02T08:00:00Z",
            original,
            None,
            "100.00",
        ),
    )
    .await;
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![11_000]);
}

/// `POST /api/v1/eeg/payouts/run` pays a receipt by its plant and billing
/// month, so the order is one the EnFG ledger can attribute: once the bank
/// settles it, `GET /api/v1/eeg/enfg-zahlungen` serves it under the plant and
/// the month the receipt is for — not the month the run happened to book it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_payout_run_pays_a_receipt_under_its_plant_and_month() {
    let Some((app, pool, _pg)) = app(false).await else {
        return;
    };
    let p = Periode::new();
    deliver(
        &app,
        &p.receipt(
            "ce-orig-5",
            "2026-11-02T08:00:00Z",
            Uuid::new_v4(),
            None,
            "123.45",
        ),
    )
    .await;
    assert!(
        p.orders(&pool).await.is_empty(),
        "auto_payout is off: the receipt waits for the run"
    );

    let run = serde_json::json!({ "billing_year": p.year, "billing_month": p.month });
    let (status, body) = call(&app, post_json("/api/v1/eeg/payouts/run", &run)).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(
        p.orders(&pool).await,
        vec![(12_345, Some(p.tr_id.clone()))],
        "the run pays the receipt under its plant and billing month"
    );
    // A second run finds nothing left to pay.
    let (status, body) = call(&app, post_json("/api/v1/eeg/payouts/run", &run)).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(p.orders(&pool).await.len(), 1, "{body}");

    sqlx::query("UPDATE eeg_payout_orders SET pain002_status = 'ACSC' WHERE malo_id = $1")
        .bind(&p.malo_id)
        .execute(&pool)
        .await
        .expect("settle");
    let (status, body) = call(
        &app,
        axum::http::Request::get(format!(
            "/api/v1/eeg/enfg-zahlungen?year={}&month={}",
            p.year, p.month
        ))
        .body(axum::body::Body::empty())
        .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let zahlungen = body["zahlungen"].as_array().expect("a list");
    assert_eq!(zahlungen.len(), 1, "{body}");
    assert_eq!(zahlungen[0]["tr_id"], p.tr_id.as_str(), "{body}");
    assert_eq!(zahlungen[0]["amount_ct"], 12_345, "{body}");
}

/// An order the bank rejected paid nothing, so the chain is outstanding again
/// and the next run pays it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rejected_payout_is_paid_by_the_next_run() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    deliver(
        &app,
        &p.receipt(
            "ce-orig-6",
            "2026-11-02T08:00:00Z",
            Uuid::new_v4(),
            None,
            "100.00",
        ),
    )
    .await;
    sqlx::query("UPDATE eeg_payout_orders SET pain002_status = 'RJCT' WHERE malo_id = $1")
        .bind(&p.malo_id)
        .execute(&pool)
        .await
        .expect("reject");
    let run = serde_json::json!({ "billing_year": p.year, "billing_month": p.month });
    let (status, body) = call(&app, post_json("/api/v1/eeg/payouts/run", &run)).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["generated"], 1, "{body}");
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![10_000, 10_000]);
}

/// A payout that is owed but cannot be ordered answers an error, so einsd
/// redelivers the event, rather than a `200` for a payment that did not
/// happen. The receipt itself is recorded, so the run pays it once the cause
/// is fixed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_payout_that_cannot_be_ordered_is_not_acknowledged() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let mut ce = p.receipt(
        "ce-orig-7",
        "2026-11-02T08:00:00Z",
        Uuid::new_v4(),
        None,
        "100.00",
    );
    ce["data"]["bank_iban"] = serde_json::Value::String("DE00000000000000000000".into());
    let (status, body) = call(&app, post_json("/webhook", &ce)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "{body}"
    );
    assert!(p.orders(&pool).await.is_empty());
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM eeg_belege WHERE tenant = $1 AND source_ce_id = 'ce-orig-7'",
    )
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(
        recorded, 1,
        "the receipt is kept for the redelivery and the run"
    );
}

/// A receipt that cannot be placed in its chain is refused, not defaulted: a
/// missing billing month would otherwise book it into today's.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_receipt_without_its_chain_is_refused() {
    let Some((app, _pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    for field in ["billing_month", "receipt_id", "tr_id", "settlement_eur"] {
        let mut ce = p.receipt("ce-x", "2026-11-02T08:00:00Z", Uuid::new_v4(), None, "1.00");
        ce["data"].as_object_mut().expect("data").remove(field);
        let (status, body) = call(&app, post_json("/webhook", &ce)).await;
        assert_eq!(
            status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {body}"
        );
    }
}
