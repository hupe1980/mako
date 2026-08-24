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
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
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

// ── GaBi Gas Nomination — NOMINT / NOMRES (DVGW PIDs 70030–70039) ────────────

/// Build an [`AdapterRegistry`] for [`GaBiGasNominationWorkflow`].
///
/// Handles outbound NOMINT (PIDs 70030–70034, Transportkunde → NB/MGV) and the
/// inbound NOMRES that answers it (PIDs 70035–70039).
///
/// The routing key is the Prüfidentifikator DVGW puts in `SG1 RFF+Z13`; it is
/// read off the wire, not synthesised from the message type and a role code.
///
/// Regulatory basis: `KoV` (Kooperationsvereinbarung Gas), BNetzA BK7-24-01-008.
#[must_use]
pub fn gabi_gas_nomination_registry() -> AdapterRegistry<GaBiGasNominationWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<DvgwMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected DvgwMessage for GaBi Gas Nomination adapter".into(),
                )
            })?;

            let pid = msg.pruefidentifikator.ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Nomination adapter: no Prüfidentifikator in SG1 RFF+Z13".into(),
                )
            })?;
            let gas_day = dvgw_gas_day(msg).ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Nomination adapter: no usable DTM+Z01 Gültigkeitszeitraum — \
                     the gas day cannot be determined"
                        .into(),
                )
            })?;

            let sender_eic = msg.sender().map(|p| p.id.clone()).unwrap_or_default();
            let receiver_eic = msg.receiver().map(|p| p.id.clone()).unwrap_or_default();
            let message_ref = MessageRef::new(msg.message_ref.as_str());

            match msg.message_type {
                DvgwMessageType::Nomint => {
                    // The Dokumentennummer is the nomination's own identity. The
                    // NOMRES answering it carries no reference back, so the pair
                    // is matched on the business key, not on this.
                    let nomination_ref = msg
                        .document_number
                        .as_deref()
                        .map_or_else(|| message_ref.clone(), MessageRef::new);
                    Ok(NominationCommand::SendNomination {
                        pruefidentifikator: pid.as_u32(),
                        sender_eic,
                        receiver_eic,
                        gas_day,
                        nomination_ref,
                        // Every position of a NOMINT is the nomination itself, so
                        // all of them count.
                        nominated_kwh: msg.single_energy_kwh(|_| true),
                    })
                }
                DvgwMessageType::Nomres => {
                    // NOMRES has no status segment, so only the document-name code
                    // says what this message decides — and only a *Bestätigung*
                    // decides anything. A Matching-Benachrichtigung (07G/19G)
                    // reports the state of the match and is filtered out upstream,
                    // in the ingest arm, so it can never reach here and be turned
                    // into a terminal outcome.
                    let acceptance = match msg.document {
                        DvgwDocument::Bestaetigung
                        | DvgwDocument::VhpBestaetigung
                        | DvgwDocument::BestaetigungFlexibilitaetsuebertragung => {
                            NomresAcceptance::Accepted
                        }
                        other => NomresAcceptance::Other(other.code().to_owned()),
                    };

                    // Only the recipient's own side counts, and only **one** label
                    // of it. A NOMRES states the nominated quantities under `IMD`
                    // `17G`, the counterparty's mirror under `18G`, and the matched
                    // result under `16G`; a message may carry `17G` *and* `16G` for
                    // the same position. Selecting both sums two figures for one
                    // quantity, which is the same double-count as including `18G`
                    // — a curtailment then reads as an over-confirmation.
                    //
                    // `16G` wins when present: the matched quantity is what will
                    // actually flow.
                    let label = |code: &'static str| {
                        move |item: &dvgw_edi::LineItem| item.description_code() == Some(code)
                    };
                    let has = |code: &'static str| msg.items.iter().any(label(code));
                    let confirmed_kwh = if has(dvgw_edi::model::imd::GEMATCHT) {
                        msg.single_energy_kwh(label(dvgw_edi::model::imd::GEMATCHT))
                    } else if has(dvgw_edi::model::imd::NOMINIERT) {
                        msg.single_energy_kwh(label(dvgw_edi::model::imd::NOMINIERT))
                    } else {
                        // Unlabelled: the message states one side only, so every
                        // position is that side.
                        msg.single_energy_kwh(|item: &dvgw_edi::LineItem| {
                            item.description_code().is_none()
                        })
                    };

                    Ok(NominationCommand::ReceiveNomres {
                        nomres_ref: message_ref,
                        acceptance,
                        gas_day,
                        confirmed_kwh,
                        rejection_reason: None,
                    })
                }
                DvgwMessageType::Alocat => Err(EngineError::Deserialization(
                    "GaBi Gas Nomination adapter: expected NOMINT or NOMRES, got ALOCAT".into(),
                )),
            }
        },
    ));
    registry
}

// ── GaBi Gas Allocation — ALOCAT (DVGW PIDs 70001–70023) ─────────────────────

/// Build an [`AdapterRegistry`] for [`GaBiGasAllocationWorkflow`].
///
/// Handles inbound ALOCAT allocation messages. No response is sent — this is a
/// receive-and-record workflow.
///
/// Regulatory basis: `KoV` (Kooperationsvereinbarung Gas), BNetzA BK7-24-01-008.
#[must_use]
pub fn gabi_gas_allocation_registry() -> AdapterRegistry<GaBiGasAllocationWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<DvgwMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected DvgwMessage for GaBi Gas Allocation adapter".into(),
                )
            })?;
            if msg.message_type != DvgwMessageType::Alocat {
                return Err(EngineError::Deserialization(format!(
                    "GaBi Gas Allocation adapter: expected ALOCAT, got {}",
                    msg.message_type
                )));
            }

            let pid = msg.pruefidentifikator.ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Allocation adapter: no Prüfidentifikator in SG1 RFF+Z13".into(),
                )
            })?;
            let gas_day = dvgw_gas_day(msg).ok_or_else(|| {
                EngineError::Deserialization(
                    "GaBi Gas Allocation adapter: no usable DTM+Z01 Gültigkeitszeitraum — \
                     the gas day cannot be determined"
                        .into(),
                )
            })?;

            // The version is stated by the document-name code, not assumed:
            // reporting every message as `Initial` hides whether the binding
            // final allocation ever arrived, which is what the KoV §6.4 deadline
            // watches for.
            //
            // `X5G` (Endgültige Allokation) is deliberately **not** mapped to
            // `Final` here. DVGW publishes `X6G`/`X7G` Korrigierte Allokation as
            // messages that follow the endgültige one, while the workflow treats
            // `Final` as settled and refuses any later correction — so mapping it
            // would drop every correction that arrives afterwards. Closing that
            // needs the KoV §6.4 question of what "binding" means once a
            // correction follows, which is a process decision, not a parse one.
            let version = match msg.document {
                DvgwDocument::KorrigierteAllokationBilanzierungsbrennwert
                | DvgwDocument::KorrigierteAllokationAbrechnungsbrennwert
                | DvgwDocument::KorrigierteMengenmeldungNkp => AllocationVersion::Correction(1),
                _ => AllocationVersion::Initial,
            };

            Ok(AllocationCommand::ReceiveAlocat {
                pruefidentifikator: pid.as_u32(),
                sender_eic: msg.sender().map(|p| p.id.clone()).unwrap_or_default(),
                receiver_eic: msg.receiver().map(|p| p.id.clone()).unwrap_or_default(),
                gas_day,
                version,
                // A DVGW `QTY` is a **rate** in kWh/h over the period its own
                // `DTM+2` names, so the allocated energy is Σ(rate × duration) —
                // never the sum of the values, which adds rates together.
                // `single_energy_kwh` refuses when the message mixes Einspeisung
                // (`Z02`) and Ausspeisung (`Z03`), because one scalar across them
                // is a net position, and when any quantity could not be
                // integrated, because a partial total understates the gas day.
                allocated_quantity: msg
                    .single_energy_kwh(|_| true)
                    .map(mako_gabi_gas::GasQuantity::from_kwh),
                clearing_number: msg.clearingnummer().map(str::to_owned),
                message_ref: MessageRef::new(msg.message_ref.as_str()),
            })
        },
    ));
    registry
}
