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

use dvgw_edi::AnyDvgwMessage;
use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_engine::{
    error::EngineError,
    message_adapter::{AdapterRegistry, FnAdapter},
    types::{
        BillingPeriod, DeviceId, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator,
    },
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
use mako_gabi_gas::{
    AllocationCommand, GaBiGasAllocationWorkflow, GaBiGasInvoicCommand, GaBiGasInvoicWorkflow,
    GaBiGasNominationWorkflow, NominationCommand, NomresAcceptance,
};
use mako_geli_gas::{
    GasMsconsDatenCommand, GasSperrungLfCommand, GasSperrungNbCommand, GasSupplierChangeCommand,
    GeliGasMsconsWorkflow, GeliGasSperrprozesseInvoicCommand, GeliGasSperrprozesseInvoicWorkflow,
    GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow, GeliGasStornierungCommand,
    GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    AbrechnungCommand, AllokationslisteCommand, AnfrageBestellungCommand,
    AnkuendigungZuordnungLfCommand, GpkeAbrechnungWorkflow, GpkeAllokationslisteWorkflow,
    GpkeAnfrageBestellungWorkflow, GpkeAnkuendigungZuordnungLfWorkflow, GpkeKonfigurationWorkflow,
    GpkeLfAbmeldungWorkflow, GpkeLfAnmeldungWorkflow, GpkeNeuanlageWorkflow,
    GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, GpkeStornierungCommand, GpkeStornierungWorkflow,
    GpkeSupplierChangeWorkflow, KonfigurationCommand, LfAbmeldungCommand, LfAnmeldungCommand,
    NeuanlageCommand, SperrungCommand, SperrungLfCommand, SupplierChangeCommand,
};
use mako_mabis::{
    BillingCommand, ClearinglisteCommand, DataStatus, IFTSTA_DATENSTATUS_PID, MabisBillingWorkflow,
    MabisClearinglisteWorkflow,
};
use mako_wim::{
    DeviceChangeCommand, GeraeteubernahmeCommand, StammdatenCommand, StornierungCommand,
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimInsrptWorkflow, WimRechnungCommand,
    WimRechnungWorkflow, WimStammdatenWorkflow, WimStornierungWorkflow,
    insrpt::StorungsmeldungCommand,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungWorkflow, WimGasInsrptWorkflow, WimGasInvoicCommand,
    WimGasInvoicWorkflow, WimGasKuendigungCommand, WimGasKuendigungWorkflow,
    WimGasVerpflichtungsanfrageCommand, WimGasVerpflichtungsanfrageWorkflow,
    insrpt::GasStorungsmeldungCommand,
};

// ── GPKE UTILMD Anfrage (PIDs 55001, 55002, 55016) ──────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSupplierChangeWorkflow`].
///
/// Registers one adapter covering all current BDEW format versions.
/// Extracts UTILMD S2.x fields to construct a
/// [`SupplierChangeCommand::ReceiveUtilmd`] for the 3 inbound ANFRAGE PIDs:
/// 55001–55002 (Lieferbeginn/Lieferende) and 55016 (Kündigung).
/// Outbound ANTWORT PIDs (55003–55006, 55017, 55018) are handled separately.
/// ORDERS Sperrung (PIDs 17115/17116/17117) uses [`gpke_sperrung_registry`].
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
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

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

// ── GPKE ORDERS Sperrung (PIDs 17115, 17116, 17117) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungWorkflow`].
///
/// Extracts ORDERS 1.4b fields from an inbound Sperrauftrag / Entsperrauftrag
/// (PIDs 17115/17116/17117) to construct a [`SperrungCommand::ReceiveSperrung`].
///
/// **Message format**: ORDERS (AWH Sperrprozesse Strom, BK6-22-024).
/// The Marktlokation is carried in the LOC segment (element 1, component 0).
///
/// **PID 55555** ("Anfrage Daten der individuellen Bestellung", GPKE Teil 4)
/// is a completely separate UTILMD-based data-request process and must NOT
/// be routed to this adapter.
#[must_use]
pub fn gpke_sperrung_registry() -> AdapterRegistry<GpkeSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE Sperrung adapter".into())
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung adapter: expected ORDERS message (PIDs 17115/17116/17117)"
                        .into(),
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

            // Marktlokation from the LOC segment (element 1, component 0).
            // LOC+7+<MaLo>::Z13 — element 0 = qualifier, element 1 = location composite.
            let location_id = mako_engine::types::MaLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "LOC")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            Ok(SperrungCommand::ReceiveSperrung {
                pid,
                sender: mako_engine::types::MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: mako_engine::types::MarktpartnerCode::new(
                    o.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                location_id,
                document_date: o
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

// ── GeLi Gas ORDERS Sperrung NB-side (PIDs 17115, 17116, 17117) ──────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrungNbWorkflow`].
///
/// Extracts ORDERS fields from an inbound Gas-Sperrauftrag / Gas-Entsperrauftrag
/// (PIDs 17115/17116/17117) to construct a [`GasSperrungNbCommand::ReceiveSperrung`].
///
/// **Message format**: ORDERS (AWH Sperrprozesse Gas, BK7-24-01-009).
/// **APERAK Frist:** 10 Werktage.
#[must_use]
pub fn geli_gas_sperrung_nb_registry() -> AdapterRegistry<GeliGasSperrungNbWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung NB adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung NB adapter: expected ORDERS message (PIDs 17115/17116/17117)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung NB adapter: PID detection failed: {e}"
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

            // Marktlokation from the LOC segment (element 1, component 0).
            let location_id = mako_engine::types::MaLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "LOC")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            Ok(GasSperrungNbCommand::ReceiveSperrung {
                pid,
                sender: mako_engine::types::MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: mako_engine::types::MarktpartnerCode::new(
                    o.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                location_id,
                document_date: o
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

// ── GPKE INVOIC billing (PIDs 31001, 31002, 31004–31008) ─────────────────────

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

// ── WiM Messstellenbetrieb (PIDs 55039, 55042, 55051, 55168) ──────────────────────────

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
/// - `17011` (Stornierung WiM Strom Teil 1) → [`GeraeteubernahmeCommand::ReceiveStornierung`]
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
                // Stornierung (17011 WiM Strom; 17009 routes to WiM Gas).
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

// ── WiM Stornierung (PID 39002) ──────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimStornierungWorkflow`].
///
/// Handles the single inbound ORDCHG PID 39002 (Stornierung der Bestellung) and produces a
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

// ── MaBiS Clearingliste (PIDs 55065, 55069, 55070) ────────────────────────────

/// Build an [`AdapterRegistry`] for [`MabisClearinglisteWorkflow`].
///
/// Handles inbound UTILMD Clearingliste messages in the MaBiS settlement cycle:
///
/// | PID   | Process name (AHB)              | Direction       |
/// |-------|---------------------------------|-----------------|
/// | 55065 | Lieferantenclearingliste        | NB → LF         |
/// | 55069 | Clearingliste DZR               | BIKO → NB / ÜNB |
/// | 55070 | Clearingliste BAS               | BIKO → BKV      |
///
/// Extracts UTILMD header fields (sender, receiver, document date, message ref)
/// and constructs a [`ClearinglisteCommand::ReceiveClearingliste`].
///
/// **Vacuous-validation guard**: AHB profiles for 55065/55069/55070 are not yet
/// imported into the `edi-energy` profile registry. Until `cargo xtask
/// import-xml-ahb` populates those rules, validation would return a vacuous
/// pass (zero rules → always valid → `validation_passed = true`). This adapter
/// detects the missing profile and forces `validation_passed = false` in that
/// case, preventing false-positive "valid" records from entering the stream.
///
/// The `billing_period` field is derived from the UTILMD document date
/// (DTM qualifier `"137"`) by truncating to `YYYYMM` format. If no document
/// date segment is present, the field is left empty.
///
/// **Regulatory basis**: BNetzA BK6-24-174 Anlage 3 MaBiS — Clearingverfahren.
/// No outbound APERAK deadline is associated with receiving these messages.
#[must_use]
pub fn mabis_clearingliste_registry() -> AdapterRegistry<MabisClearinglisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Clearingliste adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Clearingliste adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Clearingliste adapter: PID detection failed: {e}"
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

            // ── Vacuous-validation guard ──────────────────────────────────
            // PIDs 55065/55069/55070 have no AHB profiles yet. Guard against
            // false positives from empty rule sets.
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
                        message_ref = msg.message_ref(),
                        "MaBiS Clearingliste adapter: PID {} has no UTILMD AHB profile \
                         in the compiled profile set — import gap; \
                         vacuous validation forced to failed.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

            // ── Field extraction ──────────────────────────────────────────
            let document_date_str = u
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();

            // Derive billing period from document date: `YYYYMMDD` → `YYYYMM`.
            // If document date is absent or shorter than 6 chars, store empty.
            let billing_period = if document_date_str.len() >= 6 {
                BillingPeriod::new(&document_date_str[..6])
            } else {
                BillingPeriod::new("")
            };

            Ok(ClearinglisteCommand::ReceiveClearingliste {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                billing_period,
                document_date: document_date_str,
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GPKE UTILMD Antwort (PIDs 55003–55006, 55017, 55018) — LF role ──────────────

/// Build an [`AdapterRegistry`] for [`GpkeLfAnmeldungWorkflow`].
///
/// Handles inbound NB/LFA response PIDs (55003–55006, 55017, 55018) when `makod`
/// acts as the **Lieferant** — i.e. we previously sent the ANFRAGE outbound
/// and are now receiving the NB/LFA acknowledgement.
///
/// `accepted` is derived from the PID:
/// - 55003 (Bestätigung Lieferbeginn), 55005 (Bestätigung Lieferende),
///   55017 (Bestätigung Kündigung) → `accepted = true`
/// - 55004 (Ablehnung Lieferbeginn), 55006 (Ablehnung Lieferende),
///   55018 (Ablehnung Kündigung) → `accepted = false`
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
            let accepted = matches!(pid.as_u32(), 55003 | 55005 | 55017);

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
///
/// # AHB validation note
///
/// PIDs 55600/55601 are not yet present in the UTILMD Strom AHB profile JSON
/// files. Until they are imported via `cargo xtask import-xml-ahb`, `msg.validate()`
/// would return a vacuous pass (zero rules → always valid → `validation_passed = true`).
/// This guard forces `validation_passed = false` when no AHB rules are registered,
/// preventing structurally invalid or adversarially crafted messages from being
/// accepted with an empty MaLo ID.
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
            let mut validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

            // ── GPKE Neuanlage PIDs 55600/55601 — vacuous-validation guard ──
            // These PIDs are not yet in the UTILMD Strom AHB profile set.
            // Without a profile, validate() returns is_valid()=true (zero rules
            // checked). Guard against that false positive.
            if validation_passed {
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
                        "GPKE Neuanlage adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Forcing validation_passed = false to \
                         prevent accepting invalid messages with empty MaLo IDs. \
                         Import the profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

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
            let mut validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

            // ── PID 55007 — vacuous-validation guard ──────────────────────
            // PID 55007 (Ankündigung NB-seitiges Lieferende, NB → LFN) is
            // present in UTILMD AHB Strom 2.1 (FV2025-10-01) but has no
            // compiled AHB profile in the edi-energy profile set (import gap).
            // Without AHB rules, validate() returns is_valid()=true (zero
            // rules checked). Guard against that false positive by forcing
            // validation_passed=false until a proper profile is imported.
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
                        message_ref = msg.message_ref(),
                        "GPKE LF-Abmeldung adapter: PID {} (NB-seitiges Lieferende) \
                         has no UTILMD AHB profile in the compiled profile set — \
                         import gap; vacuous validation forced to failed.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

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

/// Build an [`AdapterRegistry`] for [`GpkeAnkuendigungZuordnungLfWorkflow`].
///
/// Adapts inbound UTILMD PID 55607 (Ankündigung Zuordnung LF, NB → LFN) to an
/// [`AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung`].
///
/// AHB validation is performed inline; `validation_passed` is set accordingly.
/// The LFN ERP responds with a subsequent `SendAntwort` command (within 24h,
/// BK6-22-024 §4).
#[must_use]
pub fn gpke_ankuendigung_zuordnung_lf_registry()
-> AdapterRegistry<GpkeAnkuendigungZuordnungLfWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Ankündigung Zuordnung LF adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Ankündigung Zuordnung LF adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Ankündigung Zuordnung LF adapter: PID detection failed: {e}"
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

            // ── PID 55607 — vacuous-validation guard ──────────────────────
            // PID 55607 (Ankündigung Zuordnung LF, NB → LFN) requires AHB
            // profile import before full validation is possible.
            // Guard against false positives from empty rule sets.
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
                        message_ref = msg.message_ref(),
                        "GPKE Ankündigung Zuordnung LF adapter: PID {} has no UTILMD AHB \
                         profile in the compiled profile set — import gap; \
                         vacuous validation forced to failed.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

            Ok(AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
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

// ── WiM Gas Anmeldung / Ende / Vorläufige Abmeldung (PIDs 44042–44053) ───────

/// Build an [`AdapterRegistry`] for [`WimGasAnmeldungWorkflow`].
///
/// Extracts UTILMD G fields to construct a [`WimGasAnmeldungCommand::ReceiveUtilmd`]
/// for inbound PIDs 44042–44053 (Anmeldung neuer MSB Gas / Ende MSB Gas).
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
/// Saturday counts as a Werktag; Sunday and public holidays do not.
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

// ── GPKE Stornierung (PIDs 55022–55024) ───────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeStornierungWorkflow`].
///
/// Routes UTILMD Strom messages with PIDs 55022–55024 (GPKE Stornierung):
/// - 55022 — Anfrage nach Stornierung (LFN → NB)
/// - 55023 — Bestätigung Stornierung  (NB response — accepted)
/// - 55024 — Ablehnung Stornierung    (NB response — rejected)
///
/// **APERAK Frist:** 24 Stunden wall-clock (BK6-22-024 §5).
#[must_use]
pub fn gpke_stornierung_registry() -> AdapterRegistry<GpkeStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Stornierung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Stornierung adapter: PID detection failed: {e}"
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
            // Vacuous-validation guard: warn if AHB profile not yet imported.
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
                        "GPKE Stornierung adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }
            Ok(GpkeStornierungCommand::ReceiveUtilmd {
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

// ── GPKE Anfrage Daten der individuellen Bestellung (PID 55555) ───────────────

/// Build an [`AdapterRegistry`] for [`GpkeAnfrageBestellungWorkflow`].
///
/// Routes UTILMD Strom messages with PID 55555 (GPKE Teil 4, BK6-24-174):
///
/// **Message format**: UTILMD Strom S2.x (`AnyMessage::Utilmd`).
/// **APERAK Frist:** 24 Stunden wall-clock (BK6-22-024 §5).
///
/// The key fields extracted from the UTILMD message are:
/// - `pid` — must be 55555
/// - `sender` / `receiver` — from NAD+MS / NAD+MR party identifiers
/// - `vorgang_id` — from `IDE+Z19` object ID (identifies the queried order)
/// - `bearbeitungsstatus` — from `STS` DE 9015 qualifier (`"E07"` or `"E08"`)
/// - `document_date` — from `DTM+137`
#[must_use]
pub fn gpke_anfrage_bestellung_registry() -> AdapterRegistry<GpkeAnfrageBestellungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE AnfrageBestellung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE AnfrageBestellung adapter: expected UTILMD message (PID 55555)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE AnfrageBestellung adapter: PID detection failed: {e}"
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

            // Vorgangsnummer from IDE+Z19 (element 1, component 0 = object ID).
            let vorgang_id = MaLo::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.ide.object_id.as_deref())
                    .unwrap_or(""),
            );

            // Bearbeitungsstatus from the first STS segment, element 0 (DE 9015).
            // Expected values: "E07" (known/confirmed Vorgang) or "E08" (unconfirmed).
            let bearbeitungsstatus = u
                .transactions()
                .first()
                .and_then(|t| t.sts.first())
                .and_then(|s| s.category.as_deref())
                .unwrap_or("")
                .to_owned();

            Ok(AnfrageBestellungCommand::ReceiveAnfrage {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                vorgang_id,
                bearbeitungsstatus,
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

// ── GeLi Gas Stornierung (PIDs 44022–44024) ────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasStornierungWorkflow`].
///
/// Routes UTILMD G messages with PIDs 44022–44024 (GeLi Gas Stornierung):
/// - 44022 — Anfrage nach Stornierung (LFN/LFA → GNB)
/// - 44023 — Bestätigung Stornierung  (GNB response — accepted)
/// - 44024 — Ablehnung Stornierung    (GNB response — rejected)
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
#[must_use]
pub fn geli_gas_stornierung_registry() -> AdapterRegistry<GeliGasStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Stornierung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Stornierung adapter: PID detection failed: {e}"
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
            // Vacuous-validation guard: warn if AHB profile not yet imported.
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
                        "GeLi Gas Stornierung adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }
            Ok(GeliGasStornierungCommand::ReceiveUtilmd {
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

// ── GaBi Gas INVOIC billing (PIDs 31010, 31007, 31008) ──────────────────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GaBiGasInvoicWorkflow`].
///
/// Extracts INVOIC fields to construct a [`GaBiGasInvoicCommand::ReceiveInvoic`]
/// for GaBi Gas billing PIDs:
/// - **31010** (Kapazitätsrechnung, FNB/VNB → BKV)
/// - **31007** (Aggreg. MMM-Rechnung Gas, NB → MGV)
/// - **31008** (Aggreg. MMM-Rechnung Gas selbst ausgestellt, NB → MGV)
///
/// Note: PID 31011 (Rechnung sonstige Leistung / AWH Sperrprozesse Gas, NB → LF)
/// belongs to GeLi Gas (BK7-24-01-009) and is handled by
/// `geli_gas_sperrprozesse_invoic_registry()` in `mako-geli-gas`.
///
/// Regulatory basis: BK7-14-020 (GaBi Gas 2.0).
#[must_use]
pub fn gabi_gas_invoic_registry() -> AdapterRegistry<GaBiGasInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GaBi Gas INVOIC adapter".into(),
                )
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "GaBi Gas INVOIC adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GaBi Gas INVOIC adapter: PID not found in INVOIC BGM".into(),
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

            Ok(GaBiGasInvoicCommand::ReceiveInvoic {
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

// ── GeLi Gas INVOIC billing (PID 31011 Rechnung sonstige Leistung) ───────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrprozesseInvoicWorkflow`].
///
/// Extracts INVOIC fields to construct a
/// [`GeliGasSperrprozesseInvoicCommand::ReceiveInvoic`] for GeLi Gas AWH
/// billing PID 31011 (Rechnung sonstige Leistung, VNB → LFN/LFA).
///
/// Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0).
#[must_use]
pub fn geli_gas_sperrprozesse_invoic_registry()
-> AdapterRegistry<GeliGasSperrprozesseInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrprozesse INVOIC adapter".into(),
                )
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrprozesse INVOIC adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GeLi Gas Sperrprozesse INVOIC adapter: PID not found in BGM".into(),
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

            Ok(GeliGasSperrprozesseInvoicCommand::ReceiveInvoic {
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

// ── WiM Strom INSRPT (PIDs 23001/23003/23004/23008/23011/23012) ──────────────

/// Build an [`AdapterRegistry`] for [`WimInsrptWorkflow`] (WiM Strom, 5 WT).
///
/// Handles inbound INSRPT messages for fault/inspection reporting between LF
/// and MSB in the WiM Strom Teil 2 process.  Covers both the outbound
/// Störungsmeldung (23001) and all inbound MSB responses
/// (23003/23004/23008/23011/23012).
///
/// In combined Strom+Gas deployments the ingest layer must supply
/// `Sparte::Strom` when calling [`PidRouter::route_with_sparte`] to reach this
/// workflow instead of [`wim_gas_insrpt_registry`].
///
/// [`PidRouter::route_with_sparte`]: mako_engine::pid_router::PidRouter::route_with_sparte
#[must_use]
pub fn wim_insrpt_registry() -> AdapterRegistry<WimInsrptWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Strom INSRPT adapter".into(),
                )
            })?;
            let AnyMessage::Insrpt(insrpt) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Strom INSRPT adapter: expected INSRPT message".into(),
                ));
            };
            let pid = insrpt
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Strom INSRPT adapter: PID not found in INSRPT BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            let sender = MarktpartnerCode::new(
                insrpt
                    .sender()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );
            let message_ref = MessageRef::new(
                insrpt
                    .bgm()
                    .and_then(|b| b.document_id.as_deref())
                    .unwrap_or(msg.message_ref()),
            );
            match pid.as_u32() {
                23011 | 23012 => Ok(StorungsmeldungCommand::ReceiveInformationsmeldung {
                    pid,
                    sender,
                    message_ref,
                }),
                _ => Ok(StorungsmeldungCommand::ReceiveAntwort {
                    pid,
                    sender,
                    message_ref,
                }),
            }
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

// ── GPKE Sperrung NB — MSB response (ORDRSP 19118/19119) ─────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungWorkflow`] (MSB → NB direction).
///
/// Routes ORDRSP 19118 (Bestätigung Anfrage Sperrung) and 19119 (Ablehnung
/// Anfrage Sperrung) from the MSB to the NB-side `gpke-sperrung` workflow via
/// [`SperrungCommand::ReceiveMsbAntwort`].
///
/// This is a **response adapter** — it is only used by the ingest dispatcher
/// to continue an existing NB-side process once the MSB answers the Anfrage
/// Sperrung (PID 17116).  It is distinct from [`gpke_sperrung_registry`] which
/// handles the inbound Sperrauftrag (PIDs 17115/17117).
///
/// **Loopback use**: in an integrated NB+MSB deployment (same GLN), the
/// outbox ORDRSP 19118/19119 emitted by the MSB side loops back via the
/// [`crate::ingest_dispatcher`] to complete the NB process.
#[must_use]
pub fn gpke_sperrung_msb_response_registry() -> AdapterRegistry<GpkeSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Sperrung MSB-response adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung MSB-response adapter: expected ORDRSP message \
                     (PIDs 19118/19119)"
                        .into(),
                ));
            };
            let _ = o; // sender extracted below

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung MSB-response adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // 19118 = Bestätigung (MSB confirms meter access).
            // 19119 = Ablehnung  (MSB cannot confirm meter access).
            let is_confirmed = pid.as_u32() == 19118;

            Ok(SperrungCommand::ReceiveMsbAntwort {
                pid,
                is_confirmed,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Sperrung LF side (ORDRSP 19116/19117 — NB → LF) ────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungLfWorkflow`].
///
/// Routes ORDRSP 19116 (Bestätigung Sperr-/Entsperrauftrag, NB → LF) and
/// 19117 (Ablehnung) to [`SperrungLfCommand::ReceiveOrdrsp`].
///
/// This is a **response adapter** used by the ingest dispatcher to continue
/// the LF-side process once the NB responds to the Sperrauftrag.
///
/// **Loopback use**: in an integrated NB+LF deployment (same GLN), the
/// outbox ORDRSP 19116/19117 emitted by the NB side loops back via the
/// [`crate::ingest_dispatcher`] to complete the LF process.
#[must_use]
pub fn gpke_sperrung_lf_registry() -> AdapterRegistry<GpkeSperrungLfWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Sperrung LF adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung LF adapter: expected ORDRSP message (PIDs 19116/19117)".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // 19116 = Bestätigung (NB will execute the Sperrung).
            // 19117 = Ablehnung  (NB rejects the Sperrauftrag).
            let is_confirmed = pid.as_u32() == 19116;

            Ok(SperrungLfCommand::ReceiveOrdrsp {
                pid,
                is_confirmed,
                message_ref: MessageRef::new(msg.message_ref()),
                sender: MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                reason: None,
            })
        },
    ));
    registry
}

// ── GeLi Gas Sperrung LF side (ORDRSP 19116/19117 — GNB → LFG) ──────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrungLfWorkflow`].
///
/// Routes ORDRSP 19116 (Bestätigung Gas-Sperr-/Entsperrauftrag, GNB → LFG)
/// and 19117 (Ablehnung) to [`GasSperrungLfCommand::ReceiveOrdrsp`].
///
/// This is a **response adapter** used by the ingest dispatcher to continue
/// the LFG-side process once the GNB responds to the Gas-Sperrauftrag.
///
/// **Loopback use**: in an integrated GNB+LFG deployment (same GLN), the
/// outbox ORDRSP 19116/19117 emitted by the GNB side loops back via the
/// [`crate::ingest_dispatcher`] to complete the LFG process.
#[must_use]
pub fn geli_gas_sperrung_lf_registry() -> AdapterRegistry<GeliGasSperrungLfWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung LF adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung LF adapter: expected ORDRSP message (PIDs 19116/19117)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // 19116 = Bestätigung (GNB accepts and will execute the Gas-Sperrung).
            // 19117 = Ablehnung  (GNB rejects the Gas-Sperrauftrag).
            let is_confirmed = pid.as_u32() == 19116;

            Ok(GasSperrungLfCommand::ReceiveOrdrsp {
                pid,
                is_confirmed,
                message_ref: MessageRef::new(msg.message_ref()),
                sender: MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                reason: None,
            })
        },
    ));
    registry
}

// ── GPKE Allokationsliste — ORDRSP rejection (PIDs 19110/19115) ───────────────

/// Build an [`AdapterRegistry`] for [`GpkeAllokationslisteWorkflow`] (ORDRSP path).
///
/// Handles inbound ORDRSP 19110 (Ablehnung Allokationsliste) and 19115
/// (Ablehnung Anforderung bilanzierte Menge) from the NB. Both are negative
/// responses to an LF-initiated ORDERS 17110/17114 request.
///
/// **Regulatory basis**: GPKE / MMM Strom/Gas (BK6-22-024 §8).
#[must_use]
pub fn gpke_allokationsliste_ordrsp_registry() -> AdapterRegistry<GpkeAllokationslisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Allokationsliste ORDRSP adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Allokationsliste ORDRSP adapter: expected ORDRSP message (PIDs 19110/19115)".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Allokationsliste ORDRSP adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // Extract optional rejection reason from FTX free-text segment.
            let reason: Option<String> = o.ftx().first().and_then(|f| f.text.clone());

            Ok(AllokationslisteCommand::ReceiveAblehnung {
                ordrsp_pid: pid,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Allokationsliste — MSCONS data delivery (PIDs 13013/13014) ───────────

/// Build an [`AdapterRegistry`] for [`GpkeAllokationslisteWorkflow`] (MSCONS path).
///
/// Handles inbound MSCONS 13013 (Marktlokationsscharfe Allokationsliste Gas)
/// and 13014 (Marktlokationsscharfe bilanzierte Menge) — the positive response
/// to an LF-initiated ORDERS 17110/17114 request.
///
/// These are **MMM Strom/Gas** PIDs, NOT GeLi Gas. They arrive at the LF
/// after the NB fulfils the allocation-list request.
///
/// **Regulatory basis**: GPKE / MMM Strom/Gas (BK6-22-024 §8).
#[must_use]
pub fn gpke_allokationsliste_mscons_registry() -> AdapterRegistry<GpkeAllokationslisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Allokationsliste MSCONS adapter".into(),
                )
            })?;

            // Accept MSCONS 13013/13014 only.
            if !matches!(msg, AnyMessage::Mscons(_)) {
                return Err(EngineError::Deserialization(
                    "GPKE Allokationsliste MSCONS adapter: expected MSCONS message (PIDs 13013/13014)".into(),
                ));
            }

            Ok(AllokationslisteCommand::NotifyDatenGeliefert {
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GeLi Gas MSCONS data delivery (PIDs 13002, 13007–13009) ─────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasMsconsWorkflow`].
///
/// Handles inbound Gas MSCONS metered-data messages from NB/MSB to LFG.
/// PIDs 13002, 13007–13009 (GeLi Gas 2.0 + WiM Gas data delivery per GeLi Gas 3.0).
///
/// Note: PIDs 13013/13014 (MMM Strom/Gas Allokationsliste) are NOT handled here —
/// they belong to `gpke-allokationsliste` and have their own registry.
#[must_use]
pub fn geli_gas_mscons_registry() -> AdapterRegistry<GeliGasMsconsWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas MSCONS adapter".into(),
                )
            })?;

            let AnyMessage::Mscons(m) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas MSCONS adapter: expected MSCONS message (PIDs 13002, 13007–13009)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas MSCONS adapter: PID detection failed: {e}"
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

            let sender = mako_engine::types::MarktpartnerCode::new(
                m.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
            );
            let message_ref = mako_engine::types::MessageRef::new(msg.message_ref());

            Ok(GasMsconsDatenCommand::ReceiveMscons {
                pid,
                sender,
                message_ref,
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GaBi Gas INVOIC — REMADV payment confirmation (PID 33001) ────────────────

/// Build an [`AdapterRegistry`] for REMADV 33001 routed to [`GaBiGasInvoicWorkflow`].
///
/// After the GaBi Gas invoice is received and settled (payer side), the payer
/// sends REMADV 33001 (Zahlungsavis) to confirm payment.  `makod` receives this
/// as the **invoicer** (FNB/VNB for PID 31010, NB for PIDs 31007/31008) and
/// resumes the billing process with [`GaBiGasInvoicCommand::ReceiveRemadv`].
///
/// **Correlation**: the ingest dispatcher looks up the billing process by the
/// invoice message-reference key set at spawn time (`extract_malo_from_invoic`).
/// The REMADV is correlated via the `extract_invoice_ref_from_remadv` helper
/// in `ingest_dispatcher`, which reads the `RFF+Z13` back-reference to the original INVOIC.
///
/// Regulatory basis: REMADV AHB 1.0, GaBi Gas, BK7.
#[must_use]
pub fn gabi_gas_remadv_registry() -> AdapterRegistry<GaBiGasInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GaBi Gas REMADV adapter".into(),
                )
            })?;
            let AnyMessage::Remadv(r) = msg else {
                return Err(EngineError::Deserialization(
                    "GaBi Gas REMADV adapter: expected REMADV message (PID 33001)".into(),
                ));
            };
            Ok(GaBiGasInvoicCommand::ReceiveRemadv {
                remadv_ref: MessageRef::new(msg.message_ref()),
                sender: MarktpartnerCode::new(
                    r.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
            })
        },
    ));
    registry
}

// ── GaBi Gas INVOIC — COMDIS payment rejection (PID 29001) ───────────────────

/// Build an [`AdapterRegistry`] for COMDIS 29001 routed to [`GaBiGasInvoicWorkflow`].
///
/// The invoicer (FNB/VNB or NB) rejects the payer's REMADV via COMDIS 29001
/// (Ablehnung der Zahlung).  `makod` resumes the billing process with
/// [`GaBiGasInvoicCommand::ReceiveComdis`].
///
/// **Correlation**: same `RFF+Z13` back-reference scheme as the REMADV adapter.
///
/// Regulatory basis: COMDIS AHB 1.0, GaBi Gas, BK7.
#[must_use]
pub fn gabi_gas_comdis_registry() -> AdapterRegistry<GaBiGasInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GaBi Gas COMDIS adapter".into(),
                )
            })?;
            let AnyMessage::Comdis(_) = msg else {
                return Err(EngineError::Deserialization(
                    "GaBi Gas COMDIS adapter: expected COMDIS message (PID 29001)".into(),
                ));
            };
            Ok(GaBiGasInvoicCommand::ReceiveComdis {
                comdis_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GaBi Gas Nomination — NOMINT / NOMRES (DVGW synthetic PIDs 90011/90012/90021/90022) ──

/// Build an [`AdapterRegistry`] for [`GaBiGasNominationWorkflow`].
///
/// Handles both outbound NOMINT dispatch (synthetic PIDs 90011/90012,
/// BKV → FNB/MGV) and inbound NOMRES response (synthetic PIDs 90021/90022,
/// FNB/MGV → BKV).
///
/// DVGW messages carry no BGM Prüfidentifikator; the synthetic PID is derived
/// from the message type and role qualifier via `AnyDvgwMessage::detect_pid`.
///
/// Regulatory basis: KoV (Kooperationsvereinbarung Gas), BNetzA BK7-14-020.
#[must_use]
pub fn gabi_gas_nomination_registry() -> AdapterRegistry<GaBiGasNominationWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyDvgwMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyDvgwMessage for GaBi Gas Nomination adapter".into(),
                )
            })?;

            let synthetic_pid = msg.detect_pid(None).ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Nomination adapter: could not derive synthetic PID".into(),
                )
            })?;

            let pid = Pruefidentifikator::new(synthetic_pid).map_err(|e| {
                EngineError::Deserialization(format!(
                    "GaBi Gas Nomination adapter: synthetic PID out of range: {e}"
                ))
            })?;

            let trait_msg = msg.as_trait().ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Nomination adapter: message has no trait impl".into(),
                )
            })?;
            let sender_eic = trait_msg.sender_eic().unwrap_or("").to_owned();
            let receiver_eic = trait_msg.receiver_eic().unwrap_or("").to_owned();
            let message_ref = MessageRef::new(trait_msg.message_ref());

            match msg {
                AnyDvgwMessage::Nomint(nomint) => {
                    // Outbound NOMINT — BKV sends nomination to FNB/MGV.
                    let gas_day = nomint.reference_date.clone().unwrap_or_default();
                    let nomination_ref = nomint
                        .nomination_ref
                        .as_deref()
                        .map(MessageRef::new)
                        .unwrap_or_else(|| message_ref.clone());
                    Ok(NominationCommand::SendNomination {
                        synthetic_pid: pid.as_u32(),
                        sender_eic,
                        receiver_eic,
                        gas_day,
                        nomination_ref,
                    })
                }
                AnyDvgwMessage::Nomres(nomres) => {
                    // Inbound NOMRES — FNB/MGV responds to BKV.
                    let gas_day = nomres.reference_date.clone().unwrap_or_default();
                    let acceptance = match &nomres.overall_status {
                        Some(dvgw_edi::messages::nomres::NomresStatus::Accepted) => {
                            NomresAcceptance::Accepted
                        }
                        Some(dvgw_edi::messages::nomres::NomresStatus::PartiallyAccepted) => {
                            NomresAcceptance::PartiallyAccepted
                        }
                        Some(dvgw_edi::messages::nomres::NomresStatus::Rejected) => {
                            NomresAcceptance::Rejected
                        }
                        Some(dvgw_edi::messages::nomres::NomresStatus::Other(code)) => {
                            NomresAcceptance::Other(code.clone())
                        }
                        Some(_) => NomresAcceptance::Other("unknown-variant".to_owned()),
                        None => NomresAcceptance::Other("unknown".to_owned()),
                    };
                    Ok(NominationCommand::ReceiveNomres {
                        nomres_ref: message_ref,
                        acceptance,
                        gas_day,
                        rejection_reason: None,
                    })
                }
                _ => Err(EngineError::Deserialization(
                    "GaBi Gas Nomination adapter: expected NOMINT or NOMRES message".into(),
                )),
            }
        },
    ));
    registry
}

// ── GaBi Gas Allocation — ALOCAT (DVGW synthetic PIDs 90001/90002/90003) ─────

/// Build an [`AdapterRegistry`] for [`GaBiGasAllocationWorkflow`].
///
/// Handles inbound ALOCAT allocation messages (synthetic PIDs 90001/90002/90003,
/// FNB/MGV/VNB → BKV). No response is sent — this is a receive-and-record workflow.
///
/// DVGW messages carry no BGM Prüfidentifikator; the synthetic PID is derived
/// from the message type and role qualifier via `AnyDvgwMessage::detect_pid`.
///
/// Regulatory basis: KoV (Kooperationsvereinbarung Gas), BNetzA BK7-14-020.
#[must_use]
pub fn gabi_gas_allocation_registry() -> AdapterRegistry<GaBiGasAllocationWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyDvgwMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyDvgwMessage for GaBi Gas Allocation adapter".into(),
                )
            })?;

            let AnyDvgwMessage::Alocat(alocat) = msg else {
                return Err(EngineError::Deserialization(
                    "GaBi Gas Allocation adapter: expected ALOCAT message".into(),
                ));
            };

            let synthetic_pid = msg.detect_pid(None).ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Allocation adapter: could not derive synthetic PID".into(),
                )
            })?;

            let pid = Pruefidentifikator::new(synthetic_pid).map_err(|e| {
                EngineError::Deserialization(format!(
                    "GaBi Gas Allocation adapter: synthetic PID out of range: {e}"
                ))
            })?;

            let trait_msg = msg.as_trait().ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Allocation adapter: message has no trait impl".into(),
                )
            })?;

            Ok(AllocationCommand::ReceiveAlocat {
                synthetic_pid: pid.as_u32(),
                sender_eic: trait_msg.sender_eic().unwrap_or("").to_owned(),
                receiver_eic: trait_msg.receiver_eic().unwrap_or("").to_owned(),
                gas_day: alocat.reference_date.clone().unwrap_or_default(),
                clearing_number: alocat.clearing_number.clone(),
                message_ref: MessageRef::new(trait_msg.message_ref()),
            })
        },
    ));
    registry
}
