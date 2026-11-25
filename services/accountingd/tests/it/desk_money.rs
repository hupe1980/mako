//! The money the Desk shows a Lieferant, through the router `just dev` starts:
//! bank lines no account claimed and their assignment, the Guthaben a
//! Schlussrechnung owes back, and the EEG payout orders.
//!
//! ```bash
//! just test-accountingd-db
//! ```

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};

use crate::router_builds::{call, dev_app};

/// The demo tenant.
const TENANT: &str = "stadtwerke-musterstadt";
/// The demo Lieferant, which its customer accounts are kept under.
const LF: &str = "9900357000028";
const MALO: &str = "51238696012";

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).expect("request")
}

fn post(uri: &str, body: &Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

async fn account(pool: &sqlx::PgPool) {
    accountingd::pg::upsert_account(pool, MALO, LF, TENANT)
        .await
        .expect("account");
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unreadable_issuer_account_refuses_the_advances_read() {
    let (app, pool, _pg) = dev_app().await.expect("PostgreSQL is required");
    account(&pool).await;
    // Leave the advance register readable while the issuer account cannot be decoded.
    sqlx::query("ALTER TABLE accounts RENAME COLUMN billing_day TO unavailable_fixture_day")
        .execute(&pool)
        .await
        .unwrap();
    let path = format!("/api/v1/accounts/{MALO}/abschlaege?lf_mp_id={LF}");
    let (status, body) = call(app, get(&path)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body["error_id"].is_string(), "{body}");
}

/// A bank line no account claims is kept, listed, booked once by the
/// operator's assignment and then gone from the list; the same line imported
/// again is a duplicate, not a second unmatched line.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unmatched_bank_line_is_kept_until_it_is_assigned_once() {
    let Some((app, pool, _pg)) = dev_app().await else {
        return;
    };
    account(&pool).await;
    let line = json!([{
        "iban": "DE89370400440532013000",
        "amount_eur": "42.17",
        "date": "2026-11-02",
        "reference": "Danke fuer alles",
        "bank_transaction_id": "TXN-UNMATCHED-1",
    }]);

    let (status, body) = call(app.clone(), post("/api/v1/payments/import", &line)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unmatched"], 1, "{body}");

    let (status, list) = call(app.clone(), get("/api/v1/payments/unmatched")).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["count"], 1, "{list}");
    let p = &list["payments"][0];
    assert_eq!(p["amount_ct"], 4217, "{list}");
    assert_eq!(p["is_return"], false, "{list}");
    assert_eq!(p["iban_masked"], "DE…3000", "{list}");
    assert_eq!(p["source"], "flat", "{list}");
    assert!(
        !list.to_string().contains("DE89370400440532013000"),
        "{list}"
    );
    let id = p["payment_id"].as_str().expect("id").to_owned();

    // The same line again: still one line waiting.
    let (_, again) = call(app.clone(), post("/api/v1/payments/import", &line)).await;
    assert_eq!(again["unmatched"], 1, "{again}");
    let (_, list) = call(app.clone(), get("/api/v1/payments/unmatched")).await;
    assert_eq!(list["count"], 1, "{list}");

    let assign = format!("/api/v1/payments/{id}/assign");
    let (status, booked) = call(app.clone(), post(&assign, &json!({ "malo_id": MALO }))).await;
    assert_eq!(status, StatusCode::OK, "{booked}");
    assert_eq!(booked["entry_type"], "ZAHLUNG", "{booked}");
    assert_eq!(booked["amount_ct"], 4217, "{booked}");

    let (_, list) = call(app.clone(), get("/api/v1/payments/unmatched")).await;
    assert_eq!(list["count"], 0, "{list}");
    let (_, balance) = call(
        app.clone(),
        get(&format!("/api/v1/accounts/{MALO}/balance")),
    )
    .await;
    assert_eq!(balance["balance_ct"], -4217, "{balance}");

    let (status, body) = call(app.clone(), post(&assign, &json!({ "malo_id": MALO }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "payment_already_assigned", "{body}");

    // Assigned, the line is in the import log: a re-import books nothing.
    let (_, third) = call(app.clone(), post("/api/v1/payments/import", &line)).await;
    assert_eq!(third["deduplicated"], 1, "{third}");
    let (_, balance) = call(app, get(&format!("/api/v1/accounts/{MALO}/balance"))).await;
    assert_eq!(balance["balance_ct"], -4217, "{balance}");
}

/// An assignment to an account the tenant does not keep is refused, and the
/// line stays waiting.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_assignment_to_no_account_is_refused() {
    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    let line = json!([{
        "iban": "DE89370400440532013000",
        "amount_eur": "10.00",
        "date": "2026-11-02",
        "bank_transaction_id": "TXN-UNMATCHED-2",
    }]);
    call(app.clone(), post("/api/v1/payments/import", &line)).await;
    let (_, list) = call(app.clone(), get("/api/v1/payments/unmatched")).await;
    let id = list["payments"][0]["payment_id"]
        .as_str()
        .expect("id")
        .to_owned();
    let (status, body) = call(
        app.clone(),
        post(
            &format!("/api/v1/payments/{id}/assign"),
            &json!({ "malo_id": MALO }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "account_not_found", "{body}");
    let (_, list) = call(app, get("/api/v1/payments/unmatched")).await;
    assert_eq!(list["count"], 1, "{list}");
}

fn invoice_event(id: &str, rechnungsnummer: &str, zu_zahlen: &str, extra: Value) -> Value {
    let mut data = json!({
        "record_id": uuid::Uuid::new_v4().to_string(),
        "malo_id": MALO,
        "lf_mp_id": LF,
        "is_correction": false,
        "schlussrechnung": true,
        "rechnung": {
            "rechnungsnummer": rechnungsnummer,
            "rechnungsdatum": "2026-04-01T00:00:00Z",
            "gesamtbrutto": { "wert": "100.00", "waehrung": "EUR" },
            "zuZahlen": { "wert": zu_zahlen, "waehrung": "EUR" },
        },
    });
    if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            d.insert(k.clone(), v.clone());
        }
    }
    json!({
        "specversion": "1.0",
        "id": id,
        "source": mako_service::source("billingd", TENANT),
        "type": "de.mako.billing.rechnung.erstellt",
        "subject": MALO,
        "data": data,
    })
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_invoice_uses_its_deployment_source_and_explicit_issuer() {
    let (app, pool, _pg) = dev_app().await.expect("PostgreSQL is required");
    let mut event = invoice_event("ce-issuer", "RE-2026-000042", "-42.17", json!({}));
    for source in [
        mako_service::source("billingd", "other-tenant"),
        mako_service::source("billingd", LF),
    ] {
        event["source"] = json!(source);
        let (status, body) = call(app.clone(), post("/webhook", &event)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["code"], "invoice_source_mismatch");
    }
    event["source"] = json!(mako_service::source("billingd", TENANT));
    for invalid_id in [Value::Null, json!(""), json!(" ")] {
        event["id"] = invalid_id.clone();
        let (status, body) = call(app.clone(), post("/webhook", &event)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        event["id"] = json!("ce-issuer");
        event["data"]["malo_id"] = invalid_id;
        let (status, body) = call(app.clone(), post("/webhook", &event)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["code"], "invalid_invoice_account");
        event["data"]["malo_id"] = json!(MALO);
    }
    for issuer in [Value::Null, json!("")] {
        event["data"]["lf_mp_id"] = issuer;
        let (status, body) = call(app.clone(), post("/webhook", &event)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["code"], "invalid_invoice_issuer");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE tenant=$1")
        .bind(TENANT)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let other_issuer = "9800357000028";
    event["data"]["lf_mp_id"] = json!(other_issuer);
    let (status, body) = call(app, post("/webhook", &event)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let issuer: String = sqlx::query_scalar("SELECT lf_mp_id FROM credit_payouts WHERE tenant=$1")
        .bind(TENANT)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(issuer, other_issuer);
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn invoice_amounts_must_fit_both_postings_before_any_money_is_booked() {
    let (app, pool, _pg) = dev_app().await.expect("PostgreSQL is required");
    let mut event = invoice_event(
        "ce-amount",
        "RE-2026-000043",
        "1.00",
        json!({"schlussrechnung": false}),
    );
    for (gross, payable) in [
        (Value::Null, json!({"wert":"1"})),
        (json!({"wert":"invalid"}), Value::Null),
        (json!({"wert":"79228162514264337593543950335"}), Value::Null),
        (json!({"wert":"92233720368547758.08"}), Value::Null),
        (json!({"wert":"-92233720368547758.08"}), Value::Null),
        (json!({"wert":"1"}), json!({"wert":"invalid"})),
        (
            json!({"wert":"92233720368547758.07"}),
            json!({"wert":"-0.01"}),
        ),
    ] {
        event["data"]["rechnung"]["gesamtbrutto"] = gross;
        event["data"]["rechnung"]["zuZahlen"] = payable;
        let (status, body) = call(app.clone(), post("/webhook", &event)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["code"], "invalid_invoice_amount");
    }
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(accounts, 0);
    let invoice = event["data"]["rechnung"].as_object_mut().unwrap();
    invoice.insert("gesamtbrutto".into(), json!({"wert":"0.00"}));
    invoice.remove("zuZahlen");
    let (status, body) = call(app.clone(), post("/webhook", &event)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    event["id"] = json!("ce-settled-advances");
    event["data"]["schlussrechnung"] = json!(true);
    event["data"]["rechnung"]["zuZahlen"] = json!({"wert":"-42.17"});
    let (status, body) = call(app, post("/webhook", &event)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (balance, credit): (i64,i64) = sqlx::query_as("SELECT a.balance_ct,c.amount_ct FROM accounts a JOIN credit_payouts c ON c.tenant=a.tenant AND c.malo_id=a.malo_id AND c.lf_mp_id=a.mp_id")
        .fetch_one(&pool).await.unwrap();
    assert_eq!((balance, credit), (-4217, 4217));
}

/// SR-058: a Schlussrechnung credit raises one payout item however often its
/// event arrives, due two weeks after the Rechnungsdatum and overdue past it;
/// its Storno cancels it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn credit_payout_is_raised_once_per_invoice() {
    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    for ce in ["ce-schluss-1", "ce-schluss-1-redelivered"] {
        let (status, body) = call(
            app.clone(),
            post(
                "/webhook",
                &invoice_event(ce, "RE-2026-000042", "-42.17", json!({})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, list) = call(app.clone(), get("/api/v1/credit-payouts")).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["count"], 1, "{list}");
    let item = &list["payouts"][0];
    assert_eq!(item["amount_ct"], 4217, "{list}");
    assert_eq!(item["due_on"], "2026-04-15", "{list}");
    assert_eq!(item["overdue"], true, "{list}");
    let (_, overdue) = call(app.clone(), get("/api/v1/credit-payouts?overdue=true")).await;
    assert_eq!(overdue["overdue_count"], 1, "{overdue}");

    let storno = json!({
        "specversion": "1.0",
        "id": "ce-storno-1",
        "source": mako_service::source("billingd", TENANT),
        "type": "de.mako.billing.rechnung.erstellt",
        "subject": MALO,
        "data": {
            "record_id": uuid::Uuid::new_v4().to_string(),
            "malo_id": MALO,
            "lf_mp_id": LF,
            "is_correction": true,
            "schlussrechnung": false,
            "rechnung": {
                "rechnungsnummer": "ST-2026-000001",
                "originalRechnungsnummer": "RE-2026-000042",
                "gesamtbrutto": { "wert": "-100.00", "waehrung": "EUR" },
                "zuZahlen": { "wert": "42.17", "waehrung": "EUR" },
            },
        },
    });
    let (status, body) = call(app.clone(), post("/webhook", &storno)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, open) = call(app.clone(), get("/api/v1/credit-payouts")).await;
    assert_eq!(open["count"], 0, "{open}");
    let (_, cancelled) = call(app, get("/api/v1/credit-payouts?status=cancelled")).await;
    assert_eq!(cancelled["count"], 1, "{cancelled}");
}

/// A Guthaben is paid out once: the payout books it and closes the item; a
/// second payout of the same item is refused.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_credit_payout_is_paid_out_once() {
    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    call(
        app.clone(),
        post(
            "/webhook",
            &invoice_event("ce-schluss-2", "RE-2026-000043", "-10.00", json!({})),
        ),
    )
    .await;
    let (_, list) = call(app.clone(), get("/api/v1/credit-payouts")).await;
    let id = list["payouts"][0]["payout_id"]
        .as_str()
        .expect("id")
        .to_owned();
    let paid = format!("/api/v1/credit-payouts/{id}/paid");
    let (status, body) = call(
        app.clone(),
        post(&paid, &json!({ "paid_on": "2026-04-10" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["amount_ct"], 1000, "{body}");
    let (status, body) = call(
        app.clone(),
        post(&paid, &json!({ "paid_on": "2026-04-10" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "credit_payout_not_open", "{body}");
    // The payout is a debit of exactly the Guthaben.
    let (_, ledger) = call(app, get(&format!("/api/v1/accounts/{MALO}/ledger"))).await;
    assert!(
        ledger["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .any(|l| l["entry_type"] == "ZAHLUNG" && l["signed_ct"] == 1000),
        "{ledger}"
    );
}

/// The EEG payout orders are a typed read: an empty book answers the
/// documented shape, not an untyped list.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_eeg_payouts_answer_their_documented_shape() {
    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    let (status, body) = call(app, get("/api/v1/eeg/payouts?tr_id=E0000000001")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "payouts": [], "count": 0, "saturated": false })
    );
}
