//! MaBiS adapter registries.
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
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
