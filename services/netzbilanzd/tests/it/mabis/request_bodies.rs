//! The MaBiS write routes take typed bodies: an unknown key, or a value that
//! does not parse, is a 422 before anything is read or filed.
//!
//! `corrects_run_id` is the one that decides what kind of filing a request is.
//! Read as absent when it does not parse, a correction becomes a first filing —
//! refused as a duplicate when the period has a live run, and filed as a second
//! binding Summenzeitreihe when it has none.
//!
//! The router runs on a lazy pool with nothing behind it, so a request that got
//! past the body would fail on the database instead: a 422 here proves the body
//! alone decided it.

use netzbilanzd::mabis::server::{
    DatenstatusRequest, INBOUND_PRUEFMITTEILUNG_PIDS, PruefmitteilungRequest, SyncRequest,
};

async fn serve() -> String {
    let peer = crate::router_builds::peer().await;
    let app = mako_service::build_for_test::<netzbilanzd::Netzbilanzd>(
        crate::router_builds::mabis_config(&peer, "biko-bilateral"),
    )
    .await
    .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn post(base: &str, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/v1/mabis/{path}"))
        .json(&body)
        .send()
        .await
        .expect("the request is answered");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or_default())
}

fn version_ref() -> serde_json::Value {
    serde_json::json!({
        "bilanzierungsgebiet_id": "11YMAKO-TEST-01U",
        "period_from": "2026-05-01", "period_to": "2026-05-31",
        "version": "2026-06-15T05:00:00Z",
    })
}

fn with(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) else {
        panic!("both must be objects");
    };
    b.extend(e.clone());
    base
}

#[tokio::test]
async fn an_unparseable_corrects_run_id_is_refused_not_dropped() {
    let base = serve().await;
    let (status, body) = post(
        &base,
        "sync",
        serde_json::json!({
            "period_from": "2026-05-01", "period_to": "2026-05-31",
            "corrects_run_id": "not-a-uuid",
        }),
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body.to_string().contains("corrects_run_id"),
        "the refusal names the field: {body}"
    );
}

#[tokio::test]
async fn an_unparseable_period_or_as_of_is_refused() {
    let base = serve().await;
    for bad in [
        serde_json::json!({ "period_from": "01.05.2026" }),
        serde_json::json!({ "as_of": "yesterday" }),
    ] {
        let (status, body) = post(&base, "sync", bad.clone()).await;
        assert_eq!(status, 422, "{bad} → {body}");
    }
}

#[tokio::test]
async fn an_unknown_key_is_refused_on_every_mabis_write() {
    let base = serve().await;
    for (path, body) in [
        (
            "sync",
            serde_json::json!({ "corrects_run": "550e8400-e29b-41d4-a716-446655440000" }),
        ),
        (
            "datenstatus",
            with(
                version_ref(),
                serde_json::json!({ "datenstatus": "Abrechnungsdaten", "pid": 21003 }),
            ),
        ),
        (
            "pruefmitteilung",
            with(
                version_ref(),
                serde_json::json!({
                    "positiv": false, "sender_mp_id": "9900077000006", "pid": 21000,
                    "begruendung": "x", "reason": "x",
                }),
            ),
        ),
    ] {
        let (status, json) = post(&base, path, body).await;
        assert_eq!(status, 422, "{path} → {json}");
    }
}

#[tokio::test]
async fn a_pruefmitteilung_names_its_sender_and_an_inbound_pid() {
    let base = serve().await;
    for (case, extra) in [
        (
            "no sender",
            serde_json::json!({ "positiv": false, "pid": 21000 }),
        ),
        (
            "no pid",
            serde_json::json!({ "positiv": false, "sender_mp_id": "9900077000006" }),
        ),
        (
            "empty sender",
            serde_json::json!({ "positiv": false, "sender_mp_id": " ", "pid": 21000 }),
        ),
        (
            "21005 is the NB's own, to the BIKO",
            serde_json::json!({ "positiv": false, "sender_mp_id": "9900077000006", "pid": 21005 }),
        ),
    ] {
        let (status, json) = post(&base, "pruefmitteilung", with(version_ref(), extra)).await;
        assert_eq!(status, 422, "{case} → {json}");
    }
}

/// The inbound set is the published Prüfmitteilung PIDs less the one the NB sends.
#[test]
fn the_inbound_pruefmitteilung_pids_are_published_ones() {
    for pid in INBOUND_PRUEFMITTEILUNG_PIDS {
        let pid = u32::try_from(pid).expect("positive");
        assert!(
            mako_mabis::bilanzkreisabrechnung::IFTSTA_PRUEFMITTEILUNG_PIDS.contains(&pid),
            "{pid}"
        );
    }
}

/// The operator guide's MaBiS examples parse into the typed bodies.
#[test]
fn the_documented_mabis_bodies_parse() {
    let _: SyncRequest = serde_json::from_value(serde_json::json!({
        "period_from": "2026-05-01", "period_to": "2026-05-31",
    }))
    .unwrap();
    let correction: SyncRequest = serde_json::from_value(serde_json::json!({
        "period_from": "2026-05-01", "period_to": "2026-05-31",
        "corrects_run_id": "550e8400-e29b-41d4-a716-446655440000",
    }))
    .unwrap();
    assert!(correction.corrects_run_id.is_some());
    let _: DatenstatusRequest = serde_json::from_value(with(
        version_ref(),
        serde_json::json!({ "datenstatus": "Abrechnungsdaten" }),
    ))
    .unwrap();
    let pm: PruefmitteilungRequest = serde_json::from_value(with(
        version_ref(),
        serde_json::json!({
            "positiv": false, "sender_mp_id": "9900077000006", "pid": 21000,
            "begruendung": "Abweichung MaLo 51238696012",
        }),
    ))
    .unwrap();
    assert!(INBOUND_PRUEFMITTEILUNG_PIDS.contains(&pm.pid));
}
