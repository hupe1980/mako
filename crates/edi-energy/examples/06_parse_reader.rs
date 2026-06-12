//! # Example: Parse from a reader / file
//!
//! In production, EDI@Energy messages arrive as byte streams: files on disk,
//! HTTP responses, AS4 attachments, etc.  `Parser::parse_reader` accepts any
//! `std::io::Read` source so the caller controls buffering and I/O.
//!
//! This example shows:
//!
//! - Parsing from an in-memory `Cursor<&[u8]>` (simulating a file or socket)
//! - Parsing a real file from the command-line argument `--file <path>` if
//!   provided, otherwise using the embedded fixture
//! - Using `ParseConfig` to customise the segment-size limit
//! - Serialising the parsed message to stdout as clean ASCII
//!
//! ## Run (embedded fixture)
//!
//! ```text
//! cargo run --example 06_parse_reader
//! ```
//!
//! ## Run with a file on disk
//!
//! ```text
//! cargo run --example 06_parse_reader -- --file /path/to/message.edi
//! ```

use std::fs::File;
use std::io::{self, BufReader, Cursor};

use edi_energy::{EdiEnergyMessage, ParseConfig, Parser};

const FIXTURE: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240115:0800+INTER-R-001'\
UNH+MSG-001+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013002::+9'\
DTM+137:20240115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNS+D'\
LOC+172+51238696781'\
LIN+1'\
PIA+5+1-1:1.29.0:SRW'\
QTY+220:1234.567:KWH'\
STS+Z32'\
STS+Z40'\
UNT+13+MSG-001'\
UNZ+1+INTER-R-001'";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Argument parsing (minimal, no extra dep) ─────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let file_path: Option<&str> = args
        .windows(2)
        .find(|w| w[0] == "--file")
        .map(|w| w[1].as_str());

    // ── Parse from reader ────────────────────────────────────────────────────
    let msg = if let Some(path) = file_path {
        println!("Reading from file: {path}");
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Parser::new().parse_reader(reader)?
    } else {
        println!("Using embedded fixture (pass --file <path> to read from disk)");
        Parser::new().parse_reader(Cursor::new(FIXTURE))?
    };

    println!(
        "\nMessage type : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    println!(
        "PID          : {}",
        msg.detect_pruefidentifikator()?.as_u32()
    );
    println!("Release      : {}", msg.detect_release()?.as_str());

    // ── Custom ParseConfig ────────────────────────────────────────────────────
    // Parser::with_config accepts &[u8] directly via the parse() method;
    // for a reader-based demo we serialise first so we have bytes to pass in.
    let raw_bytes = msg.serialize()?;
    let config = ParseConfig {
        max_segment_bytes: 4096, // tighter than the 64 KiB default
        ..ParseConfig::default()
    };
    let msg2 = Parser::with_config(config).parse(&raw_bytes)?;
    assert_eq!(
        msg2.try_message_type(),
        msg.try_message_type(),
        "type mismatch after config round-trip"
    );
    println!("\nRound-trip via ParseConfig: OK");

    // ── Validate ─────────────────────────────────────────────────────────────
    let report = msg.validate()?;
    println!(
        "Validation   : {}",
        if report.is_valid() { "OK" } else { "FAILED" }
    );
    println!("             : {report}");

    // ── Dump serialised bytes as text ─────────────────────────────────────────
    println!("\nSerialized output:");
    println!("{}", "-".repeat(60));
    let out = io::stdout();
    io::Write::write_all(&mut out.lock(), &raw_bytes)?;
    println!("\n{}", "-".repeat(60));

    Ok(())
}
