//! Guard: the request bodies `demos/eeg-billing` posts to `edmd` are ones `edmd`
//! accepts.
//!
//! A demo payload is shipped API surface — it is what an MSB integrator copies
//! when wiring a direct push. `DirectPushRequest` denies unknown fields, so a
//! body naming a field the endpoint does not have is a refusal; what this guard
//! is for is the demo drifting from the endpoint while still passing, because
//! its assertion is on the settled amount rather than on the request.

use edmd::server::DirectPushRequest;

/// The quarter-hour push: 96 intervals of 1 kWh, under the Einspeisung register.
///
/// `obis_code` is the load-bearing field. `edmd` never reads an unlabelled
/// reading as feed-in — an unqualified quantity is that measuring point's
/// consumption — so a push that loses the register stores intervals the EEG
/// settlement cannot see, and the demo would settle EUR 0 with every HTTP call
/// reporting success.
#[test]
fn the_eeg_quarter_hour_push_deserialises() {
    let req: DirectPushRequest = serde_json::from_value(serde_json::json!({
        "intervals": [
            { "from": "2026-06-01T00:00:00Z", "to": "2026-06-01T00:15:00Z",
              "value": "1.0", "quality": "MEASURED" }
        ],
        "source": "DIRECT_PUSH",
        "obis_code": "1-0:2.8.0"
    }))
    .expect("edmd would refuse this demo payload — fix the demo, or the endpoint");
    assert_eq!(req.obis_code.as_deref(), Some("1-0:2.8.0"));
    assert_eq!(req.source.as_deref(), Some("DIRECT_PUSH"));
    assert_eq!(req.intervals.len(), 1);
}

/// The daily-bucket push the demo uses for the rest of the month is the same
/// body with wider intervals — the endpoint takes a period, not a resolution.
#[test]
fn the_eeg_daily_bucket_push_deserialises() {
    let req: DirectPushRequest = serde_json::from_value(serde_json::json!({
        "intervals": [
            { "from": "2026-06-02T00:00:00Z", "to": "2026-06-03T00:00:00Z",
              "value": "96.0", "quality": "MEASURED" }
        ],
        "source": "DIRECT_PUSH",
        "obis_code": "1-0:2.8.0"
    }))
    .expect("a daily bucket is the same request shape");
    assert_eq!(req.intervals.len(), 1);
}

/// A field the endpoint does not have is a refusal, not a discarded value.
///
/// `obis` is the near-miss that matters: the field is `obis_code`, and a push
/// carrying the short form would be accepted with the register dropped, which
/// stores the readings as consumption on an Erzeugungs-Marktlokation.
#[test]
fn a_field_the_endpoint_does_not_have_is_refused() {
    let err = serde_json::from_value::<DirectPushRequest>(serde_json::json!({
        "intervals": [],
        "obis": "1-0:2.8.0"
    }))
    .expect_err("the register field is `obis_code`");
    assert!(err.to_string().contains("unknown field"), "{err}");
}
