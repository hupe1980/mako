//! # Example: Parse an MSCONS message and extract metered values
//!
//! MSCONS (Metered Services Consumption Report) carries time-series meter
//! readings between grid operators and balance-group managers.
//!
//! This example parses a minimal MSCONS 13002 message and walks the full
//! segment-group hierarchy:
//!
//! ```text
//! Message
//! └── DeliveryPoint (SG5: NAD)
//!     └── TimeSeries (SG6: LOC)
//!         └── LineItem (SG9: LIN + PIA)
//!             └── Quantity (SG10: QTY + DTM + STS)
//! ```
//!
//! ## Run
//!
//! ```text
//! cargo run --example 02_parse_mscons
//! ```
#![allow(clippy::result_large_err)]

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};

/// A conformant MSCONS 13002, taken from the validated fixture corpus.
///
/// Included rather than inlined, and that is the point. This example carried
/// its own hand-written interchange, and it was wrong in eleven ways at once —
/// no SG1 Prüfidentifikator, no SG7 Referenzangaben, `KWH` where the
/// Prüfschablone admits no Maßeinheit, an OBIS whose colon was never released
/// so the code silently truncated to `1-1`. None of it showed, because the
/// example parsed and printed without ever validating. A fixture the test suite
/// already holds to the AHB cannot drift that way on its own.
const MSCONS_BYTES: &[u8] =
    include_bytes!("../tests/fixtures/mscons/valid/beispiel_13002_release_2_4c.edi");

fn main() -> Result<(), edi_energy::Error> {
    let msg = Platform::with_all_profiles().parse(MSCONS_BYTES)?;

    println!(
        "Message type : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    println!("Release      : {}", msg.detect_release()?.as_str());
    println!(
        "PID          : {}",
        msg.detect_pruefidentifikator()?.as_u32()
    );

    if let AnyMessage::Mscons(mscons) = &msg {
        if let Some(bgm) = &mscons.bgm() {
            println!("Doc code     : {}", bgm.document_code);
        }

        // Message-level period (DTM+163/164)
        for dtm in mscons.dtm() {
            if dtm.is_document_date() {
                println!("Message date : {}", dtm.value_str().unwrap_or("-"));
            }
        }

        println!("\nDelivery points: {}", mscons.delivery_points().len());

        for (dp_i, dp) in mscons.delivery_points().iter().enumerate() {
            println!(
                "\n[DP {}] Location: {}",
                dp_i,
                dp.nad.party_id.as_deref().unwrap_or("-")
            );

            for (ts_i, ts) in dp.time_series.iter().enumerate() {
                println!(
                    "  [TS {}] LOC {} (qualifier: {})",
                    ts_i,
                    ts.loc.location_id.as_deref().unwrap_or("-"),
                    ts.loc.qualifier
                );

                // Delivery period for this time series
                let period_start = ts.dtm.iter().find(|d| d.is_period_start());
                let period_end = ts.dtm.iter().find(|d| d.is_period_end());
                if let (Some(s), Some(e)) = (period_start, period_end) {
                    println!(
                        "         Period: {} – {}",
                        s.value_str().unwrap_or("-"),
                        e.value_str().unwrap_or("-")
                    );
                }

                for (li_i, item) in ts.items.iter().enumerate() {
                    let obis = item
                        .pia
                        .as_ref()
                        .and_then(|p| p.item_number.as_deref())
                        .unwrap_or("-");
                    println!("    [LI {li_i}] OBIS: {obis}");

                    for qty_entry in &item.quantities {
                        let value_f64 = qty_entry.qty.value_f64().unwrap_or(f64::NAN);
                        let unit = qty_entry.qty.unit.as_deref().unwrap_or("-");
                        let metered = qty_entry.qty.is_metered();

                        println!(
                            "      QTY: {:.3} {} {}",
                            value_f64,
                            unit,
                            if metered { "(metered)" } else { "" }
                        );

                        // Interval DTMs
                        for dtm in &qty_entry.dtm {
                            if dtm.is_period_start() {
                                println!("        Start : {}", dtm.value_str().unwrap_or("-"));
                            } else if dtm.is_period_end() {
                                println!("        End   : {}", dtm.value_str().unwrap_or("-"));
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Validation ───────────────────────────────────────────────────────────
    //
    // Asserted, not printed. An example that parses a fixture and exits `0`
    // certifies nothing about the fixture: this one carried an OBIS whose colon
    // was never released (`1-1:1.29.0` split into two components, so the code
    // silently truncated to `1-1`) and a 30-character Messlokations-ID, and it
    // printed both without complaint for as long as nobody validated it.
    let report = msg.validate()?;
    assert!(
        report.is_valid(),
        "the embedded MSCONS no longer conforms to its own profile:\n{}",
        report
            .errors()
            .iter()
            .map(|e| format!("  [{}] {}", e.rule_id.as_deref().unwrap_or("-"), e.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    println!("\nValidation   : OK ({report})");

    // Serialize and verify round-trip
    let bytes = msg.serialize()?;
    let reparsed = Platform::with_all_profiles().parse(&bytes)?;
    assert_eq!(reparsed.try_message_type(), msg.try_message_type());
    println!("\nRound-trip   : OK ({} bytes)", bytes.len());

    Ok(())
}
