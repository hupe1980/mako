//! Every writer of makod's MaLo cache and every reader of it use one key: the
//! engine tenant, the data-isolation key the command doors scope by. A MaLo
//! seeded by `PUT /admin/malo` or delivered by marktd's fan-out must be found
//! by the door that sends the Anmeldung; a writer keyed by anything else
//! leaves a seeded MaLo answering `malo_not_found` — the door tells the
//! operator to seed it, and seeding changes nothing.

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    routing::{get, put},
};
use http_body_util::BodyExt as _;
use mako_engine::ids::TenantId;
use mako_engine::store_slatedb::SlateDbStore;
use makod::cedar_authz::{CedarAuthorizer, DefaultPolicy, NamedKey};
use makod::commands_api::CommandsApiState;
use makod::config::PartyConfig;
use makod::malo_admin_api::MaloAdminState;
use makod::malo_cache::{MaloIdentResultCache, SlateDbMaloCache};
use makod::malo_sync::MaloSync;
use makod::party_registry::MpIdRegistry;
use secrecy::SecretString;
use serde_json::{Value, json};
use tower::ServiceExt as _;

const LF: &str = "9900357000004";
const NB: &str = "9900001000001";
const MALO: &str = "51238696012";
const KEY: &str = "0123456789abcdef";

fn tenant() -> TenantId {
    TenantId::from_party_id(LF)
}

/// The command doors of an LF Strom deployment over `cache`.
fn tueren(store: &SlateDbStore, cache: &SlateDbMaloCache) -> Router {
    makod::commands_api::router(Arc::new(CommandsApiState {
        tenant_id: tenant(),
        sender_party_id: LF.to_owned(),
        configured_marktrollen: vec!["LF".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(cache.clone()),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store.clone()),
        marktd_client: None,
    }))
}

/// Rehearse the Desk's sample Anmeldung; the door's answer.
async fn anmeldung_proben(tueren: Router) -> Value {
    let body = json!({
        "marktrolle": "LF",
        "payload": {
            "malo_id": MALO,
            "lieferbeginn_datum": "2027-01-01",
            "transaktionsgrund": "E03",
            "bilanzkreis": "11XBK-LF-------9",
            "haushaltskunde": true,
            "enfg_grundlage": "ZG1",
            "kunde": {
                "vorname": "Erika",
                "nachname": "Mustermann",
                "anschrift": {
                    "strasse": "Hauptstraße", "hausnummer": "1",
                    "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
                }
            }
        }
    });
    let res = tueren
        .oneshot(
            Request::post("/api/v1/probe/gpke.lieferbeginn.anmelden")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("answer");
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}

fn assert_gefunden(antwort: &Value) {
    assert!(
        !antwort.to_string().contains("malo_not_found"),
        "the door does not find the seeded MaLo: {antwort:#}"
    );
}

#[tokio::test]
async fn a_malo_seeded_by_put_admin_malo_is_found_by_the_command_door() {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let cache = SlateDbMaloCache::new(store.clone(), tenant());
    let admin = makod::malo_admin_api::router(Arc::new(MaloAdminState {
        cache: cache.clone(),
        cedar: Arc::new(
            CedarAuthorizer::new(
                vec![NamedKey::from_arg(&format!("ops={KEY}")).expect("key")],
                None,
                None,
                Some(LF.to_owned()),
                DefaultPolicy::PermitAll,
            )
            .expect("cedar"),
        ),
        tenant_id: LF.to_owned(),
    }));
    let seed = json!({
        "source": "test",
        "result": { "dataMarketLocation": {
            "maloId": MALO,
            "energyDirection": "consumption",
            "measurementTechnologyClassification": "conventionalMeasuringSystem",
            "optionalChangeForecastBasis": "notPossible",
            "dataMarketLocationProperties": [],
            "dataMarketLocationNetworkOperators": [
                { "marketPartnerId": NB, "executionTimeFrom": "2000-01-01T00:00:00Z" }
            ],
            "dataMarketLocationTransmissionSystemOperators": []
        }}
    });
    let res = admin
        .oneshot(
            Request::put(format!("/admin/malo/{MALO}"))
                .header("authorization", format!("Bearer {KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(seed.to_string()))
                .expect("request"),
        )
        .await
        .expect("answer");
    assert!(res.status().is_success(), "seed refused: {}", res.status());

    assert_gefunden(&anmeldung_proben(tueren(&store, &cache)).await);
}

type Shared = Arc<Mutex<Option<Value>>>;

async fn spawn_marktd(record: Shared) -> String {
    async fn one(State(m): State<Shared>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
        match m.lock().expect("lock").clone() {
            Some(r) if r["malo_id"] == id => (StatusCode::OK, Json(r)),
            _ => (StatusCode::NOT_FOUND, Json(json!({}))),
        }
    }
    async fn all(State(m): State<Shared>) -> Json<Value> {
        let items: Vec<Value> = m.lock().expect("lock").iter().cloned().collect();
        Json(json!({ "items": items, "total": items.len(), "page": 0, "size": 500 }))
    }
    async fn sub(Path(_id): Path<String>) -> Json<Value> {
        Json(json!({ "version": 1 }))
    }
    let app = Router::new()
        .route("/api/v1/malos", get(all))
        .route("/api/v1/malos/{id}", get(one))
        .route("/api/v1/subscriptions/{id}", put(sub))
        .with_state(record);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("marktd") });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_malo_delivered_by_the_marktd_fan_out_is_found_by_the_command_door() {
    let record: Shared = Arc::new(Mutex::new(Some(json!({
        "malo_id": MALO,
        "sparte": "STROM",
        "version": 1,
        "energierichtung": "AUSSP",
        "data": {},
        "rollenzuordnung": [
            { "zuordnungstyp": "NB", "rollencodenummer": NB, "valid_from": "2020-01-01" }
        ]
    }))));
    let url = spawn_marktd(record).await;
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let cache = SlateDbMaloCache::new(store.clone(), tenant());
    let registry = Arc::new(
        MpIdRegistry::from_config(&[PartyConfig {
            mp_id: LF.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("registry"),
    );
    let sync = MaloSync::new(
        reqwest::Client::new(),
        url,
        SecretString::from("marktd-key"),
        "http://makod:8080/webhooks/marktd",
        None,
        cache.clone(),
        registry,
    );
    sync.backfill().await.expect("backfill");

    assert_gefunden(&anmeldung_proben(tueren(&store, &cache)).await);
}
