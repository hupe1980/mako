//! # Example: Build messages with the fluent builder API
//!
//! Demonstrates constructing EDI@Energy messages programmatically using the
//! fluent builder types in [`edi_energy::builders`].
//!
//! Builders guarantee syntactically correct EDIFACT output and enforce the
//! mandatory segment order defined by the EDI@Energy profiles.
//!
//! ## Run
//!
//! ```text
//! cargo run --example 03_build_messages
//! ```

use edi_energy::{
    EdiEnergyMessage, ObjectType, Pruefidentifikator, Release,
    builders::{AperakBuilder, ContrlBuilder, MsconsBuilder, UtilmdBuilder},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    build_utilmd()?;
    println!();
    build_mscons()?;
    println!();
    build_aperak()?;
    println!();
    build_contrl()?;
    Ok(())
}

// ── UTILMD ────────────────────────────────────────────────────────────────────

fn build_utilmd() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== UTILMD (S2.1 — Strom, valid from 01.10.2024) ===");

    let pid = Pruefidentifikator::new(55001)?;
    // S2.1 is the current UTILMD Strom format (fv20241001).
    // Use releases::utilmd_fv20241001() to get the validated Release constant.
    let release = edi_energy::releases::utilmd_fv20241001().clone();

    let bytes = UtilmdBuilder::new(release)
        .pruefidentifikator(pid)
        .sender("4012345000023")
        .receiver("9900357000004")
        .message_ref("MSG-001")
        .document_date("20241101")
        .document_code("E01")
        // SG4/IDE — one transaction per metering-point process
        .transaction(ObjectType::Messlokation, "DE00012345678")
        .process_date("163", "20241101") // delivery start
        .reference("Z13", "55001") // per-transaction Pruefidentifikator
        .done()
        .build()?
        .serialize()?;

    let text = String::from_utf8_lossy(&bytes);
    println!(
        "Segments     : {}",
        bytes.iter().filter(|&&b| b == b'\'').count()
    );
    println!("Payload      :\n{}", text);

    // Re-parse and validate what we built
    let msg = edi_energy::parse(&bytes)?;
    println!(
        "Type         : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    println!(
        "PID          : {}",
        msg.detect_pruefidentifikator()?.as_u32()
    );

    Ok(())
}

// ── MSCONS ────────────────────────────────────────────────────────────────────

fn build_mscons() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== MSCONS (2.4c — fv20251001, valid Oct 2025 – Sep 2026) ===");

    let pid = Pruefidentifikator::new(13003)?;
    let release = Release::new("2.4c");

    let bytes = MsconsBuilder::new(release)
        .pruefidentifikator(pid)
        .sender("4012345000023")
        .receiver("9900357000004")
        .message_ref("MSG-002")
        .document_date("20240115")
        // Add a metering point with OBIS code and quantity
        .metering_point("DE0001234567890123456789012345")
        .obis("1-1:1.29.0:SRW")
        .quantity("220", "1234.567", "KWH")
        .done()
        .build()?
        .serialize()?;

    let text = String::from_utf8_lossy(&bytes);
    println!(
        "Segments     : {}",
        bytes.iter().filter(|&&b| b == b'\'').count()
    );
    println!("Payload      :\n{}", text);

    let msg = edi_energy::parse(&bytes)?;
    println!(
        "Type         : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    println!(
        "PID          : {}",
        msg.detect_pruefidentifikator()?.as_u32()
    );

    Ok(())
}

// ── APERAK ───────────────────────────────────────────────────────────────────

fn build_aperak() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== APERAK (2.1i — fv20251001, valid Oct 2025 – Sep 2026) ===");

    let pid = Pruefidentifikator::new(29001)?;

    let bytes = AperakBuilder::new(Release::new("2.1i"))
        .pruefidentifikator(pid)
        .sender("4012345000023")
        .receiver("9900357000004")
        .acw_ref("ACK-001")
        .message_ref("MSG-003")
        .document_date("20240115")
        .error_code("Z43")
        .error_text("Unbekannte Messlokation")
        .build()?
        .serialize()?;

    let text = String::from_utf8_lossy(&bytes);
    println!(
        "Segments     : {}",
        bytes.iter().filter(|&&b| b == b'\'').count()
    );
    println!("Payload      :\n{}", text);

    let msg = edi_energy::parse(&bytes)?;
    println!(
        "Type         : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );

    Ok(())
}

// ── CONTRL ───────────────────────────────────────────────────────────────────

fn build_contrl() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== CONTRL (accept — 2.0b, fv20260101) ===");

    let release = Release::new("2.0b");

    let bytes = ContrlBuilder::new(release)
        .sender("4012345000023")
        .receiver("9900357000004")
        .interchange_ref("INTER-2024-001")
        .message_ref("MSG-004")
        .accept()
        .build()?
        .serialize()?;

    let text = String::from_utf8_lossy(&bytes);
    println!(
        "Segments     : {}",
        bytes.iter().filter(|&&b| b == b'\'').count()
    );
    println!("Payload      :\n{}", text);

    let msg = edi_energy::parse(&bytes)?;
    println!(
        "Type         : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );

    Ok(())
}
