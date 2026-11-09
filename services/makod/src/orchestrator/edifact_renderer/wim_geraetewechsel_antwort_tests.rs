//! The WiM Antwort Gerätewechselabsicht and Antwort Bestellung
//! Geräteübernahme, driven from the ORDERS 17009 resp. 17001 and rendered as
//! the ORDRSP the outbox worker sends.
//!
//! ORDRSP AHB 1.1b Kap. 4.10 (S. 45–47) puts `ZB4`/`ZB5` on 19015 („Bestätigung
//! Gerätewechselabsicht", Bedingung `[38]`) and the refusals `E17`/`Z07` on
//! 19016 („Ablehnung Gerätewechselabsicht", `[39]`); `BGM` DE 1001 is `Z13`,
//! `SG1 RFF+ON` names the ORDERS by its `BGM` DE 1004 (`[535]`), and `SG2 AJT`
//! carries the code with its Codeliste. Beside `ZB5` the 19015 names the
//! „Lieferadresse für Altgeräte" in `SG3 NAD+Z22` (`[41]`, S. 48), beside `Z07`
//! the 19016 the Eigentümer in `SG3 NAD+VY` (`[42]`, S. 47). Kap. 4.9.2
//! (S. 43–45) makes `DTM+203` Ausführungsdatum and the `SG6 CTA`/`COM`
//! Ansprechpartner Muss on 19001/19002. Each test holds the rendered message to
//! the profile the pre-send gate checks it against.

use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::{OutboxMessage, PendingOutbox};
use mako_engine::types::{
    DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator, Sparte,
};
use mako_engine::workflow::Workflow;
use mako_wim::geraeteubernahme::{
    Ansprechpartner, AntwortAngaben, Kommunikationsweg, Lieferadresse,
};
use mako_wim::{GeraeteubernahmeCommand, GeraeteubernahmeState, WimGeraeteubernahmeWorkflow as W};

use crate::config::PartyConfig;
use crate::party_registry::MpIdRegistry;

const MELO: &str = "DE0000000001234567890000000000001";
const ORDERS_NUMMER: &str = "GWA-4711";

/// The MSBN and MSBA MP-IDs of `sparte` — a DVGW-Codenummer for Gas, so the
/// `NAD` DE 3055 is one the Gas column admits.
fn parteien(sparte: Sparte) -> (&'static str, &'static str) {
    match sparte {
        Sparte::Strom => ("9900357000004", "9900555000005"),
        Sparte::Gas => ("9870012345678", "9807317000007"),
    }
}

fn receive(sparte: Sparte, termin: &str) -> (GeraeteubernahmeState, Vec<PendingOutbox>) {
    receive_orders(17_009, sparte, termin)
}

fn receive_orders(
    pid: u32,
    sparte: Sparte,
    termin: &str,
) -> (GeraeteubernahmeState, Vec<PendingOutbox>) {
    let (msbn, msba) = parteien(sparte);
    let out = W::handle(
        &GeraeteubernahmeState::New,
        GeraeteubernahmeCommand::ReceiveOrders {
            pid: Pruefidentifikator::new(pid).expect("pid"),
            sender: MarktpartnerCode::new(msbn),
            receiver: MarktpartnerCode::new(msba),
            melo_id: MeLo::new(MELO),
            device_id: DeviceId::new("1ESY1161234567"),
            document_date: "20261109".to_owned(),
            termin: Some(termin.to_owned()),
            message_ref: MessageRef::new("UNH-17009"),
            orders_nummer: ORDERS_NUMMER.to_owned(),
            validation_passed: true,
            validation_errors: Vec::new(),
            sparte,
            // Mon 2026-11-09: the 4. Werktag after it is Fri 2026-11-13.
            received_at: time::macros::datetime!(2026-11-09 09:00 UTC),
        },
    )
    .expect("the ORDERS is received");
    let state = out.events.iter().fold(GeraeteubernahmeState::New, W::apply);
    (state, out.outbox)
}

fn wire(pending: &PendingOutbox, sparte: Sparte) -> String {
    wire_under(pending, sparte, None)
}

/// [`wire`] for an answer whose process runs under `format_version`.
fn wire_under(
    pending: &PendingOutbox,
    sparte: Sparte,
    format_version: Option<mako_engine::version::FormatVersion>,
) -> String {
    let (_, msba) = parteien(sparte);
    let registry = MpIdRegistry::from_config(&[PartyConfig {
        mp_id: msba.to_owned(),
        roles: vec!["MSB".to_owned()],
        primary: true,
        agency: None,
        as4: None,
    }])
    .expect("registry");
    let mut msg = OutboxMessage::new(
        StreamId::new("process/test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        pending.message_type.clone(),
        pending.recipient.clone(),
        pending.payload.clone(),
    );
    msg.format_version = format_version;
    let rendered = super::render_to_wire_bytes(&msg, &registry).expect("the ORDRSP renders");
    String::from_utf8(rendered.bytes).expect("utf-8")
}

/// The profile findings on `text`, judged on today's date as the gate does.
fn findings(text: &str) -> Vec<String> {
    edi_energy::parse(text.as_bytes())
        .expect("the ORDRSP parses")
        .validate_on_date(mako_fristen::heute())
        .expect("a profile is registered")
        .errors()
        .iter()
        .map(|e| e.message.to_string())
        .collect()
}

/// A Gerätewechseltermin inside the Mindestvorlaufzeit is answered with an
/// ORDRSP 19016 carrying `E17` — not with an APERAK Fehlermeldung — and the
/// ORDRSP passes its column in both Sparten.
#[test]
fn an_e17_refusal_renders_an_ordrsp_19016_that_passes_its_column() {
    for (sparte, codeliste) in [(Sparte::Strom, "S_0066"), (Sparte::Gas, "G_0060")] {
        // Wed 2026-11-11 is the 2. Werktag after the Anzeige: two too early.
        let (state, outbox) = receive(sparte, "20261111");
        assert!(state.is_terminal(), "{sparte}: {}", state.status_str());
        let ordrsp = outbox
            .iter()
            .find(|o| &*o.message_type == "ORDRSP")
            .unwrap_or_else(|| panic!("{sparte}: an ORDRSP answers the E17, got {outbox:?}"));
        let text = wire(ordrsp, sparte);
        assert!(text.contains("BGM+Z13+"), "{sparte}: {text}");
        assert!(text.contains("RFF+Z13:19016'"), "{sparte}: {text}");
        assert!(
            text.contains(&format!("RFF+ON:{ORDERS_NUMMER}'")),
            "{sparte}: {text}"
        );
        assert!(
            text.contains(&format!("AJT+E17+{codeliste}'")),
            "{sparte}: {text}"
        );
        let found = findings(&text);
        assert!(found.is_empty(), "{sparte}: {found:#?}\n{text}");
    }
}

/// `ZB4` rides 19015 and passes its column; the Gerätewechsel goes ahead.
#[test]
fn a_zb4_answer_renders_an_ordrsp_19015_that_passes_its_column() {
    for (sparte, codeliste) in [(Sparte::Strom, "S_0065"), (Sparte::Gas, "G_0059")] {
        let (state, _) = receive(sparte, "20261201");
        let out = W::handle(
            &state,
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "ZB4".to_owned(),
                bemerkung: None,
                angaben: AntwortAngaben::default(),
            },
        )
        .expect("ZB4");
        let text = wire(&out.outbox[0], sparte);
        assert!(text.contains("RFF+Z13:19015'"), "{sparte}: {text}");
        assert!(
            text.contains(&format!("AJT+ZB4+{codeliste}'")),
            "{sparte}: {text}"
        );
        let found = findings(&text);
        assert!(found.is_empty(), "{sparte}: {found:#?}\n{text}");
    }
}

/// The column check is not vacuous: the same 19016 without the ORDERS'
/// `BGM` DE 1004 has no `SG1 RFF+ON`, which the column marks Muss.
#[test]
fn a_19016_without_the_orders_nummer_fails_its_column() {
    let (_, outbox) = receive(Sparte::Strom, "20261111");
    let mut ordrsp = outbox
        .into_iter()
        .find(|o| &*o.message_type == "ORDRSP")
        .expect("an ORDRSP");
    ordrsp.payload["order_reference"] = serde_json::Value::String(String::new());
    let text = wire(&ordrsp, Sparte::Strom);
    assert!(!text.contains("RFF+ON"), "{text}");
    assert!(!findings(&text).is_empty(), "{text}");
}

/// The answer to `orders_pid`'s ORDERS with `code`, and what the MSBA states
/// beside it: its Ansprechpartner on a Bestellung, the Lieferadresse beside
/// `ZB5`, the Eigentümer beside `Z07`.
fn antwort(orders_pid: u32, sparte: Sparte, code: &str) -> PendingOutbox {
    let (state, _) = receive_orders(orders_pid, sparte, "20261201");
    let eigentuemer = match sparte {
        Sparte::Strom => "9900555000005",
        Sparte::Gas => "9807317000007",
    };
    let out = W::handle(
        &state,
        GeraeteubernahmeCommand::DispatchAntwort {
            antwort_code: code.to_owned(),
            bemerkung: None,
            angaben: AntwortAngaben {
                ansprechpartner: (orders_pid == 17_001).then(|| Ansprechpartner {
                    name: "Technik MSBA".to_owned(),
                    kommunikation: vec![
                        Kommunikationsweg::Email("technik@msba.example".to_owned()),
                        Kommunikationsweg::Telefon("+4922112345".to_owned()),
                    ],
                }),
                lieferadresse: (code == "ZB5").then(|| Lieferadresse {
                    name: "MSBA Lager GmbH".to_owned(),
                    strasse: "Lagerweg 1".to_owned(),
                    ort: "Bonn".to_owned(),
                    postleitzahl: Some("53111".to_owned()),
                    land: "DE".to_owned(),
                }),
                eigentuemer: (code == "Z07").then(|| MarktpartnerCode::new(eigentuemer)),
            },
        },
    )
    .unwrap_or_else(|e| panic!("{sparte} {code}: {e}"));
    out.outbox
        .into_iter()
        .find(|o| &*o.message_type == "ORDRSP")
        .expect("an ORDRSP")
}

/// `ZB5` — the MSBN removes the devices — rides 19015 with the Lieferadresse
/// für Altgeräte in `SG3 NAD+Z22` (`[41]`), and passes its column.
#[test]
fn a_zb5_answer_names_the_lieferadresse_and_passes_its_column() {
    for sparte in [Sparte::Strom, Sparte::Gas] {
        let text = wire(&antwort(17_009, sparte, "ZB5"), sparte);
        assert!(text.contains("RFF+Z13:19015'"), "{sparte}: {text}");
        assert!(
            text.contains("NAD+Z22+++MSBA Lager GmbH:::::Z02+Lagerweg 1+Bonn++53111+DE'"),
            "{sparte}: {text}"
        );
        let found = findings(&text);
        assert!(found.is_empty(), "{sparte}: {found:#?}\n{text}");
    }
}

/// `Z07` — Ablehnung (Keine Berechtigung) — rides 19016 with the Eigentümer's
/// MP-ID in `SG3 NAD+VY` (`[42]`), and passes its column.
#[test]
fn a_z07_refusal_names_the_eigentuemer_and_passes_its_column() {
    for (sparte, nad) in [
        (Sparte::Strom, "NAD+VY+9900555000005::293'"),
        (Sparte::Gas, "NAD+VY+9807317000007::332'"),
    ] {
        let text = wire(&antwort(17_009, sparte, "Z07"), sparte);
        assert!(text.contains("RFF+Z13:19016'"), "{sparte}: {text}");
        assert!(text.contains(nad), "{sparte}: {text}");
        let found = findings(&text);
        assert!(found.is_empty(), "{sparte}: {found:#?}\n{text}");
    }
}

/// 19001 and 19002 state the Ausführungsdatum of the ORDERS 17001 they answer
/// in `DTM+203` — the German day 01.12.2026 begins at 23:00 UTC the evening
/// before in Strom, at 05:00 UTC in Gas — and the MSBA's Ansprechpartner in
/// `SG6 CTA`/`COM`; each passes its column in both Sparten.
#[test]
fn a_bestellung_answer_carries_the_ausfuehrungsdatum_and_passes_its_column() {
    for (sparte, zustimmung, ablehnung, dtm) in [
        (Sparte::Strom, "Z13", "Z32", "DTM+203:202611302300?+00:303'"),
        (Sparte::Gas, "Z13", "Z32", "DTM+203:202612010500?+00:303'"),
    ] {
        for (code, pid) in [(zustimmung, "19001"), (ablehnung, "19002")] {
            let text = wire(&antwort(17_001, sparte, code), sparte);
            assert!(
                text.contains(&format!("RFF+Z13:{pid}'")),
                "{sparte}: {text}"
            );
            assert!(text.contains(dtm), "{sparte} {pid}: {text}");
            // `+` is the EDIFACT element separator, so the `[940]` „+" goes
            // out released as `?+`.
            assert!(
                text.contains(
                    "NAD+MS+9900555000005::293'CTA+IC+:Technik MSBA'\
                     COM+technik@msba.example:EM'COM+?+4922112345:TE'NAD+MR+"
                ) || text.contains(
                    "NAD+MS+9807317000007::332'CTA+IC+:Technik MSBA'\
                     COM+technik@msba.example:EM'COM+?+4922112345:TE'NAD+MR+"
                ),
                "{sparte} {pid}: {text}"
            );
            let found = findings(&text);
            assert!(found.is_empty(), "{sparte} {pid}: {found:#?}\n{text}");
        }
    }
}

/// An answer goes out in the release of its process's format version, not in
/// the one in force on the day it is sent: a 19015 answering a Gerätewechsel-
/// absicht received under the format version in force on 15.09.2026 is an
/// ORDRSP 1.4b (fv20260401), and passes that profile.
#[test]
fn an_ordrsp_renders_in_the_release_of_its_process() {
    let fv = edi_energy::registry::ReleaseRegistry::global()
        .format_versions_in_force_on(time::macros::date!(2026 - 09 - 15))
        .into_iter()
        .filter_map(|s| mako_engine::version::FormatVersion::parse(&s).ok())
        .max()
        .expect("a format version in force");
    let text = wire_under(
        &antwort(17_009, Sparte::Strom, "ZB4"),
        Sparte::Strom,
        Some(fv),
    );
    assert!(text.contains("UNH+"), "{text}");
    assert!(text.contains(":D:10A:UN:1.4b'"), "{text}");
    let found: Vec<String> = edi_energy::parse(text.as_bytes())
        .expect("the ORDRSP parses")
        .validate_on_date(time::macros::date!(2026 - 09 - 15))
        .expect("a profile is registered")
        .errors()
        .iter()
        .map(|e| e.message.to_string())
        .collect();
    assert!(found.is_empty(), "{found:#?}\n{text}");
}
