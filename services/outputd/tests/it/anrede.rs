//! The form of address: every customer letter is written in `[policy] anrede`
//! — the reference templates, the mail subjects — except the
//! Widerrufsbelehrung inside the Vertragsbestätigung (the statutory Muster,
//! Art. 246a § 1 Abs. 2 EGBGB, keeps its effect only as published) and the
//! Gebührenbescheid (a Verwaltungsakt), which stay „Sie“.
//!
//! Each letter is rendered and its page text scanned for the other form's
//! pronouns: one stray „Ihr“ in a `du` letter is the defect, and it reads as
//! an ordinary sentence to everyone but the customer.

use outputd::config::{Anrede, DeliveryConfig, OutputdConfig};
use outputd::document::{self, RenderRequest, gate};
use outputd::template_store::TemplateKind;

/// The „Sie“ forms: the pronoun and its possessive, capitalised.
const SIE: &[&str] = &[
    "Sie", "Ihr", "Ihre", "Ihrer", "Ihres", "Ihren", "Ihrem", "Ihnen",
];

/// The „du“ forms, lower-case and at a sentence's start.
const DU: &[&str] = &[
    "du", "dich", "dir", "dein", "deine", "deiner", "deines", "deinen", "deinem", "Du", "Dich",
    "Dir", "Dein", "Deine", "Deiner", "Deines", "Deinen", "Deinem",
];

/// A covered letter: its kind, its reference template and its specimen view.
fn letters() -> Vec<(TemplateKind, &'static str, serde_json::Value)> {
    let v = |x: serde_json::Result<serde_json::Value>| x.expect("a specimen serialises");
    vec![
        (
            TemplateKind::Mahnung,
            document::REFERENCE_MAHNUNG_TEMPLATE,
            v(serde_json::to_value(document::mahnung::specimen())),
        ),
        (
            TemplateKind::Preisanpassung,
            document::REFERENCE_PREISANPASSUNG,
            v(serde_json::to_value(document::preisanpassung::specimen())),
        ),
        (
            TemplateKind::Vertragsbestaetigung,
            document::REFERENCE_VERTRAGSBESTAETIGUNG,
            v(serde_json::to_value(
                document::vertragsbestaetigung::specimen(),
            )),
        ),
        (
            TemplateKind::Vertragszusammenfassung,
            document::REFERENCE_VERTRAGSZUSAMMENFASSUNG,
            v(serde_json::to_value(
                document::vertragszusammenfassung::specimen(),
            )),
        ),
        (
            TemplateKind::Widerrufsbestaetigung,
            document::REFERENCE_WIDERRUFSBESTAETIGUNG,
            v(serde_json::to_value(
                document::widerrufsbestaetigung::specimen(),
            )),
        ),
    ]
}

/// The page text of `template` rendered from `data`.
fn page_text(template: &str, data: String) -> String {
    let rendered = document::render(&RenderRequest {
        template: template.to_owned(),
        data: Some(data),
        attachment: None,
        standard: None,
        date: gate::SPECIMEN_DATE,
        ident: "anrede".to_owned(),
    })
    .expect("the reference template renders");
    let pdf = lopdf::Document::load_mem(&rendered.pdf).expect("a PDF");
    let pages: Vec<u32> = pdf.get_pages().keys().copied().collect();
    pdf.extract_text(&pages).expect("extractable text")
}

/// A letter rendered as production renders it: its view plus `anrede`.
fn letter(template: &str, view: &serde_json::Value, anrede: Anrede) -> String {
    let data = document::mit_anrede(view, anrede).expect("a view is an object");
    page_text(template, data.to_string())
}

/// The words of `text` that are in `forms`.
fn found(text: &str, forms: &[&str]) -> Vec<String> {
    text.split(|c: char| !c.is_alphabetic())
        .filter(|w| forms.contains(w))
        .map(str::to_owned)
        .collect()
}

/// The Vertragsbestätigung split into the letter and the Widerrufsbelehrung
/// with its Muster-Widerrufsformular, which ends before the closing greeting.
fn split_belehrung(text: &str) -> (&str, &str) {
    let start = text
        .find("Widerrufsbelehrung")
        .expect("the Belehrung is printed");
    let rest = &text[start..];
    let end = start
        + ["Mit freundlichen Grüßen", "Viele Grüße"]
            .iter()
            .find_map(|g| rest.find(g))
            .expect("a closing greeting");
    (&text[..start], &text[start..end])
}

fn config(anrede: &str) -> OutputdConfig {
    serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://localhost/outputd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
        "policy": {"anrede": anrede},
    }))
    .expect("the configuration parses")
}

/// Unset, the setting is „Sie“, and every covered letter and subject is
/// written in it — no „du“ form anywhere.
#[test]
fn by_default_every_letter_says_sie_and_no_du() {
    let cfg: OutputdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://localhost/outputd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    let anrede = cfg.policy.anrede.get();
    assert_eq!(anrede, Anrede::Sie, "the default");

    for (kind, template, view) in letters() {
        let text = letter(template, &view, anrede);
        let du = found(&text, DU);
        assert!(du.is_empty(), "{} says {du:?} by default", kind.as_str());
        assert!(
            !found(&text, SIE).is_empty(),
            "{} addresses its reader with „Sie“",
            kind.as_str()
        );
        let subject = DeliveryConfig::default_subject(kind.as_str(), anrede);
        assert!(
            found(subject, DU).is_empty(),
            "{} subject `{subject}`",
            kind.as_str()
        );
    }
    assert_eq!(
        DeliveryConfig::default_subject("INVOICE", anrede),
        "Ihre Rechnung"
    );
}

/// Set to `du`, every covered letter and subject is written in „du“ — no
/// „Sie“ form outside the Widerrufsbelehrung.
#[test]
fn with_du_every_letter_says_du_and_no_sie() {
    let cfg = config("du");
    let anrede = cfg.policy.anrede.get();
    assert_eq!(anrede, Anrede::Du);

    for (kind, template, view) in letters() {
        let text = letter(template, &view, anrede);
        let body = if kind == TemplateKind::Vertragsbestaetigung {
            split_belehrung(&text).0.to_owned()
        } else {
            text.clone()
        };
        let sie = found(&body, SIE);
        assert!(sie.is_empty(), "{} says {sie:?} under du", kind.as_str());
        assert!(
            !found(&body, DU).is_empty(),
            "{} addresses its reader with „du“",
            kind.as_str()
        );
        let subject = DeliveryConfig::default_subject(kind.as_str(), anrede);
        assert!(
            found(subject, SIE).is_empty(),
            "{} subject `{subject}`",
            kind.as_str()
        );
    }
    assert_eq!(
        DeliveryConfig::default_subject("INVOICE", anrede),
        "Deine Rechnung"
    );
    // The invoice's mail subject follows the setting too.
    assert_eq!(
        cfg.delivery.subject_for("INVOICE", anrede.fuer("INVOICE")),
        "Deine Rechnung"
    );
}

/// Under `du`, the Widerrufsbelehrung and the Muster-Widerrufsformular are
/// printed exactly as under „Sie“, and the Gebührenbescheid is written in
/// „Sie“ and records it.
#[test]
fn the_belehrung_and_the_gebuehrenbescheid_stay_sie_under_du() {
    let view = serde_json::to_value(document::vertragsbestaetigung::specimen()).unwrap();
    let under_sie = letter(document::REFERENCE_VERTRAGSBESTAETIGUNG, &view, Anrede::Sie);
    let under_du = letter(document::REFERENCE_VERTRAGSBESTAETIGUNG, &view, Anrede::Du);
    let belehrung = split_belehrung(&under_du).1;
    assert_eq!(
        belehrung,
        split_belehrung(&under_sie).1,
        "the Belehrung does not follow the setting"
    );
    assert!(!found(belehrung, SIE).is_empty(), "the Muster says „Sie“");
    assert!(found(belehrung, DU).is_empty());

    let cfg = config("du");
    let anrede = outputd::handlers::anrede_of(&cfg, TemplateKind::Gebuehrenbescheid, None)
        .expect("the setting resolves");
    assert_eq!(anrede, Anrede::Sie, "a Gebührenbescheid records „Sie“");
    assert!(
        outputd::handlers::anrede_of(&cfg, TemplateKind::Gebuehrenbescheid, Some("du")).is_err(),
        "and refuses a `du` pin"
    );
    let bescheid = page_text(
        document::REFERENCE_GEBUEHRENBESCHEID,
        serde_json::to_string(&document::gebuehrenbescheid::specimen()).unwrap(),
    );
    assert!(found(&bescheid, DU).is_empty(), "{bescheid}");
    assert_eq!(
        DeliveryConfig::default_subject("GEBUEHRENBESCHEID", anrede),
        "Gebührenbescheid"
    );
}

/// The setting is reported with its source, and a value that is neither
/// option is refused rather than read as the default.
#[test]
fn the_setting_is_reported_and_an_unknown_value_refused() {
    let report = config("du").policy_report();
    let entry = report.get("anrede").expect("reported");
    assert_eq!(entry.value, "du");
    assert_eq!(entry.origin, mako_service::policy::Origin::Explicit);
    assert!(
        serde_json::from_value::<OutputdConfig>(serde_json::json!({
            "database": {"url": "postgres://localhost/outputd"},
            "tenant": "9900357000004",
            "policy": {"anrede": "ihr"},
        }))
        .is_err()
    );
}
