//! Conformance checks against the DVGW Nachrichtenbeschreibungen.
//!
//! The four families share a header and differ in a handful of rows, so the
//! rules are one table plus per-family admitted values rather than one
//! hand-written pack each. Every rule below cites the row of the
//! Nachrichtenstruktur or Segmentlayout it enforces.
//!
//! ## Rules
//!
//! | Rule id | Applies to | Severity | Row it enforces |
//! |---|---|---|---|
//! | `DVGW-BGM-AGENCY` | all | Warning | `BGM` C002 DE 3055 = `332` |
//! | `DVGW-BGM-DOCNO` | all | Error | `BGM` C106 DE 1004 Dokumentennummer, `R` |
//! | `DVGW-DTM-Z05` | all | Error | Zeitzone, `M` |
//! | `DVGW-DTM-137` | all | Error | Datum und Zeit der Nachricht, `M` |
//! | `DVGW-DTM-Z01` | all | Error | Gültigkeitszeitraum der Nachricht, `M` |
//! | `DVGW-DTM-UNDECODABLE` | all | Error | value contradicts its own DE 2379 format |
//! | `DVGW-PERIOD-INVERTED` | all | Error | a period must run forwards |
//! | `DVGW-RFF-Z13` | all | Error | `SG1 RFF+Z13` Prüfidentifikator, `R` |
//! | `DVGW-RFF-Z13-RANGE` | all | Error | the code must be in `70000–79999` |
//! | `DVGW-PID-FAMILY` | all | Warning | the `RFF+Z13` code is published for this family |
//! | `DVGW-PID-DOCUMENT` | all | Error | `BGM` DE 1001 is the code the Anwendungsfall publishes |
//! | `DVGW-PID-RETIRED` | SSQNOT | Warning | 70096 / `STS+A2G` only for Zeiträume before 1.10.2015 (Hinweise \[500\]/[501]) |
//! | `DVGW-RFF-ANX` | ALOCAT Clearing | Error | `SG1 RFF+ANX` Clearingnummer — the `D` group the six Clearing columns (70008–70010, 70018–70020) mark `Muss` |
//! | `DVGW-RFF-AGO-DTM` | NOMINT | Error | `SG1 DTM+9` is `R` beside `RFF+AGO` |
//! | `DVGW-NAD-MS` / `DVGW-NAD-MR` | all | Error | Absender / Empfänger, `M` |
//! | `DVGW-LIN-REQUIRED` | all | Error | at least one Positionsnummer, `R` |
//! | `DVGW-LOC-REQUIRED` | all | Error | every position carries a `LOC` group, `R` |
//! | `DVGW-LOC-QUALIFIER` | all | Warning | `LOC` DE 3227 is one the Segmentlayout lists |
//! | `DVGW-QTY-REQUIRED` | all | Error | every `LOC` group carries a Menge, `M` |
//! | `DVGW-QTY-NUMERIC` | all | Error | C186 DE 6060 must be a number |
//! | `DVGW-QTY-QUALIFIER` | all | Warning | C186 DE 6063 is one the Segmentlayout lists |
//! | `DVGW-QTY-UNIT` | all | Warning | C186 DE 6411 is one the Segmentlayout lists |
//! | `DVGW-QTY-INTEGER` | SSQNOT | Warning | „nur natürliche Zahlen (einschließlich Null)" |
//! | `DVGW-DTM-2-REQUIRED` | all | Error | every Menge is preceded by its period, `R` |
//! | `DVGW-LOC-MAX` | all | Warning | the DVGW column caps `DTM+2` and `SG37 QTY` at one per `LOC` group |
//! | `DVGW-STS-REQUIRED` | SSQNOT | Error | `SG37 STS` Verfahren, `Muss` |
//! | `DVGW-STS-CODE` | SSQNOT | Warning | `STS` DE 9015 is `A1G` or `A2G` |
//! | `DVGW-NAD-ITEM` | all | Error | the position-level `NAD` rows the family marks `R` (ALOCAT: both; NOMINT/NOMRES: `ZEU`; SSQNOT: `ZSH`) |
//! | `DVGW-IMD-REQUIRED` | NOMRES | Warning | `IMD` labels which side a position reports |

use crate::{
    document::{DVGW_AGENCY_CODE, DvgwDocument, DvgwMessageType},
    message::DvgwMessage,
    model::{LineItem, LocationGroup, imd, nad, rff, sts},
    pruefidentifikator::{Pruefidentifikator, SSQNOT_RLM_CUTOFF},
    report::{DvgwIssue, Severity},
    zuordnung::Zuordnung,
};

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
    check_pruefidentifikator(m, out);
    check_family_references(m, out);
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

/// `SG1 RFF+Z13` — the Prüfidentifikator, and what the Anwendungsfall it names
/// fixes: the document code, the family, and a retired Anwendungsfall.
fn check_pruefidentifikator(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
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
    // The Anwendungsfall fixes the document: every published column marks one
    // `BGM` DE 1001 code (ALOCAT 5.11a §4 and the other §4 tables). A message
    // whose code belongs to a different column of the same family is
    // family-consistent and still the wrong business message — an endgültige
    // Allokation filed as the SLP one, say.
    if let Some((pid, expected)) = m
        .pruefidentifikator
        .and_then(|pid| DvgwDocument::for_pid(pid.as_u32()).map(|doc| (pid, doc)))
        .filter(|(_, expected)| *expected != m.document)
    {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                format!(
                    "BGM DE 1001 is {} ({}) but Prüfidentifikator {pid} publishes {} ({})",
                    m.document.code(),
                    m.document.description(),
                    expected.code(),
                    expected.description()
                ),
            )
            .with_rule("DVGW-PID-DOCUMENT")
            .with_segment("BGM")
            .with_suggestion(format!(
                "write BGM+{}::{DVGW_AGENCY_CODE}+<Dokumentennummer>'",
                expected.code()
            )),
        );
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
}

/// The per-family reference rows: the Clearingnummer an Allokationsclearing
/// assigns by, and the date beside a re-nomination's back-reference.
fn check_family_references(m: &DvgwMessage, out: &mut Vec<DvgwIssue>) {
    // `SG1 RFF+ANX` is a `D` group: ALOCAT 5.11a §4 marks it `Muss` in the six
    // Clearing columns only, and they are the ones §3.3 assigns `ZG-T1`.
    let is_clearing = m
        .pruefidentifikator
        .and_then(Zuordnung::for_pid)
        .is_some_and(Zuordnung::assigns_to_geschaeftsvorfall);
    if is_clearing && m.clearingnummer().is_none() {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                "RFF+ANX (Clearingnummer) is missing — the Allokationsclearing columns \
                 mark it Muss and assign the message by it (ZG-T1)",
            )
            .with_rule("DVGW-RFF-ANX")
            .with_segment("RFF")
            .with_suggestion("add RFF+ANX:<clearing number>'"),
        );
    }
    // NOMINT 4.6 §2: the `SG1` RFF-DTM group is `D`, its `DTM+9` `R` — a
    // re-nomination names the original and when it was processed.
    if m.message_type == DvgwMessageType::Nomint
        && m.original_nomination_ref().is_some()
        && m.original_nomination_datetime.is_none()
    {
        out.push(
            DvgwIssue::new(
                Severity::Error,
                "RFF+AGO names the original nomination but its DTM+9 \
                 (Bearbeitungsdatum/-zeit) is missing — NOMINT marks it Erforderlich",
            )
            .with_rule("DVGW-RFF-AGO-DTM")
            .with_segment("DTM")
            .with_suggestion("add DTM+9:<CCYYMMDDHHMM>:203' after RFF+AGO"),
        );
    }
    // SSQNOT 5.7 §4 Hinweis [500]: 70096 only for Zeiträume before 1.10.2015.
    if m.pruefidentifikator.map(Pruefidentifikator::as_u32) == Some(70_096)
        && m.validity_period
            .is_some_and(|p| p.start.date() >= SSQNOT_RLM_CUTOFF)
    {
        out.push(
            DvgwIssue::new(
                Severity::Warning,
                format!(
                    "Prüfidentifikator 70096 (Mehr-/Mindermengenmeldung RLM) is admitted \
                     only for Zeiträume before {SSQNOT_RLM_CUTOFF} (SSQNOT 5.7 Hinweis [500])"
                ),
            )
            .with_rule("DVGW-PID-RETIRED")
            .with_segment("RFF"),
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

        // The position-level `NAD` rows the family marks `R` (Nachrichtenstruktur
        // §2 and every §4 column): ALOCAT lists both — (`ZEU`|`ZET`) and
        // (`ZSH`|`ZSO`|`ZSZ`|`VHP`) — NOMINT/NOMRES the interne Bilanzkreis with
        // the externe one `D`, SSQNOT the Netzkontonummer alone.
        let required: &[&[&str]] = match m.message_type {
            DvgwMessageType::Alocat => &[
                &[nad::BILANZKREIS_INTERN, nad::VORGELAGERTER_NETZBETREIBER],
                &[
                    nad::NETZKONTO_ZO_T3,
                    nad::NETZBETREIBER,
                    nad::NETZKONTO,
                    nad::VIRTUELLER_HANDELSPUNKT,
                ],
            ],
            DvgwMessageType::Nomint | DvgwMessageType::Nomres => &[&[nad::BILANZKREIS_INTERN]],
            DvgwMessageType::Ssqnot => &[&[nad::NETZKONTO_ZO_T3]],
        };
        for roles in required {
            if !item
                .parties
                .iter()
                .any(|p| roles.contains(&p.role.as_str()))
            {
                out.push(
                    DvgwIssue::new(
                        Severity::Error,
                        format!(
                            "position {position} carries no NAD+{} — {} marks the row \
                             Erforderlich",
                            roles.join("/"),
                            m.message_type
                        ),
                    )
                    .with_rule("DVGW-NAD-ITEM")
                    .with_segment("NAD"),
                );
            }
        }

        if item.locations.is_empty() {
            out.push(
                DvgwIssue::new(
                    Severity::Error,
                    format!("position {position} carries no LOC group"),
                )
                .with_rule("DVGW-LOC-REQUIRED")
                .with_segment("LOC"),
            );
        }

        for location in &item.locations {
            check_location(m, &position, location, out);
        }
    }
}

fn check_location(
    m: &DvgwMessage,
    position: &str,
    location: &LocationGroup,
    out: &mut Vec<DvgwIssue>,
) {
    let family = m.message_type;
    {
        {
            if !family
                .admitted_location_qualifiers()
                .contains(&location.qualifier.as_str())
            {
                out.push(
                    DvgwIssue::new(
                        Severity::Warning,
                        format!(
                            "position {position}: LOC+{} is not a qualifier the {family} \
                             Segmentlayout lists ({})",
                            location.qualifier,
                            family.admitted_location_qualifiers().join(", ")
                        ),
                    )
                    .with_rule("DVGW-LOC-QUALIFIER")
                    .with_segment("LOC"),
                );
            }
            // The DVGW column caps `DTM+2` and `SG37 QTY` at one per `LOC`; a
            // profile is a run of `LOC` groups.
            if location.quantities.len() > 1 {
                out.push(
                    DvgwIssue::new(
                        Severity::Warning,
                        format!(
                            "position {position}, LOC+{} carries {} QTY — the DVGW column \
                             admits one Menge per LOC group; repeat the LOC for a profile",
                            location.qualifier,
                            location.quantities.len()
                        ),
                    )
                    .with_rule("DVGW-LOC-MAX")
                    .with_segment("QTY"),
                );
            }
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
                check_quantity(m, position, quantity, out);
            }
        }
    }
}

/// One `SG37 QTY` with the `DTM+2` before it and the `STS` after it.
fn check_quantity(
    m: &DvgwMessage,
    position: &str,
    quantity: &crate::model::Quantity,
    out: &mut Vec<DvgwIssue>,
) {
    let family = m.message_type;
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
    if !family
        .admitted_quantity_qualifiers()
        .contains(&quantity.qualifier.as_str())
    {
        out.push(
            DvgwIssue::new(
                Severity::Warning,
                format!(
                    "position {position}: QTY+{} is not a qualifier the {family} \
                                 Segmentlayout lists ({})",
                    quantity.qualifier,
                    family.admitted_quantity_qualifiers().join(", ")
                ),
            )
            .with_rule("DVGW-QTY-QUALIFIER")
            .with_segment("QTY"),
        );
    }
    if !quantity
        .unit
        .as_deref()
        .is_some_and(|u| family.admitted_units().contains(&u))
    {
        out.push(
            DvgwIssue::new(
                Severity::Warning,
                format!(
                    "position {position}: QTY+{} is measured in {} — the {family} \
                                 Segmentlayout admits {}",
                    quantity.qualifier,
                    quantity
                        .unit
                        .as_deref()
                        .map_or_else(|| "no unit".to_owned(), |u| format!("{u:?}")),
                    family.admitted_units().join(", ")
                ),
            )
            .with_rule("DVGW-QTY-UNIT")
            .with_segment("QTY"),
        );
    }
    if family == DvgwMessageType::Ssqnot {
        check_ssqnot_quantity(m, position, quantity, out);
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

/// SSQNOT 5.7 §3.2/§4: a Mehr-/Mindermenge is a natural number in kWh and
/// carries its Verfahren in `SG37 STS` (`Muss`).
fn check_ssqnot_quantity(
    m: &DvgwMessage,
    position: &str,
    quantity: &crate::model::Quantity,
    out: &mut Vec<DvgwIssue>,
) {
    if quantity
        .value
        .is_some_and(|v| v.is_sign_negative() || v.normalize().scale() != 0)
    {
        out.push(
            DvgwIssue::new(
                Severity::Warning,
                format!(
                    "position {position}: QTY+{} carries {:?} — SSQNOT transmits only \
                     natürliche Zahlen (einschließlich Null) in kWh",
                    quantity.qualifier, quantity.raw_value
                ),
            )
            .with_rule("DVGW-QTY-INTEGER")
            .with_segment("QTY"),
        );
    }
    match quantity.status_code() {
        None => out.push(
            DvgwIssue::new(
                Severity::Error,
                format!(
                    "position {position}: QTY+{} carries no STS — SSQNOT marks the \
                     Verfahren ({} SLP / {} RLM) Muss",
                    quantity.qualifier,
                    sts::SLP,
                    sts::RLM
                ),
            )
            .with_rule("DVGW-STS-REQUIRED")
            .with_segment("STS")
            .with_suggestion(format!(
                "add STS+{}::{DVGW_AGENCY_CODE}' after the QTY",
                sts::SLP
            )),
        ),
        Some(code) if code != sts::SLP && code != sts::RLM => out.push(
            DvgwIssue::new(
                Severity::Warning,
                format!(
                    "position {position}: STS+{code} is not a Verfahren the SSQNOT \
                     Segmentlayout lists ({} SLP, {} RLM)",
                    sts::SLP,
                    sts::RLM
                ),
            )
            .with_rule("DVGW-STS-CODE")
            .with_segment("STS"),
        ),
        Some(code)
            if code == sts::RLM
                && m.validity_period
                    .is_some_and(|p| p.start.date() >= SSQNOT_RLM_CUTOFF) =>
        {
            out.push(
                DvgwIssue::new(
                    Severity::Warning,
                    format!(
                        "position {position}: STS+{} (RLM) is admitted only for Zeiträume \
                         before {SSQNOT_RLM_CUTOFF} (SSQNOT 5.7 Hinweis [501])",
                        sts::RLM
                    ),
                )
                .with_rule("DVGW-PID-RETIRED")
                .with_segment("STS"),
            );
        }
        Some(_) => {}
    }
}
