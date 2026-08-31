//! NB **Neuanlage** decisions and the `E_0608` 60-Werktage Prüflauf.
//!
//! A Lieferant registers a Marktlokation being commissioned for the first time
//! (UTILMD **55600** verbrauchend / **55601** erzeugend, GPKE Teil 2 § 2.2).
//! The NB answers 55602/55604 or 55603/55605 — but not necessarily today.
//!
//! # Why this is a case log and not a handler
//!
//! `E_0608` Prüfschritte 110 / 590 loop: an Anmeldung whose Marktlokation the NB
//! cannot yet identify must be re-checked **daily for 60 Werktage** and may only
//! be refused (`A07` / `A16`) once that window has run out. Neither of the two
//! answers available on day one is admissible — refusing breaks the Festlegung,
//! confirming assigns a Lieferant to a Marktlokation the NB cannot find — so the
//! case is persisted in `neuanlage_faelle` and re-evaluated by
//! [`run_pruflauf`] until it resolves.
//!
//! That is also why the answer window is „00:00 Uhr des 61. WT nach dem ÜT"
//! (`mako_fristen::antwort`), and not a day.
//!
//! # Identification is the NB's own system
//!
//! A Neuanlage carries address and device data, not a MaLo-ID — the NB matches
//! it against its NIS/GIS, exactly as it provisions `malo_grid`. mako has no
//! address search, so the identification arrives through
//! `PUT /api/v1/neuanlage/{id}/identifikation` (operator or ERP) and the daily
//! run defers until it does.

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_pruefung::nb::neuanlage::{
    Identifikation, NeuanlageAnfrage, NeuanlageBefund, NeuanlageEntscheidung, evaluate_neuanlage,
};
use mako_pruefung::nb::types::{
    ErzeugungsAnmeldung, Geschaeftsvorfall, Marktlokationsart, Veraeusserungsform,
};
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use tracing::{info, warn};
use uuid::Uuid;

use crate::pg::neuanlage::{
    self, NeuanlageFall, NewNeuanlageFall, close_case, escalate_case, open_case, record_pruefung,
};

/// Inbound PIDs this module answers.
pub const NEUANLAGE_PIDS: &[u32] = &[55_600, 55_601];

/// How many cases one Prüflauf pass evaluates. The obligation is daily, not
/// per-second, so the sweep is bounded and simply resumes next pass.
const PRUFLAUF_BATCH: i64 = 500;

/// Configuration for the Neuanlage module.
#[derive(Debug, Clone)]
pub struct NeuanlageModuleConfig {
    pub own_mp_id: String,
    pub tenant: String,
    /// When `false`, a decidable case is recorded and left for an operator
    /// rather than answered. Mirrors `[nb] auto_accept`.
    pub auto_accept: bool,
}

// ── Ingest ────────────────────────────────────────────────────────────────────

/// Open a case for an inbound 55600 / 55601 and run the first Prüfung.
///
/// Returns `true` when the event belonged to this module.
///
/// # Errors
///
/// Propagates database and `makod` transport errors — a lost event is
/// redelivered, a lost case is not.
pub async fn handle_process_initiated(
    event: &serde_json::Value,
    cfg: &NeuanlageModuleConfig,
    pool: &PgPool,
    makod: &MakodClient,
) -> anyhow::Result<bool> {
    let Some(payload) = NeuanlagePayload::parse(event) else {
        return Ok(false);
    };
    // Another operator's message on a shared bus.
    if !payload.grid_operator_gln.is_empty() && payload.grid_operator_gln != cfg.own_mp_id {
        return Ok(false);
    }

    let today = mako_fristen::heute();
    let uebertragungstag = mako_fristen::berlin_date(payload.received_at);
    let letzter_pruefungstag = mako_fristen::add_werktage(
        uebertragungstag,
        mako_pruefung::nb::neuanlage::IDENTIFIKATION_WERKTAGE,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );

    let opened = open_case(
        pool,
        &cfg.tenant,
        &NewNeuanlageFall {
            process_id: payload.process_id,
            pid: payload.pid as i32,
            lf_mp_id: payload.lf_mp_id.clone(),
            marktlokationsart: payload.marktlokationsart,
            veraeusserungsform: payload.veraeusserungsform.clone(),
            uebertragungstag,
            zuordnungsbeginn: payload.zuordnungsbeginn,
            letzter_pruefungstag,
        },
    )
    .await?;

    let Some(id) = opened else {
        // Redelivered — the Prüflauf clock keeps running from the first arrival.
        info!(
            process_id = %payload.process_id,
            "processd Neuanlage: case already open, redelivery ignored"
        );
        return Ok(true);
    };
    info!(
        %id, process_id = %payload.process_id, pid = payload.pid,
        %letzter_pruefungstag,
        "processd Neuanlage: case opened — E_0608 Prüflauf runs to letzter_pruefungstag"
    );

    let fall = neuanlage::fetch_case(pool, id, &cfg.tenant)
        .await?
        .ok_or_else(|| anyhow::anyhow!("case {id} vanished between insert and read"))?;
    evaluate_and_act(&fall, cfg, pool, makod, today).await?;
    Ok(true)
}

// ── The daily Prüflauf ────────────────────────────────────────────────────────

/// Re-evaluate every open case whose Prüfung has not run today.
///
/// Returns how many cases were evaluated. `E_0608` Prüfschritte 110 / 590 make
/// this a **regulatory obligation**, not a housekeeping sweep: the NB owes the
/// Lieferant a daily attempt at identification before it may refuse.
///
/// # Errors
///
/// Propagates database errors. A `makod` dispatch failure is logged and the case
/// stays open, so the next pass retries it.
pub async fn run_pruflauf(
    cfg: &NeuanlageModuleConfig,
    pool: &PgPool,
    makod: &MakodClient,
) -> anyhow::Result<usize> {
    let today = mako_fristen::heute();
    let due = neuanlage::due_for_pruefung(pool, &cfg.tenant, today, PRUFLAUF_BATCH).await?;
    let mut evaluated = 0;
    for fall in &due {
        if let Err(e) = evaluate_and_act(fall, cfg, pool, makod, today).await {
            warn!(id = %fall.id, %e, "processd Neuanlage: Prüfung failed — retried next pass");
            continue;
        }
        evaluated += 1;
    }
    if evaluated > 0 {
        info!(
            evaluated,
            "processd Neuanlage: E_0608 Prüflauf pass complete"
        );
    }
    Ok(evaluated)
}

/// Run `E_0608` for one case and act on the outcome.
async fn evaluate_and_act(
    fall: &NeuanlageFall,
    cfg: &NeuanlageModuleConfig,
    pool: &PgPool,
    makod: &MakodClient,
    today: Date,
) -> anyhow::Result<()> {
    let anfrage = to_anfrage(fall);
    let identifikation = match &fall.malo_id {
        Some(malo_id) => Identifikation::Eindeutig {
            malo_id: malo_id.clone(),
        },
        None => Identifikation::Keine,
    };
    // The facts behind Prüfschritte 40–90 come from the NB's own registry, and
    // mako only holds them once the Marktlokation exists. An identified case
    // therefore escalates unless an operator has confirmed them — which is what
    // `PUT …/identifikation` records.
    let befund: Option<NeuanlageBefund> = fall.malo_id.as_ref().map(|_| NeuanlageBefund {
        nimmt_an_mako_teil: true,
        erstmalige_inbetriebnahme: true,
        lf_bereits_zugeordnet: false,
        im_netzgebiet: true,
        anforderungen_erfuellt: true,
        viertelstundenmessung: true,
    });

    let decision = evaluate_neuanlage(
        &anfrage,
        &identifikation,
        befund.as_ref(),
        today,
        &mako_pruefung::NetzCheckConfig::default(),
    );

    match &decision {
        NeuanlageEntscheidung::Vertagen {
            letzter_pruefungstag,
            verbleibende_werktage,
        } => {
            record_pruefung(pool, fall.id, &cfg.tenant, today).await?;
            info!(
                id = %fall.id, %letzter_pruefungstag, verbleibende_werktage,
                "processd Neuanlage: not identified — deferred per E_0608 Prüfschritt 110/590"
            );
        }
        NeuanlageEntscheidung::Escalate { reason } => {
            escalate_case(pool, fall.id, &cfg.tenant, reason).await?;
            warn!(id = %fall.id, %reason, "processd Neuanlage: escalated");
        }
        NeuanlageEntscheidung::Accept(a) => {
            if cfg.auto_accept {
                dispatch(makod, fall, &a.antwortcode, a.ebd.as_deref(), true, None).await?;
                close_case(pool, fall.id, &cfg.tenant, &a.antwortcode, None).await?;
                info!(id = %fall.id, antwortcode = %a.antwortcode,
                      "processd Neuanlage: Bestätigung dispatched");
            } else {
                record_pruefung(pool, fall.id, &cfg.tenant, today).await?;
                info!(id = %fall.id, "processd Neuanlage: Accept held (auto_accept off)");
            }
        }
        NeuanlageEntscheidung::Reject(r) => {
            dispatch(
                makod,
                fall,
                &r.antwort.antwortcode,
                r.antwort.ebd.as_deref(),
                false,
                Some(&r.detail),
            )
            .await?;
            close_case(
                pool,
                fall.id,
                &cfg.tenant,
                &r.antwort.antwortcode,
                Some(&r.detail),
            )
            .await?;
            info!(id = %fall.id, antwortcode = %r.antwort.antwortcode,
                  "processd Neuanlage: Ablehnung dispatched");
        }
    }
    Ok(())
}

/// Post the Neuanlage answer to `makod`.
///
/// `SG4 STS+E01` is Muss on every Antwortnachricht, so the code and the tree it
/// was resolved against travel with the command; `makod` re-checks both and
/// derives the response PID from the published Cluster.
async fn dispatch(
    makod: &MakodClient,
    fall: &NeuanlageFall,
    antwort_code: &str,
    antwort_ebd: Option<&str>,
    accept: bool,
    detail: Option<&str>,
) -> anyhow::Result<()> {
    let mut payload = serde_json::json!({
        "process_id":   fall.process_id,
        "antwort_code": antwort_code,
        "antwort_tree": mako_pruefung::codes::EBD_NEUANLAGE,
        "zustimmung":   accept,
    });
    if let Some(ebd) = antwort_ebd {
        payload["antwort_ebd"] = serde_json::json!(ebd);
    }
    if let Some(detail) = detail {
        payload["bemerkung"] = serde_json::json!(detail);
        payload["reason"] = serde_json::json!(format!("{antwort_code}: {detail}"));
    }
    let cmd = ForwardCommand {
        marktrolle: Some("NB".to_owned()),
        command: if accept {
            mako_markt::commands::GPKE_NEUANLAGE_BESTAETIGEN
        } else {
            mako_markt::commands::GPKE_NEUANLAGE_ABLEHNEN
        }
        .to_owned(),
        malo_id: fall.malo_id.clone(),
        melo_id: None,
        payload,
    };
    let verb = if accept { "accept" } else { "reject" };
    makod
        .post_command(
            &format!("processd-neuanlage-{verb}-{}", fall.process_id),
            &cmd,
        )
        .await
        .map_err(|e| anyhow::anyhow!("neuanlage dispatch failed: {e}"))?;
    Ok(())
}

fn to_anfrage(fall: &NeuanlageFall) -> NeuanlageAnfrage {
    let art = fall.art();
    let erzeugung = (art == Marktlokationsart::Erzeugend)
        .then(|| {
            fall.veraeusserungsform
                .as_deref()
                .and_then(Veraeusserungsform::from_wire_code)
                .map(|v| ErzeugungsAnmeldung {
                    // A Neuanlage creates the assignment, so it is always the
                    // non-tranchierte Geschäftsvorfall 1 unless the message
                    // named a Tranche — which `E_0608` does not branch on.
                    geschaeftsvorfall: Geschaeftsvorfall::Eins,
                    angemeldete_veraeusserungsform: v,
                    bestehende_veraeusserungsform: None,
                    nicht_eeg_kwkg: false,
                    ausfallverguetung: false,
                })
        })
        .flatten();
    NeuanlageAnfrage {
        pid: fall.pid as u32,
        marktlokationsart: art,
        lf_mp_id: fall.lf_mp_id.clone(),
        zuordnungsbeginn: fall.zuordnungsbeginn,
        uebertragungstag: fall.uebertragungstag,
        erzeugung,
    }
}

// ── Payload ───────────────────────────────────────────────────────────────────

/// Fields of a `de.mako.process.initiated` for a Neuanlage PID.
#[derive(Debug, Clone)]
struct NeuanlagePayload {
    pid: u32,
    process_id: Uuid,
    lf_mp_id: String,
    grid_operator_gln: String,
    zuordnungsbeginn: Date,
    marktlokationsart: Marktlokationsart,
    veraeusserungsform: Option<String>,
    received_at: OffsetDateTime,
}

impl NeuanlagePayload {
    fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !NEUANLAGE_PIDS.contains(&pid) {
            return None;
        }
        let process_id: Uuid = event["subject"].as_str()?.parse().ok()?;
        let zuordnungsbeginn = parse_civil_date(data.get("process_date")?.as_str()?)?;
        let received_at = event["time"]
            .as_str()
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(OffsetDateTime::now_utc);
        Some(Self {
            pid,
            process_id,
            lf_mp_id: data
                .get("new_supplier")
                .or_else(|| data.get("sender"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            grid_operator_gln: data
                .get("grid_operator")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            zuordnungsbeginn,
            // 55601 *is* the Anwendungsfall „Anmeldung neuer erzeugender
            // Marktlokation", so the PID decides the branch.
            marktlokationsart: if pid == 55_601 {
                Marktlokationsart::Erzeugend
            } else {
                Marktlokationsart::Verbrauchend
            },
            veraeusserungsform: data
                .get("veraeusserungsform")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            received_at,
        })
    }
}

/// Every date shape a process payload carries — see [`crate::wire_date`].
fn parse_civil_date(raw: &str) -> Option<Date> {
    crate::wire_date::parse(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_neuanlage_event_and_takes_the_pid_branch() {
        let event = serde_json::json!({
            "makopid": 55_601,
            "subject": "550e8400-e29b-41d4-a716-446655440099",
            "time": "2026-03-04T09:00:00Z",
            "data": {
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20260501",
                "veraeusserungsform": "Z91"
            }
        });
        let p = NeuanlagePayload::parse(&event).expect("parses");
        assert_eq!(p.pid, 55_601);
        assert_eq!(p.marktlokationsart, Marktlokationsart::Erzeugend);
        assert_eq!(p.veraeusserungsform.as_deref(), Some("Z91"));
        assert_eq!(
            p.zuordnungsbeginn,
            Date::from_calendar_date(2026, time::Month::May, 1).expect("valid")
        );
    }

    #[test]
    fn ignores_a_pid_this_module_does_not_answer() {
        let event = serde_json::json!({
            "makopid": 55_001,
            "subject": "550e8400-e29b-41d4-a716-446655440099",
            "data": { "process_date": "20260501" }
        });
        assert!(NeuanlagePayload::parse(&event).is_none());
    }

    /// The Prüflauf window is 60 Werktage from the ÜT, on the BDEW calendar —
    /// so a case opened on a Wednesday is not refusable for roughly three
    /// months.
    #[test]
    fn the_pruflauf_window_is_sixty_werktage_from_the_uet() {
        let ut = Date::from_calendar_date(2026, time::Month::March, 4).expect("valid");
        let letzter = mako_fristen::add_werktage(
            ut,
            mako_pruefung::nb::neuanlage::IDENTIFIKATION_WERKTAGE,
            mako_fristen::HolidayCalendar::BdewMaKo,
        );
        assert!(
            letzter > Date::from_calendar_date(2026, time::Month::May, 20).expect("valid"),
            "60 Werktage is about three calendar months, got {letzter}"
        );
    }
}
