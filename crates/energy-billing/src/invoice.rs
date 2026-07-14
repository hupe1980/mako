//! `Invoice` — the aggregate root of every billing run.
//!
//! Collects all `BillingPosition` items from the `BillingEngine` providers,
//! computes totals, and can serialise to BO4E-compatible `Rechnung` JSON.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;

use crate::context::BillingContext;
use crate::position::{BillingPosition, PositionCategory};

/// A completed invoice — the immutable result of `BillingEngine::bill()`.
///
/// ## Invariant
///
/// `brutto_eur == netto_eur + mwst_eur` (to within 0.001 EUR rounding tolerance).
///
/// ## BO4E output
///
/// Call [`Invoice::to_rechnung_json`] to produce a BO4E-compatible `Rechnung`
/// JSONB suitable for ingestion by `accountingd`.
///
/// ## Sign convention
///
/// - `netto_eur > 0` → customer owes the Lieferant (debit invoice)
/// - `netto_eur < 0` → Lieferant owes the customer (credit note / Gutschrift)
/// - `mwst_eur` always has the same sign as `netto_eur`
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct Invoice {
    /// Billing metadata (period, IDs, invoice type, rates).
    pub context: BillingContext,

    /// All positions in declaration order.
    ///
    /// Debit positions have positive `net_eur`; credit positions have negative.
    pub positions: Vec<BillingPosition>,

    /// Total net amount in EUR (Nettobetrag = commodity + grid + levies).
    ///
    /// This is the German Nettobetrag: it **includes** statutory per-unit levies
    /// (Stromsteuer, Energiesteuer, BEHG) but excludes MwSt.
    pub netto_eur: Decimal,

    /// MwSt amount in EUR.
    pub mwst_eur: Decimal,

    /// Brutto total in EUR (Netto + MwSt).
    pub brutto_eur: Decimal,
}

impl Invoice {
    /// Assemble an `Invoice` from a flat list of positions.
    ///
    /// Separates Tax positions from all others:
    /// - `netto_eur` = sum of non-Tax positions
    /// - `mwst_eur`  = sum of Tax positions
    /// - `brutto_eur` = netto + mwst
    #[must_use]
    pub fn from_positions(context: BillingContext, positions: Vec<BillingPosition>) -> Self {
        let netto_eur: Decimal = positions
            .iter()
            .filter(|p| p.category != PositionCategory::Tax)
            .map(|p| p.net_eur)
            .sum();
        let mwst_eur: Decimal = positions
            .iter()
            .filter(|p| p.category == PositionCategory::Tax)
            .map(|p| p.net_eur)
            .sum();
        let brutto_eur = netto_eur + mwst_eur;
        Self {
            context,
            positions,
            netto_eur,
            mwst_eur,
            brutto_eur,
        }
    }

    /// Sum of `net_eur` for positions carrying the given tag.
    #[must_use]
    pub fn total_by_tag(&self, tag: &str) -> Decimal {
        BillingPosition::total_by_tag(&self.positions, tag)
    }

    /// Positions carrying the given tag.
    pub fn positions_by_tag<'a>(
        &'a self,
        tag: &'a str,
    ) -> impl Iterator<Item = &'a BillingPosition> {
        self.positions.iter().filter(move |p| p.has_tag(tag))
    }

    /// Validate the arithmetic invariant: `netto + mwst == brutto`.
    ///
    /// Panics with a diagnostic if the invariant is violated (tolerance: 0.001 EUR).
    /// Use in tests and `debug_assert!` blocks.
    pub fn assert_valid(&self) {
        let expected = self.netto_eur + self.mwst_eur;
        let diff = (self.brutto_eur - expected).abs();
        assert!(
            diff < dec!(0.001),
            "Invoice invariant violated: netto {:.5} + mwst {:.5} = {:.5} != brutto {:.5}",
            self.netto_eur,
            self.mwst_eur,
            expected,
            self.brutto_eur
        );
    }

    /// Produce a BO4E-compatible `Rechnung` JSONB for `accountingd` ingestion.
    ///
    /// ## Rechnungsdatum
    ///
    /// Set to `period_to` (last day of the billing period). The library is pure
    /// and has no concept of "today". Callers that need a different issue date
    /// should mutate the returned JSON.
    ///
    /// ## Zahlungsziel
    ///
    /// `period_to + 14 days` — standard German energy retail practice.
    /// Override in the returned JSON for contract-specific payment terms.
    #[must_use]
    pub fn to_rechnung_json(&self) -> serde_json::Value {
        let ctx = &self.context;
        let pos_json: Vec<serde_json::Value> = self
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                serde_json::json!({
                    "_typ": "RECHNUNGSPOSITION",
                    "positionsnummer": i + 1,
                    "positionstext": p.description,
                    "rechtlicheGrundlage": p.legal_basis,
                    "positionsMenge": {
                        "_typ": "MENGE",
                        "wert": p.quantity.to_string(),
                        "einheit": p.unit
                    },
                    "einzelpreis": {
                        "_typ": "PREIS",
                        "wert": p.unit_price_eur.to_string(),
                        "einheit": "EUR"
                    },
                    "gesamtpreis": {
                        "_typ": "BETRAG",
                        "wert": p.net_eur.to_string(),
                        "waehrung": "EUR"
                    },
                    "positionstyp": p.tags.first().map(String::as_str).unwrap_or("POSITION"),
                    "kategorie": format!("{:?}", p.category),
                })
            })
            .collect();

        let zahlungsziel = ctx.period_to + time::Duration::days(14);

        // Collect ZusatzAttribute from info positions tagged "gasqualitaet"
        let zusatz_attribute: Vec<serde_json::Value> = self
            .positions
            .iter()
            .filter(|p| p.has_tag("gasqualitaet") && p.category == PositionCategory::Info)
            .map(|p| {
                serde_json::json!({
                    "_typ": "ZUSATZ_ATTRIBUT",
                    "name": "gasqualitaet",
                    "wert": p.legal_basis.as_deref().unwrap_or("")
                })
            })
            .collect();

        serde_json::json!({
            "_typ": "RECHNUNG",
            "rechnungsnummer": ctx.rechnungsnummer,
            "rechnungsart": ctx.invoice_type.rechnungsart(),
            "rechnungsdatum": ctx.period_to.to_string(),  // deterministic: no now()
            "originalRechnungsId": ctx.invoice_type.original_invoice_id(),
            "marktlokationsId": ctx.malo_id,
            "herausgeber": {
                "_typ": "MARKTTEILNEHMER",
                "marktpartnercode": ctx.lf_mp_id
            },
            "vertragsId": ctx.contract_id,
            "rechnungsperiode": {
                "_typ": "ZEITRAUM",
                "startdatum": ctx.period_from.to_string(),
                "enddatum": ctx.period_to.to_string()
            },
            "rechnungspositionen": pos_json,
            "zusatzAttribute": if zusatz_attribute.is_empty() { serde_json::Value::Null } else { serde_json::json!(zusatz_attribute) },
            "gesamtnetto":  { "_typ": "BETRAG", "wert": self.netto_eur.to_string(),  "waehrung": "EUR" },
            "gesamtsteuer": { "_typ": "BETRAG", "wert": self.mwst_eur.to_string(),   "waehrung": "EUR" },
            "gesamtbrutto": { "_typ": "BETRAG", "wert": self.brutto_eur.to_string(), "waehrung": "EUR" },
            "rechnungsempfaenger": {
                "_typ": "MARKTTEILNEHMER",
                "externeKundenId": ctx.malo_id
            },
            "zahlungsziel": zahlungsziel.to_string()
        })
    }
}
