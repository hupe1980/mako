//! Turning an inbound `de.mako.process.initiated` into a work order.
//!
//! `makod` spawns the NB-role `gpke-sperrung` workflow when an ORDERS 17115 or
//! 17117 arrives over AS4, and announces it as `de.mako.process.initiated` with
//! the Prüfidentifikator in the `makopid` CloudEvents extension. This module
//! maps that announcement onto a row the field team can work from.

use crate::model::{Arbeitszeit, OrderType};
use crate::pg::{CreateOrderRequest, Treffpunkt};

/// Build a work order from a `de.mako.process.initiated` CloudEvent.
///
/// Returns `None` — not an error — when the event is not an executable
/// Sperr-/Entsperrauftrag. Most events reaching the webhook are other process
/// kinds, and 17116 (Anfrage Sperrung, NB→MSB) is a question the NB asks the
/// Messstellenbetreiber rather than an order for this queue.
#[must_use]
pub fn order_from_process_initiated(event: &serde_json::Value) -> Option<CreateOrderRequest> {
    if event.get("type").and_then(serde_json::Value::as_str)?
        != mako_events::mako::PROCESS_INITIATED
    {
        return None;
    }
    let data = event.get("data")?;
    let pid = u32::try_from(
        event
            .get("makopid")
            .or_else(|| data.get("pid"))
            .and_then(serde_json::Value::as_u64)?,
    )
    .ok()?;
    let order_type = OrderType::from_pid(pid)?;

    let malo_id = str_at(data, "malo_id")?;
    // The ordering LF. `sender` is what the adapter surfaces for an inbound
    // message; `lf_mp_id` is the explicit spelling.
    let lf_mp_id = str_at(data, "lf_mp_id").or_else(|| str_at(data, "sender"))?;
    // The subject of a process.initiated is the makod process id — the handle
    // the IFTSTA 21039 has to be reported into.
    let process_id = event
        .get("subject")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);

    let treffpunkt = data.get("treffpunkt");
    Some(CreateOrderRequest {
        malo_id,
        lf_mp_id,
        order_type,
        process_id,
        // DTM+203 / DTM+469. Kept apart rather than folded into one
        // `planned_date`: "carry this out on the 3rd" and "carry this out as
        // soon as you can, but not before the 3rd" are different instructions,
        // and only the first is a date the field team must hit.
        ausfuehrung_am: date_at(data, "ausfuehrung_am"),
        fruehestens_am: date_at(data, "fruehestens_am"),
        arbeitszeit: match str_at(data, "arbeitszeit").as_deref() {
            // IMD 7081 — accepted as the EDIFACT code or the domain spelling,
            // because the adapter may surface either.
            Some("Z53" | "innerhalb") => Some(Arbeitszeit::Innerhalb),
            Some("Z54" | "auch_ausserhalb") => Some(Arbeitszeit::AuchAusserhalb),
            _ => None,
        },
        treffpunkt: treffpunkt.map_or_else(Treffpunkt::default, |t| Treffpunkt {
            hinweis: str_at(t, "hinweis"),
            strasse: str_at(t, "strasse"),
            plz: str_at(t, "plz"),
            ort: str_at(t, "ort"),
            land: str_at(t, "land").map(|c| c.to_uppercase()),
        }),
        hinweis: str_at(data, "hinweis"),
    })
}

fn str_at(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Parse a date field, accepting both `CCYYMMDD` (as EDIFACT carries it) and
/// ISO `YYYY-MM-DD` (as an adapter usually normalises it).
fn date_at(v: &serde_json::Value, key: &str) -> Option<time::Date> {
    let s = str_at(v, key)?;
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        let fmt = time::macros::format_description!("[year][month][day]");
        time::Date::parse(&s, &fmt).ok()
    } else {
        let fmt = time::macros::format_description!("[year]-[month]-[day]");
        // A DTM may carry CCYYMMDDHHMMZZZ; the date is its first ten characters
        // once normalised, so a longer string is truncated rather than refused.
        time::Date::parse(s.get(..10).unwrap_or(&s), &fmt).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn ce(pid: u64, extra: serde_json::Value) -> serde_json::Value {
        let mut data = serde_json::json!({
            "malo_id": "51238696012",
            "lf_mp_id": "9900012345678",
        });
        if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                d.insert(k.clone(), v.clone());
            }
        }
        serde_json::json!({
            "type": mako_events::mako::PROCESS_INITIATED,
            "subject": "0b7c1e2a-0000-4000-8000-000000000001",
            "makopid": pid,
            "data": data,
        })
    }

    #[test]
    fn a_sperrauftrag_becomes_a_work_order() {
        let req = order_from_process_initiated(&ce(
            17115,
            serde_json::json!({ "ausfuehrung_am": "20260901" }),
        ))
        .expect("17115 is a Sperrauftrag");
        assert_eq!(req.order_type, OrderType::Sperrung);
        assert_eq!(req.malo_id, "51238696012");
        assert_eq!(req.ausfuehrung_am, Some(date!(2026 - 09 - 01)));
        assert_eq!(req.fruehestens_am, None);
        assert_eq!(
            req.process_id.as_deref(),
            Some("0b7c1e2a-0000-4000-8000-000000000001"),
            "the IFTSTA has to be reported into the process the ORDERS spawned"
        );
    }

    #[test]
    fn an_entsperrauftrag_carries_its_arbeitszeit() {
        // IMD+7081 is a Muss on the 17117: it is how the LF asks (and pays) for
        // the out-of-hours reconnection §41f Abs. 7 EnWG can require.
        let req =
            order_from_process_initiated(&ce(17117, serde_json::json!({ "arbeitszeit": "Z54" })))
                .expect("17117 is an Entsperrauftrag");
        assert_eq!(req.order_type, OrderType::Entsperrung);
        assert_eq!(req.arbeitszeit, Some(Arbeitszeit::AuchAusserhalb));
    }

    #[test]
    fn anfrage_sperrung_is_not_queued_for_the_field_team() {
        // 17116 is NB→MSB: "is the meter reachable?". Queuing it would put a
        // question in front of a technician as though it were an order.
        assert!(order_from_process_initiated(&ce(17116, serde_json::json!({}))).is_none());
    }

    #[test]
    fn other_process_kinds_are_ignored() {
        assert!(order_from_process_initiated(&ce(55001, serde_json::json!({}))).is_none());
        let wrong_type = serde_json::json!({
            "type": "de.mako.process.completed",
            "makopid": 17115,
            "data": { "malo_id": "51238696012", "lf_mp_id": "9900012345678" },
        });
        assert!(order_from_process_initiated(&wrong_type).is_none());
    }

    #[test]
    fn an_event_without_a_malo_is_refused() {
        let no_malo = serde_json::json!({
            "type": mako_events::mako::PROCESS_INITIATED,
            "makopid": 17115,
            "data": { "lf_mp_id": "9900012345678" },
        });
        assert!(order_from_process_initiated(&no_malo).is_none());
    }

    #[test]
    fn the_treffpunkt_survives_the_hop() {
        // SG2 NAD+Z24. Without it the queue names the Marktlokation to
        // disconnect and not where the technician has to go.
        let req = order_from_process_initiated(&ce(
            17115,
            serde_json::json!({
                "fruehestens_am": "2026-09-03",
                "treffpunkt": {
                    "strasse": "Musterstraße 12",
                    "plz": "10115",
                    "ort": "Berlin",
                    "land": "de",
                    "hinweis": "Zählerschrank im Hof",
                },
                "hinweis": "Hund im Garten",
            }),
        ))
        .expect("17115");
        assert_eq!(req.fruehestens_am, Some(date!(2026 - 09 - 03)));
        assert_eq!(req.treffpunkt.ort.as_deref(), Some("Berlin"));
        assert_eq!(
            req.treffpunkt.land.as_deref(),
            Some("DE"),
            "NAD 3207 is upper-case; normalising here keeps the CHECK constraint \
             from rejecting a well-formed message over letter case"
        );
        assert_eq!(req.hinweis.as_deref(), Some("Hund im Garten"));
        assert!(req.validate().is_ok());
    }

    #[test]
    fn a_datetime_dtm_is_truncated_to_its_date() {
        let req = order_from_process_initiated(&ce(
            17115,
            serde_json::json!({ "ausfuehrung_am": "2026-09-01T06:00:00Z" }),
        ))
        .expect("17115");
        assert_eq!(req.ausfuehrung_am, Some(date!(2026 - 09 - 01)));
    }
}
