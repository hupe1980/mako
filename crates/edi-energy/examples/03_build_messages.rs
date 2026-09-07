//! # Example: Build messages with the fluent builder API
//!
//! Demonstrates constructing EDI@Energy messages programmatically using the
//! fluent builder types in [`edi_energy::builders`].
//!
//! Builders guarantee syntactically correct EDIFACT output and enforce the
//! mandatory segment order defined by the EDI@Energy profiles. They do **not**
//! guarantee a conformant *Anwendungsfall*: which segments a given
//! Prüfidentifikator makes Muss is the AHB's business, so every message built
//! here is parsed back and validated, and the example fails if any of them
//! carries an error. That is the check a caller pasting this code wants to
//! have inherited.
//!
//! ## Run
//!
//! ```text
//! cargo run --example 03_build_messages
//! ```

use edi_energy::utilmd_codes::{
    self, Produktpaket, Transaktionsgrund, dtm, nad, namensformat, transaktionsgrund,
};
use edi_energy::{
    EdiEnergyMessage, Platform, Pruefidentifikator,
    builders::{AperakBuilder, ContrlBuilder, MsconsBuilder, UtilmdBuilder},
    releases,
};

/// The parties. `4…` is a GS1 GLN, `99…` a BDEW Codenummer, `98…` a DVGW one;
/// the builders derive `NAD` DE 3055 (9 / 293 / 332) from the shape of the
/// number rather than making the caller repeat it — and which of the three a
/// given Anwendungsfall admits is a Sparte question, not a preference.
const LF: &str = "4012345000023";
const NB_STROM: &str = "9900357000004";
const NB_GAS: &str = "9870000000009";

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

/// Serialize, print, parse back, and hold the result against the MIG + AHB.
///
/// An example that only *builds* proves the writer works. Whether the message
/// would be accepted by the Marktpartner it is addressed to is a different
/// question, and it is the one that matters.
fn show(label: &str, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let text = String::from_utf8_lossy(bytes);
    println!(
        "Segments     : {}",
        bytes.iter().filter(|&&b| b == b'\'').count()
    );
    println!("Payload      :\n{}", text.replace('\'', "'\n"));

    let msg = Platform::with_all_profiles().parse(bytes)?;
    println!(
        "Type         : {}",
        msg.try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned())
    );
    if let Ok(pid) = msg.detect_pruefidentifikator() {
        println!("PID          : {}", pid.as_u32());
    }

    let report = msg.validate()?;
    assert!(
        report.errors().is_empty(),
        "{label} does not pass its own AHB:\n{}",
        report
            .errors()
            .iter()
            .map(|e| format!("  [{}] {}", e.rule_id.as_deref().unwrap_or("-"), e.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    println!("Validation   : OK ({report})");
    Ok(())
}

// ── UTILMD ────────────────────────────────────────────────────────────────────

fn build_utilmd() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== UTILMD 55001 Anmeldung Lieferbeginn Strom (LFN → NB) ===");

    // `releases::utilmd_fv20251001()` rather than `Release::new("S2.1")`: the
    // accessor exists only for a Formatversion this build actually embeds, so a
    // release string that no longer ships fails to compile instead of parsing
    // into a message nothing can validate.
    let bytes = UtilmdBuilder::new(releases::utilmd_fv20251001().clone())
        .pruefidentifikator(Pruefidentifikator::new(55001)?)
        .sender(LF)
        .receiver(NB_STROM)
        .message_ref("MSG-001")
        .document_date("20260701")
        .document_code("E01")
        // SG4 — one Vorgang per Geschäftsvorfall, keyed by the sender's
        // Vorgangsnummer. `IDE+24` DE 7402 is that number and not the MaLo.
        .transaction("VORGANG0001")
        .date(dtm::BEGINN_ZUM, "20261001") // DTM+92 — the Lieferbeginn
        .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(
            transaktionsgrund::EIN_AUSZUG,
        ))
        .marktlokation("51238696012") // SG5 LOC+Z16
        // SG8/SG12 — Produktpaket, Priorisierung and the Kunde des LF. All
        // Muss on 55001: without the Bilanzkreis the NB cannot assign the LF
        // to the Marktlokation, and the answer would be an Ablehnung.
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
        .serialize()?;

    show("UTILMD 55001", &bytes)
}

// ── MSCONS ────────────────────────────────────────────────────────────────────

fn build_mscons() -> Result<(), Box<dyn std::error::Error>> {
    // 13002 is „Zählerstand (Gas)" (MSCONS AHB 3.2 Kap. 6.4.3), so the
    // Netzbetreiber here is a Gas one: its Prüfschablone admits `NAD` DE 3055
    // `9` (GS1) and `332` (DVGW) and **not** `293` (BDEW), which is the Strom
    // code list. A `99…` MP-ID would be rejected by the receiving GNB.
    println!("=== MSCONS 13002 Zählerstand Gas (GNB → LF) ===");

    let bytes = MsconsBuilder::new(releases::mscons_fv20260401().clone())
        .pruefidentifikator(Pruefidentifikator::new(13002)?)
        .sender(NB_GAS)
        .receiver(LF)
        .message_ref("MSG-002")
        .document_date("20260701")
        // `LOC+172` carries a **Messlokations-ID** — 33 characters. An 11-digit
        // MaLo here is refused by `SEM-MSCONS-LOCATION-FORMAT`.
        .metering_point("DE00056266802AO6G56M11SN51G21M24S")
        .geraetenummer("1ZHR12345678")
        .obis(rubo4e::identifiers::ObisCode::new("1-1:1.29.0").expect("valid OBIS"))
        // `quantity_for_period`, not `quantity`: a reading without a `DTM`
        // pair states a number the receiver cannot place in time.
        .quantity_for_period("220", "1234.567", "", "202606302200+00", "202606302215+00")
        .done()
        .build()?
        .serialize()?;

    show("MSCONS 13002", &bytes)
}

// ── APERAK ───────────────────────────────────────────────────────────────────

fn build_aperak() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== APERAK 29001 Fehlermeldung (BGM+313) ===");

    // No `document_code`: `BGM` DE 1001 follows from whether an error is
    // reported — `313` here, `312` for the 29002 Anerkennungsmeldung — and the
    // builder derives it. `1000`, the generic UN/EDIFACT APERAK code, is
    // admitted by no EDI@Energy Prüfschablone.
    let bytes = AperakBuilder::new(releases::aperak_fv20251001().clone())
        .pruefidentifikator(Pruefidentifikator::new(29001)?)
        .sender(NB_STROM)
        .receiver(LF)
        .message_ref("MSG-003")
        .document_date("20260701")
        // `SG2` — which message is being answered. Muss in both
        // Anwendungsfälle: an acknowledgement that does not say what it
        // acknowledges is refused.
        .acw_ref("MSG-001")
        .error_code("Z29")
        .error_text("SG8 Produktpaket fehlt")
        .build()?
        .serialize()?;

    show("APERAK 29001", &bytes)
}

// ── CONTRL ───────────────────────────────────────────────────────────────────

fn build_contrl() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== CONTRL Empfangsbestätigung (UCI action 7) ===");

    let bytes = ContrlBuilder::new(releases::contrl_fv20260101().clone())
        .sender(NB_STROM)
        .receiver(LF)
        .interchange_ref("INTER-2026-001")
        .message_ref("MSG-004")
        .accept()
        .build()?
        .serialize()?;

    show("CONTRL", &bytes)
}
