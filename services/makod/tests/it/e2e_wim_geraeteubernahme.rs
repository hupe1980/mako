//! End-to-end test: MSBN → MSBA WiM Geräteübernahme Bestellung (ORDERS 17001).
//!
//! The abgebender Messstellenbetreiber (MSBA) receives the Bestellung against a
//! standing QUOTES 15001 Angebot and answers with ORDRSP 19001 or 19002 — the
//! **cluster of the Antwortcode** picking which.
//!
//! ```text
//!   MSBN ERP (wire fixture)                    MSBA ERP (MockMsba)
//!   ─────────────────────────────────────────────────────────────────
//!                        ──── ORDERS 17001 ───►
//!                                               receive_bestellung(wire)
//!                                                 → adapter: ReceiveOrders
//!                                                 → state: ValidationPassed
//!                                               antworten("Z13")
//!                                                 → ORDRSP 19001, AJT Z13:S_0067
//!                                                 → state: Beantwortet
//!   ─────────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **ORDERS 17001** Bestellung Geräteübernahme (MSBN → MSBA). The
//!   *Anforderung* that precedes it is REQOTE 35001 and belongs to
//!   `wim-preisanfrage` — 17001 is not both.
//! - **Antwortfrist 2 Werktage** nach dem ÜT der Bestellung (WiM Strom Teil 1
//!   Kap. 3.2.2 Nr. 4 / AWH WiM Gas 2.0 Kap. 4.2.2 Nr. 4). Not five: no WiM
//!   chapter states five for this step.
//! - **`SG2 AJT`** carries DE 4465 (the Prüfschritt code) and DE 1082 (the
//!   **Codeliste**, `S_0067`/`S_0068` in Strom, `G_0061`/`G_0074` in Gas) —
//!   ORDRSP AHB 1.1b Kap. 4. Not the EBD number.
//! - Saturdays, Sundays and public holidays are not Werktage.

use mako_engine::process::RunOptions;
use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::Sparte,
    version::{FormatVersion, WorkflowId},
};
use mako_wim::{
    GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL, GeraeteubernahmeCommand, GeraeteubernahmeState,
    WimGeraeteubernahmeWorkflow,
};
use makod::adapters::wim_geraeteubernahme_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSBN_ID: &str = "9900357000004"; // incoming MSB (sender of ORDERS)
const MSBA_ID: &str = "4012345000023"; // outgoing MSB (receiver, this party)
const MELO_ID: &str = "E0000000000000000001"; // Messlokation
const DEVICE_ID: &str = "DEV-001"; // Gerätenummer (from RFF)
const FV: &str = "FV2025-10-01";

// ── ORDERS 17001 wire fixture ─────────────────────────────────────────────────
//
// Minimal EDIFACT ORDERS — Bestellung Geräteübernahme (PID 17001).
// Direction: MSBN (NAD+MS) → MSBA (NAD+MR).
// `DTM+203` carries the Übernahmezeitpunkt — `[UB3]` in ORDERS AHB 1.1b
// Kap. 4.9.2, a day boundary: 01.03.2025 00:00 MEZ is `202502282300` UTC;
// `DTM+137` is the document date.
const ORDERS_17001_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:500+4012345000023:14+250115:0800+WIM-GT-001'\
UNH+MSG-001+ORDERS:D:09B:UN:1.4b'\
BGM+Z55+00017001+9'\
DTM+137:202501150800?+00:303'\
DTM+203:202502282300?+00:303'\
RFF+Z13:DEV-001'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+24+E0000000000000000001'\
UNT+9+MSG-001'\
UNZ+1+WIM-GT-001'";

// ── Mock MSBA ERP backend ─────────────────────────────────────────────────────

/// Simulates the **MSBA ERP** receiving a WiM Geräteübernahme Bestellung.
struct MockMsba {
    process: Process<WimGeraeteubernahmeWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
    sparte: Sparte,
}

impl MockMsba {
    fn new(sparte: Sparte) -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(MSBA_ID),
                WorkflowId::new("wim-geraeteubernahme", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
            sparte,
        }
    }

    /// Adapt the wire bytes, forcing the AHB verdict.
    ///
    /// The minimal fixture does not satisfy every profile rule; AHB conformance
    /// is tested separately in the `edi-energy` suite.
    fn adapt(&self, wire: &[u8], validation_passed: bool) -> GeraeteubernahmeCommand {
        let raw = self.platform.parse(wire).expect("parse ORDERS wire bytes");
        let unh_ref = raw.message_ref().to_owned();
        assert!(!unh_ref.is_empty(), "UNH message_ref must be non-empty");

        let cmd = wim_geraeteubernahme_registry(time::OffsetDateTime::now_utc(), self.sparte)
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("adapt ORDERS 17001 to GeraeteubernahmeCommand");

        match cmd {
            GeraeteubernahmeCommand::ReceiveOrders {
                pid,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                termin,
                message_ref,
                sparte,
                received_at,
                ..
            } => {
                assert_eq!(pid.as_u32(), 17001, "adapter must extract PID 17001");
                assert_eq!(sender.as_str(), MSBN_ID, "sender must be the MSBN");
                assert_eq!(receiver.as_str(), MSBA_ID, "receiver must be the MSBA");
                assert_eq!(melo_id.as_str(), MELO_ID, "MeLo must match IDE+24");
                assert_eq!(device_id.as_str(), DEVICE_ID, "Gerätenummer from RFF");
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve the UNH message_ref",
                );
                GeraeteubernahmeCommand::ReceiveOrders {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    device_id,
                    document_date,
                    termin,
                    message_ref,
                    validation_passed,
                    validation_errors: if validation_passed {
                        vec![]
                    } else {
                        vec!["missing required IDE".to_owned()]
                    },
                    sparte,
                    received_at,
                }
            }
            _ => panic!("expected GeraeteubernahmeCommand::ReceiveOrders"),
        }
    }

    async fn receive_bestellung(&self, wire: &[u8]) {
        self.process
            .run(self.adapt(wire, true), RunOptions::new())
            .await
            .expect("execute ReceiveOrders 17001");
    }

    /// Answer with an Antwortcode; the cluster picks 19001 or 19002.
    async fn antworten(&self, code: &str) {
        self.process
            .run(
                GeraeteubernahmeCommand::DispatchAntwort {
                    antwort_code: code.to_owned(),
                    bemerkung: None,
                },
                RunOptions::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("execute DispatchAntwort {code}: {e}"));
    }

    async fn state(&self) -> GeraeteubernahmeState {
        self.process.state().await.expect("must load state")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// The happy path: a Bestellung is confirmed with `Z13` on ORDRSP 19001.
#[tokio::test]
async fn wim_geraeteubernahme_bestellung_is_confirmed() {
    let msba = MockMsba::new(Sparte::Strom);

    msba.receive_bestellung(ORDERS_17001_BYTES).await;
    let state = msba.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::ValidationPassed(_)),
        "after ReceiveOrders the MSBA owes an answer; got: {state:?}",
    );

    msba.antworten("Z13").await;
    let state = msba.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::Beantwortet(_)),
        "a Zustimmung leaves the process awaiting the physical transfer; got: {state:?}",
    );
}

/// `Z32` „Bestellumfang übersteigt Angebotsumfang" is an Ablehnung, so it rides
/// 19002 and closes the process — the cluster decides, not the caller.
#[tokio::test]
async fn wim_geraeteubernahme_bestellung_is_refused() {
    let msba = MockMsba::new(Sparte::Strom);

    msba.receive_bestellung(ORDERS_17001_BYTES).await;
    msba.antworten("Z32").await;

    let state = msba.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::Abgelehnt { .. }),
        "an Ablehnungscode closes the process; got: {state:?}",
    );
}

/// The same ORDERS answered in Gas resolves against `E_2011`, and `S_0067` is
/// not a code list the Gas market publishes.
#[tokio::test]
async fn wim_geraeteubernahme_gas_uses_its_own_tree() {
    let msba = MockMsba::new(Sparte::Gas);

    msba.receive_bestellung(&orders_17001("202503010500")).await;
    msba.antworten("Z13").await;

    let state = msba.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::Beantwortet(_)),
        "Gas publishes Z13 in E_2011 through G_0061; got: {state:?}",
    );
}

/// A code from another tree never reaches the wire: `ZB4` belongs to the
/// Gerätewechselabsicht (`E_0204`), not to the Bestellung.
#[tokio::test]
async fn wim_geraeteubernahme_refuses_a_foreign_code() {
    let msba = MockMsba::new(Sparte::Strom);

    msba.receive_bestellung(ORDERS_17001_BYTES).await;
    let err = msba
        .process
        .run(
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "ZB4".to_owned(),
                bemerkung: None,
            },
            RunOptions::new(),
        )
        .await
        .expect_err("ZB4 is not published in E_0247");
    assert!(err.to_string().contains("E_0247"), "{err}");
}

#[tokio::test]
async fn wim_geraeteubernahme_validation_failure_rejects() {
    let msba = MockMsba::new(Sparte::Strom);

    msba.process
        .run(msba.adapt(ORDERS_17001_BYTES, false), RunOptions::new())
        .await
        .expect("execute");

    let state = msba.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::Abgelehnt { .. }),
        "validation failure must reject; got: {state:?}",
    );
}

/// An ORDERS 17001 with `DTM+203` set to `stamp` (`CCYYMMDDHHMM`, UTC).
fn orders_17001(stamp: &str) -> Vec<u8> {
    String::from_utf8(ORDERS_17001_BYTES.to_vec())
        .expect("fixture is ASCII")
        .replace("DTM+203:202502282300", &format!("DTM+203:{stamp}"))
        .into_bytes()
}

/// The adapter's `termin` for an ORDERS 17001 read in `sparte`.
fn termin_of(wire: &[u8], sparte: Sparte) -> Result<Option<String>, String> {
    let raw = Platform::with_all_profiles()
        .parse(wire)
        .expect("parse ORDERS wire bytes");
    match wim_geraeteubernahme_registry(time::OffsetDateTime::now_utc(), sparte)
        .dispatch(&raw as &dyn Any, &FormatVersion::new(FV))
    {
        Ok(GeraeteubernahmeCommand::ReceiveOrders { termin, .. }) => Ok(termin),
        Ok(_) => panic!("expected ReceiveOrders"),
        Err(e) => Err(e.to_string()),
    }
}

/// The Übernahmezeitpunkt is a `303` day boundary (`[UB3]`): the UTC instant
/// the German day begins — 00:00 Europe/Berlin in Strom, the Gastag's 06:00 in
/// Gas (Allgemeine Festlegungen V6.1d Kap. 3.1). It reads back as the German
/// date, never as its first eight digits, which in Strom are the day before.
#[test]
fn a_17001_on_the_first_of_january_reads_as_that_date() {
    assert_eq!(
        termin_of(&orders_17001("202612312300"), Sparte::Strom),
        Ok(Some("20270101".to_owned())),
    );
    assert_eq!(
        termin_of(&orders_17001("202701010500"), Sparte::Gas),
        Ok(Some("20270101".to_owned())),
    );
    // Summer time: 01.07.2027 00:00 MESZ.
    assert_eq!(
        termin_of(&orders_17001("202706302200"), Sparte::Strom),
        Ok(Some("20270701".to_owned())),
    );
}

/// An instant that begins no German day in the Sparte is refused, never
/// rounded to one: UTC midnight is 01:00 Berlin.
#[test]
fn a_17001_termin_that_begins_no_day_is_refused() {
    let err = termin_of(&orders_17001("202701010000"), Sparte::Strom)
        .expect_err("UTC midnight begins no Strom day");
    assert!(err.contains("DTM+203"), "{err}");
    // A Strom day boundary is not a Gastag's.
    assert!(termin_of(&orders_17001("202612312300"), Sparte::Gas).is_err());
}

#[test]
fn wim_geraeteubernahme_deadline_label_is_canonical() {
    assert_eq!(
        GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL, "wim-geraeteubernahme-ordrsp-deadline",
        "deadline label must match the canonical form expected by deadline_dispatch.rs",
    );
}
