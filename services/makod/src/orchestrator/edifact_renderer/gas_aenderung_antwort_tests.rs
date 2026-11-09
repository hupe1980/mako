//! The GeLi Gas Antwort auf Änderung, rendered and read back.
//!
//! Every Antwort column of UTILMD AHB Gas 1.2 Kap. 5.10.1 and 5.11.1–5.11.6
//! makes `SG4 STS+E01` Muss, its DE 1131 the column's own Codeliste, beside the
//! `STS+7` Transaktionsgrund and the `SG6 RFF+TN` of the Änderung; the 44176
//! alone also restates `DTM+92` and `DTM+157`. Each test drives the workflow
//! from the inbound Änderung to its Antwort and holds the rendered UTILMD to
//! the column the pre-send gate checks it against.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::{OutboxMessage, PendingOutbox};
use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
use mako_engine::workflow::Workflow;
use mako_geli_gas::stammdatenaenderung::{STAMMDATEN_PAIRS, antwort_codeliste};
use mako_geli_gas::{
    GasStammdatenCommand, GasStammdatenState, GeliGasStammdatenaenderungWorkflow as W,
};

/// DVGW-Codenummern: a Gas UTILMD admits NAD DE 3055 `9` or `332` only.
const VERANTWORTLICHER: &str = "9870012345678";
const BERECHTIGTER: &str = "9807317000007";
const MALO: &str = "51238696012";

/// The Antwort the workflow queues for the Änderung `aenderung_pid` answered
/// with `code`.
fn antwort(aenderung_pid: u32, code: &str) -> PendingOutbox {
    let received = W::handle(
        &GasStammdatenState::New,
        GasStammdatenCommand::ReceiveAenderung {
            pid: Pruefidentifikator::new(aenderung_pid).expect("pid"),
            sender: MarktpartnerCode::new(VERANTWORTLICHER),
            receiver: MarktpartnerCode::new(BERECHTIGTER),
            location_id: MaLo::new(MALO),
            aenderungsdatum: "20261201".to_owned(),
            patch: serde_json::json!({}),
            vertragsbeginn: Some("20250101".to_owned()),
            vorgangsnummer: Some("AEND-VG-1".to_owned()),
            dokumentenart: Some("E03".to_owned()),
            message_ref: MessageRef::new("AEND-1"),
            validation_passed: true,
            validation_errors: Vec::new(),
            received_at: time::macros::datetime!(2026-11-09 10:00 UTC),
        },
    )
    .expect("the Änderung is received");
    let state = received
        .events
        .iter()
        .fold(GasStammdatenState::New, W::apply);
    let out = W::handle(
        &state,
        GasStammdatenCommand::SendAntwort {
            antwort_code: code.to_owned(),
        },
    )
    .expect("the Antwort is queued");
    out.outbox
        .into_iter()
        .find(|o| o.message_type.as_ref() == "UTILMD")
        .expect("a UTILMD Antwort")
}

fn wire(pending: &PendingOutbox) -> String {
    let msg = OutboxMessage::new(
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
    let rendered = super::utilmd::render_utilmd(&pending.payload, &msg).expect("renders");
    String::from_utf8_lossy(&rendered.bytes).into_owned()
}

/// The `STS+E01` DE 1131 codes the active Gas profile lists for `pid`.
fn column_codelisten(pid: u32) -> Vec<String> {
    let profile = edi_energy::ReleaseRegistry::global()
        .profile_for_date_and_track(
            edi_energy::MessageType::Utilmd,
            mako_fristen::heute(),
            edi_energy::ReleaseTrack::Gas,
        )
        .expect("the active Gas profile");
    let af = profile.anwendungsfall(pid).expect("the Antwort column");
    af.elements
        .iter()
        .filter(|e| e.de == "1131")
        .flat_map(|e| e.operands.iter().filter_map(|o| o.code.clone()))
        .collect()
}

/// Every Antwort auf Änderung carries its code and its column's Codeliste in
/// `SG4 STS+E01`, and passes the profile the pre-send gate checks.
#[test]
fn every_antwort_auf_aenderung_renders_its_code_and_passes_its_column() {
    for (aenderung, antwort_pid, _) in STAMMDATEN_PAIRS {
        let codeliste = antwort_codeliste(*aenderung).expect("a Codeliste");
        assert!(
            column_codelisten(*antwort_pid)
                .iter()
                .any(|c| c == codeliste),
            "{antwort_pid}: the column does not name {codeliste}"
        );
        let pending = antwort(*aenderung, "E15");
        let text = wire(&pending);
        assert!(
            text.contains(&format!("STS+E01++E15:{codeliste}'")),
            "{aenderung} → {antwort_pid}: {text}"
        );
        assert!(text.contains("RFF+TN:AEND-VG-1'"), "{antwort_pid}: {text}");
        let findings =
            crate::orchestrator::edifact_renderer::utilmd_ahb_findings(&pending, TenantId::new())
                .expect("the Antwort renders and parses");
        assert!(
            findings.is_empty(),
            "{aenderung} → {antwort_pid}: {findings:#?}"
        );
    }
}

/// The 44176 restates both dates of the Änderung der Marktlokationsstruktur,
/// `DTM+92` before `DTM+157` as the MIG orders them, each the UTC instant the
/// Gastag begins (06:00 gesetzliche Zeit; Allgemeine Festlegungen V6.1d Kap.
/// 3); every other Antwort names no date.
#[test]
fn only_the_44176_restates_the_dates_in_mig_order() {
    let text = wire(&antwort(44175, "E15"));
    let beginn = text
        .find("DTM+92:202501010500?+00:303'")
        .unwrap_or_else(|| panic!("DTM+92 at the Gastag boundary: {text}"));
    let aenderung = text
        .find("DTM+157:202612010500?+00:303'")
        .unwrap_or_else(|| panic!("DTM+157 at the Gastag boundary: {text}"));
    assert!(beginn < aenderung, "{text}");
    assert!(text.contains("STS+7++ZI9'"), "{text}");

    let text = wire(&antwort(44112, "E15"));
    assert!(!text.contains("DTM+92"), "{text}");
    assert!(!text.contains("DTM+157"), "{text}");
    assert!(text.contains("STS+7++ZE7'"), "{text}");
}

/// An Ablehnung renders from its own column's list: `E13` is `G_0025`'s, on
/// the NB's 44121.
#[test]
fn an_ablehnung_renders_from_its_columns_codeliste() {
    let pending = antwort(44120, "E13");
    let text = wire(&pending);
    assert!(text.contains("STS+E01++E13:G_0025'"), "{text}");
    let findings =
        crate::orchestrator::edifact_renderer::utilmd_ahb_findings(&pending, TenantId::new())
            .expect("renders");
    assert!(findings.is_empty(), "{findings:#?}");
}

/// The outbound Änderung carries both Muss dates, `DTM+92` then `DTM+157`.
#[test]
fn an_aenderung_carries_both_dates() {
    let out = W::handle(
        &GasStammdatenState::New,
        GasStammdatenCommand::SendAenderung {
            pid: Pruefidentifikator::new(44109).expect("pid"),
            sender: MarktpartnerCode::new(BERECHTIGTER),
            receiver: MarktpartnerCode::new(VERANTWORTLICHER),
            location_id: MaLo::new(MALO),
            aenderungsdatum: "20261215".to_owned(),
            vertragsbeginn: "20250101".to_owned(),
            stammdaten: mako_engine::payload::Stammdaten {
                sg6: vec![
                    mako_engine::payload::StammdatenSegment {
                        tag: "RFF".to_owned(),
                        elemente: vec![vec!["Z18".to_owned(), MALO.to_owned()]],
                    },
                    mako_engine::payload::StammdatenSegment {
                        tag: "DTM".to_owned(),
                        elemente: vec![vec!["Z20".to_owned(), "12".to_owned(), "802".to_owned()]],
                    },
                ],
                ..mako_engine::payload::Stammdaten::default()
            },
        },
    )
    .expect("the Änderung is queued");
    let text = wire(&out.outbox[0]);
    let beginn = text.find("DTM+92:202501010500?+00:303'").expect(&text);
    let aenderung = text.find("DTM+157:202612150500?+00:303'").expect(&text);
    assert!(beginn < aenderung, "{text}");
    assert!(text.contains("STS+7++ZE6'"), "{text}");
}
