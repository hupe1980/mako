//! GPKE **Lieferende von NB an LF** — the Netzbetreiber's side.
//!
//! The NB ends a Lieferant's Zuordnung of its own accord: it announces the
//! Beendigung with a UTILMD **55007** (Nr. 1), the LF agrees (**55008**) or
//! refuses (**55009**) by 05:00 Uhr des 1. WT nach dem ÜT (Nr. 2), and on
//! silence the NB ends the Zuordnung anyway with a second 55007, `BGM+Z90`
//! „Beendigung der Zuordnung zur Lokation" (Nr. 3). BK6-24-174 GPKE Teil 2
//! Kap. 2.5.2.2, S. 60–62. [`crate::lf_abmeldung`] is the LF's side.
//!
//! # Prozessschritte
//!
//! | Nr. | Message | Frist |
//! |---|---|---|
//! | 1 | 55007 `BGM+E02` Ankündigung der Beendigung der Zuordnung | the NB's |
//! | 2 | 55008 Zustimmung / 55009 Ablehnung, LF → NB | 05:00 Uhr des 1. WT nach dem ÜT von Nr. 1 |
//! | 3 | 55007 `BGM+Z90` Beendigung aufgrund fehlender Antwort | frühester ÜZ 05:00, spätester 06:00 Uhr des 1. WT nach dem ÜT von Nr. 1 |
//!
//! „Im Fall der Zustimmung des LF beendet der NB die Zuordnung des LF zur
//! Marktlokation bzw. Tranche unverzüglich zum Zuordnungsende" (Nr. 2), and
//! „Antwortet der LF in Prozessschritt 2 nicht fristgerecht, beendet der NB die
//! Zuordnung … unverzüglich zum Zuordnungsende" (Nr. 3). On an Ablehnung „Der
//! LF bleibt der Marktlokation bzw. Tranche zugeordnet" (Nachbedingung im
//! Fehlerfall, S. 58). Either ending leaves a `ProcessCompleted` that marktd
//! ends the supply on and edmd orders the Schlussablesung on.
//!
//! # Two windows, one step
//!
//! The Nr. 3 window opens where the LF's answer window closes, so both are the
//! one catalogued step [`mako_fristen::meldung::GPKE_LIEFERENDE_OHNE_ANTWORT`]:
//! [`LF_ANTWORT_WINDOW_LABEL`] fires at its earliest bound and sends the
//! Beendigung, [`BEENDIGUNG_SPAETESTENS_LABEL`] at its latest and sends it if
//! the first did not. An answer that arrives after the LF's window closed is
//! „nicht fristgerecht" and leads to Nr. 3 as silence does.
//!
//! # Transaktionsgründe and the Stilllegung
//!
//! The 55007 column admits `Z33` Auszug wegen Stilllegung, `ZQ7` Abmeldung wg.
//! fehlender Zuordnungsermächtigung and `ZT0` the same aufgrund Änderung ZRT
//! (UTILMD AHB Strom 2.2 Kap. 8.10). `Z33` makes `SG4 DTM+206` the
//! Geräteausbauzeitpunkt Muss (Bedingung `[79]`) — „den Geräteausbauzeitpunkt
//! der Messlokation, deren Stilllegung die Stilllegung der Marktlokation zur
//! Folge hat" (Nr. 1, S. 61).
//!
//! A Stilllegung owes three more messages, each „unverzüglich nach dem ÜZ von
//! Nr. 2, sofern es sich um eine Zustimmung handelt, bzw. nach dem ÜZ von
//! Nr. 3, jedoch spätester ÜZ ist 07:00 Uhr des 1. WT nach dem ÜT von Nr. 1"
//! (S. 63 f.): 55038 `ZH1` to the LFZ (Nr. 8), 55611 `ZC8` to the MSB (Nr. 11)
//! and 55611 `ZH1` to the MSBZ (Nr. 13). The UC owes them „bei einer
//! Stilllegung" alone (S. 57), and the LFZ and MSBZ „ggf.". This process sends
//! them with the ending that owes them, so a Meldepflicht is discharged in the
//! same write that creates it; the 07:00 deadlines
//! ([`MELDEPFLICHT_LFZ_LABEL`], [`MELDEPFLICHT_MSB_LABEL`]) fire into a
//! breach only while the Vorgang has not ended.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::payload::UtilmdPayload;
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use time::OffsetDateTime;

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-nb-lieferende";

/// The Ankündigung (Nr. 1) and the Beendigung aufgrund fehlender Antwort
/// (Nr. 3) — one Prüfidentifikator, told apart by `BGM` (Anwendungsübersicht
/// der Prüfidentifikatoren 4.0 Lfd. Nr. 20940).
pub const ANKUENDIGUNG_PID: u32 = 55_007;

/// The LF's Zustimmung.
pub const BESTAETIGUNG_PID: u32 = 55_008;

/// The LF's Ablehnung.
pub const ABLEHNUNG_PID: u32 = 55_009;

/// The answers that resume the NB's process (LF → NB).
pub const ANTWORT_PIDS: &[u32] = &[BESTAETIGUNG_PID, ABLEHNUNG_PID];

/// `BGM` DE 1001 of Nr. 1 — Bedingung `[712]`.
pub const BGM_ANKUENDIGUNG: &str = "E02";

/// `BGM` DE 1001 of Nr. 3 „Beendigung der Zuordnung zur Lokation" — Bedingung
/// `[713]`; it makes `SG6 RFF+TN` Muss (`[60]`).
pub const BGM_BEENDIGUNG: &str = "Z90";

/// `SG4 STS+7` Transaktionsgründe of the 55007; [`STILLLEGUNG`] needs its
/// [`Stilllegung`].
pub const GRUENDE: &[&str] = &["Z33", "ZQ7", "ZT0"];

/// `Z33` „Auszug wegen Stilllegung".
pub const STILLLEGUNG: &str = "Z33";

/// `SG4 STS+7` Transaktionsgrundergänzungen of the 55007: `ZW3` erzeugende,
/// `ZW4` verbrauchende, `ZW5` Tranche, `ZAP` ruhende Marktlokation.
pub const ERGAENZUNGEN: &[&str] = &["ZW3", "ZW4", "ZW5", "ZAP"];

/// Deadline label for the LF's answer window — 05:00 Uhr des 1. WT nach dem ÜT
/// von Nr. 1, the earliest bound of Nr. 3. Its expiry sends the Beendigung.
pub const LF_ANTWORT_WINDOW_LABEL: &str = "gpke-nb-lieferende-lf-antwort";

/// Deadline label for the latest bound of Nr. 3 — 06:00 Uhr des 1. WT nach dem
/// ÜT von Nr. 1. Sends the Beendigung where [`LF_ANTWORT_WINDOW_LABEL`] did not.
pub const BEENDIGUNG_SPAETESTENS_LABEL: &str = "gpke-nb-lieferende-beendigung-spaetestens";

/// Deadline label for the 55038 to the LFZ on a Stilllegung (Nr. 8) — 07:00 Uhr
/// des 1. WT nach dem ÜT von Nr. 1.
pub const MELDEPFLICHT_LFZ_LABEL: &str = "gpke-nb-lieferende-meldepflicht-55038";

/// Deadline label for the 55611 to the MSB and the MSBZ on a Stilllegung
/// (Nr. 11 / 13) — 07:00 Uhr des 1. WT nach dem ÜT von Nr. 1.
pub const MELDEPFLICHT_MSB_LABEL: &str = "gpke-nb-lieferende-meldepflicht-55611";

/// The deadline label of the Meldepflicht `pid` this process owes on a
/// Stilllegung, if it owes it.
#[must_use]
pub fn meldepflicht_label(pid: u32) -> Option<&'static str> {
    match pid {
        crate::zuordnungsmeldung::AUFHEBUNG_PID => Some(MELDEPFLICHT_LFZ_LABEL),
        crate::zuordnungsmeldung::MSB_BEENDIGUNG_PID => Some(MELDEPFLICHT_MSB_LABEL),
        _ => None,
    }
}

/// Correlation key under which an answer finds the NB's process: its `SG6
/// RFF+TN` names the Ankündigung's `SG4 IDE+24`, and neither 55008 nor 55009
/// carries an `SG5` Lokation (UTILMD AHB Strom 2.2 Kap. 8.10).
#[must_use]
pub fn nb_vorgang_key(vorgangsnummer: &str) -> String {
    format!("nb-lieferende-vorgang:{vorgangsnummer}")
}

/// `SG5 LOC` DE 3227 for an Ergänzung: `Z21` Tranche (`[481]`), `Z22` ruhende
/// Marktlokation (`[96]`), `Z16` otherwise (`[479]`/`[480]`).
#[must_use]
pub fn lokationstyp(ergaenzung: &str) -> &'static str {
    match ergaenzung {
        "ZW5" => "Z21",
        "ZAP" => "Z22",
        _ => "Z16",
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events of the NB's Lieferende.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NbLieferendeEvent {
    /// Nr. 1 queued for the LF.
    Angekuendigt {
        /// The Marktlokation or the MaLo-ID of the Tranche.
        location_id: MaLo,
        /// The NB.
        sender: MarktpartnerCode,
        /// The LF whose Zuordnung ends.
        receiver: MarktpartnerCode,
        /// `SG4 DTM+93` Zuordnungsende, `YYYYMMDD`.
        zuordnungsende: String,
        /// `SG4 STS+7` Transaktionsgrund.
        transaktionsgrund: String,
        /// `SG4 STS+7` Ergänzung.
        transaktionsgrund_ergaenzung: String,
        /// `SG4 IDE+24` of Nr. 1.
        vorgangsnummer: String,
        /// `SG4 IDE+24` drawn for Nr. 3.
        beendigung_vorgangsnummer: String,
        /// The instant the LF's answer window closes.
        #[serde(with = "time::serde::rfc3339")]
        antwortfrist: OffsetDateTime,
        /// What a Stilllegung (`Z33`) adds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stilllegung: Option<Box<Stilllegung>>,
    },
    /// The LF answered (55008 / 55009).
    AntwortErhalten {
        /// 55008 or 55009.
        response_pid: Pruefidentifikator,
        /// `true` on 55008.
        zustimmung: bool,
        /// `SG4 STS+E01` with its Codeliste.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<Antwortcode>,
        /// The Begründung of an Ablehnung.
        reason: Option<String>,
        /// Message reference of the answer.
        response_ref: MessageRef,
        /// `false` when it arrived after the window closed — Nr. 3 follows.
        fristgerecht: bool,
    },
    /// Nr. 3 queued: the Zuordnung ends without an answer.
    BeendigungGesendet {
        /// What triggered it: a deadline label, or a late answer.
        ausgeloest_durch: Box<str>,
    },
    /// A Meldung the Stilllegung owes was queued with the ending (Nr. 8, 11,
    /// 13).
    MeldungGesendet {
        /// 55038 or 55611.
        pid: u32,
        /// LFZ, MSB or MSBZ.
        receiver: MarktpartnerCode,
        /// `SG4 STS+7` — `ZC8` or `ZH1`.
        transaktionsgrund: String,
    },
    /// A Meldepflicht's 07:00 window closed before the Vorgang ended, so the
    /// Meldung it owes has not gone out.
    MeldepflichtVerstrichen {
        /// The window's label.
        label: Box<str>,
    },
}

impl EventPayload for NbLieferendeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Angekuendigt { .. } => "NbLieferendeAngekuendigt",
            Self::AntwortErhalten { .. } => "NbLieferendeAntwortErhalten",
            Self::BeendigungGesendet { .. } => "NbLieferendeBeendigungGesendet",
            Self::MeldungGesendet { .. } => "NbLieferendeMeldungGesendet",
            Self::MeldepflichtVerstrichen { .. } => "NbLieferendeMeldepflichtVerstrichen",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::Angekuendigt { .. } | Self::BeendigungGesendet { .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(ANKUENDIGUNG_PID),
                message_ref: None,
            }),
            Self::AntwortErhalten {
                response_pid,
                response_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(response_pid.as_u32()),
                message_ref: Some(response_ref.as_str().to_owned()),
            }),
            Self::MeldungGesendet { pid, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(*pid),
                message_ref: None,
            }),
            Self::MeldepflichtVerstrichen { .. } => None,
        }
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortErhalten { antwort, .. } => antwort.clone(),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// What Nr. 1 announced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NbLieferendeData {
    /// The Marktlokation or the MaLo-ID of the Tranche.
    pub location_id: MaLo,
    /// The NB.
    pub sender: MarktpartnerCode,
    /// The LF whose Zuordnung ends.
    pub receiver: MarktpartnerCode,
    /// Zuordnungsende, `YYYYMMDD`.
    pub zuordnungsende: String,
    /// `SG4 STS+7` Transaktionsgrund.
    pub transaktionsgrund: String,
    /// `SG4 STS+7` Ergänzung.
    pub transaktionsgrund_ergaenzung: String,
    /// `SG4 IDE+24` of Nr. 1 — Nr. 3 restates it in `SG6 RFF+TN`.
    pub vorgangsnummer: String,
    /// `SG4 IDE+24` drawn for Nr. 3.
    pub beendigung_vorgangsnummer: String,
    /// The instant the LF's answer window closes.
    #[serde(with = "time::serde::rfc3339")]
    pub antwortfrist: OffsetDateTime,
    /// What a Stilllegung (`Z33`) adds.
    #[serde(default)]
    pub stilllegung: Option<Stilllegung>,
}

/// A Zuordnung that has not begun yet — the LFZ's or the MSBZ's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZukuenftigeZuordnung {
    /// Its holder.
    pub mp_id: MarktpartnerCode,
    /// „Ursprünglich vom NB bestätigtes Beginndatum" (Bedingung `[507]`),
    /// `YYYYMMDD` — the `SG4 DTM+92` of the Aufhebung.
    pub zuordnungsbeginn: String,
}

/// What a Stilllegung of the Marktlokation adds to the Lieferende (GPKE Teil 2
/// Kap. 2.5.2, S. 57 and 61–64).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stilllegung {
    /// `SG4 DTM+206` Geräteausbauzeitpunkt, `YYYYMMDD` — Muss under `Z33`
    /// (Bedingung `[79]`) on the 55007 and on the LF's 55008.
    pub geraeteausbaudatum: String,
    /// The MSB whose Zuordnung ends (Nr. 11, 55611 `ZC8`).
    pub msb: MarktpartnerCode,
    /// A MSBZ whose future Zuordnung is cancelled (Nr. 13, 55611 `ZH1`).
    pub msbz: Option<ZukuenftigeZuordnung>,
    /// A LFZ whose future Zuordnung is cancelled (Nr. 8, 55038 `ZH1`).
    pub lfz: Option<ZukuenftigeZuordnung>,
}

impl Stilllegung {
    /// The Meldepflichten this Stilllegung owes, by Prüfidentifikator — 55611
    /// always (the MSB), 55038 when a LFZ exists.
    #[must_use]
    pub fn meldepflichten(&self) -> Vec<u32> {
        let mut pids = vec![crate::zuordnungsmeldung::MSB_BEENDIGUNG_PID];
        if self.lfz.is_some() {
            pids.push(crate::zuordnungsmeldung::AUFHEBUNG_PID);
        }
        pids
    }
}

/// State of the NB's Lieferende.
///
/// ```text
/// New → Angekuendigt → Beendet { ohne_antwort: false }   (55008 in time)
///                    → Beendet { ohne_antwort: true }    (silence or a late answer)
///                    → Abgelehnt                         (55009 in time)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum NbLieferendeState {
    /// No events yet.
    #[default]
    New,
    /// Nr. 1 is out; the LF's window runs.
    Angekuendigt(NbLieferendeData),
    /// The Zuordnung ends zum Zuordnungsende.
    Beendet {
        /// What Nr. 1 announced.
        data: NbLieferendeData,
        /// `true` when Nr. 3 ended it.
        ohne_antwort: bool,
    },
    /// The LF refused in time and stays zugeordnet.
    Abgelehnt {
        /// What Nr. 1 announced.
        data: NbLieferendeData,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for NbLieferendeState {
    fn occupies_business_key(&self) -> bool {
        // Only a running Ankündigung holds the Marktlokation: a second one for
        // it would end the same Zuordnung twice. Once ended or refused, the
        // next Lieferende is a new Vorgang.
        matches!(self, Self::Angekuendigt(_))
    }
}

impl NbLieferendeState {
    /// Stable label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Angekuendigt(_) => "Angekuendigt",
            Self::Beendet { .. } => "Beendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Nr. 1 as the NB states it. Every field is read or drawn at the boundary.
#[derive(Debug, Clone)]
pub struct Ankuendigung {
    /// The NB's own MP-ID.
    pub sender: MarktpartnerCode,
    /// The LF whose Zuordnung ends.
    pub receiver: MarktpartnerCode,
    /// The Marktlokation or the MaLo-ID of the Tranche.
    pub location_id: MaLo,
    /// Zuordnungsende, `YYYYMMDD` or `YYYY-MM-DD`.
    pub zuordnungsende: String,
    /// One of [`GRUENDE`].
    pub transaktionsgrund: String,
    /// One of [`ERGAENZUNGEN`].
    pub transaktionsgrund_ergaenzung: String,
    /// `SG4 IDE+24` of Nr. 1.
    pub vorgangsnummer: String,
    /// `SG4 IDE+24` for Nr. 3.
    pub beendigung_vorgangsnummer: String,
    /// The instant the LF's answer window closes —
    /// [`mako_fristen::meldung::GPKE_LIEFERENDE_OHNE_ANTWORT`]'s earliest bound.
    pub antwortfrist: OffsetDateTime,
    /// Muss under [`STILLLEGUNG`], refused under any other Grund.
    pub stilllegung: Option<Stilllegung>,
}

/// Commands of the NB's Lieferende.
#[derive(Debug, Clone)]
pub enum NbLieferendeCommand {
    /// Send Nr. 1.
    Ankuendigen(Box<Ankuendigung>),
    /// The LF's 55008 / 55009 arrived.
    ReceiveAntwort {
        /// 55008 or 55009.
        response_pid: Pruefidentifikator,
        /// `SG4 STS+E01` with its Codeliste, when the answer carries one.
        antwort: Option<Antwortcode>,
        /// `FTX+ACB` Begründung.
        reason: Option<String>,
        /// Message reference of the answer.
        response_ref: MessageRef,
        /// Receipt instant.
        received_at: OffsetDateTime,
    },
    /// A Meldepflicht's 07:00 window closed while the Vorgang had not ended.
    MeldepflichtVerstrichen {
        /// The deadline label that fired.
        label: Box<str>,
    },
    /// A window of Nr. 3 opened or closed with no answer: send Nr. 3.
    OhneAntwortBeenden {
        /// The deadline label that fired.
        label: Box<str>,
    },
}

impl CommandPayload for NbLieferendeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The NB's Lieferende (55007 out, 55008 / 55009 in, Nr. 3 on silence).
pub struct GpkeNbLieferendeWorkflow;

/// Whether `label` is one of the Stilllegung's Meldepflicht windows.
fn is_meldepflicht(label: &str) -> bool {
    label == MELDEPFLICHT_LFZ_LABEL || label == MELDEPFLICHT_MSB_LABEL
}

/// Every window is open while the Ankündigung waits for its answer: Nr. 3 is
/// owed, and so is every Meldung that only goes out with the ending.
fn window_open(label: &str, state: &NbLieferendeState) -> bool {
    matches!(state, NbLieferendeState::Angekuendigt(_))
        && (label == LF_ANTWORT_WINDOW_LABEL
            || label == BEENDIGUNG_SPAETESTENS_LABEL
            || label == MELDEPFLICHT_LFZ_LABEL
            || label == MELDEPFLICHT_MSB_LABEL)
}

impl Workflow for GpkeNbLieferendeWorkflow {
    type State = NbLieferendeState;
    type Event = NbLieferendeEvent;
    type Command = NbLieferendeCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[
        LF_ANTWORT_WINDOW_LABEL,
        BEENDIGUNG_SPAETESTENS_LABEL,
        MELDEPFLICHT_LFZ_LABEL,
        MELDEPFLICHT_MSB_LABEL,
    ];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        let label = deadline.label();
        if !window_open(label, state) {
            return None;
        }
        Some(if is_meldepflicht(label) {
            NbLieferendeCommand::MeldepflichtVerstrichen {
                label: label.into(),
            }
        } else {
            NbLieferendeCommand::OhneAntwortBeenden {
                label: label.into(),
            }
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            NbLieferendeEvent::Angekuendigt {
                location_id,
                sender,
                receiver,
                zuordnungsende,
                transaktionsgrund,
                transaktionsgrund_ergaenzung,
                vorgangsnummer,
                beendigung_vorgangsnummer,
                antwortfrist,
                stilllegung,
            } => NbLieferendeState::Angekuendigt(NbLieferendeData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                zuordnungsende: zuordnungsende.clone(),
                transaktionsgrund: transaktionsgrund.clone(),
                transaktionsgrund_ergaenzung: transaktionsgrund_ergaenzung.clone(),
                vorgangsnummer: vorgangsnummer.clone(),
                beendigung_vorgangsnummer: beendigung_vorgangsnummer.clone(),
                antwortfrist: *antwortfrist,
                stilllegung: stilllegung.as_deref().cloned(),
            }),
            NbLieferendeEvent::AntwortErhalten {
                zustimmung,
                fristgerecht,
                ..
            } => match state {
                // A late answer changes nothing here: the Beendigung recorded
                // beside it does.
                NbLieferendeState::Angekuendigt(data) if *fristgerecht => {
                    if *zustimmung {
                        NbLieferendeState::Beendet {
                            data,
                            ohne_antwort: false,
                        }
                    } else {
                        NbLieferendeState::Abgelehnt { data }
                    }
                }
                other => other,
            },
            // Recorded beside the ending, or a breach that changes nothing.
            NbLieferendeEvent::MeldungGesendet { .. }
            | NbLieferendeEvent::MeldepflichtVerstrichen { .. } => state,
            NbLieferendeEvent::BeendigungGesendet { .. } => match state {
                NbLieferendeState::Angekuendigt(data) => NbLieferendeState::Beendet {
                    data,
                    ohne_antwort: true,
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            NbLieferendeCommand::Ankuendigen(a) => {
                if !matches!(state, NbLieferendeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                ankuendigen(*a)
            }
            NbLieferendeCommand::ReceiveAntwort {
                response_pid,
                antwort,
                reason,
                response_ref,
                received_at,
            } => {
                let NbLieferendeState::Angekuendigt(data) = state else {
                    // The Zuordnung has already ended or the LF refused: an
                    // answer now has nothing left to decide.
                    return Err(WorkflowError::invalid_state("Angekuendigt", state.label()));
                };
                if !ANTWORT_PIDS.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an answer to the Ankündigung 55007 ({ANTWORT_PIDS:?}), got \
                         {response_pid}"
                    )));
                }
                let zustimmung = response_pid.as_u32() == BESTAETIGUNG_PID;
                // „Antwortet der LF in Prozessschritt 2 nicht fristgerecht,
                // beendet der NB die Zuordnung" (Nr. 3): an answer after the
                // window is no answer to act on.
                let fristgerecht = received_at <= data.antwortfrist;
                let mut events = vec![NbLieferendeEvent::AntwortErhalten {
                    response_pid,
                    zustimmung,
                    antwort: antwort.clone(),
                    reason: reason.clone(),
                    response_ref,
                    fristgerecht,
                }];
                if !fristgerecht {
                    events.push(NbLieferendeEvent::BeendigungGesendet {
                        ausgeloest_durch: "verspaetete-antwort".into(),
                    });
                    let outbox = beendigung_ohne_antwort(data)
                        .into_iter()
                        .map(|o| o.caused_by(1))
                        .collect();
                    return Ok(mit_meldungen(data, events, outbox));
                }
                let completed = process_completed(
                    data,
                    response_pid.as_u32(),
                    zustimmung,
                    false,
                    antwort.as_ref(),
                    reason.as_deref(),
                );
                if zustimmung {
                    return Ok(mit_meldungen(data, events, vec![completed]));
                }
                // „Der LF bleibt der Marktlokation bzw. Tranche zugeordnet"
                // (Nachbedingung im Fehlerfall) — nothing is ended, so nothing
                // the Stilllegung owes falls due.
                Ok(WorkflowOutput::with_outbox(events, vec![completed]))
            }
            NbLieferendeCommand::MeldepflichtVerstrichen { label } => {
                if !is_meldepflicht(&label) || !window_open(&label, state) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![NbLieferendeEvent::MeldepflichtVerstrichen { label }].into())
            }
            NbLieferendeCommand::OhneAntwortBeenden { label } => {
                let NbLieferendeState::Angekuendigt(data) = state else {
                    // Answered, or the other bound already sent it.
                    return Ok(WorkflowOutput::events(vec![]));
                };
                // A Meldepflicht window ends nothing (Nr. 3 is the LF's
                // windows' consequence, not the Meldungen's).
                if is_meldepflicht(&label) || !window_open(&label, state) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(mit_meldungen(
                    data,
                    vec![NbLieferendeEvent::BeendigungGesendet {
                        ausgeloest_durch: label,
                    }],
                    beendigung_ohne_antwort(data),
                ))
            }
        }
    }
}

/// Nr. 1, refused where its column cannot carry it.
fn ankuendigen(a: Ankuendigung) -> Result<WorkflowOutput<NbLieferendeEvent>, WorkflowError> {
    let grund = a.transaktionsgrund.trim();
    match (grund == STILLLEGUNG, &a.stilllegung) {
        (true, None) => {
            return Err(WorkflowError::rejected(
                "55007 mit Transaktionsgrund Z33 (Auszug wegen Stilllegung): SG4 DTM+206 \
                 Geräteausbauzeitpunkt ist Muss (UTILMD AHB Strom 2.2 Kap. 8.10, Bedingung \
                 [79]), und die Stilllegung beendet die Zuordnung des MSB (GPKE Teil 2 Kap. \
                 2.5.2.2 Nr. 11) — beide fehlen"
                    .to_owned(),
            ));
        }
        (true, Some(st))
            if st.geraeteausbaudatum.trim().is_empty() || st.msb.as_str().trim().is_empty() =>
        {
            return Err(WorkflowError::rejected(
                "55007 Z33: Geräteausbaudatum (SG4 DTM+206) und MSB sind Muss".to_owned(),
            ));
        }
        (false, Some(_)) => {
            return Err(WorkflowError::rejected(format!(
                "55007 {grund}: Geräteausbaudatum, MSB, MSBZ und LFZ gehören zur Stilllegung \
                 (Z33) und zu keinem anderen Grund"
            )));
        }
        _ => {}
    }
    if !GRUENDE.contains(&grund) {
        return Err(WorkflowError::rejected(format!(
            "55007: SG4 STS+7 Transaktionsgrund ist Muss — {} (UTILMD AHB Strom 2.2 Kap. 8.10), \
             nicht {grund:?}",
            GRUENDE.join("/"),
        )));
    }
    let erg = a.transaktionsgrund_ergaenzung.trim();
    if !ERGAENZUNGEN.contains(&erg) {
        return Err(WorkflowError::rejected(format!(
            "55007: SG4 STS+7 Transaktionsgrundergänzung ist Muss — {} (UTILMD AHB Strom 2.2 \
             Kap. 8.10), nicht {erg:?}",
            ERGAENZUNGEN.join("/"),
        )));
    }
    if a.zuordnungsende.trim().is_empty() {
        return Err(WorkflowError::rejected(
            "55007: SG4 DTM+93 Zuordnungsende ist Muss — „Der NB gibt in der Ankündigung zudem \
             insbesondere an: das Zuordnungsende\" (GPKE Teil 2 Kap. 2.5.2.2 Nr. 1)"
                .to_owned(),
        ));
    }
    if a.vorgangsnummer.trim().is_empty() || a.beendigung_vorgangsnummer.trim().is_empty() {
        return Err(WorkflowError::rejected(
            "55007: SG4 IDE+24 Vorgangsnummer ist Muss — die Antwort des LF nennt sie in \
             SG6 RFF+TN"
                .to_owned(),
        ));
    }
    let data = NbLieferendeData {
        location_id: a.location_id,
        sender: a.sender,
        receiver: a.receiver,
        zuordnungsende: a.zuordnungsende,
        transaktionsgrund: grund.to_owned(),
        transaktionsgrund_ergaenzung: erg.to_owned(),
        vorgangsnummer: a.vorgangsnummer,
        beendigung_vorgangsnummer: a.beendigung_vorgangsnummer,
        antwortfrist: a.antwortfrist,
        stilllegung: a.stilllegung,
    };
    let outbox = utilmd_55007(&data, BGM_ANKUENDIGUNG, &data.vorgangsnummer, None);
    Ok(WorkflowOutput::with_outbox(
        vec![NbLieferendeEvent::Angekuendigt {
            location_id: data.location_id,
            sender: data.sender,
            receiver: data.receiver,
            zuordnungsende: data.zuordnungsende,
            transaktionsgrund: data.transaktionsgrund,
            transaktionsgrund_ergaenzung: data.transaktionsgrund_ergaenzung,
            vorgangsnummer: data.vorgangsnummer,
            beendigung_vorgangsnummer: data.beendigung_vorgangsnummer,
            antwortfrist: data.antwortfrist,
            stilllegung: data.stilllegung.map(Box::new),
        }],
        vec![outbox],
    ))
}

/// A 55007 as Nr. 1 (`E02`) or Nr. 3 (`Z90`, restating Nr. 1's Vorgangsnummer
/// in `SG6 RFF+TN`). „In der Nachricht teilt der NB dem LF insbesondere die
/// bereits in Prozessschritt 1 genannten Daten mit" (Nr. 3).
fn utilmd_55007(
    data: &NbLieferendeData,
    bgm: &str,
    vorgangsnummer: &str,
    referenz: Option<&str>,
) -> PendingOutbox {
    PendingOutbox::typed(
        data.receiver.as_str(),
        &UtilmdPayload {
            pid: ANKUENDIGUNG_PID,
            sender: data.sender.as_str().to_owned(),
            receiver: Some(data.receiver.as_str().to_owned()),
            malo: Some(data.location_id.as_str().to_owned()),
            lokationstyp: Some(lokationstyp(&data.transaktionsgrund_ergaenzung).to_owned()),
            process_date: Some(data.zuordnungsende.clone()),
            document_code: Some(bgm.to_owned()),
            vorgangsnummer: Some(vorgangsnummer.to_owned()),
            referenz_vorgangsnummer: referenz.map(ToOwned::to_owned),
            transaktionsgrund: Some(data.transaktionsgrund.clone()),
            transaktionsgrund_ergaenzung: Some(data.transaktionsgrund_ergaenzung.clone()),
            geraeteausbaudatum: data
                .stilllegung
                .as_ref()
                .map(|s| s.geraeteausbaudatum.clone()),
            ..UtilmdPayload::default()
        },
    )
}

/// Nr. 3 and the completion it leaves.
fn beendigung_ohne_antwort(data: &NbLieferendeData) -> Vec<PendingOutbox> {
    vec![
        utilmd_55007(
            data,
            BGM_BEENDIGUNG,
            &data.beendigung_vorgangsnummer,
            Some(&data.vorgangsnummer),
        ),
        process_completed(data, ANKUENDIGUNG_PID, true, true, None, None),
    ]
}

/// The Meldungen a Stilllegung owes once the Zuordnung ends — Nr. 8, 11 and 13
/// (S. 63 f.) — each with the event that records it.
fn meldungen(data: &NbLieferendeData) -> Vec<(NbLieferendeEvent, PendingOutbox)> {
    use crate::zuordnungsmeldung::{Zuordnungsmeldung, meldung_payload};
    let Some(st) = &data.stilllegung else {
        return Vec::new();
    };
    // `ZC8` Beendigung der Zuordnung, `ZH1` Aufhebung wegen Stilllegung.
    let mut owed: Vec<(
        Zuordnungsmeldung,
        &MarktpartnerCode,
        &str,
        &str,
        Vec<MarktpartnerCode>,
    )> = vec![(
        Zuordnungsmeldung::MsbBeendigung,
        &st.msb,
        "ZC8",
        data.zuordnungsende.as_str(),
        Vec::new(),
    )];
    if let Some(msbz) = &st.msbz {
        owed.push((
            Zuordnungsmeldung::MsbBeendigung,
            &msbz.mp_id,
            "ZH1",
            msbz.zuordnungsbeginn.as_str(),
            Vec::new(),
        ));
    }
    if let Some(lfz) = &st.lfz {
        // `SG12 NAD+VY` — „NB bei STS+7++ZH1" (Bedingung `[579]`).
        owed.push((
            Zuordnungsmeldung::Aufhebung,
            &lfz.mp_id,
            "ZH1",
            lfz.zuordnungsbeginn.as_str(),
            vec![data.sender.clone()],
        ));
    }
    owed.into_iter()
        .map(|(meldung, receiver, grund, datum, beteiligte)| {
            (
                NbLieferendeEvent::MeldungGesendet {
                    pid: meldung.pid(),
                    receiver: receiver.clone(),
                    transaktionsgrund: grund.to_owned(),
                },
                PendingOutbox::typed(
                    receiver.as_str(),
                    &meldung_payload(
                        meldung,
                        &data.sender,
                        receiver,
                        &data.location_id,
                        None,
                        grund,
                        Some(datum),
                        &beteiligte,
                        None,
                    ),
                ),
            )
        })
        .collect()
}

/// `events` and `outbox` with the Stilllegung's Meldungen appended, each
/// Meldung caused by its own event.
fn mit_meldungen(
    data: &NbLieferendeData,
    mut events: Vec<NbLieferendeEvent>,
    mut outbox: Vec<PendingOutbox>,
) -> WorkflowOutput<NbLieferendeEvent> {
    for (event, meldung) in meldungen(data) {
        events.push(event);
        outbox.push(meldung.caused_by(events.len() - 1));
    }
    WorkflowOutput::with_outbox(events, outbox)
}

/// The `ProcessCompleted` marktd ends the supply on and edmd orders the
/// Schlussablesung on.
///
/// `pid` is the message that decided the process: the LF's 55008 / 55009, or
/// the NB's own 55007 under `ohne_antwort`. `accepted` is whether the
/// Zuordnung ends; `old_supplier` names whose, so a tranchierte Marktlokation
/// keeps its other suppliers; `process_date` is the Zuordnungsende.
fn process_completed(
    data: &NbLieferendeData,
    pid: u32,
    accepted: bool,
    ohne_antwort: bool,
    antwort: Option<&Antwortcode>,
    reason: Option<&str>,
) -> PendingOutbox {
    PendingOutbox::new(
        "ProcessCompleted",
        "",
        serde_json::json!({
            "pid":           pid,
            "malo_id":       data.location_id.as_str(),
            "process_date":  data.zuordnungsende,
            "old_supplier":  data.receiver.as_str(),
            "grid_operator": data.sender.as_str(),
            "accepted":      accepted,
            "outcome":       if accepted { "accepted" } else { "rejected" },
            "ohne_antwort":  ohne_antwort,
            "reason":        reason,
            "antwortcode":   antwort.map(|a| a.code.as_str()),
            "codeliste":     antwort.and_then(|a| a.codeliste.as_deref()),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::deadline::Deadline;
    use mako_engine::ids::{ProcessId, StreamId, TenantId};
    use mako_engine::version::WorkflowId;
    use time::macros::datetime;

    const MALO: &str = "51238696012";

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn pid(p: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(p).expect("valid PID")
    }

    /// Antwortfrist of an Ankündigung sent Monday 2026-03-02: 05:00 Uhr
    /// Tuesday, Berlin (04:00 UTC).
    const FRIST: OffsetDateTime = datetime!(2026-03-03 04:00 UTC);

    fn ankuendigung(grund: &str, erg: &str) -> NbLieferendeCommand {
        NbLieferendeCommand::Ankuendigen(Box::new(Ankuendigung {
            sender: mp("9900000000003"),
            receiver: mp("9900000000010"),
            location_id: MaLo::new(MALO),
            zuordnungsende: "20260401".to_owned(),
            transaktionsgrund: grund.to_owned(),
            transaktionsgrund_ergaenzung: erg.to_owned(),
            vorgangsnummer: "VG-1".to_owned(),
            beendigung_vorgangsnummer: "VG-3".to_owned(),
            antwortfrist: FRIST,
            stilllegung: None,
        }))
    }

    fn stilllegung(lfz: bool, msbz: bool) -> Stilllegung {
        Stilllegung {
            geraeteausbaudatum: "20260331".to_owned(),
            msb: mp("9900000000027"),
            msbz: msbz.then(|| ZukuenftigeZuordnung {
                mp_id: mp("9900000000034"),
                zuordnungsbeginn: "20260501".to_owned(),
            }),
            lfz: lfz.then(|| ZukuenftigeZuordnung {
                mp_id: mp("9900000000041"),
                zuordnungsbeginn: "20260601".to_owned(),
            }),
        }
    }

    fn stillgelegt(st: Stilllegung) -> NbLieferendeState {
        let NbLieferendeCommand::Ankuendigen(mut a) = ankuendigung("Z33", "ZW4") else {
            unreachable!("an Ankündigung")
        };
        a.stilllegung = Some(st);
        run(NbLieferendeState::New, NbLieferendeCommand::Ankuendigen(a)).0
    }

    /// GPKE Teil 2 Kap. 2.5.2.2 Nr. 1 (S. 61): a Stilllegung names the
    /// Geräteausbauzeitpunkt — `SG4 DTM+206`, Muss under `Z33` (`[79]`) — on
    /// Nr. 1 and Nr. 3 alike.
    #[test]
    fn a_stilllegung_carries_its_geraeteausbaudatum() {
        let NbLieferendeCommand::Ankuendigen(mut a) = ankuendigung("Z33", "ZW4") else {
            unreachable!("an Ankündigung")
        };
        a.stilllegung = Some(stilllegung(false, false));
        let (state, out) = run(NbLieferendeState::New, NbLieferendeCommand::Ankuendigen(a));
        assert_eq!(utilmd(&out)[0].payload["geraeteausbaudatum"], "20260331");
        let (_, out) = run(
            state,
            NbLieferendeCommand::OhneAntwortBeenden {
                label: LF_ANTWORT_WINDOW_LABEL.into(),
            },
        );
        let nr3 = utilmd(&out)
            .into_iter()
            .find(|o| o.payload["pid"] == 55_007)
            .expect("Nr. 3");
        assert_eq!(nr3.payload["geraeteausbaudatum"], "20260331");
    }

    /// The UC (S. 57) and Nr. 8 / 11 / 13 (S. 63 f.): on a Stilllegung the
    /// ending also ends the MSB's Zuordnung and cancels the LFZ's and the
    /// MSBZ's — sent with the ending, on silence and on a Zustimmung alike.
    #[test]
    fn a_stilllegung_ends_the_msb_and_cancels_lfz_and_msbz() {
        let silent = run(
            stillgelegt(stilllegung(true, true)),
            NbLieferendeCommand::OhneAntwortBeenden {
                label: LF_ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .1;
        let agreed = run(
            stillgelegt(stilllegung(true, true)),
            NbLieferendeCommand::ReceiveAntwort {
                response_pid: pid(55_008),
                antwort: None,
                reason: None,
                response_ref: MessageRef::new("R1"),
                received_at: FRIST,
            },
        )
        .1;
        for out in [silent, agreed] {
            let meldungen: Vec<_> = utilmd(&out)
                .into_iter()
                .filter(|o| o.payload["pid"] != 55_007)
                .map(|o| {
                    (
                        o.payload["pid"].as_u64().expect("pid"),
                        o.payload["receiver"].as_str().expect("receiver").to_owned(),
                        o.payload["transaktionsgrund"]
                            .as_str()
                            .expect("grund")
                            .to_owned(),
                        o.payload["process_date"].as_str().expect("date").to_owned(),
                    )
                })
                .collect();
            assert_eq!(
                meldungen,
                [
                    (
                        55_611,
                        "9900000000027".into(),
                        "ZC8".into(),
                        "20260401".into()
                    ),
                    (
                        55_611,
                        "9900000000034".into(),
                        "ZH1".into(),
                        "20260501".into()
                    ),
                    (
                        55_038,
                        "9900000000041".into(),
                        "ZH1".into(),
                        "20260601".into()
                    ),
                ]
            );
            let aufhebung = utilmd(&out)
                .into_iter()
                .find(|o| o.payload["pid"] == 55_038)
                .expect("55038");
            assert_eq!(
                aufhebung.payload["beteiligte_marktpartner"],
                serde_json::json!(["9900000000003"]),
                "NAD+VY names the NB as auslösender Marktpartner bei ZH1 ([579])"
            );
        }
    }

    /// A refused Lieferende ends nothing, so a Stilllegung owes nothing yet.
    #[test]
    fn an_ablehnung_sends_no_meldung() {
        let (_, out) = run(
            stillgelegt(stilllegung(true, true)),
            NbLieferendeCommand::ReceiveAntwort {
                response_pid: pid(55_009),
                antwort: None,
                reason: Some("x".to_owned()),
                response_ref: MessageRef::new("R1"),
                received_at: FRIST,
            },
        );
        assert!(utilmd(&out).is_empty());
    }

    /// A Meldepflicht window that closes while the Vorgang has not ended is a
    /// breach; once the ending sent the Meldungen it finds nothing open.
    #[test]
    fn a_meldepflicht_window_fires_only_while_the_meldung_is_owed() {
        let waiting = stillgelegt(stilllegung(true, false));
        let cmd =
            GpkeNbLieferendeWorkflow::on_deadline(&deadline(MELDEPFLICHT_MSB_LABEL), &waiting)
                .expect("owed while the Vorgang runs");
        let (state, out) = run(waiting, cmd);
        assert!(matches!(
            out.events[..],
            [NbLieferendeEvent::MeldepflichtVerstrichen { .. }]
        ));
        assert!(
            out.outbox.is_empty(),
            "a breach sends nothing and ends nothing"
        );
        assert!(matches!(state, NbLieferendeState::Angekuendigt(_)));
        let (ended, _) = run(
            state,
            NbLieferendeCommand::OhneAntwortBeenden {
                label: LF_ANTWORT_WINDOW_LABEL.into(),
            },
        );
        for label in [MELDEPFLICHT_MSB_LABEL, MELDEPFLICHT_LFZ_LABEL] {
            assert!(GpkeNbLieferendeWorkflow::on_deadline(&deadline(label), &ended).is_none());
        }
    }

    #[test]
    fn a_stilllegung_needs_its_facts_and_no_other_grund_takes_them() {
        assert!(
            GpkeNbLieferendeWorkflow::handle(&NbLieferendeState::New, ankuendigung("Z33", "ZW4"))
                .is_err()
        );
        let NbLieferendeCommand::Ankuendigen(mut a) = ankuendigung("ZQ7", "ZW4") else {
            unreachable!("an Ankündigung")
        };
        a.stilllegung = Some(stilllegung(false, false));
        assert!(
            GpkeNbLieferendeWorkflow::handle(
                &NbLieferendeState::New,
                NbLieferendeCommand::Ankuendigen(a)
            )
            .is_err()
        );
    }

    fn run(
        state: NbLieferendeState,
        cmd: NbLieferendeCommand,
    ) -> (NbLieferendeState, WorkflowOutput<NbLieferendeEvent>) {
        let out = GpkeNbLieferendeWorkflow::handle(&state, cmd).expect("handled");
        let next = out
            .events
            .iter()
            .fold(state, GpkeNbLieferendeWorkflow::apply);
        (next, out)
    }

    fn angekuendigt() -> NbLieferendeState {
        run(NbLieferendeState::New, ankuendigung("ZQ7", "ZW4")).0
    }

    fn deadline(label: &str) -> Deadline {
        Deadline::new(
            StreamId::new("test-stream-nb-lieferende"),
            ProcessId::new(),
            TenantId::from_party_id("9900000000003"),
            WorkflowId::new(WORKFLOW_NAME, "FV2026-10-01"),
            label,
            FRIST,
        )
    }

    fn utilmd(out: &WorkflowOutput<NbLieferendeEvent>) -> Vec<&PendingOutbox> {
        out.outbox
            .iter()
            .filter(|o| &*o.message_type == "UTILMD")
            .collect()
    }

    fn completed(out: &WorkflowOutput<NbLieferendeEvent>) -> &serde_json::Value {
        &out.outbox
            .iter()
            .find(|o| &*o.message_type == "ProcessCompleted")
            .expect("the process completes")
            .payload
    }

    #[test]
    fn the_ankuendigung_is_a_55007_e02_with_its_zuordnungsende() {
        let (state, out) = run(NbLieferendeState::New, ankuendigung("ZQ7", "ZW5"));
        assert!(matches!(state, NbLieferendeState::Angekuendigt(_)));
        let [msg] = utilmd(&out)[..] else {
            panic!("one UTILMD")
        };
        assert_eq!(msg.payload["pid"], 55_007);
        assert_eq!(msg.payload["document_code"], "E02");
        assert_eq!(msg.payload["process_date"], "20260401");
        assert_eq!(msg.payload["vorgangsnummer"], "VG-1");
        assert_eq!(msg.payload["lokationstyp"], "Z21", "a Tranche is LOC+Z21");
        assert!(
            out.outbox
                .iter()
                .all(|o| &*o.message_type != "ProcessCompleted"),
            "nothing ends yet"
        );
    }

    #[test]
    fn a_foreign_grund_is_refused() {
        for (grund, erg) in [("E01", "ZW4"), ("ZQ7", "ZW6"), ("ZQ7", "")] {
            assert!(
                GpkeNbLieferendeWorkflow::handle(&NbLieferendeState::New, ankuendigung(grund, erg))
                    .is_err(),
                "{grund}/{erg}"
            );
        }
    }

    /// GPKE Teil 2 Kap. 2.5.2.2 Nr. 3 (S. 62): an unanswered 55007 ends the
    /// Zuordnung — the NB sends the Beendigung and the supply ends zum
    /// Zuordnungsende.
    #[test]
    fn an_unanswered_55007_ends_the_zuordnung_at_its_window() {
        let state = angekuendigt();
        let cmd = GpkeNbLieferendeWorkflow::on_deadline(&deadline(LF_ANTWORT_WINDOW_LABEL), &state)
            .expect("the LF's window is open while it waits");
        let (state, out) = run(state, cmd);
        assert!(matches!(
            state,
            NbLieferendeState::Beendet {
                ohne_antwort: true,
                ..
            }
        ));
        let [msg] = utilmd(&out)[..] else {
            panic!("one UTILMD")
        };
        assert_eq!(msg.payload["pid"], 55_007);
        assert_eq!(msg.payload["document_code"], "Z90");
        assert_eq!(msg.payload["referenz_vorgangsnummer"], "VG-1");
        assert_eq!(msg.payload["vorgangsnummer"], "VG-3");
        let c = completed(&out);
        assert_eq!(c["pid"], 55_007);
        assert_eq!(c["accepted"], true);
        assert_eq!(c["ohne_antwort"], true);
        assert_eq!(c["process_date"], "20260401");
        assert_eq!(c["old_supplier"], "9900000000010");
        // The latest bound finds it sent and does nothing.
        assert!(
            GpkeNbLieferendeWorkflow::on_deadline(&deadline(BEENDIGUNG_SPAETESTENS_LABEL), &state)
                .is_none()
        );
    }

    #[test]
    fn the_latest_bound_sends_what_the_earliest_did_not() {
        let state = angekuendigt();
        let cmd =
            GpkeNbLieferendeWorkflow::on_deadline(&deadline(BEENDIGUNG_SPAETESTENS_LABEL), &state)
                .expect("still waiting at 06:00");
        let (state, out) = run(state, cmd);
        assert!(matches!(
            state,
            NbLieferendeState::Beendet {
                ohne_antwort: true,
                ..
            }
        ));
        assert_eq!(utilmd(&out).len(), 1);
    }

    #[test]
    fn a_zustimmung_in_time_ends_the_zuordnung() {
        let (state, out) = run(
            angekuendigt(),
            NbLieferendeCommand::ReceiveAntwort {
                response_pid: pid(55_008),
                antwort: Some(Antwortcode::new("A10", Some("E_0609"))),
                reason: None,
                response_ref: MessageRef::new("R1"),
                received_at: datetime!(2026-03-02 15:00 UTC),
            },
        );
        assert!(matches!(
            state,
            NbLieferendeState::Beendet {
                ohne_antwort: false,
                ..
            }
        ));
        assert!(utilmd(&out).is_empty(), "no Nr. 3 after a Zustimmung");
        let c = completed(&out);
        assert_eq!(c["pid"], 55_008);
        assert_eq!(c["accepted"], true);
        assert_eq!(c["ohne_antwort"], false);
        assert_eq!(c["antwortcode"], "A10");
        assert_eq!(c["codeliste"], "E_0609");
        assert!(
            GpkeNbLieferendeWorkflow::on_deadline(&deadline(LF_ANTWORT_WINDOW_LABEL), &state)
                .is_none(),
            "the deadline fires on the healthy path and finds nothing open"
        );
    }

    #[test]
    fn an_ablehnung_in_time_keeps_the_lf() {
        let (state, out) = run(
            angekuendigt(),
            NbLieferendeCommand::ReceiveAntwort {
                response_pid: pid(55_009),
                antwort: Some(Antwortcode::new("A02", Some("E_0609"))),
                reason: Some("bereits bestätigt".to_owned()),
                response_ref: MessageRef::new("R1"),
                received_at: FRIST,
            },
        );
        assert!(matches!(state, NbLieferendeState::Abgelehnt { .. }));
        let c = completed(&out);
        assert_eq!(c["pid"], 55_009);
        assert_eq!(c["accepted"], false);
        assert_eq!(c["outcome"], "rejected");
    }

    /// „Antwortet der LF in Prozessschritt 2 nicht fristgerecht, beendet der NB
    /// die Zuordnung" — a late Ablehnung does not keep the LF.
    #[test]
    fn a_late_answer_is_no_answer() {
        let (state, out) = run(
            angekuendigt(),
            NbLieferendeCommand::ReceiveAntwort {
                response_pid: pid(55_009),
                antwort: None,
                reason: None,
                response_ref: MessageRef::new("R1"),
                received_at: datetime!(2026-03-03 04:30 UTC),
            },
        );
        assert!(matches!(
            state,
            NbLieferendeState::Beendet {
                ohne_antwort: true,
                ..
            }
        ));
        assert_eq!(utilmd(&out)[0].payload["document_code"], "Z90");
        assert_eq!(completed(&out)["pid"], 55_007);
    }

    #[test]
    fn an_answer_after_the_end_is_refused() {
        let (state, _) = run(
            angekuendigt(),
            NbLieferendeCommand::OhneAntwortBeenden {
                label: LF_ANTWORT_WINDOW_LABEL.into(),
            },
        );
        assert!(
            GpkeNbLieferendeWorkflow::handle(
                &state,
                NbLieferendeCommand::ReceiveAntwort {
                    response_pid: pid(55_008),
                    antwort: None,
                    reason: None,
                    response_ref: MessageRef::new("R1"),
                    received_at: FRIST,
                },
            )
            .is_err()
        );
    }

    /// The two labels are the one catalogued step's two bounds, and the earliest
    /// is where the LF's own answer window to the same 55007 closes.
    #[test]
    fn the_windows_are_the_catalogued_step() {
        let step = mako_fristen::meldung::bei_ausbleibender_antwort(ANKUENDIGUNG_PID)
            .expect("Nr. 3 is catalogued");
        let sent = datetime!(2026-03-02 10:00 UTC);
        let (fruehestens, spaetestens) = step.window(sent, mako_fristen::HolidayCalendar::BdewMaKo);
        assert_eq!(fruehestens, FRIST);
        assert_eq!(spaetestens, datetime!(2026-03-03 05:00 UTC));
        assert_eq!(
            mako_fristen::antwort::antwort_deadline(ANKUENDIGUNG_PID, sent),
            Some(fruehestens)
        );
    }

    #[test]
    fn only_a_running_ankuendigung_occupies_the_marktlokation() {
        use mako_engine::workflow::OccupiesBusinessKey as _;
        assert!(!NbLieferendeState::New.occupies_business_key());
        let state = angekuendigt();
        assert!(state.occupies_business_key());
        let (ended, _) = run(
            state,
            NbLieferendeCommand::OhneAntwortBeenden {
                label: LF_ANTWORT_WINDOW_LABEL.into(),
            },
        );
        assert!(!ended.occupies_business_key());
    }

    #[test]
    fn verlauf_states_both_directions() {
        let (_, out) = run(NbLieferendeState::New, ankuendigung("ZT0", "ZW3"));
        let v = out.events[0].verlauf().expect("an Ausgang");
        assert_eq!(v.richtung, Richtung::Ausgang);
        assert_eq!(v.pid, Some(55_007));
        let (_, out) = run(
            angekuendigt(),
            NbLieferendeCommand::ReceiveAntwort {
                response_pid: pid(55_008),
                antwort: Some(Antwortcode::new("A10", Some("E_0609"))),
                reason: None,
                response_ref: MessageRef::new("R9"),
                received_at: FRIST,
            },
        );
        let v = out.events[0].verlauf().expect("an Eingang");
        assert_eq!(v.richtung, Richtung::Eingang);
        assert_eq!(v.pid, Some(55_008));
        assert_eq!(
            out.events[0].antwort().map(|a| a.code),
            Some("A10".to_owned())
        );
    }
}
