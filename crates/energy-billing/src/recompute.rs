//! Rechnung nachrechnen — recompute a stored invoice from what it states.
//!
//! An invoice the customer, a court or the Schlichtungsstelle can recompute to
//! the cent from its own lines is the cheapest answer to a dispute. This module
//! takes the BO4E `Rechnung` exactly as it was stored and checks every number
//! it states against the numbers it is made of, with the engine's own rounding
//! (kaufmännisch, DIN 1333):
//!
//! | Stated | Recomputed from |
//! |---|---|
//! | a line's `gesamtpreis` | its `positionsMenge` × `einzelpreis`, to the cent |
//! | `gesamtnetto` | the sum of the lines |
//! | a `steuerbetrag`'s `steuerwert` | its `basiswert` × `steuersatz`, to the cent |
//! | the bases of all `steuerbetraege` | `gesamtnetto` |
//! | `gesamtsteuer` | the sum of the `steuerwert`s |
//! | `gesamtbrutto` | `gesamtnetto` + `gesamtsteuer` |
//! | `zuZahlen` | `gesamtbrutto` − the `vorauszahlungen` (an Abschlagsrechnung deducts none) |
//!
//! VAT and Abschläge are not positions on a BO4E Rechnung — the engine states
//! them as `steuerbetraege` and `vorauszahlungen` — so the totals are checked
//! against those, not against lines.
//!
//! Recomputing proves the arithmetic, not the inputs: a wrong reading
//! recomputes as faithfully as a right one. The inputs are returned beside each
//! line — quantity and how it was obtained, unit price and its source, the rule
//! — so a person can check them.

use rust_decimal::Decimal;
use serde::Serialize;

use crate::position::PositionTrace;
use crate::rates::RoundMoney;

/// One line of the invoice, recomputed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecomputedLine {
    /// The line's `positionsnummer`, from 1.
    pub position: i64,
    /// The line's text as printed.
    pub text: String,
    /// Quantity as stated.
    pub quantity: Option<Decimal>,
    /// Unit, as the BO4E `Mengeneinheit` or mako's own spelling.
    pub unit: Option<String>,
    /// Unit price in EUR as stated, full precision.
    pub unit_price_eur: Option<Decimal>,
    /// The line amount the invoice states.
    pub stated_eur: Option<Decimal>,
    /// Quantity × unit price, to the cent. `None` when either is missing.
    pub recomputed_eur: Option<Decimal>,
    /// `stated − recomputed`; zero when the line matches.
    pub difference_eur: Option<Decimal>,
    /// The line recomputes to the cent.
    pub matches: bool,
    /// The calculation as the engine wrote it, when it did.
    pub formula: Option<String>,
    /// The provisions the line bills under — the trace's citations, else the
    /// line's own `mako:rechtliche_grundlage`.
    pub rules: Vec<String>,
    /// Where the unit price comes from (a product sheet, a Preisblatt row).
    pub price_source: Option<String>,
    /// How the quantity was obtained (§ 40 Abs. 2 Nr. 6 EnWG), when known.
    pub quantity_source: Option<String>,
    /// The quantity is an estimate or an Ersatzwert, not a reading (§ 40a EnWG).
    pub estimated: bool,
}

/// Which document total a [`RecomputedTotal`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TotalKind {
    /// `gesamtnetto` against the sum of the lines.
    Netto,
    /// One `steuerbetrag` against its base × rate.
    Steuer,
    /// The bases of all `steuerbetraege` against `gesamtnetto`.
    Steuerbasis,
    /// `gesamtsteuer` against the sum of the `steuerwert`s.
    Gesamtsteuer,
    /// `gesamtbrutto` against netto + Steuer.
    Brutto,
    /// `zuZahlen` against brutto − Vorauszahlungen.
    ZuZahlen,
}

/// One document total, recomputed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecomputedTotal {
    /// Which total.
    pub kind: TotalKind,
    /// The VAT rate in percent, for [`TotalKind::Steuer`].
    pub rate_percent: Option<Decimal>,
    /// The amount the invoice states.
    pub stated_eur: Option<Decimal>,
    /// The amount recomputed from what it is made of.
    pub recomputed_eur: Decimal,
    /// The two agree to the cent.
    pub matches: bool,
}

/// The whole invoice, recomputed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recomputation {
    /// Every line, in the invoice's order.
    pub lines: Vec<RecomputedLine>,
    /// Every document total.
    pub totals: Vec<RecomputedTotal>,
    /// Sum of the `vorauszahlungen` the invoice deducts.
    pub vorauszahlungen_eur: Decimal,
    /// Every line and every total recomputes to the cent.
    pub matches: bool,
}

impl Recomputation {
    /// The positions whose stated amount does not recompute.
    #[must_use]
    pub fn mismatched_lines(&self) -> Vec<i64> {
        self.lines
            .iter()
            .filter(|l| !l.matches)
            .map(|l| l.position)
            .collect()
    }
}

fn attribut<'a>(
    attrs: Option<&'a Vec<rubo4e::current::ZusatzAttribut>>,
    name: &str,
) -> Option<&'a serde_json::Value> {
    attrs?
        .iter()
        .find(|a| a.name.as_deref() == Some(name))
        .and_then(|a| a.wert.as_ref())
}

fn betrag(b: Option<&rubo4e::current::Betrag>) -> Option<Decimal> {
    b.and_then(|b| b.wert)
}

fn total(kind: TotalKind, stated: Option<Decimal>, recomputed: Decimal) -> RecomputedTotal {
    RecomputedTotal {
        kind,
        rate_percent: None,
        stated_eur: stated,
        recomputed_eur: recomputed,
        matches: stated == Some(recomputed),
    }
}

/// Recompute every line and every total of a stored invoice.
#[must_use]
pub fn recompute(rechnung: &rubo4e::current::Rechnung) -> Recomputation {
    let mut lines = Vec::new();
    for (i, p) in rechnung
        .rechnungspositionen
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let attrs = p.zusatz_attribute.as_ref();
        let trace: Option<PositionTrace> = attribut(attrs, "mako:calculation_trace")
            .and_then(|t| serde_json::from_value(t.clone()).ok());
        let quantity = p.positions_menge.as_ref().and_then(|m| m.wert);
        let unit = attribut(attrs, "mako:einheit")
            .and_then(|v| v.as_str().map(ToOwned::to_owned))
            .or_else(|| {
                p.positions_menge
                    .as_ref()
                    .and_then(|m| m.einheit.as_ref())
                    .and_then(|e| serde_json::to_value(e).ok())
                    .and_then(|v| v.as_str().map(ToOwned::to_owned))
            });
        let unit_price = p.einzelpreis.as_ref().and_then(|e| e.wert);
        let stated = betrag(p.gesamtpreis.as_ref());
        let recomputed = quantity
            .zip(unit_price)
            .map(|(q, price)| (q * price).round_kfm(2));
        let difference = stated.zip(recomputed).map(|(s, r)| s - r);
        let mut rules: Vec<String> = trace
            .as_ref()
            .map(|t| t.regulatory_basis.clone())
            .unwrap_or_default();
        if rules.is_empty()
            && let Some(lb) =
                attribut(attrs, "mako:rechtliche_grundlage").and_then(serde_json::Value::as_str)
        {
            rules.push(lb.to_owned());
        }
        lines.push(RecomputedLine {
            position: p
                .positionsnummer
                .unwrap_or_else(|| i64::try_from(i + 1).unwrap_or(i64::MAX)),
            text: p.positionstext.clone().unwrap_or_default(),
            quantity,
            unit,
            unit_price_eur: unit_price,
            stated_eur: stated,
            recomputed_eur: recomputed,
            difference_eur: difference,
            matches: stated.is_some() && stated == recomputed,
            formula: trace
                .as_ref()
                .map(|t| t.formula.clone())
                .filter(|f| !f.is_empty()),
            rules,
            price_source: trace.as_ref().and_then(|t| t.tariff_source.clone()),
            quantity_source: trace.as_ref().and_then(|t| t.quantity_source.clone()),
            estimated: trace.as_ref().is_some_and(|t| t.estimated),
        });
    }

    let mut totals = Vec::new();
    let netto_lines: Decimal = lines.iter().filter_map(|l| l.stated_eur).sum();
    let netto = betrag(rechnung.gesamtnetto.as_ref());
    totals.push(total(TotalKind::Netto, netto, netto_lines));

    let steuerbetraege = rechnung.steuerbetraege.as_deref().unwrap_or_default();
    let mut steuer_sum = Decimal::ZERO;
    let mut basis_sum = Decimal::ZERO;
    for s in steuerbetraege {
        let basis = s.basiswert.unwrap_or_default();
        let satz = s.steuersatz.unwrap_or_default();
        basis_sum += basis;
        let recomputed = (basis * satz / Decimal::ONE_HUNDRED).round_kfm(2);
        steuer_sum += s.steuerwert.unwrap_or_default();
        totals.push(RecomputedTotal {
            kind: TotalKind::Steuer,
            rate_percent: Some(satz),
            stated_eur: s.steuerwert,
            recomputed_eur: recomputed,
            matches: s.steuerwert == Some(recomputed),
        });
    }
    if !steuerbetraege.is_empty() {
        totals.push(total(
            TotalKind::Steuerbasis,
            Some(basis_sum),
            netto.unwrap_or(netto_lines),
        ));
    }
    let steuer = betrag(rechnung.gesamtsteuer.as_ref());
    totals.push(total(TotalKind::Gesamtsteuer, steuer, steuer_sum));

    let brutto = betrag(rechnung.gesamtbrutto.as_ref());
    totals.push(total(
        TotalKind::Brutto,
        brutto,
        netto.unwrap_or(netto_lines) + steuer.unwrap_or(steuer_sum),
    ));

    // An Abschlagsrechnung asks for the advance itself and deducts nothing.
    let is_abschlag = matches!(
        rechnung.rechnungstyp,
        Some(rubo4e::current::Rechnungstyp::Abschlagsrechnung)
    );
    let vorauszahlungen: Decimal = if is_abschlag {
        Decimal::ZERO
    } else {
        rechnung
            .vorauszahlungen
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| betrag(v.betrag.as_ref()))
            .sum()
    };
    totals.push(total(
        TotalKind::ZuZahlen,
        betrag(rechnung.zu_zahlen.as_ref()),
        brutto.unwrap_or_default() - vorauszahlungen,
    ));

    let matches = lines.iter().all(|l| l.matches) && totals.iter().all(|t| t.matches);
    Recomputation {
        lines,
        totals,
        vorauszahlungen_eur: vorauszahlungen,
        matches,
    }
}
