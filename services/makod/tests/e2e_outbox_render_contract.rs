//! Outbox entries must reach the wire as EDIFACT, not as JSON.
//!
//! An outbox `message_type` with no renderer behind it does not fail loudly.
//! `render_to_wire_bytes` returns `InsufficientPayload`, and the AS4 sender
//! substitutes the raw domain **JSON** for the interchange: the message leaves
//! the system, looks delivered, and cannot be parsed by the receiving partner.
//! Only the in-process loopback path dead-letters.
//!
//! Workflow-level tests cannot see this. They assert that an outbox entry
//! exists and carries the expected fields, which it does — the mismatch is
//! between the producer's key names and the renderer's contract. These tests
//! cross that boundary: render the entry, then parse the bytes back.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::edifact_renderer::render_to_wire_bytes;
use makod::party_registry::MpIdRegistry;

const GNB: &str = "9870000000009";
const LFN: &str = "9871111111116";
const MALO: &str = "51238696780";

fn registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: GNB.to_owned(),
            roles: vec!["GNB".to_owned()],
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

fn pid_of(bytes: &[u8]) -> u32 {
    use edi_energy::EdiEnergyMessage as _;
    edi_energy::parse(bytes)
        .expect("rendered bytes must parse as EDIFACT")
        .detect_pruefidentifikator()
        .expect("the rendered interchange must announce its Prüfidentifikator")
        .as_u32()
}

fn outbox(message_type: &str, recipient: &str, payload: serde_json::Value) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/geli-gas-test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        message_type,
        recipient,
        payload,
    )
}

/// The Bestätigung Anmeldung NN (44002) must render and re-parse as UTILMD.
#[test]
fn gnb_bestaetigung_renders_to_parseable_utilmd() {
    let msg = outbox(
        "UTILMD",
        LFN,
        serde_json::json!({
            "pid":           44002,
            "anfrage_pid":   44001,
            "accepted":      true,
            "malo":          MALO,
            "sender":        GNB,
            "receiver":      LFN,
            "document_date": "20261101",
            "process_date":  "20261101",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry())
        .expect("the GNB answer must have a renderer — it is a UTILMD interchange");

    assert_eq!(
        pid_of(&rendered.bytes),
        44002,
        "the wire message must carry the *answer* PID, not the Anfrage PID"
    );
}

/// The Ablehnung (44003) must render too — the rejection path is not special.
#[test]
fn gnb_ablehnung_renders_to_parseable_utilmd() {
    let msg = outbox(
        "UTILMD",
        LFN,
        serde_json::json!({
            "pid":           44003,
            "anfrage_pid":   44001,
            "accepted":      false,
            "reason":        "MaLo nicht im Netzgebiet",
            "malo":          MALO,
            "sender":        GNB,
            "receiver":      LFN,
            "document_date": "20261101",
            "process_date":  "20261101",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry()).expect("Ablehnung must render");
    assert_eq!(pid_of(&rendered.bytes), 44003);
}

/// A GNB-initiated Anfrage (44007 Abmeldung NN vom NB) must render.
#[test]
fn gnb_initiated_anfrage_renders_to_parseable_utilmd() {
    let msg = outbox(
        "UTILMD",
        LFN,
        serde_json::json!({
            "pid":           44007,
            "anfrage_pid":   44007,
            "malo":          MALO,
            "sender":        GNB,
            "receiver":      LFN,
            "document_date": "20261101",
            "process_date":  "20261101",
            "message_ref":   "MSG-001",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry()).expect("Anfrage must render");
    assert_eq!(pid_of(&rendered.bytes), 44007);
}

/// The old type names must stay unrenderable, so a reintroduction fails loudly.
///
/// This is the actual regression: the names looked plausible and the producer
/// compiled, but nothing downstream could act on them.
#[test]
fn the_old_intent_only_type_names_have_no_renderer() {
    for stale in ["UtilmdAnfrage", "UtilmdAntwort"] {
        let msg = outbox(
            stale,
            LFN,
            serde_json::json!({ "pid": 44002, "malo": MALO, "sender": GNB }),
        );
        assert!(
            render_to_wire_bytes(&msg, &registry()).is_err(),
            "{stale} must not gain a renderer — GeLi Gas emits plain `UTILMD` now, \
             and a message type only this crate understands cannot reach a partner"
        );
    }
}

// ── INSRPT ────────────────────────────────────────────────────────────────────

/// The WiM Störungsmeldung must render to an interchange that passes AHB
/// validation — not merely one that parses.
///
/// `mako-wim` emitted `INSRPT` outbox entries with no renderer behind them, so
/// the AS4 sender substituted raw domain JSON. Adding a renderer is only half
/// the fix: the INSRPT AHB marks `BGM`, `DOC`, `DTM`, `LIN`, `LOC`, `NAD`,
/// `RFF` and `STS` mandatory, and the builder emitted only three of them. This
/// asserts the full set by validating the result.
#[test]
fn wim_stoerungsmeldung_renders_to_ahb_valid_insrpt() {
    use edi_energy::EdiEnergyMessage as _;

    let msg = outbox(
        "INSRPT",
        LFN,
        serde_json::json!({
            "type":          "Stoerungsmeldung",
            "pid":           23001,
            "melo":          "DE00056266802AO6G56M11SN51G",
            "receiver":      LFN,
            "document_date": "20261101",
            "message_ref":   "MSG-INSRPT-1",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry())
        .expect("INSRPT must have a renderer — mako-wim enqueues it");

    assert_eq!(pid_of(&rendered.bytes), 23001);

    let parsed = edi_energy::parse(&rendered.bytes).expect("rendered INSRPT must parse");
    let report = parsed
        .validate_on_date(time::macros::date!(2026 - 11 - 01))
        .expect("validation runs");
    assert!(
        report.is_valid(),
        "rendered INSRPT must satisfy the AHB, got: {:?}",
        report
            .iter_issues()
            .map(|i| format!(
                "{:?} {} {}",
                i.severity,
                i.rule_id.clone().unwrap_or_default(),
                i.message
            ))
            .collect::<Vec<_>>()
    );
}

// ── MSCONS SG6 LOC qualifiers ─────────────────────────────────────────────────

/// The MaBiS Summenzeitreihe must name its Meldepunkt and its
/// Bilanzierungsgebiet under **different** LOC qualifiers.
///
/// MSCONS AHB 3.2 gives PIDs 13003/13023 three SG6 LOC qualifiers: `172`
/// Meldepunkt (the MaBiS-Zählpunkt), `107` Bilanzierungsgebiet, `237`
/// Bilanzkreis. mako emitted the Bilanzierungsgebiet EIC under `172` and no
/// `107` at all — telling the BIKO a 16-character territory code was the
/// Meldepunkt, and omitting the territory. The message still parsed and still
/// validated, because both fields are free text at the MIG level.
#[test]
fn summenzeitreihe_separates_meldepunkt_from_bilanzierungsgebiet() {
    const MABIS_ZP: &str = "DE0004030099000000000000000012345";
    const BILANZIERUNGSGEBIET: &str = "11YAPG4CTRDNZ--A";

    let msg = outbox(
        "MSCONS",
        LFN,
        serde_json::json!({
            "pid": 13003,
            "mabis_zp_id": MABIS_ZP,
            "bilanzierungsgebiet_id": BILANZIERUNGSGEBIET,
            "balancing_period": "202606",
            "version": "20260714050000+00",
            "sender_mp_id": GNB,
            "receiver_mp_id": LFN,
            "intervals": [
                { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
            ],
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry()).expect("13003 must render");
    let wire = String::from_utf8(rendered.bytes).expect("utf-8");

    assert!(
        wire.contains(&format!("LOC+172+{MABIS_ZP}")),
        "LOC+172 must carry the MaBiS-Zählpunkt:\n{wire}"
    );
    assert!(
        wire.contains(&format!("LOC+107+{BILANZIERUNGSGEBIET}")),
        "LOC+107 must carry the Bilanzierungsgebiet:\n{wire}"
    );
    assert!(
        !wire.contains(&format!("LOC+172+{BILANZIERUNGSGEBIET}")),
        "the Bilanzierungsgebiet must never appear as the Meldepunkt:\n{wire}"
    );
}

/// Passing the same value for both is refused at the boundary.
///
/// That is exactly the original defect — one identifier standing in for two —
/// and it is silent on the wire, so it has to fail before rendering.
#[test]
fn the_same_identifier_cannot_serve_as_both_loc_qualifiers() {
    const BOTH: &str = "11YAPG4CTRDNZ--A";
    let msg = outbox(
        "MSCONS",
        LFN,
        serde_json::json!({
            "pid": 13003,
            "mabis_zp_id": BOTH,
            "bilanzierungsgebiet_id": BOTH,
            "balancing_period": "202606",
            "version": "20260714050000+00",
            "intervals": [
                { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
            ],
        }),
    );
    assert!(
        render_to_wire_bytes(&msg, &registry()).is_err(),
        "the territory EIC standing in for the Meldepunkt must be refused"
    );
}

/// The rendered Summenzeitreihe must still satisfy the AHB.
///
/// The shipped MSCONS profile restricts SG6 `LOC` DE3227 to `172` — it was
/// imported before the `107`/`237` qualifiers were noticed — so emitting the
/// Bilanzierungsgebiet under `107` could be rejected by mako's own validator
/// even though the AHB permits it.
#[test]
fn the_rendered_summenzeitreihe_still_validates() {
    use edi_energy::EdiEnergyMessage as _;
    let msg = outbox(
        "MSCONS",
        LFN,
        serde_json::json!({
            "pid": 13003,
            "mabis_zp_id": "DE0004030099000000000000000012345",
            "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--A",
            "balancing_period": "202606",
            "version": "20260714050000+00",
            "sender_mp_id": GNB,
            "receiver_mp_id": LFN,
            "intervals": [
                { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
            ],
        }),
    );
    let rendered = render_to_wire_bytes(&msg, &registry()).expect("13003 must render");
    let parsed = edi_energy::parse(&rendered.bytes).expect("must parse");
    let report = parsed
        .validate_on_date(time::macros::date!(2026 - 06 - 30))
        .expect("validation runs");
    assert!(
        report.is_valid(),
        "rendered 13003 must satisfy the AHB, got: {:?}",
        report
            .iter_issues()
            .map(|i| format!(
                "{:?} {} {}",
                i.severity,
                i.rule_id.clone().unwrap_or_default(),
                i.message
            ))
            .collect::<Vec<_>>()
    );
}
