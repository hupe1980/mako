//! The GPKE Lieferant's Anfragen go out as messages their AHB column accepts,
//! to the party the column names (UTILMD AHB Strom 2.2).
//!
//! The Abmeldung 55004 makes `SG4 STS+7` Muss with its Ergänzung (Kap. 5.4,
//! S. 165–166); the Kündigung 55016 is „LFN an LFA" — it goes to the
//! Altlieferant, never the grid operator.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;

use super::*;

const LF_MP_ID: &str = "9900555000003";
const NB_MP_ID: &str = "9900357000004";
const LFA_MP_ID: &str = "9900555000010";
const MALO_ID: &str = "51238696012";

async fn lf_state() -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let state = CommandsApiState {
        tenant_id: TenantId::from_party_id(LF_MP_ID),
        sender_party_id: LF_MP_ID.to_owned(),
        configured_marktrollen: vec!["LF".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: None,
    };
    let record: energy_api::models::electricity::MaloIdentResultPositive =
        serde_json::from_value(serde_json::json!({
            "dataMarketLocation": {
                "maloId": MALO_ID,
                "energyDirection": "consumption",
                "measurementTechnologyClassification": "conventionalMeasuringSystem",
                "optionalChangeForecastBasis": "notPossible",
                "dataMarketLocationProperties": [],
                "dataMarketLocationNetworkOperators": [
                    { "marketPartnerId": NB_MP_ID, "executionTimeFrom": "2000-01-01T00:00:00Z" }
                ],
                "dataMarketLocationTransmissionSystemOperators": []
            }
        }))
        .expect("MaLo fixture matches the energy-api schema");
    state
        .malo_cache
        .upsert(&state.tenant_id.to_string(), &record)
        .await
        .expect("seed MaLo cache");
    state
}

/// The UTILMD outbox entries, as the outbox worker reads them.
async fn utilmd(state: &CommandsApiState) -> Vec<mako_engine::outbox::OutboxMessage> {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == "UTILMD")
        .collect()
}

/// Render an outbox entry as the LF sends it.
fn wire(msg: &mako_engine::outbox::OutboxMessage) -> String {
    let registry = crate::core::party_registry::MpIdRegistry::from_config(&[
        crate::core::config::PartyConfig {
            mp_id: LF_MP_ID.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        },
    ])
    .expect("registry");
    let rendered = crate::orchestrator::edifact_renderer::render_to_wire_bytes(msg, &registry)
        .unwrap_or_else(|e| panic!("the UTILMD renders: {e}"));
    rendered.bytes.iter().map(|&b| char::from(b)).collect()
}

/// What the profile in force says about the one UTILMD `body` makes `command`
/// write, and the wire: empty findings when it passes.
async fn door(command: &str, body: &serde_json::Value) -> (Vec<String>, String, String) {
    let state = lf_state().await;
    if let Err(e) = dispatch_command(&state, command, body).await {
        return (
            vec![format!("{command} refused: {e:?}")],
            String::new(),
            String::new(),
        );
    }
    let sent = utilmd(&state).await;
    assert_eq!(sent.len(), 1, "{command}: one UTILMD");
    let edi = wire(&sent[0]);
    let parsed = edi_energy::parse(&edi.chars().map(|c| c as u8).collect::<Vec<u8>>())
        .expect("the UTILMD parses back");
    let report = edi_energy::EdiEnergyMessage::validate_on_date(&parsed, mako_fristen::heute())
        .expect("validates");
    let mut found: Vec<String> = report
        .errors()
        .iter()
        .map(|e| e.message.to_string())
        .collect();
    if !found.is_empty() {
        found.push(edi.replace('\'', "'\n"));
    }
    (found, sent[0].recipient.to_string(), edi)
}

/// The Abmeldung 55004 the door writes passes its column, under an Ende-Grund
/// and under an Aufhebung, which names the confirmed Beginn instead (`[7]`).
#[tokio::test]
async fn a_55004_from_the_door_passes_its_profile() {
    for (grund, ergaenzung, date) in [
        ("ZT5", "ZW4", "DTM+93:"),
        ("E01", "ZW4", "DTM+93:"),
        ("ZT5", "ZW3", "DTM+93:"),
        ("ZH2", "ZW4", "DTM+92:"),
    ] {
        let body = serde_json::json!({
            "malo_id": MALO_ID,
            "lieferende_datum": "2026-12-31",
            "transaktionsgrund": grund,
            "transaktionsgrund_ergaenzung": ergaenzung,
        });
        let (found, recipient, edi) = door("gpke.lieferende.anmelden", &body).await;
        assert!(
            found.is_empty(),
            "55004 {grund}/{ergaenzung}:\n{}",
            found.join("\n")
        );
        assert_eq!(recipient, NB_MP_ID);
        assert!(
            edi.contains(&format!("STS+7++{grund}+{ergaenzung}'")),
            "{edi}"
        );
        assert!(edi.contains(date), "{grund}: {edi}");
    }
}

/// A 55004 without its Transaktionsgrund, or with a code the column does not
/// admit, is refused and nothing is written.
#[tokio::test]
async fn a_55004_needs_a_grund_its_column_admits() {
    let state = lf_state().await;
    for body in [
        serde_json::json!({ "malo_id": MALO_ID, "lieferende_datum": "2026-12-31" }),
        serde_json::json!({
            "malo_id": MALO_ID, "lieferende_datum": "2026-12-31", "transaktionsgrund": "E03"
        }),
        serde_json::json!({
            "malo_id": MALO_ID, "lieferende_datum": "2026-12-31", "transaktionsgrund": "ZT5",
            "transaktionsgrund_ergaenzung": "ZW0"
        }),
    ] {
        let refused = dispatch_command(&state, "gpke.lieferende.anmelden", &body).await;
        assert!(
            matches!(&refused, Err(DispatchError::InvalidPayload(_))),
            "{body}: {refused:?}"
        );
    }
    assert!(utilmd(&state).await.is_empty());
}

/// Without a stated Ergänzung the 55004 names the Lokationsart the NB's
/// MaLo-Ident record gives — `consumption` is the verbrauchende `ZW4`.
#[tokio::test]
async fn an_unstated_ergaenzung_is_the_malos_energy_direction() {
    let body = serde_json::json!({
        "malo_id": MALO_ID, "lieferende_datum": "2026-12-31", "transaktionsgrund": "ZT5"
    });
    let (found, _, edi) = door("gpke.lieferende.anmelden", &body).await;
    assert!(found.is_empty(), "{}", found.join("\n"));
    assert!(edi.contains("STS+7++ZT5+ZW4'"), "{edi}");
}

/// The Kündigung 55016 goes to the Altlieferant the caller names — not to the
/// grid operator — and passes its column.
#[tokio::test]
async fn a_55016_goes_to_the_altlieferant_and_passes_its_profile() {
    let body = serde_json::json!({
        "malo_id": MALO_ID,
        "kuendigung_datum": "2026-12-31",
        "lfa_mp_id": LFA_MP_ID,
    });
    let (found, recipient, edi) = door("gpke.kuendigung.anmelden", &body).await;
    assert!(found.is_empty(), "55016:\n{}", found.join("\n"));
    assert_eq!(recipient, LFA_MP_ID);
    assert!(edi.contains(&format!("NAD+MR+{LFA_MP_ID}::293'")), "{edi}");

    let state = lf_state().await;
    let refused = dispatch_command(
        &state,
        "gpke.kuendigung.anmelden",
        &serde_json::json!({ "malo_id": MALO_ID, "kuendigung_datum": "2026-12-31" }),
    )
    .await;
    assert!(
        matches!(&refused, Err(DispatchError::InvalidPayload(m)) if m.contains("lfa_mp_id")),
        "{refused:?}"
    );
}

/// Guard: every Anfrage door whose AHB column is „LFN an LFA" sends to the
/// Altlieferant it is given, never to the grid operator the MaLo cache names.
///
/// The column's own „Kommunikation von" decides which doors this covers, so a
/// new Kündigung door is held here the moment it is registered: one missing
/// from [`LFA_DOORS`] fails until it states its body.
#[tokio::test]
async fn every_lfn_an_lfa_door_sends_to_the_altlieferant() {
    /// `(command, sender, Altlieferant, body)` — each in its own Sparte's
    /// codes: a BDEW-Codenummer for Strom, a DVGW-Codenummer for Gas.
    const LFA_DOORS: &[(&str, &str, &str, &str)] = &[
        (
            "gpke.kuendigung.anmelden",
            "9900555000003",
            "9900555000010",
            r#"{"malo_id":"51238696012","kuendigung_datum":"2026-12-31","lfa_mp_id":"9900555000010"}"#,
        ),
        (
            "geli.kuendigung.anmelden",
            "9805550000003",
            "9805550000010",
            r#"{"malo_id":"51238696012","process_date":"20261231","lfa_mp_id":"9805550000010"}"#,
        ),
    ];
    let communication = |pid: u32| {
        edi_energy::registry::ReleaseRegistry::global()
            .all_profiles()
            .iter()
            .filter_map(|p| p.anwendungsfall(pid))
            .find_map(|af| af.communication.clone())
    };
    let gated: Vec<&str> = super::registry::COMMAND_REGISTRY
        .iter()
        .filter(|d| {
            d.primary_pid
                .is_some_and(|p| communication(p.as_u32()).as_deref() == Some("LFN an LFA"))
        })
        .map(|d| d.name)
        .collect();
    assert!(
        gated.len() >= 2,
        "found only {gated:?} — the profile lookup reads nothing"
    );
    for name in &gated {
        let Some((_, sender, lfa, body)) = LFA_DOORS.iter().find(|(n, ..)| n == name) else {
            panic!("{name} is „LFN an LFA“ and has no body in LFA_DOORS");
        };
        let mut state = lf_state().await;
        state.configured_marktrollen = vec!["LF".to_owned(), "LFG".to_owned()];
        state.sender_party_id = (*sender).to_owned();
        let body: serde_json::Value = serde_json::from_str(body).expect("body");
        dispatch_command(&state, name, &body)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let sent = utilmd(&state).await;
        assert_eq!(sent.len(), 1, "{name}");
        assert_eq!(
            sent[0].recipient.as_ref(),
            *lfa,
            "{name} went to {}",
            sent[0].recipient
        );
    }
}
