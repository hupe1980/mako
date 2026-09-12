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

/// A UTILMD **55001 Anmeldung Lieferbeginn Strom** (LFN → NB), release S2.1.
///
/// It is a complete Anwendungsfall, not an abbreviation of one: the AHB's
/// Prüfschablone for 55001 makes `SG8` (Produktpaket, Priorisierung, Daten des
/// Kunden) and `SG12` (Kunde des Lieferanten, Korrespondenzanschrift) **Muss**,
/// so a message that stops after `LOC+Z16` is not a small 55001 — it is a
/// rejected one. `main` asserts the validator finds nothing, which is what
/// makes this fixture safe to copy.
///
/// ```text
/// UNB            interchange header — DE 0004/0010 equal the NAD MP-IDs
/// UNH            UTILMD:D:11A:UN:S2.1 (fv20251001)
/// BGM+E01        Anmeldung; DE 1004 is the Dokumentennummer
/// DTM+137        Dokumentendatum, DE 2379 = 303 (CCYYMMDDHHMMZZZ)
/// NAD+MS/MR      sender / receiver, DE 3055 = 9 (GS1)
/// IDE+24         SG4 — the sender's Vorgangsnummer, not the MaLo
/// DTM+92         „Beginn zum" — the Lieferbeginn
/// STS+7          Transaktionsgrund E01, verbrauchende MaLo (ZW4)
/// LOC+Z16        SG5 — the Marktlokation
/// RFF+Z13        SG6 — the Prüfidentifikator, per Vorgang
/// SEQ+Z79 …      SG8 — Produktpaket with the Bilanzkreis in CAV+ZV4
/// SEQ+ZH0 …      SG8 — Priorisierung
/// SEQ+Z01/Z75    SG8 — Daten der Marktlokation / des Kunden des LF
/// NAD+Z09/Z04    SG12 — Kunde des LF and his Korrespondenzanschrift
/// UNT / UNZ      trailers
/// ```
const UTILMD_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+260701:0800+INTER-2026-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01+00055001'\
DTM+137:202607010800?+00:303'\
NAD+MS+4012345000023::9'\
NAD+MR+9900357000004::9'\
IDE+24+VORGANG0001'\
DTM+92:202610010000?+00:303'\
STS+7++E01+ZW4'\
LOC+Z16+51238696012'\
RFF+Z13:55001'\
SEQ+Z79+1'\
PIA+5+9991000002082:Z11'\
CCI+Z66'\
CAV+ZV4:::11XBK-STD-----9'\
SEQ+ZH0+1'\
CCI+Z65+++Z01'\
SEQ+Z01'\
CCI+++Z15'\
SEQ+Z75'\
CCI+Z61++ZF9'\
CAV+ZU5'\
NAD+Z09+++Mustermann:::::Z01'\
NAD+Z04+++Mustermann:::::Z01+Musterstr. 1+Berlin+++DE'\
UNT+24+MSG-001'\
UNZ+1+INTER-2026-001'";

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
    //
    // Asserted, not printed. `cargo check` compiles an example without running
    // it, and a run that prints its own findings and exits `0` reports a stale
    // fixture as a success — so the fixture's conformance is an assertion the
    // run fails on, not a line in its output.
    let report = msg.validate()?;
    assert!(
        report.is_valid(),
        "the fixture in this example must pass the AHB:\n{}",
        report
            .errors()
            .iter()
            .map(|e| format!("  [{}] {}", e.rule_id.as_deref().unwrap_or("-"), e.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    println!("\nValidation   : OK ({report})");

    // ── Serialization round-trip ─────────────────────────────────────────────
    let bytes = msg.serialize()?;
    println!("\nSerialized   : {} bytes", bytes.len());

    Ok(())
}
