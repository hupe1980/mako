//! `Invoice` — the aggregate root of every billing run.
//!
//! Collects all `BillingPosition` items from the `BillingEngine` providers,
//! computes totals, and can serialise to BO4E-compatible `Rechnung` JSON.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;

use crate::context::BillingContext;
use crate::position::{BillingPosition, BillingWarning, PositionCategory};

/// A completed invoice — the immutable result of `BillingEngine::bill()`.
///
/// ## Invariants
///
/// - `brutto_eur == netto_eur + mwst_eur` (within 0.001 EUR rounding tolerance)
/// - `zahlbetrag_eur == brutto_eur - abschlag_total_eur`
///
/// ## §40a EnWG — Kilowattstundenpreis
///
/// For electricity billing, call `kilowattstundenpreis_brutto_ct(kwh)` to obtain
/// the all-inclusive price per kWh required on every invoice.
///
/// ## Sign convention
///
/// - `netto_eur > 0` → customer owes the Lieferant (debit invoice)
/// - `netto_eur < 0` → Lieferant owes the customer (credit note / Gutschrift)
/// - `mwst_eur` always has the same sign as `netto_eur`
/// - `zahlbetrag_eur < 0` → refund due to customer (after Abschlag deduction)
///
/// ## Regulatory warnings
///
/// `warnings` contains all non-fatal compliance notes produced during billing.
/// Check for `WarningSeverity::Error` warnings before dispatching the invoice.
/// Error-severity warnings indicate definite regulatory issues that the operator
/// must resolve (e.g. §41b iMSys mismatch, §41 disclosure fields missing).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct Invoice {
    /// Billing metadata (period, IDs, invoice type, rates).
    pub context: BillingContext,

    /// All positions in declaration order.
    ///
    /// Debit positions have positive `net_eur`; credit positions have negative.
    /// `Abschlag` positions appear last (deducted from `zahlbetrag_eur` only).
    pub positions: Vec<BillingPosition>,

    /// Total net amount in EUR (Nettobetrag = commodity + grid + levies).
    ///
    /// This is the German Nettobetrag: it **includes** statutory per-unit levies
    /// (Stromsteuer, Energiesteuer, BEHG) but excludes MwSt.
    /// Does NOT include `Abschlag` deductions.
    pub netto_eur: Decimal,

    /// MwSt amount in EUR — the aggregate across every rate.
    ///
    /// For the EN16931 BG-23 breakdown use [`Invoice::tax_subtotals`]: a single
    /// aggregate cannot express an invoice that mixes rates.
    pub mwst_eur: Decimal,

    /// Brutto total in EUR (Netto + MwSt).
    ///
    /// Before subtracting advance payments. `Abschlag` deductions are in
    /// `zahlbetrag_eur`.
    pub brutto_eur: Decimal,

    /// Total of advance payments (Abschläge) deducted on this invoice.
    ///
    /// Non-zero only for `InvoiceType::Final` with `ctx.abschlage` populated.
    pub abschlag_total_eur: Decimal,

    /// Amount actually due / refundable after Abschlag deduction (§41 EnWG).
    ///
    /// `zahlbetrag_eur = brutto_eur - abschlag_total_eur`
    ///
    /// - Positive → customer owes this balance
    /// - Negative → Lieferant refunds this amount to the customer
    pub zahlbetrag_eur: Decimal,

    /// Billing run identifier (from `BillingContext.billing_run_id`).
    ///
    /// `None` when the context did not specify a run ID (e.g. preview calls).
    /// Propagated to the Rechnung JSON as a `ZusatzAttribut` for audit trail.
    pub billing_run_id: Option<String>,

    /// Non-fatal regulatory compliance warnings produced during billing.
    ///
    /// Check for [`WarningSeverity::Error`](crate::WarningSeverity) warnings
    /// before dispatching the invoice. Error-severity warnings indicate definite
    /// regulatory issues (e.g. §41b iMSys mismatch). Informational warnings are
    /// advisory only.
    ///
    /// These warnings are also emitted by [`BillingEngine::validate()`](crate::BillingEngine::validate)
    /// so operators can run a pre-flight check before committing to billing.
    pub warnings: Vec<BillingWarning>,
}

impl Invoice {
    /// The EN16931 BG-23 VAT breakdown — one entry per distinct rate.
    ///
    /// Derived rather than stored, so it cannot drift from the positions.
    /// `default_rate` applies to positions with no explicit
    /// `applicable_tax_rate`.
    #[must_use]
    pub fn tax_subtotals(&self, default_rate: Decimal) -> Vec<TaxSubtotal> {
        tax_subtotals_of(&self.positions, default_rate)
    }

    /// Assemble an `Invoice` from a flat list of positions and warnings.
    ///
    /// Separates Tax and Abschlag positions from all others:
    /// - `netto_eur` = sum of non-Tax, non-Abschlag positions
    /// - `mwst_eur`  = sum of Tax positions
    /// - `brutto_eur` = netto + mwst
    /// - `abschlag_total_eur` = sum of Abschlag positions
    /// - `zahlbetrag_eur` = brutto - abschlag_total_eur
    #[must_use]
    pub fn from_positions(
        context: BillingContext,
        positions: Vec<BillingPosition>,
        warnings: Vec<BillingWarning>,
    ) -> Self {
        let netto_eur: Decimal = positions
            .iter()
            .filter(|p| {
                p.category != PositionCategory::Tax && p.category != PositionCategory::Abschlag
            })
            .map(|p| p.net_eur)
            .sum();
        let mwst_eur: Decimal = positions
            .iter()
            .filter(|p| p.category == PositionCategory::Tax)
            .map(|p| p.net_eur)
            .sum();
        let brutto_eur = netto_eur + mwst_eur;
        let abschlag_total_eur: Decimal = positions
            .iter()
            .filter(|p| p.category == PositionCategory::Abschlag)
            .map(|p| p.net_eur.abs())
            .sum();
        let zahlbetrag_eur = brutto_eur - abschlag_total_eur;
        let billing_run_id = context.billing_run_id.clone();
        Self {
            context,
            positions,
            netto_eur,
            mwst_eur,
            brutto_eur,
            abschlag_total_eur,
            zahlbetrag_eur,
            billing_run_id,
            warnings,
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

    /// Validate the arithmetic invariants.
    ///
    /// Panics with a diagnostic if any invariant is violated (tolerance: 0.001 EUR).
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
        let zahlbetrag_expected = self.brutto_eur - self.abschlag_total_eur;
        let zdiff = (self.zahlbetrag_eur - zahlbetrag_expected).abs();
        assert!(
            zdiff < dec!(0.001),
            "Invoice invariant violated: zahlbetrag {:.5} != brutto {:.5} - abschlag {:.5}",
            self.zahlbetrag_eur,
            self.brutto_eur,
            self.abschlag_total_eur,
        );
    }

    /// §40a EnWG — all-inclusive Kilowattstundenpreis (ct/kWh) for display on invoice.
    ///
    /// §40a Abs. 1 EnWG requires that every electricity invoice shows the total
    /// all-inclusive price per kilowatt-hour (Gesamtbetrag je Kilowattstunde),
    /// inclusive of all energy charges, grid charges, levies, and taxes.
    ///
    /// Returns `None` when `total_kwh == 0` (avoid division by zero).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // 500 kWh total, brutto EUR 198.50 → 39.70 ct/kWh
    /// let ct = invoice.kilowattstundenpreis_brutto_ct(dec!(500)).unwrap();
    /// assert_eq!(ct.round_dp(2), dec!(39.70));
    /// ```
    #[must_use]
    pub fn kilowattstundenpreis_brutto_ct(&self, total_kwh: Decimal) -> Option<Decimal> {
        if total_kwh <= Decimal::ZERO {
            return None;
        }
        // brutto_eur / kWh × 100 → ct/kWh
        Some((self.brutto_eur / total_kwh * dec!(100)).round_dp(4))
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
        let mut zusatz_attribute: Vec<serde_json::Value> = self
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

        // §41 Abs. 1 Nr. 8 + §42 EnWG — Energiemix (fuel/source mix on invoice)
        if let Some(mix) = &ctx.energiemix {
            zusatz_attribute.push(serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "energiemix",
                "wert": mix
            }));
        }

        // §41 EnWG Abs. 1 Nr. 3 — Verbrauchshistorie summary as ZusatzAttribut
        if let Some(vh) = &ctx.verbrauchshistorie {
            if let Some(vj) = vh.vorjahr_kwh {
                zusatz_attribute.push(serde_json::json!({
                    "_typ": "ZUSATZ_ATTRIBUT",
                    "name": "verbrauchVorjahr",
                    "wert": vj.to_string()
                }));
            }
            if let Some(avg) = vh.bundesdurchschnitt_kwh {
                zusatz_attribute.push(serde_json::json!({
                    "_typ": "ZUSATZ_ATTRIBUT",
                    "name": "verbrauchBundesdurchschnitt",
                    "wert": avg.to_string()
                }));
            }
        }

        // Audit trail: billing run ID for ERP reconciliation and duplicate detection.
        if let Some(run_id) = &self.billing_run_id {
            zusatz_attribute.push(serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "billingRunId",
                "wert": run_id
            }));
        }

        // Customer category for downstream ERP routing and regulatory rule selection.
        {
            let kat = format!("{:?}", ctx.kundenkategorie);
            zusatz_attribute.push(serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "kundenkategorie",
                "wert": kat
            }));
        }

        // §40a EnWG Abs. 1 — Kilowattstundenpreis (all-inclusive total price per kWh).
        // Compute from brutto_eur / billable kWh. Use total eligible kWh from positions.
        let total_kwh_positions: Decimal = self
            .positions
            .iter()
            .filter(|p| {
                p.category == PositionCategory::Commodity
                    && (p.has_tag("strom") || p.has_tag("arbeitspreis"))
                    && p.unit == "kWh"
                    && p.quantity > Decimal::ZERO
            })
            .map(|p| p.quantity)
            .sum();
        let kilowattstundenpreis_ct = if total_kwh_positions > Decimal::ZERO {
            self.kilowattstundenpreis_brutto_ct(total_kwh_positions)
        } else {
            None
        };

        serde_json::json!({
            "_typ": "RECHNUNG",
            "rechnungsnummer": ctx.rechnungsnummer,
            "rechnungsart": ctx.invoice_type.rechnungsart(),
            "rechnungsdatum": ctx.period_to.to_string(),  // deterministic: no now()
            "originalRechnungsId": ctx.invoice_type.original_invoice_id(),
            "marktlokationsId": ctx.malo_id,
            "zaehlerIdLieferstelle": ctx.zaehler_id,
            "herausgeber": {
                "_typ": "MARKTTEILNEHMER",
                "marktpartnercode": ctx.lf_mp_id
            },
            // §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification (mandatory on energy invoices).
            // Identifies the network operator providing grid access at the delivery point.
            "netzbetreiber": ctx.nb_mp_id.as_deref().map(|id| serde_json::json!({
                "_typ": "MARKTTEILNEHMER",
                "marktpartnercode": id
            })),
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
            "abschlagTotal": if self.abschlag_total_eur > Decimal::ZERO { serde_json::json!({ "_typ": "BETRAG", "wert": self.abschlag_total_eur.to_string(), "waehrung": "EUR" }) } else { serde_json::Value::Null },
            "zahlbetrag": { "_typ": "BETRAG", "wert": self.zahlbetrag_eur.to_string(), "waehrung": "EUR" },
            // §40a EnWG Abs. 1 Satz 2 — Gesamtbetrag je Kilowattstunde (all-inclusive ct/kWh).
            // Only set when consumption positions exist (electricity commodity kWh known).
            "kilowattstundenpreisGesamt": kilowattstundenpreis_ct.map(|ct| serde_json::json!({
                "_typ": "PREIS",
                "wert": ct.to_string(),
                "einheit": "ct/kWh",
                "bezugswert": "KWH",
                "rechtlicheGrundlage": "§40a EnWG"
            })),
            // §40b EnWG — Strukturierte Preisvergleichsdaten für Vergleichsportale.
            // Enables price comparison portals (e.g. Verivox, Check24, BNetzA tools)
            // to ingest tariff structure from the invoice machine-readably.
            "preisvergleichsdaten": {
                "_typ": "PREISVERGLEICH",
                "grundpreisEurProJahr": self.positions.iter()
                    .filter(|p| p.has_tag("commodity") && p.unit == "Tage")
                    .map(|p| p.unit_price_eur * dec!(365))
                    .next()
                    .map(|eur_year| serde_json::json!({ "_typ": "BETRAG", "wert": eur_year.to_string(), "waehrung": "EUR" })),
                "arbeitspreisCtProKwh": self.positions.iter()
                    .filter(|p| (p.has_tag("strom") || p.has_tag("gas")) && p.category == crate::position::PositionCategory::Commodity && p.unit.starts_with("kWh"))
                    .map(|p| (p.unit_price_eur * dec!(100)).round_dp(4))
                    .next()
                    .map(|ct| ct.to_string()),
                "gesamtpreisCtProKwh": kilowattstundenpreis_ct.map(|ct| ct.to_string()),
                "rechtlicheGrundlage": "§40b EnWG"
            },
            "rechnungsempfaenger": {
                "_typ": "MARKTTEILNEHMER",
                "externeKundenId": ctx.malo_id
            },
            "zahlungsziel": zahlungsziel.to_string()
        })
    }

    /// Merge two invoices for adjacent billing periods (§41 EnWG Tarifwechsel).
    ///
    /// Positions from `self` appear first, then `other`. Totals are re-summed.
    /// Tax layers are **not** re-applied — each invoice was already taxed independently
    /// for its sub-period.
    ///
    /// Uses the context from `self` (billing period, IDs) for the merged invoice.
    /// `other.context.period_to` is used to update the effective period end.
    ///
    /// ## Equivalent to `billing::merge_period_documents`
    ///
    /// This function applies the same logic as `billing::merge_period_documents` but
    /// operates directly on `Invoice` without requiring a `BillingDocument` conversion.
    ///
    /// ## Use case — Tarifwechsel (price change mid-period)
    ///
    /// ```rust,ignore
    /// // Old tariff: Jan 1–14
    /// let inv_old = old_engine.bill(ctx_jan1_14, &quantities_old)?;
    /// // New tariff: Jan 15–31
    /// let inv_new = new_engine.bill(ctx_jan15_31, &quantities_new)?;
    /// // Combined January invoice
    /// let merged = inv_old.merge(inv_new);
    /// ```
    #[must_use]
    pub fn merge(self, other: Invoice) -> Invoice {
        let mut ctx = self.context;
        // Extend period to cover both sub-periods
        if other.context.period_to > ctx.period_to {
            ctx.period_to = other.context.period_to;
        }
        let mut positions = self.positions;
        positions.extend(other.positions);
        let mut all_warnings = self.warnings;
        all_warnings.extend(other.warnings);
        Invoice::from_positions(ctx, positions, all_warnings)
    }

    /// Proportionally split this invoice across N recipients.
    ///
    /// Uses `billing::proportional_split` for **penny-correct** arithmetic:
    /// the sum of all recipient totals equals `self.brutto_eur` exactly.
    ///
    /// ## Use cases
    ///
    /// - B2B building: split a shared transformer fee by tenant floor area
    /// - Portfolio billing: allocate a shared network cost across sub-accounts
    /// - GGV cost sharing: divide a building's common-parts energy cost
    ///
    /// ## Arguments
    ///
    /// - `fractions`: allocation fractions per recipient. Do NOT need to sum to 1.0;
    ///   they are normalised internally by `billing::proportional_split`.
    /// - `contexts`: one `BillingContext` per recipient (must match `fractions.len()`).
    ///   Each recipient gets their own rechnungsnummer, malo_id, etc.
    ///
    /// ## Errors
    ///
    /// Returns `Err` when `fractions.len() != contexts.len()` or `fractions` is empty.
    pub fn allocate_proportionally(
        self,
        fractions: &[Decimal],
        contexts: Vec<crate::context::BillingContext>,
    ) -> Result<Vec<Invoice>, billing::BillingError> {
        if fractions.len() != contexts.len() || fractions.is_empty() {
            return Err(billing::BillingError::InvalidInput {
                reason: format!(
                    "fractions.len() ({}) must equal contexts.len() ({})",
                    fractions.len(),
                    contexts.len()
                ),
            });
        }

        let n = fractions.len();
        let mut recipient_positions: Vec<Vec<crate::position::BillingPosition>> =
            (0..n).map(|_| Vec::new()).collect();

        for pos in &self.positions {
            // Split this position's net_eur across recipients using penny-correct split.
            // billing::proportional_split requires non-negative total; handle sign separately.
            let (abs_eur, sign) = if pos.net_eur < Decimal::ZERO {
                (-pos.net_eur, -Decimal::ONE)
            } else {
                (pos.net_eur, Decimal::ONE)
            };
            let splits = billing::proportional_split(abs_eur, fractions, 5)?;
            for (i, split_abs) in splits.iter().enumerate() {
                let split_amount = sign * split_abs;
                // Adjust quantity proportionally where it makes sense.
                let split_qty = if pos.quantity.is_zero() || fractions.len() <= 1 {
                    pos.quantity
                } else {
                    let total_frac: Decimal = fractions.iter().sum();
                    if total_frac.is_zero() {
                        pos.quantity
                    } else {
                        (pos.quantity * fractions[i] / total_frac).round_dp(4)
                    }
                };
                let mut split_pos = pos.clone();
                split_pos.net_eur = split_amount;
                split_pos.quantity = split_qty;
                recipient_positions[i].push(split_pos);
            }
        }

        Ok(recipient_positions
            .into_iter()
            .zip(contexts)
            .map(|(positions, ctx)| Invoice::from_positions(ctx, positions, vec![]))
            .collect())
    }

    /// Returns `true` when any warning has `WarningSeverity::Error`.
    ///
    /// Operators should block invoice dispatch when `has_errors()` returns `true`.
    /// Typical causes: §41b iMSys mismatch, missing mandatory tariff fields.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        use crate::position::WarningSeverity;
        self.warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Error)
    }

    /// Returns `true` when any warning has `WarningSeverity::Warning` or higher.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        use crate::position::WarningSeverity;
        self.warnings
            .iter()
            .any(|w| w.severity >= WarningSeverity::Warning)
    }
}

// ── Correction / Storno helpers ───────────────────────────────────────────────

/// Produce a Korrekturrechnung (correction invoice) JSON from a stored Rechnung JSON.
///
/// Used when the original `Invoice` object is not available (only the stored JSON).
/// Negates all monetary amounts and sets correction identity fields.
///
/// ## When to use
///
/// - `post_correction` handler: original invoice is loaded from `billing_records`,
///   only `rechnung_json` is available, not the original `Invoice` struct.
///
/// ## What this produces
///
/// - `rechnungsart` → `"KORREKTURRECHNUNG"` (or `"STORNORECHNUNG"` for full cancellations)
/// - `istOriginal` → `false`
/// - `originalRechnungsnummer` → the original invoice number
/// - All `wert` monetary fields → sign-negated
///
/// ## Moved from `handlers.rs`
///
/// Previously `negate_rechnung_json()` in `billingd/src/handlers.rs`. Moved here
/// so the library owns all sign-negation logic for invoices.
pub fn negate_rechnung_json_for_correction(
    original: &serde_json::Value,
    original_rechnungsnummer: &str,
    new_rechnungsnummer: &str,
) -> serde_json::Value {
    let mut corrected = original.clone();
    if let Some(obj) = corrected.as_object_mut() {
        obj.insert("istOriginal".to_owned(), serde_json::json!(false));
        obj.insert(
            "originalRechnungsnummer".to_owned(),
            serde_json::json!(original_rechnungsnummer),
        );
        obj.insert(
            "rechnungsnummer".to_owned(),
            serde_json::json!(new_rechnungsnummer),
        );
        obj.insert(
            "rechnungsart".to_owned(),
            serde_json::json!("KORREKTURRECHNUNG"),
        );

        negate_betrag_in_obj(obj, "gesamtbrutto");
        negate_betrag_in_obj(obj, "gesamtnetto");
        negate_betrag_in_obj(obj, "gesamtsteuer");
        negate_betrag_in_obj(obj, "abschlagTotal");
        negate_betrag_in_obj(obj, "zahlbetrag");

        if let Some(serde_json::Value::Array(positionen)) = obj.get_mut("rechnungspositionen") {
            for pos in positionen.iter_mut() {
                if let Some(pos_obj) = pos.as_object_mut() {
                    negate_betrag_in_obj(pos_obj, "gesamtpreis");
                    if let Some(serde_json::Value::Object(ep)) = pos_obj.get_mut("einzelpreis") {
                        negate_wert_field(ep);
                    }
                }
            }
        }
    }
    corrected
}

fn negate_betrag_in_obj(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str) {
    if let Some(serde_json::Value::Object(betrag)) = obj.get_mut(key) {
        negate_wert_field(betrag);
    }
}

fn negate_wert_field(obj: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(v) = obj.get("wert") {
        let negated = match v {
            serde_json::Value::String(s) => s
                .parse::<Decimal>()
                .ok()
                .map(|d| serde_json::json!((-d).to_string())),
            serde_json::Value::Number(n) => n.as_f64().map(|f| serde_json::json!(-f)),
            _ => None,
        };
        if let Some(neg) = negated {
            obj.insert("wert".to_owned(), neg);
        }
    }
}

// ── EN16931 BG-23 VAT breakdown ───────────────────────────────────────────────

/// EN16931 VAT category code for one tax subtotal (BT-118).
///
/// A structured code, not free text: EN16931 validates the category against the
/// rate, and the wrong pairing fails a receiving system's schematron rather than
/// merely looking odd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VatCategory {
    /// `S` — standard rate.
    Standard,
    /// `Z` — zero-rated goods. §12 Abs. 3 UStG (Solar ≤ 30 kWp) lands here.
    ZeroRated,
    /// `AE` — VAT reverse charge, §13b UStG.
    ReverseCharge,
    /// `E` — exempt from VAT.
    Exempt,
}

impl VatCategory {
    /// The EN16931 code (BT-118).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Standard => "S",
            Self::ZeroRated => "Z",
            Self::ReverseCharge => "AE",
            Self::Exempt => "E",
        }
    }
}

/// One VAT subtotal — EN16931 BG-23.
///
/// EN16931 requires **one breakdown entry per distinct category and rate**, each
/// carrying its own taxable base (BT-116) and tax amount (BT-117). A single
/// aggregate `mwst_eur` cannot express that, and an invoice mixing rates — 19 %
/// commodity with 7 % Fernwärme (§12 Abs. 2 Nr. 1 UStG) or 0 % Solar (§12 Abs. 3
/// UStG) — is structurally invalid without it.
///
/// Zero-rated bases are included. Omitting them would make the sum of the
/// taxable bases differ from the invoice net, which is exactly what the
/// EN16931 total-reconciliation rules check.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaxSubtotal {
    /// EN16931 BT-118 category.
    pub category: VatCategory,
    /// Rate as a percentage (BT-119), e.g. `19`, `7`, `0`.
    pub rate_percent: Decimal,
    /// Taxable base in EUR (BT-116).
    pub taxable_base_eur: Decimal,
    /// Tax amount in EUR (BT-117).
    pub tax_amount_eur: Decimal,
}

impl TaxSubtotal {
    /// Project into the BO4E [`rubo4e::current::Steuerbetrag`].
    #[must_use]
    pub fn to_bo4e(&self) -> rubo4e::current::Steuerbetrag {
        rubo4e::current::Steuerbetrag {
            basiswert: Some(self.taxable_base_eur),
            steuerwert: Some(self.tax_amount_eur),
            // BO4E carries the rate as a percentage, matching BT-119.
            steuersatz: Some(self.rate_percent),
            steuerart: Some(match self.category {
                VatCategory::ReverseCharge => rubo4e::current::Steuerart::Rcv,
                _ => rubo4e::current::Steuerart::Ust,
            }),
            waehrungscode: Some(rubo4e::current::Waehrungscode::Eur),
            ..Default::default()
        }
    }
}

/// Group an invoice's positions into EN16931 VAT subtotals.
///
/// Groups by effective rate — a position's own `applicable_tax_rate` when set,
/// otherwise `default_rate`. `Tax`, `Abschlag` and `Info` positions are excluded
/// from the base: they are not supplies.
#[must_use]
pub fn tax_subtotals_of(positions: &[BillingPosition], default_rate: Decimal) -> Vec<TaxSubtotal> {
    use std::collections::BTreeMap;

    // Keyed on the rate's string form so ordering is stable and 0.190 groups
    // with 0.19.
    let mut buckets: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for p in positions {
        if matches!(
            p.category,
            PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
        ) {
            continue;
        }
        let rate = p.applicable_tax_rate.unwrap_or(default_rate).normalize();
        let entry = buckets
            .entry(rate.to_string())
            .or_insert((rate, Decimal::ZERO));
        entry.1 += p.net_eur;
    }

    buckets
        .into_values()
        .map(|(rate, base)| {
            let pct = (rate * Decimal::ONE_HUNDRED).normalize();
            let tax = (base * rate).round_dp(2);
            TaxSubtotal {
                category: if rate.is_zero() {
                    VatCategory::ZeroRated
                } else {
                    VatCategory::Standard
                },
                rate_percent: pct,
                taxable_base_eur: base.round_dp(2),
                tax_amount_eur: tax,
            }
        })
        .collect()
}

#[cfg(test)]
mod tax_subtotal_tests {
    use super::*;
    use crate::position::PositionCategory;
    use rust_decimal_macros::dec;

    fn pos(net: Decimal, rate: Option<Decimal>, cat: PositionCategory) -> BillingPosition {
        let mut p = BillingPosition::debit("x", Decimal::ONE, "kWh", net, cat);
        p.applicable_tax_rate = rate;
        p
    }

    /// EN16931 BG-23 needs one entry per rate. A single aggregate cannot
    /// represent 19 % commodity next to 7 % Fernwärme.
    #[test]
    fn mixed_rates_produce_one_subtotal_each() {
        let positions = vec![
            pos(dec!(1000), None, PositionCategory::Commodity),
            pos(dec!(500), Some(dec!(0.07)), PositionCategory::Commodity),
        ];
        let subs = tax_subtotals_of(&positions, dec!(0.19));
        assert_eq!(subs.len(), 2, "one entry per rate: {subs:?}");

        let standard = subs.iter().find(|s| s.rate_percent == dec!(19)).unwrap();
        assert_eq!(standard.taxable_base_eur, dec!(1000));
        assert_eq!(standard.tax_amount_eur, dec!(190));
        assert_eq!(standard.category, VatCategory::Standard);

        let reduced = subs.iter().find(|s| s.rate_percent == dec!(7)).unwrap();
        assert_eq!(reduced.taxable_base_eur, dec!(500));
        assert_eq!(reduced.tax_amount_eur, dec!(35));
    }

    /// A zero-rated base must still appear. Omitting it leaves the sum of the
    /// taxable bases short of the invoice net, which is what the EN16931
    /// total-reconciliation rules check.
    #[test]
    fn zero_rated_positions_still_get_a_subtotal() {
        let positions = vec![
            pos(dec!(1000), None, PositionCategory::Commodity),
            // §12 Abs. 3 UStG — Solar ≤ 30 kWp.
            pos(dec!(250), Some(Decimal::ZERO), PositionCategory::Commodity),
        ];
        let subs = tax_subtotals_of(&positions, dec!(0.19));
        let zero = subs
            .iter()
            .find(|s| s.rate_percent.is_zero())
            .expect("zero-rated subtotal must be present");
        assert_eq!(zero.taxable_base_eur, dec!(250));
        assert_eq!(zero.tax_amount_eur, Decimal::ZERO);
        assert_eq!(zero.category, VatCategory::ZeroRated);

        // The bases must reconcile with the invoice net.
        let base_sum: Decimal = subs.iter().map(|s| s.taxable_base_eur).sum();
        assert_eq!(base_sum, dec!(1250));
    }

    /// Tax, Abschlag and Info positions are not supplies and must stay out of
    /// the base — otherwise VAT is levied on VAT.
    #[test]
    fn non_supply_positions_are_excluded_from_the_base() {
        let positions = vec![
            pos(dec!(1000), None, PositionCategory::Commodity),
            pos(dec!(190), None, PositionCategory::Tax),
            pos(dec!(-300), None, PositionCategory::Abschlag),
            pos(dec!(99), None, PositionCategory::Info),
        ];
        let subs = tax_subtotals_of(&positions, dec!(0.19));
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].taxable_base_eur, dec!(1000));
        assert_eq!(subs[0].tax_amount_eur, dec!(190));
    }

    /// A credit note carries negative bases and negative tax.
    #[test]
    fn credit_positions_yield_negative_tax() {
        let positions = vec![pos(dec!(-500), None, PositionCategory::Commodity)];
        let subs = tax_subtotals_of(&positions, dec!(0.19));
        assert_eq!(subs[0].taxable_base_eur, dec!(-500));
        assert_eq!(subs[0].tax_amount_eur, dec!(-95));
    }

    /// Equivalent rates must group, not split into near-duplicate entries.
    #[test]
    fn equivalent_rate_spellings_group_together() {
        let positions = vec![
            pos(dec!(100), Some(dec!(0.19)), PositionCategory::Commodity),
            pos(dec!(100), Some(dec!(0.190)), PositionCategory::Commodity),
        ];
        let subs = tax_subtotals_of(&positions, dec!(0.19));
        assert_eq!(subs.len(), 1, "0.19 and 0.190 are one rate: {subs:?}");
        assert_eq!(subs[0].taxable_base_eur, dec!(200));
    }

    /// The BO4E projection carries the rate as a percentage, matching BT-119.
    #[test]
    fn bo4e_projection_uses_percent_and_eur() {
        let sub = TaxSubtotal {
            category: VatCategory::Standard,
            rate_percent: dec!(19),
            taxable_base_eur: dec!(1000),
            tax_amount_eur: dec!(190),
        };
        let bo = sub.to_bo4e();
        assert_eq!(bo.steuersatz, Some(dec!(19)));
        assert_eq!(bo.basiswert, Some(dec!(1000)));
        assert_eq!(bo.steuerwert, Some(dec!(190)));
        assert_eq!(bo.waehrungscode, Some(rubo4e::current::Waehrungscode::Eur));
        assert_eq!(bo.steuerart, Some(rubo4e::current::Steuerart::Ust));
    }

    /// Reverse charge (§13b UStG) maps onto BO4E `Rcv` and EN16931 `AE`.
    #[test]
    fn reverse_charge_maps_to_rcv_and_ae() {
        let sub = TaxSubtotal {
            category: VatCategory::ReverseCharge,
            rate_percent: Decimal::ZERO,
            taxable_base_eur: dec!(1000),
            tax_amount_eur: Decimal::ZERO,
        };
        assert_eq!(sub.category.code(), "AE");
        assert_eq!(
            sub.to_bo4e().steuerart,
            Some(rubo4e::current::Steuerart::Rcv)
        );
    }
}
