//! `Invoice` — the aggregate root of every billing run.
//!
//! Collects all `BillingPosition` items from the `BillingEngine` providers,
//! computes totals, and can serialise to BO4E-compatible `Rechnung` JSON.

use crate::EuroAmount;
use crate::rates::RoundMoney;
use rust_decimal::Decimal;
use rust_decimal::dec;
use serde::Serialize;

use crate::context::{AbschlagDeduction, BillingContext};
use crate::error::EngineError;
use crate::position::{BillingPosition, BillingWarning, PositionCategory};

/// Split `total` across `fractions` so the shares sum back to `total` exactly.
///
/// [`billing::proportional_split`] is exact but refuses a negative total, and
/// every reversal, Gutschrift and Abschlag deduction here carries one. The sign
/// is lifted out and re-applied; negation is exact in `Decimal`.
/// Normalise arbitrary non-negative weights into shares that sum to exactly one.
///
/// [`billing::proportional_split`] takes *shares* and refuses a set that does
/// not sum to one; callers allocate by floor area, sub-meter reading or
/// headcount. The last share absorbs the division residue, so the set sums to
/// one exactly rather than within a rounding error.
fn normalise_weights(weights: &[Decimal]) -> Result<Vec<Decimal>, EngineError> {
    let mut sum = Decimal::ZERO;
    for &w in weights {
        if w < Decimal::ZERO {
            return Err(EngineError::AllocationWeightsInvalid { sum: w });
        }
        sum += w;
    }
    if sum <= Decimal::ZERO {
        return Err(EngineError::AllocationWeightsInvalid { sum });
    }
    let mut shares: Vec<Decimal> = weights.iter().map(|&w| w / sum).collect();
    let last = shares.len() - 1;
    let head: Decimal = shares[..last].iter().sum();
    shares[last] = Decimal::ONE - head;
    Ok(shares)
}

fn signed_split(
    total: Decimal,
    fractions: &[Decimal],
    scale: u32,
) -> Result<Vec<Decimal>, EngineError> {
    let negative = total < Decimal::ZERO;
    let magnitude = if negative { -total } else { total };
    let mut shares = billing::proportional_split(magnitude, fractions, scale)?;
    if negative {
        for share in &mut shares {
            *share = -*share;
        }
    }
    Ok(shares)
}

/// A completed invoice — the immutable result of `BillingEngine::bill()`.
///
/// ## Invariants
///
/// - `brutto_eur == netto_eur + mwst_eur` (within 0.001 EUR rounding tolerance)
/// - `zahlbetrag_eur == brutto_eur - abschlag_total_eur`
///
/// ## §40 EnWG — Kilowattstundenpreis
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
/// must resolve (e.g. §41a iMSys mismatch, §41 disclosure fields missing).
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

    /// Total of advance payments (Abschläge) deducted on this invoice, gross.
    ///
    /// Non-zero only for `InvoiceType::Final` with `ctx.abschlage` populated.
    /// Negative on a Stornorechnung, which reverses the deduction along with
    /// everything else.
    pub abschlag_total_eur: Decimal,

    /// Tax contained in `abschlag_total_eur`.
    ///
    /// §14 Abs. 5 Satz 2 UStG requires an Endrechnung to deduct the advances and
    /// the tax attributable to them, so the tax already invoiced on the advances
    /// is stated separately rather than folded into the gross deduction. Summed
    /// per advance at the rate that advance was invoiced at, which need not be
    /// the rate on this invoice.
    pub abschlag_ust_eur: Decimal,

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
    /// regulatory issues (e.g. §41a iMSys mismatch). Informational warnings are
    /// advisory only.
    ///
    /// These warnings are also emitted by [`BillingEngine::validate()`](crate::BillingEngine::validate)
    /// so operators can run a pre-flight check before committing to billing.
    pub warnings: Vec<BillingWarning>,
}

/// The attribute carrying the process label BO4E's `Rechnungstyp` cannot
/// express (Gutschrift, Storno, Korrektur, Teilrechnung).
///
/// Every `ZusatzAttribut` mako emits is namespaced `mako:<snake_case>` and
/// listed in the registry `cargo xtask check-bo4e-attributes` enforces — BO4E
/// mandates no convention for its extension slot, so an unprefixed name could
/// collide with a future BO4E field or the counterparty's own attributes.
pub(crate) const RECHNUNGSART_ATTRIBUT: &str = "mako:rechnungsart";

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

    /// The advances on this invoice as [`billing::AdvancePayment`]s.
    ///
    /// Empty unless the context carries `abschlage`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Arithmetic`] if an advance's amounts overflow.
    pub fn advance_payments(&self) -> Result<Vec<billing::AdvancePayment>, EngineError> {
        self.context
            .abschlage
            .iter()
            .map(AbschlagDeduction::to_advance_payment)
            .collect()
    }

    /// The advances as a [`billing::Prepayment`], in the resolution this invoice
    /// carries.
    ///
    /// Itemised whenever advances are present: the per-advance tax is what makes
    /// the deduction lawful under §14 Abs. 5 Satz 2 UStG, so it is never collapsed
    /// to a flat total here.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Arithmetic`] if an advance's amounts overflow, or
    /// if the set of advances is not a valid prepayment.
    pub fn prepayment(&self) -> Result<billing::Prepayment, EngineError> {
        let advances = self.advance_payments()?;
        if advances.is_empty() {
            return Ok(billing::Prepayment::None);
        }
        Ok(billing::Prepayment::itemised(advances)?)
    }

    /// The residual VAT breakdown for a **Restrechnung** — the whole supply per
    /// rate, less what the advances already taxed.
    ///
    /// This is the form the BMF recommends for e-invoices (Schreiben v.
    /// 15.10.2024, Rn. 48): bill the remainder and do not list the advances,
    /// because EN 16931's core profiles have nowhere to carry per-advance tax.
    ///
    /// Returns the full breakdown unchanged when there are no advances.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Arithmetic`] when an advance taxed a
    /// `(category, rate)` group this supply does not contain, or when the
    /// advances exceed the supply in any group — over-deduction would understate
    /// the output tax owed.
    pub fn residual_breakdown(
        &self,
        default_rate: Decimal,
    ) -> Result<Vec<billing::TaxBreakdownEntry>, EngineError> {
        let full: Vec<billing::TaxBreakdownEntry> = self
            .tax_subtotals(default_rate)
            .iter()
            .map(TaxSubtotal::to_breakdown_entry)
            .collect::<Result<_, _>>()?;
        let advances = self.advance_payments()?;
        if advances.is_empty() {
            return Ok(full);
        }
        Ok(billing::advance::residual_breakdown(&full, &advances)?)
    }

    /// Assemble an `Invoice` from a flat list of positions and warnings.
    ///
    /// Separates Tax and Abschlag positions from all others:
    /// - `netto_eur` = sum of non-Tax, non-Abschlag positions
    /// - `mwst_eur`  = sum of Tax positions
    /// - `brutto_eur` = netto + mwst
    /// - `abschlag_total_eur` = negated sum of Abschlag positions (signed)
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
        // Signed, not `.abs()`: an Abschlag position carries the deduction as a
        // negative net, so the advance total is its negation. A Stornorechnung
        // has already negated every position, which flips this total too — that
        // is what makes the reversal the exact negation of the original
        // (`−(B − A)`, not `−B − A`).
        let abschlag_total_eur: Decimal = positions
            .iter()
            .filter(|p| p.category == PositionCategory::Abschlag)
            .map(|p| -p.net_eur)
            .sum();
        let zahlbetrag_eur = brutto_eur - abschlag_total_eur;
        // From the context, not the positions: an Abschlag position carries only
        // the gross paid, while the rate it was invoiced at lives on the deduction.
        // Sign-follows-the-document, as above.
        let reversal_sign = if context.invoice_type.is_reversal() {
            Decimal::NEGATIVE_ONE
        } else {
            Decimal::ONE
        };
        let abschlag_ust_eur: Decimal = context
            .abschlage
            .iter()
            .map(AbschlagDeduction::ust_eur)
            .sum::<Decimal>()
            * reversal_sign;
        let billing_run_id = context.billing_run_id.clone();
        Self {
            context,
            positions,
            netto_eur,
            mwst_eur,
            brutto_eur,
            abschlag_total_eur,
            abschlag_ust_eur,
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

    /// §40 EnWG — all-inclusive Kilowattstundenpreis (ct/kWh) for display on invoice.
    ///
    /// §40 EnWG requires that every electricity invoice shows the total
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
    /// assert_eq!(ct.round_kfm(2), dec!(39.70));
    /// ```
    #[must_use]
    pub fn kilowattstundenpreis_brutto_ct(&self, total_kwh: Decimal) -> Option<Decimal> {
        if total_kwh <= Decimal::ZERO {
            return None;
        }
        // brutto_eur / kWh × 100 → ct/kWh
        Some((self.brutto_eur / total_kwh * dec!(100)).round_kfm(4))
    }

    /// Produce the fully-typed BO4E [`rubo4e::current::Rechnung`] for this invoice.
    ///
    /// This is the single source of the invoice document shape; the JSONB stored
    /// by `billingd` and ingested by `accountingd` is exactly
    /// `serde_json::to_value(self.to_rechnung())` (see [`Invoice::to_rechnung_json`]).
    ///
    /// ## Rechnungsdatum
    ///
    /// [`BillingContext::issue_date`] when the caller set one, else `period_to`.
    /// The library is pure and has no concept of "today"; a caller that has a
    /// clock supplies the real issue date there rather than patching the
    /// returned BO, because the Fälligkeit below is derived from it.
    ///
    /// ## Fälligkeitsdatum
    ///
    /// Issue date **+ 14 days** (§ 40c Abs. 1 EnWG: due at the earliest two
    /// weeks after the payment request reaches the customer). Counting from the
    /// period end instead made every catch-up invoice and every late
    /// Schlussrechnung arrive already overdue.
    ///
    /// ## Where mako-specific facts live
    ///
    /// Everything with a BO4E-canonical field uses it (`rechnungstyp`,
    /// `originalRechnungsnummer`, `marktlokation`, `zaehler`, `netzbetreiber`,
    /// `vertrag`, `faelligkeitsdatum`, `zuZahlen`, …). Facts BO4E does not model
    /// ride as `zusatzAttribute` (§40 Kilowattstundenpreis, §40b
    /// Preisvergleichsdaten, §40 Abs. 2 Verbraucherinformationen, §42
    /// Stromkennzeichnung, contract facts, audit ids). Per-position facts with
    /// no BO4E home (`mako:rechtliche_grundlage`, `mako:positionstyp`,
    /// `mako:positionskategorie`) ride as `zusatzAttribute` on the position —
    /// `_additional` is for what a *counterparty* sent that this schema version
    /// does not define, not for mako's own vocabulary.
    ///
    /// ## Money is Decimal-exact
    ///
    /// Every legally relevant amount (`gesamtnetto`, `gesamtsteuer`,
    /// `gesamtbrutto`, `zuZahlen`, position amounts, Steuerbeträge,
    /// Vorauszahlungen) is a `rust_decimal::Decimal` end to end — rubo4e
    /// serialises `Decimal` as a JSON string, so no value ever passes through
    /// an `f64`.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    #[cfg(feature = "bo4e")]
    pub fn to_rechnung(&self) -> rubo4e::current::Rechnung {
        use rubo4e::current as bo;
        let ctx = &self.context;

        let betrag_eur = |wert: Decimal| bo::Betrag {
            wert: Some(wert),
            waehrung: Some(bo::Waehrungscode::Eur),
            ..Default::default()
        };

        // Derived from the positions, so the breakdown cannot drift from the totals.
        let steuerbetraege: Vec<bo::Steuerbetrag> = self
            .tax_subtotals(ctx.regulatory_rates.mwst_rate)
            .iter()
            .map(TaxSubtotal::to_bo4e)
            .collect();

        // Each advance as its own BO4E Vorauszahlung, carrying the date the
        // customer paid it — §41 EnWG requires the reconciliation to be
        // verifiable per payment, not as one lump sum.
        let vorauszahlungen: Vec<bo::Vorauszahlung> = ctx
            .abschlage
            .iter()
            .map(|a| bo::Vorauszahlung {
                betrag: Some(betrag_eur(a.betrag_eur)),
                // BO4E types this as a datetime; the payment date carries no
                // time of day, so it is pinned to midnight UTC.
                datum: Some(a.datum.midnight().assume_utc()),
                referenz: a.beschreibung.clone(),
                ..Default::default()
            })
            .collect();

        // A BO4E `Rechnungsposition` is a **net supply line**. BO4E states the
        // relationship itself — `gesamtnetto` is „Die Summe der Nettobeträge
        // der Rechnungsteile" — and expresses the other two things separately:
        // tax as `steuerbetraege`/`gesamtsteuer`, advances as
        // `vorauszahlungen` and the `zuZahlen` balance.
        //
        // So the `Tax` and `Abschlag` positions this engine carries internally
        // must not be emitted as positions. Doing so stated each of them
        // **twice**, in two shapes, and left `gesamtnetto` irreconcilable
        // against the position vector for every conforming reader — the same
        // defect `invoic-checker` stage 3 disputes when a counterparty sends
        // it. The EN 16931 mapping (`en16931_map`) has always excluded them;
        // this path was the outlier.
        //
        // `Info` positions stay: they carry `net_eur == 0` (Zählerstand,
        // Brennwertkorrektur, § 51 suspension notes), so they change no sum,
        // and § 40 EnWG wants them on the document.
        let rechnungspositionen: Vec<bo::Rechnungsposition> = self
            .positions
            .iter()
            .filter(|p| p.is_rechnungsposition())
            .enumerate()
            .map(|(i, p)| {
                let einheit = mengeneinheit_of(&p.unit);
                let mut attrs: Vec<bo::ZusatzAttribut> = Vec::new();
                // The calculation trace travels with the position it explains.
                // BO4E has no field for one, so it rides as a ZusatzAttribut —
                // the sanctioned place for what the schema does not model. This
                // is the only surviving record of *why* the amount is what it
                // is once the Invoice value is dropped after storage.
                if let Ok(t) = serde_json::to_value(&p.trace) {
                    attrs.push(zusatz_attribut("mako:calculation_trace", t));
                }
                // Units the BO4E Mengeneinheit vocabulary cannot express
                // ("EUR", "Ereignisse", "Sessionen", …) keep their original
                // spelling here instead of being silently dropped.
                if einheit.is_none() && !p.unit.is_empty() {
                    attrs.push(zusatz_attribut(
                        "mako:einheit",
                        serde_json::Value::String(p.unit.clone()),
                    ));
                }
                let mut pos = bo::Rechnungsposition {
                    positionsnummer: Some((i + 1) as i64),
                    positionstext: Some(p.description.clone()),
                    positions_menge: Some(bo::Menge {
                        wert: Some(p.quantity),
                        einheit,
                        ..Default::default()
                    }),
                    einzelpreis: Some(bo::Preis {
                        wert: Some(p.unit_price_eur),
                        einheit: Some(bo::Waehrungseinheit::Eur),
                        ..Default::default()
                    }),
                    gesamtpreis: Some(betrag_eur(p.net_eur)),
                    ..Default::default()
                };
                // Per-position facts BO4E does not model ride in
                // `zusatzAttribute` — a real BO4E field on `Rechnungsposition`
                // — under the `mako:` namespace, like every other mako
                // extension. A bare key in `_additional` would be
                // indistinguishable from a field BO4E might introduce and from
                // one the counterparty writes, and the outbound gate refuses
                // it (`Bo4eExtensions::ensure_no_extension_data`).
                let mut attrs = attrs;
                if let Some(lb) = &p.legal_basis {
                    attrs.push(zusatz_attribut(
                        "mako:rechtliche_grundlage",
                        serde_json::json!(lb),
                    ));
                }
                attrs.push(zusatz_attribut(
                    "mako:positionstyp",
                    serde_json::json!(p.tags.first().map(String::as_str).unwrap_or("POSITION")),
                ));
                attrs.push(zusatz_attribut(
                    "mako:positionskategorie",
                    serde_json::json!(format!("{:?}", p.category)),
                ));
                pos.zusatz_attribute = Some(attrs);
                pos
            })
            .collect();

        let faelligkeitsdatum = ctx.faelligkeitsdatum();

        // Collect ZusatzAttribute from info positions tagged "gasqualitaet"
        let mut zusatz_attribute: Vec<bo::ZusatzAttribut> = self
            .positions
            .iter()
            .filter(|p| p.has_tag("gasqualitaet") && p.category == PositionCategory::Info)
            .map(|p| {
                zusatz_attribut(
                    "mako:gasqualitaet",
                    serde_json::json!(p.legal_basis.as_deref().unwrap_or("")),
                )
            })
            .collect();

        // §40 Abs. 1 EnWG — contract facts. They change no amount, but an
        // invoice without them is incomplete; each rides as its own attribute
        // so a renderer can place them individually.
        if let Some(vi) = &ctx.vertragsinformationen {
            for (name, wert) in [
                ("mako:vertragsdauer", vi.vertragsdauer.clone()),
                ("mako:kuendigungsfrist", vi.kuendigungsfrist.clone()),
                (
                    "mako:naechstmoeglicher_kuendigungstermin",
                    vi.naechstmoeglicher_kuendigungstermin
                        .map(|d| d.to_string()),
                ),
                (
                    "mako:naechster_abrechnungstermin",
                    vi.naechster_abrechnungstermin.map(|d| d.to_string()),
                ),
            ] {
                if let Some(wert) = wert {
                    zusatz_attribute.push(zusatz_attribut(name, serde_json::json!(wert)));
                }
            }
        }

        // §42 EnWG — Stromkennzeichnung, structured: fuel-mix percentages, the
        // CO₂ figure §42 Abs. 2 Nr. 2 makes mandatory, and HKN certification.
        if let Some(quellen) = &ctx.energiequellen
            && let Ok(wert) = serde_json::to_value(quellen)
        {
            zusatz_attribute.push(zusatz_attribut("mako:stromkennzeichnung", wert));
        }

        // §40 Abs. 2 EnWG — Verbrauchshistorie summary as ZusatzAttribut
        if let Some(vh) = &ctx.verbrauchshistorie {
            if let Some(vj) = vh.vorjahr_kwh {
                zusatz_attribute.push(zusatz_attribut(
                    "mako:verbrauch_vorjahr",
                    serde_json::json!(vj.to_string()),
                ));
            }
            if let Some(avg) = vh.bundesdurchschnitt_kwh {
                zusatz_attribute.push(zusatz_attribut(
                    "mako:verbrauch_bundesdurchschnitt",
                    serde_json::json!(avg.to_string()),
                ));
            }
        }

        // Audit trail: billing run ID for ERP reconciliation and duplicate detection.
        if let Some(run_id) = &self.billing_run_id {
            zusatz_attribute.push(zusatz_attribut(
                "mako:billing_run_id",
                serde_json::json!(run_id),
            ));
        }

        // § 40c Abs. 3 EnWG — a credit balance carries a deadline and a rule
        // about how it may be discharged. Downstream (ledger, payout run,
        // customer document) can only honour it if the document says so.
        if let Some(g) = self.guthabenerstattung() {
            zusatz_attribute.push(zusatz_attribut(
                "mako:guthabenerstattung",
                serde_json::json!({
                    "betragEur": g.betrag_eur.to_string(),
                    "spaetestens": g.spaetestens.to_string(),
                    "verrechnungZulaessig": g.verrechnung_zulaessig,
                    "rechtlicheGrundlage": g.rechtsgrundlage,
                }),
            ));
        }

        // Customer category for downstream ERP routing and regulatory rule selection.
        zusatz_attribute.push(zusatz_attribut(
            "mako:kundenkategorie",
            serde_json::json!(format!("{:?}", ctx.kundenkategorie)),
        ));

        // The contractual regime the prices come from — Grundversorgung invoices
        // bill the published Allgemeine Preise (§36 EnWG, §5 StromGVV/GasGVV),
        // Ersatzversorgung the §38 EnWG fallback terms. Emitted for every
        // invoice so the regime is explicit, not inferred from the tariff.
        zusatz_attribute.push(zusatz_attribut(
            "mako:vertragsart",
            serde_json::json!(ctx.vertragsart.label()),
        ));

        // Process-level Rechnungsart labels the BO4E `Rechnungstyp` vocabulary
        // cannot express (GUTSCHRIFT, KORREKTURRECHNUNG, STORNORECHNUNG,
        // TEILRECHNUNG) survive as a ZusatzAttribut; the typed `rechnungstyp`,
        // `istStorno` and `originalRechnungsnummer` fields carry what BO4E can.
        if ctx.invoice_type.rechnungstyp().is_none()
            || ctx.invoice_type == crate::context::InvoiceType::PartialInvoice
        {
            zusatz_attribute.push(zusatz_attribut(
                "mako:rechnungsart",
                serde_json::json!(ctx.invoice_type.rechnungsart()),
            ));
        }

        // §40 EnWG — Kilowattstundenpreis (all-inclusive total price per kWh).
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

        // §40 EnWG — Gesamtbetrag je Kilowattstunde (all-inclusive
        // ct/kWh). Not a lawful BO4E Preis (its Einheit is ct/kWh, not a
        // Waehrungseinheit), so it rides as a structured ZusatzAttribut.
        // Only set when consumption positions exist (electricity commodity kWh known).
        if let Some(ct) = kilowattstundenpreis_ct {
            zusatz_attribute.push(zusatz_attribut(
                "mako:kilowattstundenpreis_gesamt",
                serde_json::json!({
                    "wert": ct.to_string(),
                    "einheit": "ct/kWh",
                    "bezugswert": "KWH",
                    "rechtlicheGrundlage": "§40 EnWG"
                }),
            ));
        }

        // §40b EnWG — Strukturierte Preisvergleichsdaten für Vergleichsportale.
        // Enables price comparison portals (e.g. Verivox, Check24, BNetzA tools)
        // to ingest tariff structure from the invoice machine-readably. No BO4E
        // home exists, so it rides as a structured ZusatzAttribut.
        zusatz_attribute.push(zusatz_attribut(
            "mako:preisvergleichsdaten",
            serde_json::json!({
                "grundpreisEurProJahr": self.positions.iter()
                    .filter(|p| p.has_tag("commodity") && p.unit == "Tage")
                    .map(|p| p.unit_price_eur * dec!(365))
                    .next()
                    .map(|eur_year| serde_json::json!({ "wert": eur_year.to_string(), "waehrung": "EUR" })),
                "arbeitspreisCtProKwh": self.positions.iter()
                    .filter(|p| (p.has_tag("strom") || p.has_tag("gas")) && p.category == crate::position::PositionCategory::Commodity && p.unit.starts_with("kWh"))
                    .map(|p| (p.unit_price_eur * dec!(100)).round_kfm(4))
                    .next()
                    .map(|ct| ct.to_string()),
                "gesamtpreisCtProKwh": kilowattstundenpreis_ct.map(|ct| ct.to_string()),
                "rechtlicheGrundlage": "§40b EnWG"
            }),
        ));

        // §40 Abs. 2 EnWG Nr. 1/9/10/11/12 — supplier contact,
        // Schlichtungsstelle Energie (§111b EnWG), BNetzA Verbraucherservice,
        // Energieberatung and Wechselhinweis. Falls back to the statutory
        // defaults so the mandatory hints are never silently absent from a
        // Letztverbraucher invoice. BO4E does not model them, so they ride as
        // one structured ZusatzAttribut.
        if let Ok(vi) =
            serde_json::to_value(ctx.verbraucherinformationen.clone().unwrap_or_default())
        {
            zusatz_attribute.push(zusatz_attribut("mako:verbraucherinformationen", vi));
        }

        // §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification (mandatory on
        // energy invoices). Identifies the network operator providing grid
        // access at the delivery point. A code that is not a valid 13-digit
        // Marktpartner-ID still survives, as a ZusatzAttribut on the BO.
        let netzbetreiber = ctx.nb_mp_id.as_deref().map(|id| {
            let rollencodenummer = rubo4e::identifiers::MarktpartnerId::new(id).ok();
            let zusatz_attribute = rollencodenummer.is_none().then(|| {
                vec![zusatz_attribut(
                    "mako:marktpartnercode",
                    serde_json::json!(id),
                )]
            });
            Box::new(bo::Marktteilnehmer {
                rollencodenummer,
                zusatz_attribute,
                ..Default::default()
            })
        });

        bo::Rechnung {
            rechnungsnummer: Some(ctx.rechnungsnummer.clone()),
            rechnungstyp: ctx.invoice_type.rechnungstyp(),
            // BO4E marks a Stornorechnung with `istStorno`, not a Rechnungstyp.
            ist_storno: ctx.invoice_type.is_reversal().then_some(true),
            original_rechnungsnummer: ctx.invoice_type.original_invoice_id().map(str::to_owned),
            rechnungsdatum: as_bo4e_timestamp(ctx.ausstellungsdatum()),
            faelligkeitsdatum: as_bo4e_timestamp(faelligkeitsdatum),
            // The delivery point as a typed Marktlokation. `_id` always carries
            // the raw string; `marktlokationsId` only when it passes the BDEW
            // checksum — a synthetic or aggregate subject id must not claim to
            // be a valid MaLo-ID.
            marktlokation: (!ctx.malo_id.is_empty()).then(|| {
                Box::new(bo::Marktlokation {
                    id: Some(ctx.malo_id.clone()),
                    marktlokations_id: rubo4e::identifiers::MaloId::new(&ctx.malo_id).ok(),
                    ..Default::default()
                })
            }),
            // §41 EnWG — Zählernummer, as the typed Zaehler BO.
            zaehler: ctx.zaehler_id.as_ref().map(|z| {
                vec![Box::new(bo::Zaehler {
                    zaehlernummer: Some(z.clone()),
                    ..Default::default()
                })]
            }),
            // The issuer. Geschaeftspartner has no Marktpartner-code field, so
            // the code rides as a ZusatzAttribut; the display name comes from
            // the §40 Abs. 2 Nr. 1 supplier identity when provided.
            rechnungsersteller: Some(Box::new(bo::Geschaeftspartner {
                organisationsname: ctx
                    .verbraucherinformationen
                    .as_ref()
                    .and_then(|vi| vi.lieferant_name.clone()),
                zusatz_attribute: Some(vec![zusatz_attribut(
                    "mako:marktpartnercode",
                    serde_json::json!(ctx.lf_mp_id),
                )]),
                ..Default::default()
            })),
            netzbetreiber,
            vertrag: ctx.contract_id.as_ref().map(|c| {
                Box::new(bo::Vertrag {
                    vertragsnummer: Some(c.clone()),
                    ..Default::default()
                })
            }),
            rechnungsperiode: Some(bo::Zeitraum {
                startdatum: Some(ctx.period_from()),
                enddatum: Some(ctx.period_to()),
                ..Default::default()
            }),
            rechnungspositionen: Some(rechnungspositionen),
            zusatz_attribute: (!zusatz_attribute.is_empty()).then_some(zusatz_attribute),
            // Rounded to cents: these are document totals, and the Steuerbeträge
            // they must reconcile against are themselves stated to the cent.
            gesamtnetto: Some(betrag_eur(self.netto_eur.round_kfm(2))),
            gesamtsteuer: Some(betrag_eur(self.mwst_eur.round_kfm(2))),
            gesamtbrutto: Some(betrag_eur(self.brutto_eur.round_kfm(2))),
            // BO4E: "Eine Liste mit Steuerbeträgen pro Steuerkennzeichen/Steuersatz;
            // die Summe dieser Beträge ergibt den Wert für gesamtsteuer." Also
            // EN 16931 BG-23, which a receiving system validates rate-by-rate:
            // gesamtsteuer alone cannot describe an invoice that mixes rates.
            steuerbetraege: Some(steuerbetraege),
            vorauszahlungen: Some(vorauszahlungen),
            // BO4E `zuZahlen`: gesamtbrutto − vorausbezahlt (§41 EnWG balance).
            zu_zahlen: Some(betrag_eur(self.zahlbetrag_eur.round_kfm(2))),
            // The recipient. The engine knows the customer only through the
            // MaLo; the reference survives as a ZusatzAttribut on the BO.
            rechnungsempfaenger: Some(Box::new(bo::Geschaeftspartner {
                zusatz_attribute: Some(vec![zusatz_attribut(
                    "mako:externe_kunden_id",
                    serde_json::json!(ctx.malo_id),
                )]),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    /// The BO4E `Rechnung` as a `serde_json::Value` — exactly
    /// `serde_json::to_value(self.to_rechnung())`.
    ///
    /// Kept for callers that store or transport the document as JSONB.
    ///
    /// # Panics
    ///
    /// Never, for any `Invoice` this crate can build. A `Rechnung` is a tree of
    /// `Decimal`, `String`, BO4E enums and two timestamps; the first three
    /// cannot fail, and the timestamps are produced by `as_bo4e_timestamp`,
    /// which yields `None` rather than a date RFC 3339 cannot express. Stated
    /// rather than defaulted, because the callers store and dispatch the result
    /// — and PostgreSQL accepts a JSON `null` into a `JSONB NOT NULL` column, so
    /// a defaulted failure would travel as the invoice.
    #[must_use]
    #[cfg(feature = "bo4e")]
    pub fn to_rechnung_json(&self) -> serde_json::Value {
        serde_json::to_value(self.to_rechnung())
            .expect("a Rechnung is always serialisable; see the note on this method")
    }

    /// Merge two invoices for adjacent billing periods (§41 EnWG Tarifwechsel).
    ///
    /// Positions from `self` appear first, then `other`. Totals are re-summed.
    /// Tax layers are **not** re-applied — each invoice was already taxed independently
    /// for its sub-period.
    ///
    /// Uses the context from `self` (billing period, IDs) for the merged invoice.
    /// `other.context.period_to()` is used to update the effective period end.
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
        if other.context.period_to() > ctx.period_to() {
            ctx.period = crate::BillingPeriod::new(ctx.period_from(), other.context.period_to())
                .expect("extending the end of a valid period keeps from <= to");
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
    /// - `fractions`: a non-negative weight per recipient — square metres, a
    ///   sub-meter reading, a headcount. They need not sum to one; they are
    ///   normalised into shares here.
    /// - `contexts`: one `BillingContext` per recipient (must match `fractions.len()`).
    ///   Each recipient gets their own rechnungsnummer, malo_id, etc.
    ///
    /// Every allocated invoice inherits the source invoice's warnings, so a
    /// blocking finding still blocks each share.
    ///
    /// ## Errors
    ///
    /// [`EngineError::AllocationMismatch`] when `fractions.len() != contexts.len()`
    /// or `fractions` is empty; [`EngineError::AllocationWeightsInvalid`] when a
    /// weight is negative or the weights sum to zero.
    pub fn allocate_proportionally(
        self,
        fractions: &[Decimal],
        contexts: Vec<crate::context::BillingContext>,
    ) -> Result<Vec<Invoice>, EngineError> {
        if fractions.len() != contexts.len() || fractions.is_empty() {
            return Err(EngineError::AllocationMismatch {
                fractions: fractions.len(),
                contexts: contexts.len(),
            });
        }

        let n = fractions.len();
        let shares = normalise_weights(fractions)?;
        let mut recipient_positions: Vec<Vec<crate::position::BillingPosition>> =
            (0..n).map(|_| Vec::new()).collect();

        for pos in &self.positions {
            // Amount *and* quantity go through the remainder-distributing split.
            // Rounding each share on its own is exact only when the weights
            // divide evenly — three shares of 1000 kWh give 3 × 333.3333 —
            // so the recipients' volumes would stop adding up to the metered one.
            let split_amounts = signed_split(pos.net_eur, &shares, 5)?;
            let split_quantities = signed_split(pos.quantity, &shares, 4)?;
            for i in 0..n {
                let mut split_pos = pos.clone();
                split_pos.net_eur = split_amounts[i];
                split_pos.quantity = split_quantities[i];
                recipient_positions[i].push(split_pos);
            }
        }

        // Every recipient inherits the source warnings: `has_errors()` gates
        // dispatch, and a §41a iMSys mismatch on the building invoice applies to
        // every share of it.
        Ok(recipient_positions
            .into_iter()
            .zip(contexts)
            .map(|(positions, ctx)| Invoice::from_positions(ctx, positions, self.warnings.clone()))
            .collect())
    }

    /// Returns `true` when any warning has `WarningSeverity::Error`.
    ///
    /// Operators should block invoice dispatch when `has_errors()` returns `true`.
    /// Typical causes: §41a iMSys mismatch, missing mandatory tariff fields.
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

/// What has to happen to a credit balance, and by when (§ 40c Abs. 3 EnWG).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Guthabenerstattung {
    /// The amount owed back to the customer, positive.
    pub betrag_eur: Decimal,
    /// The last day it may be paid out.
    pub spaetestens: time::Date,
    /// Whether offsetting it against the next Abschlag discharges the
    /// obligation. A Schlussrechnung has no next Abschlag, so it does not.
    pub verrechnung_zulaessig: bool,
    pub rechtsgrundlage: &'static str,
}

impl Invoice {
    /// The § 40c Abs. 3 EnWG obligation this invoice creates, if any.
    ///
    /// A settling invoice whose advances exceeded the consumption owes the
    /// customer the difference. The statute does not leave the timing open:
    /// the credit is offset **in full** against the next Abschlag or paid out
    /// **within two weeks** — and a credit from an Abschlussrechnung must be
    /// paid out within two weeks regardless, because there is no next Abschlag
    /// to offset it against.
    ///
    /// Stating it on the document is what lets the ledger and the payout run
    /// act on it; computing the balance and saying nothing about it left the
    /// obligation to whoever happened to read the sign of `zahlbetrag_eur`.
    #[must_use]
    pub fn guthabenerstattung(&self) -> Option<Guthabenerstattung> {
        if self.zahlbetrag_eur >= Decimal::ZERO {
            return None;
        }
        let ist_schlussrechnung = self.context.invoice_type == crate::context::InvoiceType::Final;
        Some(Guthabenerstattung {
            betrag_eur: -self.zahlbetrag_eur,
            spaetestens: self.context.faelligkeitsdatum(),
            verrechnung_zulaessig: !ist_schlussrechnung,
            rechtsgrundlage: if ist_schlussrechnung {
                "§ 40c Abs. 3 Satz 2 EnWG"
            } else {
                "§ 40c Abs. 3 Satz 1 EnWG"
            },
        })
    }
}

// ── BO4E construction helpers ─────────────────────────────────────────────────

/// A BO4E date-only market value as the `date-time` the schema declares.
///
/// BDEW INVOIC transmits `rechnungsdatum` and `faelligkeitsdatum` as DTM
/// qualifier 102 — a bare `YYYYMMDD` — while BO4E types both `format: date-time`.
///
/// **Midnight UTC.** `Rechnung::rechnungsdatum_date()` reads the date in the
/// offset the payload carries, so `+00:00` reads back as the date that went in
/// and stays that date under any later normalisation; a `+01:00` midnight
/// becomes the previous day the moment someone converts it.
///
/// `None` for a year outside RFC 3339's `0000`–`9999`, which the field
/// serialises as. The field is optional and an invoice with no billing period
/// has no issue date, so it is omitted rather than made fatal — rejecting a
/// periodless invoice is the engine's job, not the serializer's.
#[cfg(feature = "bo4e")]
fn as_bo4e_timestamp(date: time::Date) -> Option<time::OffsetDateTime> {
    (0..=9999)
        .contains(&date.year())
        .then(|| date.midnight().assume_utc())
}

/// A BO4E `ZusatzAttribut` — the sanctioned extension point for facts the
/// schema does not model.
#[cfg(feature = "bo4e")]
fn zusatz_attribut(name: &str, wert: serde_json::Value) -> rubo4e::current::ZusatzAttribut {
    rubo4e::current::ZusatzAttribut {
        name: Some(name.to_owned()),
        wert: Some(wert),
        ..Default::default()
    }
}

/// Map an engine unit string onto the BO4E `Mengeneinheit` vocabulary.
///
/// Returns `None` for units BO4E cannot express (`"EUR"` on tax lines,
/// `"Ereignisse"`, `"Sessionen"`, `"m²"`, …); the caller preserves the original
/// spelling as a `mako:einheit` ZusatzAttribut in that case. `"kWh_Hs"` maps to
/// `KWH`: gas is billed in kWh on the superior calorific basis (§25 Nr. 4
/// MessEV), and the Brennwert derivation is recorded in the position trace.
#[cfg(feature = "bo4e")]
fn mengeneinheit_of(unit: &str) -> Option<rubo4e::current::Mengeneinheit> {
    use rubo4e::current::Mengeneinheit as M;
    match unit {
        "kWh" | "kWh_Hs" => Some(M::Kwh),
        "MWh" => Some(M::Mwh),
        "kW" => Some(M::Kw),
        "Tag" | "Tage" => Some(M::Tag),
        "Woche" | "Wochen" => Some(M::Woche),
        "Monat" | "Monate" => Some(M::Monat),
        "Jahr" | "Jahre" => Some(M::Jahr),
        "h" | "Stunde" | "Stunden" => Some(M::Stunde),
        "m³" | "m3" => Some(M::Kubikmeter),
        "%" => Some(M::Prozent),
        "Stück" => Some(M::Stueck),
        _ => None,
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
/// - `istOriginal` → `false`
/// - `originalRechnungsnummer` → the original invoice number
/// - ZusatzAttribut `rechnungsart` → `"KORREKTURRECHNUNG"` (BO4E has no
///   Korrektur value in `Rechnungstyp`; the process label rides as the same
///   attribute `to_rechnung()` uses)
/// - All `wert` monetary fields → sign-negated
///
/// Sign negation lives here rather than in a service, so every caller that
/// corrects an invoice negates it the same way.
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
        upsert_rechnungsart_attribut(obj, "KORREKTURRECHNUNG");

        negate_betrag_in_obj(obj, "gesamtbrutto");
        negate_betrag_in_obj(obj, "gesamtnetto");
        negate_betrag_in_obj(obj, "gesamtsteuer");
        negate_betrag_in_obj(obj, "zuZahlen");

        // The VAT breakdown must follow gesamtsteuer: BO4E requires the
        // Steuerbeträge to sum to it, and a correction that negates the total
        // while leaving the breakdown positive fails that check.
        if let Some(serde_json::Value::Array(steuerbetraege)) = obj.get_mut("steuerbetraege") {
            for entry in steuerbetraege.iter_mut() {
                if let Some(e) = entry.as_object_mut() {
                    negate_decimal_field(e, "basiswert");
                    negate_decimal_field(e, "steuerwert");
                }
            }
        }

        // Advances already paid reverse with the document they settle.
        if let Some(serde_json::Value::Array(vorauszahlungen)) = obj.get_mut("vorauszahlungen") {
            for entry in vorauszahlungen.iter_mut() {
                if let Some(e) = entry.as_object_mut() {
                    negate_betrag_in_obj(e, "betrag");
                }
            }
        }

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

/// Set (or replace) the `rechnungsart` ZusatzAttribut on a stored Rechnung JSON.
///
/// Tolerates a missing or `null` `zusatzAttribute` array — older or
/// hand-assembled documents still get the label.
fn upsert_rechnungsart_attribut(obj: &mut serde_json::Map<String, serde_json::Value>, label: &str) {
    let attrs = obj
        .entry("zusatzAttribute")
        .or_insert_with(|| serde_json::json!([]));
    if !attrs.is_array() {
        *attrs = serde_json::json!([]);
    }
    if let Some(arr) = attrs.as_array_mut() {
        if let Some(existing) = arr
            .iter_mut()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(RECHNUNGSART_ATTRIBUT))
        {
            existing["wert"] = serde_json::json!(label);
        } else {
            arr.push(serde_json::json!({ "name": RECHNUNGSART_ATTRIBUT, "wert": label }));
        }
    }
}

fn negate_betrag_in_obj(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str) {
    if let Some(serde_json::Value::Object(betrag)) = obj.get_mut(key) {
        negate_wert_field(betrag);
    }
}

/// Negate a bare decimal field, as BO4E `Steuerbetrag` carries rather than a
/// nested `Betrag` object.
fn negate_decimal_field(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str) {
    let Some(v) = obj.get(key) else { return };
    let negated = match v {
        serde_json::Value::String(s) => s
            .parse::<Decimal>()
            .ok()
            .map(|d| serde_json::json!((-d).to_string())),
        serde_json::Value::Number(n) => n
            .to_string()
            .parse::<Decimal>()
            .ok()
            .map(|d| serde_json::json!((-d).to_string())),
        _ => None,
    };
    if let Some(neg) = negated {
        obj.insert(key.to_owned(), neg);
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum VatCategory {
    /// `S` — standard rate.
    Standard,
    /// `Z` — zero-rated goods. §12 Abs. 3 UStG (Solar ≤ 30 kWp) lands here.
    ZeroRated,
    /// `AE` — VAT reverse charge, §13b UStG.
    ReverseCharge,
    /// `E` — exempt from VAT.
    Exempt,
    /// `O` — not subject to VAT (services outside the scope).
    ///
    /// The hoheitliche Abwassergebühr under a KAG-Satzung lands here. EN 16931
    /// **BR-O-11 … BR-O-14** make it exclusive: no other category may appear on
    /// the same document.
    OutOfScope,
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
            Self::OutOfScope => "O",
        }
    }

    /// `true` for a category EN 16931 forbids mixing with any other (`O`).
    #[must_use]
    pub const fn is_exclusive(self) -> bool {
        matches!(self, Self::OutOfScope)
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
    /// Project into a [`billing::TaxBreakdownEntry`].
    ///
    /// The rate is carried as a fraction there and as a percentage here, so it is
    /// scaled back down on the way across.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Arithmetic`] if the base or tax overflows
    /// [`EuroAmount`].
    pub fn to_breakdown_entry(&self) -> Result<billing::TaxBreakdownEntry, EngineError> {
        Ok(billing::TaxBreakdownEntry::new(
            match self.category {
                VatCategory::Standard => billing::TaxCategory::Standard,
                VatCategory::ZeroRated => billing::TaxCategory::ZeroRated,
                VatCategory::ReverseCharge => billing::TaxCategory::ReverseCharge,
                VatCategory::Exempt => billing::TaxCategory::Exempt,
                VatCategory::OutOfScope => billing::TaxCategory::OutOfScope,
            },
            self.rate_percent / Decimal::ONE_HUNDRED,
            EuroAmount::checked_from_decimal(self.taxable_base_eur)?,
            EuroAmount::checked_from_decimal(self.tax_amount_eur)?,
        ))
    }

    /// Project into the BO4E [`rubo4e::current::Steuerbetrag`].
    #[must_use]
    #[cfg(feature = "bo4e")]
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

    // Keyed on `(is_reverse_charge, rate-string)` so ordering is stable, 0.190
    // groups with 0.19, and §13b reverse-charge supplies form their own subtotal:
    // they carry rate 0 like a genuine zero-rated supply but are legally distinct
    // (EN 16931 `AE` vs `Z`), so they must never be merged.
    let mut buckets: BTreeMap<(VatCategory, String), (Decimal, Decimal)> = BTreeMap::new();
    for p in positions {
        if matches!(
            p.category,
            PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
        ) {
            continue;
        }
        let rate = p.applicable_tax_rate.unwrap_or(default_rate).normalize();
        let cat = vat_category_of(p, rate);
        let entry = buckets
            .entry((cat, rate.to_string()))
            .or_insert((rate, Decimal::ZERO));
        entry.1 += p.net_eur;
    }

    buckets
        .into_iter()
        .map(|((category, _), (rate, base))| TaxSubtotal {
            category,
            rate_percent: (rate * Decimal::ONE_HUNDRED).normalize(),
            taxable_base_eur: base.round_kfm(2),
            tax_amount_eur: (base * rate).round_kfm(2),
        })
        .collect()
}

/// The EN 16931 VAT category one position falls into.
///
/// Three zero-rate cases that must not be merged: §13b reverse charge (`AE`,
/// the recipient owes the tax), a genuine zero-rated supply (`Z`), and a
/// hoheitliche Leistung the UStG does not reach at all (`O`).
#[must_use]
pub fn vat_category_of(position: &BillingPosition, effective_rate: Decimal) -> VatCategory {
    if position.is_out_of_scope() {
        VatCategory::OutOfScope
    } else if position.is_reverse_charge() {
        VatCategory::ReverseCharge
    } else if effective_rate.is_zero() {
        VatCategory::ZeroRated
    } else {
        VatCategory::Standard
    }
}

#[cfg(all(test, feature = "bo4e"))]
mod tax_subtotal_tests {
    use super::*;
    use crate::position::PositionCategory;
    use rust_decimal::dec;

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

    /// §13b UStG reverse charge: a supply to a Stromwiederverkäufer carries 0 %
    /// VAT on the supplier's invoice, but must be categorised `AE` (ReverseCharge),
    /// NOT `Z` (ZeroRated) — the two must not merge even though both have rate 0.
    #[test]
    fn reverse_charge_is_a_distinct_ae_subtotal() {
        let positions = vec![
            pos(dec!(1000), None, PositionCategory::Commodity),
            // §12 Abs. 3 UStG — genuine zero-rated (Solar ≤ 30 kWp).
            pos(dec!(250), Some(Decimal::ZERO), PositionCategory::Commodity),
            // §13b Abs. 2 Nr. 5 lit. b UStG — electricity to a reseller.
            BillingPosition::debit(
                "Reststrom Wiederverkäufer",
                dec!(5000),
                "kWh",
                dec!(1),
                PositionCategory::Commodity,
            )
            .with_reverse_charge(),
        ];
        let subs = tax_subtotals_of(&positions, dec!(0.19));

        let ae = subs
            .iter()
            .find(|s| s.category == VatCategory::ReverseCharge)
            .expect("a reverse-charge (AE) subtotal must be present");
        assert_eq!(ae.taxable_base_eur, dec!(5000));
        assert_eq!(ae.tax_amount_eur, Decimal::ZERO, "supplier charges no VAT");
        assert!(ae.rate_percent.is_zero());

        // The genuine zero-rated supply stays a separate `Z` subtotal.
        let zero = subs
            .iter()
            .find(|s| s.category == VatCategory::ZeroRated)
            .expect("zero-rated (Z) subtotal must remain distinct from AE");
        assert_eq!(zero.taxable_base_eur, dec!(250));

        // Three legally-distinct categories → three subtotals (S, Z, AE).
        assert_eq!(subs.len(), 3, "S/Z/AE must not merge: {subs:?}");
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

#[cfg(all(test, feature = "bo4e"))]
mod rechnung_json_tests {
    use super::*;
    use crate::context::AbschlagDeduction;
    use crate::position::PositionCategory;
    use rust_decimal::dec;
    use time::macros::date;

    /// Build a Final invoice carrying one advance and one taxable position.
    fn invoice_with_advance() -> Invoice {
        let ctx = BillingContext {
            invoice_type: crate::context::InvoiceType::Final,
            abschlage: vec![AbschlagDeduction {
                datum: date!(2026 - 01 - 15),
                betrag_eur: dec!(119.00),
                ust_satz: dec!(0.19),
                beschreibung: Some("Abschlag Januar 2026".to_owned()),
            }],
            ..BillingContext::default()
        };
        let positions = vec![
            // debit() takes the *unit price*: 1000 kWh x 0.30 = 300.00 net.
            BillingPosition::debit(
                "Arbeitspreis",
                dec!(1000),
                "kWh",
                dec!(0.30),
                PositionCategory::Commodity,
            ),
            BillingPosition::debit(
                "MwSt 19 %",
                Decimal::ONE,
                "EUR",
                dec!(57.00),
                PositionCategory::Tax,
            ),
        ];
        Invoice::from_positions(ctx, positions, vec![])
    }

    /// The emitted keys must be the ones BO4E defines. `rubo4e` routes unknown
    /// keys into its extension map, so a misspelt or invented field name
    /// deserialises to `None` on the typed field rather than failing loudly —
    /// asserting the typed fields are populated is what catches it.
    #[cfg(feature = "bo4e")]
    #[test]
    fn rechnung_json_uses_real_bo4e_field_names() {
        let json = invoice_with_advance().to_rechnung_json();
        let rechnung: rubo4e::current::Rechnung =
            serde_json::from_value(json).expect("emitted JSON is a BO4E Rechnung");

        let steuerbetraege = rechnung
            .steuerbetraege
            .expect("steuerbetraege must be populated, not routed to the extension map");
        assert_eq!(steuerbetraege.len(), 1);
        assert_eq!(steuerbetraege[0].basiswert, Some(dec!(300.00)));
        assert_eq!(steuerbetraege[0].steuerwert, Some(dec!(57.00)));
        assert_eq!(steuerbetraege[0].steuersatz, Some(dec!(19)));

        let vorauszahlungen = rechnung
            .vorauszahlungen
            .expect("vorauszahlungen must be populated, not routed to the extension map");
        assert_eq!(vorauszahlungen.len(), 1);
        assert_eq!(
            vorauszahlungen[0].betrag.as_ref().and_then(|b| b.wert),
            Some(dec!(119.00))
        );
    }

    /// Every §40/§41/§42 EnWG Pflichtangabe must reach the typed BO —
    /// enumerated one by one, not sampled.
    ///
    /// Typed fields: Zählernummer (§41 Abs. 1 Nr. 6 → `zaehler`), Netzbetreiber
    /// (§41 Abs. 1 Nr. 5 → `netzbetreiber.rollencodenummer`), Abrechnungszeitraum
    /// (§40 Abs. 1 → `rechnungsperiode`), Fälligkeit (§40c → `faelligkeitsdatum`).
    /// ZusatzAttribute: contract facts (§40 Abs. 1), Verbraucherinformationen
    /// (§40 Abs. 2), Kilowattstundenpreis (§40), Preisvergleichsdaten (§40b),
    /// Verbrauchshistorie (§40 Abs. 2), Stromkennzeichnung (§42), plus
    /// the mako audit facts (billingRunId, kundenkategorie, vertragsart).
    #[test]
    fn every_sect40_pflichtangabe_survives_the_typed_migration() {
        use time::macros::date;
        let ctx = BillingContext {
            malo_id: "51238696012".to_owned(), // valid BDEW checksum
            lf_mp_id: "9900000000001".to_owned(),
            rechnungsnummer: "R40-PFLICHT-1".to_owned(),
            period: crate::BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
                .unwrap(),
            zaehler_id: Some("1EFW1234567".to_owned()),
            nb_mp_id: Some("9900357000004".to_owned()),
            contract_id: Some("V-2026-042".to_owned()),
            billing_run_id: Some("run-1".to_owned()),
            verbrauchshistorie: Some(crate::context::Verbrauchshistorie {
                vorjahr_kwh: Some(dec!(5800)),
                bundesdurchschnitt_kwh: Some(dec!(3500)),
                kundengruppe: None,
            }),
            vertragsinformationen: Some(crate::context::Vertragsinformationen {
                vertragsdauer: Some("24 Monate".to_owned()),
                kuendigungsfrist: Some("6 Wochen".to_owned()),
                naechstmoeglicher_kuendigungstermin: Some(date!(2026 - 12 - 31)),
                naechster_abrechnungstermin: Some(date!(2027 - 01 - 31)),
            }),
            energiequellen: Some(crate::tariff::EnergieQuellen {
                erneuerbar_pct: dec!(100),
                co2_g_per_kwh: Decimal::ZERO,
                hkn_certified: true,
                ..Default::default()
            }),
            ..BillingContext::default()
        };
        let positions = vec![
            {
                let mut p = BillingPosition::debit(
                    "Arbeitspreis",
                    dec!(500),
                    "kWh",
                    dec!(0.30),
                    PositionCategory::Commodity,
                );
                p.tags.push("strom".to_owned());
                p.tags.push("arbeitspreis".to_owned());
                p
            },
            BillingPosition::debit(
                "MwSt 19 %",
                Decimal::ONE,
                "EUR",
                dec!(28.50),
                PositionCategory::Tax,
            ),
        ];
        let invoice = Invoice::from_positions(ctx, positions, vec![]);
        let rechnung = invoice.to_rechnung();

        // Typed §40/§41 fields.
        assert_eq!(
            rechnung
                .zaehler
                .as_ref()
                .and_then(|z| z[0].zaehlernummer.clone()),
            Some("1EFW1234567".to_owned()),
            "§41 Abs. 1 Nr. 6 — Zählernummer"
        );
        assert_eq!(
            rechnung
                .netzbetreiber
                .as_ref()
                .and_then(|nb| nb.rollencodenummer.as_ref())
                .map(|id| id.as_ref().to_owned()),
            Some("9900357000004".to_owned()),
            "§41 Abs. 1 Nr. 5 — Netzbetreiber"
        );
        assert_eq!(
            rechnung
                .marktlokation
                .as_ref()
                .and_then(|m| m.marktlokations_id.as_ref())
                .map(|id| id.as_ref().to_owned()),
            Some("51238696012".to_owned()),
            "checksum-valid MaLo lands in the typed field"
        );
        assert_eq!(
            rechnung
                .vertrag
                .as_ref()
                .and_then(|v| v.vertragsnummer.clone()),
            Some("V-2026-042".to_owned())
        );
        assert_eq!(
            rechnung.faelligkeitsdatum_date(),
            Some(date!(2026 - 02 - 14)),
            "the schema types this as date-time; the calendar date must survive the promotion"
        );
        let periode = rechnung
            .rechnungsperiode
            .as_ref()
            .expect("rechnungsperiode");
        assert_eq!(periode.startdatum, Some(date!(2026 - 01 - 01)));
        assert_eq!(periode.enddatum, Some(date!(2026 - 01 - 31)));

        // Every ZusatzAttribut Pflichtangabe, by name.
        let attrs = rechnung.zusatz_attribute.as_ref().expect("zusatzAttribute");
        let names: Vec<&str> = attrs.iter().filter_map(|a| a.name.as_deref()).collect();
        for required in [
            "mako:vertragsdauer",                       // §40 Abs. 1
            "mako:kuendigungsfrist",                    // §40 Abs. 1
            "mako:naechstmoeglicher_kuendigungstermin", // §40 Abs. 1
            "mako:naechster_abrechnungstermin",         // §40 Abs. 1
            "mako:verbraucherinformationen",            // §40 Abs. 2 Nr. 1/9/10/11/12
            "mako:kilowattstundenpreis_gesamt",         // §40
            "mako:preisvergleichsdaten",                // §40b
            "mako:verbrauch_vorjahr",                   // §40 Abs. 2 Nr. 7
            "mako:verbrauch_bundesdurchschnitt",        // §40 Abs. 2 Nr. 8
            "mako:stromkennzeichnung",                  // §42
            "mako:billing_run_id",                      // audit trail
            "mako:kundenkategorie",                     // ERP routing
            "mako:vertragsart",                         // §36/§38/§41 regime
        ] {
            assert!(
                names.contains(&required),
                "Pflichtangabe {required:?} missing; present: {names:?}"
            );
        }

        // The §40 Abs. 2 statutory hints fall back to their defaults — never
        // silently absent from a Letztverbraucher invoice.
        let vi = attrs
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:verbraucherinformationen"))
            .and_then(|a| a.wert.clone())
            .expect("verbraucherinformationen wert");
        for key in [
            "schlichtungsstelle",
            "bnetza_verbraucherservice",
            "energieberatung",
            "wechselhinweis",
        ] {
            assert!(
                vi[key].as_str().is_some_and(|s| !s.is_empty()),
                "§40 Abs. 2 hint {key:?} must be non-empty"
            );
        }
    }

    /// Legally relevant totals round-trip Decimal-exact through the JSON —
    /// serialised as strings by rubo4e, never through an `f64`.
    #[cfg(feature = "bo4e")]
    #[test]
    fn money_round_trips_decimal_exact() {
        let ctx = BillingContext {
            abschlage: vec![AbschlagDeduction {
                datum: date!(2026 - 03 - 15),
                betrag_eur: dec!(119.01),
                ust_satz: dec!(0.19),
                beschreibung: None,
            }],
            invoice_type: crate::context::InvoiceType::Final,
            ..BillingContext::default()
        };
        // An awkward net that exercises rounding: 333.335 → totals to the cent.
        let positions = vec![
            BillingPosition::debit(
                "Arbeitspreis",
                dec!(1111.1),
                "kWh",
                dec!(0.30003),
                PositionCategory::Commodity,
            ),
            BillingPosition::debit(
                "MwSt 19 %",
                Decimal::ONE,
                "EUR",
                dec!(63.33),
                PositionCategory::Tax,
            ),
        ];
        let invoice = Invoice::from_positions(ctx, positions, vec![]);
        let json = invoice.to_rechnung_json();
        let back: rubo4e::current::Rechnung =
            serde_json::from_value(json).expect("typed round-trip");

        let wert = |b: &Option<rubo4e::current::Betrag>| b.as_ref().and_then(|b| b.wert);
        assert_eq!(
            wert(&back.gesamtnetto),
            Some(invoice.netto_eur.round_kfm(2))
        );
        assert_eq!(
            wert(&back.gesamtsteuer),
            Some(invoice.mwst_eur.round_kfm(2))
        );
        assert_eq!(
            wert(&back.gesamtbrutto),
            Some(invoice.brutto_eur.round_kfm(2))
        );
        assert_eq!(
            wert(&back.zu_zahlen),
            Some(invoice.zahlbetrag_eur.round_kfm(2))
        );
        assert_eq!(
            back.vorauszahlungen.as_ref().unwrap()[0]
                .betrag
                .as_ref()
                .and_then(|b| b.wert),
            Some(dec!(119.01)),
            "advance gross survives to the exact cent"
        );
        // Position amounts survive at full engine precision (5 dp), exactly.
        let pos = &back.rechnungspositionen.as_ref().unwrap()[0];
        assert_eq!(
            pos.gesamtpreis.as_ref().and_then(|b| b.wert),
            Some(invoice.positions[0].net_eur)
        );
        assert_eq!(invoice.positions[0].net_eur, dec!(333.36333));
        assert_eq!(
            pos.einzelpreis.as_ref().and_then(|p| p.wert),
            Some(dec!(0.30003))
        );
    }

    /// BO4E: the sum of `steuerbetraege` must equal `gesamtsteuer`.
    #[test]
    fn steuerbetraege_sum_to_gesamtsteuer() {
        let invoice = invoice_with_advance();
        let sum: Decimal = invoice
            .tax_subtotals(invoice.context.regulatory_rates.mwst_rate)
            .iter()
            .map(|s| s.tax_amount_eur)
            .sum();
        assert_eq!(sum, invoice.mwst_eur);
    }
}

#[cfg(test)]
mod guthaben_tests {
    use super::*;
    use crate::context::{BillingPeriod, InvoiceType};
    use rust_decimal::dec;
    use time::macros::date;

    fn invoice(zahlbetrag: Decimal, typ: InvoiceType) -> Invoice {
        let context = BillingContext {
            period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 12 - 31))
                .expect("period"),
            issue_date: Some(date!(2027 - 01 - 15)),
            invoice_type: typ,
            ..Default::default()
        };
        Invoice {
            context,
            positions: Vec::new(),
            netto_eur: Decimal::ZERO,
            mwst_eur: Decimal::ZERO,
            brutto_eur: Decimal::ZERO,
            abschlag_total_eur: Decimal::ZERO,
            abschlag_ust_eur: Decimal::ZERO,
            zahlbetrag_eur: zahlbetrag,
            billing_run_id: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn an_invoice_the_customer_owes_creates_no_refund_obligation() {
        assert!(
            invoice(dec!(240), InvoiceType::Final)
                .guthabenerstattung()
                .is_none()
        );
    }

    #[test]
    fn a_credit_on_an_ordinary_settlement_may_be_offset_or_paid_out() {
        // § 40c Abs. 3 Satz 1: offset in full against the next Abschlag, or
        // paid out within two weeks.
        let g = invoice(dec!(-180.50), InvoiceType::Initial)
            .guthabenerstattung()
            .expect("a negative balance is a credit");
        assert_eq!(g.betrag_eur, dec!(180.50), "reported positive, as owed");
        assert!(g.verrechnung_zulaessig);
        assert_eq!(g.spaetestens, date!(2027 - 01 - 29));
        assert_eq!(g.rechtsgrundlage, "§ 40c Abs. 3 Satz 1 EnWG");
    }

    #[test]
    fn a_credit_on_a_schlussrechnung_has_to_be_paid_out() {
        // Satz 2: there is no next Abschlag to offset it against, so offsetting
        // is not an option the supplier has.
        let g = invoice(dec!(-90), InvoiceType::Final)
            .guthabenerstattung()
            .expect("credit");
        assert!(!g.verrechnung_zulaessig);
        assert_eq!(g.rechtsgrundlage, "§ 40c Abs. 3 Satz 2 EnWG");
    }
}

#[cfg(test)]
mod settlement_tests {
    use super::*;
    use crate::context::{AbschlagDeduction, InvoiceType};
    use crate::position::PositionCategory;
    use rust_decimal::dec;
    use time::macros::date;

    /// A year's supply of 1000.00 net + 190.00 VAT, of which 750.00 + 142.50 has
    /// already been collected in advances.
    fn jahresabrechnung() -> Invoice {
        let ctx = BillingContext {
            invoice_type: InvoiceType::Final,
            abschlage: vec![AbschlagDeduction {
                datum: date!(2026 - 06 - 15),
                betrag_eur: dec!(892.50), // 750.00 net + 142.50 VAT
                ust_satz: dec!(0.19),
                beschreibung: Some("Abschläge 2026".to_owned()),
            }],
            ..BillingContext::default()
        };
        let positions = vec![
            BillingPosition::debit(
                "Arbeitspreis",
                dec!(1000),
                "kWh",
                dec!(1.00),
                PositionCategory::Commodity,
            ),
            BillingPosition::debit(
                "MwSt 19 %",
                Decimal::ONE,
                "EUR",
                dec!(190.00),
                PositionCategory::Tax,
            ),
        ];
        Invoice::from_positions(ctx, positions, vec![])
    }

    /// The advance projects into `billing` carrying its own tax, which is the
    /// structure BT-113 cannot hold.
    #[test]
    fn advance_carries_its_own_tax_across_the_boundary() {
        let advances = jahresabrechnung().advance_payments().unwrap();
        assert_eq!(advances.len(), 1);
        assert_eq!(
            advances[0].net(),
            billing::Amount::parse("750.00000").unwrap()
        );
        assert_eq!(
            advances[0].tax_total(),
            billing::Amount::parse("142.50000").unwrap()
        );
        assert_eq!(
            advances[0].gross(),
            billing::Amount::parse("892.50000").unwrap()
        );
    }

    /// Advances present → itemised, never collapsed to a flat total.
    #[test]
    fn prepayment_is_itemised_when_advances_exist() {
        assert!(matches!(
            jahresabrechnung().prepayment().unwrap(),
            billing::Prepayment::Itemised(_)
        ));
        let no_advances = Invoice::from_positions(BillingContext::default(), vec![], vec![]);
        assert!(matches!(
            no_advances.prepayment().unwrap(),
            billing::Prepayment::None
        ));
    }

    /// Restrechnung: the residual is the supply less what the advances taxed.
    #[test]
    fn residual_breakdown_bills_only_the_remainder() {
        let residual = jahresabrechnung().residual_breakdown(dec!(0.19)).unwrap();
        assert_eq!(residual.len(), 1);
        assert_eq!(
            residual[0].taxable_base,
            billing::Amount::parse("250.00000").unwrap()
        );
        assert_eq!(
            residual[0].tax_amount,
            billing::Amount::parse("47.50000").unwrap()
        );
    }

    /// Over-deduction is refused: it would understate the output tax owed on the
    /// supply, which is the error §14c Abs. 1 exists to punish.
    #[test]
    fn advances_exceeding_the_supply_are_refused() {
        let mut invoice = jahresabrechnung();
        invoice.context.abschlage[0].betrag_eur = dec!(2000.00);
        assert!(invoice.residual_breakdown(dec!(0.19)).is_err());
    }
}

#[cfg(test)]
mod correction_tests {
    use super::*;
    use crate::context::{AbschlagDeduction, InvoiceType};
    use crate::position::PositionCategory;
    use rust_decimal::dec;
    use time::macros::date;

    /// A correction negates every monetary figure, and the VAT breakdown has to
    /// travel with `gesamtsteuer` — BO4E requires the Steuerbeträge to sum to it,
    /// so a breakdown left positive contradicts the negated total it belongs to.
    #[cfg(feature = "bo4e")]
    #[test]
    fn correction_negates_the_vat_breakdown_with_the_total() {
        let ctx = BillingContext {
            invoice_type: InvoiceType::Final,
            abschlage: vec![AbschlagDeduction {
                datum: date!(2026 - 01 - 15),
                betrag_eur: dec!(119.00),
                ust_satz: dec!(0.19),
                beschreibung: None,
            }],
            ..BillingContext::default()
        };
        let positions = vec![
            BillingPosition::debit(
                "Arbeitspreis",
                dec!(1000),
                "kWh",
                dec!(0.30),
                PositionCategory::Commodity,
            ),
            BillingPosition::debit(
                "MwSt 19 %",
                Decimal::ONE,
                "EUR",
                dec!(57.00),
                PositionCategory::Tax,
            ),
        ];
        let json = Invoice::from_positions(ctx, positions, vec![]).to_rechnung_json();
        let corrected = negate_rechnung_json_for_correction(&json, "ORIG-1", "KORR-1");

        let steuer = &corrected["steuerbetraege"][0];
        assert_eq!(steuer["basiswert"], serde_json::json!("-300.00"));
        assert_eq!(steuer["steuerwert"], serde_json::json!("-57.00"));
        assert_eq!(
            corrected["gesamtsteuer"]["wert"],
            serde_json::json!("-57.00")
        );

        // The advance reverses with the document that settles it.
        assert_eq!(
            corrected["vorauszahlungen"][0]["betrag"]["wert"],
            serde_json::json!("-119.00")
        );
    }
}

#[cfg(test)]
mod trace_emission_tests {
    use super::*;
    use crate::position::PositionCategory;
    use rust_decimal::dec;

    /// The calculation trace must survive into the stored Rechnung.
    ///
    /// `PositionTrace` was serializable from the start and never emitted:
    /// `to_rechnung_json` dropped it, so billingd's explain tool read a field
    /// that was always null — while its own note promised seven trace fields.
    #[cfg(feature = "bo4e")]
    #[test]
    fn the_position_trace_reaches_the_stored_rechnung() {
        let mut pos = BillingPosition::debit(
            "Arbeitspreis",
            dec!(1000),
            "kWh",
            dec!(0.30),
            PositionCategory::Commodity,
        );
        pos.trace.formula = "1000 kWh x 0.30 EUR/kWh".to_owned();
        pos.trace.regulatory_basis = vec!["§40 EnWG".to_owned()];

        let invoice = Invoice::from_positions(BillingContext::default(), vec![pos], vec![]);
        let json = invoice.to_rechnung_json();

        // Extract exactly the way the MCP explain tool does.
        let trace = json["rechnungspositionen"][0]["zusatzAttribute"]
            .as_array()
            .expect("position carries attributes")
            .iter()
            .find(|a| a["name"] == "mako:calculation_trace")
            .and_then(|a| a.get("wert"))
            .expect("mako:calculation_trace present");

        assert_eq!(trace["formula"], "1000 kWh x 0.30 EUR/kWh");
        assert_eq!(trace["regulatory_basis"][0], "§40 EnWG");
        assert!(trace.get("input_quantity").is_some(), "{trace}");
    }
}
