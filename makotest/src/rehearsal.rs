//! Offline NB decision for a consuming, decommissioned Strom Marktlokation.

use mako_pruefung::{NbEntscheidung, NetzCheckConfig, nb::evaluate_anmeldung_teilnahme};
use pyo3::{exceptions::PyValueError, prelude::*};
use rubo4e::identifiers::MaloId;

/// Evaluate the consuming branch with grid membership and Stilllegung attested at receipt.
#[pyfunction]
#[must_use = "the result carries the decision or the input error"]
pub(crate) fn decommissioned_nb_refusal(
    malo: &str,
    delivery: &str,
    receipt: &str,
) -> PyResult<(String, String, u16)> {
    malo.parse::<MaloId>()
        .map_err(|_| PyValueError::new_err("invalid MaLo"))?;
    let instant = crate::fristen::parse_dt(receipt)?;
    let outcome = evaluate_anmeldung_teilnahme(
        malo,
        true,
        None,
        mako_fristen::berlin_date(instant),
        crate::fristen::parse_date(delivery)?,
        NetzCheckConfig::default().holiday_calendar,
    );
    match outcome {
        Some(NbEntscheidung::Reject(reason)) => Ok((
            reason.antwort.tree,
            reason.antwort.antwortcode,
            reason.pruefschritt,
        )),
        _ => Err(PyValueError::new_err("unsupported decision")),
    }
}

type InvoiceVerdict = (Option<u32>, Vec<String>, Vec<u16>);

/// Run the production ESA first-round invoice decision using the custodian's records.
#[pyfunction]
pub fn esa_invoice_verdict(
    invoice: &str,
    receipt: &str,
    known_order: bool,
    duplicate_invoice: bool,
    prices: Vec<(String, String)>,
    services: Vec<(u16, bool, Option<String>)>,
) -> PyResult<InvoiceVerdict> {
    use invoic_checker::{CheckConfig, EmpfaengerFakten, antwort_auf_rechnung};
    let r: rubo4e::current::Rechnung =
        serde_json::from_str(invoice).map_err(|_| PyValueError::new_err("invalid BO4E invoice"))?;
    let mut facts = EmpfaengerFakten::neu(mako_fristen::berlin_date(crate::fristen::parse_dt(
        receipt,
    )?));
    facts.bestellung_bekannt = known_order;
    facts.rechnungsnummer_bereits_verwendet = duplicate_invoice;
    for (n, performed, prior) in services {
        facts.leistung_erbracht.insert(n, performed);
        if let Some(number) = prior {
            facts.bereits_abgerechnet_in.insert(n, number);
        }
    }
    let mut agreed = Vec::with_capacity(prices.len());
    for (article, price) in prices {
        let amount = rust_decimal::Decimal::from_str_exact(&price)
            .ok()
            .and_then(invoic_checker::amount::euro_from_decimal)
            .ok_or_else(|| PyValueError::new_err("invalid agreed price"))?;
        agreed.push((article, amount));
    }
    let answer = antwort_auf_rechnung(
        mako_pruefung::rechnung::ESA,
        &r,
        &agreed,
        &facts,
        &CheckConfig::default(),
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    Ok((
        answer.remadv_pid(),
        answer
            .antwortcodes()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        answer.offen.iter().map(|s| s.pruefschritt).collect(),
    ))
}
