//! # Example: Parse a UTILMD message
//!
//! Demonstrates how to parse a UTILMD (Utilities Master Data) message from
//! raw EDIFACT bytes and inspect its typed fields.
//!
//! UTILMD is used for grid-connection processes: supplier switches,
//! registrations, cancellations, and meter installations.
//!
//! ## Run
//!
//! ```text
//! cargo run --example 01_parse_utilmd
//! ```

#![allow(clippy::result_large_err)]

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};

/// A minimal UTILMD 55001 "Lieferbeginn Strom" interchange.
///
/// UNB  — interchange header
/// UNH  — message header (UTILMD release S2.1, fv20241001 Strom)
/// BGM  — "E01" document type, Pruefidentifikator 55001
/// DTM  — document date 2024-01-15
/// RFF  — Z13 reference (SG1, mandatory for PID 55001)
/// NAD+MS — sender (market-participant ID + qualifier 293)
/// NAD+MR — receiver
/// IDE  — metering-point / process identifier (SG4, qualifier Z19)
/// UNT  — message trailer (8 segments)
/// UNZ  — interchange trailer
const UTILMD_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240115:0800+INTER-2024-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20240115:102'\
RFF+Z13:REF-2024-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::\'
UNT+8+MSG-001'\
UNZ+1+INTER-2024-001'";

fn main() -> Result<(), edi_energy::Error> {
    let msg = Platform::with_all_profiles().parse(UTILMD_BYTES)?;

    // ── Message-type routing ─────────────────────────────────────────────────
    println!(
        "Message type : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    println!("Release      : {}", msg.detect_release()?.as_str());

    // ── Pruefidentifikator ───────────────────────────────────────────────────
    let pid = msg.detect_pruefidentifikator()?;
    println!("PID          : {} (\"Lieferbeginn Strom\")", pid.as_u32());

    // ── Typed fields via AnyMessage downcast ─────────────────────────────────
    if let AnyMessage::Utilmd(utilmd) = &msg {
        // BGM — document code and Pruefidentifikator
        if let Some(bgm) = &utilmd.bgm() {
            println!("Doc code     : {}", bgm.document_code);
            println!(
                "Doc ID (PID) : {}",
                bgm.document_id.as_deref().unwrap_or("-")
            );
        }

        // DTM — document date
        for dtm in utilmd.dtm() {
            if dtm.is_document_date() {
                println!("Document date: {}", dtm.value_str().unwrap_or("-"));
            }
        }

        // Parties
        if let Some(sender) = &utilmd.sender() {
            println!(
                "Sender party : {}",
                sender.party_id.as_deref().unwrap_or("-")
            );
        }
        if let Some(receiver) = &utilmd.receiver() {
            println!(
                "Receiver     : {}",
                receiver.party_id.as_deref().unwrap_or("-")
            );
        }

        // Header references (SG1) — e.g. RFF+Z13 Auftragsreferenz
        for r in utilmd.references() {
            println!(
                "Reference    : {} = {}",
                r.rff.qualifier,
                r.rff.reference.as_deref().unwrap_or("-")
            );
        }

        // Transactions / metering points (SG4)
        println!("Transactions : {}", utilmd.transactions().len());
        for (i, tx) in utilmd.transactions().iter().enumerate() {
            println!(
                "  [{i}] IDE: {} ({})",
                tx.ide.object_id.as_deref().unwrap_or("-"),
                &tx.ide.qualifier,
            );
        }
    }

    // ── Validation ───────────────────────────────────────────────────────────
    let report = msg.validate()?;
    if report.is_valid() {
        println!("\nValidation   : OK ({report})");
    } else {
        println!("\nValidation   : {} finding(s)", report.errors().len());
        for err in report.errors() {
            let rule = err.rule_id.as_deref().unwrap_or("-");
            println!("  [{}] {}", rule, err.message);
        }
    }

    // ── Serialization round-trip ─────────────────────────────────────────────
    let bytes = msg.serialize()?;
    println!("\nSerialized   : {} bytes", bytes.len());

    Ok(())
}
