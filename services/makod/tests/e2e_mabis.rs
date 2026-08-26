//! End-to-end test: MaBiS Bilanzkreisabrechnung Strom.
//!
//! Models one settlement — a single MaBiS-Zählpunkt over a single
//! Bilanzierungsmonat — from the first version through the Clearingphase to the
//! close of the window (BNetzA BK6-24-174 Anlage 3, Kap. 3.8 and 3.10).
//!
//! # Lifecycle trace
//!
//! ```text
//!   BIKO / ÜNB                            this participant
//!   ──────────────────────────────────────────────────────────────────────────
//!   Summenzeitreihe V1 (Erstaufschlag) ─►  ReceiveSummenzeitreihe
//!   IFTSTA 21003 Datenstatus A01       ─►  „Abrechnungsdaten" (automatic)
//!                                      ◄─  IFTSTA 21005 Prüfmitteilung negativ
//!                                          Datenstatus unchanged (Kap. 3.8.3)
//!   Summenzeitreihe V2 (Clearingphase) ─►  ReceiveSummenzeitreihe
//!   IFTSTA 21003 Datenstatus A02       ─►  „Prüfdaten"
//!                                      ◄─  IFTSTA 21005 Prüfmitteilung positiv
//!   IFTSTA 21003 Datenstatus A01       ─►  promoted to „Abrechnungsdaten"
//!   (30. WT)                               CloseClearing
//!   ──────────────────────────────────────────────────────────────────────────
//! ```
//!
//! # What changed and why
//!
//! This test previously asserted a **1-Werktag Prüfmitteilung deadline** citing
//! BK6-24-174 §13.8. There is no such deadline: Kap. 9.8.2 Nr. 1 leaves the
//! Frist cell empty and says the receiving party „kann" answer, and Kap. 13.8.2
//! defines no answer at all — its two rows are the BIKO's own dispatch dates
//! (18. WT vorläufig, 42. WT endgültig). What bounds a Prüfmitteilung is the
//! clearing window of Tabelle 2, which is what this test now exercises.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{BikoId, BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    Abrechnungslauf, Bilanzierungsmonat, BillingCommand, BillingState, Datenstatus, Familie,
    Kategorie, MabisBillingWorkflow, MabisZaehlpunktId, Phase, SUMMENZEITREIHE_PID, SzrVersion,
    Zeitreihe,
};
use time::{Date, Month};

// ── Constants ────────────────────────────────────────────────────────────────

const BKV_ID: &str = "4033872000022";
const UENB_ID: &str = "9900357000004";
const BIKO_ID: &str = "10YDE-VE-TRANSMIX";
const MABIS_ZP: &str = "DE0001111222233334444555566667777";
const MONAT: &str = "2026-01";
const FV: &str = "FV2025-10-01";

fn zeitreihe() -> Zeitreihe {
    Zeitreihe::new(Familie::BkSzr, Some(Kategorie::B)).expect("BK-SZR Kategorie B")
}

fn monat() -> Bilanzierungsmonat {
    Bilanzierungsmonat::new(Date::from_calendar_date(2026, Month::January, 31).expect("valid date"))
}

fn version(n: u32) -> SzrVersion {
    SzrVersion::new(format!("202601011200{n:02}+00")).expect("17 characters")
}

// ── Mock participant ─────────────────────────────────────────────────────────

/// The BKV's settlement stream for one MaBiS-Zählpunkt and Bilanzierungsmonat.
struct MockBkv {
    process: Process<MabisBillingWorkflow, InMemoryEventStore>,
}

impl MockBkv {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(BKV_ID),
                WorkflowId::new("mabis-billing", FV),
            ),
        }
    }

    /// A version arrives. `eingang` decides the phase, which decides the
    /// Datenstatus the BIKO will assign (Kap. 3.8.3) — so it comes from the
    /// Fristenkalender rather than from the message.
    async fn receive(&self, n: u32, eingang: Date) -> Result<Phase, String> {
        let phase = monat().phase(zeitreihe(), eingang);
        self.process
            .execute(BillingCommand::ReceiveSummenzeitreihe {
                pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID).expect("13003"),
                zeitreihe: zeitreihe(),
                mabis_zp: MabisZaehlpunktId::new(MABIS_ZP).expect("33 characters"),
                bilanzierungsmonat: BillingPeriod::new(MONAT),
                version: version(n),
                im_erstaufschlag: phase.ist_erstaufschlag(),
                absender: MarktpartnerCode::new(UENB_ID),
                biko_id: BikoId::new(BIKO_ID),
                message_ref: MessageRef::new(format!("MSCONS-BK-2026-01-V{n}")),
            })
            .await
            .map(|_| phase)
            .map_err(|e| e.to_string())
    }

    /// The BIKO assigns a Datenstatus (IFTSTA 21003 → NB/ÜNB, 21004 → BKV).
    async fn datenstatus(&self, pid: u32, n: u32, status: Datenstatus) -> Result<(), String> {
        self.process
            .execute(BillingCommand::ReceiveIftsta {
                pid: Pruefidentifikator::new(pid).expect("valid PID"),
                version: version(n),
                datenstatus: Some(status),
                abweisungsgrund: None,
                message_ref: MessageRef::new(format!("IFTSTA-DS-V{n}")),
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// This participant sends its Prüfmitteilung (IFTSTA 21005 → BIKO).
    /// `E_0064` decides a BK-SZR (Kategorie B): `A03` is its Zustimmung,
    /// `A02` its Ablehnung.
    async fn pruefmitteilung(
        &self,
        n: u32,
        antwortcode: &str,
        grund: Option<&str>,
    ) -> Result<(), String> {
        self.process
            .execute(BillingCommand::SendPruefmitteilung {
                version: version(n),
                pid: Pruefidentifikator::new(21_005).expect("21005"),
                antwortcode: antwortcode.to_owned(),
                grund: grund.map(ToOwned::to_owned),
                message_ref: MessageRef::new(format!("IFTSTA-PM-V{n}")),
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn close(&self, lauf: Abrechnungslauf) -> Result<(), String> {
        self.process
            .execute(BillingCommand::CloseClearing { lauf })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn state(&self) -> BillingState {
        self.process.state().await.expect("state loads")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Erstaufschlag → negative check → correction → promotion → close.
#[tokio::test]
async fn full_settlement_lifecycle() {
    let bkv = MockBkv::new();
    let m = monat();

    // ── Erstaufschlag (1.–12. WT for a BK-SZR) ───────────────────────────────
    let phase = bkv.receive(1, m.werktag(4)).await.expect("V1 accepted");
    assert_eq!(phase, Phase::Erstaufschlag);
    bkv.datenstatus(21_003, 1, Datenstatus::Abrechnungsdaten)
        .await
        .expect("BIKO assigns Abrechnungsdaten automatically");

    // ── The check is negative; the Datenstatus does not move ─────────────────
    bkv.pruefmitteilung(
        1,
        "A02",
        Some("Summe weicht um 12 kWh von der eigenen Aggregation ab"),
    )
    .await
    .expect("negative Prüfmitteilung");

    let state = bkv.state().await;
    let data = state.data().expect("open");
    assert_eq!(
        data.version(&version(1)).expect("V1").datenstatus,
        Some(Datenstatus::Abrechnungsdaten),
        "Kap. 3.8.3 — „Eine negative Prüfmitteilung verändert nicht den Datenstatus\""
    );
    assert_eq!(data.offener_korrekturbedarf(), vec![&version(1)]);

    // ── Clearingphase (13.–30. WT) ───────────────────────────────────────────
    let phase = bkv.receive(2, m.werktag(20)).await.expect("V2 accepted");
    assert_eq!(phase, Phase::Clearing);
    bkv.datenstatus(21_003, 2, Datenstatus::Pruefdaten)
        .await
        .expect("filed after the Erstaufschlag → Prüfdaten");
    bkv.pruefmitteilung(2, "A03", None)
        .await
        .expect("positive Prüfmitteilung");
    bkv.datenstatus(21_003, 2, Datenstatus::Abrechnungsdaten)
        .await
        .expect("a positive check promotes it");

    let state = bkv.state().await;
    let data = state.data().expect("open");
    assert!(data.offener_korrekturbedarf().is_empty());
    assert_eq!(
        data.abrechnungsrelevante_version().map(|r| &r.version),
        Some(&version(2))
    );

    // ── The window closes on the 30. WT ──────────────────────────────────────
    bkv.close(Abrechnungslauf::Bka)
        .await
        .expect("window closes");
    assert!(matches!(bkv.state().await, BillingState::Geschlossen(_)));
}

/// The BK-SZR Erstaufschlag runs two Werktage longer than the BG-SZR's — a
/// version filed on the 11. WT is still an Erstaufschlag for one and already
/// Clearing for the other (Kap. 3.10 Tabelle 2).
#[tokio::test]
async fn the_erstaufschlag_window_differs_per_summenzeitreihe() {
    let m = monat();
    let bg = Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B)).expect("row");
    let bk = Zeitreihe::new(Familie::BkSzr, Some(Kategorie::B)).expect("row");

    assert_eq!(m.phase(bg, m.werktag(11)), Phase::Clearing);
    assert_eq!(m.phase(bk, m.werktag(11)), Phase::Erstaufschlag);
    assert_eq!(m.phase(bk, m.werktag(13)), Phase::Clearing);
}

/// Kap. 3.8.2 — versions ascend across the whole BKA.
#[tokio::test]
async fn a_repeated_version_is_refused() {
    let bkv = MockBkv::new();
    let m = monat();
    bkv.receive(3, m.werktag(4)).await.expect("V3");
    assert!(bkv.receive(3, m.werktag(5)).await.is_err());
    assert!(bkv.receive(2, m.werktag(5)).await.is_err());
    bkv.receive(4, m.werktag(6)).await.expect("V4");
}

/// 21000, 21001 and 21005 are this participant's own outbound Prüfmitteilungen.
#[tokio::test]
async fn an_outbound_pruefmitteilung_cannot_arrive() {
    let bkv = MockBkv::new();
    bkv.receive(1, monat().werktag(4)).await.expect("V1");
    for pid in [21_000_u32, 21_001, 21_005] {
        let err = bkv
            .process
            .execute(BillingCommand::ReceiveIftsta {
                pid: Pruefidentifikator::new(pid).expect("valid"),
                version: version(1),
                datenstatus: Some(Datenstatus::Abrechnungsdaten),
                abweisungsgrund: None,
                message_ref: MessageRef::new("IN"),
            })
            .await;
        assert!(err.is_err(), "PID {pid} is outbound");
    }
}

/// Both Datenstatus PIDs are accepted; which one arrives follows from the role.
#[tokio::test]
async fn both_datenstatus_pids_are_accepted() {
    for pid in [21_003_u32, 21_004] {
        let bkv = MockBkv::new();
        bkv.receive(1, monat().werktag(20)).await.expect("V1");
        bkv.datenstatus(pid, 1, Datenstatus::Pruefdaten)
            .await
            .unwrap_or_else(|e| panic!("PID {pid} must carry a Datenstatus: {e}"));
        let state = bkv.state().await;
        assert_eq!(
            state
                .data()
                .unwrap()
                .version(&version(1))
                .unwrap()
                .datenstatus,
            Some(Datenstatus::Pruefdaten)
        );
    }
}

/// The BIKO can reject a Prüfmitteilung (IFTSTA 21002); it is then never
/// forwarded, so the check has to be redone (Kap. 9.8.2 Nr. 2).
#[tokio::test]
async fn an_abgewiesene_pruefmitteilung_no_longer_stands() {
    let bkv = MockBkv::new();
    bkv.receive(1, monat().werktag(4)).await.expect("V1");
    bkv.pruefmitteilung(1, "A03", None)
        .await
        .expect("check sent");
    bkv.process
        .execute(BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_002).expect("21002"),
            version: version(1),
            datenstatus: None,
            abweisungsgrund: Some("MaBiS-ZP für diesen Monat nicht aktiv".into()),
            message_ref: MessageRef::new("IFTSTA-AB-1"),
        })
        .await
        .expect("Abweisung accepted");

    let state = bkv.state().await;
    let rec = state.data().unwrap().version(&version(1)).unwrap();
    assert!(rec.pruefergebnis.is_none(), "the check no longer stands");
    assert!(rec.pruefmitteilung_abgewiesen.is_some());
}

/// After the clearing window closes nothing can change the settlement
/// (Kap. 3.10 Tabelle 2).
#[tokio::test]
async fn a_closed_settlement_refuses_versions_and_checks() {
    let bkv = MockBkv::new();
    bkv.receive(1, monat().werktag(4)).await.expect("V1");
    bkv.close(Abrechnungslauf::Bka).await.expect("closed");

    assert!(bkv.receive(2, monat().werktag(20)).await.is_err());
    assert!(bkv.pruefmitteilung(1, "A03", None).await.is_err());
}

/// The Abrechnungsstichtage of Tabelle 2 sit *after* the clearing window, so a
/// „abgerechnete Daten" Datenstatus still arrives on a closed settlement.
#[tokio::test]
async fn the_abrechnungsstichtag_follows_the_clearing_window() {
    let m = monat();
    let clearing = m
        .clearing(zeitreihe(), Abrechnungslauf::Bka)
        .expect("BK-SZR has one");
    let stichtag = m.abrechnungsrelevante_bilanzierung(Abrechnungslauf::Bka);

    assert_eq!(clearing.bis, m.werktag(30));
    assert_eq!(stichtag.faellig, m.werktag(42));
    assert_eq!(stichtag.datenstand, m.werktag(30));
    assert!(
        stichtag.faellig > clearing.bis,
        "the BIKO settles on a data cut-off that is already closed"
    );
}
