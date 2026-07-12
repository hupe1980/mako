//! NNE and MMM invoice generation logic.
//!
//! All monetary arithmetic uses [`EuroAmount`] (`i64 × 10⁻⁵`) internally for
//! exact representation, then round-trips through [`rust_decimal::Decimal`] for
//! the BO4E `Betrag` / `Preis` fields.
//!
//! `EuroAmount` is now `billing::Amount<5>` — a fixed-point integer type from the
//! standalone `billing` crate. This removes the transitive `invoic-checker → rubo4e`
//! path for a type that has no BO4E dependency.

use billing::EuroAmount;
use rubo4e::current::{Betrag, Menge, Mengeneinheit, Preis, Rechnung, Rechnungsposition, Zeitraum};
use rust_decimal::Decimal;
use time::Date;

use crate::error::BillingError;
use crate::types::{BillingResult, MmmInput, MsbInput, NneInput};

// ── helpers ───────────────────────────────────────────────────────────────────

const HUNDRED: Decimal = Decimal::from_parts(100, 0, 0, false, 0);

/// Convert ct/kWh (tariff) to EUR/kWh (BO4E Preis.wert).
fn ct_to_eur(ct: Decimal) -> Decimal {
    ct / HUNDRED
}

fn decimal_to_betrag(d: Decimal) -> Betrag {
    Betrag {
        wert: Some(d.round_dp(5)),
        ..Default::default()
    }
}

fn kwh_menge(kwh: Decimal) -> Menge {
    Menge {
        wert: Some(kwh.round_dp(3)),
        einheit: Some(Mengeneinheit::Kwh),
        ..Default::default()
    }
}

fn kw_menge(kw: Decimal) -> Menge {
    Menge {
        wert: Some(kw.round_dp(3)),
        einheit: Some(Mengeneinheit::Kw),
        ..Default::default()
    }
}

fn eur_per_kwh_preis(eur_per_kwh: Decimal) -> Preis {
    Preis {
        wert: Some(eur_per_kwh.round_dp(6)),
        ..Default::default()
    }
}

fn eur_per_kw_preis(eur_per_kw: Decimal) -> Preis {
    Preis {
        wert: Some(eur_per_kw.round_dp(6)),
        ..Default::default()
    }
}

fn periode(from: Date, to: Date) -> Zeitraum {
    Zeitraum {
        startdatum: Some(from),
        enddatum: Some(to),
        ..Default::default()
    }
}

/// Multiply quantity × unit_price, return the net amount rounded to 5 dp.
fn position_net(qty: Decimal, unit_price_eur: Decimal) -> Decimal {
    (qty * unit_price_eur).round_dp(5)
}

fn decimal_to_euro_amount(d: Decimal) -> Result<EuroAmount, BillingError> {
    EuroAmount::checked_from_decimal(d).map_err(|_| BillingError::MonetaryOverflow)
}

// ── NNE invoice (PID 31001 / 31005) ──────────────────────────────────────────

/// Calculate a NNE invoice (PID 31001 for Strom, 31005 for Gas).
///
/// Generates a [`BillingResult`] containing a BO4E `Rechnung` that satisfies
/// `invoic-checker` checks 1–3 (period, arithmetic, total) by construction.
///
/// # Billing positions
///
/// | Pos | Description | Unit | Condition |
/// |---|---|---|---|
/// | 1 | Netznutzung Arbeit | ct/kWh × kWh | Always |
/// | 2 | Netznutzung Leistung | EUR/kW × kW | RLM only |
/// | 3 | Konzessionsabgabe | ct/kWh × kWh | When `ka_satz_ct_per_kwh` is set |
///
/// # Errors
///
/// Returns [`BillingError::InvalidInput`] when:
/// - `period_from >= period_to`
/// - `arbeitsmenge_kwh` is negative
/// - `spitzenleistung_kw` is set but `leistungspreis_eur_per_kw` is not (or vice versa)
///
/// Returns [`BillingError::MonetaryOverflow`] when the total exceeds the
/// representable range (~92 million EUR).
#[must_use = "handle the BillingError"]
pub fn calculate_nne_invoice(input: &NneInput) -> Result<BillingResult, BillingError> {
    // ── Validate inputs ─────────────────────────────────────────────────────
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to",
        });
    }
    if input.arbeitsmenge_kwh < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "arbeitsmenge_kwh must be non-negative",
        });
    }
    if input.spitzenleistung_kw.is_some() != input.leistungspreis_eur_per_kw.is_some() {
        return Err(BillingError::InvalidInput {
            reason: "spitzenleistung_kw and leistungspreis_eur_per_kw must both be set or both absent",
        });
    }

    let lz = periode(input.period_from, input.period_to);
    let mut positions: Vec<Rechnungsposition> = Vec::new();
    let mut total = Decimal::ZERO;

    // ── Position 1(a/b): Netznutzung Arbeit — flat OR §14a Modul 2 ToU ───────
    //
    // §14a Modul 2 (BNetzA BK6-22-300): when the MaLo is equipped with a
    // smart meter and ToU tariff bands, the NB MUST bill HT and NT separately.
    // The `arbeitsmenge_ht_kwh` / `arbeitsmenge_nt_kwh` fields carry the
    // edmd-reported split; `arbeitspreis_ht_ct_per_kwh` / `..._nt_ct_per_kwh`
    // come from `PreisblattNetznutzung.zeitvariablePreispositionen`.
    let has_tou = input.arbeitsmenge_ht_kwh.is_some()
        && input.arbeitspreis_ht_ct_per_kwh.is_some()
        && input.arbeitsmenge_nt_kwh.is_some()
        && input.arbeitspreis_nt_ct_per_kwh.is_some();

    if has_tou {
        // HT (Hochlasttarif) — high-price band
        let ht_kwh = input.arbeitsmenge_ht_kwh.unwrap();
        let ht_eur = ct_to_eur(input.arbeitspreis_ht_ct_per_kwh.unwrap());
        let ht_kosten = position_net(ht_kwh, ht_eur);
        total += ht_kosten;
        positions.push(Rechnungsposition {
            positionsnummer: Some(1),
            positionstext: Some("Netznutzung Arbeit HT (§14a Modul 2)".to_owned()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(kwh_menge(ht_kwh)),
            einzelpreis: Some(eur_per_kwh_preis(ht_eur)),
            gesamtpreis: Some(decimal_to_betrag(ht_kosten)),
            ..Default::default()
        });

        // NT (Niedertarif) — off-peak reduced band
        let nt_kwh = input.arbeitsmenge_nt_kwh.unwrap();
        let nt_eur = ct_to_eur(input.arbeitspreis_nt_ct_per_kwh.unwrap());
        let nt_kosten = position_net(nt_kwh, nt_eur);
        total += nt_kosten;
        positions.push(Rechnungsposition {
            positionsnummer: Some(2),
            positionstext: Some("Netznutzung Arbeit NT (§14a Modul 2)".to_owned()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(kwh_menge(nt_kwh)),
            einzelpreis: Some(eur_per_kwh_preis(nt_eur)),
            gesamtpreis: Some(decimal_to_betrag(nt_kosten)),
            ..Default::default()
        });
    } else {
        // Flat Arbeitspreis (static NNE, SLP/RLM without ToU)
        let arbeitspreis_eur = ct_to_eur(input.arbeitspreis_ct_per_kwh);
        let arbeitskosten = position_net(input.arbeitsmenge_kwh, arbeitspreis_eur);
        total += arbeitskosten;
        positions.push(Rechnungsposition {
            positionsnummer: Some(1),
            positionstext: Some("Netznutzung Arbeit".to_owned()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(kwh_menge(input.arbeitsmenge_kwh)),
            einzelpreis: Some(eur_per_kwh_preis(arbeitspreis_eur)),
            gesamtpreis: Some(decimal_to_betrag(arbeitskosten)),
            ..Default::default()
        });
    }

    // Next position number after Arbeit (1 flat or 1+2 ToU)
    let next_pos = if has_tou { 3u32 } else { 2u32 };

    // ── Leistung (RLM only) ───────────────────────────────────────────────────
    if let (Some(sl_kw), Some(lp_eur_per_kw)) =
        (input.spitzenleistung_kw, input.leistungspreis_eur_per_kw)
    {
        let leistungskosten = position_net(sl_kw, lp_eur_per_kw);
        total += leistungskosten;

        positions.push(Rechnungsposition {
            positionsnummer: Some(next_pos as i64),
            positionstext: Some("Netznutzung Leistung".to_owned()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(kw_menge(sl_kw)),
            einzelpreis: Some(eur_per_kw_preis(lp_eur_per_kw)),
            gesamtpreis: Some(decimal_to_betrag(leistungskosten)),
            ..Default::default()
        });
    }

    // ── Konzessionsabgabe ─────────────────────────────────────────────────────
    // KA base is always total arbeitsmenge_kwh (flat + HT + NT sum = total).
    let ka_base_kwh = if has_tou {
        input.arbeitsmenge_ht_kwh.unwrap_or(Decimal::ZERO)
            + input.arbeitsmenge_nt_kwh.unwrap_or(Decimal::ZERO)
    } else {
        input.arbeitsmenge_kwh
    };

    if let Some(ka_ct) = input.ka_satz_ct_per_kwh {
        let ka_eur = ct_to_eur(ka_ct);
        let ka_kosten = position_net(ka_base_kwh, ka_eur);
        total += ka_kosten;
        let ka_pos = if input.spitzenleistung_kw.is_some() {
            next_pos + 1
        } else {
            next_pos
        };
        positions.push(Rechnungsposition {
            positionsnummer: Some(ka_pos as i64),
            positionstext: Some("Konzessionsabgabe".to_owned()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(kwh_menge(ka_base_kwh)),
            einzelpreis: Some(eur_per_kwh_preis(ka_eur)),
            gesamtpreis: Some(decimal_to_betrag(ka_kosten)),
            ..Default::default()
        });
    }

    // Round total to 2 decimal places (EUR, standard invoice precision).
    let total_rounded = total.round_dp(2);

    // Validate EuroAmount range.
    decimal_to_euro_amount(total_rounded)?;

    let positions_count = positions.len();

    // Determine PID based on sparte (caller can override if needed; use field convention).
    // PID 31001 = Strom, 31005 = Gas.  The caller sets both; we default to 31001.
    let pid = 31001_u32;

    let rechnung = Rechnung {
        rechnungsnummer: Some(input.rechnungsnummer.clone()),
        rechnungsdatum: Some(input.invoice_date),
        faelligkeitsdatum: Some(input.due_date),
        rechnungsperiode: Some(lz),
        gesamtnetto: Some(decimal_to_betrag(total_rounded)),
        rechnungspositionen: Some(positions),
        ..Default::default()
    };

    Ok(BillingResult {
        rechnung,
        pid,
        total_eur: total_rounded,
        nb_mp_id: input.nb_mp_id.clone(),
        positions_count,
    })
}

// ── MMM invoice (PID 31002) ───────────────────────────────────────────────────

/// Calculate a Mehr-/Mindermengen settlement invoice (PID 31002).
///
/// Generates one position for Mehrmengen (actual > profil) and one for
/// Mindermengen (actual < profil).  The net can be negative, representing a
/// credit from NB to LF.
///
/// # Errors
///
/// Returns [`BillingError::InvalidInput`] when `period_from >= period_to`.
#[must_use = "handle the BillingError"]
pub fn calculate_mmm_invoice(input: &MmmInput) -> Result<BillingResult, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to",
        });
    }

    let lz = periode(input.period_from, input.period_to);
    let mut positions: Vec<Rechnungsposition> = Vec::new();
    let mut total = Decimal::ZERO;

    let mehr_eur = ct_to_eur(input.mehr_preis_ct_per_kwh);
    let minder_eur = ct_to_eur(input.minder_preis_ct_per_kwh);

    let diff = input.actual_kwh - input.profil_kwh;

    // ── Position 1: Mehrmengen ───────────────────────────────────────────────
    let mehr_kwh = if diff > Decimal::ZERO {
        diff
    } else {
        Decimal::ZERO
    };
    let mehr_kosten = position_net(mehr_kwh, mehr_eur);
    total += mehr_kosten;

    positions.push(Rechnungsposition {
        positionsnummer: Some(1),
        positionstext: Some("Mehrmengen".to_owned()),
        lieferungszeitraum: Some(lz.clone()),
        positions_menge: Some(kwh_menge(mehr_kwh)),
        einzelpreis: Some(eur_per_kwh_preis(mehr_eur)),
        gesamtpreis: Some(decimal_to_betrag(mehr_kosten)),
        ..Default::default()
    });

    // ── Position 2: Mindermengen (credit — negative gesamtpreis) ─────────────
    let minder_kwh = if diff < Decimal::ZERO {
        -diff
    } else {
        Decimal::ZERO
    };
    let minder_kosten = -position_net(minder_kwh, minder_eur); // negative = credit
    total += minder_kosten;

    positions.push(Rechnungsposition {
        positionsnummer: Some(2),
        positionstext: Some("Mindermengen (Gutschrift)".to_owned()),
        lieferungszeitraum: Some(lz.clone()),
        positions_menge: Some(kwh_menge(minder_kwh)),
        einzelpreis: Some(eur_per_kwh_preis(minder_eur)),
        gesamtpreis: Some(decimal_to_betrag(minder_kosten)),
        ..Default::default()
    });

    let total_rounded = total.round_dp(2);
    decimal_to_euro_amount(total_rounded.abs())?;

    let positions_count = positions.len();

    let rechnung = Rechnung {
        rechnungsnummer: Some(input.rechnungsnummer.clone()),
        rechnungsdatum: Some(input.invoice_date),
        faelligkeitsdatum: Some(input.due_date),
        rechnungsperiode: Some(lz),
        gesamtnetto: Some(decimal_to_betrag(total_rounded)),
        rechnungspositionen: Some(positions),
        ..Default::default()
    };

    Ok(BillingResult {
        rechnung,
        pid: 31002,
        total_eur: total_rounded,
        nb_mp_id: input.nb_mp_id.clone(),
        positions_count,
    })
}

// ── MSB invoice (PID 31009) ───────────────────────────────────────────────────

/// Calculate a MSB-Rechnung (PID 31009): NB → MSB metering service settlement.
///
/// # Billing positions
///
/// | Pos | Description | Unit | Condition |
/// |---|---|---|---|
/// | 1 | Grundgebühr Messstellenbetrieb | EUR/month × months | Always |
/// | 2 | Messdienstleistung | EUR | When `messdienstleistung_eur` is set |
///
/// # Errors
///
/// Returns [`BillingError::InvalidInput`] when:
/// - `period_from >= period_to`
/// - `grundgebuehr_eur_per_month` is negative
/// - `billing_months` is zero
///
/// Returns [`BillingError::MonetaryOverflow`] when the total exceeds the
/// safe integer range for [`EuroAmount`].
#[must_use = "handle the BillingError"]
pub fn calculate_msb_invoice(input: &MsbInput) -> Result<BillingResult, BillingError> {
    // ── Validation ────────────────────────────────────────────────────────────
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to",
        });
    }
    if input.grundgebuehr_eur_per_month < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "grundgebuehr_eur_per_month must be non-negative",
        });
    }
    if input.billing_months == 0 {
        return Err(BillingError::InvalidInput {
            reason: "billing_months must be at least 1",
        });
    }

    let lz = periode(input.period_from, input.period_to);
    let mut positions: Vec<Rechnungsposition> = Vec::new();
    let mut total = Decimal::ZERO;

    // ── Position 1: Grundgebühr Messstellenbetrieb ────────────────────────────
    let months = Decimal::from(input.billing_months);
    let gb_net = position_net(months, input.grundgebuehr_eur_per_month);
    total += gb_net;

    positions.push(Rechnungsposition {
        positionsnummer: Some(1),
        positionstext: Some("Grundgebühr Messstellenbetrieb".to_owned()),
        lieferungszeitraum: Some(lz.clone()),
        positions_menge: Some(Menge {
            wert: Some(months.round_dp(3)),
            einheit: Some(Mengeneinheit::Monat),
            ..Default::default()
        }),
        einzelpreis: Some(Preis {
            wert: Some(input.grundgebuehr_eur_per_month.round_dp(6)),
            ..Default::default()
        }),
        gesamtpreis: Some(decimal_to_betrag(gb_net)),
        ..Default::default()
    });

    // ── Position 2: Messdienstleistung (optional) ─────────────────────────────
    if let Some(msl_eur) = input.messdienstleistung_eur {
        let msl_net = msl_eur.round_dp(5);
        total += msl_net;

        positions.push(Rechnungsposition {
            positionsnummer: Some(2),
            positionstext: Some("Messdienstleistung".to_owned()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(Menge {
                wert: Some(Decimal::ONE),
                einheit: Some(Mengeneinheit::Monat),
                ..Default::default()
            }),
            einzelpreis: Some(Preis {
                wert: Some(msl_net),
                ..Default::default()
            }),
            gesamtpreis: Some(decimal_to_betrag(msl_net)),
            ..Default::default()
        });
    }

    let total_rounded = total.round_dp(2);
    // Validate EuroAmount range.
    decimal_to_euro_amount(total_rounded)?;

    let positions_count = positions.len();

    let rechnung = Rechnung {
        rechnungsnummer: Some(input.rechnungsnummer.clone()),
        rechnungsdatum: Some(input.invoice_date),
        faelligkeitsdatum: Some(input.due_date),
        rechnungsperiode: Some(lz),
        gesamtnetto: Some(decimal_to_betrag(total_rounded)),
        rechnungspositionen: Some(positions),
        ..Default::default()
    };

    Ok(BillingResult {
        rechnung,
        pid: 31009,
        total_eur: total_rounded,
        nb_mp_id: input.nb_mp_id.clone(),
        positions_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use invoic_checker::{InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore};
    use rust_decimal::Decimal;
    use time::macros::date;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("valid decimal literal")
    }

    fn base_nne_input() -> NneInput {
        NneInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "NNE-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            arbeitsmenge_kwh: d("1500"),
            arbeitspreis_ct_per_kwh: d("3.5"),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
        }
    }

    #[test]
    fn nne_slp_no_ka_arithmetic() {
        let result = calculate_nne_invoice(&base_nne_input()).unwrap();
        // 1500 kWh × 3.5 ct/kWh = 1500 × 0.035 = 52.50 EUR
        assert_eq!(result.total_eur, d("52.50"));
        assert_eq!(result.positions_count, 1);
        // Invoic-checker must pass
        let report = InvoicCheckEngine::check(
            31001,
            &result.nb_mp_id,
            &result.rechnung,
            &InMemoryPreisblattStore::new(),
            &CheckConfig::default(),
        );
        assert!(!report.has_dispute());
    }

    #[test]
    fn nne_slp_with_ka() {
        let mut input = base_nne_input();
        input.ka_satz_ct_per_kwh = Some(d("0.11"));
        let result = calculate_nne_invoice(&input).unwrap();
        // 1500 × 0.035 + 1500 × 0.0011 = 52.50 + 1.65 = 54.15 EUR
        assert_eq!(result.total_eur, d("54.15"));
        assert_eq!(result.positions_count, 2);
        let report = InvoicCheckEngine::check(
            31001,
            &result.nb_mp_id,
            &result.rechnung,
            &InMemoryPreisblattStore::new(),
            &CheckConfig::default(),
        );
        assert!(!report.has_dispute());
    }

    #[test]
    fn nne_rlm_with_leistungspreis() {
        let mut input = base_nne_input();
        input.spitzenleistung_kw = Some(d("12.5"));
        input.leistungspreis_eur_per_kw = Some(d("4.20"));
        input.ka_satz_ct_per_kwh = Some(d("0.11"));
        let result = calculate_nne_invoice(&input).unwrap();
        // 1500 × 0.035 = 52.50 (Arbeit)
        // 12.5 × 4.20 = 52.50 (Leistung)
        // 1500 × 0.0011 = 1.65 (KA)
        // Total = 106.65
        assert_eq!(result.total_eur, d("106.65"));
        assert_eq!(result.positions_count, 3);
        let report = InvoicCheckEngine::check(
            31001,
            &result.nb_mp_id,
            &result.rechnung,
            &InMemoryPreisblattStore::new(),
            &CheckConfig::default(),
        );
        assert!(!report.has_dispute());
    }

    #[test]
    fn nne_invalid_period() {
        let mut input = base_nne_input();
        input.period_to = input.period_from; // equal = invalid
        assert!(matches!(
            calculate_nne_invoice(&input),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    #[test]
    fn nne_mismatched_rlm_fields() {
        let mut input = base_nne_input();
        input.spitzenleistung_kw = Some(d("10"));
        // leistungspreis_eur_per_kw is None → mismatch
        assert!(matches!(
            calculate_nne_invoice(&input),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    #[test]
    fn mmm_mehr_settlement() {
        let input = MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "MMM-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            actual_kwh: d("1600"), // 100 kWh more than profil
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let result = calculate_mmm_invoice(&input).unwrap();
        // Mehr: 100 × 0.04 = 4.00 EUR; Minder: 0; Total = 4.00
        assert_eq!(result.total_eur, d("4.00"));
        let report = InvoicCheckEngine::check(
            31002,
            &result.nb_mp_id,
            &result.rechnung,
            &InMemoryPreisblattStore::new(),
            &CheckConfig::default(),
        );
        assert!(!report.has_dispute());
    }

    #[test]
    fn mmm_minder_credit() {
        let input = MmmInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            lf_mp_id: "9900012345678".into(),
            rechnungsnummer: "MMM-TEST-002".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            actual_kwh: d("1400"), // 100 kWh less than profil
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let result = calculate_mmm_invoice(&input).unwrap();
        // Mehr: 0; Minder: -100 × 0.02 = -2.00 EUR (credit); Total = -2.00
        assert_eq!(result.total_eur, d("-2.00"));
    }

    #[test]
    fn msb_grundgebuehr_only() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            rechnungsnummer: "MSB-TEST-001".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            invoice_date: date!(2025 - 02 - 15),
            due_date: date!(2025 - 03 - 15),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 1,
            messdienstleistung_eur: None,
        };
        let result = calculate_msb_invoice(&input).unwrap();
        // 1 month × 12.50 EUR = 12.50 EUR
        assert_eq!(result.total_eur, d("12.50"));
        assert_eq!(result.pid, 31009);
        assert_eq!(result.positions_count, 1);
    }

    #[test]
    fn msb_with_messdienstleistung() {
        let input = MsbInput {
            malo_id: "51238696780".into(),
            nb_mp_id: "9900357000004".into(),
            msb_mp_id: "9900123400001".into(),
            rechnungsnummer: "MSB-TEST-002".into(),
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 03 - 31),
            invoice_date: date!(2025 - 04 - 15),
            due_date: date!(2025 - 05 - 15),
            grundgebuehr_eur_per_month: d("12.50"),
            billing_months: 3,
            messdienstleistung_eur: Some(d("8.00")),
        };
        let result = calculate_msb_invoice(&input).unwrap();
        // 3 months × 12.50 = 37.50 + 8.00 MDL = 45.50 EUR
        assert_eq!(result.total_eur, d("45.50"));
        assert_eq!(result.positions_count, 2);
    }
}
