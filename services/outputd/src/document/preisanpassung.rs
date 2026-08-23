//! The typed view a **Preisanpassung** template renders from.
//!
//! Same design as [`super::mahnung`]: the operator owns the layout, mako owns
//! the contract, and the contract is flat, named for what a reader sees, and
//! carries its legal anchors here rather than on the page.
//!
//! # What § 41 Abs. 5 EnWG actually requires the letter to say
//!
//! This is a **statutory notice**, not a marketing e-mail, and the statute is
//! specific about its content — which is why the view is not a free-form blob:
//!
//! | Requirement | Field | Norm |
//! |---|---|---|
//! | the change and **when it takes effect** | [`PreisanpassungView::wirksam_ab`] | § 41 Abs. 5 Satz 1 |
//! | **Anlass, Voraussetzungen und Umfang** of the change | [`PreisanpassungView::anlass`], [`PreisanpassungView::positionen`] | § 41 Abs. 5 Satz 1 |
//! | the notice period actually observed | [`PreisanpassungView::ankuendigungsfrist`] | § 41 Abs. 5 Satz 2 (1 Monat für Haushaltskunden), § 5 Abs. 2 GVV (6 Wochen, nur zum Monatsersten) |
//! | the **Sonderkündigungsrecht**, in the same notice | [`PreisanpassungView::sonderkuendigung`] | § 41 Abs. 5 Satz 4 |
//!
//! The Sonderkündigungsrecht is the field with teeth: Satz 4 gives the customer
//! a termination right *without notice* to the day the change takes effect, and
//! Satz 1 obliges the supplier to state it **in the same notice**. A letter that
//! announces the price and omits the right is not a valid
//! Preisänderungsanzeige, so the publish gate checks the rendered page prints
//! it.
//!
//! # Textform
//!
//! § 41 Abs. 5 Satz 2 says *in Textform*: readable, on a durable medium, with
//! the declarant named. No PDF/A to meet, nothing embedded — the same shape as
//! a Mahnung.
//!
//! Amounts are decimal strings, exactly as in [`super::view`]: pad, never
//! truncate.

use serde::Serialize;

use super::view::PartyView;

/// One price line as it changes. § 41 Abs. 5 Satz 1 asks for the **Umfang**,
/// which one sentence cannot state: a customer whose Arbeitspreis rises while
/// their Grundpreis falls has to see both.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreisPosition {
    /// What is priced, as the customer's tariff names it — `"Arbeitspreis"`,
    /// `"Grundpreis"`, `"Arbeitspreis HT"`.
    pub bezeichnung: String,
    /// The unit the two amounts are in — `"ct/kWh"`, `"EUR/Jahr"`.
    pub einheit: String,
    /// What it costs today.
    pub bisher: String,
    /// What it will cost from [`PreisanpassungView::wirksam_ab`].
    pub neu: String,
}

/// The § 41 Abs. 5 Satz 4 EnWG termination right, as the page must state it —
/// one struct, because the three facts only mean anything together: a right,
/// the date it runs to, and that exercising it costs nothing.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SonderkuendigungView {
    /// The date the customer may terminate to — the day the change takes
    /// effect. § 41 Abs. 5 Satz 4: *ohne Einhaltung einer Kündigungsfrist*.
    pub wirksam_zum: String,
    /// The norm, printed so the customer can look it up.
    pub rechtsgrundlage: String,
    /// Whether exercising the right is free of charge — always `true` under
    /// § 41 Abs. 5 Satz 4, and stated because a page that omits it invites the
    /// question.
    pub entgeltfrei: bool,
}

/// Everything a Preisanpassung template may render, and nothing else.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreisanpassungView {
    /// The date the notice bears, ISO 8601. The § 41 Abs. 5 Satz 2 period runs
    /// from it.
    pub datum: String,
    /// The supplier — § 126b's declarant. The page-content gate requires the
    /// name on the page.
    pub absender: PartyView,
    /// The customer.
    pub empfaenger: PartyView,
    /// The contract this concerns, as the customer knows it.
    pub vertragsnummer: Option<String>,
    /// The Marktlokation, for a customer with several supply points.
    pub malo_id: Option<String>,
    /// `STROM`, `GAS`, `WAERME`, `WASSER`.
    pub sparte: Option<String>,
    /// When the new prices apply, ISO 8601.
    pub wirksam_ab: String,
    /// § 41 Abs. 5 Satz 1 — **Anlass** of the change, in prose the customer
    /// reads: a levy change, a procurement-cost change, a Grundversorgung
    /// recalculation.
    pub anlass: String,
    /// The notice period actually observed, named — `"1 Monat
    /// (§ 41 Abs. 5 Satz 2 EnWG)"` — so the customer can check it.
    pub ankuendigungsfrist: String,
    /// What changes, line by line. May be empty for a change stated only in
    /// prose, which is why [`Self::anlass`] is not optional.
    pub positionen: Vec<PreisPosition>,
    /// The § 41 Abs. 5 Satz 4 termination right. **Not optional**: the right
    /// exists whenever the price changes, and a notice that omits it is not a
    /// valid Preisänderungsanzeige.
    pub sonderkuendigung: SonderkuendigungView,
    /// Free-text the operator adds — a service number, a comparison-portal
    /// pointer. Rendered where the template puts it; no statutory meaning.
    pub hinweis: Option<String>,
}

/// The Preisanpassung a template is proven against.
///
/// The awkward one, per the gate's philosophy: a **mixed** change — one price
/// up, one down — so a template that assumes prices rise, or prints one line,
/// renders it wrong and is refused.
#[must_use]
pub fn specimen() -> PreisanpassungView {
    let party = |name: &str, line1: &str, plz: &str, city: &str| PartyView {
        name: Some(name.to_owned()),
        vat_id: None,
        tax_number: None,
        line1: Some(line1.to_owned()),
        post_code: Some(plz.to_owned()),
        city: Some(city.to_owned()),
        country: Some("DE".to_owned()),
        contact_name: None,
        phone: None,
        email: None,
    };
    PreisanpassungView {
        datum: "2026-03-01".to_owned(),
        absender: PartyView {
            vat_id: Some("DE123456789".to_owned()),
            contact_name: Some("Kundenservice".to_owned()),
            phone: Some("0800 1234567".to_owned()),
            email: Some("service@stadtwerke-musterstadt.example".to_owned()),
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
        vertragsnummer: Some("V-2024-004711".to_owned()),
        malo_id: Some("51238696012".to_owned()),
        sparte: Some("STROM".to_owned()),
        wirksam_ab: "2026-05-01".to_owned(),
        anlass: "Gestiegene Beschaffungskosten und die zum 01.01.2026 geänderten \
                 Netzentgelte Ihres Netzbetreibers."
            .to_owned(),
        ankuendigungsfrist: "1 Monat (§ 41 Abs. 5 Satz 2 EnWG)".to_owned(),
        positionen: vec![
            PreisPosition {
                bezeichnung: "Arbeitspreis".to_owned(),
                einheit: "ct/kWh".to_owned(),
                bisher: "34.90".to_owned(),
                neu: "37.20".to_owned(),
            },
            // Deliberately a reduction: a template assuming every line rises
            // renders this one wrong, and the gate is where that is found.
            PreisPosition {
                bezeichnung: "Grundpreis".to_owned(),
                einheit: "EUR/Jahr".to_owned(),
                bisher: "143.88".to_owned(),
                neu: "131.40".to_owned(),
            },
        ],
        sonderkuendigung: SonderkuendigungView {
            wirksam_zum: "2026-05-01".to_owned(),
            rechtsgrundlage: "§ 41 Abs. 5 Satz 4 EnWG".to_owned(),
            entgeltfrei: true,
        },
        hinweis: Some(
            "Sie erreichen uns unter 0800 1234567 oder service@stadtwerke-musterstadt.example."
                .to_owned(),
        ),
    }
}
