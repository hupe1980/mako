//! CloudEvent dispatch and inbound routing for `vertragd`.

use serde_json::Value;
use uuid::Uuid;

/// Build a `de.vertrag.*` CloudEvent.
///
/// Every event carries the workspace-standard tracing attributes:
/// `tenantid` (data-isolation scope) and `correlationid` (the Vertrag the
/// event belongs to — same value as `subject`, so consumers correlate all
/// lifecycle events of one contract without parsing `data`).
pub fn build_cloud_event(event_type: &str, vertrag_id: Uuid, tenant: &str, data: Value) -> Value {
    serde_json::json!({
        "specversion": "1.0",
        "type": format!("de.vertrag.{event_type}"),
        "source": format!("urn:vertragd:lf:{tenant}"),
        "id": Uuid::new_v4().to_string(),
        "time": time::OffsetDateTime::now_utc().to_string(),
        "subject": vertrag_id.to_string(),
        "tenantid": tenant,
        "correlationid": vertrag_id.to_string(),
        "datacontenttype": "application/json",
        "data": data,
    })
}

/// CloudEvent types emitted by `vertragd` (every emission goes through
/// [`build_cloud_event`] and is HMAC-signed by the caller):
///
/// | Event type | When |
/// |---|---|
/// | `de.vertrag.aktiv` | All components NB-confirmed, billing may start |
/// | `de.vertrag.gekuendigt` | Lieferende dispatched (Rahmenvertrag cascade, per child) |
/// | `de.vertrag.kuendigung` | Kündigung accepted, Lieferende dispatched |
/// | `de.vertrag.kuendigung_widerrufen` | Kündigung withdrawn before Lieferende |
/// | `de.vertrag.tarifwechsel` | Product change applied immediately |
/// | `de.vertrag.tarifwechsel_geplant` | Future-dated product change stored |
/// | `de.vertrag.preisgarantie_updated` | Price guarantee stored/replaced |
/// | `de.vertrag.preisaenderung.ankuendigung` | Notice worker, ≤ 42 days before Wirksamkeit |
/// | `de.vertrag.autoerneuerung.ankuendigung` | 30 days before auto-renewal |
/// | `de.vertrag.ablauf.ankuendigung` | 30 days before vertragsende / preisgarantie_bis |
///
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
