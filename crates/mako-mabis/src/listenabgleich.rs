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
//! The lists in [`crate::clearingliste`] (55067/55069/55070/55073) are
//! **record-only**: they are distributed and nothing is expected back. The four
//! here carry a reply leg, so modelling them the same way would drop the
//! correction obligation entirely.
//!
//! **55065 is one of them.** The PID overview gives the Lieferantenclearingliste
//! a Prozessschritt-3 answer:
//! **55066 „Korrekturliste zu Lieferantenclearingliste"**, LF → NB with EBD
//! `E_0047` and LF → ÜNB with `E_0004`. An LF that never sends it has silently
//! accepted whatever the NB filed.
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
//! | Liste | Von → An       | Antwort | Von → An       | EBD der Antwort       | Inhalt                            |
//! |------:|----------------|--------:|----------------|-----------------------|-----------------------------------|
//! | 55065 | NB → LF        | 55066   | LF → NB        | `E_0047`              | Lieferantenclearingliste          |
//! | 55065 | ÜNB → LF       | 55066   | LF → ÜNB       | `E_0004`              | Lieferantenclearingliste          |
//! | 55195 | ÜNB → NB       | 55196   | NB → ÜNB       | `E_0017`              | Bilanzierungsgebietsclearingliste |
//! | 55201 | NB → LF        | 55202   | LF → NB        | `E_0097`              | LF-AACL                           |
//! | 55223 | ÜNB → NB       | 55224   | NB → ÜNB       | `E_0070`              | DZÜ-Liste                         |
//!
//! The reply always travels back along the same axis the list came down, with
//! the roles swapped — which is why the receiving role is derived from the list
//! rather than passed in.
//!
//! **The EBD depends on who sent the list, not on the answer PID.** 55066 is
//! answered out of `E_0047` when the NB distributed the list and out of `E_0004`
//! when the ÜNB did. One PID, two disjoint code spaces — the same trap `A02`
//! sets across the GPKE trees — so [`ListenFamilie::antwort_ebd`] is looked up
//! by sender role and never assumed.
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
use mako_pruefung::mabis::Korrekturposition;

// ── List table ────────────────────────────────────────────────────────────────

/// Which MaBiS list is being reconciled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenTyp {
    /// 55065/55066 — Lieferantenclearingliste (NB ↔ LF, ÜNB ↔ LF).
    Lieferantenclearingliste,
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
    /// Market roles that may distribute the list, each with the EBD the
    /// answering party runs to build its reply.
    ///
    /// More than one entry means the same PID pair is used on two axes with
    /// **different** answer code spaces — see the module docs on 55066.
    pub sender_ebd: &'static [(&'static str, &'static str)],
    /// Market role that reconciles it and returns the corrections.
    pub empfaenger_rolle: &'static str,
}

impl ListenFamilie {
    /// The EBD the answering party runs when `sender_rolle` distributed the list.
    ///
    /// Returns `None` for a role this list is never sent by — answering out of
    /// the wrong tree would produce a code that means something else there.
    #[must_use]
    pub fn antwort_ebd(&self, sender_rolle: &str) -> Option<&'static str> {
        self.sender_ebd
            .iter()
            .find(|(r, _)| *r == sender_rolle)
            .map(|(_, ebd)| *ebd)
    }

    /// Every role that may distribute this list.
    pub fn sender_rollen(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.sender_ebd.iter().map(|(r, _)| *r)
    }
}

/// Every list/correction pair this workflow handles.
pub const LISTEN_FAMILIEN: &[ListenFamilie] = &[
    ListenFamilie {
        liste: 55065,
        antwort: 55066,
        typ: ListenTyp::Lieferantenclearingliste,
        // One PID pair, two axes, two disjoint EBD code spaces.
        sender_ebd: &[("NB", "E_0047"), ("ÜNB", "E_0004")],
        empfaenger_rolle: "LF",
    },
    ListenFamilie {
        liste: 55195,
        antwort: 55196,
        typ: ListenTyp::Bilanzierungsgebietsclearingliste,
        sender_ebd: &[("ÜNB", "E_0017")],
        empfaenger_rolle: "NB",
    },
    ListenFamilie {
        liste: 55201,
        antwort: 55202,
        typ: ListenTyp::LfAacl,
        sender_ebd: &[("NB", "E_0097")],
        empfaenger_rolle: "LF",
    },
    ListenFamilie {
        liste: 55223,
        antwort: 55224,
        typ: ListenTyp::DzuListe,
        sender_ebd: &[("ÜNB", "E_0070")],
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
        /// The market role that distributed the list (`"NB"`, `"ÜNB"`).
        ///
        /// It decides which Entscheidungsbaum the corrections are drawn from:
        /// 55066 is answered out of `E_0047` when the NB sent the list and out
        /// of `E_0004` when the ÜNB did. One PID, two disjoint code spaces.
        sender_rolle: String,
        /// The disputed positions, one per Marktlokation. An **empty** vector
        /// is valid and still sends a reply — silence would read as acceptance
        /// of whatever the sender filed.
        positionen: Vec<Korrekturposition>,
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

            ListenabgleichCommand::SendKorrektur {
                sender_rolle,
                positionen,
            } => {
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

                // The EBD follows from who *sent* the list, not from the answer
                // PID — 55066 is answered out of `E_0047` (NB) or `E_0004`
                // (ÜNB), whose Korrekturgründe carry different code numbers.
                let ebd = familie.antwort_ebd(&sender_rolle).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "{sender_rolle} verteilt die Liste {} nicht; \
                         zulässig: {:?}",
                        familie.liste,
                        familie.sender_rollen().collect::<Vec<_>>()
                    ))
                })?;

                // `SendKorrektur` *is* the Korrekturlisten leg: the caller has
                // already established that the list is assessable. So resolve
                // each position against the tree directly rather than walking
                // the whole-list Prüfschritte with facts nobody checked.
                let mut eintraege = Vec::with_capacity(positionen.len());
                for pos in &positionen {
                    let (code, _) = mako_pruefung::mabis::korrekturcode(ebd, pos.grund)
                        .ok_or_else(|| {
                            WorkflowError::rejected(format!(
                                "{ebd} veröffentlicht keinen Code für {:?}",
                                pos.grund
                            ))
                        })?;
                    eintraege.push((pos.malo.clone(), code));
                }

                let korrekturen = u32::try_from(eintraege.len()).unwrap_or(u32::MAX);
                let outbox = PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": familie.antwort,
                        "ebd": ebd,
                        "korrekturen": korrekturen,
                        "positionen": eintraege
                            .iter()
                            .map(|(malo, code)| serde_json::json!({
                                "malo": malo,
                                "antwortcode": code.code,
                                "bedeutung": code.bedeutung,
                            }))
                            .collect::<Vec<_>>(),
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

    /// One disputed position, with the semantic Korrekturgrund. Which code it
    /// becomes is the tree's decision, not the caller's.
    fn pos(n: usize, grund: mako_pruefung::mabis::Korrekturgrund) -> Korrekturposition {
        Korrekturposition {
            malo: format!("5123869678{n}"),
            grund,
        }
    }

    fn korrektur(rolle: &str, positionen: Vec<Korrekturposition>) -> ListenabgleichCommand {
        ListenabgleichCommand::SendKorrektur {
            sender_rolle: rolle.to_owned(),
            positionen,
        }
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
        assert_eq!(all_pids().len(), LISTEN_FAMILIEN.len() * 2);
    }

    #[test]
    fn each_list_answers_with_its_own_pid() {
        assert_eq!(familie_for(55065).unwrap().antwort, 55066);
        assert_eq!(familie_for(55195).unwrap().antwort, 55196);
        assert_eq!(familie_for(55201).unwrap().antwort, 55202);
        assert_eq!(familie_for(55223).unwrap().antwort, 55224);
    }

    #[test]
    fn the_lieferantenclearingliste_is_not_record_only() {
        // It used to sit with the record-only lists; the PID overview gives it
        // a Prozessschritt-3 Korrekturliste, so an LF owes 55066 back.
        assert!(familie_for(55065).is_some());
        assert!(!crate::clearingliste::CLEARINGLISTE_PIDS.contains(&55065));
    }

    #[test]
    fn the_55066_ebd_depends_on_who_sent_the_list() {
        // One PID, two axes, two disjoint code spaces — answering out of the
        // wrong tree produces a code that means something else there.
        let f = familie_for(55065).unwrap();
        assert_eq!(f.antwort_ebd("NB"), Some("E_0047"));
        assert_eq!(f.antwort_ebd("ÜNB"), Some("E_0004"));
        assert_eq!(f.antwort_ebd("BIKO"), None, "never sent by the BIKO");
    }

    #[test]
    fn every_family_names_an_ebd_for_every_sender_it_admits() {
        for f in LISTEN_FAMILIEN {
            assert!(!f.sender_ebd.is_empty(), "{} has no sender", f.liste);
            for rolle in f.sender_rollen() {
                let ebd = f.antwort_ebd(rolle).expect("declared sender");
                assert!(ebd.starts_with("E_0"), "{} → {ebd}", f.liste);
            }
        }
    }

    #[test]
    fn a_clean_reconciliation_still_sends_a_reply() {
        // Zero corrections is not silence: the AHB obliges a Prüfmitteilung
        // either way, so `korrekturen: 0` must still put a message on the wire.
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55223)).unwrap();
        let state = fold(&out.events);
        assert_eq!(state.label(), "ListeErhalten");

        let out = MabisListenabgleichWorkflow::handle(&state, korrektur("ÜNB", vec![]))
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
            korrektur(
                "NB",
                (1..=3)
                    .map(|n| pos(n, mako_pruefung::mabis::Korrekturgrund::DatenFehlerhaft))
                    .collect(),
            ),
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

    /// One PID, two disjoint code spaces: 55066 is answered out of `E_0047`
    /// when the NB distributed the list and out of `E_0004` when the ÜNB did.
    #[test]
    fn the_ebd_follows_the_distributor_not_the_answer_pid() {
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55065)).unwrap();
        let state = fold(&out.events);
        let positionen = vec![pos(
            1,
            mako_pruefung::mabis::Korrekturgrund::DatenFehlerhaft,
        )];

        let code = |rolle: &str| {
            let out =
                MabisListenabgleichWorkflow::handle(&state, korrektur(rolle, positionen.clone()))
                    .expect("both roles distribute 55065");
            (
                out.outbox[0].payload["ebd"].as_str().unwrap().to_owned(),
                out.outbox[0].payload["positionen"][0]["antwortcode"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        };
        assert_eq!(code("NB"), ("E_0047".to_owned(), "A07".to_owned()));
        assert_eq!(code("ÜNB"), ("E_0004".to_owned(), "A06".to_owned()));
    }

    #[test]
    fn the_reply_goes_back_to_the_distributor() {
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55195)).unwrap();
        let state = fold(&out.events);
        // 55195 is distributed by the ÜNB, so its corrections come from `E_0017`.
        let out = MabisListenabgleichWorkflow::handle(
            &state,
            korrektur(
                "ÜNB",
                vec![pos(
                    1,
                    mako_pruefung::mabis::Korrekturgrund::DatenFehlerhaft,
                )],
            ),
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["ebd"], "E_0017");
        assert_eq!(
            out.outbox[0].recipient.as_ref(),
            "9900123456789",
            "the Korrekturliste travels back up the axis the list came down"
        );
    }

    #[test]
    fn a_reply_before_a_list_is_rejected() {
        let err =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, korrektur("NB", vec![]))
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
