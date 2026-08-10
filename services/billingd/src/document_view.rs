//! billingd's copy of the outputd template view — the render-boundary contract.
//!
//! outputd renders documents from a JSON view; **outputd's
//! `document::view::DocumentView` is the normative definition** of that view,
//! because it is what the publish gate proves templates against. This module is
//! billingd's client-side copy: the projection from the stored
//! [`en16931::Invoice`] onto the JSON that crosses the HTTP boundary.
//!
//! Two copies is the mako idiom for wire structs at service boundaries (the
//! same way CloudEvents payloads are duplicated at each end): billingd must not
//! link outputd's crate — that would drag the whole Typst engine back in and
//! undo the extraction. Drift is caught on both sides: outputd's gate specimen
//! pins what templates consume, and `tests/einvoice_render.rs` pins what this
//! projection emits.
//!
//! # Numbers
//!
//! Amounts are decimal strings, not floats. The strings keep the scale their
//! business term carries — `InvoiceAmount` always prints two decimals, a
//! quantity or a VAT rate prints its own — and the template pads, never
//! truncates.

use serde::Serialize;

/// One party as it appears on the document (BG-4 seller / BG-7 buyer).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PartyView {
    /// BT-27 / BT-44 — registered name.
    pub name: Option<String>,
    /// BT-31 / BT-48 — VAT identifier, when the party has one.
    pub vat_id: Option<String>,
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
/// `tests/einvoice_render.rs::the_view_may_omit_document_level_allowances_only_while_there_are_none`
/// is the tripwire that fails the day it stops holding.
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
