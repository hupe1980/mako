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
//! | `ActivationDocument` (ACO) | `ReceiveAco` | response window (Betreiberfrist) + [`ACK_FRIST`] |
//! | `Stammdaten` | `Receive` | [`ACK_FRIST`] + forward window (Betreiberfrist) |
//! | 6 ack-forward documents | `Receive` | [`ACK_FRIST`] |
//! | `AcknowledgementDocument` | `ReceiveAck` via correlation | — |
//!
//! # Where the numbers come from
//!
//! Every acknowledgement window is [`ACK_FRIST`] — **3 minutes**, „unverzüglich,
//! jedoch spätestens 3 Minuten nach Erhalt der Übertragungsdatei"
//! (`AcknowledgementDocument` FB 1.0g). This module armed 6 hours for five of
//! them and 24 hours for the Statusanfrage, so a late `AcknowledgementDocument`
//! was never once detected as late: the deadline fired hundreds of times later
//! than the obligation it was meant to watch, by which point the transfer file
//! it answered was long settled. The constant is read from `mako-redispatch`
//! rather than restated here, so the two cannot drift apart again.
//!
//! The 24 hours the Statusanfrage carried were not a second, longer ack window:
//! the historical 24-hour figure was the window for *answering* a Statusanfrage,
//! a different obligation on a different message, and the label armed here is
//! `statusanfrage::ACK_WINDOW_LABEL` — the acknowledgement. That answer window
//! has no published source under the current regime either (BK6-23-241 repealed
//! BK6-20-059 Tz. 1, BK6-20-060 and BK6-20-061; the replacing
//! Prozessbeschreibungen of Tz. 7 are not published), and `mako-redispatch`
//! carries no field for it, so nothing is armed for it here.
//!
//! The two windows that are *not* Fristen — the ACO response window and the
//! Stammdaten forward window — are [`Betreiberfristen`]: operator-configured,
//! defaulted to the repealed BK6-20-05x figures because that is what the market
//! ran on until 30.06.2026. They are read from `historisch()` so the code says
//! which they are; a deployment with its own Prozessbeschreibung replaces them.
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
use mako_redispatch::fristen::{ACK_FRIST, Betreiberfristen};
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

/// The windows that are the operator's own rather than a Frist.
///
/// `historisch()` is the BK6-20-05x default; the decisions that set those
/// figures are repealed (BK6-23-241 Tz. 1/3/4) and their replacements are not
/// published, so these are configuration wearing a documented default — not a
/// regulatory obligation, and deliberately not spelled as bare literals next to
/// the sourced [`ACK_FRIST`].
const BETREIBERFRISTEN: Betreiberfristen = Betreiberfristen::historisch();

/// When the `AcknowledgementDocument` for a transfer file received at `now` is
/// due.
///
/// One helper for all eight document types because it is one obligation: the
/// receiver of the Übertragungsdatei answers its *syntax* within [`ACK_FRIST`],
/// whatever business document it carried.
fn ack_deadline(now: OffsetDateTime) -> OffsetDateTime {
    now + ACK_FRIST
}

/// How long a VNB has to forward received `Stammdaten` upstream.
///
/// A Betreiberfrist counted in Werktage; taken here as calendar days, which is
/// the earliest it can fall due and therefore never later than the obligation.
fn stammdaten_forward_window() -> Duration {
    Duration::days(i64::from(
        BETREIBERFRISTEN.stammdaten_weiterleitung_werktage,
    ))
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
                    mako_redispatch::aktivierung::WORKFLOW_NAME,
                    cmd,
                    &[
                        (
                            ACTIVATION_RESPONSE_WINDOW_LABEL,
                            now + BETREIBERFRISTEN.aktivierung_antwort,
                        ),
                        (AKT_ACK_WINDOW, ack_deadline(now)),
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
                    workflow_name: mako_redispatch::aktivierung::WORKFLOW_NAME,
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
                .resume_redispatch::<AktivierungWorkflow>(
                    &recv_id,
                    mako_redispatch::aktivierung::WORKFLOW_NAME,
                    cmd,
                )
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
                    mako_redispatch::stammdaten::WORKFLOW_NAME,
                    cmd,
                    &[
                        (SD_ACK_WINDOW, ack_deadline(now)),
                        // Werktage as calendar days — a next-day floor. The
                        // Werktage calendar refinement lives with the scheduler.
                        (SD_FORWARD_WINDOW, now + stammdaten_forward_window()),
                    ],
                )
                .await
        }

        // The six ack-forward documents share one command shape.
        (doc, kind) => {
            // One window for all six: every one of these labels is an
            // `AcknowledgementDocument` window, and that Frist is the same
            // 3 minutes whatever document the transfer file carried.
            let (workflow_name, ack_label) = match kind {
                RedispatchDocumentKind::Unavailability => (
                    ack_forward::verfuegbarkeit::WORKFLOW_NAME,
                    ack_forward::verfuegbarkeit::ACK_WINDOW_LABEL,
                ),
                RedispatchDocumentKind::NetworkConstraint => (
                    ack_forward::netzengpass::WORKFLOW_NAME,
                    ack_forward::netzengpass::ACK_WINDOW_LABEL,
                ),
                RedispatchDocumentKind::Kaskade => (
                    ack_forward::kaskade::WORKFLOW_NAME,
                    ack_forward::kaskade::ACK_WINDOW_LABEL,
                ),
                RedispatchDocumentKind::PlannedResourceSchedule => (
                    ack_forward::planungsdaten::WORKFLOW_NAME,
                    ack_forward::planungsdaten::ACK_WINDOW_LABEL,
                ),
                RedispatchDocumentKind::StatusRequest => (
                    ack_forward::statusanfrage::WORKFLOW_NAME,
                    ack_forward::statusanfrage::ACK_WINDOW_LABEL,
                ),
                RedispatchDocumentKind::Kostenblatt => (
                    ack_forward::kostenblatt::WORKFLOW_NAME,
                    ack_forward::kostenblatt::ACK_WINDOW_LABEL,
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
            dispatch_ack_forward(dispatcher, kind, &mrid, workflow_name, cmd, ack_label).await
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
) -> Result<IngestOutcome, EngineError> {
    let due = ack_deadline(OffsetDateTime::now_utc());
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

    /// Every acknowledgement window is the sourced 3-minute Frist.
    ///
    /// This module armed `now + 6 hours` for five ack windows and `now + 24
    /// hours` for the Statusanfrage's, against a Frist of „unverzüglich, jedoch
    /// spätestens 3 Minuten" (`AcknowledgementDocument` FB 1.0g). A window 120×
    /// too long is not a lenient deadline, it is no deadline: the process it
    /// watches has been settled for hours by the time it fires, so a late
    /// `AcknowledgementDocument` was never once detected as late.
    #[test]
    fn every_acknowledgement_window_is_the_sourced_frist() {
        let now = OffsetDateTime::now_utc();
        assert_eq!(
            ack_deadline(now) - now,
            ACK_FRIST,
            "the window must be the ACK_FRIST constant, not a literal"
        );
        assert!(
            ack_deadline(now) - now <= Duration::minutes(3),
            "a window longer than 3 minutes cannot detect a late ack"
        );
    }

    /// No deadline in this module is a bare hour or minute literal.
    ///
    /// The value guard above only sees the helper. This one sees the call
    /// sites: a second window given a literal six-hour duration next to it
    /// would pass every other test in the crate, which is exactly how the
    /// 6-hour and 24-hour windows survived. The only durations this module may
    /// build itself are the Werktage of a [`Betreiberfristen`] field.
    #[test]
    fn no_deadline_in_this_module_is_a_bare_literal() {
        const SRC: &str = include_str!("redispatch_xml_ingest.rs");
        // Only the production half: the tests below name durations in order to
        // compare against them, which is the opposite of the mistake.
        let production = SRC.split("#[cfg(test)]").next().unwrap_or(SRC);
        // Assembled rather than written out, so the needle is not itself a hit.
        for unit in ["hours", "minutes", "seconds"] {
            let forbidden = &format!("Duration::{unit}(");
            assert!(
                !production.contains(forbidden),
                "{forbidden}…) restates a Frist mako-redispatch owns — arm ACK_FRIST or a Betreiberfristen field"
            );
        }
    }

    #[test]
    fn the_stammdaten_forward_window_is_the_operator_frist() {
        // Not a Frist: BK6-23-241 Tz. 4 repealed BK6-20-060 and `BilAReM`
        // Kap. 6.2.1.1 states the obligation without a countable window. The
        // number therefore has to come from the configurable Betreiberfrist,
        // so a deployment with its own Prozessbeschreibung changes one place.
        assert_eq!(
            stammdaten_forward_window(),
            Duration::days(i64::from(
                Betreiberfristen::historisch().stammdaten_weiterleitung_werktage
            ))
        );
    }

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
