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

use edi_energy::{AnyMessage, EdiEnergyMessage, parse};

const MSCONS_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240115:0800+INTER-MS-001'\
UNH+MSG-002+MSCONS:D:04B:UN:2.4c'\
BGM+7+13002+9'\
DTM+137:20240115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNS+D'\
NAD+DP+DE0001234567890123456789012345::293'\
LOC+172+DE0001234567890123456789012345::293'\
DTM+163:20240101:102'\
DTM+164:20240131:102'\
LIN+1'\
PIA+5+1-1:1.29.0:SRW'\
QTY+220:1234.567:KWH'\
DTM+163:20240101000000:203'\
DTM+164:20240131235959:203'\
STS+7::293'\
UNT+17+MSG-002'\
UNZ+1+INTER-MS-001'";

fn main() -> Result<(), edi_energy::Error> {
    let msg = parse(MSCONS_BYTES)?;

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
                    println!("    [LI {}] OBIS: {}", li_i, obis);

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

    // Serialize and verify round-trip
    let bytes = msg.serialize()?;
    let reparsed = parse(&bytes)?;
    assert_eq!(reparsed.try_message_type(), msg.try_message_type());
    println!("\nRound-trip   : OK ({} bytes)", bytes.len());

    Ok(())
}
