//! INVOIC plausibility check engine — operates on BO4E [`Rechnung`].
//!
//! [`InvoicCheckEngine::check`] runs a multi-stage pipeline of automated
//! plausibility checks against a [`rubo4e::v202501::Rechnung`] and returns a
//! [`CheckReport`] that drives the REMADV / dispute workflow in `invoicd`.
//!
//! # Check stages
//!
//! | Stage | Finding kind | Outcome |
//! |---|---|---|
//! | Period check | [`FindingKind::PeriodInvalid`] | `Dispute` |
//! | Arithmetic check | [`FindingKind::ArithmeticError`] | `Dispute` |
//! | Total check | [`FindingKind::TotalMismatch`] | `Warn` |
//! | Tariff check | [`FindingKind::TariffDeviation`] | `Warn` or `Dispute` |
//!
//! # Outcome escalation
//!
//! The overall [`CheckOutcome`] is the highest-severity outcome across all
//! findings.  A single `Dispute`-severity finding escalates the whole invoice
//! to `Dispute`.  Warn-only findings produce `Warn`.  A clean invoice is `Ok`.
//!
//! # Architecture
//!
//! This module has **zero dependency on `edifact-rs`**.  It operates solely on
//! [`rubo4e::v202501::Rechnung`] — the industry-standard BO4E domain model.
//! EDIFACT → BO4E translation is the responsibility of the `makod` transport
//! adapter (anti-corruption layer).
//!
//! # Example
//!
//! ```rust
//! use invoic_checker::check::{CheckConfig, CheckOutcome, InvoicCheckEngine};
//! use invoic_checker::tariff::InMemoryPreisblattStore;
//! use rubo4e::v202501::Rechnung;
//!
//! // An empty preisblatt store yields a TariffNotFound *warning* (not a dispute)
//! // even for an invoice with no line items, because the tariff check always
//! // runs and flags unknown sender GLNs with is_dispute = require_tariff.
//! let report = InvoicCheckEngine::check(
//!     31001,
//!     "9900357000004",
//!     &Rechnung::default(),
//!     &InMemoryPreisblattStore::new(),
//!     &CheckConfig::default(),
//! );
//! assert_eq!(report.outcome, CheckOutcome::Warn);
//! ```

use rubo4e::convenience::{BetragExt, MengeExt, PreisExt};
use rubo4e::v202501::{Rechnung, Rechnungsposition};
use rust_decimal::prelude::ToPrimitive as _;

use crate::{amount::EuroAmount, tariff::PreisblattStore};

// ── CheckConfig ───────────────────────────────────────────────────────────────

/// Configuration for [`InvoicCheckEngine::check`].
#[derive(Debug, Clone)]
pub struct CheckConfig {
    /// Tolerance for arithmetic checks (line quantity × unit price vs. line net).
    ///
    /// Default: `0.01` (1 %). Increase for rough invoice types (e.g. MMM
    /// settlement that uses SLP approximations).
    pub arithmetic_tolerance: f64,

    /// Tolerance for the cross-check between sum of line nets and total net.
    ///
    /// Default: `0.01` (1 %).
    pub total_tolerance: f64,

    /// Tolerance for tariff deviation findings.
    ///
    /// Default: `0.02` (2 %).
    pub tariff_tolerance: f64,

    /// When `true`, a missing tariff entry for the sender GLN produces a
    /// `Dispute`-severity finding.  When `false` (default), it produces `Warn`.
    ///
    /// Set to `true` once the tariff store is fully seeded and the LF has
    /// received PRICAT 27003 from all active NB counterparties.
    pub require_tariff: bool,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            arithmetic_tolerance: 0.01,
            total_tolerance: 0.01,
            tariff_tolerance: 0.02,
            require_tariff: false,
        }
    }
}

// ── CheckOutcome ──────────────────────────────────────────────────────────────

/// Overall outcome of an automated INVOIC check.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum CheckOutcome {
    /// All checks passed.  Safe to auto-dispatch REMADV 33001.
    Ok,
    /// Non-blocking issues found.  Route to operator for review before payment.
    Warn,
    /// Blocking issues found.  Open dispute process; do NOT auto-pay.
    Dispute,
}

// ── FindingKind ───────────────────────────────────────────────────────────────

/// Structured category of a check finding.
///
/// Each variant maps to a specific regulatory dispute reason that can be cited
/// in a REMADV or COMDIS.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FindingKind {
    /// A billing period is invalid (start ≥ end, or missing a boundary).
    PeriodInvalid,
    /// Line item `quantity × unit_price` does not match `teilsumme_netto`.
    ArithmeticError,
    /// Sum of line net amounts does not match the message-level `gesamtnetto`.
    TotalMismatch,
    /// INVOIC unit price deviates from the PRICAT-published tariff.
    TariffDeviation,
    /// No PRICAT tariff exists in the store for this sender GLN.
    TariffNotFound,
}

// ── Finding ───────────────────────────────────────────────────────────────────

/// A single finding from the check engine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    /// Category of this finding.
    pub kind: FindingKind,
    /// Whether this finding alone escalates the outcome to `Dispute` (vs. `Warn`).
    pub is_dispute: bool,
    /// Human-readable description.
    pub message: String,
    /// Line item `positionsnummer` this finding applies to.  `None` for
    /// message-level findings (e.g. total mismatch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_number: Option<u32>,
    /// Expected amount (for numeric comparisons).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<EuroAmount>,
    /// Actual amount from the INVOIC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<EuroAmount>,
    /// Deviation as a percentage of expected (positive = overbilling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation_pct: Option<f64>,
}

impl Finding {
    fn dispute(
        kind: FindingKind,
        message: impl Into<String>,
        line_number: Option<u32>,
        expected: Option<EuroAmount>,
        actual: Option<EuroAmount>,
    ) -> Self {
        let deviation_pct = deviation(expected, actual);
        Self {
            kind,
            is_dispute: true,
            message: message.into(),
            line_number,
            expected,
            actual,
            deviation_pct,
        }
    }

    fn warn(
        kind: FindingKind,
        message: impl Into<String>,
        line_number: Option<u32>,
        expected: Option<EuroAmount>,
        actual: Option<EuroAmount>,
    ) -> Self {
        let deviation_pct = deviation(expected, actual);
        Self {
            kind,
            is_dispute: false,
            message: message.into(),
            line_number,
            expected,
            actual,
            deviation_pct,
        }
    }
}

fn deviation(expected: Option<EuroAmount>, actual: Option<EuroAmount>) -> Option<f64> {
    match (expected, actual) {
        (Some(exp), Some(act)) if exp.0 != 0 => {
            Some((act.0 - exp.0) as f64 / exp.0.unsigned_abs() as f64 * 100.0)
        }
        _ => None,
    }
}

// ── CheckReport ───────────────────────────────────────────────────────────────

/// Full report from [`InvoicCheckEngine::check`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckReport {
    /// Overall outcome — highest severity across all findings.
    pub outcome: CheckOutcome,
    /// Ordered list of findings (empty when `outcome == Ok`).
    pub findings: Vec<Finding>,
    /// BDEW Prüfidentifikator from the checked INVOIC.
    pub pid: u32,
    /// Total net amount as stated in `Rechnung.gesamtnetto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_net_invoic: Option<EuroAmount>,
    /// Total net amount as re-computed by summing `Rechnungsposition.teilsumme_netto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_net_computed: Option<EuroAmount>,
    /// Number of `Rechnungsposition` entries checked.
    pub line_items_checked: usize,
}

impl CheckReport {
    /// `true` when the invoice passed all checks without findings.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.outcome == CheckOutcome::Ok
    }

    /// `true` when at least one finding escalates to `Dispute`.
    #[must_use]
    pub fn has_dispute(&self) -> bool {
        self.outcome == CheckOutcome::Dispute
    }
}

// ── InvoicCheckEngine ─────────────────────────────────────────────────────────

/// Stateless INVOIC plausibility check engine.
///
/// All logic is in [`InvoicCheckEngine::check`], which is a pure function over
/// a [`rubo4e::v202501::Rechnung`].  No state is held between calls.
pub struct InvoicCheckEngine;

impl InvoicCheckEngine {
    /// Run all plausibility checks and return a [`CheckReport`].
    ///
    /// # Arguments
    ///
    /// - `pid` — BDEW Prüfidentifikator (31001–31011) from `AbrechnungData`.
    /// - `sender_mp_id` — verified sender GLN from `AbrechnungData.sender`
    ///   (identity-checked at transport layer; used for tariff lookups).
    /// - `rechnung` — BO4E invoice object stored in the event.
    /// - `tariff_store` — tariff database seeded from PRICAT 27003.
    /// - `config` — tolerance and policy configuration.
    #[must_use]
    pub fn check(
        pid: u32,
        sender_mp_id: &str,
        rechnung: &Rechnung,
        preisblatt_store: &dyn PreisblattStore,
        config: &CheckConfig,
    ) -> CheckReport {
        let mut findings: Vec<Finding> = Vec::new();

        // ── Stage 1: Period validity ──────────────────────────────────────────
        Self::check_periods(rechnung, &mut findings);

        // ── Stage 2: Arithmetic (qty × unit_price ≈ teilsumme_netto) ─────────
        Self::check_arithmetic(rechnung, config, &mut findings);

        // ── Stage 3: Total consistency (Σ teilsumme_netto ≈ gesamtnetto) ─────
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // ── Stage 4: Tariff check (PRICAT vs INVOIC unit price) ───────────────
        Self::check_tariffs(
            rechnung,
            sender_mp_id,
            preisblatt_store,
            config,
            &mut findings,
        );

        // ── Derive overall outcome ────────────────────────────────────────────
        let outcome = findings
            .iter()
            .map(|f| {
                if f.is_dispute {
                    CheckOutcome::Dispute
                } else {
                    CheckOutcome::Warn
                }
            })
            .max()
            .unwrap_or(CheckOutcome::Ok);

        let total_net_invoic = rechnung
            .gesamtnetto
            .wert_decimal()
            .and_then(EuroAmount::from_decimal);

        CheckReport {
            outcome,
            findings,
            pid,
            total_net_invoic,
            total_net_computed: computed_total,
            line_items_checked: rechnung.rechnungspositionen.iter().flatten().count(),
        }
    }

    // ── Stage implementations ──────────────────────────────────────────────────

    /// Stage 1: Verify that every billing period has start < end.
    ///
    /// All period fields are native `time::Date` in rubo4e v0.4 — compared
    /// directly. `billing_period()` returns `None` when either bound is absent,
    /// which correctly skips the check for partially-specified periods.
    fn check_periods(rechnung: &Rechnung, findings: &mut Vec<Finding>) {
        // Message-level period (Rechnungsperiode) via convenience method.
        if let Some((start, end)) = rechnung.billing_period()
            && start >= end
        {
            findings.push(Finding::dispute(
                FindingKind::PeriodInvalid,
                format!("Message-level billing period invalid: start {start} ≥ end {end}"),
                None,
                None,
                None,
            ));
        }
        // Line-level periods (Lieferung von/bis).
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            if let (Some(start), Some(end)) =
                (pos.lieferung_von.as_ref(), pos.lieferung_bis.as_ref())
                && start >= end
            {
                let (line_no, malo) = pos_ident(pos);
                findings.push(Finding::dispute(
                    FindingKind::PeriodInvalid,
                    format!(
                        "Line {line_no} ({malo}) billing period invalid: start {start} ≥ end {end}"
                    ),
                    Some(line_no),
                    None,
                    None,
                ));
            }
        }
    }

    /// Stage 2: For each position with quantity + unit_price, verify
    /// `positions_menge × einzelpreis ≈ teilsumme_netto`.
    fn check_arithmetic(rechnung: &Rechnung, config: &CheckConfig, findings: &mut Vec<Finding>) {
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let qty = pos.positions_menge.wert_decimal().and_then(|d| d.to_f64());
            let price = pos
                .einzelpreis
                .wert_decimal()
                .and_then(EuroAmount::from_decimal);
            let stated_net = pos
                .teilsumme_netto
                .wert_decimal()
                .and_then(EuroAmount::from_decimal);

            if let (Some(qty), Some(price), Some(stated_net)) = (qty, price, stated_net) {
                let computed = price.multiply_by_kwh(qty);
                if !stated_net.within_tolerance(computed, config.arithmetic_tolerance) {
                    let (line_no, malo) = pos_ident(pos);
                    findings.push(Finding {
                        kind: FindingKind::ArithmeticError,
                        is_dispute: true,
                        message: format!(
                            "Line {line_no} ({malo}): \
                             {qty} kWh × {price} EUR/kWh = {computed} EUR, \
                             but Rechnungsposition states {stated_net} EUR",
                        ),
                        line_number: Some(line_no),
                        expected: Some(computed),
                        actual: Some(stated_net),
                        deviation_pct: deviation(Some(computed), Some(stated_net)),
                    });
                }
            }
        }
    }

    /// Stage 3: Verify Σ `teilsumme_netto` ≈ `gesamtnetto`.
    ///
    /// Returns the computed sum (used in the `CheckReport`).
    fn check_total(
        rechnung: &Rechnung,
        config: &CheckConfig,
        findings: &mut Vec<Finding>,
    ) -> Option<EuroAmount> {
        let line_nets: Vec<EuroAmount> = rechnung
            .rechnungspositionen
            .iter()
            .flatten()
            .filter_map(|pos| {
                pos.teilsumme_netto
                    .wert_decimal()
                    .and_then(EuroAmount::from_decimal)
            })
            .collect();

        if line_nets.is_empty() {
            return None;
        }

        let computed = line_nets
            .iter()
            .copied()
            .fold(EuroAmount::ZERO, |acc, a| acc + a);

        if let Some(stated) = rechnung
            .gesamtnetto
            .wert_decimal()
            .and_then(EuroAmount::from_decimal)
            && !stated.within_tolerance(computed, config.total_tolerance)
        {
            findings.push(Finding::warn(
                FindingKind::TotalMismatch,
                format!(
                    "Total net mismatch: Σ teilsumme_netto = {computed} EUR, \
                     gesamtnetto = {stated} EUR",
                ),
                None,
                Some(computed),
                Some(stated),
            ));
        }

        Some(computed)
    }

    /// Stage 4: Compare `einzelpreis` against the tariff store (PRICAT 27003).
    fn check_tariffs(
        rechnung: &Rechnung,
        sender_mp_id: &str,
        preisblatt_store: &dyn PreisblattStore,
        config: &CheckConfig,
        findings: &mut Vec<Finding>,
    ) {
        // Use billing_period() start or fall back to the invoice document date.
        // Both are native time::Date in rubo4e v0.4.
        let billing_date: time::Date = rechnung
            .billing_period()
            .map(|(start, _)| start)
            .or(rechnung.rechnungsdatum)
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        if !preisblatt_store.has_preisblatt_for(sender_mp_id) {
            findings.push(Finding {
                kind: FindingKind::TariffNotFound,
                is_dispute: config.require_tariff,
                message: format!(
                    "No PRICAT tariff found for sender GLN {sender_mp_id} on {billing_date}. \
                     Tariff check skipped — seed the tariff store from PRICAT 27003.",
                ),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            });
            return;
        }

        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let Some(invoic_price) = pos
                .einzelpreis
                .wert_decimal()
                .and_then(EuroAmount::from_decimal)
            else {
                continue;
            };
            let (line_no, malo) = pos_ident(pos);
            // lieferung_von is Option<time::Date> in rubo4e v0.4 — B-03 fixed,
            // no .date() extraction needed.
            let line_date = pos.lieferung_von.unwrap_or(billing_date);

            let Some(preisblatt) = preisblatt_store.get(sender_mp_id, line_date) else {
                findings.push(Finding::warn(
                    FindingKind::TariffNotFound,
                    format!(
                        "Line {line_no} ({malo}): no Preisblatt effective on {line_date} \
                         for GLN {sender_mp_id}",
                    ),
                    Some(line_no),
                    None,
                    Some(invoic_price),
                ));
                continue;
            };

            // Collect all published Einheitspreise from all Preispositionen and
            // their Preisstaffeln.  If the INVOIC Einzelpreis is within tolerance
            // of ANY published rate, the line passes.
            let tol = config.tariff_tolerance;
            let published: Vec<EuroAmount> = preisblatt
                .preispositionen
                .iter()
                .flatten()
                .flat_map(|pp| pp.preisstaffeln.iter().flatten())
                .filter_map(|ps| ps.einheitspreis)
                .filter_map(EuroAmount::from_decimal)
                .collect();

            if published.is_empty() {
                findings.push(Finding::warn(
                    FindingKind::TariffNotFound,
                    format!(
                        "Line {line_no} ({malo}): Preisblatt for GLN {sender_mp_id} \
                         on {line_date} contains no Preisstaffeln — skipping price check",
                    ),
                    Some(line_no),
                    None,
                    Some(invoic_price),
                ));
                continue;
            }

            if !published
                .iter()
                .any(|p| invoic_price.within_tolerance(*p, tol))
            {
                // Report the closest published rate for diagnostics.
                let closest = *published
                    .iter()
                    .min_by_key(|p| (invoic_price.0 - p.0).unsigned_abs())
                    .unwrap_or(&EuroAmount::ZERO);
                findings.push(Finding::dispute(
                    FindingKind::TariffDeviation,
                    format!(
                        "Line {line_no} ({malo}): einzelpreis {invoic_price} EUR/kWh \
                         does not match any published rate in Preisblatt for GLN {sender_mp_id} \
                         on {line_date} (closest: {closest} EUR/kWh, tolerance {pct:.1}%)",
                        pct = tol * 100.0,
                    ),
                    Some(line_no),
                    Some(closest),
                    Some(invoic_price),
                ));
            }
        }
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Extract a stable (line_number, malo_id) pair for error messages.
fn pos_ident(pos: &Rechnungsposition) -> (u32, &str) {
    let line_no = pos.positionsnummer.unwrap_or(0) as u32;
    let malo = pos.lokations_id.as_deref().unwrap_or("-");
    (line_no, malo)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rubo4e::v202501::{
        Betrag, Menge, Mengeneinheit, Preis, Rechnung, Rechnungsposition, Zeitraum,
    };
    use rust_decimal::Decimal;

    use super::*;
    use crate::{amount::EuroAmount, tariff::InMemoryPreisblattStore};
    use rubo4e::v202501::{PreisblattNetznutzung, Preisposition, Preisstaffel};

    const SENDER: &str = "9900357000004";

    fn betrag(eur: EuroAmount) -> Betrag {
        Betrag {
            wert: Some(Decimal::from_str_exact(&eur.to_eur_string()).expect("valid decimal")),
            ..Default::default()
        }
    }

    /// Parse a `"YYYY-MM-DD"` string to `time::Date` (rubo4e v0.3 field type).
    fn parse_date(s: &str) -> time::Date {
        time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
            .expect("valid ISO date")
    }

    /// Parse a `"YYYY-MM-DD"` string to midnight UTC `OffsetDateTime`.
    fn periode(start: &str, end: &str) -> Zeitraum {
        Zeitraum {
            startdatum: Some(parse_date(start)),
            enddatum: Some(parse_date(end)),
            ..Default::default()
        }
    }

    fn make_pos(
        n: i64,
        malo: &str,
        qty: Option<f64>,
        price: Option<EuroAmount>,
        net: Option<EuroAmount>,
    ) -> Rechnungsposition {
        Rechnungsposition {
            positionsnummer: Some(n),
            lokations_id: Some(malo.to_owned()),
            lieferung_von: Some(parse_date("2024-12-01")),
            lieferung_bis: Some(parse_date("2024-12-31")),
            positions_menge: qty.map(|q| Menge {
                wert: Some(Decimal::try_from(q).expect("valid f64")),
                einheit: Some(Mengeneinheit::Kwh),
                ..Default::default()
            }),
            einzelpreis: price.map(|pr| Preis {
                wert: Some(Decimal::from_str_exact(&pr.to_eur_string()).expect("valid decimal")),
                ..Default::default()
            }),
            teilsumme_netto: net.map(betrag),
            ..Default::default()
        }
    }

    fn make_rechnung(
        positions: Vec<Rechnungsposition>,
        gesamtnetto: Option<EuroAmount>,
    ) -> Rechnung {
        Rechnung {
            rechnungsperiode: Some(periode("2024-12-01", "2024-12-31")),
            rechnungsdatum: Some(parse_date("2025-01-15")),
            gesamtnetto: gesamtnetto.map(betrag),
            rechnungspositionen: if positions.is_empty() {
                None
            } else {
                Some(positions)
            },
            ..Default::default()
        }
    }

    fn empty_store() -> InMemoryPreisblattStore {
        InMemoryPreisblattStore::new()
    }

    fn seeded_store(price: EuroAmount) -> InMemoryPreisblattStore {
        use rust_decimal::Decimal;
        let mut store = InMemoryPreisblattStore::new();
        let einheitspreis = Decimal::from_str_exact(&price.to_eur_string()).expect("valid decimal");
        let sheet = PreisblattNetznutzung {
            gueltigkeit: None,
            herausgeber: None,
            preispositionen: Some(vec![Preisposition {
                preisstaffeln: Some(vec![Preisstaffel {
                    einheitspreis: Some(einheitspreis),
                    ..Default::default()
                }]),
                ..Default::default()
            }]),
            ..Default::default()
        };
        store.insert(SENDER.to_owned(), sheet);
        store
    }

    // ── Period check ──────────────────────────────────────────────────────────

    #[test]
    fn period_start_gte_end_is_dispute() {
        let mut r = make_rechnung(vec![], None);
        r.rechnungsperiode = Some(periode("2024-12-31", "2024-12-01"));
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::PeriodInvalid)
        );
    }

    #[test]
    fn period_valid_no_finding() {
        let r = make_rechnung(vec![], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::PeriodInvalid)
        );
    }

    #[test]
    fn line_period_invalid_is_dispute() {
        let mut pos = make_pos(1, "DE001", None, None, None);
        pos.lieferung_von = Some(parse_date("2024-12-31"));
        pos.lieferung_bis = Some(parse_date("2024-12-01"));
        let r = make_rechnung(vec![pos], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(report.has_dispute());
        assert_eq!(report.findings[0].line_number, Some(1));
    }

    // ── Arithmetic check ──────────────────────────────────────────────────────

    #[test]
    fn arithmetic_correct_no_finding() {
        // 1000 kWh × 0.03456 EUR/kWh = 34.56000 EUR
        let pos = make_pos(
            1,
            "DE001",
            Some(1000.0),
            Some(EuroAmount(3_456)),
            Some(EuroAmount(3_456_000)),
        );
        let r = make_rechnung(vec![pos], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ArithmeticError)
        );
    }

    #[test]
    fn arithmetic_mismatch_is_dispute() {
        // 1000 × 0.03456 = 34.56, but invoice says 40.00
        let pos = make_pos(
            1,
            "DE001",
            Some(1000.0),
            Some(EuroAmount(3_456)),
            Some(EuroAmount(4_000_000)),
        );
        let r = make_rechnung(vec![pos], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ArithmeticError)
        );
    }

    #[test]
    fn arithmetic_within_tolerance_no_finding() {
        // 1% tolerance: 34.56 vs 34.90 → ~0.98% deviation → no finding
        let pos = make_pos(
            1,
            "DE001",
            Some(1000.0),
            Some(EuroAmount(3_456)),
            Some(EuroAmount(3_490_000)),
        );
        let config = CheckConfig {
            arithmetic_tolerance: 0.01,
            ..Default::default()
        };
        let r = make_rechnung(vec![pos], None);
        let report = InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &config);
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ArithmeticError)
        );
    }

    // ── Total check ───────────────────────────────────────────────────────────

    #[test]
    fn total_match_no_finding() {
        let pos = make_pos(1, "DE001", None, None, Some(EuroAmount(3_456_000)));
        let r = make_rechnung(vec![pos], Some(EuroAmount(3_456_000)));
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TotalMismatch)
        );
    }

    #[test]
    fn total_mismatch_is_warn() {
        let pos = make_pos(1, "DE001", None, None, Some(EuroAmount(3_456_000)));
        let r = make_rechnung(vec![pos], Some(EuroAmount(5_000_000)));
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(!report.has_dispute()); // warn only
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TotalMismatch)
        );
    }

    // ── Tariff check ──────────────────────────────────────────────────────────

    #[test]
    fn no_tariff_warn_by_default() {
        let r = make_rechnung(vec![], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(!report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffNotFound)
        );
    }

    #[test]
    fn no_tariff_dispute_when_required() {
        let config = CheckConfig {
            require_tariff: true,
            ..Default::default()
        };
        let r = make_rechnung(vec![], None);
        let report = InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &config);
        assert!(report.has_dispute());
    }

    #[test]
    fn tariff_match_no_finding() {
        let price = EuroAmount(3_456);
        let pos = make_pos(
            1,
            "DE001",
            Some(1000.0),
            Some(price),
            Some(EuroAmount(3_456_000)),
        );
        let r = make_rechnung(vec![pos], None);
        let report = InvoicCheckEngine::check(
            31001,
            SENDER,
            &r,
            &seeded_store(price),
            &CheckConfig::default(),
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffDeviation)
        );
    }

    #[test]
    fn tariff_deviation_is_dispute() {
        let tariff_price = EuroAmount(3_456); // 0.03456 EUR/kWh (PRICAT)
        let invoic_price = EuroAmount(4_000); // 0.04000 EUR/kWh (INVOIC, +15.7%)
        let pos = make_pos(
            1,
            "DE001",
            Some(1000.0),
            Some(invoic_price),
            Some(EuroAmount(4_000_000)),
        );
        let r = make_rechnung(vec![pos], None);
        let report = InvoicCheckEngine::check(
            31001,
            SENDER,
            &r,
            &seeded_store(tariff_price),
            &CheckConfig::default(),
        );
        assert!(report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffDeviation)
        );
    }

    #[test]
    fn clean_invoice_outcome_is_ok() {
        let price = EuroAmount(3_456);
        let net = EuroAmount(3_456_000);
        let pos = make_pos(1, "DE001", Some(1000.0), Some(price), Some(net));
        let r = make_rechnung(vec![pos], Some(net));
        let report = InvoicCheckEngine::check(
            31001,
            SENDER,
            &r,
            &seeded_store(price),
            &CheckConfig::default(),
        );
        assert_eq!(report.outcome, CheckOutcome::Ok);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn pid_is_carried_in_report() {
        let r = make_rechnung(vec![], None);
        let report =
            InvoicCheckEngine::check(31005, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert_eq!(report.pid, 31005);
    }
}
