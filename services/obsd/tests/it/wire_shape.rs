//! The REST and the MCP surface must serve one representation of one instant.
//!
//! `obsd` answers the same projection two ways: the REST routes serialise the
//! `ProcessProjection` itself (`serde_json::to_value`), while the MCP tools go
//! through `projection_json`, which formats every instant as RFC 3339. Those two
//! paths disagreed: `time`'s derived `serde` format writes
//! `1970-01-01 00:00:00.0 +00:00:00` — a space where ISO 8601 puts a `T`, and an
//! offset carrying seconds — so a caller reading `deadline_at` off the REST
//! surface could not parse what an agent reading the MCP surface got.
//!
//! The whole point of these fields is arithmetic against now. This holds the two
//! together.

use mako_obs::domain::{DeadlineRisk, ProcessProjection, ProcessState};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn projection() -> ProcessProjection {
    ProcessProjection {
        process_id: uuid::Uuid::nil(),
        tenant: "9900000000001".to_owned(),
        pid: 55_001,
        family: "GPKE".to_owned(),
        workflow_name: "gpke-supplier-change".to_owned(),
        state: ProcessState::Running,
        malo_id: None,
        partner_mp_id: None,
        mdm_role: None,
        deadline_at: Some(OffsetDateTime::UNIX_EPOCH),
        deadline_source: None,
        deadline_risk: DeadlineRisk::Unknown,
        started_at: OffsetDateTime::UNIX_EPOCH,
        last_event_at: OffsetDateTime::UNIX_EPOCH,
        erc_code: None,
        initiator_is_affiliate: false,
    }
}

#[test]
fn every_instant_on_the_rest_surface_is_rfc_3339() {
    let value = serde_json::to_value(projection()).expect("projection serialises");
    let expected = OffsetDateTime::UNIX_EPOCH
        .format(&Rfc3339)
        .expect("epoch formats");

    for field in ["deadline_at", "started_at", "last_event_at"] {
        let served = value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{field} is not a string on the REST surface"));
        assert_eq!(served, expected, "{field}");
        OffsetDateTime::parse(served, &Rfc3339)
            .unwrap_or_else(|e| panic!("{field} does not round-trip as RFC 3339: {e}"));
    }
}
