//! Metered values must survive the round trip from the wire to the ERP event.
//!
//! # What was broken
//!
//! `makod` decoded every MSCONS interval and then threw it away. The adapter
//! took only the `SG5 NAD` party id off the message; `MesswerteLieferungData`
//! had no field for a quantity, an OBIS code or a period; and the workflow's
//! only outbox entry was
//!
//! ```text
//! PendingOutbox::new("ProcessCompleted", "", json!({ "pid": pid.as_u32() }))
//! ```
//!
//! `edmd` refuses an event with no `malo_id` before it looks at anything else,
//! so the delivery was acknowledged, the process completed — and nothing was
//! stored. That applied to every PID in `MSCONS_PIDS`, not only the ESA's
//! Typ-2 values: `edmd`'s `store_typ2_reads` has exactly one caller, inside the
//! branch that could never be reached, so `esa_typ2_reads` was unpopulatable.
//!
//! # Why the test renders first
//!
//! The fixture is `makod`'s **own outbound 13027**, rendered by the production
//! renderer and parsed back. A hand-written interchange would test the values
//! against a shape nobody sends; this tests them against the bytes the MSB half
//! of the same feature emits, so the two directions cannot drift apart.

use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use mako_engine::version::FormatVersion;
use makod::adapters;

const MSB: &str = "9900357000004";
const ESA: &str = "9900555000005";
const MALO: &str = "51238696781";
const ORDER_REF: &str = "ORDERDOC0001";

/// Render the MSB-side delivery and hand back the wire bytes.
fn rendered_13027() -> String {
    let payload = serde_json::json!({
        "pid": 13027_u32,
        "sender_mp_id": MSB,
        "receiver_mp_id": ESA,
        "malo_id": MALO,
        "korrelation_ref": ORDER_REF,
        "reads": [
            { "dtm_from": "202606010000+00", "dtm_to": "202606010015+00",
              "quantity_kwh": "0.250", "obis_code": "1-0:1.29.0" },
            { "dtm_from": "202606010015+00", "dtm_to": "202606010030+00",
              "quantity_kwh": "0.310", "obis_code": "1-0:1.29.0" }
        ]
    });
    let msg = OutboxMessage::new(
        StreamId::new("process/test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        "MSCONS",
        ESA,
        payload,
    );
    let registry =
        makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: MSB.to_owned(),
            roles: vec!["MSB".to_owned()],
            primary: true,
            agency: None,
        }])
        .expect("registry");
    let out = makod::edifact_renderer::render_to_wire_bytes(&msg, &registry)
        .expect("the MSB-side 13027 renders");
    String::from_utf8(out.bytes).expect("utf-8")
}

/// The message `makod` itself emits satisfies the AHB rules for 13027.
///
/// MSCONS AHB 3.1g §11.2 makes `SG9 LIN`, `SG9 PIA` and both `SG1 RFF` groups
/// *Muss*; the profile used to call all three optional, with a note claiming
/// "no AHB constraint for PID 13027". A 13027 arriving without `PIA` — hence
/// without an OBIS code — validated clean, which would have masked half of
/// this fix.
#[test]
fn the_rendered_13027_passes_its_own_ahb_rules() {
    let wire = rendered_13027();
    let parsed = edi_energy::parse(wire.as_bytes()).expect("parses");
    let report = parsed
        .validate()
        .expect("a profile is registered for 13027");
    assert!(
        report.is_valid(),
        "mako's own 13027 must satisfy the tightened rules: {:?}",
        report.errors()
    );
    // The segments the AHB now requires are the ones the values live in.
    assert!(wire.contains("LIN+"), "{wire}");
    assert!(wire.contains("PIA+5+"), "{wire}");
    assert!(wire.contains("RFF+AGI:"), "{wire}");
}

/// The ESA-side adapter carries the readings into the domain command.
#[test]
fn the_adapter_carries_the_readings() {
    let wire = rendered_13027();
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let raw: &dyn std::any::Any = &msg;
    let cmd = adapters::gpke_messwerte_registry()
        .dispatch(raw, &FormatVersion::new("FV2025-10-01"))
        .expect("the MSCONS adapter accepts a 13027");

    let mako_gpke::MesswerteLieferungCommand::ReceiveMscons {
        pid,
        location_id,
        reads,
        ..
    } = cmd;

    assert_eq!(pid.as_u32(), 13027);
    assert_eq!(location_id.as_str(), MALO);
    assert_eq!(reads.len(), 2, "both quarter-hours: {reads:?}");

    let first = &reads[0];
    assert_eq!(first.obis_code.as_deref(), Some("1-0:1.29.0"));
    assert_eq!(first.quantity, "0.250");
    assert_eq!(first.unit.as_deref(), Some("KWH"));
    // `SG10 QTY` DE 6063 = 220 Wahrer Wert → the ERP quality vocabulary.
    assert_eq!(first.qualifier, "220");
    assert_eq!(first.quality(), "MEASURED");
    // `DTM+163`/`+164` in format 303, converted to RFC 3339.
    assert_eq!(first.dtm_from, "2026-06-01T00:00:00+00:00");
    assert_eq!(first.dtm_to, "2026-06-01T00:15:00+00:00");
    assert_eq!(reads[1].quantity, "0.310");
}

/// The `ProcessCompleted` payload is one `edmd` can actually store.
///
/// Every assertion here mirrors a field `services/edmd/src/handler.rs` reads.
/// `malo_id` is the one it checks first and rejects on; the rest is what the
/// interval parser and the PID 13027 Typ-2 fork consume.
#[test]
fn the_erp_payload_is_one_edmd_can_store() {
    use mako_engine::workflow::Workflow as _;

    let wire = rendered_13027();
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let raw: &dyn std::any::Any = &msg;
    let cmd = adapters::gpke_messwerte_registry()
        .dispatch(raw, &FormatVersion::new("FV2025-10-01"))
        .expect("adapter");

    // Run the real workflow rather than re-deriving the payload here.
    let out = mako_gpke::GpkeMesswerteLieferungWorkflow::handle(
        &mako_gpke::MesswerteLieferungState::default(),
        cmd,
    )
    .expect("the workflow accepts the delivery");

    let completed = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "ProcessCompleted")
        .expect("a validated delivery notifies the ERP");
    let data = &completed.payload;

    // `edmd` reads this first and drops the event when it is empty. It used to
    // be absent entirely.
    assert_eq!(data["malo_id"].as_str(), Some(MALO));
    assert_eq!(data["pid"].as_u64(), Some(13027));
    assert_eq!(data["sender"].as_str(), Some(MSB));
    assert_eq!(data["sparte"].as_str(), Some("STROM"));

    let reads = data["reads"].as_array().expect("reads array");
    assert_eq!(reads.len(), 2);

    // Parse exactly as `edmd` does, so a format change here fails here.
    use time::format_description::well_known::Rfc3339;
    let from = reads[0]["dtm_from"].as_str().expect("dtm_from");
    let to = reads[0]["dtm_to"].as_str().expect("dtm_to");
    let from =
        time::OffsetDateTime::parse(from, &Rfc3339).expect("edmd parses dtm_from as RFC 3339");
    let to = time::OffsetDateTime::parse(to, &Rfc3339).expect("edmd parses dtm_to as RFC 3339");
    assert!(
        from < to,
        "edmd drops an interval whose start is not before its end"
    );
    assert_eq!((to - from), time::Duration::minutes(15));

    let kwh = reads[0]["quantity_kwh"]
        .as_str()
        .expect("edmd reads quantity_kwh as a string")
        .parse::<rust_decimal::Decimal>()
        .expect("edmd parses it as a Decimal");
    assert_eq!(kwh.to_string(), "0.250");

    // `quality_from_mscons` maps this; anything outside its vocabulary becomes
    // `Unknown`, which is not billable.
    assert_eq!(reads[0]["quality"].as_str(), Some("MEASURED"));
    assert_eq!(reads[0]["obis_code"].as_str(), Some("1-0:1.29.0"));
}

/// A reading whose period is not in format `303` is skipped, not dated by
/// guesswork.
///
/// `102` (`CCYYMMDD`) and `203` (`CCYYMMDDHHMM`) carry no UTC offset. Reading
/// one as UTC is a silent one-hour error for half the year — on a quarter-hour
/// settlement value.
#[test]
fn a_reading_without_an_offset_is_skipped_rather_than_guessed() {
    let wire = rendered_13027().replace(":303'", ":203'");
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let raw: &dyn std::any::Any = &msg;
    let cmd = adapters::gpke_messwerte_registry()
        .dispatch(raw, &FormatVersion::new("FV2025-10-01"))
        .expect("adapter");
    let mako_gpke::MesswerteLieferungCommand::ReceiveMscons { reads, .. } = cmd;
    assert!(
        reads.is_empty(),
        "an offset-less period must not be read as UTC: {reads:?}"
    );
}

/// A conformant third-party 13027 carries the MaLo in `SG6 LOC+172` only.
///
/// MSCONS AHB 3.1g §11.2 gives `SG5 NAD` just DE 3035 = `DP`; the identifier is
/// `LOC` DE 3225. The adapter used to read `NAD` and worked only because mako's
/// own renderer happens to fill both — a message from anyone else produced an
/// empty location, which is the field `edmd` refuses the whole event on.
#[test]
fn the_malo_is_read_from_loc_not_only_from_nad() {
    // Strip the party id from NAD+DP, leaving LOC+172 as the only carrier.
    let wire = rendered_13027().replace(&format!("NAD+DP+{MALO}::293'"), "NAD+DP'");
    assert!(wire.contains("NAD+DP'"), "the fixture must lose the NAD id");
    assert!(
        wire.contains(&format!("LOC+172+{MALO}")),
        "LOC must still carry it"
    );

    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let raw: &dyn std::any::Any = &msg;
    let cmd = adapters::gpke_messwerte_registry()
        .dispatch(raw, &FormatVersion::new("FV2025-10-01"))
        .expect("adapter");
    let mako_gpke::MesswerteLieferungCommand::ReceiveMscons { location_id, .. } = cmd;
    assert_eq!(
        location_id.as_str(),
        MALO,
        "the MaLo must come from SG6 LOC+172"
    );
}
