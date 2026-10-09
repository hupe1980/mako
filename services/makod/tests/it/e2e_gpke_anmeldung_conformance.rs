//! Every GPKE Anmeldung mako sends must pass its own AHB — and carry the
//! products the Codeliste attaches to the Geschäftsvorfall it states.
//!
//! `e2e_gpke_antwort_conformance.rs` does this for the answer side. The request
//! side has one rule the Prüfschablone cannot express, because it lives in the
//! Codeliste der Konfigurationen rather than in an AHB Bedingung: Kap. 6.1.1
//! makes the Produkt-Code `9991000002090` (Tranchengröße) „zwingend" on
//! `STS+7++xxx+ZW2`, the Geschäftsvorfall that builds a new Tranche. An
//! Anmeldung under `ZW2` carrying only the Bilanzkreis renders, parses and
//! validates clean — and the receiving NB still has to refuse it.
//!
//! So the rule is mako's own, and these tests are where it is stated.

use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::edifact_renderer::{RenderError, render_to_wire_bytes};
use makod::party_registry::MpIdRegistry;

const LFN: &str = "9900987654321";
const NB: &str = "9900357000004";
const MALO: &str = "51238696012";

fn registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[PartyConfig {
        mp_id: LFN.to_owned(),
        roles: vec!["LF".to_owned()],
        primary: true,
        agency: None,
    }])
    .expect("valid registry")
}

/// The Anmeldung payload `GpkeLfAnmeldungWorkflow` enqueues for an erzeugende
/// Marktlokation, with the Geschäftsvorfall and the Tranchengröße left to the
/// caller — the two the Codeliste ties together.
fn anmeldung(ergaenzung: &str, tranchengroesse: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "pid": 55077,
        "direction": "outbound",
        "sender": LFN,
        "receiver": NB,
        "malo": MALO,
        "process_date": "2026-11-01",
        "bilanzkreis": "11XBILANZKREIS1K",
        "transaktionsgrund": "E03",
        "transaktionsgrund_ergaenzung": ergaenzung,
        "tranchengroesse": tranchengroesse,
    })
}

fn render(payload: serde_json::Value) -> Result<String, RenderError> {
    let msg = OutboxMessage::new(
        StreamId::new("process/gpke-anmeldung-test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        "UTILMD",
        NB,
        payload,
    );
    let rendered = render_to_wire_bytes(&msg, &registry())?;
    let wire = String::from_utf8(rendered.bytes.clone()).expect("utf-8");
    let parsed = edi_energy::parse(&rendered.bytes).expect("rendered UTILMD must parse");
    let report = parsed.validate().expect("validation must run");
    assert!(
        report.errors().is_empty(),
        "55077 does not pass its own AHB:\n{}\n{}",
        wire.replace('\'', "'\n"),
        report
            .errors()
            .iter()
            .map(|i| format!("  [{}] {}", i.rule_id.as_deref().unwrap_or("-"), i.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    Ok(wire)
}

/// Geschäftsvorfall 1 („vollständige (100%ige) Zuordnung") has no Tranche and
/// therefore no Tranchengröße: one `SEQ+Z79` group carrying the Bilanzkreis.
#[test]
fn a_hundred_percent_anmeldung_carries_the_bilanzkreis_alone() {
    let wire = render(anmeldung("ZW0", serde_json::Value::Null)).expect("renders");
    assert!(wire.contains("PIA+5+9991000002082:Z11"), "{wire}");
    assert!(
        !wire.contains("9991000002090"),
        "a 100 % Zuordnung builds no Tranche:\n{wire}"
    );
}

/// Geschäftsvorfall 3 („der LFN wird einer neu zu bildenden Tranche
/// zugeordnet") adds the Tranchengröße to the same Produktpaket-ID.
///
/// Each product needs its **own** `SG8 SEQ+Z79` group: the MIG gives the group
/// a single `PIA` place, so a second Produkt-Code inside one group has nowhere
/// to sit. What binds them into a package is the repeated Produktpaket-ID.
#[test]
fn a_tranchenbildung_carries_the_tranchengroesse_in_its_own_seq_group() {
    let wire = render(anmeldung("ZW2", serde_json::json!("33.33"))).expect("renders");
    assert_eq!(
        wire.matches("SEQ+Z79+1").count(),
        2,
        "one SEQ+Z79 group per product, both under Produktpaket-ID 1:\n{wire}"
    );
    assert!(wire.contains("PIA+5+9991000002082:Z11"), "{wire}");
    assert!(wire.contains("PIA+5+9991000002090:Z11"), "{wire}");
    // `CAV+ZH9` names which of the three Produkteigenschaften it is; only the
    // prozentuale Aufteilung is a share `E_0623` can add up.
    assert!(wire.contains("CAV+ZH9:::9991000003014"), "{wire}");
    assert!(wire.contains("CAV+ZV4:::33.33"), "{wire}");
}

/// The Aufteilungsfaktor and the Technische-Ressourcen forms travel under the
/// same Produkt-Code and a different Produkteigenschaft.
#[test]
fn the_other_two_tranchengroessen_state_their_own_eigenschaft() {
    let faktor = render(anmeldung(
        "ZW2",
        serde_json::json!({"art": "aufteilungsfaktor", "wert": "0.25"}),
    ))
    .expect("renders");
    assert!(faktor.contains("CAV+ZH9:::9991000003022"), "{faktor}");

    let tr = render(anmeldung(
        "ZW2",
        serde_json::json!({"art": "technische_ressourcen", "wert": "TR-1"}),
    ))
    .expect("renders");
    assert!(tr.contains("CAV+ZH9:::9991000003220"), "{tr}");
}

/// A Geschäftsvorfall 3 without a Tranchengröße is refused rather than sent.
/// The NB has to reject it, and a rejection costs the LF the Anmeldefrist.
#[test]
fn a_tranchenbildung_without_a_tranchengroesse_is_refused() {
    match render_to_wire_bytes(
        &OutboxMessage::new(
            StreamId::new("process/x"),
            ProcessId::new(),
            TenantId::new(),
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            "UTILMD",
            NB,
            anmeldung("ZW2", serde_json::Value::Null),
        ),
        &registry(),
    ) {
        Err(RenderError::MissingField { field, .. }) => {
            assert!(field.contains("tranchengroesse"), "{field}");
        }
        other => panic!("a ZW2 Anmeldung without a Tranchengröße must not render: {other:?}"),
    }
}

/// Codeliste der Konfigurationen 1.4 Kap. 6.1.1 bounds the prozentuale
/// Aufteilung with `[914] ∧ [930] ∧ [955]` — greater than zero, at most two
/// decimals, less than 100. Both bounds are strict: a 100 % Zuordnung is
/// Geschäftsvorfall 1 and carries no Tranchengröße at all.
#[test]
fn the_prozentuale_tranchengroesse_is_held_to_its_bedingungen() {
    for wert in ["0", "0.00", "100", "100.01", "33.333", "abc", ""] {
        let r = render_to_wire_bytes(
            &OutboxMessage::new(
                StreamId::new("process/x"),
                ProcessId::new(),
                TenantId::new(),
                CorrelationId::new(),
                ConversationId::new(),
                EventId::new(),
                "UTILMD",
                NB,
                anmeldung("ZW2", serde_json::json!(wert)),
            ),
            &registry(),
        );
        assert!(
            matches!(r, Err(RenderError::MissingField { .. })),
            "{wert:?} is not a Tranchengröße the NB can apply"
        );
    }
    for wert in ["0.01", "33.33", "50", "99.99"] {
        assert!(
            render(anmeldung("ZW2", serde_json::json!(wert))).is_ok(),
            "{wert:?} is a lawful share"
        );
    }
}
