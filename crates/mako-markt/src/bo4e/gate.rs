//! The one gate every BO4E payload crosses, in or out.
//!
//! Accepting a BO4E document is four decisions. [`decode`] is all four, once,
//! for every endpoint:
//!
//! | Stage | Refuses | [`Bo4eRejection`] |
//! |---|---|---|
//! | 1. Discriminator | a `Zaehler` posted to the `Geraet` endpoint | [`Discriminator`](Bo4eRejection::Discriminator) |
//! | 2. Schema | a value the type cannot hold | [`Schema`](Bo4eRejection::Schema) |
//! | 3. Strict enums | `"sparte": "STROMM"`, at any depth | [`UnknownEnum`](Bo4eRejection::UnknownEnum) |
//! | 4. BO4E rules | a gross that is not net plus tax | [`Rule`](Bo4eRejection::Rule) |
//!
//! Stage 3 is the one that most needs to be unmissable. BO4E enums all carry an
//! `Unknown` forward-compatibility catch-all, so a typo decodes rather than
//! failing — and `Unknown` **serialises back as the string `"UNKNOWN"`**. An
//! endpoint that canonicalises what it stores therefore does not merely accept
//! the typo, it overwrites what the caller sent.
//!
//! # Outbound
//!
//! [`ensure_conformant`] runs stages 3 and 4 on a value mako has *built* — the
//! first two are the compiler's job there — plus a check that every field name
//! is one BO4E defines, which belongs on this side only. Every document mako
//! emits crosses it, so mako never ships one it would refuse to accept.
//!
//! # What this module is not
//!
//! It is not the endpoint's requirements. BO4E makes every field optional; an
//! endpoint that needs a `sparte` says so itself, in a mako profile, after the
//! gate has run. Keeping the two apart is what lets the gate be identical
//! everywhere.

use rubo4e::Bo4eStrict;
use serde::de::DeserializeOwned;

use super::conformance::Bo4eConformance;
use rubo4e::validation::{ValidationFailure, report_errors};

/// The `_typ` discriminator, read off the type.
///
/// `rubo4e`'s own trait, re-exported. It spans Geschäftsobjekte **and**
/// components, so the gate takes one bound over "anything BO4E stamps a `_typ`
/// on" and reads `T::TYP_WIRE` without constructing a value — which is what
/// admits the types the schema marks `required`, since those derive no
/// `Default`.
pub use rubo4e::Bo4eTyped;

/// Why a BO4E payload was refused.
///
/// The variants are the gate's stages, so a caller can tell a malformed request
/// from an out-of-schema value from a document the standard forbids without
/// parsing prose. [`code`](Bo4eRejection::code) is the stable machine key;
/// `Display` is the human sentence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Bo4eRejection {
    /// The payload names a different BO than the endpoint accepts.
    #[error("expected a BO4E {expected}, got _typ '{found}'")]
    Discriminator {
        /// The `_typ` this endpoint accepts.
        expected: &'static str,
        /// The `_typ` the payload carried.
        found: String,
    },
    /// The payload does not deserialize as the type.
    #[error("not a valid BO4E {typ}: {detail}")]
    Schema {
        /// The type that was expected.
        typ: &'static str,
        /// The deserializer's own message.
        detail: String,
    },
    /// One or more enum fields hold a value this schema version does not define.
    #[error(
        "{typ} carries {} out-of-schema enum value(s) at: {}",
        paths.len(),
        paths.join(", ")
    )]
    UnknownEnum {
        /// The type that was decoded.
        typ: &'static str,
        /// Dotted JSON-paths of the offending fields.
        paths: Vec<String>,
    },
    /// The payload breaks one or more rules BO4E states.
    ///
    /// Both `rubo4e`'s derived validators (which descend the whole tree) and
    /// mako's two residual rules report into this one variant, in `rubo4e`'s own
    /// [`ValidationFailure`] shape — so a caller reads one list and does not
    /// have to know which side found what.
    #[error(
        "{typ} breaks {} BO4E rule(s): {}",
        failures.len(),
        failures.iter().map(|f| format!("{}: {}", f.path, f.message))
            .collect::<Vec<_>>().join("; ")
    )]
    Rule {
        /// The type that was decoded.
        typ: &'static str,
        /// Every rule the document broke, each at its JSON-path.
        failures: Vec<ValidationFailure>,
    },
    /// The document carries fields this BO4E schema version does not define.
    ///
    /// **Outbound only.** Refusing an unknown field on a *received* document
    /// would throw away the forward compatibility `_additional` exists for — a
    /// counterparty one BO4E release ahead is to be read, not rejected. On a
    /// document mako authored the same field can only be a mistake, and the
    /// mistake it usually is is a misspelling that a decode round-trip cannot
    /// see: serde ignores a key no field declares, `rubo4e` files it under
    /// `_additional`, and the field it was meant to fill stays `None`.
    #[error(
        "{typ} carries {} field(s) BO4E does not define at: {}",
        paths.len(),
        paths.join(", ")
    )]
    UnknownField {
        /// The type that was checked.
        typ: &'static str,
        /// JSON-paths of every field that landed in extension data.
        paths: Vec<String>,
    },
}

impl Bo4eRejection {
    /// A stable key for the refusing stage, for a caller matching on the cause
    /// rather than reading the sentence.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Discriminator { .. } => "bo4e.discriminator",
            Self::Schema { .. } => "bo4e.schema",
            Self::UnknownEnum { .. } => "bo4e.unknown_enum",
            Self::Rule { .. } => "bo4e.rule",
            Self::UnknownField { .. } => "bo4e.unknown_field",
        }
    }

    /// The rejection as the body of a `422`.
    ///
    /// `error` is the sentence and the rest is [`detail`](Bo4eRejection::detail).
    /// Handlers that build their own response body render this verbatim; those
    /// on [`ApiError`] pass `detail()` to `unprocessable_with`. Either way every
    /// BO4E endpoint in mako refuses with the same keys.
    ///
    /// [`ApiError`]: https://docs.rs/mako-service
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut body = self.detail();
        body.insert("error".into(), self.to_string().into());
        serde_json::Value::Object(body)
    }

    /// The machine-readable half of the rejection: the stage that refused, and
    /// whatever it can point at.
    ///
    /// `code` is always present. A discriminator mismatch adds `expected_typ`
    /// and `found_typ`; an out-of-schema enum adds `paths`; a rule violation
    /// adds `failures`, one `{path, message}` per broken rule.
    #[must_use]
    pub fn detail(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut obj = serde_json::Map::new();
        obj.insert("code".into(), self.code().into());
        match self {
            Self::Discriminator { expected, found } => {
                obj.insert("expected_typ".into(), (*expected).into());
                obj.insert("found_typ".into(), found.clone().into());
            }
            Self::Schema { .. } => {}
            Self::UnknownEnum { paths, .. } | Self::UnknownField { paths, .. } => {
                obj.insert("paths".into(), paths.clone().into());
            }
            Self::Rule { failures, .. } => {
                obj.insert(
                    "failures".into(),
                    failures
                        .iter()
                        .map(|f| serde_json::json!({ "path": f.path, "message": f.message }))
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
        }
        obj
    }
}

/// Decode an untrusted BO4E payload into its typed form, or refuse it.
///
/// Runs all four stages in order. `_typ` is **injected when absent** — the
/// endpoint already fixes which BO it takes, so requiring the caller to repeat
/// it adds a way to be wrong and no information — and refused when present and
/// different.
///
/// The returned value is what should be stored: serialising it yields the
/// canonical BO4E form, and unknown keys survive in the extension map, so the
/// round-trip loses nothing.
///
/// # Errors
///
/// [`Bo4eRejection`], naming the stage that refused and what it saw.
pub fn decode<T>(data: serde_json::Value) -> Result<T, Bo4eRejection>
where
    T: DeserializeOwned + Bo4eTyped + Bo4eStrict + Bo4eConformance,
    T: rubo4e::prelude::Validate<Context = ()> + rubo4e::json::Bo4eJsonExt,
{
    let typed: T = decode_structural(data)?;
    let failures = rule_failures(&typed);
    if failures.is_empty() {
        Ok(typed)
    } else {
        Err(Bo4eRejection::Rule {
            typ: T::TYP_WIRE,
            failures,
        })
    }
}

/// Stage 4: every BO4E-stated rule the value breaks, from both sources.
///
/// `rubo4e`'s derived validators run first and cover the tree — since 0.11 the
/// generator emits `garde(dive)` for every field carrying rules, so one call
/// reaches a `Zeitraum` on a position or a `Kostenposition` two levels down, and
/// reports each at its path. mako's two residual rules
/// ([`Bo4eConformance`]) are appended in the same shape.
///
/// `Validate::validate` takes `&self`, so nothing is cloned.
/// `Validated::new` would consume the value, and this has to hand the value
/// back on success and the whole failure list on error.
fn rule_failures<T>(value: &T) -> Vec<ValidationFailure>
where
    T: rubo4e::prelude::Validate<Context = ()> + Bo4eConformance,
{
    let mut failures = match value.validate() {
        Ok(()) => Vec::new(),
        Err(report) => report_errors(&report),
    };
    failures.extend(value.residual_rules());
    failures
}

/// Decode a market document a counterparty sent, separating what makes it
/// unreadable from what makes it wrong.
///
/// The rule stage does not belong on this path. A `Rechnung` whose
/// `gesamtbrutto` is not net plus tax is a **disputable** invoice, not an
/// unreadable one: the market's answer to it is a REMADV naming the defect, and
/// refusing to parse the document takes that answer away — mako would fall
/// silent where the process requires it to speak, and an operator would have to
/// look at a dead letter to find out why.
///
/// So stages 1–3 still refuse (a document that will not type has nothing to
/// adjudicate), and the stage-4 violation is **returned alongside** the value
/// for the caller to fold into whatever it answers with.
///
/// # Errors
///
/// [`Bo4eRejection`] from stages 1–3 only.
pub fn decode_received<T>(
    data: serde_json::Value,
) -> Result<(T, Vec<ValidationFailure>), Bo4eRejection>
where
    T: DeserializeOwned + Bo4eTyped + Bo4eStrict + Bo4eConformance,
    T: rubo4e::prelude::Validate<Context = ()> + rubo4e::json::Bo4eJsonExt,
{
    let typed: T = decode_structural(data)?;
    let failures = rule_failures(&typed);
    Ok((typed, failures))
}

/// Stages 1–3: the payload is the BO it claims, types, and holds no
/// out-of-schema enum. Shared by [`decode`] and [`decode_received`].
///
/// `_typ` is injected when absent rather than demanded. That covers the nested
/// case too — a COM read out of the extension map of the BO that carried it,
/// where the key already named it and producers do not reliably stamp `_typ`.
/// A *wrong* `_typ` is still refused: nothing downstream catches one, because
/// `ensure_known_enums` walks the value's fields and never reaches `typ`, so
/// `{"_typ": "MARKTLOKATION"}` on a COM would decode to `typ: Some(Unknown)`
/// and serialise back out as `"UNKNOWN"`.
fn decode_structural<T>(data: serde_json::Value) -> Result<T, Bo4eRejection>
where
    T: DeserializeOwned + Bo4eTyped + Bo4eStrict + rubo4e::json::Bo4eJsonExt,
{
    let expected = T::TYP_WIRE;
    let data = match data {
        serde_json::Value::Object(mut obj) => {
            match obj.get("_typ").and_then(serde_json::Value::as_str) {
                None => {
                    obj.insert("_typ".into(), expected.into());
                }
                Some(found) if !found.eq_ignore_ascii_case(expected) => {
                    return Err(Bo4eRejection::Discriminator {
                        expected,
                        found: found.to_owned(),
                    });
                }
                Some(_) => {}
            }
            serde_json::Value::Object(obj)
        }
        other => other,
    };
    // `T::from_json_value`, not `serde_json::from_value`: the payload is
    // untrusted, and rubo4e's own entry point enforces `DEFAULT_MAX_NESTING_DEPTH`
    // where plain serde enforces nothing. The depth guard is the crate's
    // documented protection and the gate was the one place bypassing it.
    //
    // **Not** `from_json_value_hardened` with `max_extension_field_count(0)`.
    // That would refuse a field this schema version does not define, which on an
    // *inbound* document is the forward compatibility `_additional` exists for —
    // a counterparty one BO4E release ahead is to be read, not rejected. The
    // field check belongs on the outbound path; see `ensure_conformant`.
    let typed: T = T::from_json_value(data).map_err(|e| Bo4eRejection::Schema {
        typ: expected,
        detail: e.to_string(),
    })?;
    Bo4eStrict::ensure_known_enums(&typed).map_err(|e| Bo4eRejection::UnknownEnum {
        typ: expected,
        paths: e.paths,
    })?;
    Ok(typed)
}

/// A BO4E value mako built could not be serialised.
///
/// Carries the type's own `_typ`, read off [`Bo4eTyped`] rather than passed in,
/// so the message cannot name a different BO than the one that failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the validated BO4E {typ} is not serialisable: {detail}")]
pub struct Bo4eSerialiseError {
    /// The `_typ` of the value that failed.
    pub typ: &'static str,
    /// `serde_json`'s own message.
    pub detail: String,
}

/// Serialise a BO4E value mako built into its canonical JSON form, or fail.
///
/// # Why this exists rather than `serde_json::to_value(&bo)` with a fallback
///
/// The obvious spellings are both wrong, and neither fails a build:
///
/// - **`.unwrap_or_default()`** yields `Value::Null`. `PostgreSQL` accepts the
///   JSON literal `null` into a `JSONB NOT NULL` column — SQL `NULL` and JSON
///   `null` are different things — so a validated document is replaced by
///   `null` and the write *succeeds*. In a feed served to consumers it is a
///   null entry where a tariff should be, counted in the total like a real one.
/// - **`.unwrap_or(original.clone())`** is worse: it stores the caller's own
///   unvalidated input under a line that claims to canonicalise it, silently
///   undoing every stage the gate just ran.
///
/// Failure is not reachable for the generated BO4E types — no non-string map
/// keys, no non-finite floats. That is exactly why it should be *stated*: if it
/// ever becomes reachable, the write must not happen.
///
/// # Errors
///
/// [`Bo4eSerialiseError`], naming the type and `serde_json`'s message.
pub fn to_canonical_json<T>(value: &T) -> Result<serde_json::Value, Bo4eSerialiseError>
where
    T: Bo4eTyped + serde::Serialize,
{
    serde_json::to_value(value).map_err(|e| Bo4eSerialiseError {
        typ: T::TYP_WIRE,
        detail: e.to_string(),
    })
}

/// Run the value-level stages on an already-typed BO4E value.
///
/// This is the outbound gate: a document mako built is type-correct by
/// construction, but nothing stops it carrying an enum some other code path set
/// to `Unknown`, a field name no BO4E schema declares, or totals that do not add
/// up. Every emission path runs it, so mako never sends a document it would
/// refuse to receive.
///
/// # The field check is outbound-only, and deliberately
///
/// [`ensure_known_enums`](Bo4eStrict::ensure_known_enums) finds out-of-schema
/// **values**; [`ensure_no_extension_data`](rubo4e::json::Bo4eExtensions::ensure_no_extension_data)
/// finds out-of-schema **fields**. Neither sees the other's finding, and only
/// the first belongs on an inbound path: refusing an unknown *field* from a
/// counterparty throws away the forward compatibility `_additional` exists for,
/// and a sender one BO4E release ahead is to be read rather than rejected.
///
/// On a document mako authored the calculation inverts. An undefined field can
/// only be a mistake there, and the mistake it usually is — a misspelled or
/// renamed key — is invisible to every other check: serde ignores a key no field
/// declares, `rubo4e` files it under `_additional`, and the field it was meant
/// to fill reads back `None`. mako's own extensions are unaffected, because they
/// ride in `zusatzAttribute`, which *is* a BO4E field (`mako:<snake_case>`, one
/// registry, `xtask check-bo4e-attributes`).
///
/// # Errors
///
/// [`Bo4eRejection::UnknownEnum`], [`Bo4eRejection::UnknownField`] or
/// [`Bo4eRejection::Rule`].
pub fn ensure_conformant<T>(value: &T) -> Result<(), Bo4eRejection>
where
    T: Bo4eTyped + Bo4eStrict + Bo4eConformance + rubo4e::json::Bo4eExtensions,
    T: rubo4e::prelude::Validate<Context = ()>,
{
    let typ = T::TYP_WIRE;
    Bo4eStrict::ensure_known_enums(value).map_err(|e| Bo4eRejection::UnknownEnum {
        typ,
        paths: e.paths,
    })?;
    value
        .ensure_no_extension_data()
        .map_err(|e| Bo4eRejection::UnknownField {
            typ,
            paths: e.paths,
        })?;
    // Outbound also runs the emission rules: BO4E does not state them, and mako
    // controls this document. See `Bo4eConformance::emission_rules`.
    let mut failures = rule_failures(value);
    failures.extend(value.emission_rules());
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Bo4eRejection::Rule { typ, failures })
    }
}

#[cfg(test)]
mod tests {
    use super::{Bo4eRejection, Bo4eTyped as _, decode, ensure_conformant};
    use rubo4e::current::{Betrag, Marktlokation, Rechnung, Waehrungscode, Zaehler};
    use rust_decimal::dec;

    /// The discriminator comes from the type, not from a literal that can rot.
    #[test]
    fn the_discriminator_is_the_types_own() {
        assert_eq!(Marktlokation::TYP_WIRE, "MARKTLOKATION");
        assert_eq!(Zaehler::TYP_WIRE, "ZAEHLER");
        assert_eq!(rubo4e::current::Energiemix::TYP_WIRE, "ENERGIEMIX");
    }

    #[test]
    fn an_absent_typ_is_injected() {
        let malo: Marktlokation =
            decode(serde_json::json!({ "marktlokationsId": "51238696781" })).expect("valid");
        assert_eq!(malo.typ, Some(rubo4e::current::BoTyp::Marktlokation));
    }

    #[test]
    fn a_lowercase_typ_is_accepted() {
        assert!(decode::<Marktlokation>(serde_json::json!({ "_typ": "marktlokation" })).is_ok());
    }

    #[test]
    fn the_wrong_bo_is_refused_before_it_is_parsed() {
        let err = decode::<Marktlokation>(serde_json::json!({ "_typ": "ZAEHLER" }))
            .expect_err("a Zaehler is not a Marktlokation");
        assert_eq!(err.code(), "bo4e.discriminator");
        assert_eq!(err.to_json()["found_typ"], "ZAEHLER");
    }

    /// A typo decodes to `Unknown` rather than failing, and `Unknown`
    /// serialises back as `"UNKNOWN"` — so an endpoint that canonicalises what
    /// it stores would overwrite the value without this stage.
    #[test]
    fn an_out_of_schema_enum_is_refused_with_its_path() {
        let err = decode::<Marktlokation>(serde_json::json!({ "sparte": "STROMM" }))
            .expect_err("STROMM is not a Sparte");
        assert_eq!(err.code(), "bo4e.unknown_enum");
        assert_eq!(err.to_json()["paths"], serde_json::json!(["sparte"]));
    }

    /// The gate decodes through `rubo4e`'s own entry point, which enforces the
    /// nesting-depth cap. Plain `serde_json::from_value` enforces nothing, and
    /// the gate — the one place in mako that reads untrusted BO4E — was
    /// bypassing the crate's documented protection.
    #[test]
    fn a_pathologically_nested_payload_is_refused() {
        // Far past `DEFAULT_MAX_NESTING_DEPTH` (128). Built under a key
        // `Marktlokation` does not declare, so nothing but the depth guard can
        // refuse it: extension data is preserved by design.
        let mut deep = serde_json::json!(null);
        for _ in 0..600 {
            deep = serde_json::json!([deep]);
        }
        let err = decode::<Marktlokation>(serde_json::json!({ "tief": deep }))
            .expect_err("600 levels is past the cap");
        assert_eq!(err.code(), "bo4e.schema");
    }

    /// …and an ordinarily-nested document still decodes, so the guard is a cap
    /// rather than a ban on nesting.
    #[test]
    fn an_ordinary_payload_is_not_caught_by_the_depth_cap() {
        assert!(
            decode::<Marktlokation>(serde_json::json!({
                "marktlokationsId": "51238696781",
                "lokationsadresse": { "strasse": "Musterstraße", "hausnummer": "1" }
            }))
            .is_ok()
        );
    }

    #[test]
    fn a_value_the_type_cannot_hold_is_a_schema_error() {
        let err = decode::<Marktlokation>(serde_json::json!({ "zaehlwerke": 7 }))
            .expect_err("a number is not a list of Zaehlwerk");
        assert_eq!(err.code(), "bo4e.schema");
    }

    /// A rule `rubo4e`'s own validators own: net plus tax is gross.
    #[test]
    fn a_bo4e_rule_violation_is_reported_with_its_path() {
        let payload = serde_json::json!({
            "gesamtnetto":  { "wert": "300.00", "waehrung": "EUR" },
            "gesamtsteuer": { "wert": "57.00",  "waehrung": "EUR" },
            "gesamtbrutto": { "wert": "358.00", "waehrung": "EUR" },
        });
        let err = decode::<Rechnung>(payload).expect_err("357 != 358");
        assert_eq!(err.code(), "bo4e.rule");
        let body = err.to_json();
        let failures = body["failures"].as_array().expect("failures list");
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0]["message"]
                .as_str()
                .is_some_and(|m| m.contains("gesamtbrutto")),
            "{failures:?}"
        );
    }

    /// A rule mako still owns, because `rubo4e` does not check it: the
    /// positions must sum to `gesamtnetto`.
    #[test]
    fn a_residual_mako_rule_reports_in_the_same_shape() {
        let payload = serde_json::json!({
            "gesamtnetto":         { "wert": "300.00", "waehrung": "EUR" },
            "rechnungspositionen": [{ "gesamtpreis": { "wert": "299.00", "waehrung": "EUR" } }],
        });
        let err = decode::<Rechnung>(payload).expect_err("299 != 300");
        assert_eq!(err.code(), "bo4e.rule");
        let failures = err.to_json();
        let failures = failures["failures"].as_array().expect("failures list");
        assert_eq!(failures[0]["path"], "gesamtnetto");
    }

    /// Both sides report into one list, so a caller reads a single answer.
    #[test]
    fn rubo4e_and_mako_failures_arrive_together() {
        let payload = serde_json::json!({
            "gesamtnetto":         { "wert": "300.00", "waehrung": "EUR" },
            "gesamtsteuer":        { "wert": "57.00",  "waehrung": "EUR" },
            "gesamtbrutto":        { "wert": "358.00", "waehrung": "EUR" },
            "istStorno":           true,
            "rechnungspositionen": [{ "gesamtpreis": { "wert": "299.00", "waehrung": "EUR" } }],
        });
        let err = decode::<Rechnung>(payload).expect_err("three rules broken");
        let body = err.to_json();
        let failures = body["failures"].as_array().expect("failures list");
        assert_eq!(
            failures.len(),
            3,
            "rubo4e's gesamtbrutto plus mako's gesamtnetto and storno: {failures:?}"
        );
    }

    /// A decimal spelled as a JSON string keeps its scale; the gate accepts both
    /// spellings because BO4E producers disagree on which to use.
    #[test]
    fn both_decimal_spellings_decode() {
        let as_string = decode::<Rechnung>(serde_json::json!({
            "gesamtbrutto": { "wert": "119.00" }
        }))
        .expect("string spelling");
        let as_number = decode::<Rechnung>(serde_json::json!({
            "gesamtbrutto": { "wert": 119.00 }
        }))
        .expect("number spelling");
        let wert = |r: &Rechnung| r.gesamtbrutto.as_ref().and_then(|b| b.wert);
        assert_eq!(wert(&as_string), wert(&as_number));
    }

    /// A nested COM may omit its `_typ` — the key it was filed under already
    /// named it, and producers do not reliably stamp `_typ` on one. The gate
    /// injects it, so what comes back out carries the discriminant every other
    /// BO4E implementation writes.
    #[test]
    fn a_nested_value_may_omit_its_typ() {
        use rubo4e::current::ZeitvariablePreisposition;
        let zvp: ZeitvariablePreisposition =
            decode(serde_json::json!({ "zaehlzeitregister": "HT" }))
                .expect("a nested COM need not stamp _typ");
        assert_eq!(zvp.zaehlzeitregister.as_deref(), Some("HT"));
        assert_eq!(
            zvp.typ,
            Some(rubo4e::current::ComTyp::ZeitvariablePreisposition),
            "the gate injects the discriminant rather than leaving the COM \
             distinguishable from one the reference implementation produced"
        );
    }

    /// …and *wrong* is not. Nothing downstream catches it:
    /// `ensure_known_enums` walks the value's fields and never reaches `typ`,
    /// so without this check it deserializes to `typ: Some(Unknown)`, passes
    /// every remaining stage, and serialises back out as `"UNKNOWN"`.
    #[test]
    fn a_nested_value_may_not_misname_itself() {
        use rubo4e::current::ZeitvariablePreisposition;
        let err =
            decode::<ZeitvariablePreisposition>(serde_json::json!({ "_typ": "MARKTLOKATION" }))
                .expect_err("a Marktlokation is not a ZeitvariablePreisposition");
        assert_eq!(err.code(), "bo4e.discriminator");
        assert_eq!(err.to_json()["found_typ"], "MARKTLOKATION");
    }

    /// The canonical serialisation names the BO from the type, so the message
    /// cannot drift from the value that failed — and it works for a COM as
    /// well as a BO, which is what `Bo4eTyped` spanning both buys.
    #[test]
    fn canonical_serialisation_round_trips_and_names_its_type() {
        let malo = Marktlokation {
            marktlokations_id: Some(
                rubo4e::identifiers::MaloId::new("51238696781").expect("a real MaLo"),
            ),
            ..Default::default()
        };
        let json = super::to_canonical_json(&malo).expect("a generated BO always serialises");
        assert_eq!(json["_typ"], "MARKTLOKATION");
        assert_eq!(json["marktlokationsId"], "51238696781");

        // A COM goes through the same call — before 0.12 there was no bound
        // that admitted both.
        let json = super::to_canonical_json(&Betrag {
            wert: Some(dec!(1.00)),
            waehrung: Some(Waehrungscode::Eur),
            ..Default::default()
        })
        .expect("a generated COM always serialises");
        assert_eq!(json["_typ"], "BETRAG");
    }

    /// Outbound: a document mako built is type-correct by construction, and
    /// still has to add up.
    #[test]
    fn the_outbound_gate_catches_a_document_mako_built() {
        let eur = |w| {
            Some(Betrag {
                wert: Some(w),
                waehrung: Some(Waehrungscode::Eur),
                ..Default::default()
            })
        };
        let r = Rechnung {
            gesamtnetto: eur(dec!(100.00)),
            gesamtsteuer: eur(dec!(19.00)),
            gesamtbrutto: eur(dec!(120.00)),
            ..Default::default()
        };
        assert!(matches!(
            ensure_conformant(&r),
            Err(Bo4eRejection::Rule { .. })
        ));
    }

    /// Every gated type reports a discriminator that round-trips through the
    /// strict enum parser — a `_typ` mako injects must be one BO4E defines.
    #[test]
    fn every_gated_discriminator_is_a_schema_value() {
        use rubo4e::current::{BoTyp, ComTyp};
        for wire in [
            Marktlokation::TYP_WIRE,
            Zaehler::TYP_WIRE,
            Rechnung::TYP_WIRE,
            rubo4e::current::Energiemix::TYP_WIRE,
            rubo4e::current::Zahlungsinformation::TYP_WIRE,
        ] {
            assert!(
                BoTyp::from_wire(wire).is_ok() || ComTyp::from_wire(wire).is_ok(),
                "`{wire}` is neither a BoTyp nor a ComTyp"
            );
        }
    }
}
