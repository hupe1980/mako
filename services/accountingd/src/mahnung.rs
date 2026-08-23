//! Turning a dunning case into the document the customer receives.
//!
//! `accountingd` computes the Mahnwesen — the Mahnstufe, the fee, the § 288 BGB
//! Verzugszinsen, the § 41f EnWG threat. This projects a case onto
//! [`MahnungView`], outputd's page contract, and issues it as a document with
//! delivery evidence.
//!
//! # Where each fact comes from
//!
//! | View field | Source |
//! |---|---|
//! | `stufe`, `zahlungsfrist` | `dunning_cases` |
//! | `posten` | the ledger's open receivables, after FIFO clearing — not `amount_due_ct`, which is the total when the case opened |
//! | `mahngebuehr`, `verzugszinsen` | the `MAHNGEBUEHR` / `VERZUGSZINSEN` postings, i.e. what was charged rather than what is configured |
//! | `absender` | `[creditor_*]` config — the § 126b BGB declarant |
//! | `empfaenger` | `vertragd`, by MaLo |
//! | `sperrandrohung` | the case's `sperrandrohung_at`, so the threat is printed once it has been made and not merely because the Stufe is 3 |
//!
//! § 126b BGB wants a readable declaration on a durable medium naming the
//! declarant *and* the recipient, so a case whose recipient cannot be resolved
//! is not documented: an unaddressed Mahnung would look like a sent one on
//! every later report.

use std::sync::Arc;

use serde::Serialize;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::clients::{IssueDocumentRequest, OutputdClient, Recipient, VertragdClient};
use crate::config::AccountingdConfig;
use crate::handlers::format_ct_as_eur;
use crate::ledger::PgLedger;

/// One overdue item the Mahnung demands payment for. Mirrors
/// `outputd::document::mahnung::MahnPosten`; that copy is normative.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MahnPosten {
    pub rechnungsnummer: String,
    pub rechnungsdatum: String,
    pub faellig_am: String,
    pub offener_betrag: String,
}

/// A party as the page prints it. Mirrors `outputd::document::view::PartyView`.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
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

/// Everything a Mahnung template may render. Mirrors
/// `outputd::document::mahnung::MahnungView`, pinned to it by
/// `tests::the_view_matches_outputds_specimen`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MahnungView {
    pub stufe: u8,
    pub datum: String,
    pub zahlungsfrist: String,
    pub absender: PartyView,
    pub empfaenger: PartyView,
    pub posten: Vec<MahnPosten>,
    pub mahngebuehr: Option<String>,
    pub verzugszinsen: Option<String>,
    pub zins_grundlage: Option<String>,
    pub gesamtforderung: String,
    pub iban: Option<String>,
    pub sperrandrohung: Option<String>,
    pub geplantes_sperrdatum: Option<String>,
}

/// A dunning case that has no document yet, with everything the page needs.
#[derive(Debug)]
pub struct PendingMahnung {
    pub case_id: Uuid,
    pub account_id: Uuid,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub kunden_nr: Option<String>,
    pub stufe: i16,
    pub due_date: Date,
    pub issued_at: OffsetDateTime,
    pub sperrandrohung_at: Option<OffsetDateTime>,
    pub geplantes_sperrdatum: Option<Date>,
    pub iban: Option<String>,
}

/// § 288 BGB, stated beside the amount: a demanded interest figure without its
/// basis invites the dispute it should prevent.
const ZINS_GRUNDLAGE_ABS1: &str = "5 Prozentpunkte über dem Basiszinssatz (§ 288 Abs. 1 BGB)";

/// The § 41f Abs. 1 EnWG threat block, printed only once the case carries a
/// `sperrandrohung_at`. Printing it because the Stufe is 3 would threaten a
/// customer whose Abs. 3 gates were never cleared.
fn sperrandrohung_text(sperrkosten_ct: i64, entsperrkosten_ct: i64) -> String {
    format!(
        "Sollte der Gesamtbetrag nicht bis zum genannten Termin eingehen, drohen wir hiermit \
         gemäß § 41f Abs. 1 EnWG die Unterbrechung Ihrer Versorgung an. Die voraussichtlichen \
         Kosten der Unterbrechung betragen {} EUR, die der Wiederherstellung {} EUR \
         (§ 41f Abs. 6 EnWG).",
        format_ct_as_eur(sperrkosten_ct),
        format_ct_as_eur(entsperrkosten_ct),
    )
}

/// The creditor as § 126b BGB's declarant, from the operator's own config.
fn absender(cfg: &AccountingdConfig) -> PartyView {
    let a = &cfg.creditor_address;
    PartyView {
        name: Some(
            cfg.creditor_name
                .clone()
                .unwrap_or_else(|| cfg.tenant.clone()),
        ),
        line1: match (a.street.as_deref(), a.building_number.as_deref()) {
            (Some(s), Some(n)) => Some(format!("{s} {n}")),
            (Some(s), None) => Some(s.to_owned()),
            (None, n) => n.map(str::to_owned),
        },
        post_code: a.post_code.clone(),
        city: a.town.clone(),
        country: a.country.clone(),
        ..PartyView::default()
    }
}

/// Build the page for one case.
///
/// # Errors
///
/// Propagates ledger and database errors. `Ok(None)` when `vertragd` cannot
/// resolve the recipient: an unaddressed Mahnung is not Textform.
pub async fn build_view(
    ledger: &PgLedger,
    vertragd: &VertragdClient,
    cfg: &AccountingdConfig,
    case: &PendingMahnung,
) -> anyhow::Result<Option<MahnungView>> {
    let Some(buyer) = vertragd.rechnungsempfaenger_by_malo(&case.malo_id).await? else {
        return Ok(None);
    };

    // The live open items, not the total the case opened with: a customer who
    // has part-paid since is owed a list that says so.
    let open = ledger
        .open_receivables(&case.lf_mp_id, &case.malo_id)
        .await?;
    let mut posten = Vec::new();
    let mut posten_total = 0i64;
    let mut gebuehr_total = 0i64;
    let mut zinsen_total = 0i64;
    for r in &open {
        match r.entry_type.as_deref() {
            // Verzugsschaden is demanded on its own lines: a reader has to be
            // able to see what is supply debt and what the dunning added.
            Some("MAHNGEBUEHR") => gebuehr_total += r.outstanding_ct,
            Some("VERZUGSZINSEN") => zinsen_total += r.outstanding_ct,
            _ => {
                posten_total += r.outstanding_ct;
                posten.push(MahnPosten {
                    rechnungsnummer: r.document.clone().unwrap_or_else(|| r.entry_id.to_string()),
                    rechnungsdatum: r.booking_date.to_string(),
                    faellig_am: r.booking_date.to_string(),
                    offener_betrag: format_ct_as_eur(r.outstanding_ct),
                });
            }
        }
    }
    posten.sort_by(|a, b| a.rechnungsdatum.cmp(&b.rechnungsdatum));

    let stufe = u8::try_from(case.stufe).unwrap_or(1);
    let gesamt = posten_total + gebuehr_total + zinsen_total;
    let threat = case.sperrandrohung_at.is_some();

    Ok(Some(MahnungView {
        stufe,
        datum: OffsetDateTime::now_utc().date().to_string(),
        zahlungsfrist: case.due_date.to_string(),
        absender: absender(cfg),
        empfaenger: PartyView {
            name: buyer.name,
            vat_id: buyer.vat_id,
            line1: buyer.line1,
            post_code: buyer.post_code,
            city: buyer.city,
            country: buyer.country,
            email: buyer.email,
            ..PartyView::default()
        },
        posten,
        mahngebuehr: (gebuehr_total > 0).then(|| format_ct_as_eur(gebuehr_total)),
        verzugszinsen: (zinsen_total > 0).then(|| format_ct_as_eur(zinsen_total)),
        zins_grundlage: (zinsen_total > 0).then(|| ZINS_GRUNDLAGE_ABS1.to_owned()),
        gesamtforderung: format_ct_as_eur(gesamt),
        iban: cfg.creditor_iban.clone(),
        sperrandrohung: threat.then(|| {
            sperrandrohung_text(
                cfg.sperrkosten_ct.unwrap_or(0),
                cfg.entsperrkosten_ct.unwrap_or(0),
            )
        }),
        geplantes_sperrdatum: threat
            .then(|| case.geplantes_sperrdatum.map(|d| d.to_string()))
            .flatten(),
    }))
}

/// What one sweep did.
#[derive(Debug, Default, Clone, Copy)]
pub struct MahnungSummary {
    pub issued: u32,
    /// Cases `vertragd` could not resolve a recipient for. An unaddressable
    /// Mahnung is not sent, and the count is the cue that master data is
    /// missing.
    pub unaddressable: u32,
    pub errors: u32,
}

/// Issue a document for every unresolved dunning case that has none.
///
/// Idempotent twice over — the case is stamped with its `dokument_id`, and
/// outputd keys the document on the case id — so a crash between the render and
/// the stamp cannot send a second notice. A duplicate Mahnung is a second
/// statutory demand with its own payment deadline.
///
/// # Errors
///
/// Propagates database errors from the candidate scan; a failure on one case is
/// counted and logged, and the sweep continues.
pub async fn issue_pending(
    pool: &PgPool,
    ledger: &PgLedger,
    outputd: &OutputdClient,
    vertragd: &VertragdClient,
    cfg: &Arc<AccountingdConfig>,
) -> anyhow::Result<MahnungSummary> {
    let mut summary = MahnungSummary::default();
    for case in crate::pg::list_undocumented_dunning_cases(pool, &cfg.tenant, 200).await? {
        let view = match build_view(ledger, vertragd, cfg, &case).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                summary.unaddressable += 1;
                tracing::warn!(
                    case_id = %case.case_id, malo_id = %case.malo_id,
                    "accountingd: Mahnung not issued — vertragd has no customer for this \
                     Marktlokation, so the notice cannot be addressed (§ 126b BGB names the \
                     recipient). No document was created."
                );
                continue;
            }
            Err(e) => {
                summary.errors += 1;
                tracing::warn!(case_id = %case.case_id, error = %e, "accountingd: Mahnung view failed");
                continue;
            }
        };
        // § 41f Abs. 5 requires the *Ankündigung* to be brieflich; a Mahnung is
        // not that notice, so the portal alone is lawful Textform and the
        // letter is added where an address is on file.
        let has_address = view.empfaenger.post_code.is_some() && view.empfaenger.city.is_some();
        let mut channels = vec!["PORTAL".to_owned()];
        if view.empfaenger.email.is_some() {
            channels.push("EMAIL".to_owned());
        }
        if has_address {
            channels.push("POST".to_owned());
        }
        let recipient = Recipient {
            name: view.empfaenger.name.clone(),
            email: view.empfaenger.email.clone(),
            address: Some(serde_json::json!({
                "name":      view.empfaenger.name,
                "line1":     view.empfaenger.line1,
                "post_code": view.empfaenger.post_code,
                "city":      view.empfaenger.city,
                "country":   view.empfaenger.country,
            })),
        };
        let request = IssueDocumentRequest {
            view: serde_json::to_value(&view)?,
            subject_ref: case.case_id.to_string(),
            malo_id: Some(case.malo_id.clone()),
            kunden_nr: case.kunden_nr.clone(),
            recipient,
            channels,
            date: OffsetDateTime::now_utc().date(),
            ident: case.case_id.to_string(),
        };
        match outputd.issue_mahnung(&request).await {
            Ok(issued) => {
                if let Err(e) = crate::pg::record_dunning_document(
                    pool,
                    &cfg.tenant,
                    case.case_id,
                    issued.document_id,
                )
                .await
                {
                    tracing::warn!(case_id = %case.case_id, error = %e, "accountingd: could not stamp the Mahnung document id");
                }
                summary.issued += 1;
                tracing::info!(
                    case_id = %case.case_id, malo_id = %case.malo_id, stufe = case.stufe,
                    document_id = %issued.document_id,
                    "accountingd: Mahnung issued and queued for delivery"
                );
            }
            Err(e) => {
                summary.errors += 1;
                tracing::warn!(case_id = %case.case_id, error = %e, "accountingd: Mahnung render failed");
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::MahnungView;

    /// The view this service sends and the one outputd's gate proves templates
    /// against are the same contract.
    ///
    /// It is duplicated across an HTTP boundary, so drift is the failure mode
    /// worth guarding: a renamed field gives templates that pass the publish
    /// gate and render blank in production. A purely additive field on
    /// outputd's side is safe — a template that does not print it renders the
    /// same page.
    #[test]
    fn the_view_matches_outputds_specimen() {
        // outputd's `document::mahnung::specimen()`, serialised. Verbatim
        // rather than imported: importing it would make accountingd depend on
        // outputd, a crate dependency between two services.
        let specimen = serde_json::json!({
            "stufe": 3,
            "datum": "2026-03-01",
            "zahlungsfrist": "2026-03-15",
            "absender": {
                "name": "Stadtwerke Musterstadt GmbH",
                "vat_id": "DE123456789",
                "tax_number": null,
                "line1": "Musterstraße 1",
                "post_code": "12345",
                "city": "Musterstadt",
                "country": "DE",
                "contact_name": "Forderungsmanagement",
                "phone": "0800 1234567",
                "email": "mahnung@stadtwerke-musterstadt.example"
            },
            "empfaenger": {
                "name": "Erika Mustermann-Übelacker",
                "vat_id": null,
                "tax_number": null,
                "line1": "Beispielweg 7",
                "post_code": "10115",
                "city": "Berlin",
                "country": "DE",
                "contact_name": null,
                "phone": null,
                "email": null
            },
            "posten": [{
                "rechnungsnummer": "R-2025-000871",
                "rechnungsdatum": "2025-12-31",
                "faellig_am": "2026-01-14",
                "offener_betrag": "383.17"
            }],
            "mahngebuehr": "5.00",
            "verzugszinsen": "7.83",
            "zins_grundlage": "5 Prozentpunkte über dem Basiszinssatz (§ 288 Abs. 1 BGB)",
            "gesamtforderung": "523.40",
            "iban": "DE89370400440532013000",
            "sperrandrohung": "…",
            "geplantes_sperrdatum": "2026-04-01"
        });
        let view: MahnungView =
            serde_json::from_value(specimen.clone()).expect("outputd's specimen fits this view");
        let round_tripped = serde_json::to_value(&view).expect("serialise");
        assert_eq!(
            round_tripped, specimen,
            "the field set must match outputd's page contract exactly"
        );
    }
}
