//! The CloudEvents contract: which `type` values exist, and how a subscription
//! pattern matches one.
//!
//! EDIFACT is only one of the wire contracts a MaKo platform exposes; the other
//! is the event stream, and asserting on it needs the same discipline. A test
//! that writes the type as a string literal cannot tell "the platform never
//! emitted this" from "this type no longer exists" — and the second is silent,
//! because an event nobody emits is exactly what a missing-event assertion
//! expects to find. Type names move between prefixes as services are split and
//! renamed, so the catalog is bound rather than copied.
//!
//! `mako-events` also owns the **single** glob matcher every subscription
//! mechanism in the workspace uses, so a test asserting "this pattern would
//! have delivered that event" measures the platform's own routing.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Every CloudEvents `type` the platform declares, sorted.
///
/// Parametrize over this to assert a consumer handles the whole catalog, or
/// draw from it in a strategy — a type invented for a test is one no platform
/// will ever emit.
#[pyfunction]
pub fn event_types() -> Vec<String> {
    let mut out: Vec<String> = mako_events::all().iter().map(|t| (*t).to_owned()).collect();
    out.sort_unstable();
    out
}

/// `True` when `event_type` is a declared type.
///
/// The assertion helpers call this first: a subscription or an expectation
/// naming a type the catalog does not carry is a typo or a rename, and it would
/// otherwise pass forever as "no such event was emitted".
#[pyfunction]
pub fn event_type_exists(event_type: &str) -> bool {
    mako_events::all().contains(&event_type)
}

/// `True` when a subscription `pattern` selects `event_type`.
///
/// `*` matches any sequence, `?` exactly one character, everything else is
/// literal — so `de.mako.*` behaves like a prefix and `de.*.rechnung.*` works
/// too. Bound rather than reimplemented because there is deliberately **one**
/// matcher in the workspace: a harness with its own would disagree with the
/// routing it is testing.
#[pyfunction]
pub fn event_matches(pattern: &str, event_type: &str) -> bool {
    mako_events::matches(pattern, event_type)
}

/// Every declared type a subscription `pattern` would deliver, sorted.
///
/// Empty means the pattern is dead — nothing the platform emits reaches that
/// subscriber, which is a configuration defect a test should be able to state.
#[pyfunction]
pub fn event_types_matching(pattern: &str) -> Vec<String> {
    let mut out: Vec<String> = mako_events::all()
        .iter()
        .filter(|t| mako_events::matches(pattern, t))
        .map(|t| (*t).to_owned())
        .collect();
    out.sort_unstable();
    out
}

/// The eight CloudEvents 1.0 **context attributes**, plus `data`.
///
/// `data` is the event payload rather than a context attribute — §3 defines
/// four required context attributes (`id`, `source`, `specversion`, `type`) and
/// four optional ones (`datacontenttype`, `dataschema`, `subject`, `time`). It
/// is listed here because an extension attribute must not collide with any of
/// these *names*: the JSON format serialises extensions flat beside them, and a
/// collision emits the key twice, which every receiver rejects as a duplicate
/// field. `data` collides exactly as a context attribute would.
#[pyfunction]
pub fn cloudevent_core_attributes() -> Vec<String> {
    [
        "specversion",
        "id",
        "source",
        "type",
        "time",
        "subject",
        "datacontenttype",
        "dataschema",
        "data",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Every member name the CloudEvents **JSON format** allows besides extensions.
///
/// The eight context attributes, `data`, and `data_base64` — which the JSON
/// format defines as the carrier for binary payloads. `data_base64` is not a
/// context attribute and its underscore makes it an illegal *extension* name, so
/// an envelope check that knew only the other members would reject a conformant
/// binary event.
///
/// §3.1 also makes `data` and `data_base64` mutually exclusive: an event
/// carrying both has two payloads and no rule for which one wins.
#[pyfunction]
pub fn cloudevent_json_members() -> Vec<String> {
    let mut out = cloudevent_core_attributes();
    out.push("data_base64".to_owned());
    out
}

/// `True` when `key` is a legal CloudEvents extension attribute name.
///
/// §3.3 restricts an extension name to lowercase letters and digits, and it
/// must not collide with a context attribute or with `data`. A `traceparent`
/// passes; a `makoPid`, a `mako-pid` or an `id` does not.
#[pyfunction]
pub fn is_valid_extension_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && !cloudevent_core_attributes().iter().any(|a| a == key)
}

/// Validate an ISO 8601 / RFC 3339 `time` attribute, returning it normalised.
///
/// Raises `ValueError` when the value is not a timestamp a CloudEvents receiver
/// can parse — the attribute is defined as RFC 3339, and a
/// `Debug`-formatted datetime is the shape that slips through untyped emitters.
#[pyfunction]
pub fn parse_cloudevent_time(value: &str) -> PyResult<String> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|e| {
            PyValueError::new_err(format!(
                "CloudEvents `time` must be RFC 3339, got {value:?}: {e}"
            ))
        })?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(event_types, m)?)?;
    m.add_function(wrap_pyfunction!(event_type_exists, m)?)?;
    m.add_function(wrap_pyfunction!(event_matches, m)?)?;
    m.add_function(wrap_pyfunction!(event_types_matching, m)?)?;
    m.add_function(wrap_pyfunction!(cloudevent_core_attributes, m)?)?;
    m.add_function(wrap_pyfunction!(cloudevent_json_members, m)?)?;
    m.add_function(wrap_pyfunction!(is_valid_extension_key, m)?)?;
    m.add_function(wrap_pyfunction!(parse_cloudevent_time, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_substantial_sorted_and_unique() {
        let types = event_types();
        assert!(types.len() > 50, "got {}", types.len());
        assert!(types.windows(2).all(|w| w[0] < w[1]));
        assert!(types.iter().all(|t| t.starts_with("de.")));
    }

    /// The point of binding the catalog: a renamed or invented type is not a
    /// missing event, and the two must not be confusable.
    #[test]
    fn a_retired_prefix_is_not_a_declared_type() {
        assert!(event_type_exists("de.mako.process.completed"));
        assert!(!event_type_exists("de.edmd.reading.stored"));
        assert!(!event_type_exists("de.mako.process.complete"));
    }

    #[test]
    fn a_pattern_resolves_to_the_types_it_would_deliver() {
        let mako = event_types_matching("de.mako.*");
        assert!(mako.contains(&"de.mako.process.initiated".to_owned()));
        assert!(!mako.iter().any(|t| t.starts_with("de.markt.")));
        assert_eq!(event_types_matching("*").len(), event_types().len());
        assert!(
            event_types_matching("de.nosuch.*").is_empty(),
            "a dead subscription must be visible as one"
        );
    }

    /// `data_base64` is a JSON-format member, not a context attribute and not a
    /// legal extension name — an envelope check that knew only the eight
    /// context attributes and `data` would reject a conformant binary event.
    #[test]
    fn the_json_format_carries_one_member_beyond_the_context_attributes() {
        let members = cloudevent_json_members();
        assert!(members.contains(&"data_base64".to_owned()));
        assert!(!is_valid_extension_key("data_base64"));
        assert_eq!(members.len(), cloudevent_core_attributes().len() + 1);
    }

    #[test]
    fn extension_keys_follow_the_spec() {
        assert!(is_valid_extension_key("makopid"));
        assert!(is_valid_extension_key("traceparent"));
        assert!(!is_valid_extension_key("makoPid"), "no uppercase");
        assert!(!is_valid_extension_key("mako-pid"), "no punctuation");
        assert!(
            !is_valid_extension_key("id"),
            "collides with a core attribute"
        );
        assert!(!is_valid_extension_key(""));
    }

    #[test]
    fn the_time_attribute_must_be_rfc_3339() {
        assert_eq!(
            parse_cloudevent_time("2026-03-02T09:00:00Z").unwrap(),
            "2026-03-02T09:00:00Z"
        );
        assert!(parse_cloudevent_time("2026-03-02 09:00:00 +0:00:00").is_err());
        assert!(parse_cloudevent_time("2026-03-02").is_err());
    }
}
