//! LF process decision module — answering the NB-seitiges Lieferende.
//!
//! ## The process
//!
//! GPKE Teil 2 § 2.5 "NB-seitiges Lieferende": the Netzbetreiber announces that
//! the network assignment is ending, and the supplier answers.
//!
//! | PID | Direction | Meaning |
//! |-----|-----------|---------|
//! | **55007** | NB → LF | Ankündigung NB-seitiges Lieferende — **the inbound trigger** |
//! | 55008 | LF → NB | Bestätigung (this module's `einwilligung`) |
//! | 55009 | LF → NB | Ablehnung with an ERC (this module's `ablehnen`) |
//!
//! The trigger is 55007. 55008/55009 are the *answers* — `makod` never spawns a
//! process from them (`gpke-lf-abmeldung` accepts 55007 alone, anything else is
//! `pid_not_in_spawn_table`), so a module keyed on 55008 waits for an event that
//! cannot arrive.
//!
//! ## Two clocks, and which one bounds the queue
//!
//! An inbound UTILMD starts two independent timers, and conflating them is the
//! mistake this note exists to prevent:
//!
//! | Clock | Window | Owner |
//! |-------|--------|-------|
//! | Technical acknowledgement (APERAK) | **45 min** on weekdays, Sunday 12:00 Berlin for a Saturday arrival (APERAK AHB 1.0 § 2.4.1) | **`makod`**, automatically |
//! | Business answer (55008/55009) | **05:00 Uhr des 1. WT nach dem ÜT** (GPKE Teil 2 § 2.5.2 SD Prozessschritt 2) | this module / the operator |
//!
//! The queue is bounded by the **business** window, read from
//! [`crate::fristen`] rather than approximated: a Friday-afternoon Ankündigung
//! is answerable until Monday 05:00, a Tuesday-evening one until Wednesday
//! 05:00 — nine hours.
//!
//! ## Not this module
//!
//! - **PID 55010** "Anfrage zur Beendigung der Zuordnung" (EBD **E_0624**) is a
//!   *different* GPKE process, answered 55011/55012 — see the sibling
//!   [`BEENDIGUNG_ZUORDNUNG`] descriptor below.
//! - **PID 55013** (Zuordnung EOG, NB → E/G) belongs to `mako-gpke::eog` and,
//!   on the NB side, to [`crate::eog_module`].
//!
//! ## Decision logic
//!
//! ```text
//! GET /api/v1/versorgung/{malo_id}
//!   supplying + scenario "standard"          → Bestätigung (55008)
//!   supplying + scenario "vertragsbindung"   → Ablehnung A35 (55009)
//!   supplying + scenario "einzug"            → Ablehnung A32 (55009)
//!   supplying + scenario "ersatzversorgung"  → Bestätigung
//!   MaLo unknown / not supplying / LF mismatch → approval_queue
//! ```
//!
//! ## Regulatory basis
//!
//! - **BK6-24-174 GPKE Teil 2 § 2.5** — NB-seitiges Lieferende; the answer
//!   Fristen are in [`mako_gpke::antwortfrist`]
//! - **EBD 4.3** — `E_0609` (55007 → 55008/55009), `E_0624` (55010 → 55011/55012)
//! - **APERAK AHB 1.0 § 2.4.1** — the separate 45-minute technical window

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use secrecy::SecretString;

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the LF module.
#[derive(Debug, Clone)]
pub struct LfModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    /// When `true`, dispatch `einwilligung`/`ablehnen` automatically.
    pub auto_respond: bool,
}

// ── marktd reader (shared with nb_module via direct reqwest) ──────────────────

// ── NB-seitiges Lieferende payload ────────────────────────────────────────────────────────────

/// An NB-initiated GPKE process this module answers on the LF's behalf.
///
/// Both processes have the same shape — the NB asks the supplier to agree that
/// something ends, the supplier answers yes or no from its own supply state —
/// so they share the evaluation and differ only in which PID arrives and which
/// pair of `makod` commands carries the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LfAntwortProcess {
    /// The inbound PID that triggers it.
    pub trigger_pid: u32,
    /// Human-readable process name, for logs and queue reasons.
    pub name: &'static str,
    /// EBD that governs the decision, where one is published.
    pub ebd: Option<&'static str>,
    /// `makod` command for the positive answer.
    pub bestaetigen: &'static str,
    /// `makod` command for the negative answer.
    pub ablehnen: &'static str,
}

/// **Ankündigung NB-seitiges Lieferende** (GPKE Teil 2 § 2.5).
///
/// Inbound 55007, answered 55008 (Bestätigung) / 55009 (Ablehnung).
pub const NB_LIEFERENDE: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 55_007,
    name: "NB-seitiges Lieferende",
    // „Abmeldung prüfen" on the LF side — a *different* tree from the NB's
    // E_0607 of the same name, and different again from the E_0624 that governs
    // the Beendigung der Zuordnung below.
    ebd: Some("E_0609"),
    bestaetigen: mako_markt::commands::GPKE_NB_LIEFERENDE_BESTAETIGEN,
    ablehnen: mako_markt::commands::GPKE_NB_LIEFERENDE_ABLEHNEN,
};

/// **Anfrage zur Beendigung der Zuordnung** — the process EBD **E_0624**
/// actually governs ("Anfrage zur Beendigung der Zuordnung prüfen").
///
/// Inbound 55010, answered 55011 (Bestätigung) / 55012 (Ablehnung). The label
/// belongs here and not on [`NB_LIEFERENDE`], which is a different process with
/// different PIDs and a different answer pair.
pub const BEENDIGUNG_ZUORDNUNG: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 55_010,
    name: "Beendigung der Zuordnung",
    ebd: Some("E_0624"),
    bestaetigen: mako_markt::commands::GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    ablehnen: mako_markt::commands::GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
};

/// Every process this module answers, in match order.
pub const LF_ANTWORT_PROCESSES: &[LfAntwortProcess] = &[NB_LIEFERENDE, BEENDIGUNG_ZUORDNUNG];

/// Parsed fields from a `de.mako.process.initiated` for one of
/// [`LF_ANTWORT_PROCESSES`].
#[derive(Debug, Clone)]
pub struct NbLieferendePayload {
    /// Which process this event belongs to.
    pub process: LfAntwortProcess,
    pub process_id: Uuid,
    pub malo_id: String,
    /// GLN of the grid operator who sent the NB-seitiges Lieferende.
    pub initiating_nb_gln: String,
    /// Requested Lieferende date.
    pub lieferende_date: Option<time::Date>,
    /// Whether this is a Vertragsbindung or Einzug scenario.
    pub scenario: NbLieferendeScenario,
    /// The business answer deadline and the operator window derived from it.
    pub window: crate::fristen::OperatorWindow,
}

/// The situation the NB states in the Ankündigung, which decides the answer.
///
/// The variants are exactly the ones the AHB's `scenario` marker can carry —
/// there is no `Unknown`, because nothing could construct it and a variant no
/// value reaches is a branch that only ever reads as covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NbLieferendeScenario {
    /// An ordinary end of the assignment — the supplier agrees.
    Standard,
    /// The Anschlussnutzer moved in and a new supply has begun; the supplier
    /// refuses with `A32`.
    Einzug,
    /// The MaLo passes into the statutory fallback supply — the supplier
    /// agrees, whatever its own contract says.
    Ersatzversorgung,
    /// A running Vertragsbindung (Mindestvertragslaufzeit / Kündigungsfrist)
    /// that the announced Zuordnungsende would break; the supplier refuses with
    /// `A35`.
    Vertragsbindung,
}

impl NbLieferendePayload {
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())?;
        let process = *LF_ANTWORT_PROCESSES
            .iter()
            .find(|p| u64::from(p.trigger_pid) == pid)?;

        let subject = event["subject"].as_str()?;
        let process_id: Uuid = subject.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let initiating_nb_gln = data
            .get("grid_operator")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        let scenario_str = data
            .get("scenario")
            .and_then(|v| v.as_str())
            .unwrap_or("standard");
        let scenario = match scenario_str.to_ascii_lowercase().as_str() {
            "einzug" => NbLieferendeScenario::Einzug,
            "ersatzversorgung" => NbLieferendeScenario::Ersatzversorgung,
            "vertragsbindung" => NbLieferendeScenario::Vertragsbindung,
            _ => NbLieferendeScenario::Standard,
        };

        let lieferende_date = data
            .get("lieferende")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                if s.len() == 8 {
                    let fmt = time::macros::format_description!("[year][month][day]");
                    time::Date::parse(s, &fmt).ok()
                } else {
                    let fmt = time::macros::format_description!("[year]-[month]-[day]");
                    time::Date::parse(s, &fmt).ok()
                }
            });

        // The **business** answer window, from the same GPKE Teil 2 table
        // `makod` registers the process deadline from. The 45-minute APERAK
        // window is a different clock on the same message and is `makod`'s.
        let event_time = event["time"]
            .as_str()
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(OffsetDateTime::now_utc);
        let window = crate::fristen::operator_window(process.trigger_pid, event_time);

        Some(Self {
            process,
            process_id,
            malo_id,
            initiating_nb_gln,
            lieferende_date,
            scenario,
            window,
        })
    }
}

// ── LF decision ───────────────────────────────────────────────────────────────

/// Outcome of the NB-seitiges-Lieferende evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LfDecision {
    /// Dispatch `einwilligung` (consent to Abmeldung).
    Einwilligung,
    /// Dispatch `ablehnen` with `erc_code` (A32 = Einzug, A35 = Vertragsbindung).
    Ablehnen { erc_code: String },
    /// Enqueue for ERP review.
    Escalate { reason: String },
}

/// Evaluate the LF's answer from the current VersorgungsStatus.
fn evaluate_lf_antwort(
    payload: &NbLieferendePayload,
    versorgung: Option<&VersorgungsStatusRecord>,
    own_mp_id: &str,
) -> LfDecision {
    let Some(vs) = versorgung else {
        return LfDecision::Escalate {
            reason: format!(
                "MaLo {} not found in master data. Cannot auto-decide NB-seitiges Lieferende.",
                payload.malo_id
            ),
        };
    };

    // Verify this LF is actually supplying the MaLo.
    if vs.lieferstatus != LieferStatus::Beliefert
        && vs.lieferstatus != LieferStatus::Grundversorgung
        && vs.lieferstatus != LieferStatus::Ersatzversorgung
    {
        return LfDecision::Escalate {
            reason: format!(
                "MaLo {} is not in Beliefert/Grundversorgung/Ersatzversorgung state \
                 (current: {}). Cannot auto-decide.",
                payload.malo_id, vs.lieferstatus
            ),
        };
    }

    // Verify the LF GLN matches our own.
    if vs.lf_mp_id.as_deref().is_some_and(|lf| lf != own_mp_id) {
        let active_lf = vs.lf_mp_id.as_deref().unwrap_or("");
        return LfDecision::Escalate {
            reason: format!(
                "MaLo {} is supplied by {} but our GLN is {}. \
                 Cannot auto-decide — LF mismatch.",
                payload.malo_id, active_lf, own_mp_id
            ),
        };
    }

    // Apply the scenario rules. Every branch is reachable: the guard above has
    // already narrowed `lieferstatus` to Beliefert / Grundversorgung /
    // Ersatzversorgung, all three of which are a supply this LF may end.
    match payload.scenario {
        NbLieferendeScenario::Einzug => LfDecision::Ablehnen {
            erc_code: ERC_EINZUG.to_owned(),
        },
        NbLieferendeScenario::Vertragsbindung => LfDecision::Ablehnen {
            erc_code: ERC_VERTRAGSBINDUNG.to_owned(),
        },
        // The statutory fallback supply is not something a supplier can refuse.
        NbLieferendeScenario::Ersatzversorgung | NbLieferendeScenario::Standard => {
            LfDecision::Einwilligung
        }
    }
}

/// `A32` — the Anschlussnutzer moved in and a new supply has begun, so the
/// announced Beendigung cannot be agreed to.
const ERC_EINZUG: &str = "A32";
/// `A35` — a running Vertragsbindung prevents the announced Zuordnungsende.
const ERC_VERTRAGSBINDUNG: &str = "A35";

// ── process_lf_antwort ─────────────────────────────────────────────────────────────

/// Handle one `de.mako.process.initiated` for any of [`LF_ANTWORT_PROCESSES`].
///
/// Returns `true` if the event was handled (even if escalated), `false` if its
/// PID belongs to none of them.
pub async fn process_lf_antwort(
    event: &serde_json::Value,
    config: &LfModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    queue: &PgApprovalQueue,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let Some(payload) = NbLieferendePayload::parse(event) else {
        return Ok(false);
    };

    info!(
        process_id = %payload.process_id,
        malo_id = %payload.malo_id,
        pid = payload.process.trigger_pid,
        process = payload.process.name,
        "processd LF: evaluating"
    );

    // ── Fetch VersorgungsStatus ────────────────────────────────────────────
    let versorgung = reader.get_versorgung(&payload.malo_id).await.inspect_err(
        |e| warn!(%e, malo_id = %payload.malo_id, "processd LF: marktd fetch failed"),
    )?;

    let decision = evaluate_lf_antwort(&payload, versorgung.as_ref(), &config.own_mp_id);

    info!(
        process_id = %payload.process_id,
        malo_id = %payload.malo_id,
        process = payload.process.name,
        outcome = ?decision,
        "processd LF: decision"
    );

    // The queue entry expires with the headroom `crate::fristen` applies, so an
    // operator acting on it still has time for the answer to reach the NB.
    let enqueue = async |reason: String| -> Result<(), sqlx::Error> {
        let entry = ApprovalQueueEntry::pending(
            payload.process_id,
            i32::try_from(payload.process.trigger_pid).unwrap_or(0),
            Some(payload.malo_id.clone()),
            format!(
                "{reason} (Antwortfrist {}: {})",
                payload.window.deadline, payload.window.source
            ),
            payload.window.expires_at,
            config.tenant.clone(),
        )
        .with_commands(
            payload.process.bestaetigen,
            payload.process.ablehnen,
            Some("LF"),
        );
        queue
            .enqueue(&entry)
            .await
            .inspect_err(|e| warn!(%e, "processd LF: failed to enqueue approval entry"))
    };

    match &decision {
        LfDecision::Einwilligung => {
            if config.auto_respond {
                let cmd = ForwardCommand {
                    marktrolle: None,
                    command: payload.process.bestaetigen.to_owned(),
                    malo_id: Some(payload.malo_id.clone()),
                    melo_id: None,
                    payload: serde_json::json!({
                        "process_id": payload.process_id,
                        "lieferende": payload.lieferende_date,
                    }),
                };
                makod
                    .post_command(
                        &format!("processd-lf-einwilligung-{}", payload.process_id),
                        &cmd,
                    )
                    .await
                    .inspect_err(|e| warn!(%e, "processd LF: einwilligung dispatch failed"))?;
                info!(process_id = %payload.process_id, "processd LF: dispatched einwilligung");
            } else {
                // auto_respond off is "operator decides", not "nobody answers":
                // without a queue row the NB-seitiges Lieferende goes unanswered and unseen.
                enqueue(format!(
                    "auto_respond disabled — decidable NB-seitiges Lieferende for MaLo {}: Einwilligung",
                    payload.malo_id
                ))
                .await?;
            }
        }
        LfDecision::Ablehnen { erc_code } => {
            if config.auto_respond {
                let cmd = ForwardCommand {
                    marktrolle: None,
                    command: payload.process.ablehnen.to_owned(),
                    malo_id: Some(payload.malo_id.clone()),
                    melo_id: None,
                    payload: serde_json::json!({
                        "process_id": payload.process_id,
                        "erc_code": erc_code,
                    }),
                };
                makod
                    .post_command(
                        &format!("processd-lf-ablehnen-{}", payload.process_id),
                        &cmd,
                    )
                    .await
                    .inspect_err(|e| warn!(%e, "processd LF: ablehnen dispatch failed"))?;
                info!(process_id = %payload.process_id, %erc_code, "processd LF: dispatched ablehnen");
            } else {
                enqueue(format!(
                    "auto_respond disabled — decidable NB-seitiges Lieferende for MaLo {}: Ablehnung {erc_code}",
                    payload.malo_id
                ))
                .await?;
            }
        }
        LfDecision::Escalate { reason } => {
            warn!(
                process_id = %payload.process_id,
                malo_id = %payload.malo_id,
                %reason,
                "processd LF: NB-seitiges Lieferende escalated — creating approval_queue entry"
            );
            enqueue(reason.clone()).await?;
        }
    }

    Ok(true)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::domain::MaloId;
    use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
    use time::OffsetDateTime;

    fn make_vs(status: LieferStatus, lf_mp_id: Option<&str>) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696012".parse::<MaloId>().unwrap(),
            lieferstatus: status,
            lf_mp_id: lf_mp_id.map(ToOwned::to_owned),
            lf_mp_id_next: None,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: "9900000000001".to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        }
    }

    fn make_payload(scenario: NbLieferendeScenario) -> NbLieferendePayload {
        NbLieferendePayload {
            process: NB_LIEFERENDE,
            process_id: Uuid::new_v4(),
            malo_id: "51238696012".to_owned(),
            initiating_nb_gln: "9900000000001".to_owned(),
            lieferende_date: None,
            scenario,
            window: crate::fristen::operator_window(
                NB_LIEFERENDE.trigger_pid,
                OffsetDateTime::now_utc(),
            ),
        }
    }

    #[test]
    fn beliefert_standard_einwilligung() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900357000004"));
        let payload = make_payload(NbLieferendeScenario::Standard);
        let result = evaluate_lf_antwort(&payload, Some(&vs), "9900357000004");
        assert_eq!(result, LfDecision::Einwilligung);
    }

    #[test]
    fn einzug_ablehnen_a32() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900357000004"));
        let payload = make_payload(NbLieferendeScenario::Einzug);
        let result = evaluate_lf_antwort(&payload, Some(&vs), "9900357000004");
        assert_eq!(
            result,
            LfDecision::Ablehnen {
                erc_code: "A32".to_owned()
            }
        );
    }

    #[test]
    fn unknown_malo_escalates() {
        let payload = make_payload(NbLieferendeScenario::Standard);
        let result = evaluate_lf_antwort(&payload, None, "9900357000004");
        assert!(matches!(result, LfDecision::Escalate { .. }));
    }

    #[test]
    fn wrong_lf_gln_escalates() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900999000001")); // different LF
        let payload = make_payload(NbLieferendeScenario::Standard);
        let result = evaluate_lf_antwort(&payload, Some(&vs), "9900357000004"); // own_mp_id differs
        assert!(matches!(result, LfDecision::Escalate { .. }));
    }

    /// A35 is only reachable while the scenario enum carries a
    /// `Vertragsbindung` variant for the parser to produce.
    #[test]
    fn vertragsbindung_ablehnen_a35() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900357000004"));
        let payload = make_payload(NbLieferendeScenario::Vertragsbindung);
        assert_eq!(
            evaluate_lf_antwort(&payload, Some(&vs), "9900357000004"),
            LfDecision::Ablehnen {
                erc_code: "A35".to_owned()
            }
        );
    }

    /// The scenario marker is matched case-insensitively — the AHB does not fix
    /// its casing and the adapters have emitted both.
    #[test]
    fn the_scenario_marker_is_case_insensitive() {
        for raw in ["Vertragsbindung", "VERTRAGSBINDUNG", "vertragsbindung"] {
            let event = serde_json::json!({
                "makopid": 55_007,
                "subject": Uuid::new_v4().to_string(),
                "data": { "malo_id": "51238696012", "scenario": raw },
            });
            let p = NbLieferendePayload::parse(&event).expect("parses");
            assert_eq!(p.scenario, NbLieferendeScenario::Vertragsbindung, "{raw}");
        }
    }

    /// A MaLo in the statutory fallback supply must not escalate on a Standard
    /// Ankündigung: the guard admits Ersatzversorgung, so the scenario branch
    /// has to as well.
    #[test]
    fn ersatzversorgung_standard_is_einwilligung() {
        let vs = make_vs(LieferStatus::Ersatzversorgung, Some("9900357000004"));
        let payload = make_payload(NbLieferendeScenario::Standard);
        assert_eq!(
            evaluate_lf_antwort(&payload, Some(&vs), "9900357000004"),
            LfDecision::Einwilligung
        );
    }

    /// The queue window comes from the GPKE table: a Friday-afternoon
    /// Ankündigung is answerable on Monday, not on Saturday.
    #[test]
    fn the_answer_window_is_the_next_werktag_at_0500() {
        use time::{Date, Month, Time};
        let friday = OffsetDateTime::new_utc(
            Date::from_calendar_date(2026, Month::March, 6).expect("valid date"),
            Time::from_hms(13, 0, 0).expect("valid time"),
        );
        let w = crate::fristen::operator_window(NB_LIEFERENDE.trigger_pid, friday);
        assert!(w.is_regulatory);
        assert_eq!(
            w.deadline.date(),
            Date::from_calendar_date(2026, Month::March, 9).expect("valid date"),
            "Monday, not Saturday"
        );
    }

    #[test]
    fn grundversorgung_einwilligung() {
        let vs = make_vs(LieferStatus::Grundversorgung, Some("9900357000004"));
        let payload = make_payload(NbLieferendeScenario::Standard);
        let result = evaluate_lf_antwort(&payload, Some(&vs), "9900357000004");
        assert_eq!(result, LfDecision::Einwilligung);
    }
}
