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
    // Every applied VAT rate must be a rate that exists in German law.
    for s in &subtotals {
        let pct = s.rate_percent.normalize();
        if ![dec!(0), dec!(7), dec!(16), dec!(19)].contains(&pct) {
            add(
                "INVALID_MWST_RATE",
                60,
                format!("USt-Satz {pct} % ist kein gültiger deutscher Satz (0/7/16/19)"),
            );
        }
    }

    // Consumption facts from the positions.
    let consumption_kwh: Decimal = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Commodity && p.unit.starts_with("kWh"))
        .map(|p| p.quantity)
        .sum();
    let period_days = (period_to - period_from).whole_days() + 1;

    if consumption_kwh < Decimal::ZERO {
        add(
            "NEGATIVE_CONSUMPTION",
            45,
            format!("Verbrauch {consumption_kwh} kWh ist negativ"),
        );
    }
    if consumption_kwh == Decimal::ZERO && period_days >= 28 {
        add(
            "ZERO_CONSUMPTION",
            30,
            format!("0 kWh über {period_days} Tage — Leerstand oder Messausfall?"),
        );
    }

    // ── Engine-warning findings (Layer 1 surfaced into the score) ─────────────
    for w in &invoice.warnings {
        let (weight, code) = match w.code {
            "ESTIMATED_READING" => (15, "ESTIMATED_READING"),
            "VERBRAUCH_ABWEICHUNG_50PCT" => (25, "VORJAHR_DEVIATION"),
            "MWST_STICHTAG_IM_ZEITRAUM" => (20, "MWST_STICHTAG_IM_ZEITRAUM"),
            "SECT40C_DEADLINE_EXCEEDED" => (10, "SECT40C_DEADLINE_EXCEEDED"),
            "PREISGARANTIE_ENDET" => (5, "PREISGARANTIE_ENDET"),
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
                 (§ 60 Abs. 2 MsbG)",
                ctx.recent_estimated_count
            ),
        );
    }

    let score: u8 = findings
        .iter()
        .fold(0u32, |acc, f| acc + u32::from(f.weight))
        .min(100) as u8;

    RiskAssessment {
        score,
        band: cfg.band_for(score),
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
            r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0,"grundpreis_ct_per_day":8.0}"#,
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
    fn zero_consumption_over_a_month_is_flagged() {
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
        assert!(a.findings.iter().any(|f| f.code == "ZERO_CONSUMPTION"));
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
