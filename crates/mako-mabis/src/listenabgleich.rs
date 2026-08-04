//! MaBiS Listenabgleich — list distribution with a correction leg.
//!
//! # Process overview
//!
//! One party distributes a settlement list; the receiving party reconciles it
//! against its own records and returns a **Korrekturliste** or
//! **Prüfmitteilung**. The reply is not an acceptance — it is the corrections
//! themselves, and it is sent whether or not any were found.
//!
//! ```text
//! Liste ──→ receiver
//!   │           │
//!   └───────────┴──→ Korrekturliste / Prüfmitteilung ──→ sender
//! ```
//!
//! # Why this is not `mabis-clearingliste`
//!
//! The existing clearing lists (55065/55069/55070) are **record-only**: they are
//! distributed and nothing is expected back. These three carry a reply leg, so
//! modelling them the same way would drop the correction obligation entirely.
//!
//! # Why the reply is not a Bestätigung/Ablehnung
//!
//! The lifecycle families in [`crate::zp_lifecycle`] answer with a binary
//! confirm/reject pair. These do not: 55202 is a *Korrekturliste*, 55224 a
//! *Prüfmitteilung*. A reply carrying corrections is neither an acceptance nor a
//! rejection of the list — it is the reconciliation result. Forcing it into an
//! accept/reject shape would make "list received, three positions corrected"
//! unrepresentable.
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt*.
//!
//! | Liste | Von → An  | Antwort | Von → An  | EBD           | Inhalt                            |
//! |------:|-----------|--------:|-----------|---------------|-----------------------------------|
//! | 55195 | ÜNB → NB  | 55196   | NB → ÜNB  | E_0017/E_0052 | Bilanzierungsgebietsclearingliste |
//! | 55201 | NB → LF   | 55202   | LF → NB   | E_0097/E_0096 | LF-AACL                           |
//! | 55223 | ÜNB → NB  | 55224   | NB → ÜNB  | E_0070        | DZÜ-Liste                         |
//!
//! The reply always travels back along the same axis the list came down, with
//! the roles swapped — which is why the receiving role is derived from the list
//! rather than passed in.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 (MaBiS)** — Clearingverfahren
//! - **UTILMD AHB Strom S2.1 / S2.2** — message format
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ ListeErhalten ─┬─ (validation failed) ─→ ValidationFailed  (terminal)
//!                    └─ KorrekturGesendet ───→ Abgeglichen       (terminal)
//! ```

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── List table ────────────────────────────────────────────────────────────────

/// Which MaBiS list is being reconciled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenTyp {
    /// 55195/55196 — Bilanzierungsgebietsclearingliste (ÜNB ↔ NB).
    Bilanzierungsgebietsclearingliste,
    /// 55201/55202 — Lieferantenausfallarbeitsclearingliste (NB ↔ LF).
    LfAacl,
    /// 55223/55224 — Deltazeitreihenübertrag-Liste (ÜNB ↔ NB).
    DzuListe,
}

/// One row of the Liste → Antwort table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenFamilie {
    /// Inbound list Prüfidentifikator.
    pub liste: u32,
    /// Outbound Korrekturliste / Prüfmitteilung Prüfidentifikator.
    pub antwort: u32,
    /// Which list this row describes.
    pub typ: ListenTyp,
    /// Market role that distributes the list.
    pub sender_rolle: &'static str,
    /// Market role that reconciles it and returns the corrections.
    pub empfaenger_rolle: &'static str,
}

/// Every list/correction pair this workflow handles.
pub const LISTEN_FAMILIEN: &[ListenFamilie] = &[
    ListenFamilie {
        liste: 55195,
        antwort: 55196,
        typ: ListenTyp::Bilanzierungsgebietsclearingliste,
        sender_rolle: "ÜNB",
        empfaenger_rolle: "NB",
    },
    ListenFamilie {
        liste: 55201,
        antwort: 55202,
        typ: ListenTyp::LfAacl,
        sender_rolle: "NB",
        empfaenger_rolle: "LF",
    },
    ListenFamilie {
        liste: 55223,
        antwort: 55224,
        typ: ListenTyp::DzuListe,
        sender_rolle: "ÜNB",
        empfaenger_rolle: "NB",
    },
];

/// Look up the family for an inbound list PID.
#[must_use]
pub fn familie_for(liste: u32) -> Option<&'static ListenFamilie> {
    LISTEN_FAMILIEN.iter().find(|f| f.liste == liste)
}

/// Every PID this workflow is registered for — lists and their replies.
#[must_use]
pub fn all_pids() -> Vec<u32> {
    let mut v: Vec<u32> = LISTEN_FAMILIEN
        .iter()
        .flat_map(|f| [f.liste, f.antwort])
        .collect();
    v.sort_unstable();
    v
}

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-listenabgleich";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a list is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenabgleichData {
    /// Prüfidentifikator of the inbound list.
    pub pruefidentifikator: Pruefidentifikator,
    /// Which list was received.
    pub typ: ListenTyp,
    /// GLN of the distributing party.
    pub sender: MarktpartnerCode,
    /// GLN of the reconciling party.
    pub receiver: MarktpartnerCode,
    /// Billing period the list covers.
    pub billing_period: BillingPeriod,
    /// EDIFACT message reference of the list.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MaBiS Listenabgleich workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ListenabgleichEvent {
    /// Inbound list received and recorded.
    ListeErhalten {
        /// Prüfidentifikator of the list.
        pruefidentifikator: Pruefidentifikator,
        /// Which list was received.
        typ: ListenTyp,
        /// GLN of the distributing party.
        sender: MarktpartnerCode,
        /// GLN of the reconciling party.
        receiver: MarktpartnerCode,
        /// Billing period the list covers.
        billing_period: BillingPeriod,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Korrekturliste / Prüfmitteilung dispatched back to the distributor.
    KorrekturGesendet {
        /// Prüfidentifikator of the reply actually sent.
        antwort_pid: Pruefidentifikator,
        /// Number of corrected positions — `0` means the list was confirmed
        /// unchanged, which is still an obligatory reply.
        korrekturen: u32,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Human-readable summary of validation errors.
        reason: String,
    },
}

impl EventPayload for ListenabgleichEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::ListeErhalten { .. } => "MabisListeErhalten",
            Self::KorrekturGesendet { .. } => "MabisKorrekturGesendet",
            Self::ValidationFailed { .. } => "MabisListenabgleichValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a MaBiS Listenabgleich process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum ListenabgleichState {
    /// No events yet.
    #[default]
    New,
    /// List received; a Korrekturliste is owed.
    ListeErhalten(Box<ListenabgleichData>),
    /// Korrekturliste dispatched (terminal).
    Abgeglichen {
        /// Which list was reconciled.
        typ: ListenTyp,
        /// Number of corrected positions reported.
        korrekturen: u32,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Validation error summary.
        reason: String,
    },
}

impl ListenabgleichState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::ListeErhalten(_) => "ListeErhalten",
            Self::Abgeglichen { .. } => "Abgeglichen",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MaBiS Listenabgleich workflow.
#[derive(Clone)]
pub enum ListenabgleichCommand {
    /// Inbound list received from the AS4 layer.
    ReceiveListe {
        /// Prüfidentifikator of the inbound UTILMD.
        pid: Pruefidentifikator,
        /// GLN of the distributing party.
        sender: MarktpartnerCode,
        /// GLN of the reconciling party.
        receiver: MarktpartnerCode,
        /// Billing period the list covers.
        billing_period: BillingPeriod,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
    /// Send the Korrekturliste / Prüfmitteilung back to the distributor.
    SendKorrektur {
        /// Number of corrected positions. `0` is valid and still sends a reply.
        korrekturen: u32,
    },
}

impl CommandPayload for ListenabgleichCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MaBiS Listenabgleich workflow — list distribution with a correction leg.
///
/// See the module documentation for the PID table and why the reply is modelled
/// as a correction count rather than an accept/reject flag.
pub struct MabisListenabgleichWorkflow;

impl Workflow for MabisListenabgleichWorkflow {
    type State = ListenabgleichState;
    type Event = ListenabgleichEvent;
    type Command = ListenabgleichCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ListenabgleichEvent::ListeErhalten {
                pruefidentifikator,
                typ,
                sender,
                receiver,
                billing_period,
                message_ref,
            } => ListenabgleichState::ListeErhalten(Box::new(ListenabgleichData {
                pruefidentifikator: *pruefidentifikator,
                typ: *typ,
                sender: sender.clone(),
                receiver: receiver.clone(),
                billing_period: billing_period.clone(),
                message_ref: message_ref.clone(),
            })),

            ListenabgleichEvent::KorrekturGesendet { korrekturen, .. } => match state {
                ListenabgleichState::ListeErhalten(d) => ListenabgleichState::Abgeglichen {
                    typ: d.typ,
                    korrekturen: *korrekturen,
                },
                other => other,
            },

            ListenabgleichEvent::ValidationFailed { reason } => {
                ListenabgleichState::ValidationFailed {
                    reason: reason.clone(),
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ListenabgleichCommand::ReceiveListe {
                pid,
                sender,
                receiver,
                billing_period,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ListenabgleichState::New) {
                    return Ok(vec![].into());
                }

                let Some(familie) = familie_for(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a MaBiS Listenabgleich list; expected one of {:?}",
                        LISTEN_FAMILIEN.iter().map(|f| f.liste).collect::<Vec<_>>()
                    )));
                };

                if !validation_passed {
                    return Ok(vec![ListenabgleichEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }

                Ok(vec![ListenabgleichEvent::ListeErhalten {
                    pruefidentifikator: pid,
                    typ: familie.typ,
                    sender,
                    receiver,
                    billing_period,
                    message_ref,
                }]
                .into())
            }

            ListenabgleichCommand::SendKorrektur { korrekturen } => {
                let ListenabgleichState::ListeErhalten(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendKorrektur requires state ListeErhalten, got {}",
                        state.label()
                    )));
                };

                let familie = familie_for(data.pruefidentifikator.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "no family for recorded list {}",
                        data.pruefidentifikator
                    ))
                })?;

                let antwort_pid = Pruefidentifikator::new(familie.antwort).map_err(|e| {
                    WorkflowError::rejected(format!("invalid Antwort PID {}: {e}", familie.antwort))
                })?;

                let outbox = PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": familie.antwort,
                        "korrekturen": korrekturen,
                        "billing_period": data.billing_period.as_str(),
                    }),
                );

                Ok(WorkflowOutput {
                    events: vec![ListenabgleichEvent::KorrekturGesendet {
                        antwort_pid,
                        korrekturen,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn receive(pid: u32) -> ListenabgleichCommand {
        ListenabgleichCommand::ReceiveListe {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            billing_period: BillingPeriod::new("2026-07"),
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn fold(events: &[ListenabgleichEvent]) -> ListenabgleichState {
        events.iter().fold(ListenabgleichState::default(), |s, e| {
            MabisListenabgleichWorkflow::apply(s, e)
        })
    }

    #[test]
    fn no_list_pid_is_also_a_reply_pid() {
        for f in LISTEN_FAMILIEN {
            assert!(
                familie_for(f.antwort).is_none(),
                "{} is a reply PID but also registered as a list — receiving it \
                 would ask for a correction to a correction",
                f.antwort
            );
        }
        assert_eq!(all_pids().len(), 6);
    }

    #[test]
    fn each_list_answers_with_its_own_pid() {
        assert_eq!(familie_for(55195).unwrap().antwort, 55196);
        assert_eq!(familie_for(55201).unwrap().antwort, 55202);
        assert_eq!(familie_for(55223).unwrap().antwort, 55224);
    }

    #[test]
    fn a_clean_reconciliation_still_sends_a_reply() {
        // Zero corrections is not silence: the AHB obliges a Prüfmitteilung
        // either way, so `korrekturen: 0` must still put a message on the wire.
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55223)).unwrap();
        let state = fold(&out.events);
        assert_eq!(state.label(), "ListeErhalten");

        let out = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendKorrektur { korrekturen: 0 },
        )
        .expect("clean reply");
        assert_eq!(out.outbox.len(), 1, "a clean list still owes a reply");
        assert_eq!(out.outbox[0].payload["pid"], 55224);
        assert_eq!(out.outbox[0].payload["korrekturen"], 0);
    }

    #[test]
    fn corrections_are_carried_into_the_terminal_state() {
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55201)).unwrap();
        let state = fold(&out.events);
        let out = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendKorrektur { korrekturen: 3 },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pid"], 55202);
        let final_state = out
            .events
            .iter()
            .fold(state, MabisListenabgleichWorkflow::apply);
        match final_state {
            ListenabgleichState::Abgeglichen { typ, korrekturen } => {
                assert_eq!(typ, ListenTyp::LfAacl);
                assert_eq!(korrekturen, 3);
            }
            other => panic!("expected Abgeglichen, got {}", other.label()),
        }
    }

    #[test]
    fn the_reply_goes_back_to_the_distributor() {
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55195)).unwrap();
        let state = fold(&out.events);
        let out = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendKorrektur { korrekturen: 1 },
        )
        .unwrap();
        assert_eq!(
            out.outbox[0].recipient.as_ref(),
            "9900123456789",
            "the Korrekturliste travels back up the axis the list came down"
        );
    }

    #[test]
    fn a_reply_before_a_list_is_rejected() {
        let err = MabisListenabgleichWorkflow::handle(
            &ListenabgleichState::New,
            ListenabgleichCommand::SendKorrektur { korrekturen: 0 },
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("ListeErhalten"), "{err}");
    }

    #[test]
    fn a_record_only_clearingliste_pid_is_not_accepted_here() {
        // 55069 is a `mabis-clearingliste` PID: distributed, nothing owed back.
        let err = MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55069))
            .expect_err("must reject");
        assert!(
            format!("{err}").contains("not a MaBiS Listenabgleich"),
            "{err}"
        );
    }

    #[test]
    fn validation_failure_is_terminal_and_owes_nothing() {
        let cmd = ListenabgleichCommand::ReceiveListe {
            pid: Pruefidentifikator::new(55195).expect("valid PID"),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            billing_period: BillingPeriod::new("2026-07"),
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: false,
            validation_errors: vec!["SG6 LOC missing".to_owned()],
        };
        let out = MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, cmd).unwrap();
        assert!(out.outbox.is_empty());
        assert_eq!(fold(&out.events).label(), "ValidationFailed");
    }
}
