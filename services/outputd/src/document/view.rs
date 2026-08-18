//! The typed view a document template renders from.
//!
//! # Why a projection and not the model
//!
//! An operator owns the invoice *layout* — logo, Briefkopf, where the
//! Pflichtangaben sit — so the template is their file, not ours. Handing it
//! [`en16931::Invoice`] directly would make every field of the semantic model a
//! public API that no template may outgrow: rename one and somebody's invoice
//! stops rendering at the next release.
//!
//! [`DocumentView`] is the contract instead. It is deliberately small, flat and
//! named for what a reader sees rather than for the BT/BG codes underneath, and
//! it carries the term identifiers in its documentation so an operator can find
//! the legal basis for a field without reading EN 16931.
//!
//! It is also **total in the fields it declares**: every value is copied from
//! the model, so a page and the XML embedded beside it cannot disagree. See the
//! [module docs](super) for the layering this sits in.
//!
//! # Numbers
//!
//! Amounts are decimal strings, not floats: an invoice total that survives a
//! round trip through `f64` is not an invoice total. The strings keep the scale
//! their business term carries — `InvoiceAmount` always prints two decimals, a
//! quantity or a VAT rate prints its own — so a template must *pad* a value to
//! the precision it wants to show and must never truncate one. The reference
//! template's `money` and `num` helpers do exactly that.

use serde::Serialize;

/// One party as it appears on the document (BG-4 seller / BG-7 buyer).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PartyView {
    /// BT-27 / BT-44 — registered name.
    pub name: Option<String>,
    /// BT-31 / BT-48 — VAT identifier, when the party has one.
    pub vat_id: Option<String>,
    /// BT-32 — the seller's Steuernummer. Seller side only; EN 16931 gives the
    /// buyer no equivalent term. § 14 Abs. 4 Nr. 2 UStG requires **one of**
    /// this and [`Self::vat_id`] on every Rechnung, and the publish gate
    /// checks that one of them reaches the page.
    pub tax_number: Option<String>,
    /// BT-35 / BT-50 — street and number.
    pub line1: Option<String>,
    /// BT-38 / BT-53 — post code.
    pub post_code: Option<String>,
    /// BT-37 / BT-52 — city.
    pub city: Option<String>,
    /// BT-40 / BT-55 — ISO 3166-1 alpha-2 country code.
    pub country: Option<String>,
    /// BT-41/42/43 — contact point, seller side only in practice.
    pub contact_name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
}

/// One invoice line (BG-25).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LineView {
    /// BT-126 — line identifier, as printed.
    pub id: String,
    /// BT-153 — item name.
    pub name: Option<String>,
    /// BT-154 — item description, when it differs from the name.
    pub description: Option<String>,
    /// BT-129 — invoiced quantity.
    pub quantity: String,
    /// BT-130 — UN/ECE Rec 20 unit code (`KWH`, `MTQ`, …).
    pub unit: String,
    /// BT-146 — net unit price.
    pub unit_price: String,
    /// BT-131 — line net amount.
    pub net_amount: String,
    /// BT-151 — VAT category code (`S`, `E`, `Z`, …).
    pub vat_category: String,
    /// BT-152 — VAT rate in percent, absent for the categories that carry none.
    pub vat_rate: Option<String>,
}

/// One VAT breakdown entry (BG-23) — one per rate on the document.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VatView {
    /// BT-118 — category code.
    pub category: String,
    /// BT-119 — rate in percent.
    pub rate: Option<String>,
    /// BT-116 — taxable base for this rate.
    pub taxable_amount: String,
    /// BT-117 — VAT for this rate.
    pub tax_amount: String,
    /// BT-120 — why VAT is not charged, for the exempt categories.
    pub exemption_reason: Option<String>,
}

/// Document totals (BG-22).
///
/// There is no BG-20/BG-21 here — no document-level allowance or charge —
/// because `energy_billing` never emits one: every discount in this engine is a
/// negative *line*, so BT-106 and BT-109 are always equal. That is an invariant
/// of the mapping rather than of EN 16931, and
/// billingd's
/// `tests/einvoice_render.rs::the_view_may_omit_document_level_allowances_only_while_there_are_none`
/// is the tripwire that fails the day it stops holding — it lives with the
/// mapping it guards.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TotalsView {
    /// BT-106 — sum of line net amounts.
    pub line_total: String,
    /// BT-109 — total without VAT.
    pub taxable_total: String,
    /// BT-110 — total VAT.
    pub vat_total: Option<String>,
    /// BT-112 — total with VAT.
    pub gross_total: String,
    /// BT-113 — already paid.
    pub paid: Option<String>,
    /// BT-115 — amount due for payment.
    pub due: String,
}

/// Everything a template may render, and nothing else.
///
/// See the [module docs](self) for why this exists rather than the semantic
/// model itself.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocumentView {
    /// BT-1 — invoice number.
    pub number: Option<String>,
    /// BT-2 — issue date, ISO 8601.
    pub issue_date: Option<String>,
    /// BT-9 — payment due date, ISO 8601.
    pub due_date: Option<String>,
    /// BT-5 — currency code.
    pub currency: Option<String>,
    /// BT-10 — buyer reference (Leitweg-ID on a B2G document).
    pub buyer_reference: Option<String>,
    /// BT-20 — payment terms in words.
    pub payment_terms: Option<String>,
    /// BT-73/74 — the billed period, ISO 8601.
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    /// BG-4.
    pub seller: PartyView,
    /// BG-7.
    pub buyer: PartyView,
    /// BG-25, in document order.
    pub lines: Vec<LineView>,
    /// BG-23, one entry per VAT rate.
    pub vat_breakdown: Vec<VatView>,
    /// BG-22.
    pub totals: TotalsView,
    /// BT-22 — free-text notes, in document order.
    pub notes: Vec<String>,
}

fn party(p: &en16931::invoice::Party) -> PartyView {
    PartyView {
        name: p.name.clone(),
        vat_id: p.vat_identifier.clone(),
        tax_number: p.tax_registration.clone(),
        line1: p.address.line1.clone(),
        post_code: p.address.post_code.clone(),
        city: p.address.city.clone(),
        country: p.address.country.as_ref().map(ToString::to_string),
        contact_name: p.contact.name.clone(),
        phone: p.contact.phone.clone(),
        email: p.contact.email.clone(),
    }
}

impl DocumentView {
    /// Project the semantic model onto the template contract.
    ///
    /// Total and lossless in the fields it declares: every value comes straight
    /// from the model, so a template and the embedded XML can never disagree
    /// about what the invoice says.
    #[must_use]
    pub fn of(inv: &en16931::Invoice) -> Self {
        Self {
            number: inv.number.clone(),
            issue_date: inv.issue_date.map(|d| d.to_string()),
            due_date: inv.due_date.map(|d| d.to_string()),
            currency: inv.currency.as_ref().map(ToString::to_string),
            buyer_reference: inv.buyer_reference.clone(),
            payment_terms: inv.payment_terms.clone(),
            period_start: inv
                .invoicing_period
                .as_ref()
                .and_then(|p| p.start)
                .map(|d| d.to_string()),
            period_end: inv
                .invoicing_period
                .as_ref()
                .and_then(|p| p.end)
                .map(|d| d.to_string()),
            seller: party(&inv.seller),
            buyer: party(&inv.buyer),
            lines: inv
                .lines
                .iter()
                .map(|l| LineView {
                    id: l.id.clone(),
                    name: l.item.name.clone(),
                    description: l.item.description.clone(),
                    quantity: l.quantity.to_string(),
                    unit: l.unit_code.to_string(),
                    unit_price: l.price.net_price.to_string(),
                    net_amount: l.net_amount.to_string(),
                    vat_category: l.vat.category.to_string(),
                    vat_rate: l.vat.rate.as_ref().map(ToString::to_string),
                })
                .collect(),
            vat_breakdown: inv
                .vat_breakdown
                .iter()
                .map(|v| VatView {
                    category: v.category.to_string(),
                    rate: v.rate.as_ref().map(ToString::to_string),
                    taxable_amount: v.taxable_amount.to_string(),
                    tax_amount: v.tax_amount.to_string(),
                    exemption_reason: v.exemption_reason.clone(),
                })
                .collect(),
            totals: TotalsView {
                line_total: inv.totals.line_total.to_string(),
                taxable_total: inv.totals.taxable_total.to_string(),
                vat_total: inv.totals.vat_total.as_ref().map(ToString::to_string),
                gross_total: inv.totals.gross_total.to_string(),
                paid: inv.totals.paid.as_ref().map(ToString::to_string),
                due: inv.totals.due.to_string(),
            },
            notes: inv.notes.iter().filter_map(|n| n.note.clone()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentView;

    /// The view must carry the invoice, not a lossy summary of it.
    ///
    /// An operator's layout renders from `DocumentView` and never from the
    /// semantic model, so anything missing here is a field no invoice can ever
    /// print. The specimen is deliberately awkward — two VAT rates, an exempt
    /// line, a credit line — so a breakdown loop that assumes one rate is
    /// caught here rather than by a customer.
    ///
    /// It lives with the projection it guards, and only here: a copy on the
    /// caller's side would test a second implementation of one contract, with
    /// the gate proving templates against this one and production feeding them
    /// the other.
    #[test]
    fn the_view_carries_what_an_invoice_must_print() {
        let model = crate::document::gate::specimen_invoice();
        let view = DocumentView::of(&model);

        // §14 Abs. 4 UStG Pflichtangaben the page cannot omit.
        assert!(view.number.is_some(), "BT-1 invoice number");
        assert!(view.issue_date.is_some(), "BT-2 issue date");
        assert!(view.seller.name.is_some(), "BT-27 seller name");
        assert!(view.buyer.name.is_some(), "BT-44 buyer name");
        // § 14 Abs. 4 Nr. 2 is a disjunction — one of the two must be printable.
        // Projecting only BT-31 made the Steuernummer unrenderable, which left a
        // § 19 UStG Kleinunternehmer with no lawful page at all.
        assert!(
            view.seller.vat_id.is_some() || view.seller.tax_number.is_some(),
            "BT-31 or BT-32 seller tax identifier",
        );
        assert_eq!(
            view.seller.tax_number, model.seller.tax_registration,
            "BT-32 is projected, not dropped",
        );

        // Every rate reaches the breakdown a template iterates. A single
        // blended rate on the page is exactly the defect the hand-rolled
        // renderer had.
        assert!(
            view.vat_breakdown.len() >= 2,
            "the mixed-rate specimen keeps its BG-23 entries: {:?}",
            view.vat_breakdown,
        );

        assert!(
            !view.lines.is_empty(),
            "an invoice with no lines prints nothing"
        );
        for l in &view.lines {
            assert!(!l.net_amount.is_empty() && !l.quantity.is_empty());
            assert!(!l.vat_category.is_empty(), "BT-151 per line");
        }

        // Amounts are decimal strings — a total that round-trips through f64 is
        // not a total.
        assert!(
            view.totals.gross_total.parse::<f64>().is_ok() && view.totals.gross_total.contains('.'),
            "totals are decimal strings: {}",
            view.totals.gross_total,
        );

        // The view is what a template consumes: it has to serialise.
        let json = serde_json::to_value(&view).expect("DocumentView serialises");
        assert!(json["totals"]["gross_total"].is_string());
        assert!(json["lines"].as_array().is_some_and(|l| !l.is_empty()));
    }

    /// The view must never become a second source of truth for the XML.
    ///
    /// It is a *projection*: every field is copied from the model, so the page
    /// and the embedded CII cannot disagree. This checks the pair most likely
    /// to drift — what the customer owes.
    #[test]
    fn the_view_agrees_with_the_model_it_projects() {
        let model = crate::document::gate::specimen_invoice();
        let view = DocumentView::of(&model);

        assert_eq!(
            view.totals.gross_total,
            model.totals.gross_total.to_string()
        );
        assert_eq!(view.totals.due, model.totals.due.to_string());
        assert_eq!(view.lines.len(), model.lines.len());
        assert_eq!(view.vat_breakdown.len(), model.vat_breakdown.len());
    }

    /// BG-14 reaches the page. billingd pins that the period reaches the model;
    /// this pins the other half of the same journey.
    #[test]
    fn the_billing_period_reaches_the_page() {
        let mut model = crate::document::gate::specimen_invoice();
        model.invoicing_period = Some(en16931::invoice::Period {
            start: Some(
                en16931::Date::try_from(time::macros::date!(2026 - 01 - 01)).expect("valid"),
            ),
            end: Some(en16931::Date::try_from(time::macros::date!(2026 - 01 - 31)).expect("valid")),
        });
        let view = DocumentView::of(&model);
        assert_eq!(view.period_start.as_deref(), Some("2026-01-01"));
        assert_eq!(view.period_end.as_deref(), Some("2026-01-31"));
    }
}
