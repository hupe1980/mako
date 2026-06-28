//! `MessageAdapter` registries for all domain workflows.
//!
//! This module is the **wiring layer** between raw parsed `AnyMessage` values
//! (produced by `edi-energy`) and typed domain commands (consumed by each
//! workflow). It is the only place in the codebase that knows about both sides.
//!
//! # Design rationale
//!
//! Domain crates (`mako-gpke`, `mako-wim`, …) contain **pure domain logic**
//! and must never import `edi-energy`. The field-extraction code that maps
//! wire-format EDIFACT segments to domain command fields lives here, where
//! both `edi-energy` and the domain crates are visible.
//!
//! # Cross-FV behaviour
//!
//! Each registry registers a single `FnAdapter` that accepts **all known BDEW
//! format versions** (FV ≥ `FV2024-10-01`). The UTILMD S2.x wire format for
//! the fields used by the current workflows has been stable across all current
//! BDEW format versions. When a future release changes field layout, add an
//! internal branch on `fv` inside the adapter closure.
//!
//! # Adding a new format version
//!
//! 1. Add the new `FormatVersion` to `known_fvs()` below.
//! 2. If the wire format changed, branch inside the adapter closure.
//! 3. Rebuild and verify the startup `validate_policy` check passes.

use std::any::Any;

use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_engine::{
    error::EngineError,
    message_adapter::{AdapterRegistry, FnAdapter},
    types::{DeviceId, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::FormatVersion,
};

/// Convert an `edi_energy::Pruefidentifikator` to the domain `Pruefidentifikator`.
///
/// This is the only permitted crossing point between the two crates for PID values.
/// The `edi-energy` type guarantees the code is already in the 10 000–99 999 range,
/// so the conversion must always succeed.
#[inline]
fn convert_pid(p: edi_energy::Pruefidentifikator) -> Result<Pruefidentifikator, EngineError> {
    Pruefidentifikator::new(p.as_u32())
        .map_err(|e| EngineError::Deserialization(format!("PID out of range: {e}")))
}
use mako_geli_gas::{
    GasSperrungCommand, GasSupplierChangeCommand, GeliGasSperrungWorkflow,
    GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    AbrechnungCommand, GpkeAbrechnungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeNeuanlageWorkflow, GpkeSperrungWorkflow,
    GpkeSupplierChangeWorkflow, KonfigurationCommand, LfAbmeldungCommand, LfAnmeldungCommand,
    NeuanlageCommand, SperrungCommand, SupplierChangeCommand,
};
use mako_mabis::{BillingCommand, DataStatus, IFTSTA_DATENSTATUS_PID, MabisBillingWorkflow};
use mako_wim::{
    DeviceChangeCommand, GeraeteubernahmeCommand, StammdatenCommand, StornierungCommand,
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimRechnungCommand, WimRechnungWorkflow,
    WimStammdatenWorkflow, WimStornierungWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungWorkflow, WimGasKuendigungCommand,
    WimGasKuendigungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageWorkflow,
};

// ── GPKE UTILMD Anfrage (PIDs 55001, 55002, 55017, 56001–56004) ───────────────

/// Build an [`AdapterRegistry`] for [`GpkeSupplierChangeWorkflow`].
///
/// Registers one adapter covering all current BDEW format versions.
/// Extracts UTILMD S2.x fields to construct a
/// [`SupplierChangeCommand::ReceiveUtilmd`] for the 7 inbound ANFRAGE PIDs:
/// 55001–55002 (Lieferbeginn/Lieferende), 55017 (Kündigung), and 56001–56004
/// (ex-MPES feed-in site). Outbound ANTWORT PIDs (55003–55006, 55018) and
/// PID 55555 (Sperrung) are handled separately.
#[must_use]
pub fn gpke_registry() -> AdapterRegistry<GpkeSupplierChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                // IFTSTA Vollzugsmeldung (PIDs 21024–21033) are also routed to
                // the gpke-supplier-change workflow. Handle them here.
                if let AnyMessage::Iftsta(_) = msg {
                    return build_gpke_iftsta_command(msg);
                }
                return Err(EngineError::Deserialization(
                    "GPKE adapter: expected UTILMD or IFTSTA message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!("GPKE adapter: PID detection failed: {e}"))
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

            // ── ex-MPES PIDs 56001–56004 — vacuous-validation guard ───
            // PIDs 56001–56004 (Einspeisestelle, transferred from MPES to GPKE
            // per BK6-22-024, effective 2025-06-06) have no UTILMD AHB profile
            // yet. Without a profile, validate() returns Ok(valid=true) because
            // zero rules are checked — a "vacuous pass" that is indistinguishable
            // from a genuine pass.
            //
            // We detect this by querying the registry: if no registered UTILMD
            // profile has AHB rules for this PID, validation was vacuous and we
            // force validation_passed = false to prevent dispatching an
            // unvalidated message as "validated".
            //
            // This guard self-corrects once the profile is imported:
            //   cargo xtask import-xml-ahb
            if matches!(pid.as_u32(), 56001..=56004) && validation_passed {
                // Re-construct the edi_energy PID for the registry query.
                // The new() call is infallible here — the PID was already
                // accepted by convert_pid() earlier in this closure.
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);

                if !has_ahb_rules {
                    tracing::error!(
                        pid = pid.as_u32(),
                        message_ref = msg.message_ref(),
                        "GPKE adapter: PID {} (ex-MPES Einspeisestelle) has no UTILMD \
                         AHB profile — validation was vacuous. Forcing \
                         validation_passed = false to prevent a false positive. \
                         Import the profile with \
                         `cargo xtask import-xml-ahb` to restore processing.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

            Ok(SupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                location_id: MaLo::new(
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
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.dtm.iter().find(|d| d.is_period_start()))
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

// ── GPKE UTILMD Sperrung (PID 55555) ─────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungWorkflow`].
///
/// Extracts UTILMD S2.x fields from an inbound Anweisung Sperrung (PID 55555)
/// to construct a [`SperrungCommand::ReceiveSperrung`].
#[must_use]
pub fn gpke_sperrung_registry() -> AdapterRegistry<GpkeSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE Sperrung adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung adapter: PID detection failed: {e}"
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

            Ok(SperrungCommand::ReceiveSperrung {
                pid,
                sender: mako_engine::types::MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                location_id: mako_engine::types::MaLo::new(
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
                message_ref: mako_engine::types::MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GPKE INVOIC billing (PIDs 56005–56010) ───────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeAbrechnungWorkflow`].
///
/// Extracts INVOIC 2.x fields to construct an
/// [`AbrechnungCommand::ReceiveInvoic`] for any of the INVOIC-based GPKE
/// billing PIDs (31001–31008: Netznutzungsabrechnung, Mehr-/Mindermengen Strom).
#[must_use]
pub fn gpke_abrechnung_registry() -> AdapterRegistry<GpkeAbrechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Abrechnung adapter".into(),
                )
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Abrechnung adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GPKE Abrechnung adapter: PID not found in INVOIC BGM".into(),
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

            Ok(AbrechnungCommand::ReceiveInvoic {
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

// ── WiM INVOIC billing (PIDs 31003, 31009) ───────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimRechnungWorkflow`].
///
/// Extracts INVOIC fields to construct a [`WimRechnungCommand::ReceiveInvoic`]
/// for WiM-domain billing PIDs 31003 (WiM-Rechnung) and 31009 (MSB-Rechnung).
/// These PIDs are explicitly excluded from `mako-gpke`'s INVOIC_PIDS.
#[must_use]
pub fn wim_rechnung_registry() -> AdapterRegistry<WimRechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for WiM Rechnung adapter".into())
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Rechnung adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Rechnung adapter: PID not found in INVOIC BGM".into(),
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

            Ok(WimRechnungCommand::ReceiveInvoic {
                pruefidentifikator: pid,
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

// ── WiM Gerätewechsel (PID 11001) ───────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimDeviceChangeWorkflow`].
#[must_use]
pub fn wim_registry() -> AdapterRegistry<WimDeviceChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for WiM adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                // IFTSTA status messages (PIDs 21009–21018) are also routed to
                // the wim-device-change workflow. Handle them here.
                if let AnyMessage::Iftsta(_) = msg {
                    return build_wim_iftsta_command(msg);
                }
                return Err(EngineError::Deserialization(
                    "WiM adapter: expected UTILMD or IFTSTA message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!("WiM adapter: PID detection failed: {e}"))
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

            // WiM uses MeLo (Messlokation) as the object ID.
            let melo_id = MeLo::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.ide.object_id.as_deref())
                    .unwrap_or(""),
            );
            // Device ID from the first transaction reference (EIC / AGS).
            let device_id = DeviceId::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.references.first())
                    .and_then(|r| r.reference.as_deref())
                    .unwrap_or(""),
            );

            Ok(DeviceChangeCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                melo_id,
                device_id,
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

// ── WiM Steuerungsauftrag — REST-only, no EDIFACT adapter ───────────────────
//
// `WimSteuerungsauftragWorkflow` is driven exclusively through the
// BDEW API-Webdienste Strom `controlMeasuresV1` REST channel. It has no
// EDIFACT Prüfidentifikator and receives no AS4 inbound messages.
// No `AdapterRegistry` is registered for this workflow; commands are
// constructed in `energy-api` and submitted directly.

// ── WiM Geräteübernahme (PIDs 17001, 17002, 17005, 17009, 17011) ─────────────

/// Build an [`AdapterRegistry`] for [`WimGeraeteubernahmeWorkflow`].
///
/// Handles all five ORDERS PIDs in the Geräteübernahme family:
/// - `17001`/`17002` (Anfrage) → [`GeraeteubernahmeCommand::ReceiveAnfrage`]
/// - `17005` (Bestellung) → [`GeraeteubernahmeCommand::ReceiveBestellung`]
/// - `17009`/`17011` (Stornierung) → [`GeraeteubernahmeCommand::ReceiveStornierung`]
///
/// The MeLo ID is extracted from the `IDE` segment (element 1, component 0).
/// The `DeviceId` (Anfrage only) is extracted from the first `RFF` segment's
/// reference value (element 0, component 1).
#[must_use]
pub fn wim_geraeteubernahme_registry() -> AdapterRegistry<WimGeraeteubernahmeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Geräteübernahme adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Geräteübernahme adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Geräteübernahme adapter: PID detection failed: {e}"
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

            // MeLo from the IDE segment (element 1, component 0 = object ID).
            let melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            let pid_u32 = pid.as_u32();
            if matches!(pid_u32, 17001 | 17002) {
                // Phase 1: Anfrage Geräteübernahmeangebot — extract DeviceId from
                // the first RFF reference value (element 0, component 1).
                let device_id = DeviceId::new(
                    o.segments()
                        .iter()
                        .find(|s| s.tag == "RFF")
                        .and_then(|s| s.component_str(0, 1))
                        .unwrap_or(""),
                );
                Ok(GeraeteubernahmeCommand::ReceiveAnfrage {
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
            } else if pid_u32 == 17005 {
                // Phase 2: Bestellung Geräteübernahme.
                Ok(GeraeteubernahmeCommand::ReceiveBestellung { pid, message_ref })
            } else {
                // Stornierung (17009 or 17011).
                Ok(GeraeteubernahmeCommand::ReceiveStornierung { pid, message_ref })
            }
        },
    ));
    registry
}

// ── WiM Stammdaten (PID 17101) ───────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimStammdatenWorkflow`].
///
/// Handles the single inbound ORDERS PID 17101 (Anforderung Stammdaten) and
/// produces a [`StammdatenCommand::ReceiveAnforderung`].
///
/// The MeLo ID is extracted from the `IDE` segment (element 1, component 0).
#[must_use]
pub fn wim_stammdaten_registry() -> AdapterRegistry<WimStammdatenWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Stammdaten adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Stammdaten adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Stammdaten adapter: PID detection failed: {e}"
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

            // MeLo from the IDE segment (element 1, component 0 = object ID).
            let melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            Ok(StammdatenCommand::ReceiveAnforderung {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Stornierung (PID 39000) ──────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimStornierungWorkflow`].
///
/// Handles the single inbound ORDCHG PID 39000 (Stornierung) and produces a
/// [`StornierungCommand::ReceiveOrdchg`].
///
/// - The MeLo ID is extracted from the `IDE` segment (element 1, component 0).
/// - The `cancelled_ref` is extracted from the `RFF` segment with qualifier
///   `Z13` (element 0, component 1), which references the original ORDERS.
#[must_use]
pub fn wim_stornierung_registry() -> AdapterRegistry<WimStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Stornierung adapter".into(),
                )
            })?;

            let AnyMessage::Ordchg(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Stornierung adapter: expected ORDCHG message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Stornierung adapter: PID detection failed: {e}"
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

            // MeLo from the IDE segment (element 1, component 0 = object ID).
            let melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            // Original ORDERS reference from RFF+Z13 (element 0: qual=comp[0], ref=comp[1]).
            let cancelled_ref = o
                .segments()
                .iter()
                .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some("Z13"))
                .and_then(|s| s.component_str(0, 1))
                .map(MessageRef::new);

            Ok(StornierungCommand::ReceiveOrdchg {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                cancelled_ref,
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GeLi Gas Lieferantenwechsel (PIDs 44001–44006, 44017–44018) ──────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSupplierChangeWorkflow`].
#[must_use]
pub fn geli_gas_registry() -> AdapterRegistry<GeliGasSupplierChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GeLi Gas adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas adapter: PID detection failed: {e}"
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

            Ok(GasSupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                malo_id: MaLo::new(
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

// ── GeLi Gas Sperrung (PID 44555) ────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrungWorkflow`].
///
/// Extracts UTILMD G fields to construct a [`GasSperrungCommand::ReceiveSperrung`]
/// for inbound PID 44555 (Anweisung Sperrung/Entsperrung Gas, GNB → LFN).
///
/// **APERAK Frist:** 10 Werktage (BK7 GeLi Gas 3.0, BK7-24-01-009).
/// Saturday counts as a Werktag; Sunday and public holidays do not.
#[must_use]
pub fn geli_gas_sperrung_registry() -> AdapterRegistry<GeliGasSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung adapter: PID detection failed: {e}"
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

            Ok(GasSperrungCommand::ReceiveSperrung {
                pid,
                gnb: mako_engine::types::MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                lieferant: mako_engine::types::MarktpartnerCode::new(
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
                message_ref: mako_engine::types::MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GPKE Konfigurationseinrichtung (PIDs 19001/19002 — ORDRSP from MSB) ───────

/// Build an [`AdapterRegistry`] for [`GpkeKonfigurationWorkflow`].
///
/// Registers one adapter covering all known BDEW format versions.
/// Extracts ORDRSP fields to construct a [`KonfigurationCommand::ReceiveOrdrsp`]
/// for inbound ORDRSP 19001 (Bestätigung) and 19002 (Ablehnung der Bestellung)
/// from the MSB in response to an outbound ORDERS 17134.
#[must_use]
pub fn gpke_konfiguration_registry() -> AdapterRegistry<GpkeKonfigurationWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Konfiguration adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Konfiguration adapter: expected ORDRSP message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Konfiguration adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // ORDRSP 19001 = Bestätigung (accept), 19002 = Ablehnung (reject).
            let accepted = pid.as_u32() == 19001;

            // For ORDRSP 19002 (Ablehnung), extract the rejection reason from
            // the first FTX segment (DE 4440 / element 3, component 0).
            let reason: Option<String> = if accepted {
                None
            } else {
                o.ftx().first().and_then(|f| f.text.clone())
            };

            Ok(KonfigurationCommand::ReceiveOrdrsp {
                pid,
                accepted,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── MABIS Bilanzkreisabrechnung (PID 13003) ───────────────────────────────────

/// Build an [`AdapterRegistry`] for [`MabisBillingWorkflow`].
///
/// MaBiS billing commands (`ReceiveSummenzeitreihe`, `ReceivePruefmitteilung`,
/// …) for MSCONS PID 13003 are constructed by the billing aggregation layer,
/// not by direct EDIFACT downcast. However, inbound **IFTSTA** messages with
/// MaBiS PIDs 21000–21007 must be handled here so they are not dead-lettered.
///
/// The adapter matches on `AnyMessage::Iftsta` and constructs
/// [`BillingCommand::ReceiveIftsta`]. Any other message type is rejected with
/// an error directing callers to use the aggregation layer.
#[must_use]
pub fn mabis_registry() -> AdapterRegistry<MabisBillingWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        // Accept all FVs.
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for MABIS adapter".into())
            })?;
            match msg {
                AnyMessage::Iftsta(_) => build_mabis_iftsta_command(msg),
                _ => Err(EngineError::Deserialization(
                    "MABIS: MSCONS billing commands must be constructed via the \
                     aggregation layer, not directly from a single EDIFACT message"
                        .into(),
                )),
            }
        },
    ));
    registry
}

// ── GPKE UTILMD Antwort (PIDs 55003–55006, 55018) — LF role ──────────────────

/// Build an [`AdapterRegistry`] for [`GpkeLfAnmeldungWorkflow`].
///
/// Handles inbound NB/LFA response PIDs (55003–55006, 55018) when `makod`
/// acts as the **Lieferant** — i.e. we previously sent the ANFRAGE outbound
/// and are now receiving the NB/LFA acknowledgement.
///
/// `accepted` is derived from the PID:
/// - 55003 (Bestätigung Lieferbeginn), 55005 (Bestätigung Lieferende),
///   55018 (Bestätigung Kündigung) → `accepted = true`
/// - 55004 (Ablehnung Lieferbeginn), 55006 (Ablehnung Lieferende)
///   → `accepted = false`
///
/// An optional rejection reason is extracted from the first `STS` segment's
/// free-text description when present.
#[must_use]
pub fn gpke_lf_anmeldung_registry() -> AdapterRegistry<GpkeLfAnmeldungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE LF-Anmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE LF-Anmeldung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE LF-Anmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // Acceptance is determined by the PID alone per BDEW GPKE AHB.
            let accepted = matches!(pid.as_u32(), 55003 | 55005 | 55018);

            // Extract the rejection reason from the first transaction's FTX
            // segment (typically qualifier AAI or ZZZ in 55004/55006).
            // For acceptance PIDs this is typically absent; returns None.
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());

            Ok(LfAnmeldungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── Known format versions ─────────────────────────────────────────────────────

// The set of BDEW format versions for which all active domain workflows must
// have registered adapters.
// ── IFTSTA extraction helpers ─────────────────────────────────────────────────

/// Extract a [`SupplierChangeCommand::ReceiveVollzugsmeldung`] from an IFTSTA
/// message routed to the GPKE supplier-change workflow (PIDs 21024–21033).
///
/// Called from `gpke_registry()` when `msg` is `AnyMessage::Iftsta`.
fn build_gpke_iftsta_command(msg: &AnyMessage) -> Result<SupplierChangeCommand, EngineError> {
    let AnyMessage::Iftsta(i) = msg else {
        return Err(EngineError::Deserialization(
            "GPKE IFTSTA adapter: expected AnyMessage::Iftsta".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("GPKE IFTSTA adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;
    let validation_result = msg.validate().ok();
    let validation_passed = validation_result
        .as_ref()
        .map(|r| r.is_valid())
        .unwrap_or(false);
    let validation_errors: Vec<String> = validation_result
        .as_ref()
        .map(|r| r.errors().iter().map(|e| format!("{e}")).collect())
        .unwrap_or_default();
    Ok(SupplierChangeCommand::ReceiveVollzugsmeldung {
        pid,
        sender: MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            i.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        message_ref: MessageRef::new(msg.message_ref()),
        validation_passed,
        validation_errors,
    })
}

/// Extract a [`DeviceChangeCommand::ReceiveIftsta`] from an IFTSTA message
/// routed to the WiM device-change workflow (PIDs 21009–21018).
///
/// Called from `wim_registry()` when `msg` is `AnyMessage::Iftsta`.
fn build_wim_iftsta_command(msg: &AnyMessage) -> Result<DeviceChangeCommand, EngineError> {
    let AnyMessage::Iftsta(i) = msg else {
        return Err(EngineError::Deserialization(
            "WiM IFTSTA adapter: expected AnyMessage::Iftsta".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("WiM IFTSTA adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;
    let validation_result = msg.validate().ok();
    let validation_passed = validation_result
        .as_ref()
        .map(|r| r.is_valid())
        .unwrap_or(false);
    let validation_errors: Vec<String> = validation_result
        .as_ref()
        .map(|r| r.errors().iter().map(|e| format!("{e}")).collect())
        .unwrap_or_default();
    Ok(DeviceChangeCommand::ReceiveIftsta {
        pid,
        sender: MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            i.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        message_ref: MessageRef::new(msg.message_ref()),
        validation_passed,
        validation_errors,
    })
}

/// Extract a [`BillingCommand::ReceiveIftsta`] from an IFTSTA message routed
/// to the MABIS billing workflow (PIDs 21000–21007).
///
/// Called from `mabis_registry()` when `msg` is `AnyMessage::Iftsta`.
///
/// For PID 21004 (Datenstatus vom BIKO), the `DataStatus` value is extracted
/// from the STS segment element 2 (DE 9013, status reason):
///
/// | STS element 2 | `DataStatus` variant |
/// |---------------|----------------------|
/// | `Z03`         | `Abrechnungsdaten`   |
/// | `Z49`         | `AbgerechtneteDaten` |
/// | `Z86`         | `AbgerechtneteDatenKbka` |
///
/// Codes are per BDEW MaBiS IFTSTA AHB 2.0g. All other MaBiS PIDs set
/// `data_status = None`.
fn build_mabis_iftsta_command(msg: &AnyMessage) -> Result<BillingCommand, EngineError> {
    let AnyMessage::Iftsta(i) = msg else {
        return Err(EngineError::Deserialization(
            "MABIS IFTSTA adapter: expected AnyMessage::Iftsta".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("MABIS IFTSTA adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;
    let validation_result = msg.validate().ok();
    let validation_passed = validation_result
        .as_ref()
        .map(|r| r.is_valid())
        .unwrap_or(false);
    let validation_errors: Vec<String> = validation_result
        .as_ref()
        .map(|r| r.errors().iter().map(|e| format!("{e}")).collect())
        .unwrap_or_default();

    // For PID 21004 (Statusmeldung vom BIKO an BKV/NB), extract the Datenstatus
    // code from the first STS segment at element index 2 (DE 9013, status reason).
    //
    // IFTSTA MaBiS AHB 2.0g STS segment: STS+status_category+status_code+status_reason
    // The BDEW Datenstatus qualifier occupies element position 2 (0-based).
    let data_status = if pid.as_u32() == IFTSTA_DATENSTATUS_PID {
        i.segments()
            .iter()
            .find(|s| s.tag == "STS")
            .and_then(|s| s.element_str(2))
            .and_then(|code| match code {
                "Z03" => Some(DataStatus::Abrechnungsdaten),
                "Z49" => Some(DataStatus::AbgerechtneteDaten),
                "Z86" => Some(DataStatus::AbgerechtneteDatenKbka),
                _ => None,
            })
    } else {
        None
    };

    Ok(BillingCommand::ReceiveIftsta {
        pid,
        sender: MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            i.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        message_ref: MessageRef::new(msg.message_ref()),
        validation_passed,
        validation_errors,
        data_status,
    })
}

///
/// Returns `true` when `fv` is in the set of format versions derived from the
/// compiled `edi-energy` profile registry.
///
/// Prefer this over lexicographic `>=` comparisons — a new FV is not
/// automatically supported until `edi-energy` has a profile with that
/// `valid_from` date and the adapter closures above have been verified.
fn is_known_fv(fv: &FormatVersion) -> bool {
    known_fvs().iter().any(|k| k.as_str() == fv.as_str())
}

// ── GPKE Neuanlage (PIDs 55600, 55601) ───────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeNeuanlageWorkflow`].
///
/// Adapts inbound UTILMD PIDs 55600 (neue verbrauchende MaLo) and 55601
/// (neue erzeugende MaLo) from the Lieferant to a
/// [`NeuanlageCommand::ReceiveAnmeldung`].
///
/// AHB validation is performed inline; `validation_passed` is set accordingly.
/// Acceptance/rejection is decided by the NB ERP via a subsequent
/// `SendAntwort` command.
#[must_use]
pub fn gpke_neuanlage_registry() -> AdapterRegistry<GpkeNeuanlageWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Neuanlage adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Neuanlage adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Neuanlage adapter: PID detection failed: {e}"
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

            Ok(NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                location_id: MaLo::new(
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
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.dtm.iter().find(|d| d.qualifier == "92"))
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

// ── GPKE NB-initiated Lieferende (PID 55007) ─────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeLfAbmeldungWorkflow`].
///
/// Adapts inbound UTILMD PID 55007 (Ankündigung Lieferende, NB → LF) to a
/// [`LfAbmeldungCommand::ReceiveAnkuendigung`].
///
/// AHB validation is performed inline; `validation_passed` is set accordingly.
/// The LF ERP responds with a subsequent `SendAntwort` command (within 24h).
#[must_use]
pub fn gpke_lf_abmeldung_registry() -> AdapterRegistry<GpkeLfAbmeldungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE LF-Abmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE LF-Abmeldung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE LF-Abmeldung adapter: PID detection failed: {e}"
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

            Ok(LfAbmeldungCommand::ReceiveAnkuendigung {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                location_id: MaLo::new(
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
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.dtm.iter().find(|d| d.qualifier == "92"))
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

/// Return all BDEW format versions registered in the compiled `edi-energy`
/// profile registry, sorted chronologically.
///
/// This replaces the previously hand-maintained allowlist. Adding a new BDEW
/// format version now only requires shipping a new `edi-energy` profile;
/// `makod` picks it up automatically on the next rebuild.
///
/// If the wire-format *changed* in the new format version, add a branch on
/// `fv` inside the relevant adapter closure above before deploying.
#[must_use]
pub fn known_fvs() -> Vec<FormatVersion> {
    edi_energy::registry::ReleaseRegistry::global()
        .format_versions()
        .into_iter()
        .filter_map(|s| FormatVersion::parse(&s).ok())
        .collect()
}

// ── WiM Gas Anmeldung / Ende / Vorläufige Abmeldung (PIDs 44039–44053) ───────

/// Build an [`AdapterRegistry`] for [`WimGasAnmeldungWorkflow`].
///
/// Extracts UTILMD G fields to construct a [`WimGasAnmeldungCommand::ReceiveUtilmd`]
/// for inbound PIDs 44039–44053 (Anmeldung/Ende/Vorläufige Abmeldung MSB Gas).
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
/// Saturday counts as a Werktag; Sunday and public holidays do not.
///
/// # AHB validation note
///
/// WiM Gas PIDs (44039–44053) are not yet in the `fv*_gas` AHB profile set.
/// Until profiles are imported via `cargo xtask import-xml-ahb`, `msg.validate()`
/// returns a vacuous pass. This function applies the `pid_has_ahb_rules()` guard
/// (same as ex-MPES PIDs 56001–56004) to force `validation_passed = false` when
/// no AHB rules are registered, preventing false positives.
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

            // Vacuous-validation guard: WiM Gas PIDs 44039–44053 have no AHB
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
            })
        },
    ));
    registry
}

// ── WiM Gas Kündigung (PIDs 44022–44024) ─────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimGasKuendigungWorkflow`].
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
            // Vacuous-validation guard (same pattern as PIDs 56001–56004 and 44039–44053).
            if validation_passed {
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);
                if !has_ahb_rules {
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
        |raw: &dyn Any, _fv: &FormatVersion| {
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
            let validation_result = msg.validate().ok();
            let mut validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
            // Vacuous-validation guard (same pattern as PIDs 56001–56004).
            if validation_passed {
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);
                if !has_ahb_rules {
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
