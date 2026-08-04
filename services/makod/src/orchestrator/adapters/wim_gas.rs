//! WiM Gas adapter registries.
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
// ── WiM Gas INVOIC billing (PIDs 31003, 31004) ───────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimGasInvoicWorkflow`].
///
/// Extracts INVOIC fields to construct a [`WimGasInvoicCommand::ReceiveInvoic`]
/// for WiM Gas billing PIDs 31003 (WiM-Rechnung) and 31004 (Stornorechnung).
///
/// Deadline: 10 Werktage per BK7-24-01-009 §5.
#[must_use]
pub fn wim_gas_invoic_registry() -> AdapterRegistry<WimGasInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas INVOIC adapter".into(),
                )
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas INVOIC adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Gas INVOIC adapter: PID not found in INVOIC BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
            let invoice_ref = inv
                .bgm()
                .and_then(|b| b.document_id.as_deref())
                .unwrap_or(msg.message_ref());

            Ok(WimGasInvoicCommand::ReceiveInvoic {
                pid,
                sender: MarktpartnerCode::new(
                    inv.sender()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                recipient: MarktpartnerCode::new(
                    inv.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                invoice_ref: MessageRef::new(invoice_ref),
                document_date: inv
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Gas billing — REMADV payment advice (PIDs 33001/33002) ────────────────

/// Build an [`AdapterRegistry`] for REMADV 33001/33002 routed to
/// [`WimGasInvoicWorkflow`] (gMSB invoicer role).
///
/// After the gMSB sends INVOIC 31003/31004, the NB (payer) returns a REMADV;
/// `makod` resumes the billing process with [`WimGasInvoicCommand::ReceiveRemadv`].
/// Gas has no itemized Abweisungen (33003/34 are Strom-only), so the set is
/// 33001/33002. Mirrors `wim_invoic_remadv_registry`, duplicated per Sparte.
#[must_use]
pub fn wim_gas_invoic_remadv_registry() -> AdapterRegistry<WimGasInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas REMADV adapter".into(),
                )
            })?;
            let AnyMessage::Remadv(r) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas REMADV adapter: expected REMADV message (PIDs 33001/33002)".into(),
                ));
            };
            let pid = r
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Gas REMADV adapter: PID not found in REMADV BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            Ok(WimGasInvoicCommand::ReceiveRemadv {
                pid,
                remadv_ref: MessageRef::new(msg.message_ref()),
                sender: MarktpartnerCode::new(
                    r.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
            })
        },
    ));
    registry
}

// ── WiM Gas billing — COMDIS payment rejection (PID 29001) ────────────────────

/// Build an [`AdapterRegistry`] for COMDIS 29001 routed to [`WimGasInvoicWorkflow`].
///
/// After the NB sends a REMADV, the gMSB (invoicer) may reject it via COMDIS 29001;
/// `makod` resumes with [`WimGasInvoicCommand::ReceiveComdis`].
#[must_use]
pub fn wim_gas_invoic_comdis_registry() -> AdapterRegistry<WimGasInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas COMDIS adapter".into(),
                )
            })?;
            let AnyMessage::Comdis(_) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas COMDIS adapter: expected COMDIS message (PID 29001)".into(),
                ));
            };
            Ok(WimGasInvoicCommand::ReceiveComdis {
                comdis_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── WiM Gas Stornierung — GNB side (PID 44022) ───────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimGasStornierungWorkflow`].
///
/// Handles inbound PID 44022 (Anfrage nach Stornierung) from LF → GNB.
/// Produces [`WimGasStornierungCommand::ReceiveUtilmd`].
///
/// The Vorgangsnummer from `IDE+24` is used as the process correlation key.
/// Regulatory basis: BK7-24-01-009, WiM Gas (Msb/Nmsb/all deployment roles).
#[must_use]
pub fn wim_gas_stornierung_registry() -> AdapterRegistry<WimGasStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas Stornierung adapter: expected UTILMD message (PID 44022)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Gas Stornierung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
            Ok(WimGasStornierungCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                vorgang_id: MaLo::new(
                    u.transactions()
                        .first()
                        .and_then(|t| t.ide.object_id.as_deref())
                        .unwrap_or(""),
                ),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Gas Anmeldung / Ende / Vorläufige Abmeldung (PIDs 44042–44053) ───────

/// Build an [`AdapterRegistry`] for [`WimGasAnmeldungWorkflow`].
///
/// Extracts UTILMD G fields to construct a [`WimGasAnmeldungCommand::ReceiveUtilmd`]
/// for inbound PIDs 44042–44053 (Anmeldung neuer MSB Gas / Ende MSB Gas).
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
/// Saturdays, Sundays and public holidays are not Werktage.
///
/// # AHB validation note
///
/// WiM Gas PIDs 44039–44053, 44168–44170 have full AHB profiles in `fv20251001_gas`
/// and `fv20261001_gas` (9+ segment rules each). The `pid_has_ahb_rules()` guard
/// below is retained as a permanent defensive check against future import gaps.
#[must_use]
pub fn wim_gas_anmeldung_registry() -> AdapterRegistry<WimGasAnmeldungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas Anmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas Anmeldung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Gas Anmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let validation_result = msg.validate().ok();
            let mut validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

            // Vacuous-validation guard: WiM Gas PIDs 44042–44053 have no AHB
            // profile yet. Without a profile, validate() returns Ok(valid=true)
            // (zero rules checked). Force validation_passed = false until profiles
            // are imported via `cargo xtask import-xml-ahb`.
            if validation_passed {
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);
                if !has_ahb_rules {
                    tracing::warn!(
                        pid = pid.as_u32(),
                        "WiM Gas Anmeldung adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

            Ok(WimGasAnmeldungCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                malo_id: mako_engine::types::MaLo::new(
                    u.transactions()
                        .first()
                        .and_then(|t| t.ide.object_id.as_deref())
                        .unwrap_or(""),
                ),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
                received_at: time::OffsetDateTime::now_utc(),
            })
        },
    ));
    registry
}

// ── WiM Gas Kündigung (PIDs 44039–44041) ─────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimGasKuendigungWorkflow`].
///
/// Routes UTILMD G messages with PIDs 44039–44041 (WiM Gas Kündigung MSB Gas).
/// Note: PIDs 44022–44024 are WiM Gas Stornierung (routed by `WimGasModule` → `wim-gas-stornierung`);
/// the `GeliGasStornierungWorkflow` is only used for the startup policy-coverage check.
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
#[must_use]
pub fn wim_gas_kuendigung_registry() -> AdapterRegistry<WimGasKuendigungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas Kündigung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas Kündigung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Gas Kündigung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let validation_result = msg.validate().ok();
            let mut validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
            // Vacuous-validation guard (same pattern as PIDs 44039–44053).
            if validation_passed {
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);
                if !has_ahb_rules {
                    tracing::warn!(
                        pid = pid.as_u32(),
                        "WiM Gas Kündigung adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }
            Ok(WimGasKuendigungCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                malo_id: mako_engine::types::MaLo::new(
                    u.transactions()
                        .first()
                        .and_then(|t| t.ide.object_id.as_deref())
                        .unwrap_or(""),
                ),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Gas Verpflichtungsanfrage (PIDs 44168–44170) ─────────────────────────

/// Build an [`AdapterRegistry`] for [`WimGasVerpflichtungsanfrageWorkflow`].
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
#[must_use]
pub fn wim_gas_verpflichtungsanfrage_registry()
-> AdapterRegistry<WimGasVerpflichtungsanfrageWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas Verpflichtungsanfrage adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas Verpflichtungsanfrage adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Gas Verpflichtungsanfrage adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            // PID 44170 (Ablehnung Verpflichtungsanfrage) was removed in PID 4.0
            // (FV2026-10-01). Reject it for any format version other than FV2025-10-01.
            if pid.as_u32() == 44170 && fv != &FormatVersion::new("FV2025-10-01") {
                return Err(EngineError::Deserialization(format!(
                    "PID 44170 (Ablehnung Verpflichtungsanfrage) is not valid under \
                     format version {fv} — it was removed in FV2026-10-01 (PID 4.0 \u{26a0}\u{fe0f}). \
                     Only FV2025-10-01 messages may carry this PID."
                )));
            }
            let validation_result = msg.validate().ok();
            let mut validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
            // Vacuous-validation guard (same pattern as PIDs 44039–44053).
            if validation_passed {
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);
                if !has_ahb_rules {
                    tracing::warn!(
                        pid = pid.as_u32(),
                        "WiM Gas Verpflichtungsanfrage adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }
            Ok(WimGasVerpflichtungsanfrageCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                malo_id: mako_engine::types::MaLo::new(
                    u.transactions()
                        .first()
                        .and_then(|t| t.ide.object_id.as_deref())
                        .unwrap_or(""),
                ),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Gas INSRPT (PIDs 23001/23003/23004/23005/23008/23009) ─────────────────

/// Build an [`AdapterRegistry`] for [`WimGasInsrptWorkflow`] (WiM Gas, 10 WT).
///
/// Handles inbound INSRPT messages for fault/inspection reporting between LF
/// and gMSB in the WiM Gas process.  Covers both the outbound Störungsmeldung
/// (23001) and all inbound gMSB responses, including Gas-only variants:
/// 23005 (Ablehnung Gas) and 23009 (Ergebnisbericht Gas).
///
/// In combined Strom+Gas deployments the ingest layer must supply `Sparte::Gas`
/// when calling [`PidRouter::route_with_sparte`] so that this workflow is
/// selected instead of [`wim_insrpt_registry`] (5 WT).
///
/// [`PidRouter::route_with_sparte`]: mako_engine::pid_router::PidRouter::route_with_sparte
#[must_use]
pub fn wim_gas_insrpt_registry() -> AdapterRegistry<WimGasInsrptWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas INSRPT adapter".into(),
                )
            })?;
            let AnyMessage::Insrpt(insrpt) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas INSRPT adapter: expected INSRPT message".into(),
                ));
            };
            let pid = insrpt
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Gas INSRPT adapter: PID not found in INSRPT BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            let message_ref = MessageRef::new(
                insrpt
                    .bgm()
                    .and_then(|b| b.document_id.as_deref())
                    .unwrap_or(msg.message_ref()),
            );
            Ok(GasStorungsmeldungCommand::ReceiveResponse {
                pid,
                response_ref: message_ref,
                reason: None,
            })
        },
    ));
    registry
}

// ── WiM Gas Geräteübernahme (ORDERS 17001/17002/17009) ───────────────────────

/// Build an [`AdapterRegistry`] for [`WimGasGeraeteubernahmeWorkflow`].
///
/// Maps inbound ORDERS by PID (WiM Gas AWH V2.0 §4.2):
/// - `17001`/`17002` (Anfrage) → [`GasGeraeteubernahmeCommand::ReceiveAnfrage`]
/// - `17009` (Ankündigung Gerätewechselabsicht) →
///   [`GasGeraeteubernahmeCommand::ReceiveStornierung`]
///
/// MeLo from the `IDE` segment (element 1, component 0); the Anfrage `DeviceId`
/// from the first `RFF` reference (element 0, component 1). Gas twin of
/// [`super::wim_geraeteubernahme_registry`]; `msg.validate()` resolves the Gas
/// ORDERS AHB profile automatically.
#[must_use]
pub fn wim_gas_geraeteubernahme_registry() -> AdapterRegistry<WimGasGeraeteubernahmeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Gas Geräteübernahme adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Gas Geräteübernahme adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Gas Geräteübernahme adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

            let sender =
                MarktpartnerCode::new(o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""));
            let receiver = MarktpartnerCode::new(
                o.receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );
            let document_date = o
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();
            let message_ref = MessageRef::new(msg.message_ref());

            let melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            if matches!(pid.as_u32(), 17001 | 17002) {
                let device_id = DeviceId::new(
                    o.segments()
                        .iter()
                        .find(|s| s.tag == "RFF")
                        .and_then(|s| s.component_str(0, 1))
                        .unwrap_or(""),
                );
                Ok(GasGeraeteubernahmeCommand::ReceiveAnfrage {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    device_id,
                    document_date,
                    message_ref,
                    validation_passed,
                    validation_errors,
                })
            } else {
                // 17009 — Ankündigung Gerätewechselabsicht.
                Ok(GasGeraeteubernahmeCommand::ReceiveStornierung { pid, message_ref })
            }
        },
    ));
    registry
}
