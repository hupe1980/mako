//! The typed view a **Gebührenbescheid** template renders from.
//!
//! A municipal operator bills taxable Trinkwasser on an invoice and the
//! öffentlich-rechtliche Abwassergebühr on a Bescheid. The two cannot share a
//! document as an e-invoice — EN 16931 BR-O-11 ff. make the `O` category
//! („not subject to VAT") exclusive — and they are not the same kind of
//! document: an invoice states a supply, a Bescheid fixes a public-law levy.
//! `billingd` therefore issues them as two records, and this is the view of the
//! second.
//!
//! # What the page states
//!
//! The content follows the form § 157 Abs. 1 AO gives a Steuerbescheid
//! (gesetze-im-internet.de, read 2026-11-10): it must „die festgesetzte Steuer
//! nach Art und Betrag bezeichnen und angeben, wer die Steuer schuldet", and
//! „Ihnen ist außerdem eine Belehrung darüber beizufügen, welcher Rechtsbehelf
//! zulässig ist und binnen welcher Frist und bei welcher Behörde er einzulegen
//! ist".
//!
//! | Requirement | Field |
//! |---|---|
//! | the levy **nach Art und Betrag** | [`GebuehrenbescheidView::positionen`], [`GebuehrenbescheidView::festgesetzt`] |
//! | **wer schuldet** | [`GebuehrenbescheidView::schuldner`] |
//! | the **Rechtsbehelfsbelehrung**: Rechtsbehelf, Frist, Behörde | [`GebuehrenbescheidView::rechtsbehelf`] |
//!
//! The publish gate renders the specimen and checks the page prints each of
//! them. Like a Mahnung it is a letter, not a ZUGFeRD carrier: no PDF/A to meet
//! and nothing to embed, so it is proven to `RENDERED_TEXTFORM`.
//!
//! Amounts are decimal strings, exactly as in [`super::view`]: pad, never
//! truncate.

use serde::Serialize;

use super::view::PartyView;

/// One festgesetzte Gebühr, by Art and Betrag.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GebuehrPosition {
    /// The Art of the Gebühr — `"Schmutzwassergebühr"`,
    /// `"Niederschlagswassergebühr"`.
    pub art: String,
    /// What it is assessed on — the quantity with its unit, `"84 m³"`.
    pub bemessungsgrundlage: String,
    /// The rate with its unit, `"2,35 EUR/m³"` — as the Satzung states it.
    pub satz: String,
    /// The amount festgesetzt.
    pub betrag: String,
}

/// The Rechtsbehelfsbelehrung — the three facts § 157 Abs. 1 Satz 3 AO names.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RechtsbehelfView {
    /// Which Rechtsbehelf is zulässig — `"Widerspruch"`, `"Klage"`.
    pub rechtsbehelf: String,
    /// Within which period — `"innerhalb eines Monats nach Bekanntgabe"`.
    pub frist: String,
    /// With which Behörde, and where — name and address as one line.
    pub behoerde: String,
}

/// Everything a Gebührenbescheid template may render, and nothing else.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GebuehrenbescheidView {
    /// The date the Bescheid bears, ISO 8601.
    pub datum: String,
    /// Its number, as the Schuldner quotes it.
    pub bescheidnummer: String,
    /// The issuing body. The gate requires its name on the page.
    pub absender: PartyView,
    /// Who owes the Gebühr.
    pub schuldner: PartyView,
    /// The Marktlokation or Abnahmestelle the Gebühr is assessed for.
    pub malo_id: Option<String>,
    /// The assessed period, ISO 8601, both ends inclusive.
    pub zeitraum_von: String,
    pub zeitraum_bis: String,
    /// Each Gebühr by Art and Betrag; at least one.
    pub positionen: Vec<GebuehrPosition>,
    /// The sum festgesetzt.
    pub festgesetzt: String,
    /// When it is due, ISO 8601.
    pub faellig_am: String,
    /// IBAN payment goes to.
    pub iban: Option<String>,
    /// The Rechtsbehelfsbelehrung. **Not optional**: a Bescheid without it does
    /// not state what § 157 Abs. 1 Satz 3 AO attaches to one.
    pub rechtsbehelf: RechtsbehelfView,
}

/// The Gebührenbescheid a template is proven against: two Gebühren — one on a
/// metered quantity, one on a sealed area — so a template printing only the
/// first position is refused.
#[must_use]
pub fn specimen() -> GebuehrenbescheidView {
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
    GebuehrenbescheidView {
        datum: "2026-03-01".to_owned(),
        bescheidnummer: "GB-2026-000017".to_owned(),
        absender: PartyView {
            contact_name: Some("Abwasserabrechnung".to_owned()),
            phone: Some("0800 1234567".to_owned()),
            ..party(
                "Stadtentwässerung Musterstadt AöR",
                "Musterstraße 1",
                "12345",
                "Musterstadt",
            )
        },
        schuldner: party(
            "Erika Mustermann-Übelacker",
            "Beispielweg 7",
            "10115",
            "Berlin",
        ),
        malo_id: Some("51238696012".to_owned()),
        zeitraum_von: "2025-01-01".to_owned(),
        zeitraum_bis: "2025-12-31".to_owned(),
        positionen: vec![
            GebuehrPosition {
                art: "Schmutzwassergebühr".to_owned(),
                bemessungsgrundlage: "84 m³".to_owned(),
                satz: "2,35 EUR/m³".to_owned(),
                betrag: "197.40".to_owned(),
            },
            GebuehrPosition {
                art: "Niederschlagswassergebühr".to_owned(),
                bemessungsgrundlage: "120 m²".to_owned(),
                satz: "0,58 EUR/m²".to_owned(),
                betrag: "69.60".to_owned(),
            },
        ],
        festgesetzt: "267.00".to_owned(),
        faellig_am: "2026-04-01".to_owned(),
        iban: Some("DE89370400440532013000".to_owned()),
        rechtsbehelf: RechtsbehelfView {
            rechtsbehelf: "Widerspruch".to_owned(),
            frist: "innerhalb eines Monats nach Bekanntgabe".to_owned(),
            behoerde: "Stadtentwässerung Musterstadt AöR, Musterstraße 1, 12345 Musterstadt"
                .to_owned(),
        },
    }
}
