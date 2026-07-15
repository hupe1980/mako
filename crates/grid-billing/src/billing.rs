//! NNE, MMM, and MSB invoice generation logic.
//!
//! All monetary arithmetic uses [`billing::EuroAmount`] internally for exact
//! representation.  Functions return [`GridInvoice`] — a pure domain type
//! with no BO4E coupling.  The service layer (netzbilanzd / invoicd) converts
//! `GridInvoice` to `rubo4e::current::Rechnung` via a local `into_rechnung()`
//! helper, keeping BO4E as a service-layer concern.

use billing::EuroAmount;
use rust_decimal::Decimal;

use crate::error::BillingError;
use crate::types::{GridInvoice, InvoicePosition, MmmInput, MsbInput, NneInput, QuantityUnit};

// ── helpers ───────────────────────────────────────────────────────────────────

const HUNDRED: Decimal = Decimal::from_parts(100, 0, 0, false, 0);

fn ct_to_eur(ct: Decimal) -> Decimal {
    ct / HUNDRED
}

fn pos_net(qty: Decimal, unit_price_eur: Decimal) -> Decimal {
    (qty * unit_price_eur).round_dp(5)
}

fn kwh_pos(number: u32, text: &str, kwh: Decimal, unit_price_eur: Decimal) -> InvoicePosition {
    InvoicePosition {
        number,
        text: text.to_owned(),
        quantity: kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kwh, unit_price_eur),
    }
}

fn kw_pos(number: u32, text: &str, kw: Decimal, unit_price_eur: Decimal) -> InvoicePosition {
    InvoicePosition {
        number,
        text: text.to_owned(),
        quantity: kw.round_dp(3),
        unit: QuantityUnit::Kw,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(kw, unit_price_eur),
    }
}

fn monat_pos(number: u32, text: &str, months: Decimal, unit_price_eur: Decimal) -> InvoicePosition {
    InvoicePosition {
        number,
        text: text.to_owned(),
        quantity: months.round_dp(3),
        unit: QuantityUnit::Monat,
        unit_price_eur: unit_price_eur.round_dp(6),
        net_eur: pos_net(months, unit_price_eur),
    }
}

fn decimal_to_euro_amount(d: Decimal) -> Result<EuroAmount, BillingError> {
    EuroAmount::checked_from_decimal(d).map_err(|_| BillingError::MonetaryOverflow)
}

// ── NNE invoice (PID 31001 / 31005 / 31006) ──────────────────────────────────

/// Calculate a NNE invoice (PID 31001 Strom, 31005 Gas, 31006 selbstausstellt).
///
/// Returns a [`GridInvoice`] with pure domain positions.  The service layer
/// converts this to BO4E `Rechnung` and validates via `invoic-checker`.
///
/// # Positions
///
/// | # | Description | Condition |
/// |---|---|---|
/// | 1 | Netznutzung Arbeit | flat mode |
/// | 1+2 | Arbeit HT + NT (§14a Modul 2) | ToU mode |
/// | next | Netznutzung Leistung | RLM only |
/// | last | Konzessionsabgabe | when `ka_satz_ct_per_kwh` set |
///
/// # Errors
///
/// [`BillingError::InvalidInput`] or [`BillingError::MonetaryOverflow`].
#[must_use = "handle the BillingError"]
pub fn calculate_nne_invoice(input: &NneInput) -> Result<GridInvoice, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.arbeitsmenge_kwh < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "arbeitsmenge_kwh must be non-negative".to_owned(),
        });
    }
    if input.spitzenleistung_kw.is_some() != input.leistungspreis_eur_per_kw.is_some() {
        return Err(BillingError::InvalidInput {
            reason: "spitzenleistung_kw and leistungspreis_eur_per_kw must both be set or both absent".to_owned(),
        });
    }

    let mut positions: Vec<InvoicePosition> = Vec::new();
    let mut total = Decimal::ZERO;
    let mut next: u32 = 1;

    // §14a Modul 2 ToU or flat Arbeit
    let has_tou = input.arbeitsmenge_ht_kwh.is_some()
        && input.arbeitspreis_ht_ct_per_kwh.is_some()
        && input.arbeitsmenge_nt_kwh.is_some()
        && input.arbeitspreis_nt_ct_per_kwh.is_some();

    if has_tou {
        let ht_kwh = input.arbeitsmenge_ht_kwh.unwrap();
        let ht_eur = ct_to_eur(input.arbeitspreis_ht_ct_per_kwh.unwrap());
        let p = kwh_pos(next, "Netznutzung Arbeit HT (§14a Modul 2)", ht_kwh, ht_eur);
        total += p.net_eur;
        positions.push(p);
        next += 1;

        let nt_kwh = input.arbeitsmenge_nt_kwh.unwrap();
        let nt_eur = ct_to_eur(input.arbeitspreis_nt_ct_per_kwh.unwrap());
        let p = kwh_pos(next, "Netznutzung Arbeit NT (§14a Modul 2)", nt_kwh, nt_eur);
        total += p.net_eur;
        positions.push(p);
        next += 1;
    } else {
        let eur = ct_to_eur(input.arbeitspreis_ct_per_kwh);
        let p = kwh_pos(next, "Netznutzung Arbeit", input.arbeitsmenge_kwh, eur);
        total += p.net_eur;
        positions.push(p);
        next += 1;
    }

    // Leistung (RLM only)
    if let (Some(sl_kw), Some(lp_eur)) = (input.spitzenleistung_kw, input.leistungspreis_eur_per_kw)
    {
        let p = kw_pos(next, "Netznutzung Leistung", sl_kw, lp_eur);
        total += p.net_eur;
        positions.push(p);
        next += 1;
    }

    // Konzessionsabgabe
    let ka_base_kwh = if has_tou {
        input.arbeitsmenge_ht_kwh.unwrap_or(Decimal::ZERO)
            + input.arbeitsmenge_nt_kwh.unwrap_or(Decimal::ZERO)
    } else {
        input.arbeitsmenge_kwh
    };
    if let Some(ka_ct) = input.ka_satz_ct_per_kwh {
        let p = kwh_pos(next, "Konzessionsabgabe", ka_base_kwh, ct_to_eur(ka_ct));
        total += p.net_eur;
        positions.push(p);
    }

    let total_eur = total.round_dp(2);
    decimal_to_euro_amount(total_eur)?;

    Ok(GridInvoice {
        pid: 31001,
        rechnungsnummer: input.rechnungsnummer.clone(),
        invoice_date: input.invoice_date,
        due_date: input.due_date,
        period_from: input.period_from,
        period_to: input.period_to,
        nb_mp_id: input.nb_mp_id.clone(),
        positions,
        total_eur,
    })
}

// ── MMM invoice (PID 31002) ───────────────────────────────────────────────────

/// Calculate a Mehr-/Mindermengen settlement invoice (PID 31002).
///
/// # Errors
///
/// [`BillingError::InvalidInput`] when `period_from >= period_to`.
#[must_use = "handle the BillingError"]
pub fn calculate_mmm_invoice(input: &MmmInput) -> Result<GridInvoice, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }

    let mehr_eur = ct_to_eur(input.mehr_preis_ct_per_kwh);
    let minder_eur = ct_to_eur(input.minder_preis_ct_per_kwh);
    let diff = input.actual_kwh - input.profil_kwh;

    let mehr_kwh = if diff > Decimal::ZERO {
        diff
    } else {
        Decimal::ZERO
    };
    let p1 = kwh_pos(1, "Mehrmengen", mehr_kwh, mehr_eur);

    let minder_kwh = if diff < Decimal::ZERO {
        -diff
    } else {
        Decimal::ZERO
    };
    let minder_net = -pos_net(minder_kwh, minder_eur);
    let p2 = InvoicePosition {
        number: 2,
        text: "Mindermengen (Gutschrift)".to_owned(),
        quantity: minder_kwh.round_dp(3),
        unit: QuantityUnit::Kwh,
        unit_price_eur: minder_eur.round_dp(6),
        net_eur: minder_net,
    };

    let total_eur = (p1.net_eur + p2.net_eur).round_dp(2);
    decimal_to_euro_amount(total_eur.abs())?;

    Ok(GridInvoice {
        pid: 31002,
        rechnungsnummer: input.rechnungsnummer.clone(),
        invoice_date: input.invoice_date,
        due_date: input.due_date,
        period_from: input.period_from,
        period_to: input.period_to,
        nb_mp_id: input.nb_mp_id.clone(),
        positions: vec![p1, p2],
        total_eur,
    })
}

// ── MSB invoice (PID 31009) ───────────────────────────────────────────────────

/// Calculate a MSB-Rechnung (PID 31009): NB → MSB metering service settlement.
///
/// # Errors
///
/// [`BillingError::InvalidInput`] or [`BillingError::MonetaryOverflow`].
#[must_use = "handle the BillingError"]
pub fn calculate_msb_invoice(input: &MsbInput) -> Result<GridInvoice, BillingError> {
    if input.period_from >= input.period_to {
        return Err(BillingError::InvalidInput {
            reason: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.grundgebuehr_eur_per_month < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "grundgebuehr_eur_per_month must be non-negative".to_owned(),
        });
    }
    if input.billing_months == 0 {
        return Err(BillingError::InvalidInput {
            reason: "billing_months must be at least 1".to_owned(),
        });
    }

    let mut positions: Vec<InvoicePosition> = Vec::new();
    let mut total = Decimal::ZERO;

    let months = Decimal::from(input.billing_months);
    let p = monat_pos(
        1,
        "Grundgebühr Messstellenbetrieb",
        months,
        input.grundgebuehr_eur_per_month,
    );
    total += p.net_eur;
    positions.push(p);

    if let Some(msl_eur) = input.messdienstleistung_eur {
        let msl = msl_eur.round_dp(5);
        total += msl;
        positions.push(InvoicePosition {
            number: 2,
            text: "Messdienstleistung".to_owned(),
            quantity: Decimal::ONE,
            unit: QuantityUnit::Monat,
            unit_price_eur: msl,
            net_eur: msl,
        });
    }

    let total_eur = total.round_dp(2);
    decimal_to_euro_amount(total_eur)?;

    Ok(GridInvoice {
        pid: 31009,
        rechnungsnummer: input.rechnungsnummer.clone(),
        invoice_date: input.invoice_date,
        due_date: input.due_date,
        period_from: input.period_from,
        period_to: input.period_to,
        nb_mp_id: input.nb_mp_id.clone(),
        positions,
        total_eur,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use time::macros::date;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("valid decimal literal")
    }

    fn base_nne() -> NneInput {
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
        let r = calculate_nne_invoice(&base_nne()).unwrap();
        assert_eq!(r.total_eur, d("52.50"));
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].unit, QuantityUnit::Kwh);
        assert_eq!(r.positions[0].net_eur, d("52.50000"));
    }

    #[test]
    fn nne_slp_with_ka() {
        let mut i = base_nne();
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.total_eur, d("54.15"));
        assert_eq!(r.positions.len(), 2);
        assert_eq!(r.positions[1].text, "Konzessionsabgabe");
    }

    #[test]
    fn nne_rlm_with_leistungspreis() {
        let mut i = base_nne();
        i.spitzenleistung_kw = Some(d("12.5"));
        i.leistungspreis_eur_per_kw = Some(d("4.20"));
        i.ka_satz_ct_per_kwh = Some(d("0.11"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.total_eur, d("106.65"));
        assert_eq!(r.positions.len(), 3);
        assert_eq!(r.positions[1].unit, QuantityUnit::Kw);
    }

    #[test]
    fn nne_sect14a_tou_arithmetic() {
        let mut i = base_nne();
        i.arbeitsmenge_ht_kwh = Some(d("900"));
        i.arbeitspreis_ht_ct_per_kwh = Some(d("4.0"));
        i.arbeitsmenge_nt_kwh = Some(d("600"));
        i.arbeitspreis_nt_ct_per_kwh = Some(d("2.0"));
        let r = calculate_nne_invoice(&i).unwrap();
        assert_eq!(r.total_eur, d("48.00"));
        assert_eq!(r.positions.len(), 2);
        assert_eq!(r.positions[0].text, "Netznutzung Arbeit HT (§14a Modul 2)");
        assert_eq!(r.positions[0].net_eur, d("36.00000"));
        assert_eq!(r.positions[1].net_eur, d("12.00000"));
    }

    #[test]
    fn nne_invalid_period() {
        let mut i = base_nne();
        i.period_to = i.period_from;
        assert!(matches!(
            calculate_nne_invoice(&i),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    #[test]
    fn nne_mismatched_rlm_fields() {
        let mut i = base_nne();
        i.spitzenleistung_kw = Some(d("10"));
        assert!(matches!(
            calculate_nne_invoice(&i),
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
            actual_kwh: d("1600"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let r = calculate_mmm_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("4.00"));
        assert_eq!(r.positions[1].net_eur, Decimal::ZERO);
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
            actual_kwh: d("1400"),
            profil_kwh: d("1500"),
            mehr_preis_ct_per_kwh: d("4.0"),
            minder_preis_ct_per_kwh: d("2.0"),
        };
        let r = calculate_mmm_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("-2.00"));
        assert_eq!(r.positions[1].net_eur, d("-2.00000"));
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
        let r = calculate_msb_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("12.50"));
        assert_eq!(r.pid, 31009);
        assert_eq!(r.positions.len(), 1);
        assert_eq!(r.positions[0].unit, QuantityUnit::Monat);
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
        let r = calculate_msb_invoice(&input).unwrap();
        assert_eq!(r.total_eur, d("45.50"));
        assert_eq!(r.positions.len(), 2);
    }

    #[test]
    fn pid_is_mutable_for_overrides() {
        let mut r = calculate_nne_invoice(&base_nne()).unwrap();
        r.pid = 31005;
        assert_eq!(r.pid, 31005);
    }
}
