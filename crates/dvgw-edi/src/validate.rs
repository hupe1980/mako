//! Conformance checks against the DVGW Nachrichtenbeschreibungen.
//!
//! The three families share a header and differ in a handful of rows, so the
//! rules are one table plus a per-family delta rather than one hand-written pack
//! each. Every rule below cites the row of the Segmentlayout it enforces.
//!
//! ## Rules
//!
//! | Rule id | Applies to | Severity | Row it enforces |
//! |---|---|---|---|
//! | `DVGW-BGM-AGENCY` | all | Warning | `BGM` C002 DE 3055 = `332` |
//! | `DVGW-BGM-DOCNO` | all | Error | `BGM` C106 DE 1004 Dokumentennummer, `Muss` |
//! | `DVGW-DTM-Z05` | all | Error | Zeitzone, `Muss` |
//! | `DVGW-DTM-137` | all | Error | Datum und Zeit der Nachricht, `Muss` |
//! | `DVGW-DTM-Z01` | all | Error | Gültigkeitszeitraum der Nachricht, `Muss` |
//! | `DVGW-DTM-UNDECODABLE` | all | Error | value contradicts its own DE 2379 format |
//! | `DVGW-PERIOD-INVERTED` | all | Error | a period must run forwards |
//! | `DVGW-RFF-Z13` | all | Error | `SG1 RFF+Z13` Prüfidentifikator, `Muss` |
//! | `DVGW-RFF-Z13-RANGE` | all | Error | the code must be in `70000–79999` |
//! | `DVGW-NAD-MS` / `DVGW-NAD-MR` | all | Error | Absender / Empfänger, `Muss` |
//! | `DVGW-LIN-REQUIRED` | all | Error | at least one Positionsnummer, `Muss` |
//! | `DVGW-QTY-REQUIRED` | all | Error | every `LOC` group carries a Menge, `Muss` |
//! | `DVGW-QTY-NUMERIC` | all | Error | C186 DE 6060 must be a number |
//! | `DVGW-QTY-UNIT` | all | Warning | C186 DE 6411 is `KW1` |
//! | `DVGW-DTM-2-REQUIRED` | all | Error | every Menge is preceded by its period, `R` |
//! | `DVGW-RFF-ANX` | ALOCAT | Error | `RFF+ANX` Clearingnummer, `Muss` |
//! | `DVGW-NAD-ITEM-PAIR` | ALOCAT, NOMINT, NOMRES | Error | two position-level `NAD`, `Muss` |
//! | `DVGW-IMD-REQUIRED` | NOMRES | Warning | `IMD` labels which side a position reports |
//! | `DVGW-PID-FAMILY` | all | Warning | the `RFF+Z13` code belongs to this family |

use crate::{
    document::{DVGW_AGENCY_CODE, DvgwMessageType},
    message::DvgwMessage,
    model::{LineItem, LocationGroup, imd, nad, rff},
    report::{DvgwIssue, Severity},
};

/// The unit every DVGW quantity is expressed in — kWh/h.
const UNIT_KWH_PER_HOUR: &str = "KW1";

/// Run every applicable rule and return the findings in rule order.
#[must_use]
pub(crate) fn check(message: &DvgwMessage) -> Vec<DvgwIssue> {
    let mut issues = Vec::new();
    check_header(message, &mut issues);
    check_positions(message, &mut issues);
    issues
}

fn check_header(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    check_bgm(m, out);
    check_dates(m, out);
    check_references(m, out);
    check_parties(m, out);
}

/// `BGM` — the segment that says which message this is.
fn check_bgm(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    let bgm_agency = m
        .segments()
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(0, 2));
    if bgm_agency != Some(DVGW_AGENCY_CODE) {
        out.push(
            DvgwIssue::new(
                Severity::Warning,
                format!(
                    "BGM C002 DE 3055 is {} — DVGW document-name codes are maintained under {DVGW_AGENCY_CODE}",
                    bgm_agency.map_or_else(|| "absent".to_owned(), |a| format!("{a:?}"))
                ),
            )
            .with_rule("DVGW-BGM-AGENCY")
            .with_segment("BGM"),
        );
    }
    if m.document_number.as_deref().unwrap_or_default().is_empty() {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                "BGM C106 DE 1004 Dokumentennummer is missing — the sender must supply a \
                 unique document identification",
            )
            .with_rule("DVGW-BGM-DOCNO")
            .with_segment("BGM")
            .with_suggestion(format!(
                "render BGM+{}::332+{}<unique-id>'",
                m.document.code(),
                m.message_type.as_str()
            )),
        );
    }
}

/// The three mandatory header `DTM` rows, and whether their values match the
/// format code each one declares.
fn check_dates(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    let has_dtm = |qualifier: &str| {
        m.segments()
            .iter()
            .take_while(|s| s.tag != "LIN")
            .any(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
    };
    for (qualifier, rule, name, format) in [
        ("Z05", "DVGW-DTM-Z05", "Zeitzone", "805"),
        ("137", "DVGW-DTM-137", "Datum und Zeit der Nachricht", "203"),
        (
            "Z01",
            "DVGW-DTM-Z01",
            "Gültigkeitszeitraum der Nachricht",
            "719",
        ),
    ] {
        if !has_dtm(qualifier) {
            out.push(
                DvgwIssue::new(
                    Severity::Error,
                    format!("DTM+{qualifier} ({name}) is missing"),
                )
                .with_rule(rule)
                .with_segment("DTM")
                .with_suggestion(format!("add DTM+{qualifier}:<value>:{format}'")),
            );
        }
    }
    for qualifier in &m.undecodable_dtm {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                format!(
                    "DTM+{qualifier} carries a value that does not match the format code in \
                     C507 DE 2379"
                ),
            )
            .with_rule("DVGW-DTM-UNDECODABLE")
            .with_segment("DTM")
            .with_suggestion(
                "DVGW uses 102 (CCYYMMDD), 203 (CCYYMMDDHHMM), 719 \
                 (CCYYMMDDHHMMCCYYMMDDHHMM) and 805 (whole hours)",
            ),
        );
    }
    if let Some(period) = m.validity_period.filter(|p| !p.is_forward()) {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                format!("DTM+Z01 Gültigkeitszeitraum does not run forwards: {period}"),
            )
            .with_rule("DVGW-PERIOD-INVERTED")
            .with_segment("DTM"),
        );
    }
}

/// `SG1 RFF+Z13` and the per-family reference rows.
fn check_references(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    match m.reference(rff::PRUEFIDENTIFIKATOR) {
        None => out.push(
            DvgwIssue::new(
                Severity::Error,
                "SG1 RFF+Z13 (Prüfidentifikator) is missing — DVGW requires it in every message",
            )
            .with_rule("DVGW-RFF-Z13")
            .with_segment("RFF")
            .with_suggestion("add RFF+Z13:<70000-79999>'"),
        ),
        Some(raw) if m.pruefidentifikator.is_none() => out.push(
            DvgwIssue::new(
                Severity::Error,
                format!(
                    "RFF+Z13 carries {raw:?}, which is not a DVGW Prüfidentifikator \
                     (70000–79999)"
                ),
            )
            .with_rule("DVGW-RFF-Z13-RANGE")
            .with_segment("RFF"),
        ),
        Some(_) => {}
    }
    if let Some((pid, info)) = m
        .pruefidentifikator
        .and_then(|pid| pid.info().map(|info| (pid, info)))
        .filter(|(_, info)| info.message_type != m.message_type)
    {
        {
            out.push(
                DvgwIssue::new(
                    Severity::Warning,
                    format!(
                        "Prüfidentifikator {pid} is published for {} but this message is {} ({})",
                        info.message_type,
                        m.message_type,
                        m.document.code()
                    ),
                )
                .with_rule("DVGW-PID-FAMILY")
                .with_segment("RFF"),
            );
        }
    }
    if m.message_type == DvgwMessageType::Alocat && m.clearingnummer().is_none() {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                "RFF+ANX (Clearingnummer) is missing — ALOCAT marks it Muss",
            )
            .with_rule("DVGW-RFF-ANX")
            .with_segment("RFF")
            .with_suggestion("add RFF+ANX:<clearing number>'"),
        );
    }
}

/// `NAD+MS` and `NAD+MR`.
fn check_parties(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    for (role, rule, name) in [
        (nad::ABSENDER, "DVGW-NAD-MS", "Absender der Nachricht"),
        (nad::EMPFAENGER, "DVGW-NAD-MR", "Empfänger der Nachricht"),
    ] {
        if m.party(role).is_none_or(|p| p.id.is_empty()) {
            out.push(
                DvgwIssue::new(Severity::Error, format!("NAD+{role} ({name}) is missing"))
                    .with_rule(rule)
                    .with_segment("NAD")
                    .with_suggestion(format!("add NAD+{role}+<code>::{DVGW_AGENCY_CODE}'")),
            );
        }
    }
}

fn check_positions(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    if m.items.is_empty() {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                "the message carries no LIN position — every DVGW message needs at least one",
            )
            .with_rule("DVGW-LIN-REQUIRED")
            .with_segment("LIN"),
        );
        return;
    }

    for (index, item) in m.items.iter().enumerate() {
        check_position(m, item, index, out);
    }
}

fn check_position(m: &DvgwMessage, item: &LineItem, index: usize, out: &mut Vec<DvgwIssue>) {
    {
        let position = item
            .number
            .clone()
            .unwrap_or_else(|| (index + 1).to_string());

        if m.message_type == DvgwMessageType::Nomres && item.description_code().is_none() {
            out.push(
                DvgwIssue::new(
                    Severity::Warning,
                    format!(
                        "position {position} has no IMD — without it the quantity cannot be \
                         told apart from the counterparty's ({}/{}/{})",
                        imd::NOMINIERT,
                        imd::GEGENSEITE,
                        imd::GEMATCHT
                    ),
                )
                .with_rule("DVGW-IMD-REQUIRED")
                .with_segment("IMD"),
            );
        }

        if item.parties.len() < 2 {
            out.push(
                DvgwIssue::new(
                    Severity::Error,
                    format!(
                        "position {position} carries {} position-level NAD segment(s) — DVGW \
                         requires two (e.g. NAD+{} and NAD+{})",
                        item.parties.len(),
                        nad::BILANZKREIS_INTERN,
                        nad::BILANZKREIS_EXTERN
                    ),
                )
                .with_rule("DVGW-NAD-ITEM-PAIR")
                .with_segment("NAD"),
            );
        }

        if item.locations.is_empty() {
            out.push(
                DvgwIssue::new(
                    Severity::Error,
                    format!("position {position} carries no LOC group"),
                )
                .with_rule("DVGW-QTY-REQUIRED")
                .with_segment("LOC"),
            );
        }

        for location in &item.locations {
            check_location(&position, location, out);
        }
    }
}

fn check_location(position: &str, location: &LocationGroup, out: &mut Vec<DvgwIssue>) {
    {
        {
            if location.quantities.is_empty() {
                out.push(
                    DvgwIssue::new(
                        Severity::Error,
                        format!(
                            "position {position}, LOC+{} carries no QTY — the Menge is Muss",
                            location.qualifier
                        ),
                    )
                    .with_rule("DVGW-QTY-REQUIRED")
                    .with_segment("QTY"),
                );
            }
            for quantity in &location.quantities {
                if quantity.period.is_none() {
                    out.push(
                        DvgwIssue::new(
                            Severity::Error,
                            format!(
                                "position {position}: QTY+{} has no DTM+2 period — a Menge is \
                                 a rate in kWh/h and means nothing without the period it \
                                 applies to",
                                quantity.qualifier
                            ),
                        )
                        .with_rule("DVGW-DTM-2-REQUIRED")
                        .with_segment("DTM")
                        .with_suggestion(
                            "add DTM+2:<CCYYMMDDHHMMCCYYMMDDHHMM>:719' before the QTY; \
                             the Segmentlayout marks it Erforderlich",
                        ),
                    );
                }
                if quantity.value.is_none() {
                    out.push(
                        DvgwIssue::new(
                            Severity::Error,
                            format!(
                                "position {position}: QTY+{} carries {:?}, which is not a number",
                                quantity.qualifier, quantity.raw_value
                            ),
                        )
                        .with_rule("DVGW-QTY-NUMERIC")
                        .with_segment("QTY"),
                    );
                }
                if quantity.unit.as_deref() != Some(UNIT_KWH_PER_HOUR) {
                    out.push(
                        DvgwIssue::new(
                            Severity::Warning,
                            format!(
                                "position {position}: QTY+{} is measured in {} — DVGW quantities \
                                 are {UNIT_KWH_PER_HOUR} (kWh/h)",
                                quantity.qualifier,
                                quantity
                                    .unit
                                    .as_deref()
                                    .map_or_else(|| "no unit".to_owned(), |u| format!("{u:?}"))
                            ),
                        )
                        .with_rule("DVGW-QTY-UNIT")
                        .with_segment("QTY"),
                    );
                }
                if let Some(period) = quantity
                    .period
                    .filter(|p: &crate::DvgwPeriod| !p.is_forward())
                {
                    {
                        out.push(
                            DvgwIssue::new(
                                Severity::Error,
                                format!(
                                    "position {position}: the DTM+2 period does not run \
                                     forwards: {period}"
                                ),
                            )
                            .with_rule("DVGW-PERIOD-INVERTED")
                            .with_segment("DTM"),
                        );
                    }
                }
            }
        }
    }
}
