//! HTTP clients for the two services `accountingd` needs to put a document in
//! front of a customer: **`vertragd`** for who the customer is, **`outputd`**
//! for what the document looks like and how it reaches them.
//!
//! `accountingd` keys everything on a Marktlokation and holds no customer
//! master — no name, no address, no e-mail — because `vertragd` is the
//! platform's one OIDC-to-MaLo boundary. Both clients use the shared
//! [`Upstream`] transport, so a deployment without a credential gets a 401 on
//! the lookup rather than an unaddressed notice.

use anyhow::{Context as _, Result};
use mako_service::http::Upstream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn upstream(name: &'static str, base_url: &str, api_key: Option<String>) -> Upstream {
    Upstream::new(
        name,
        base_url,
        api_key.map(secrecy::SecretString::from),
        mako_service::http::default_client(),
    )
}

// ── vertragd ──────────────────────────────────────────────────────────────────

/// The customer behind a Marktlokation, as far as a document needs them —
/// `vertragd`'s `RechnungsempfaengerRow`, duplicated at this HTTP boundary.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rechnungsempfaenger {
    pub name: Option<String>,
    pub line1: Option<String>,
    pub post_code: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub vat_id: Option<String>,
    /// Where an electronic document goes.
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VertragByMalo {
    #[serde(default)]
    rechnungsempfaenger: Option<Rechnungsempfaenger>,
}

/// Client for `vertragd`, the contract and customer registry.
pub struct VertragdClient {
    up: Upstream,
}

impl VertragdClient {
    #[must_use]
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            up: upstream("vertragd", base_url, api_key),
        }
    }

    /// `GET /api/v1/vertraege/by-malo/{malo_id}` → the addressee.
    ///
    /// `Ok(None)` when the MaLo has no contract or no Kunde on file — a fact,
    /// not a failure. § 126b BGB names the recipient, so an unaddressable
    /// Mahnung is not issued rather than issued unaddressed.
    ///
    /// # Errors
    ///
    /// Propagates transport and deserialisation failures.
    pub async fn rechnungsempfaenger_by_malo(
        &self,
        malo_id: &str,
    ) -> Result<Option<Rechnungsempfaenger>> {
        let path = format!("/api/v1/vertraege/by-malo/{malo_id}");
        let body: Option<VertragByMalo> = self
            .up
            .json(self.up.get(&path))
            .await
            .context("vertragd GET vertrag by malo")?;
        Ok(body.and_then(|b| b.rechnungsempfaenger))
    }
}

// ── outputd ───────────────────────────────────────────────────────────────────

/// Where a document is sent, snapshotted at issue.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Recipient {
    pub name: Option<String>,
    pub email: Option<String>,
    pub address: Option<serde_json::Value>,
}

/// A document to render, record and deliver.
#[derive(Debug)]
pub struct IssueDocumentRequest {
    /// The Textform view, already serialised — `outputd` renders it with the
    /// tenant's rolled-out template.
    pub view: serde_json::Value,
    /// The idempotency key: what this document is about. For a Mahnung, the
    /// dunning-case id, so a retry returns the notice already sent rather than
    /// a second one with its own payment deadline.
    pub subject_ref: String,
    pub malo_id: Option<String>,
    pub kunden_nr: Option<String>,
    pub recipient: Recipient,
    /// `PORTAL`, `EMAIL`, `POST`, `ERP`.
    pub channels: Vec<String>,
    /// The date the document bears.
    pub date: time::Date,
    /// Stable identity for the PDF `/ID`.
    pub ident: String,
}

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
            up: upstream("outputd", base_url, api_key),
        }
    }

    /// `POST /api/v1/documents/MAHNUNG` — render, record and queue.
    ///
    /// # Errors
    ///
    /// Propagates transport failures and outputd's refusals — most often
    /// `NO_CURRENT_TEMPLATE`, meaning no Mahnung layout is rolled out.
    pub async fn issue_mahnung(&self, req: &IssueDocumentRequest) -> Result<IssuedDocument> {
        let body = serde_json::json!({
            "view":        req.view,
            "subject_ref": req.subject_ref,
            "malo_id":     req.malo_id,
            "kunden_nr":   req.kunden_nr,
            "recipient":   req.recipient,
            "channels":    req.channels,
            "date":        req.date.to_string(),
            "ident":       req.ident,
        });
        self.up
            .json(self.up.post("/api/v1/documents/MAHNUNG").json(&body))
            .await
            .context("outputd POST document MAHNUNG")?
            .context("outputd answered 404 for the document endpoint — is it on this version?")
    }
}
