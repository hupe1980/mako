//! Turning a scheduled price change into the notice the customer receives.
//!
//! `vertragd` computes the § 41 Abs. 5 EnWG Preisänderungsanzeige — which
//! regime applies, whether the period is kept, what the Sonderkündigungsrecht
//! runs to — and this renders and delivers it through `outputd`.
//!
//! # Why the prices come from the caller and not from `productd`
//!
//! § 41 Abs. 5 Satz 3 wants the **Umfang** of the change, which the product
//! catalogue looks like the source for and is not, twice over: `vertragd` owns
//! which product a Marktlokation is on and `productd` owns what it costs, and
//! the two are deliberately uncoupled (BILLING.md § 3); and the question asked
//! afterwards is *what were we told our new price would be*, which is a fact
//! about the notice that a catalogue read years later cannot answer.
//!
//! So the announced lines travel with the Tarifwechsel that schedules the
//! change — the caller chose the tariff and holds both price sheets — and are
//! stored on the slice. A supplier-initiated change cannot be scheduled without
//! them: a Preisänderungsanzeige stating no Umfang is not a valid one, and a
//! price change that cannot be validly announced must not take effect.

use anyhow::{Context as _, Result};
use mako_service::http::Upstream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── The page contract ─────────────────────────────────────────────────────────
//
// Mirrors `outputd::document::preisanpassung`; that copy is normative.

/// A party as the page prints it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartyView {
    pub name: Option<String>,
    pub vat_id: Option<String>,
    pub tax_number: Option<String>,
    pub line1: Option<String>,
    pub post_code: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub contact_name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
}

impl PartyView {
    /// The postal address `outputd` accepts, or `None` when it is incomplete.
    ///
    /// One predicate for two decisions that must agree — whether to *request*
    /// the `POST` channel and what to *send* as its address. Separate tests
    /// disagree: a `post_code && city` check admits an address with no `line1`,
    /// and `outputd` reads a present `recipient.address` as "there is somewhere
    /// to post this to".
    #[must_use]
    pub fn postal_address(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "line1":     self.line1.as_deref()?,
            "post_code": self.post_code.as_deref()?,
            "city":      self.city.as_deref()?,
            "country":   self.country.as_deref().unwrap_or("DE"),
        }))
    }
}

/// One price line as it changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreisPosition {
    pub bezeichnung: String,
    pub einheit: String,
    pub bisher: String,
    pub neu: String,
}

/// The § 41 Abs. 5 Satz 4 EnWG termination right, as the page must state it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonderkuendigungView {
    pub wirksam_zum: String,
    pub rechtsgrundlage: String,
    pub entgeltfrei: bool,
}

/// Everything a Preisanpassung template may render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreisanpassungView {
    pub datum: String,
    pub absender: PartyView,
    pub empfaenger: PartyView,
    pub vertragsnummer: Option<String>,
    pub malo_id: Option<String>,
    pub sparte: Option<String>,
    pub wirksam_ab: String,
    pub anlass: String,
    pub ankuendigungsfrist: String,
    pub positionen: Vec<PreisPosition>,
    pub sonderkuendigung: SonderkuendigungView,
    pub hinweis: Option<String>,
}

// ── outputd ───────────────────────────────────────────────────────────────────

/// What outputd recorded.
#[derive(Debug, Deserialize)]
pub struct IssuedDocument {
    pub document_id: Uuid,
    pub template_hash: String,
}

/// Client for `outputd`, the customer-communications daemon.
pub struct OutputdClient {
    up: Upstream,
}

impl OutputdClient {
    #[must_use]
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            up: Upstream::new(
                "outputd",
                base_url,
                api_key.map(secrecy::SecretString::from),
                mako_service::http::default_client(),
            ),
        }
    }

    /// `POST /api/v1/documents/issue/PREISANPASSUNG` — render, record and queue.
    ///
    /// Idempotent on the slice id: announcing the same change twice would
    /// leave the customer with two Sonderkündigungsfristen and no way to tell
    /// which counts.
    ///
    /// # Errors
    ///
    /// Propagates transport failures and outputd's refusals — most often
    /// `NO_CURRENT_TEMPLATE`, meaning no PREISANPASSUNG layout is rolled out.
    pub async fn issue_preisanpassung(
        &self,
        view: &PreisanpassungView,
        subject_ref: &str,
        malo_id: Option<&str>,
        channels: &[String],
        wirksam: time::Date,
    ) -> Result<IssuedDocument> {
        let body = serde_json::json!({
            "view":        view,
            "subject_ref": subject_ref,
            "malo_id":     malo_id,
            "recipient": {
                "name":  view.empfaenger.name,
                "email": view.empfaenger.email,
                "address": view.empfaenger.postal_address(),
            },
            "channels":    channels,
            // The notice bears the day it is written: the § 41 Abs. 5 Satz 2
            // period runs from it, not from the change.
            "date":        view.datum,
            "ident":       format!("{subject_ref}:{wirksam}"),
        });
        self.up
            .json(
                self.up
                    .post("/api/v1/documents/issue/PREISANPASSUNG")
                    .json(&body),
            )
            .await
            .context("outputd POST document PREISANPASSUNG")?
            .context("outputd answered 404 for the document endpoint — is it on this version?")
    }
}

/// The operator, as § 126b BGB's declarant. Configured rather than derived:
/// `vertragd` holds customers, not the operator's own letterhead.
#[must_use]
pub fn absender(cfg: &crate::config::VertragdConfig) -> PartyView {
    let a = cfg.absender.as_ref();
    PartyView {
        name: a
            .and_then(|a| a.name.clone())
            .or_else(|| Some(cfg.lf_mp_id.clone())),
        line1: a.and_then(|a| a.line1.clone()),
        post_code: a.and_then(|a| a.post_code.clone()),
        city: a.and_then(|a| a.city.clone()),
        country: a.and_then(|a| a.country.clone()),
        vat_id: a.and_then(|a| a.vat_id.clone()),
        contact_name: a.and_then(|a| a.contact_name.clone()),
        phone: a.and_then(|a| a.phone.clone()),
        email: a.and_then(|a| a.email.clone()),
        tax_number: None,
    }
}

// ── The documents an online contract owes ─────────────────────────────────────
//
// Mirrors `outputd::document::{vertragszusammenfassung, vertragsbestaetigung,
// widerrufsbestaetigung}`; those copies are normative.

/// One price line as the page prints it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreisZeile {
    pub bezeichnung: String,
    pub einheit: String,
    /// A decimal string.
    pub wert: String,
}

/// The § 41 Abs. 4 EnWG Vertragszusammenfassung — owed to every
/// Letztverbraucher „innerhalb einer angemessenen Frist nach dem
/// Vertragsschluss", carrying Satz 2 Nr. 1–6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertragszusammenfassungView {
    pub datum: String,
    /// Nr. 1 — die Kontaktdaten des Energielieferanten.
    pub absender: PartyView,
    pub empfaenger: PartyView,
    pub vertragsnummer: String,
    pub sparte: Option<String>,
    /// Nr. 2 — die Verbrauchsstelle.
    pub verbrauchsstelle: String,
    pub malo_id: Option<String>,
    /// Nr. 3 — geltende Preise.
    pub preise: Vec<PreisZeile>,
    /// Nr. 4 — der voraussichtliche Belieferungsbeginn, ISO 8601.
    pub belieferungsbeginn: String,
    /// Nr. 5 — die Kündigungsfrist.
    pub kuendigungsfrist: String,
    /// Nr. 6 — etwaige Bonusvereinbarungen.
    pub bonus: Option<String>,
    /// Nr. 6 — etwaige Mindestvertragslaufzeiten.
    pub mindestvertragslaufzeit: Option<String>,
}

/// The Widerrufsbelehrung a Vertragsbestätigung carries (Art. 246a § 1 Abs. 2
/// Satz 1 Nr. 1 and Nr. 3 EGBGB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WiderrufsbelehrungView {
    pub frist: String,
    /// The operator's registered Widerrufsbelehrung, printed.
    pub belehrung_text: String,
    /// The operator's registered Muster-Widerrufsformular, printed.
    pub muster_widerrufsformular: String,
    pub widerrufsfunktion: Option<String>,
    pub wertersatz_hinweis: String,
}

/// The § 312f Abs. 2 BGB Vertragsbestätigung on a durable medium.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertragsbestaetigungView {
    pub datum: String,
    pub absender: PartyView,
    pub empfaenger: PartyView,
    pub vertragsnummer: String,
    /// ISO 8601 date-time in Berlin local time.
    pub vertragsschluss_am: String,
    pub sparte: Option<String>,
    pub verbrauchsstelle: String,
    pub malo_id: Option<String>,
    pub produkt: String,
    pub preise: Vec<PreisZeile>,
    pub belieferungsbeginn: String,
    pub laufzeit: String,
    pub kuendigungsfrist: String,
    pub widerruf: WiderrufsbelehrungView,
    pub lieferung_in_widerrufsfrist_verlangt: bool,
}

/// The declaration a Widerrufsbestätigung repeats (§ 356a Abs. 2 BGB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErklaerungView {
    pub name: String,
    pub vertrag: String,
    pub bestaetigung_an: String,
}

/// The § 356a Abs. 4 BGB Eingangsbestätigung: the declaration's content and
/// the date and time of receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WiderrufsbestaetigungView {
    pub datum: String,
    pub absender: PartyView,
    pub empfaenger: PartyView,
    pub vertragsnummer: String,
    /// ISO 8601 date-time in Berlin local time.
    pub eingegangen_am: String,
    pub abgesendet_am: Option<String>,
    pub kanal: String,
    pub erklaerung: ErklaerungView,
}

/// The Art. 246a § 1 Abs. 2 Satz 1 Nr. 3 EGBGB notice on Wertersatz for
/// Strom and Gas delivered after an express request — the page's wording.
pub const WERTERSATZ_HINWEIS: &str = "Haben Sie verlangt, dass die Belieferung mit Strom oder \
     Gas während der Widerrufsfrist beginnen soll, so haben Sie uns für die bis zu Ihrem Widerruf \
     gelieferte Energie Wertersatz zu leisten (§ 357a Abs. 2 BGB). Grundlage ist der vereinbarte \
     Gesamtpreis.";

/// An instant as the page prints it: ISO 8601 in gesetzlicher deutscher Zeit.
/// `outputd` prints the clock time as it stands and converts nothing.
#[must_use]
pub fn berlin_iso(t: time::OffsetDateTime) -> String {
    mako_fristen::in_berlin(t)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| t.to_string())
}

/// The customer of an order as the page addresses him, from the canonical
/// BO4E `Geschaeftspartner` and the order's e-mail.
#[must_use]
pub fn empfaenger(geschaeftspartner: &serde_json::Value, email: Option<&str>) -> PartyView {
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let name = s(geschaeftspartner, "name1").or_else(|| {
        let teile: Vec<String> = ["vorname", "nachname"]
            .iter()
            .filter_map(|k| s(geschaeftspartner, k))
            .collect();
        (!teile.is_empty()).then(|| teile.join(" "))
    });
    let adresse = geschaeftspartner
        .get("adresse")
        .cloned()
        .unwrap_or_default();
    let line1 = s(&adresse, "strasse").map(|strasse| match s(&adresse, "hausnummer") {
        Some(nr) => format!("{strasse} {nr}"),
        None => strasse,
    });
    PartyView {
        name,
        line1,
        post_code: s(&adresse, "postleitzahl"),
        city: s(&adresse, "ort"),
        country: s(&adresse, "landescode"),
        email: email.map(str::to_owned),
        ..PartyView::default()
    }
}

/// The body of `POST /api/v1/documents/issue/{kind}` for one document:
/// delivered by e-mail where the customer gave one, by post where the address
/// is complete, and to the portal always.
#[must_use]
pub fn issue_body(
    view: serde_json::Value,
    empfaenger: &PartyView,
    subject_ref: &str,
    malo_id: Option<&str>,
    datum: &str,
) -> serde_json::Value {
    let mut channels = vec!["PORTAL"];
    if empfaenger.email.is_some() {
        channels.push("EMAIL");
    }
    if empfaenger.postal_address().is_some() {
        channels.push("POST");
    }
    serde_json::json!({
        "view":        view,
        "subject_ref": subject_ref,
        "malo_id":     malo_id,
        "recipient": {
            "name":    empfaenger.name,
            "email":   empfaenger.email,
            "address": empfaenger.postal_address(),
        },
        "channels":    channels,
        "date":        datum,
        "ident":       subject_ref,
    })
}

/// One delivery track of an issued document, as `outputd` reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct Zustellung {
    pub status: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub delivered_at: Option<time::OffsetDateTime>,
}

/// An issued document with its delivery tracks.
#[derive(Debug, Clone, Deserialize)]
pub struct DokumentMitZustellung {
    #[serde(default)]
    pub deliveries: Vec<Zustellung>,
}

impl DokumentMitZustellung {
    /// The earliest proven delivery — a `DELIVERED` track's `delivered_at`. A
    /// `SENT` e-mail without a delivery report proves no Zugang.
    #[must_use]
    pub fn zugestellt_am(&self) -> Option<time::OffsetDateTime> {
        self.deliveries
            .iter()
            .filter(|d| d.status == "DELIVERED")
            .filter_map(|d| d.delivered_at)
            .min()
    }
}

impl OutputdClient {
    /// `GET /api/v1/documents/by-subject/{kind}/{subject_ref}` — the document
    /// and its delivery tracks; `None` when none was issued.
    ///
    /// # Errors
    ///
    /// Propagates transport failures and outputd's refusals.
    pub async fn by_subject(
        &self,
        kind: &str,
        subject_ref: &str,
    ) -> Result<Option<DokumentMitZustellung>> {
        self.up
            .json(self.up.get(&format!(
                "/api/v1/documents/by-subject/{kind}/{subject_ref}"
            )))
            .await
            .context("outputd GET document by subject")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_natural_person_is_addressed_by_name_and_street() {
        let p = empfaenger(
            &serde_json::json!({
                "vorname": "Erika", "nachname": "Mustermann",
                "adresse": { "strasse": "Hauptstraße", "hausnummer": "1",
                             "postleitzahl": "50667", "ort": "Köln", "landescode": "DE" }
            }),
            Some("erika@example.org"),
        );
        assert_eq!(p.name.as_deref(), Some("Erika Mustermann"));
        assert_eq!(p.line1.as_deref(), Some("Hauptstraße 1"));
        assert!(p.postal_address().is_some());
    }

    /// § 356 Abs. 3 BGB reads Zugang: only a `DELIVERED` track counts.
    #[test]
    fn a_sent_email_is_no_delivery() {
        let d: DokumentMitZustellung = serde_json::from_value(serde_json::json!({
            "deliveries": [
                { "status": "SENT", "delivered_at": null },
                { "status": "DELIVERED", "delivered_at": "2026-11-20T10:00:00Z" }
            ]
        }))
        .unwrap();
        assert_eq!(
            d.zugestellt_am(),
            Some(time::macros::datetime!(2026-11-20 10:00 UTC))
        );
        let nur_gesendet: DokumentMitZustellung = serde_json::from_value(serde_json::json!({
            "deliveries": [{ "status": "SENT", "delivered_at": null }]
        }))
        .unwrap();
        assert_eq!(nur_gesendet.zugestellt_am(), None);
    }
}
