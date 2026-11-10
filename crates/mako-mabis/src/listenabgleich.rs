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
//! The lists in [`crate::clearingliste`] (55067/55069/55070/55073) are
//! **record-only**; the four here carry a reply leg, and modelling them the
//! same way would drop the correction obligation. **55065 is one of them**: the
//! PID overview gives the Lieferantenclearingliste a Prozessschritt-3 answer,
//! **55066 „Korrekturliste zu Lieferantenclearingliste"** — an LF that never
//! sends it has silently accepted whatever the NB filed.
//!
//! Unlike [`crate::zp_lifecycle`], the reply is not a confirm/reject pair:
//! 55202 is a *Korrekturliste*, 55224 a *Prüfmitteilung*. An accept/reject
//! shape would make "list received, three positions corrected"
//! unrepresentable.
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt*.
//!
//! | Liste | Von → An       | Antwort | Von → An       | EBD Einzelanforderung | EBD gültige Abonnierung | Inhalt                            |
//! |------:|----------------|--------:|----------------|-----------------------|-------------------------|-----------------------------------|
//! | 55065 | NB → LF        | 55066   | LF → NB        | `E_0047`              | `E_0049`                | Lieferantenclearingliste          |
//! | 55065 | ÜNB → LF       | 55066   | LF → ÜNB       | `E_0014`              | `E_0004`                | Lieferantenclearingliste          |
//! | 55195 | ÜNB → NB       | 55196   | NB → ÜNB       | `E_0017`              | `E_0052`                | Bilanzierungsgebietsclearingliste |
//! | 55201 | NB → LF        | 55202   | LF → NB        | `E_0097`              | `E_0096`                | LF-AACL                           |
//! | 55223 | ÜNB → NB       | 55224   | NB → ÜNB       | `E_0070`              | `E_0070`                | DZÜ-Liste                         |
//!
//! The reply always travels back along the same axis the list came down, with
//! the roles swapped — which is why the receiving role is derived from the list
//! rather than passed in.
//!
//! **Two facts select the tree, and the answer PID is neither.** EBD 4.3
//! publishes one tree per distributor *and* per kind of order: the
//! Aktivitätsdiagramme „Austausch der Lieferantenclearingliste zwischen NB und
//! LF (Einzelanforderung)" (`E_0047`, Kap. 7.14.2, S. 445) and „… (gültige
//! Abonnierung)" (`E_0049`, Kap. 7.15.1, S. 448), the ÜNB's pair `E_0014`
//! (Kap. 7.18.2, S. 453) / `E_0004` (Kap. 7.19.1, S. 456), the
//! Bilanzierungsgebietsclearingliste `E_0017` (Kap. 7.23.2, S. 463) / `E_0052`
//! (Kap. 7.24.1, S. 466) and the LF-AACL `E_0097` (Kap. 7.59.1, S. 513) /
//! `E_0096` (Kap. 7.58.1, S. 511). The DZÜ-Liste has the one tree `E_0070`
//! (Kap. 7.48.1, S. 501). The trees differ in their whole-list questions —
//! an Abonnement tree asks whether the Abonnement exists, an Einzelanforderung
//! tree whether the list answers the Zeitraum and MaBiS-ZP asked for — and in
//! their numbering, so [`ListenFamilie::antwort_ebd`] takes the sender role and
//! the [`Anforderungsart`] and never assumes either.
//!
//! # Both ends of the list
//!
//! The Netzbetreiber distributes 55065 and 55201 itself
//! ([`ListenabgleichCommand::SendListe`]) and the Lieferant's Korrekturliste
//! 55066 / 55202 resumes that process
//! ([`ListenabgleichCommand::ReceiveKorrektur`]). Every code it carries is
//! checked against the tree the Lieferant must have answered from — the one
//! the list's own sender role and Anforderungsart select — so a code from
//! another tree is refused rather than recorded under a meaning it does not
//! have.
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
//!  ├─ ListeErhalten ─┬─ (validation failed) ────→ ValidationFailed  (terminal)
//!  │                 ├─ KorrekturGesendet ──────→ Abgeglichen       (terminal)
//!  │                 └─ GesamtAblehnungGesendet ─→ Abgelehnt        (terminal)
//!  └─ ListeGesendet ─┬─ KorrekturErhalten ──────→ Korrigiert        (terminal)
//!                    └─ GesamtAblehnungErhalten ─→ ListeAbgelehnt    (terminal)
//! ```
//!
//! # Two disjoint refusals, and only one of them carries positions
//!
//! Every Clearinglisten-Tree publishes two clusters. A
//! **Korrekturliste** disputes named Marktlokationen — one entry each — and says
//! the list was assessed. An **Ablehnung der gesamten Liste** carries *no*
//! positions and demands a new list: the Abonnement was never ordered, the
//! version is not admitted, the Zeitraum is implausible, or (`E_0070`) the list
//! arrived outside the Clearingphase DZÜ.
//!
//! They are not interchangeable: an empty Korrekturliste states „reconciled,
//! nothing to correct", and a whole-list refusal throws away every position the
//! receiver did check. The whole-list Prüfschritte
//! therefore run **first and to completion**
//! ([`mako_pruefung::mabis::pruefe_liste`]), and which command the caller sends
//! is checked against what that walk decides rather than trusted.
//!
//! # On the wire it is a list, not a Vorgang
//!
//! `BGM+Z05` Clearingliste, the Bilanzierungsmonat in `DTM+157` (`610`
//! `CCYYMM`, never the Dokumentendatum — a December list distributed in
//! January reconciles December), then an `IDE+Z01` head that *is* the
//! Geschäftsvorfall: „Alle aufgelisteten IDE+24 sind Bestandteil des
//! Geschäftsvorfalls" (UTILMD AHB Strom 2.2 Kap. 13.4, Bedingung `[564]`).
//! The head names the MaBiS-Zählpunkt (`SG5 LOC+Z15`), the Version der
//! Zeitreihe (`SG8 SEQ+Z22` / `RFF+AUU`) and the answered list's own number
//! (`SG6 RFF+TN`); each disputed Marktlokation follows as an `IDE+24` with its
//! own `STS+E01` and `LOC+Z16`.
//!
//! Head status and members are **mutually exclusive** — Bedingung `[238]`
//! marks the head `STS+E01` Muss „wenn SG4 IDE+24 (Vorgang) nicht vorhanden",
//! and `[630]`: „wenn die Liste abgelehnt wird, ist kein Vorgang enthalten".
//! So a whole-list Ablehnung renders as a single Vorgang and a Korrekturliste
//! as a list, which is the same split the two commands make.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
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

/// How the list was ordered — the second axis EBD 4.3 selects a tree on.
///
/// The Lieferant's 17202 / 17210 orders a list either once or as an
/// Abonnement, and the list echoes that order in `SG1 RFF+AAV` („Angabe des
/// BGM DE1004 aus der ORDERS", UTILMD AHB Strom 2.2 Kap. 13.2, S. 1110).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anforderungsart {
    /// A single list for one Bilanzierungsmonat („Einzelanforderung").
    Einzelanforderung,
    /// A list delivered under a running Abonnement („gültige Abonnierung").
    Abonnement,
}

impl Anforderungsart {
    /// Parse the REST spelling (`einzelanforderung` / `abonnement`).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "einzelanforderung" => Some(Self::Einzelanforderung),
            "abonnement" => Some(Self::Abonnement),
            _ => None,
        }
    }
}

/// The trees a distributor's list is answered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baeume {
    /// One tree whatever the order (`E_0070`, the DZÜ-Liste).
    Einer(&'static str),
    /// One tree per [`Anforderungsart`].
    JeAnforderung {
        /// The tree for an Einzelanforderung.
        einzelanforderung: &'static str,
        /// The tree for a list under a valid Abonnement.
        abonnement: &'static str,
    },
}

impl Baeume {
    /// The tree for `art`; `None` when two trees exist and `art` is unknown.
    #[must_use]
    pub fn waehle(self, art: Option<Anforderungsart>) -> Option<&'static str> {
        match (self, art) {
            (Self::Einer(ebd), _) => Some(ebd),
            (
                Self::JeAnforderung {
                    einzelanforderung, ..
                },
                Some(Anforderungsart::Einzelanforderung),
            ) => Some(einzelanforderung),
            (Self::JeAnforderung { abonnement, .. }, Some(Anforderungsart::Abonnement)) => {
                Some(abonnement)
            }
            (Self::JeAnforderung { .. }, None) => None,
        }
    }

    /// Every tree this entry names.
    #[must_use]
    pub fn alle(self) -> Vec<&'static str> {
        match self {
            Self::Einer(ebd) => vec![ebd],
            Self::JeAnforderung {
                einzelanforderung,
                abonnement,
            } => vec![einzelanforderung, abonnement],
        }
    }
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
    /// Market roles that may distribute the list, each with the trees the
    /// answering party runs to build its reply.
    ///
    /// More than one entry means the same PID pair is used on two axes with
    /// **different** answer code spaces — see the module docs on 55066.
    pub sender_ebd: &'static [(&'static str, Baeume)],
    /// Market role that reconciles it and returns the corrections.
    pub empfaenger_rolle: &'static str,
}

impl ListenFamilie {
    /// The trees the answering party runs when `sender_rolle` distributed the
    /// list, or `None` for a role this list is never sent by.
    #[must_use]
    pub fn baeume(&self, sender_rolle: &str) -> Option<Baeume> {
        self.sender_ebd
            .iter()
            .find(|(r, _)| *r == sender_rolle)
            .map(|(_, b)| *b)
    }

    /// The EBD the answering party runs when `sender_rolle` distributed the
    /// list ordered as `art`.
    ///
    /// Returns `None` for a role this list is never sent by, and for a family
    /// with one tree per Anforderungsart when `art` is unknown — answering out
    /// of the wrong tree would produce a code that means something else there.
    #[must_use]
    pub fn antwort_ebd(
        &self,
        sender_rolle: &str,
        art: Option<Anforderungsart>,
    ) -> Option<&'static str> {
        self.baeume(sender_rolle)?.waehle(art)
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
        // One PID pair, two axes, four disjoint EBD code spaces.
        sender_ebd: &[
            (
                "NB",
                Baeume::JeAnforderung {
                    einzelanforderung: "E_0047",
                    abonnement: "E_0049",
                },
            ),
            (
                "ÜNB",
                Baeume::JeAnforderung {
                    einzelanforderung: "E_0014",
                    abonnement: "E_0004",
                },
            ),
        ],
        empfaenger_rolle: "LF",
    },
    ListenFamilie {
        liste: 55195,
        antwort: 55196,
        typ: ListenTyp::Bilanzierungsgebietsclearingliste,
        sender_ebd: &[(
            "ÜNB",
            Baeume::JeAnforderung {
                einzelanforderung: "E_0017",
                abonnement: "E_0052",
            },
        )],
        empfaenger_rolle: "NB",
    },
    ListenFamilie {
        liste: 55201,
        antwort: 55202,
        typ: ListenTyp::LfAacl,
        sender_ebd: &[(
            "NB",
            Baeume::JeAnforderung {
                einzelanforderung: "E_0097",
                abonnement: "E_0096",
            },
        )],
        empfaenger_rolle: "LF",
    },
    ListenFamilie {
        liste: 55223,
        antwort: 55224,
        typ: ListenTyp::DzuListe,
        sender_ebd: &[("ÜNB", Baeume::Einer("E_0070"))],
        empfaenger_rolle: "NB",
    },
];

/// Look up the family for an inbound list PID.
#[must_use]
pub fn familie_for(liste: u32) -> Option<&'static ListenFamilie> {
    LISTEN_FAMILIEN.iter().find(|f| f.liste == liste)
}

/// The business key a list this participant distributes runs under: its
/// Listennummer, which the answer echoes in its head's `SG6 RFF+TN` (UTILMD AHB
/// Strom 2.2 Kap. 13.2, Bedingung `[631]`).
#[must_use]
pub fn liste_key(listennummer: &str) -> String {
    format!("mabis-liste:{listennummer}")
}

/// Look up the family a Korrekturliste / Prüfmitteilung PID answers.
#[must_use]
pub fn familie_for_antwort(antwort: u32) -> Option<&'static ListenFamilie> {
    LISTEN_FAMILIEN.iter().find(|f| f.antwort == antwort)
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
    /// `SG5 LOC+Z15` — the MaBiS-Zählpunkt the list is about.
    ///
    /// Muss on both the list and its answer: it is the Summenzeitreihe the
    /// whole exchange reconciles, and an answer that omits it cannot be matched
    /// to one.
    pub mabis_zaehlpunkt: String,
    /// `SG8 RFF+AUU` — the Version der Zeitreihe, a MaBiS Erstellungszeitpunkt.
    pub zeitreihen_version: String,
    /// `SG4 IDE+Z01` DE 7402 — the Listennummer the distributor assigned.
    ///
    /// The answer echoes it in `SG6 RFF+TN` („Es ist die Listennummer aus der
    /// Lieferanten- bzw. Bilanzierungsgebietsclearingliste zu verwenden").
    pub listennummer: String,
}

/// Data recorded when this participant distributes a list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GesendeteListe {
    /// Prüfidentifikator of the list sent.
    pub pruefidentifikator: Pruefidentifikator,
    /// Which list was sent.
    pub typ: ListenTyp,
    /// MP-ID of this participant, the distributor.
    pub sender: MarktpartnerCode,
    /// MP-ID of the reconciling party.
    pub receiver: MarktpartnerCode,
    /// Bilanzierungsmonat the list covers.
    pub billing_period: BillingPeriod,
    /// `SG4 IDE+Z01` — the Listennummer the answer echoes in `SG6 RFF+TN`.
    pub listennummer: String,
    /// `SG5 LOC+Z15` — the MaBiS-Zählpunkt.
    pub mabis_zaehlpunkt: String,
    /// The tree the receiver must answer from: the list's sender role and
    /// Anforderungsart select it, and every code the answer carries is checked
    /// against it.
    pub antwort_ebd: String,
    /// How many Marktlokationen the list named.
    pub positionen: u32,
}

/// What a received Korrekturliste / Prüfmitteilung states.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EingehendeAntwort {
    /// The list was assessed: one `(Marktlokation, Antwortcode)` per disputed
    /// position, empty when nothing was found.
    Korrekturliste {
        /// The disputed positions.
        positionen: Vec<(String, String)>,
    },
    /// The whole list is refused (head `STS+E01`, no `IDE+24`).
    GesamtAblehnung {
        /// The head's Antwortcode.
        antwortcode: String,
    },
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
        /// `SG5 LOC+Z15` — the MaBiS-Zählpunkt the list is about.
        mabis_zaehlpunkt: String,
        /// `SG8 RFF+AUU` — the Version der Zeitreihe.
        zeitreihen_version: String,
        /// `SG4 IDE+Z01` DE 7402 — the Listennummer.
        listennummer: String,
    },
    /// Korrekturliste / Prüfmitteilung dispatched back to the distributor.
    KorrekturGesendet {
        /// Prüfidentifikator of the reply actually sent.
        antwort_pid: Pruefidentifikator,
        /// Number of corrected positions — `0` means the list was confirmed
        /// unchanged, which is still an obligatory reply.
        korrekturen: u32,
    },
    /// The list was refused **entire** — it carried no assessable positions.
    GesamtAblehnungGesendet {
        /// Prüfidentifikator of the reply actually sent. The same PID a
        /// Korrekturliste rides; the cluster is what differs.
        antwort_pid: Pruefidentifikator,
        /// The whole-list Antwortcode out of the tree the sender's role names.
        antwortcode: String,
        /// That tree — `E_0047` or `E_0004` for a Lieferantenclearingliste,
        /// by who distributed it; the family's one tree otherwise.
        codeliste: String,
        /// The Prüfschritt that refused it, for the audit trail.
        pruefschritt: u16,
    },
    /// This participant distributed a list and waits for its answer.
    ListeGesendet(Box<GesendeteListe>),
    /// The receiver's Korrekturliste arrived: the list was assessed.
    KorrekturErhalten {
        /// Prüfidentifikator of the answer received.
        antwort_pid: Pruefidentifikator,
        /// EDIFACT message reference of the answer.
        message_ref: MessageRef,
        /// The disputed positions, each with its code.
        positionen: Vec<(String, String)>,
        /// The tree every code was checked against.
        codeliste: String,
    },
    /// The receiver refused the whole list and a new one is owed.
    GesamtAblehnungErhalten {
        /// Prüfidentifikator of the answer received.
        antwort_pid: Pruefidentifikator,
        /// EDIFACT message reference of the answer.
        message_ref: MessageRef,
        /// The whole-list Antwortcode.
        antwortcode: String,
        /// The tree it was checked against.
        codeliste: String,
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
            Self::GesamtAblehnungGesendet { .. } => "MabisGesamtAblehnungGesendet",
            Self::ListeGesendet(_) => "MabisListeGesendet",
            Self::KorrekturErhalten { .. } => "MabisKorrekturErhalten",
            Self::GesamtAblehnungErhalten { .. } => "MabisGesamtAblehnungErhalten",
            Self::ValidationFailed { .. } => "MabisListenabgleichValidationFailed",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::ListeErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::KorrekturGesendet { antwort_pid, .. }
            | Self::GesamtAblehnungGesendet { antwort_pid, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(antwort_pid.as_u32()),
                message_ref: None,
            }),
            Self::ListeGesendet(liste) => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(liste.pruefidentifikator.as_u32()),
                message_ref: None,
            }),
            Self::KorrekturErhalten {
                antwort_pid,
                message_ref,
                ..
            }
            | Self::GesamtAblehnungErhalten {
                antwort_pid,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(antwort_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::ValidationFailed { .. } => None,
        }
    }

    /// The whole-list refusal states its code and the tree it was drawn from.
    /// One answer PID is drawn from a different tree per sender role, so the
    /// tree is the one the walk ran, not one derived from the PID. A
    /// Korrekturliste carries its codes per position, not as one answer.
    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::GesamtAblehnungGesendet {
                antwortcode,
                codeliste,
                ..
            }
            | Self::GesamtAblehnungErhalten {
                antwortcode,
                codeliste,
                ..
            } => Some(Antwortcode::new(
                antwortcode.clone(),
                Some(codeliste.clone()),
            )),
            _ => None,
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
    /// The list was refused entire and a new one is owed (terminal).
    ///
    /// Distinct from [`Self::Abgeglichen`] with zero corrections, which says the
    /// opposite: that the list was assessed and agreed with.
    Abgelehnt {
        /// Which list was refused.
        typ: ListenTyp,
        /// The whole-list Antwortcode sent.
        antwortcode: String,
    },
    /// This participant distributed a list; its Korrekturliste is awaited.
    ListeGesendet(Box<GesendeteListe>),
    /// The receiver's Korrekturliste arrived (terminal).
    Korrigiert {
        /// Which list was answered.
        typ: ListenTyp,
        /// Number of disputed positions.
        korrekturen: u32,
    },
    /// The receiver refused the whole list (terminal); a new one is owed.
    ListeAbgelehnt {
        /// Which list was refused.
        typ: ListenTyp,
        /// The whole-list Antwortcode received.
        antwortcode: String,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Validation error summary.
        reason: String,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for ListenabgleichState {
    /// Occupied while a Korrekturliste is owed — by this participant or to
    /// it; a reconciled, refused or invalid list is settled.
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::ListeErhalten(_) | Self::ListeGesendet(_))
    }
}

impl ListenabgleichState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::ListeErhalten(_) => "ListeErhalten",
            Self::Abgeglichen { .. } => "Abgeglichen",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::ListeGesendet(_) => "ListeGesendet",
            Self::Korrigiert { .. } => "Korrigiert",
            Self::ListeAbgelehnt { .. } => "ListeAbgelehnt",
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
        /// `SG5 LOC+Z15` — the MaBiS-Zählpunkt the list is about.
        mabis_zaehlpunkt: String,
        /// `SG8 RFF+AUU` — the Version der Zeitreihe.
        zeitreihen_version: String,
        /// `SG4 IDE+Z01` DE 7402 — the Listennummer.
        listennummer: String,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
    /// Send the Korrekturliste / Prüfmitteilung back to the distributor.
    SendKorrektur {
        /// The market role that distributed the list (`"NB"`, `"ÜNB"`).
        ///
        /// With [`Self::SendKorrektur::anforderungsart`] it decides which
        /// Entscheidungsbaum the corrections are drawn from.
        sender_rolle: String,
        /// How the list was ordered; required wherever EBD 4.3 publishes one
        /// tree per Anforderungsart (every list but the DZÜ-Liste).
        anforderungsart: Option<Anforderungsart>,
        /// The disputed positions, one per Marktlokation. An **empty** vector
        /// is valid and still sends a reply — silence would read as acceptance
        /// of whatever the sender filed.
        positionen: Vec<Korrekturposition>,
    },
    /// Refuse the **whole list** and demand a new one.
    ///
    /// The cluster that carries no positions: the Abonnement was never ordered,
    /// the version is not admitted, the Zeitraum is implausible, the MaBiS-ZP
    /// does not match, or the list arrived outside the Clearingphase DZÜ. Each
    /// fact is `Option<bool>` because „the caller does not know" is a third
    /// answer, and the tree escalates on it rather than guessing — a wrong
    /// guess here refuses a list the receiver could have reconciled.
    ///
    /// The walk decides, not the command name: a caller that asks to refuse a
    /// list whose whole-list Prüfschritte all pass is rejected, because the
    /// answer it owes is a Korrekturliste.
    SendGesamtAblehnung {
        /// The market role that distributed the list (`"NB"`, `"ÜNB"`), which
        /// selects the tree exactly as it does for a Korrekturliste.
        sender_rolle: String,
        /// How the list was ordered — the second half of the tree selection.
        anforderungsart: Option<Anforderungsart>,
        /// „Wurde das Abonnement bestellt?"
        abonnement_bestellt: Option<bool>,
        /// „Ist der Zeitraum plausibel?"
        zeitraum_plausibel: Option<bool>,
        /// „Entspricht der MaBiS-ZP dem angefragten MaBiS-ZP?"
        mabis_zp_passt: Option<bool>,
        /// „Ist die Version zugelassen?"
        version_zugelassen: Option<bool>,
        /// „Liegt der Eingang innerhalb der Clearingphase DZÜ?" (`E_0070`).
        innerhalb_clearingphase: Option<bool>,
    },
    /// Distribute a list (55065 / 55201) and wait for its Korrekturliste.
    SendListe {
        /// Prüfidentifikator of the list.
        pid: Pruefidentifikator,
        /// The role this participant distributes it as (`"NB"`).
        sender_rolle: String,
        /// How the receiver ordered it; selects the tree it answers from.
        anforderungsart: Option<Anforderungsart>,
        /// `SG1 RFF+AAV` — the `BGM` DE 1004 of the ORDERS that ordered it.
        anfrage_referenz: String,
        /// MP-ID of this participant.
        sender: MarktpartnerCode,
        /// MP-ID of the receiver.
        receiver: MarktpartnerCode,
        /// Bilanzierungsmonat, `YYYY-MM`.
        billing_period: BillingPeriod,
        /// `SG4 IDE+Z01` — the Listennummer.
        listennummer: String,
        /// `SG5 LOC+Z15` — the MaBiS-Zählpunkt.
        mabis_zaehlpunkt: String,
        /// `SG8 RFF+AUU` — the Version der Zeitreihe the list belongs to.
        zeitreihen_version: String,
        /// The Marktlokationen the list names, at least one: UTILMD AHB Strom
        /// 2.2 Kap. 13.2 Bedingung `[523]` asks for an `IDE+24` „Mindestens
        /// einmal für jede Marktlokation bzw. Tranche" of the
        /// Summenzeitreihe.
        positionen: Vec<mako_engine::payload::ListenMitglied>,
    },
    /// The receiver's Korrekturliste / whole-list refusal arrived.
    ReceiveKorrektur {
        /// Prüfidentifikator of the answer (55066 / 55202).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `STS+E01` DE 1131 as written on the wire, when present.
        codeliste: Option<String>,
        /// What the answer states.
        antwort: EingehendeAntwort,
    },
}

impl CommandPayload for ListenabgleichCommand {}

/// Which tree answers this list, and under which Prüfidentifikator.
///
/// **The EBD follows from who *sent* the list, not from the answer PID.** 55066
/// is answered out of `E_0047` when the NB distributed the list and out of
/// `E_0004` when the ÜNB did, and their codes carry different numbers — so a
/// sender role the family does not publish is refused rather than defaulted.
/// Both reply legs resolve it the same way; a second spelling would let the
/// Korrekturliste and the whole-list refusal answer out of different trees.
fn answer_shape<'a>(
    data: &ListenabgleichData,
    sender_rolle: &str,
    art: Option<Anforderungsart>,
) -> Result<(&'a ListenFamilie, &'static str, Pruefidentifikator), WorkflowError> {
    let familie = familie_for(data.pruefidentifikator.as_u32()).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "no family for recorded list {}",
            data.pruefidentifikator
        ))
    })?;
    let antwort_pid = Pruefidentifikator::new(familie.antwort).map_err(|e| {
        WorkflowError::rejected(format!("invalid Antwort PID {}: {e}", familie.antwort))
    })?;
    let ebd = tree_for(familie, sender_rolle, art)?;
    Ok((familie, ebd, antwort_pid))
}

/// The tree `sender_rolle`'s list ordered as `art` is answered from, or the
/// refusal naming what is missing.
fn tree_for(
    familie: &ListenFamilie,
    sender_rolle: &str,
    art: Option<Anforderungsart>,
) -> Result<&'static str, WorkflowError> {
    let baeume = familie.baeume(sender_rolle).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "{sender_rolle} verteilt die Liste {} nicht; zulässig: {:?}",
            familie.liste,
            familie.sender_rollen().collect::<Vec<_>>()
        ))
    })?;
    baeume.waehle(art).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "die Liste {} von {sender_rolle} wird je Anforderungsart aus einem eigenen \
             Entscheidungsbaum beantwortet ({:?}); die Anforderungsart \
             (einzelanforderung / abonnement) ist anzugeben",
            familie.liste,
            baeume.alle()
        ))
    })
}

/// Check one received code against the tree it must come from and the
/// cluster its place on the wire states.
fn pruefe_eingangscode(
    ebd: &str,
    code: &str,
    erwartet: mako_pruefung::codes::Cluster,
) -> Result<(), WorkflowError> {
    let eintrag = mako_pruefung::mabis::lookup(ebd, code).ok_or_else(|| {
        WorkflowError::rejected(format!("{ebd} veröffentlicht keinen Antwortcode {code}"))
    })?;
    if eintrag.cluster != erwartet {
        return Err(WorkflowError::rejected(format!(
            "{ebd} {code} gehört zum Cluster {:?}, nicht {erwartet:?}",
            eintrag.cluster
        )));
    }
    Ok(())
}

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
                mabis_zaehlpunkt,
                zeitreihen_version,
                listennummer,
                sender,
                receiver,
                billing_period,
                message_ref,
            } => ListenabgleichState::ListeErhalten(Box::new(ListenabgleichData {
                mabis_zaehlpunkt: mabis_zaehlpunkt.clone(),
                zeitreihen_version: zeitreihen_version.clone(),
                listennummer: listennummer.clone(),
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

            ListenabgleichEvent::GesamtAblehnungGesendet { antwortcode, .. } => match state {
                ListenabgleichState::ListeErhalten(d) => ListenabgleichState::Abgelehnt {
                    typ: d.typ,
                    antwortcode: antwortcode.clone(),
                },
                other => other,
            },

            ListenabgleichEvent::ListeGesendet(liste) => {
                ListenabgleichState::ListeGesendet(liste.clone())
            }

            ListenabgleichEvent::KorrekturErhalten { positionen, .. } => match state {
                ListenabgleichState::ListeGesendet(d) => ListenabgleichState::Korrigiert {
                    typ: d.typ,
                    korrekturen: u32::try_from(positionen.len()).unwrap_or(u32::MAX),
                },
                other => other,
            },

            ListenabgleichEvent::GesamtAblehnungErhalten { antwortcode, .. } => match state {
                ListenabgleichState::ListeGesendet(d) => ListenabgleichState::ListeAbgelehnt {
                    typ: d.typ,
                    antwortcode: antwortcode.clone(),
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
                mabis_zaehlpunkt,
                zeitreihen_version,
                listennummer,
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
                    mabis_zaehlpunkt,
                    zeitreihen_version,
                    listennummer,
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
                anforderungsart,
                positionen,
            } => {
                let ListenabgleichState::ListeErhalten(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendKorrektur requires state ListeErhalten, got {}",
                        state.label()
                    )));
                };

                let (familie, ebd, antwort_pid) =
                    answer_shape(data, &sender_rolle, anforderungsart)?;

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
                // The answer travels back the way the list came, so the parties
                // swap. `antwort_codeliste` is the DE 1131 key the renderer
                // reads; the per-position codes ride `positionen`, which the
                // renderer refuses rather than flattening — see below.
                let outbox = PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": familie.antwort,
                        "sender": data.receiver.as_str(),
                        "receiver": data.sender.as_str(),
                        "antwort_codeliste": ebd,
                        // The head of the `IDE+Z01` list: which
                        // Summenzeitreihe, which version, and the Listennummer
                        // the answer echoes in `SG6 RFF+TN`.
                        "mabis_zaehlpunkt": data.mabis_zaehlpunkt,
                        "zeitreihen_version": data.zeitreihen_version,
                        "listennummer": format!("{}-K", data.listennummer),
                        "referenz_listennummer": data.listennummer,
                        "positionen": eintraege
                            .iter()
                            .map(|(malo, code)| serde_json::json!({
                                "malo": malo,
                                "antwort_code": code.code,
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

            ListenabgleichCommand::SendGesamtAblehnung {
                sender_rolle,
                anforderungsart,
                abonnement_bestellt,
                zeitraum_plausibel,
                mabis_zp_passt,
                version_zugelassen,
                innerhalb_clearingphase,
            } => {
                let ListenabgleichState::ListeErhalten(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendGesamtAblehnung requires state ListeErhalten, got {}",
                        state.label()
                    )));
                };
                let (familie, ebd, antwort_pid) =
                    answer_shape(data, &sender_rolle, anforderungsart)?;

                // The walk decides. Passing **no** positions is what makes this
                // a whole-list question: `pruefe_liste` runs the Gesamt-
                // Prüfschritte to completion before it would look at any.
                let pruefung = mako_pruefung::mabis::ListenPruefung {
                    abonnement_bestellt,
                    zeitraum_plausibel,
                    mabis_zp_passt,
                    version_zugelassen,
                    innerhalb_clearingphase,
                    positionen: &[],
                };
                let antwort = match mako_pruefung::mabis::pruefe_liste(ebd, &pruefung) {
                    mako_pruefung::mabis::ListenEntscheidung::GesamtAblehnung(a) => *a,
                    // Every whole-list Prüfschritt passed, so the list *is*
                    // assessable and what it is owed is a Korrekturliste —
                    // refusing it entire would discard positions the receiver
                    // could reconcile.
                    mako_pruefung::mabis::ListenEntscheidung::Korrekturliste(_) => {
                        return Err(WorkflowError::rejected(format!(
                            "{ebd} refuses no whole-list Prüfschritt on these facts — the                              Liste {} is assessable and owes a Korrekturliste",
                            familie.liste
                        )));
                    }
                    mako_pruefung::mabis::ListenEntscheidung::Eskalation {
                        grund,
                        pruefschritt,
                    } => {
                        return Err(WorkflowError::rejected(format!(
                            "{ebd} Prüfschritt {pruefschritt} is unanswered: {grund}"
                        )));
                    }
                };

                let outbox = PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid": familie.antwort,
                        "sender": data.receiver.as_str(),
                        "receiver": data.sender.as_str(),
                        "antwort_code": antwort.code,
                        "antwort_codeliste": ebd,
                        "mabis_zaehlpunkt": data.mabis_zaehlpunkt,
                        "zeitreihen_version": data.zeitreihen_version,
                        "listennummer": format!("{}-A", data.listennummer),
                        "referenz_listennummer": data.listennummer,
                        // No positions: an AblehnungDerGesamtenListe names
                        // no Marktlokation.
                        "positionen": [],
                        "billing_period": data.billing_period.as_str(),
                    }),
                );

                Ok(WorkflowOutput {
                    events: vec![ListenabgleichEvent::GesamtAblehnungGesendet {
                        antwort_pid,
                        antwortcode: antwort.code.clone(),
                        codeliste: ebd.to_owned(),
                        pruefschritt: antwort.pruefschritt,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            ListenabgleichCommand::SendListe {
                pid,
                sender_rolle,
                anforderungsart,
                anfrage_referenz,
                sender,
                receiver,
                billing_period,
                listennummer,
                mabis_zaehlpunkt,
                zeitreihen_version,
                positionen,
            } => {
                if !matches!(state, ListenabgleichState::New) {
                    return Err(WorkflowError::rejected(format!(
                        "SendListe opens a process; this one is {}",
                        state.label()
                    )));
                }
                let familie = familie_for(pid.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {pid} is not a MaBiS list; expected one of {:?}",
                        LISTEN_FAMILIEN.iter().map(|f| f.liste).collect::<Vec<_>>()
                    ))
                })?;
                let ebd = tree_for(familie, &sender_rolle, anforderungsart)?;
                for (feld, wert) in [
                    ("anfrage_referenz", &anfrage_referenz),
                    ("listennummer", &listennummer),
                    ("mabis_zaehlpunkt", &mabis_zaehlpunkt),
                    ("zeitreihen_version", &zeitreihen_version),
                ] {
                    if wert.trim().is_empty() {
                        return Err(WorkflowError::rejected(format!(
                            "SendListe {pid}: {feld} ist ein Muss-Bestandteil der Liste"
                        )));
                    }
                }
                if positionen.is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "SendListe {pid}: eine Liste nennt mindestens eine Marktlokation \
                         (UTILMD AHB Strom 2.2 Kap. 13.2, Bedingung [523])"
                    )));
                }
                let liste = GesendeteListe {
                    pruefidentifikator: pid,
                    typ: familie.typ,
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    billing_period: billing_period.clone(),
                    listennummer: listennummer.clone(),
                    mabis_zaehlpunkt: mabis_zaehlpunkt.clone(),
                    antwort_ebd: ebd.to_owned(),
                    positionen: u32::try_from(positionen.len()).unwrap_or(u32::MAX),
                };
                let outbox = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "pid": familie.liste,
                        "sender": sender.as_str(),
                        "receiver": receiver.as_str(),
                        "anfrage_referenz": anfrage_referenz,
                        "mabis_zaehlpunkt": mabis_zaehlpunkt,
                        "zeitreihen_version": zeitreihen_version,
                        "listennummer": listennummer,
                        "billing_period": billing_period.as_str(),
                        "listenmitglieder": positionen,
                    }),
                );
                Ok(WorkflowOutput {
                    events: vec![ListenabgleichEvent::ListeGesendet(Box::new(liste))],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            ListenabgleichCommand::ReceiveKorrektur {
                pid,
                message_ref,
                codeliste,
                antwort,
            } => {
                let ListenabgleichState::ListeGesendet(liste) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "ReceiveKorrektur requires state ListeGesendet, got {}",
                        state.label()
                    )));
                };
                let familie = familie_for(liste.pruefidentifikator.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "no family for sent list {}",
                        liste.pruefidentifikator
                    ))
                })?;
                if pid.as_u32() != familie.antwort {
                    return Err(WorkflowError::rejected(format!(
                        "the list {} is answered by {}, not {pid}",
                        familie.liste, familie.antwort
                    )));
                }
                // The tree is the one the list's sender role and Anforderungsart
                // select. A DE 1131 naming another one states a code from a
                // Codeliste this list is not answered from.
                let ebd = liste.antwort_ebd.as_str();
                if let Some(genannt) = codeliste.as_deref().filter(|c| !c.is_empty())
                    && genannt != ebd
                {
                    return Err(WorkflowError::rejected(format!(
                        "die Liste {} wird aus {ebd} beantwortet, die Antwort nennt {genannt}",
                        familie.liste
                    )));
                }
                match antwort {
                    EingehendeAntwort::Korrekturliste { positionen } => {
                        for (_, code) in &positionen {
                            pruefe_eingangscode(
                                ebd,
                                code,
                                mako_pruefung::codes::Cluster::KorrekturlisteWegenAblehnung,
                            )?;
                        }
                        Ok(vec![ListenabgleichEvent::KorrekturErhalten {
                            antwort_pid: pid,
                            message_ref,
                            positionen,
                            codeliste: ebd.to_owned(),
                        }]
                        .into())
                    }
                    EingehendeAntwort::GesamtAblehnung { antwortcode } => {
                        pruefe_eingangscode(
                            ebd,
                            &antwortcode,
                            mako_pruefung::codes::Cluster::AblehnungDerGesamtenListe,
                        )?;
                        Ok(vec![ListenabgleichEvent::GesamtAblehnungErhalten {
                            antwort_pid: pid,
                            message_ref,
                            antwortcode,
                            codeliste: ebd.to_owned(),
                        }]
                        .into())
                    }
                }
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

    const EINZEL: Option<Anforderungsart> = Some(Anforderungsart::Einzelanforderung);
    const ABO: Option<Anforderungsart> = Some(Anforderungsart::Abonnement);

    fn korrektur(
        rolle: &str,
        anforderungsart: Option<Anforderungsart>,
        positionen: Vec<Korrekturposition>,
    ) -> ListenabgleichCommand {
        ListenabgleichCommand::SendKorrektur {
            sender_rolle: rolle.to_owned(),
            anforderungsart,
            positionen,
        }
    }

    /// The whole-list facts, all answered „ja" unless overridden — the shape of
    /// a list that is assessable.
    fn ablehnung(rolle: &str) -> ListenabgleichCommand {
        ListenabgleichCommand::SendGesamtAblehnung {
            sender_rolle: rolle.to_owned(),
            anforderungsart: EINZEL,
            abonnement_bestellt: Some(true),
            zeitraum_plausibel: Some(true),
            mabis_zp_passt: Some(true),
            version_zugelassen: Some(true),
            innerhalb_clearingphase: Some(true),
        }
    }

    fn receive(pid: u32) -> ListenabgleichCommand {
        ListenabgleichCommand::ReceiveListe {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            mabis_zaehlpunkt: "DE0004096999000000000000000000009".to_owned(),
            zeitreihen_version: "20270115T090000000".to_owned(),
            listennummer: "LST-1".to_owned(),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            billing_period: BillingPeriod::new("2026-07"),
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    /// The state after driving `commands` from `New`.
    fn after(commands: &[ListenabgleichCommand]) -> ListenabgleichState {
        let mut state = ListenabgleichState::default();
        for cmd in commands {
            let out = MabisListenabgleichWorkflow::handle(&state, cmd.clone())
                .expect("the setup commands must succeed");
            state = fold_from(state, &out.events);
        }
        state
    }

    fn fold_from(
        start: ListenabgleichState,
        events: &[ListenabgleichEvent],
    ) -> ListenabgleichState {
        events
            .iter()
            .fold(start, MabisListenabgleichWorkflow::apply)
    }

    fn fold(events: &[ListenabgleichEvent]) -> ListenabgleichState {
        events.iter().fold(ListenabgleichState::default(), |s, e| {
            MabisListenabgleichWorkflow::apply(s, e)
        })
    }

    #[test]
    fn a_whole_list_refusal_states_its_code_and_a_korrekturliste_states_none() {
        assert_eq!(
            ListenabgleichEvent::GesamtAblehnungGesendet {
                antwort_pid: Pruefidentifikator::const_new(55066),
                antwortcode: "A01".to_owned(),
                codeliste: "E_0047".to_owned(),
                pruefschritt: 1,
            }
            .antwort(),
            Some(Antwortcode::new("A01", Some("E_0047")))
        );
        assert_eq!(
            ListenabgleichEvent::KorrekturGesendet {
                antwort_pid: Pruefidentifikator::const_new(55066),
                korrekturen: 0,
            }
            .antwort(),
            None
        );
    }

    #[test]
    fn verlauf_states_the_messages_and_nothing_else() {
        assert_eq!(
            ListenabgleichEvent::KorrekturGesendet {
                antwort_pid: Pruefidentifikator::const_new(55066),
                korrekturen: 0,
            }
            .verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(55066),
                message_ref: None,
            })
        );
        assert_eq!(
            ListenabgleichEvent::ValidationFailed {
                reason: "x".to_owned(),
            }
            .verlauf(),
            None
        );
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
        // The PID overview gives it a Prozessschritt-3 Korrekturliste, so an
        // LF owes 55066 back — it does not belong with the record-only lists.
        assert!(familie_for(55065).is_some());
        assert!(!crate::clearingliste::CLEARINGLISTE_PIDS.contains(&55065));
    }

    #[test]
    fn the_55066_ebd_depends_on_who_sent_the_list_and_how_it_was_ordered() {
        // One PID, two axes, two kinds of order, four disjoint code spaces —
        // answering out of the wrong tree produces a code that means something
        // else there (EBD 4.3 Kap. 7.14.2, 7.15.1, 7.18.2, 7.19.1).
        let f = familie_for(55065).unwrap();
        assert_eq!(f.antwort_ebd("NB", EINZEL), Some("E_0047"));
        assert_eq!(f.antwort_ebd("NB", ABO), Some("E_0049"));
        assert_eq!(f.antwort_ebd("ÜNB", EINZEL), Some("E_0014"));
        assert_eq!(f.antwort_ebd("ÜNB", ABO), Some("E_0004"));
        assert_eq!(f.antwort_ebd("NB", None), None, "two trees, no order kind");
        assert_eq!(
            f.antwort_ebd("BIKO", EINZEL),
            None,
            "never sent by the BIKO"
        );
        // The LF-AACL (Kap. 7.58.1 / 7.59.1) and the BG-CL (7.23.2 / 7.24.1).
        let aacl = familie_for(55201).unwrap();
        assert_eq!(aacl.antwort_ebd("NB", EINZEL), Some("E_0097"));
        assert_eq!(aacl.antwort_ebd("NB", ABO), Some("E_0096"));
        let bg = familie_for(55195).unwrap();
        assert_eq!(bg.antwort_ebd("ÜNB", EINZEL), Some("E_0017"));
        assert_eq!(bg.antwort_ebd("ÜNB", ABO), Some("E_0052"));
        // The DZÜ-Liste has one tree whatever the order (Kap. 7.48.1).
        assert_eq!(
            familie_for(55223).unwrap().antwort_ebd("ÜNB", None),
            Some("E_0070")
        );
    }

    #[test]
    fn every_family_names_a_catalogued_tree_for_every_sender_it_admits() {
        for f in LISTEN_FAMILIEN {
            assert!(!f.sender_ebd.is_empty(), "{} has no sender", f.liste);
            for rolle in f.sender_rollen() {
                for ebd in f.baeume(rolle).expect("declared sender").alle() {
                    assert!(
                        mako_pruefung::mabis::MABIS_TREES
                            .iter()
                            .any(|(id, _)| *id == ebd),
                        "{} → {ebd} is not catalogued",
                        f.liste
                    );
                }
            }
        }
    }

    /// A list delivered under a running Abonnement is answered from the
    /// Abonnement tree: the NB's Lieferantenclearingliste from `E_0049`, whose
    /// „bilanzierungsrel. Daten" Korrekturgrund is `A07` (EBD 4.3 Kap. 7.15.1,
    /// S. 448), not the Einzelanforderung tree `E_0047`.
    #[test]
    fn an_abonnement_list_is_answered_from_e_0049() {
        let state = after(&[receive(55_065)]);
        let out = MabisListenabgleichWorkflow::handle(
            &state,
            korrektur(
                "NB",
                ABO,
                vec![pos(
                    1,
                    mako_pruefung::mabis::Korrekturgrund::DatenFehlerhaft,
                )],
            ),
        )
        .expect("an Abonnement list is answerable");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_0049");
        assert_eq!(
            out.outbox[0].payload["positionen"][0]["antwort_code"],
            "A07"
        );

        let err = MabisListenabgleichWorkflow::handle(&state, korrektur("NB", None, vec![]))
            .expect_err("two trees and no order kind");
        assert!(format!("{err}").contains("Anforderungsart"), "{err}");
    }

    #[test]
    fn a_clean_reconciliation_still_sends_a_reply() {
        // Zero corrections is not silence: the AHB obliges a Prüfmitteilung
        // either way, so `korrekturen: 0` must still put a message on the wire.
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, receive(55223)).unwrap();
        let state = fold(&out.events);
        assert_eq!(state.label(), "ListeErhalten");

        let out = MabisListenabgleichWorkflow::handle(&state, korrektur("ÜNB", None, vec![]))
            .expect("clean reply");
        assert_eq!(out.outbox.len(), 1, "a clean list still owes a reply");
        assert_eq!(out.outbox[0].payload["pid"], 55224);
        assert_eq!(
            out.outbox[0].payload["positionen"].as_array().map(Vec::len),
            Some(0)
        );
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
                EINZEL,
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
            let art = if rolle == "NB" { EINZEL } else { ABO };
            let out = MabisListenabgleichWorkflow::handle(
                &state,
                korrektur(rolle, art, positionen.clone()),
            )
            .expect("both roles distribute 55065");
            (
                out.outbox[0].payload["antwort_codeliste"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                out.outbox[0].payload["positionen"][0]["antwort_code"]
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
                EINZEL,
                vec![pos(
                    1,
                    mako_pruefung::mabis::Korrekturgrund::DatenFehlerhaft,
                )],
            ),
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_0017");
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
            korrektur("NB", EINZEL, vec![]),
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
            mabis_zaehlpunkt: "DE0004096999000000000000000000009".to_owned(),
            zeitreihen_version: "20270115T090000000".to_owned(),
            listennummer: "LST-1".to_owned(),
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

    // ── The whole-list refusal ────────────────────────────────────────────────

    /// A list nobody ordered is refused **entire**, and the answer carries no
    /// positions at all.
    ///
    /// Answering it with an empty Korrekturliste would say „assessed, nothing to
    /// correct" — the opposite statement, on a list the receiver never agreed to
    /// receive. The ÜNB's Lieferantenclearingliste is answered out of `E_0004`,
    /// which is the tree that asks about the Abonnement.
    #[test]
    fn an_unordered_abonnement_refuses_the_whole_list() {
        let state = after(&[receive(55_065)]);
        let out = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendGesamtAblehnung {
                sender_rolle: "ÜNB".to_owned(),
                anforderungsart: ABO,
                abonnement_bestellt: Some(false),
                zeitraum_plausibel: Some(true),
                mabis_zp_passt: Some(true),
                version_zugelassen: Some(true),
                innerhalb_clearingphase: Some(true),
            },
        )
        .expect("an unordered Abonnement refuses the list");

        let payload = &out.outbox[0].payload;
        assert_eq!(payload["positionen"].as_array().map(Vec::len), Some(0));
        // The code must come out of the whole-list cluster. A Korrekturgrund
        // code carried on a message with no positions would name a
        // Marktlokation the message does not contain.
        let code = payload["antwort_code"]
            .as_str()
            .expect("a code was resolved");
        let entry = mako_pruefung::mabis::lookup(
            payload["antwort_codeliste"]
                .as_str()
                .expect("the tree is named"),
            code,
        )
        .expect("the tree publishes the code it answered with");
        assert_eq!(
            entry.cluster,
            mako_pruefung::codes::Cluster::AblehnungDerGesamtenListe,
            "{code} is not a whole-list refusal"
        );

        let next = out
            .events
            .iter()
            .fold(state.clone(), MabisListenabgleichWorkflow::apply);
        assert_eq!(next.label(), "Abgelehnt");
    }

    /// A list every whole-list Prüfschritt passes is **assessable**, and what it
    /// owes is a Korrekturliste. Refusing it entire would discard every position
    /// the receiver did check, so the tree refuses the caller instead.
    #[test]
    fn an_assessable_list_may_not_be_refused_entire() {
        let state = after(&[receive(55_065)]);
        let err = MabisListenabgleichWorkflow::handle(&state, ablehnung("NB"))
            .expect_err("an assessable list owes a Korrekturliste");
        assert!(
            format!("{err}").contains("Korrekturliste"),
            "the refusal must say what the list is owed instead: {err}"
        );
    }

    /// „The caller cannot answer this Prüfschritt" is a third answer, and the
    /// tree escalates on it rather than guessing — a guess refuses a list the
    /// receiver could have reconciled.
    #[test]
    fn an_unanswered_pruefschritt_escalates_rather_than_refusing() {
        let state = after(&[receive(55_065)]);
        let err = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendGesamtAblehnung {
                sender_rolle: "ÜNB".to_owned(),
                anforderungsart: ABO,
                abonnement_bestellt: None,
                zeitraum_plausibel: Some(true),
                mabis_zp_passt: Some(true),
                version_zugelassen: Some(true),
                innerhalb_clearingphase: Some(true),
            },
        )
        .expect_err("an unanswered Prüfschritt is not a refusal");
        assert!(
            format!("{err}").contains("unanswered"),
            "the caller must be told which question is open: {err}"
        );
    }

    /// **A tree only refuses on the questions it asks.** One Prüfidentifikator,
    /// two trees: `E_0004` (ÜNB) asks about the Abonnement, `E_0047` (NB) asks
    /// about Zeitraum, MaBiS-ZP and Version. A caller stating „the Abonnement
    /// was never ordered" on the NB's list is not refusing anything that tree
    /// publishes, so its list stays assessable — which is the answer, not an
    /// oversight, and the reason the facts are not collapsed into one flag.
    #[test]
    fn a_fact_the_tree_does_not_ask_refuses_nothing() {
        let state = after(&[receive(55_065)]);
        let err = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendGesamtAblehnung {
                sender_rolle: "NB".to_owned(),
                anforderungsart: EINZEL,
                abonnement_bestellt: Some(false),
                zeitraum_plausibel: Some(true),
                mabis_zp_passt: Some(true),
                version_zugelassen: Some(true),
                innerhalb_clearingphase: Some(true),
            },
        )
        .expect_err("E_0047 asks no Abonnement question");
        assert!(format!("{err}").contains("Korrekturliste"), "{err}");

        // …and the question that tree *does* ask refuses it.
        let out = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::SendGesamtAblehnung {
                sender_rolle: "NB".to_owned(),
                anforderungsart: EINZEL,
                abonnement_bestellt: None,
                zeitraum_plausibel: Some(true),
                mabis_zp_passt: Some(true),
                version_zugelassen: Some(false),
                innerhalb_clearingphase: Some(true),
            },
        )
        .expect("an inadmissible version refuses the list");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_0047");
        assert_eq!(
            out.outbox[0].payload["positionen"].as_array().map(Vec::len),
            Some(0)
        );
    }

    /// **The refusal names the tree its code is defined in**, so the Verlauf
    /// can read the wording: one Prüfidentifikator, and the tree follows the
    /// sender — `E_0047` for the NB's list, `E_0004` for the ÜNB's.
    #[test]
    fn a_whole_list_refusal_names_the_tree_the_sender_decides() {
        let refuse = |rolle: &str, abonnement, version| {
            let out = MabisListenabgleichWorkflow::handle(
                &after(&[receive(55_065)]),
                ListenabgleichCommand::SendGesamtAblehnung {
                    sender_rolle: rolle.to_owned(),
                    anforderungsart: if rolle == "NB" { EINZEL } else { ABO },
                    abonnement_bestellt: abonnement,
                    zeitraum_plausibel: Some(true),
                    mabis_zp_passt: Some(true),
                    version_zugelassen: version,
                    innerhalb_clearingphase: Some(true),
                },
            )
            .expect("the walk refuses the list");
            out.events[0].antwort().expect("a refusal states its code")
        };
        for (rolle, antwort, tree) in [
            ("NB", refuse("NB", None, Some(false)), "E_0047"),
            ("ÜNB", refuse("ÜNB", Some(false), Some(true)), "E_0004"),
        ] {
            assert_eq!(antwort.codeliste.as_deref(), Some(tree), "{rolle}");
            assert!(
                mako_pruefung::codes::lies(tree, &antwort.code).is_some(),
                "{tree} {} has its wording in the catalogue",
                antwort.code
            );
        }
    }

    /// The whole-list refusal resolves its tree from the **sender's role**, the
    /// same way a Korrekturliste does: one Prüfidentifikator, two disjoint code
    /// spaces, and a role the family does not publish is refused.
    #[test]
    fn the_refusal_resolves_its_tree_from_the_sender_role() {
        let state = after(&[receive(55_065)]);
        let err = MabisListenabgleichWorkflow::handle(&state, ablehnung("MSB"))
            .expect_err("an MSB distributes no Clearingliste");
        assert!(format!("{err}").contains("MSB"), "{err}");
    }

    // ── The distributor's leg ─────────────────────────────────────────────────

    fn sende(pid: u32, art: Option<Anforderungsart>) -> ListenabgleichCommand {
        ListenabgleichCommand::SendListe {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender_rolle: "NB".to_owned(),
            anforderungsart: art,
            anfrage_referenz: "ORD-17202-1".to_owned(),
            sender: mp("9900987654321"),
            receiver: mp("9900123456789"),
            billing_period: BillingPeriod::new("2026-07"),
            listennummer: "LCL-1".to_owned(),
            mabis_zaehlpunkt: "DE0004096999000000000000000000009".to_owned(),
            zeitreihen_version: "20260801090000+00".to_owned(),
            positionen: vec![mako_engine::payload::ListenMitglied {
                malo: "51238696012".to_owned(),
                bilanzierungsbeginn: "202606302200".to_owned(),
                ..Default::default()
            }],
        }
    }

    fn korrekturliste(pid: u32, codes: &[&str], codeliste: Option<&str>) -> ListenabgleichCommand {
        ListenabgleichCommand::ReceiveKorrektur {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            message_ref: MessageRef::new("KORR-1"),
            codeliste: codeliste.map(str::to_owned),
            antwort: EingehendeAntwort::Korrekturliste {
                positionen: codes
                    .iter()
                    .map(|c| ("51238696012".to_owned(), (*c).to_owned()))
                    .collect(),
            },
        }
    }

    /// The NB distributes a 55065 and the process waits for the Korrekturliste:
    /// the list goes to the Lieferant, and the process is occupied until the
    /// answer arrives.
    #[test]
    fn a_sent_lieferantenclearingliste_waits_for_its_korrekturliste() {
        use mako_engine::workflow::OccupiesBusinessKey as _;
        let out =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, sende(55_065, EINZEL))
                .expect("the NB distributes 55065");
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].payload["pid"], 55_065);
        assert_eq!(out.outbox[0].recipient.as_ref(), "9900123456789");
        assert_eq!(out.outbox[0].payload["anfrage_referenz"], "ORD-17202-1");
        let state = fold(&out.events);
        assert_eq!(state.label(), "ListeGesendet");
        assert!(state.occupies_business_key());
        let ListenabgleichState::ListeGesendet(liste) = &state else {
            unreachable!()
        };
        assert_eq!(liste.antwort_ebd, "E_0047");
    }

    /// A 55066 closes the Vorgang, its codes checked against `E_0047`; a code
    /// that tree does not publish, or a DE 1131 naming another tree, is refused.
    #[test]
    fn a_55066_closes_the_vorgang_and_a_code_outside_e_0047_is_refused() {
        let state = after(&[sende(55_065, EINZEL)]);

        let out = MabisListenabgleichWorkflow::handle(
            &state,
            korrekturliste(55_066, &["A07"], Some("E_0047")),
        )
        .expect("A07 is E_0047's Daten-Korrekturgrund");
        let next = fold_from(state.clone(), &out.events);
        match next {
            ListenabgleichState::Korrigiert { typ, korrekturen } => {
                assert_eq!(typ, ListenTyp::Lieferantenclearingliste);
                assert_eq!(korrekturen, 1);
            }
            other => panic!("expected Korrigiert, got {}", other.label()),
        }

        // `A08` exists in no list tree; `A01` of E_0047 is a whole-list code
        // and cannot stand on a position.
        for code in ["A08", "A01"] {
            let err =
                MabisListenabgleichWorkflow::handle(&state, korrekturliste(55_066, &[code], None))
                    .expect_err("a code outside E_0047's Korrekturliste cluster");
            assert!(format!("{err}").contains("E_0047"), "{code}: {err}");
        }
        // The ÜNB's Abonnement tree is not this list's.
        let err = MabisListenabgleichWorkflow::handle(
            &state,
            korrekturliste(55_066, &["A06"], Some("E_0004")),
        )
        .expect_err("E_0004 does not answer the NB's Einzelanforderung");
        assert!(format!("{err}").contains("E_0047"), "{err}");
        // …and a 55202 does not answer a 55065.
        assert!(
            MabisListenabgleichWorkflow::handle(&state, korrekturliste(55_202, &[], None)).is_err()
        );
    }

    /// An empty Korrekturliste is the ordinary „assessed, nothing to correct"
    /// answer, and a whole-list refusal states its code and tree.
    #[test]
    fn an_empty_korrekturliste_and_a_whole_list_refusal_both_close_the_vorgang() {
        let state = after(&[sende(55_201, ABO)]);
        let out = MabisListenabgleichWorkflow::handle(&state, korrekturliste(55_202, &[], None))
            .expect("an empty Korrekturliste");
        assert!(matches!(
            fold_from(state.clone(), &out.events),
            ListenabgleichState::Korrigiert { korrekturen: 0, .. }
        ));

        let out = MabisListenabgleichWorkflow::handle(
            &state,
            ListenabgleichCommand::ReceiveKorrektur {
                pid: Pruefidentifikator::const_new(55_202),
                message_ref: MessageRef::new("KORR-2"),
                codeliste: Some("E_0096".to_owned()),
                antwort: EingehendeAntwort::GesamtAblehnung {
                    antwortcode: "A01".to_owned(),
                },
            },
        )
        .expect("E_0096 A01 refuses the whole list");
        assert_eq!(
            out.events[0].antwort(),
            Some(Antwortcode::new("A01", Some("E_0096")))
        );
        assert_eq!(fold_from(state, &out.events).label(), "ListeAbgelehnt");
    }

    /// The ÜNB's lists are not this participant's to send, and a list needs
    /// its Anfrage reference.
    #[test]
    fn a_list_the_nb_does_not_distribute_is_refused() {
        let err =
            MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, sende(55_195, EINZEL))
                .expect_err("the ÜNB distributes 55195");
        assert!(format!("{err}").contains("NB"), "{err}");
        let mut ohne = sende(55_065, EINZEL);
        if let ListenabgleichCommand::SendListe {
            anfrage_referenz, ..
        } = &mut ohne
        {
            anfrage_referenz.clear();
        }
        assert!(MabisListenabgleichWorkflow::handle(&ListenabgleichState::New, ohne).is_err());
    }
}
