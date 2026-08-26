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
//!                                  ├── list delivered as its own process
//!                                  └── or, for 17207 only, refused with
//!                                      ORDRSP 19204
//! ```
//!
//! # One code carries an Ablehnung
//!
//! Eight of the nine requests here are simply recorded; the list either arrives
//! or the counterparty raises the problem outside this process. **17207 is the
//! exception**: the PID overview gives it a Prozessschritt-2 answer,
//! **ORDRSP 19204 „Ablehnung Ab-/Bestellung der Aggregationsebene"** (ÜNB →
//! BKV), and the answering ÜNB runs a *different* decision tree per direction —
//! `E_0003` for a Bestellung, `E_0022` for an Abbestellung. Without that leg a
//! refused subscription is indistinguishable from an accepted one, and the BKV
//! keeps expecting a BK-SZR on an aggregation level it was never granted.
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
//! | 17210 | Lieferantenausfallarbeitsclearingliste       | LF → NB (ANB) | ✅          |
//!
//! 17207 is the clearest case: its own AHB name is *Ab-/Bestellung*, both
//! directions in one code.
//!
//! **17210 is a MaBiS request, not a Redispatch one.** Its subject is the
//! Ausfallarbeit, but the PID overview puts it under the MaBiS
//! Prozessbeschreibung: it asks the ANB for the
//! Lieferantenausfallarbeitsclearingliste that [`crate::listenabgleich`]
//! reconciles as 55201/55202. Routed to a Redispatch workflow it would leave
//! the LF with no way to subscribe to that list.
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
//! New ─┬─ AnforderungGesendet  → Gesendet ─┬─ (17207 only) AnforderungAbgelehnt → Abgelehnt
//!      │                                   └─ (terminal otherwise)
//!      ├─ AnforderungErhalten  → Erhalten (terminal)
//!      └─ ValidationFailed     → ValidationFailed (terminal)
//! ```
//!
//! Terminal apart from the 19204 leg: the requested list is delivered by its own
//! process (`mabis-clearingliste`, `mabis-listenabgleich`, `mabis-billing`),
//! correlated by the list message, not by this stream. Keeping the request open
//! until the list arrives would model a deadline BK6-24-174 does not define for
//! these codes.

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
    /// 17210 — Lieferantenausfallarbeitsclearingliste (LF → NB (ANB)).
    LieferantenausfallarbeitsClearingliste,
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
            17210 => Self::LieferantenausfallarbeitsClearingliste,
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
            Self::LieferantenausfallarbeitsClearingliste => 17210,
        }
    }

    /// The ORDRSP Prüfidentifikator on which this request can be **refused**,
    /// with the EBD the answering party runs per direction.
    ///
    /// Only 17207 has one (19204, ÜNB → BKV). For every other code the AHB
    /// defines no Ablehnung, so an `Ablehnung` command on one is refused rather
    /// than sent as a message the counterparty has no process for.
    #[must_use]
    pub fn ablehnung(self, vorgang: AbonnementVorgang) -> Option<(u32, &'static str)> {
        match (self, vorgang) {
            (Self::BkSzrAggregationsebene, AbonnementVorgang::Bestellung) => {
                Some((ABLEHNUNG_PID, "E_0003"))
            }
            (Self::BkSzrAggregationsebene, AbonnementVorgang::Abbestellung) => {
                Some((ABLEHNUNG_PID, "E_0022"))
            }
            _ => None,
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
            Self::LieferantenausfallarbeitsClearingliste => {
                "Anforderung Lieferantenausfallarbeitsclearingliste"
            }
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
            Self::NormierteProfile
            | Self::Lieferantenclearingliste
            | Self::LieferantenausfallarbeitsClearingliste => "LF",
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
            Self::NormierteProfile | Self::LieferantenausfallarbeitsClearingliste => &["NB"],
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
pub const ANFORDERUNG_PIDS: &[u32] = &[
    17201, 17202, 17203, 17204, 17205, 17206, 17207, 17208, 17210,
];

/// ORDRSP 19204 — „Ablehnung Ab-/Bestellung der Aggregationsebene" (ÜNB → BKV),
/// the only Ablehnung any MaBiS Anforderung has.
pub const ABLEHNUNG_PID: u32 = 19204;

/// Every Prüfidentifikator this workflow is registered for — requests and the
/// one Ablehnung.
#[must_use]
pub fn all_pids() -> Vec<u32> {
    let mut v = ANFORDERUNG_PIDS.to_vec();
    v.push(ABLEHNUNG_PID);
    v.sort_unstable();
    v
}

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
    /// The counterparty refused the Anforderung (ORDRSP 19204). Only 17207 has
    /// this leg.
    AnforderungAbgelehnt {
        /// ORDRSP Prüfidentifikator of the refusal (19204).
        pruefidentifikator: Pruefidentifikator,
        /// EBD the answering party ran — `E_0003` for a Bestellung, `E_0022`
        /// for an Abbestellung.
        ebd: String,
        /// Antwortcode from that EBD's Codeliste.
        code: String,
        /// EDIFACT message reference of the ORDRSP.
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
            Self::AnforderungAbgelehnt { .. } => "MabisAnforderungAbgelehnt",
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
    /// The counterparty refused the Anforderung with ORDRSP 19204 (terminal).
    Abgelehnt {
        /// Which list was requested.
        kind: AnforderungKind,
        /// Subscription start or end.
        vorgang: AbonnementVorgang,
        /// EBD the answering party ran.
        ebd: String,
        /// Antwortcode from that EBD's Codeliste.
        code: String,
    },
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
            Self::Abgelehnt { .. } => "Abgelehnt",
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
    /// Record an inbound ORDRSP 19204 refusing an Anforderung this participant
    /// sent.
    ReceiveAblehnung {
        /// ORDRSP Prüfidentifikator; must be [`ABLEHNUNG_PID`].
        pid: Pruefidentifikator,
        /// EBD the answering party ran, from `SG…` of the ORDRSP.
        ebd: String,
        /// Antwortcode from that EBD's Codeliste.
        code: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
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
    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
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
            AnforderungEvent::AnforderungAbgelehnt { ebd, code, .. } => match state {
                AnforderungState::Gesendet { kind, vorgang } => AnforderungState::Abgelehnt {
                    kind,
                    vorgang,
                    ebd: ebd.clone(),
                    code: code.clone(),
                },
                other => other,
            },
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

            AnforderungCommand::ReceiveAblehnung {
                pid,
                ebd,
                code,
                message_ref,
            } => {
                let AnforderungState::Gesendet { kind, vorgang } = state else {
                    return Err(WorkflowError::invalid_state("Gesendet", state.label()));
                };
                if pid.as_u32() != ABLEHNUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} ist keine Ablehnung einer MaBiS-Anforderung \
                         (erwartet {ABLEHNUNG_PID})"
                    )));
                }
                let Some((_, erwarteter_ebd)) = kind.ablehnung(*vorgang) else {
                    return Err(WorkflowError::rejected(format!(
                        "{} (PID {}) hat keine Ablehnung — der AHB definiert für \
                         diesen Code keinen ORDRSP",
                        kind.label(),
                        kind.pid()
                    )));
                };
                // The ÜNB answers a Bestellung out of E_0003 and an Abbestellung
                // out of E_0022. A code read against the wrong tree means
                // something else there, so the tree is checked, not assumed.
                if ebd != erwarteter_ebd {
                    return Err(WorkflowError::rejected(format!(
                        "Ablehnung nennt EBD {ebd}, für {} ist aber \
                         {erwarteter_ebd} maßgeblich",
                        kind.label()
                    )));
                }
                if code.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "Ablehnung ohne Antwortcode ist nicht auswertbar",
                    ));
                }
                Ok(vec![AnforderungEvent::AnforderungAbgelehnt {
                    pruefidentifikator: pid,
                    ebd,
                    code,
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

#[cfg(test)]
mod ablehnung_tests {
    use super::*;

    fn gesendet(kind: AnforderungKind, vorgang: AbonnementVorgang) -> AnforderungState {
        AnforderungState::Gesendet { kind, vorgang }
    }

    fn ablehnung(ebd: &str, code: &str) -> AnforderungCommand {
        AnforderungCommand::ReceiveAblehnung {
            pid: Pruefidentifikator::new(ABLEHNUNG_PID).expect("19204"),
            ebd: ebd.to_owned(),
            code: code.to_owned(),
            message_ref: MessageRef::new("ORDRSP-1"),
        }
    }

    #[test]
    fn only_17207_can_be_refused() {
        for &pid in ANFORDERUNG_PIDS {
            let kind = AnforderungKind::from_pid(pid).expect("in the table");
            let expected = kind == AnforderungKind::BkSzrAggregationsebene;
            for vorgang in [
                AbonnementVorgang::Bestellung,
                AbonnementVorgang::Abbestellung,
            ] {
                assert_eq!(
                    kind.ablehnung(vorgang).is_some(),
                    expected,
                    "{} ({pid}) / {vorgang:?}",
                    kind.label()
                );
            }
        }
    }

    #[test]
    fn the_ebd_differs_between_bestellung_and_abbestellung() {
        // One ORDRSP PID, two decision trees — reading a code against the wrong
        // one silently changes what the refusal says.
        let k = AnforderungKind::BkSzrAggregationsebene;
        assert_eq!(
            k.ablehnung(AbonnementVorgang::Bestellung),
            Some((ABLEHNUNG_PID, "E_0003"))
        );
        assert_eq!(
            k.ablehnung(AbonnementVorgang::Abbestellung),
            Some((ABLEHNUNG_PID, "E_0022"))
        );
    }

    #[test]
    fn a_refusal_against_the_wrong_tree_is_rejected() {
        let state = gesendet(
            AnforderungKind::BkSzrAggregationsebene,
            AbonnementVorgang::Bestellung,
        );
        assert!(MabisAnforderungWorkflow::handle(&state, ablehnung("E_0022", "A01")).is_err());
        assert!(MabisAnforderungWorkflow::handle(&state, ablehnung("E_0003", "A01")).is_ok());
    }

    #[test]
    fn a_refusal_needs_a_code() {
        let state = gesendet(
            AnforderungKind::BkSzrAggregationsebene,
            AbonnementVorgang::Bestellung,
        );
        assert!(MabisAnforderungWorkflow::handle(&state, ablehnung("E_0003", "  ")).is_err());
    }

    #[test]
    fn a_refusal_of_a_code_that_has_none_is_rejected() {
        let state = gesendet(
            AnforderungKind::ClearinglisteBas,
            AbonnementVorgang::Bestellung,
        );
        assert!(MabisAnforderungWorkflow::handle(&state, ablehnung("E_0003", "A01")).is_err());
    }

    #[test]
    fn a_refusal_before_the_request_was_sent_is_rejected() {
        assert!(
            MabisAnforderungWorkflow::handle(&AnforderungState::New, ablehnung("E_0003", "A01"))
                .is_err()
        );
    }

    #[test]
    fn a_refusal_lands_in_abgelehnt() {
        let state = gesendet(
            AnforderungKind::BkSzrAggregationsebene,
            AbonnementVorgang::Abbestellung,
        );
        let out =
            MabisAnforderungWorkflow::handle(&state, ablehnung("E_0022", "A02")).expect("accepted");
        let next = out
            .events
            .iter()
            .fold(state, MabisAnforderungWorkflow::apply);
        assert_eq!(next.label(), "Abgelehnt");
    }

    #[test]
    fn the_lieferantenausfallarbeits_clearingliste_is_a_mabis_request() {
        // A Redispatch workflow would leave the LF with no way to subscribe
        // to the list mabis-listenabgleich reconciles.
        let k = AnforderungKind::from_pid(17210).expect("17210 is a MaBiS Anforderung");
        assert_eq!(k.requester_role(), "LF");
        assert_eq!(k.target_roles(), &["NB"]);
        assert!(
            k.supports_abonnement(),
            "the AHB names a Beendigung des Abonnements"
        );
    }

    #[test]
    fn all_pids_includes_the_ablehnung() {
        let pids = all_pids();
        assert!(pids.contains(&ABLEHNUNG_PID));
        assert_eq!(pids.len(), ANFORDERUNG_PIDS.len() + 1);
    }
}
