//! Redispatch 2.0 XML ingest — the AS4 XML leg of the transport boundary.
//!
//! Mirrors the EDIFACT pipeline (`edi-energy` → `PidRouter` → dispatcher):
//! `redispatch-xml` parses and validates the nine BDEW document types, the
//! canonical [`document_kind`] mapping picks the workflow, and the same
//! `spawn_or_resume` machinery executes the command — with the regulatory
//! deadlines registered atomically at spawn:
//!
//! | Document | Command | Deadlines at spawn |
//! |---|---|---|
//! | `ActivationDocument` (ACO) | `ReceiveAco` | **5-min response window** (BK6-20-060) + 6h ACK |
//! | `Stammdaten` | `Receive` | 6h ACK + 1-Werktag forward |
//! | 6 ack-forward documents | `Receive` | their 6h/24h ack windows |
//! | `AcknowledgementDocument` | `ReceiveAck` via correlation | — |
//!
//! The ACO's `Abwicklung` defaults to **Aufforderungsfall/Sollwert** — the
//! strict case (response window enforced). Resolving the Duldungsfall from
//! the resource's Stammdaten tightens this later; defaulting to the lenient
//! case would silently disable a hard real-time deadline.

use mako_engine::error::EngineError;
use mako_redispatch::aktivierung::{
    ACK_WINDOW_LABEL as AKT_ACK_WINDOW, ACTIVATION_RESPONSE_WINDOW_LABEL, Abrufart, Abwicklung,
    AktivierungCommand, AktivierungWorkflow,
};
use mako_redispatch::stammdaten::{
    ACK_WINDOW_LABEL as SD_ACK_WINDOW, FORWARD_WINDOW_LABEL as SD_FORWARD_WINDOW,
    StammdatenCommand, StammdatenWorkflow,
};
use mako_redispatch::{RedispatchDocumentKind, ack_forward};
use redispatch_xml::Document;
use redispatch_xml::documents::DocumentType;
use time::{Duration, OffsetDateTime};

use crate::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

/// Canonical `DocumentType → RedispatchDocumentKind` mapping.
///
/// Lives here — at the transport boundary — because makod is the only crate
/// that depends on both halves; the engine stays format-agnostic (like
/// mako-gpke/mako-wim/mako-mabis vs. `edi-energy`). Exhaustive by
/// construction: a tenth document type in `redispatch-xml` fails compilation
/// here instead of silently never routing.
#[must_use]
pub fn document_kind(dt: DocumentType) -> RedispatchDocumentKind {
    match dt {
        DocumentType::Activation => RedispatchDocumentKind::Activation,
        DocumentType::PlannedResourceSchedule => RedispatchDocumentKind::PlannedResourceSchedule,
        DocumentType::Acknowledgement => RedispatchDocumentKind::Acknowledgement,
        DocumentType::Stammdaten => RedispatchDocumentKind::Stammdaten,
        DocumentType::StatusRequest => RedispatchDocumentKind::StatusRequest,
        DocumentType::Unavailability => RedispatchDocumentKind::Unavailability,
        DocumentType::Kaskade => RedispatchDocumentKind::Kaskade,
        DocumentType::NetworkConstraint => RedispatchDocumentKind::NetworkConstraint,
        DocumentType::Kostenblatt => RedispatchDocumentKind::Kostenblatt,
    }
}

/// Sniff: does this AS4 payload look like an XML document (vs EDIFACT)?
#[must_use]
pub fn looks_like_xml(payload: &[u8]) -> bool {
    payload
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'<')
}

/// Parse, validate, and dispatch one Redispatch XML document.
///
/// # Errors
///
/// Engine errors bubble up; parse/validation failures are returned as
/// `IngestOutcome::Skipped` with the reason so the AS4 handler can
/// dead-letter them (the document was received — it must not vanish).
pub async fn dispatch_redispatch_xml(
    dispatcher: &EdifactIngestDispatcher,
    payload: &[u8],
) -> Result<IngestOutcome, EngineError> {
    let doc = match redispatch_xml::parse_and_validate(payload) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "redispatch-xml ingest: parse/validation failed");
            return Ok(IngestOutcome::Skipped {
                workflow_name: "redispatch-xml",
                reason: "xml_parse_or_validation_failed",
            });
        }
    };
    let kind = document_kind(doc.document_type());
    let now = OffsetDateTime::now_utc();

    match (&doc, kind) {
        (Document::Activation(d), _) => {
            let mrid = d.document_identification.v.as_str().to_owned();
            let ts = d.time_series.first();
            // Ordered MW: the maximum quarter-hour quantity of the first
            // time series — the activation's peak instruction.
            let ordered_mw = ts
                .and_then(|ts| {
                    ts.period
                        .intervals
                        .iter()
                        .map(|i| i.qty.v.value())
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                })
                .unwrap_or(0.0);
            let resource_id = ts
                .map(|ts| ts.resource_object.v.clone())
                .unwrap_or_default();
            let cmd = AktivierungCommand::ReceiveAco {
                mrid: mrid.clone(),
                // Strict default — see module docs.
                abwicklung: Abwicklung::Aufforderungsfall {
                    abrufart: Abrufart::Sollwert,
                },
                ordered_mw,
                resource_id,
                period: d.activation_time_interval.v.to_string(),
                sender: d.sender_identification.v.as_str().to_owned(),
                receiver: d.receiver_identification.v.as_str().to_owned(),
                received_at: now
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            };
            dispatcher
                .spawn_or_resume_redispatch::<AktivierungWorkflow>(
                    &mrid,
                    "redispatch-aktivierung",
                    cmd,
                    &[
                        (ACTIVATION_RESPONSE_WINDOW_LABEL, now + Duration::minutes(5)),
                        (AKT_ACK_WINDOW, now + Duration::hours(6)),
                    ],
                )
                .await
        }

        (Document::Acknowledgement(d), _) => {
            let Some(recv_id) = d
                .receiving_document_identification
                .as_ref()
                .map(|a| a.v.as_str().to_owned())
            else {
                return Ok(IngestOutcome::Skipped {
                    workflow_name: "redispatch-aktivierung",
                    reason: "ack_without_receiving_document_identification",
                });
            };
            let cmd = AktivierungCommand::ReceiveAck {
                ack_mrid: d.document_identification.v.as_str().to_owned(),
                acknowledged_mrid: recv_id.clone(),
                reason_code: String::new(),
            };
            // Correlation delivery: the process is registered under the MRID
            // of the document being acknowledged.
            dispatcher
                .resume_redispatch::<AktivierungWorkflow>(&recv_id, "redispatch-aktivierung", cmd)
                .await
        }

        (Document::Stammdaten(d), _) => {
            let mrid = d.document_identification.as_str().to_owned();
            let cmd = StammdatenCommand::Receive {
                mrid: mrid.clone(),
                sender: d.sender.code.as_str().to_owned(),
                receiver: d.empfaenger.code.as_str().to_owned(),
                doc_type: format!("{:?}", d.document_type),
                anlagen_count: u32::try_from(d.sr_objekte.len()).unwrap_or(u32::MAX),
                received_at: now
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            };
            dispatcher
                .spawn_or_resume_redispatch::<StammdatenWorkflow>(
                    &mrid,
                    "redispatch-stammdaten",
                    cmd,
                    &[
                        (SD_ACK_WINDOW, now + Duration::hours(6)),
                        // 1 Werktag ≈ next-day floor; the Werktage calendar
                        // refinement lives with the scheduler.
                        (SD_FORWARD_WINDOW, now + Duration::hours(24)),
                    ],
                )
                .await
        }

        // The six ack-forward documents share one command shape.
        (doc, kind) => {
            let (workflow_name, ack_label, ack_hours) = match kind {
                RedispatchDocumentKind::Unavailability => (
                    "redispatch-verfuegbarkeit",
                    "redispatch-verfuegbarkeit-ack-window",
                    6,
                ),
                RedispatchDocumentKind::NetworkConstraint => (
                    "redispatch-netzengpass",
                    "redispatch-netzengpass-ack-window",
                    6,
                ),
                RedispatchDocumentKind::Kaskade => {
                    ("redispatch-kaskade", "redispatch-kaskade-ack-window", 6)
                }
                RedispatchDocumentKind::PlannedResourceSchedule => (
                    "redispatch-planungsdaten",
                    "redispatch-planungsdaten-ack-window",
                    6,
                ),
                RedispatchDocumentKind::StatusRequest => (
                    "redispatch-statusanfrage",
                    "redispatch-statusanfrage-response-window",
                    24,
                ),
                RedispatchDocumentKind::Kostenblatt => (
                    "redispatch-kostenblatt",
                    "redispatch-kostenblatt-ack-window",
                    6,
                ),
                _ => {
                    return Ok(IngestOutcome::Skipped {
                        workflow_name: "redispatch-xml",
                        reason: "unroutable_document_kind",
                    });
                }
            };
            let mrid = doc.mrid().to_owned();
            let cmd = ack_forward::AckForwardCommand::Receive {
                mrid: mrid.clone(),
                doc_type: format!("{kind:?}"),
                sender: doc.sender_id().to_owned(),
                receiver: doc.receiver_id().to_owned(),
                received_at: now
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            };
            dispatch_ack_forward(
                dispatcher,
                kind,
                &mrid,
                workflow_name,
                cmd,
                ack_label,
                ack_hours,
            )
            .await
        }
    }
}

/// Monomorphised dispatch for the six macro-generated ack-forward workflows.
async fn dispatch_ack_forward(
    dispatcher: &EdifactIngestDispatcher,
    kind: RedispatchDocumentKind,
    key: &str,
    workflow_name: &'static str,
    cmd: ack_forward::AckForwardCommand,
    ack_label: &'static str,
    ack_hours: i64,
) -> Result<IngestOutcome, EngineError> {
    let due = OffsetDateTime::now_utc() + Duration::hours(ack_hours);
    let deadlines = [(ack_label, due)];
    match kind {
        RedispatchDocumentKind::Unavailability => {
            dispatcher
                .spawn_or_resume_redispatch::<ack_forward::VerfuegbarkeitWorkflow>(
                    key,
                    workflow_name,
                    cmd,
                    &deadlines,
                )
                .await
        }
        RedispatchDocumentKind::NetworkConstraint => {
            dispatcher
                .spawn_or_resume_redispatch::<ack_forward::NetzengpassWorkflow>(
                    key,
                    workflow_name,
                    cmd,
                    &deadlines,
                )
                .await
        }
        RedispatchDocumentKind::Kaskade => {
            dispatcher
                .spawn_or_resume_redispatch::<ack_forward::KaskadeWorkflow>(
                    key,
                    workflow_name,
                    cmd,
                    &deadlines,
                )
                .await
        }
        RedispatchDocumentKind::PlannedResourceSchedule => {
            dispatcher
                .spawn_or_resume_redispatch::<ack_forward::PlanungsdatenWorkflow>(
                    key,
                    workflow_name,
                    cmd,
                    &deadlines,
                )
                .await
        }
        RedispatchDocumentKind::StatusRequest => {
            dispatcher
                .spawn_or_resume_redispatch::<ack_forward::StatusanfrageWorkflow>(
                    key,
                    workflow_name,
                    cmd,
                    &deadlines,
                )
                .await
        }
        RedispatchDocumentKind::Kostenblatt => {
            dispatcher
                .spawn_or_resume_redispatch::<ack_forward::KostenblattWorkflow>(
                    key,
                    workflow_name,
                    cmd,
                    &deadlines,
                )
                .await
        }
        _ => Ok(IngestOutcome::Skipped {
            workflow_name: "redispatch-xml",
            reason: "unroutable_document_kind",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_sniff_distinguishes_xml_from_edifact() {
        assert!(looks_like_xml(
            b"<?xml version=\"1.0\"?><ActivationDocument/>"
        ));
        assert!(looks_like_xml(b"  \n<Stammdaten/>"));
        assert!(!looks_like_xml(b"UNB+UNOC:3+9900123..."));
        assert!(!looks_like_xml(b""));
    }
}
