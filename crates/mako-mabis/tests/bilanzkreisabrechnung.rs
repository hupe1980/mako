//! Integration tests for the MABIS Bilanzkreisabrechnung (PID 13003) workflow.
//!
//! Covers the full write→store→read cycle using `InMemoryEventStore` — no
//! SlateDB required. Tests exercise the complete billing lifecycle,
//! validation guards, deadline wiring, dispute paths, and the
//! `BillingProjection` read-model.
//!
//! # State machine under test
//!
//! ```text
//! New → SummenzeitreiheReceived → PruefmitteilungSent → Settled
//!                              ↘ Disputed (negative Prüfmitteilung)
//!                              ↘ DeadlineExpired (1-Werktag deadline)
//! ```
//!
//! # Regulatory basis
//!
//! BNetzA BK6-24-174 §13.8: The BIKO sends the Abrechnungssummenzeitreihe to
//! the BKV. The BKV must respond with a Prüfmitteilung within **1 Werktag**.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    projection::ProjectionRunner,
    types::{BikoId, BillingPeriod, BkvId, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    BillingCommand, BillingProjection, BillingState, BillingVersion, DataStatus,
    MabisBillingWorkflow, PRUEFMITTEILUNG_DEADLINE_LABEL,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<MabisBillingWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    )
}

fn receive_summenzeitreihe_cmd(version: BillingVersion) -> BillingCommand {
    BillingCommand::ReceiveSummenzeitreihe {
        pid: Pruefidentifikator::new(13_003).unwrap(),
        billing_period: BillingPeriod::new("2025-09"),
        bkv_id: BkvId::new("BKV-DE-TEST-001"),
        biko_id: BikoId::new("BIKO-DE-TEST-001"),
        version,
        message_ref: MessageRef::new("MSCONS-BKA-2025-09-001"),
    }
}

// ── Happy-path lifecycle ───────────────────────────────────────────────────────

/// Full MABIS billing lifecycle:
/// New → SummenzeitreiheReceived → PruefmitteilungSent → Settled.
#[tokio::test]
async fn happy_path_full_lifecycle() {
    let p = make_process();

    // Step 1: BIKO sends Abrechnungssummenzeitreihe
    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Vorlaeufig))
        .await
        .expect("ReceiveSummenzeitreihe must succeed for PID 13003");

    let state = p.state().await.expect("state after ReceiveSummenzeitreihe");
    assert!(
        matches!(state, BillingState::SummenzeitreiheReceived(_)),
        "process must be SummenzeitreiheReceived, got: {state:?}",
    );

    // Step 2: BKV sends positive Prüfmitteilung within 1 Werktag
    p.execute(BillingCommand::SendPruefmitteilungPositiv {
        message_ref: MessageRef::new("PRUEF-POS-2025-09-001"),
    })
    .await
    .expect("SendPruefmitteilungPositiv must succeed from SummenzeitreiheReceived");

    let state = p
        .state()
        .await
        .expect("state after SendPruefmitteilungPositiv");
    assert!(
        matches!(state, BillingState::PruefmitteilungSent(_)),
        "process must be PruefmitteilungSent, got: {state:?}",
    );

    // Step 3: BIKO sends Datenstatus confirming settlement
    p.execute(BillingCommand::ReceiveDatastatus {
        data_status: DataStatus::AbgerechtneteDaten,
    })
    .await
    .expect("ReceiveDatastatus must succeed from PruefmitteilungSent");

    let state = p.state().await.expect("state after ReceiveDatastatus");
    assert!(
        matches!(state, BillingState::Settled(_)),
        "process must be Settled, got: {state:?}",
    );
}

/// Final billing version (endgültig) also completes the lifecycle correctly.
#[tokio::test]
async fn happy_path_endgueltig_version() {
    let p = make_process();

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Endgueltig))
        .await
        .expect("endgültig version must succeed");

    let state = p.state().await.unwrap();
    if let BillingState::SummenzeitreiheReceived(d) = &state {
        assert_eq!(d.version, BillingVersion::Endgueltig);
    } else {
        panic!("expected SummenzeitreiheReceived, got: {state:?}");
    }
}

// ── Dispute path ───────────────────────────────────────────────────────────────

/// A negative Prüfmitteilung transitions to `Disputed`.
#[tokio::test]
async fn negative_pruefmitteilung_transitions_to_disputed() {
    let p = make_process();

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Vorlaeufig))
        .await
        .unwrap();

    p.execute(BillingCommand::SendPruefmitteilungNegativ {
        message_ref: MessageRef::new("PRUEF-NEG-2025-09-001"),
        reason: "Zählpunkt DE00123456789012345678901234567890 fehlt in der Summenzeitreihe"
            .to_owned(),
    })
    .await
    .expect("SendPruefmitteilungNegativ must succeed from SummenzeitreiheReceived");

    let state = p
        .state()
        .await
        .expect("state after negative Prüfmitteilung");
    assert!(
        matches!(state, BillingState::Disputed { .. }),
        "process must be Disputed, got: {state:?}",
    );
}

// ── Guard: unsupported PID ─────────────────────────────────────────────────────

/// PIDs other than 13003 must return `WorkflowError::NotImplemented`.
#[tokio::test]
async fn unsupported_pid_returns_not_implemented() {
    let p = make_process();

    let result = p
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(13_002).unwrap(),
            billing_period: BillingPeriod::new("2025-09"),
            bkv_id: BkvId::new("BKV-001"),
            biko_id: BikoId::new("BIKO-001"),
            version: BillingVersion::Vorlaeufig,
            message_ref: MessageRef::new("REF-001"),
        })
        .await;

    assert!(
        result.is_err(),
        "PID 13002 must return NotImplemented error"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not implemented") || err.to_string().contains("NotImplemented"),
        "error must mention NotImplemented, got: {err}",
    );
}

// ── Guard: wrong-state transitions ────────────────────────────────────────────

/// Sending a Prüfmitteilung from `New` must fail.
#[tokio::test]
async fn pruefmitteilung_from_new_is_rejected() {
    let p = make_process();

    let result = p
        .execute(BillingCommand::SendPruefmitteilungPositiv {
            message_ref: MessageRef::new("PRUEF-POS-EARLY"),
        })
        .await;

    assert!(result.is_err(), "Prüfmitteilung from New must fail");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("SummenzeitreiheReceived"),
        "error must reference expected state, got: {err}",
    );
}

/// Receiving Datenstatus before sending Prüfmitteilung must fail.
#[tokio::test]
async fn datenstatus_from_summenzeitreihe_received_is_rejected() {
    let p = make_process();

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Vorlaeufig))
        .await
        .unwrap();

    let result = p
        .execute(BillingCommand::ReceiveDatastatus {
            data_status: DataStatus::Abrechnungsdaten,
        })
        .await;

    assert!(
        result.is_err(),
        "ReceiveDatastatus from SummenzeitreiheReceived must fail"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("PruefmitteilungSent"),
        "error must reference expected state, got: {err}",
    );
}

// ── Deadline wiring ────────────────────────────────────────────────────────────

/// A 1-Werktag deadline firing on `SummenzeitreiheReceived` transitions to
/// `DeadlineExpired`.
#[tokio::test]
async fn pruefmitteilung_deadline_transitions_to_deadline_expired() {
    let p = make_process();

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Vorlaeufig))
        .await
        .unwrap();

    let deadline_id = DeadlineId::new();
    p.execute(BillingCommand::PruefmitteilungDeadlineExpired {
        deadline_id,
        label: PRUEFMITTEILUNG_DEADLINE_LABEL.into(),
    })
    .await
    .expect("PruefmitteilungDeadlineExpired must succeed from SummenzeitreiheReceived");

    let state = p.state().await.expect("state after deadline");
    assert!(
        matches!(state, BillingState::DeadlineExpired(_)),
        "process must be DeadlineExpired, got: {state:?}",
    );
}

/// A deadline firing on an already-`Settled` process is absorbed harmlessly
/// (produces zero events, state unchanged).
#[tokio::test]
async fn deadline_on_settled_is_absorbed() {
    let p = make_process();

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Vorlaeufig))
        .await
        .unwrap();
    p.execute(BillingCommand::SendPruefmitteilungPositiv {
        message_ref: MessageRef::new("PRUEF-POS-001"),
    })
    .await
    .unwrap();
    p.execute(BillingCommand::ReceiveDatastatus {
        data_status: DataStatus::AbgerechtneteDaten,
    })
    .await
    .unwrap();

    // Late deadline fire after Settled → absorbed
    let deadline_id = DeadlineId::new();
    p.execute(BillingCommand::PruefmitteilungDeadlineExpired {
        deadline_id,
        label: PRUEFMITTEILUNG_DEADLINE_LABEL.into(),
    })
    .await
    .expect("late deadline on Settled must be absorbed");

    let state = p.state().await.expect("state after late deadline");
    assert!(
        matches!(state, BillingState::Settled(_)),
        "process must remain Settled, got: {state:?}",
    );
}

// ── Read-model projection ──────────────────────────────────────────────────────

/// Verify that `BillingProjection` correctly tracks the full happy-path lifecycle.
#[tokio::test]
async fn projection_tracks_full_lifecycle() {
    let store = InMemoryEventStore::new();
    let p: Process<MabisBillingWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    );

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Vorlaeufig))
        .await
        .unwrap();
    p.execute(BillingCommand::SendPruefmitteilungPositiv {
        message_ref: MessageRef::new("PRUEF-POS-001"),
    })
    .await
    .unwrap();
    p.execute(BillingCommand::ReceiveDatastatus {
        data_status: DataStatus::AbgerechtneteDaten,
    })
    .await
    .unwrap();

    let events = store.all_events().await;
    let mut projection = BillingProjection::default();
    ProjectionRunner::run(&mut projection, &events);

    assert_eq!(
        projection.records.len(),
        1,
        "exactly one stream in projection"
    );

    let record = projection.records.values().next().unwrap();
    assert_eq!(
        record.status(),
        "Settled",
        "projection status must be Settled"
    );
    // SummenzeitreiheReceived + PruefmitteilungPositivSent + DatenstatusReceived = 3 events
    assert_eq!(record.event_count(), 3, "3 events in the stream");
    let data = record
        .active_data()
        .expect("record must be Active after full lifecycle");
    assert!(
        !data.billing_period.to_string().is_empty(),
        "billing_period must be populated"
    );
    assert!(!data.bkv_id.as_str().is_empty(), "bkv_id must be populated");
    assert!(
        !data.biko_id.as_str().is_empty(),
        "biko_id must be populated"
    );
    assert_eq!(
        *data.version,
        BillingVersion::Vorlaeufig,
        "version must be Vorlaeufig"
    );
}

/// Verify that `BillingProjection` correctly tracks a disputed process.
#[tokio::test]
async fn projection_tracks_disputed_process() {
    let store = InMemoryEventStore::new();
    let p: Process<MabisBillingWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    );

    p.execute(receive_summenzeitreihe_cmd(BillingVersion::Endgueltig))
        .await
        .unwrap();
    p.execute(BillingCommand::SendPruefmitteilungNegativ {
        message_ref: MessageRef::new("PRUEF-NEG-001"),
        reason: "Datenfehler beim Aggregat".to_owned(),
    })
    .await
    .unwrap();

    let events = store.all_events().await;
    let mut projection = BillingProjection::default();
    ProjectionRunner::run(&mut projection, &events);

    let record = projection.records.values().next().unwrap();
    assert_eq!(
        record.status(),
        "Disputed",
        "projection status must be Disputed"
    );
    // SummenzeitreiheReceived + PruefmitteilungNegativSent = 2 events
    assert_eq!(record.event_count(), 2, "2 events in the stream");
}

// ── Deadline accuracy ─────────────────────────────────────────────────────────

/// MABIS BK6-24-174 §13.8: The BKV must respond with a Prüfmitteilung within
/// **1 Werktag** of receiving the Summenzeitreihe.
///
/// This test verifies the deadline arithmetic on a holiday-boundary date.
/// The Summenzeitreihe arrives on Mon 2025-01-06 (Heilige Drei Könige, a
/// BDEW MaKo public holiday).  The 1-Werktag deadline skips the holiday and
/// lands on Tue 2025-01-07 — not on Mon 06 itself (non-Werktag) or Wed 08
/// (would be 2 Werktage).
#[test]
fn pruefmitteilung_deadline_1_werktag_after_heilige_drei_koenige() {
    use mako_engine::fristen::{self, HolidayCalendar};
    use time::{Date, Month};

    // Received on Mon 2025-01-06 — Heilige Drei Könige (public holiday).
    let received = Date::from_calendar_date(2025, Month::January, 6).unwrap();
    let due = fristen::add_werktage(received, 1, HolidayCalendar::BdewMaKo);

    // Mon 06 is a holiday → the next Werktag is Tue 07 Jan 2025.
    assert_eq!(
        due,
        Date::from_calendar_date(2025, Month::January, 7).unwrap(),
        "1 Werktag after Heilige Drei Könige (Mon 2025-01-06) must be Tue 2025-01-07, \
         not Mon 2025-01-06 (holiday) or Wed 2025-01-08 (2 Werktage)"
    );
}

/// Verify the deadline label constant is stable.
///
/// Changing [`PRUEFMITTEILUNG_DEADLINE_LABEL`] after deployment would
/// orphan all existing `Deadline` records in the store — processes whose
/// deadline was registered with the old label would never fire.
#[test]
fn pruefmitteilung_deadline_label_is_stable() {
    assert_eq!(
        PRUEFMITTEILUNG_DEADLINE_LABEL, "mabis-pruefmitteilung-1-werktag",
        "changing this label orphans existing Deadline records"
    );
}

// ── Negative AHB conformance tests ───────────────────────────────────────────

/// A MSCONS PID 13003 with an invalid BGM qualifier (`Z27` instead of `7`)
/// must be rejected by the AHB rule pack with rule `AHB-13003-BGM-1001-Q`.
///
/// `Z27` is the Geschäftsbestätigung qualifier valid for PID 13002 but not for
/// PID 13003 (Summenzeitreihe Bilanzkreisabrechnung).
///
/// This guards against a profile regression that removes or relaxes the BGM
/// qualifier check, or against the `Some(_unknown)` fallback silently returning
/// `is_valid = true` for any PID absent from the MSCONS AHB dispatch table.
#[test]
fn negative_ahb_wrong_bgm_qualifier_pid_13003() {
    use edi_energy::{EdiEnergyMessage, Platform};

    let invalid_bytes: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.5'\
BGM+Z27:::+00013003::+9'\
DTM+137:20230101:102'\
RFF+Z13:13003'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:1500.000:KWH'\
UNT+10+1'\
UNZ+1+1'";

    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(invalid_bytes)
        .expect("EDIFACT syntax is valid; parse must succeed");

    // Reference date 2027-10-01 is conformance_reference_date() — max(valid_from) + 365d
    // — which selects the mscons fv20261001 profile (MSCONS 2.5).
    let ref_date = time::Date::from_calendar_date(2027, time::Month::October, 1).unwrap();
    let report = msg
        .validate_on_date(ref_date)
        .expect("validate_on_date must not error");

    assert!(
        !report.is_valid(),
        "wrong BGM qualifier Z27 must cause AHB validation failure for PID 13003"
    );
    assert!(
        report.errors().iter().any(|e| e
            .rule_id
            .as_deref()
            .is_some_and(|id| id == "AHB-13003-BGM-1001-Q")),
        "error list must contain rule AHB-13003-BGM-1001-Q; got: {:?}",
        report
            .errors()
            .iter()
            .map(|e| e.rule_id.as_deref())
            .collect::<Vec<_>>(),
    );
}
