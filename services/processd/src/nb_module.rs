//! NB process decision module — the Netzbetreiber's own GPKE / GeLi Gas
//! answer obligations.
//!
//! # What the NB owes an answer to
//!
//! | Inbound PID | Process | Answers | EBD | Frist |
//! |---|---|---|---|---|
//! | **55001** | Anmeldung verb. MaLo (Lieferbeginn) | 55002 / 55003 | `E_0622` | 11:00 Uhr des 1. WT nach dem ÜT |
//! | **55077** | Anmeldung erz. MaLo (Lieferbeginn) | 55078 / 55080 | `E_0622` | 11:00 Uhr des 1. WT nach dem ÜT |
//! | **55004** | Abmeldung (Lieferende von LF an NB) | 55005 / 55006 | `E_0607` | 06:00 Uhr des 1. WT nach dem ÜT |
//! | **44001** | Anmeldung NN (Gas Lieferbeginn) | 44002 / 44003 | — | Ablauf des 4. Werktags |
//! | **44004** | Abmeldung NN (Gas Lieferende) | 44005 / 44006 | — | Ablauf des 3. Werktags |
//!
//! Every Frist comes from [`crate::fristen`], which reads the same tables
//! `makod` registers the process deadline from.
//!
//! ## What is deliberately *not* here
//!
//! **55016 „Kündigung" is not an NB process** and is answered by no role here.
//! The Anwendungsübersicht der Prüfidentifikatoren 4.0 (lfd. Nr. 20030) has it
//! going **LFN → LFA**, answered 55017/55018 by the *Altlieferant* under EBD
//! `E_0614`. Evaluating it here would make an `nb-only` binary answer a
//! supplier-role message — the § 7 EnWG separation the Cargo features exist for
//! — with grid-topology checks the LFA has no basis for. `ROADMAP.md` records
//! what the LFA answer path needs first.
//!
//! # Decision pipeline
//!
//! ```text
//! Anmeldung (55001 / 55077 / 44001)          Abmeldung (55004 / 44004)
//!   → GET /api/v1/versorgung/{malo}            → GET /api/v1/versorgung/{malo}
//!   → GET /api/v1/malos/{malo}/grid
//!   → GET /api/v1/partners/{lf}
//!   → mako_pruefung::evaluate                   → mako_pruefung::evaluate_abmeldung
//!       Accept   → bestaetigen [auto_accept]       Accept   → bestaetigen [auto_accept]
//!                  else approval_queue                        else approval_queue
//!       Reject   → ablehnen (ERC)                  Reject   → ablehnen (ERC)
//!       Escalate → approval_queue                  Escalate → approval_queue
//! ```
//!
//! The two decision trees have **separate ERC code spaces** — `A02` is
//! „Marktlokation nimmt nicht an der Marktkommunikation teil" in `E_0622` and
//! „Vorlauffrist nicht eingehalten" in `E_0607` — which is why they are
//! separate functions in `mako-pruefung` rather than one with a flag.
//!
//! # Regulatory basis
//!
//! - GPKE: BK6-24-174 Teil 2 (SD Lieferbeginn, SD Lieferende von LF an NB)
//! - GeLi Gas: BK7-24-01-009 Kap. 3.2.2 / 3.2.3
//! - EBD 4.3 Kap. 6.6.1 (`E_0622`), 6.3.1 (`E_0607`)
//! - § 20 EnWG parity: `initiator_is_affiliate` recorded on every decision

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_pruefung::nb::types::RejectReason;
use mako_pruefung::{AnmeldungAnfrage, Messtyp, NbEntscheidung};
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use mako_markt::domain::Sparte;
use secrecy::SecretString;

use crate::pg::anmeldung::{AnmeldungDecision, AnmeldungDecisionRecord, PgAnmeldungRepository};
use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the NB module.
#[derive(Debug, Clone)]
pub struct NbModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    pub auto_accept: bool,
    /// Gas Bearbeitungsfrist (WT) added to the 6-week retroactive Anmeldung
    /// window. Defaults to [`mako_pruefung::nb::anmeldung::GAS_BEARBEITUNGSFRIST_WT_DEFAULT`]
    /// (3 WT); operators whose AWH reading differs may override it.
    pub gas_bearbeitungsfrist_wt: u32,
}

impl NbModuleConfig {
    /// Build the pure-library [`mako_pruefung::NetzCheckConfig`] from this config.
    #[must_use]
    pub fn netz_check_config(&self) -> mako_pruefung::NetzCheckConfig {
        mako_pruefung::NetzCheckConfig {
            gas_bearbeitungsfrist_wt: self.gas_bearbeitungsfrist_wt,
            ..mako_pruefung::NetzCheckConfig::default()
        }
    }
}

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Inbound **Anmeldung** PIDs the NB answers.
///
/// 55001 verbrauchende MaLo, 55077 erzeugende MaLo (both LFN → NB, GPKE Teil 2
/// SD Lieferbeginn), 44001 Anmeldung NN (GeLi Gas 3.0 Kap. 3.2.3).
///
/// 55016 is **not** here: it is the Kündigung, LFN → LFA, and belongs to the
/// supplier role (see the module docs).
pub const ANMELDUNG_PIDS: &[u32] = &[55_001, 55_077, 44_001];

/// Inbound **Abmeldung** PIDs the NB answers.
///
/// 55004 „Abmeldung" (LF → NB, GPKE Teil 2 SD Lieferende von LF an NB) and
/// 44004 „Abmeldung NN" (GeLi Gas 3.0 Kap. 3.2.2). Neither was routed anywhere
/// before, so every Lieferende a supplier initiated ran out its Frist unseen.
pub const ABMELDUNG_PIDS: &[u32] = &[55_004, 44_004];

/// Every inbound PID this module answers.
#[must_use]
pub fn answered_pids() -> Vec<u32> {
    let mut v: Vec<u32> = ANMELDUNG_PIDS
        .iter()
        .chain(ABMELDUNG_PIDS)
        .copied()
        .collect();
    v.sort_unstable();
    v
}

/// The Sparte an NB PID belongs to — Strom in the 55xxx band, Gas in 44xxx.
const fn sparte_of(pid: u32) -> Sparte {
    if pid >= 44_000 && pid < 45_000 {
        Sparte::Gas
    } else {
        Sparte::Strom
    }
}

// ── NB module payload ─────────────────────────────────────────────────────────

/// Fields extracted from a `de.mako.process.initiated` CloudEvent payload
/// for a Lieferbeginn PID.
#[derive(Debug, Clone)]
pub struct AnmeldungPayload {
    pub pid: u32,
    pub process_id: Uuid,
    pub malo_id: String,
    pub new_supplier_gln: String,
    pub grid_operator_gln: String,
    pub bilanzierungsgebiet: Option<String>,
    pub process_date: time::Date,
    /// SG4 STS Transaktionsgrund (DE9013) — e.g. `E01` Ein-/Auszug,
    /// `E03` Lieferantenwechsel. Drives the date-plausibility rules.
    pub transaktionsgrund: Option<String>,
    /// `true` when the Anmeldung is for an Erzeugende (EEG-/KWKG-Einspeise-)
    /// Marktlokation (STS 9013=ZW3). Triggers the §10c EEG Monatserster date rule.
    pub ist_erzeugende_marktlokation: bool,
    /// Bilanzierungsmethode from UTILMD TM+EM (`SLP` | `RLM` | `IMS`).
    pub bilanzierungsmethode: Option<String>,
}

impl AnmeldungPayload {
    /// Parse from the `data` field of a `de.mako.process.initiated` CloudEvent.
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;

        if !ANMELDUNG_PIDS.contains(&pid) {
            return None;
        }

        let subject = event["subject"].as_str()?;
        let process_id: Uuid = subject.parse().ok()?;

        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let new_supplier_gln = data.get("new_supplier")?.as_str()?.to_owned();
        let grid_operator_gln = data.get("grid_operator")?.as_str()?.to_owned();
        let bilanzierungsgebiet = data
            .get("bilanzierungsgebiet")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let transaktionsgrund = data
            .get("transaktionsgrund")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let ist_erzeugende_marktlokation = data
            .get("ist_erzeugende_marktlokation")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let bilanzierungsmethode = data
            .get("bilanzierungsmethode")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        let date_str = data.get("process_date")?.as_str()?;
        let process_date = if date_str.len() == 8 {
            let fmt = time::macros::format_description!("[year][month][day]");
            time::Date::parse(date_str, &fmt).ok()?
        } else {
            let fmt = time::macros::format_description!("[year]-[month]-[day]");
            time::Date::parse(date_str, &fmt).ok()?
        };

        Some(Self {
            pid,
            process_id,
            malo_id,
            new_supplier_gln,
            grid_operator_gln,
            bilanzierungsgebiet,
            process_date,
            transaktionsgrund,
            ist_erzeugende_marktlokation,
            bilanzierungsmethode,
        })
    }

    /// Derive `AnmeldungAnfrage` for passing to `mako-pruefung`.
    pub fn into_anfrage(self) -> AnmeldungAnfrage {
        let sparte = sparte_of(self.pid);
        // PID 55077 *is* the „Anmeldung erz. MaLo" use case, so the § 10c EEG
        // Monatserster rule applies whether or not the adapter also surfaced
        // the ZW3 Transaktionsgrundergänzung. Relying on the flag alone let a
        // message without it take the ordinary LFW24 Werktag rule.
        let ist_erzeugende_marktlokation = self.ist_erzeugende_marktlokation || self.pid == 55_077;
        // Messtyp from the UTILMD TM+EM marker carried in the payload
        // (Z01=SLP, Z02=RLM, Z04=IMS → adapter emits "SLP"/"RLM"/"IMS").
        // Default SLP when absent — the conservative Vorlauffrist bound.
        let messtyp = match self.bilanzierungsmethode.as_deref() {
            Some("RLM") => Messtyp::Rlm,
            Some("IMS") => Messtyp::Imsys,
            _ => Messtyp::Slp,
        };
        AnmeldungAnfrage {
            pid: self.pid,
            process_id: self.process_id,
            malo_id: self.malo_id,
            new_supplier_gln: self.new_supplier_gln,
            grid_operator_gln: self.grid_operator_gln,
            bilanzierungsgebiet: self.bilanzierungsgebiet,
            process_date: self.process_date,
            sparte,
            messtyp,
            transaktionsgrund: self.transaktionsgrund,
            ist_erzeugende_marktlokation,
        }
    }
}

// ── evaluate_and_decide ───────────────────────────────────────────────────────

/// Decide one `de.mako.process.initiated` event addressed to the NB.
///
/// Routes to the Anmeldung pipeline (55001 / 55077 / 44001) or the Abmeldung
/// pipeline (55004 / 44004). Returns `true` when this module handled the event
/// — including when it escalated — and `false` when the PID belongs to another
/// role or another module.
pub async fn evaluate_and_decide(
    event: &serde_json::Value,
    config: &NbModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    repo: &PgAnmeldungRepository,
    queue: &PgApprovalQueue,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(payload) = AbmeldungPayload::parse(event) {
        return decide_abmeldung(payload, event, config, reader, makod, repo, queue).await;
    }

    // ── 1. Parse payload ──────────────────────────────────────────────────
    let Some(payload) = AnmeldungPayload::parse(event) else {
        return Ok(false);
    };

    // ── 2. Misdirection check ─────────────────────────────────────────────
    // Fast pre-check: if the event is not for our GLN, skip silently.
    if !payload.grid_operator_gln.is_empty() && payload.grid_operator_gln != config.own_mp_id {
        return Ok(false);
    }

    let initiator_is_affiliate = payload.new_supplier_gln == config.own_mp_id;
    let pid = payload.pid;
    let process_id = payload.process_id;
    let malo_id = payload.malo_id.clone();
    let lf_mp_id = payload.new_supplier_gln.clone();
    // The answer Frist runs from receipt of the market message; the CloudEvent
    // `time` is when makod emitted it.
    let received_at = event["time"]
        .as_str()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);
    let payload_meta = AnmeldungMeta {
        pid,
        process_id,
        malo_id: malo_id.clone(),
        received_at,
    };

    info!(
        %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
        "processd NB: evaluating Anmeldung"
    );

    // ── 3. Fetch marktd data ──────────────────────────────────────────────
    let versorgung = reader
        .get_versorgung(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd versorgung fetch failed"))?;

    let malo = reader
        .get_malo(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd malo fetch failed"))
        .unwrap_or(None);

    let grid = reader
        .get_malo_grid(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd grid fetch failed"))?;

    let partner_known = reader.partner_known(&lf_mp_id).await.inspect_err(
        |e| warn!(%e, lf_mp_id = %lf_mp_id, "processd NB: marktd partner check failed"),
    )?;

    let anfrage = payload.into_anfrage();
    let now = OffsetDateTime::now_utc();

    // ── 4. Evaluate ───────────────────────────────────────────────────────
    // Build a grid record for `mako-pruefung` from the best available source:
    //  1. `malo_grid` side table (NB-role PUT provisioning) — most authoritative
    //  2. `malo.bilanzierungsgebiet` (B1 typed extraction) — fallback when the
    //     malo_grid record is absent; raises STP from ~60% to ~80% for SLP MaLos
    let vs_ref = versorgung.as_ref();
    let grid_nc: Option<mako_pruefung::MaloGridRecord> = if grid.is_some() {
        grid.as_ref().map(Into::into)
    } else if let Some(ref m) = malo {
        if m.bilanzierungsgebiet.is_some() || m.netzebene.is_some() {
            Some(mako_pruefung::MaloGridRecord {
                malo_id: malo_id.clone(),
                nb_mp_id: anfrage.grid_operator_gln.clone(),
                bilanzierungsgebiet: m.bilanzierungsgebiet.clone(),
                netzgebiet: None,
            })
        } else {
            None
        }
    } else {
        None
    };
    let grid_ref = grid_nc.as_ref();

    let result = mako_pruefung::evaluate(
        &anfrage,
        vs_ref,
        grid_ref,
        partner_known,
        now,
        &config.netz_check_config(),
    );

    info!(
        %process_id, pid, %malo_id,
        grid_source = if grid.is_some() { "malo_grid" } else if grid_nc.is_some() { "malo_typed" } else { "none" },
        outcome = ?result,
        "processd NB: `mako-pruefung` result"
    );

    // ── 5. Persist decision ───────────────────────────────────────────────
    let (decision, antwortcode, detail) = match &result {
        NbEntscheidung::Accept => (AnmeldungDecision::Accept, None, None),
        NbEntscheidung::Reject(RejectReason {
            antwortcode,
            detail,
            ..
        }) => (
            AnmeldungDecision::Reject,
            Some(antwortcode.clone()),
            Some(detail.clone()),
        ),
        NbEntscheidung::Escalate { reason } => {
            (AnmeldungDecision::Escalate, None, Some(reason.clone()))
        }
    };

    let rec = AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: pid as i32,
        malo_id: malo_id.clone(),
        lf_mp_id: lf_mp_id.clone(),
        decision,
        antwortcode: antwortcode.clone(),
        detail: detail.clone(),
        initiator_is_affiliate,
        decided_at: now,
        tenant: config.tenant.clone(),
    };

    // `insert` is ON CONFLICT DO NOTHING on (process_id, tenant), so a
    // redelivered event does not double-count. Report the counter from the rows
    // actually written rather than from every delivery attempt.
    if repo.insert(&rec).await? {
        crate::metrics::record_decision(decision.as_str(), pid);
    }

    // ── 6. Dispatch command to makod ──────────────────────────────────────
    match &result {
        NbEntscheidung::Accept => {
            // §20 EnWG Diskriminierungsfreiheitspflicht:
            // When the initiating LF shares the same MP-ID as our operator
            // (vertically integrated utility — §6b EnWG deployment), automatic
            // acceptance is forbidden.  The operator must review manually.
            // Bypassing this check exposes the NB to BNetzA sanctions.
            if initiator_is_affiliate {
                warn!(
                    %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
                    "processd NB: §20 EnWG — affiliate Anmeldung detected; \
                     auto_accept overridden to false — operator must review"
                );
                enqueue_for_operator(
                    queue,
                    config,
                    &payload_meta,
                    &format!(
                        "§20 EnWG affiliate Anmeldung (LF {lf_mp_id} is this operator) — \
                         `mako-pruefung` says Accept, but an affiliate may not take the \
                         automatic path a third party does not get"
                    ),
                )
                .await?;
            } else if config.auto_accept {
                let cmd_body = ForwardCommand {
                    marktrolle: Some("NB".to_owned()),
                    command: answer_commands(pid).0.to_owned(),
                    malo_id: Some(malo_id.clone()),
                    melo_id: None,
                    payload: serde_json::json!({ "process_id": process_id }),
                };
                makod
                    .post_command(&format!("processd-nb-accept-{process_id}"), &cmd_body)
                    .await
                    .inspect_err(
                        |e| warn!(%e, %process_id, "processd NB: bestaetigen dispatch failed"),
                    )?;
                info!(%process_id, pid, %malo_id, "processd NB: dispatched bestaetigen");
            } else {
                info!(%process_id, pid, %malo_id, "processd NB: Accept held for operator confirmation (auto_accept = false)");
                enqueue_for_operator(
                    queue,
                    config,
                    &payload_meta,
                    "`mako-pruefung` says Accept; auto_accept is off, so the \
                     Bestätigung is dispatched on operator approval",
                )
                .await?;
            }
        }
        NbEntscheidung::Reject(reason) => {
            let cmd_body = ForwardCommand {
                marktrolle: Some("NB".to_owned()),
                command: answer_commands(pid).1.to_owned(),
                malo_id: Some(malo_id.clone()),
                melo_id: None,
                payload: serde_json::json!({
                    "process_id": process_id,
                    "antwortcode": reason.antwortcode,
                    "detail": reason.detail,
                }),
            };
            makod
                .post_command(&format!("processd-nb-reject-{process_id}"), &cmd_body)
                .await
                .inspect_err(|e| warn!(%e, %process_id, "processd NB: ablehnen dispatch failed"))?;
            info!(%process_id, pid, %malo_id, erc = %reason.antwortcode, "processd NB: dispatched ablehnen");
        }
        NbEntscheidung::Escalate { reason } => {
            warn!(%process_id, pid, %malo_id, %reason, "processd NB: Escalate — operator action required");
            enqueue_for_operator(queue, config, &payload_meta, reason).await?;
        }
    }

    Ok(true)
}

/// The trigger PID, process and MaLo an escalated Anmeldung is queued under.
struct AnmeldungMeta {
    pid: u32,
    process_id: Uuid,
    malo_id: String,
    received_at: OffsetDateTime,
}

/// Put a decision the NB may not dispatch automatically in front of an operator,
/// with the answer deadline attached.
///
/// Escalations, and Accepts held back by `auto_accept = false` or by the § 20
/// EnWG affiliate rule, all take this path. `anmeldung_decisions` is the audit
/// log and carries no Frist: only a queue entry expires, surfaces in
/// `processd_approval_queue_overdue`, and gives the operator something to act on.
async fn enqueue_for_operator(
    queue: &PgApprovalQueue,
    config: &NbModuleConfig,
    meta: &AnmeldungMeta,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let window = crate::fristen::operator_window(meta.pid, meta.received_at);
    let (accept, reject) = answer_commands(meta.pid);
    let entry = ApprovalQueueEntry::pending(
        meta.process_id,
        meta.pid as i32,
        Some(meta.malo_id.clone()),
        format!(
            "{reason} (Antwortfrist {}: {})",
            window.deadline, window.source
        ),
        window.expires_at,
        config.tenant.clone(),
    )
    .with_commands(accept, reject, Some("NB"));
    queue.enqueue(&entry).await?;
    info!(
        process_id = %meta.process_id,
        pid = meta.pid,
        malo_id = %meta.malo_id,
        deadline = %window.deadline,
        "processd NB: queued for operator decision"
    );
    Ok(())
}

/// The `makod` command pair that answers an inbound NB PID.
///
/// Anmeldung and Abmeldung take **different commands**, and both resolve from
/// the PID alone: an Abmeldung answered with `gpke.lieferbeginn.bestaetigen`
/// would drive the wrong response PID onto the wire.
fn answer_commands(pid: u32) -> (&'static str, &'static str) {
    match pid {
        44_001 => (
            mako_markt::commands::GELI_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GELI_LIEFERBEGINN_ABLEHNEN,
        ),
        44_004 => (
            mako_markt::commands::GELI_LIEFERENDE_BESTAETIGEN,
            mako_markt::commands::GELI_LIEFERENDE_ABLEHNEN,
        ),
        55_004 => (
            mako_markt::commands::GPKE_LIEFERENDE_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERENDE_ABLEHNEN,
        ),
        // 55001 / 55077 — makod derives 55002/55003 and 55078/55080 from the
        // inbound PID the process was spawned with, so one command pair covers
        // both Anmeldung variants.
        _ => (
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        ),
    }
}

// ── Abmeldung ─────────────────────────────────────────────────────────────────

/// Fields extracted from a `de.mako.process.initiated` for an Abmeldung PID.
#[derive(Debug, Clone)]
pub struct AbmeldungPayload {
    pub pid: u32,
    pub process_id: Uuid,
    pub malo_id: String,
    /// The supplier ending the assignment. `makod`'s adapter surfaces it as
    /// `current_supplier` where it can tell, else as `new_supplier` (the UTILMD
    /// NAD sender is the same party in both directions of this process).
    pub lf_mp_id: String,
    pub grid_operator_gln: String,
    pub abmeldedatum: time::Date,
    pub transaktionsgrund: Option<String>,
    pub ist_erzeugende_marktlokation: bool,
    pub bilanzierungsmethode: Option<String>,
}

impl AbmeldungPayload {
    /// Parse an Abmeldung event, or `None` when the PID is not one.
    #[must_use]
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !ABMELDUNG_PIDS.contains(&pid) {
            return None;
        }
        let process_id: Uuid = event["subject"].as_str()?.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let lf_mp_id = data
            .get("current_supplier")
            .or_else(|| data.get("new_supplier"))
            .or_else(|| data.get("sender"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let grid_operator_gln = data
            .get("grid_operator")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let abmeldedatum = parse_civil_date(data.get("process_date")?.as_str()?)?;
        Some(Self {
            pid,
            process_id,
            malo_id,
            lf_mp_id,
            grid_operator_gln,
            abmeldedatum,
            transaktionsgrund: data
                .get("transaktionsgrund")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            ist_erzeugende_marktlokation: data
                .get("ist_erzeugende_marktlokation")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            bilanzierungsmethode: data
                .get("bilanzierungsmethode")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
        })
    }

    /// Derive the `mako-pruefung` input.
    #[must_use]
    pub fn into_anfrage(self) -> mako_pruefung::AbmeldungAnfrage {
        mako_pruefung::AbmeldungAnfrage {
            pid: self.pid,
            process_id: self.process_id,
            malo_id: self.malo_id,
            lf_mp_id: self.lf_mp_id,
            grid_operator_gln: self.grid_operator_gln,
            abmeldedatum: self.abmeldedatum,
            sparte: sparte_of(self.pid),
            messtyp: messtyp_of(self.bilanzierungsmethode.as_deref()),
            transaktionsgrund: self.transaktionsgrund,
            ist_erzeugende_marktlokation: self.ist_erzeugende_marktlokation,
        }
    }
}

/// `YYYYMMDD` or `YYYY-MM-DD`, the two shapes the `makod` adapters emit.
fn parse_civil_date(raw: &str) -> Option<time::Date> {
    if raw.len() == 8 {
        time::Date::parse(raw, time::macros::format_description!("[year][month][day]")).ok()
    } else {
        time::Date::parse(
            raw,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .ok()
    }
}

/// UTILMD TM+EM marker → `mako-pruefung` metering class. SLP is the default: it
/// is the class with the *widest* retroactive window, so an unknown marker can
/// never turn an admissible date into an auto-reject.
fn messtyp_of(bilanzierungsmethode: Option<&str>) -> Messtyp {
    match bilanzierungsmethode {
        Some("RLM") => Messtyp::Rlm,
        Some("IMS") => Messtyp::Imsys,
        _ => Messtyp::Slp,
    }
}

/// The NB's decision on an inbound Abmeldung (55004 / 44004), EBD `E_0607`.
#[allow(clippy::too_many_arguments)]
async fn decide_abmeldung(
    payload: AbmeldungPayload,
    event: &serde_json::Value,
    config: &NbModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    repo: &PgAnmeldungRepository,
    queue: &PgApprovalQueue,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // Not addressed to this NB — another operator's message on a shared bus.
    if !payload.grid_operator_gln.is_empty() && payload.grid_operator_gln != config.own_mp_id {
        return Ok(false);
    }

    let pid = payload.pid;
    let process_id = payload.process_id;
    let malo_id = payload.malo_id.clone();
    let lf_mp_id = payload.lf_mp_id.clone();
    let initiator_is_affiliate = lf_mp_id == config.own_mp_id;
    let received_at = received_at(event);

    info!(%process_id, pid, %malo_id, %lf_mp_id, "processd NB: evaluating Abmeldung");

    // A transport failure is not evidence of absence: propagate so the fan-out
    // redelivers rather than deciding on a missing projection.
    let versorgung = reader
        .get_versorgung(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd versorgung fetch failed"))?;

    let anfrage = payload.into_anfrage();
    let now = OffsetDateTime::now_utc();
    let result = mako_pruefung::evaluate_abmeldung(
        &anfrage,
        versorgung.as_ref(),
        now,
        &config.netz_check_config(),
    );

    info!(%process_id, pid, %malo_id, outcome = ?result, "processd NB: E_0607 result");

    let (decision, antwortcode, detail) = classify(&result);
    let rec = AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: pid as i32,
        malo_id: malo_id.clone(),
        lf_mp_id: lf_mp_id.clone(),
        decision,
        antwortcode,
        detail,
        initiator_is_affiliate,
        decided_at: now,
        tenant: config.tenant.clone(),
    };
    if repo.insert(&rec).await? {
        crate::metrics::record_decision(decision.as_str(), pid);
    }

    let meta = AnmeldungMeta {
        pid,
        process_id,
        malo_id: malo_id.clone(),
        received_at,
    };

    match &result {
        NbEntscheidung::Accept => {
            // § 20 EnWG parity applies to the Abmeldung too: an affiliate must
            // not get an automatic path a third party does not get.
            if initiator_is_affiliate {
                warn!(%process_id, pid, %malo_id, %lf_mp_id,
                      "processd NB: § 20 EnWG — affiliate Abmeldung held for operator review");
                enqueue_for_operator(
                    queue,
                    config,
                    &meta,
                    &format!(
                        "§ 20 EnWG affiliate Abmeldung (LF {lf_mp_id} is this operator) — \
                         E_0607 says Accept, but an affiliate may not take the automatic path"
                    ),
                )
                .await?;
            } else if config.auto_accept {
                dispatch(makod, pid, &malo_id, process_id, true, None).await?;
                info!(%process_id, pid, %malo_id, "processd NB: dispatched Bestätigung Abmeldung");
            } else {
                enqueue_for_operator(
                    queue,
                    config,
                    &meta,
                    "E_0607 says Accept; auto_accept is off, so the Bestätigung is \
                     dispatched on operator approval",
                )
                .await?;
            }
        }
        NbEntscheidung::Reject(reason) => {
            dispatch(
                makod,
                pid,
                &malo_id,
                process_id,
                false,
                Some((&reason.antwortcode, &reason.detail)),
            )
            .await?;
            info!(%process_id, pid, %malo_id, erc = %reason.antwortcode,
                  "processd NB: dispatched Ablehnung Abmeldung");
        }
        NbEntscheidung::Escalate { reason } => {
            warn!(%process_id, pid, %malo_id, %reason, "processd NB: Abmeldung escalated");
            enqueue_for_operator(queue, config, &meta, reason).await?;
        }
    }

    Ok(true)
}

/// The answer Frist runs from receipt of the market message; the CloudEvent
/// `time` is when `makod` emitted it, which is the closest instant available.
fn received_at(event: &serde_json::Value) -> OffsetDateTime {
    event["time"]
        .as_str()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
}

/// Map a `mako-pruefung` verdict onto the audit-log columns.
fn classify(result: &NbEntscheidung) -> (AnmeldungDecision, Option<String>, Option<String>) {
    match result {
        NbEntscheidung::Accept => (AnmeldungDecision::Accept, None, None),
        NbEntscheidung::Reject(RejectReason {
            antwortcode,
            detail,
            ..
        }) => (
            AnmeldungDecision::Reject,
            Some(antwortcode.clone()),
            Some(detail.clone()),
        ),
        NbEntscheidung::Escalate { reason } => {
            (AnmeldungDecision::Escalate, None, Some(reason.clone()))
        }
    }
}

/// Post the answer command for `pid` to `makod`.
async fn dispatch(
    makod: &MakodClient,
    pid: u32,
    malo_id: &str,
    process_id: Uuid,
    accept: bool,
    reject_reason: Option<(&str, &str)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (accept_cmd, reject_cmd) = answer_commands(pid);
    let cmd = ForwardCommand {
        marktrolle: Some("NB".to_owned()),
        command: if accept { accept_cmd } else { reject_cmd }.to_owned(),
        malo_id: Some(malo_id.to_owned()),
        melo_id: None,
        payload: match reject_reason {
            Some((erc, detail)) => serde_json::json!({
                "process_id": process_id,
                "antwortcode": erc,
                // makod forwards `reason` onto the APERAK; carry the code
                // inside it so the ground survives the hop.
                "reason": format!("{erc}: {detail}"),
                "detail": detail,
            }),
            None => serde_json::json!({ "process_id": process_id }),
        },
    };
    let verb = if accept { "accept" } else { "reject" };
    makod
        .post_command(&format!("processd-nb-{verb}-{process_id}"), &cmd)
        .await
        .inspect_err(|e| warn!(%e, %process_id, "processd NB: dispatch failed"))?;
    Ok(())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AnmeldungPayload parsing ───────────────────────────────────────────────

    #[test]
    fn parse_strom_lieferbeginn_event() {
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "bilanzierungsgebiet": "11YF-VATTENFALL-2",
                "process_date": "20261001",
                "transaktionsgrund": "E01",
                "bilanzierungsmethode": "RLM"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse");
        assert_eq!(payload.pid, 55001);
        assert_eq!(payload.malo_id, "51238696012");
        assert_eq!(payload.new_supplier_gln, "9900357000004");
        assert_eq!(payload.grid_operator_gln, "9900000000001");
        assert_eq!(
            payload.bilanzierungsgebiet.as_deref(),
            Some("11YF-VATTENFALL-2")
        );
        assert_eq!(payload.transaktionsgrund.as_deref(), Some("E01"));
        // Absent flag → not an Erzeugende MaLo.
        assert!(!payload.ist_erzeugende_marktlokation);
        // Messtyp derives from the TM+EM marker in the payload.
        let anfrage = payload.into_anfrage();
        assert_eq!(anfrage.messtyp, mako_pruefung::Messtyp::Rlm);
        assert_eq!(anfrage.transaktionsgrund.as_deref(), Some("E01"));
        assert!(!anfrage.ist_erzeugende_marktlokation);
    }

    #[test]
    fn parse_erzeugende_malo_sets_eeg_flag() {
        // An Erzeugende (EEG-/KWKG-) MaLo Anmeldung carries the STS 9013=ZW3
        // ergänzung, surfaced by the makod adapter as `ist_erzeugende_marktlokation`.
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261001",
                "transaktionsgrund": "E01",
                "ist_erzeugende_marktlokation": true
            }
        });
        let anfrage = AnmeldungPayload::parse(&event)
            .expect("should parse")
            .into_anfrage();
        assert!(
            anfrage.ist_erzeugende_marktlokation,
            "ZW3 Erzeugende flag must reach the `mako-pruefung` to trigger §10c EEG"
        );
    }

    #[test]
    fn parse_gas_lieferbeginn_event() {
        let event = serde_json::json!({
            "makopid": 44001,
            "subject": "550e8400-e29b-41d4-a716-446655440001",
            "data": {
                "malo_id": "51238696781",
                "new_supplier": "9800357000004",
                "grid_operator": "9800000000001",
                "process_date": "2026-10-01"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse gas event");
        assert_eq!(payload.pid, 44001);
        let anfrage = payload.into_anfrage();
        assert!(matches!(anfrage.sparte, mako_markt::domain::Sparte::Gas));
    }

    #[test]
    fn parse_ignores_unknown_pids() {
        let event = serde_json::json!({
            "makopid": 55008, // E_0624 — LF PID, not NB
            "subject": "550e8400-e29b-41d4-a716-446655440002",
            "data": { "malo_id": "51238696012", "new_supplier": "99x", "grid_operator": "99y", "process_date": "20261001" }
        });
        assert!(AnmeldungPayload::parse(&event).is_none());
    }

    // ── Command name mapping ───────────────────────────────────────────────────
    //
    // Anmeldung and Abmeldung answer through *different* commands. Answering an
    // Abmeldung with `gpke.lieferbeginn.bestaetigen` puts the wrong response PID
    // on the wire, and both names are plausible enough to survive review.

    #[test]
    fn anmeldung_and_abmeldung_take_different_commands() {
        assert_eq!(
            answer_commands(55_001),
            (
                "gpke.lieferbeginn.bestaetigen",
                "gpke.lieferbeginn.ablehnen"
            )
        );
        assert_eq!(
            answer_commands(55_077),
            (
                "gpke.lieferbeginn.bestaetigen",
                "gpke.lieferbeginn.ablehnen"
            ),
            "makod derives 55078/55080 from the inbound PID it spawned with"
        );
        assert_eq!(
            answer_commands(55_004),
            ("gpke.lieferende.bestaetigen", "gpke.lieferende.ablehnen")
        );
        assert_eq!(
            answer_commands(44_001),
            (
                "geli.lieferbeginn.bestaetigen",
                "geli.lieferbeginn.ablehnen"
            )
        );
        assert_eq!(
            answer_commands(44_004),
            ("geli.lieferende.bestaetigen", "geli.lieferende.ablehnen")
        );
    }

    /// Every posted name must be in the shared list `makod`'s registry test
    /// cross-checks — an unregistered name comes back as HTTP 422.
    #[test]
    fn every_answer_command_is_registered() {
        for pid in answered_pids() {
            let (accept, reject) = answer_commands(pid);
            for name in [accept, reject] {
                assert!(
                    mako_markt::commands::DISPATCHED_BY_SERVICES.contains(&name),
                    "{name:?} (PID {pid}) missing from DISPATCHED_BY_SERVICES"
                );
            }
        }
    }

    /// 55016 „Kündigung" is LFN → LFA: parsing it here would make an `nb-only`
    /// binary answer a supplier obligation.
    #[test]
    fn the_kuendigung_is_not_an_nb_anmeldung() {
        assert!(!ANMELDUNG_PIDS.contains(&55_016));
        let event = serde_json::json!({
            "makopid": 55_016,
            "subject": "550e8400-e29b-41d4-a716-446655440009",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261001"
            }
        });
        assert!(AnmeldungPayload::parse(&event).is_none());
        assert!(AbmeldungPayload::parse(&event).is_none());
    }

    // ── Abmeldung ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_strom_abmeldung_event() {
        let event = serde_json::json!({
            "makopid": 55_004,
            "subject": "550e8400-e29b-41d4-a716-446655440010",
            "data": {
                "malo_id": "51238696012",
                "current_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261101",
                "transaktionsgrund": "E01"
            }
        });
        let p = AbmeldungPayload::parse(&event).expect("parses");
        assert_eq!(p.pid, 55_004);
        assert_eq!(p.lf_mp_id, "9900357000004");
        let a = p.into_anfrage();
        assert!(matches!(a.sparte, Sparte::Strom));
        assert_eq!(a.messtyp, Messtyp::Slp);
    }

    #[test]
    fn parse_gas_abmeldung_event() {
        let event = serde_json::json!({
            "makopid": 44_004,
            "subject": "550e8400-e29b-41d4-a716-446655440011",
            "data": {
                "malo_id": "51238696012",
                "current_supplier": "9800357000004",
                "grid_operator": "9800000000001",
                "process_date": "2026-11-01",
                "bilanzierungsmethode": "RLM"
            }
        });
        let a = AbmeldungPayload::parse(&event)
            .expect("parses")
            .into_anfrage();
        assert!(matches!(a.sparte, Sparte::Gas));
        assert_eq!(a.messtyp, Messtyp::Rlm);
    }

    /// An Anmeldung PID must not parse as an Abmeldung and vice versa —
    /// the two pipelines dispatch different market messages.
    #[test]
    fn the_two_payloads_do_not_overlap() {
        for pid in answered_pids() {
            let event = serde_json::json!({
                "makopid": pid,
                "subject": "550e8400-e29b-41d4-a716-446655440012",
                "data": {
                    "malo_id": "51238696012",
                    "new_supplier": "9900357000004",
                    "grid_operator": "9900000000001",
                    "process_date": "20261101"
                }
            });
            let anmeldung = AnmeldungPayload::parse(&event).is_some();
            let abmeldung = AbmeldungPayload::parse(&event).is_some();
            assert!(
                anmeldung ^ abmeldung,
                "PID {pid} parses as {} — exactly one pipeline must claim it",
                if anmeldung { "both" } else { "neither" }
            );
        }
    }

    /// PID 55077 *is* the „Anmeldung erz. MaLo" use case, so the § 10c EEG
    /// Monatserster rule must apply even when the adapter omitted the ZW3 flag.
    #[test]
    fn pid_55077_is_always_an_erzeugende_marktlokation() {
        let event = serde_json::json!({
            "makopid": 55_077,
            "subject": "550e8400-e29b-41d4-a716-446655440013",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261101"
            }
        });
        let a = AnmeldungPayload::parse(&event)
            .expect("parses")
            .into_anfrage();
        assert!(a.ist_erzeugende_marktlokation);
    }

    // ── Misdirection check ─────────────────────────────────────────────────────

    #[test]
    fn affiliate_detection() {
        // When new_supplier == own_mp_id, initiator_is_affiliate must be true.
        let own_mp_id = "9900357000004";
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440003",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": own_mp_id, // affiliate!
                "grid_operator": "9900000000001",
                "process_date": "20261001"
            }
        });
        let payload = AnmeldungPayload::parse(&event).unwrap();
        let initiator_is_affiliate = payload.new_supplier_gln == own_mp_id;
        assert!(
            initiator_is_affiliate,
            "affiliate must be detected when new_supplier == own_mp_id"
        );
    }
}
