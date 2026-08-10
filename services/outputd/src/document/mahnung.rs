//! The typed view a **Mahnung** template renders from.
//!
//! Same design as [`super::view::DocumentView`], different document: the
//! operator owns the layout, mako owns the contract, and the contract is
//! deliberately flat, named for what a reader sees, and carries its legal
//! anchors in the documentation.
//!
//! # Why this lives in `outputd` when the data lives in `accountingd`
//!
//! `accountingd` owns the Mahnwesen — `dunning_cases` (Stufe 1–3, § 41f/41g
//! EnWG disconnection sequence at Stufe 3), `interest_charges` (§ 288 BGB
//! Verzugszinsen over the § 247 BGB Basiszinssatz) and the MAHNGEBUEHR ledger
//! entries. Every fact in this view is computed there; `outputd` neither
//! computes a Mahnstufe nor decides a fee.
//!
//! The view is here anyway because it is a **page-description contract, and a
//! contract belongs to the consumer of the format**: the publish gate must
//! render the specimen at publish time, inside the process that owns the
//! template store — and services in this workspace talk HTTP, never crates, so
//! the type cannot live in `accountingd` without either a service-to-service
//! crate dependency or a second renderer+store there (the two-template-systems
//! outcome the platform explicitly rejects). `accountingd` conforms to this
//! view the way a browser conforms to HTML. The precedent is
//! [`DocumentView`](super::view::DocumentView) itself: `vertragd` owns the
//! buyer master, the renderer owns the projection.
//!
//! **That extraction happened** (2026-08-10): renderer, store, gates and view
//! contracts moved here together from `billingd`, because they are one thing.
//! Producers duplicate the view struct at their HTTP boundary, exactly as
//! `vertragd` client types are duplicated elsewhere — the copy here is the
//! normative one.
//!
//! # Textform, not PDF/A
//!
//! A Mahnung is Textform (§ 126b BGB): readable, on a durable medium, with the
//! declarant named. There is no PDF/A conformance to meet and nothing to embed
//! — which is why the publish gate proves this kind to
//! [`Proof::RenderedTextform`](super::gate::Proof) rather than
//! `RENDERED_PDFA`, and why the § 126b **declarant** is part of the page-content
//! check: a Mahnung that does not say who is declaring is not Textform.
//!
//! Amounts are decimal strings, exactly as in [`super::view`]: pad, never
//! truncate.

use serde::Serialize;

use super::view::PartyView;

/// One overdue invoice the Mahnung demands payment for.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MahnPosten {
    /// The invoice's BT-1, as the customer knows it.
    pub rechnungsnummer: String,
    /// Its issue date, ISO 8601.
    pub rechnungsdatum: String,
    /// The payment deadline that has passed, ISO 8601.
    pub faellig_am: String,
    /// What is still open on it.
    pub offener_betrag: String,
}

/// Everything a Mahnung template may render, and nothing else.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MahnungView {
    /// 1, 2 or 3 — `dunning_cases.stufe`. A template addresses the reader
    /// differently at each, and at 3 the § 41f fields below carry the threat.
    pub stufe: u8,
    /// The date this Mahnung bears, ISO 8601.
    pub datum: String,
    /// The new payment deadline, ISO 8601 — `dunning_cases.due_date`.
    pub zahlungsfrist: String,
    /// The creditor — § 126b's declarant. The page-content gate requires the
    /// name on the page.
    pub absender: PartyView,
    /// The debtor.
    pub empfaenger: PartyView,
    /// The overdue invoices, oldest first.
    pub posten: Vec<MahnPosten>,
    /// Mahngebühr for this Stufe, if one is charged.
    pub mahngebuehr: Option<String>,
    /// § 288 BGB Verzugszinsen accrued so far, if charged.
    pub verzugszinsen: Option<String>,
    /// The § 288 basis as prose, e.g. `"5 Prozentpunkte über dem
    /// Basiszinssatz (§ 288 Abs. 1 BGB)"` — stated because a demanded interest
    /// amount without its basis invites the dispute it should prevent.
    pub zins_grundlage: Option<String>,
    /// Sum of everything demanded: Posten + Gebühr + Zinsen.
    pub gesamtforderung: String,
    /// IBAN payment goes to.
    pub iban: Option<String>,
    /// Stufe 3 only: the § 41f Abs. 1 EnWG Sperrandrohung. `Some` makes a
    /// template print the threat block; the 4-Wochen-Frist lives in
    /// [`Self::geplantes_sperrdatum`].
    pub sperrandrohung: Option<String>,
    /// Stufe 3 only: the earliest lawful disconnection date, ISO 8601.
    pub geplantes_sperrdatum: Option<String>,
}

/// The Mahnung a template is proven against.
///
/// The awkward one, per the gate's philosophy: **Stufe 3** — the most legally
/// loaded variant, whose § 41f threat block and Sperrdatum a template must
/// place — with two overdue invoices, a fee, and interest with its basis. A
/// template that renders this renders Stufe 1 too; the reverse is not true.
#[must_use]
pub fn specimen() -> MahnungView {
    let party = |name: &str, line1: &str, plz: &str, city: &str| PartyView {
        name: Some(name.to_owned()),
        vat_id: None,
        line1: Some(line1.to_owned()),
        post_code: Some(plz.to_owned()),
        city: Some(city.to_owned()),
        country: Some("DE".to_owned()),
        contact_name: None,
        phone: None,
        email: None,
    };
    MahnungView {
        stufe: 3,
        datum: "2026-03-01".to_owned(),
        zahlungsfrist: "2026-03-15".to_owned(),
        absender: PartyView {
            vat_id: Some("DE123456789".to_owned()),
            contact_name: Some("Forderungsmanagement".to_owned()),
            phone: Some("0800 1234567".to_owned()),
            email: Some("mahnung@stadtwerke-musterstadt.example".to_owned()),
            ..party(
                "Stadtwerke Musterstadt GmbH",
                "Musterstraße 1",
                "12345",
                "Musterstadt",
            )
        },
        empfaenger: party(
            "Erika Mustermann-Übelacker",
            "Beispielweg 7",
            "10115",
            "Berlin",
        ),
        posten: vec![
            MahnPosten {
                rechnungsnummer: "R-2025-000871".to_owned(),
                rechnungsdatum: "2025-12-31".to_owned(),
                faellig_am: "2026-01-14".to_owned(),
                offener_betrag: "383.17".to_owned(),
            },
            MahnPosten {
                rechnungsnummer: "R-2026-000042".to_owned(),
                rechnungsdatum: "2026-01-31".to_owned(),
                faellig_am: "2026-02-14".to_owned(),
                offener_betrag: "127.40".to_owned(),
            },
        ],
        mahngebuehr: Some("5.00".to_owned()),
        verzugszinsen: Some("7.83".to_owned()),
        zins_grundlage: Some(
            "5 Prozentpunkte über dem Basiszinssatz (§ 288 Abs. 1 BGB)".to_owned(),
        ),
        gesamtforderung: "523.40".to_owned(),
        iban: Some("DE89370400440532013000".to_owned()),
        sperrandrohung: Some(
            "Sollte der Gesamtbetrag nicht bis zum genannten Termin eingehen, drohen wir \
             hiermit gemäß § 41f Abs. 1 EnWG die Unterbrechung Ihrer Stromversorgung an."
                .to_owned(),
        ),
        geplantes_sperrdatum: Some("2026-04-01".to_owned()),
    }
}
