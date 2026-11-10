//! The pre-send check of a Neuanlage answer, run in the release its process
//! renders in.

use mako_engine::ids::TenantId;
use mako_engine::types::{MaLo, MarktpartnerCode, Pruefidentifikator};
use mako_engine::version::FormatVersion;
use mako_engine::workflow::Workflow as _;
use mako_gpke::{GpkeNeuanlageWorkflow, NeuanlageCommand, NeuanlageState};

const LF: &str = "9900000000002";
const NB: &str = "9900357000004";

/// The Bestätigung 55602 a Neuanlage process answers with — pauschal, so it
/// names no Messlokation.
fn bestaetigung_55602() -> mako_engine::outbox::PendingOutbox {
    let data = mako_gpke::neuanlage::NeuanlageData {
        location_id: MaLo::new(""),
        sender: MarktpartnerCode::new(LF),
        receiver: MarktpartnerCode::new(NB),
        document_date: "20260915".to_owned(),
        process_date: "20261201".to_owned(),
        pruefidentifikator: Pruefidentifikator::const_new(55_600),
        vorgangsnummer: Some("NEU-1".to_owned()),
        transaktionsgrund: Some("E02".to_owned()),
        transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
        produktpaket_id: Some("1".to_owned()),
    };
    let mut antwort = mako_gpke::LfAntwort::zustimmung("A09", "E_0608");
    antwort.malo_art = Some("ZW6".to_owned());
    let out = GpkeNeuanlageWorkflow::handle(
        &NeuanlageState::ValidationPassed(data),
        NeuanlageCommand::SendAntwort {
            antwort,
            marktlokation: Some(MaLo::new("51238696012")),
            tranche: None,
        },
    )
    .expect("the Bestätigung is answered");
    out.outbox
        .into_iter()
        .find(|p| &*p.message_type == "UTILMD")
        .expect("one UTILMD")
}

/// A process keeps the format version it started under, and the outbox
/// worker renders its answer in that version's release — so the door's
/// check renders it there too, not in the release in force on the day the
/// answer is posted. A Neuanlage that arrived on 15.09.2026 runs under
/// FV2026-04-01, whose UTILMD Strom release is S2.1; one that arrived after
/// the 01.10.2026 cutover runs under S2.2. Each passes its own column.
#[test]
fn the_gate_renders_an_answer_in_the_release_of_its_process() {
    let pending = bestaetigung_55602();
    for (fv, version) in [("FV2026-04-01", "S2.1"), ("FV2026-10-01", "S2.2")] {
        let check = crate::orchestrator::edifact_renderer::utilmd_gate_check(
            &pending,
            TenantId::from_party_id(NB),
            &FormatVersion::parse(fv).expect("a format version"),
        )
        .expect("the answer renders");
        let wire = String::from_utf8(check.bytes).expect("UNOC text");
        assert!(wire.contains(&format!(":{version}'")), "{fv}: {wire}");
        assert!(check.findings.is_empty(), "{fv}: {:#?}", check.findings);
    }
}
