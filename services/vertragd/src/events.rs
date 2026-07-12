//! CloudEvent dispatch and inbound routing for `vertragd`.

use serde_json::Value;
use uuid::Uuid;

/// Build a `de.vertrag.*` CloudEvent.
pub fn build_cloud_event(event_type: &str, vertrag_id: Uuid, tenant: &str, data: Value) -> Value {
    serde_json::json!({
        "specversion": "1.0",
        "type": format!("de.vertrag.{event_type}"),
        "source": format!("urn:vertragd:lf:{tenant}"),
        "id": Uuid::new_v4().to_string(),
        "time": time::OffsetDateTime::now_utc().to_string(),
        "subject": vertrag_id.to_string(),
        "datacontenttype": "application/json",
        "data": data,
    })
}

/// CloudEvent types emitted by `vertragd`:
///
/// | Event type | When |
/// |---|---|
/// | `de.vertrag.aktiv` | All components confirmed, billing running |
/// | `de.vertrag.teilerfuellung` | First component confirmed, others pending |
/// | `de.vertrag.abgelehnt` | NB rejected one or more components |
/// | `de.vertrag.gekuendigt` | Lieferende dispatched for all components |
/// | `de.vertrag.abgeschlossen` | All components ended, Schlussrechnung trigger |
/// | `de.vertrag.komponente.bestaetigt` | Individual component NB-confirmed |
/// | `de.vertrag.rahmen.aktiv` | Rahmenvertrag activated |
/// CloudEvent types consumed from the MaKo event bus.
pub fn parse_mako_outcome(ce: &Value) -> Option<MakoOutcome> {
    let ce_type = ce.get("type")?.as_str()?;
    let data = ce.get("data")?;
    let process_id = data
        .get("process_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let malo_id = data
        .get("malo_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let erc_code = data
        .get("erc_code")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let reason = data
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    match ce_type {
        t if t.ends_with(".bestaetigt")
            || t.ends_with(".confirmed")
            || t.ends_with(".completed") =>
        {
            Some(MakoOutcome {
                process_id,
                malo_id,
                confirmed: true,
                erc_code: None,
                reason: None,
            })
        }
        t if t.ends_with(".abgelehnt") || t.ends_with(".rejected") => Some(MakoOutcome {
            process_id,
            malo_id,
            confirmed: false,
            erc_code,
            reason,
        }),
        _ => None,
    }
}

pub struct MakoOutcome {
    pub process_id: Option<String>,
    pub malo_id: Option<String>,
    pub confirmed: bool,
    pub erc_code: Option<String>,
    pub reason: Option<String>,
}
