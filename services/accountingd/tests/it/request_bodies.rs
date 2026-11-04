//! Request bodies are typed: the five write endpoints that read a JSON body
//! read it as a declared type through `mako_service::Json`.
//!
//! A body extracted as `serde_json::Value` has no fields, so the extractor's
//! unknown-key refusal cannot refuse anything and a misspelt key
//! (`abschlag_cent`, `due_day`) is read as absent. Each test below drives the
//! real extractor over the handler's own body type: an unknown key and a missing
//! mandatory field are refused with `422`, a valid body is accepted. The last
//! test pins the handler signatures to those types, so a body that goes back to
//! `serde_json::Value` fails here rather than only in `cargo xtask
//! check-request-bodies`.

use accountingd::handlers::{
    EscalateDunningRequest, PutAbschlagRequest, PutBusinessPartnerRequest,
};
use axum::{
    body::Body,
    extract::FromRequest as _,
    http::{Request, StatusCode, header},
    response::IntoResponse as _,
};
use mako_markt::bo4e::Bo4e;
use mako_service::Json;
use serde::de::DeserializeOwned;

const HANDLERS: &str = include_str!("../../src/handlers.rs");

/// Run `mako_service::Json<T>` over `body`; `Ok(T)` or the refusal's status.
async fn extract<T: DeserializeOwned>(body: serde_json::Value) -> Result<T, StatusCode> {
    let req = Request::builder()
        .method("PUT")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    match Json::<T>::from_request(req, &()).await {
        Ok(Json(v)) => Ok(v),
        Err(rejection) => Err(rejection.into_response().status()),
    }
}

// ── PUT /api/v1/accounts/{malo_id}/abschlag ─────────────────────────────────

#[tokio::test]
async fn abschlag_body_is_accepted() {
    let b: PutAbschlagRequest = extract(serde_json::json!({ "abschlag_ct": 7500 }))
        .await
        .expect("valid body");
    assert_eq!(b.abschlag_ct, 7500);
}

#[tokio::test]
async fn abschlag_body_refuses_an_unknown_key() {
    let r = extract::<PutAbschlagRequest>(
        serde_json::json!({ "abschlag_ct": 7500, "abschlag_cent": 7500 }),
    )
    .await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

#[tokio::test]
async fn abschlag_body_refuses_a_missing_amount() {
    let r = extract::<PutAbschlagRequest>(serde_json::json!({})).await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

#[tokio::test]
async fn abschlag_body_refuses_a_fractional_amount() {
    // ct are integers; a float is not silently truncated to one.
    let r = extract::<PutAbschlagRequest>(serde_json::json!({ "abschlag_ct": 75.5 })).await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

// ── PUT /api/v1/accounts/{malo_id}/business-partner ─────────────────────────

#[tokio::test]
async fn business_partner_body_is_accepted() {
    let b: PutBusinessPartnerRequest = extract(serde_json::json!({ "kunden_nr": "K-1001" }))
        .await
        .expect("valid body");
    assert_eq!(b.kunden_nr, "K-1001");
}

#[tokio::test]
async fn business_partner_body_refuses_an_unknown_key() {
    let r = extract::<PutBusinessPartnerRequest>(serde_json::json!({ "kundennr": "K-1001" })).await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

#[tokio::test]
async fn business_partner_body_refuses_a_missing_kunden_nr() {
    let r = extract::<PutBusinessPartnerRequest>(serde_json::json!({})).await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

// ── POST /api/v1/dunning/{account_id}/escalate ──────────────────────────────

#[tokio::test]
async fn escalate_body_is_accepted() {
    let b: EscalateDunningRequest = extract(serde_json::json!({ "stufe": 2, "due_days": 14 }))
        .await
        .expect("valid body");
    assert_eq!((b.stufe, b.due_days), (2, 14));
}

#[tokio::test]
async fn escalate_body_refuses_an_unknown_key() {
    let r = extract::<EscalateDunningRequest>(
        serde_json::json!({ "stufe": 2, "due_days": 14, "due_day": 7 }),
    )
    .await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

/// The Mahnstufe and the Zahlungsfrist decide what the Mahnung tells the
/// customer; neither is substituted when the operator leaves it out.
#[tokio::test]
async fn escalate_body_refuses_a_missing_stufe_or_due_days() {
    for body in [
        serde_json::json!({ "due_days": 14 }),
        serde_json::json!({ "stufe": 1 }),
        serde_json::json!({}),
    ] {
        let r = extract::<EscalateDunningRequest>(body.clone()).await;
        assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY), "{body}");
    }
}

#[tokio::test]
async fn escalate_body_refuses_a_negative_due_period() {
    let r =
        extract::<EscalateDunningRequest>(serde_json::json!({ "stufe": 1, "due_days": -3 })).await;
    assert_eq!(r.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

// ── PUT …/vorauszahlung and …/zahlungsinformation (BO4E) ─────────────────────

#[tokio::test]
async fn vorauszahlung_body_runs_the_bo4e_gate() {
    let ok = extract::<Bo4e<rubo4e::current::Vorauszahlung>>(serde_json::json!({
        "_typ": "VORAUSZAHLUNG",
        "betrag": { "_typ": "BETRAG", "wert": "75.00", "waehrung": "EUR" }
    }))
    .await
    .expect("valid Vorauszahlung");
    assert!(ok.betrag.is_some());

    let wrong_typ = extract::<Bo4e<rubo4e::current::Vorauszahlung>>(
        serde_json::json!({ "_typ": "ZAHLUNGSINFORMATION" }),
    )
    .await;
    assert_eq!(wrong_typ.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

#[tokio::test]
async fn zahlungsinformation_body_runs_the_bo4e_gate() {
    let ok = extract::<Bo4e<rubo4e::current::Zahlungsinformation>>(serde_json::json!({
        "_typ": "ZAHLUNGSINFORMATION",
        "iban": "DE89370400440532013000"
    }))
    .await
    .expect("valid Zahlungsinformation");
    assert_eq!(ok.iban.as_deref(), Some("DE89370400440532013000"));

    // The strict-enum stage: a `zahlungsart` the schema does not define.
    let bad_enum = extract::<Bo4e<rubo4e::current::Zahlungsinformation>>(serde_json::json!({
        "_typ": "ZAHLUNGSINFORMATION",
        "zahlungsart": "BARGELD_PER_BRIEFTAUBE"
    }))
    .await;
    assert_eq!(bad_enum.err(), Some(StatusCode::UNPROCESSABLE_ENTITY));
}

// ── The handlers read these types ───────────────────────────────────────────

/// The parameter list of `pub async fn <name>(…)` in `handlers.rs`.
fn signature(name: &str) -> &'static str {
    let start = HANDLERS
        .find(&format!("\npub async fn {name}("))
        .unwrap_or_else(|| panic!("handler `{name}` not found"));
    let rest = &HANDLERS[start..];
    let end = rest.find(") ->").expect("signature end");
    &rest[..end]
}

#[test]
fn every_written_body_is_read_as_its_declared_type() {
    for (handler, body) in [
        ("put_abschlag", "Json<PutAbschlagRequest>"),
        (
            "put_account_business_partner",
            "Json<PutBusinessPartnerRequest>",
        ),
        ("escalate_dunning", "Json<EscalateDunningRequest>"),
        (
            "put_vorauszahlung",
            "Json<Bo4e<rubo4e::current::Vorauszahlung>>",
        ),
        (
            "put_zahlungsinformation",
            "Json<Bo4e<rubo4e::current::Zahlungsinformation>>",
        ),
    ] {
        let sig = signature(handler);
        assert!(
            sig.contains(body),
            "`{handler}` must read its body as `{body}` — an untyped body cannot refuse \
             an unknown key: {sig}"
        );
        assert!(
            !sig.contains("Json<serde_json::Value>"),
            "`{handler}` reads an untyped body: {sig}"
        );
    }
    // The gate keeps a key BO4E does not define; an operator's document is
    // refused for one instead, so a misspelt `iban` is not stored as absent.
    for handler in ["put_vorauszahlung", "put_zahlungsinformation"] {
        let start = HANDLERS
            .find(&format!("\npub async fn {handler}("))
            .expect("handler");
        let body = &HANDLERS[start..];
        let body = &body[..body[1..]
            .find("\npub async fn ")
            .map_or(body.len(), |e| e + 1)];
        assert!(
            body.contains("ensure_no_undefined_fields()"),
            "`{handler}` must refuse a key BO4E does not define"
        );
    }
}

#[test]
fn escalate_refuses_a_stufe_outside_one_to_three_and_a_zero_due_period() {
    let today = time::macros::date!(2026 - 11 - 04);
    let r = |stufe, due_days| EscalateDunningRequest { stufe, due_days }.resolve(today);
    assert_eq!(r(3, 7).ok(), Some((3, time::macros::date!(2026 - 11 - 11))));
    for (stufe, due_days) in [(0, 14), (4, 14), (-1, 14), (1, 0)] {
        let e = r(stufe, due_days).expect_err("must be refused");
        assert_eq!(
            axum::response::IntoResponse::into_response(e).status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "stufe {stufe}, due_days {due_days}"
        );
    }
}
