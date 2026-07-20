//! Integration tests for WiM Gas deadline wiring.
//!
//! Verifies that:
//!
//! - The APERAK Frist is **10 Werktage** (BK7-24-01-009), not 5.
//! - Saturdays, Sundays and public holidays are not Werktage.
//! - `deadline_at_werktage` fires at 17:00 Europe/Berlin, not midnight UTC.
//! - The deadline label matches `RESPONSE_WINDOW_LABEL` in the workflow constants.
//! - A timeout fires `DeadlineExpired` and transitions the process to `Rejected`.

use mako_engine::event_store::InMemoryEventStore;
use mako_engine::{
    fristen::{self, HolidayCalendar},
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_wim_gas::{
    ANMELDUNG_PIDS, ANMELDUNG_RESPONSE_WINDOW_LABEL, WimGasAnmeldungCommand, WimGasAnmeldungState,
    WimGasAnmeldungWorkflow,
};
use time::{Date, Month, OffsetDateTime, Time, UtcOffset};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_process() -> Process<WimGasAnmeldungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("wim-gas-anmeldung", "FV2025-10-01"),
    )
}

fn receive_utilmd(pid: u32, validation_passed: bool) -> WimGasAnmeldungCommand {
    WimGasAnmeldungCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(pid).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: MarktpartnerCode::new("9900357000004"),
        malo_id: MaLo::new("DE0000123456789012345678901234567890"),
        document_date: "20250115".to_owned(),
        message_ref: MessageRef::new("MSG-WIM-GAS-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["Pflichtfeld SG2:NAD+MS fehlt".to_owned()]
        },
        received_at: time::OffsetDateTime::now_utc(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// The APERAK window label must match the constant declared in `anmeldung`.
#[test]
fn response_window_label_matches_constant() {
    assert_eq!(
        ANMELDUNG_RESPONSE_WINDOW_LABEL, "wim-gas-response-10-werktage",
        "response window label must be 'wim-gas-response-10-werktage' (BK7-24-01-009)"
    );
}

/// WiM Gas deadline is 10 Werktage, not 5.
///
/// Starting on Monday 2025-01-06, 10 Werktage (Saturday counts) lands on
/// Wednesday 2025-01-22.
///
/// Weekday count from Mon 2025-01-06:
///   Mon 06, Tue 07, Wed 08, Thu 09, Fri 10, Sat 11 → 6 WT
///   Mon 13, Tue 14, Wed 15, Thu 16, Fri 17, Sat 18 → 6+6 = 12 (overshoot)
///   10 WT: Mon 06..Sat 11 = 6, Mon 13..Fri 17 = 4 more → total 10 → Fri 17
///
/// This is distinct from WiM Strom (5 Werktage → Mon 06..Sat 11 → Mon 13 = 6, clip to Fri 10 = 5).
#[test]
fn wim_gas_aperak_is_10_werktage_not_5() {
    // Monday 2025-01-06
    let monday = Date::from_calendar_date(2025, Month::January, 6).unwrap();
    let gas_deadline = fristen::add_werktage(monday, 10, HolidayCalendar::BdewMaKo);
    let strom_deadline = fristen::add_werktage(monday, 5, HolidayCalendar::BdewMaKo);

    assert_ne!(
        gas_deadline, strom_deadline,
        "WiM Gas (10 WT) and WiM Strom (5 WT) deadlines must differ"
    );

    // 10 WT from Mon 2025-01-06 (Heilige Drei Könige is Mon 06, already past):
    // Tue 07 … Fri 10 = 4, Mon 13 … Fri 17 = 9, Mon 20 = 10. Weekends are not
    // Werktage.
    let expected = Date::from_calendar_date(2025, Month::January, 20).unwrap();
    assert_eq!(
        gas_deadline, expected,
        "10 WT from Mon 2025-01-06 must land on Mon 2025-01-20"
    );
}

/// Saturday is **not** a Werktag in market communication.
///
/// GPKE (BK6-24-174) Teil 1: "alle Tage ..., die kein Samstag, Sonntag oder
/// gesetzlicher Feiertag sind". So 1 WT from Friday 2025-01-10 is Monday
/// 2025-01-13, not Saturday.
#[test]
fn saturday_is_not_a_werktag() {
    let friday = Date::from_calendar_date(2025, Month::January, 10).unwrap();
    let next = fristen::add_werktage(friday, 1, HolidayCalendar::BdewMaKo);
    assert_eq!(
        next,
        Date::from_calendar_date(2025, Month::January, 13).unwrap(),
        "1 WT from Friday must skip the weekend and land on Monday"
    );
}

/// Counting from a Saturday still lands on the next Monday.
#[test]
fn sunday_skipped_in_werktag_count() {
    let saturday = Date::from_calendar_date(2025, Month::January, 11).unwrap();
    let next = fristen::add_werktage(saturday, 1, HolidayCalendar::BdewMaKo);
    assert_eq!(
        next,
        Date::from_calendar_date(2025, Month::January, 13).unwrap(),
        "1 WT from Saturday must skip Sunday and land on Monday"
    );
}

/// `deadline_at_werktage` fires at 17:00 Europe/Berlin, not midnight UTC.
///
/// In CET (UTC+1), 17:00 Berlin = 16:00 UTC.
///
/// We use a January date (CET) and assert the UTC hour is 16.
///
/// Werktag count from Wed 2025-01-08 (10 WT):
///   Thu 9 (1), Fri 10 (2), Mon 13 (3), Tue 14 (4), Wed 15 (5),
///   Thu 16 (6), Fri 17 (7), Mon 20 (8), Tue 21 (9), Wed 22 (10).
#[test]
fn deadline_fires_at_1700_berlin_not_midnight_utc() {
    // Wednesday 2025-01-08 CET (UTC+1): any hour before midnight Berlin.
    let received = OffsetDateTime::new_in_offset(
        Date::from_calendar_date(2025, Month::January, 8).unwrap(),
        Time::from_hms(10, 0, 0).unwrap(),
        UtcOffset::from_hms(1, 0, 0).unwrap(), // CET
    );
    let deadline = fristen::deadline_at_werktage(received, 10, HolidayCalendar::BdewMaKo);
    // 10 WT from Wed 2025-01-08 → Mon 2025-01-20 (Sun Jan 12 and Sun Jan 19 skipped).
    let due_date = deadline.date();
    assert_eq!(
        due_date,
        Date::from_calendar_date(2025, Month::January, 22).unwrap(),
        "10 WT from Wed 2025-01-08 should land on Wed 2025-01-22"
    );

    // The deadline must be at 17:00 CET = 16:00 UTC (not midnight 00:00 UTC).
    let utc_deadline = deadline.to_offset(UtcOffset::UTC);
    assert_eq!(
        utc_deadline.hour(),
        16,
        "deadline must be at 16:00 UTC (= 17:00 CET) in January"
    );
    assert_eq!(utc_deadline.minute(), 0, "deadline must be at :00 minutes");
}

/// Happy path: UTILMD G received, ValidationPassed, APERAK dispatched.
#[tokio::test]
async fn happy_path_anmeldung() {
    let process = make_process();

    // Step 1: receive UTILMD G (PID 44042 — Anmeldung neuer MSB Gas)
    process
        .execute(receive_utilmd(44042, true))
        .await
        .expect("ReceiveUtilmd should succeed");

    let state = process.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, WimGasAnmeldungState::ValidationPassed(_)),
        "state must be ValidationPassed after valid UTILMD, got: {state:?}"
    );

    // Step 2: dispatch positive APERAK
    process
        .execute(WimGasAnmeldungCommand::DispatchAperak {
            positive: true,
            reason: None,
        })
        .await
        .expect("DispatchAperak should succeed");

    let state = process.state().await.expect("state after DispatchAperak");
    assert!(
        matches!(state, WimGasAnmeldungState::AperakSent(_)),
        "state must be AperakSent after positive APERAK, got: {state:?}"
    );
}

/// Validation failure leads to Rejected.
#[tokio::test]
async fn validation_failure_leads_to_rejected() {
    let process = make_process();
    process
        .execute(receive_utilmd(44042, false))
        .await
        .expect("ReceiveUtilmd should not panic on validation failure");
    let state = process.state().await.expect("state");
    assert!(
        matches!(state, WimGasAnmeldungState::Rejected { .. }),
        "validation failure must transition to Rejected, got: {state:?}"
    );
}

/// Timeout expired leads to Rejected.
#[tokio::test]
async fn timeout_leads_to_rejected() {
    let process = make_process();
    process
        .execute(receive_utilmd(44042, true))
        .await
        .expect("step 1 ok");

    process
        .execute(WimGasAnmeldungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANMELDUNG_RESPONSE_WINDOW_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired should not panic");

    let state = process.state().await.expect("state");
    assert!(
        matches!(state, WimGasAnmeldungState::Rejected { .. }),
        "deadline expiry must transition to Rejected, got: {state:?}"
    );
}

/// Duplicate TimeoutExpired on a terminal state is absorbed without error.
#[tokio::test]
async fn timeout_on_terminal_state_is_absorbed() {
    let process = make_process();
    process
        .execute(receive_utilmd(44042, false)) // → Rejected
        .await
        .unwrap();

    let result = process
        .execute(WimGasAnmeldungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANMELDUNG_RESPONSE_WINDOW_LABEL.into(),
        })
        .await;
    assert!(
        result.is_ok(),
        "TimeoutExpired on Rejected must be absorbed"
    );
}

/// PID routing smoke: all ANMELDUNG_PIDS route to `"wim-gas-anmeldung"`.
#[test]
fn anmeldung_pids_route_to_wim_gas_anmeldung() {
    use mako_engine::builder::EngineModule;
    use mako_engine::marktrolle::DeploymentRoles;
    use mako_engine::pid_router::PidRouter;
    use mako_wim_gas::WimGasModule;

    let roles = DeploymentRoles::all();
    let mut router = PidRouter::new();
    WimGasModule.register_pids_with_roles(&mut router, &roles);

    for &pid in ANMELDUNG_PIDS {
        assert_eq!(
            router.route(pid),
            Some("wim-gas-anmeldung"),
            "PID {pid} must route to wim-gas-anmeldung"
        );
    }
}
