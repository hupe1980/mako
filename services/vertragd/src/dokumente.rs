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

    /// `POST /api/v1/documents/PREISANPASSUNG` — render, record and queue.
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
                "address": {
                    "line1":     view.empfaenger.line1,
                    "post_code": view.empfaenger.post_code,
                    "city":      view.empfaenger.city,
                    "country":   view.empfaenger.country,
                },
            },
            "channels":    channels,
            // The notice bears the day it is written: the § 41 Abs. 5 Satz 2
            // period runs from it, not from the change.
            "date":        view.datum,
            "ident":       format!("{subject_ref}:{wirksam}"),
        });
        self.up
            .json(self.up.post("/api/v1/documents/PREISANPASSUNG").json(&body))
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
