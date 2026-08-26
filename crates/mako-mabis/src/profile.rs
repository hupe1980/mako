//! Normierte Profile und Profilscharen — delivery and Reklamation
//! (BK6-24-174 Anlage 3, Kap. 6.5 and 6.7).
//!
//! The NB publishes its normierte Lastprofile so an LF can forecast its
//! procurement and plausibilise the LF-SZR, and so an MSB can use them. The
//! subscription is [`crate::anforderung`] (ORDERS 17201), the list of profile
//! *definitions* is [`crate::clearingliste`] (UTILMD 55073); this module is the
//! **values** and the one obligation they carry back.
//!
//! ```text
//! NB ──(MSCONS 13010 / 13011 / 13012)──► LF | MSB
//!                                          │
//!                                          └──(ORDERS 17211 Reklamation, E_0100)──► NB
//! ```
//!
//! # A Reklamation does not invalidate the profile
//!
//! Kap. 6.5.2 Nr. 2 is explicit: „Wird eine Reklamation gesendet, hat das Profil
//! **weiterhin Gültigkeit**." The LF keeps billing on the profile it has and the
//! NB corrects „unverzüglich … über eine Korrektur". Modelling the Reklamation
//! as a rejection would strand the LF with no profile at all for the period,
//! which is the one outcome the Festlegung rules out.
//!
//! Kap. 6.5.1 adds the other half: „Diese mögliche Prüfung durch den LF entbindet
//! den NB nicht von seiner Pflicht, die Profile ordnungsgemäß und fristgerecht
//! zu erstellen." Checking is optional for the LF; delivering correctly is not
//! optional for the NB.
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt*.
//!
//! | PID   | Nachricht | Inhalt                                | Von → An      | Prozessschritt |
//! |-------|-----------|---------------------------------------|---------------|---------------:|
//! | 13010 | MSCONS    | normiertes Profil                     | NB → LF / MSB | 1              |
//! | 13011 | MSCONS    | Profilschar                           | NB → LF / MSB | 1              |
//! | 13012 | MSCONS    | TEP vergangenheitsbezogene Werte, Referenzmessung | NB → LF / MSB | 1 |
//! | 17211 | ORDERS    | Reklamation Profile bzw. Profilscharen | LF → NB      | 2              |
//!
//! **17211 is a MaBiS code.** Its Prozessbeschreibung column reads „MABIS" and
//! its sequence step is the answer leg of exactly this use case. It was
//! previously filed with the Redispatch ORDERS codes, which left the profile
//! delivery with no Reklamation at all.
//!
//! # Fristen (Kap. 6.5.3)
//!
//! The delivery Frist depends on the **Bilanzierungsverfahren the NB applies in
//! that Bilanzierungsgebiet**, not on the profile type:
//!
//! | Anlass | synthetisches Verfahren | analytisches Verfahren |
//! |---|---|---|
//! | Erstmalige Übermittlung nach einem neuen Abonnement | unverzüglich, spätestens **1 WT** nach Eingang der Anforderung | unverzüglich, spätestens **1 WT** |
//! | Laufende Übermittlung für den Bilanzierungsmonat | bis Ablauf des **10. WT** nach dem Bilanzierungsmonat | bis Ablauf des **12. WT** nach dem Bilanzierungsmonat |
//!
//! Those are the same 10/12 Werktage that separate the BG-SZR and BK-SZR
//! Erstaufschlag windows in [`crate::fristen`] — the profile has to be in the
//! LF's hands before the Summenzeitreihe it plausibilises closes.
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ ProfileErhalten ─┬─ (validation failed) ─→ ValidationFailed (terminal)
//!                      ├─ ReklamationGesendet ─→ Reklamiert       (terminal)
//!                      └─ (no defect found)   ─→ Erfasst          (terminal)
//! ```

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Constants ────────────────────────────────────────────────────────────────

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-profile";

/// MSCONS Prüfidentifikatoren carrying normierte Profile (NB → LF / MSB).
pub const PROFIL_PIDS: &[u32] = &[13_010, 13_011, 13_012];

/// ORDERS 17211 — Reklamation Profile bzw. Profilscharen (LF → NB).
pub const REKLAMATION_PID: u32 = 17_211;

/// EBD the NB runs on an inbound Reklamation.
pub const REKLAMATION_EBD: &str = "E_0100";

/// „Unverzüglich, spätestens jedoch 1 WT nach Eingang der Anforderung des
/// Abonnements" — the first delivery after a new subscription (Kap. 6.5.3).
pub const ERSTLIEFERUNG_WERKTAGE: u32 = 1;

// The two delivery Fristen above are the **NB's** obligations toward the LF;
// this workflow models the receiving side, so it registers no deadline for
// them. The constants stay because they bound when the LF may treat a profile
// as overdue — and because the monthly figures are the same 10/12 Werktage that
// separate the BG-SZR and BK-SZR Erstaufschlag windows in `crate::fristen`.

// ── Bilanzierungsverfahren ───────────────────────────────────────────────────

/// Which Bilanzierungsverfahren the NB applies in the Bilanzierungsgebiet.
///
/// This — not the profile type — decides the delivery Frist (Kap. 6.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bilanzierungsverfahren {
    /// Synthetisches Verfahren — monthly delivery by the 10. WT.
    Synthetisch,
    /// Analytisches Verfahren — monthly delivery by the 12. WT.
    Analytisch,
}

impl Bilanzierungsverfahren {
    /// Werktage after the Bilanzierungsmonat by which the monthly profiles must
    /// have reached the LF (Kap. 6.5.3).
    #[must_use]
    pub fn monatsfrist_werktage(self) -> u32 {
        match self {
            Self::Synthetisch => 10,
            Self::Analytisch => 12,
        }
    }
}

// ── Profilart ────────────────────────────────────────────────────────────────

/// Which profile artefact a delivery carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profilart {
    /// 13010 — normiertes Profil.
    NormiertesProfil,
    /// 13011 — Profilschar.
    Profilschar,
    /// 13012 — TEP vergangenheitsbezogene Werte aus Referenzmessung.
    TepReferenzmessung,
}

impl Profilart {
    /// Derive the artefact from its MSCONS Prüfidentifikator.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            13_010 => Some(Self::NormiertesProfil),
            13_011 => Some(Self::Profilschar),
            13_012 => Some(Self::TepReferenzmessung),
            _ => None,
        }
    }

    /// The MSCONS Prüfidentifikator this artefact travels on.
    #[must_use]
    pub fn pid(self) -> u32 {
        match self {
            Self::NormiertesProfil => 13_010,
            Self::Profilschar => 13_011,
            Self::TepReferenzmessung => 13_012,
        }
    }

    /// Canonical BDEW name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NormiertesProfil => "normiertes Profil",
            Self::Profilschar => "Profilschar",
            Self::TepReferenzmessung => "TEP vergangenheitsbezogene Werte (Referenzmessung)",
        }
    }
}

// ── Domain data ──────────────────────────────────────────────────────────────

/// Data captured when a profile delivery arrives.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilData {
    /// MSCONS Prüfidentifikator of the delivery.
    pub pruefidentifikator: Pruefidentifikator,
    /// Which artefact arrived.
    pub art: Profilart,
    /// GLN of the NB that published it.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving LF or MSB.
    pub receiver: MarktpartnerCode,
    /// Bilanzierungsmonat the values cover.
    pub bilanzierungsmonat: BillingPeriod,
    /// Version of the profile. Kap. 6.5.1: „Der NB übermittelt für jeden
    /// Zeitraum das Profil mit der höchsten Versionsnummer."
    pub version: u32,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ────────────────────────────────────────────────────────────

/// Events emitted by the Profil workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ProfilEvent {
    /// Profile delivery received and recorded.
    ProfileErhalten {
        /// MSCONS Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Which artefact arrived.
        art: Profilart,
        /// GLN of the publishing NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LF or MSB.
        receiver: MarktpartnerCode,
        /// Bilanzierungsmonat the values cover.
        bilanzierungsmonat: BillingPeriod,
        /// Profile version.
        version: u32,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Checked and accepted; nothing owed back (terminal).
    Erfasst {
        /// Reference of the recorded delivery.
        message_ref: MessageRef,
    },
    /// A Reklamation was dispatched to the NB (terminal).
    ///
    /// The profile stays in force — see the module docs.
    ReklamationGesendet {
        /// ORDERS Prüfidentifikator (17211).
        pruefidentifikator: Pruefidentifikator,
        /// Defect reported to the NB.
        maengel: String,
        /// Reference of the dispatched ORDERS.
        message_ref: MessageRef,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Human-readable summary of validation errors.
        reason: String,
    },
}

impl EventPayload for ProfilEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::ProfileErhalten { .. } => "MabisProfileErhalten",
            Self::Erfasst { .. } => "MabisProfileErfasst",
            Self::ReklamationGesendet { .. } => "MabisProfilReklamationGesendet",
            Self::ValidationFailed { .. } => "MabisProfilValidationFailed",
        }
    }
}

// ── Domain state ─────────────────────────────────────────────────────────────

/// Current state of a profile-delivery stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum ProfilState {
    /// No events yet.
    #[default]
    New,
    /// Delivery received; the LF may still check it.
    Erhalten(Box<ProfilData>),
    /// Checked and accepted (terminal).
    Erfasst(Box<ProfilData>),
    /// Reklamation sent; the profile remains in force (terminal).
    Reklamiert {
        /// The delivery that was complained about.
        data: Box<ProfilData>,
        /// Defect reported to the NB.
        maengel: String,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Validation error summary.
        reason: String,
    },
}

impl ProfilState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Erhalten(_) => "Erhalten",
            Self::Erfasst(_) => "Erfasst",
            Self::Reklamiert { .. } => "Reklamiert",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }

    /// The recorded delivery, when the state carries one.
    #[must_use]
    pub fn data(&self) -> Option<&ProfilData> {
        match self {
            Self::Erhalten(d) | Self::Erfasst(d) | Self::Reklamiert { data: d, .. } => Some(d),
            Self::New | Self::ValidationFailed { .. } => None,
        }
    }

    /// Whether the delivered profile is in force.
    ///
    /// Kap. 6.5.2 Nr. 2 — a Reklamation does **not** take it out of force.
    #[must_use]
    pub fn profil_gilt(&self) -> bool {
        matches!(
            self,
            Self::Erhalten(_) | Self::Erfasst(_) | Self::Reklamiert { .. }
        )
    }
}

// ── Domain commands ──────────────────────────────────────────────────────────

/// Commands for the Profil workflow.
#[derive(Clone)]
pub enum ProfilCommand {
    /// Inbound MSCONS profile delivery.
    ReceiveProfile {
        /// MSCONS Prüfidentifikator (13010 / 13011 / 13012).
        pid: Pruefidentifikator,
        /// GLN of the publishing NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LF or MSB.
        receiver: MarktpartnerCode,
        /// Bilanzierungsmonat the values cover.
        bilanzierungsmonat: BillingPeriod,
        /// Profile version.
        version: u32,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
    /// Record the delivery as accepted without a Reklamation.
    Akzeptieren,
    /// Send a Reklamation to the NB (ORDERS 17211).
    ///
    /// „Unverzüglich nach Feststellung eines Fehlers" (Kap. 6.5.2 Nr. 2) — no
    /// countable Frist, so none is registered.
    SendReklamation {
        /// The `E_0100` Reklamationsgrund (`A01`–`A06`).
        ///
        /// A published code, not a free-text category: `E_0100` names six and
        /// four of them (`A03`–`A06`) sit on the Profilschar branch, which a
        /// normiertes Profil never enters. The workflow refuses a code the
        /// tree cannot reach for the Profilart that arrived.
        antwortcode: String,
        /// Defect to report. Empty is refused: a Reklamation the NB cannot act
        /// on wastes the only correction leg the process has.
        maengel: String,
        /// Reference to assign to the outbound ORDERS.
        message_ref: MessageRef,
    },
}

impl CommandPayload for ProfilCommand {}

// ── Workflow ─────────────────────────────────────────────────────────────────

/// Normierte-Profile delivery workflow (MSCONS 13010–13012, ORDERS 17211).
pub struct MabisProfilWorkflow;

impl Workflow for MabisProfilWorkflow {
    type State = ProfilState;
    type Event = ProfilEvent;
    type Command = ProfilCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ProfilEvent::ProfileErhalten {
                pruefidentifikator,
                art,
                sender,
                receiver,
                bilanzierungsmonat,
                version,
                message_ref,
            } => ProfilState::Erhalten(Box::new(ProfilData {
                pruefidentifikator: *pruefidentifikator,
                art: *art,
                sender: sender.clone(),
                receiver: receiver.clone(),
                bilanzierungsmonat: bilanzierungsmonat.clone(),
                version: *version,
                message_ref: message_ref.clone(),
            })),

            ProfilEvent::Erfasst { .. } => match state {
                ProfilState::Erhalten(d) => ProfilState::Erfasst(d),
                other => other,
            },

            ProfilEvent::ReklamationGesendet { maengel, .. } => match state {
                ProfilState::Erhalten(d) => ProfilState::Reklamiert {
                    data: d,
                    maengel: maengel.clone(),
                },
                other => other,
            },

            ProfilEvent::ValidationFailed { reason } => ProfilState::ValidationFailed {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ProfilCommand::ReceiveProfile {
                pid,
                sender,
                receiver,
                bilanzierungsmonat,
                version,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ProfilState::New) {
                    // Idempotent: a redelivered profile is a no-op. A *newer*
                    // version is a new stream, keyed on (Profil, Monat, Version).
                    return Ok(vec![].into());
                }
                let Some(art) = Profilart::from_pid(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} trägt kein normiertes Profil — erwartet {PROFIL_PIDS:?}"
                    )));
                };
                if !validation_passed {
                    return Ok(vec![ProfilEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }
                Ok(vec![ProfilEvent::ProfileErhalten {
                    pruefidentifikator: pid,
                    art,
                    sender,
                    receiver,
                    bilanzierungsmonat,
                    version,
                    message_ref,
                }]
                .into())
            }

            ProfilCommand::Akzeptieren => {
                let ProfilState::Erhalten(data) = state else {
                    return Err(WorkflowError::invalid_state("Erhalten", state.label()));
                };
                Ok(vec![ProfilEvent::Erfasst {
                    message_ref: data.message_ref.clone(),
                }]
                .into())
            }

            ProfilCommand::SendReklamation {
                antwortcode,
                maengel,
                message_ref,
            } => {
                let ProfilState::Erhalten(data) = state else {
                    return Err(WorkflowError::invalid_state("Erhalten", state.label()));
                };
                if maengel.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "eine Reklamation ohne Mangelbeschreibung ist für den NB nicht \
                         bearbeitbar",
                    ));
                }
                let code = mako_pruefung::mabis::lookup(REKLAMATION_EBD, &antwortcode).ok_or_else(
                    || {
                        WorkflowError::rejected(format!(
                            "{REKLAMATION_EBD} veröffentlicht den Antwortcode \
                             {antwortcode} nicht"
                        ))
                    },
                )?;
                // `E_0100` Prüfschritt 2 splits and the halves never rejoin.
                if data.art == Profilart::NormiertesProfil
                    && matches!(code.code, "A03" | "A04" | "A05" | "A06")
                {
                    return Err(WorkflowError::rejected(format!(
                        "{} ist im Profilschar-Zweig von {REKLAMATION_EBD} \
                         veröffentlicht und für ein normiertes Profil nicht \
                         erreichbar",
                        code.code
                    )));
                }
                let pid = Pruefidentifikator::new(REKLAMATION_PID).map_err(|e| {
                    WorkflowError::rejected(format!("invalid PID {REKLAMATION_PID}: {e}"))
                })?;
                let outbox = PendingOutbox::new(
                    "ORDERS",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": REKLAMATION_PID,
                        "ebd": REKLAMATION_EBD,
                        "antwortcode": code.code,
                        "bedeutung": code.bedeutung,
                        "profilart": data.art,
                        "bilanzierungsmonat": data.bilanzierungsmonat.as_str(),
                        "version": data.version,
                        "maengel": maengel,
                    }),
                );
                Ok(WorkflowOutput {
                    events: vec![ProfilEvent::ReklamationGesendet {
                        pruefidentifikator: pid,
                        maengel,
                        message_ref,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }
        }
    }
}

/// Every Prüfidentifikator this workflow is registered for.
#[must_use]
pub fn all_pids() -> Vec<u32> {
    let mut v = PROFIL_PIDS.to_vec();
    v.push(REKLAMATION_PID);
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn receive(pid: u32) -> ProfilCommand {
        ProfilCommand::ReceiveProfile {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            bilanzierungsmonat: BillingPeriod::new("2026-01"),
            version: 3,
            message_ref: MessageRef::new("MSCONS-PROFIL-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn fold(events: &[ProfilEvent]) -> ProfilState {
        events
            .iter()
            .fold(ProfilState::default(), MabisProfilWorkflow::apply)
    }

    #[test]
    fn the_pid_table_round_trips() {
        for &pid in PROFIL_PIDS {
            let art = Profilart::from_pid(pid).expect("in the table");
            assert_eq!(art.pid(), pid);
        }
        assert!(
            Profilart::from_pid(13_003).is_none(),
            "13003 is a Summenzeitreihe"
        );
        assert!(Profilart::from_pid(REKLAMATION_PID).is_none());
    }

    #[test]
    fn the_frist_follows_the_bilanzierungsverfahren() {
        // Kap. 6.5.3 — the same 10/12 Werktage that separate the BG-SZR and
        // BK-SZR Erstaufschlag windows.
        assert_eq!(
            Bilanzierungsverfahren::Synthetisch.monatsfrist_werktage(),
            10
        );
        assert_eq!(
            Bilanzierungsverfahren::Analytisch.monatsfrist_werktage(),
            12
        );
    }

    #[test]
    fn a_reklamation_leaves_the_profile_in_force() {
        // Kap. 6.5.2 Nr. 2: „Wird eine Reklamation gesendet, hat das Profil
        // weiterhin Gültigkeit."
        let out = MabisProfilWorkflow::handle(&ProfilState::New, receive(13_011)).expect("ok");
        let state = fold(&out.events);
        assert!(state.profil_gilt());

        let out = MabisProfilWorkflow::handle(
            &state,
            ProfilCommand::SendReklamation {
                antwortcode: "A01".into(),
                maengel: "Profil H0 gehört zu keiner abonnierten Profilgruppe".into(),
                message_ref: MessageRef::new("ORDERS-REK-1"),
            },
        )
        .expect("ok");
        assert_eq!(out.outbox[0].payload["pid"], REKLAMATION_PID);
        assert_eq!(out.outbox[0].payload["ebd"], REKLAMATION_EBD);
        assert_eq!(out.outbox[0].payload["antwortcode"], "A01");
        assert_eq!(
            out.outbox[0].recipient.as_ref(),
            "9900123456789",
            "the Reklamation goes back to the publishing NB"
        );

        let state = out.events.iter().fold(state, MabisProfilWorkflow::apply);
        assert_eq!(state.label(), "Reklamiert");
        assert!(
            state.profil_gilt(),
            "a Reklamation must not strand the LF without a profile"
        );
    }

    /// `E_0100` Prüfschritt 2 splits into a Profil and a Profilschar branch
    /// that never rejoin: `A03`–`A06` are reachable only from the Profilschar
    /// half.
    #[test]
    fn a_profilschar_code_is_unreachable_for_a_normiertes_profil() {
        let profil = fold(
            &MabisProfilWorkflow::handle(&ProfilState::New, receive(13_010))
                .unwrap()
                .events,
        );
        let schar = fold(
            &MabisProfilWorkflow::handle(&ProfilState::New, receive(13_011))
                .unwrap()
                .events,
        );
        let reklamation = |code: &str| ProfilCommand::SendReklamation {
            antwortcode: code.to_owned(),
            maengel: "Maßeinheit weicht ab".into(),
            message_ref: MessageRef::new("ORDERS-REK-1"),
        };

        for code in ["A03", "A04", "A05", "A06"] {
            assert!(
                MabisProfilWorkflow::handle(&profil, reklamation(code)).is_err(),
                "{code} is Profilschar-only"
            );
            assert!(MabisProfilWorkflow::handle(&schar, reklamation(code)).is_ok());
        }
        // `A01` and `A02` are on the shared trunk resp. the Profil branch.
        assert!(MabisProfilWorkflow::handle(&profil, reklamation("A01")).is_ok());
    }

    #[test]
    fn an_unpublished_reklamationsgrund_is_refused() {
        let state = fold(
            &MabisProfilWorkflow::handle(&ProfilState::New, receive(13_010))
                .unwrap()
                .events,
        );
        assert!(
            MabisProfilWorkflow::handle(
                &state,
                ProfilCommand::SendReklamation {
                    antwortcode: "A99".into(),
                    maengel: "Sonstiges".into(),
                    message_ref: MessageRef::new("ORDERS-REK-1"),
                },
            )
            .is_err(),
            "E_0100 publishes A01-A06 only"
        );
    }

    #[test]
    fn a_reklamation_needs_a_defect() {
        let out = MabisProfilWorkflow::handle(&ProfilState::New, receive(13_010)).expect("ok");
        let state = fold(&out.events);
        assert!(
            MabisProfilWorkflow::handle(
                &state,
                ProfilCommand::SendReklamation {
                    antwortcode: "A01".into(),
                    maengel: "   ".into(),
                    message_ref: MessageRef::new("ORDERS-REK-1"),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn accepting_is_terminal_and_emits_nothing() {
        let out = MabisProfilWorkflow::handle(&ProfilState::New, receive(13_012)).expect("ok");
        let state = fold(&out.events);
        let out = MabisProfilWorkflow::handle(&state, ProfilCommand::Akzeptieren).expect("ok");
        assert!(out.outbox.is_empty());
        let state = out.events.iter().fold(state, MabisProfilWorkflow::apply);
        assert_eq!(state.label(), "Erfasst");
        assert!(MabisProfilWorkflow::handle(&state, ProfilCommand::Akzeptieren).is_err());
    }

    #[test]
    fn a_non_profile_pid_is_rejected() {
        assert!(MabisProfilWorkflow::handle(&ProfilState::New, receive(13_003)).is_err());
    }

    #[test]
    fn validation_failure_is_terminal() {
        let cmd = ProfilCommand::ReceiveProfile {
            pid: Pruefidentifikator::new(13_010).expect("valid PID"),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            bilanzierungsmonat: BillingPeriod::new("2026-01"),
            version: 3,
            message_ref: MessageRef::new("MSCONS-PROFIL-1"),
            validation_passed: false,
            validation_errors: vec!["SG6 LOC fehlt".into()],
        };
        let out = MabisProfilWorkflow::handle(&ProfilState::New, cmd).expect("ok");
        assert_eq!(fold(&out.events).label(), "ValidationFailed");
    }

    #[test]
    fn all_pids_covers_the_delivery_and_the_reklamation() {
        assert_eq!(all_pids(), vec![13_010, 13_011, 13_012, REKLAMATION_PID]);
    }
}
