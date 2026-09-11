//! [`Json`] — the request-body extractor every mako handler uses.
//!
//! `axum::Json` renders its own rejections as **plain text** with no
//! `Content-Type: application/json`, which would make a malformed request body
//! the one error in mako that does not answer in the `{error, detail}` problem
//! shape [`ApiError`] guarantees — handing a client parsing the body a parse
//! error on top of the one it was told about.
//!
//! This extractor is `axum::Json` with the rejection mapped onto [`ApiError`]:
//!
//! | What went wrong | Status | `detail` |
//! |---|---|---|
//! | not JSON at all | 400 | `serde_json`'s syntax message |
//! | JSON, but not this type | 422 | the field and why |
//! | no/foreign `Content-Type` | 415 | what was expected |
//! | the body could not be read | 400 | the transport's message |
//!
//! # Structured detail from a `Deserialize` impl
//!
//! `serde`'s error type carries a string and nothing else, so a `Deserialize`
//! impl that knows *more* than a sentence — a JSON-path, a rule name, a stage —
//! has nowhere to put it. [`DETAIL_SENTINEL`] is the convention that gives it
//! one: an impl may append the sentinel and a JSON object to its message, and
//! this extractor lifts that object into the problem body's top level, exactly
//! as [`ApiError::unprocessable_with`] does for a handler that refuses by hand.
//!
//! `mako_markt::bo4e::Bo4e<T>` is the reason it exists — a BO4E payload
//! refused during deserialisation answers with the same `code` / `paths` /
//! `failures` keys as one refused inside a handler, so a caller never has to
//! know which of the two happened.

use axum::{
    extract::{FromRequest, OptionalFromRequest, Request, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// Separates a `serde` error's sentence from machine-readable detail.
///
/// A `Deserialize` impl that wants structured detail in the HTTP problem body
/// formats its message as `<sentence><DETAIL_SENTINEL><json object>`. Two
/// `U+0001` bytes make it unambiguous: the character is illegal unescaped in
/// JSON text and appears in no message any of these types produce, so finding
/// it cannot be a false positive.
///
/// **Duplicated, deliberately.** `mako-markt` is a library with no web
/// framework in it and this crate is the web layer; a shared constant would
/// mean one depending on the other. `cargo xtask check-detail-sentinel` asserts
/// the two literals agree.
pub const DETAIL_SENTINEL: &str = "\u{1}bo4e-rejection\u{1}";

/// JSON in and JSON out, with mako's problem shape when a body will not parse.
///
/// A drop-in for `axum::Json` in **both** directions, so a handler module
/// swaps one import rather than distinguishing the two uses. Extraction is
/// where the difference is; a response is `axum::Json`'s, unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match <axum::Json<T> as FromRequest<S>>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(map_rejection(&rejection)),
        }
    }
}

impl<T, S> OptionalFromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    /// `Option<Json<T>>` — an **absent** body, not a malformed one.
    ///
    /// A route whose body is genuinely optional (a `POST` that takes a note or
    /// nothing) needs this, and without it `Option<Json<T>>` does not compile
    /// against a custom extractor at all. The distinction it draws is the one
    /// that matters: no `Content-Type: application/json` means the caller sent
    /// no body and gets `None`; a body that is present and will not parse is
    /// still a refusal, with the same problem shape as everywhere else.
    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        match <axum::Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(Some(axum::Json(value))) => Ok(Some(Self(value))),
            Ok(None) => Ok(None),
            Err(rejection) => Err(map_rejection(&rejection)),
        }
    }
}

/// `axum`'s rejection as an [`ApiError`], keeping the status it chose.
///
/// The status comes from the rejection rather than a match arm of our own:
/// `axum` distinguishes "not JSON" (400) from "not this type" (422) from "no
/// content type" (415), and re-deriving that mapping here would let the two
/// drift. Only the *body* is ours.
fn map_rejection(rejection: &JsonRejection) -> ApiError {
    let text = rejection.body_text();
    // A `Deserialize` impl may have attached structured detail; if so it is the
    // most useful thing in the message and belongs at the top level of the body.
    if let Some((sentence, detail)) = recover_detail(&text) {
        return ApiError::unprocessable_with(sentence, detail);
    }
    match rejection.status() {
        StatusCode::UNPROCESSABLE_ENTITY => ApiError::unprocessable(text),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::UnsupportedMediaType(text),
        _ => ApiError::bad_request(text),
    }
}

/// Split a message on [`DETAIL_SENTINEL`] into its sentence and its detail
/// object, if it carries one.
///
/// The object is parsed as a **prefix** of what follows the sentinel, because
/// `serde_json` appends its own `" at line L column C"` context to a custom
/// message and that trailing text is not part of it.
///
/// The sentence comes out of the object's own `message` key, not from the text
/// in front of the sentinel. Two layers wrap a custom `serde` message in their
/// own prose before it reaches here — `serde_json` prefixes the field path
/// (`geschaeftspartner: `) and `axum` prefixes `"Failed to deserialize the JSON
/// body into the target type: "` — and recovering the sentence from that meant
/// guessing where their words ended and the impl's began. A key does not have
/// to be guessed at.
fn recover_detail(message: &str) -> Option<(String, serde_json::Value)> {
    let (_, rest) = message.split_once(DETAIL_SENTINEL)?;
    let mut detail = serde_json::Deserializer::from_str(rest)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?;
    let obj = detail.as_object_mut()?;
    let sentence = obj.remove("message")?.as_str()?.to_owned();
    Some((sentence, detail))
}

#[cfg(test)]
mod tests {
    use super::{DETAIL_SENTINEL, Json};
    use axum::{Router, body::Body, http::Request, routing::post};
    use serde::Deserialize;
    use tower::ServiceExt as _;

    #[derive(Deserialize)]
    #[allow(dead_code)] // the extractor's rejection is the subject, not the value
    struct Body1 {
        n: u8,
        #[serde(default)]
        gated: Option<Gated>,
    }

    /// Stands in for `mako_markt::bo4e::Bo4e<T>`: a `Deserialize` impl that
    /// refuses with structured detail behind the sentinel.
    struct Gated;

    impl<'de> Deserialize<'de> for Gated {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            use serde::de::Error as _;
            let _ = serde_json::Value::deserialize(d)?;
            Err(D::Error::custom(format!(
                "expected a BO4E GESCHAEFTSPARTNER, got _typ 'MARKTLOKATION'{DETAIL_SENTINEL}\
                 {{\"code\":\"bo4e.discriminator\",\
                 \"expected_typ\":\"GESCHAEFTSPARTNER\",\
                 \"message\":\"expected a BO4E GESCHAEFTSPARTNER, got _typ 'MARKTLOKATION'\"}}"
            )))
        }
    }

    async fn post_body(content_type: Option<&str>, body: &'static str) -> (u16, serde_json::Value) {
        let app = Router::new().route("/", post(|Json(_): Json<Body1>| async { "ok" }));
        let mut req = Request::builder().method("POST").uri("/");
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        let resp = app
            .oneshot(req.body(Body::from(body)).expect("request"))
            .await
            .expect("response");
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// Every failure answers in the problem shape — that is the whole point.
    /// `axum::Json` answers three of these four in `text/plain`.
    #[tokio::test]
    async fn every_rejection_is_a_json_problem_body() {
        for (ct, body, want) in [
            (Some("application/json"), "{ not json", 400_u16),
            (Some("application/json"), r#"{"n": 999}"#, 422),
            (None, r#"{"n":1}"#, 415),
        ] {
            let (status, json) = post_body(ct, body).await;
            assert_eq!(status, want, "body {body:?} → {json}");
            assert!(json.get("error").is_some(), "no problem body: {json}");
            assert!(json.get("detail").is_some(), "no detail: {json}");
        }
    }

    /// Structured detail attached by a `Deserialize` impl reaches the client as
    /// top-level keys, not as a sentence with a control character in it.
    #[tokio::test]
    async fn structured_detail_is_lifted_into_the_body() {
        let (status, json) = post_body(
            Some("application/json"),
            r#"{"n":1,"gated":{"_typ":"MARKTLOKATION"}}"#,
        )
        .await;
        assert_eq!(status, 422);
        assert_eq!(json["code"], "bo4e.discriminator");
        assert_eq!(json["expected_typ"], "GESCHAEFTSPARTNER");
        assert_eq!(
            json["detail"], "expected a BO4E GESCHAEFTSPARTNER, got _typ 'MARKTLOKATION'",
            "the sentence keeps its own wording and loses serde's field prefix"
        );
        assert!(
            !json.to_string().contains('\u{1}'),
            "the sentinel must never reach a client: {json}"
        );
    }

    /// The happy path is unchanged.
    #[tokio::test]
    async fn a_valid_body_extracts() {
        let (status, _) = post_body(Some("application/json"), r#"{"n":7}"#).await;
        assert_eq!(status, 200);
    }

    /// The same type answers on the way out, so a handler module swaps one
    /// import instead of keeping two `Json`s straight.
    #[tokio::test]
    async fn it_is_also_a_response() {
        let app = Router::new().route(
            "/",
            post(|| async { Json(serde_json::json!({ "ok": true })) }),
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
    }
}
