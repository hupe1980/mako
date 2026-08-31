//! The three UTILMD legs that move a Marktlokation into and out of Modell 2.
//!
//! # One shape, three legs
//!
//! Every leg is a request and exactly one answer, and the answer PID is fixed
//! rather than chosen from a Bestätigungs-/Ablehnungs-pair — the agreement
//! lives in `SG4 STS+E01` DE 9013 with the tree named in DE 1131. So one state
//! machine serves all three, and the three `Workflow` impls differ only in the
//! leg they pass to it and the deadline label they answer to:
//!
//! | Workflow | Request | Answer | Tree | Frist | Answered by |
//! |---|---|---|---|---|---|
//! | [`EmobAnmeldungWorkflow`] | 55238 | 55239 | `E_0513` → `E_0510` | 7 WT | NB (VNB) |
//! | [`EmobZuordnungsendeWorkflow`] | 55240 | 55241 | `E_0511` | 3 WT | **LF** |
//! | [`EmobAbmeldungWorkflow`] | 55242 | 55243 | `E_0512` | 3 WT | NB (VNB) |
//!
//! Three workflow names rather than one, because all three run on the **same
//! Marktlokation** and `makod` resolves a process by (business key, workflow
//! name): folding them together would make an Abmeldung resume the Anmeldung's
//! stream, and the 55240 leg — which runs *inside* the Anmeldung's window —
//! would collide with the Anmeldung outright.
//!
//! # The Anmeldung's answer is not this workflow's decision
//!
//! `E_0510` Prüfschritt 1 asks „Ging innerhalb der Antwortfrist eine Ablehnung
//! des Lieferanten ein?" — a fact that lives on the *other* process, the
//! [`EmobZuordnungsendeWorkflow`] the VNB opened against the LF. This crate is
//! transport-layer and does not depend on `mako-pruefung`; the decision is made
//! there (`mako_pruefung::emob`) and arrives here as a resolved
//! [`EmobAntwort`] on [`ModellwechselCommand::SendAntwort`], the same way GPKE
//! resolves its `LfAntwort`.
//!
//! # Fristen
//!
//! The answer windows are [`mako_fristen::antwort`]'s, keyed by trigger PID, so
//! `makod`, `processd` and `obsd` cannot disagree with them. Each leg registers
//! its own window at spawn under the label its `*_WINDOW_LABEL` constant names
//! — a `Deadline` whose label no `on_deadline` arm matches fires into `None`
//! and is lost silently.
//!
//! Unlike the GPKE Beendigung der Zuordnung, **silence is not consent here**.
//! Neither Anlage 6 nor the AWH gives an unanswered Modell-2 leg a default
//! outcome, so an expired window escalates rather than confirming: confirming
//! would move a Marktlokation between Bilanzierungsgebiete on no one's say-so.
//!
//! # Sources
//!
//! - BDEW AWH „Zum Modell 2 …" V1.3 (01.04.2025) Kap. 2.1.2 / 2.2.2
//! - BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 (23.06.2026) Kap. 17
//! - UTILMD AHB Strom 2.2 Kap. 11 — the `BGM`/`DTM`/`LOC`/`RFF` columns below
//! - EDI@Energy Anwendungsübersicht der Prüfidentifikatoren 4.0, Lfd. Nr.
//!   19000 / 19010 / 19020 / 19030 / 19050 / 19060

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use serde::{Deserialize, Serialize};

// ── Deadline labels ───────────────────────────────────────────────────────────

/// The VNB's 7-Werktage window to answer a 55238 (AWH Kap. 2.1.2 Nr. 4).
pub const ANMELDUNG_WINDOW_LABEL: &str = "emob-anmeldung-antwort";

/// The LF's 3-Werktage window to answer a 55240 (AWH Kap. 2.1.2 Nr. 3).
pub const ZUORDNUNGSENDE_WINDOW_LABEL: &str = "emob-zuordnungsende-antwort";

/// The VNB's 3-Werktage window to answer a 55242 (AWH Kap. 2.2.2 Nr. 2).
pub const ABMELDUNG_WINDOW_LABEL: &str = "emob-abmeldung-antwort";

// ── The answer ────────────────────────────────────────────────────────────────

/// A resolved Modell-2 answer, ready for `SG4 STS+E01`.
///
/// `mako_pruefung::emob` produces it; this crate only carries it to the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmobAntwort {
    /// `SG4 STS+E01` DE 9013 — the Antwortcode (`A01`, `A02`, `A99`).
    pub antwort_code: String,
    /// `SG4 STS+E01` DE 1131 — the tree that produced it.
    ///
    /// Never inferred from the code: `A01` is an Ablehnung in `E_0510` and a
    /// Zustimmung in `E_0511` and `E_0512`, so the pair `(tree, code)` is the
    /// smallest thing that means anything.
    pub codeliste: String,
    /// `true` when the code sits in the Zustimmungs-Cluster **of its own
    /// tree**.
    pub zustimmung: bool,
    /// `SG4 FTX+ACB` — Muss alongside `A99`, whose EBD says „Das identifizierte
    /// Problem ist in der Antwort zu beschreiben".
    pub bemerkung: Option<String>,
    /// `SG5 LOC+Z15` — the Zählpunktbezeichnung of the ZP der NGZ.
    ///
    /// AHB Bedingung `[663]` makes the 55239 carry „die ID der Marktlokation
    /// und die ZPB des ZP der NGZ", so a Bestätigung without it leaves the LPB
    /// unable to receive the Netzgangzeitreihe it just won the right to.
    pub zp_ngz: Option<String>,
}

impl EmobAntwort {
    /// A Zustimmung from a named tree.
    #[must_use]
    pub fn zustimmung(code: impl Into<String>, tree: impl Into<String>) -> Self {
        Self {
            antwort_code: code.into(),
            codeliste: tree.into(),
            zustimmung: true,
            bemerkung: None,
            zp_ngz: None,
        }
    }

    /// An Ablehnung from a named tree.
    #[must_use]
    pub fn ablehnung(code: impl Into<String>, tree: impl Into<String>) -> Self {
        Self {
            antwort_code: code.into(),
            codeliste: tree.into(),
            zustimmung: false,
            bemerkung: None,
            zp_ngz: None,
        }
    }

    /// Attach the `FTX+ACB` Erläuterung.
    #[must_use]
    pub fn mit_bemerkung(mut self, text: impl Into<String>) -> Self {
        self.bemerkung = Some(text.into());
        self
    }

    /// Attach the Zählpunktbezeichnung of the ZP der NGZ.
    #[must_use]
    pub fn mit_zp_ngz(mut self, zp: impl Into<String>) -> Self {
        self.zp_ngz = Some(zp.into());
        self
    }

    /// `true` when `A99` was answered without the text its EBD requires.
    #[must_use]
    pub fn fehlende_bemerkung(&self) -> bool {
        self.antwort_code == "A99" && self.bemerkung.as_ref().is_none_or(|t| t.trim().is_empty())
    }
}

// ── Per-leg wire facts ────────────────────────────────────────────────────────

/// The wire columns one leg's AHB fixes, read from UTILMD AHB Strom 2.2 Kap. 11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegWire {
    /// The request's Prüfidentifikator.
    pub anfrage_pid: u32,
    /// The answer's Prüfidentifikator.
    pub antwort_pid: u32,
    /// `BGM` DE 1001 — `E01` Anmeldungen, `E44` Beendigung der Zuordnung,
    /// `E02` Abmeldungen. Both messages of a leg carry the same code.
    pub bgm: &'static str,
    /// `SG4 DTM` DE 2005 for the process date — `92` on the Anmeldung, `93` on
    /// the two that end something.
    pub dtm: &'static str,
    /// The second `SG4 DTM` — `158` Bilanzierungsbeginn beside `92`, `159`
    /// Bilanzierungsende beside `93`. AHB Bedingung `[317]` makes it carry the
    /// same value as the first, which is why
    /// [`crate::uebergabestelle::Modellwechsel`] holds one date.
    pub dtm_bilanzierung: &'static str,
    /// The renderer payload key that emits [`Self::dtm_bilanzierung`].
    ///
    /// Two named keys rather than a qualifier the caller supplies: a payload
    /// that could say `"bilanzierung_qualifier": "92"` is a payload that can
    /// state a Vertragsbeginn where the AHB wants a Bilanzierungsbeginn.
    pub bilanzierung_key: &'static str,
    /// The deadline label this leg's answer window is registered under.
    pub window_label: &'static str,
}

/// `SG4 DTM` DE 2005 — Datum Vertragsbeginn.
const DTM_VERTRAGSBEGINN: &str = "92";
/// `SG4 DTM` DE 2005 — Datum Vertragsende.
const DTM_VERTRAGSENDE: &str = "93";
/// `SG4 DTM` DE 2005 — Bilanzierungsbeginn.
const DTM_BILANZIERUNGSBEGINN: &str = "158";
/// `SG4 DTM` DE 2005 — Bilanzierungsende.
const DTM_BILANZIERUNGSENDE: &str = "159";

/// 55238 → 55239, „Anmeldung in Modell 2".
pub const ANMELDUNG: LegWire = LegWire {
    anfrage_pid: 55_238,
    antwort_pid: 55_239,
    bgm: "E01",
    dtm: DTM_VERTRAGSBEGINN,
    dtm_bilanzierung: DTM_BILANZIERUNGSBEGINN,
    bilanzierung_key: "bilanzierungsbeginn",
    window_label: ANMELDUNG_WINDOW_LABEL,
};

/// 55240 → 55241, „Beendigung der Zuordnung zur Marktlokation".
pub const ZUORDNUNGSENDE: LegWire = LegWire {
    anfrage_pid: 55_240,
    antwort_pid: 55_241,
    bgm: "E44",
    dtm: DTM_VERTRAGSENDE,
    dtm_bilanzierung: DTM_BILANZIERUNGSENDE,
    bilanzierung_key: "bilanzierungsende",
    window_label: ZUORDNUNGSENDE_WINDOW_LABEL,
};

/// 55242 → 55243, „Abmeldung aus dem Modell 2".
pub const ABMELDUNG: LegWire = LegWire {
    anfrage_pid: 55_242,
    antwort_pid: 55_243,
    bgm: "E02",
    dtm: DTM_VERTRAGSENDE,
    dtm_bilanzierung: DTM_BILANZIERUNGSENDE,
    bilanzierung_key: "bilanzierungsende",
    window_label: ABMELDUNG_WINDOW_LABEL,
};

// ── Domain data ───────────────────────────────────────────────────────────────

/// What one leg of a Modellwechsel is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modellwechseldaten {
    /// The physical Marktlokation, `SG5 LOC+Z16`.
    pub malo: MaLo,
    /// Who sent the request.
    pub sender: MarktpartnerCode,
    /// Who owes the answer.
    pub receiver: MarktpartnerCode,
    /// The Modellwechseltermin, `YYYYMMDD`. Always a Monatserster.
    pub process_date: String,
    /// The request's Prüfidentifikator.
    pub pruefidentifikator: Pruefidentifikator,
    /// The request's `SG4 IDE+24` DE 7402.
    ///
    /// Kept because the answer must echo it in `SG4 RFF+TN`; it is never
    /// reused as the answer's own `IDE+24`, which has to stay globally unique.
    pub vorgangsnummer: Option<String>,
}

/// State of one leg.
///
/// ```text
/// New ─┬─ Gesendet ──── AntwortErhalten                (we asked)
///      └─ Erhalten ──── Beantwortet                    (we were asked)
///                    ↘ Eskaliert (window lapsed, no default outcome)
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum ModellwechselState {
    /// No events yet.
    #[default]
    New,
    /// We sent the request; the counterparty's window is running.
    Gesendet(Box<Modellwechseldaten>),
    /// We received the request; our own window is running.
    Erhalten(Box<Modellwechseldaten>),
    /// Terminal — the answer is out.
    Beantwortet {
        /// What the leg was about.
        data: Box<Modellwechseldaten>,
        /// What we answered.
        antwort: Box<EmobAntwort>,
    },
    /// Terminal — the answer came back.
    AntwortErhalten {
        /// What the leg was about.
        data: Box<Modellwechseldaten>,
        /// What came back.
        antwort: Box<EmobAntwort>,
    },
    /// Terminal — the window lapsed with no answer.
    ///
    /// Not a Zustimmung: no published rule gives an unanswered Modell-2 leg a
    /// default outcome, so this waits for an operator.
    Eskaliert {
        /// Why.
        grund: String,
    },
    /// Terminal — refused before it began.
    Rejected {
        /// Why.
        grund: String,
    },
}

impl ModellwechselState {
    /// Stable label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Gesendet(_) => "Gesendet",
            Self::Erhalten(_) => "Erhalten",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::AntwortErhalten { .. } => "AntwortErhalten",
            Self::Eskaliert { .. } => "Eskaliert",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// `true` when nothing more will happen on this process.
    ///
    /// Read by `makod` before resuming: a second Anmeldung on the same
    /// Marktlokation must spawn rather than reopen a settled one.
    #[must_use]
    pub const fn ist_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. }
                | Self::AntwortErhalten { .. }
                | Self::Eskaliert { .. }
                | Self::Rejected { .. }
        )
    }

    /// The leg's data, once it has any.
    #[must_use]
    pub fn daten(&self) -> Option<&Modellwechseldaten> {
        match self {
            Self::Gesendet(d) | Self::Erhalten(d) => Some(d),
            Self::Beantwortet { data, .. } | Self::AntwortErhalten { data, .. } => Some(data),
            Self::New | Self::Eskaliert { .. } | Self::Rejected { .. } => None,
        }
    }
}

/// What happened on one leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ModellwechselEvent {
    /// We put the request on the wire.
    AnfrageGesendet {
        /// What it was about.
        data: Box<Modellwechseldaten>,
    },
    /// The request arrived.
    AnfrageErhalten {
        /// What it is about.
        data: Box<Modellwechseldaten>,
        /// The inbound EDIFACT reference.
        message_ref: MessageRef,
    },
    /// We put the answer on the wire.
    AntwortGesendet {
        /// What we answered.
        antwort: Box<EmobAntwort>,
    },
    /// The answer arrived.
    AntwortErhalten {
        /// What came back.
        antwort: Box<EmobAntwort>,
    },
    /// The answer window lapsed.
    FristAbgelaufen {
        /// The deadline's label.
        label: String,
    },
    /// The leg was refused before it began.
    Abgewiesen {
        /// Why.
        grund: String,
    },
}

impl EventPayload for ModellwechselEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "EmobAnfrageGesendet",
            Self::AnfrageErhalten { .. } => "EmobAnfrageErhalten",
            Self::AntwortGesendet { .. } => "EmobAntwortGesendet",
            Self::AntwortErhalten { .. } => "EmobAntwortErhalten",
            Self::FristAbgelaufen { .. } => "EmobFristAbgelaufen",
            Self::Abgewiesen { .. } => "EmobAbgewiesen",
        }
    }
}

/// Commands for one leg.
#[derive(Debug, Clone)]
pub enum ModellwechselCommand {
    /// Render and queue the request.
    Senden {
        /// What it is about.
        data: Box<Modellwechseldaten>,
    },
    /// The request arrived from the AS4 layer.
    ReceiveAnfrage {
        /// What it is about.
        data: Box<Modellwechseldaten>,
        /// The inbound EDIFACT reference.
        message_ref: MessageRef,
        /// `false` when the AHB layer rejected it.
        validation_passed: bool,
        /// What the AHB layer said.
        validation_errors: Vec<String>,
    },
    /// Render and queue the answer.
    SendAntwort {
        /// The resolved answer, from `mako_pruefung::emob`.
        antwort: Box<EmobAntwort>,
    },
    /// The answer arrived.
    ReceiveAntwort {
        /// What came back.
        antwort: Box<EmobAntwort>,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Which one.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl CommandPayload for ModellwechselCommand {}

// ── Shared behaviour ──────────────────────────────────────────────────────────

fn apply(state: ModellwechselState, event: &ModellwechselEvent) -> ModellwechselState {
    match event {
        ModellwechselEvent::AnfrageGesendet { data } => ModellwechselState::Gesendet(data.clone()),
        ModellwechselEvent::AnfrageErhalten { data, .. } => {
            ModellwechselState::Erhalten(data.clone())
        }
        ModellwechselEvent::AntwortGesendet { antwort } => match state {
            ModellwechselState::Erhalten(data) => ModellwechselState::Beantwortet {
                data,
                antwort: antwort.clone(),
            },
            other => other,
        },
        ModellwechselEvent::AntwortErhalten { antwort } => match state {
            ModellwechselState::Gesendet(data) => ModellwechselState::AntwortErhalten {
                data,
                antwort: antwort.clone(),
            },
            other => other,
        },
        ModellwechselEvent::FristAbgelaufen { label } => {
            if state.ist_terminal() {
                state
            } else {
                ModellwechselState::Eskaliert {
                    grund: format!("Antwortfrist {label} verstrichen, keine Antwort eingegangen"),
                }
            }
        }
        ModellwechselEvent::Abgewiesen { grund } => ModellwechselState::Rejected {
            grund: grund.clone(),
        },
    }
}

/// The `PendingOutbox` for a request on `leg`.
fn anfrage_outbox(leg: LegWire, data: &Modellwechseldaten) -> PendingOutbox {
    let mut payload = serde_json::json!({
        "pid":            leg.anfrage_pid,
        "sender":         data.sender.as_str(),
        "receiver":       data.receiver.as_str(),
        "malo":           data.malo.as_str(),
        "process_date":   data.process_date,
        "document_code":  leg.bgm,
        "dtm_qualifier":  leg.dtm,
    });
    // AHB Bedingung [317]: the Bilanzierungs-date carries the same value as
    // the Vertrags-date. One date in the domain, two segments on the wire.
    payload[leg.bilanzierung_key] = serde_json::Value::String(data.process_date.clone());
    if let Some(vn) = &data.vorgangsnummer {
        payload["vorgangsnummer"] = serde_json::Value::String(vn.clone());
    }
    PendingOutbox::new("UTILMD", data.receiver.as_str(), payload)
}

/// The `PendingOutbox` for an answer on `leg`.
///
/// The parties swap: the request's receiver is the answer's sender.
fn antwort_outbox(leg: LegWire, data: &Modellwechseldaten, antwort: &EmobAntwort) -> PendingOutbox {
    let mut payload = serde_json::json!({
        "pid":            leg.antwort_pid,
        "sender":         data.receiver.as_str(),
        "receiver":       data.sender.as_str(),
        "malo":           data.malo.as_str(),
        "process_date":   data.process_date,
        "document_code":  leg.bgm,
        "dtm_qualifier":  leg.dtm,
        "antwort_code":   antwort.antwort_code,
        // DE 1131. The one payload key for it — see the renderer's field table.
        "antwort_codeliste": antwort.codeliste,
    });
    payload[leg.bilanzierung_key] = serde_json::Value::String(data.process_date.clone());
    if let Some(text) = &antwort.bemerkung {
        payload["bemerkung"] = serde_json::Value::String(text.clone());
    }
    if let Some(zp) = &antwort.zp_ngz {
        payload["mabis_zaehlpunkt"] = serde_json::Value::String(zp.clone());
    }
    if let Some(vn) = &data.vorgangsnummer {
        payload["referenz_vorgangsnummer"] = serde_json::Value::String(vn.clone());
    }
    PendingOutbox::new("UTILMD", data.sender.as_str(), payload)
}

fn handle(
    state: &ModellwechselState,
    command: ModellwechselCommand,
    leg: LegWire,
) -> Result<WorkflowOutput<ModellwechselEvent>, WorkflowError> {
    match command {
        ModellwechselCommand::Senden { data } => {
            if !matches!(state, ModellwechselState::New) {
                return Err(WorkflowError::invalid_state("New", state.label()));
            }
            let outbox = vec![anfrage_outbox(leg, &data).caused_by(0)];
            Ok(WorkflowOutput::with_outbox(
                vec![ModellwechselEvent::AnfrageGesendet { data }],
                outbox,
            ))
        }

        ModellwechselCommand::ReceiveAnfrage {
            data,
            message_ref,
            validation_passed,
            validation_errors,
        } => {
            if !matches!(state, ModellwechselState::New) {
                return Err(WorkflowError::invalid_state("New", state.label()));
            }
            if data.pruefidentifikator.as_u32() != leg.anfrage_pid {
                return Err(WorkflowError::rejected(format!(
                    "expected the {} Anfrage, got {}",
                    leg.anfrage_pid, data.pruefidentifikator
                )));
            }
            if validation_passed {
                Ok(vec![ModellwechselEvent::AnfrageErhalten { data, message_ref }].into())
            } else {
                let grund = validation_errors.join("; ");
                Ok(vec![
                    ModellwechselEvent::AnfrageErhalten { data, message_ref },
                    ModellwechselEvent::Abgewiesen { grund },
                ]
                .into())
            }
        }

        ModellwechselCommand::SendAntwort { antwort } => {
            let ModellwechselState::Erhalten(data) = state else {
                return Err(WorkflowError::invalid_state("Erhalten", state.label()));
            };
            // „Das identifizierte Problem ist in der Antwort zu
            // beschreiben/benennen" — an `A99` with no `FTX+ACB` is an
            // incomplete answer the receiving AHB layer rejects, and the
            // counterparty learns only that it failed.
            if antwort.fehlende_bemerkung() {
                return Err(WorkflowError::rejected(
                    "A99 must carry the FTX+ACB Erläuterung its EBD requires".to_owned(),
                ));
            }
            let outbox = vec![antwort_outbox(leg, data, &antwort).caused_by(0)];
            Ok(WorkflowOutput::with_outbox(
                vec![ModellwechselEvent::AntwortGesendet { antwort }],
                outbox,
            ))
        }

        ModellwechselCommand::ReceiveAntwort { antwort } => match state {
            ModellwechselState::Gesendet(_) => {
                Ok(vec![ModellwechselEvent::AntwortErhalten { antwort }].into())
            }
            // A duplicate answer is not an error; the first one settled it.
            ModellwechselState::AntwortErhalten { .. } => Ok(vec![].into()),
            other => Err(WorkflowError::invalid_state("Gesendet", other.label())),
        },

        ModellwechselCommand::TimeoutExpired { label, .. } => {
            if state.ist_terminal() {
                return Ok(vec![].into());
            }
            Ok(vec![ModellwechselEvent::FristAbgelaufen {
                label: label.to_string(),
            }]
            .into())
        }
    }
}

// ── One workflow per leg ──────────────────────────────────────────────────────

/// The shared tail of every leg's `on_deadline`: fire only while the leg is
/// still open.
///
/// Each impl tests `deadline.label()` against **its own** window before calling
/// this. That test belongs in the impl and not here: an `on_deadline` that does
/// not read the label consumes every deadline on the stream, the APERAK
/// 45-minute and CONTRL 6-hour delivery windows included, and a late
/// acknowledgement then fails the business process.
fn timeout_command(
    deadline: &Deadline,
    state: &ModellwechselState,
) -> Option<ModellwechselCommand> {
    (!state.ist_terminal()).then(|| ModellwechselCommand::TimeoutExpired {
        deadline_id: deadline.deadline_id(),
        label: deadline.label().into(),
    })
}

/// **55238 → 55239** — the LPB asks the VNB to balance a Marktlokation in its
/// Bilanzierungsgebiet.
///
/// The VNB answers within **7 Werktage**, and not before the LF's own window on
/// the [`EmobZuordnungsendeWorkflow`] leg has run: `E_0510` Prüfschritt 1 reads
/// that leg's outcome, which cannot be known before the 6th Werktag. Three is
/// arithmetically impossible, and is the Abmeldung's window (AWH Kap. 2.2.2
/// Nr. 2), not this one's.
pub struct EmobAnmeldungWorkflow;

/// **55240 → 55241** — the VNB tells the Marktlokation's LF that its Zuordnung
/// ends, and the LF answers with `E_0511`.
///
/// The leg `E_0514` stands for: the AWH numbers it Kap. 2.1.2 Nr. 2 and gives
/// the VNB „unverzüglich, jedoch spätestens bis zum Ablauf des 3. WT nach
/// Eingang der Anmeldung" to open it, and the LF 3 Werktage to answer (Nr. 3).
/// The LF is not being asked to consent — the Anmeldung is the LPB's right —
/// which is why `A01` here is the **Zustimmung** while the same code in
/// `E_0510` refuses.
pub struct EmobZuordnungsendeWorkflow;

/// **55242 → 55243** — the LPB takes a Marktlokation back out of Modell 2,
/// answered with `E_0512` within 3 Werktage (AWH Kap. 2.2.2 Nr. 2).
pub struct EmobAbmeldungWorkflow;

impl EmobAnmeldungWorkflow {
    /// The workflow name `makod` routes 55238 and 55239 to.
    pub const WORKFLOW_NAME: &'static str = "emob-anmeldung";

    /// The AHB columns this leg's two messages carry.
    #[must_use]
    pub const fn wire() -> LegWire {
        ANMELDUNG
    }
}

impl EmobZuordnungsendeWorkflow {
    /// The workflow name `makod` routes 55240 and 55241 to.
    pub const WORKFLOW_NAME: &'static str = "emob-zuordnungsende";

    /// The AHB columns this leg's two messages carry.
    #[must_use]
    pub const fn wire() -> LegWire {
        ZUORDNUNGSENDE
    }
}

impl EmobAbmeldungWorkflow {
    /// The workflow name `makod` routes 55242 and 55243 to.
    pub const WORKFLOW_NAME: &'static str = "emob-abmeldung";

    /// The AHB columns this leg's two messages carry.
    #[must_use]
    pub const fn wire() -> LegWire {
        ABMELDUNG
    }
}

impl Workflow for EmobAnmeldungWorkflow {
    type State = ModellwechselState;
    type Event = ModellwechselEvent;
    type Command = ModellwechselCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        if deadline.label() != ANMELDUNG_WINDOW_LABEL {
            return None;
        }
        timeout_command(deadline, state)
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        apply(state, event)
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        handle(state, command, ANMELDUNG)
    }
}

impl Workflow for EmobZuordnungsendeWorkflow {
    type State = ModellwechselState;
    type Event = ModellwechselEvent;
    type Command = ModellwechselCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        if deadline.label() != ZUORDNUNGSENDE_WINDOW_LABEL {
            return None;
        }
        timeout_command(deadline, state)
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        apply(state, event)
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        handle(state, command, ZUORDNUNGSENDE)
    }
}

impl Workflow for EmobAbmeldungWorkflow {
    type State = ModellwechselState;
    type Event = ModellwechselEvent;
    type Command = ModellwechselCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        if deadline.label() != ABMELDUNG_WINDOW_LABEL {
            return None;
        }
        timeout_command(deadline, state)
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        apply(state, event)
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        handle(state, command, ABMELDUNG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daten(pid: u32) -> Modellwechseldaten {
        Modellwechseldaten {
            malo: MaLo::new("51238696012"),
            sender: MarktpartnerCode::new("9900123456789"),
            receiver: MarktpartnerCode::new("9900987654321"),
            process_date: "20270101".to_owned(),
            pruefidentifikator: Pruefidentifikator::const_new(pid),
            vorgangsnummer: Some("LPB-0001".to_owned()),
        }
    }

    fn erhalten(leg: LegWire) -> ModellwechselState {
        let out = handle(
            &ModellwechselState::New,
            ModellwechselCommand::ReceiveAnfrage {
                data: Box::new(daten(leg.anfrage_pid)),
                message_ref: MessageRef::new("MSG1"),
                validation_passed: true,
                validation_errors: Vec::new(),
            },
            leg,
        )
        .expect("accepted");
        out.events.iter().fold(ModellwechselState::New, apply)
    }

    #[test]
    fn the_three_legs_carry_their_ahb_columns() {
        assert_eq!(
            (
                ANMELDUNG.bgm,
                ANMELDUNG.dtm,
                ANMELDUNG.dtm_bilanzierung,
                ANMELDUNG.bilanzierung_key
            ),
            ("E01", "92", "158", "bilanzierungsbeginn")
        );
        assert_eq!(
            (
                ZUORDNUNGSENDE.bgm,
                ZUORDNUNGSENDE.dtm,
                ZUORDNUNGSENDE.dtm_bilanzierung,
                ZUORDNUNGSENDE.bilanzierung_key
            ),
            ("E44", "93", "159", "bilanzierungsende")
        );
        assert_eq!(
            (
                ABMELDUNG.bgm,
                ABMELDUNG.dtm,
                ABMELDUNG.dtm_bilanzierung,
                ABMELDUNG.bilanzierung_key
            ),
            ("E02", "93", "159", "bilanzierungsende")
        );
    }

    #[test]
    fn every_leg_has_its_own_workflow_name_and_label() {
        let names = [
            EmobAnmeldungWorkflow::WORKFLOW_NAME,
            EmobZuordnungsendeWorkflow::WORKFLOW_NAME,
            EmobAbmeldungWorkflow::WORKFLOW_NAME,
        ];
        let labels = [
            ANMELDUNG.window_label,
            ZUORDNUNGSENDE.window_label,
            ABMELDUNG.window_label,
        ];
        for set in [names, labels] {
            let mut sorted = set.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), 3, "{set:?} collide");
        }
    }

    /// The request goes out with the AHB's own `BGM`/`DTM` columns.
    #[test]
    fn a_sent_anmeldung_states_its_bgm_and_both_dates() {
        let out = handle(
            &ModellwechselState::New,
            ModellwechselCommand::Senden {
                data: Box::new(daten(55_238)),
            },
            ANMELDUNG,
        )
        .expect("sent");
        let p = &out.outbox[0].payload;
        assert_eq!(p["pid"], 55_238);
        assert_eq!(p["document_code"], "E01");
        assert_eq!(p["dtm_qualifier"], "92");
        // AHB Bedingung [317] — one date, two segments.
        assert_eq!(p["bilanzierungsbeginn"], p["process_date"]);
        assert!(p.get("bilanzierungsende").is_none());
    }

    /// The answer swaps the parties and echoes `SG4 RFF+TN`.
    #[test]
    fn the_answer_travels_back_and_names_its_tree() {
        let state = erhalten(ANMELDUNG);
        let out = handle(
            &state,
            ModellwechselCommand::SendAntwort {
                antwort: Box::new(
                    EmobAntwort::zustimmung("A02", "E_0510")
                        .mit_zp_ngz("DE0001234567890000000000000000123"),
                ),
            },
            ANMELDUNG,
        )
        .expect("answered");
        let p = &out.outbox[0].payload;
        assert_eq!(p["pid"], 55_239);
        assert_eq!(p["sender"], "9900987654321", "the answerer sends");
        assert_eq!(p["receiver"], "9900123456789", "the asker receives");
        assert_eq!(p["antwort_code"], "A02");
        assert_eq!(p["antwort_codeliste"], "E_0510");
        assert_eq!(p["referenz_vorgangsnummer"], "LPB-0001");
        assert_eq!(
            p["mabis_zaehlpunkt"], "DE0001234567890000000000000000123",
            "AHB Bedingung [663] — the Bestätigung names the ZP der NGZ"
        );
        assert!(
            p.get("vorgangsnummer").is_none(),
            "IDE+24 stays a fresh number; the request's rides RFF+TN"
        );
    }

    /// `A01` means opposite things in `E_0510` and `E_0511`, so the pair
    /// travels together and the code never rides alone.
    #[test]
    fn the_tree_rides_with_every_code() {
        for (leg, antwort) in [
            (ANMELDUNG, EmobAntwort::ablehnung("A01", "E_0510")),
            (ZUORDNUNGSENDE, EmobAntwort::zustimmung("A01", "E_0511")),
            (ABMELDUNG, EmobAntwort::zustimmung("A01", "E_0512")),
        ] {
            let out = handle(
                &erhalten(leg),
                ModellwechselCommand::SendAntwort {
                    antwort: Box::new(antwort.clone()),
                },
                leg,
            )
            .expect("answered");
            let p = &out.outbox[0].payload;
            assert_eq!(p["antwort_code"], "A01");
            assert_eq!(p["antwort_codeliste"], antwort.codeliste);
        }
    }

    #[test]
    fn an_a99_without_its_erlaeuterung_is_refused() {
        let state = erhalten(ABMELDUNG);
        assert!(
            handle(
                &state,
                ModellwechselCommand::SendAntwort {
                    antwort: Box::new(EmobAntwort::ablehnung("A99", "E_0512")),
                },
                ABMELDUNG,
            )
            .is_err()
        );
        assert!(
            handle(
                &state,
                ModellwechselCommand::SendAntwort {
                    antwort: Box::new(
                        EmobAntwort::ablehnung("A99", "E_0512").mit_bemerkung("BG nicht gültig")
                    ),
                },
                ABMELDUNG,
            )
            .is_ok()
        );
    }

    /// A leg answered on the wrong PID is not this leg.
    #[test]
    fn a_leg_refuses_another_legs_pid() {
        assert!(
            handle(
                &ModellwechselState::New,
                ModellwechselCommand::ReceiveAnfrage {
                    data: Box::new(daten(55_242)),
                    message_ref: MessageRef::new("MSG1"),
                    validation_passed: true,
                    validation_errors: Vec::new(),
                },
                ANMELDUNG,
            )
            .is_err()
        );
    }

    /// **Silence is not consent.** No published rule gives an unanswered
    /// Modell-2 leg a default outcome, so the window escalates.
    #[test]
    fn an_expired_window_escalates_rather_than_confirming() {
        let state = erhalten(ANMELDUNG);
        let out = handle(
            &state,
            ModellwechselCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANMELDUNG.window_label.into(),
            },
            ANMELDUNG,
        )
        .expect("fires");
        let next = out.events.iter().fold(state, apply);
        assert!(
            matches!(next, ModellwechselState::Eskaliert { .. }),
            "{next:?}"
        );
        assert!(next.ist_terminal());
    }

    /// A deadline that fires after the answer went out changes nothing.
    #[test]
    fn a_late_deadline_on_a_settled_leg_is_a_no_op() {
        let state = erhalten(ABMELDUNG);
        let answered = handle(
            &state,
            ModellwechselCommand::SendAntwort {
                antwort: Box::new(EmobAntwort::zustimmung("A01", "E_0512")),
            },
            ABMELDUNG,
        )
        .expect("answered");
        let settled = answered.events.iter().fold(state, apply);
        assert!(settled.ist_terminal());
        let out = handle(
            &settled,
            ModellwechselCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ABMELDUNG.window_label.into(),
            },
            ABMELDUNG,
        )
        .expect("no-op");
        assert!(out.events.is_empty());
    }

    /// A failed AHB validation records the request *and* the refusal — the
    /// counterparty's Vorgangsnummer has to stay auditable.
    #[test]
    fn a_failed_validation_still_records_what_arrived() {
        let out = handle(
            &ModellwechselState::New,
            ModellwechselCommand::ReceiveAnfrage {
                data: Box::new(daten(55_240)),
                message_ref: MessageRef::new("MSG1"),
                validation_passed: false,
                validation_errors: vec!["SG10 CCI ist nicht erlaubt".to_owned()],
            },
            ZUORDNUNGSENDE,
        )
        .expect("recorded");
        assert_eq!(out.events.len(), 2);
        let state = out.events.iter().fold(ModellwechselState::New, apply);
        assert!(matches!(state, ModellwechselState::Rejected { .. }));
    }

    #[test]
    fn the_requester_takes_the_answer_back() {
        let sent = handle(
            &ModellwechselState::New,
            ModellwechselCommand::Senden {
                data: Box::new(daten(55_238)),
            },
            ANMELDUNG,
        )
        .expect("sent");
        let state = sent.events.iter().fold(ModellwechselState::New, apply);
        let got = handle(
            &state,
            ModellwechselCommand::ReceiveAntwort {
                antwort: Box::new(EmobAntwort::zustimmung("A02", "E_0510")),
            },
            ANMELDUNG,
        )
        .expect("received");
        let settled = got.events.iter().fold(state, apply);
        assert!(matches!(
            settled,
            ModellwechselState::AntwortErhalten { .. }
        ));
        assert!(settled.ist_terminal());
    }
}
