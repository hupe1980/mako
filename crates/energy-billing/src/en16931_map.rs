//! EN 16931 semantic-model bridge — behind the `en16931` feature.
//!
//! Maps a finished [`Invoice`] to an [`en16931::Invoice`], the syntax-neutral
//! semantic model that [`en16931-formats`](https://docs.rs/en16931-formats)
//! renders to XRechnung/CII and PEPPOL UBL. The mapping lives here —
//! where every position still carries its own amount, VAT category and rate —
//! rather than by re-parsing the BO4E `Rechnung`, so a mixed-rate invoice keeps
//! a correct **per-line** VAT (BT-151/BT-152) that reconciles with the BG-23
//! breakdown. BO4E stays the accounting/GoBD representation; EN 16931 is the
//! e-invoicing one.
//!
//! The caller supplies the seller and buyer `Party` — a calculation engine has
//! no business knowing postal addresses or VAT identifiers. Everything the
//! standard derives from the arithmetic (lines, BG-23, BG-22 totals) comes from
//! the invoice.

use rust_decimal::Decimal;

use en16931::amount::{InvoiceAmount, UnitPriceAmount};
use en16931::date::Date;
use en16931::invoice::{
    Code, Invoice as EnInvoice, InvoiceLine, Item, LineVat, Party, Period, PriceDetails,
};
use en16931::numeric::{Percentage, Quantity};

use crate::invoice::{Invoice, VatCategory};
use crate::position::PositionCategory;
use crate::rates::RoundMoney;

/// The XRechnung 3.0 specification identifier (BT-24).
///
/// The namespace changed **at 3.0**, when XRechnung governance moved from XÖV to
/// XStandards Einkauf: 2.x used `urn:xoev-de:kosit:standard:xrechnung_2.3`, and
/// 3.0 uses `urn:xeinkauf.de:kosit:xrechnung_3.0`. Pairing the 2.x namespace
/// with a `_3.0` version produces an identifier that matches *no* published
/// XRechnung version, so BR-DE-21 fails and a receiving validator picks the
/// wrong profile or rejects outright.
pub const XRECHNUNG_SPEC_ID: &str =
    "urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0";

/// The plain EN 16931 core specification identifier (BT-24).
pub const EN16931_SPEC_ID: &str = "urn:cen.eu:en16931:2017";

/// BT-120 exemption reason for the §13b UStG reverse-charge (`AE`) breakdown —
/// the statement § 14a Abs. 5 UStG requires on a reverse-charge invoice.
pub const SECT13B_EXEMPTION_REASON: &str =
    "Steuerschuldnerschaft des Leistungsempfängers (§13b UStG)";

/// BT-121 VATEX code for the reverse-charge category.
pub const VATEX_REVERSE_CHARGE: &str = "VATEX-EU-AE";

/// BT-97 reason on the document-level allowance a **Restrechnung** deducts each
/// advance with (§ 14 Abs. 5 Satz 2 UStG).
pub const ABSCHLAG_ALLOWANCE_REASON: &str = "Abzug erhaltener Abschlagszahlung";

/// UN/ECE Rec 20 unit code (BT-130) for a mako unit-of-measure label.
fn unece_unit(unit: &str) -> &'static str {
    match unit {
        "kWh" | "kWh_th" | "kWh_Hs" => "KWH",
        "kW" => "KWT",
        "m³" | "m3" => "MTQ",
        "Tage" | "Tag" | "d" => "DAY",
        "Monat" => "MON",
        "Jahr" => "ANN",
        // Lump-sum / dimensionless (EUR line, Bonus, Pauschale): "one".
        _ => "C62",
    }
}

impl Invoice {
    /// Map this invoice to the [`en16931::Invoice`] semantic model.
    ///
    /// `spec_id` selects the profile identifier (BT-24) — use
    /// [`XRECHNUNG_SPEC_ID`] for a German B2G/XRechnung document or
    /// [`EN16931_SPEC_ID`] for the plain core. `seller`/`buyer` carry the terms
    /// the standard requires and the engine cannot know (name, postal address,
    /// VAT id, electronic address).
    ///
    /// Every billable net position becomes a BG-25 invoice line carrying its own
    /// BT-151/BT-152 VAT; `Tax`, `Abschlag` and `Info` positions do not (the VAT
    /// is the BG-23 breakdown, advances are BT-113, and info lines are not
    /// billable). A reversal/credit-note invoice is emitted as document type 381
    /// with positive amounts (the document kind conveys the sign).
    ///
    /// # Errors
    ///
    /// [`EngineError::ValidationBlocked`](crate::EngineError::ValidationBlocked) when the invoice mixes a
    /// **not-subject-to-VAT** line (category `O` — a hoheitliche Gebühr) with
    /// any other VAT category. EN 16931 **BR-O-11 … BR-O-14** make `O`
    /// exclusive to its document, so such an invoice has no valid rendering:
    /// producing one anyway hands the recipient a file their schematron
    /// rejects, with the failure surfacing days later at the wrong end.
    pub fn to_en16931(
        &self,
        spec_id: &str,
        seller: Party,
        buyer: Party,
    ) -> Result<EnInvoice, crate::EngineError> {
        /// A `time::Date` as EN 16931 spells one. `None` only for a date outside
        /// the four-digit calendar year, which a billing period cannot be.
        fn calendar_date(d: time::Date) -> Option<Date> {
            Date::new(d.year(), d.month() as u8, d.day()).ok()
        }

        let is_credit = self.context.invoice_type.is_reversal()
            || matches!(self.context.invoice_type, crate::InvoiceType::CreditNote);
        let sign = if is_credit {
            Decimal::NEGATIVE_ONE
        } else {
            Decimal::ONE
        };
        let type_code = if is_credit { "381" } else { "380" };
        let default_rate = self.context.regulatory_rates.mwst_rate;

        // BR-O-11 … BR-O-14 — `O` may not share a document with any other
        // category. Checked before anything is built, so the caller gets the
        // reason rather than a file that fails validation downstream.
        let categories: std::collections::BTreeSet<VatCategory> = self
            .positions
            .iter()
            .filter(|p| {
                !matches!(
                    p.category,
                    PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                )
            })
            .map(|p| {
                crate::invoice::vat_category_of(
                    p,
                    p.applicable_tax_rate.unwrap_or(default_rate).normalize(),
                )
            })
            .collect();
        if categories.len() > 1 && categories.iter().any(|c| c.is_exclusive()) {
            return Err(crate::EngineError::ValidationBlocked {
                warnings: vec![crate::BillingWarning {
                    code: "EN16931_KATEGORIE_O_NICHT_KOMBINIERBAR",
                    severity: crate::WarningSeverity::Error,
                    message: format!(
                        "EN 16931 BR-O-11 ff.: eine nicht steuerbare Position (Kategorie O, \
                         z. B. eine öffentlich-rechtliche Abwassergebühr) darf nicht mit \
                         anderen Steuerkategorien auf einem Beleg stehen. Gefunden: {}",
                        categories
                            .iter()
                            .map(|c| c.code())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }],
            });
        }

        // BT-2 — the actual Ausstellungsdatum (§ 14 Abs. 4 Nr. 3 UStG), which
        // is the issue date the caller supplied and only falls back to the
        // period end when it has no clock.
        let issue = self.context.ausstellungsdatum();
        let issue_date = Date::new(issue.year(), issue.month() as u8, issue.day())
            .unwrap_or_else(|_| Date::new(2000, 1, 1).expect("epoch fallback is valid"));
        // BT-9 — § 40c Abs. 1 EnWG: due at the earliest two weeks after the
        // payment request reaches the customer, so measured from the issue date
        // and not from the period end. A due date also satisfies BR-CO-25 when
        // an amount is owed.
        let due = self.context.faelligkeitsdatum();
        let due_date = Date::new(due.year(), due.month() as u8, due.day())
            .unwrap_or_else(|_| Date::new(2000, 1, 15).expect("epoch fallback is valid"));

        let mut builder = EnInvoice::builder(
            spec_id,
            self.context.rechnungsnummer.clone(),
            issue_date,
            type_code,
            "EUR",
        )
        .seller(seller)
        .buyer(buyer)
        .due_date(due_date)
        // BG-14 — the billing period. Not optional in practice for a utility
        // invoice: § 14 Abs. 4 Nr. 6 UStG requires the Leistungszeitraum on the
        // document, and XRechnung's BR-DE-TMP-32 requires BT-72, BG-14 or a
        // period on every line. Every `BillingContext` carries one, so this is
        // free — it was simply never mapped, which left the term absent from the
        // semantic model and therefore from every syntax rendered out of it,
        // including the period line on the PDF.
        .invoicing_period(Period {
            start: calendar_date(self.context.period_from()),
            end: calendar_date(self.context.period_to()),
        });
        if is_credit {
            builder = builder.credit_note();
        }

        // ── BG-25 lines: one per billable net position, its own BT-151/152 VAT ──
        // The line net is rounded to 2 dp; `en16931::reconcile` then derives the
        // BG-23 breakdown and BG-22 totals from the lines (grouping per the
        // category `-08`/`-01` rules and netting the advances against the gross),
        // so BR-CO-10..16 and the BR-S/BR-Z family reconcile by construction — the
        // crate owns the reconciliation instead of us re-deriving it.
        let mut line_no = 0u32;
        let mut has_reverse_charge = false;
        for p in &self.positions {
            if matches!(
                p.category,
                PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
            ) {
                continue;
            }
            line_no += 1;
            let rate = p.applicable_tax_rate.unwrap_or(default_rate).normalize();
            // Same categorisation as `tax_subtotals_of`: a §13b reverse-charge
            // supply is `AE`, never `Z` — both carry rate 0, but only `AE`
            // states that the recipient owes the VAT.
            let cat = crate::invoice::vat_category_of(p, rate);
            if cat == VatCategory::ReverseCharge {
                has_reverse_charge = true;
            }
            let cat = cat.code();
            let pct = (rate * Decimal::ONE_HUNDRED).normalize();
            let net = (p.net_eur * sign).round_kfm(2);
            builder = builder.line(InvoiceLine {
                id: line_no.to_string(),
                note: None,
                order_line_reference: None,
                accounting_reference: None,
                object_identifier: None,
                quantity: Quantity::from(p.quantity),
                unit_code: Code::from(unece_unit(&p.unit)),
                net_amount: amount(net),
                period: None,
                allowances: Vec::new(),
                charges: Vec::new(),
                price: PriceDetails {
                    net_price: UnitPriceAmount::from((p.unit_price_eur * sign).abs()),
                    ..Default::default()
                },
                vat: LineVat {
                    category: Code::from(cat),
                    rate: Some(Percentage::from(pct)),
                },
                item: Item {
                    name: Some(p.description.clone()),
                    ..Default::default()
                },
            });
        }

        // ── The advances, in the form the context asked for ───────────────────
        //
        // Both forms are lawful and the customer pays the same amount; they
        // differ in what the document *shows*, which is what
        // `BillingContext::settlement_form` selects. It selected nothing until
        // now: the field was declared, documented in the README, and read
        // nowhere, so every settling invoice went out as an Endrechnung
        // regardless — including the e-invoices for which the BMF recommends
        // the other form.
        let advances = self.advance_payments()?;
        let restrechnung = self.context.settlement_form == crate::SettlementForm::Restrechnung
            && !advances.is_empty()
            && !is_credit;

        if restrechnung {
            // **Restrechnung** (BMF-Schreiben v. 15.10.2024, Rn. 48): invoice
            // the remainder. EN 16931's core profiles have nowhere to carry
            // per-advance tax, so each advance rides as a BG-20 document-level
            // allowance carrying *its own* VAT category and rate; the reconciler
            // then derives BG-23 as `lines − allowances`, which is the residual.
            // Unlike a flat BT-113 that stays right when the advances were
            // invoiced at a different rate from the settlement.
            //
            // One group per (category, rate), not one per advance: BR-CO-*
            // reconciles on the group, and a monthly Abschlagsplan would
            // otherwise put eleven identical allowances on the page.
            let mut groups: std::collections::BTreeMap<(&'static str, String), Decimal> =
                std::collections::BTreeMap::new();
            for advance in &advances {
                for entry in advance.tax() {
                    let (cat, rate) = entry.group_key();
                    *groups
                        .entry((cat.code(), rate.to_string()))
                        .or_insert(Decimal::ZERO) += entry.taxable_base.into_decimal();
                }
            }
            for ((cat, rate), base) in groups {
                let rate: Decimal = rate.parse().unwrap_or(Decimal::ZERO);
                builder = builder.allowance(en16931::invoice::DocumentAllowanceCharge {
                    amount: amount(base.round_kfm(2)),
                    base_amount: None,
                    percentage: None,
                    vat: LineVat {
                        category: Code::from(cat),
                        rate: Some(Percentage::from((rate * Decimal::ONE_HUNDRED).normalize())),
                    },
                    reason: Some(ABSCHLAG_ALLOWANCE_REASON.to_owned()),
                    // UNCL 5189 code 95 — "Discount". EN 16931 has no advance-
                    // payment allowance reason code; the reason text carries the
                    // meaning and the code keeps BR-33/BR-38 satisfied.
                    reason_code: Some(Code::from("95")),
                });
            }
        }

        // Derive BG-23 + BG-22 from the lines. On an Endrechnung the advance
        // payments (BT-113) net against the gross to give the amount due
        // (BT-115); on a Restrechnung the allowances have already reduced the
        // base, so nothing is stated as paid.
        let mut inv = builder.build();
        let paid = (self.abschlag_total_eur * sign).round_kfm(2);
        let mut rec = en16931::reconcile::Reconciler::new();
        if has_reverse_charge {
            // BR-AE-10: the AE breakdown must carry an exemption reason.
            rec = rec.exemption(
                "AE",
                Some(SECT13B_EXEMPTION_REASON),
                Some(VATEX_REVERSE_CHARGE),
            );
        }
        if !restrechnung && !paid.is_zero() {
            rec = rec.paid(amount(paid));
        }
        let _ = rec.apply(&mut inv);
        Ok(inv)
    }
}

/// Convert a 2-dp `Decimal` to an `InvoiceAmount`, saturating on the (unreachable
/// for real invoices) out-of-range case rather than panicking in a render path.
fn amount(d: Decimal) -> InvoiceAmount {
    InvoiceAmount::try_from(d).unwrap_or_else(|_| InvoiceAmount::from_minor_units(0))
}
