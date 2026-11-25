//! Supply-period reads select the same issuer, component and customer.
use super::{MALO, make_kunde, pg, pg_container, vertrag_input};
use axum::{Router, body::Body, http::Request};
use serde_json::Value;
use sqlx::PgPool;
use time::macros::date;
use tower::ServiceExt as _;

async fn get(app: &Router, path: &str, issuer: &str, from: &str, to: &str) -> (u16, Value) {
    let uri = format!("/api/v1/{path}?lf_mp_id={issuer}&from={from}&to={to}");
    let response = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_billing_period_pins_price_module_and_recipient_to_one_supply() {
    let (url, _container) = pg_container().await.expect("PostgreSQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let mut cfg: vertragd::config::VertragdConfig =
        toml::from_str(include_str!("../../../../../deploy/demo/vertragd.toml")).unwrap();
    cfg.database.url.clone_from(&url);
    let app = mako_service::build_for_test_on::<vertragd::Vertragd>(cfg, pool.clone())
        .await
        .unwrap();
    let tenant = mako_service::oidc::demo::TENANT;
    let lf_a = "9900357000028";
    let lf_b = "9910000000002";
    let october = date!(2026 - 10 - 01);
    let november = date!(2026 - 11 - 01);
    for (code, lf, start, end, module) in [
        (
            "ALPHA",
            lf_a,
            october,
            Some(date!(2026 - 10 - 31)),
            pg::Sect14aModul::Modul1,
        ),
        ("SUCCESSOR", lf_a, november, None, pg::Sect14aModul::Modul2),
        ("BETA", lf_b, october, None, pg::Sect14aModul::Modul2),
    ] {
        let customer = make_kunde(&pool, tenant).await;
        sqlx::query("UPDATE kunden SET geschaeftspartner = jsonb_set(geschaeftspartner, '{nachname}', to_jsonb($2::text)) WHERE id = $1")
            .bind(customer).bind(code).execute(&pool).await.unwrap();
        let mut input = vertrag_input(code);
        input.vertragsbeginn = start;
        input.komponenten[0].lieferbeginn = start;
        input.komponenten[0].lieferende = end;
        input.komponenten[0].product_code = code.to_owned();
        input.komponenten[0].sect14a = Some(pg::sect14a::Sect14aIntake {
            module: vec![module],
        });
        let contract = pg::insert_versorgungsvertrag(&pool, customer, tenant, lf, &input)
            .await
            .unwrap();
        if end.is_some() {
            sqlx::query("UPDATE vertragskomponenten SET status = 'BEENDET' WHERE id = $1")
                .bind(contract.komponenten[0].id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE versorgungsvertraege SET status = 'ABGELAUFEN' WHERE id = $1")
                .bind(contract.id)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    let facts = format!("vertraege/by-malo/{MALO}");
    let prices = format!("malo/{MALO}/produkte");
    let modules = format!("malo/{MALO}/sect14a");
    for (lf, from, to, name, module) in [
        (lf_a, "2026-10-01", "2026-10-31", "ALPHA", "MODUL_1"),
        (lf_a, "2026-11-01", "2026-11-30", "SUCCESSOR", "MODUL_2"),
        (lf_b, "2026-10-01", "2026-10-31", "BETA", "MODUL_2"),
    ] {
        let (status, contract) = get(&app, &facts, lf, from, to).await;
        assert_eq!(status, 200, "{contract}");
        assert_eq!(
            contract["rechnungsempfaenger"]["name"],
            format!("Erika {name}")
        );
        assert_eq!(contract["komponente"]["lf_mp_id"], lf);
        let (status, products) = get(&app, &prices, lf, from, to).await;
        assert_eq!(status, 200, "{products}");
        assert_eq!(products["slices"].as_array().unwrap().len(), 1);
        assert_eq!(products["slices"][0]["product_code"], name);
        let (status, rates) = get(&app, &modules, lf, from, to).await;
        assert_eq!(status, 200, "{rates}");
        assert_eq!(rates["slices"].as_array().unwrap().len(), 1);
        assert_eq!(rates["slices"][0]["module"][0], module);
    }
    for path in [&facts, &prices, &modules] {
        let (status, refusal) = get(&app, path, lf_a, "2026-10-31", "2026-11-01").await;
        assert_eq!(status, 409, "{refusal}");
        assert_eq!(refusal["code"], "ambiguous_supply_period");
        assert_eq!(
            get(&app, path, "9900357000003", "2026-10-01", "2026-10-31")
                .await
                .0,
            404
        );
        assert_eq!(
            get(&app, path, lf_a, "2026-09-30", "2026-10-01").await.0,
            404
        );
    }
    let change = pg::Sect14aInput {
        module: vec![pg::Sect14aModul::Modul2],
        gueltig_ab: date!(2026 - 10 - 15),
    };
    assert!(matches!(
        pg::record_sect14a(&pool, tenant, MALO, &change).await,
        Err(pg::Sect14aError::MehrdeutigeKomponente)
    ));
    pg::record_sect14a_for_issuer(&pool, tenant, MALO, Some(lf_a), &change)
        .await
        .unwrap();
    assert_eq!(
        get(&app, &modules, lf_a, "2026-10-01", "2026-10-31")
            .await
            .1["slices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        get(&app, &modules, lf_b, "2026-10-01", "2026-10-31")
            .await
            .1["slices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_price_change_notice_names_its_own_contract_holder() {
    let Some((pool, _pg)) = super::test_pool("issuer_notice").await else {
        return;
    };
    let tenant = "stadtwerke-musterstadt";
    let customer = make_kunde(&pool, tenant).await;
    let original = pg::insert_versorgungsvertrag(
        &pool,
        customer,
        tenant,
        "9900357000028",
        &vertrag_input("ORIGINAL"),
    )
    .await
    .unwrap();
    let other = make_kunde(&pool, tenant).await;
    sqlx::query("UPDATE kunden SET geschaeftspartner = jsonb_set(geschaeftspartner, '{nachname}', '\"Other\"'::jsonb) WHERE id = $1")
        .bind(other).execute(&pool).await.unwrap();
    let mut later = vertrag_input("OTHER");
    later.vertragsbeginn = later.vertragsbeginn.next_day().unwrap();
    later.komponenten[0].lieferbeginn = later.vertragsbeginn;
    pg::insert_versorgungsvertrag(&pool, other, tenant, "9910000000002", &later)
        .await
        .unwrap();
    super::schedule_slice(
        &pool,
        tenant,
        original.komponenten[0].id,
        mako_fristen::heute() + time::Duration::days(90),
        pg::Initiator::Lieferant,
        Some(super::umfang()),
    )
    .await;

    let (captured, mut received) = tokio::sync::mpsc::channel(1);
    let app = Router::new().route(
        "/api/v1/documents/issue/PREISANPASSUNG",
        axum::routing::post(move |axum::Json(request): axum::Json<Value>| async move {
            captured.send(request).await.unwrap();
            axum::Json(
                serde_json::json!({"document_id":uuid::Uuid::new_v4(),"template_hash":"template"}),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut cfg = super::worker_config(tenant);
    cfg.outputd_url = Some(format!("http://{}", listener.local_addr().unwrap()));
    cfg.absender = Some(serde_json::from_value(serde_json::json!({"name":"Testwerk"})).unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    vertragd::workers::preisanpassung(&pool, &cfg)
        .await
        .unwrap();
    let document = received
        .try_recv()
        .expect("a price-change notice was issued");
    assert_eq!(document["recipient"]["name"], "Erika Mustermann");
    server.abort();
}
