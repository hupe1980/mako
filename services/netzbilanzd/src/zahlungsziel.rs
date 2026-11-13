//! The Zahlungsziel (`SG8 DTM+265`) an invoice is issued with.
//!
//! Two sources, one judge. A request may state its own `due_date`; otherwise
//! the operator's [`zahlungsziel_werktage`](crate::config::NetzbilanzConfig::zahlungsziel_werktage)
//! counts from the Rechnungsdatum. Either way the rendered invoice is held to
//! the published bounds by the same `invoic-checker` run the recipient makes,
//! and [`refuse_unzulaessig`] turns a breach into a refusal before an invoice
//! number is spent:
//!
//! - the floor — „Das Zahlungsziel darf 10 WT nach Empfang der Rechnung nicht
//!   unterschreiten" (GPKE Teil 2 Kap. 3.3.2 Nr. 1, S. 90), on an invoice the
//!   recipient pays;
//! - the ceiling — at most 10 WT after the Rechnungsdatum (INVOIC AHB 1.0b
//!   `[25]`, S. 16/39; `E_0406` Prüfschritt 34, EBD 4.3 S. 108), on a
//!   Rückerstattung and on 31006/31008.

use invoic_checker::{CheckReport, FindingKind, InvoicCheckEngine};
use mako_fristen::{HolidayCalendar, vorlauf::RUECKERSTATTUNG_ZAHLUNGSZIEL_HOECHST_WT};
use mako_service::{ApiError, ApiResult};

/// The due date of one invoice.
///
/// `requested` wins when the request states one; [`refuse_unzulaessig`]
/// judges it once the invoice is rendered. Otherwise the configured term
/// counts `werktage` Werktage from `invoice_date` — capped at the published
/// ceiling where [`InvoicCheckEngine::zahlungsziel_hoechstfrist_gilt`] says
/// the invoice carries one, because a term the Netzbetreiber sets for what it
/// is owed does not lengthen what it owes.
///
/// # Errors
///
/// `422` naming `zahlungsziel_werktage` when neither the request nor the
/// configuration states a term — never a substituted one.
pub fn faellig(
    werktage: Option<u32>,
    invoice_date: time::Date,
    requested: Option<time::Date>,
    pid: u32,
    sparte: grid_billing::Sparte,
    rueckerstattung: bool,
    storno: bool,
) -> ApiResult<time::Date> {
    if let Some(d) = requested {
        return Ok(d);
    }
    let Some(n) = werktage else {
        return Err(ApiError::unprocessable(
            "the request states no due_date and zahlungsziel_werktage is not configured — the \
             Zahlungsziel is the Netzbetreiber's own term and is not guessed",
        )
        .with_code("zahlungsziel_unconfigured"));
    };
    let cal = HolidayCalendar::BdewMaKo;
    let term = mako_fristen::add_werktage(invoice_date, n, cal);
    if InvoicCheckEngine::zahlungsziel_hoechstfrist_gilt(
        pid,
        Some(crate::billing::empfaenger_sparte(sparte)),
        rueckerstattung,
        storno,
    ) {
        let ceiling =
            mako_fristen::add_werktage(invoice_date, RUECKERSTATTUNG_ZAHLUNGSZIEL_HOECHST_WT, cal);
        return Ok(term.min(ceiling));
    }
    Ok(term)
}

/// Refuse an invoice whose Zahlungsziel breaks a published bound.
///
/// Reads the issuer's own `invoic-checker` report, so the bound is the one the
/// recipient applies — for the same PIDs and the same sign of the fälliger
/// Betrag — rather than a second copy of it.
///
/// # Errors
///
/// `422` carrying each breach's sentence.
pub fn refuse_unzulaessig(report: &CheckReport) -> ApiResult<()> {
    let breaches: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| {
            f.is_dispute
                && matches!(
                    f.kind,
                    FindingKind::ZahlungszielUnterschritten
                        | FindingKind::ZahlungszielUeberschritten
                )
        })
        .map(|f| f.message.as_str())
        .collect();
    if breaches.is_empty() {
        return Ok(());
    }
    Err(ApiError::unprocessable(format!(
        "the Zahlungsziel breaks a published bound: {}",
        breaches.join(" ")
    ))
    .with_code("zahlungsziel_unzulaessig"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// Wednesday 01.07.2026; no holiday in the fortnight after it.
    const DATUM: time::Date = date!(2026 - 07 - 01);

    const STROM: grid_billing::Sparte = grid_billing::Sparte::Strom;
    const GAS: grid_billing::Sparte = grid_billing::Sparte::Gas;

    #[test]
    fn the_configured_term_counts_werktage_from_the_rechnungsdatum() {
        let d = faellig(Some(14), DATUM, None, 31_002, STROM, false, false).expect("configured");
        assert_eq!(d, date!(2026 - 07 - 21));
    }

    #[test]
    fn a_stated_due_date_wins_over_the_configured_term() {
        let d = faellig(
            Some(14),
            DATUM,
            Some(date!(2026 - 08 - 31)),
            31_002,
            STROM,
            false,
            false,
        )
        .expect("stated");
        assert_eq!(d, date!(2026 - 08 - 31));
    }

    #[test]
    fn without_a_term_the_due_date_is_refused_by_name() {
        let err = faellig(None, DATUM, None, 31_002, STROM, false, false).expect_err("no term");
        assert!(
            format!("{err:?}").contains("zahlungsziel_werktage"),
            "{err:?}"
        );
    }

    /// A refund falls due at the ceiling when the operator's term is longer,
    /// and at the term when it is shorter.
    #[test]
    fn a_refund_is_capped_at_the_published_ceiling() {
        let d = faellig(Some(14), DATUM, None, 31_002, STROM, true, false).expect("capped");
        assert_eq!(d, date!(2026 - 07 - 15));
        let d = faellig(Some(14), DATUM, None, 31_006, STROM, false, false).expect("31006 always");
        assert_eq!(d, date!(2026 - 07 - 15));
        let d = faellig(Some(14), DATUM, None, 31_002, STROM, true, true).expect("a Storno is not");
        assert_eq!(d, date!(2026 - 07 - 21));
    }

    /// A Rechnung dated [`DATUM`], due on `faellig`, owing `zu_zahlen` EUR.
    fn rechnung(faellig: time::Date, zu_zahlen: i64) -> rubo4e::current::Rechnung {
        rubo4e::current::Rechnung {
            rechnungsdatum: Some(DATUM.midnight().assume_utc()),
            faelligkeitsdatum: Some(faellig.midnight().assume_utc()),
            zu_zahlen: Some(rubo4e::current::Betrag {
                wert: Some(rust_decimal::Decimal::from(zu_zahlen)),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn report(pid: u32, faellig: time::Date, zu_zahlen: i64) -> CheckReport {
        report_in(pid, faellig, zu_zahlen, STROM)
    }

    fn report_in(
        pid: u32,
        faellig: time::Date,
        zu_zahlen: i64,
        sparte: grid_billing::Sparte,
    ) -> CheckReport {
        crate::billing::check(
            &rechnung(faellig, zu_zahlen),
            "9900357000004",
            pid,
            DATUM,
            sparte,
        )
    }

    /// INVOIC AHB 1.0b `[24]` „Wert muss mindestens 10 WT nach Wert aus
    /// DTM+137 DE2380 liegen" holds an MMM-Rechnung 31005 the recipient pays
    /// in either Sparte: nine Werktage after the Rechnungsdatum is refused.
    #[test]
    fn an_mmm_rechnung_below_the_ahb_floor_is_refused() {
        for sparte in [STROM, GAS] {
            refuse_unzulaessig(&report_in(31_005, date!(2026 - 07 - 14), 100, sparte))
                .expect_err("nine Werktage");
            refuse_unzulaessig(&report_in(31_005, date!(2026 - 07 - 15), 100, sparte))
                .expect("ten Werktage");
        }
    }

    /// The Gas-only rows reach an invoice netzbilanzd issues in Gas: a 31003
    /// Rückerstattung to a Gas recipient is capped, to a Strom one it is not.
    #[test]
    fn the_gas_rows_follow_the_invoice_sparte() {
        let d = faellig(Some(14), DATUM, None, 31_003, GAS, true, false).expect("Gas");
        assert_eq!(d, date!(2026 - 07 - 15));
        let d = faellig(Some(14), DATUM, None, 31_003, STROM, true, false).expect("Strom");
        assert_eq!(d, date!(2026 - 07 - 21));
    }

    /// A stated due date nine Werktage out falls short of „Das Zahlungsziel
    /// darf 10 WT nach Empfang der Rechnung nicht unterschreiten" (GPKE Teil 2
    /// Kap. 3.3.2 Nr. 1); ten is the floor itself.
    #[test]
    fn a_due_date_below_the_floor_is_refused() {
        let err = refuse_unzulaessig(&report(31_002, date!(2026 - 07 - 14), 100))
            .expect_err("nine Werktage");
        assert!(
            format!("{err:?}").contains("zahlungsziel_unzulaessig"),
            "{err:?}"
        );
        refuse_unzulaessig(&report(31_002, date!(2026 - 07 - 15), 100)).expect("ten Werktage");
    }

    /// A Rückerstattung due eleven Werktage after its Rechnungsdatum breaks the
    /// ceiling (INVOIC AHB 1.0b `[25]`; `E_0406` Prüfschritt 34).
    #[test]
    fn a_refund_beyond_the_ceiling_is_refused() {
        refuse_unzulaessig(&report(31_002, date!(2026 - 07 - 16), -100))
            .expect_err("eleven Werktage");
        refuse_unzulaessig(&report(31_002, date!(2026 - 07 - 15), -100)).expect("ten Werktage");
    }
}
