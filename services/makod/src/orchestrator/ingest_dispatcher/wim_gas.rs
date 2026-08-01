//! WiM Gas ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the WiM Gas workflow family:
    /// `wim-gas-anmeldung`
    /// `wim-gas-kuendigung`
    /// `wim-gas-verpflichtungsanfrage`
    /// `wim-gas-invoic`
    /// `wim-gas-insrpt`
    /// `wim-gas-stornierung`
    pub(super) async fn dispatch_wim_gas(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── WiM Gas Anmeldung — PIDs 44042–44044, 44051–44053 ────────────
            // PIDs 44042–44044: Anmeldung neuer MSB Gas (MSBN ↔ NB) — spawn.
            // PIDs 44051–44053: Ende MSB Gas / Vorläufige Abmeldung (NB ↔ MSBA) — spawn.
            "wim-gas-anmeldung" => match pid {
                44042..=44044 | 44051..=44053 => {
                    let cmd = adapters::wim_gas_anmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasAnmeldungWorkflow>(
                        malo_id.as_str(),
                        "wim-gas-anmeldung",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::anmeldung::RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-anmeldung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Gas Kündigung — PIDs 44039/44040/44041 ───────────────────
            // PID 44039: Kündigung MSB Gas Anfrage (MSBA → NB) — spawn.
            // PIDs 44040/44041: Bestätigung/Ablehnung (NB → MSBA) — spawn (NB-initiating path).
            "wim-gas-kuendigung" => match pid {
                44039..=44041 => {
                    let cmd = adapters::wim_gas_kuendigung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasKuendigungWorkflow>(
                        malo_id.as_str(),
                        "wim-gas-kuendigung",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::kuendigung::RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-kuendigung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Gas Verpflichtungsanfrage — PIDs 44168/44169/44170 ───────
            // PID 44168: Verpflichtungsanfrage (NB → gMSB) — spawn.
            // PIDs 44169/44170: Bestätigung/Ablehnung (gMSB → NB) — spawn.
            //
            // PID 44170 present in FV2025-10-01 (PID 3.3), absent from FV2026-10-01
            // (PID 4.0).  In-flight FV2025 processes may still receive it after the
            // cutover — the adapter handles it for forward compatibility.
            "wim-gas-verpflichtungsanfrage" => match pid {
                44168..=44170 => {
                    let cmd =
                        adapters::wim_gas_verpflichtungsanfrage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasVerpflichtungsanfrageWorkflow>(
                        malo_id.as_str(),
                        "wim-gas-verpflichtungsanfrage",
                        cmd,
                        &fv,
                        &[(
                            mako_wim_gas::verpflichtungsanfrage::RESPONSE_WINDOW_LABEL,
                            due_at,
                        )],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-verpflichtungsanfrage",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── INVOIC billing hosted by the generic invoic workflow ─────────
            // PID 31003: WiM-Rechnung Gas (gMSB → NB).
            // PID 31004: Stornorechnung — the Sparte-neutral universal Storno
            //   (INVOIC AHB §3.1.2), co-hosted here because its receive→settle/
            //   dispute machine is commodity-agnostic.
            "wim-gas-invoic" => match pid {
                31003 | 31004 => {
                    let cmd = adapters::wim_gas_invoic_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_invoic(msg);
                    // Settlement-response deadline.
                    //
                    // - PID 31004 (Storno): the invoice's Fälligkeitsdatum / Zahlungsziel
                    //   (DTM+265). The receiver must answer "zum Zahlungsziel" — the one
                    //   rule that holds for Strom *and* Gas (the Gas 10-Werktage is only a
                    //   sender-side floor already baked into that date). Fall back to
                    //   +10 Werktage only when the invoice omits it.
                    // - PID 31003 (WiM Gas Rechnung): 10 Werktage floor (BK7-24-01-009).
                    let due_at = if pid == 31004 {
                        faelligkeitsdatum_from_invoic(msg)
                    } else {
                        None
                    }
                    .unwrap_or_else(|| {
                        fristen::deadline_at_werktage(
                            OffsetDateTime::now_utc(),
                            10,
                            HolidayCalendar::BdewMaKo,
                        )
                    });
                    self.spawn_or_resume::<WimGasInvoicWorkflow>(
                        malo_id.as_str(),
                        "wim-gas-invoic",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::invoic::SETTLEMENT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-invoic",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Gas INSRPT — PIDs 23001/23003–23005/23008/23009 ──────────
            // PID 23001: Anfrage Störungsmeldung (shared, gMSB → NB) — spawn.
            // PIDs 23003/23004/23008: Antwort (shared, NB → gMSB) — spawn.
            // PIDs 23005/23009: Gas-only variants — spawn.
            "wim-gas-insrpt" => match pid {
                23001 | 23003 | 23004 | 23005 | 23008 | 23009 => {
                    let cmd = adapters::wim_gas_insrpt_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // INSRPT Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasInsrptWorkflow>(
                        malo_id.as_str(),
                        "wim-gas-insrpt",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::insrpt::ANTWORT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-insrpt",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Gas Stornierung — GNB side (PID 44022) ───────────────────
            // PID 44022: Anfrage nach Stornierung (LF → GNB) — spawn.
            // Response PIDs 44023/44024 are outbound (dispatched by the ERP layer); no
            // inbound arm needed on the GNB side.
            "wim-gas-stornierung" => match pid {
                44022 => {
                    let cmd = adapters::wim_gas_stornierung_registry().dispatch(raw, &fv)?;
                    let vorgang_id = extract_melo_from_utilmd(msg);
                    // WiM Gas: 10 Werktage response deadline (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasStornierungWorkflow>(
                        &vorgang_id,
                        "wim-gas-stornierung",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::STORNIERUNG_RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-stornierung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
