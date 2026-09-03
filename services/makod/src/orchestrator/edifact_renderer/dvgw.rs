//! DVGW renderer — ALOCAT, NOMINT, NOMRES and SSQNOT from domain-intent JSON.
//!
//! The four families share one shape (`dvgw_edi::MessageBuilder`), so one
//! renderer serves them; the outbox `message_type` names the family and the
//! payload's `pid` fixes the `BGM` document code the column admits
//! (`DvgwDocument::for_pid`), unless `document` states one.
//!
//! | Field | Required | Description |
//! |---|---|---|
//! | `pid` | yes | Prüfidentifikator → `SG1 RFF+Z13`, and the `BGM` DE 1001 code |
//! | `document` | no | `BGM` DE 1001 code, where the column admits several |
//! | `sender` / `receiver` | no | MP-IDs (default: the primary MP-ID / `msg.recipient`); the DE 3055 agency follows the id range |
//! | `document_number` | no | `BGM` DE 1004 (default: the message reference) |
//! | `message_ref` | no | derived from `causation_event_id` when absent |
//! | `message_datetime` | no | `DTM+137`, RFC 3339 (default: now) |
//! | `validity_period` | yes | `DTM+Z01` `{start, end}`, RFC 3339 — the gas day, or the SSQNOT Abrechnungszeitraum |
//! | `clearingnummer` | no | ALOCAT `RFF+ANX` |
//! | `original_nomination` | no | NOMINT `{reference, processed_at}` → `RFF+AGO` + `DTM+9` |
//! | `version` | no | `UNH` DE 0057 (default: the family's Anwendungscode) |
//! | `positions` | yes | one per `LIN`: `location {qualifier, code?}`, `item_type?`, `description?` (NOMRES `IMD`), `quantities [{qualifier, value, unit?, period {start, end}, status []}]`, `parties [{role, code, agency?}]` |
//!
//! Every rendered message is checked against the family's Nachrichtenbeschreibung
//! before it leaves; a `Muss` row the payload cannot fill is a
//! [`RenderError::BuilderError`], never a message on the wire.

use super::*;
use dvgw_edi::{DvgwDocument, DvgwPeriod, DvgwPlatform, MessageBuilder, Position};

/// Render a DVGW message; `family` is the outbox `message_type`.
pub(super) fn render_dvgw(
    family: &'static str,
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let pid = require_u32(p, family, "pid")?;
    let document = match p.get("document").and_then(|v| v.as_str()) {
        Some(code) => DvgwDocument::from_code(code).ok_or_else(|| {
            RenderError::BuilderError(format!("{family}: {code:?} is not a DVGW BGM DE 1001 code"))
        })?,
        None => DvgwDocument::for_pid(pid).ok_or_else(|| {
            RenderError::BuilderError(format!(
                "{family}: no shipped column publishes Prüfidentifikator {pid}; pass `document`"
            ))
        })?,
    };
    if document.message_type().as_str() != family {
        return Err(RenderError::BuilderError(format!(
            "{family}: Prüfidentifikator {pid} / document {document} belong to {}",
            document.message_type()
        )));
    }
    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));
    let document_number = p
        .get("document_number")
        .and_then(|v| v.as_str())
        .map_or_else(|| message_ref.clone(), str::to_owned);
    let message_datetime = match p.get("message_datetime").and_then(|v| v.as_str()) {
        Some(s) => instant(family, "message_datetime", s)?,
        None => time::OffsetDateTime::now_utc(),
    };
    let validity = period(family, "validity_period", p.get("validity_period"))?;

    let mut b = MessageBuilder::new(document)
        .message_ref(&message_ref)
        .document_number(document_number)
        .pruefidentifikator(pid)
        .message_datetime(message_datetime)
        .validity_period(validity)
        .sender_coded(sender, edi_energy::AgencyCode::for_mp_id(sender).as_str())
        .receiver_coded(
            receiver,
            edi_energy::AgencyCode::for_mp_id(receiver).as_str(),
        );
    if let Some(v) = p.get("version").and_then(|v| v.as_str()) {
        b = b.version(v);
    }
    if let Some(c) = p.get("clearingnummer").and_then(|v| v.as_str()) {
        b = b.clearingnummer(c);
    }
    if let Some(o) = p.get("original_nomination") {
        let reference = o
            .get("reference")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing(family, "original_nomination.reference"))?;
        let at = o
            .get("processed_at")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing(family, "original_nomination.processed_at"))?;
        b = b.original_nomination(
            reference,
            instant(family, "original_nomination.processed_at", at)?,
        );
    }
    let positions = p
        .get("positions")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| missing(family, "positions"))?;
    for pos in positions {
        b = b.position(position(family, pos)?);
    }
    let bytes = b
        .build()
        .map_err(|e| RenderError::BuilderError(format!("{family}: {e}")))?;

    // The Nachrichtenbeschreibung is the contract; a message that fails it
    // does not leave.
    let report = DvgwPlatform::default()
        .validate(&bytes)
        .map_err(|e| RenderError::BuilderError(format!("{family}: {e}")))?;
    if !report.is_valid() {
        let errors: Vec<String> = report.errors().map(ToString::to_string).collect();
        return Err(RenderError::BuilderError(format!(
            "{family} {pid} does not satisfy its Nachrichtenbeschreibung: {}",
            errors.join("; ")
        )));
    }
    finish_interchange(Ok(bytes), sender, receiver, msg)
}

fn position(family: &str, pos: &serde_json::Value) -> Result<Position, RenderError> {
    let mut position = Position::new();
    if let Some(n) = pos.get("number").and_then(|v| v.as_str()) {
        position = position.number(n);
    }
    if let Some(t) = pos.get("item_type").and_then(|v| v.as_str()) {
        position = position.item_type(t);
    }
    if let Some(d) = pos.get("description").and_then(|v| v.as_str()) {
        position = position.description(d);
    }
    let location = pos
        .get("location")
        .ok_or_else(|| missing(family, "positions[].location"))?;
    let qualifier = location
        .get("qualifier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| missing(family, "positions[].location.qualifier"))?;
    position = position.location(qualifier, location.get("code").and_then(|v| v.as_str()));
    let quantities = pos
        .get("quantities")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| missing(family, "positions[].quantities"))?;
    for q in quantities {
        let qualifier = q
            .get("qualifier")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing(family, "positions[].quantities[].qualifier"))?;
        let value = q
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing(family, "positions[].quantities[].value"))?;
        let per = period(family, "positions[].quantities[].period", q.get("period"))?;
        position = match q.get("unit").and_then(|v| v.as_str()) {
            Some(unit) => position.quantity_in(qualifier, value, unit, per),
            None => position.quantity(qualifier, value, per),
        };
        for status in q
            .get("status")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str())
        {
            position = position.status(status);
        }
    }
    for party in pos
        .get("parties")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let role = party
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing(family, "positions[].parties[].role"))?;
        let code = party
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing(family, "positions[].parties[].code"))?;
        position = match party.get("agency").and_then(|v| v.as_str()) {
            Some(agency) => position.party_coded(role, code, agency),
            None => position.party(role, code),
        };
    }
    Ok(position)
}

fn missing(family: &str, field: &str) -> RenderError {
    RenderError::MissingField {
        message_type: family.into(),
        field: field.into(),
    }
}

fn instant(family: &str, field: &str, s: &str) -> Result<time::OffsetDateTime, RenderError> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).map_err(|e| {
        RenderError::BuilderError(format!("{family}: {field} {s:?} is not RFC 3339: {e}"))
    })
}

fn period(
    family: &str,
    field: &str,
    v: Option<&serde_json::Value>,
) -> Result<DvgwPeriod, RenderError> {
    let v = v.ok_or_else(|| missing(family, field))?;
    let start = v
        .get("start")
        .and_then(|s| s.as_str())
        .ok_or_else(|| missing(family, &format!("{field}.start")))?;
    let end = v
        .get("end")
        .and_then(|s| s.as_str())
        .ok_or_else(|| missing(family, &format!("{field}.end")))?;
    Ok(DvgwPeriod {
        start: instant(family, &format!("{field}.start"), start)?,
        end: instant(family, &format!("{field}.end"), end)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};

    const NB: &str = "9870012345678";
    const MGV: &str = "9800505300009";

    fn registry() -> MpIdRegistry {
        MpIdRegistry::from_config(&[crate::config::PartyConfig {
            mp_id: NB.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
        }])
        .expect("registry")
    }

    fn outbox(message_type: &str, payload: serde_json::Value) -> OutboxMessage {
        OutboxMessage::new(
            StreamId::new("process/gabi-gas-test"),
            ProcessId::new(),
            TenantId::new(),
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            message_type,
            MGV,
            payload,
        )
    }

    /// The SSQNOT a Netzbetreiber tenant reports renders from the workflow's
    /// own payload, satisfies SSQNOT 5.7, and reads back as the same record.
    #[test]
    fn the_workflows_ssqnot_payload_renders_and_reads_back() {
        use mako_gabi_gas::{MehrMindermengenData, MmmVerfahren};
        use rust_decimal::Decimal;
        let data = MehrMindermengenData {
            pruefidentifikator: 70095,
            netzbetreiber: NB.into(),
            marktgebietsverantwortlicher: MGV.into(),
            netzkonto: "THE0NKH712345678".into(),
            zeitraum_von: time::macros::date!(2026 - 03 - 01),
            zeitraum_bis: time::macros::date!(2026 - 04 - 01),
            verfahren: MmmVerfahren::Slp,
            mehrmenge_kwh: Decimal::from(120),
            mindermenge_kwh: Decimal::from(6782),
            message_ref: mako_engine::types::MessageRef::new("MM1"),
        };
        let rendered = render_to_wire_bytes(&outbox("SSQNOT", data.ssqnot_payload()), &registry())
            .expect("renders");
        let wire = String::from_utf8(rendered.bytes).expect("utf-8");
        assert!(wire.contains("BGM+BAG::332+SSQNOTMM1'"), "{wire}");
        assert!(wire.contains("RFF+Z13:70095'"), "{wire}");
        assert!(wire.contains("NAD+MS+9870012345678::332'"), "{wire}");
        assert!(
            wire.contains("QTY+ZY2:6782:KWH'STS+A1G::332'NAD+ZSH+THE0NKH712345678::332'"),
            "{wire}"
        );

        let msg = DvgwPlatform::default()
            .parse(wire.as_bytes())
            .expect("parses back");
        let report = DvgwPlatform::validate_message(&msg);
        assert!(
            report.is_valid(),
            "{:?}",
            report.errors().collect::<Vec<_>>()
        );
        let record = dvgw_edi::ssqnot::MehrMindermengenmeldung::from_message(&msg).unwrap();
        assert_eq!(record.netzkonto, "THE0NKH712345678");
        assert_eq!(record.saldo_kwh(), Decimal::from(-6662));
        assert_eq!(record.zeitraum.start.date(), data.zeitraum_von);
    }

    /// The NOMINT the nomination workflow enqueues renders, satisfies NOMINT 4.6
    /// and reads back with the energy the workflow computed.
    #[test]
    fn the_workflows_nomint_payload_renders_and_reads_back() {
        use mako_engine::workflow::Workflow as _;
        use mako_gabi_gas::{
            GaBiGasNominationWorkflow, NominationCommand, NominationMenge, NominationPosition,
            NominationState,
        };
        let gas_day = mako_gabi_gas::GasDay::parse("2026-03-01").unwrap();
        let out = GaBiGasNominationWorkflow::handle(
            &NominationState::New,
            NominationCommand::SendNomination {
                pruefidentifikator: 70031,
                sender_eic: NB.into(),
                receiver_eic: MGV.into(),
                gas_day,
                nomination_ref: mako_engine::types::MessageRef::new("NOMINT00052"),
                positions: vec![NominationPosition {
                    ort_qualifier: "Z19".into(),
                    ort: "37Z005053MH0000D".into(),
                    richtung: "Z02".into(),
                    bilanzkreis_intern: "THE0BFH000000001".into(),
                    bilanzkreis_extern: Some("THE0BFH000000002".into()),
                    mengen: vec![
                        NominationMenge {
                            von: gas_day.start_utc(),
                            bis: gas_day.start_utc() + time::Duration::hours(12),
                            kwh_pro_h: rust_decimal::Decimal::from(100),
                        },
                        NominationMenge {
                            von: gas_day.start_utc() + time::Duration::hours(12),
                            bis: gas_day.end_utc(),
                            kwh_pro_h: rust_decimal::Decimal::from(200),
                        },
                    ],
                }],
                corrects: None,
            },
        )
        .expect("nominates");
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "NOMINT");
        let rendered = render_to_wire_bytes(
            &outbox("NOMINT", out.outbox[0].payload.clone()),
            &registry(),
        )
        .expect("renders");
        let wire = String::from_utf8(rendered.bytes).unwrap();
        assert!(wire.contains("BGM+55G::332+NOMINT00052'"), "{wire}");
        assert_eq!(
            wire.matches("LOC+Z19+37Z005053MH0000D::332'").count(),
            2,
            "one LOC per period: {wire}"
        );
        let msg = DvgwPlatform::default().parse(wire.as_bytes()).unwrap();
        assert!(DvgwPlatform::validate_message(&msg).is_valid());
        // 100 kWh/h × 12 h + 200 kWh/h × 12 h.
        assert_eq!(msg.energy_by_qualifier()["Z02"].to_string(), "3600");
        assert_eq!(
            msg.correlation_key().unwrap().to_string(),
            "Nominierung|2026-03-01|37Z005053MH0000D|THE0BFH000000001|THE0BFH000000002"
        );
    }

    /// A NOMINT with its re-nomination reference, from a payload that names
    /// only what the column asks.
    #[test]
    fn a_nomint_renders_with_its_original_nomination() {
        let payload = serde_json::json!({
            "pid": 70030,
            "validity_period": { "start": "2026-03-01T05:00:00Z", "end": "2026-03-02T05:00:00Z" },
            "original_nomination": { "reference": "NOMINT00051", "processed_at": "2026-02-28T18:00:00Z" },
            "positions": [{
                "location": { "qualifier": "Z19", "code": "ABCD1234" },
                "quantities": [{ "qualifier": "Z03", "value": "6782",
                                 "period": { "start": "2026-03-01T05:00:00Z", "end": "2026-03-02T05:00:00Z" } }],
                "parties": [{ "role": "ZEU", "code": "BK-CODE-1" }, { "role": "ZES", "code": "BK-CODE-2" }],
            }],
        });
        let rendered =
            render_to_wire_bytes(&outbox("NOMINT", payload), &registry()).expect("renders");
        let wire = String::from_utf8(rendered.bytes).unwrap();
        assert!(
            wire.contains("UNH+") && wire.contains("+ORDERS:D:07A:UN:DVGW17'"),
            "{wire}"
        );
        assert!(wire.contains("BGM+01G::332+"), "{wire}");
        assert!(
            wire.contains("RFF+Z13:70030'RFF+AGO:NOMINT00051'DTM+9:202602281800:203'"),
            "{wire}"
        );
        assert!(wire.contains("QTY+Z03:6782:KW1'"), "{wire}");
        assert!(
            DvgwPlatform::default()
                .validate(wire.as_bytes())
                .unwrap()
                .is_valid()
        );
    }

    /// A family mismatch and a column the payload cannot fill are refused,
    /// not shipped.
    #[test]
    fn a_wrong_family_or_a_short_payload_is_refused() {
        let payload = serde_json::json!({
            "pid": 70030,
            "validity_period": { "start": "2026-03-01T05:00:00Z", "end": "2026-03-02T05:00:00Z" },
            "positions": [{ "location": { "qualifier": "Z19", "code": "P" },
                            "quantities": [{ "qualifier": "Z03", "value": "1",
                                             "period": { "start": "2026-03-01T05:00:00Z", "end": "2026-03-02T05:00:00Z" } }] }],
        });
        assert!(matches!(
            render_to_wire_bytes(&outbox("ALOCAT", payload.clone()), &registry()),
            Err(RenderError::BuilderError(_))
        ));
        // No `NAD+ZEU`: NOMINT marks the interne Bilanzkreis Erforderlich.
        let err = render_to_wire_bytes(&outbox("NOMINT", payload), &registry()).unwrap_err();
        assert!(err.to_string().contains("NAD+ZEU"), "{err}");
    }
}
