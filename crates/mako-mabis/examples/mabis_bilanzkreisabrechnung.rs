//! # MaBiS Bilanzkreisabrechnung — mako-mabis end-to-end example
//!
//! Demonstrates the full write→store→read cycle for the settlement of one
//! Bilanzierungsgebietssummenzeitreihe (BG-SZR Kategorie B) over one
//! Bilanzierungsmonat, using the `mako-engine` event-sourced runtime and
//! `mako-mabis` domain logic.
//!
//! ## What the run shows
//!
//! 1. **Erstaufschlag** — version 1 arrives inside the 1.–10. WT window, so the
//!    BIKO assigns „Abrechnungsdaten" automatically (Kap. 3.8.3). Nobody has to
//!    check it first, and no deadline is registered, because the Festlegung
//!    defines none for a Prüfmitteilung.
//! 2. **Clearingphase** — the NB checks version 1 negatively. The Datenstatus
//!    stays where it was: „Eine negative Prüfmitteilung verändert nicht den
//!    Datenstatus einer Summenzeitreihe" (Kap. 3.8.3).
//! 3. **Correction** — version 2 arrives in the Clearingphase as „Prüfdaten"
//!    and a positive Prüfmitteilung promotes it to „Abrechnungsdaten".
//! 4. **Close** — the clearing window (30. WT) lapses and the settlement stops
//!    accepting versions.
//!
//! Every window in the run comes from [`mako_mabis::Bilanzierungsmonat`], which
//! is Tabelle 2 of Kap. 3.10 executable.
//!
//! ## Run
//!
//! ```text
//! cargo run --example mabis_bilanzkreisabrechnung -p mako-mabis
//! ```

use mako_engine::{
    builder::EngineBuilder,
    event_store::{EventStore, InMemoryEventStore},
    ids::TenantId,
    outbox::{InMemoryOutboxStore, OutboxMessage, OutboxStore},
    projection::ProjectionRunner,
    registry::{InMemoryProcessRegistry, ProcessRegistry, RegistryKey},
    snapshot::InMemorySnapshotStore,
    types::{BikoId, BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    Abrechnungslauf, Bilanzierungsmonat, BillingCommand, BillingProjection, BillingState,
    Datenstatus, Familie, Kategorie, MabisBillingWorkflow, MabisZaehlpunktId, SUMMENZEITREIHE_PID,
    SzrVersion, Zeitreihe,
};
use time::{Date, Month};

/// Build a version from an ordinal — the wire form is an Erstellungszeitpunkt
/// (IFTSTA `RFF+AUU`), so the ordinals become ascending seconds.
fn v(n: u32) -> SzrVersion {
    SzrVersion::new(format!("202601011200{n:02}+00")).expect("17 characters")
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  mako-mabis — Bilanzkreisabrechnung Strom example          ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_outbox_store(InMemoryOutboxStore::new())
        .with_registry(InMemoryProcessRegistry::new())
        .build();

    const SNAP_INTERVAL: u64 = 2;

    let zeitreihe = Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B))?;
    let mabis_zp = MabisZaehlpunktId::new("DE0001111222233334444555566667777")?;
    let biko = BikoId::new("10YDE-VE-TRANSMIX");
    let absender = MarktpartnerCode::new("9900357000004");

    // ── The Fristenkalender for this month ───────────────────────────────────
    let monat = Bilanzierungsmonat::new(Date::from_calendar_date(2026, Month::January, 31)?);
    let erstaufschlag = monat
        .erstaufschlag(zeitreihe, Abrechnungslauf::Bka)
        .expect("BG-SZR has an Erstaufschlag window");
    let clearing = monat
        .clearing(zeitreihe, Abrechnungslauf::Bka)
        .expect("BG-SZR has a clearing window");
    let stichtag = monat.abrechnungsrelevante_bilanzierung(Abrechnungslauf::Bka);

    println!("  Zeitreihe      : {zeitreihe}");
    println!("  MaBiS-ZP       : {mabis_zp}");
    println!("  Monat          : 2026-01 (Ende {})", monat.monatsende());
    println!(
        "  Erstaufschlag  : {} … {}   (1.–10. WT)",
        erstaufschlag.von, erstaufschlag.bis
    );
    println!(
        "  Clearing       : {} … {}   (11.–30. WT)",
        clearing.von, clearing.bis
    );
    println!(
        "  Abrechnung     : {} auf Datenstand {}   (42. WT / 30. WT)",
        stichtag.faellig, stichtag.datenstand
    );
    println!();

    let process = ctx.spawn::<MabisBillingWorkflow>(
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    );
    println!("  Stream : {}", process.stream_id());
    println!();

    // ── Step 1: Erstaufschlag ────────────────────────────────────────────────
    let eingang_v1 = monat.werktag(4);
    let phase_v1 = monat.phase(zeitreihe, eingang_v1);
    println!("[1/5] Version 1 arrives on {eingang_v1} — Phase {phase_v1:?}");

    let envs = process
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID)?,
            zeitreihe,
            mabis_zp: mabis_zp.clone(),
            bilanzierungsmonat: BillingPeriod::new("2026-01"),
            version: v(1),
            im_erstaufschlag: phase_v1.ist_erstaufschlag(),
            absender: absender.clone(),
            biko_id: biko.clone(),
            message_ref: MessageRef::new("MSCONS-BG-2026-01-V1"),
        })
        .await?;
    for env in &envs {
        println!(
            "  ✓ {} (seq {}, schema_v{})",
            env.event_type, env.sequence_number, env.schema_version
        );
    }
    println!(
        "  → inside the Erstaufschlag window, so the BIKO assigns \
         „Abrechnungsdaten\" without a Prüfmitteilung (Kap. 3.8.3)."
    );

    process
        .execute(BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_003)?,
            version: v(1),
            datenstatus: Some(Datenstatus::Abrechnungsdaten),
            abweisungsgrund: None,
            message_ref: MessageRef::new("IFTSTA-DS-V1"),
        })
        .await?;

    // ── Step 2: negative Prüfmitteilung in the Clearingphase ─────────────────
    println!();
    println!("[2/5] The NB checks version 1 and rejects it.");
    let pruef_envs = process
        .execute(BillingCommand::SendPruefmitteilung {
            version: v(1),
            pid: Pruefidentifikator::new(21_005)?,
            antwortcode: "A02".into(),
            grund: Some("Summe weicht um 12 kWh von der eigenen Aggregation ab".into()),
            message_ref: MessageRef::new("IFTSTA-PM-V1"),
        })
        .await?;
    for env in &pruef_envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    let state = process.state().await?;
    let v1 = state
        .data()
        .and_then(|d| d.version(&v(1)))
        .expect("V1 recorded");
    println!(
        "  Datenstatus of V1 after the negative check: {:?}",
        v1.datenstatus
    );
    assert_eq!(
        v1.datenstatus,
        Some(Datenstatus::Abrechnungsdaten),
        "Kap. 3.8.3 — a negative Prüfmitteilung does not change the Datenstatus"
    );
    println!("  → unchanged, exactly as Kap. 3.8.3 requires.");

    // The outbound Prüfmitteilung goes on the wire through the outbox.
    let pruef_env = &pruef_envs[0];
    ctx.outbox_store()
        .enqueue(&[OutboxMessage::new(
            process.stream_id().clone(),
            pruef_env.process_id,
            pruef_env.tenant_id,
            pruef_env.correlation_id,
            pruef_env.conversation_id,
            pruef_env.event_id,
            "IFTSTA",
            biko.as_str(),
            serde_json::json!({
                "pid":                 21_005,
                "message_ref":         "IFTSTA-PM-V1",
                "bilanzierungsmonat":  "2026-01",
                "version":             1,
                "positiv":             false,
            }),
        )])
        .await?;
    println!(
        "  [outbox] Prüfmitteilung queued ({} pending)",
        ctx.outbox_store().len().await?
    );

    let snapped = process
        .take_snapshot(ctx.snapshot_store(), SNAP_INTERVAL)
        .await?;
    println!(
        "  [snap] Snapshot taken: {snapped} (event count {})",
        process.event_count().await?
    );

    // ── Step 3: the correction ───────────────────────────────────────────────
    println!();
    let eingang_v2 = monat.werktag(17);
    let phase_v2 = monat.phase(zeitreihe, eingang_v2);
    println!("[3/5] Version 2 arrives on {eingang_v2} — Phase {phase_v2:?}");
    process
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID)?,
            zeitreihe,
            mabis_zp,
            bilanzierungsmonat: BillingPeriod::new("2026-01"),
            version: v(2),
            im_erstaufschlag: phase_v2.ist_erstaufschlag(),
            absender,
            biko_id: biko,
            message_ref: MessageRef::new("MSCONS-BG-2026-01-V2"),
        })
        .await?;
    process
        .execute(BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_003)?,
            version: v(2),
            datenstatus: Some(Datenstatus::Pruefdaten),
            abweisungsgrund: None,
            message_ref: MessageRef::new("IFTSTA-DS-V2a"),
        })
        .await?;
    println!("  → filed after the Erstaufschlag, so it arrives as „Prüfdaten\".");

    process
        .execute(BillingCommand::SendPruefmitteilung {
            version: v(2),
            pid: Pruefidentifikator::new(21_005)?,
            antwortcode: "A03".into(),
            grund: None,
            message_ref: MessageRef::new("IFTSTA-PM-V2"),
        })
        .await?;
    process
        .execute(BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_003)?,
            version: v(2),
            datenstatus: Some(Datenstatus::Abrechnungsdaten),
            abweisungsgrund: None,
            message_ref: MessageRef::new("IFTSTA-DS-V2b"),
        })
        .await?;
    println!("  → a positive Prüfmitteilung promotes it to „Abrechnungsdaten\".");

    let state = process.state_with_snapshot(ctx.snapshot_store()).await?;
    let data = state.data().expect("open");
    println!(
        "  Abrechnungsrelevante Version: {}",
        data.abrechnungsrelevante_version()
            .map_or_else(|| "—".to_owned(), |v| v.version.to_string())
    );
    assert!(data.offener_korrekturbedarf().is_empty());

    // ── Step 4: the window closes ────────────────────────────────────────────
    println!();
    println!(
        "[4/5] The clearing window closes on {} (30. WT).",
        clearing.bis
    );
    process
        .execute(BillingCommand::CloseClearing {
            lauf: Abrechnungslauf::Bka,
        })
        .await?;
    let state = process.state().await?;
    println!("  Status : {}", state.status_str());
    assert!(matches!(state, BillingState::Geschlossen(_)));

    // ── Step 5: projections and guards ───────────────────────────────────────
    println!();
    println!("[5/5] Projections and guards...");
    let all_events = ctx.event_store().load(process.stream_id()).await?;
    let mut proj = BillingProjection::default();
    ProjectionRunner::run(&mut proj, &all_events);
    if let Some(rec) = proj.records.get(process.stream_id().as_str()) {
        println!(
            "  Status {} · höchste Version {:?} · Datenstatus {:?} · {} Events",
            rec.status, rec.hoechste_version, rec.datenstatus, rec.event_count
        );
    }

    println!();
    println!("  [+] Guard: a version after the window closed is refused...");
    let guard_err = process
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID)?,
            zeitreihe,
            mabis_zp: MabisZaehlpunktId::new("DE0001111222233334444555566667777")?,
            bilanzierungsmonat: BillingPeriod::new("2026-01"),
            version: v(3),
            im_erstaufschlag: false,
            absender: MarktpartnerCode::new("9900357000004"),
            biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
            message_ref: MessageRef::new("MSCONS-BG-2026-01-V3"),
        })
        .await
        .unwrap_err();
    assert!(
        guard_err
            .as_workflow_error()
            .is_some_and(|we| we.is_invalid_state())
    );
    println!("  ✓ Rejected (invalid state): {guard_err}");

    println!();
    println!("  [+] Guard: registry lookup...");
    ctx.registry()
        .register(
            process.tenant_id(),
            &RegistryKey::from_static("bg-szr-b-2026-01"),
            process.identity(),
        )
        .await?;
    let found = ctx
        .registry()
        .lookup(
            process.tenant_id(),
            &RegistryKey::from_static("bg-szr-b-2026-01"),
        )
        .await?
        .expect("must be registered");
    assert_eq!(found.process_id, process.process_id());
    let resumed = ctx.resume::<MabisBillingWorkflow>(found);
    println!(
        "  ✓ Resumed process event count: {}",
        resumed.event_count().await?
    );

    println!();
    println!("  [+] Guard: outbox delivery and drain...");
    let pending = ctx.outbox_store().pending_now(10).await?;
    assert_eq!(pending.len(), 1, "expected one pending Prüfmitteilung");
    ctx.outbox_store()
        .acknowledge(pending[0].message_id)
        .await?;
    println!(
        "  ✓ Outbox drained ({} remaining)",
        ctx.outbox_store().len().await?
    );

    println!();
    println!("══════════════════════════════════════════════════════════════════");
    println!("  All checks passed — mako-mabis round-trip OK.");
    println!("══════════════════════════════════════════════════════════════════");

    Ok(())
}
