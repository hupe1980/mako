//! GaBi Gas adapter registries (INVOIC, Nomination, Allocation).
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
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
/// Regulatory basis: BK7-24-01-008 (GaBi Gas 2.1).
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
/// Regulatory basis: KoV (Kooperationsvereinbarung Gas), BNetzA BK7-24-01-008.
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
/// Regulatory basis: KoV (Kooperationsvereinbarung Gas), BNetzA BK7-24-01-008.
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
