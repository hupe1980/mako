//! PostgreSQL implementation for `approval_queue`.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum QueueStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl std::fmt::Display for QueueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Approved => write!(f, "Approved"),
            Self::Rejected => write!(f, "Rejected"),
            Self::Expired => write!(f, "Expired"),
        }
    }
}

impl std::str::FromStr for QueueStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(Self::Pending),
            "Approved" => Ok(Self::Approved),
            "Rejected" => Ok(Self::Rejected),
            "Expired" => Ok(Self::Expired),
            other => Err(format!("unknown QueueStatus: {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalQueueEntry {
    pub id: Uuid,
    pub process_id: Uuid,
    pub pid: i32,
    pub malo_id: Option<String>,
    pub reason: String,
    pub status: QueueStatus,
    /// `makod` command to dispatch when an operator approves. `None` means the
    /// approval carries no market message.
    pub approve_command: Option<String>,
    /// `makod` command to dispatch when an operator rejects. `None` means the
    /// rejection is recorded without a market message.
    pub reject_command: Option<String>,
    /// Marktrolle forwarded on the dispatched command.
    pub marktrolle: Option<String>,
    /// A **Meldepflicht** the approved answer carries with it: a second `makod`
    /// command, dispatched only after the market answer itself went out.
    ///
    /// A Meldepflicht is a message a Festlegung obliges the operator to send
    /// with no answer expected back, and several of them are owed „unverzüglich
    /// nach dem ÜZ" of an answer an operator may hold. Storing it beside the
    /// answer is what keeps the two together when the decision is deferred.
    /// `None` means the approval sends one message.
    pub followup_command: Option<String>,
    /// The body of [`Self::followup_command`]. Built at enqueue time, because
    /// the facts the Meldung states — the Altlieferant, the Zuordnungsende —
    /// are the ones that were true when the decision was taken, not the ones
    /// the projection holds when an operator gets to it.
    pub followup_payload: Option<serde_json::Value>,
    /// The body [`Self::approve_command`] is dispatched with, frozen at
    /// enqueue: for a held Accept it is exactly what the automatic path would
    /// have sent, so the operator approves the answer they were shown. `None`
    /// means the answer is the operator's to state on approval (an Escalate).
    pub answer_payload: Option<serde_json::Value>,
    /// The body [`Self::reject_command`] is dispatched with, where it is known
    /// at enqueue — the business key the answer is addressed by. The
    /// Antwortcode of a refusal is always the operator's.
    pub reject_payload: Option<serde_json::Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    /// `sub` of the principal who decided this entry (§ 20 EnWG / GoBD).
    pub decided_by: Option<String>,
    pub tenant: String,
}

impl ApprovalQueueEntry {
    /// A pending entry. Pair it with [`Self::with_commands`] unless the decision
    /// genuinely has no market message — the REST handler dispatches what is
    /// stored here and nothing else.
    pub fn pending(
        process_id: Uuid,
        pid: i32,
        malo_id: Option<String>,
        reason: String,
        expires_at: OffsetDateTime,
        tenant: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            process_id,
            pid,
            malo_id,
            reason,
            status: QueueStatus::Pending,
            approve_command: None,
            reject_command: None,
            marktrolle: None,
            followup_command: None,
            followup_payload: None,
            answer_payload: None,
            reject_payload: None,
            expires_at,
            created_at: OffsetDateTime::now_utc(),
            decided_at: None,
            decided_by: None,
            tenant,
        }
    }

    #[must_use]
    pub fn with_commands(mut self, approve: &str, reject: &str, marktrolle: Option<&str>) -> Self {
        self.approve_command = Some(approve.to_owned());
        self.reject_command = Some(reject.to_owned());
        self.marktrolle = marktrolle.map(ToOwned::to_owned);
        self
    }

    /// Attach a Meldepflicht to the approval: a second command, dispatched
    /// after the market answer and only if that answer reached `makod`.
    ///
    /// Ordering is the point. The Meldungen that hang off an answer are owed
    /// „unverzüglich nach dem ÜZ" of it, so sending one for an answer that
    /// never went out states an outcome the market did not see.
    #[must_use]
    pub fn with_followup(mut self, command: &str, payload: serde_json::Value) -> Self {
        self.followup_command = Some(command.to_owned());
        self.followup_payload = Some(payload);
        self
    }

    /// Freeze the body the approval dispatches. See [`Self::answer_payload`].
    #[must_use]
    pub fn with_answer_payload(mut self, payload: serde_json::Value) -> Self {
        self.answer_payload = Some(payload);
        self
    }

    /// Freeze what the refusal is addressed by. See [`Self::reject_payload`].
    #[must_use]
    pub fn with_reject_payload(mut self, payload: serde_json::Value) -> Self {
        self.reject_payload = Some(payload);
        self
    }

    /// A decision where only *rejecting* sends a market message — the refusal
    /// is dispatchable from the queue, the positive answer is not (an ESA
    /// Angebot needs prices the queue does not hold).
    #[must_use]
    pub fn with_reject_command(mut self, reject: &str, marktrolle: Option<&str>) -> Self {
        self.approve_command = None;
        self.reject_command = Some(reject.to_owned());
        self.marktrolle = marktrolle.map(ToOwned::to_owned);
        self
    }

    /// A decision where only *approving* sends a market message.
    ///
    /// The REQOTE Preisanfrage is the case: the AHB answer to it is a QUOTES,
    /// and there is no „Preisanfrage ablehnen". Rejecting such an entry records
    /// the operator's decision not to quote automatically and sends nothing —
    /// which is why `reject_command` stays `None` rather than repeating the
    /// approve command.
    #[must_use]
    pub fn with_approve_command(mut self, approve: &str, marktrolle: Option<&str>) -> Self {
        self.approve_command = Some(approve.to_owned());
        self.reject_command = None;
        self.marktrolle = marktrolle.map(ToOwned::to_owned);
        self
    }
}

/// Every column `map_entry` reads.
const ENTRY_COLUMNS: &str = "id, process_id, pid, malo_id, reason, status, approve_command, \
                             reject_command, marktrolle, followup_command, followup_payload, \
                             answer_payload, reject_payload, \
                             expires_at, created_at, decided_at, decided_by, tenant";

fn map_entry(row: &PgRow) -> Result<ApprovalQueueEntry, sqlx::Error> {
    let status_str: String = row.try_get("status")?;
    let status = status_str
        .parse::<QueueStatus>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "status".into(),
            source: Box::new(std::io::Error::other(e)),
        })?;
    Ok(ApprovalQueueEntry {
        id: row.try_get("id")?,
        process_id: row.try_get("process_id")?,
        pid: row.try_get("pid")?,
        malo_id: row.try_get("malo_id")?,
        reason: row.try_get("reason")?,
        status,
        approve_command: row.try_get("approve_command")?,
        reject_command: row.try_get("reject_command")?,
        marktrolle: row.try_get("marktrolle")?,
        followup_command: row.try_get("followup_command")?,
        followup_payload: row.try_get("followup_payload")?,
        answer_payload: row.try_get("answer_payload")?,
        reject_payload: row.try_get("reject_payload")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        decided_at: row.try_get("decided_at")?,
        decided_by: row.try_get("decided_by")?,
        tenant: row.try_get("tenant")?,
    })
}

#[derive(Clone, Debug)]
pub struct PgApprovalQueue {
    pool: PgPool,
}

impl PgApprovalQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn enqueue(&self, entry: &ApprovalQueueEntry) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO approval_queue (id, process_id, pid, malo_id, reason, status, approve_command, reject_command, marktrolle, followup_command, followup_payload, answer_payload, reject_payload, expires_at, created_at, tenant) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) ON CONFLICT (process_id, tenant) DO NOTHING",
        )
        .bind(entry.id).bind(entry.process_id).bind(entry.pid).bind(&entry.malo_id)
        .bind(&entry.reason).bind(entry.status.to_string())
        .bind(&entry.approve_command).bind(&entry.reject_command).bind(&entry.marktrolle)
        .bind(&entry.followup_command).bind(&entry.followup_payload)
        .bind(&entry.answer_payload).bind(&entry.reject_payload)
        .bind(entry.expires_at).bind(entry.created_at).bind(&entry.tenant)
        .execute(&self.pool).await?;
        Ok(())
    }

    /// Atomically move a `Pending` entry to `status` and return it.
    ///
    /// The decision must be claimed **before** the market command is dispatched:
    /// dispatch-then-record let two operators deciding at once send both an
    /// einwilligung and an ablehnen (different idempotency keys) while the DB
    /// recorded only one, and let a terminal entry re-trigger a market message.
    /// `Ok(None)` means the entry is gone or no longer Pending.
    pub async fn claim(
        &self,
        id: Uuid,
        tenant: &str,
        status: QueueStatus,
        decided_by: &str,
    ) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
        let opt = sqlx::query(&format!(
            "UPDATE approval_queue SET status = $3, decided_at = now(), decided_by = $4 \
             WHERE id = $1 AND tenant = $2 AND status = 'Pending' RETURNING {ENTRY_COLUMNS}"
        ))
        .bind(id)
        .bind(tenant)
        .bind(status.to_string())
        .bind(decided_by)
        .fetch_optional(&self.pool)
        .await?;
        opt.map(|r| map_entry(&r)).transpose()
    }

    /// Release a claim taken by [`Self::claim`] so the operator can retry.
    pub async fn unclaim(&self, id: Uuid, tenant: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE approval_queue SET status = 'Pending', decided_at = NULL, decided_by = NULL \
             WHERE id = $1 AND tenant = $2",
        )
        .bind(id)
        .bind(tenant)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn expire_stale(&self) -> Result<u64, sqlx::Error> {
        let r = sqlx::query("UPDATE approval_queue SET status = 'Expired', decided_at = now() WHERE status = 'Pending' AND expires_at < now()")
            .execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    pub async fn list(
        &self,
        tenant: &str,
        status: Option<QueueStatus>,
        limit: u32,
    ) -> Result<Vec<ApprovalQueueEntry>, sqlx::Error> {
        let status_str = status.map(|s| s.to_string());
        let rows = sqlx::query(&format!(
            "SELECT {ENTRY_COLUMNS} FROM approval_queue WHERE tenant = $1 AND ($2::text IS NULL OR status = $2) ORDER BY created_at DESC LIMIT $3"
        ))
        .bind(tenant).bind(status_str).bind(limit as i64)
        .fetch_all(&self.pool).await?;
        rows.iter().map(map_entry).collect()
    }

    pub async fn find_by_id(
        &self,
        id: Uuid,
        tenant: &str,
    ) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
        let opt = sqlx::query(&format!(
            "SELECT {ENTRY_COLUMNS} FROM approval_queue WHERE id = $1 AND tenant = $2"
        ))
        .bind(id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await?;
        opt.map(|r| map_entry(&r)).transpose()
    }
}

// ── What a decision dispatches ────────────────────────────────────────────────

/// A field `makod` refuses an answer command without, beyond `process_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pflichtfeld {
    /// `malo_id` — the business key of a Marktlokation answer. The queue row's
    /// own `malo_id` column supplies it.
    MaloId,
    /// `melo_id` — the business key of a WiM Messlokation answer.
    MeloId,
    /// One of `malo_id` / `melo_id` / `lokations_id` — the ESA location key.
    Lokation,
    /// `tx_id` — the API-Webdienste transaction a Steuerungsauftrag answer is
    /// addressed by.
    TxId,
    /// `antwort_code` — the EBD Antwortcode of `SG4 STS+E01` / `AJT`.
    Antwortcode,
    /// `reason` — the Begründung of an answer that carries no code (the
    /// QUOTES refusal of an ESA Werteanfrage rides `FTX+ACB`).
    Begruendung,
}

impl Pflichtfeld {
    fn satisfied_by(self, body: &serde_json::Map<String, serde_json::Value>) -> bool {
        let has = |k: &str| {
            body.get(k)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|v| !v.is_empty())
        };
        match self {
            Self::MaloId => has("malo_id"),
            Self::MeloId => has("melo_id"),
            Self::Lokation => has("malo_id") || has("melo_id") || has("lokations_id"),
            Self::TxId => has("tx_id"),
            Self::Antwortcode => has("antwort_code"),
            Self::Begruendung => has("reason"),
        }
    }

    /// The payload key an operator or the frozen body supplies it under.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::MaloId => "malo_id",
            Self::MeloId => "melo_id",
            Self::Lokation => "malo_id | melo_id | lokations_id",
            Self::TxId => "tx_id",
            Self::Antwortcode => "antwort_code",
            Self::Begruendung => "reason (stated as bemerkung)",
        }
    }
}

/// What each command the queue can hold needs in its body — read off the
/// `makod` handler that answers it, which refuses the command with `422`
/// without them.
///
/// A command missing here cannot be decided from the queue: the decision is
/// refused naming the command rather than sent and failed in `makod`.
/// `cargo xtask check-approval-commands` refuses an enqueue site naming a
/// command this table does not hold.
pub const ANSWER_CONTRACTS: &[(&str, &[Pflichtfeld])] = {
    use Pflichtfeld::{Antwortcode, Begruendung, Lokation, MaloId, MeloId, TxId};
    use mako_markt::commands as c;
    const MALO_CODE: &[Pflichtfeld] = &[MaloId, Antwortcode];
    &[
        // NB answers to an Anmeldung / Abmeldung (`dispatch_supplier_change_antwort`,
        // `dispatch_geli_gas_antwort`).
        (c::GPKE_LIEFERBEGINN_BESTAETIGEN, MALO_CODE),
        (c::GPKE_LIEFERBEGINN_ABLEHNEN, MALO_CODE),
        (c::GPKE_LIEFERENDE_BESTAETIGEN, MALO_CODE),
        (c::GPKE_LIEFERENDE_ABLEHNEN, MALO_CODE),
        (c::GELI_LIEFERBEGINN_BESTAETIGEN, MALO_CODE),
        (c::GELI_LIEFERBEGINN_ABLEHNEN, MALO_CODE),
        (c::GELI_LIEFERENDE_BESTAETIGEN, MALO_CODE),
        (c::GELI_LIEFERENDE_ABLEHNEN, MALO_CODE),
        // LF answers (`extract_lf_antwort` / `dispatch_geli_lf_antwort`).
        (c::GPKE_NB_LIEFERENDE_BESTAETIGEN, MALO_CODE),
        (c::GPKE_NB_LIEFERENDE_ABLEHNEN, MALO_CODE),
        (c::GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN, MALO_CODE),
        (c::GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN, MALO_CODE),
        (c::GPKE_KUENDIGUNG_BESTAETIGEN, MALO_CODE),
        (c::GPKE_KUENDIGUNG_ABLEHNEN, MALO_CODE),
        (c::GPKE_ZUORDNUNG_LF_BESTAETIGEN, MALO_CODE),
        (c::GPKE_ZUORDNUNG_LF_ABLEHNEN, MALO_CODE),
        (c::GELI_NB_LIEFERENDE_BESTAETIGEN, MALO_CODE),
        (c::GELI_NB_LIEFERENDE_ABLEHNEN, MALO_CODE),
        (c::GELI_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN, MALO_CODE),
        (c::GELI_BEENDIGUNG_ZUORDNUNG_ABLEHNEN, MALO_CODE),
        (c::GELI_KUENDIGUNG_BESTAETIGEN, MALO_CODE),
        (c::GELI_KUENDIGUNG_ABLEHNEN, MALO_CODE),
        (c::GELI_EOG_BESTAETIGEN, MALO_CODE),
        (c::GELI_EOG_ABLEHNEN, MALO_CODE),
        // `dispatch_gpke_eog_antwort` reads the cluster from the command.
        (c::GPKE_EOG_BESTAETIGEN, &[MaloId]),
        (c::GPKE_EOG_ABLEHNEN, &[MaloId]),
        // WiM MSB-Wechsel (`dispatch_wim_antwort`): the code defaults from the
        // process's own Entscheidungsbaum.
        (c::WIM_GERAETEWECHSEL_BESTAETIGEN, &[MeloId]),
        (c::WIM_GERAETEWECHSEL_ABLEHNEN, &[MeloId]),
        (c::WIM_PREISANFRAGE_ANGEBOT_SENDEN, &[MeloId]),
        // §14a Steuerungsauftrag (`dispatch_steuerungsauftrag_endantwort`).
        (c::WIM_STEUERUNGSAUFTRAG_BESTAETIGEN, &[TxId]),
        (c::WIM_STEUERUNGSAUFTRAG_ABLEHNEN, &[TxId]),
        // ESA Wertebestellung (`extract_esa_location`, `esa_antwort_code`).
        // `anbieten` is not here: the Angebot needs `preise` and `obis`
        // (QUOTES AHB 1.1a §4.3, Muss), which no queue entry holds.
        (
            c::WIM_WERTEBESTELLUNG_ANFRAGE_ABLEHNEN,
            &[Lokation, Begruendung],
        ),
        (
            c::WIM_WERTEBESTELLUNG_BESTELLUNG_BEANTWORTEN,
            &[Lokation, Antwortcode],
        ),
        (
            c::WIM_WERTEBESTELLUNG_ABBESTELLUNG_BEANTWORTEN,
            &[Lokation, Antwortcode],
        ),
        (
            c::WIM_WERTEBESTELLUNG_STORNIERUNG_BEANTWORTEN,
            &[Lokation, Antwortcode],
        ),
    ]
};

/// The fields `command` needs, or `None` when the queue holds no contract for it.
#[must_use]
pub fn answer_contract(command: &str) -> Option<&'static [Pflichtfeld]> {
    ANSWER_CONTRACTS
        .iter()
        .find(|(c, _)| *c == command)
        .map(|(_, f)| *f)
}

/// The answer an operator states on approve or reject.
///
/// Every field is optional because a held Accept carries its answer already;
/// an escalated decision does not, and is refused without the ones its command
/// needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorAnswer {
    /// The EBD Antwortcode (`SG4 STS+E01` / `AJT`).
    #[serde(default)]
    pub antwort_code: Option<String>,
    /// The Entscheidungsbaum the code is resolved in (`E_0622`, …).
    #[serde(default)]
    pub antwort_tree: Option<String>,
    /// The Codeliste DE 1131 written on the wire, where the MIG carries one.
    #[serde(default)]
    pub antwort_codeliste: Option<String>,
    /// `FTX+ACB` Erläuterung.
    #[serde(default)]
    pub bemerkung: Option<String>,
}

/// Why a decision cannot be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionRefusal {
    /// The queue holds no contract for the command.
    UnknownCommand(String),
    /// The answer is frozen; the operator may approve it as shown, not edit it.
    AnswerFrozen,
    /// Fields the command needs are missing from the body.
    Missing {
        /// The command.
        command: String,
        /// What is missing.
        fields: Vec<&'static str>,
    },
}

impl std::fmt::Display for DecisionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand(c) => write!(
                f,
                "the approval queue holds no answer contract for {c:?}; it cannot be decided here"
            ),
            Self::AnswerFrozen => {
                f.write_str("this entry's answer was frozen when it was queued; decide it as shown")
            }
            Self::Missing { command, fields } => write!(
                f,
                "{command} needs {} — state it on the decision",
                fields.join(", ")
            ),
        }
    }
}

/// The body `command` is dispatched with when an operator decides `entry`.
///
/// `frozen` is the entry's [`ApprovalQueueEntry::answer_payload`] or
/// [`ApprovalQueueEntry::reject_payload`]; the operator's answer fills what it
/// leaves open and is refused where it would replace a frozen field, so what
/// is approved is what was shown. The row's `malo_id` is the business key
/// where the command takes one.
///
/// # Errors
///
/// A [`DecisionRefusal`] naming what the command still needs.
pub fn decision_body(
    entry: &ApprovalQueueEntry,
    command: &str,
    frozen: Option<&serde_json::Value>,
    operator: &OperatorAnswer,
) -> Result<serde_json::Value, DecisionRefusal> {
    let contract =
        answer_contract(command).ok_or_else(|| DecisionRefusal::UnknownCommand(command.into()))?;
    let mut body = match frozen {
        Some(serde_json::Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    for (key, value) in [
        ("antwort_code", &operator.antwort_code),
        ("antwort_tree", &operator.antwort_tree),
        ("antwort_codeliste", &operator.antwort_codeliste),
        ("bemerkung", &operator.bemerkung),
    ] {
        let Some(value) = value.as_ref().filter(|v| !v.is_empty()) else {
            continue;
        };
        // What was frozen is what is decided: a stated field may fill a gap,
        // never replace the answer the operator was shown.
        if body.contains_key(key) {
            return Err(DecisionRefusal::AnswerFrozen);
        }
        body.insert(key.to_owned(), serde_json::Value::String(value.clone()));
    }
    // The operator's Erläuterung is the Begründung of a refusal that carries
    // no Antwortcode.
    if let Some(text) = operator.bemerkung.as_ref().filter(|t| !t.is_empty()) {
        body.entry("reason")
            .or_insert_with(|| serde_json::Value::String(text.clone()));
    }
    body.insert("process_id".into(), serde_json::json!(entry.process_id));
    if let Some(malo) = entry.malo_id.as_ref().filter(|m| !m.is_empty()) {
        body.entry("malo_id")
            .or_insert_with(|| serde_json::Value::String(malo.clone()));
    }
    let missing: Vec<&'static str> = contract
        .iter()
        .filter(|f| !f.satisfied_by(&body))
        .map(|f| f.key())
        .collect();
    if !missing.is_empty() {
        return Err(DecisionRefusal::Missing {
            command: command.to_owned(),
            fields: missing,
        });
    }
    Ok(serde_json::Value::Object(body))
}

#[cfg(test)]
mod decision_tests {
    use super::*;

    fn entry(approve: &str, reject: &str) -> ApprovalQueueEntry {
        ApprovalQueueEntry::pending(
            Uuid::nil(),
            55001,
            Some("51238696012".into()),
            "test".into(),
            OffsetDateTime::now_utc(),
            "t".into(),
        )
        .with_commands(approve, reject, Some("NB"))
    }

    /// A held Accept is approved with what the automatic path would have sent.
    #[test]
    fn a_held_accept_is_dispatched_with_its_frozen_answer() {
        let e = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        )
        .with_answer_payload(serde_json::json!({
            "antwort_code": "A01", "antwort_tree": "E_0622", "zustimmung": true,
        }));
        let body = decision_body(
            &e,
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            e.answer_payload.as_ref(),
            &OperatorAnswer::default(),
        )
        .expect("a frozen answer is complete");
        assert_eq!(body["antwort_code"], "A01");
        assert_eq!(body["malo_id"], "51238696012");
        assert_eq!(body["process_id"], serde_json::json!(Uuid::nil()));
    }

    /// An escalated entry has no answer until the operator states one.
    #[test]
    fn an_escalation_needs_the_operators_antwortcode() {
        let e = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        );
        let refused = decision_body(
            &e,
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            None,
            &OperatorAnswer::default(),
        );
        assert!(
            matches!(&refused, Err(DecisionRefusal::Missing { fields, .. }) if fields == &["antwort_code"]),
            "{refused:?}"
        );
        let answer = OperatorAnswer {
            antwort_code: Some("A01".into()),
            antwort_tree: Some("E_0622".into()),
            ..OperatorAnswer::default()
        };
        let body = decision_body(
            &e,
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            None,
            &answer,
        )
        .expect("complete");
        assert_eq!(body["antwort_code"], "A01");
    }

    /// A refusal's Antwortcode is the operator's; the business key is frozen.
    #[test]
    fn a_reject_carries_every_field_makod_requires() {
        let e = entry(
            mako_markt::commands::WIM_STEUERUNGSAUFTRAG_BESTAETIGEN,
            mako_markt::commands::WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
        )
        .with_reject_payload(serde_json::json!({ "tx_id": "tx-1" }));
        let body = decision_body(
            &e,
            mako_markt::commands::WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
            e.reject_payload.as_ref(),
            &OperatorAnswer::default(),
        )
        .expect("the tx_id is frozen");
        assert_eq!(body["tx_id"], "tx-1");

        let gpke = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        );
        assert!(matches!(
            decision_body(
                &gpke,
                mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
                None,
                &OperatorAnswer::default()
            ),
            Err(DecisionRefusal::Missing { .. })
        ));
    }

    /// What was shown is what is approved.
    #[test]
    fn a_frozen_answer_cannot_be_edited_on_approval() {
        let e = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        )
        .with_answer_payload(serde_json::json!({ "antwort_code": "A01" }));
        let edited = OperatorAnswer {
            antwort_code: Some("A99".into()),
            ..OperatorAnswer::default()
        };
        assert_eq!(
            decision_body(
                &e,
                mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
                e.answer_payload.as_ref(),
                &edited
            ),
            Err(DecisionRefusal::AnswerFrozen)
        );
    }
}
