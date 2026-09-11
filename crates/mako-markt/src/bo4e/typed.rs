//! [`Bo4e<T>`] — a BO4E document that cannot exist without having crossed the
//! gate.
//!
//! [`super::decode`] is one function call, and the defect it keeps
//! producing is that somebody does not make it. A request struct declares
//!
//! ```ignore
//! /// Full BO4E `Vertrag` payload.
//! pub vertrag: serde_json::Value,
//! ```
//!
//! and the document is stored exactly as it arrived: wrong `_typ`, an enum the
//! schema does not define, arbitrary nesting depth, no rules. Nothing in the
//! type system separates that field from one whose handler decodes it — the
//! difference is a line somewhere else.
//!
//! `Bo4e<T>` moves the decision into the type. The field becomes
//!
//! ```ignore
//! pub vertrag: Bo4e<Vertrag>,
//! ```
//!
//! and `serde` runs the gate while it deserialises the request. There is no
//! constructor that skips it: [`Deserialize`] is the only way in from untrusted
//! JSON, and it is [`super::decode`].
//!
//! # What the handler then does
//!
//! - `&*field` / `field.get()` — the typed BO, for reading.
//! - [`canonical_json`](Bo4e::canonical_json) — what to **store**. The gate's
//!   round-trip, not the request body: camelCase, the right `_typ`, unknown
//!   keys preserved in `_additional`.
//! - [`into_inner`](Bo4e::into_inner) — the typed BO, owned.
//!
//! # The rejection survives serde
//!
//! `serde`'s error type carries a string, so a [`Bo4eRejection`]'s stage, its
//! JSON-paths and its rule failures would be flattened to a sentence on the way
//! out — and the whole point of the gate's `code`/`paths`/`failures` keys is
//! that a caller does not have to read prose. The `Display` sentence is
//! therefore followed by the machine-readable half, tagged with
//! [`REJECTION_SENTINEL`]; `mako_service::Json` finds it and re-emits a `422`
//! with the same body every hand-written gate call site produces.
//!
//! A caller using plain `axum::Json` still gets the sentence, so nothing
//! *depends* on the recovery — it upgrades the answer, it does not carry it.

use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use super::conformance::Bo4eConformance;
use super::gate::{Bo4eRejection, Bo4eSerialiseError, Bo4eTyped, decode};

/// Marks the machine-readable half of a [`Bo4eRejection`] inside a `serde`
/// error message.
///
/// Deliberately not valid JSON and not a word that occurs in a BO4E document,
/// so finding it in an error string cannot be a false positive.
pub const REJECTION_SENTINEL: &str = "\u{1}bo4e-rejection\u{1}";

/// A BO4E document that has crossed [`super::decode`].
///
/// Deserialises through the gate — see the [module docs](self). Serialises as
/// the document itself, so a struct carrying one round-trips unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct Bo4e<T>(T);

impl<T> Bo4e<T> {
    /// The typed BO, borrowed.
    pub const fn get(&self) -> &T {
        &self.0
    }

    /// The typed BO, owned.
    #[allow(clippy::missing_const_for_fn)] // `T` may need dropping.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Wrap a value mako **built**, without re-running the inbound gate.
    ///
    /// The gate answers "is this untrusted JSON the BO it claims"; a value
    /// constructed in Rust is that by the compiler's account. Use this to hand
    /// an internally-assembled document to something typed as `Bo4e<T>`; use
    /// [`ensure_conformant`](super::ensure_conformant) before **emitting** it.
    pub const fn from_built(value: T) -> Self {
        Self(value)
    }
}

impl<T> Bo4e<T>
where
    T: Bo4eTyped + Serialize,
{
    /// The canonical BO4E JSON — what a caller should store.
    ///
    /// Not the request body: the gate injects an absent `_typ`, normalises
    /// every enum to its wire spelling and preserves unknown keys under
    /// `_additional`, and it is that round-trip a stored row must carry so a
    /// shadow column cannot disagree with the document it indexes.
    ///
    /// # Errors
    ///
    /// [`Bo4eSerialiseError`] — unreachable for the generated BO4E types, and
    /// stated rather than swallowed so that a write cannot happen if it ever
    /// becomes reachable. See [`to_canonical_json`](super::to_canonical_json).
    pub fn canonical_json(&self) -> Result<serde_json::Value, Bo4eSerialiseError> {
        super::to_canonical_json(&self.0)
    }
}

impl<T> Deref for Bo4e<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> AsRef<T> for Bo4e<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: Serialize> Serialize for Bo4e<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T> Deserialize<'de> for Bo4e<T>
where
    T: serde::de::DeserializeOwned + Bo4eTyped + rubo4e::Bo4eStrict + Bo4eConformance,
    T: rubo4e::prelude::Validate<Context = ()> + rubo4e::json::Bo4eJsonExt,
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Buffer to `Value` first: the gate needs the whole document (it
        // injects `_typ`, then walks every field by JSON-path), and a
        // streaming visitor cannot see the end of an object before the
        // beginning has been consumed.
        let raw = serde_json::Value::deserialize(d)?;
        decode::<T>(raw)
            .map(Self)
            .map_err(|e| D::Error::custom(encode_rejection(&e)))
    }
}

/// The rejection as one JSON object behind [`REJECTION_SENTINEL`].
///
/// The sentence travels **inside** the object, under `message`, rather than in
/// front of the sentinel. `serde_json` and `axum` each wrap a custom message in
/// their own prose — `"data: …"`, `"Failed to deserialize the JSON body into
/// the target type: …"` — and recovering the sentence meant guessing where
/// theirs ended and ours began. A key does not have to be guessed at.
fn encode_rejection(e: &Bo4eRejection) -> String {
    let mut detail = e.detail();
    detail.insert("message".into(), e.to_string().into());
    format!(
        "{e}{REJECTION_SENTINEL}{}",
        serde_json::Value::Object(detail)
    )
}

/// Recover the machine-readable half of a [`Bo4eRejection`] from a `serde`
/// error message, if there is one.
///
/// `serde_json` wraps a custom message in its own context — a field path in
/// front, `" at line L column C"` behind — so the object is found by its
/// sentinel and parsed as a **prefix** of what follows, letting the trailing
/// context be ignored rather than having to be stripped.
///
/// Returns the rejection's own sentence and the detail keys, with `message`
/// removed from the latter so a caller renders it once.
#[must_use]
pub fn recover_rejection(message: &str) -> Option<(String, serde_json::Value)> {
    let (_, rest) = message.split_once(REJECTION_SENTINEL)?;
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
    use super::{Bo4e, recover_rejection};
    use rubo4e::current::{Geschaeftspartner, Marktlokation};

    #[derive(Debug, serde::Deserialize)]
    struct Req {
        geschaeftspartner: Bo4e<Geschaeftspartner>,
    }

    /// The happy path: a request struct carrying a `Bo4e<T>` deserialises, and
    /// what it yields is the gate's canonical round-trip, not the request body.
    #[test]
    fn a_field_deserialises_through_the_gate_and_canonicalises() {
        let req: Req = serde_json::from_value(serde_json::json!({
            "geschaeftspartner": { "organisationsname": "Demo Energie GmbH" }
        }))
        .expect("a valid Geschaeftspartner");
        assert_eq!(
            req.geschaeftspartner.organisationsname.as_deref(),
            Some("Demo Energie GmbH")
        );
        let stored = req
            .geschaeftspartner
            .canonical_json()
            .expect("serialisable");
        // `_typ` was absent in the request and is present in what gets stored —
        // the round-trip is the point, and it is what a BO4E reader needs.
        assert_eq!(stored["_typ"], "GESCHAEFTSPARTNER");
    }

    /// A `_typ` naming another BO is refused *by the field's type*, with no
    /// handler code involved.
    #[test]
    fn a_wrong_discriminator_is_refused_during_deserialisation() {
        let err = serde_json::from_value::<Req>(serde_json::json!({
            "geschaeftspartner": { "_typ": "MARKTLOKATION" }
        }))
        .expect_err("a Marktlokation is not a Geschaeftspartner");
        let (sentence, detail) =
            recover_rejection(&err.to_string()).expect("the rejection survives serde");
        assert_eq!(detail["code"], "bo4e.discriminator");
        assert_eq!(detail["expected_typ"], "GESCHAEFTSPARTNER");
        assert_eq!(detail["found_typ"], "MARKTLOKATION");
        assert!(
            sentence.starts_with("expected a BO4E GESCHAEFTSPARTNER"),
            "the sentence is the rejection's own, without serde's field prefix: {sentence}"
        );
    }

    /// Stage 3 through the same door: an out-of-schema enum value names its
    /// JSON-path rather than being silently normalised to `"UNKNOWN"`.
    #[test]
    fn an_out_of_schema_enum_names_its_path() {
        let err = serde_json::from_value::<Bo4e<Marktlokation>>(serde_json::json!({
            "marktlokationsId": "51238696781", "sparte": "STROMM"
        }))
        .expect_err("STROMM is not a Sparte");
        let (_, detail) = recover_rejection(&err.to_string()).expect("a recoverable rejection");
        assert_eq!(detail["code"], "bo4e.unknown_enum");
        assert_eq!(detail["paths"][0], "sparte");
    }

    /// A message that is not the gate's is left alone — the recovery must not
    /// invent a BO4E rejection out of an ordinary serde error.
    #[test]
    fn an_unrelated_serde_error_is_not_recovered() {
        let err = serde_json::from_value::<Req>(serde_json::json!({})).expect_err("field missing");
        assert!(recover_rejection(&err.to_string()).is_none());
    }

    /// `from_built` exists for values mako assembles; it round-trips as the
    /// document itself, so a struct holding one serialises unchanged.
    #[test]
    fn a_built_value_serialises_as_the_document() {
        let gp = Geschaeftspartner {
            organisationsname: Some("Demo".into()),
            ..Default::default()
        };
        let wrapped = Bo4e::from_built(gp.clone());
        assert_eq!(
            serde_json::to_value(&wrapped).expect("serialisable"),
            serde_json::to_value(&gp).expect("serialisable")
        );
    }
}
