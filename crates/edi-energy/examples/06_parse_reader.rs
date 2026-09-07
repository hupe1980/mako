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

/// A UTILMD **55002 Bestätigung Anmeldung Lieferbeginn** (NB → LFN), the answer
/// to the 55001 in `01_parse_utilmd`.
///
/// It validates clean — `main` asserts it. A fixture an example tells people to
/// swap their own file into has to be the thing it claims to be, or the first
/// thing they learn is that mako's own sample fails mako's own validator.
/// A UTILMD **55002 Bestätigung Anmeldung Lieferbeginn** (NB → LFN), the answer
/// to the 55001 in `01_parse_utilmd`.
///
/// A Bestätigung is not a short message. Its Prüfschablone makes six things
/// Muss that the Anmeldung does not:
///
/// - `STS+7++E01+ZW7` — the NB's own classification of the Marktlokation. The
///   Anmeldung says `ZW4` „verbrauchende Marktlokation"; 55002 admits only
///   `ZW6` pauschal, `ZW7` gemessen and `ZAP` ruhend.
/// - `STS+E01++A51:E_0623` — the Antwortcode *with* the Entscheidungsbaum it
///   was resolved in. DE 1131 is Muss in the MIG.
/// - `RFF+TN` — the Anfrage's Vorgangsnummer. The answer's own `IDE+24` is a
///   fresh number, so this is the only correlation it carries.
/// - `RFF+Z60` — which Produktpaket-ID the NB will implement.
/// - `LOC+Z17` and `SEQ+ZF3` — the Messlokation, and behind `ZW7` all of them
///   (Bedingung `[623]`).
/// - `SEQ+Z98` / `SEQ+ZF3` with `CCI+++ZB3` — the Messstellenbetreiber of the
///   Marktlokation and of each Messlokation, `CAV+Z91:<MP-ID>::<Rolle>:<Grundlage>`
///   plus `CAV+ZF0` for the grundzuständiger MSB.
///
/// `main` asserts the validator finds nothing. A fixture an example tells
/// people to swap their own file into has to be the thing it claims to be.
const FIXTURE: &[u8] = b"\
UNB+UNOC:3+9900357000004:500+4012345000023:14+260701:0900+INTER-R-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01+00055002'\
DTM+137:202607010900?+00:303'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::9'\
IDE+24+VORGANG0002'\
DTM+92:202610010000?+00:303'\
STS+7++E01+ZW7'\
STS+E01++A51:E_0623'\
LOC+Z16+51238696012'\
LOC+Z17+DE00056266802AO6G56M11SN51G21M24S'\
RFF+Z13:55002'\
RFF+TN:VORGANG0001'\
RFF+Z60:1'\
SEQ+Z98'\
CCI+++ZB3'\
CAV+Z91:9903456000009::Z39:Z19'\
SEQ+ZF3'\
RFF+Z19:DE00056266802AO6G56M11SN51G21M24S'\
CCI+++ZB3'\
CAV+Z91:9903456000009::Z39:Z19'\
CAV+ZF0:9903456000009'\
UNT+23+MSG-001'\
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
    //
    // A file passed with `--file` is whatever the caller has; the embedded
    // fixture is mako's own and has to pass. Reporting `FAILED` and exiting `0`
    // is how the previous one — an MSCONS with no `SG5` and the wrong `NAD`
    // code list — stayed broken.
    let report = msg.validate()?;
    println!(
        "Validation   : {}",
        if report.is_valid() { "OK" } else { "FAILED" }
    );
    println!("             : {report}");
    for issue in report.errors() {
        println!(
            "             : [{}] {}",
            issue.rule_id.as_deref().unwrap_or("-"),
            issue.message
        );
    }
    assert!(
        file_path.is_some() || report.is_valid(),
        "the embedded fixture must pass the AHB",
    );

    // ── Dump serialised bytes as text ─────────────────────────────────────────
    println!("\nSerialized output:");
    println!("{}", "-".repeat(60));
    let out = io::stdout();
    io::Write::write_all(&mut out.lock(), &raw_bytes)?;
    println!("\n{}", "-".repeat(60));

    Ok(())
}
