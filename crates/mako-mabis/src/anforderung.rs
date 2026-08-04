//! MaBiS Anforderungen — ORDERS-based subscription and one-shot list requests.
//!
//! # Process overview
//!
//! A market partner requests a MaBiS list from the party that maintains it. The
//! list itself arrives out of band as its own message (UTILMD Clearingliste,
//! MSCONS Profile), so this workflow models the **request**, not the delivery.
//!
//! ```text
//! Anforderung (ORDERS 172xx) ──→ target
//!                                  │
//!                                  └── list delivered as its own process
//! ```
//!
//! # The subscription is in the payload, not the PID
//!
//! Five of the eight codes carry **both** the start and the end of an
//! Abonnement under the same Prüfidentifikator — the sequence diagrams for
//! 17201 read *"Start eines Abonnements"* and *"Beendigung eines Abonnements"*
//! against the identical code. Which one a message is comes from its content,
//! so [`AbonnementVorgang`] is an explicit input rather than something derived
//! from the PID. Deriving it would silently turn every unsubscribe into a
//! subscribe.
//!
//! The remaining three (17204, 17205, 17208) are one-shot requests with no
//! subscription to end; asking them to unsubscribe is rejected rather than sent
//! as a message the counterparty has no process for.
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt*.
//!
//! | PID   | Anforderung                                  | Von → An      | Abonnement |
//! |-------|----------------------------------------------|---------------|------------|
//! | 17201 | normierte Profile und Profilschar            | LF → NB       | ✅          |
//! | 17202 | Lieferantenclearingliste                     | LF → NB/ÜNB   | ✅          |
//! | 17203 | Bilanzkreiszuordnungsliste                   | BKV → NB/ÜNB  | ✅          |
//! | 17204 | Clearingliste BAS                            | BKV → BIKO    | —          |
//! | 17205 | Clearingliste DZR                            | NB → BIKO     | —          |
//! | 17206 | Bilanzierungsgebietsclearingliste            | NB → ÜNB      | ✅          |
//! | 17207 | Ab-/Bestellung BK-SZR auf Aggregationsebene  | BKV → ÜNB     | ✅          |
//! | 17208 | Clearingliste ÜNB-DZR                        | ÜNB → BIKO    | —          |
//!
//! 17207 is the clearest case: its own AHB name is *Ab-/Bestellung*, both
//! directions in one code.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 (MaBiS)**
//! - **ORDERS AHB** — message format
//!
//! # State machine
//!
//! mako plays both sides: it requests lists as LF/BKV/NB/ÜNB, and it receives
//! requests as NB/ÜNB/BIKO. The state records which side this stream is on.
//!
//! ```text
//! New ─┬─ AnforderungGesendet  → Gesendet (terminal)
//!      ├─ AnforderungErhalten  → Erhalten (terminal)
//!      └─ ValidationFailed     → ValidationFailed (terminal)
//! ```
//!
//! Terminal on both sides: the requested list is delivered by its own process
//! (`mabis-clearingliste`, `mabis-billing`), correlated by the list message, not
//! by this stream. Keeping the request open until the list arrives would model a
//! deadline BK6-24-174 does not define for these codes.

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Kind table ────────────────────────────────────────────────────────────────

/// Which MaBiS list an Anforderung asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnforderungKind {
    /// 17201 — normierte Profile und Profilschar (LF → NB).
    NormierteProfile,
    /// 17202 — Lieferantenclearingliste (LF → NB/ÜNB).
    Lieferantenclearingliste,
    /// 17203 — Bilanzkreiszuordnungsliste (BKV → NB/ÜNB).
    Bilanzkreiszuordnungsliste,
    /// 17204 — Clearingliste BAS (BKV → BIKO).
    ClearinglisteBas,
    /// 17205 — Clearingliste DZR (NB → BIKO).
    ClearinglisteDzr,
    /// 17206 — Bilanzierungsgebietsclearingliste (NB → ÜNB).
    Bilanzierungsgebietsclearingliste,
    /// 17207 — Ab-/Bestellung BK-SZR auf Aggregationsebene (BKV → ÜNB).
    BkSzrAggregationsebene,
    /// 17208 — Clearingliste ÜNB-DZR (ÜNB → BIKO).
    ClearinglisteUenbDzr,
}

impl AnforderungKind {
    /// Derive the kind from an ORDERS Prüfidentifikator.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        Some(match pid {
            17201 => Self::NormierteProfile,
            17202 => Self::Lieferantenclearingliste,
            17203 => Self::Bilanzkreiszuordnungsliste,
            17204 => Self::ClearinglisteBas,
            17205 => Self::ClearinglisteDzr,
            17206 => Self::Bilanzierungsgebietsclearingliste,
            17207 => Self::BkSzrAggregationsebene,
            17208 => Self::ClearinglisteUenbDzr,
            _ => return None,
        })
    }

    /// The ORDERS Prüfidentifikator carrying this Anforderung.
    #[must_use]
    pub fn pid(self) -> u32 {
        match self {
            Self::NormierteProfile => 17201,
            Self::Lieferantenclearingliste => 17202,
            Self::Bilanzkreiszuordnungsliste => 17203,
            Self::ClearinglisteBas => 17204,
            Self::ClearinglisteDzr => 17205,
            Self::Bilanzierungsgebietsclearingliste => 17206,
            Self::BkSzrAggregationsebene => 17207,
            Self::ClearinglisteUenbDzr => 17208,
        }
    }

    /// Canonical BDEW AHB name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NormierteProfile => "Anforderung normierter Profile und Profilschar",
            Self::Lieferantenclearingliste => "Anforderung Lieferantenclearingliste",
            Self::Bilanzkreiszuordnungsliste => "Anforderung Bilanzkreiszuordnungsliste",
            Self::ClearinglisteBas => "Anforderung Clearingliste BAS",
            Self::ClearinglisteDzr => "Anforderung Clearingliste DZR",
            Self::Bilanzierungsgebietsclearingliste => {
                "Anforderung Bilanzierungsgebietsclearingliste"
            }
            Self::BkSzrAggregationsebene => "Ab-/Bestellung BK-SZR auf Aggregationsebene",
            Self::ClearinglisteUenbDzr => "Anforderung Clearingliste ÜNB-DZR",
        }
    }

    /// Whether the AHB defines an Abonnement that can later be ended.
    ///
    /// One-shot requests (17204, 17205, 17208) ask for a list once; there is no
    /// subscription to cancel, so an [`AbonnementVorgang::Abbestellung`] on one
    /// is a modelling error, not a message.
    #[must_use]
    pub fn supports_abonnement(self) -> bool {
        !matches!(
            self,
            Self::ClearinglisteBas | Self::ClearinglisteDzr | Self::ClearinglisteUenbDzr
        )
    }

    /// The market role that sends this Anforderung.
    #[must_use]
    pub fn requester_role(self) -> &'static str {
        match self {
            Self::NormierteProfile | Self::Lieferantenclearingliste => "LF",
            Self::Bilanzkreiszuordnungsliste
            | Self::ClearinglisteBas
            | Self::BkSzrAggregationsebene => "BKV",
            Self::ClearinglisteDzr | Self::Bilanzierungsgebietsclearingliste => "NB",
            Self::ClearinglisteUenbDzr => "ÜNB",
        }
    }

    /// The market role(s) that receive this Anforderung.
    #[must_use]
    pub fn target_roles(self) -> &'static [&'static str] {
        match self {
            Self::NormierteProfile => &["NB"],
            Self::Lieferantenclearingliste | Self::Bilanzkreiszuordnungsliste => &["NB", "ÜNB"],
            Self::ClearinglisteBas | Self::ClearinglisteDzr | Self::ClearinglisteUenbDzr => {
                &["BIKO"]
            }
            Self::Bilanzierungsgebietsclearingliste | Self::BkSzrAggregationsebene => &["ÜNB"],
        }
    }
}

/// Whether the Anforderung starts or ends an Abonnement.
///
/// Not derivable from the Prüfidentifikator — see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbonnementVorgang {
    /// Start the subscription, or make a one-shot request.
    Bestellung,
    /// End an existing subscription.
    Abbestellung,
}

/// Every ORDERS Prüfidentifikator this workflow handles.
pub const ANFORDERUNG_PIDS: &[u32] = &[17201, 17202, 17203, 17204, 17205, 17206, 17207, 17208];

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-anforderung";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured for one Anforderung, in either direction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnforderungData {
    /// ORDERS Prüfidentifikator.
    pub pruefidentifikator: Pruefidentifikator,
    /// Which list is requested.
    pub kind: AnforderungKind,
    /// Whether the subscription is being started or ended.
    pub vorgang: AbonnementVorgang,
    /// GLN of the requesting party.
    pub sender: MarktpartnerCode,
    /// GLN of the party that maintains the list.
    pub receiver: MarktpartnerCode,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MaBiS Anforderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AnforderungEvent {
    /// Outbound Anforderung dispatched (mako is the requester).
    AnforderungGesendet {
        /// ORDERS Prüfidentifikator sent.
        pruefidentifikator: Pruefidentifikator,
        /// Which list was requested.
        kind: AnforderungKind,
        /// Subscription start or end.
        vorgang: AbonnementVorgang,
        /// GLN of the party the request was addressed to.
        receiver: MarktpartnerCode,
    },
    /// Inbound Anforderung received (mako maintains the list).
    AnforderungErhalten {
        /// ORDERS Prüfidentifikator received.
        pruefidentifikator: Pruefidentifikator,
        /// Which list was requested.
        kind: AnforderungKind,
        /// Subscription start or end.
        vorgang: AbonnementVorgang,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party (mako).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Human-readable summary of validation errors.
        reason: String,
    },
}

impl EventPayload for AnforderungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnforderungGesendet { .. } => "MabisAnforderungGesendet",
            Self::AnforderungErhalten { .. } => "MabisAnforderungErhalten",
            Self::ValidationFailed { .. } => "MabisAnforderungValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a MaBiS Anforderung process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum AnforderungState {
    /// No events yet.
    #[default]
    New,
    /// Outbound Anforderung dispatched (terminal).
    Gesendet {
        /// Which list was requested.
        kind: AnforderungKind,
        /// Subscription start or end.
        vorgang: AbonnementVorgang,
    },
    /// Inbound Anforderung recorded (terminal).
    Erhalten(Box<AnforderungData>),
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Validation error summary.
        reason: String,
    },
}

impl AnforderungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Gesendet { .. } => "Gesendet",
            Self::Erhalten(_) => "Erhalten",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MaBiS Anforderung workflow.
#[derive(Clone)]
pub enum AnforderungCommand {
    /// Dispatch an Anforderung to the party maintaining the list.
    SendAnforderung {
        /// Which list to request.
        kind: AnforderungKind,
        /// Start or end the subscription.
        vorgang: AbonnementVorgang,
        /// GLN of the party that maintains the list.
        receiver: MarktpartnerCode,
    },
    /// Record an inbound Anforderung addressed to mako.
    ReceiveAnforderung {
        /// ORDERS Prüfidentifikator of the inbound message.
        pid: Pruefidentifikator,
        /// Start or end, taken from the message content.
        vorgang: AbonnementVorgang,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party (mako).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for AnforderungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MaBiS Anforderung workflow — ORDERS 17201–17208.
///
/// Covers both directions: mako requesting a list, and mako receiving a request
/// for a list it maintains. See the module documentation for the PID table.
pub struct MabisAnforderungWorkflow;

impl Workflow for MabisAnforderungWorkflow {
    type State = AnforderungState;
    type Event = AnforderungEvent;
    type Command = AnforderungCommand;

    // Every event fully determines the next state, so the prior state is not
    // read — each variant is terminal.
    fn apply(_state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AnforderungEvent::AnforderungGesendet { kind, vorgang, .. } => {
                AnforderungState::Gesendet {
                    kind: *kind,
                    vorgang: *vorgang,
                }
            }
            AnforderungEvent::AnforderungErhalten {
                pruefidentifikator,
                kind,
                vorgang,
                sender,
                receiver,
                message_ref,
            } => AnforderungState::Erhalten(Box::new(AnforderungData {
                pruefidentifikator: *pruefidentifikator,
                kind: *kind,
                vorgang: *vorgang,
                sender: sender.clone(),
                receiver: receiver.clone(),
                message_ref: message_ref.clone(),
            })),
            AnforderungEvent::ValidationFailed { reason } => AnforderungState::ValidationFailed {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            AnforderungCommand::SendAnforderung {
                kind,
                vorgang,
                receiver,
            } => {
                if !matches!(state, AnforderungState::New) {
                    return Ok(vec![].into());
                }

                if vorgang == AbonnementVorgang::Abbestellung && !kind.supports_abonnement() {
                    return Err(WorkflowError::rejected(format!(
                        "{} (PID {}) is a one-shot request — the AHB defines no \
                         Abonnement to end",
                        kind.label(),
                        kind.pid()
                    )));
                }

                let pid = Pruefidentifikator::new(kind.pid()).map_err(|e| {
                    WorkflowError::rejected(format!("invalid PID {}: {e}", kind.pid()))
                })?;

                let outbox = PendingOutbox::new(
                    "ORDERS",
                    receiver.as_str(),
                    serde_json::json!({
                        "pid": kind.pid(),
                        "vorgang": vorgang,
                        "kind": kind,
                    }),
                );

                Ok(WorkflowOutput {
                    events: vec![AnforderungEvent::AnforderungGesendet {
                        pruefidentifikator: pid,
                        kind,
                        vorgang,
                        receiver,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            AnforderungCommand::ReceiveAnforderung {
                pid,
                vorgang,
                sender,
                receiver,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, AnforderungState::New) {
                    return Ok(vec![].into());
                }

                let Some(kind) = AnforderungKind::from_pid(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a MaBiS Anforderung; expected one of {ANFORDERUNG_PIDS:?}"
                    )));
                };

                if !validation_passed {
                    return Ok(vec![AnforderungEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }

                if vorgang == AbonnementVorgang::Abbestellung && !kind.supports_abonnement() {
                    return Err(WorkflowError::rejected(format!(
                        "inbound {} (PID {}) claims an Abbestellung, but the AHB \
                         defines no Abonnement for it",
                        kind.label(),
                        kind.pid()
                    )));
                }

                Ok(vec![AnforderungEvent::AnforderungErhalten {
                    pruefidentifikator: pid,
                    kind,
                    vorgang,
                    sender,
                    receiver,
                    message_ref,
                }]
                .into())
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

    fn fold(events: &[AnforderungEvent]) -> AnforderungState {
        events.iter().fold(AnforderungState::default(), |s, e| {
            MabisAnforderungWorkflow::apply(s, e)
        })
    }

    #[test]
    fn the_pid_table_round_trips() {
        for &pid in ANFORDERUNG_PIDS {
            let kind = AnforderungKind::from_pid(pid).expect("known PID");
            assert_eq!(kind.pid(), pid, "from_pid/pid disagree for {pid}");
        }
        assert!(AnforderungKind::from_pid(17209).is_none());
        assert!(AnforderungKind::from_pid(55062).is_none());
    }

    #[test]
    fn one_shot_requests_cannot_be_unsubscribed() {
        // 17204/17205/17208 have no Abonnement — the AHB sequence diagrams show
        // only "Anforderung und Übermittlung", never a Beendigung.
        for kind in [
            AnforderungKind::ClearinglisteBas,
            AnforderungKind::ClearinglisteDzr,
            AnforderungKind::ClearinglisteUenbDzr,
        ] {
            assert!(!kind.supports_abonnement(), "{kind:?}");
            let err = MabisAnforderungWorkflow::handle(
                &AnforderungState::New,
                AnforderungCommand::SendAnforderung {
                    kind,
                    vorgang: AbonnementVorgang::Abbestellung,
                    receiver: mp("9900123456789"),
                },
            )
            .expect_err("must reject");
            assert!(format!("{err}").contains("one-shot"), "{err}");
        }
    }

    #[test]
    fn subscription_kinds_accept_both_directions_under_the_same_pid() {
        // The whole point: 17207 is "Ab-/Bestellung" — one code, both verbs.
        let kind = AnforderungKind::BkSzrAggregationsebene;
        assert!(kind.supports_abonnement());
        for vorgang in [
            AbonnementVorgang::Bestellung,
            AbonnementVorgang::Abbestellung,
        ] {
            let out = MabisAnforderungWorkflow::handle(
                &AnforderungState::New,
                AnforderungCommand::SendAnforderung {
                    kind,
                    vorgang,
                    receiver: mp("9900123456789"),
                },
            )
            .expect("accepted");
            assert_eq!(out.outbox.len(), 1);
            assert_eq!(out.outbox[0].payload["pid"], 17207);
            assert_eq!(fold(&out.events).label(), "Gesendet");
        }
    }

    #[test]
    fn an_inbound_anforderung_is_recorded() {
        let out = MabisAnforderungWorkflow::handle(
            &AnforderungState::New,
            AnforderungCommand::ReceiveAnforderung {
                pid: Pruefidentifikator::new(17205).unwrap(),
                vorgang: AbonnementVorgang::Bestellung,
                sender: mp("9900123456789"),
                receiver: mp("9900987654321"),
                message_ref: MessageRef::new("MSG-1"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect("accepted");
        assert!(out.outbox.is_empty(), "receiving emits no message");
        assert_eq!(fold(&out.events).label(), "Erhalten");
    }

    #[test]
    fn an_inbound_abbestellung_on_a_one_shot_code_is_rejected() {
        let err = MabisAnforderungWorkflow::handle(
            &AnforderungState::New,
            AnforderungCommand::ReceiveAnforderung {
                pid: Pruefidentifikator::new(17204).unwrap(),
                vorgang: AbonnementVorgang::Abbestellung,
                sender: mp("9900123456789"),
                receiver: mp("9900987654321"),
                message_ref: MessageRef::new("MSG-1"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("no Abonnement"), "{err}");
    }

    #[test]
    fn validation_failure_is_terminal() {
        let out = MabisAnforderungWorkflow::handle(
            &AnforderungState::New,
            AnforderungCommand::ReceiveAnforderung {
                pid: Pruefidentifikator::new(17201).unwrap(),
                vorgang: AbonnementVorgang::Bestellung,
                sender: mp("9900123456789"),
                receiver: mp("9900987654321"),
                message_ref: MessageRef::new("MSG-1"),
                validation_passed: false,
                validation_errors: vec!["BGM missing".to_owned()],
            },
        )
        .expect("accepted");
        assert_eq!(fold(&out.events).label(), "ValidationFailed");
    }

    #[test]
    fn an_unknown_orders_pid_is_rejected() {
        let err = MabisAnforderungWorkflow::handle(
            &AnforderungState::New,
            AnforderungCommand::ReceiveAnforderung {
                pid: Pruefidentifikator::new(17004).unwrap(),
                vorgang: AbonnementVorgang::Bestellung,
                sender: mp("9900123456789"),
                receiver: mp("9900987654321"),
                message_ref: MessageRef::new("MSG-1"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect_err("must reject");
        assert!(
            format!("{err}").contains("not a MaBiS Anforderung"),
            "{err}"
        );
    }

    #[test]
    fn roles_match_the_bdew_overview() {
        // Spot-checks against the Kommunikation von/an columns.
        assert_eq!(AnforderungKind::NormierteProfile.requester_role(), "LF");
        assert_eq!(AnforderungKind::NormierteProfile.target_roles(), &["NB"]);
        assert_eq!(
            AnforderungKind::ClearinglisteUenbDzr.requester_role(),
            "ÜNB"
        );
        assert_eq!(
            AnforderungKind::ClearinglisteUenbDzr.target_roles(),
            &["BIKO"]
        );
        assert_eq!(
            AnforderungKind::Bilanzkreiszuordnungsliste.target_roles(),
            &["NB", "ÜNB"]
        );
    }
}
