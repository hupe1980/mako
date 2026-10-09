//! The GeLi Gas command doors register the window the Festlegung publishes.
//!
//! - `geli.datenabruf.anfragen`: the GNB answers within ten Werktage of the
//!   Anfrage, but not before the 10. Werktag of the month after the Zeitspanne
//!   ends (GeLi Gas 3.0 Kap. 4.2) — so the Zeitspanne is a required input.
//! - `geli.stornierung.initiieren`: a Stornierung has no window of its own; the
//!   GNB answers within the cancelled message's Antwortfrist, and with no
//!   unanswered message there is nothing to cancel (GeLi Gas 3.0 Kap. 2.7).

use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::ids::{StreamId, TenantId};

use super::*;

const LF_MP_ID: &str = "9805550000003";
const GNB_MP_ID: &str = "9800357000005";
const MALO_ID: &str = "51238696012";

async fn lfg_state() -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let state = CommandsApiState {
        tenant_id: TenantId::from_party_id(LF_MP_ID),
        sender_party_id: LF_MP_ID.to_owned(),
        configured_marktrollen: vec!["LFG".to_owned()],
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
                    { "marketPartnerId": GNB_MP_ID, "executionTimeFrom": "2000-01-01T00:00:00Z" }
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

/// The deadlines registered for `outcome`'s process.
async fn registered(
    state: &CommandsApiState,
    outcome: &DispatchOutcome,
) -> Vec<mako_engine::deadline::Deadline> {
    let DispatchOutcome::Spawned { process_id } = outcome else {
        panic!("expected a spawn, got {outcome:?}");
    };
    state
        .store
        .as_deadline_store()
        .for_stream(&StreamId::for_process(state.tenant_id, process_id))
        .await
        .expect("read deadlines")
}

#[tokio::test]
async fn a_datenabruf_without_its_zeitspanne_is_refused() {
    let state = lfg_state().await;
    let refused = dispatch_command(
        &state,
        "geli.datenabruf.anfragen",
        &serde_json::json!({ "malo_id": MALO_ID }),
    )
    .await;
    assert!(
        matches!(refused, Err(DispatchError::InvalidPayload(ref m)) if m.contains("zeitspanne_bis")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_datenabruf_waits_for_the_tenth_werktag_of_the_following_month() {
    let state = lfg_state().await;
    // A Zeitspanne ending this month: its Brennwert exists only from the
    // 10. Werktag of next month, which is later than ten Werktage from now.
    let heute = mako_fristen::heute();
    let outcome = dispatch_command(
        &state,
        "geli.datenabruf.anfragen",
        &serde_json::json!({ "malo_id": MALO_ID, "zeitspanne_bis": heute.to_string() }),
    )
    .await
    .expect("accepted");
    let deadlines = registered(&state, &outcome).await;
    assert_eq!(deadlines.len(), 1, "{deadlines:?}");
    assert_eq!(
        deadlines[0].label(),
        mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL
    );
    let monatsende = time::Date::from_calendar_date(
        heute.year(),
        heute.month(),
        heute.month().length(heute.year()),
    )
    .expect("calendar date");
    let zehnter = mako_fristen::end_of_day_berlin(mako_fristen::add_werktage(
        monatsende,
        10,
        mako_fristen::HolidayCalendar::BdewMaKo,
    ));
    assert!(
        deadlines[0].due_at() >= zehnter,
        "the answer cannot be due before the 10. WT of the following month"
    );
}

#[tokio::test]
async fn a_stornierung_with_nothing_to_cancel_is_refused() {
    let state = lfg_state().await;
    let refused = dispatch_command(
        &state,
        "geli.stornierung.initiieren",
        &serde_json::json!({ "malo_id": MALO_ID }),
    )
    .await;
    assert!(
        matches!(refused, Err(DispatchError::InvalidPayload(ref m)) if m.contains("Kap. 2.7")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_stornierung_inherits_the_cancelled_messages_window() {
    let state = lfg_state().await;
    let anmeldung = dispatch_command(
        &state,
        "geli.lieferbeginn.anmelden",
        &serde_json::json!({
            "malo_id": MALO_ID,
            "zaehlpunkt": "DE0001111222233334444555566667777",
            "process_date": "20261201",
            "bilanzkreis": "THE0BFH000000001",
        }),
    )
    .await
    .expect("Anmeldung accepted");
    let original = registered(&state, &anmeldung).await;
    let original_due = original
        .iter()
        .find(|d| d.label() == mako_geli_gas::LF_ANMELDUNG_RESPONSE_WINDOW_LABEL)
        .expect("the 44001 Antwortfrist is published")
        .due_at();

    let storno = dispatch_command(
        &state,
        "geli.stornierung.initiieren",
        &serde_json::json!({ "malo_id": MALO_ID }),
    )
    .await
    .expect("Stornierung accepted");
    let window = registered(&state, &storno).await;
    assert_eq!(window.len(), 1, "{window:?}");
    assert_eq!(
        window[0].label(),
        mako_geli_gas::STORNIERUNG_LF_RESPONSE_WINDOW_LABEL
    );
    assert_eq!(window[0].due_at(), original_due);
}
