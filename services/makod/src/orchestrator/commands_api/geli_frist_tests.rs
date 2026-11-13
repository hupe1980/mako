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
const MELO_ID: &str = "DE00056266802AO6G56M11SN51G21M24S";

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
        malo_cache: Arc::new(SlateDbMaloCache::new(
            store.clone(),
            TenantId::from_party_id(LF_MP_ID),
        )),
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
        .upsert(&record)
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
        &serde_json::json!({ "malo_id": MALO_ID, "melo_id": MELO_ID, "zeitspanne_von": "2026-01-01" }),
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
    // The latest Zeitspanne the LF may ask for ends with yesterday's Gastag:
    // its Brennwert exists only from the 10. Werktag of the following month,
    // which is later than ten Werktage from now unless that month is this one.
    let bis = mako_fristen::heute()
        .previous_day()
        .and_then(time::Date::previous_day)
        .expect("calendar date");
    let outcome = dispatch_command(
        &state,
        "geli.datenabruf.anfragen",
        &serde_json::json!({
            "malo_id": MALO_ID,
            "melo_id": MELO_ID,
            "zeitspanne_von": bis.to_string(),
            "zeitspanne_bis": bis.to_string(),
        }),
    )
    .await
    .expect("accepted");
    let deadlines = registered(&state, &outcome).await;
    assert_eq!(deadlines.len(), 1, "{deadlines:?}");
    assert_eq!(
        deadlines[0].label(),
        mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL
    );
    let monatsende =
        time::Date::from_calendar_date(bis.year(), bis.month(), bis.month().length(bis.year()))
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

/// The ORDERS names the Messlokation (ORDERS AHB 1.1b Kap. 4.1.3 `LOC+172`).
#[tokio::test]
async fn a_datenabruf_without_its_messlokation_is_refused() {
    let state = lfg_state().await;
    let refused = dispatch_command(
        &state,
        "geli.datenabruf.anfragen",
        &serde_json::json!({
            "malo_id": MALO_ID,
            "zeitspanne_von": "2026-01-01",
            "zeitspanne_bis": "2026-01-31",
        }),
    )
    .await;
    assert!(
        matches!(refused, Err(DispatchError::InvalidPayload(ref m)) if m.contains("melo_id")),
        "{refused:?}"
    );
}

/// GeLi Gas 3.0 Kap. 4.2: „die in der Vergangenheit liegende Zeitspanne",
/// asked „bis 12 Monate nach dem letzten Tag der betreffenden Zeitspanne".
#[test]
fn a_datenabruf_zeitspanne_lies_in_the_past_and_within_twelve_months() {
    use time::macros::{date, datetime};
    let check = |von, bis, am| zeitspanne_in_der_vergangenheit(von, bis, am).is_ok();
    // Monday 02.11.2026, 09:00 MEZ.
    let am = datetime!(2026-11-02 08:00 UTC);
    assert!(check(date!(2026 - 10 - 01), date!(2026 - 10 - 31), am));
    // Sunday's Gastag runs until Monday 06:00 MEZ — it has ended.
    assert!(check(date!(2026 - 11 - 01), date!(2026 - 11 - 01), am));
    // Today's Gastag has not.
    assert!(!check(date!(2026 - 11 - 02), date!(2026 - 11 - 02), am));
    // At 05:30 MEZ on Monday, Sunday's Gastag has not ended either.
    assert!(!check(
        date!(2026 - 11 - 01),
        date!(2026 - 11 - 01),
        datetime!(2026-11-02 04:30 UTC)
    ));
    // Twelve months after the last day is still in time; the day after is not.
    assert!(check(date!(2025 - 10 - 01), date!(2025 - 11 - 02), am));
    assert!(!check(date!(2025 - 10 - 01), date!(2025 - 11 - 01), am));
    // A Zeitspanne cannot end before it begins.
    assert!(!check(date!(2026 - 10 - 31), date!(2026 - 10 - 01), am));
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
            "process_date": "20261201",
            "transaktionsgrund": "E03",
            "bilanzkreis": "THE0BFH000000001",
            "abrechnungsintervall_monate": 12,
            "haushaltskunde": true,
            "netznutzungsvertrag": "LIEFERANT_NB",
            "netznutzung_zahler": "LIEFERANT",
            "gemeinderabatt_prozent": "0",
            "kunde": {
                "nachname": "Mustermann",
                "anschrift": { "strasse": "Hauptstraße 1", "postleitzahl": "50667",
                               "ort": "Köln", "landescode": "DE" }
            },
            "ablesekarte": {
                "nachname": "Mustermann",
                "anschrift": { "strasse": "Hauptstraße 1", "postleitzahl": "50667",
                               "ort": "Köln", "landescode": "DE" }
            },
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
