//! The VAT of a § 41e VPP settlement is the period's statutory rate.
//!
//! VAT on the manual run comes from
//! `BillingdConfig::try_regulatory_rates_for_period`, never from the request
//! body. A period straddling a USt Stichtag has no single correct rate, so it
//! is refused before anything is priced, and a body that names a VAT rate is
//! refused as an unknown field: a caller does not get to state the tax on the
//! Gutschrift it is owed.
//!
//! The router runs with the dev configuration and a lazy pool that points at a
//! closed port. A refusal answers before the first query; a request that got
//! past VAT resolution would reach the ledger lookup and answer `500`.

use billingd::daemon::Billingd;

async fn serve() -> std::net::SocketAddr {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/config/billingd.toml");
    let cfg = mako_service::load_config_file::<billingd::config::BillingdConfig>("billingd", &path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let app = mako_service::build_for_test::<Billingd>(cfg)
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    addr
}

fn body(period_from: &str, period_to: &str) -> serde_json::Value {
    serde_json::json!({
        "lf_mp_id": "9910000000002",
        "malo_id": "51238696012",
        "period_from": period_from,
        "period_to": period_to,
        "capacity_price_eur_per_kwh": "0.12",
        "dispatch_events": [{
            "start_utc": "2020-07-15T10:00:00Z",
            "end_utc": "2020-07-15T10:15:00Z",
            "flexibility_kwh": "2.75",
            "tx_id": "tx-vat-1"
        }]
    })
}

async fn post(addr: std::net::SocketAddr, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/billing/vpp/VPP-PORTFOLIO-001"
        ))
        .json(body)
        .send()
        .await
        .expect("the request is answered");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(serde_json::Value::Null))
}

/// Strom carried 16 % from 01.07.2020 (§ 28 Abs. 1 UStG a. F.); a June–July
/// period has no single rate and is refused, naming the Stichtag.
#[tokio::test]
async fn a_vpp_period_straddling_a_vat_stichtag_is_refused() {
    let addr = serve().await;
    let (status, body) = post(addr, &body("2020-06-01", "2020-07-31")).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(
        body["error"]["code"], "ZEITRAUM_UEBERSCHREITET_SATZGRENZE",
        "{body}"
    );
    assert_eq!(
        body["error"]["stichtage"],
        serde_json::json!(["2020-07-01"]),
        "{body}"
    );
}

/// The VAT rate is not the caller's to set.
#[tokio::test]
async fn a_vpp_body_naming_a_vat_rate_is_refused() {
    let addr = serve().await;
    let mut b = body("2026-06-01", "2026-06-30");
    b["mwst_rate_override"] = serde_json::json!("0");
    let (status, body) = post(addr, &b).await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body.to_string().contains("mwst_rate_override"),
        "the refusal names the field: {body}"
    );
}

/// A uniform period gets past VAT resolution — here to the ledger lookup, whose
/// closed pool answers `500`. Without this the two refusals above could pass
/// for a handler that refuses everything.
#[tokio::test]
async fn a_uniform_vpp_period_is_priced() {
    let addr = serve().await;
    let (status, body) = post(addr, &body("2020-07-01", "2020-07-31")).await;
    assert_eq!(status, 500, "{body}");
}
