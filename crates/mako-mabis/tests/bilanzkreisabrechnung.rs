//! Integration tests for the MaBiS Bilanzkreisabrechnung workflow.
//!
//! Covers the full write→store→read cycle using `InMemoryEventStore` — no
//! SlateDB required.
//!
//! # The two invariants
//!
//! - **A Prüfmitteilung has no answer Frist.** BK6-24-174 Anlage 3 Kap. 9.8.2
//!   Nr. 1 leaves its Frist cell empty — the receiving party „kann" answer —
//!   and Kap. 13.8.2 states only the **BIKO's own** dispatch dates (18. WT /
//!   42. WT). Registering a countdown on the arrival would breach a window the
//!   Festlegung does not open.
//! - **A settlement carries a *sequence* of versions** (Kap. 3.8.2), so a
//!   one-shot request/response state machine cannot represent the Clearingphase
//!   at all.
//!
//! # State machine under test
//!
//! ```text
//! New → Offen ──(versions arrive, are checked, get a Datenstatus)──► Geschlossen
//! ```

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    projection::ProjectionRunner,
    types::{BikoId, BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    Abrechnungslauf, BillingCommand, BillingProjection, BillingState, Datenstatus, Familie,
    Kategorie, MabisBillingWorkflow, MabisZaehlpunktId, Pruefergebnis, SUMMENZEITREIHE_PID,
    SzrVersion, Zeitreihe,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_process() -> Process<MabisBillingWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    )
}

/// Build a version from an ordinal — the wire form is an Erstellungszeitpunkt
/// (IFTSTA `RFF+AUU`), so the ordinals become ascending seconds.
fn v(n: u32) -> SzrVersion {
    SzrVersion::new(format!("202601011200{n:02}+00")).expect("17 characters")
}

fn bg_szr() -> Zeitreihe {
    Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B)).expect("BG-SZR Kategorie B")
}

fn mabis_zp() -> MabisZaehlpunktId {
    MabisZaehlpunktId::new("DE0001111222233334444555566667777").expect("33 characters")
}

fn receive(version: u32, im_erstaufschlag: bool) -> BillingCommand {
    BillingCommand::ReceiveSummenzeitreihe {
        pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID).expect("13003"),
        zeitreihe: bg_szr(),
        mabis_zp: mabis_zp(),
        bilanzierungsmonat: BillingPeriod::new("2026-01"),
        version: v(version),
        im_erstaufschlag,
        absender: MarktpartnerCode::new("9900357000004"),
        biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
        message_ref: MessageRef::new(format!("MSCONS-BG-2026-01-V{version}")),
    }
}

/// `E_0062` decides a BG-SZR: `A03` is its Zustimmung, `A02` its Ablehnung.
fn pruefmitteilung(version: u32, antwortcode: &str, grund: Option<&str>) -> BillingCommand {
    BillingCommand::SendPruefmitteilung {
        version: v(version),
        pid: Pruefidentifikator::new(21_005).expect("21005"),
        antwortcode: antwortcode.to_owned(),
        grund: grund.map(ToOwned::to_owned),
        message_ref: MessageRef::new(format!("IFTSTA-PM-V{version}")),
    }
}

fn datenstatus(pid: u32, version: u32, status: Datenstatus) -> BillingCommand {
    BillingCommand::ReceiveIftsta {
        pid: Pruefidentifikator::new(pid).expect("valid PID"),
        version: v(version),
        datenstatus: Some(status),
        abweisungsgrund: None,
        message_ref: MessageRef::new(format!("IFTSTA-DS-V{version}")),
    }
}

// ── Happy path ───────────────────────────────────────────────────────────────

/// Erstaufschlag → Datenstatus „Abrechnungsdaten" → Abrechnungsstichtag.
#[tokio::test]
async fn erstaufschlag_settles_without_a_pruefmitteilung() {
    let process = make_process();

    process
        .execute(receive(1, true))
        .await
        .expect("V1 accepted");
    // Kap. 3.8.3: a version inside the Erstaufschlag window is assigned
    // „Abrechnungsdaten" automatically — nobody has to check it first.
    process
        .execute(datenstatus(21_003, 1, Datenstatus::Abrechnungsdaten))
        .await
        .expect("Datenstatus accepted");

    let state = process.state().await.expect("state loads");
    let data = state.data().expect("open");
    assert_eq!(
        data.abrechnungsrelevante_version().map(|r| &r.version),
        Some(&v(1))
    );

    // The Abrechnungsstichtag (42. WT) turns it into „abgerechnete Daten".
    process
        .execute(datenstatus(21_003, 1, Datenstatus::AbgerechneteDaten))
        .await
        .expect("final Datenstatus accepted");
    let state = process.state().await.expect("state loads");
    assert!(
        state
            .data()
            .unwrap()
            .version(&v(1))
            .unwrap()
            .datenstatus
            .unwrap()
            .ist_abgerechnet()
    );
}

/// Clearingphase: a version filed after the Erstaufschlag arrives as
/// „Prüfdaten" and a positive Prüfmitteilung promotes it.
#[tokio::test]
async fn clearing_version_is_promoted_by_a_positive_pruefmitteilung() {
    let process = make_process();

    process.execute(receive(1, true)).await.expect("V1");
    process
        .execute(datenstatus(21_003, 1, Datenstatus::Abrechnungsdaten))
        .await
        .expect("V1 status");
    process
        .execute(pruefmitteilung(1, "A02", Some("Summe weicht um 12 kWh ab")))
        .await
        .expect("negative check");

    // Kap. 3.8.3: the negative check leaves the Datenstatus untouched.
    let state = process.state().await.expect("state loads");
    assert_eq!(
        state.data().unwrap().version(&v(1)).unwrap().datenstatus,
        Some(Datenstatus::Abrechnungsdaten),
        "a negative Prüfmitteilung must not change the Datenstatus"
    );
    assert_eq!(state.data().unwrap().offener_korrekturbedarf(), vec![&v(1)]);

    // The correction is a new version, filed in the Clearingphase.
    process.execute(receive(2, false)).await.expect("V2");
    process
        .execute(datenstatus(21_003, 2, Datenstatus::Pruefdaten))
        .await
        .expect("V2 status");
    process
        .execute(pruefmitteilung(2, "A03", None))
        .await
        .expect("positive check");
    process
        .execute(datenstatus(21_003, 2, Datenstatus::Abrechnungsdaten))
        .await
        .expect("promotion");

    let state = process.state().await.expect("state loads");
    let data = state.data().unwrap();
    assert!(data.offener_korrekturbedarf().is_empty());
    assert_eq!(
        data.abrechnungsrelevante_version().map(|r| &r.version),
        Some(&v(2))
    );
}

// ── Versionierung ────────────────────────────────────────────────────────────

/// Kap. 3.8.2 — versions ascend across the whole BKA.
#[tokio::test]
async fn a_repeated_or_lower_version_is_refused() {
    let process = make_process();
    process.execute(receive(3, true)).await.expect("V3");

    assert!(
        process.execute(receive(3, false)).await.is_err(),
        "the same version twice is a filing error, not a redelivery"
    );
    assert!(process.execute(receive(2, false)).await.is_err());
    process.execute(receive(4, false)).await.expect("V4");
}

// ── PID direction ────────────────────────────────────────────────────────────

/// 21003 and 21004 both carry a Datenstatus; which one a participant receives
/// follows from its role.
#[tokio::test]
async fn both_biko_datenstatus_pids_are_accepted() {
    for pid in [21_003_u32, 21_004] {
        let process = make_process();
        process.execute(receive(1, false)).await.expect("V1");
        process
            .execute(datenstatus(pid, 1, Datenstatus::Pruefdaten))
            .await
            .unwrap_or_else(|e| panic!("PID {pid} must carry a Datenstatus: {e}"));
        let state = process.state().await.expect("state loads");
        assert_eq!(
            state.data().unwrap().version(&v(1)).unwrap().datenstatus,
            Some(Datenstatus::Pruefdaten)
        );
    }
}

/// 21000 / 21001 / 21005 are this participant's own outbound Prüfmitteilungen.
#[tokio::test]
async fn an_outbound_pruefmitteilung_pid_is_refused_as_an_inbound_message() {
    let process = make_process();
    process.execute(receive(1, true)).await.expect("V1");

    for pid in [21_000_u32, 21_001, 21_005] {
        let cmd = BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(pid).expect("valid"),
            version: v(1),
            datenstatus: Some(Datenstatus::Abrechnungsdaten),
            abweisungsgrund: None,
            message_ref: MessageRef::new("IN"),
        };
        assert!(
            process.execute(cmd).await.is_err(),
            "PID {pid} is outbound and must not be recorded as an arrival"
        );
    }
}

// ── Abweisung ────────────────────────────────────────────────────────────────

/// Kap. 9.8.2 Nr. 2 — a rejected Prüfmitteilung is never forwarded, so the
/// check has to be redone.
#[tokio::test]
async fn an_abgewiesene_pruefmitteilung_no_longer_stands() {
    let process = make_process();
    process.execute(receive(1, true)).await.expect("V1");
    process
        .execute(pruefmitteilung(1, "A03", None))
        .await
        .expect("check sent");
    process
        .execute(BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_002).expect("21002"),
            version: v(1),
            datenstatus: None,
            abweisungsgrund: Some("MaBiS-ZP für diesen Monat nicht aktiv".into()),
            message_ref: MessageRef::new("IFTSTA-AB-1"),
        })
        .await
        .expect("Abweisung accepted");

    let state = process.state().await.expect("state loads");
    let rec = state.data().unwrap().version(&v(1)).unwrap();
    assert!(rec.pruefergebnis.is_none());
    assert_eq!(
        rec.pruefmitteilung_abgewiesen.as_deref(),
        Some("MaBiS-ZP für diesen Monat nicht aktiv")
    );

    // Re-checking supersedes the Abweisung.
    process
        .execute(pruefmitteilung(1, "A03", None))
        .await
        .expect("re-check");
    let state = process.state().await.expect("state loads");
    let rec = state.data().unwrap().version(&v(1)).unwrap();
    assert!(rec.pruefergebnis.is_some());
    assert!(rec.pruefmitteilung_abgewiesen.is_none());
}

// ── Clearing window ──────────────────────────────────────────────────────────

/// Kap. 3.10 — after the clearing window closes, nothing can change the
/// settlement.
#[tokio::test]
async fn a_closed_settlement_takes_no_version_and_no_check() {
    let process = make_process();
    process.execute(receive(1, true)).await.expect("V1");
    process
        .execute(BillingCommand::CloseClearing {
            lauf: Abrechnungslauf::Bka,
        })
        .await
        .expect("window closed");

    let state = process.state().await.expect("state loads");
    assert!(matches!(state, BillingState::Geschlossen(_)));

    assert!(process.execute(receive(2, false)).await.is_err());
    assert!(
        process
            .execute(pruefmitteilung(1, "A03", None))
            .await
            .is_err()
    );
}

// ── Validation guards ────────────────────────────────────────────────────────

/// Kap. 3.8.3 — a Kategorie-C series carries neither a Prüfmitteilung nor a
/// Datenstatus, so it never enters a settlement stream.
#[tokio::test]
async fn a_kategorie_c_series_is_refused() {
    let process = make_process();
    let cmd = BillingCommand::ReceiveSummenzeitreihe {
        pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID).expect("13003"),
        zeitreihe: Zeitreihe::new(Familie::BkSzr, Some(Kategorie::C)).expect("row"),
        mabis_zp: mabis_zp(),
        bilanzierungsmonat: BillingPeriod::new("2026-01"),
        version: v(1),
        im_erstaufschlag: true,
        absender: MarktpartnerCode::new("9900357000004"),
        biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
        message_ref: MessageRef::new("MSCONS-C"),
    };
    assert!(process.execute(cmd).await.is_err());
}

/// A stream settles exactly one Summenzeitreihe; a second one belongs in its
/// own stream, because its Fristen and its Kategorie differ.
#[tokio::test]
async fn a_second_zeitreihe_is_refused_in_the_same_stream() {
    let process = make_process();
    process.execute(receive(1, true)).await.expect("V1");

    let cmd = BillingCommand::ReceiveSummenzeitreihe {
        pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID).expect("13003"),
        zeitreihe: Zeitreihe::new(Familie::BkSzr, Some(Kategorie::B)).expect("row"),
        mabis_zp: mabis_zp(),
        bilanzierungsmonat: BillingPeriod::new("2026-01"),
        version: v(2),
        im_erstaufschlag: false,
        absender: MarktpartnerCode::new("9900357000004"),
        biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
        message_ref: MessageRef::new("MSCONS-BK"),
    };
    assert!(process.execute(cmd).await.is_err());
}

/// A Prüfmitteilung always refers to a version that was actually received
/// (Kap. 3.8.3).
#[tokio::test]
async fn a_check_on_an_unknown_version_is_refused() {
    let process = make_process();
    process.execute(receive(1, true)).await.expect("V1");
    assert!(
        process
            .execute(pruefmitteilung(2, "A03", None))
            .await
            .is_err()
    );
}

// ── Projection ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn projection_tracks_the_highest_version_and_its_status() {
    let store = InMemoryEventStore::new();
    let process: Process<MabisBillingWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    );

    process.execute(receive(1, true)).await.expect("V1");
    process
        .execute(datenstatus(21_003, 1, Datenstatus::Abrechnungsdaten))
        .await
        .expect("V1 status");
    process
        .execute(pruefmitteilung(1, "A02", Some("Abweichung")))
        .await
        .expect("negative check");

    let mut projection = BillingProjection::default();
    ProjectionRunner::run(
        &mut projection,
        &store.events_for(process.stream_id()).await,
    );

    let record = projection
        .records
        .get(process.stream_id().as_str())
        .expect("stream recorded");
    assert_eq!(record.status, "Offen");
    assert_eq!(record.hoechste_version, Some(v(1)));
    assert_eq!(record.datenstatus, Some(Datenstatus::Abrechnungsdaten));
    assert_eq!(record.offene_korrekturen, vec![v(1)]);
    assert_eq!(record.zeitreihe, Some(bg_szr()));

    // A new version clears the correction need and drops the stale status.
    process.execute(receive(2, false)).await.expect("V2");
    let mut projection = BillingProjection::default();
    ProjectionRunner::run(
        &mut projection,
        &store.events_for(process.stream_id()).await,
    );
    let record = projection
        .records
        .get(process.stream_id().as_str())
        .expect("stream recorded");
    assert_eq!(record.hoechste_version, Some(v(2)));
    assert_eq!(record.datenstatus, None);
    assert!(record.offene_korrekturen.is_empty());
}

/// `A06` is „Zeitreihe akzeptiert" in `E_0041` and is not published by
/// `E_0062` at all. Because the command names a code rather than a verdict,
/// the mistake is caught instead of being sent as a positive check.
#[tokio::test]
async fn a_code_from_another_trees_codeliste_is_refused() {
    let process = make_process();
    process.execute(receive(1, true)).await.expect("V1");

    let lf_szr = Zeitreihe::new(Familie::LfSzr, Some(Kategorie::B)).expect("LF-SZR Kategorie B");
    assert_eq!(lf_szr.pruef_ebd(), Some("E_0041"));
    assert_eq!(bg_szr().pruef_ebd(), Some("E_0062"));

    let err = process
        .execute(pruefmitteilung(1, "A06", None))
        .await
        .expect_err("E_0062 does not publish A06");
    assert!(format!("{err}").contains("E_0062"), "{err}");
}

/// The two refusal clusters differ in whether the BIKO forwards the
/// Prüfmitteilung at all (MaBiS Kap. 9.8.2 Nr. 2).
#[test]
fn an_abweisung_is_negative_but_not_forwarded() {
    let dublette = Pruefergebnis::negativ(bg_szr(), "A01", "Zeitreihe bereits vorhanden")
        .expect("E_0062 publishes A01");
    let abweichung =
        Pruefergebnis::negativ(bg_szr(), "A02", "Abweichung").expect("E_0062 publishes A02");
    assert!(!dublette.ist_positiv() && !abweichung.ist_positiv());
    assert!(!dublette.wird_weitergeleitet());
    assert!(abweichung.wird_weitergeleitet());
}

/// A tree's Zustimmung cannot be smuggled onto a negative Prüfmitteilung.
#[test]
fn the_zustimmungscode_is_refused_as_a_rejection() {
    assert!(Pruefergebnis::negativ(bg_szr(), "A03", "…").is_err());
    assert!(
        Pruefergebnis::negativ(bg_szr(), "A99", "…").is_err(),
        "E_0062 has no A99"
    );
}
