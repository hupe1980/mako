//! A quotation is priced under billingd's `[policy] behg_regime`, never a copy.
//!
//! The regime decides whether a supplied 2027 nEHS price is admissible at
//! all. A second key in productd could admit a price billingd later refuses,
//! and the customer would sign for a CO₂ cost the invoice cannot bill. So
//! productd reads the one setting from billingd, and without an answer from
//! billingd the Gas position is refused rather than priced under a default.

use energy_billing::BehgRegime;
use productd::billingd::BehgRegimeQuelle;
use productd::handlers::{compute_cost_breakdown, grund, pricing_context_fuer};
use rust_decimal::dec;
use time::macros::date;

/// productd's own `behg_regime` key is refused, naming where the setting is.
#[test]
fn a_productd_behg_regime_key_is_refused() {
    let cfg: productd::config::ProductdConfig = toml::from_str(
        r#"
        tenant = "9900357000004"
        allow_insecure_no_auth = true
        behg_regime = "korridor"
        [database]
        url = "env:DATABASE_URL"
    "#,
    )
    .expect("the key still parses, so it can be named");
    let err = mako_service::validate_config::<productd::Productd>(&cfg)
        .expect_err("a second copy of billingd's setting is refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("billingd") && msg.contains("[policy] behg_regime"),
        "{msg}"
    );
}

/// A billingd answering `GET /api/v1/policy` with its policy report, the shape
/// `mako_service::policy::router` serves.
async fn billingd_stub(regime: &'static str) -> String {
    use axum::routing::get;
    let app = axum::Router::new().route(
        "/api/v1/policy",
        get(move || async move {
            axum::Json(serde_json::json!({ "entries": [
                {
                    "key": "settlement_form", "value": "endrechnung", "origin": "default",
                    "class": "policy", "quelle": "x", "doc": null, "insecure": false,
                    "review_by": null, "review_grund": null
                },
                {
                    "key": "behg_regime", "value": regime, "origin": "explicit",
                    "class": "policy", "quelle": "BT-Drs. 21/7869", "doc": null,
                    "insecure": false, "review_by": null, "review_grund": null
                }
            ]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn gas_produkt() -> serde_json::Value {
    serde_json::json!({ "tarifpreise": [
        { "preistyp": "ARBEITSPREIS_EINTARIF", "preisstaffeln": [ { "preis": "5" } ] }
    ]})
}

fn gas_position() -> productd::pg::AngebotPositionInput {
    serde_json::from_value(serde_json::json!({
        "product_code": "GAS-B2B-12",
        "sparte": "GAS",
        "jahresverbrauch_kwh": "1000",
        "energiesteuer_gas_ct_per_kwh": "0.55"
    }))
    .expect("a Gas position")
}

/// With billingd on `korridor` and productd configured with no regime of its
/// own, a 2027 point outside 55–65 EUR/t is refused and one inside prices
/// the position.
#[tokio::test]
async fn a_quotation_applies_billingds_korridor() {
    let base = billingd_stub("korridor").await;
    let quelle = BehgRegimeQuelle::billingd(&base, Some("key".into()), reqwest::Client::new());
    let regime = quelle.regime().await;
    assert_eq!(regime, Ok(BehgRegime::Korridor));

    let stichtag = date!(2027 - 03 - 01);
    let draussen = pricing_context_fuer(
        stichtag,
        Some((date!(2027 - 02 - 10), dec!(70))),
        regime.clone(),
        false,
    );
    assert!(
        draussen.behg_gas_ct_per_kwh.is_err(),
        "70 EUR/t lies outside the corridor billingd bills under"
    );
    let drinnen = pricing_context_fuer(
        stichtag,
        Some((date!(2027 - 02 - 10), dec!(60))),
        regime,
        false,
    );
    assert!(drinnen.behg_gas_ct_per_kwh.is_ok());

    // The same 70 EUR/t under billingd's `rvo_bezugspreis` is admitted:
    // the answer moved with billingd's setting, not with productd's config.
    let rvo = BehgRegimeQuelle::billingd(
        &billingd_stub("rvo_bezugspreis").await,
        None,
        reqwest::Client::new(),
    )
    .regime()
    .await;
    assert_eq!(rvo, Ok(BehgRegime::RvoBezugspreis));
    assert!(
        pricing_context_fuer(
            stichtag,
            Some((date!(2027 - 02 - 10), dec!(70))),
            rvo,
            false
        )
        .behg_gas_ct_per_kwh
        .is_ok()
    );
}

/// No billingd, or one that does not answer: the Gas position of a 2027
/// quotation is refused `BEHG_PREIS_FEHLT`, never priced under a default.
#[tokio::test]
async fn no_answer_from_billingd_refuses_the_gas_position() {
    // Port 9 (discard) on loopback: nothing listens there in a test run.
    let unerreichbar =
        BehgRegimeQuelle::billingd("http://127.0.0.1:9", None, reqwest::Client::new())
            .regime()
            .await;
    for regime in [unerreichbar, BehgRegimeQuelle::keine().regime().await] {
        assert!(regime.is_err());
        let ctx = pricing_context_fuer(
            date!(2027 - 03 - 01),
            Some((date!(2027 - 02 - 10), dec!(60))),
            regime,
            false,
        );
        let err = compute_cost_breakdown(&gas_produkt(), &gas_position(), None, &ctx)
            .expect_err("no regime, no 2027 price");
        assert_eq!(err.grund, grund::BEHG_PREIS_FEHLT, "{}", err.detail);
        assert!(err.detail.contains("billingd"), "{}", err.detail);
    }
}

/// A year § 10 BEHG prices needs no regime, so billingd is not consulted and
/// its absence refuses nothing.
#[test]
fn a_festpreis_year_needs_no_regime() {
    let ctx = pricing_context_fuer(
        date!(2025 - 06 - 01),
        None,
        Err("billingd nicht gefragt".to_owned()),
        false,
    );
    assert_eq!(
        ctx.behg_gas_ct_per_kwh,
        Ok(energy_billing::behg_ct_per_kwh_for_year(2025).unwrap())
    );
}
