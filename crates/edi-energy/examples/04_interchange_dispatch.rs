//! # Example: Multi-message interchange dispatch
//!
//! A real EDI@Energy Übertragungsdatei (`UNB … UNZ`) can carry several
//! messages, and they need not be of one type. `parse_interchange` splits the
//! stream at message boundaries and yields each message on its own, so routing
//! logic can hand each to the handler for its type.
//!
//! The interchange below is **built** rather than written by hand: the same
//! builders `03_build_messages` demonstrates produce the four messages, they
//! are framed by [`InterchangeBuilder`], and every message is validated against
//! its own Anwendungsfall on the way back out. A hand-written fixture drifts
//! from the profiles the moment a Formatversion changes; a built one cannot.
//!
//! ## Run
//!
//! ```text
//! cargo run --example 04_interchange_dispatch
//! ```

use std::io::Cursor;

use edi_energy::utilmd_codes::{
    self, Produktpaket, Transaktionsgrund, dtm, nad, namensformat, transaktionsgrund,
};
use edi_energy::{
    AnyMessage, EdiEnergyMessage, Platform, Pruefidentifikator,
    builders::{AperakBuilder, InterchangeBuilder, MsconsBuilder, UtilmdBuilder},
    releases,
};

const LF: &str = "4012345000023";
const NB: &str = "9900357000004";
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let interchange = build_interchange()?;
    println!("Parsing a {}-byte interchange…\n", interchange.len());

    let mut counts = std::collections::BTreeMap::<String, usize>::new();

    for (i, result) in Platform::with_all_profiles()
        .parse_interchange(Cursor::new(&interchange))
        .enumerate()
    {
        let msg = result?;

        let type_name = msg
            .try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned());
        *counts.entry(type_name.clone()).or_insert(0) += 1;

        // CONTRL has no Prüfidentifikator; every other type does.
        let pid_str = match msg.detect_pruefidentifikator() {
            Ok(pid) => pid.as_u32().to_string(),
            Err(_) => "n/a".to_owned(),
        };
        let release = msg.detect_release()?;

        print!(
            "  [{i}] {type_name:<8}  PID={pid_str:<6}  release={:<6}",
            release.as_str(),
        );

        // Type-specific handling per message variant — what a router does once
        // it knows which handler owns the message.
        match &msg {
            AnyMessage::Utilmd(u) => {
                let vorgaenge = u.transactions().len();
                let doc_code = u.bgm().map(|b| b.document_code.clone()).unwrap_or_default();
                print!("  doc_code={doc_code}  Vorgänge={vorgaenge}");
            }
            AnyMessage::Mscons(m) => {
                let points = m.delivery_points().len();
                print!("  Lieferorte={points}");
            }
            AnyMessage::Aperak(a) => {
                let codes: Vec<&str> = a
                    .errors()
                    .iter()
                    .map(|e| e.erc.error_code.as_str())
                    .collect();
                print!("  ERC={codes:?}");
            }
            _ => {}
        }
        println!();

        // Each message stands on its own: a router that forwards one must be
        // able to hand it on as a complete message, and it must still be the
        // Anwendungsfall it claims to be.
        let report = msg.validate()?;
        assert!(
            report.errors().is_empty(),
            "message {i} ({type_name} {pid_str}) does not pass its own AHB:\n{}",
            report
                .errors()
                .iter()
                .map(|e| format!("  [{}] {}", e.rule_id.as_deref().unwrap_or("-"), e.message))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        assert!(!msg.serialize()?.is_empty(), "message {i} re-serialises");
    }

    println!("\nSummary:");
    for (t, n) in &counts {
        println!("  {t:<8} × {n}");
    }
    println!(
        "\nAll {} messages processed, each validated on its own.",
        counts.values().sum::<usize>()
    );
    Ok(())
}

/// Frame four freshly built messages in one `UNB … UNZ`.
///
/// The `UNB` MP-IDs must equal the `NAD+MS` / `NAD+MR` of the messages inside
/// it (Allgemeine Festlegungen 6.1d Kap. 2), so an interchange only ever holds
/// messages between the *same* two parties. A second counterparty means a
/// second interchange, not a second `NAD` inside this one.
fn build_interchange() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let anmeldung = utilmd_anmeldung("MSG-1", "VORGANG0001")?;
    let abmeldung = utilmd_anmeldung("MSG-2", "VORGANG0002")?;
    let lastgang = mscons_lastgang("MSG-3")?;
    let aperak = aperak_fehler("MSG-4", "MSG-1")?;

    Ok(InterchangeBuilder::new(LF, NB, "BULK-2026-001")
        .transmission("260701", "0800")
        .message(anmeldung)
        .message(abmeldung)
        .message(lastgang)
        .message(aperak)
        .build()?)
}

fn utilmd_anmeldung(
    message_ref: &str,
    vorgang: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(UtilmdBuilder::new(releases::utilmd_fv20251001().clone())
        .pruefidentifikator(Pruefidentifikator::new(55001)?)
        .sender(LF)
        .receiver(NB)
        .message_ref(message_ref)
        .document_date("20260701")
        .document_code("E01")
        .transaction(vorgang)
        .date(dtm::BEGINN_ZUM, "20261001")
        .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(
            transaktionsgrund::EIN_AUSZUG,
        ))
        .marktlokation("51238696012")
        .produktpaket(Produktpaket::bilanzkreis("11XBK-STD-----9"))
        .stammdaten(utilmd_codes::SEQ_DATEN_DER_MARKTLOKATION)
        .cci("", "Z15")
        .done()
        .stammdaten(utilmd_codes::SEQ_DATEN_DES_KUNDEN)
        .cci("Z61", "ZF9")
        .cav("ZU5")
        .done()
        .kunde_des_lf(["Mustermann".to_owned()], namensformat::PERSON)
        .anschrift(
            nad::KORRESPONDENZANSCHRIFT_KUNDE,
            ["Mustermann".to_owned()],
            namensformat::PERSON,
            "Musterstr. 1",
            "Berlin",
            "",
            "DE",
        )
        .done()
        .build()?
        .serialize()?)
}

/// MSCONS 13018 „Lastgang Messlokation" — Strom, so `NAD` DE 3055 is `9`/`293`.
fn mscons_lastgang(message_ref: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(MsconsBuilder::new(releases::mscons_fv20260401().clone())
        .pruefidentifikator(Pruefidentifikator::new(13018)?)
        .sender(LF)
        .receiver(NB)
        .message_ref(message_ref)
        .document_date("20260701")
        .metering_point("DE00056266802AO6G56M11SN51G21M24S")
        .messperiode("202606302200+00", "202606302300+00")
        .obis(rubo4e::identifiers::ObisCode::new("1-1:1.29.0").expect("valid OBIS"))
        .quantity_for_period("220", "1234.567", "", "202606302200+00", "202606302215+00")
        .done()
        .build()?
        .serialize()?)
}

fn aperak_fehler(
    message_ref: &str,
    answering: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(AperakBuilder::new(releases::aperak_fv20251001().clone())
        .pruefidentifikator(Pruefidentifikator::new(29001)?)
        .sender(LF)
        .receiver(NB)
        .message_ref(message_ref)
        .document_date("20260701")
        .acw_ref(answering)
        .error_code("Z29")
        .error_text("SG8 Produktpaket fehlt")
        .build()?
        .serialize()?)
}
