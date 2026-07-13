//! INVOIC plausibility check engine — operates on BO4E [`Rechnung`].
//!
//! [`InvoicCheckEngine::check`] runs a multi-stage pipeline of automated
//! plausibility checks against a [`rubo4e::current::Rechnung`] and returns a
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
//! [`rubo4e::current::Rechnung`] — the industry-standard BO4E domain model.
//! EDIFACT → BO4E translation is the responsibility of the `makod` transport
//! adapter (anti-corruption layer).
//!
//! # Example
//!
//! ```rust
//! use invoic_checker::check::{CheckConfig, CheckOutcome, InvoicCheckEngine};
//! use invoic_checker::tariff::InMemoryPreisblattStore;
//! use rubo4e::current::Rechnung;
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
use rubo4e::current::{Rechnung, Rechnungsposition};

use crate::{amount::EuroAmount, tariff::PreisblattStore};

// ── CheckConfig ───────────────────────────────────────────────────────────────

/// Configuration for [`InvoicCheckEngine::check`].
#[derive(Debug, Clone)]
pub struct CheckConfig {
    /// Tolerance for arithmetic checks (line quantity × unit price vs. line net),
    /// expressed in parts-per-million (ppm). Unsigned — zero means strict equality.
    ///
    /// Default: `10_000` ppm = 1 %. Increase for rough invoice types (e.g. MMM
    /// settlement that uses SLP approximations).
    pub arithmetic_tolerance_ppm: u32,

    /// Tolerance for the cross-check between sum of line nets and total net.
    ///
    /// Default: `10_000` ppm = 1 %.
    pub total_tolerance_ppm: u32,

    /// Tolerance for tariff deviation findings.
    ///
    /// Default: `20_000` ppm = 2 %.
    pub tariff_tolerance_ppm: u32,

    /// When `true`, a missing tariff entry for the sender GLN produces a
    /// `Dispute`-severity finding.  When `false` (default), it produces `Warn`.
    ///
    /// Set to `true` once the tariff store is fully seeded and the LF has
    /// received PRICAT 27003 from all active NB counterparties.
    pub require_tariff: bool,

    /// Maximum allowed payment term (Zahlungsziel) in days from the invoice date
    /// (`rechnungsdatum`) to the due date (`faelligkeitsdatum`, DTM+92).
    ///
    /// Per §7 Allgemeine Festlegungen V6.1d: standard GPKE and WiM payment term
    /// is **30 days**. Set to `0` to disable this check.
    ///
    /// Default: `30`.
    pub max_zahlungsziel_days: u16,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            arithmetic_tolerance_ppm: 10_000,
            total_tolerance_ppm: 10_000,
            tariff_tolerance_ppm: 20_000,
            require_tariff: false,
            max_zahlungsziel_days: 30,
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
    /// Line item `quantity × unit_price` does not match `gesamtpreis` (BO4E v202607).
    ArithmeticError,
    /// Sum of line net amounts does not match the message-level `gesamtnetto`.
    TotalMismatch,
    /// INVOIC unit price deviates from the PRICAT-published tariff.
    TariffDeviation,
    /// No PRICAT tariff exists in the store for this sender GLN.
    TariffNotFound,
    /// `ist_storno = true` but `original_rechnungsnummer` is absent.
    ///
    /// Per BK6-24-174 §5: a Stornorechnung must reference the original invoice
    /// number so the LF can reconcile it against the original receipt.
    StorniertWithoutReference,
    /// `faelligkeitsdatum` (DTM+92) exceeds the maximum allowed payment term.
    ///
    /// Basis: §7 Allgemeine Festlegungen V6.1d — standard GPKE/WiM payment
    /// term is 30 days from invoice date.
    ZahlungszielExceeded,
    /// `faelligkeitsdatum` (DTM+92) is in the past or before `rechnungsdatum`.
    ZahlungszielInvalid,
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
        (Some(exp), Some(act)) if exp.to_raw() != 0 => {
            Some((act.to_raw() - exp.to_raw()) as f64 / exp.to_raw().unsigned_abs() as f64 * 100.0)
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
    /// Total net amount as re-computed by summing `Rechnungsposition.gesamtpreis` (BO4E v202607).
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

/// Return `true` when `rechnung` is a Stornorechnung (cancellation invoice).
///
/// A Stornorechnung is identified by `ist_storno = Some(true)`.
/// When true, the tariff check (stage 4) must be skipped — cancellations
/// do not carry original tariff positions, they carry negated amounts.
///
/// The presence of `original_rechnungsnummer` is checked separately by
/// `InvoicCheckEngine::check` (finding kind `StorniertWithoutReference`).
///
/// # Example
///
/// ```rust
/// use invoic_checker::check::is_stornierung;
/// use rubo4e::current::Rechnung;
/// let mut r = Rechnung::default();
/// r.ist_storno = Some(true);
/// assert!(is_stornierung(&r));
/// ```
#[must_use]
pub fn is_stornierung(rechnung: &Rechnung) -> bool {
    rechnung.ist_storno == Some(true)
}

/// Stateless INVOIC plausibility check engine.
///
/// All logic is in [`InvoicCheckEngine::check`], which is a pure function over
/// a [`rubo4e::current::Rechnung`].  No state is held between calls.
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

        let storno = is_stornierung(rechnung);

        // ── Stage 0: Stornierung reference check ──────────────────────────────
        // When ist_storno=true, original_rechnungsnummer must be present.
        // Source: BK6-24-174 §5; Allgemeine Festlegungen §8.
        if storno
            && rechnung
                .original_rechnungsnummer
                .as_deref()
                .unwrap_or("")
                .is_empty()
        {
            findings.push(Finding::dispute(
                FindingKind::StorniertWithoutReference,
                "Stornorechnung (ist_storno=true) does not reference the original invoice \
                 (original_rechnungsnummer is missing). \
                 Source: BK6-24-174 §5; Allgemeine Festlegungen §8.",
                None,
                None,
                None,
            ));
        }

        // ── Stage 1: Period validity ──────────────────────────────────────────
        Self::check_periods(rechnung, &mut findings);

        // ── Stage 1.5: Zahlungsziel check ─────────────────────────────────────
        // DTM+92 (faelligkeitsdatum) must not exceed max_zahlungsziel_days.
        // Source: §7 Allgemeine Festlegungen V6.1d; BK6-22-024 §5.
        if config.max_zahlungsziel_days > 0 {
            Self::check_zahlungsziel(rechnung, config, &mut findings);
        }

        // ── Stage 2: Arithmetic (qty × unit_price ≈ gesamtpreis) ─────────────
        Self::check_arithmetic(rechnung, config, &mut findings);

        // ── Stage 3: Total consistency (Σ gesamtpreis ≈ gesamtnetto) ──────────
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // ── Stage 4: Tariff check (PRICAT vs INVOIC unit price) ───────────────
        // Skipped for Stornorechnungen: they carry negated original amounts,
        // not tariff positions. Skipping prevents false TariffDeviation disputes.
        if !storno {
            Self::check_tariffs(
                rechnung,
                sender_mp_id,
                preisblatt_store,
                config,
                &mut findings,
            );
        }

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

    /// Stage 1.5: Validate `faelligkeitsdatum` (Zahlungsziel / DTM+92).
    ///
    /// Checks:
    /// - If `faelligkeitsdatum < rechnungsdatum`: invalid (past due before issued).
    ///   Produces a `Dispute` finding (`ZahlungszielInvalid`).
    /// - If `faelligkeitsdatum - rechnungsdatum > max_zahlungsziel_days`:
    ///   exceeds contractual/regulatory payment term.
    ///   Produces a `Warn` finding (`ZahlungszielExceeded`).
    ///
    /// Source: §7 Allgemeine Festlegungen V6.1d (30 days standard);
    ///         BK6-22-024 §5; BK7-24-01-009 §5.
    fn check_zahlungsziel(rechnung: &Rechnung, config: &CheckConfig, findings: &mut Vec<Finding>) {
        let Some(faellig) = rechnung.faelligkeitsdatum else {
            return; // DTM+92 absent — not required on all PID types
        };
        let rechnungs_datum = match rechnung.rechnungsdatum {
            Some(d) => d,
            None => return, // Cannot compute term without invoice date
        };

        if faellig < rechnungs_datum {
            findings.push(Finding::dispute(
                FindingKind::ZahlungszielInvalid,
                format!(
                    "Zahlungsziel {faellig} is before invoice date {rechnungs_datum}. \
                     DTM+92 must not precede rechnungsdatum. \
                     Source: §7 Allgemeine Festlegungen V6.1d.",
                ),
                None,
                None,
                None,
            ));
            return;
        }

        let days = (faellig - rechnungs_datum).whole_days();
        let max = config.max_zahlungsziel_days as i64;
        if max > 0 && days > max {
            findings.push(Finding {
                kind: FindingKind::ZahlungszielExceeded,
                is_dispute: false, // Warn, not Dispute — give the NB a chance to correct
                message: format!(
                    "Zahlungsziel is {days} days (from {rechnungs_datum} to {faellig}), \
                     exceeding the {max}-day maximum per §7 Allgemeine Festlegungen V6.1d. \
                     Review before payment.",
                ),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: Some(days as f64 - max as f64),
            });
        }
    }

    /// Stage 1: Verify that every billing period has start < end.
    ///
    /// All period fields are native `time::Date` in rubo4e v0.5 — compared
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
        // Line-level periods (Lieferung von/bis via lieferungszeitraum).
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            if let (Some(start), Some(end)) = (pos.lieferung_von_date(), pos.lieferung_bis_date())
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
    /// `positions_menge × einzelpreis ≈ gesamtpreis` (BO4E v202607).
    ///
    /// Uses `billing::Amount::checked_sub` + `checked_mul_qty` — no `f64`
    /// intermediate — satisfying the §40 EnWG itemised-billing accuracy requirement.
    fn check_arithmetic(rechnung: &Rechnung, config: &CheckConfig, findings: &mut Vec<Finding>) {
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let qty = pos.positions_menge.wert_decimal();
            let price = pos
                .einzelpreis
                .wert_decimal()
                .and_then(EuroAmount::from_decimal);
            let stated_net = pos
                .gesamtpreis
                .wert_decimal()
                .and_then(EuroAmount::from_decimal);

            if let (Some(qty), Some(price), Some(stated_net)) = (qty, price, stated_net) {
                let computed = price.mul_qty(qty);
                if !stated_net
                    .within_tolerance_ppm(computed, config.arithmetic_tolerance_ppm)
                    .unwrap_or(false)
                {
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

    /// Stage 3: Verify Σ `gesamtpreis` ≈ `gesamtnetto`.
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
                pos.gesamtpreis
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
            && !stated
                .within_tolerance_ppm(computed, config.total_tolerance_ppm)
                .unwrap_or(false)
        {
            findings.push(Finding::warn(
                FindingKind::TotalMismatch,
                format!(
                    "Total net mismatch: \u{03a3} gesamtpreis = {computed} EUR, \
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
        // Both are native time::Date in rubo4e v0.5.
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
            // lieferung_von_date() reads lieferungszeitraum.startdatum (v202607).
            let line_date = pos.lieferung_von_date().unwrap_or(billing_date);

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

            // Collect published prices split into flat and ToU (§14a Modul 2) sets.
            //
            // - `flat_prices`: prices from `Preisposition.preisstaffeln`
            //   (flat Arbeitspreis, Leistungspreis, Grundpreis)
            // - `tou_prices`: prices from `zeitvariablePreispositionen` extension
            //   (HT/NT band prices per §14a Modul 2 BK6-22-300)
            //
            // ToU-aware matching (L3):
            //   • Position text contains "HT" (Hochlast/Hochtarif) → only `tou_prices`
            //   • Position text contains "NT" (Niedertarif) → only `tou_prices`
            //   • All others → `flat_prices` (primary) then fallback to all prices
            //
            // This prevents a ToU-banded NB INVOIC from accidentally passing
            // plausibility when a flat band price coincidentally equals a ToU rate.
            let tol = config.tariff_tolerance_ppm;
            let flat_prices: Vec<EuroAmount> = preisblatt
                .preispositionen
                .iter()
                .flatten()
                .flat_map(|pp| pp.preisstaffeln.iter().flatten())
                .filter_map(|ps| ps.preis)
                .filter_map(EuroAmount::from_decimal)
                .collect();

            // Extract (zaehlzeitregister, price) pairs from zeitvariablePreispositionen.
            // Band codes are validated on PUT (M5) — every entry has a non-empty register.
            use rubo4e::json::Bo4eExtensionData as _;
            let tou_bands: Vec<(String, EuroAmount)> = preisblatt
                .extension_data()
                .get("zeitvariablePreispositionen")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|entry| {
                            let register = entry
                                .get("zaehlzeitregister")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_owned();
                            let price_val = entry
                                .get("preis")
                                .and_then(|p| p.get("wert"))
                                .and_then(|w| w.as_str())
                                .and_then(|s| rust_decimal::Decimal::from_str_exact(s).ok())
                                .and_then(EuroAmount::from_decimal)?;
                            Some((register, price_val))
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Determine which band(s) apply to this INVOIC position.
            // 1. Try direct `zaehlzeitregister` match (case-insensitive contains).
            // Match position text against published `zaehlzeitregister` band codes.
            let pos_text = pos.positionstext.as_deref().unwrap_or("").to_lowercase();

            let matching_band_prices: Vec<EuroAmount> = tou_bands
                .iter()
                .filter(|(code, _)| {
                    let code_lc = code.to_lowercase();
                    !code_lc.is_empty() && pos_text.contains(code_lc.as_str())
                })
                .map(|(_, price)| *price)
                .collect();

            let all_tou_prices: Vec<EuroAmount> = tou_bands.iter().map(|(_, p)| *p).collect();

            let published: Vec<EuroAmount> = if !matching_band_prices.is_empty() {
                // Direct zaehlzeitregister match — most precise.
                matching_band_prices
            } else if !flat_prices.is_empty() {
                // No matching band: use flat prices.
                flat_prices.clone()
            } else {
                // No flat prices — fall back to all ToU band prices.
                all_tou_prices
            };

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
                .any(|p| invoic_price.within_tolerance_ppm(*p, tol).unwrap_or(false))
            {
                // Report the closest published rate for diagnostics.
                let closest = *published
                    .iter()
                    .min_by_key(|p| (invoic_price.to_raw() - p.to_raw()).unsigned_abs())
                    .unwrap_or(&EuroAmount::ZERO);
                findings.push(Finding::dispute(
                    FindingKind::TariffDeviation,
                    format!(
                        "Line {line_no} ({malo}): einzelpreis {invoic_price} EUR/kWh \
                         does not match any published rate in Preisblatt for GLN {sender_mp_id} \
                         on {line_date} (closest: {closest} EUR/kWh, tolerance {pct:.1}%)",
                        pct = tol as f64 / 10_000.0,
                    ),
                    Some(line_no),
                    Some(closest),
                    Some(invoic_price),
                ));
            }
        }
    }

    // ── Stage 5: MMM settlement price check ──────────────────────────────────

    /// Check 6 (MMM settlement): validate that `mehr_preis` / `minder_preis`
    /// positions in an MMM INVOIC (PIDs 31002, 31005, 31007, 31008) match the
    /// reference prices from the `marktd` MMMA store within tolerance.
    ///
    /// Check a WiM MSB-Rechnung (PID 31009) against `PreisblattMessung`.
    ///
    /// Replaces the standard `check()` call for PID 31009.  The key difference:
    /// - Checks 1–3 (period, arithmetic, totals) run identically.
    /// - Checks 4–5 (tariff deviation / not found) use `PreisblattMessung.preispositionen`
    ///   instead of `PreisblattNetznutzung.preispositionen`.
    ///
    /// `PreisblattMessung` has `preispositionen: Option<Vec<Preisposition>>` — the same type
    /// as `PreisblattNetznutzung` — so the price extraction logic is identical.
    ///
    /// When `preisblatt_messung` is `None`, tariff checks (4–5) emit warnings
    /// (never hard disputes) to match the standard engine's missing-tariff behaviour.
    #[must_use]
    pub fn check_msb_rechnung(
        sender_mp_id: &str,
        rechnung: &Rechnung,
        preisblatt_messung: Option<&rubo4e::current::PreisblattMessung>,
        config: &CheckConfig,
    ) -> CheckReport {
        Self::check_msb_rechnung_with_aufabschlaege(
            sender_mp_id,
            rechnung,
            preisblatt_messung,
            &[],
            config,
        )
    }

    /// MSB-Rechnung (INVOIC 31009) plausibility check with `AufAbschlag` validation.
    ///
    /// Extends `check_msb_rechnung` with check 6:
    ///
    /// | # | Check | Source |
    /// |---|---|---|
    /// | 6 | Discount/surcharge positions are backed by a contracted `AufAbschlag` | WiM PRICAT 27001–27003 |
    ///
    /// `contracted_names` is the list of contracted AufAbschlag names from
    /// `PreisblattMessungRecord.auf_abschlaege` (pre-extracted by the caller).
    /// Pass `&[]` when absent (check 6 is then skipped, not disputed).
    pub fn check_msb_rechnung_with_aufabschlaege(
        sender_mp_id: &str,
        rechnung: &Rechnung,
        preisblatt_messung: Option<&rubo4e::current::PreisblattMessung>,
        contracted_names: &[String],
        config: &CheckConfig,
    ) -> CheckReport {
        let mut findings = Vec::new();

        // Checks 1–3 are identical to the standard pipeline.
        Self::check_periods(rechnung, &mut findings);
        Self::check_arithmetic(rechnung, config, &mut findings);
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // Checks 4–5 against PreisblattMessung.preispositionen.
        let billing_date: time::Date = rechnung
            .billing_period()
            .map(|(start, _)| start)
            .or(rechnung.rechnungsdatum)
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        let published_prices: Vec<EuroAmount> = preisblatt_messung
            .and_then(|pm| pm.preispositionen.as_ref())
            .into_iter()
            .flatten()
            .flat_map(|pp| pp.preisstaffeln.iter().flatten())
            .filter_map(|ps| ps.preis)
            .filter_map(EuroAmount::from_decimal)
            .collect();

        if preisblatt_messung.is_none() {
            findings.push(Finding {
                kind: FindingKind::TariffNotFound,
                is_dispute: config.require_tariff,
                message: format!(
                    "No PreisblattMessung found for MSB GLN {sender_mp_id} on {billing_date}. \
                     Tariff check 4/5 skipped — upload via \
                     PUT /api/v1/preisblaetter-messung/{{msb_mp_id}}.",
                ),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            });
        } else {
            let tol = config.tariff_tolerance_ppm;
            for pos in rechnung.rechnungspositionen.iter().flatten() {
                let Some(invoic_price) = pos
                    .einzelpreis
                    .wert_decimal()
                    .and_then(EuroAmount::from_decimal)
                else {
                    continue;
                };
                let (line_no, malo) = pos_ident(pos);

                if published_prices.is_empty() {
                    findings.push(Finding::warn(
                        FindingKind::TariffNotFound,
                        format!(
                            "Line {line_no} ({malo}): PreisblattMessung for GLN \
                             {sender_mp_id} contains no Preisstaffeln — skipping price check",
                        ),
                        Some(line_no),
                        None,
                        Some(invoic_price),
                    ));
                    continue;
                }

                if !published_prices
                    .iter()
                    .any(|p| invoic_price.within_tolerance_ppm(*p, tol).unwrap_or(false))
                {
                    let closest = *published_prices
                        .iter()
                        .min_by_key(|p| (invoic_price.to_raw() - p.to_raw()).unsigned_abs())
                        .unwrap_or(&EuroAmount::ZERO);
                    findings.push(Finding::dispute(
                        FindingKind::TariffDeviation,
                        format!(
                            "Line {line_no} ({malo}): einzelpreis {invoic_price} does not \
                             match any MSB tariff in PreisblattMessung for GLN {sender_mp_id} \
                             on {billing_date} (closest: {closest}, tolerance {pct:.1}%)",
                            pct = tol as f64 / 10_000.0,
                        ),
                        Some(line_no),
                        Some(closest),
                        Some(invoic_price),
                    ));
                }
            }
        }

        // Check 6 — AufAbschlag: verify discount/surcharge positions are contracted.
        // `contracted_names` contains the lowercase names of all authorised AufAbschlag
        // entries from the MSB's PRICAT 27001–27003.  When empty, check 6 is skipped.
        if !contracted_names.is_empty() {
            let name_set: std::collections::HashSet<String> =
                contracted_names.iter().map(|s| s.to_lowercase()).collect();

            for pos in rechnung.rechnungspositionen.iter().flatten() {
                let net = pos.einzelpreis.wert_decimal().unwrap_or_default();
                if net >= rust_decimal::Decimal::ZERO {
                    continue; // Only check negative (discount) positions
                }
                let (line_no, malo) = pos_ident(pos);
                let description = pos.positionstext.as_deref().unwrap_or("").to_lowercase();

                let is_contracted = name_set
                    .iter()
                    .any(|name: &String| description.contains(name.as_str()));

                if !is_contracted {
                    findings.push(Finding::dispute(
                        FindingKind::TariffNotFound,
                        format!(
                            "Line {line_no} ({malo}): discount \"{}\" not backed by \
                             any AufAbschlag in PreisblattMessung for GLN {sender_mp_id} \
                             (check 6). Verify PRICAT 27001-27003.",
                            pos.positionstext.as_deref().unwrap_or("?"),
                        ),
                        Some(line_no),
                        None,
                        None,
                    ));
                }
            }
        }

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
            pid: 31009,
            total_net_invoic,
            total_net_computed: computed_total,
            line_items_checked: rechnung.rechnungspositionen.iter().flatten().count(),
        }
    }

    /// Arithmetic-only check for Stornorechnungen (cancellation invoices).
    ///
    /// Runs stages 0–3 (Storno reference, period, arithmetic, totals) only.
    /// Stage 4 (tariff check) is skipped because a Stornierung carries negated
    /// amounts from the original invoice, not new tariff positions.
    ///
    /// Returns a `CheckReport` with outcome `AcceptedPartial` when all checks
    /// pass (represented as `Ok` in `CheckOutcome` — the `AcceptedPartial` label
    /// is set by `invoicd` when it detects a Storno outcome).
    ///
    /// Call this instead of `check()` when you know the invoice is a Storno
    /// (either by PID routing — e.g. PID 31004 — or by `is_stornierung()` check).
    ///
    /// # Example
    ///
    /// ```rust
    /// use invoic_checker::check::{CheckConfig, CheckOutcome, InvoicCheckEngine, is_stornierung};
    /// use rubo4e::current::Rechnung;
    ///
    /// let mut r = Rechnung::default();
    /// r.ist_storno = Some(true);
    /// r.original_rechnungsnummer = Some("31001-2026-001".to_owned());
    /// assert!(is_stornierung(&r));
    ///
    /// let report = InvoicCheckEngine::check_storno(31004, &r, &CheckConfig::default());
    /// assert_eq!(report.outcome, CheckOutcome::Ok);
    /// ```
    #[must_use]
    pub fn check_storno(pid: u32, rechnung: &Rechnung, config: &CheckConfig) -> CheckReport {
        let mut findings = Vec::new();

        // Stage 0: Storno reference must be present.
        if rechnung
            .original_rechnungsnummer
            .as_deref()
            .unwrap_or("")
            .is_empty()
        {
            findings.push(Finding::dispute(
                FindingKind::StorniertWithoutReference,
                "Stornorechnung does not reference the original invoice \
                 (original_rechnungsnummer is missing). Source: BK6-24-174 §5.",
                None,
                None,
                None,
            ));
        }

        // Stage 1: Period validity (same as full check).
        Self::check_periods(rechnung, &mut findings);

        // Stage 1.5: Zahlungsziel check.
        if config.max_zahlungsziel_days > 0 {
            Self::check_zahlungsziel(rechnung, config, &mut findings);
        }

        // Stage 2 + 3: Arithmetic and total (still apply to Storno amounts).
        Self::check_arithmetic(rechnung, config, &mut findings);
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // Stage 4: SKIPPED — Storno carries negated original amounts.

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

    /// Called by `invoicd` handler **after** the standard 5-check pipeline when
    /// `mehr_ct_kwh` / `minder_ct_kwh` are available from `marktd`.
    ///
    /// Returns additional `Finding` objects to be merged into an existing
    /// `CheckReport`. Does not modify the existing findings.
    pub fn check_mmm_settlement(
        rechnung: &Rechnung,
        mehr_ct_kwh: rust_decimal::Decimal,
        minder_ct_kwh: rust_decimal::Decimal,
        config: &CheckConfig,
    ) -> Vec<Finding> {
        let tol = config.tariff_tolerance_ppm;

        // Convert reference prices from ct/kWh → EUR/kWh
        let ref_mehr = EuroAmount::from_decimal(mehr_ct_kwh / rust_decimal::Decimal::from(100));
        let ref_minder = EuroAmount::from_decimal(minder_ct_kwh / rust_decimal::Decimal::from(100));

        let mut findings = Vec::new();

        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let Some(invoic_price) = pos
                .einzelpreis
                .wert_decimal()
                .and_then(EuroAmount::from_decimal)
            else {
                continue;
            };
            let (line_no, malo) = pos_ident(pos);
            let text = pos.positionstext.as_deref().unwrap_or("").to_lowercase();
            let is_mehr = text.contains("mehrmengen");
            let is_minder = text.contains("mindermengen");
            if !is_mehr && !is_minder {
                continue;
            }
            let Some(ref_p) = (if is_mehr { ref_mehr } else { ref_minder }) else {
                continue;
            };

            if !invoic_price
                .within_tolerance_ppm(ref_p, tol)
                .unwrap_or(false)
            {
                let ref_raw = ref_p.to_raw() as f64;
                let pct = if ref_raw != 0.0 {
                    ((invoic_price.to_raw() as f64 - ref_raw) / ref_raw.abs() * 100.0).abs()
                } else {
                    0.0
                };
                let kind_str = if is_mehr {
                    "Mehrmengen"
                } else {
                    "Mindermengen"
                };
                findings.push(Finding {
                    kind: FindingKind::TariffDeviation,
                    is_dispute: config.require_tariff,
                    message: format!(
                        "Line {line_no} ({malo}): MMM {kind_str} price {invoic_price} EUR/kWh                          deviates {pct:.1}% from MMMA reference {ref_p} EUR/kWh                          (tolerance {t:.1}%)",
                        t = tol as f64 / 10_000.0,
                    ),
                    line_number: Some(line_no),
                    expected: Some(ref_p),
                    actual: Some(invoic_price),
                    deviation_pct: Some(pct),
                });
            }
        }
        findings
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Extract a stable (line_number, malo_id) pair for error messages.
fn pos_ident(pos: &Rechnungsposition) -> (u32, &str) {
    let line_no = pos.positionsnummer.unwrap_or(0) as u32;
    // `lokations_id` was removed in BO4E v202607; fall back to positionstext.
    let malo = pos.positionstext.as_deref().unwrap_or("-");
    (line_no, malo)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rubo4e::current::{
        Betrag, Menge, Mengeneinheit, Preis, Rechnung, Rechnungsposition, Zeitraum,
    };
    use rust_decimal::Decimal;

    use super::*;
    use crate::{amount::EuroAmount, tariff::InMemoryPreisblattStore};
    use rubo4e::current::{PreisblattNetznutzung, Preisposition, Preisstaffel};

    const SENDER: &str = "9900357000004";

    fn betrag(eur: EuroAmount) -> Betrag {
        Betrag {
            wert: Some(Decimal::from_str_exact(&eur.to_string()).expect("valid decimal")),
            ..Default::default()
        }
    }

    /// Parse a `"YYYY-MM-DD"` string to `time::Date` (rubo4e v0.5 field type).
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
        qty: Option<&str>,
        price: Option<EuroAmount>,
        net: Option<EuroAmount>,
    ) -> Rechnungsposition {
        Rechnungsposition {
            positionsnummer: Some(n),
            // lokations_id removed in v202607; use positionstext for test ident.
            positionstext: Some(malo.to_owned()),
            lieferungszeitraum: Some(periode("2024-12-01", "2024-12-31")),
            positions_menge: qty.map(|q| Menge {
                wert: Some(Decimal::from_str_exact(q).expect("valid decimal literal")),
                einheit: Some(Mengeneinheit::Kwh),
                ..Default::default()
            }),
            einzelpreis: price.map(|pr| Preis {
                wert: Some(Decimal::from_str_exact(&pr.to_string()).expect("valid decimal")),
                ..Default::default()
            }),
            gesamtpreis: net.map(betrag),
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
        let einheitspreis = Decimal::from_str_exact(&price.to_string()).expect("valid decimal");
        let sheet = PreisblattNetznutzung {
            gueltigkeit: None,
            herausgeber: None,
            preispositionen: Some(vec![Preisposition {
                preisstaffeln: Some(vec![Preisstaffel {
                    preis: Some(einheitspreis),
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
        // Override the lieferungszeitraum to an invalid range (start > end).
        pos.lieferungszeitraum = Some(periode("2024-12-31", "2024-12-01"));
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
            Some("1000.0"),
            Some(EuroAmount::from_raw_units(3_456)),
            Some(EuroAmount::from_raw_units(3_456_000)),
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
            Some("1000.0"),
            Some(EuroAmount::from_raw_units(3_456)),
            Some(EuroAmount::from_raw_units(4_000_000)),
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
            Some("1000.0"),
            Some(EuroAmount::from_raw_units(3_456)),
            Some(EuroAmount::from_raw_units(3_490_000)),
        );
        let config = CheckConfig {
            arithmetic_tolerance_ppm: 10_000,
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
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(3_456_000)),
        );
        let r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(3_456_000)));
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
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(3_456_000)),
        );
        let r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(5_000_000)));
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
        let price = EuroAmount::from_raw_units(3_456);
        let pos = make_pos(
            1,
            "DE001",
            Some("1000.0"),
            Some(price),
            Some(EuroAmount::from_raw_units(3_456_000)),
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
        let tariff_price = EuroAmount::from_raw_units(3_456); // 0.03456 EUR/kWh (PRICAT)
        let invoic_price = EuroAmount::from_raw_units(4_000); // 0.04000 EUR/kWh (INVOIC, +15.7%)
        let pos = make_pos(
            1,
            "DE001",
            Some("1000.0"),
            Some(invoic_price),
            Some(EuroAmount::from_raw_units(4_000_000)),
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
        let price = EuroAmount::from_raw_units(3_456);
        let net = EuroAmount::from_raw_units(3_456_000);
        let pos = make_pos(1, "DE001", Some("1000.0"), Some(price), Some(net));
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

    // ── Stornierung tests ─────────────────────────────────────────────────────

    #[test]
    fn stornierung_with_reference_skips_tariff_check() {
        // A valid Storno: ist_storno=true + original_rechnungsnummer present.
        // Tariff stage must be skipped — no TariffNotFound finding expected.
        let price = EuroAmount::from_raw_units(3_456);
        let net = EuroAmount::from_raw_units(3_456_000);
        let pos = make_pos(1, "DE001", Some("1000.0"), Some(price), Some(net));
        let mut r = make_rechnung(vec![pos], Some(net));
        r.ist_storno = Some(true);
        r.original_rechnungsnummer = Some("31001-2025-0042".to_owned());

        // Empty tariff store — would produce TariffNotFound if tariff stage ran.
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert_eq!(
            report.outcome,
            CheckOutcome::Ok,
            "Storno with valid ref + correct arithmetic should be Ok"
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffNotFound),
            "Tariff stage must be skipped for Stornierung"
        );
    }

    #[test]
    fn stornierung_without_reference_is_dispute() {
        // ist_storno=true but original_rechnungsnummer absent → StorniertWithoutReference.
        let mut r = make_rechnung(vec![], None);
        r.ist_storno = Some(true);
        r.original_rechnungsnummer = None;

        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::StorniertWithoutReference),
            "Missing original_rechnungsnummer must produce StorniertWithoutReference"
        );
    }

    #[test]
    fn is_stornierung_predicate() {
        let mut r = Rechnung::default();
        assert!(!is_stornierung(&r), "default Rechnung is not a Storno");
        r.ist_storno = Some(true);
        assert!(is_stornierung(&r), "ist_storno=true → is Storno");
        r.ist_storno = Some(false);
        assert!(!is_stornierung(&r), "ist_storno=false → not Storno");
    }

    #[test]
    fn check_storno_clean_returns_ok() {
        let price = EuroAmount::from_raw_units(3_456);
        let net = EuroAmount::from_raw_units(3_456_000);
        let pos = make_pos(1, "DE001", Some("1000.0"), Some(price), Some(net));
        let mut r = make_rechnung(vec![pos], Some(net));
        r.ist_storno = Some(true);
        r.original_rechnungsnummer = Some("31001-2025-0042".to_owned());

        let report = InvoicCheckEngine::check_storno(31004, &r, &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Ok);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn check_storno_without_reference_is_dispute() {
        let mut r = make_rechnung(vec![], None);
        r.ist_storno = Some(true);
        r.original_rechnungsnummer = None;

        let report = InvoicCheckEngine::check_storno(31004, &r, &CheckConfig::default());
        assert!(report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::StorniertWithoutReference)
        );
    }

    // ── Zahlungsziel tests ────────────────────────────────────────────────────

    #[test]
    fn zahlungsziel_within_limit_no_finding() {
        let mut r = make_rechnung(vec![], None);
        r.rechnungsdatum = Some(parse_date("2026-07-01"));
        r.faelligkeitsdatum = Some(parse_date("2026-07-31")); // exactly 30 days

        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ZahlungszielExceeded),
            "Exactly 30 days is within the default limit"
        );
    }

    #[test]
    fn zahlungsziel_exceeded_is_warn() {
        let mut r = make_rechnung(vec![], None);
        r.rechnungsdatum = Some(parse_date("2026-07-01"));
        r.faelligkeitsdatum = Some(parse_date("2026-09-01")); // 62 days — exceeds 30

        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        let finding = report
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::ZahlungszielExceeded);
        assert!(
            finding.is_some(),
            "62-day payment term must produce ZahlungszielExceeded"
        );
        assert!(
            !finding.unwrap().is_dispute,
            "ZahlungszielExceeded is Warn, not Dispute"
        );
    }

    #[test]
    fn zahlungsziel_before_invoice_date_is_dispute() {
        let mut r = make_rechnung(vec![], None);
        r.rechnungsdatum = Some(parse_date("2026-07-15"));
        r.faelligkeitsdatum = Some(parse_date("2026-07-01")); // before invoice date

        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(report.has_dispute());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ZahlungszielInvalid),
            "pay_by before rechnungsdatum must produce ZahlungszielInvalid Dispute"
        );
    }

    #[test]
    fn zahlungsziel_check_disabled_at_zero() {
        let mut r = make_rechnung(vec![], None);
        r.rechnungsdatum = Some(parse_date("2026-01-01"));
        r.faelligkeitsdatum = Some(parse_date("2026-12-31")); // 364 days — would normally trigger

        let config = CheckConfig {
            max_zahlungsziel_days: 0,
            ..Default::default()
        };
        let report = InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &config);
        assert!(
            !report.findings.iter().any(|f| matches!(
                f.kind,
                FindingKind::ZahlungszielExceeded | FindingKind::ZahlungszielInvalid
            )),
            "Zahlungsziel check must be skipped when max_zahlungsziel_days = 0"
        );
    }
}
