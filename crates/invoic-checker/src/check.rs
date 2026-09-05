//! INVOIC plausibility check engine — operates on BO4E [`Rechnung`].
//!
//! [`InvoicCheckEngine::check`] runs a multi-stage pipeline of automated
//! plausibility checks against a [`rubo4e::current::Rechnung`] and returns a
//! [`CheckReport`] that drives the REMADV / dispute workflow in `invoicd`.
//!
//! # Check stages
//!
//! Eight stages, in this order. The order matters twice: the currency check
//! runs before the arithmetic that would otherwise compare a CHF amount against
//! a EUR one, and the tariff check runs last because it is the only stage that
//! reaches outside the document.
//!
//! | # | Stage | Finding kind | Outcome |
//! |---|---|---|---|
//! | 1 | Storno reference | [`FindingKind::StorniertWithoutReference`] | `Dispute` |
//! | 2 | Period validity | [`FindingKind::PeriodInvalid`] | `Dispute` |
//! | 3 | Zahlungsziel | [`FindingKind::ZahlungszielInvalid`] · [`FindingKind::ZahlungszielExceeded`] | `Dispute` · `Warn` |
//! | 4 | Currency agreement | [`FindingKind::WaehrungMismatch`] | `Dispute` |
//! | 5 | Position arithmetic | [`FindingKind::ArithmeticError`] | `Dispute` |
//! | 6 | Document total | [`FindingKind::TotalMismatch`] | `Warn` |
//! | 7 | Umsatzsteuer | [`FindingKind::SteuerMissing`] · [`FindingKind::SteuerMismatch`] · [`FindingKind::ReverseChargeStatesTax`] | `Dispute` |
//! | 8 | Tariff / Angebot | [`FindingKind::TariffDeviation`] · [`FindingKind::TariffNotFound`] · [`FindingKind::AngebotDeviation`] · [`FindingKind::AngebotPositionUnknown`] | `Warn` or `Dispute` |
//!
//! Stage 8 is skipped for a Stornorechnung, which carries negated original
//! amounts rather than tariff positions, and stage 3 is skipped when
//! `CheckConfig::max_zahlungsziel_days` is zero.
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
//! use invoic_checker::check::{CheckConfig, CheckOutcome, FindingKind, InvoicCheckEngine};
//! use invoic_checker::tariff::InMemoryPreisblattStore;
//! use rubo4e::current::Rechnung;
//!
//! // A default `Rechnung` states no Umsatzsteuer, and §14 Abs. 4 Nr. 8 UStG
//! // makes the rate and the amount mandatory content — so it is disputed
//! // before any tariff question arises. An invoice the recipient cannot deduct
//! // is one the recipient does not pay.
//! let report = InvoicCheckEngine::check(
//!     31001,
//!     "9900357000004",
//!     &Rechnung::default(),
//!     &InMemoryPreisblattStore::new(),
//!     &CheckConfig::default(),
//! );
//! assert_eq!(report.outcome, CheckOutcome::Dispute);
//! assert!(report.findings.iter().any(|f| f.kind == FindingKind::SteuerMissing));
//! ```

use rubo4e::convenience::{BetragExt, MengeExt, PreisExt};
use rubo4e::current::{Rechnung, Rechnungsposition};

use crate::{
    amount::{EuroAmount, euro_from_decimal},
    tariff::PreisblattStore,
};

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
    /// (`rechnungsdatum`) to the due date (`faelligkeitsdatum`, DTM+265).
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
    /// `faelligkeitsdatum` (DTM+265) exceeds the maximum allowed payment term.
    ///
    /// Basis: §7 Allgemeine Festlegungen V6.1d — standard GPKE/WiM payment
    /// term is 30 days from invoice date.
    ZahlungszielExceeded,
    /// `faelligkeitsdatum` (DTM+265) is in the past or before `rechnungsdatum`.
    ZahlungszielInvalid,
    /// The invoice states no Umsatzsteuer at all.
    ///
    /// §14 Abs. 4 Nr. 8 UStG requires the rate and the tax amount, or a note
    /// saying why neither is stated. An invoice carrying only a net figure gives
    /// its recipient no Vorsteuerabzug — which is the receiving LF's money.
    SteuerMissing,
    /// `gesamtbrutto` does not equal `gesamtnetto + gesamtsteuer`.
    SteuerMismatch,
    /// The document's monetary fields do not agree on a currency.
    ///
    /// Every amount in this crate is an [`EuroAmount`], because a German MaKo
    /// invoice is denominated in EUR — which means a `Betrag` carrying
    /// `waehrung: CHF` is read *as if it were EUR* and every later comparison
    /// silently comes out right. This is the check that stops that, and it runs
    /// before the arithmetic for exactly that reason.
    WaehrungMismatch,
    /// A reverse-charge invoice (`RCV`) nonetheless states a tax amount.
    ///
    /// Tax shown on a §13b invoice is owed under §14c Abs. 1 UStG and is still
    /// not deductible, because the recipient owes it too.
    ReverseChargeStatesTax,
    /// An INVOIC 31009 position's unit price deviates from the price the MSB
    /// **offered** and the ESA accepted (QUOTES 15003 `SG31 PRI+CAL`).
    ///
    /// Distinct from [`Self::TariffDeviation`], which compares against a
    /// *published* Preisblatt. An ESA has none: there is no price sheet for
    /// Kapitel-4.6 Messprodukte, and §35 MsbG leaves the Entgelt for a
    /// Zusatzleistung to be agreed per request. The Angebot **is** the price
    /// agreement, which is why UC 4.1.1 has the ESA asking for „die
    /// Übermittlung von Werten und die damit verbundenen Kosten" and why the
    /// offer carries a Bindungsfrist at all.
    AngebotDeviation,
    /// An INVOIC 31009 position names an Artikel-ID the accepted Angebot never
    /// priced.
    ///
    /// The offer prices one to three Artikel-IDs per `SG27 LIN` (QUOTES AHB
    /// 1.1a condition `[2042]`); a fourth on the invoice is a charge the ESA
    /// never agreed to.
    AngebotPositionUnknown,
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

/// The signed deviation of `actual` from `expected`, in percent.
///
/// A diagnostic figure for the finding message, never an input to a comparison —
/// which is why `f64` is admissible here and nowhere else in this crate.
///
/// The difference is taken in `i128`: two independently valid `Amount<5>` values
/// can sit at opposite ends of the `i64` range, and their difference does not
/// fit in one.
fn deviation(expected: Option<EuroAmount>, actual: Option<EuroAmount>) -> Option<f64> {
    match (expected, actual) {
        (Some(exp), Some(act)) if exp.to_raw() != 0 => {
            let diff = i128::from(act.to_raw()) - i128::from(exp.to_raw());
            Some(diff as f64 / i128::from(exp.to_raw()).unsigned_abs() as f64 * 100.0)
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
    /// Assemble a report from the findings a check pipeline produced.
    ///
    /// The outcome is the **highest severity** across the findings: one dispute
    /// makes the whole report a dispute, any finding at all makes it at least a
    /// warning, none makes it `Ok`. That rule, the invoice's own stated total
    /// and the line count were open-coded once per pipeline — three copies of
    /// the same four lines, in the one place where a divergence would silently
    /// change whether an invoice is auto-paid.
    #[must_use]
    pub fn from_findings(
        pid: u32,
        rechnung: &Rechnung,
        findings: Vec<Finding>,
        computed_total: Option<EuroAmount>,
    ) -> Self {
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

        Self {
            outcome,
            findings,
            pid,
            total_net_invoic: rechnung
                .gesamtnetto
                .wert_decimal()
                .and_then(euro_from_decimal),
            total_net_computed: computed_total,
            line_items_checked: rechnung.rechnungspositionen.iter().flatten().count(),
        }
    }

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
/// When true, the tariff check (stage 8) must be skipped — cancellations
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
    /// - `pid` — BDEW Prüfidentifikator (31001–31011) from `InvoicData`.
    /// - `sender_mp_id` — verified sender GLN from `InvoicData.sender`
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

        // ── Stage 1: Stornierung reference check ──────────────────────────────
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

        // ── Stage 2: Period validity ──────────────────────────────────────────
        Self::check_periods(rechnung, &mut findings);

        // ── Stage 3: Zahlungsziel check ───────────────────────────────────────
        // DTM+265 (faelligkeitsdatum) must not exceed max_zahlungsziel_days.
        // Source: §7 Allgemeine Festlegungen V6.1d; BK6-22-024 §5.
        if config.max_zahlungsziel_days > 0 {
            Self::check_zahlungsziel(rechnung, config, &mut findings);
        }

        // ── Stage 4: Currency agreement ───────────────────────────────────────
        // Before the arithmetic, which would otherwise read a CHF `Betrag` as
        // if it were EUR and find every later comparison consistent.
        Self::check_waehrung(rechnung, &mut findings);

        // ── Stage 5: Arithmetic (qty × unit_price ≈ gesamtpreis) ──────────────
        Self::check_arithmetic(rechnung, config, &mut findings);

        // ── Stage 6: Total consistency (Σ gesamtpreis ≈ gesamtnetto) ──────────
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // ── Stage 7: The Umsatzsteuer block ───────────────────────────────────
        Self::check_steuer(rechnung, config, &mut findings);

        // ── Stage 8: Tariff check (PRICAT vs INVOIC unit price) ───────────────
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

        CheckReport::from_findings(pid, rechnung, findings, computed_total)
    }

    // ── Stage implementations ──────────────────────────────────────────────────

    /// Stage 3: Validate `faelligkeitsdatum` (Zahlungsziel / DTM+265).
    ///
    /// Checks:
    /// - If `faelligkeitsdatum < rechnungsdatum`: invalid (past due before issued).
    ///   Produces a `Dispute` finding (`ZahlungszielInvalid`).
    /// - If `faelligkeitsdatum - rechnungsdatum > max_zahlungsziel_days`:
    ///   exceeds contractual/regulatory payment term.
    ///   Produces a `Warn` finding (`ZahlungszielExceeded`).
    ///
    /// Source: §7 Allgemeine Festlegungen V6.1d (30 days standard).
    ///
    /// # Compared as calendar dates, not timestamps
    ///
    /// BO4E types both fields `format: date-time`, but a Zahlungsziel is a
    /// term in *days*. Senders pin the timestamp to midnight in their own
    /// offset, so subtracting two `OffsetDateTime`s measures the offsets as well
    /// as the days: an invoice issued `2026-07-01T00:00+02:00` and due
    /// `2026-07-31T00:00Z` is 30 calendar days but 30 days *plus two hours*,
    /// and one issued at `23:00Z` reads as 29. `rechnungsdatum_date()` and
    /// `faelligkeitsdatum_date()` take the date in the offset the payload
    /// carries, which is the comparison the rule is about.
    fn check_zahlungsziel(rechnung: &Rechnung, config: &CheckConfig, findings: &mut Vec<Finding>) {
        let Some(faellig) = rechnung.faelligkeitsdatum_date() else {
            return; // DTM+265 absent — not required on all PID types
        };
        let Some(rechnungs_datum) = rechnung.rechnungsdatum_date() else {
            return; // Cannot compute term without invoice date
        };

        if faellig < rechnungs_datum {
            findings.push(Finding::dispute(
                FindingKind::ZahlungszielInvalid,
                format!(
                    "Zahlungsziel {faellig} is before invoice date {rechnungs_datum}. \
                     DTM+265 must not precede rechnungsdatum. \
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

    /// Stage 2: Verify that every billing period is orientated forwards.
    ///
    /// **The two periods on a `Rechnung` use different interval conventions, and
    /// the check differs accordingly.** BO4E is not uniform here:
    ///
    /// | Field | Kind | Interval | Invalid when |
    /// |---|---|---|---|
    /// | `rechnungsperiode` | `Zeitraum` **date** pair | `[start, end]` — „Enddatum … ist **inklusiv**" | `start > end` |
    /// | `lieferungszeitraum` `von` / `bis` | **date-time** pair | `[start, end)` | `start >= end` |
    ///
    /// So a Rechnungsperiode with `start == end` is a legitimate **one-day**
    /// period — the most common shape in a daily-granularity payload — while a
    /// Lieferung with `von == bis` is an empty interval.
    fn check_periods(rechnung: &Rechnung, findings: &mut Vec<Finding>) {
        // Message-level period (Rechnungsperiode) — inclusive end.
        if let Some(period) = rechnung.billing_period() {
            let (start, end) = (*period.start(), *period.end());
            if start > end {
                findings.push(Finding::dispute(
                    FindingKind::PeriodInvalid,
                    format!("Message-level billing period invalid: start {start} > end {end}"),
                    None,
                    None,
                    None,
                ));
            }
        }
        // Line-level periods (Lieferung von/bis) — half-open, so an empty
        // interval is invalid too.
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

    /// Stage 5: For each position with quantity + unit_price, verify
    /// `positions_menge × einzelpreis ≈ gesamtpreis` (BO4E v202607).
    ///
    /// Uses `billing::Amount::checked_sub` + `checked_mul_qty` — no `f64`
    /// intermediate — satisfying the §40 EnWG itemised-billing accuracy requirement.
    /// One currency across every monetary field on the document.
    ///
    /// BO4E does not state this as a sentence of its own; it is the premise of
    /// the two sums it *does* state (`gesamtbrutto` is „Die Summe aus Netto-
    /// und Steuerbetrag", `steuerbetraege` sum to `gesamtsteuer`) — amounts
    /// denominated differently have no sum.
    ///
    /// A **dispute**, not a warning: the checker reads every amount as an
    /// [`EuroAmount`], so a mixed-currency document does not fail any later
    /// comparison. It passes them, wrongly.
    fn check_waehrung(rechnung: &Rechnung, findings: &mut Vec<Finding>) {
        // Every field that names a currency, not only the document-level totals.
        // A position denominated differently from the header is read as EUR by
        // every later stage exactly as a header field would be, and it is the
        // positions that carry the arithmetic.
        let mut fields: Vec<(String, rubo4e::current::Waehrungscode)> = [
            ("gesamtnetto", &rechnung.gesamtnetto),
            ("gesamtsteuer", &rechnung.gesamtsteuer),
            ("gesamtbrutto", &rechnung.gesamtbrutto),
            ("rabattNetto", &rechnung.rabatt_netto),
            ("zuZahlen", &rechnung.zu_zahlen),
        ]
        .into_iter()
        .filter_map(|(f, b)| {
            b.as_ref()
                .and_then(|b| b.waehrung)
                .map(|c| (f.to_owned(), c))
        })
        .collect();
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let (line_no, _) = pos_ident(pos);
            if let Some(code) = pos.gesamtpreis.as_ref().and_then(|b| b.waehrung) {
                fields.push((format!("Line {line_no} gesamtpreis"), code));
            }
        }
        for (i, b) in rechnung.steuerbetraege.iter().flatten().enumerate() {
            if let Some(code) = b.waehrungscode {
                fields.push((format!("Steuerbetrag {i}"), code));
            }
        }

        let mut first: Option<(String, rubo4e::current::Waehrungscode)> = None;
        for (field, code) in fields {
            match &first {
                None => {}
                Some((first_field, first_code)) if *first_code != code => {
                    findings.push(Finding::dispute(
                        FindingKind::WaehrungMismatch,
                        format!(
                            "{first_field} is denominated in {} but {field} in {} — \
                             amounts in different currencies have no sum, and every \
                             figure below is read as EUR.",
                            first_code.as_wire(),
                            code.as_wire()
                        ),
                        None,
                        None,
                        None,
                    ));
                    return;
                }
                Some(_) => {}
            }
            if first.is_none() {
                first = Some((field, code));
            }
        }
    }

    fn check_arithmetic(rechnung: &Rechnung, config: &CheckConfig, findings: &mut Vec<Finding>) {
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let qty = pos.positions_menge.wert_decimal();
            let price = pos.einzelpreis.wert_decimal().and_then(euro_from_decimal);
            let stated_net = pos.gesamtpreis.wert_decimal().and_then(euro_from_decimal);

            if let (Some(qty), Some(price), Some(stated_net)) = (qty, price, stated_net) {
                let (line_no, malo) = pos_ident(pos);
                // The quantity comes off the wire and nothing has range-checked
                // it — the price has been through `euro_from_decimal`, the
                // quantity has not. `mul_qty` panics on a product outside the
                // representable range, so a counterparty document stating
                // `menge = 1e15` would take down the request that validates it.
                // An unrepresentable product is a finding about the document,
                // not a fault in the checker.
                let Ok(computed) = price.checked_mul_qty(qty) else {
                    findings.push(Finding {
                        kind: FindingKind::ArithmeticError,
                        is_dispute: true,
                        message: format!(
                            "Line {line_no} ({malo}): {qty} × {price} EUR is not a \
                             representable amount — the stated quantity or unit price is \
                             out of range, so the position cannot be checked or paid",
                        ),
                        line_number: Some(line_no),
                        expected: None,
                        actual: Some(stated_net),
                        deviation_pct: None,
                    });
                    continue;
                };
                if !stated_net.within_tolerance_ppm(computed, config.arithmetic_tolerance_ppm) {
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

    /// Stage 6: Verify Σ `gesamtpreis` ≈ `gesamtnetto`.
    ///
    /// Returns the computed sum (used in the `CheckReport`).
    /// Stage 7: the Umsatzsteuer block.
    ///
    /// §14 Abs. 4 Nr. 8 UStG makes the rate and the tax amount mandatory content
    /// — or, where the supply is not taxed by the issuer, a note saying so. An
    /// invoice without either is one the recipient cannot deduct, so this is a
    /// **dispute**: paying it means paying tax that cannot be recovered.
    ///
    /// The arithmetic is checked too. `gesamtbrutto` is what is actually owed,
    /// and an invoice whose parts do not sum to its whole is the one error
    /// nobody catches by reading it.
    fn check_steuer(rechnung: &Rechnung, config: &CheckConfig, findings: &mut Vec<Finding>) {
        let netto = rechnung
            .gesamtnetto
            .wert_decimal()
            .and_then(euro_from_decimal);
        let steuer = rechnung
            .gesamtsteuer
            .wert_decimal()
            .and_then(euro_from_decimal);
        let brutto = rechnung
            .gesamtbrutto
            .wert_decimal()
            .and_then(euro_from_decimal);
        let breakdown = rechnung.steuerbetraege.as_deref().unwrap_or_default();

        // A reverse charge states no tax by design, so its absence is only a
        // defect when nothing explains it.
        let reverse_charge = breakdown
            .iter()
            .any(|b| b.steuerart == Some(rubo4e::current::Steuerart::Rcv));

        // Stating `0` is not the same as stating nothing — but with no breakdown
        // and no reverse-charge entry it carries no *ground* either, and
        // §14 Abs. 4 Nr. 8 UStG wants the rate and amount **or** the note that
        // the recipient owes the tax. A Kleinunternehmer invoice (§19 UStG) may
        // legitimately show zero and carry its ground in free text, which is why
        // this is a Warning rather than a Dispute: refusing it would reject a
        // lawful invoice, while staying silent — as this did — hides the one
        // remaining shape of "states no Umsatzsteuer" that reached acceptance.
        if steuer == Some(EuroAmount::ZERO) && breakdown.is_empty() {
            findings.push(Finding::warn(
                FindingKind::SteuerMissing,
                "The invoice states 0,00 EUR Umsatzsteuer with no Steuerbetrag \
                 breakdown and no reverse-charge entry, so it names no ground for \
                 the exemption. §14 Abs. 4 Nr. 8 UStG requires the rate and amount \
                 or a note that the recipient owes the tax; if the ground is stated \
                 only in free text, the document is complete but this check cannot \
                 see it.",
                None,
                None,
                None,
            ));
        }

        if steuer.is_none() && breakdown.is_empty() {
            findings.push(Finding::dispute(
                FindingKind::SteuerMissing,
                "The invoice states no Umsatzsteuer and no Steuerbetrag breakdown. \
                 §14 Abs. 4 Nr. 8 UStG requires the rate and the amount, or a note \
                 that the recipient owes the tax — without either there is no \
                 Vorsteuerabzug.",
                None,
                None,
                None,
            ));
            return;
        }

        if reverse_charge
            && let Some(steuer) = steuer
            && steuer != EuroAmount::ZERO
        {
            findings.push(Finding::dispute(
                FindingKind::ReverseChargeStatesTax,
                format!(
                    "The invoice is reverse-charged (§13b UStG) and states {steuer} EUR of \
                     Umsatzsteuer anyway. That tax is owed under §14c Abs. 1 UStG and is \
                     still not deductible, because the recipient owes it too."
                ),
                None,
                Some(EuroAmount::ZERO),
                Some(steuer),
            ));
        }

        if let (Some(netto), Some(steuer), Some(brutto)) = (netto, steuer, brutto)
            && !brutto.within_tolerance_ppm(netto + steuer, config.total_tolerance_ppm)
        {
            findings.push(Finding::dispute(
                FindingKind::SteuerMismatch,
                format!(
                    "gesamtbrutto = {brutto} EUR, but gesamtnetto + gesamtsteuer = {} EUR",
                    netto + steuer
                ),
                None,
                Some(netto + steuer),
                Some(brutto),
            ));
        }

        // **The breakdown must add up to the total it breaks down.**
        //
        // BO4E states this rule outright („die Summe dieser Beträge ergibt den
        // Wert für gesamtsteuer") and enforces it nowhere: `rubo4e` ships a
        // validator for it behind a feature mako does not enable, and no
        // reference implementation runs it. This check verified
        // `netto + steuer == brutto` and never looked inside `steuerbetraege`. The two figures are read by different
        // parties for different purposes — the recipient computes its
        // Vorsteuerabzug from the per-rate breakdown (§14 Abs. 4 Nr. 8 UStG,
        // §15 Abs. 1) and pays from the total — so an invoice stating 19 % on
        // 50 EUR and 7 % on 10 EUR while `gesamtsteuer` says 100 EUR is
        // internally consistent to neither of them, and passed.
        //
        // Skipped when the breakdown is absent: its absence is already a
        // `SteuerMissing` finding above, and a reverse-charged invoice states
        // no amounts by design.
        if let Some(steuer) = steuer
            && !breakdown.is_empty()
            && !reverse_charge
        {
            let summed = breakdown
                .iter()
                .filter_map(|b| b.steuerwert.and_then(euro_from_decimal))
                .fold(EuroAmount::ZERO, |acc, v| acc + v);
            if !steuer.within_tolerance_ppm(summed, config.total_tolerance_ppm) {
                findings.push(Finding::dispute(
                    FindingKind::SteuerMismatch,
                    format!(
                        "gesamtsteuer = {steuer} EUR, but the {} Steuerbetrag entries \
                         sum to {summed} EUR. §14 Abs. 4 Nr. 8 UStG makes the per-rate \
                         breakdown the basis of the recipient's Vorsteuerabzug, so it \
                         must agree with the total it is a breakdown of.",
                        breakdown.len()
                    ),
                    None,
                    Some(summed),
                    Some(steuer),
                ));
            }
        }

        // **The rate must produce the amount it is stated beside.**
        //
        // §14 Abs. 4 Nr. 8 UStG makes „der anzuwendende Steuersatz sowie der
        // auf das Entgelt entfallende Steuerbetrag" mandatory content, and the
        // recipient's Vorsteuerabzug is the second figure while the tax office
        // reads the first. Checking that the breakdown sums to `gesamtsteuer`
        // does not reach this: an invoice stating 19 % on a base of 10 000 with
        // a Steuerwert of 100 sums to its own total perfectly, so
        // `netto + steuer = brutto` holds, the breakdown agrees with
        // `gesamtsteuer`, and 1 800 EUR of tax is neither charged nor
        // deductible.
        //
        // A **dispute**: paying it books a Vorsteuer the invoice does not
        // support, and the difference is recoverable from nobody.
        for (i, b) in breakdown.iter().enumerate() {
            if b.steuerart == Some(rubo4e::current::Steuerart::Rcv) {
                continue;
            }
            let (Some(basis), Some(satz), Some(stated)) = (
                b.basiswert.and_then(euro_from_decimal),
                b.steuersatz,
                b.steuerwert.and_then(euro_from_decimal),
            ) else {
                continue;
            };
            // The rate arrives off the wire unchecked, so the product is taken
            // in the checked form rather than panicking the request.
            let Ok(computed) = basis
                .checked_mul_qty(satz)
                .and_then(|x| x.checked_div(rust_decimal::Decimal::ONE_HUNDRED))
            else {
                findings.push(Finding::dispute(
                    FindingKind::SteuerMismatch,
                    format!(
                        "Steuerbetrag {i}: {satz} % of {basis} EUR is not a representable \
                         amount — the stated rate or base is out of range."
                    ),
                    None,
                    None,
                    Some(stated),
                ));
                continue;
            };
            // § 14 UStG amounts are stated in whole cents, so the lawful figure
            // is the rounded one and a deviation below the rounding unit is not
            // a defect. A relative tolerance alone cannot express that: 19 % of
            // one cent is 0,19 cent, which rounds to 0,00 — correct, and 100 %
            // away from the unrounded product. One cent is therefore the floor,
            // and it is negligible against the base a real breakdown carries.
            // Taken in `i128`: both operands are independently valid amounts,
            // so their difference can leave `i64`.
            const ONE_CENT_RAW: i128 = 1_000;
            let within_a_cent =
                (i128::from(stated.to_raw()) - i128::from(computed.to_raw())).abs() <= ONE_CENT_RAW;
            if !within_a_cent && !stated.within_tolerance_ppm(computed, config.total_tolerance_ppm)
            {
                findings.push(Finding::dispute(
                    FindingKind::SteuerMismatch,
                    format!(
                        "Steuerbetrag {i}: {satz} % of a basiswert of {basis} EUR is \
                         {computed} EUR, but the entry states {stated} EUR. §14 Abs. 4 Nr. 8 \
                         UStG makes the rate and the amount it produces both mandatory, and \
                         the recipient deducts the amount."
                    ),
                    None,
                    Some(computed),
                    Some(stated),
                ));
            }
        }
    }

    fn check_total(
        rechnung: &Rechnung,
        config: &CheckConfig,
        findings: &mut Vec<Finding>,
    ) -> Option<EuroAmount> {
        let line_nets: Vec<EuroAmount> = rechnung
            .rechnungspositionen
            .iter()
            .flatten()
            .filter_map(|pos| pos.gesamtpreis.wert_decimal().and_then(euro_from_decimal))
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
            .and_then(euro_from_decimal)
            && !stated.within_tolerance_ppm(computed, config.total_tolerance_ppm)
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

    /// Stage 8: Compare `einzelpreis` against the tariff store (PRICAT 27003).
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
            .map(|p| *p.start())
            .or_else(|| rechnung.rechnungsdatum_date())
            .unwrap_or_else(mako_fristen::heute);

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
            let Some(invoic_price) = pos.einzelpreis.wert_decimal().and_then(euro_from_decimal)
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

            // **The Staffel that applies to this quantity, not every Staffel.**
            //
            // A Preisposition states its price in tiers — `0 – 1000 → 0.30`,
            // `1001 – 2000 → 0.25`, `2001+ → 0.20`. Collecting every tier's price
            // and asking whether the invoice matches *any* of them ignores the
            // bounds completely: it accepted a 500 kWh position billed at the
            // 2001+ rate, which is the cheapest tier applied to the smallest
            // quantity and exactly the deviation this check exists to catch.
            //
            // `select_for` picks the tier by the position's own quantity and
            // implements BO4E's gap rule with it — the schema states bounds as
            // `0 – 1000, 1001 – 2000` and rules that a value *between* two tiers
            // („1000.6") *„rutscht in die obere Zone"*, which a plain
            // `von <= x <= bis` scan finds no tier for at all.
            //
            // Without a quantity there is no tier to select, so the check falls
            // back to every published price — permissive, but it only widens
            // what is accepted and never invents a deviation.
            use rubo4e::convenience::PreisstaffelSliceExt as _;
            let flat_prices: Vec<EuroAmount> = match pos.positions_menge.wert_decimal() {
                Some(menge) => preisblatt
                    .preispositionen
                    .iter()
                    .flatten()
                    .filter_map(|pp| {
                        pp.preisstaffeln
                            .as_deref()
                            .and_then(|staffeln| staffeln.select_for(menge))
                    })
                    .filter_map(|ps| ps.preis)
                    .filter_map(euro_from_decimal)
                    .collect(),
                None => preisblatt
                    .preispositionen
                    .iter()
                    .flatten()
                    .flat_map(|pp| pp.preisstaffeln.iter().flatten())
                    .filter_map(|ps| ps.preis)
                    .filter_map(euro_from_decimal)
                    .collect(),
            };

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
                                .and_then(euro_from_decimal)?;
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
                .any(|p| invoic_price.within_tolerance_ppm(*p, tol))
            {
                // Report the closest published rate for diagnostics.
                // The distance is taken in i128: two valid `Amount<5>` values
                // can sit at opposite ends of the i64 range.
                let closest = *published
                    .iter()
                    .min_by_key(|p| {
                        (i128::from(invoic_price.to_raw()) - i128::from(p.to_raw())).unsigned_abs()
                    })
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

    // ── The ESA's price basis is its own accepted Angebot ────────────────────

    /// Check an INVOIC 31009 against the **Angebot the ESA accepted**.
    ///
    /// # Why an ESA cannot use the Preisblatt path
    ///
    /// [`check_msb_rechnung`](Self::check_msb_rechnung) compares against
    /// `PreisblattMessung` — the price sheet an MSB publishes toward the NB and
    /// the LF. **An ESA has none**: there is no published sheet for the
    /// Kapitel-4.6 Messprodukte, because §35 MsbG leaves the Entgelt for a
    /// Zusatzleistung to be agreed per request.
    ///
    /// Its basis is the offer it accepted. UC 4.1.1 has the ESA asking for „die
    /// Übermittlung von Werten **und die damit verbundenen Kosten**"; QUOTES AHB
    /// 1.1a §4.3 makes `SG4 CUX` and one `SG31 PRI+CAL` per `SG27 PIA+Z02`
    /// Artikel-ID **Muss**; and the offer carries a Bindungsfrist because it
    /// binds. The invoice names the same Artikel-IDs back (`SG26 LIN` DE 7143
    /// `Z09`, INVOIC AHB 1.0b), so the two join exactly rather than by a
    /// plausibility band.
    ///
    /// # What it reports
    ///
    /// - a position whose `einzelpreis` deviates from the agreed one beyond
    ///   `tariff_tolerance_ppm` → [`FindingKind::AngebotDeviation`], a dispute;
    /// - a position naming an Artikel-ID the offer never priced →
    ///   [`FindingKind::AngebotPositionUnknown`], a dispute — a charge the ESA
    ///   did not agree to;
    /// - a position carrying **no** Artikel-ID → skipped with a warning. DE 7143
    ///   admits `Z01` Artikelnummer as well as `Z09` Artikel-ID, and an
    ///   Artikelnummer names no offer position.
    ///
    /// `agreed` is the accepted offer as `(Artikel-ID, price)` pairs. Empty
    /// means no accepted offer is on record: the checks are **skipped with a
    /// warning**, never disputed, because absence is a gap in mako's own
    /// records rather than a defect in the MSB's invoice.
    #[must_use]
    pub fn check_esa_rechnung(
        sender_mp_id: &str,
        rechnung: &Rechnung,
        agreed: &[(String, EuroAmount)],
        config: &CheckConfig,
    ) -> CheckReport {
        let mut findings = Vec::new();

        // The structural checks are the same invoice arithmetic as everywhere
        // else; only the price basis differs.
        Self::check_periods(rechnung, &mut findings);
        Self::check_waehrung(rechnung, &mut findings);
        Self::check_arithmetic(rechnung, config, &mut findings);
        let computed_total = Self::check_total(rechnung, config, &mut findings);
        Self::check_zahlungsziel(rechnung, config, &mut findings);
        Self::check_steuer(rechnung, config, &mut findings);

        Self::check_against_angebot(rechnung, sender_mp_id, agreed, config, &mut findings);

        CheckReport::from_findings(31009, rechnung, findings, computed_total)
    }

    /// The price comparison of [`check_esa_rechnung`], split out so the
    /// Preisblatt path and the Angebot path cannot drift into one another.
    fn check_against_angebot(
        rechnung: &Rechnung,
        sender_mp_id: &str,
        agreed: &[(String, EuroAmount)],
        config: &CheckConfig,
        findings: &mut Vec<Finding>,
    ) {
        if agreed.is_empty() {
            findings.push(Finding {
                kind: FindingKind::TariffNotFound,
                // Never a dispute: mako not holding the accepted offer says
                // nothing about whether the MSB billed correctly.
                is_dispute: false,
                message: format!(
                    "No accepted Angebot on record for MSB {sender_mp_id}. The ESA price basis \
                     is the QUOTES 15003 the ESA ordered against (§35 MsbG — there is no \
                     published Preisblatt for Kapitel-4.6 Messprodukte), so the price check is \
                     skipped."
                ),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            });
            return;
        }

        let tol = config.tariff_tolerance_ppm;
        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let (line_no, text) = pos_ident(pos);
            let Some(invoiced) = pos.einzelpreis.wert_decimal().and_then(euro_from_decimal) else {
                continue;
            };
            // DE 7143 admits `Z01` Artikelnummer beside `Z09` Artikel-ID, and an
            // Artikelnummer names no offer position — so a position without an
            // Artikel-ID is not comparable rather than wrong.
            let Some(artikel_id) = pos.artikel_id.as_deref().filter(|a| !a.is_empty()) else {
                findings.push(Finding::warn(
                    FindingKind::TariffNotFound,
                    format!(
                        "Line {line_no} ({text}): no Artikel-ID, so the position cannot be \
                         matched to the accepted Angebot — price check skipped for this line"
                    ),
                    Some(line_no),
                    None,
                    Some(invoiced),
                ));
                continue;
            };

            let Some((_, expected)) = agreed.iter().find(|(id, _)| id == artikel_id) else {
                findings.push(Finding::dispute(
                    FindingKind::AngebotPositionUnknown,
                    format!(
                        "Line {line_no} ({text}): Artikel-ID {artikel_id} was never priced in \
                         the Angebot this subscription was ordered against — the ESA did not \
                         agree to this charge"
                    ),
                    Some(line_no),
                    None,
                    Some(invoiced),
                ));
                continue;
            };

            if !invoiced.within_tolerance_ppm(*expected, tol) {
                findings.push(Finding::dispute(
                    FindingKind::AngebotDeviation,
                    format!(
                        "Line {line_no} ({text}): Artikel-ID {artikel_id} billed at {invoiced} \
                         EUR, but the accepted Angebot from MSB {sender_mp_id} priced it at \
                         {expected} EUR (tolerance {pct:.1}%)",
                        pct = f64::from(tol) / 10_000.0,
                    ),
                    Some(line_no),
                    Some(*expected),
                    Some(invoiced),
                ));
            }
        }
    }

    // ── The MSB price basis is `PreisblattMessung` ──────────────────────────

    /// Check a WiM MSB-Rechnung (PID 31003 / 31009) against `PreisblattMessung`.
    ///
    /// Replaces the standard [`check`](Self::check) call for those PIDs. The
    /// only difference is the price basis: the document stages — period,
    /// currency, position arithmetic, document total, Zahlungsziel and
    /// Umsatzsteuer — run identically, and the tariff comparison reads
    /// `PreisblattMessung.preispositionen` instead of
    /// `PreisblattNetznutzung.preispositionen`.
    ///
    /// `PreisblattMessung` has `preispositionen: Option<Vec<Preisposition>>` — the same type
    /// as `PreisblattNetznutzung` — so the price extraction logic is identical.
    ///
    /// When `preisblatt_messung` is `None`, the tariff comparison emits a
    /// warning (never a hard dispute) to match the standard engine's
    /// missing-tariff behaviour.
    #[must_use]
    pub fn check_msb_rechnung(
        pid: u32,
        sender_mp_id: &str,
        rechnung: &Rechnung,
        preisblatt_messung: Option<&rubo4e::current::PreisblattMessung>,
        config: &CheckConfig,
    ) -> CheckReport {
        Self::check_msb_rechnung_with_aufabschlaege(
            pid,
            sender_mp_id,
            rechnung,
            preisblatt_messung,
            &[],
            config,
        )
    }

    /// MSB-Rechnung (INVOIC 31003 / 31009) plausibility check with
    /// `AufAbschlag` validation.
    ///
    /// Runs the document stages of [`check`](Self::check) — period, currency,
    /// position arithmetic, document total, Zahlungsziel and Umsatzsteuer —
    /// prices against `PreisblattMessung` instead of `PreisblattNetznutzung`,
    /// and adds one check of its own:
    ///
    /// | # | Check | Source |
    /// |---|---|---|
    /// | — | Discount/surcharge positions are backed by a contracted `AufAbschlag` | WiM PRICAT 27001–27003 |
    ///
    /// `contracted_names` is the list of contracted AufAbschlag names from
    /// `PreisblattMessungRecord.auf_abschlaege` (pre-extracted by the caller).
    /// Pass `&[]` when absent (check 6 is then skipped, not disputed).
    pub fn check_msb_rechnung_with_aufabschlaege(
        pid: u32,
        sender_mp_id: &str,
        rechnung: &Rechnung,
        preisblatt_messung: Option<&rubo4e::current::PreisblattMessung>,
        contracted_names: &[String],
        config: &CheckConfig,
    ) -> CheckReport {
        let mut findings = Vec::new();

        // The document-level stages are identical to the standard pipeline:
        // period, currency, position arithmetic and document total.
        Self::check_periods(rechnung, &mut findings);
        Self::check_waehrung(rechnung, &mut findings);
        Self::check_arithmetic(rechnung, config, &mut findings);
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // **The Zahlungsziel and the Umsatzsteuer block are checked here too.**
        //
        // They were not, and the omission was accidental rather than a
        // judgement about MSB invoices: this entry point was written when the
        // pipeline had five stages, and the two were added to `check()`
        // afterwards without being wired in here. Nothing about a
        // Messstellenbetriebs-Rechnung exempts it from either —
        //
        // - `SG8 DTM+265` (Fälligkeitsdatum, MIG Nr. 00033) is **Muss** on
        //   PIDs 31003 and 31009 in the INVOIC AHB, exactly as it is on the
        //   31001/31002 invoices the standard pipeline checks; and
        // - `TAX` Nr. 00058 with `MOA` Nr. 00061/00062 is **Muss** on those
        //   same PIDs, because §14 Abs. 4 Nr. 8 UStG makes the rate and the
        //   tax amount (or the ground for stating neither) mandatory content
        //   of *every* invoice. Messstellenbetrieb is a taxable service at the
        //   regular rate — §13b UStG does not reach it — so an MSB invoice
        //   carrying only a net figure leaves its recipient without the
        //   Vorsteuerabzug that is the recipient's own money.
        //
        // The same PID 31009 already ran both through
        // [`check_esa_rechnung`](Self::check_esa_rechnung), so which door the
        // invoice came through decided whether its tax block was looked at.
        //
        // `check_steuer` distinguishes an absent tax block from a zero one
        // carrying a `RCV` ground, so a §13b invoice is not disputed for
        // stating 0,00 EUR.
        if config.max_zahlungsziel_days > 0 {
            Self::check_zahlungsziel(rechnung, config, &mut findings);
        }
        Self::check_steuer(rechnung, config, &mut findings);

        // Stage 8, against `PreisblattMessung.preispositionen`.
        let billing_date: time::Date = rechnung
            .billing_period()
            .map(|p| *p.start())
            .or_else(|| rechnung.rechnungsdatum_date())
            .unwrap_or_else(mako_fristen::heute);

        let published_prices: Vec<EuroAmount> = preisblatt_messung
            .and_then(|pm| pm.preispositionen.as_ref())
            .into_iter()
            .flatten()
            .flat_map(|pp| pp.preisstaffeln.iter().flatten())
            .filter_map(|ps| ps.preis)
            .filter_map(euro_from_decimal)
            .collect();

        if preisblatt_messung.is_none() {
            findings.push(Finding {
                kind: FindingKind::TariffNotFound,
                is_dispute: config.require_tariff,
                message: format!(
                    "No PreisblattMessung found for MSB GLN {sender_mp_id} on {billing_date}. \
                     Tariff check skipped — upload via \
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
                let Some(invoic_price) = pos.einzelpreis.wert_decimal().and_then(euro_from_decimal)
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
                    .any(|p| invoic_price.within_tolerance_ppm(*p, tol))
                {
                    let closest = *published_prices
                        .iter()
                        .min_by_key(|p| {
                            (i128::from(invoic_price.to_raw()) - i128::from(p.to_raw()))
                                .unsigned_abs()
                        })
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

        // Check 6 — AufAbschlag: verify discount/surcharge positions are
        // contracted. `contracted_names` holds the names of the authorised
        // AufAbschlag entries from the MSB's PRICAT 27001–27003; with none of
        // them, check 6 is skipped.
        //
        // An empty entry is a substring of every description, so leaving one in
        // the set would pass every discount position — the opposite of what this
        // check does. Blank entries are dropped, and a set holding nothing else
        // skips the check as though none had been supplied.
        let name_set: std::collections::HashSet<String> = contracted_names
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !name_set.is_empty() {
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

        CheckReport::from_findings(pid, rechnung, findings, computed_total)
    }

    /// Arithmetic-only check for Stornorechnungen (cancellation invoices).
    ///
    /// Runs stages 1–6 — Storno reference, period, Zahlungsziel, currency,
    /// position arithmetic and document total. Stages 7 and 8 are skipped: a
    /// Stornierung carries the original invoice's negated amounts rather than
    /// new tariff positions, so there is no Preisblatt to compare against.
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
    /// // A Storno still states its own Umsatzsteuer: `TAX` and the header `MOA`
    /// // are Muss for 31004, so a reversal of a 19 % invoice reverses the tax
    /// // with it. Without this block the report disputes with `SteuerMissing`.
    /// r.gesamtsteuer = Some(rubo4e::current::Betrag {
    ///     wert: Some("-19.00".parse().unwrap()),
    ///     ..Default::default()
    /// });
    /// r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
    ///     steuersatz: Some("19".parse().unwrap()),
    ///     steuerwert: Some("-19.00".parse().unwrap()),
    ///     ..Default::default()
    /// }]);
    ///
    /// let report = InvoicCheckEngine::check_storno(31004, &r, &CheckConfig::default());
    /// assert_eq!(report.outcome, CheckOutcome::Ok);
    /// ```
    #[must_use]
    pub fn check_storno(pid: u32, rechnung: &Rechnung, config: &CheckConfig) -> CheckReport {
        let mut findings = Vec::new();

        // Stage 1: Storno reference must be present.
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

        // Stage 2: Period validity (same as full check).
        Self::check_periods(rechnung, &mut findings);

        // Stage 3: Zahlungsziel check.
        if config.max_zahlungsziel_days > 0 {
            Self::check_zahlungsziel(rechnung, config, &mut findings);
        }

        // Stages 4-6: currency, arithmetic and total (still apply to Storno amounts).
        Self::check_waehrung(rechnung, &mut findings);
        Self::check_arithmetic(rechnung, config, &mut findings);
        let computed_total = Self::check_total(rechnung, config, &mut findings);

        // Stage 7: the Storno states its own tax. Verified against the imported
        // AHB: for 31004 the header `TAX` (Nr 00058) and `MOA` (00061/00062) are
        // **Muss** in both fv20260401 and fv20261001 — only the *position*-level
        // `TAX` (00044) is absent, which is why stage 8 below stays skipped and
        // this one does not. A Storno stating no Umsatzsteuer at all was
        // accepted, and it reverses an invoice that had to state one.
        //
        // The negated amounts are not an obstacle: `check_steuer` is entirely
        // sign-agnostic — it asserts `netto + steuer == brutto` and that the
        // breakdown sums to `gesamtsteuer`, both of which hold under negation.
        Self::check_steuer(rechnung, config, &mut findings);

        // Stage 8: SKIPPED — position-level `TAX` is not published for 31004.

        CheckReport::from_findings(pid, rechnung, findings, computed_total)
    }

    /// MMM settlement price check: validate that the Mehrmengen / Mindermengen
    /// positions of an MMM INVOIC (PIDs 31005, 31006, 31007, 31008) match the
    /// reference prices from the `marktd` MMMA store within tolerance.
    ///
    /// Called by the `invoicd` handler **after** the standard pipeline, when
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
        let ref_mehr = euro_from_decimal(mehr_ct_kwh / rust_decimal::Decimal::from(100));
        let ref_minder = euro_from_decimal(minder_ct_kwh / rust_decimal::Decimal::from(100));

        let mut findings = Vec::new();

        for pos in rechnung.rechnungspositionen.iter().flatten() {
            let Some(invoic_price) = pos.einzelpreis.wert_decimal().and_then(euro_from_decimal)
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

            if !invoic_price.within_tolerance_ppm(ref_p, tol) {
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
                        "Line {line_no} ({malo}): MMM {kind_str} price {invoic_price} EUR/kWh \
                         deviates {pct:.1}% from MMMA reference {ref_p} EUR/kWh \
                         (tolerance {t:.1}%)",
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

    /// A market date as the `date-time` BO4E declares for `rechnungsdatum` and
    /// `faelligkeitsdatum`: midnight UTC, which is how a producer pins a value
    /// BDEW transmits as a bare `YYYYMMDD`.
    fn parse_dt(s: &str) -> time::OffsetDateTime {
        parse_date(s).midnight().assume_utc()
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
        // Every fixture carries a lawful tax block: §14 Abs. 4 Nr. 8 UStG makes
        // it mandatory content, so an invoice without one is not a realistic
        // subject for the other checks — it is already a dispute.
        let netto =
            gesamtnetto.map(|n| Decimal::from_str_exact(&n.to_string()).unwrap_or_default());
        let steuer = netto.map(|n| {
            (n * Decimal::from(19) / Decimal::from(100))
                .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        });
        Rechnung {
            rechnungsperiode: Some(periode("2024-12-01", "2024-12-31")),
            rechnungsdatum: Some(parse_dt("2025-01-15")),
            gesamtnetto: gesamtnetto.map(betrag),
            gesamtsteuer: steuer.map(|w| Betrag {
                wert: Some(w),
                ..Default::default()
            }),
            gesamtbrutto: netto.zip(steuer).map(|(n, t)| Betrag {
                wert: Some(n + t),
                ..Default::default()
            }),
            steuerbetraege: steuer.map(|w| {
                vec![rubo4e::current::Steuerbetrag {
                    steuerart: Some(rubo4e::current::Steuerart::Ust),
                    steuersatz: Some(Decimal::from(19)),
                    basiswert: netto,
                    steuerwert: Some(w),
                    ..Default::default()
                }]
            }),
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

    // ── Umsatzsteuer ──────────────────────────────────────────────────────────

    /// An invoice stating no tax is disputed, not merely flagged.
    ///
    /// §14 Abs. 4 Nr. 8 UStG makes the rate and the amount mandatory content.
    /// Paying an invoice without them means paying tax that cannot be recovered,
    /// which is the receiving LF's money.
    #[test]
    fn an_invoice_without_a_tax_block_is_disputed() {
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtsteuer = None;
        r.gesamtbrutto = None;
        r.steuerbetraege = None;

        let report =
            InvoicCheckEngine::check(31002, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Dispute);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMissing)
        );
    }

    /// A reverse charge states no tax, and that is correct rather than missing.
    #[test]
    fn a_reverse_charge_without_a_tax_amount_is_accepted() {
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::ZERO));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(100_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Rcv),
            steuersatz: Some(Decimal::ZERO),
            steuerwert: Some(Decimal::ZERO),
            ..Default::default()
        }]);

        let report =
            InvoicCheckEngine::check(31005, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMissing),
            "a §13b invoice states no tax by design: {:#?}",
            report.findings
        );
    }

    /// A reverse charge that states tax anyway is disputed.
    ///
    /// That tax is owed under §14c Abs. 1 UStG *and* undeductible, because the
    /// recipient owes it too under §13b — the worst of both.
    #[test]
    fn a_reverse_charge_stating_tax_is_disputed() {
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::from_raw_units(19_000)));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(119_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Rcv),
            steuersatz: Some(Decimal::ZERO),
            steuerwert: Some(Decimal::from(190)),
            ..Default::default()
        }]);

        let report =
            InvoicCheckEngine::check(31005, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Dispute);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ReverseChargeStatesTax)
        );
    }

    /// The gross must equal net plus tax.
    ///
    /// An invoice whose parts do not sum to its whole is the one error nobody
    /// catches by reading it.
    #[test]
    fn a_gross_that_does_not_equal_net_plus_tax_is_disputed() {
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(999_999)));

        let report =
            InvoicCheckEngine::check(31002, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Dispute);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMismatch)
        );
    }

    /// A Stornorechnung passes the tax stage: every amount is negative, and the
    /// arithmetic holds with the signs.
    ///
    /// Every reversal `netzbilanzd` issues goes through this gate, so a stage
    /// that only reasons about positive amounts would block them all.
    #[test]
    fn a_storno_with_negative_amounts_passes_the_tax_stage() {
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(-100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(-100_000)));
        r.ist_storno = Some(true);
        r.original_rechnungsnummer = Some("NNE-2026-000001".to_owned());
        r.gesamtsteuer = Some(betrag(EuroAmount::from_raw_units(-19_000)));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(-119_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Ust),
            steuersatz: Some(Decimal::from(19)),
            basiswert: Some(Decimal::from(-1)),
            steuerwert: Some(Decimal::from_str_exact("-0.19").expect("decimal")),
            ..Default::default()
        }]);

        let report =
            InvoicCheckEngine::check(31002, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report.findings.iter().any(|f| {
                matches!(
                    f.kind,
                    FindingKind::SteuerMissing
                        | FindingKind::SteuerMismatch
                        | FindingKind::ReverseChargeStatesTax
                )
            }),
            "a reversal is a lawful document: {:#?}",
            report.findings
        );
    }

    // ── Tariff check ──────────────────────────────────────────────────────────

    #[test]
    fn no_tariff_warn_by_default() {
        // A realistic invoice, so the assertion isolates the tariff stage: an
        // empty document fails §14 UStG on its own and would dispute for that.
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

    /// A price sheet stated in **tiers**, as BO4E states them: bounds on
    /// `staffelgrenzeVon` / `staffelgrenzeBis`, cheaper as the quantity grows.
    fn tiered_store() -> InMemoryPreisblattStore {
        use rust_decimal::Decimal;
        let mut store = InMemoryPreisblattStore::new();
        let tier = |von: i64, bis: Option<i64>, preis: &str| Preisstaffel {
            staffelgrenze_von: Some(Decimal::from(von)),
            staffelgrenze_bis: bis.map(Decimal::from),
            preis: Some(Decimal::from_str_exact(preis).expect("valid decimal")),
            ..Default::default()
        };
        store.insert(
            SENDER.to_owned(),
            PreisblattNetznutzung {
                gueltigkeit: None,
                herausgeber: None,
                preispositionen: Some(vec![Preisposition {
                    preisstaffeln: Some(vec![
                        tier(0, Some(1000), "0.30"),
                        tier(1001, Some(2000), "0.25"),
                        tier(2001, None, "0.20"),
                    ]),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        store
    }

    /// A 500 kWh position billed at the **2001+** rate is a deviation.
    ///
    /// The tier is selected by the position's **quantity**, not by matching the
    /// billed price against any published tier: accepting whichever tier happens
    /// to match would let the cheapest tier price the smallest quantity and pass
    /// silently. `PreisstaffelSliceExt::select_for` picks the tier the quantity
    /// falls in, so the position is measured against 0.30 and disputed.
    #[test]
    fn a_position_billed_at_the_wrong_staffel_is_a_deviation() {
        let invoic_price = EuroAmount::from_raw_units(20_000); // 0.20 EUR/kWh — the 2001+ tier
        let pos = make_pos(
            1,
            "DE001",
            Some("500.0"), // …but only 500 kWh, which is the 0 – 1000 tier
            Some(invoic_price),
            Some(EuroAmount::from_raw_units(10_000_000)), // 500 × 0.20
        );
        let r = make_rechnung(vec![pos], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &tiered_store(), &CheckConfig::default());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffDeviation),
            "500 kWh belongs in the 0 – 1000 tier at 0.30, not the 2001+ tier at 0.20"
        );
    }

    /// The tier the quantity really falls in passes.
    #[test]
    fn a_position_billed_at_its_own_staffel_is_clean() {
        let invoic_price = EuroAmount::from_raw_units(30_000); // 0.30 EUR/kWh
        let pos = make_pos(
            1,
            "DE001",
            Some("500.0"),
            Some(invoic_price),
            Some(EuroAmount::from_raw_units(15_000_000)), // 500 × 0.30
        );
        let r = make_rechnung(vec![pos], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &tiered_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffDeviation)
        );
    }

    /// BO4E's **gap rule**: a quantity between two tiers „rutscht in die obere
    /// Zone", so 1000.6 kWh bills at the `1001 – 2000` rate rather than matching
    /// no tier at all.
    #[test]
    fn a_quantity_in_the_gap_between_two_staffeln_bills_at_the_upper_one() {
        let invoic_price = EuroAmount::from_raw_units(25_000); // the 1001 – 2000 tier
        let pos = make_pos(
            1,
            "DE001",
            Some("1000.6"),
            Some(invoic_price),
            Some(EuroAmount::from_raw_units(25_015_000)), // 1000.6 × 0.25
        );
        let r = make_rechnung(vec![pos], None);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &tiered_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::TariffDeviation),
            "1000.6 falls between the tiers and rutscht in die obere Zone (1001 – 2000)"
        );
    }

    /// A breakdown that does not add up to `gesamtsteuer` is a dispute.
    ///
    /// The recipient's Vorsteuerabzug comes from the per-rate entries and its
    /// payment from the total; when the two disagree the invoice is usable for
    /// neither. Checked since the rule became explicit in BO4E.
    #[test]
    fn a_tax_breakdown_that_does_not_sum_to_gesamtsteuer_is_a_dispute() {
        use rust_decimal::Decimal;
        let mut r = make_rechnung(vec![], Some(EuroAmount::from_raw_units(100_000_000)));
        // gesamtsteuer says 19.00; the single entry says 5.00.
        r.gesamtsteuer = Some(betrag(EuroAmount::from_raw_units(1_900_000)));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(101_900_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Ust),
            steuersatz: Some(Decimal::from(19)),
            basiswert: Some(Decimal::from(1000)),
            steuerwert: Some(Decimal::from(5)),
            ..Default::default()
        }]);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMismatch),
            "a breakdown summing to 5.00 against a stated 19.00 must be disputed"
        );
    }

    /// **Invariant: the tax equals the rate applied to the base.**
    ///
    /// §14 Abs. 4 Nr. 8 UStG makes both the rate and the amount it produces
    /// mandatory content, and the recipient deducts the amount. The three checks
    /// around this one — presence, `netto + steuer = brutto`, and Σ breakdown =
    /// `gesamtsteuer` — are all satisfied by an invoice stating 19 % on a base of
    /// 10 000 with a Steuerwert of 100: it returns `Ok`, triggers an auto-REMADV
    /// 33001 and an auto-payment, and books 100 EUR of Vorsteuer where 1 900 is
    /// owed.
    #[test]
    fn a_tax_amount_that_is_not_the_rate_times_the_base_is_a_dispute() {
        use rust_decimal::Decimal;
        // netto 10 000, steuer 100, brutto 10 100 — internally consistent.
        let mut r = make_rechnung(vec![], Some(EuroAmount::from_raw_units(1_000_000_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::from_raw_units(10_000_000)));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(1_010_000_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Ust),
            steuersatz: Some(Decimal::from(19)),
            basiswert: Some(Decimal::from(10_000)),
            steuerwert: Some(Decimal::from(100)),
            ..Default::default()
        }]);

        let report =
            InvoicCheckEngine::check(31002, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert_eq!(
            report.outcome,
            CheckOutcome::Dispute,
            "19 % of 10 000 is 1 900, not 100: {:#?}",
            report.findings
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMismatch && f.is_dispute),
            "{:#?}",
            report.findings
        );
    }

    /// The same entry, stated correctly, is silent — including a rate of zero.
    #[test]
    fn a_tax_amount_that_is_the_rate_times_the_base_is_silent() {
        use rust_decimal::Decimal;
        let mut r = make_rechnung(vec![], Some(EuroAmount::from_raw_units(1_000_000_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::from_raw_units(190_000_000)));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(1_190_000_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Ust),
            steuersatz: Some(Decimal::from(19)),
            basiswert: Some(Decimal::from(10_000)),
            steuerwert: Some(Decimal::from(1_900)),
            ..Default::default()
        }]);
        let report =
            InvoicCheckEngine::check(31002, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMismatch),
            "{:#?}",
            report.findings
        );
    }

    /// **Invariant: an oversized quantity is a finding, not a panic.**
    ///
    /// The unit price is range-checked on the way in and the quantity is not, so
    /// an absurd Menge reaches the multiplication unbounded. Aborting the
    /// request that validates it would make a counterparty document a remote
    /// denial of service on a message-processing path. It is a fact about the
    /// document, so it is reported as one.
    #[test]
    fn an_unrepresentable_line_product_is_reported_rather_than_panicking() {
        let pos = make_pos(
            1,
            "DE001",
            // 10^15 kWh at 1.00 EUR/kWh overflows the 5-dp fixed-point range.
            Some("1000000000000000"),
            Some(EuroAmount::from_raw_units(100_000)),
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        let report =
            InvoicCheckEngine::check(31002, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ArithmeticError && f.is_dispute),
            "{:#?}",
            report.findings
        );
    }

    /// **Invariant: a blank contracted name authorises nothing.**
    ///
    /// `""` is a substring of every description, so a single blank entry in the
    /// PRICAT-derived set passed every discount position — the opposite of what
    /// check 6 is for. It is dropped, and the names beside it still decide.
    #[test]
    fn a_blank_contracted_name_does_not_authorise_every_discount() {
        let discount = |text: &str| {
            make_pos(
                1,
                text,
                Some("1.0"),
                Some(EuroAmount::from_raw_units(-500_000)),
                Some(EuroAmount::from_raw_units(-500_000)),
            )
        };
        let contracted = ["".to_owned(), "   ".to_owned(), "winterrabatt".to_owned()];
        let disputed = |text: &str| {
            let r = make_rechnung(
                vec![discount(text)],
                Some(EuroAmount::from_raw_units(-500_000)),
            );
            InvoicCheckEngine::check_msb_rechnung_with_aufabschlaege(
                31_009,
                SENDER,
                &r,
                None,
                &contracted,
                &CheckConfig::default(),
            )
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::TariffNotFound && f.is_dispute)
        };

        assert!(
            disputed("Nachlass Sondervereinbarung"),
            "the blank entry must not back a discount nothing else names"
        );
        assert!(
            !disputed("Winterrabatt Netznutzung"),
            "a contracted name still authorises its discount"
        );
    }

    // ── The MSB/WiM path runs the same document checks as every other ────────

    /// A WiM/MSB invoice (PIDs 31003 and 31009) that states **no Umsatzsteuer
    /// at all** is disputed, exactly as a Netznutzungsrechnung is.
    ///
    /// §14 Abs. 4 Nr. 8 UStG makes the rate and the amount mandatory content of
    /// every invoice, and the INVOIC AHB agrees: `TAX` Nr. 00058 and `MOA`
    /// Nr. 00061/00062 are **Muss** on 31003 and 31009 just as on 31001/31002.
    #[test]
    fn an_msb_invoice_without_a_tax_block_is_disputed() {
        let pos = make_pos(
            1,
            "Messstellenbetrieb",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtsteuer = None;
        r.gesamtbrutto = None;
        r.steuerbetraege = None;

        let report = InvoicCheckEngine::check_msb_rechnung(
            31_009,
            SENDER,
            &r,
            None,
            &CheckConfig::default(),
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMissing && f.is_dispute),
            "an MSB invoice stating no Umsatzsteuer gives its recipient no \
             Vorsteuerabzug and must be disputed: {:#?}",
            report.findings
        );
        assert_eq!(report.outcome, CheckOutcome::Dispute);
    }

    /// **A zero tax with a stated ground is not a missing tax.** A §13b
    /// reverse-charged MSB invoice states 0,00 EUR by design, and naming the
    /// ground is what distinguishes it from an invoice that simply omits the
    /// tax — so wiring the Umsatzsteuer stage into this path must not dispute
    /// it.
    #[test]
    fn a_reverse_charged_msb_invoice_is_not_a_missing_tax_block() {
        let pos = make_pos(
            1,
            "Messstellenbetrieb",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::ZERO));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(100_000)));
        r.steuerbetraege = Some(vec![rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Rcv),
            steuersatz: Some(Decimal::ZERO),
            steuerwert: Some(Decimal::ZERO),
            ..Default::default()
        }]);

        let report = InvoicCheckEngine::check_msb_rechnung(
            31_009,
            SENDER,
            &r,
            None,
            &CheckConfig::default(),
        );
        assert!(
            !report.findings.iter().any(|f| matches!(
                f.kind,
                FindingKind::SteuerMissing | FindingKind::ReverseChargeStatesTax
            )),
            "a §13b invoice states no tax by design: {:#?}",
            report.findings
        );
    }

    /// A Fälligkeitsdatum before the invoice date is a dispute on the MSB path
    /// too. `SG8 DTM+265` is **Muss** on 31003 and 31009, so the date is there
    /// to be checked.
    #[test]
    fn an_msb_invoice_due_before_it_was_issued_is_disputed() {
        let mut r = make_rechnung(vec![], Some(EuroAmount::from_raw_units(100_000)));
        r.rechnungsdatum = Some(parse_dt("2026-07-15"));
        r.faelligkeitsdatum = Some(parse_dt("2026-07-01"));

        let report = InvoicCheckEngine::check_msb_rechnung(
            31_009,
            SENDER,
            &r,
            None,
            &CheckConfig::default(),
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ZahlungszielInvalid && f.is_dispute),
            "a due date before the invoice date must be disputed: {:#?}",
            report.findings
        );
        assert_eq!(report.outcome, CheckOutcome::Dispute);
    }

    /// A payment term beyond the 30 days of §7 Allgemeine Festlegungen V6.1d
    /// warns on the MSB path, as it does on the standard one — a warning, so
    /// the MSB can correct it.
    #[test]
    fn an_msb_invoice_with_an_overlong_zahlungsziel_warns() {
        let mut r = make_rechnung(vec![], Some(EuroAmount::from_raw_units(100_000)));
        r.rechnungsdatum = Some(parse_dt("2026-07-01"));
        r.faelligkeitsdatum = Some(parse_dt("2026-09-01")); // 62 days

        let report = InvoicCheckEngine::check_msb_rechnung(
            31_009,
            SENDER,
            &r,
            None,
            &CheckConfig::default(),
        );
        let finding = report
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::ZahlungszielExceeded)
            .unwrap_or_else(|| panic!("no ZahlungszielExceeded in {:#?}", report.findings));
        assert!(!finding.is_dispute, "ZahlungszielExceeded is a warning");
    }

    /// A breakdown that does add up passes — including one split across rates.
    #[test]
    fn a_tax_breakdown_split_across_rates_that_sums_is_clean() {
        use rust_decimal::Decimal;
        let mut r = make_rechnung(vec![], Some(EuroAmount::from_raw_units(100_000_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::from_raw_units(2_600_000))); // 26.00
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(102_600_000)));
        let entry = |satz: i64, basis: i64, wert: i64| rubo4e::current::Steuerbetrag {
            steuerart: Some(rubo4e::current::Steuerart::Ust),
            steuersatz: Some(Decimal::from(satz)),
            basiswert: Some(Decimal::from(basis)),
            steuerwert: Some(Decimal::from(wert)),
            ..Default::default()
        };
        r.steuerbetraege = Some(vec![entry(19, 100, 19), entry(7, 100, 7)]);
        let report =
            InvoicCheckEngine::check(31001, SENDER, &r, &empty_store(), &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMismatch),
            "19 + 7 = 26, which is what gesamtsteuer states"
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

    /// A Storno reverses an invoice that had to state Umsatzsteuer, so it states
    /// its own — and 31004 publishes the header `TAX` (Nr 00058) and `MOA`
    /// (00061/00062) as **Muss** in both imported Formatversionen. The Storno
    /// path skipped the Steuer stage entirely, so a reversal with no tax block
    /// at all was accepted.
    #[test]
    fn a_storno_without_a_tax_block_is_disputed() {
        let r = Rechnung {
            ist_storno: Some(true),
            original_rechnungsnummer: Some("31001-2026-001".to_owned()),
            ..Default::default()
        };

        let report = InvoicCheckEngine::check_storno(31_004, &r, &CheckConfig::default());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMissing),
            "a Storno stating no Umsatzsteuer must be disputed, got {:?}",
            report.findings
        );
    }

    /// The negated amounts are not an obstacle: `check_steuer` asserts
    /// `netto + steuer == brutto` and that the breakdown sums to `gesamtsteuer`,
    /// both of which hold under negation. A correctly-reversed Storno passes.
    #[test]
    fn a_storno_that_reverses_its_tax_is_accepted() {
        let r = Rechnung {
            ist_storno: Some(true),
            original_rechnungsnummer: Some("31001-2026-001".to_owned()),
            gesamtnetto: Some(betrag(EuroAmount::from_raw_units(-100_000))),
            gesamtsteuer: Some(betrag(EuroAmount::from_raw_units(-19_000))),
            gesamtbrutto: Some(betrag(EuroAmount::from_raw_units(-119_000))),
            steuerbetraege: Some(vec![rubo4e::current::Steuerbetrag {
                steuersatz: Some(Decimal::from(19)),
                steuerwert: Some(Decimal::from(-190)),
                ..Default::default()
            }]),
            ..Default::default()
        };

        let report = InvoicCheckEngine::check_storno(31_004, &r, &CheckConfig::default());
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::SteuerMissing),
            "a Storno that reverses its tax states one, got {:?}",
            report.findings
        );
    }

    /// Stating `0` is not the same as stating nothing, but with no breakdown and
    /// no reverse-charge entry it names no ground either — and that shape passed
    /// silently on every check path. It warns rather than disputes, because a
    /// §19 UStG Kleinunternehmer invoice may carry its ground in free text.
    #[test]
    fn zero_tax_with_no_stated_ground_is_reported() {
        let pos = make_pos(
            1,
            "DE001",
            None,
            None,
            Some(EuroAmount::from_raw_units(100_000)),
        );
        let mut r = make_rechnung(vec![pos], Some(EuroAmount::from_raw_units(100_000)));
        r.gesamtsteuer = Some(betrag(EuroAmount::ZERO));
        r.gesamtbrutto = Some(betrag(EuroAmount::from_raw_units(100_000)));
        r.steuerbetraege = None;

        let mut findings = Vec::new();
        InvoicCheckEngine::check_steuer(&r, &CheckConfig::default(), &mut findings);
        let f = findings
            .iter()
            .find(|f| f.kind == FindingKind::SteuerMissing)
            .expect("zero tax with no ground is reported");
        assert!(
            !f.is_dispute,
            "a lawful §19 UStG invoice must not be refused outright"
        );
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
        r.rechnungsdatum = Some(parse_dt("2026-07-01"));
        r.faelligkeitsdatum = Some(parse_dt("2026-07-31")); // exactly 30 days

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
        r.rechnungsdatum = Some(parse_dt("2026-07-01"));
        r.faelligkeitsdatum = Some(parse_dt("2026-09-01")); // 62 days — exceeds 30

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
        r.rechnungsdatum = Some(parse_dt("2026-07-15"));
        r.faelligkeitsdatum = Some(parse_dt("2026-07-01")); // before invoice date

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
        r.rechnungsdatum = Some(parse_dt("2026-01-01"));
        r.faelligkeitsdatum = Some(parse_dt("2026-12-31")); // 364 days — would normally trigger

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

    // ── The ESA's price basis is its accepted Angebot ─────────────────────────

    /// An ESA-flavoured position: the same shape as `make_pos`, plus the
    /// `SG26 LIN` DE 7143 `Z09` Artikel-ID that joins it to the offer.
    fn esa_pos(n: i64, artikel_id: &str, price: EuroAmount) -> Rechnungsposition {
        Rechnungsposition {
            artikel_id: Some(artikel_id.to_owned()),
            ..make_pos(n, "ESA-Messprodukt", Some("1"), Some(price), Some(price))
        }
    }

    /// `EuroAmount` is fixed-point at 5 decimal places, so one cent is 1 000
    /// raw units.
    fn cents(n: i64) -> EuroAmount {
        EuroAmount::from_raw_units(n * 1_000)
    }

    fn agreed() -> Vec<(String, EuroAmount)> {
        vec![
            // Betriebspreis, per Tag.
            ("9990001100002".to_owned(), cents(1)),
            // Einrichtungspreis, per Stück.
            ("9990001100001".to_owned(), cents(2_500)),
        ]
    }

    /// The offer priced it, the invoice bills it, the two agree.
    #[test]
    fn an_invoice_matching_the_accepted_angebot_passes() {
        let r = make_rechnung(
            vec![
                esa_pos(1, "9990001100002", cents(1)),
                esa_pos(2, "9990001100001", cents(2_500)),
            ],
            Some(cents(2_501)),
        );
        let report =
            InvoicCheckEngine::check_esa_rechnung(SENDER, &r, &agreed(), &CheckConfig::default());
        assert_eq!(
            report.outcome,
            CheckOutcome::Ok,
            "clean ESA invoice: {:?}",
            report.findings
        );
        assert_eq!(report.pid, 31009);
    }

    /// A price the ESA never agreed to is a dispute — and this is the check an
    /// ESA had **no** substitute for: `PreisblattMessung` is the MSB's sheet
    /// toward NB and LF, and there is none for Kapitel-4.6 Messprodukte, so the
    /// Preisblatt path skipped price checking entirely.
    #[test]
    fn a_position_billed_above_the_agreed_price_is_disputed() {
        let r = make_rechnung(
            // Agreed 25.00, billed 40.00.
            vec![esa_pos(1, "9990001100001", cents(4_000))],
            Some(cents(4_000)),
        );
        let report =
            InvoicCheckEngine::check_esa_rechnung(SENDER, &r, &agreed(), &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Dispute);
        let f = report
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::AngebotDeviation)
            .expect("the deviation is reported");
        assert_eq!(f.expected, Some(cents(2_500)));
        assert_eq!(f.actual, Some(cents(4_000)));
        assert!(f.is_dispute);
    }

    /// The offer prices one to three Artikel-IDs per position block (QUOTES AHB
    /// 1.1a condition `[2042]`); a fourth on the invoice is a charge nobody
    /// agreed to, which is a different defect from a wrong price.
    #[test]
    fn an_artikel_id_the_angebot_never_priced_is_its_own_finding() {
        let r = make_rechnung(
            vec![esa_pos(1, "9990009900009", cents(500))],
            Some(cents(500)),
        );
        let report =
            InvoicCheckEngine::check_esa_rechnung(SENDER, &r, &agreed(), &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Dispute);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::AngebotPositionUnknown),
            "{:?}",
            report.findings
        );
    }

    /// No accepted offer on record is a gap in **mako's** records, not a defect
    /// in the MSB's invoice — so it warns and skips, never disputes. Disputing
    /// it would send a REMADV 33002 rejecting a correct invoice.
    #[test]
    fn a_missing_angebot_warns_rather_than_disputing() {
        let r = make_rechnung(vec![esa_pos(1, "9990001100002", cents(1))], Some(cents(1)));
        let report =
            InvoicCheckEngine::check_esa_rechnung(SENDER, &r, &[], &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Warn);
        let f = report
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::TariffNotFound)
            .expect("the gap is reported");
        assert!(!f.is_dispute);
        assert!(f.message.contains("Angebot"), "{}", f.message);
    }

    /// DE 7143 admits `Z01` Artikelnummer beside `Z09` Artikel-ID, and an
    /// Artikelnummer names no offer position — so such a line is not comparable
    /// rather than wrong.
    #[test]
    fn a_position_without_an_artikel_id_is_skipped_not_disputed() {
        let r = make_rechnung(
            vec![make_pos(
                1,
                "Artikelnummer-Position",
                Some("1"),
                Some(cents(999)),
                Some(cents(999)),
            )],
            Some(cents(999)),
        );
        let report =
            InvoicCheckEngine::check_esa_rechnung(SENDER, &r, &agreed(), &CheckConfig::default());
        assert_eq!(report.outcome, CheckOutcome::Warn);
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.kind != FindingKind::AngebotDeviation)
        );
    }

    /// The structural checks still run: an ESA invoice is an invoice.
    #[test]
    fn the_esa_path_still_checks_arithmetic_and_totals() {
        let r = make_rechnung(
            // 1 × 0.01 EUR billed as a 5.00 EUR line net.
            vec![Rechnungsposition {
                artikel_id: Some("9990001100002".to_owned()),
                ..make_pos(1, "ESA", Some("1"), Some(cents(1)), Some(cents(500)))
            }],
            Some(cents(500)),
        );
        let report =
            InvoicCheckEngine::check_esa_rechnung(SENDER, &r, &agreed(), &CheckConfig::default());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::ArithmeticError),
            "{:?}",
            report.findings
        );
    }
}

#[cfg(test)]
mod waehrung_tests {
    use super::{CheckConfig, FindingKind, InvoicCheckEngine};
    use rubo4e::current::{Betrag, Rechnung, Waehrungscode};
    use rust_decimal::dec;

    fn betrag(wert: rust_decimal::Decimal, waehrung: Waehrungscode) -> Option<Betrag> {
        Some(Betrag {
            wert: Some(wert),
            waehrung: Some(waehrung),
            ..Default::default()
        })
    }

    /// The arithmetic below this check reads every amount as EUR, so a
    /// mixed-currency invoice does not fail it — it *passes* it, wrongly.
    #[test]
    fn a_mixed_currency_invoice_is_disputed() {
        let mut findings = Vec::new();
        let r = Rechnung {
            gesamtnetto: betrag(dec!(300.00), Waehrungscode::Eur),
            gesamtsteuer: betrag(dec!(57.00), Waehrungscode::Eur),
            gesamtbrutto: betrag(dec!(357.00), Waehrungscode::Chf),
            ..Default::default()
        };
        InvoicCheckEngine::check_waehrung(&r, &mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::WaehrungMismatch);
        assert!(findings[0].is_dispute);
        // …and note the totals themselves reconcile, which is the point.
        assert_eq!(dec!(300.00) + dec!(57.00), dec!(357.00));
    }

    #[test]
    fn one_currency_throughout_is_silent() {
        let mut findings = Vec::new();
        let r = Rechnung {
            gesamtnetto: betrag(dec!(300.00), Waehrungscode::Eur),
            gesamtbrutto: betrag(dec!(357.00), Waehrungscode::Eur),
            ..Default::default()
        };
        InvoicCheckEngine::check_waehrung(&r, &mut findings);
        assert!(findings.is_empty());
    }

    /// **Invariant: the check reaches every field that names a currency.**
    ///
    /// A position or a Steuerbetrag denominated differently from the header is
    /// read as EUR by every later stage exactly as a header field would be — and
    /// it is the positions that carry the arithmetic the recipient pays from.
    #[test]
    fn a_position_or_tax_entry_in_another_currency_is_disputed() {
        use rubo4e::current::{Rechnungsposition, Steuerbetrag};

        let mut findings = Vec::new();
        let r = Rechnung {
            gesamtnetto: betrag(dec!(300.00), Waehrungscode::Eur),
            rechnungspositionen: Some(vec![Rechnungsposition {
                positionsnummer: Some(1),
                gesamtpreis: betrag(dec!(300.00), Waehrungscode::Chf),
                ..Default::default()
            }]),
            ..Default::default()
        };
        InvoicCheckEngine::check_waehrung(&r, &mut findings);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::WaehrungMismatch);

        let mut findings = Vec::new();
        let r = Rechnung {
            gesamtnetto: betrag(dec!(300.00), Waehrungscode::Eur),
            steuerbetraege: Some(vec![Steuerbetrag {
                waehrungscode: Some(Waehrungscode::Chf),
                ..Default::default()
            }]),
            ..Default::default()
        };
        InvoicCheckEngine::check_waehrung(&r, &mut findings);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::WaehrungMismatch);
    }

    /// A document that states no currency at all is not this check's business —
    /// BO4E makes the field optional, and there is nothing to disagree about.
    #[test]
    fn an_absent_currency_is_not_a_mismatch() {
        let mut findings = Vec::new();
        let r = Rechnung {
            gesamtnetto: Some(Betrag {
                wert: Some(dec!(300.00)),
                ..Default::default()
            }),
            ..Default::default()
        };
        InvoicCheckEngine::check_waehrung(&r, &mut findings);
        assert!(findings.is_empty());
        let _ = CheckConfig::default();
    }
}
