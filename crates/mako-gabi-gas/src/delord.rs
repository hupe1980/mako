//! GaBi Gas DELORD/DELRES workflow — delivery order and response (PIDs 90061/90062).
//!
//! # Process overview
//!
//! The **delivery order** cycle allows a market participant (BKV / GH) to request
//! a specific gas delivery at a defined delivery point. The FNB or MGV confirms
//! or rejects the delivery via a **delivery response** (DELRES).
//!
//! ```text
//! BKV / GH ──(DELORD 90061)──→  FNB / MGV
//! FNB / MGV ──(DELRES 90062)──→  BKV / GH
//! ```
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ OrderSent (DELORD dispatched outbound)
//!       ├─ Confirmed     (DELRES status = Accepted)         [terminal]
//!       ├─ Modified      (DELRES with quantity adjustment)   [terminal]
//!       ├─ Rejected      (DELRES status = Rejected)         [terminal]
//!       └─ DeadlineExpired (no DELRES within response window) [terminal]
//! ```
//!
//! # Synthetic Prüfidentifikatoren
//!
//! | PID   | Message | Direction         | Content                     |
//! |-------|---------|-------------------|-----------------------------|
//! | 90061 | DELORD  | BKV/GH → FNB/MGV  | Delivery order              |
//! | 90062 | DELRES  | FNB/MGV → BKV/GH  | Delivery response           |
//!
//! # Deadline
//!
//! Per the Kooperationsvereinbarung Gas, the FNB/MGV must respond to a delivery
//! order before the gas day nomination deadline (D-1 15:00 CET). Register a
//! [`mako_engine::deadline::Deadline`] with label [`DELRES_DEADLINE_LABEL`]
//! immediately after the `DeliveryOrderSent` event is persisted.
//!
//! # Regulatory basis
//!
//! - **DVGW G 685 / G 2000** — delivery order protocol
//! - **Kooperationsvereinbarung Gas (KoV)** — delivery obligations and response deadlines
//! - **BNetzA GaBi Gas 2.0 (BK7-14-020)** — regulatory framework

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Synthetic PID set ─────────────────────────────────────────────────────────

/// All PIDs for the DELORD/DELRES delivery order cycle.
///
/// | PID   | Message | Direction         |
/// |-------|---------|-------------------|
/// | 90061 | DELORD  | BKV/GH → FNB/MGV  |
/// | 90062 | DELRES  | FNB/MGV → BKV/GH  |
pub const DELIVERY_ORDER_PIDS: &[u32] = &[90061, 90062];

/// Synthetic PID for the outbound DELORD delivery order.
pub const DELORD_PID: u32 = 90061;

/// Synthetic PIDs for the inbound DELRES delivery response.
pub const DELRES_PID: u32 = 90062;

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-delivery-order";

/// Deadline label for the DELRES response window.
///
/// Per the Kooperationsvereinbarung Gas, the FNB/MGV must respond to a
/// delivery order before the gas day nomination deadline (D-1 15:00 CET).
/// Register a [`mako_engine::deadline::Deadline`] with this label immediately
/// after the `DeliveryOrderSent` event is persisted.
pub const DELRES_DEADLINE_LABEL: &str = "gabi-gas-delres-response-deadline";

// ── Acceptance status ─────────────────────────────────────────────────────────

/// Response status from the FNB/MGV in the DELRES message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelresStatus {
    /// Delivery accepted as requested.
    Accepted,
    /// Delivery accepted with modified quantity or terms.
    Modified,
    /// Delivery rejected.
    Rejected,
    /// Status not mapped to a known variant (raw code preserved).
    Other(String),
}

impl DelresStatus {
    /// Human-readable display string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Accepted => "Accepted",
            Self::Modified => "Modified",
            Self::Rejected => "Rejected",
            Self::Other(code) => code.as_str(),
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a DELORD delivery order is dispatched.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryOrderData {
    /// Synthetic PID (90061).
    pub synthetic_pid: u32,
    /// EIC code of the ordering party (BKV / GH).
    pub sender_eic: String,
    /// EIC code of the receiving FNB / MGV.
    pub receiver_eic: String,
    /// Gas day / delivery period (from DTM qualifier 137).
    pub gas_day: String,
    /// Requested delivery quantity in kWh (from QTY segment).
    pub quantity_kwh: i64,
    /// DELORD document reference (from BGM element 1 — used for DELRES correlation).
    pub order_ref: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas DELORD/DELRES workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DeliveryOrderEvent {
    /// BKV/GH dispatched a DELORD delivery order to FNB or MGV.
    DeliveryOrderSent {
        /// Synthetic PID (90061).
        synthetic_pid: u32,
        /// EIC code of the sender (BKV / GH).
        sender_eic: String,
        /// EIC code of the receiver (FNB / MGV).
        receiver_eic: String,
        /// Gas day / delivery period.
        gas_day: String,
        /// Requested quantity in kWh.
        quantity_kwh: i64,
        /// DELORD document reference.
        order_ref: MessageRef,
    },
    /// FNB/MGV confirmed the delivery as requested.
    Confirmed {
        /// DELRES message reference.
        delres_ref: MessageRef,
        /// Gas day confirmed by FNB/MGV.
        gas_day: String,
    },
    /// FNB/MGV confirmed the delivery with modified quantity or terms.
    Modified {
        /// DELRES message reference.
        delres_ref: MessageRef,
        /// Gas day from DELRES.
        gas_day: String,
        /// Adjusted quantity in kWh (may differ from ordered quantity).
        adjusted_quantity_kwh: Option<i64>,
    },
    /// FNB/MGV rejected the delivery order.
    Rejected {
        /// DELRES message reference.
        delres_ref: MessageRef,
        /// Human-readable rejection reason.
        reason: String,
    },
    /// DELRES deadline expired — FNB/MGV did not respond in time.
    DeadlineExpired {
        /// Deadline identifier for audit.
        deadline_id: DeadlineId,
        /// Deadline label (always [`DELRES_DEADLINE_LABEL`]).
        label: String,
    },
}

impl EventPayload for DeliveryOrderEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::DeliveryOrderSent { .. } => "DeliveryOrderSent",
            Self::Confirmed { .. } => "DeliveryOrderConfirmed",
            Self::Modified { .. } => "DeliveryOrderModified",
            Self::Rejected { .. } => "DeliveryOrderRejected",
            Self::DeadlineExpired { .. } => "DeliveryOrderDeadlineExpired",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Process state for the GaBi Gas DELORD/DELRES workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub enum DeliveryOrderState {
    /// Initial state — no DELORD sent yet.
    #[default]
    New,
    /// DELORD dispatched; awaiting DELRES.
    OrderSent(DeliveryOrderData),
    /// FNB/MGV confirmed the delivery as requested (terminal).
    Confirmed(DeliveryOrderData),
    /// FNB/MGV confirmed the delivery with modifications (terminal).
    Modified(DeliveryOrderData),
    /// FNB/MGV rejected the delivery order (terminal).
    Rejected {
        /// Original order data.
        data: DeliveryOrderData,
        /// Rejection reason from DELRES.
        reason: String,
    },
    /// DELRES deadline expired — no response from FNB/MGV (terminal).
    DeadlineExpired(DeliveryOrderData),
}

impl DeliveryOrderState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::OrderSent(_) => "OrderSent",
            Self::Confirmed(_) => "Confirmed",
            Self::Modified(_) => "Modified",
            Self::Rejected { .. } => "Rejected",
            Self::DeadlineExpired(_) => "DeadlineExpired",
        }
    }

    /// Returns `true` if no further commands can be applied.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Confirmed(_)
                | Self::Modified(_)
                | Self::Rejected { .. }
                | Self::DeadlineExpired(_)
        )
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the GaBi Gas DELORD/DELRES workflow.
///
/// [`Workflow::handle`] is pure — no I/O.
#[derive(Clone)]
pub enum DeliveryOrderCommand {
    /// BKV/GH is dispatching a DELORD delivery order (PID 90061).
    ///
    /// Constructed by the outbound dispatch layer in `makod` after the BKV/GH
    /// submits a delivery order via the Commands API.
    SendDeliveryOrder {
        /// Must be 90061.
        synthetic_pid: u32,
        /// EIC code of the sending BKV / GH.
        sender_eic: String,
        /// EIC code of the receiving FNB / MGV.
        receiver_eic: String,
        /// Gas day / delivery period.
        gas_day: String,
        /// Requested delivery quantity in kWh.
        quantity_kwh: i64,
        /// DELORD document reference.
        order_ref: MessageRef,
    },

    /// Inbound DELRES received from FNB/MGV (PID 90062).
    ///
    /// Constructed by the DVGW adapter in `makod` when a DELRES arrives on the
    /// inbound channel. The `order_ref` must match the one in the outbound
    /// DELORD to correlate correctly.
    ReceiveDelres {
        /// DELRES message reference.
        delres_ref: MessageRef,
        /// Response status from FNB/MGV.
        status: DelresStatus,
        /// Gas day confirmed in DELRES.
        gas_day: String,
        /// Adjusted quantity (for `Modified` responses).
        adjusted_quantity_kwh: Option<i64>,
        /// Human-readable rejection reason (for `Rejected` responses).
        rejection_reason: Option<String>,
    },

    /// DELRES deadline expired — FNB/MGV did not respond in time.
    DelresDeadlineExpired {
        /// Deadline identifier for audit.
        deadline_id: DeadlineId,
        /// Deadline label (always [`DELRES_DEADLINE_LABEL`]).
        label: String,
    },
}

impl CommandPayload for DeliveryOrderCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas DELORD/DELRES delivery order workflow (PIDs 90061/90062).
///
/// Tracks the lifecycle of a single DELORD submission and its corresponding
/// DELRES reply for the BKV/GH → FNB/MGV delivery order cycle.
pub struct GaBiGasDeliveryOrderWorkflow;

impl Workflow for GaBiGasDeliveryOrderWorkflow {
    type State = DeliveryOrderState;
    type Event = DeliveryOrderEvent;
    type Command = DeliveryOrderCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            DeliveryOrderEvent::DeliveryOrderSent {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                gas_day,
                quantity_kwh,
                order_ref,
            } => DeliveryOrderState::OrderSent(DeliveryOrderData {
                synthetic_pid: *synthetic_pid,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: gas_day.clone(),
                quantity_kwh: *quantity_kwh,
                order_ref: order_ref.clone(),
            }),

            DeliveryOrderEvent::Confirmed { .. } => match state {
                DeliveryOrderState::OrderSent(data) => DeliveryOrderState::Confirmed(data),
                other => other,
            },

            DeliveryOrderEvent::Modified { .. } => match state {
                DeliveryOrderState::OrderSent(data) => DeliveryOrderState::Modified(data),
                other => other,
            },

            DeliveryOrderEvent::Rejected { reason, .. } => match state {
                DeliveryOrderState::OrderSent(data) => DeliveryOrderState::Rejected {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            DeliveryOrderEvent::DeadlineExpired { .. } => match state {
                DeliveryOrderState::OrderSent(data) => DeliveryOrderState::DeadlineExpired(data),
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            DeliveryOrderCommand::SendDeliveryOrder {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                gas_day,
                quantity_kwh,
                order_ref,
            } => {
                if !matches!(state, DeliveryOrderState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if synthetic_pid != DELORD_PID {
                    return Err(WorkflowError::rejected(format!(
                        "PID {synthetic_pid} is not a valid DELORD PID (expected {DELORD_PID})"
                    )));
                }
                Ok(vec![DeliveryOrderEvent::DeliveryOrderSent {
                    synthetic_pid,
                    sender_eic,
                    receiver_eic,
                    gas_day,
                    quantity_kwh,
                    order_ref,
                }]
                .into())
            }

            DeliveryOrderCommand::ReceiveDelres {
                delres_ref,
                status,
                gas_day,
                adjusted_quantity_kwh,
                rejection_reason,
            } => {
                if !matches!(state, DeliveryOrderState::OrderSent(_)) {
                    return Err(WorkflowError::invalid_state("OrderSent", state.label()));
                }
                let event = match &status {
                    DelresStatus::Accepted => DeliveryOrderEvent::Confirmed {
                        delres_ref,
                        gas_day,
                    },
                    DelresStatus::Modified => DeliveryOrderEvent::Modified {
                        delres_ref,
                        gas_day,
                        adjusted_quantity_kwh,
                    },
                    DelresStatus::Rejected | DelresStatus::Other(_) => {
                        DeliveryOrderEvent::Rejected {
                            delres_ref,
                            reason: rejection_reason.unwrap_or_else(|| status.as_str().to_owned()),
                        }
                    }
                };
                Ok(vec![event].into())
            }

            DeliveryOrderCommand::DelresDeadlineExpired { deadline_id, label } => {
                if state.is_terminal() {
                    // Deadline fired after DELRES already received — absorb silently.
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![DeliveryOrderEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{ids::DeadlineId, types::MessageRef, workflow::Workflow};

    use super::*;

    fn make_order_ref() -> MessageRef {
        MessageRef::new("DELORD-0001")
    }

    fn make_res_ref() -> MessageRef {
        MessageRef::new("DELRES-0001")
    }

    fn sent_state() -> DeliveryOrderState {
        DeliveryOrderState::OrderSent(DeliveryOrderData {
            synthetic_pid: 90061,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001370C".to_owned(),
            gas_day: "2026-01-15".to_owned(),
            quantity_kwh: 1_000_000,
            order_ref: make_order_ref(),
        })
    }

    #[test]
    fn workflow_name_is_stable() {
        assert_eq!(WORKFLOW_NAME, "gabi-gas-delivery-order");
    }

    #[test]
    fn delord_pid_is_90061() {
        assert_eq!(DELORD_PID, 90061);
        assert_eq!(DELRES_PID, 90062);
        assert!(DELIVERY_ORDER_PIDS.contains(&90061));
        assert!(DELIVERY_ORDER_PIDS.contains(&90062));
    }

    #[test]
    fn deadline_label_is_stable() {
        assert_eq!(DELRES_DEADLINE_LABEL, "gabi-gas-delres-response-deadline");
    }

    #[test]
    fn send_delivery_order_transitions_new_to_order_sent() {
        let state = DeliveryOrderState::New;
        let cmd = DeliveryOrderCommand::SendDeliveryOrder {
            synthetic_pid: 90061,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001370C".to_owned(),
            gas_day: "2026-01-15".to_owned(),
            quantity_kwh: 1_000_000,
            order_ref: make_order_ref(),
        };
        let output = GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(output.events.len(), 1);
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasDeliveryOrderWorkflow::apply);
        assert!(matches!(next, DeliveryOrderState::OrderSent(_)));
    }

    #[test]
    fn receive_accepted_delres_transitions_to_confirmed() {
        let state = sent_state();
        let cmd = DeliveryOrderCommand::ReceiveDelres {
            delres_ref: make_res_ref(),
            status: DelresStatus::Accepted,
            gas_day: "2026-01-15".to_owned(),
            adjusted_quantity_kwh: None,
            rejection_reason: None,
        };
        let output = GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(output.events.len(), 1);
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasDeliveryOrderWorkflow::apply);
        assert!(matches!(next, DeliveryOrderState::Confirmed(_)));
    }

    #[test]
    fn receive_modified_delres_transitions_to_modified() {
        let state = sent_state();
        let cmd = DeliveryOrderCommand::ReceiveDelres {
            delres_ref: make_res_ref(),
            status: DelresStatus::Modified,
            gas_day: "2026-01-15".to_owned(),
            adjusted_quantity_kwh: Some(900_000),
            rejection_reason: None,
        };
        let output = GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).unwrap();
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasDeliveryOrderWorkflow::apply);
        assert!(matches!(next, DeliveryOrderState::Modified(_)));
    }

    #[test]
    fn receive_rejected_delres_transitions_to_rejected() {
        let state = sent_state();
        let cmd = DeliveryOrderCommand::ReceiveDelres {
            delres_ref: make_res_ref(),
            status: DelresStatus::Rejected,
            gas_day: "2026-01-15".to_owned(),
            adjusted_quantity_kwh: None,
            rejection_reason: Some("Capacity unavailable".to_owned()),
        };
        let output = GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).unwrap();
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasDeliveryOrderWorkflow::apply);
        assert!(matches!(next, DeliveryOrderState::Rejected { .. }));
    }

    #[test]
    fn deadline_expired_on_sent_transitions_to_deadline_expired() {
        let state = sent_state();
        let cmd = DeliveryOrderCommand::DelresDeadlineExpired {
            deadline_id: DeadlineId::new(),
            label: DELRES_DEADLINE_LABEL.to_owned(),
        };
        let output = GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).unwrap();
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasDeliveryOrderWorkflow::apply);
        assert!(matches!(next, DeliveryOrderState::DeadlineExpired(_)));
    }

    #[test]
    fn deadline_absorbed_when_already_terminal() {
        let state = DeliveryOrderState::Confirmed(DeliveryOrderData {
            synthetic_pid: 90061,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001370C".to_owned(),
            gas_day: "2026-01-15".to_owned(),
            quantity_kwh: 1_000_000,
            order_ref: make_order_ref(),
        });
        let cmd = DeliveryOrderCommand::DelresDeadlineExpired {
            deadline_id: DeadlineId::new(),
            label: DELRES_DEADLINE_LABEL.to_owned(),
        };
        let output = GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).unwrap();
        assert!(output.events.is_empty()); // silently absorbed
    }

    #[test]
    fn wrong_pid_on_send_returns_error() {
        let state = DeliveryOrderState::New;
        let cmd = DeliveryOrderCommand::SendDeliveryOrder {
            synthetic_pid: 90062, // wrong: DELRES PID used for send
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001370C".to_owned(),
            gas_day: "2026-01-15".to_owned(),
            quantity_kwh: 500_000,
            order_ref: make_order_ref(),
        };
        assert!(GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn receive_delres_in_wrong_state_returns_error() {
        let state = DeliveryOrderState::New;
        let cmd = DeliveryOrderCommand::ReceiveDelres {
            delres_ref: make_res_ref(),
            status: DelresStatus::Accepted,
            gas_day: "2026-01-15".to_owned(),
            adjusted_quantity_kwh: None,
            rejection_reason: None,
        };
        assert!(GaBiGasDeliveryOrderWorkflow::handle(&state, cmd).is_err());
    }
}
