//! # Example: Validate a message against EDI@Energy profiles
//!
//! Every EDI@Energy message type has an associated AHB (Anwendungshandbuch)
//! profile that defines mandatory/conditional segment rules.  After parsing,
//! call [`EdiEnergyMessage::validate`] to run the full rule-set.
//!
//! This example shows:
//!
//! - Parsing a valid UTILMD and checking the report
//! - Filtering findings by severity and rule-id
//! - Using `filter_by_rule_prefix` for AHB-section scoping
//! - Using `into_result()` to turn the report into a `Result`
//! - Reporting on a message with deliberate defects
//!
//! ## Run
//!
//! ```text
//! cargo run --example 05_validate
//! ```

use edi_energy::{EdiEnergyMessage, Platform, ValidationSeverity};

// ── Fixtures ─────────────────────────────────────────────────────────────────

const VALID_UTILMD: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:500+240115:0800+INTER-V-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01+MSG-001'\
DTM+137:202401150800?+00:303'\
NAD+MS+4012345000023::9'\
NAD+MR+9900357000004::293'\
IDE+24+VORGANG-0001'\
DTM+92:202402010000?+00:303'\
STS+7++E01+ZW4'\
LOC+Z16+51238696781'\
RFF+Z13:55001'\
SEQ+Z79+1'\
PIA+5+9991000002082:Z11'\
CCI+Z66'\
SEQ+ZH0+1'\
CCI+Z65+++Z01'\
SEQ+Z01'\
CCI+++Z15'\
SEQ+Z75'\
CCI+Z61++ZF9'\
CAV+ZU5'\
NAD+Z09+++Mustermann:::::Z01'\
NAD+Z04+++Mustermann:::::Z01+Musterstr. 1+Berlin+++DE'\
UNT+23+MSG-001'\
UNZ+1+INTER-V-001'";

/// The same Anmeldung with three deliberate defects, so the report API below has
/// something to filter and render. The rule ids each one fires are named,
/// because the assertions below check for them and a defect the report does not
/// reach is a defect this example only claims to demonstrate:
///
/// - `IDE+Z19` — `Z19` is not an IDE qualifier; the AHB admits only `24`.
///   (`MIG-00012-IDE-7495-CODE`, `AHB-55001-00012-IDE-NOT-PERMITTED`)
/// - `DTM+137:…:102` — the Dokumentendatum's DE 2379 format code. Every
///   EDI@Energy MIG fixes it to `303` (`CCYYMMDDHHMMZZZ`).
///   (`MIG-00005-DTM-REQUIRED`, `AHB-55001-00005-DTM-MISSING`)
/// - everything after `LOC+Z16` is gone — the `DTM+92` Lieferbeginn, the
///   `STS+7` Transaktionsgrund and the whole Produktpaket — so the Vorgang
///   group is incomplete. The report names that as **`SG4`**, the group the
///   `IDE` opens, not as the `SG8` the Bilanzkreis would have sat in: a
///   Produktpaket cannot be missing from a Vorgang the validator never got to
///   enter. (`AHB-55001-SG4-00020-MISSING`)
const INVALID_UTILMD: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:500+240115:0800+INTER-I-001'\
UNH+MSG-002+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20240115:102'\
RFF+Z13:REF-2024-002'\
NAD+MS+4012345000023::9'\
NAD+MR+9900357000004::293'\
IDE+Z19+VORGANG-0002'\
LOC+Z16+51238696781'\
UNT+9+MSG-002'\
UNZ+1+INTER-I-001'";

/// Declares that this example prints validation findings on its happy path.
///
/// `just examples` scans an example's output for findings, because an example
/// that reports „FAILED" and exits `0` passes a run gate while shipping a
/// message no counterparty accepts. This one's *subject* is an invalid
/// message — showing what a rejection looks like is the point — so it opts out
/// of that scan by name. The gate greps for this symbol; nothing else grants
/// the exemption, and the valid half above still asserts.
const _EXAMPLE_EXPECTS_VALIDATION_FINDINGS: () = ();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Valid UTILMD ===\n");
    check_valid()?;

    println!("\n=== Validation API demo ===\n");
    demo_report_api()?;

    Ok(())
}

fn check_valid() -> Result<(), Box<dyn std::error::Error>> {
    let msg = Platform::with_all_profiles().parse(VALID_UTILMD)?;

    println!(
        "Type    : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    println!("PID     : {}", msg.detect_pruefidentifikator()?.as_u32());

    let report = msg.validate()?;

    // High-level status
    println!("Valid   : {}", report.is_valid());
    println!("Report  : {report}");

    if report.has_errors() {
        println!("\nErrors:");
        for e in report.errors() {
            let seg = e.segment_tag.as_deref().unwrap_or("-");
            let code = e.rule_id.as_deref().unwrap_or("-");
            println!("  [{seg}] [{code}] {}", e.message);
            if let Some(hint) = &e.suggestion {
                println!("    hint: {hint}");
            }
        }
    }

    if report.has_warnings() {
        println!("\nWarnings:");
        for w in report.warnings() {
            let seg = w.segment_tag.as_deref().unwrap_or("-");
            let code = w.rule_id.as_deref().unwrap_or("-");
            println!("  [{seg}] [{code}] {}", w.message);
        }
    }

    // `into_result()` converts the report to Ok(())/Err based on error presence
    report
        .into_result()
        .map_err(|r| format!("validation failed: {r}").into())
}

fn demo_report_api() -> Result<(), Box<dyn std::error::Error>> {
    let msg = Platform::with_all_profiles().parse(INVALID_UTILMD)?;
    let report = msg.validate()?;

    // The subject of this half is a message with three named defects, so the
    // report has to carry them. Printing the count alone would let a validator
    // that found nothing render `Total issues: 0` and exit 0 — the example would
    // still be demonstrating the report API, against an empty report, and the
    // `_EXAMPLE_EXPECTS_VALIDATION_FINDINGS` exemption turns off the output scan
    // that would otherwise notice.
    assert!(
        report.has_errors(),
        "INVALID_UTILMD carries three deliberate defects and the report found no error: {report}"
    );
    let rules: Vec<&str> = report
        .iter_issues()
        .filter_map(|i| i.rule_id.as_deref())
        .collect();
    for defect in ["IDE", "DTM", "SG4"] {
        assert!(
            report
                .iter_issues()
                .any(|i| i.segment_tag.as_deref() == Some(defect)
                    || i.rule_id.as_deref().is_some_and(|r| r.contains(defect))
                    || i.message.contains(defect)),
            "no finding names the deliberate {defect} defect; rules fired: {rules:?}"
        );
    }

    // Total issue count across all severities
    println!("Total issues     : {}", report.total_issues());

    // Severity breakdown
    println!("  Errors         : {}", report.errors().len());
    println!("  Warnings       : {}", report.warnings().len());
    println!("  Infos          : {}", report.infos().len());

    // Iterate all issues in order (errors → warnings → infos)
    for issue in report.iter_issues() {
        let label = match issue.severity {
            ValidationSeverity::Error => "ERROR",
            ValidationSeverity::Warning => "WARN ",
            ValidationSeverity::Info => "INFO ",
            _ => "?????  ",
        };
        let seg = issue.segment_tag.as_deref().unwrap_or("-");
        let rule = issue.rule_id.as_deref().unwrap_or("-");
        println!("  [{label}] seg={seg} rule={rule}: {}", issue.message);
    }

    // Filter by rule-id prefix — "AHB" scopes the report to the
    // Anwendungshandbuch layer, dropping the MIG and semantic findings.
    let ahb_report = report.filter_by_rule_prefix("AHB");
    println!(
        "\nAHB-prefixed rules: {} issue(s)",
        ahb_report.total_issues()
    );

    // Deterministic text rendering for snapshots / logging
    let snapshot = report.render_deterministic();
    if snapshot.is_empty() {
        println!("Snapshot         : <no issues>");
    } else {
        println!("Snapshot         :\n{snapshot}");
    }

    Ok(())
}
