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
//! - Constructing a message that triggers a deliberate warning
//!
//! ## Run
//!
//! ```text
//! cargo run --example 05_validate
//! ```

use edi_energy::{EdiEnergyMessage, Platform, ValidationSeverity};

// ── Fixtures ─────────────────────────────────────────────────────────────────

const VALID_UTILMD: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240115:0800+INTER-V-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20240115:102'\
RFF+Z13:REF-2024-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::\'
UNT+8+MSG-001'\
UNZ+1+INTER-V-001'";

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
    let msg = Platform::with_all_profiles().parse(VALID_UTILMD)?;
    let report = msg.validate()?;

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

    // Filter by rule-id prefix (e.g. "BGM" rules only)
    let bgm_report = report.filter_by_rule_prefix("BGM");
    println!(
        "\nBGM-prefixed rules: {} issue(s)",
        bgm_report.total_issues()
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
