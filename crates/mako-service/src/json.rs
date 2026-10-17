//! [`Json`] — the request-body extractor every mako handler uses.
//!
//! `axum::Json` renders its own rejections as **plain text** with no
//! `Content-Type: application/json`, which would make a malformed request body
//! the one error in mako that does not answer in the `{error, detail}` problem
//! shape [`ApiError`] guarantees — handing a client parsing the body a parse
//! error on top of the one it was told about.
//!
//! This extractor is `axum::Json` with the rejection mapped onto [`ApiError`],
//! and with one refusal `serde` does not make on its own:
//!
//! | What went wrong | Status | `detail` |
//! |---|---|---|
//! | not JSON at all | 400 | `serde_json`'s syntax message |
//! | JSON, but not this type | 422 | the field and why |
//! | a key the type does not declare, at any depth | 422 | the key's path; `code: "unknown_field"`, `paths: [...]` |
//! | no/foreign `Content-Type` | 415 | what was expected |
//! | the body could not be read | 400 | the transport's message |
//!
//! # Unknown keys
//!
//! `serde` skips a key no field declares, so a misspelt setting succeeds and
//! does nothing. [`Json`] deserialises through `serde_ignored`, which reports
//! every key `serde` skips, and refuses the body naming each one by its JSON
//! path (`kunde.vornme`, `intervals.3.quality`) — whatever attributes the type
//! carries.
//!
//! What `serde` buffers before it decides — a `#[serde(flatten)]` field, an
//! `#[serde(untagged)]` or internally tagged enum — is read from that buffer,
//! out of `serde_ignored`'s sight, so a key left over there is **not**
//! reported. Those shapes keep `#[serde(deny_unknown_fields)]` on their
//! members (or, for `flatten`, refuse in the type itself).
//!
//! [`Lenient`] is the same extractor without the refusal, for a body whose
//! schema a published external specification defines and evolves — a
//! counterparty one revision ahead must be read, not refused.
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
use serde::{Deserialize, Deserializer, de::DeserializeOwned};

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
        let Lenient(Strict(value)) =
            <Lenient<Strict<T>> as FromRequest<S>>::from_request(req, state).await?;
        Ok(Self(value))
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
        let body = <Lenient<Strict<T>> as OptionalFromRequest<S>>::from_request(req, state).await?;
        Ok(body.map(|Lenient(Strict(value))| Self(value)))
    }
}

/// [`Json`] without the unknown-key refusal: `serde`'s own reading of `T`.
///
/// Only for a body whose schema a published external specification defines —
/// a newer revision adds keys, and refusing the document over one would be
/// this service failing an interoperability requirement it does not get to
/// redefine. Every other body is a [`Json`]. The rejection mapping is the same:
/// a malformed body is still refused in the problem shape.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lenient<T>(pub T);

impl<T, S> FromRequest<S> for Lenient<T>
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

impl<T, S> OptionalFromRequest<S> for Lenient<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    /// `Option<Lenient<T>>` — absent without a JSON `Content-Type`, refused when
    /// present and malformed; the same distinction as `Option<Json<T>>`.
    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        match <axum::Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(Some(axum::Json(value))) => Ok(Some(Self(value))),
            Ok(None) => Ok(None),
            Err(rejection) => Err(map_rejection(&rejection)),
        }
    }
}

/// A `serde_ignored` path as the caller wrote it: map keys and array indices
/// joined by `.`. `serde_ignored`'s own `Display` also prints a `?` for every
/// `Option` it passed through, which names a Rust type, not a JSON position.
fn json_path(path: &serde_ignored::Path<'_>) -> String {
    use serde_ignored::Path;
    let mut segments = Vec::new();
    let mut at = path;
    loop {
        at = match at {
            Path::Root => break,
            Path::Seq { parent, index } => {
                segments.push(index.to_string());
                parent
            }
            Path::Map { parent, key } => {
                segments.push(key.clone());
                parent
            }
            Path::Some { parent }
            | Path::NewtypeStruct { parent }
            | Path::NewtypeVariant { parent } => parent,
        };
    }
    segments.reverse();
    segments.join(".")
}

/// `T`, refused if the document holds a key `T` does not declare.
///
/// The refusal is a `serde` error carrying [`DETAIL_SENTINEL`] detail, so it
/// leaves through the same 422 path as every other data error and
/// [`map_rejection`] lifts `code` and `paths` into the problem body.
struct Strict<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Strict<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let mut unknown: Vec<String> = Vec::new();
        let value = serde_ignored::deserialize(deserializer, |path| {
            unknown.push(json_path(&path));
        })?;
        if unknown.is_empty() {
            return Ok(Self(value));
        }
        let named = unknown
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let plural = if unknown.len() == 1 { "" } else { "s" };
        let detail = serde_json::json!({
            "code": "unknown_field",
            "paths": unknown,
            "message": format!("unknown field{plural} {named}"),
        });
        Err(D::Error::custom(format!(
            "unknown field{plural} {named}{DETAIL_SENTINEL}{detail}"
        )))
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
    use super::{DETAIL_SENTINEL, Json, Lenient};
    use axum::{Router, body::Body, http::Request, routing::post};
    use serde::Deserialize;
    use tower::ServiceExt as _;

    #[derive(Deserialize)]
    #[expect(
        dead_code,
        reason = "the extractor's rejection is the subject, not the value"
    )]
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

    /// No `deny_unknown_fields` anywhere: the refusal is the extractor's.
    #[derive(Deserialize)]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    struct Kunde {
        vorname: String,
        #[serde(default)]
        adresse: Option<Adresse>,
        #[serde(default)]
        intervalle: Vec<Adresse>,
        #[serde(default)]
        frei: Option<serde_json::Value>,
    }

    #[derive(Deserialize)]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    struct Adresse {
        ort: String,
    }

    /// `outputd`'s shape: a body that flattens a member.
    #[derive(Deserialize)]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    struct Flattened {
        name: String,
        #[serde(flatten)]
        inner: Adresse,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    enum Untagged {
        A { ort: String },
        B { n: u8 },
    }

    #[derive(Deserialize)]
    #[serde(tag = "art")]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    enum Tagged {
        A { ort: String },
    }

    #[derive(Deserialize)]
    #[serde(tag = "art", deny_unknown_fields)]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    enum TaggedDenying {
        A { ort: String },
    }

    #[derive(Deserialize)]
    #[serde(untagged, deny_unknown_fields)]
    #[expect(dead_code, reason = "the extractor's refusal is the subject")]
    enum UntaggedDenying {
        A { ort: String },
    }

    async fn post_to(app: Router, body: &'static str) -> (u16, serde_json::Value) {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
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

    fn strict<T: serde::de::DeserializeOwned + Send + 'static>() -> Router {
        Router::new().route("/", post(|Json(_): Json<T>| async { "ok" }))
    }

    /// A misspelt key is refused with its path, though the type carries no
    /// `deny_unknown_fields` — at the top level, one level in, and inside a
    /// sequence.
    #[tokio::test]
    async fn an_undeclared_key_is_refused_with_its_path() {
        for (body, path) in [
            (r#"{"vorname":"Erika","vornme":"x"}"#, "vornme"),
            (
                r#"{"vorname":"Erika","adresse":{"ort":"Berlin","plz":"10115"}}"#,
                "adresse.plz",
            ),
            (
                r#"{"vorname":"Erika","intervalle":[{"ort":"a"},{"ort":"b","qualitaet":1}]}"#,
                "intervalle.1.qualitaet",
            ),
        ] {
            let (status, json) = post_to(strict::<Kunde>(), body).await;
            assert_eq!(status, 422, "{body} → {json}");
            assert_eq!(json["code"], "unknown_field", "{json}");
            assert_eq!(json["paths"], serde_json::json!([path]), "{json}");
            assert_eq!(json["detail"], format!("unknown field `{path}`"), "{json}");
        }
    }

    #[tokio::test]
    async fn every_undeclared_key_is_named() {
        let (status, json) = post_to(strict::<Kunde>(), r#"{"vorname":"E","a":1,"b":2}"#).await;
        assert_eq!(status, 422);
        assert_eq!(json["paths"], serde_json::json!(["a", "b"]), "{json}");
    }

    /// A `serde_json::Value` field takes any document by design; what is in
    /// it is the BO4E gate's business, not this refusal's.
    #[tokio::test]
    async fn a_value_field_accepts_any_keys() {
        let (status, json) = post_to(
            strict::<Kunde>(),
            r#"{"vorname":"Erika","frei":{"anything":{"at":"all"}}}"#,
        )
        .await;
        assert_eq!(status, 200, "{json}");
    }

    #[tokio::test]
    async fn an_optional_body_is_refused_the_same_way() {
        let app = Router::new().route(
            "/",
            post(|body: Option<Json<Kunde>>| async move {
                if body.is_some() { "some" } else { "none" }
            }),
        );
        let (status, json) = post_to(app, r#"{"vorname":"Erika","vornme":"x"}"#).await;
        assert_eq!(status, 422, "{json}");
        assert_eq!(json["paths"], serde_json::json!(["vornme"]), "{json}");
    }

    #[tokio::test]
    async fn lenient_reads_what_json_refuses() {
        let app = Router::new().route("/", post(|Lenient(_): Lenient<Kunde>| async { "ok" }));
        let (status, _) = post_to(app.clone(), r#"{"vorname":"Erika","vornme":"x"}"#).await;
        assert_eq!(status, 200);
        let (status, json) = post_to(app, r#"{"vorname":7}"#).await;
        assert_eq!(status, 422, "a malformed body is still refused: {json}");
        assert!(json.get("detail").is_some(), "{json}");
    }

    /// The limit of the runtime check, measured: `serde` reads a `flatten`
    /// member, an `untagged` enum and an internally tagged enum out of a
    /// buffer it filled itself, and a key left over there never reaches
    /// `serde_ignored`. These shapes stay `deny_unknown_fields`'s business.
    #[tokio::test]
    async fn flatten_and_untagged_leftovers_are_not_seen() {
        let (status, json) =
            post_to(strict::<Flattened>(), r#"{"name":"n","ort":"o","extra":1}"#).await;
        assert_eq!(status, 200, "flatten: {json}");
        let (status, json) = post_to(strict::<Untagged>(), r#"{"ort":"o","extra":1}"#).await;
        assert_eq!(status, 200, "untagged: {json}");
        let (status, json) =
            post_to(strict::<Tagged>(), r#"{"art":"A","ort":"o","extra":1}"#).await;
        assert_eq!(status, 200, "internally tagged: {json}");
    }

    /// What closes the gap the runtime check leaves: `deny_unknown_fields` on
    /// the buffered shape itself — the rule `check-request-bodies` holds those
    /// shapes to.
    #[tokio::test]
    async fn deny_on_the_buffered_shape_refuses_the_leftover() {
        let (status, json) = post_to(
            strict::<TaggedDenying>(),
            r#"{"art":"A","ort":"o","extra":1}"#,
        )
        .await;
        assert_eq!(status, 422, "internally tagged: {json}");
        let (status, json) = post_to(strict::<UntaggedDenying>(), r#"{"ort":"o","extra":1}"#).await;
        assert_eq!(status, 422, "untagged: {json}");
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
