//! WiM ESA Wertebestellung — **ESA origination side**.
//!
//! The mirror of [`super::wertebestellung`] (which is the MSB side). Here the
//! deployment **is** the Energieserviceanbieter: it *originates* the Werteanfrage
//! (REQOTE 35002), Bestellung (ORDERS 17007), Stornierung (ORDCHG 39002) and
//! Abbestellung (ORDERS 17008), and *receives* the MSB's answers (QUOTES 15003,
//! ORDRSP 19011/19012/19013/19014).
//!
//! §49 Abs. 2 Nr. 9 MsbG makes the ESA a consent-derived role: it may request a
//! location's values only while it holds a GDPR-Art.-7 Einwilligung. That guard
//! is enforced at the makod command boundary (the `esa_outbound` consent check)
//! before `SendWerteanfrage`/`SendBestellung` reach this pure workflow. GDPR
//! Art. 7(3) revocation drives `SendAbbestellung` (17008) — the only market
//! mechanism that stops a running delivery — which is therefore **not** gated.
//!
//! # Message flow
//!
//! ```text
//! ESA ──REQOTE 35002 Anfrage──────────────────────────────────────────▶ MSB
//! ESA ◀─QUOTES 15003 Angebot──────────── 5 WT nach ÜT der Anfrage ────── MSB
//! ESA ──ORDERS 17007 Bestellung──────────── bis Ablauf der Bindungsfrist ▶ MSB
//! ESA ◀─ORDRSP 19011 / 19012──────────── 2 WT nach ÜT der Bestellung ─── MSB
//!
//! (before delivery starts)
//! ESA ──ORDCHG 39002 Stornierung──────────────────────────────────────▶ MSB
//! ESA ◀─ORDRSP 19013 / 19014─────────── 2 WT nach ÜT der Stornierung ─── MSB
//!
//! (once delivery is running — the Art. 7(3) revocation path)
//! ESA ──ORDERS 17008 Abbestellung─────────────────────────────────────▶ MSB
//! ESA ◀─ORDRSP 19011 / 19012──────────── 2 WT nach ÜT der Abbestellung ─ MSB
//! ```

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use time::OffsetDateTime;

// Reuse the shared vocabulary from the MSB side so both directions speak the
// same PIDs, Fristen and location model.
pub use super::wertebestellung::{
    ABBESTELLUNG_PID, ABLEHNUNG_PID, ANFRAGE_PID, ANGEBOT_PID, ANTWORT_FRIST_WT, BESTAETIGUNG_PID,
    BESTELLUNG_PID, Lokationsebene, STORNIERUNG_PID, STORNO_ABLEHNUNG_PID, STORNO_BESTAETIGUNG_PID,
    Zustellquittung,
};

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "esa-wertebestellung";

/// Deadline label for the Angebot the ESA awaits after its Anfrage (5 WT).
pub const ANGEBOT_WINDOW_LABEL: &str = "esa-wertebestellung-angebot";

/// Deadline label for the Bindungsfrist within which the ESA must order.
pub const BINDUNGSFRIST_LABEL: &str = "esa-wertebestellung-bindungsfrist";

/// Deadline label for the ORDRSP answer the ESA awaits (2 WT).
pub const ANTWORT_WINDOW_LABEL: &str = "esa-wertebestellung-antwort";

/// PIDs an ESA deployment receives inbound (MSB → ESA). Identical to
/// [`super::wertebestellung::ESA_INBOUND_PIDS`]; re-exported for routing clarity.
pub use super::wertebestellung::ESA_INBOUND_PIDS;

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the ESA-origination workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EsaWertebestellungEvent {
    /// REQOTE 35002 sent — the ESA asked the MSB for values.
    AnfrageGesendet {
        /// GLN of this ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB addressed.
        msb: MarktpartnerCode,
        /// Location level requested.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB or NeLo-ID.
        lokations_id: String,
        /// Reference of the outbound REQOTE.
        message_ref: MessageRef,
    },
    /// QUOTES 15003 Angebot received — the ESA may order until `bindungsfrist`.
    AngebotErhalten {
        /// Reference of the inbound QUOTES.
        message_ref: MessageRef,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
    },
    /// QUOTES 15003 Ablehnung received — the MSB refused the Anfrage; the process
    /// ends. (Distinguished from an Angebot by carrying no Bindungsfrist.)
    AnfrageAbgelehnt {
        /// Reason communicated by the MSB.
        reason: String,
    },
    /// ORDERS 17007 Bestellung sent.
    BestellungGesendet {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19011 received — delivery is authorised.
    BestellungBestaetigt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// ORDRSP 19012 received — the MSB refused the Bestellung; the process ends.
    BestellungAbgelehnt {
        /// Reason communicated by the MSB.
        reason: String,
    },
    /// ORDCHG 39002 Stornierung sent (before delivery began).
    StornierungGesendet {
        /// Reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19013 received — the Stornierung was accepted; the order is void.
    StornierungBestaetigt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// ORDRSP 19014 received — the Stornierung was refused; the order stands.
    StornierungAbgelehnt {
        /// Reason communicated by the MSB.
        reason: String,
    },
    /// ORDERS 17008 Abbestellung sent (the Art. 7(3) revocation path).
    AbbestellungGesendet {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// Date delivery is to stop.
        beendigung_zum: OffsetDateTime,
        /// Trigger — typically `einwilligung_widerrufen`.
        grund: String,
    },
    /// ORDRSP 19011 received for the Abbestellung — delivery has ended.
    AbbestellungBestaetigt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// ORDRSP 19012 received for the Abbestellung — the MSB refused to stop;
    /// delivery continues. Surfaced so the operator can escalate (refusing a
    /// GDPR-Art.-7(3) Widerruf is a compliance incident).
    AbbestellungAbgelehnt {
        /// Reason communicated by the MSB.
        reason: String,
    },
    /// First values arrived; the Stornierung window closes.
    LieferungBegonnen,
    /// A regulatory window elapsed without the awaited answer.
    FristVersaeumt {
        /// Deadline label that fired.
        label: String,
    },
}

impl EventPayload for EsaWertebestellungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "EsaWertebestellungAnfrageGesendet",
            Self::AngebotErhalten { .. } => "EsaWertebestellungAngebotErhalten",
            Self::BestellungGesendet { .. } => "EsaWertebestellungBestellungGesendet",
            Self::BestellungBestaetigt { .. } => "EsaWertebestellungBestellungBestaetigt",
            Self::AnfrageAbgelehnt { .. } => "EsaWertebestellungAnfrageAbgelehnt",
            Self::BestellungAbgelehnt { .. } => "EsaWertebestellungBestellungAbgelehnt",
            Self::StornierungGesendet { .. } => "EsaWertebestellungStornierungGesendet",
            Self::StornierungBestaetigt { .. } => "EsaWertebestellungStornierungBestaetigt",
            Self::StornierungAbgelehnt { .. } => "EsaWertebestellungStornierungAbgelehnt",
            Self::AbbestellungGesendet { .. } => "EsaWertebestellungAbbestellungGesendet",
            Self::AbbestellungBestaetigt { .. } => "EsaWertebestellungAbbestellungBestaetigt",
            Self::AbbestellungAbgelehnt { .. } => "EsaWertebestellungAbbestellungAbgelehnt",
            Self::LieferungBegonnen => "EsaWertebestellungLieferungBegonnen",
            Self::FristVersaeumt { .. } => "EsaWertebestellungFristVersaeumt",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data carried from the Anfrage through the whole process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EsaWertebestellungData {
    /// GLN of this ESA.
    pub esa: MarktpartnerCode,
    /// GLN of the MSB.
    pub msb: MarktpartnerCode,
    /// Location level requested.
    pub ebene: Lokationsebene,
    /// MaLo-ID, ZPB or NeLo-ID.
    pub lokations_id: String,
    /// Belegnummer of the ORDERS Bestellung this ESA sent. A later ORDCHG
    /// Stornierung references it (`RFF+ON`) so the MSB can correlate the
    /// cancellation — an ORDCHG carries no LOC.
    #[serde(default)]
    pub bestellung_ref: Option<String>,
}

/// State of an ESA-origination Wertebestellung process.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum EsaWertebestellungState {
    /// No events yet.
    #[default]
    New,
    /// REQOTE sent; the ESA awaits an Angebot within 5 WT.
    AnfrageGesendet(Box<EsaWertebestellungData>),
    /// Angebot received; the ESA may order until the Bindungsfrist lapses.
    AngebotErhalten {
        /// Process data.
        data: Box<EsaWertebestellungData>,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
    },
    /// Bestellung sent; the ESA awaits an ORDRSP within 2 WT.
    BestellungGesendet(Box<EsaWertebestellungData>),
    /// Bestellung confirmed — delivery is authorised and may be running.
    Beliefert {
        /// Process data.
        data: Box<EsaWertebestellungData>,
        /// `true` once the first values arrived, which closes the Stornierung
        /// window (UC 4.3 Vorbedingung).
        lieferung_begonnen: bool,
    },
    /// Stornierung sent; the ESA awaits an ORDRSP 19013/19014 within 2 WT.
    StornierungGesendet(Box<EsaWertebestellungData>),
    /// Abbestellung sent; the ESA awaits an ORDRSP 19011 within 2 WT.
    AbbestellungGesendet(Box<EsaWertebestellungData>),
    /// Order cancelled before delivery began.
    Storniert(Box<EsaWertebestellungData>),
    /// Delivery ended (Abbestellung confirmed).
    Beendet(Box<EsaWertebestellungData>),
    /// Terminal rejection (Anfrage timed out or Bestellung refused).
    Abgelehnt {
        /// Reason.
        reason: String,
    },
}

impl EsaWertebestellungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet(_) => "AnfrageGesendet",
            Self::AngebotErhalten { .. } => "AngebotErhalten",
            Self::BestellungGesendet(_) => "BestellungGesendet",
            Self::Beliefert { .. } => "Beliefert",
            Self::StornierungGesendet(_) => "StornierungGesendet",
            Self::AbbestellungGesendet(_) => "AbbestellungGesendet",
            Self::Storniert(_) => "Storniert",
            Self::Beendet(_) => "Beendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }

    /// `true` when delivery to the ESA is authorised (a confirmed Bestellung).
    #[must_use]
    pub const fn beliefert(&self) -> bool {
        matches!(self, Self::Beliefert { .. } | Self::AbbestellungGesendet(_))
    }

    /// Process data, when the process has advanced past `New`.
    #[must_use]
    pub const fn data(&self) -> Option<&EsaWertebestellungData> {
        match self {
            Self::AnfrageGesendet(d)
            | Self::BestellungGesendet(d)
            | Self::StornierungGesendet(d)
            | Self::AbbestellungGesendet(d)
            | Self::Storniert(d)
            | Self::Beendet(d) => Some(d),
            Self::AngebotErhalten { data, .. } | Self::Beliefert { data, .. } => Some(data),
            Self::New | Self::Abgelehnt { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the ESA-origination workflow.
#[derive(Clone)]
pub enum EsaWertebestellungCommand {
    /// Originate REQOTE 35002 (UC 4.1 Nr. 1). Consent-gated at the makod
    /// boundary (`esa_outbound`) before it reaches this workflow.
    SendWerteanfrage {
        /// GLN of this ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB addressed.
        msb: MarktpartnerCode,
        /// Location level.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB or NeLo-ID.
        lokations_id: String,
        /// Reference of the outbound REQOTE.
        message_ref: MessageRef,
    },
    /// QUOTES 15003 Angebot received (UC 4.1 Nr. 2).
    ReceiveAngebot {
        /// Reference of the inbound QUOTES.
        message_ref: MessageRef,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
    },
    /// QUOTES 15003 Ablehnung received — the MSB refused the Anfrage.
    ReceiveAnfrageAblehnung {
        /// Reason communicated by the MSB.
        reason: Option<String>,
    },
    /// Originate ORDERS 17007 Bestellung (UC 4.1 Nr. 3). Consent-gated.
    SendBestellung {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19011 received — a **Bestätigung** of the Bestellung (UC 4.1 Nr. 4)
    /// or, once running, of the Abbestellung (UC 4.3 Nr. 2). One PID, resolved
    /// against the current state.
    ReceiveBestaetigung {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// ORDRSP 19012 received — an **Ablehnung** of the Bestellung (ends the
    /// process) or of the Abbestellung (delivery continues). Resolved against
    /// the current state.
    ReceiveAblehnung {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// Reason communicated by the MSB.
        reason: Option<String>,
    },
    /// Originate ORDCHG 39002 Stornierung (UC 4.1 Nr. 5) before delivery began.
    SendStornierung {
        /// Reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19013/19014 received answering the Stornierung (UC 4.1 Nr. 6).
    ReceiveStornierungAntwort {
        /// Prüfidentifikator of the inbound ORDRSP (19013 or 19014).
        pid: Pruefidentifikator,
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// Reason, present on rejection (19014).
        reason: Option<String>,
    },
    /// Originate ORDERS 17008 Abbestellung (UC 4.3 Nr. 1) — the GDPR Art. 7(3)
    /// revocation path. **Not** consent-gated: it is the act of stopping.
    SendAbbestellung {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// Date delivery is to stop.
        beendigung_zum: OffsetDateTime,
        /// Trigger — typically `einwilligung_widerrufen`.
        grund: String,
    },
    /// Mark the first values as delivered, closing the Stornierung window.
    MarkLieferungBegonnen,
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for EsaWertebestellungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// ESA-origination Wertebestellung workflow (WiM Strom Teil 2, Kapitel 4).
pub struct EsaWertebestellungWorkflow;

fn require_pid(pid: Pruefidentifikator, allowed: &[u32], what: &str) -> Result<(), WorkflowError> {
    if allowed.contains(&pid.as_u32()) {
        Ok(())
    } else {
        Err(WorkflowError::rejected(format!(
            "{what} erwartet PID {allowed:?}, erhielt {pid}"
        )))
    }
}

impl Workflow for EsaWertebestellungWorkflow {
    type State = EsaWertebestellungState;
    type Event = EsaWertebestellungEvent;
    type Command = EsaWertebestellungCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        use EsaWertebestellungEvent as E;
        use EsaWertebestellungState as S;
        match event {
            E::AnfrageGesendet {
                esa,
                msb,
                ebene,
                lokations_id,
                ..
            } => S::AnfrageGesendet(Box::new(EsaWertebestellungData {
                esa: esa.clone(),
                msb: msb.clone(),
                ebene: *ebene,
                lokations_id: lokations_id.clone(),
                bestellung_ref: None,
            })),
            E::AngebotErhalten { bindungsfrist, .. } => match state {
                S::AnfrageGesendet(data) => S::AngebotErhalten {
                    data,
                    bindungsfrist: *bindungsfrist,
                },
                other => other,
            },
            E::AnfrageAbgelehnt { reason } => match state {
                S::AnfrageGesendet(_) => S::Abgelehnt {
                    reason: reason.clone(),
                },
                other => other,
            },
            E::BestellungGesendet { message_ref } => match state {
                S::AngebotErhalten { mut data, .. } => {
                    data.bestellung_ref = Some(message_ref.as_str().to_owned());
                    S::BestellungGesendet(data)
                }
                other => other,
            },
            E::BestellungBestaetigt { .. } => match state {
                S::BestellungGesendet(data) => S::Beliefert {
                    data,
                    lieferung_begonnen: false,
                },
                other => other,
            },
            E::BestellungAbgelehnt { reason } => S::Abgelehnt {
                reason: reason.clone(),
            },
            E::StornierungGesendet { .. } => match state {
                S::Beliefert { data, .. } => S::StornierungGesendet(data),
                other => other,
            },
            E::StornierungBestaetigt { .. } => match state {
                S::StornierungGesendet(data) => S::Storniert(data),
                other => other,
            },
            // A refused Stornierung leaves the delivery running.
            E::StornierungAbgelehnt { .. } => match state {
                S::StornierungGesendet(data) => S::Beliefert {
                    data,
                    lieferung_begonnen: false,
                },
                other => other,
            },
            E::AbbestellungGesendet { .. } => match state {
                S::Beliefert { data, .. } => S::AbbestellungGesendet(data),
                other => other,
            },
            E::AbbestellungBestaetigt { .. } => match state {
                S::AbbestellungGesendet(data) => S::Beendet(data),
                other => other,
            },
            // A refused Abbestellung leaves delivery running.
            E::AbbestellungAbgelehnt { .. } => match state {
                S::AbbestellungGesendet(data) => S::Beliefert {
                    data,
                    lieferung_begonnen: true,
                },
                other => other,
            },
            E::LieferungBegonnen => match state {
                S::Beliefert { data, .. } => S::Beliefert {
                    data,
                    lieferung_begonnen: true,
                },
                other => other,
            },
            E::FristVersaeumt { .. } => match state {
                // Only an outstanding Angebot turns into a terminal rejection;
                // a missed ORDRSP is a process anomaly surfaced by the event but
                // does not collapse an authorised delivery.
                S::AnfrageGesendet(_) => S::Abgelehnt {
                    reason: "Angebot nicht innerhalb der Frist erhalten".to_owned(),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // Build the outbound render intent that puts a message on the wire to
        // the MSB. The renderer turns this into REQOTE/ORDERS/ORDCHG with the
        // PID in BGM DE 1004 and the location in LOC.
        fn esa_send(
            message_type: &'static str,
            pid: u32,
            data: &EsaWertebestellungData,
            message_ref: &MessageRef,
            order_reference: Option<&str>,
        ) -> PendingOutbox {
            PendingOutbox::new(
                message_type,
                data.msb.as_str(),
                serde_json::json!({
                    "pid": pid,
                    "sender": data.esa.as_str(),
                    "receiver": data.msb.as_str(),
                    "message_ref": message_ref.as_str(),
                    "location": data.lokations_id,
                    // The ORDCHG Stornierung carries no LOC; it references the
                    // original Bestellung's Belegnummer in `RFF+ON` instead so
                    // the MSB can correlate it.
                    "order_reference": order_reference,
                }),
            )
        }

        use EsaWertebestellungCommand as C;
        use EsaWertebestellungEvent as E;
        use EsaWertebestellungState as S;

        match command {
            C::SendWerteanfrage {
                esa,
                msb,
                ebene,
                lokations_id,
                message_ref,
            } => {
                if !matches!(state, S::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if lokations_id.trim().is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "Werteanfrage auf Ebene {} ohne Lokations-ID",
                        ebene.as_str()
                    )));
                }
                let data = EsaWertebestellungData {
                    esa: esa.clone(),
                    msb: msb.clone(),
                    ebene,
                    lokations_id: lokations_id.clone(),
                    bestellung_ref: None,
                };
                let outbox = esa_send("REQOTE", ANFRAGE_PID, &data, &message_ref, None);
                // The MSB owes an Angebot within 5 WT; arm the window from now
                // (the AS4 Receipt for our REQOTE is issued in the same request).
                let due = mako_engine::fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    super::wertebestellung::ANGEBOT_FRIST_WT,
                    mako_engine::fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::AnfrageGesendet {
                        esa,
                        msb,
                        ebene,
                        lokations_id,
                        message_ref,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANGEBOT_WINDOW_LABEL, due)],
                })
            }

            C::ReceiveAngebot {
                message_ref,
                bindungsfrist,
            } => {
                if !matches!(state, S::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                Ok(WorkflowOutput {
                    events: vec![E::AngebotErhalten {
                        message_ref,
                        bindungsfrist,
                    }],
                    outbox: Vec::new(),
                    deadlines: vec![PendingDeadline::new(BINDUNGSFRIST_LABEL, bindungsfrist)],
                })
            }

            C::ReceiveAnfrageAblehnung { reason } => {
                if !matches!(state, S::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                Ok(WorkflowOutput::events(vec![E::AnfrageAbgelehnt {
                    reason: reason.unwrap_or_else(|| "Anfrage vom MSB abgelehnt".to_owned()),
                }]))
            }

            C::SendBestellung { message_ref } => {
                let S::AngebotErhalten {
                    data,
                    bindungsfrist,
                } = state
                else {
                    return Err(WorkflowError::invalid_state(
                        "AngebotErhalten",
                        state.label(),
                    ));
                };
                // UC 4.1 Nr. 3: order only within the MSB's Bindungsfrist.
                if OffsetDateTime::now_utc() > *bindungsfrist {
                    return Err(WorkflowError::rejected(format!(
                        "Bindungsfrist des Angebots endete am {bindungsfrist}"
                    )));
                }
                let outbox = esa_send("ORDERS", BESTELLUNG_PID, data, &message_ref, None);
                let due = mako_engine::fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    ANTWORT_FRIST_WT,
                    mako_engine::fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::BestellungGesendet { message_ref }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::ReceiveBestaetigung { message_ref } => match state {
                // ORDRSP 19011 confirms the Bestellung → delivery authorised.
                S::BestellungGesendet(_) => {
                    Ok(WorkflowOutput::events(vec![E::BestellungBestaetigt {
                        message_ref,
                    }]))
                }
                // ORDRSP 19011 confirms the Abbestellung → delivery ended.
                S::AbbestellungGesendet(_) => {
                    Ok(WorkflowOutput::events(vec![E::AbbestellungBestaetigt {
                        message_ref,
                    }]))
                }
                _ => Err(WorkflowError::invalid_state(
                    "BestellungGesendet|AbbestellungGesendet",
                    state.label(),
                )),
            },

            C::ReceiveAblehnung {
                message_ref: _,
                reason,
            } => match state {
                // ORDRSP 19012 refuses the Bestellung → the process ends.
                S::BestellungGesendet(_) => {
                    Ok(WorkflowOutput::events(vec![E::BestellungAbgelehnt {
                        reason: reason.unwrap_or_else(|| "ohne Begründung".to_owned()),
                    }]))
                }
                // ORDRSP 19012 refuses the Abbestellung → delivery continues.
                S::AbbestellungGesendet(_) => {
                    Ok(WorkflowOutput::events(vec![E::AbbestellungAbgelehnt {
                        reason: reason.unwrap_or_else(|| "ohne Begründung".to_owned()),
                    }]))
                }
                _ => Err(WorkflowError::invalid_state(
                    "BestellungGesendet|AbbestellungGesendet",
                    state.label(),
                )),
            },

            C::SendStornierung { message_ref } => {
                let S::Beliefert {
                    data,
                    lieferung_begonnen,
                } = state
                else {
                    return Err(WorkflowError::invalid_state("Beliefert", state.label()));
                };
                if *lieferung_begonnen {
                    return Err(WorkflowError::rejected(
                        "Stornierung ist nach Lieferbeginn nicht mehr möglich \
                         (UC 4.3 Vorbedingung) — nutze die Abbestellung (17008)",
                    ));
                }
                let outbox = esa_send(
                    "ORDCHG",
                    STORNIERUNG_PID,
                    data,
                    &message_ref,
                    data.bestellung_ref.as_deref(),
                );
                let due = mako_engine::fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    ANTWORT_FRIST_WT,
                    mako_engine::fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::StornierungGesendet { message_ref }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::ReceiveStornierungAntwort {
                pid,
                message_ref,
                reason,
            } => {
                if !matches!(state, S::StornierungGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "StornierungGesendet",
                        state.label(),
                    ));
                }
                require_pid(
                    pid,
                    &[STORNO_BESTAETIGUNG_PID, STORNO_ABLEHNUNG_PID],
                    "Antwort auf Stornierung",
                )?;
                if pid.as_u32() == STORNO_BESTAETIGUNG_PID {
                    Ok(WorkflowOutput::events(vec![E::StornierungBestaetigt {
                        message_ref,
                    }]))
                } else {
                    Ok(WorkflowOutput::events(vec![E::StornierungAbgelehnt {
                        reason: reason.unwrap_or_else(|| "ohne Begründung".to_owned()),
                    }]))
                }
            }

            C::SendAbbestellung {
                message_ref,
                beendigung_zum,
                grund,
            } => {
                let S::Beliefert { data, .. } = state else {
                    return Err(WorkflowError::invalid_state("Beliefert", state.label()));
                };
                // UC 4.3 Nr. 1: the Abbestellung (17008) ends a running delivery.
                let outbox = esa_send("ORDERS", ABBESTELLUNG_PID, data, &message_ref, None);
                let due = mako_engine::fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    ANTWORT_FRIST_WT,
                    mako_engine::fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::AbbestellungGesendet {
                        message_ref,
                        beendigung_zum,
                        grund,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::MarkLieferungBegonnen => {
                if !matches!(state, S::Beliefert { .. }) {
                    return Err(WorkflowError::invalid_state("Beliefert", state.label()));
                }
                Ok(WorkflowOutput::events(vec![E::LieferungBegonnen]))
            }

            C::TimeoutExpired { label, .. } => {
                Ok(WorkflowOutput::events(vec![E::FristVersaeumt {
                    label: label.to_string(),
                }]))
            }
        }
    }
}
