//! The ESA Werteanfrage must reach the wire with the ESA's own inputs.
//!
//! `esa.werteanfrage.stellen` takes a Messprodukt and an Ansprechpartner from
//! the caller. REQOTE AHB 1.1 §4.3 makes both **Muss** on PID 35003: `SG27 PIA`
//! carries the Messprodukt (`4347 = 5`, `7143 = Z11`) and `SG14 CTA+IC`/`COM`
//! the contact. A renderer that substituted a placeholder would send a request
//! for a product the ESA never ordered — so these are pinned end to end,
//! through the production renderer rather than a hand-written fixture.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::edifact_renderer::render_to_wire_bytes;
use makod::party_registry::MpIdRegistry;

const ESA: &str = "9905550000005";
const MSB: &str = "9900357000004";

fn registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[PartyConfig {
        mp_id: ESA.to_owned(),
        roles: vec!["ESA".to_owned()],
        primary: true,
        agency: None,
    }])
    .expect("registry")
}

fn render(payload: serde_json::Value) -> Result<String, String> {
    let msg = OutboxMessage::new(
        StreamId::new("process/esa-werteanfrage"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        "REQOTE",
        MSB,
        payload,
    );
    render_to_wire_bytes(&msg, &registry())
        .map(|out| String::from_utf8(out.bytes).expect("UTF-8 wire"))
        .map_err(|e| e.to_string())
}

fn werteanfrage() -> serde_json::Value {
    serde_json::json!({
        "pid": 35003_u32,
        "sender": ESA,
        "receiver": MSB,
        "location": "51238696012",
        "messprodukt": "9991000003056",
        "wunschtermin": "2026-09-01",
        "contact": "Acme ESA Service",
        "contact_comm": "mako@acme-esa.example",
    })
}

#[test]
fn the_ordered_messprodukt_reaches_the_wire_unchanged() {
    let wire = render(werteanfrage()).expect("35003 renders");
    assert!(
        wire.contains("PIA+5+9991000003056:Z11"),
        "SG27 PIA must carry the caller's Messprodukt with DE 7143 = Z11:\n{wire}"
    );
    assert!(
        wire.contains("LIN+1+Z67"),
        "SG27 LIN 1229 = Z67 introduces the Messprodukt line:\n{wire}"
    );
}

#[test]
fn the_ansprechpartner_reaches_the_wire_unchanged() {
    let wire = render(werteanfrage()).expect("35003 renders");
    assert!(
        wire.contains("CTA+IC+:Acme ESA Service"),
        "SG14 CTA+IC must carry the caller's contact:\n{wire}"
    );
    assert!(
        wire.contains("COM+mako@acme-esa.example:EM"),
        "SG14 COM must carry the caller's communication address:\n{wire}"
    );
}

#[test]
fn a_werteanfrage_without_a_messprodukt_is_refused_rather_than_defaulted() {
    let mut payload = werteanfrage();
    payload
        .as_object_mut()
        .expect("object")
        .remove("messprodukt");
    let err = render(payload).expect_err("no Messprodukt must not render");
    assert!(
        err.contains("messprodukt"),
        "the error must name the missing field, not silently substitute a \
         placeholder product: {err}"
    );
}

/// REQOTE AHB 1.1 §4.3 gives both header dates as DE 2379 `303`
/// (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to `+00`. A `303`
/// without its zone is a value mako's own parser rejects.
#[test]
fn both_header_dates_are_303_with_the_931_zone() {
    let wire = render(werteanfrage()).expect("35003 renders");
    let dtm137 = wire
        .split('\'')
        .find(|s| s.starts_with("DTM+137"))
        .expect("DTM+137 present");
    assert!(
        dtm137.ends_with("+00:303"),
        "DTM+137 must be a zoned 303 stamp: {dtm137}"
    );
    assert!(
        wire.contains("DTM+76:202609010000?+00:303"),
        "DTM+76 must carry the Wunschtermin as a zoned 303 stamp:\n{wire}"
    );
}

/// `BGM DE 1004` is a Dokumentennummer; the Prüfidentifikator travels in
/// `SG1 RFF+Z13`. Putting the PID in BGM leaves the message unroutable.
#[test]
fn the_pruefidentifikator_travels_in_rff_z13_not_bgm() {
    let wire = render(werteanfrage()).expect("35003 renders");
    assert!(wire.contains("RFF+Z13:35003"), "RFF+Z13 missing:\n{wire}");
    assert!(
        !wire.contains("BGM+Z57+35003"),
        "BGM DE 1004 must be a Dokumentennummer, not the PID:\n{wire}"
    );
}
