//! A portal identity carries exactly one restriction, `standort_filter`.
//!
//! An identity has no role: a `rolle` key in the upsert body is a setting that
//! would narrow nothing, so the body refuses it as an unknown field instead of
//! storing a value an operator could mistake for an access control.
//!
//! The request runs on a lazy pool with nothing behind it, so a request that
//! got past the body would fail on the database instead: a `422` proves the
//! body alone decided it.

use uuid::Uuid;

fn demo_config() -> vertragd::config::VertragdConfig {
    toml::from_str(include_str!("../../../../demos/o2c/vertragd.toml"))
        .expect("the demo configuration parses")
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_identity_upsert_carrying_rolle_is_refused_as_an_unknown_field() {
    let app = mako_service::build_for_test::<vertragd::Vertragd>(demo_config())
        .await
        .expect("the router builds");
    let base = serve(app).await;
    let id = Uuid::new_v4();

    let resp = reqwest::Client::new()
        .post(format!("{base}/api/v1/kunden/{id}/identitaeten"))
        .json(&serde_json::json!({
            "oidc_sub": "auth0|buchhaltung",
            "standort_filter": "Werk Nord",
            "rolle": "FINANZEN",
        }))
        .send()
        .await
        .expect("the request is answered");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();

    // The body type denies unknown fields itself, so serde refuses the key
    // before `mako_service::Json`'s own path report runs; either way it is a
    // 422 naming `rolle`.
    assert_eq!(status, 422, "{body}");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("unknown field `rolle`"),
        "the refusal names the key: {body}"
    );
}
