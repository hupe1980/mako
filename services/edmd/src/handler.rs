//! Axum webhook handler for inbound `MarktEvent` CloudEvents from `marktd`.
//!
//! ## Event routing
//!
//! | `ce_type`                    | `makopid` | Action |
//! |------------------------------|-----------|--------|
//! | `de.mako.process.completed`  | MSCONS set | Store `MeterDataReceipt` |
//! | `de.mako.process.completed`  | 55002/55078, accepted (Bestätigung Anmeldung) | Auto-create `LIEFERBEGINN` reading order |
//! | `de.mako.process.completed`  | 55005, accepted (Bestätigung Abmeldung) | Auto-create `LIEFERENDE` reading order |
//! | *(anything else)*            | *(any)*   | 204 No Content (ignored) |
//!
//! ## Supply-handover reading orders
//!
//! A confirmed Anmeldung (Lieferbeginn) or Abmeldung (Lieferende) triggers reading
//! orders to capture the meter reading at the supply handover boundary —
//! required for accurate Mehr-/Mindermengensaldo calculation.

use std::sync::Arc;

/// The answer an ingest door gives when the store refused a batch.
///
/// `marktd` treats 2xx as delivered and redelivers on 5xx, so the choice is
/// whether this delivery should come back. A transient failure — a lost
/// connection, a lock the statement declined to queue for — should: the same
/// bytes will store once the condition clears. A **refused** one must not: an
/// overlapping span, a restated value under an existing version or a second
/// network operator on one reading is a statement about the message, and
/// redelivering it is a loop that runs until the retry budget is exhausted
/// while an operator watches a 5xx that never resolves.
///
/// 422 is the honest answer for the second: the delivery was received and
/// understood, and it cannot be stored as sent.
///
/// The 500 carries an `error_id` its log line carries; the 422 says what the
/// store refused.
pub(crate) fn ingest_refusal(err: crate::domain::error::EdmError, what: String) -> ApiError {
    if err.is_retryable() {
        ApiError::internal(anyhow::Error::new(err).context(what))
    } else {
        ApiError::unprocessable(format!("{what}: {err}"))
    }
}

use crate::domain::ALL_MSCONS_PIDS;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_service::ApiError;
use secrecy::{ExposeSecret, SecretString};
use tracing::{debug, info, warn};

use crate::store::{MeterStoreTimeSeriesRepository, MeterStoreTyp2Repository};

/// The calendar month `day` falls in, as an inclusive date range.
///
/// The default period for a Gasbeschaffenheit delivery that states none: the
/// grid operator publishes the Abrechnungsbrennwert monthly, so the month is the
/// narrowest honest guess. Applying it to the MaLo's whole history — the former
/// behaviour — silently repriced every past period.
pub(crate) fn month_of(day: time::Date) -> (time::Date, time::Date) {
    let first = day.replace_day(1).unwrap_or(day);
    let last = first
        .replace_day(first.month().length(first.year()))
        .unwrap_or(day);
    (first, last)
}

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    /// Authoritative meter-data store (hot Postgres + cold Iceberg via meterstore),
    /// plus the edmd business-table pool it exposes through `repo.pool()`.
    pub repo: MeterStoreTimeSeriesRepository,
    /// Separate store for ESA "Werte nach Typ 2" (non-authoritative; never billing).
    pub typ2_repo: MeterStoreTyp2Repository,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// Tenant identifier — used as Cedar resource_tenant for REST queries.
    pub tenant: String,
    /// `marktd` base URL — used by the Jahresablesung campaign to enumerate SLP MaLos.
    pub marktd_url: String,
    /// `marktd` bearer token.
    pub marktd_api_key: secrecy::SecretString,
    /// `marktd` client — places a Messlokation an MSCONS names under its
    /// Marktlokation.
    pub marktd: mako_markt::marktd_client::MarktdClient,
    /// ERP webhook URL for outbound CloudEvents from direct push and quality warnings.
    pub erp_webhook_url: Option<String>,
    /// Optional secret signing outbound CloudEvents (Standard Webhooks).
    pub erp_webhook_secret: Option<secrecy::SecretString>,
    /// §14a SMGW/CLS compliance thresholds, so the synchronous checks on the
    /// upsert and audit endpoints use the same numbers as the daily sweep —
    /// four call sites that would otherwise each carry their own literal.
    pub smgw: crate::config::SmgwConfig,
    /// Delivery-surveillance thresholds, shared by the worker and the on-demand
    /// scan endpoint so both judge by the same numbers.
    pub surveillance: crate::config::SurveillanceConfig,
    /// Whether a real cold tier is configured and its maintenance loop running.
    ///
    /// `false` means meterstore is hot-only against an in-memory warehouse:
    /// nothing is ever archived and the settled history has nowhere to go.
    /// `GET /api/v1/archive/status` reports this rather than a constant, so it
    /// cannot read as "archival is working" on the deployment where it is not.
    pub cold_tier_enabled: bool,
    /// `[policy] physische_loeschung` — the reading every erasure result names.
    pub physische_loeschung: mako_service::policy::PolicyTriple,
}

impl HandlerState {
    /// Hand a `CloudEvent` to the transactional outbox — see [`crate::outbox`].
    pub(crate) async fn emit(&self, ce: &mako_service::CloudEvent) {
        crate::outbox::emit(self.repo.pool(), ce).await;
    }
}

/// `POST /webhook` — receive a `MarktEvent` from `marktd`.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Verify the HMAC signature — fail closed. This webhook stores meter
    //    readings and auto-creates reading orders, so accepting it without a
    //    configured secret would let anything that can reach the port inject data.
    //    An unconfigured secret is rejected, mirroring edmd's fail-closed OIDC
    //    posture, rather than waved through.
    match (*state.inbound_secret).as_ref() {
        Some(secret) => {
            // The shared verifier, which also refuses a stale
            // `webhook-timestamp`: a replayed reading would be stored twice.
            if let Err(err) = mako_service::webhook::verify_request(
                Some(secret.expose_secret().as_bytes()),
                &headers,
                &body,
            ) {
                warn!(%err, "edmd: inbound webhook refused");
                return (StatusCode::from(err), err.to_string()).into_response();
            }
        }
        None => {
            warn!(
                "edmd: /webhook rejected — no [webhook].inbound_secret configured; refusing \
                 unauthenticated reading ingestion"
            );
            return ApiError::Unauthorized.into_response();
        }
    }

    // 2. Parse JSON body.
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "edmd: failed to parse MarktEvent");
            return ApiError::bad_request("invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();
    // Prefer the forwarded makopid extension; fall back to data["pid"].
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| event["data"]["pid"].as_u64())
        .unwrap_or(0) as u32;

    debug!(ce_type, pid, "edmd: received event");

    // No branch for `de.mako.process.initiated`: an INSRPT Störungsbehebung
    // orders no reading. WiM Teil 2 Kap. 1.1 (S. 5) leaves the Zählerstand as
    // read — „Der von der Messeinrichtung abgelesene Zählerstand wird nicht
    // korrigiert." — and has the MSB transmit it, with any
    // Korrekturenergiemenge, through the Aufbereitung und Übermittlung von
    // Werten: an MSCONS this door already stores.

    // ── Lieferbeginn / Lieferende → reading orders ────────────────────────────
    //
    // A confirmed Anmeldung starts supply and a confirmed Abmeldung ends it;
    // both boundaries need a reading order to capture the meter reading at the
    // supply handover — required for an accurate Mehr-/Mindermengensaldo.
    // `ablese_anlass` reads the answer PID and the verdict.
    //
    // Legal basis: GPKE (BK6-24-174) Teil 1 — Ablesung bei Lieferbeginn/-ende.
    // Not BK6-22-024: GPKE Teil 1–3 were reissued under BK6-24-174, and what
    // stayed behind there is GPKE Teil 4 and WiM Strom Teil 1/2.
    if let Some((anlass, label)) = ablese_anlass(&ce_type, pid, &event["data"]) {
        let data = &event["data"];
        let malo_id = data["malo_id"]
            .as_str()
            .or_else(|| data["location_id"].as_str())
            .unwrap_or("")
            .to_owned();

        // The reading date is the Lieferbeginndatum / Lieferendedatum from the event.
        // Fall back to today when the field is absent.
        let geplant_am = ablese_datum(data).unwrap_or_else(mako_fristen::heute);

        let ausfuehrt_bis = geplant_am
            .checked_add(time::Duration::days(3))
            .unwrap_or(geplant_am);

        // The role that ordered the reading is the one whose process the
        // completion came from: the NB's on a 55007 it sent or a 55005 it
        // confirmed, the LF's on an answer it received or sent.
        let auftraggeber = auftraggeber_rolle(&event);
        if auftraggeber.is_none() {
            warn!(
                malo_id = %malo_id, pid,
                "edmd: {label} completion names no marktrole — no reading order without the \
                 role that orders it"
            );
        }
        if let (false, Some(auftraggeber)) = (malo_id.is_empty(), auftraggeber) {
            let process_id_str = event["subject"].as_str().unwrap_or("").to_owned();
            let pool = state.repo.pool();
            // 55002/55078/55005/55007/55008 are UTILMD Strom PIDs (GPKE), so the order reads
            // an electricity meter; the Gas Lieferbeginn is a 44xxx PID and
            // never reaches this branch.
            let result = sqlx::query(
                r#"INSERT INTO ablese_auftraege
                   (malo_id, tenant, anlass, auftraggeber_rolle,
                    geplant_am, ausfuehrt_bis, insrpt_process_id, sparte)
                   VALUES ($1, $2, $3, $7, $4, $5, $6, 'STROM')
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(&malo_id)
            .bind(&state.tenant)
            .bind(anlass)
            .bind(geplant_am)
            .bind(ausfuehrt_bis)
            .bind(if process_id_str.is_empty() {
                None
            } else {
                Some(process_id_str.clone())
            })
            .bind(auftraggeber)
            .execute(pool)
            .await;

            match result {
                Ok(r) if r.rows_affected() > 0 => {
                    info!(
                        malo_id = %malo_id,
                        anlass,
                        geplant_am = %geplant_am,
                        "edmd: auto-created {label} reading order (GPKE Beginn-/Schlussablesung)"
                    );
                }
                Ok(_) => debug!(malo_id = %malo_id, "edmd: {label} reading order already exists"),
                Err(e) => {
                    // As above: a 2xx here would be read as delivered by marktd.
                    return ApiError::internal(anyhow::Error::from(e).context(format!(
                        "failed to create {label} reading order (malo_id {malo_id})"
                    )))
                    .into_response();
                }
            }
        }
        // Fall through to MSCONS handling (none of these is an MSCONS PID — returns NO_CONTENT)
        return StatusCode::NO_CONTENT.into_response();
    }

    // 3. Route: only process.completed events for known MSCONS PIDs (Messwesen + Redispatch 2.0).
    //
    // `MSCONS_PIDS` = Messwesen PIDs (13005–13027, excl. 13003/13013).
    // `ALL_MSCONS_PIDS` = MSCONS_PIDS + REDISPATCH_MSCONS_PIDS (13020–13023, 13026).
    // Redispatch 2.0 Ausfallarbeit/meteorological data (PIDs 13020–13023, 13026) must also be stored
    // in `edmd` for OLAP aggregation and archive, even though `mako-redispatch` handles the
    // workflow routing (the two concerns are orthogonal).
    // ── A MaLo's master data changed → re-read its Bundesland ────────────
    // The Ersatzwert's day class needs the Land, and the substitution reads it
    // from `malo_bundesland` rather than calling `marktd`; this is where the
    // row is kept current. A `marktd` that cannot be reached answers 5xx, so
    // the fan-out redelivers.
    if ce_type == mako_events::markt::MALO_UPDATED {
        let malo_id = event["subject"].as_str().unwrap_or("").to_owned();
        if malo_id.is_empty() {
            return StatusCode::NO_CONTENT.into_response();
        }
        return match crate::bundesland_cache::refresh(
            state.repo.pool(),
            &state.marktd,
            &state.tenant,
            &malo_id,
        )
        .await
        {
            Ok(_) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => ApiError::internal(e.context(format!("Bundesland of MaLo {malo_id}")))
                .into_response(),
        };
    }

    if ce_type == mako_events::mako::PROCESS_COMPLETED && ALL_MSCONS_PIDS.contains(&pid) {
        return crate::mscons_ingest::ingest(&state, &event, pid).await;
    } else {
        debug!(ce_type, pid, "edmd: event ignored");
    }

    StatusCode::NO_CONTENT.into_response()
}

/// A decimal from a JSON string or number, read from its decimal text.
///
/// A JSON number goes through its own rendering, never through `f64`'s binary
/// expansion: `0.1` must read as `0.1`, not `0.1000000000000000055511151231257827`,
/// because the quantity is billed.
pub(crate) fn json_decimal(v: &serde_json::Value) -> Option<rust_decimal::Decimal> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => {
            let text = n.to_string();
            text.parse()
                .ok()
                .or_else(|| rust_decimal::Decimal::from_scientific(&text).ok())
        }
        _ => None,
    }
}

#[cfg(test)]
mod json_decimal_tests {
    use super::json_decimal;
    use std::str::FromStr as _;

    /// A JSON number is read from its decimal text, so a value `f64` cannot
    /// represent exactly arrives exactly.
    #[test]
    fn a_json_number_keeps_its_decimal_value() {
        let v: serde_json::Value = serde_json::from_str("0.1").unwrap();
        assert_eq!(
            json_decimal(&v),
            Some(rust_decimal::Decimal::from_str("0.1").unwrap())
        );
    }

    #[test]
    fn a_string_an_integer_and_an_exponent_are_read() {
        for (json, want) in [("\"12.345\"", "12.345"), ("42", "42"), ("1e3", "1000")] {
            let v: serde_json::Value = serde_json::from_str(json).unwrap();
            assert_eq!(
                json_decimal(&v),
                Some(rust_decimal::Decimal::from_str(want).unwrap()),
                "{json}"
            );
        }
    }

    #[test]
    fn anything_else_is_none() {
        assert_eq!(json_decimal(&serde_json::Value::Null), None);
        assert_eq!(json_decimal(&serde_json::json!("not a number")), None);
    }
}

/// The reading a completed process orders, as `(anlass, label)`.
///
/// A `de.mako.process.completed` names the **answer** that ended the process —
/// `makopid` is the Prüfidentifikator the NB answered with, not the one the
/// Lieferant sent — and `data.accepted` / `data.outcome` carry the verdict. A
/// Bestätigung der Anmeldung (55002 verb. MaLo, 55078 erz. MaLo) starts supply,
/// a Bestätigung der Abmeldung (55005) ends it (PID-Übersicht 4.0, GPKE
/// Teil 2), and so does the Lieferant's Zustimmung to an NB-initiated
/// Lieferende (55008, Kap. 2.5.2.2 Nr. 2, S. 62); their Ablehnungen (55003,
/// 55080, 55006, 55009) leave supply as it was.
/// The date a reading order is planned for: the `process_date` the GPKE
/// completions carry (`CCYYMMDD`, as the UTILMD states it), else an ISO date
/// under `lieferbeginn_datum` / `lieferende_datum` / `wechseldatum`.
fn ablese_datum(data: &serde_json::Value) -> Option<time::Date> {
    use time::format_description::well_known::Iso8601;
    let ccyymmdd = time::macros::format_description!("[year][month][day]");
    data["process_date"]
        .as_str()
        .and_then(|s| time::Date::parse(s, &ccyymmdd).ok())
        .or_else(|| {
            data["lieferbeginn_datum"]
                .as_str()
                .or_else(|| data["lieferende_datum"].as_str())
                .or_else(|| data["wechseldatum"].as_str())
                .and_then(|s| time::Date::parse(s, &Iso8601::DEFAULT).ok())
        })
}

/// The Marktrolle that ordered a reading: the `marktrole` marktd sets on its
/// fan-out from the workflow that completed — `NB` for the NB's own processes
/// (a 55007 it sent, a 55005 it confirmed), `LF` for the Lieferant's. `None`
/// when the event names no role this table admits.
fn auftraggeber_rolle(event: &serde_json::Value) -> Option<&'static str> {
    match event["marktrole"].as_str()? {
        "NB" => Some("NB"),
        "LF" => Some("LF"),
        _ => None,
    }
}

fn ablese_anlass(
    ce_type: &str,
    pid: u32,
    data: &serde_json::Value,
) -> Option<(&'static str, &'static str)> {
    if ce_type != mako_events::mako::PROCESS_COMPLETED {
        return None;
    }
    let refused =
        data["accepted"].as_bool() == Some(false) || data["outcome"].as_str() == Some("rejected");
    if refused {
        return None;
    }
    match pid {
        55002 | 55078 => Some(("LIEFERBEGINN", "Lieferbeginn")),
        // 55005: the NB confirms the Lieferant's Abmeldung. 55008: the
        // Lieferant agrees to the NB's Lieferende, after which the NB ends the
        // Zuordnung zum Zuordnungsende (GPKE Teil 2 Kap. 2.5.2.2 Nr. 2, S. 62).
        55005 | 55008 => Some(("LIEFERENDE", "Lieferende")),
        // 55007 `ohne_antwort`: the LF did not answer in time and the NB ended
        // the Zuordnung itself (Nr. 3, S. 62). The Ankündigung alone ends
        // nothing.
        55007 if data["ohne_antwort"].as_bool() == Some(true) => Some(("LIEFERENDE", "Lieferende")),
        _ => None,
    }
}

#[cfg(test)]
mod ablese_anlass_tests {
    use super::ablese_anlass;

    /// The completion makod emits: `makopid` is the **answer** PID the NB sent
    /// (`gpke-lf-anmeldung` writes `pid: response_pid`), and `data` carries the
    /// verdict.
    fn completed(pid: u32, accepted: bool) -> serde_json::Value {
        serde_json::json!({
            "pid": pid,
            "malo_id": "51238696012",
            "accepted": accepted,
            "outcome": if accepted { "accepted" } else { "rejected" },
        })
    }

    #[test]
    fn a_confirmed_anmeldung_orders_the_beginnablesung() {
        let t = mako_events::mako::PROCESS_COMPLETED;
        for pid in [55002, 55078] {
            assert_eq!(
                ablese_anlass(t, pid, &completed(pid, true)),
                Some(("LIEFERBEGINN", "Lieferbeginn")),
                "{pid}"
            );
        }
        assert_eq!(
            ablese_anlass(t, 55005, &completed(55005, true)),
            Some(("LIEFERENDE", "Lieferende"))
        );
    }

    /// The Lieferant's Zustimmung to an NB-initiated Lieferende (55008 on
    /// 55007) is the step after which the NB ends the Zuordnung zum
    /// Zuordnungsende (BK6-24-174 GPKE Teil 2 Kap. 2.5.2.2 Nr. 2, S. 62); its
    /// Ablehnung (55009) leaves the LF zugeordnet.
    #[test]
    fn an_accepted_nb_lieferende_orders_the_schlussablesung() {
        let t = mako_events::mako::PROCESS_COMPLETED;
        assert_eq!(
            ablese_anlass(t, 55008, &completed(55008, true)),
            Some(("LIEFERENDE", "Lieferende"))
        );
        assert_eq!(ablese_anlass(t, 55009, &completed(55009, false)), None);
    }

    /// An unanswered 55007 ends the Zuordnung as well (GPKE Teil 2 Kap.
    /// 2.5.2.2 Nr. 3, S. 62): the NB's completion of it orders the
    /// Schlussablesung; a 55007 that is not that completion orders nothing.
    #[test]
    fn an_unanswered_nb_lieferende_orders_the_schlussablesung() {
        let t = mako_events::mako::PROCESS_COMPLETED;
        let mut data = completed(55007, true);
        assert_eq!(ablese_anlass(t, 55007, &data), None);
        data["ohne_antwort"] = serde_json::json!(true);
        assert_eq!(
            ablese_anlass(t, 55007, &data),
            Some(("LIEFERENDE", "Lieferende"))
        );
    }

    /// The completions carry the date the process is about as `process_date`
    /// (`CCYYMMDD`, as the UTILMD states it); the reading is planned for it.
    #[test]
    fn the_reading_is_planned_for_the_process_date() {
        assert_eq!(
            super::ablese_datum(&serde_json::json!({ "process_date": "20260101" })),
            Some(time::macros::date!(2026 - 01 - 01))
        );
        assert_eq!(
            super::ablese_datum(&serde_json::json!({ "lieferende_datum": "2026-02-01" })),
            Some(time::macros::date!(2026 - 02 - 01))
        );
        assert_eq!(super::ablese_datum(&serde_json::json!({})), None);
    }

    /// The NB orders the reading on the 55007 it sent, the LF on its own
    /// completions; a role the table does not admit orders nothing.
    #[test]
    fn the_ordering_role_is_the_completing_process_s() {
        let nb = serde_json::json!({ "marktrole": "NB", "makopid": 55007 });
        let lf = serde_json::json!({ "marktrole": "LF", "makopid": 55008 });
        assert_eq!(super::auftraggeber_rolle(&nb), Some("NB"));
        assert_eq!(super::auftraggeber_rolle(&lf), Some("LF"));
        assert_eq!(super::auftraggeber_rolle(&serde_json::json!({})), None);
        assert_eq!(
            super::auftraggeber_rolle(&serde_json::json!({ "marktrole": "BIKO" })),
            None
        );
    }

    #[test]
    fn a_refusal_orders_no_reading() {
        let t = mako_events::mako::PROCESS_COMPLETED;
        for pid in [55003, 55080, 55006] {
            assert_eq!(ablese_anlass(t, pid, &completed(pid, false)), None, "{pid}");
        }
        // A confirming PID whose verdict says otherwise is not a Bestätigung.
        assert_eq!(ablese_anlass(t, 55002, &completed(55002, false)), None);
    }
}
