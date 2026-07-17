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
use edifact_rs::OwnedSegment;
use mako_engine::{
    error::EngineError,
    message_adapter::{AdapterRegistry, FnAdapter},
    types::{
        BillingPeriod, DeviceId, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator,
    },
    version::FormatVersion,
};
use rubo4e::current as bo4e;
use rust_decimal::Decimal;

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
    GasKommunikationsdatenCommand, GasMsconsDatenCommand, GasSperrungLfCommand,
    GasSperrungNbCommand, GasSupplierChangeCommand, GeliGasLfStornierungWorkflow,
    GeliGasMsconsWorkflow, GeliGasPartinWorkflow, GeliGasSperrprozesseInvoicCommand,
    GeliGasSperrprozesseInvoicWorkflow, GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow,
    GeliGasStornierungCommand, GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
    LfStornierungCommand,
};
use mako_gpke::{
    AbrechnungCommand, AllokationslisteCommand, AnfrageBestellungCommand,
    AnkuendigungZuordnungLfCommand, DatanabrufCommand, GpkeAbrechnungWorkflow,
    GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeDatanabrufWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeMesswerteLieferungWorkflow, GpkeNeuanlageWorkflow,
    GpkePartinWorkflow, GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, GpkeStornierungCommand,
    GpkeStornierungWorkflow, GpkeSupplierChangeWorkflow, GpkeUtiltsWorkflow,
    KommunikationsdatenCommand, KonfigurationAenderungCommand, KonfigurationCommand,
    LfAbmeldungCommand, LfAnmeldungCommand, MesswerteLieferungCommand, NeuanlageCommand,
    SperrungCommand, SperrungLfCommand, SupplierChangeCommand, UtiltsKonfigCommand,
};
use mako_mabis::{
    BillingCommand, ClearinglisteCommand, DataStatus, IFTSTA_DATENSTATUS_PID, MabisBillingWorkflow,
    MabisClearinglisteWorkflow,
};
use mako_wim::{
    DeviceChangeCommand, GeraeteubernahmeCommand, PreisanfrageCommand, PreislisteCommand,
    StammdatenCommand, StornierungCommand, WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow,
    WimInsrptWorkflow, WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimRechnungCommand,
    WimRechnungWorkflow, WimStammdatenWorkflow, WimStornierungWorkflow,
    insrpt::StorungsmeldungCommand,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungWorkflow, WimGasInsrptWorkflow, WimGasInvoicCommand,
    WimGasInvoicWorkflow, WimGasKuendigungCommand, WimGasKuendigungWorkflow,
    WimGasStornierungCommand, WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageWorkflow, insrpt::GasStorungsmeldungCommand,
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
                // Bilanzierungsgebiet EIC from UTILMD NAD+Z09 / LOC+237.
                // processd NB check 4 uses this field directly; when None,
                // it falls back to marktd malo.bilanzierungsgebiet instead.
                // TODO(L1/N2): call t.bilanzierungsgebiet_eic() once edi-energy
                // exposes the LOC+237 segment accessor on UtilmdTransaction.
                bilanzierungsgebiet: None,
                // Bilanzierungsmethode from UTILMD TM+EM segment (L1/N1).
                // TM qualifier Z01 = SLP, Z02 = RLM, Z04 = IMS.
                // Extracted from the message-level raw segments: TM segment
                // immediately after the first IDE in the SG4 transaction group.
                bilanzierungsmethode: extract_bilanzierungsmethode(u.segments()),
                // Gas GaBi RLM Fallgruppe from UTILMD TM+Z10 segment (L1/N1).
                // Only populated for Gas PIDs; Strom UTILMD has no TM+Z10.
                fallgruppe: extract_fallgruppe(u.segments()),
                message_ref: MessageRef::new(msg.message_ref()),
                received_at: time::OffsetDateTime::now_utc(),
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
                rechnung: Some(Box::new(build_rechnung(inv.segments()))),
            })
        },
    ));
    registry
}

// ── GPKE billing — REMADV payment advice (PIDs 33001–33004) ──────────────────

/// Build an [`AdapterRegistry`] for REMADV 33001–33004 routed to [`GpkeAbrechnungWorkflow`].
///
/// After the NB sends an INVOIC to the LF, the LF (payer) responds with a REMADV
/// confirming or partially disputing the payment.  `makod` resumes the billing
/// process with [`AbrechnungCommand::ReceiveRemadv`].
///
/// **Correlation**: `extract_invoice_ref_from_remadv` reads `RFF+Z13:<invoice_ref>` to
/// map back to the spawned billing process.
///
/// Source: REMADV AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
#[must_use]
pub fn gpke_abrechnung_remadv_registry() -> AdapterRegistry<GpkeAbrechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE REMADV adapter".into())
            })?;
            let AnyMessage::Remadv(r) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE REMADV adapter: expected REMADV message (PIDs 33001–33004)".into(),
                ));
            };
            let pid = r
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GPKE REMADV adapter: PID not found in REMADV BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            Ok(AbrechnungCommand::ReceiveRemadv {
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

// ── GPKE billing — COMDIS payment rejection (PID 29001) ──────────────────────

/// Build an [`AdapterRegistry`] for COMDIS 29001 routed to [`GpkeAbrechnungWorkflow`].
///
/// After the LF (payer) sends a REMADV, the NB (invoicer) may reject it via
/// COMDIS 29001 (Ablehnung der Zahlung).  `makod` resumes the billing process
/// with [`AbrechnungCommand::ReceiveComdis`].
///
/// **Correlation**: `extract_invoice_ref_from_comdis` reads `RFF+Z13:<invoice_ref>`.
///
/// Source: COMDIS AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
#[must_use]
pub fn gpke_abrechnung_comdis_registry() -> AdapterRegistry<GpkeAbrechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE COMDIS adapter".into())
            })?;
            let AnyMessage::Comdis(_) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE COMDIS adapter: expected COMDIS message (PID 29001)".into(),
                ));
            };
            Ok(AbrechnungCommand::ReceiveComdis {
                comdis_ref: MessageRef::new(msg.message_ref()),
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
                rechnung: serde_json::to_value(build_rechnung(inv.segments()))
                    .unwrap_or(serde_json::Value::Null),
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
                received_at: time::OffsetDateTime::now_utc(),
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
/// Extract ZAK+ZE+ZD register definitions from WiM ORDERS Stammdaten segments.
///
/// Parses the flat list of `OwnedSegment`s and groups them into per-register
/// JSON objects following the nesting: **ZAK → ZE → ZD**.
///
/// # Segment mapping (BDEW ORDERS AHB fv20251001 — WiM Stammdatenübermittlung)
///
/// | Segment | Field | Description |
/// |---|---|---|
/// | `ZAK` element 0 | `obis_kennzahl` | OBIS code (e.g. `"1-1:1.8.0"`) |
/// | `ZAK` element 1 | `zaehlerauspraegung` | `Z01`→`HT`, `Z02`→`NT`, `Z03`→`EINZEL` |
/// | `ZAK` element 2 | `bezeichnung` | Human-readable register label |
/// | `ZE` element 0 | `saison` | `Z01`→`SOMMER`, `Z02`→`WINTER`, `Z03`→`GESAMT` |
/// | `ZD` element 0 | `tagtyp` | `Z01`→`WERKTAG`, `Z02`→`SAMSTAG`, `Z03`→`SONNTAG_FEIERTAG` |
/// | `ZD` elements 1..N | `fenster` | `"HHMM:code"` switch-point pairs |
///
/// # Output shape per register
///
/// ```json
/// {
///   "obis_kennzahl": "1-1:1.8.0",
///   "zaehlerauspraegung": "HT",
///   "bezeichnung": "HT Tarif",
///   "saisons": [
///     {
///       "saison": "GESAMT",
///       "tagtypen": [
///         {
///           "tagtyp": "WERKTAG",
///           "wochentage": [1, 2, 3, 4, 5],
///           "fenster": [
///             {"von": "07:00", "bis": "22:00"},
///             {"von": "22:00", "bis": "07:00"}
///           ]
///         }
///       ]
///     }
///   ]
/// }
/// ```
///
/// The `fenster` windows are derived from consecutive switch points: the `von`
/// time of window `i` equals the switch time, and `bis` equals the next switch
/// time (wrapping to the first for the last entry).
pub fn extract_zak_ze_zaehlwerke(segs: &[OwnedSegment]) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = Vec::new();

    // --- mutable accumulator state ---
    let mut cur_zw: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut cur_saisons: Vec<serde_json::Value> = Vec::new();
    let mut cur_saison: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut cur_tagtypen: Vec<serde_json::Value> = Vec::new();

    /// Flush accumulated `tagtypen` into `cur_saison`, then push saison into `cur_saisons`.
    fn flush_saison(
        cur_saison: &mut Option<serde_json::Map<String, serde_json::Value>>,
        cur_tagtypen: &mut Vec<serde_json::Value>,
        cur_saisons: &mut Vec<serde_json::Value>,
    ) {
        if let Some(mut s) = cur_saison.take() {
            s.insert(
                "tagtypen".into(),
                serde_json::Value::Array(std::mem::take(cur_tagtypen)),
            );
            cur_saisons.push(serde_json::Value::Object(s));
        } else {
            cur_tagtypen.clear();
        }
    }

    /// Flush accumulated `saisons` into `cur_zw`, then push zaehlwerk into `result`.
    fn flush_zaehlwerk(
        cur_zw: &mut Option<serde_json::Map<String, serde_json::Value>>,
        cur_saisons: &mut Vec<serde_json::Value>,
        result: &mut Vec<serde_json::Value>,
    ) {
        if let Some(mut zw) = cur_zw.take() {
            zw.insert(
                "saisons".into(),
                serde_json::Value::Array(std::mem::take(cur_saisons)),
            );
            result.push(serde_json::Value::Object(zw));
        } else {
            cur_saisons.clear();
        }
    }

    for seg in segs {
        match seg.tag.as_str() {
            "ZAK" => {
                // Flush any in-progress register.
                flush_saison(&mut cur_saison, &mut cur_tagtypen, &mut cur_saisons);
                flush_zaehlwerk(&mut cur_zw, &mut cur_saisons, &mut result);

                let obis = seg.element_str(0).unwrap_or("").to_owned();
                let zaehlerauspraegung = match seg.element_str(1).unwrap_or("Z03") {
                    "Z01" => "HT",
                    "Z02" => "NT",
                    _ => "EINZEL",
                };
                let bezeichnung = seg.element_str(2).unwrap_or("").to_owned();

                let mut zw = serde_json::Map::new();
                zw.insert("obis_kennzahl".into(), serde_json::Value::String(obis));
                zw.insert(
                    "zaehlerauspraegung".into(),
                    serde_json::Value::String(zaehlerauspraegung.to_owned()),
                );
                zw.insert("bezeichnung".into(), serde_json::Value::String(bezeichnung));
                cur_zw = Some(zw);
            }
            "ZE" if cur_zw.is_some() => {
                // Flush any in-progress saison.
                flush_saison(&mut cur_saison, &mut cur_tagtypen, &mut cur_saisons);

                let saison = match seg.element_str(0).unwrap_or("Z03") {
                    "Z01" => "SOMMER",
                    "Z02" => "WINTER",
                    _ => "GESAMT",
                };
                let mut s = serde_json::Map::new();
                s.insert(
                    "saison".into(),
                    serde_json::Value::String(saison.to_owned()),
                );
                cur_saison = Some(s);
            }
            "ZD" if cur_saison.is_some() => {
                let (tagtyp, wochentage) = match seg.element_str(0).unwrap_or("Z01") {
                    "Z02" => ("SAMSTAG", serde_json::json!([6])),
                    "Z03" => ("SONNTAG_FEIERTAG", serde_json::json!([7])),
                    _ => ("WERKTAG", serde_json::json!([1, 2, 3, 4, 5])),
                };

                // Collect all "HHMM:code" switch-point pairs from elements 1..N.
                let mut switches: Vec<String> = Vec::new();
                let mut idx = 1usize;
                while let Some(pair) = seg.element_str(idx) {
                    if !pair.is_empty() {
                        switches.push(pair.to_owned());
                    }
                    idx += 1;
                }

                // Build time windows: window i = [switch[i].time, switch[i+1].time).
                // The last window wraps around to switch[0].time.
                let times: Vec<String> = switches
                    .iter()
                    .map(|p| {
                        // "HHMM:code" → "HH:MM"
                        let raw = p.split(':').next().unwrap_or(p);
                        if raw.len() == 4 {
                            format!("{}:{}", &raw[..2], &raw[2..])
                        } else {
                            raw.to_owned()
                        }
                    })
                    .collect();

                let mut fenster: Vec<serde_json::Value> = Vec::with_capacity(times.len());
                for i in 0..times.len() {
                    let von = &times[i];
                    let bis = if i + 1 < times.len() {
                        times[i + 1].clone()
                    } else if !times.is_empty() {
                        times[0].clone()
                    } else {
                        "00:00".to_owned()
                    };
                    fenster.push(serde_json::json!({ "von": von, "bis": bis }));
                }

                cur_tagtypen.push(serde_json::json!({
                    "tagtyp":     tagtyp,
                    "wochentage": wochentage,
                    "fenster":    fenster,
                }));
            }
            _ => {}
        }
    }

    // Flush any remaining accumulated state.
    flush_saison(&mut cur_saison, &mut cur_tagtypen, &mut cur_saisons);
    flush_zaehlwerk(&mut cur_zw, &mut cur_saisons, &mut result);

    result
}

/// Build the inbound ORDERS adapter for WiM Stammdaten **Übermittlung** (PIDs 17102–17133).
///
/// This is the **responding-party** adapter (NB receiving MSB's master-data
/// response). It extracts:
/// - ZAK+ZE+ZD register definitions → `zaehlwerke`  
/// - LOC/QTY/MEA Standorteigenschaften → `standorteigenschaften` (if present)
/// - MeLo ID from the IDE segment
///
/// The resulting [`StammdatenCommand::TransmitStammdaten`] resumes (or starts)
/// the existing `wim-stammdaten` workflow on the NB side.
#[must_use]
pub fn wim_stammdaten_uebermittlung_registry() -> AdapterRegistry<WimStammdatenWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Stammdaten Übermittlung adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Stammdaten Übermittlung adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Stammdaten Übermittlung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let message_ref = MessageRef::new(msg.message_ref());

            // MeLo from the IDE segment (element 1, component 0).
            let _melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            // ZAK+ZE+ZD → typed register definitions.
            let zaehlwerke = extract_zak_ze_zaehlwerke(o.segments());

            // Standorteigenschaften is carried by UTILMD, not ORDERS 17102–17133.
            // Future: extend if LOC/QTY segments appear in Stammdaten ORDERS.
            let standorteigenschaften: Option<serde_json::Value> = None;

            Ok(StammdatenCommand::TransmitStammdaten {
                response_pid: pid,
                response_ref: message_ref,
                standorteigenschaften,
                zaehlwerke,
            })
        },
    ));
    registry
}

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
                received_at: time::OffsetDateTime::now_utc(),
                // L1/N1: extract Bilanzierungsmethode (TM+EM) and Fallgruppe (TM+Z10)
                bilanzierungsmethode: extract_bilanzierungsmethode(u.segments()),
                fallgruppe: extract_fallgruppe(u.segments()),
                // H2-readiness: gas quality type (placeholder — maps AHB qualifier when published)
                gasqualitaet: extract_gasqualitaet(u.segments()),
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
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

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
                received_at: time::OffsetDateTime::now_utc(),
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

// ── GPKE PARTIN Kommunikationsdaten (PIDs 37000–37006) ───────────────────────

/// Build an [`AdapterRegistry`] for [`GpkePartinWorkflow`].
///
/// Handles all inbound PARTIN messages with PIDs 37000–37006 (Strom
/// Kommunikationsdaten). Produces [`KommunikationsdatenCommand::ReceivePartin`].
#[must_use]
pub fn gpke_partin_registry() -> AdapterRegistry<GpkePartinWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE PARTIN adapter".into())
            })?;
            let AnyMessage::Partin(p) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE PARTIN adapter: expected PARTIN message (PIDs 37000–37006)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE PARTIN adapter: PID detection failed: {e}"
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
            Ok(KommunikationsdatenCommand::ReceivePartin {
                pid,
                sender: MarktpartnerCode::new(
                    p.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                document_date: p
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

// ── GPKE MSCONS Messwerte (PIDs 13002, 13005–13006) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeMesswerteLieferungWorkflow`].
///
/// Handles inbound MSCONS metered-data messages from NB/MSB to LF. The
/// delivery location MaLo is extracted from the first SG5 NAD segment.
/// Produces [`MesswerteLieferungCommand::ReceiveMscons`].
#[must_use]
pub fn gpke_messwerte_registry() -> AdapterRegistry<GpkeMesswerteLieferungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Messwerte adapter".into(),
                )
            })?;
            let AnyMessage::Mscons(m) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Messwerte adapter: expected MSCONS message (PIDs 13002, 13005–13006)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Messwerte adapter: PID detection failed: {e}"
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
            // First SG5 NAD (qualifier 172 = metering location) carries the MaLo.
            let location_id = mako_engine::types::MaLo::new(
                m.delivery_points()
                    .first()
                    .map(|dp| dp.nad.party_id.as_deref().unwrap_or(""))
                    .unwrap_or(""),
            );
            Ok(MesswerteLieferungCommand::ReceiveMscons {
                pid,
                sender: MarktpartnerCode::new(
                    m.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                location_id,
                document_date: m
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

// ── GPKE UTILTS Konfigurationsdaten (PIDs 11002, 11003, …) ───────────────────

/// Build an [`AdapterRegistry`] for [`GpkeUtiltsWorkflow`].
///
/// Handles inbound UTILTS configuration-data messages for GPKE Teil 3.
/// Produces [`UtiltsKonfigCommand::ReceiveUtilts`].
#[must_use]
pub fn gpke_utilts_registry() -> AdapterRegistry<GpkeUtiltsWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE UTILTS adapter".into())
            })?;
            let AnyMessage::Utilts(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE UTILTS adapter: expected UTILTS message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE UTILTS adapter: PID detection failed: {e}"
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
            Ok(UtiltsKonfigCommand::ReceiveUtilts {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
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

// ── GPKE Konfigurationsänderung ORDRSP (PIDs 17102, 17113) ───────────────────

/// Build an [`AdapterRegistry`] for [`GpkeKonfigurationAenderungWorkflow`].
///
/// Handles inbound ORDRSP messages (NB/MSB response to LF config-change
/// request). Produces [`KonfigurationAenderungCommand::ReceiveOrdrsp`].
///
/// The `accepted` flag is set to `true` when the ORDRSP BGM response code
/// indicates acceptance (`27` = accepted without amendment). Any other
/// response is treated as a rejection and `accepted` is `false`.
#[must_use]
pub fn gpke_konfiguration_aenderung_registry() -> AdapterRegistry<GpkeKonfigurationAenderungWorkflow>
{
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Konfigurationsänderung adapter".into(),
                )
            })?;
            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Konfigurationsänderung adapter: expected ORDRSP message (PIDs 17102, 17113)".into(),
                ));
            };
            let ordrsp_pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Konfigurationsänderung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            // BGM response code 27 = accepted without amendment; anything else = rejection.
            let (accepted, reason) = {
                let code = o
                    .segments()
                    .iter()
                    .find(|s| s.tag == "BGM")
                    .and_then(|s| s.component_str(2, 0));
                let accepted = code == Some("27");
                let reason = if accepted {
                    None
                } else {
                    Some(format!(
                        "ORDRSP response code: {}",
                        code.unwrap_or("unknown")
                    ))
                };
                (accepted, reason)
            };
            Ok(KonfigurationAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                accepted,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Datenabruf ORDRSP / Ablehnung (PIDs 17102, 17113) ───────────────────

/// Build an [`AdapterRegistry`] for [`GpkeDatanabrufWorkflow`].
///
/// The Datenabruf process is LF-initiated (outbound ORDERS); the only inbound
/// message is a rejection ORDRSP from NB/MSB. Produces
/// [`DatanabrufCommand::ReceiveAblehnung`].
#[must_use]
pub fn gpke_datenabruf_registry() -> AdapterRegistry<GpkeDatanabrufWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Datenabruf adapter".into(),
                )
            })?;
            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Datenabruf adapter: expected ORDRSP message (PIDs 17102, 17113)".into(),
                ));
            };
            let ordrsp_pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Datenabruf adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let reason = o
                .segments()
                .iter()
                .find(|s| s.tag == "FTX")
                .and_then(|s| s.component_str(4, 0))
                .map(|s| s.to_owned());
            Ok(DatanabrufCommand::ReceiveAblehnung {
                ordrsp_pid,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GeLi Gas PARTIN Kommunikationsdaten (PIDs 37008–37014) ───────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasPartinWorkflow`].
///
/// Handles all inbound Gas PARTIN messages with PIDs 37008–37014
/// (Gas Kommunikationsdaten). Produces
/// [`GasKommunikationsdatenCommand::ReceivePartin`].
#[must_use]
pub fn geli_gas_partin_registry() -> AdapterRegistry<GeliGasPartinWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas PARTIN adapter".into(),
                )
            })?;
            let AnyMessage::Partin(p) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas PARTIN adapter: expected PARTIN message (PIDs 37008–37014)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas PARTIN adapter: PID detection failed: {e}"
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
            Ok(GasKommunikationsdatenCommand::ReceivePartin {
                pid,
                sender: MarktpartnerCode::new(
                    p.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                document_date: p
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

// ── WiM Preisanfrage REQOTE (PIDs 35001–35005) ────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimPreisanfrageWorkflow`].
///
/// Handles inbound REQOTE price-inquiry messages from nMSB to MSB.
/// Produces [`PreisanfrageCommand::ReceiveReqote`].
#[must_use]
pub fn wim_preisanfrage_registry() -> AdapterRegistry<WimPreisanfrageWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Preisanfrage adapter".into(),
                )
            })?;
            let AnyMessage::Reqote(r) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Preisanfrage adapter: expected REQOTE message (PIDs 35001–35005)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Preisanfrage adapter: PID detection failed: {e}"
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
            Ok(PreisanfrageCommand::ReceiveReqote {
                pid,
                sender: MarktpartnerCode::new(
                    r.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    r.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Preisliste PRICAT (PIDs 27001–27003) ──────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimPreislisteWorkflow`].
///
/// Handles inbound PRICAT price-list messages from MSB to nMSB.
/// Produces [`PreislisteCommand::ReceivePricat`].
#[must_use]
pub fn wim_preisliste_registry() -> AdapterRegistry<WimPreislisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Preisliste adapter".into(),
                )
            })?;
            let AnyMessage::Pricat(p) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Preisliste adapter: expected PRICAT message (PIDs 27001–27003)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Preisliste adapter: PID detection failed: {e}"
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
            Ok(PreislisteCommand::ReceivePricat {
                pid,
                sender: MarktpartnerCode::new(
                    p.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    p.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
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

// ── GeLi Gas Stornierung LF side (PIDs 44023–44024) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasLfStornierungWorkflow`].
///
/// Handles inbound PIDs 44023/44024 (Bestätigung / Ablehnung Stornierung)
/// from GNB → LF. Produces [`LfStornierungCommand::HandleAntwort`].
///
/// Build an [`AdapterRegistry`] for `GeliGasDatanabrufWorkflow` — ORDERS 17103/17104 receive.
///
/// NB-side: receives inbound ORDERS from LF requesting Brennwert/Zustandszahl.
#[must_use]
pub fn geli_gas_datenabruf_receive_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasDatanabrufWorkflow> {
    use mako_geli_gas::datenabruf::{GeliGasDatanabrufCommand, ORDERS_ANFRAGE_PIDS};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Datenabruf receive adapter".into(),
                )
            })?;
            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Datenabruf receive adapter: expected ORDERS message (PIDs 17103/17104)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Datenabruf receive adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            if !ORDERS_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas Datenabruf receive adapter: unexpected PID {pid}"
                )));
            }
            Ok(GeliGasDatanabrufCommand::ReceiveAnfrage {
                pid,
                sender: MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    o.receiver().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for `GeliGasDatanabrufWorkflow` — ORDRSP 19103/19104.
///
/// LF-side: receives ORDRSP rejection from NB after sending ORDERS 17103.
#[must_use]
pub fn geli_gas_datenabruf_ablehnung_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasDatanabrufWorkflow> {
    use mako_geli_gas::datenabruf::{GeliGasDatanabrufCommand, ORDRSP_ABLEHNUNG_PIDS};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Datenabruf Ablehnung adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Datenabruf Ablehnung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            if !ORDRSP_ABLEHNUNG_PIDS.contains(&pid.as_u32()) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas Datenabruf Ablehnung adapter: unexpected PID {pid}"
                )));
            }
            // ORDRSP sender is the NB/MSB rejecting our request.
            let sender_gln = match msg {
                AnyMessage::Orders(o) => o
                    .sender()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or("")
                    .to_owned(),
                _ => String::new(),
            };
            Ok(GeliGasDatanabrufCommand::ReceiveAblehnung {
                pid,
                sender: MarktpartnerCode::new(sender_gln),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// Acceptance is determined by PID alone: 44023 = accepted, 44024 = rejected.
/// The rejection reason is extracted from the first transaction's FTX segment.
/// Regulatory basis: BK7-24-01-009, GeLi Gas (Lf-only deployment role).
#[must_use]
pub fn geli_gas_lf_anmeldung_registry() -> AdapterRegistry<mako_geli_gas::GeliGasLfAnmeldungWorkflow>
{
    use mako_geli_gas::{GeliGasLfAnmeldungCommand, LF_ANMELDUNG_ANTWORT_PIDS};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas LF-Anmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas LF-Anmeldung adapter: expected UTILMD G message (PIDs 44003–44006)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas LF-Anmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            if !LF_ANMELDUNG_ANTWORT_PIDS.contains(&pid.as_u32()) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas LF-Anmeldung adapter: unexpected PID {pid} (expected 44003–44006)"
                )));
            }

            // PID 44003/44005 = Bestätigung (accepted), 44004/44006 = Ablehnung (rejected).
            let accepted = matches!(pid.as_u32(), 44003 | 44005);
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());

            Ok(GeliGasLfAnmeldungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// The rejection reason is extracted from the first transaction's FTX segment.
/// Regulatory basis: BK7-24-01-009, GeLi Gas (Lf-only deployment role).
#[must_use]
pub fn geli_gas_stornierung_lf_registry() -> AdapterRegistry<GeliGasLfStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Stornierung LF adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Stornierung LF adapter: expected UTILMD message (PIDs 44023–44024)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Stornierung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            // PID 44023 = Bestätigung (accepted), 44024 = Ablehnung (rejected).
            let accepted = pid.as_u32() == 44023;
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());
            Ok(LfStornierungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
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
                brennwert_kwh_per_m3: extract_qty_z08(m),
                zustandszahl: extract_qty_z10(m),
                // H2-readiness: gas quality type not yet in MSCONS AHB — None until standardized
                gasqualitaet: None,
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
                    let gas_day = parse_dvgw_gas_day(nomint.reference_date.as_deref());
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
                    let gas_day = parse_dvgw_gas_day(nomres.reference_date.as_deref());
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
                gas_day: parse_dvgw_gas_day(alocat.reference_date.as_deref()),
                version: mako_gabi_gas::allocation::AllocationVersion::Initial,
                allocated_quantity: None,
                clearing_number: alocat.clearing_number.clone(),
                message_ref: MessageRef::new(trait_msg.message_ref()),
            })
        },
    ));
    registry
}

// ── EDIFACT → BO4E anti-corruption layer ─────────────────────────────────────

/// Convert raw INVOIC EDIFACT segments into a [`bo4e::Rechnung`].
///
/// This is the **only** place in the codebase where EDIFACT segment parsing
/// knowledge about the INVOIC message structure is combined with BO4E object
/// construction.  All downstream domain logic and the `invoic-checker` engine
/// work exclusively with the resulting [`bo4e::Rechnung`].
///
/// # Date types
///
/// In rubo4e v0.5, `Zeitraum.startdatum / enddatum` are `time::Date` (date-only).
/// EDIFACT DTM `YYYYMMDD` values are parsed to `time::Date` for all period
/// fields. Delivery periods are wrapped in a `Zeitraum` and stored in
/// `Rechnungsposition.lieferungszeitraum` (v202607 schema).
#[must_use]
fn build_rechnung(segs: &[OwnedSegment]) -> bo4e::Rechnung {
    // Split at the first LIN segment: header vs. detail sections.
    let lin_start = segs
        .iter()
        .position(|s| s.tag == "LIN")
        .unwrap_or(segs.len());
    let header = &segs[..lin_start];

    // Zeitraum.startdatum/enddatum are time::Date in rubo4e v0.5.
    let period_start = dtm(header, "163").and_then(edifact_date_to_date);
    let period_end = dtm(header, "164").and_then(edifact_date_to_date);
    // Rechnung.rechnungsdatum is still OffsetDateTime.
    // rechnungsdatum is time::Date in rubo4e v0.5 (follows *datum convention).
    let invoice_date = dtm(header, "137").and_then(edifact_date_to_date);

    let gesamtnetto = moa_betrag(header, "79");
    let gesamtbrutto = moa_betrag(header, "9");

    let rechnungsnummer = segs
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(1, 0))
        .map(str::to_owned);

    let rechnungsperiode = match (period_start, period_end) {
        (Some(s), Some(e)) => Some(bo4e::Zeitraum {
            startdatum: Some(s),
            enddatum: Some(e),
            ..Default::default()
        }),
        _ => None,
    };
    let rechnungspositionen = {
        let p = build_positions(segs);
        if p.is_empty() { None } else { Some(p) }
    };
    bo4e::Rechnung {
        rechnungsnummer,
        rechnungsdatum: invoice_date,
        rechnungsperiode,
        gesamtnetto,
        gesamtbrutto,
        rechnungspositionen,
        ..Default::default()
    }
}

/// Build a `Vec<Rechnungsposition>` by splitting on `LIN` segment boundaries.
fn build_positions(segs: &[OwnedSegment]) -> Vec<bo4e::Rechnungsposition> {
    let mut result = Vec::new();
    let mut group: Vec<&OwnedSegment> = Vec::new();
    let mut in_detail = false;

    for seg in segs {
        if seg.tag == "LIN" {
            if in_detail && !group.is_empty() {
                result.push(build_position(&group));
            }
            group.clear();
            in_detail = true;
        }
        if in_detail {
            group.push(seg);
        }
    }
    if in_detail && !group.is_empty() {
        result.push(build_position(&group));
    }
    result
}

/// Build a single `Rechnungsposition` from one LIN group.
fn build_position(group: &[&OwnedSegment]) -> bo4e::Rechnungsposition {
    let positionsnummer = group
        .first()
        .and_then(|s| s.component_str(0, 0))
        .and_then(|s| s.parse::<i64>().ok());

    // `lokations_id` was removed in BO4E v202607; store it as positionstext.
    let positionstext = group
        .iter()
        .find(|s| s.tag == "LOC" && s.component_str(0, 0) == Some("172"))
        .and_then(|s| s.component_str(1, 0))
        .map(str::to_owned);

    // Delivery period now lives in lieferungszeitraum (v202607).
    let lieferung_von = dtm_in_group(group, "163").and_then(edifact_date_to_date);
    let lieferung_bis = dtm_in_group(group, "164").and_then(edifact_date_to_date);
    let lieferungszeitraum = if lieferung_von.is_some() || lieferung_bis.is_some() {
        Some(bo4e::Zeitraum {
            startdatum: lieferung_von,
            enddatum: lieferung_bis,
            ..Default::default()
        })
    } else {
        None
    };

    let positions_menge = group
        .iter()
        .find(|s| s.tag == "QTY" && s.component_str(0, 0) == Some("46"))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|wert| {
            let normalized = wert.replace(',', ".");
            normalized.parse::<Decimal>().ok().map(|d| bo4e::Menge {
                wert: Some(d),
                einheit: Some(bo4e::Mengeneinheit::Kwh),
                ..Default::default()
            })
        });

    let einzelpreis = group
        .iter()
        .find(|s| s.tag == "PRI" && s.component_str(0, 0) == Some("AAB"))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|p| {
            let normalized = p.replace(',', ".");
            normalized.parse::<Decimal>().ok().map(|d| bo4e::Preis {
                wert: Some(d),
                ..Default::default()
            })
        });

    let gesamtpreis = moa_betrag_in_group(group, "77");

    bo4e::Rechnungsposition {
        positionsnummer,
        positionstext,
        lieferungszeitraum,
        positions_menge,
        einzelpreis,
        gesamtpreis,
        ..Default::default()
    }
}

// ── EDIFACT segment accessor helpers ─────────────────────────────────────────

/// Find the value of a `DTM` segment with a given qualifier in a slice.
fn dtm<'a>(segs: &'a [OwnedSegment], qualifier: &str) -> Option<&'a str> {
    segs.iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
}

/// Find the value of a `DTM` segment within a LIN group (slices of references).
fn dtm_in_group<'a>(group: &[&'a OwnedSegment], qualifier: &str) -> Option<&'a str> {
    group
        .iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
}

/// Build a [`bo4e::Betrag`] from a `MOA` segment with a given qualifier.
fn moa_betrag(segs: &[OwnedSegment], qualifier: &str) -> Option<bo4e::Betrag> {
    segs.iter()
        .find(|s| s.tag == "MOA" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|wert| {
            wert.replace(',', ".")
                .parse::<Decimal>()
                .ok()
                .map(|d| bo4e::Betrag {
                    wert: Some(d),
                    ..Default::default()
                })
        })
}

/// Build a [`bo4e::Betrag`] from a `MOA` segment within a LIN group.
fn moa_betrag_in_group(group: &[&OwnedSegment], qualifier: &str) -> Option<bo4e::Betrag> {
    group
        .iter()
        .find(|s| s.tag == "MOA" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|wert| {
            wert.replace(',', ".")
                .parse::<Decimal>()
                .ok()
                .map(|d| bo4e::Betrag {
                    wert: Some(d),
                    ..Default::default()
                })
        })
}

/// Convert an EDIFACT date (`YYYYMMDD`) to ISO 8601 (`YYYY-MM-DD`).
///
/// Lexicographic comparison of ISO dates is correct — required by
/// `invoic_checker`'s period-validity check (string comparison on
/// `Zeitraum.startdatum` / `enddatum`).
/// Parse an EDIFACT date string (`YYYYMMDD`) to a `time::Date`.
///
/// Used for BO4E fields typed as `Option<time::Date>` in rubo4e v0.5
/// (e.g. `Zeitraum.startdatum`, `Zeitraum.enddatum`).
///
/// Returns `None` if the string is not exactly 8 digits or cannot be parsed as a
/// valid calendar date.
fn edifact_date_to_date(yyyymmdd: &str) -> Option<time::Date> {
    use time::{Date, Month};
    if yyyymmdd.len() != 8 {
        return None;
    }
    let year: i32 = yyyymmdd[..4].parse().ok()?;
    let month: u8 = yyyymmdd[4..6].parse().ok()?;
    let day: u8 = yyyymmdd[6..8].parse().ok()?;
    let month = Month::try_from(month).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

// ── Gas quality helpers (PID 13007 Gasbeschaffenheitsdaten) ──────────────────
//
// PID 13007 MSCONS carries Brennwert and Zustandszahl in QTY segments:
//   QTY+Z08:{value} — Abrechnungsbrennwert (kWh/m³)
//   QTY+Z10:{value} — Zustandszahl (dimensionless compressibility factor)
//
// Source: Allgemeine Festlegungen V6.1d §6 / MSCONS AHB Gas 1.x.

/// Extract Abrechnungsbrennwert from `QTY+Z08` in a Gas MSCONS.
///
/// Scans all delivery-point → time-series → line-item → quantity leaves
/// for the first quantity with qualifier `Z08` and returns its value.
fn extract_qty_z08(m: &edi_energy::messages::mscons::MsconsMessage) -> Option<String> {
    for dp in m.delivery_points() {
        for ts in &dp.time_series {
            for item in &ts.items {
                for qty in &item.quantities {
                    if qty.qty.qualifier == "Z08" {
                        let normalized = qty
                            .qty
                            .value
                            .as_deref()
                            .map(|v| v.replace(',', "."))
                            .unwrap_or_default();
                        if !normalized.is_empty() {
                            return Some(normalized);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extract Zustandszahl from `QTY+Z10` in a Gas MSCONS.
///
/// Scans all delivery-point → time-series → line-item → quantity leaves
/// for the first quantity with qualifier `Z10` and returns its value.
fn extract_qty_z10(m: &edi_energy::messages::mscons::MsconsMessage) -> Option<String> {
    for dp in m.delivery_points() {
        for ts in &dp.time_series {
            for item in &ts.items {
                for qty in &item.quantities {
                    if qty.qty.qualifier == "Z10" {
                        let normalized = qty
                            .qty
                            .value
                            .as_deref()
                            .map(|v| v.replace(',', "."))
                            .unwrap_or_default();
                        if !normalized.is_empty() {
                            return Some(normalized);
                        }
                    }
                }
            }
        }
    }
    None
}

// ── UTILMD Typenmerkmale (TM) segment extractors ─────────────────────────────
//
// The BDEW UTILMD S2.x TM segment encodes energy classification metadata:
//   TM+EM+<qualifier>  — Energiemenge / Bilanzierungsmethode
//   TM+Z10+<code>      — Gas GaBi Fallgruppe (RLM only)
//
// These extractors scan the raw UTILMD segment list. They are best-effort:
// if the segment is absent or malformed, they return `None`.

/// Extract `bilanzierungsmethode` from a UTILMD segment list.
///
/// Maps `TM+EM` qualifier to BO4E `Bilanzierungsmethode`:
/// - Z01 → `"SLP"` (Standardlastprofil)
/// - Z02 → `"RLM"` (Registrierende Leistungsmessung)
/// - Z04 → `"IMS"` (Intelligentes Messsystem / iMSys)
pub fn extract_bilanzierungsmethode(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter()
        .find(|s| s.tag == "TM" && s.element_str(0).is_some_and(|q| q == "EM"))
        .and_then(|s| s.element_str(1))
        .and_then(|qualifier| match qualifier {
            "Z01" => Some("SLP".to_owned()),
            "Z02" => Some("RLM".to_owned()),
            "Z04" => Some("IMS".to_owned()),
            _ => None,
        })
}

/// Extract Gas GaBi `Fallgruppe` from a UTILMD segment list.
///
/// The `TM+Z10` segment in Gas UTILMD encodes the GaBi RLM Fallgruppe,
/// which determines whether the Gas MMM uses the `differenzierter` or
/// `pauschalierter` Abwicklungsweg (§ 4 GaBi-Strom / §5 KoV IX).
///
/// Returns the raw DE 7065 value (e.g. `"Z01"`, `"Z02"`) when present.
pub fn extract_fallgruppe(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter()
        .find(|s| s.tag == "TM" && s.element_str(0).is_some_and(|q| q == "Z10"))
        .and_then(|s| s.element_str(1))
        .map(str::to_owned)
}

/// Extract gas quality type from a UTILMD G segment list.
///
/// ## Current status — placeholder for H2-blend AHBs (2026–2028)
///
/// The DVGW and BNetzA have not yet standardized an EDIFACT qualifier for gas
/// quality type (`H_GAS` | `L_GAS` | `H2_BLEND`) in UTILMD G messages.
///
/// When the 2026–2028 H2-blend AHB wave is published, add the UTILMD G segment
/// code here — e.g. `TM+Z20` or a `CAV`/`ALC` characteristic.  The
/// `mako_geli_gas::gas_quality::GasQualitaet::from_raw()` normalization function
/// will convert whatever raw qualifier is used to the canonical `H_GAS` / `L_GAS` /
/// `H2_BLEND` form before storage in `marktd.malo.gasqualitaet`.
///
/// ## DVGW G 260 background
///
/// German gas quality types are defined in DVGW G 260 §3.2:
/// - **H-Gas** (high calorific): Wobbe index 12.4–15.7 kWh/m³
/// - **L-Gas** (low calorific): Wobbe index 10.5–13.0 kWh/m³
///
/// ## H2-blend EDIFACT pilot observation
///
/// In GET H2 and GASCADE H2 pilot messages (2025), some implementations carry
/// gas quality information in the `MKT+Z10` or `CAV+Z20` characteristic segment.
/// These are NOT standardized in the BDEW AHB yet. A monitoring adapter
/// that logs unknown `TM`/`CAV` qualifiers would help detect new codes before
/// the formal AHB publication.
#[allow(unused_variables)]
pub fn extract_gasqualitaet(segs: &[OwnedSegment]) -> Option<String> {
    use mako_geli_gas::gas_quality::normalize_gasqualitaet;
    // Placeholder: scan for any TM segment with a gas-quality-like qualifier.
    // Currently returns None for all standard UTILMD G messages.
    // TODO: add the canonical BDEW AHB segment code when published (2026-2028 wave).
    // Example future implementation:
    //   segs.iter()
    //       .find(|s| s.tag == "TM" && s.element_str(0).is_some_and(|q| q == "Z20"))
    //       .and_then(|s| s.element_str(1))
    //       .map(|raw| normalize_gasqualitaet(raw).to_owned())
    let _ = normalize_gasqualitaet; // suppress unused warning until real mapping is added
    None
}

// ── DVGW gas-day conversion helper ───────────────────────────────────────────

/// Parse a DVGW reference-date string (`YYYY-MM-DD` or `YYYYMMDD`) into a
/// typed [`mako_gabi_gas::GasDay`].
///
/// DVGW messages encode the gas day in DTM qualifier 137.  The format is
/// `YYYYMMDD` in older versions and `YYYY-MM-DD` in current NOMINT/ALOCAT.
/// Both are accepted here; an invalid or absent date falls back to today.
fn parse_dvgw_gas_day(raw: Option<&str>) -> mako_gabi_gas::GasDay {
    let fallback = || mako_gabi_gas::GasDay::new(time::OffsetDateTime::now_utc().date());
    let Some(s) = raw else { return fallback() };
    // Try ISO 8601 first (`YYYY-MM-DD`), then compact form (`YYYYMMDD`).
    if let Ok(d) = mako_gabi_gas::GasDay::parse(s) {
        return d;
    }
    // Compact `YYYYMMDD` → insert dashes and retry.
    if s.len() == 8 {
        let iso = format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..8]);
        if let Ok(d) = mako_gabi_gas::GasDay::parse(&iso) {
            return d;
        }
    }
    tracing::warn!(
        raw = s,
        "adapters: could not parse DVGW gas day — using today as fallback"
    );
    fallback()
}
