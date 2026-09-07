//! Every GPKE Antwortnachricht mako sends must pass its own AHB.
//!
//! `e2e_ahb_conformance.rs` validates BDEW's **example files**; it never
//! renders one. `e2e_outbox_render_contract.rs` renders and re-parses, which
//! proves the bytes are EDIFACT and not JSON — it does not ask whether the
//! Anwendungsfall is complete. Between the two, the 55002 Bestätigung —
//! `demos/nb-stp`'s headline output, the message the whole NB STP path exists
//! to produce — went out with four AHB errors:
//!
//! - no `SG4 STS+7` Transaktionsgrund, and when one was supplied the answer
//!   echoed the Anmeldung's `ZW4`, which 55002 does not admit (`ZW6`/`ZW7`/
//!   `ZAP`);
//! - no `SG6 RFF+TN`, so nothing tied the answer to the Vorgang it answers;
//! - no `SG6 RFF+Z60`, so the LFN was not told which Produktpaket the NB will
//!   implement;
//! - behind `ZW7`, no `SG5 LOC+Z17` and no `SG8 SEQ+Z98`/`SEQ+ZF3` blocks, so
//!   the LFN learned neither the Messlokation nor its Messstellenbetreiber.
//!
//! Each of those renders and re-parses cleanly. Only the AHB says otherwise.

use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::edifact_renderer::{RenderError, render_to_wire_bytes};
use makod::party_registry::MpIdRegistry;

const NB: &str = "9900357000004";
const LFN: &str = "9900987654321";
const MSB: &str = "9903456000009";
const MALO: &str = "51238696012";
const MELO: &str = "DE00056266802AO6G56M11SN51G21M24S";

fn registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: NB.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
        },
        PartyConfig {
            mp_id: LFN.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: false,
            agency: None,
        },
    ])
    .expect("valid registry")
}

fn outbox(payload: serde_json::Value) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/gpke-antwort-test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        "UTILMD",
        LFN,
        payload,
    )
}

/// The answer payload `GpkeSupplierChangeWorkflow` enqueues, per PID.
///
/// Built from [`mako_gpke::AntwortForm`] — the same table the workflow gates
/// on — so this cannot drift into testing a payload nothing produces. The
/// per-PID differences are the point: an Ablehnung carries no Lokation, no
/// date and no Produktpaket, and 55005/55006 carry no Ergänzung at all.
fn antwort_payload(pid: u32, accepted: bool) -> serde_json::Value {
    let form = mako_gpke::AntwortForm::of(pid).unwrap_or_else(|| panic!("PID {pid} has a form"));
    let _ = accepted;
    let mut p = serde_json::json!({
        "pid":                     pid,
        "sender":                  NB,
        "receiver":                LFN,
        "antwort_code":            if form.lokationsdaten { "A51" } else { "A01" },
        // The Entscheidungsbaum the Antwortcode was resolved in: `E_0607` for
        // the Abmeldungs-Antworten, `E_0623`/`E_0622` for the Anmeldung's.
        "antwort_codeliste":       match pid {
            55005 | 55006 => "E_0607",
            _ if form.lokationsdaten => "E_0623",
            _ => "E_0622",
        },
        "document_code":           form.document_code,
        "referenz_vorgangsnummer": "VORGANG0001",
        "transaktionsgrund":       if matches!(pid, 55078 | 55080) { "E03" } else { "E01" },
        // Stated unconditionally, as the workflow does: the renderer drops it
        // where the column lists no place for the qualifier the PID rides.
        "process_date":            "20261001",
    });
    // `SG4 STS+7` DE 9013 element 3, exactly as the workflow decides it.
    let ergaenzung: Option<&str> = if form.klassifiziert_malo {
        Some("ZW7")
    } else {
        form.ergaenzung.or(if form.lokationsdaten {
            Some("ZW0") // 55078 echoes the erzeugende Anmeldung's own code
        } else {
            None
        })
    };
    if let Some(e) = ergaenzung {
        p["transaktionsgrund_ergaenzung"] = serde_json::json!(e);
    }
    if form.lokationsdaten {
        p["malo"] = serde_json::json!(MALO);
        p["geplantes_produktpaket"] = serde_json::json!("1");
        p["messlokationen"] = serde_json::json!([{
            "melo_id": MELO,
            "msb": { "mp_id": MSB, "rolle": "Z39", "grundlage": "Z19", "gmsb_mp_id": MSB },
        }]);
        p["malo_msb"] = serde_json::json!({ "mp_id": MSB, "rolle": "Z39", "grundlage": "Z19", "gmsb_mp_id": MSB });
    }
    p
}

fn render_and_validate(payload: serde_json::Value) -> String {
    let pid = payload["pid"].as_u64().expect("pid");
    let msg = outbox(payload);
    let rendered = render_to_wire_bytes(&msg, &registry())
        .unwrap_or_else(|e| panic!("PID {pid} must render: {e}"));
    let wire = String::from_utf8(rendered.bytes.clone()).expect("utf-8");
    let parsed = edi_energy::parse(&rendered.bytes).expect("rendered UTILMD must parse");
    let report = parsed.validate().expect("validation must run");
    assert!(
        report.errors().is_empty(),
        "PID {pid} does not pass its own AHB:\n{}\n{}",
        wire.replace('\'', "'\n"),
        report
            .errors()
            .iter()
            .map(|i| format!("  [{}] {}", i.rule_id.as_deref().unwrap_or("-"), i.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    wire
}

/// 55002 — Bestätigung Anmeldung verbrauchende MaLo, the NB STP demo's output.
#[test]
fn bestaetigung_anmeldung_passes_the_ahb() {
    let wire = render_and_validate(antwort_payload(55002, true));

    // The answer's own classification, not the Anmeldung's `ZW4`.
    assert!(wire.contains("STS+7++E01+ZW7'"), "{wire}");
    // The Antwortcode with the Entscheidungsbaum it was resolved in.
    assert!(wire.contains("STS+E01++A51:E_0623'"), "{wire}");
    // What it answers, and which Produktpaket the NB will implement.
    assert!(wire.contains("RFF+TN:VORGANG0001'"), "{wire}");
    assert!(wire.contains("RFF+Z60:1'"), "{wire}");
    // Behind `ZW7`: the Messlokation, and the MSB of both Lokationen.
    assert!(wire.contains(&format!("LOC+Z17+{MELO}'")), "{wire}");
    assert!(wire.contains("SEQ+Z98'"), "{wire}");
    assert!(wire.contains("SEQ+ZF3'"), "{wire}");
    assert!(wire.contains(&format!("RFF+Z19:{MELO}'")), "{wire}");
    assert!(wire.contains(&format!("CAV+Z91:{MSB}::Z39:Z19'")), "{wire}");
    // `CAV+ZF0` has one place: the Messlokations-Datenblock.
    assert_eq!(wire.matches("CAV+ZF0:").count(), 1, "{wire}");
}

/// The other five Antwort-PIDs `response_pid_for` derives.
#[test]
fn every_derived_antwort_pid_passes_the_ahb() {
    for (pid, accepted) in [
        (55003, false), // Ablehnung Anmeldung verb. MaLo
        (55005, true),  // Bestätigung Abmeldung
        (55006, false), // Ablehnung Abmeldung
        (55078, true),  // Bestätigung Anmeldung erz. MaLo
        (55080, false), // Ablehnung Anmeldung erz. MaLo
    ] {
        render_and_validate(antwort_payload(pid, accepted));
    }

    // An Ablehnung states no Lokation: everything the NB would have told the
    // LFN about the Marktlokation is what it is declining to supply.
    let ablehnung = render_and_validate(antwort_payload(55003, false));
    assert!(!ablehnung.contains("LOC+"), "{ablehnung}");
    assert!(!ablehnung.contains("SEQ+"), "{ablehnung}");
    assert!(!ablehnung.contains("RFF+Z60"), "{ablehnung}");
    assert!(ablehnung.contains("STS+7++E01+ZW4'"), "{ablehnung}");
}

/// `ZW7` says the Marktlokation is metered. An answer that says so and names
/// no Messlokation is incomplete in a way only the counterparty would notice,
/// so the renderer refuses it here.
#[test]
fn a_gemessene_malo_without_its_messlokation_is_refused() {
    let mut payload = antwort_payload(55002, true);
    payload["messlokationen"] = serde_json::json!([]);

    match render_to_wire_bytes(&outbox(payload), &registry()) {
        Err(RenderError::MissingField { field, .. }) => {
            assert!(field.contains("messlokationen"), "{field}");
        }
        Err(other) => panic!("expected MissingField, got {other}"),
        Ok(r) => panic!(
            "a ZW7 answer with no Messlokation reached the wire: {}",
            String::from_utf8_lossy(&r.bytes)
        ),
    }
}

/// The `SG8` blocks a `ZW7` answer opens make „Zugeordneter Marktpartner" Muss.
#[test]
fn a_gemessene_malo_without_its_messstellenbetreiber_is_refused() {
    let mut payload = antwort_payload(55002, true);
    payload.as_object_mut().expect("object").remove("malo_msb");

    match render_to_wire_bytes(&outbox(payload), &registry()) {
        Err(RenderError::MissingField { field, .. }) => {
            assert!(field.contains("malo_msb"), "{field}");
        }
        Err(other) => panic!("expected MissingField, got {other}"),
        Ok(r) => panic!(
            "a ZW7 answer with no MSB reached the wire: {}",
            String::from_utf8_lossy(&r.bytes)
        ),
    }
}
