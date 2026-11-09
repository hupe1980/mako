//! The GNB's nicht bilanzierungsrelevante Änderung 44112 (UTILMD AHB Gas 1.2
//! Kap. 5.10.1, S. 97–110), sent by `geli.stammdaten-aenderung.senden` with
//! the changed groups read from marktd on the Änderungsdatum, rendered, parsed
//! back and held against its AHB profile.

use std::sync::Arc;

use mako_engine::outbox::OutboxStore;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::commands_api::{CommandsApiState, DispatchError, dispatch_command};

use crate::support;

// DVGW-Codenummern: NAD DE 3055 `332`.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "9870012345678";
const MALO: &str = "51238696012";

async fn gnb(
    record: Option<mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten>,
) -> CommandsApiState {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    CommandsApiState {
        tenant_id: TenantId::from_party_id(GNB_MP),
        sender_party_id: GNB_MP.to_owned(),
        configured_marktrollen: vec!["GNB".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new((*store).clone())),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store,
        marktd_client: Some(Arc::new(support::stub_marktd(record).await)),
    }
}

fn body(gruppen: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "pid": 44112,
        "empfaenger_mp_id": LF_MP,
        "aenderungsdatum": "20261201",
        "vertragsbeginn": "20250101",
        "gruppen": gruppen,
    })
}

#[tokio::test]
async fn the_gnb_sends_a_44112_with_marktds_changed_groups() {
    for gruppen in [
        &["MARKTGEBIET", "NETZNUTZUNG"][..],
        &[
            "TERMIN_NETZNUTZUNGSABRECHNUNG",
            "NETZNUTZUNGSABRECHNUNGSINTERVALL",
            "DRUCKEBENE",
            "MARKTGEBIET",
            "KONZESSIONSABGABE",
            "ZUGEORDNETE_MARKTPARTNER",
            "GASQUALITAET",
            "MARKTLOKATIONSANSCHRIFT",
        ][..],
    ] {
        sends_a_44112(gruppen).await;
    }
}

async fn sends_a_44112(gruppen: &[&str]) {
    let state = gnb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    dispatch_command(
        &state,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &body(gruppen),
    )
    .await
    .expect("the 44112 is dispatched");
    let out: Vec<_> = state
        .store
        .pending_now(100)
        .await
        .expect("outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == "UTILMD")
        .collect();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].recipient.as_ref(), LF_MP);
    assert_eq!(out[0].payload["pid"], 44112);

    // Render, parse back and hold against the profile, as the pre-send gate
    // does.
    use edi_energy::EdiEnergyMessage as _;
    let party = |mp: &str, role: &str| makod::config::PartyConfig {
        mp_id: mp.to_owned(),
        roles: vec![role.to_owned()],
        primary: mp == GNB_MP,
        agency: None,
        as4: None,
    };
    let registry = makod::party_registry::MpIdRegistry::from_config(&[
        party(GNB_MP, "NB"),
        party(LF_MP, "LF"),
    ])
    .expect("valid registry");
    let wire = makod::edifact_renderer::render_to_wire_bytes(&out[0], &registry)
        .expect("the 44112 renders");
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let msg = edi_energy::parse(&wire.bytes).expect("parses");
    let report = msg
        .validate_on_date(mako_fristen::heute())
        .expect("the AHB validation runs");
    assert!(
        report.is_valid(),
        "{:#?}\n{}",
        report
            .errors()
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        text.replace('\'', "'\n")
    );
    assert!(text.contains("CCI+Z21"), "the Marktgebiet: {text}");
}

/// Without marktd there is no 44112, and nothing is written.
#[tokio::test]
async fn a_44112_marktd_cannot_serve_is_refused() {
    let state = gnb(None).await;
    let err = dispatch_command(
        &state,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &body(&["MARKTGEBIET"]),
    )
    .await
    .expect_err("refused");
    assert!(
        !matches!(err, DispatchError::DuplicateProcess { .. }),
        "{err:?}"
    );
    assert!(
        state
            .store
            .pending_now(100)
            .await
            .expect("outbox")
            .is_empty()
    );
}
