//! Conformance tests for the MSCONS renderers.
//!
//! Every rendered message is parsed back and validated against its registered
//! release profile, rather than checked by asserting on segment substrings.
//! A substring assertion confirms that a segment the author thought of is
//! present; profile validation confirms that the message satisfies the rules the
//! receiver applies — mandatory segments, segment order, group repeats, and code
//! lists — including rules the author did not think of.
//!
//! This is not a substitute for a live BIKO exchange. It is the strongest check
//! available without one.
//!
//! ## Coverage
//!
//! The single-line-item case is checked on every run. The repeated-group cases
//! are `#[ignore]`d: the MSCONS profile models the AHB's SG5 (`NAD+DP`) and SG6
//! (`LOC`) as one group triggered by `LOC`, so a message with more than one
//! `LIN`/`QTY` cycle is reported out of order even when it conforms. Correcting
//! that changes how all inbound MSCONS is validated, so it is deliberately not
//! papered over here — run these with `--include-ignored` to see the gap.

use edi_energy::{Platform, Pruefidentifikator, validate_and_check_pid};
use makod::edifact_renderer::render_to_wire_bytes;

mod support;
use support::{outbox_message, registry};

/// Render, parse back, and validate against the release profile.
///
/// Panics with the validation issues when the message does not conform, and
/// separately when the round-tripped Prüfidentifikator is not the one asked for
/// — a message that validates but carries the wrong PID is routed to the wrong
/// process by the receiver.
fn assert_conforms(pid: u32, payload: serde_json::Value) -> String {
    let msg = outbox_message("MSCONS", "9900077000006", payload);
    let bytes = render_to_wire_bytes(&msg, &registry("9900357000004"))
        .unwrap_or_else(|e| panic!("PID {pid}: render failed: {e:?}"));

    let parsed = Platform::with_all_profiles()
        .parse(&bytes)
        .unwrap_or_else(|e| {
            let wire = String::from_utf8_lossy(&bytes);
            panic!("PID {pid}: rendered message does not parse: {e}\n{wire}");
        });

    let expected = Pruefidentifikator::new(pid).expect("valid PID");
    let report = validate_and_check_pid(&parsed, expected)
        .unwrap_or_else(|e| panic!("PID {pid}: validation failed to run: {e}"));

    assert!(
        report.is_valid(),
        "PID {pid}: rendered message does not conform to its profile: {:#?}\n{}",
        report.errors(),
        String::from_utf8_lossy(&bytes)
    );

    String::from_utf8(bytes).expect("EDIFACT output is UTF-8")
}

// ── Summed series (13003, 13023) ──────────────────────────────────────────────

fn summed_series_payload(pid: u32) -> serde_json::Value {
    serde_json::json!({
        "pid": pid,
        "sender_mp_id": "9900357000004",
        "receiver_mp_id": "9900077000006",
        "bilanzierungsgebiet_id": "DE0011YAPG4",
        "balancing_period": "202606",
        "version": "20260714050000+00",
        "intervals": [
            { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "12.500" },
            { "from": "202606010015+00", "to": "202606010030+00", "quantity_kwh": "13.000" },
        ],
    })
}

#[test]
#[ignore = "profile models AHB SG5+SG6 as one LOC-triggered group; repeated LIN/QTY reads as out of order"]
fn summenzeitreihe_conforms() {
    let wire = assert_conforms(13003, summed_series_payload(13003));
    // BGM DE 1001 `BK` — "Zeitreihen im Rahmen der Bilanzkreisabrechnung".
    assert!(wire.contains("BGM+BK"), "{wire}");
}

#[test]
#[ignore = "profile models AHB SG5+SG6 as one LOC-triggered group; repeated LIN/QTY reads as out of order"]
fn redispatch_ausfallarbeit_summenzeitreihe_conforms() {
    let wire = assert_conforms(13023, summed_series_payload(13023));
    assert!(wire.contains("BGM+Z46"), "{wire}");
}

// ── Work and power maxima (13015, 13016, 13019) ───────────────────────────────

fn arbeit_payload(pid: u32, with_maxima: bool) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "pid": pid,
        "sender_mp_id": "9900357000004",
        "receiver_mp_id": "9900077000006",
        "malo_id": "51238696781",
        "arbeit": {
            "quantity": "184500.000",
            "from": "202601010000+00",
            "to": "202605010000+00",
        },
    });
    if with_maxima {
        payload["leistungsmaxima"] = serde_json::json!([
            { "quantity": "412.500", "period": "202602" },
            { "quantity": "398.000", "period": "202601", "ersatzwert": true },
        ]);
    }
    payload
}

#[test]
#[ignore = "profile models AHB SG5+SG6 as one LOC-triggered group; repeated LIN/QTY reads as out of order"]
fn arbeit_leistungsmaximum_vor_lieferbeginn_conforms() {
    let wire = assert_conforms(13015, arbeit_payload(13015, true));
    assert!(wire.contains("BGM+Z27"), "{wire}");
}

#[test]
#[ignore = "profile models AHB SG5+SG6 as one LOC-triggered group; repeated LIN/QTY reads as out of order"]
fn energiemenge_und_leistungsmaximum_conforms() {
    let wire = assert_conforms(13016, arbeit_payload(13016, true));
    assert!(wire.contains("BGM+Z28"), "{wire}");
}

#[test]
fn energiemenge_conforms() {
    let wire = assert_conforms(13019, arbeit_payload(13019, false));
    assert!(wire.contains("BGM+7"), "{wire}");
}
