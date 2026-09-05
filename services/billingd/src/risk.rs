//! Deterministic invoice risk scoring — the release gate between "calculated"
//! and "dispatched".
//!
//! Quality assurance is layered: rule-based validation (the engine's
//! `ValidationBlocked` — an invoice with an Error-severity violation never
//! exists), statistical baselines, and a banded risk score that routes
//! analyst attention instead of a binary pass/fail. This module is the
//! scoring layer — **deterministic and
//! explainable by construction**: every point on the score is a coded
//! [`RiskFinding`] with a human-readable reason, so no SHAP values are
//! needed to justify a hold. ML-based detection deliberately lives outside
//! the billing core (the platform's Iceberg/Arrow surface feeds external
//! analytics; agentd's LLM specialists investigate flagged invoices).
//!
//! ## Bands
//!
//! | Score | Band | Action |
//! |---|---|---|
//! | 0–19 | `AUTO_RELEASED` | dispatched immediately |
//! | 20–49 | `SAMPLE` | dispatched; sampled for review |
//! | 50–79 | `REVIEW` | dispatched; queued for analyst review |
//! | 80–100 | `HELD` | **not dispatched** until released by an analyst |
//!
//! Thresholds are operator-configurable (`[risk]` in `billingd.toml`); the
//! hold gate can be disabled (`hold_dispatch = false`) to run scoring in
//! shadow mode.

use energy_billing::{Invoice, PositionCategory, RoundMoney};
use rust_decimal::{Decimal, dec};
use serde::{Deserialize, Serialize};

// ── Findings ──────────────────────────────────────────────────────────────────

/// One scored observation about an invoice. The full set is persisted as
/// `billing_records.risk_findings` — the audit-proof explanation of the score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskFinding {
    /// Stable machine-readable code (e.g. `TAX_BREAKDOWN_MISMATCH`).
    pub code: String,
    /// Points this finding contributes to the score.
    pub weight: u8,
    /// Human-readable reason with the concrete values that triggered it.
    pub message: String,
    /// This finding holds the invoice on its own, whatever the score says.
    ///
    /// Most findings are *evidence* — they say an invoice looks unusual, and
    /// the score decides how much attention that earns. A few are *verdicts*:
    /// a period straddling a statutory rate boundary has no correct single
    /// rate, so part of what was billed is wrong no matter what else is true.
    /// Expressing a verdict as a large weight leaves it at the mercy of
    /// `hold_at`, which an operator may raise.
    #[serde(default)]
    pub blocking: bool,
}

/// Risk band derived from the score and the configured thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskBand {
    AutoReleased,
    Sample,
    Review,
    Held,
}

impl RiskBand {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoReleased => "AUTO_RELEASED",
            Self::Sample => "SAMPLE",
            Self::Review => "REVIEW",
            Self::Held => "HELD",
        }
    }
}

/// The scored result.
#[derive(Debug, Clone, Serialize)]
pub struct RiskAssessment {
    /// 0–100, saturating sum of finding weights.
    pub score: u8,
    pub band: RiskBand,
    pub findings: Vec<RiskFinding>,
}

// ── Config ────────────────────────────────────────────────────────────────────

/// `[risk]` section of `billingd.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskConfig {
    /// Score the invoice and persist the assessment. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// When the band is HELD, skip automatic dispatch until an analyst
    /// releases the record. `false` = shadow mode (score only). Default: true.
    #[serde(default = "default_true")]
    pub hold_dispatch: bool,
    /// Lower bound of the SAMPLE band. Default 20.
    #[serde(default = "default_sample")]
    pub sample_at: u8,
    /// Lower bound of the REVIEW band. Default 50.
    #[serde(default = "default_review")]
    pub review_at: u8,
    /// Lower bound of the HELD band. Default 80.
    #[serde(default = "default_hold")]
    pub hold_at: u8,
}

fn default_true() -> bool {
    true
}
fn default_sample() -> u8 {
    20
}
fn default_review() -> u8 {
    50
}
fn default_hold() -> u8 {
    80
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hold_dispatch: true,
            sample_at: default_sample(),
            review_at: default_review(),
            hold_at: default_hold(),
        }
    }
}

impl RiskConfig {
    /// Refuse a band configuration that cannot mean what it says.
    ///
    /// `band_for` tests the thresholds in descending order, so a configuration
    /// with `review_at` above `hold_at` does not produce the bands its author
    /// intended — it produces a REVIEW band that can never be reached, and
    /// invoices land in a queue nobody is watching. The scale is 0–100, so a
    /// threshold above 100 has the same effect.
    ///
    /// # Errors
    ///
    /// When the thresholds are not strictly ascending.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.sample_at < self.review_at && self.review_at < self.hold_at,
            "[risk] thresholds must ascend: sample_at ({}) < review_at ({}) < hold_at ({})",
            self.sample_at,
            self.review_at,
            self.hold_at
        );
        anyhow::ensure!(
            self.hold_at <= 100,
            "[risk] hold_at ({}) is above the 0–100 score scale, so no invoice can ever be held",
            self.hold_at
        );
        Ok(())
    }

    #[must_use]
    pub fn band_for(&self, score: u8) -> RiskBand {
        if score >= self.hold_at {
            RiskBand::Held
        } else if score >= self.review_at {
            RiskBand::Review
        } else if score >= self.sample_at {
            RiskBand::Sample
        } else {
            RiskBand::AutoReleased
        }
    }
}

// ── Cross-record context (fetched from PostgreSQL by the caller) ─────────────

/// History-derived inputs the pure scoring function cannot compute itself.
#[derive(Debug, Clone, Default)]
pub struct RiskContext {
    /// Average gross of the up-to-3 previous non-correction invoices for the
    /// MaLo. `None` when fewer than 2 exist (no baseline).
    pub rolling_avg_brutto_eur: Option<Decimal>,
    /// `period_to` of the latest previous invoice whose period started
    /// before this one. `None` for the first invoice of a MaLo.
    pub prev_period_to: Option<time::Date>,
    /// How many of the latest 3 previous invoices carried an
    /// `ESTIMATED_READING` finding.
    pub recent_estimated_count: i64,
}

// ── Scoring ───────────────────────────────────────────────────────────────────

/// The findings that hold an invoice on their own, whatever the score says.
///
/// Both mean the billed period straddles a statutory rate boundary, so it has
/// no correct single rate and part of what was billed is wrong — a verdict, not
/// evidence. Stated as a set rather than as a heavy weight, because a weight
/// only holds while it stays above `hold_at`, and raising that threshold is
/// ordinary tuning with no visible connection to this promise.
const BLOCKING_FINDINGS: [&str; 3] = [
    "MWST_STICHTAG_IM_ZEITRAUM",
    "BEHG_JAHRESGRENZE_IM_ZEITRAUM",
    "HT_NT_SUMME_WEICHT_AB",
];

/// Score one calculated invoice.
///
/// Pure given its inputs: the engine's invoice (warnings, positions,
/// totals), the statutory default VAT rate for the period, and the
/// history-derived [`RiskContext`]. Weights are fixed and documented — a
/// deterministic scorecard, not a model.
#[must_use]
pub fn assess(
    cfg: &RiskConfig,
    invoice: &Invoice,
    default_mwst_rate: Decimal,
    period_from: time::Date,
    period_to: time::Date,
    ctx: &RiskContext,
) -> RiskAssessment {
    let mut findings: Vec<RiskFinding> = Vec::new();
    let mut add = |code: &str, weight: u8, message: String| {
        findings.push(RiskFinding {
            code: code.to_owned(),
            weight,
            message,
            blocking: false,
        });
    };

    // ── Content checks ────────────────────────────────────────────────────────
    // Σ steuerbetraege must equal the invoice tax total (EN16931 BR-CO-14
    // discipline at runtime, not just by construction).
    let subtotals = invoice.tax_subtotals(default_mwst_rate);
    let tax_sum: Decimal = subtotals.iter().map(|s| s.tax_amount_eur).sum();
    if (tax_sum - invoice.mwst_eur.round_kfm(2)).abs() > dec!(0.01) {
        add(
            "TAX_BREAKDOWN_MISMATCH",
            60,
            format!(
                "Σ Steuerbeträge {} € ≠ gesamtsteuer {} €",
                tax_sum,
                invoice.mwst_eur.round_kfm(2)
            ),
        );
    }
    // Every applied VAT rate must be one German law has actually used. 16/5 are
    // the 01.07.–31.12.2020 Corona rates, still reachable through a correction
    // of that period; 0 is reverse charge, §19 Kleinunternehmer and the §12
    // Abs. 3 hardware supply.
    const GERMAN_VAT_RATES: [Decimal; 5] = [dec!(0), dec!(5), dec!(7), dec!(16), dec!(19)];
    for s in &subtotals {
        let pct = s.rate_percent.normalize();
        if !GERMAN_VAT_RATES.contains(&pct) {
            add(
                "INVALID_MWST_RATE",
                60,
                format!("USt-Satz {pct} % ist kein gültiger deutscher Satz (0/5/7/16/19)"),
            );
        }
    }

    // Consumption: signed, and only what the customer was charged for.
    let consumption_kwh: Decimal = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Commodity && p.unit.starts_with("kWh"))
        .map(|p| p.quantity)
        .sum();

    // Energy that moved at all, in either direction. A feed-in settlement bills
    // `Credit` positions and carries no `Commodity` line, so a measure counting
    // only consumption reads every EEG and Einspeisung invoice as a dead meter.
    // Magnitudes, not a signed sum: on a Mieterstrom invoice that both charges
    // consumption and credits a feed-in, the two would otherwise cancel.
    let energy_kwh: Decimal = invoice
        .positions
        .iter()
        .filter(|p| {
            matches!(
                p.category,
                PositionCategory::Commodity | PositionCategory::Credit
            ) && p.unit.starts_with("kWh")
        })
        .map(|p| p.quantity.abs())
        .sum();
    let period_days = (period_to - period_from).whole_days() + 1;

    if consumption_kwh < Decimal::ZERO {
        add(
            "NEGATIVE_CONSUMPTION",
            45,
            format!("Verbrauch {consumption_kwh} kWh ist negativ"),
        );
    }
    if energy_kwh == Decimal::ZERO && period_days >= 28 {
        add(
            "ZERO_ENERGY",
            30,
            format!("0 kWh über {period_days} Tage — Leerstand oder Messausfall?"),
        );
    }

    // ── Engine-warning findings (Layer 1 surfaced into the score) ─────────────
    //
    // The two boundary warnings are **verdicts**, not evidence: the period has
    // no correct single rate, so part of what was billed is wrong. billingd
    // refuses such a period upstream; one that reaches scoring anyway (an
    // operator-pinned rate, a preview promoted to a bill) must not dispatch.
    // They carry `blocking` rather than a heavy weight, because a weight only
    // holds while it stays above `hold_at` — which is operator-configurable.
    for w in &invoice.warnings {
        let (weight, code) = match w.code {
            "ESTIMATED_READING" => (15, "ESTIMATED_READING"),
            "VERBRAUCH_ABWEICHUNG_50PCT" => (25, "VORJAHR_DEVIATION"),
            "MWST_STICHTAG_IM_ZEITRAUM" => (80, "MWST_STICHTAG_IM_ZEITRAUM"),
            "BEHG_JAHRESGRENZE_IM_ZEITRAUM" => (80, "BEHG_JAHRESGRENZE_IM_ZEITRAUM"),
            "SECT40C_DEADLINE_EXCEEDED" => (10, "SECT40C_DEADLINE_EXCEEDED"),
            "PREISGARANTIE_ENDET" => (5, "PREISGARANTIE_ENDET"),
            // The index for a Preisgleitklausel has not arrived, and a static
            // price carried the invoice instead. Not blocking — the engine
            // already refuses the case where *nothing* can price the commodity
            // — but the customer is being billed a figure their contract does
            // not name, which is squarely an analyst's call.
            "INDEXWERT_FEHLT" => (40, "INDEXWERT_FEHLT"),
            // A § 40 Abs. 2 Nr. 6 Pflichtangabe is missing from the page. Not a
            // money defect, so a light weight — but an operator shipping every
            // invoice without it will see it accumulate.
            "ABLESUNGSART_FEHLT" => (10, "ABLESUNGSART_FEHLT"),
            // The HT/NT registers do not add up to the stated total. The engine
            // refuses it, so this only appears where a caller pinned rates past
            // the guard — and then it is a verdict, not evidence: one of the two
            // registers is wrong and the difference is billed at whichever rate
            // happens to apply.
            "HT_NT_SUMME_WEICHT_AB" => (80, "HT_NT_SUMME_WEICHT_AB"),
            // A hoheitliche Gebühr shares the document with a taxable supply.
            // Lawful on paper, and impossible as an e-invoice (BR-O-11 ff.) —
            // `to_en16931` refuses it, so a record that reaches scoring at all
            // is one an operator is about to send on paper. Worth a look.
            "GEBUEHR_UND_ENTGELT_AUF_EINEM_BELEG" => (30, "GEBUEHR_UND_ENTGELT_AUF_EINEM_BELEG"),
            _ => continue,
        };
        add(code, weight, w.message.clone());
    }
    // Meter exchange in the period rides as an Info position.
    if invoice
        .positions
        .iter()
        .any(|p| p.tags.iter().any(|t| t == "zaehlerwechsel"))
    {
        add(
            "METER_EXCHANGE",
            10,
            "Zählerwechsel im Abrechnungszeitraum — Ablesungen prüfen".to_owned(),
        );
    }

    // ── History checks ────────────────────────────────────────────────────────
    if let Some(avg) = ctx.rolling_avg_brutto_eur
        && avg > Decimal::ZERO
    {
        {
            let brutto = invoice.brutto_eur.round_kfm(2);
            let deviation_pct = ((brutto - avg) / avg * dec!(100)).round_kfm(1);
            if deviation_pct.abs() > dec!(50) {
                add(
                    "ROLLING_DEVIATION",
                    35,
                    format!("Brutto {brutto} € weicht {deviation_pct} % vom Mittel {avg} € ab"),
                );
            } else if deviation_pct.abs() > dec!(20) {
                add(
                    "ROLLING_DEVIATION",
                    20,
                    format!("Brutto {brutto} € weicht {deviation_pct} % vom Mittel {avg} € ab"),
                );
            }
        }
    }
    if let Some(prev_to) = ctx.prev_period_to {
        if prev_to >= period_from {
            add(
                "PERIOD_OVERLAP",
                50,
                format!(
                    "Zeitraum ab {period_from} überlappt die Vorrechnung (bis {prev_to}) — \
                     Doppelabrechnung möglich"
                ),
            );
        } else if (period_from - prev_to).whole_days() > 1 {
            add(
                "PERIOD_GAP",
                15,
                format!(
                    "Lücke von {} Tagen zwischen Vorrechnung (bis {prev_to}) und diesem \
                     Zeitraum (ab {period_from})",
                    (period_from - prev_to).whole_days() - 1
                ),
            );
        }
    }
    if ctx.recent_estimated_count >= 3 {
        add(
            "CONSECUTIVE_ESTIMATES",
            30,
            format!(
                "{} aufeinanderfolgende Rechnungen auf Schätzbasis — reale Ablesung anfordern \
                 (§ 40a Abs. 2 EnWG)",
                ctx.recent_estimated_count
            ),
        );
    }

    for f in &mut findings {
        f.blocking = BLOCKING_FINDINGS.contains(&f.code.as_str());
    }

    let score: u8 = findings
        .iter()
        .fold(0u32, |acc, f| acc + u32::from(f.weight))
        .min(100) as u8;

    // A verdict outranks the scorecard. Without this the guarantee that a
    // straddling period never dispatches would hold only while `hold_at` stays
    // at or below the finding's weight, and raising a threshold is a normal
    // tuning action with no visible connection to that promise.
    let band = if findings.iter().any(|f| f.blocking) {
        RiskBand::Held
    } else {
        cfg.band_for(score)
    };

    RiskAssessment {
        score,
        band,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use energy_billing::{BillingContext, BillingPeriod, GridInput, Product, Quantities};
    use time::macros::date;

    fn invoice(kwh: Decimal) -> Invoice {
        let rates = energy_billing::RegulatoryRates::default();
        let product: Product = serde_json::from_str(
            r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0","grundpreis_ct_per_day":"8.0"}"#,
        )
        .unwrap();
        let ctx = BillingContext {
            malo_id: "51238696781".into(),
            lf_mp_id: "9900000000001".into(),
            rechnungsnummer: "RISK-1".into(),
            period: BillingPeriod::new(date!(2026 - 06 - 01), date!(2026 - 06 - 30)).unwrap(),
            regulatory_rates: rates.clone(),
            ..Default::default()
        };
        let quantities = Quantities {
            electricity: Some(energy_billing::MeterInput {
                arbeitsmenge_kwh: kwh,
                ..Default::default()
            }),
            ..Default::default()
        };
        product
            .build_engine(&GridInput::default(), &rates)
            .bill(ctx, &quantities)
            .unwrap()
    }

    /// Raising `hold_at` must not release a straddling period.
    ///
    /// The two boundary findings are verdicts: the period has no correct single
    /// rate. Carried as a weight of 80 they hold only while `hold_at` stays at
    /// or below 80 — and raising a threshold is ordinary tuning that looks
    /// unrelated to this promise, so the invoice would dispatch with a silent
    /// over- or undercharge.
    #[test]
    fn a_raised_hold_threshold_cannot_release_a_straddling_period() {
        let mut inv = invoice(dec!(300));
        inv.warnings.push(energy_billing::BillingWarning {
            code: "MWST_STICHTAG_IM_ZEITRAUM",
            severity: energy_billing::WarningSeverity::Warning,
            message: "Zeitraum überschreitet eine Satzgrenze".to_owned(),
        });
        // Every threshold above the finding's own weight.
        let cfg = RiskConfig {
            sample_at: 90,
            review_at: 95,
            hold_at: 100,
            ..RiskConfig::default()
        };
        cfg.validate().expect("a legal band configuration");

        let a = assess(
            &cfg,
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &RiskContext::default(),
        );
        assert!(
            a.score < cfg.hold_at,
            "the score alone does not reach the band"
        );
        assert_eq!(a.band, RiskBand::Held, "the verdict holds it anyway");
        assert!(
            a.findings.iter().any(|f| f.blocking),
            "and it is recorded as the reason: {:?}",
            a.findings,
        );
    }

    /// A feed-in settlement carries no consumption, and that is not a fault.
    ///
    /// EEG positions are `PositionCategory::Credit`, so a measure that counts
    /// only `Commodity` kWh reads every feed-in invoice as a dead meter.
    #[test]
    fn a_feed_in_settlement_is_not_a_dead_meter() {
        let rates = energy_billing::RegulatoryRates::default();
        let product: Product =
            serde_json::from_str(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":"8.2"}"#)
                .expect("EEG product");
        let ctx = BillingContext {
            malo_id: "51238696781".into(),
            lf_mp_id: "9900000000001".into(),
            rechnungsnummer: "RISK-EEG".into(),
            period: BillingPeriod::new(date!(2026 - 06 - 01), date!(2026 - 06 - 30)).unwrap(),
            regulatory_rates: rates.clone(),
            ..Default::default()
        };
        let quantities = Quantities {
            eeg: Some(energy_billing::EegMeterInput {
                einspeisung_kwh: dec!(4200),
                ..Default::default()
            }),
            ..Default::default()
        };
        let inv = product
            .build_engine(&GridInput::default(), &rates)
            .bill(ctx, &quantities)
            .expect("EEG invoice");

        let a = assess(
            &RiskConfig::default(),
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &RiskContext::default(),
        );
        assert!(
            !a.findings.iter().any(|f| f.code == "ZERO_ENERGY"),
            "4200 kWh were fed in: {:?}",
            a.findings,
        );
    }

    /// A gas period crossing a statutory rate boundary has no correct single
    /// rate, so whatever was billed is wrong for part of it. Such an invoice
    /// must never dispatch automatically.
    ///
    /// billingd refuses these periods upstream; this is the backstop for a path
    /// that reaches scoring anyway — an operator-pinned rate, or a preview
    /// promoted to a bill.
    #[test]
    fn a_period_crossing_a_rate_boundary_is_held() {
        for code in ["MWST_STICHTAG_IM_ZEITRAUM", "BEHG_JAHRESGRENZE_IM_ZEITRAUM"] {
            let mut inv = invoice(dec!(300));
            inv.warnings.push(energy_billing::BillingWarning {
                code,
                severity: energy_billing::WarningSeverity::Warning,
                message: "Zeitraum überschreitet eine Satzgrenze".to_owned(),
            });
            let cfg = RiskConfig::default();
            let a = assess(
                &cfg,
                &inv,
                dec!(0.19),
                date!(2026 - 06 - 01),
                date!(2026 - 06 - 30),
                &RiskContext::default(),
            );
            assert_eq!(
                a.band,
                RiskBand::Held,
                "{code} must reach the HELD band on its own — score was {}",
                a.score
            );
            assert!(
                a.findings.iter().any(|f| f.code == code),
                "{code} must be a coded finding, not just a score bump"
            );
        }
    }

    #[test]
    fn a_clean_invoice_auto_releases() {
        let cfg = RiskConfig::default();
        let inv = invoice(dec!(300));
        let a = assess(
            &cfg,
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &RiskContext::default(),
        );
        assert_eq!(a.score, 0, "findings: {:?}", a.findings);
        assert_eq!(a.band, RiskBand::AutoReleased);
    }

    #[test]
    fn zero_energy_over_a_month_is_flagged() {
        let cfg = RiskConfig::default();
        let inv = invoice(Decimal::ZERO);
        let a = assess(
            &cfg,
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &RiskContext::default(),
        );
        assert!(a.findings.iter().any(|f| f.code == "ZERO_ENERGY"));
        assert_eq!(a.band, RiskBand::Sample);
    }

    #[test]
    fn period_overlap_plus_spike_holds_the_invoice() {
        let cfg = RiskConfig::default();
        let inv = invoice(dec!(900));
        let ctx = RiskContext {
            // Baseline ~30 € → this invoice (~280 €) deviates far over 50 %.
            rolling_avg_brutto_eur: Some(dec!(30)),
            // Previous invoice ran through 15 June — overlap.
            prev_period_to: Some(date!(2026 - 06 - 15)),
            recent_estimated_count: 0,
        };
        let a = assess(
            &cfg,
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &ctx,
        );
        assert!(a.findings.iter().any(|f| f.code == "PERIOD_OVERLAP"));
        assert!(a.findings.iter().any(|f| f.code == "ROLLING_DEVIATION"));
        assert_eq!(
            a.band,
            RiskBand::Held,
            "score {}: {:?}",
            a.score,
            a.findings
        );
    }

    #[test]
    fn a_gap_between_invoices_is_visible_but_releases() {
        let cfg = RiskConfig::default();
        let inv = invoice(dec!(300));
        let ctx = RiskContext {
            prev_period_to: Some(date!(2026 - 05 - 20)),
            ..Default::default()
        };
        let a = assess(
            &cfg,
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &ctx,
        );
        assert!(a.findings.iter().any(|f| f.code == "PERIOD_GAP"));
        assert_eq!(a.band, RiskBand::AutoReleased);
    }

    #[test]
    fn consecutive_estimates_escalate() {
        let cfg = RiskConfig::default();
        let inv = invoice(dec!(300));
        let ctx = RiskContext {
            recent_estimated_count: 3,
            ..Default::default()
        };
        let a = assess(
            &cfg,
            &inv,
            dec!(0.19),
            date!(2026 - 06 - 01),
            date!(2026 - 06 - 30),
            &ctx,
        );
        assert!(a.findings.iter().any(|f| f.code == "CONSECUTIVE_ESTIMATES"));
        assert_eq!(a.band, RiskBand::Sample);
    }

    /// A configuration whose bands cannot mean what they say is refused at
    /// startup, not discovered when an invoice lands in an unreachable queue.
    #[test]
    fn misordered_thresholds_are_refused() {
        assert!(RiskConfig::default().validate().is_ok());
        let inverted = RiskConfig {
            review_at: 90,
            hold_at: 80,
            ..RiskConfig::default()
        };
        assert!(inverted.validate().is_err(), "review_at above hold_at");
        let unreachable = RiskConfig {
            hold_at: 101,
            ..RiskConfig::default()
        };
        assert!(unreachable.validate().is_err(), "hold_at above the scale");
    }

    #[test]
    fn bands_follow_the_configured_thresholds() {
        let cfg = RiskConfig::default();
        assert_eq!(cfg.band_for(0), RiskBand::AutoReleased);
        assert_eq!(cfg.band_for(19), RiskBand::AutoReleased);
        assert_eq!(cfg.band_for(20), RiskBand::Sample);
        assert_eq!(cfg.band_for(50), RiskBand::Review);
        assert_eq!(cfg.band_for(80), RiskBand::Held);
        assert_eq!(cfg.band_for(100), RiskBand::Held);
    }
}
