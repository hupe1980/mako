//! GPKE Allokationsliste — LF requests allocation lists and billing basis data.
//!
//! This module handles LF-initiated requests for **Allokationslisten** and
//! **bilanzierte Menge** data as defined in the MMM (Mehr-/Mindermenge) process
//! family — the BDEW/VKU/bne/GEODE AWH „Prozesse Mehr-/Mindermengen Strom Gas"
//! V2.1, not a BNetzA Festlegung:
//!
//! | PID   | ⚡ | 🔥 | Direction | Description |
//! |-------|---|---|-----------|-------------|
//! | 17110 | — | ✅ | LF → NB   | Anforderung Allokationsliste (Gas-only; sent from here, received by `mako-gabi-gas` `gabi-gas-mmma`) |
//! | 17114 | ✅ | — | NB → ÜNB  | Anforderung der bilanzierten Menge (Strom-only) |
//! | 19110 | — | ✅ | NB → LF   | Ablehnung Allokationsliste (Gas-only; sent by `mako-gabi-gas` `gabi-gas-mmma`) |
//! | 19115 | ✅ | — | NB → LF   | Ablehnung Anforderung bilanzierte Menge (Strom-only) |
//!
//! > **Note on 17110 / 19110**: These PIDs are Gas-only, the Gas twins of
//! > 17114/19115. This workflow is the LF's side — it sends the 17110 and
//! > receives the 19110. The inbound 17110 reaches the GNB, whose receiver is
//! > `mako-gabi-gas` `gabi-gas-mmma`.
//!
//! Positive responses (actual data) arrive via **MSCONS** (13014 for Strom bilanzierte
//! Menge, further MSCONS PIDs for Strom). PID 13013 (Gas-only Allokationsliste) is
//! handled by `mako-gabi-gas` `gabi-gas-mmma`.
//!
//! # Regulatory basis
//!
//! - **BDEW/VKU/bne/GEODE AWH „Prozesse Mehr-/Mindermengen Strom Gas" V2.1**
//!   (18.03.2025) — the MMM process family. There is no BNetzA Festlegung for
//!   it — BK6-22-024 covers GPKE Teil 4 and BK6-24-174 the rest, and neither
//!   mentions Allokationslisten.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE Allokationsliste / bilanzierte Menge process.
pub const WORKFLOW_NAME: &str = "gpke-allokationsliste";

/// ORDERS PIDs sent by LF (or other party) to NB requesting allocation data.
///
/// | PID   | ⚡ | 🔥 | Description |
/// |-------|---|---|-------------|
/// | 17110 | — | ✅ | Anforderung Allokationsliste (Gas-only; formally `gabi-gas-mmma`) |
/// | 17114 | ✅ | — | Anforderung der bilanzierten Menge (Strom-only) |
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[ORDERS_ANFRAGE_GAS_PID, 17114];

/// ORDERS 17110 — sent from here, received by the GNB's `gabi-gas-mmma`, so
/// `mako-gpke` does not route it.
pub const ORDERS_ANFRAGE_GAS_PID: u32 = 17110;

/// ORDRSP rejection PIDs sent by NB back to LF.
///
/// | PID   | ⚡ | 🔥 | Description |
/// |-------|---|---|-------------|
/// | 19110 | — | ✅ | Ablehnung Allokationsliste (Gas-only; formally `gabi-gas-mmma`) |
/// | 19115 | ✅ | — | Ablehnung Anforderung bilanzierte Menge (Strom-only) |
pub const ORDRSP_ABLEHNUNG_PIDS: &[u32] = &[19110, 19115];

/// MSCONS positive-response PIDs (NB sends Strom allocation data to LF).
///
/// These arrive inbound at the LF as the positive answer to ORDERS 17110/17114.
/// They are **MMM Strom** PIDs — NOT GeLi Gas (BK7-24-01-009) PIDs.
///
/// PID 13013 (Marktlokationsscharfe Allokationsliste Gas, Gas-only) has moved to
/// `mako-gabi-gas` `gabi-gas-mmma`. Only the Strom side (13014) is registered here.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 13014 | Marktlokationsscharfe bilanzierte Menge Strom/Gas (MMMA), Strom side |
pub const MSCONS_RESPONSE_PIDS: &[u32] = &[13014];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Allokationsliste workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AllokationslisteEvent {
    /// LF sent an ORDERS allocation-list request.
    AnforderungGesendet {
        /// ORDERS Prüfidentifikator (17114 for Strom; 17110 for Gas/shared).
        orders_pid: Pruefidentifikator,
        /// NB GLN.
        nb_mp_id: MarktpartnerCode,
        /// Affected MaLo (optional).
        malo: Option<MaLo>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// NB rejected the allocation-list request.
    AnforderungAbgelehnt {
        /// ORDRSP Prüfidentifikator (19115 for Strom; 19110 for Gas/shared).
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Allocation list data delivered (via MSCONS).
    DatenGeliefert {
        /// Message reference of the MSCONS.
        message_ref: MessageRef,
    },
}

impl EventPayload for AllokationslisteEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnforderungGesendet { .. } => "AllokationslisteAnforderungGesendet",
            Self::AnforderungAbgelehnt { .. } => "AllokationslisteAnforderungAbgelehnt",
            Self::DatenGeliefert { .. } => "AllokationslisteDatenGeliefert",
        }
    }

    /// Every variant is a message: the outbound ORDERS, the inbound ORDRSP
    /// Ablehnung and the inbound MSCONS. `DatenGeliefert` carries no PID.
    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnforderungGesendet {
                orders_pid,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(orders_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AnforderungAbgelehnt {
                ordrsp_pid,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(ordrsp_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::DatenGeliefert { message_ref } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: None,
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data from the sent ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnforderungData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// NB GLN.
    pub nb_mp_id: MarktpartnerCode,
    /// Optional MaLo.
    pub malo: Option<MaLo>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Allokationsliste process.
///
/// ```text
/// New → AnforderungGesendet → DatenErhalten
///                          ↘ Abgelehnt
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum AllokationslisteState {
    /// No request sent yet.
    #[default]
    New,
    /// LF sent the request; awaiting MSCONS data or rejection.
    AnforderungGesendet(AnforderungData),
    /// NB rejected the request.
    Abgelehnt {
        /// Original request.
        anforderung: AnforderungData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// Data arrived via MSCONS.
    DatenErhalten {
        /// Original request.
        anforderung: AnforderungData,
    },
}

impl AllokationslisteState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnforderungGesendet(_) => "AnforderungGesendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::DatenErhalten { .. } => "DatenErhalten",
        }
    }

    /// Whether terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Abgelehnt { .. } | Self::DatenErhalten { .. })
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Allokationsliste workflow.
#[derive(Clone)]
pub enum AllokationslisteCommand {
    /// LF requests an allocation list or billing-basis data.
    SendAnforderung {
        /// ORDERS PID (17114 for Strom; 17110 for Gas/shared).
        orders_pid: Pruefidentifikator,
        /// NB GLN.
        nb_mp_id: MarktpartnerCode,
        /// Affected MaLo (optional).
        malo: Option<MaLo>,
        /// Message reference.
        message_ref: MessageRef,
        /// ORDERS body payload.
        payload: serde_json::Value,
    },
    /// NB rejected the request.
    ReceiveAblehnung {
        /// ORDRSP PID (19115 for Strom; 19110 for Gas/shared).
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSCONS data delivered (positive response).
    NotifyDatenGeliefert {
        /// Message reference.
        message_ref: MessageRef,
    },
}

impl CommandPayload for AllokationslisteCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Allokationsliste workflow — LF requests allocation and billing-basis data.
///
/// The workflow registers no Frist. The MMM AWH V2.1 answers the Gas
/// Abonnement (17110) „unverzüglich", which counts nothing, and its 1-WT
/// window on the Strom Bestellung der bilanzierten Menge (17114, Kap. 4.1.2.3
/// Nr. 2) has no application from 01.10.2026 (Kap. 4.1.2.2 „Weitere
/// Anforderungen").
pub struct GpkeAllokationslisteWorkflow;

impl Workflow for GpkeAllokationslisteWorkflow {
    type State = AllokationslisteState;
    type Event = AllokationslisteEvent;
    type Command = AllokationslisteCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AllokationslisteEvent::AnforderungGesendet {
                orders_pid,
                nb_mp_id,
                malo,
                message_ref,
            } => AllokationslisteState::AnforderungGesendet(AnforderungData {
                orders_pid: *orders_pid,
                nb_mp_id: nb_mp_id.clone(),
                malo: malo.clone(),
                message_ref: message_ref.clone(),
            }),
            AllokationslisteEvent::AnforderungAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                AllokationslisteState::AnforderungGesendet(anforderung) => {
                    AllokationslisteState::Abgelehnt {
                        anforderung,
                        ordrsp_pid: *ordrsp_pid,
                        reason: reason.clone(),
                    }
                }
                other => other,
            },
            AllokationslisteEvent::DatenGeliefert { .. } => match state {
                AllokationslisteState::AnforderungGesendet(anforderung) => {
                    AllokationslisteState::DatenErhalten { anforderung }
                }
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            AllokationslisteCommand::SendAnforderung {
                orders_pid,
                nb_mp_id,
                malo,
                message_ref,
                payload,
            } => {
                if !matches!(state, AllokationslisteState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Allokationsliste ORDERS PID: {orders_pid}",
                    )));
                }
                let event = AllokationslisteEvent::AnforderungGesendet {
                    orders_pid,
                    nb_mp_id: nb_mp_id.clone(),
                    malo: malo.clone(),
                    message_ref: message_ref.clone(),
                };
                // The column's own fields come in `payload`, laid over the
                // head at the top, where the renderer reads them.
                let body = crate::outbound::mit_spalte(
                    &mako_engine::payload::OrdersPayload {
                        pid: Some(orders_pid.as_u32()),
                        location: malo.as_ref().map(|m| m.as_str().to_owned()),
                        message_ref: Some(message_ref.as_str().to_owned()),
                        ..Default::default()
                    },
                    &payload,
                )?;
                let outbox = vec![PendingOutbox::typed(nb_mp_id.as_str(), &body)];
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            AllokationslisteCommand::ReceiveAblehnung {
                ordrsp_pid,
                reason,
                message_ref,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, AllokationslisteState::AnforderungGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnforderungGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_ABLEHNUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Allokationsliste rejection ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                Ok(vec![AllokationslisteEvent::AnforderungAbgelehnt {
                    ordrsp_pid,
                    reason,
                    message_ref,
                }]
                .into())
            }

            AllokationslisteCommand::NotifyDatenGeliefert { message_ref } => {
                if !matches!(state, AllokationslisteState::AnforderungGesendet(_)) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![AllokationslisteEvent::DatenGeliefert { message_ref }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).expect("valid PID")
    }

    #[test]
    fn verlauf_states_orders_out_and_ordrsp_and_mscons_in() {
        let gesendet = AllokationslisteEvent::AnforderungGesendet {
            orders_pid: pid(17_114),
            nb_mp_id: MarktpartnerCode::new("9900357000004"),
            malo: None,
            message_ref: MessageRef::new("ORD-1"),
        };
        assert_eq!(
            gesendet.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(17_114),
                message_ref: Some("ORD-1".to_owned()),
            })
        );
        let abgelehnt = AllokationslisteEvent::AnforderungAbgelehnt {
            ordrsp_pid: pid(19_115),
            reason: None,
            message_ref: MessageRef::new("RSP-1"),
        };
        assert_eq!(
            abgelehnt.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(19_115),
                message_ref: Some("RSP-1".to_owned()),
            })
        );
        let geliefert = AllokationslisteEvent::DatenGeliefert {
            message_ref: MessageRef::new("MSC-1"),
        };
        assert_eq!(
            geliefert.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: None,
                message_ref: Some("MSC-1".to_owned()),
            })
        );
    }
}
