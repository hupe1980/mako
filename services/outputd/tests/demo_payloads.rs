//! Guard: the request bodies `demos/o2c` posts to `outputd` are ones `outputd`
//! accepts.
//!
//! A demo payload is shipped API surface — rolling a layout out is the first
//! thing an operator does, because `outputd` ships no layout of its own and
//! refuses to render an INVOICE until one is published. Both bodies deny
//! unknown fields, so this guard is for the demo drifting from the endpoint
//! while still passing on the effect it asserts.

use outputd::handlers::{PublishTemplateRequest, SetCurrentRequest};

/// Publishing the reference template. `source` is Typst, not a hash or a path:
/// `POST /templates` proves the template by rendering a specimen invoice to
/// PDF/A before storing it, so it needs the text.
#[test]
fn the_o2c_publish_template_payload_deserialises() {
    let req: PublishTemplateRequest = serde_json::from_value(serde_json::json!({
        "kind": "INVOICE",
        "source": "#let render(invoice) = [ #invoice.rechnungsnummer ]"
    }))
    .expect("outputd would refuse this demo payload — fix the demo, or the endpoint");
    assert!(req.source.contains("render"));
    assert_eq!(
        req.pdf_standard, None,
        "the demo takes the default, which is the a-3b ZUGFeRD 2.3 requires"
    );
}

/// Rolling it out. The body names the proven hash, so a layout that never
/// rendered cannot become the current one.
#[test]
fn the_o2c_set_current_payload_deserialises() {
    let req: SetCurrentRequest = serde_json::from_value(serde_json::json!({
        "hash": "d4e9c165ab31000000000000000000000000000000000000000000000000abcd"
    }))
    .expect("outputd would refuse this demo payload — fix the demo, or the endpoint");
    assert_eq!(req.hash.len(), 64);
}

/// A field the endpoint does not have is a refusal, not a discarded value.
///
/// `template` is the near-miss: the field is `source`, and a body carrying the
/// Typst under another name would publish an empty template that renders a
/// blank invoice.
#[test]
fn a_field_the_endpoint_does_not_have_is_refused() {
    let err = serde_json::from_value::<PublishTemplateRequest>(serde_json::json!({
        "kind": "INVOICE",
        "template": "#let render(invoice) = []"
    }))
    .expect_err("the Typst field is `source`");
    assert!(err.to_string().contains("unknown field"), "{err}");
}
