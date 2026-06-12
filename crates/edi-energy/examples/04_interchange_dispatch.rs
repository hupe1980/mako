//! # Example: Multi-message interchange dispatch
//!
//! A real EDI@Energy interchange envelope (`UNB … UNZ`) can contain multiple
//! messages of different types.  `parse_interchange` splits the stream at
//! message boundaries and yields each message independently so that routing
//! logic can handle every type separately.
//!
//! This example builds a five-message interchange (UTILMD × 2, MSCONS,
//! APERAK, CONTRL) in-memory and processes it with the iterator API.
//!
//! ## Run
//!
//! ```text
//! cargo run --example 04_interchange_dispatch
//! ```

use std::io::Cursor;

use edi_energy::{AnyMessage, EdiEnergyMessage, parse_interchange};

/// An interchange containing five different EDI@Energy message types.
const INTERCHANGE: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240115:0800+BULK-001'\
UNH+MSG-1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03+11001+9'\
DTM+137:20240115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+MSG-1'\
UNH+MSG-2+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01+11003+9'\
DTM+137:20240115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+MSG-2'\
UNH+MSG-3+MSCONS:D:04B:UN:2.4c'\
BGM+7+13002+9'\
DTM+137:20240115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNS+D'\
UNT+7+MSG-3'\
UNH+MSG-4+APERAK:D:07B:UN:2.1i'\
BGM+312+00029001+9'\
DTM+137:20240115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+MSG-4'\
UNH+MSG-5+CONTRL:D:3:UN:2.0b'\
UCI+BULK-001+9900357000004:14+4012345000023:14+7'\
UNT+3+MSG-5'\
UNZ+5+BULK-001'";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Parsing interchange…\n");

    let reader = Cursor::new(INTERCHANGE);
    let mut counts = std::collections::HashMap::<String, usize>::new();

    for (i, result) in parse_interchange(reader).enumerate() {
        let msg = result?;

        let type_name = msg
            .try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned());
        *counts.entry(type_name.clone()).or_insert(0) += 1;

        // CONTRL has no Pruefidentifikator; other types do.
        let pid_str = match msg.detect_pruefidentifikator() {
            Ok(pid) => pid.as_u32().to_string(),
            Err(_) => "n/a".to_owned(),
        };
        let release = msg.detect_release()?;

        print!(
            "  [{i}] {type_name:<8}  PID={pid_str:<5}  release={:<8}",
            release.as_str(),
        );

        // Type-specific logic via exhaustive match
        match &msg {
            AnyMessage::Utilmd(u) => {
                let doc_code = u.bgm().map(|b| b.document_code.as_str()).unwrap_or("-");
                print!("  doc_code={doc_code}");
            }
            AnyMessage::Mscons(m) => {
                print!("  delivery_points={}", m.delivery_points().len());
            }
            AnyMessage::Aperak(a) => {
                print!("  errors={}", a.errors().len());
            }
            AnyMessage::Contrl(c) => {
                let uci_code = c
                    .uci()
                    .and_then(|u| u.action_code.as_deref())
                    .unwrap_or("-");
                print!("  uci_action={uci_code}");
            }
            _ => {}
        }

        println!();

        // Serialize each message back to bytes (round-trip check)
        let _bytes = msg.serialize()?;
    }

    println!("\nSummary:");
    let mut pairs: Vec<_> = counts.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    for (t, n) in &pairs {
        println!("  {t:<8} × {n}");
    }

    println!(
        "\nAll {} messages processed.",
        pairs.iter().map(|(_, n)| *n).sum::<usize>()
    );
    Ok(())
}
