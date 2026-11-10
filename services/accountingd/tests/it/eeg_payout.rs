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
    /// emits it; `folge` is the chain position einsd wrote it at, and
    /// `korrektur_von` names the receipt a correction supersedes.
    fn receipt(
        &self,
        ce_id: &str,
        folge: i64,
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
            "time": "2026-11-02T08:00:00Z",
            "datacontenttype": "application/json",
            "data": {
                "receipt_id": receipt_id,
                "ketten_folge": folge,
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
    deliver(&app, &p.receipt("ce-orig-1", 1, original, None, "100.00")).await;
    deliver(
        &app,
        &p.receipt("ce-korr-1", 2, Uuid::new_v4(), Some(original), "110.00"),
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
    deliver(&app, &p.receipt("ce-orig-2", 1, original, None, "100.00")).await;
    deliver(
        &app,
        &p.receipt("ce-korr-2", 2, Uuid::new_v4(), Some(original), "90.00"),
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
    let ce = p.receipt("ce-orig-3", 1, Uuid::new_v4(), None, "100.00");
    deliver(&app, &ce).await;
    deliver(&app, &ce).await;
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![10_000]);
}

/// A receipt that arrives after the correction that supersedes it — a retried
/// delivery overtaken by the next one — does not become the period's amount
/// again: the chain is ordered by the position einsd wrote each receipt at,
/// not by arrival.
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
        &p.receipt("ce-korr-4", 2, Uuid::new_v4(), Some(original), "110.00"),
    )
    .await;
    deliver(&app, &p.receipt("ce-orig-4", 1, original, None, "100.00")).await;
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
        &p.receipt("ce-orig-5", 1, Uuid::new_v4(), None, "123.45"),
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

/// einsd states a § 52 Abs. 6 Satz 2 EEG 2023 set-off beside the receipt's
/// claim; it is recorded as the Aufrechnung that pays that part (§ 13 Abs. 1
/// Satz 2 EnFG), the payout pays the rest, and a replay records it once.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_sect52_set_off_is_recorded_and_the_rest_is_paid() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let mut ce = p.receipt("ce-s52-1", 1, Uuid::new_v4(), None, "123.45");
    ce["data"]["sect52"] = serde_json::json!({
        "anspruch_kumuliert_eur": "1000.00",
        "monat_eur": "1000.00",
        "aufgerechnet_eur": "100.00",
        "rest_eur": "900.00",
    });
    deliver(&app, &ce).await;
    deliver(&app, &ce).await;

    let aufrechnungen: Vec<(i64, String)> = sqlx::query_as(
        "SELECT amount_ct, gegenforderung FROM eeg_aufrechnungen
          WHERE tenant = $1 AND tr_id = $2",
    )
    .bind(TENANT)
    .bind(&p.tr_id)
    .fetch_all(&pool)
    .await
    .expect("read");
    assert_eq!(
        aufrechnungen,
        vec![(10_000, "EEG_52_PFLICHTZAHLUNG".to_owned())],
        "the set-off is recorded once"
    );
    assert_eq!(
        p.orders(&pool).await,
        vec![(2_345, Some(p.tr_id.clone()))],
        "the payout pays what the set-off left of the claim"
    );
}

/// The Netzbetreiber reads the payouts it ran — over REST and over MCP, each
/// naming the plant and billing month it paid.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_payouts_run_are_read_over_rest_and_mcp() {
    let Some((app, _pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    deliver(
        &app,
        &p.receipt("ce-read-1", 1, Uuid::new_v4(), None, "42.10"),
    )
    .await;

    let (status, body) = call(
        &app,
        axum::http::Request::get(format!(
            "/api/v1/eeg/payouts?year={}&month={}",
            p.year, p.month
        ))
        .body(axum::body::Body::empty())
        .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let rest = body["payouts"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|o| o["tr_id"] == p.tr_id.as_str())
        .unwrap_or_else(|| panic!("the payout is listed: {body}"))
        .clone();
    assert_eq!(rest["amount_ct"], 4_210);

    let mcp = mako_service::mcp::conformance::Endpoint::serve(app.clone(), "/mcp").await;
    let answer = mcp
        .request_2026(
            "tools/call",
            Some("list_eeg_payouts"),
            serde_json::json!({
                "name": "list_eeg_payouts",
                "arguments": { "year": p.year, "month": p.month },
            }),
        )
        .await;
    let text = answer.message["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result: {}", answer.body));
    let listed: serde_json::Value = serde_json::from_str(text).expect("JSON");
    assert!(
        listed["payouts"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|o| o["tr_id"] == p.tr_id.as_str() && o["amount_ct"] == 4_210),
        "{listed}"
    );
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
        &p.receipt("ce-orig-6", 1, Uuid::new_v4(), None, "100.00"),
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
    let mut ce = p.receipt("ce-orig-7", 1, Uuid::new_v4(), None, "100.00");
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
    for field in [
        "billing_month",
        "receipt_id",
        "tr_id",
        "settlement_eur",
        "ketten_folge",
    ] {
        let mut ce = p.receipt("ce-x", 1, Uuid::new_v4(), None, "1.00");
        ce["data"].as_object_mut().expect("data").remove(field);
        let (status, body) = call(&app, post_json("/webhook", &ce)).await;
        assert_eq!(
            status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {body}"
        );
    }
}

impl Periode {
    /// The same receipt announced as `de.eeg.marktpraemie.berechnet` — a
    /// Direktvermarktung plant's Marktprämie.
    fn marktpraemie(&self, ce_id: &str, folge: i64, settlement_eur: &str) -> serde_json::Value {
        let mut ce = self.receipt(ce_id, folge, Uuid::new_v4(), None, settlement_eur);
        ce["type"] = "de.eeg.marktpraemie.berechnet".into();
        ce["data"]["settlement_model"] = "DIREKTVERMARKTUNG".into();
        ce
    }

    /// A receipt re-settled to `kein_anspruch`: the period owes nothing.
    fn kein_anspruch(&self, ce_id: &str, folge: i64, receipt_id: Uuid) -> serde_json::Value {
        let mut ce = self.receipt(ce_id, folge, receipt_id, None, "0");
        ce["data"]["status"] = "kein_anspruch".into();
        ce
    }

    /// Every payout order's EndToEndId, oldest first.
    async fn end_to_end_refs(&self, pool: &sqlx::PgPool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT end_to_end_ref FROM eeg_payout_orders
              WHERE tenant = $1 AND malo_id = $2 ORDER BY created_at",
        )
        .bind(TENANT)
        .bind(&self.malo_id)
        .fetch_all(pool)
        .await
        .expect("read payout orders")
    }

    /// The period's row in `GET /api/v1/eeg/rueckforderungen`, if listed.
    async fn rueckforderung(&self, app: &axum::Router) -> Option<serde_json::Value> {
        let (status, body) = call(
            app,
            axum::http::Request::get("/api/v1/eeg/rueckforderungen")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        body["rueckforderungen"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|r| r["tr_id"] == self.tr_id.as_str())
            .cloned()
    }
}

/// The bank's pain.002 saying the credit transfer `e2e` settled (`ACSC`).
fn pain002_acsc(e2e: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pain.002.001.10">
  <CstmrPmtStsRpt>
    <GrpHdr><MsgId>STS-EEG-1</MsgId><CreDtTm>2026-11-03T10:00:00</CreDtTm></GrpHdr>
    <OrgnlGrpInfAndSts><OrgnlMsgId>EEG-RUN-1</OrgnlMsgId><OrgnlMsgNmId>pain.001.001.09</OrgnlMsgNmId></OrgnlGrpInfAndSts>
    <OrgnlPmtInfAndSts>
      <OrgnlPmtInfId>EEG-RUN-1-1</OrgnlPmtInfId>
      <TxInfAndSts>
        <OrgnlEndToEndId>{e2e}</OrgnlEndToEndId>
        <TxSts>ACSC</TxSts>
      </TxInfAndSts>
    </OrgnlPmtInfAndSts>
  </CstmrPmtStsRpt>
</Document>"#
    )
}

async fn settle_by_pain002(app: &axum::Router, e2e: &str) {
    let request = axum::http::Request::post("/api/v1/sepa/pain002")
        .header("content-type", "application/xml")
        .body(axum::body::Body::from(pain002_acsc(e2e)))
        .expect("request");
    let (status, body) = call(app, request).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["payouts_updated"], 1, "{body}");
}

/// A plant paid 100,00 € whose period is re-settled to `kein_anspruch` — einsd
/// announces the zero — owes the 100,00 € back.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_period_resettled_to_kein_anspruch_is_owed_back_in_full() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let receipt = Uuid::new_v4();
    deliver(&app, &p.receipt("ce-null-1", 1, receipt, None, "100.00")).await;
    deliver(&app, &p.kein_anspruch("ce-null-2", 2, receipt)).await;
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![10_000], "the zero pays nothing");
    let row = p
        .rueckforderung(&app)
        .await
        .expect("the paid 100,00 € is owed back");
    assert_eq!(row["rueckforderung_ct"], 10_000, "{row}");
    assert_eq!(row["anspruch_ct"], 0, "{row}");
}

/// A Direktvermarktung receipt is paid its Marktprämie once, on the
/// auto-payout path; a replay of the event pays nothing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_direktvermarktung_receipt_is_paid_its_marktpraemie_once() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let ce = p.marktpraemie("ce-mp-1", 1, "42.10");
    deliver(&app, &ce).await;
    deliver(&app, &ce).await;
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(amounts, vec![4_210], "the Marktprämie is paid, once");
}

/// `POST /api/v1/eeg/payouts/run` pays a Marktprämie chain like a Vergütung
/// one, and a second run pays nothing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_payout_run_pays_the_marktpraemie() {
    let Some((app, pool, _pg)) = app(false).await else {
        return;
    };
    let p = Periode::new();
    deliver(&app, &p.marktpraemie("ce-mp-2", 1, "42.10")).await;
    let run = serde_json::json!({ "billing_year": p.year, "billing_month": p.month });
    for _ in 0..2 {
        let (status, body) = call(&app, post_json("/api/v1/eeg/payouts/run", &run)).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    }
    assert_eq!(
        p.orders(&pool).await,
        vec![(4_210, Some(p.tr_id.clone()))],
        "the run pays the Marktprämie under its plant, once"
    );
}

/// A payout the bank settles (`ACSC`) is booked on the Kontokorrent: the
/// 100,00 € claim and its payment leave the account at 0. A replayed pain.002
/// books nothing more.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_settled_payout_leaves_the_account_at_zero() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    deliver(
        &app,
        &p.receipt("ce-acsc-1", 1, Uuid::new_v4(), None, "100.00"),
    )
    .await;
    assert_eq!(balance_ct(&pool, &p.malo_id).await, -10_000, "the claim");
    let e2e = p.end_to_end_refs(&pool).await.remove(0);
    settle_by_pain002(&app, &e2e).await;
    assert_eq!(
        balance_ct(&pool, &p.malo_id).await,
        0,
        "the payment discharges the claim"
    );
    settle_by_pain002(&app, &e2e).await;
    assert_eq!(balance_ct(&pool, &p.malo_id).await, 0, "booked once");
}

/// An amount owed back is a receivable on the Kontokorrent; once the operator
/// repays it, it leaves the Rückforderungen and the account is square.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_repaid_rueckforderung_leaves_the_list() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let receipt = Uuid::new_v4();
    deliver(&app, &p.receipt("ce-rz-1", 1, receipt, None, "100.00")).await;
    let e2e = p.end_to_end_refs(&pool).await.remove(0);
    settle_by_pain002(&app, &e2e).await;
    deliver(&app, &p.kein_anspruch("ce-rz-2", 2, receipt)).await;
    assert_eq!(
        balance_ct(&pool, &p.malo_id).await,
        10_000,
        "the operator owes the 100,00 € paid"
    );
    assert!(p.rueckforderung(&app).await.is_some());

    let rueckzahlung = |amount_ct: i64| {
        serde_json::json!({
            "tr_id": p.tr_id,
            "billing_year": p.year,
            "billing_month": p.month,
            "amount_ct": amount_ct,
            "eingegangen_am": "2026-11-06",
        })
    };
    let (status, body) = call(
        &app,
        post_json("/api/v1/eeg/rueckzahlungen", &rueckzahlung(10_001)),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "more than is owed back: {body}"
    );
    let (status, body) = call(
        &app,
        post_json("/api/v1/eeg/rueckzahlungen", &rueckzahlung(10_000)),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");
    assert_eq!(body["verbleibend_ct"], 0, "{body}");

    assert!(
        p.rueckforderung(&app).await.is_none(),
        "a repaid Rückforderung leaves the list"
    );
    assert_eq!(
        balance_ct(&pool, &p.malo_id).await,
        0,
        "the account is square"
    );
    let (status, body) = call(
        &app,
        post_json("/api/v1/eeg/rueckzahlungen", &rueckzahlung(100)),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "nothing is owed back any more: {body}"
    );
}

/// § 13 Abs. 1 Satz 1 Nr. 1 EnFG reimburses the § 19 EEG payments „abzüglich
/// der Rückzahlungen nach § 26 Absatz 1 Satz 3" EEG 2023. A paid period
/// re-decided to zero and repaid is served with its Rückzahlung as a negative
/// amount, so the month's payments net to what is owed: nothing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rueckzahlung_is_netted_in_the_enfg_payments() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let receipt = Uuid::new_v4();
    deliver(&app, &p.receipt("ce-enfg-rz-1", 1, receipt, None, "100.00")).await;
    let e2e = p.end_to_end_refs(&pool).await.remove(0);
    settle_by_pain002(&app, &e2e).await;
    deliver(&app, &p.kein_anspruch("ce-enfg-rz-2", 2, receipt)).await;
    let (status, body) = call(
        &app,
        post_json(
            "/api/v1/eeg/rueckzahlungen",
            &serde_json::json!({
                "tr_id": p.tr_id,
                "billing_year": p.year,
                "billing_month": p.month,
                "amount_ct": 10_000,
                "eingegangen_am": "2026-11-06",
            }),
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");

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
    let mut zahlungen: Vec<(String, i64)> = body["zahlungen"]
        .as_array()
        .expect("a list")
        .iter()
        .filter(|z| z["tr_id"] == p.tr_id.as_str())
        .map(|z| {
            (
                z["quelle"].as_str().unwrap_or_default().to_owned(),
                z["amount_ct"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    zahlungen.sort();
    assert_eq!(
        zahlungen,
        vec![
            ("AUSZAHLUNG".to_owned(), 10_000),
            ("RUECKZAHLUNG".to_owned(), -10_000)
        ],
        "{body}"
    );
}

/// Two receipts of one period are applied in the order einsd wrote them
/// (`ketten_folge`), whatever their CloudEvent times say: a replica whose
/// clock runs behind stamps the correction earlier than the receipt it
/// corrects, and the correction is still the period's amount.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn receipts_are_applied_in_their_sequence_not_their_time() {
    let Some((app, pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let original = Uuid::new_v4();
    let mut erst = p.receipt("ce-seq-1", 1, original, None, "100.00");
    erst["time"] = "2026-11-05T08:00:00Z".into();
    let mut dann = p.receipt("ce-seq-2", 2, Uuid::new_v4(), Some(original), "110.00");
    dann["time"] = "2026-11-02T08:00:00Z".into();
    deliver(&app, &erst).await;
    deliver(&app, &dann).await;
    let amounts: Vec<i64> = p.orders(&pool).await.into_iter().map(|o| o.0).collect();
    assert_eq!(
        amounts,
        vec![10_000, 1_000],
        "the second receipt einsd wrote is the period's amount"
    );
    assert_eq!(balance_ct(&pool, &p.malo_id).await, -11_000);
}

/// A second event claiming a chain position already taken is not a receipt
/// einsd wrote, and is refused rather than ordered by guesswork.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_second_receipt_at_a_taken_position_is_refused() {
    let Some((app, _pool, _pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    deliver(
        &app,
        &p.receipt("ce-pos-1", 1, Uuid::new_v4(), None, "100.00"),
    )
    .await;
    let (status, body) = call(
        &app,
        post_json(
            "/webhook",
            &p.receipt("ce-pos-2", 1, Uuid::new_v4(), None, "90.00"),
        ),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
}

/// A plant operator owing 100,00 € back of a settled payout is not a customer in
/// arrears: the EEG claim is no payment obligation from a supply contract, and
/// the dunning sequence it would enter ends in an interruption of supply. The
/// account is listed in neither `GET /api/v1/offene-posten` nor any dunning
/// case — automatic or manual — and the claim stays visible in
/// `GET /api/v1/eeg/rueckforderungen`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_eeg_rueckforderung_is_never_dunned() {
    let Some((app, pool, pg)) = app(true).await else {
        return;
    };
    let p = Periode::new();
    let receipt = Uuid::new_v4();
    deliver(&app, &p.receipt("ce-mahn-1", 1, receipt, None, "100.00")).await;
    let e2e = p.end_to_end_refs(&pool).await.remove(0);
    settle_by_pain002(&app, &e2e).await;
    deliver(&app, &p.kein_anspruch("ce-mahn-2", 2, receipt)).await;
    assert_eq!(
        balance_ct(&pool, &p.malo_id).await,
        10_000,
        "the operator owes the 100,00 € paid"
    );
    assert!(p.rueckforderung(&app).await.is_some());

    let (status, body) = call(
        &app,
        axum::http::Request::get("/api/v1/offene-posten")
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert!(
        !body
            .as_array()
            .expect("a list")
            .iter()
            .any(|a| a["malo_id"] == p.malo_id.as_str()),
        "the plant account is not an Offener Posten: {body}"
    );

    let account_id: Uuid =
        sqlx::query_scalar("SELECT account_id FROM accounts WHERE tenant = $1 AND malo_id = $2")
            .bind(TENANT)
            .bind(&p.malo_id)
            .fetch_one(&pool)
            .await
            .expect("account");
    let (status, body) = call(
        &app,
        post_json(
            &format!("/api/v1/dunning/{account_id}/escalate"),
            &serde_json::json!({ "stufe": 1, "due_days": 14 }),
        ),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "a manual Mahnung on the plant account is refused: {body}"
    );

    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let ledger = accountingd::ledger::PgLedger::connect(
        &format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres"),
        TENANT,
        mako_fristen::heute(),
    )
    .await
    .expect("ledger");
    let run =
        accountingd::pg::run_auto_dunning(&ledger, &pool, TENANT, 0, &crate::receivables::STUFEN)
            .await
            .expect("the dunning run completes");
    assert_eq!(run.mahnstufe1_created, 0, "no Mahnstufe 1 on the plant");
    let cases: i64 = sqlx::query_scalar("SELECT count(*) FROM dunning_cases WHERE account_id = $1")
        .bind(account_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(cases, 0, "no dunning case on the plant account");
    assert!(
        p.rueckforderung(&app).await.is_some(),
        "the claim is still listed as a Rückforderung"
    );
}
