//! The NB sends a Berechnungsformel (UTILTS 25001) to an MSB of the
//! Lokationsbündel — WiM Strom Teil 2 Kap. 2.3.
//!
//! The formula is edmd's: the body carries what `GET
//! /api/v1/virtual/{id}/berechnungsformel` answers — the formula edmd computes
//! the virtual Marktlokation with, which the Desk draws for the person who
//! sends it. makod renders it, holds it to its AHB before anything is written,
//! and tracks the MSB's 25010 by the Vorgangsnummer for 5 Werktage, after
//! which silence „gilt … als Zustimmung".

use mako_engine::payload::{UtiltsBestandteil, UtiltsPayload, UtiltsZeitraum};
use mako_gpke::berechnungsformel::{
    ANTWORT_WINDOW_LABEL, FORMEL_PID, FormelCommand, GpkeBerechnungsformelWorkflow, WORKFLOW_NAME,
};

use super::*;

/// The Verwendungszeitraum the formula is sent for — one, from its validity.
const ZEITRAUM: u32 = 1;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    /// The MSB of the Lokationsbündel the formula goes to.
    empfaenger_mp_id: String,
    /// edmd's answer for the virtual Marktlokation, as served.
    formel: Formel,
}

/// edmd's `GET /api/v1/virtual/{id}/berechnungsformel` answer.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Formel {
    virtual_malo_id: String,
    gueltig_ab: Option<String>,
    gueltig_bis: Option<String>,
    status: String,
    ergebnis_schritt: u32,
    bestandteile: Vec<Bestandteil>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Bestandteil {
    schritt: u32,
    operator: String,
    messlokation: Option<String>,
    schritt_ref: Option<u32>,
    richtung: Option<String>,
    verlust_trafo: Option<String>,
    verlust_leitung: Option<String>,
    aufteilung: Option<String>,
}

pub(super) fn cmd_gpke_berechnungsformel_senden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch(s, p))
}

async fn dispatch(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: Body = serde_json::from_value(payload.clone())
        .map_err(|e| DispatchError::InvalidPayload(format!("payload: {e}")))?;
    let formel = utilts(&body.formel, uuid::Uuid::new_v4())?;
    let empfaenger = MarktpartnerCode::new(body.empfaenger_mp_id.trim());
    let command = FormelCommand::SendFormel {
        formel: Box::new(formel.clone()),
        empfaenger,
    };
    {
        use mako_engine::workflow::Workflow as _;
        let out = GpkeBerechnungsformelWorkflow::handle(
            &mako_gpke::berechnungsformel::FormelState::New,
            command.clone(),
        )
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
        super::eog_stammdaten::refuse_what_the_gate_refuses(
            state,
            &latest_format_version(),
            &out.outbox,
        )?;
    }
    // WiM Strom Teil 2 Kap. 2.3.2 Nr. 2: the answer by the 5. WT.
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        mako_fristen::antwort::BERECHNUNGSFORMEL_ANTWORT_WERKTAGE,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let process_id = spawn_process::<GpkeBerechnungsformelWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(WORKFLOW_NAME, latest_format_version()),
        command,
        &[(ANTWORT_WINDOW_LABEL, due_at)],
        &[formel.vorgangsnummer.as_str()],
    )
    .await?;
    Ok(DispatchOutcome::Spawned { process_id })
}

/// edmd's formula as the 25001 Vorgang: one Verwendungszeitraum from its
/// validity, every Bestandteil in it.
///
/// # Errors
///
/// [`DispatchError::InvalidPayload`] for a formula without a validity start,
/// or with a date that is no calendar date.
fn utilts(f: &Formel, id: uuid::Uuid) -> Result<UtiltsPayload, DispatchError> {
    let instant = |raw: &str| -> Result<String, DispatchError> {
        let tag = super::eog_stammdaten::stichtag(raw)?;
        let t = mako_fristen::berlin_midnight(tag);
        Ok(format!(
            "{:04}{:02}{:02}{:02}{:02}",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute()
        ))
    };
    let ab = f.gueltig_ab.as_deref().ok_or_else(|| {
        DispatchError::InvalidPayload(
            "formel.gueltig_ab — the Berechnungsformel's Verwendungszeitraum needs its start \
             (SG6 DTM+Z25)"
                .to_owned(),
        )
    })?;
    Ok(UtiltsPayload {
        pid: FORMEL_PID,
        // `BF` + 32 hex digits: 34 characters, inside IDE DE 7402's 35.
        vorgangsnummer: format!("BF{}", id.simple()),
        lokation: Some(f.virtual_malo_id.clone()),
        zeitraeume: vec![UtiltsZeitraum {
            id: ZEITRAUM,
            ab: instant(ab)?,
            bis: f.gueltig_bis.as_deref().map(instant).transpose()?,
            status: f.status.clone(),
            ergebnis_schritt: (f.status == "Z33").then_some(f.ergebnis_schritt),
        }],
        bestandteile: if f.status == "Z33" {
            f.bestandteile
                .iter()
                .map(|b| UtiltsBestandteil {
                    schritt: b.schritt,
                    zeitraum_id: ZEITRAUM,
                    operator: b.operator.clone(),
                    messlokation: b.messlokation.clone(),
                    schritt_ref: b.schritt_ref,
                    richtung: b.richtung.clone(),
                    verlust_trafo: b.verlust_trafo.clone(),
                    verlust_leitung: b.verlust_leitung.clone(),
                    aufteilung: b.aufteilung.clone(),
                })
                .collect()
        } else {
            Vec::new()
        },
        ..UtiltsPayload::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::TenantId;
    use mako_engine::workflow::Workflow as _;

    /// edmd's answer for a Summenmessung of two Messlokationen — exactly what
    /// `GET /api/v1/virtual/{id}/berechnungsformel` serves.
    fn edmd() -> serde_json::Value {
        serde_json::json!({
            "virtual_malo_id": "51238696012",
            "gueltig_ab": "2026-10-15",
            "gueltig_bis": null,
            "status": "Z33",
            "ergebnis_schritt": 1,
            "bestandteile": [
                { "schritt": 1, "operator": "Z69", "messlokation": "DE0000011000000000000000012345678", "richtung": "Z71" },
                { "schritt": 1, "operator": "Z69", "messlokation": "DE0000011000000000000000087654321", "richtung": "Z71" }
            ]
        })
    }

    /// The formula edmd serves becomes a 25001 the pre-send gate passes.
    #[test]
    fn edmds_formula_becomes_a_25001_its_profile_accepts() {
        let formel: Formel = serde_json::from_value(edmd()).expect("edmd's shape");
        let payload = utilts(&formel, uuid::Uuid::nil()).expect("complete");
        assert_eq!(
            payload.zeitraeume[0].ab, "202610142200",
            "the Berlin day begins at 22:00 UTC in summer"
        );
        let out = GpkeBerechnungsformelWorkflow::handle(
            &mako_gpke::berechnungsformel::FormelState::New,
            FormelCommand::SendFormel {
                formel: Box::new(payload),
                empfaenger: MarktpartnerCode::new("9900259000002"),
            },
        )
        .expect("sends");
        let pending = &out.outbox[0];
        let mut msg = mako_engine::outbox::OutboxMessage::new(
            mako_engine::ids::StreamId::new("process/test"),
            mako_engine::ids::ProcessId::new(),
            TenantId::from_tenant("9900259000001"),
            mako_engine::ids::CorrelationId::new(),
            mako_engine::ids::ConversationId::new(),
            mako_engine::ids::EventId::new(),
            pending.message_type.clone(),
            pending.recipient.clone(),
            pending.payload.clone(),
        );
        msg.format_version = Some(latest_format_version());
        let registry =
            crate::party_registry::MpIdRegistry::from_config(&[crate::core::config::PartyConfig {
                mp_id: "9900259000001".to_owned(),
                roles: vec!["NB".to_owned()],
                primary: true,
                agency: None,
                as4: None,
            }])
            .expect("registry");
        let rendered = crate::orchestrator::edifact_renderer::render_to_wire_bytes(&msg, &registry)
            .expect("renders");
        let parsed = edi_energy::parse(&rendered.bytes).expect("parses");
        use edi_energy::EdiEnergyMessage as _;
        let report = parsed
            .validate_on_date(mako_fristen::heute())
            .expect("validates");
        let refusals: Vec<_> = report
            .sending_refusals()
            .map(|f| f.message.clone())
            .collect();
        assert!(refusals.is_empty(), "{refusals:#?}");
    }

    /// A formula without the start of its Zeitraum is refused, not sent.
    #[test]
    fn a_formula_without_its_start_is_refused() {
        let mut v = edmd();
        v["gueltig_ab"] = serde_json::Value::Null;
        let formel: Formel = serde_json::from_value(v).expect("edmd's shape");
        assert!(utilts(&formel, uuid::Uuid::nil()).is_err());
    }
}
