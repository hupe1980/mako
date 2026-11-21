//! UTILTS renderer — the NB's Berechnungsformel (25001) and the MSB's Antwort
//! auf Berechnungsformel (25010), UTILTS AHB 1.1 / MIG 1.1e.
//!
//! A 25001 names, per Verwendungszeitraum, the formula's status (`STS+Z23`,
//! `Z33` attached or `Z40` no Rechenoperation), the Rechenschritt yielding the
//! Energiemenge (`SEQ+Z36`) and every Bestandteil (`SEQ+Z37`); a 25010 one
//! `E_0218` code per Zeitraum and the answered Vorgangsnummer in `RFF+TN`.

use super::*;

/// Render a UTILTS Vorgang from its [`UtiltsPayload`].
///
/// [`UtiltsPayload`]: mako_engine::payload::UtiltsPayload
pub(super) fn render_utilts(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "UTILTS";
    let p: mako_engine::payload::UtiltsPayload = super::typed_payload(p)?;
    let missing = |field: &'static str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };
    if !matches!(p.pid, 25_001 | 25_010) {
        return Err(RenderError::InsufficientPayload {
            message_type: mt.into(),
            detail: format!(
                "UTILTS {} has no renderer arm — only the Berechnungsformel and its Antwort are built",
                p.pid
            )
            .into(),
        });
    }
    let sender = super::sender_of(p.sender.as_deref(), registry, mt)?;
    let receiver = p.receiver.as_deref().unwrap_or(msg.recipient.as_ref());
    let release = release_for(msg, MessageType::Utilts, ReleaseTrack::Short)?;
    let message_ref = p
        .message_ref
        .as_deref()
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let mut vorgang = builders::UtiltsVorgang::new(&p.vorgangsnummer, p.pid);
    if p.pid == 25_001 {
        vorgang = vorgang.location(
            p.lokation
                .as_deref()
                .ok_or_else(|| missing("lokation (LOC+172)"))?,
        );
        if p.zeitraeume.is_empty() {
            return Err(missing("zeitraeume (SG6 RFF+Z49)"));
        }
        for z in &p.zeitraeume {
            vorgang = vorgang.formula_status(&z.status, z.id);
            let mut period = builders::UtiltsUsagePeriod::new(z.id, &z.ab).qualifier("Z49");
            if let Some(bis) = &z.bis {
                period = period.usage_to(bis);
            }
            vorgang = vorgang.add_usage_period(period);
            if let Some(ergebnis) = z.ergebnis_schritt {
                vorgang =
                    vorgang.add_energy_ref(builders::UtiltsEnergyAmountRef::new(z.id, ergebnis));
            }
        }
        for b in &p.bestandteile {
            let mut step = builders::UtiltsCalcStep::new(b.schritt)
                .time_period(b.zeitraum_id)
                .operator(&b.operator);
            if let Some(melo) = &b.messlokation {
                step = step.messlokation(melo);
            }
            if let Some(r) = b.schritt_ref {
                step = step.ref_calc_step(r);
            }
            if let Some(d) = &b.richtung {
                step = step.energy_direction(d);
            }
            if let Some(v) = &b.verlust_trafo {
                step = step.loss_factor_trafo(v);
            }
            if let Some(v) = &b.verlust_leitung {
                step = step.loss_factor_line(v);
            }
            if let Some(v) = &b.aufteilung {
                step = step.split_factor(v);
            }
            vorgang = vorgang.add_calc_step(step);
        }
    } else {
        if p.antworten.is_empty() {
            return Err(missing("antworten (STS+E01 per Zeitraum)"));
        }
        for a in &p.antworten {
            vorgang = vorgang.antwort(&a.code, a.zeitraum_id);
        }
        if let Some(text) = &p.freitext {
            vorgang = vorgang.antwort_freitext(text);
        }
        vorgang = vorgang.ref_transaction(
            p.referenz
                .as_deref()
                .ok_or_else(|| missing("referenz (RFF+TN — the 25001's Vorgangsnummer)"))?,
        );
    }

    let builder = builders::UtiltsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .add_vorgang(vorgang);
    finish_interchange(builder.serialize(), sender, receiver, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edi_energy::EdiEnergyMessage as _;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
    use mako_engine::payload::{UtiltsAntwort, UtiltsBestandteil, UtiltsPayload, UtiltsZeitraum};

    const NB: &str = "9900259000001";
    const MSB: &str = "9900259000002";

    /// Render as the outbox worker does, parse back, and return what the
    /// pre-send gate would refuse.
    fn refusals(payload: &UtiltsPayload) -> Vec<String> {
        let tenant = TenantId::from_tenant(NB);
        let process_id = ProcessId::new();
        let msg = OutboxMessage::new(
            StreamId::for_process(tenant, &process_id),
            process_id,
            tenant,
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            "UTILTS",
            MSB,
            serde_json::to_value(payload).expect("serialises"),
        );
        let registry = super::super::tests::test_registry(NB);
        let rendered = render_utilts(&msg.payload, &msg, &registry).expect("renders");
        let parsed = edi_energy::parse(&rendered.bytes).expect("parses");
        let report = parsed
            .validate_on_date(mako_fristen::heute())
            .expect("validates");
        report
            .sending_refusals()
            .map(|f| f.message.clone())
            .collect()
    }

    /// A Berechnungsformel with one Rechenschritt over one Messlokation — the
    /// shape of a Summenmessung's smallest case — passes its AHB.
    #[test]
    fn a_berechnungsformel_passes_its_profile() {
        let p = UtiltsPayload {
            pid: 25_001,
            sender: Some(NB.into()),
            receiver: Some(MSB.into()),
            vorgangsnummer: "BF0001".into(),
            lokation: Some("51238696012".into()),
            zeitraeume: vec![UtiltsZeitraum {
                id: 1,
                ab: "202610142200".into(),
                bis: None,
                status: "Z33".into(),
                ergebnis_schritt: Some(1),
            }],
            bestandteile: vec![UtiltsBestandteil {
                schritt: 1,
                zeitraum_id: 1,
                operator: "Z69".into(),
                messlokation: Some("DE0000011000000000000000012345678".into()),
                schritt_ref: None,
                richtung: Some("Z71".into()),
                verlust_trafo: None,
                verlust_leitung: None,
                aufteilung: None,
            }],
            ..UtiltsPayload::default()
        };
        assert_eq!(refusals(&p), Vec::<String>::new());
    }

    /// The MSB's Antwort with an `A99` and its remark passes its AHB.
    #[test]
    fn an_antwort_auf_berechnungsformel_passes_its_profile() {
        let p = UtiltsPayload {
            pid: 25_010,
            sender: Some(MSB.into()),
            receiver: Some(NB.into()),
            vorgangsnummer: "ANT0001".into(),
            referenz: Some("BF0001".into()),
            antworten: vec![UtiltsAntwort {
                code: "A99".into(),
                zeitraum_id: 1,
            }],
            freitext: Some("Die Messlokation fehlt im Lokationsbündel.".into()),
            ..UtiltsPayload::default()
        };
        assert_eq!(refusals(&p), Vec::<String>::new());
    }
}
