//! GPKE Teil 3 — the NB's Einrichtung der Konfigurationen aufgrund einer
//! Zuordnung eines LF (ORDERS 17134).
//!
//! After a Zuordnung of a LF the NB tells the MSB which Konfigurationen to set
//! up, with ORDERS 17134 „NB an MSB" (ORDERS AHB 1.1b Kap. 4.17.1, S. 102). A
//! MSB that does not operate every Messlokation forwards the order to the
//! weiterer MSB as 17135 „MSB an MSB" (Kap. 4.17.2, S. 111).
//!
//! # No answer message
//!
//! Neither ORDERS is answered by an ORDRSP: the MSB sets the Konfigurationen up
//! — „spätestens am WT vor dem Beginn des Wirkungszeitraums bis 22:00 Uhr"
//! where that lies in the future — and reports through the
//! Stammdatenänderung vom MSB (GPKE Teil 3 Kap. 1.3.7.2 Nr. 1–3, S. 82–84).
//! ORDRSP 19001/19002 answer the WiM Geräteübernahme only (ORDRSP AHB 1.1b Kap.
//! 4.9). So the NB's process ends when its 17134 is sent: it waits for no
//! message and registers no window.
//!
//! The receiving MSB runs [`crate::konfiguration_aenderung`]'s receiving side.
//!
//! # Refused until its facts have a source
//!
//! The 17134 column makes Muss facts no source of the NB carries — the
//! Lieferrichtung, the Lieferant and MSB of the Marktlokation, and per Lokation
//! the erforderlichen Produkte with their Zählzeitdefinition (ORDERS AHB 1.1b
//! Kap. 4.17.1, S. 101–110). `NbSendsBeauftragung` refuses with
//! [`MISSING_COLUMN_FACTS`] rather than queue an ORDERS the renderer and the
//! pre-send gate would refuse.

use std::collections::HashMap;

use mako_engine::{
    envelope::{EventEnvelope, Richtung, Verlauf},
    error::WorkflowError,
    projection::Projection,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-konfiguration";

/// The Einrichtung der Konfigurationen aufgrund einer Zuordnung eines LF:
/// 17134 „NB an MSB" (ORDERS AHB 1.1b Kap. 4.17.1, S. 102) and 17135 „MSB an
/// MSB" (Kap. 4.17.2, S. 111) — the NB sends 17134, a MSB that does not
/// operate every Messlokation forwards 17135 to the weiterer MSB (GPKE Teil 3
/// Kap. 1.3.7.2 Nr. 2, S. 84). The recipient MSB runs
/// `gpke-konfiguration-aenderung`'s receiving side.
pub const ORDERS_PIDS: &[u32] = &[17134, 17135];

/// Why no 17134 is queued: ORDERS AHB 1.1b Kap. 4.17.1 (S. 101–110) makes Muss
/// the Lieferrichtung (`IMD`), the Lieferant and the MSB of the Marktlokation
/// (`SG2 NAD`), and per Lokation the erforderlichen Produkte (`SG29 LIN`/`PIA`)
/// with their Zählzeitdefinition (`SG30 CCI`) — facts no source of the NB
/// carries.
pub const MISSING_COLUMN_FACTS: &str = "ORDERS 17134 is not sent: its column (ORDERS AHB 1.1b \
     Kap. 4.17.1, S. 101–110) requires the Lieferrichtung, the Lieferant and the MSB of the \
     Marktlokation and, per Lokation, the erforderlichen Produkte (SG29 LIN/PIA) with their \
     Zählzeitdefinition (SG30 CCI), and no source of the NB carries them";

/// The ORDERS the NB sends here: 17134 „NB an MSB".
pub const NB_ORDERS_PID: u32 = 17_134;

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the NB's Einrichtung der Konfigurationen.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KonfigurationEvent {
    /// NB sent ORDERS 17134 to the MSB.
    BeauftragungGesendet {
        /// Prüfidentifikator of the sent ORDERS (17134).
        orders_pid: Pruefidentifikator,
        /// MP-ID of the MSB that received the ORDERS.
        msb_mp_id: MarktpartnerCode,
        /// The Marktlokation.
        malo: MaLo,
        /// MP-ID of the new supplier (LFN).
        new_supplier: MarktpartnerCode,
        /// Reference of the sent ORDERS.
        message_ref: MessageRef,
    },
}

impl EventPayload for KonfigurationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::BeauftragungGesendet { .. } => "KonfigurationBeauftragungGesendet",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::BeauftragungGesendet {
                orders_pid,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(orders_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set when `BeauftragungGesendet` is applied.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BeauftragungData {
    /// Prüfidentifikator of the sent ORDERS (17134).
    pub orders_pid: Pruefidentifikator,
    /// MP-ID of the addressed MSB.
    pub msb_mp_id: MarktpartnerCode,
    /// The Marktlokation.
    pub malo: MaLo,
    /// MP-ID of the new supplier (LFN).
    pub new_supplier: MarktpartnerCode,
    /// Reference of the sent ORDERS.
    pub message_ref: MessageRef,
}

/// State of the NB's Einrichtung der Konfigurationen.
///
/// ```text
/// New → Beauftragt   (17134 sent; nothing answers it)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum KonfigurationState {
    /// No ORDERS sent yet.
    #[default]
    New,
    /// ORDERS 17134 was sent — the NB's part is done.
    Beauftragt(BeauftragungData),
}

/// The Beauftragung holds its Marktlokation only until it is sent: no message
/// answers it, so a sent one never blocks the next.
impl mako_engine::workflow::OccupiesBusinessKey for KonfigurationState {
    fn occupies_business_key(&self) -> bool {
        false
    }
}

impl KonfigurationState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Beauftragt(_) => "Beauftragt",
        }
    }

    /// Whether the process has ended — once the 17134 is sent.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Beauftragt(_))
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the NB's Einrichtung der Konfigurationen.
#[derive(Clone)]
pub enum KonfigurationCommand {
    /// NB sends ORDERS 17134 to the MSB.
    NbSendsBeauftragung {
        /// Prüfidentifikator of the ORDERS (17134).
        orders_pid: Pruefidentifikator,
        /// MP-ID of the MSB receiving the ORDERS.
        msb_mp_id: MarktpartnerCode,
        /// The Marktlokation.
        malo: MaLo,
        /// MP-ID of the new supplier (LFN).
        new_supplier: MarktpartnerCode,
        /// Reference of the sent ORDERS.
        message_ref: MessageRef,
    },
}

impl CommandPayload for KonfigurationCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The NB's Einrichtung der Konfigurationen (ORDERS 17134, GPKE Teil 3 Kap.
/// 1.3.7).
pub struct GpkeKonfigurationWorkflow;

impl Workflow for GpkeKonfigurationWorkflow {
    type State = KonfigurationState;
    type Event = KonfigurationEvent;
    type Command = KonfigurationCommand;

    fn apply(_state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            KonfigurationEvent::BeauftragungGesendet {
                orders_pid,
                msb_mp_id,
                malo,
                new_supplier,
                message_ref,
            } => KonfigurationState::Beauftragt(BeauftragungData {
                orders_pid: *orders_pid,
                msb_mp_id: msb_mp_id.clone(),
                malo: malo.clone(),
                new_supplier: new_supplier.clone(),
                message_ref: message_ref.clone(),
            }),
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            KonfigurationCommand::NbSendsBeauftragung { orders_pid, .. } => {
                if !matches!(state, KonfigurationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if orders_pid.as_u32() != NB_ORDERS_PID {
                    return Err(WorkflowError::rejected(format!(
                        "the NB sends ORDERS {NB_ORDERS_PID}; 17135 is the MSB's — got {orders_pid}",
                    )));
                }
                // The 17134 column makes Muss what this command does not
                // carry and the NB holds no source for; an ORDERS without them
                // is refused by the renderer and the pre-send gate, so none is
                // queued.
                Err(WorkflowError::rejected(MISSING_COLUMN_FACTS))
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single Konfigurationseinrichtung process stream.
#[derive(Debug)]
pub struct KonfigurationRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// MSB GLN once `BeauftragungGesendet` is applied.
    pub msb_mp_id: Option<MarktpartnerCode>,
    /// MaLo once `BeauftragungGesendet` is applied.
    pub malo: Option<MaLo>,
    /// Total events processed.
    pub event_count: usize,
}

impl Default for KonfigurationRecord {
    fn default() -> Self {
        Self {
            status: "New",
            msb_mp_id: None,
            malo: None,
            event_count: 0,
        }
    }
}

/// In-process read model for all GPKE Konfigurationseinrichtung streams.
#[derive(Debug, Default)]
pub struct KonfigurationProjection {
    /// All known Konfiguration process records keyed by stream ID.
    pub records: HashMap<String, KonfigurationRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for KonfigurationProjection {
    fn name(&self) -> &'static str {
        "KonfigurationProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<KonfigurationEvent>() else {
            return;
        };

        match event {
            KonfigurationEvent::BeauftragungGesendet {
                msb_mp_id, malo, ..
            } => {
                record.status = "Beauftragt";
                record.msb_mp_id = Some(msb_mp_id);
                record.malo = Some(malo);
            }
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        if self.last_seq == 0 {
            None
        } else {
            Some(self.last_seq)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beauftragung(pid: u32) -> KonfigurationCommand {
        KonfigurationCommand::NbSendsBeauftragung {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            msb_mp_id: MarktpartnerCode::new("9900357000004"),
            malo: MaLo::new("51238696012"),
            new_supplier: MarktpartnerCode::new("9900357000011"),
            message_ref: MessageRef::new("ORD-1"),
        }
    }

    /// The 17134 is refused before anything is queued: no window, no event,
    /// no ORDERS the renderer would refuse.
    #[test]
    fn the_17134_is_refused_before_it_is_queued() {
        assert!(GpkeKonfigurationWorkflow::DEADLINE_LABELS.is_empty());
        let err = GpkeKonfigurationWorkflow::handle(&KonfigurationState::New, beauftragung(17_134))
            .expect_err("refused");
        assert!(err.to_string().contains("Zählzeitdefinition"), "{err}");
    }

    /// 17135 is the MSB's forward to the weiterer MSB, not the NB's.
    #[test]
    fn the_nb_does_not_send_17135() {
        assert!(
            GpkeKonfigurationWorkflow::handle(&KonfigurationState::New, beauftragung(17_135))
                .is_err()
        );
    }
}
