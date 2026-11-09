//! PostgreSQL implementation for `approval_queue`.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
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
    /// The Entscheidungsbaum the enqueuing module decided this process with,
    /// recorded at enqueue — the tree whose Codeliste a refusal (and, through
    /// [`mako_pruefung::codes::zustimmung_tree_of`], an operator's Bestätigung)
    /// must draw its Antwortcode from. A code means something only inside its
    /// tree, so the decision routes check an operator's code against it.
    ///
    /// `None` where the answer carries no EBD code at all: the ESA Werteanfrage
    /// refusal (a QUOTES Begründung), the REQOTE Preisanfrage, the
    /// API-Webdienste Steuerungsauftrag.
    pub ablehnung_ebd: Option<String>,
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
            ablehnung_ebd: None,
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

    /// Record the Entscheidungsbaum the module decided with. See
    /// [`Self::ablehnung_ebd`].
    #[must_use]
    pub fn with_ebd(mut self, ebd: &str) -> Self {
        self.ablehnung_ebd = Some(ebd.to_owned());
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
                             answer_payload, reject_payload, ablehnung_ebd, \
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
        ablehnung_ebd: row.try_get("ablehnung_ebd")?,
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
            "INSERT INTO approval_queue (id, process_id, pid, malo_id, reason, status, approve_command, reject_command, marktrolle, followup_command, followup_payload, answer_payload, reject_payload, ablehnung_ebd, expires_at, created_at, tenant) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) ON CONFLICT (process_id, tenant) DO NOTHING",
        )
        .bind(entry.id).bind(entry.process_id).bind(entry.pid).bind(&entry.malo_id)
        .bind(&entry.reason).bind(entry.status.to_string())
        .bind(&entry.approve_command).bind(&entry.reject_command).bind(&entry.marktrolle)
        .bind(&entry.followup_command).bind(&entry.followup_payload)
        .bind(&entry.answer_payload).bind(&entry.reject_payload)
        .bind(&entry.ablehnung_ebd)
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
        let opt = sqlx::query(mako_service::sql::dynamic(format!(
            "UPDATE approval_queue SET status = $3, decided_at = now(), decided_by = $4 \
             WHERE id = $1 AND tenant = $2 AND status = 'Pending' RETURNING {ENTRY_COLUMNS}"
        )))
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

    /// Entries of `tenant`, optionally of one `status` and one `marktrolle`,
    /// the **earliest `expires_at` first** — the order an operator has to work
    /// them in, since `expires_at` is the business Antwortfrist less headroom.
    pub async fn list(
        &self,
        tenant: &str,
        status: Option<QueueStatus>,
        marktrolle: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ApprovalQueueEntry>, sqlx::Error> {
        let status_str = status.map(|s| s.to_string());
        let rows = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {ENTRY_COLUMNS} FROM approval_queue WHERE tenant = $1 \
             AND ($2::text IS NULL OR status = $2) AND ($3::text IS NULL OR marktrolle = $3) \
             ORDER BY expires_at ASC, created_at ASC, id ASC LIMIT $4"
        )))
        .bind(tenant)
        .bind(status_str)
        .bind(marktrolle)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(map_entry).collect()
    }

    pub async fn find_by_id(
        &self,
        id: Uuid,
        tenant: &str,
    ) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
        let opt = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {ENTRY_COLUMNS} FROM approval_queue WHERE id = $1 AND tenant = $2"
        )))
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
    /// `bilanzkreis` — the LF's Bilanzkreis on an E/G Zustimmung (Strom
    /// `SG8 SEQ+Z79`, Gas `SG10 CCI+Z19`).
    Bilanzkreis,
    /// `versorgungsart` — `SG10 CCI+Z36` on a Strom 55014.
    Versorgungsart,
    /// `abrechnungsintervall_monate` — `SG6 DTM+Z20` on a Gas 44014, a number.
    Abrechnungsintervall,
    /// The Grundlage nach EnFG on a Strom 55014 — `enfg_grundlage` (`SG8
    /// SEQ+Z75 CCI+Z61`) and, beside `ZF9`, `enfg_grund` (`SG10 CAV`,
    /// Bedingung `[463]`). The operator states both as one [`enfg_fakt`] value.
    Enfg,
}

/// The values an operator states the Grundlage nach EnFG with: `ZG0`, `ZG1`,
/// or `ZF9+<Grund>` for each Grund der Privilegierung — the one shape that
/// cannot name a Grund beside `ZG0`/`ZG1` or leave it out beside `ZF9`.
#[must_use]
pub fn enfg_werte() -> Vec<(String, String)> {
    use mako_markt::commands::{ENFG_GRUNDLAGEN, ENFG_PRIVILEGIERUNGEN, enfg_mit_grund};
    let mut out = Vec::new();
    for (code, text) in ENFG_GRUNDLAGEN {
        if enfg_mit_grund(code) {
            for (grund, grund_text) in ENFG_PRIVILEGIERUNGEN {
                out.push((format!("{code}+{grund}"), format!("{text}: {grund_text}")));
            }
        } else {
            out.push((code.to_owned(), text.to_owned()));
        }
    }
    out
}

/// `(enfg_grundlage, enfg_grund)` of an [`enfg_werte`] value, or `None` when
/// it is none of them.
#[must_use]
pub fn enfg_fakt(wert: &str) -> Option<(String, Option<String>)> {
    enfg_werte()
        .iter()
        .any(|(w, _)| w == wert)
        .then(|| match wert.split_once('+') {
            Some((grundlage, grund)) => (grundlage.to_owned(), Some(grund.to_owned())),
            None => (wert.to_owned(), None),
        })
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
            Self::Bilanzkreis => has("bilanzkreis"),
            Self::Versorgungsart => has("versorgungsart"),
            Self::Abrechnungsintervall => body
                .get("abrechnungsintervall_monate")
                .is_some_and(serde_json::Value::is_u64),
            Self::Enfg => match body
                .get("enfg_grundlage")
                .and_then(serde_json::Value::as_str)
            {
                Some(g) if mako_markt::commands::enfg_mit_grund(g) => has("enfg_grund"),
                Some(g) => !g.is_empty(),
                None => false,
            },
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
            Self::Bilanzkreis => "bilanzkreis",
            Self::Versorgungsart => "versorgungsart",
            Self::Abrechnungsintervall => "abrechnungsintervall_monate",
            Self::Enfg => "enfg",
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
    use Pflichtfeld::{
        Abrechnungsintervall, Antwortcode, Begruendung, Bilanzkreis, Enfg, Lokation, MaloId,
        MeloId, TxId, Versorgungsart,
    };
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
        // A 44014 states the LF's Bilanzkreis and Abrechnungsintervall (UTILMD
        // AHB Gas 1.2 Kap. 5.9, Muss), and `geli.eog.bestaetigen` refuses it
        // without either.
        (
            c::GELI_EOG_BESTAETIGEN,
            &[MaloId, Antwortcode, Bilanzkreis, Abrechnungsintervall],
        ),
        (c::GELI_EOG_ABLEHNEN, MALO_CODE),
        // `dispatch_gpke_eog_antwort` reads the cluster from the command; a
        // 55014 states the Versorgungsart, the Bilanzkreis and the Grundlage
        // nach EnFG (UTILMD AHB Strom 2.2 Kap. 8.6, S. 129, 136, 138).
        (
            c::GPKE_EOG_BESTAETIGEN,
            &[MaloId, Versorgungsart, Bilanzkreis, Enfg],
        ),
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

/// The facts — [`Pflichtfeld::Bilanzkreis`], [`Pflichtfeld::Versorgungsart`],
/// [`Pflichtfeld::Abrechnungsintervall`], [`Pflichtfeld::Enfg`] — `command`
/// needs that `frozen` does not hold: what the operator states on the decision,
/// beside the code.
#[must_use]
pub fn fehlende_fakten(command: &str, frozen: Option<&serde_json::Value>) -> Vec<&'static str> {
    let empty = serde_json::Map::new();
    let body = frozen
        .and_then(serde_json::Value::as_object)
        .unwrap_or(&empty);
    answer_contract(command)
        .unwrap_or_default()
        .iter()
        .filter(|f| {
            matches!(
                f,
                Pflichtfeld::Bilanzkreis
                    | Pflichtfeld::Versorgungsart
                    | Pflichtfeld::Abrechnungsintervall
                    | Pflichtfeld::Enfg
            ) && !f.satisfied_by(body)
        })
        .map(|f| f.key())
        .collect()
}

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
    /// The Bilanzkreis an E/G Zustimmung names, where the entry's own facts
    /// did not resolve one.
    #[serde(default)]
    pub bilanzkreis: Option<String>,
    /// `SG10 CCI+Z36` on a Strom 55014 — `ZC9`, `ZD0` or `ZE3`.
    #[serde(default)]
    pub versorgungsart: Option<String>,
    /// `SG6 DTM+Z20` on a Gas 44014 — 1, 3, 6 or 12.
    #[serde(default)]
    pub abrechnungsintervall_monate: Option<u8>,
    /// The Grundlage nach EnFG on a Strom 55014 — `ZG0`, `ZG1` or
    /// `ZF9+<Grund der Privilegierung>` ([`enfg_werte`]).
    #[serde(default)]
    pub enfg: Option<String>,
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
    /// The stated `antwort_tree` is not the tree this decision answers from.
    WrongTree {
        /// What the operator stated.
        stated: String,
        /// The tree the entry was decided in, for this direction.
        expected: String,
    },
    /// The stated Antwortcode is not in the tree's cluster for this decision —
    /// a refusal with a Zustimmungscode, or a code the tree does not publish.
    CodeOutsideTree {
        /// The stated code.
        code: String,
        /// The tree it was checked in.
        ebd: String,
        /// The cluster the decision requires.
        cluster: &'static str,
    },
    /// A fact stated for a command that does not carry it.
    FactNotTaken {
        /// The command.
        command: String,
        /// The fact.
        field: &'static str,
    },
    /// A stated fact outside the values its segment admits.
    InvalidFact {
        /// The fact.
        field: &'static str,
        /// What was stated.
        value: String,
        /// What the segment admits, and where that is published.
        admitted: &'static str,
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
            Self::WrongTree { stated, expected } => write!(
                f,
                "antwort_tree {stated:?} is not the Entscheidungsbaum this decision answers \
                 from; state a code of {expected}"
            ),
            Self::CodeOutsideTree { code, ebd, cluster } => write!(
                f,
                "Antwortcode {code:?} is not in the {cluster}-Cluster of {ebd} — a code means \
                 something only inside its tree"
            ),
            Self::FactNotTaken { command, field } => {
                write!(
                    f,
                    "{command} carries no {field}; leave it off this decision"
                )
            }
            Self::InvalidFact {
                field,
                value,
                admitted,
            } => write!(f, "{field} {value:?} is not admitted: {admitted}"),
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
    // The facts an E/G Zustimmung states beside its code, where the entry's
    // own did not resolve: each only on a command that carries it, never over
    // a frozen one, and only a value its segment admits.
    let fakten: [(Pflichtfeld, Option<serde_json::Value>); 3] = [
        (
            Pflichtfeld::Bilanzkreis,
            operator
                .bilanzkreis
                .clone()
                .filter(|v| !v.is_empty())
                .map(serde_json::Value::String),
        ),
        (
            Pflichtfeld::Versorgungsart,
            match operator.versorgungsart.as_deref().filter(|v| !v.is_empty()) {
                Some(code) if mako_gpke::Versorgungsart::from_code(code).is_none() => {
                    return Err(DecisionRefusal::InvalidFact {
                        field: "versorgungsart",
                        value: code.to_owned(),
                        admitted: "ZC9, ZD0 or ZE3 (SG10 CCI+Z36, DE 7037)",
                    });
                }
                other => other.map(|c| serde_json::Value::String(c.to_owned())),
            },
        ),
        (
            Pflichtfeld::Abrechnungsintervall,
            match operator.abrechnungsintervall_monate {
                Some(n) if !mako_markt::commands::ABRECHNUNGSINTERVALLE_MONATE.contains(&n) => {
                    return Err(DecisionRefusal::InvalidFact {
                        field: "abrechnungsintervall_monate",
                        value: n.to_string(),
                        admitted: "1, 3, 6 or 12 (SG6 DTM+Z20, UTILMD MIG Gas G1.2 S. 55)",
                    });
                }
                other => other.map(|n| serde_json::json!(n)),
            },
        ),
    ];
    // The Grundlage nach EnFG goes out as two keys: `enfg_grundlage` and,
    // beside `ZF9`, `enfg_grund`.
    if let Some(wert) = operator.enfg.as_deref().filter(|v| !v.is_empty()) {
        let Some((grundlage, grund)) = enfg_fakt(wert) else {
            return Err(DecisionRefusal::InvalidFact {
                field: "enfg",
                value: wert.to_owned(),
                admitted: "ZG0, ZG1 or ZF9+<ZU5…ZV3> (SG8 SEQ+Z75 CCI+Z61 and, beside ZF9, \
                           SG10 CAV [463])",
            });
        };
        if !contract.contains(&Pflichtfeld::Enfg) {
            return Err(DecisionRefusal::FactNotTaken {
                command: command.to_owned(),
                field: Pflichtfeld::Enfg.key(),
            });
        }
        if body.contains_key("enfg_grundlage") || body.contains_key("enfg_grund") {
            return Err(DecisionRefusal::AnswerFrozen);
        }
        body.insert(
            "enfg_grundlage".into(),
            serde_json::Value::String(grundlage),
        );
        if let Some(grund) = grund {
            body.insert("enfg_grund".into(), serde_json::Value::String(grund));
        }
    }
    for (feld, value) in fakten {
        let Some(value) = value else { continue };
        if !contract.contains(&feld) {
            return Err(DecisionRefusal::FactNotTaken {
                command: command.to_owned(),
                field: feld.key(),
            });
        }
        if body.contains_key(feld.key()) {
            return Err(DecisionRefusal::AnswerFrozen);
        }
        body.insert(feld.key().to_owned(), value);
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

/// Check the Antwortcode an operator states against the tree `entry` was
/// decided in ([`ApprovalQueueEntry::ablehnung_ebd`]).
///
/// The code must sit in the cluster of the answer the decision dispatches:
///
/// | Entry | Decision | Cluster |
/// |---|---|---|
/// | escalated | approve | Zustimmung |
/// | escalated | reject | Ablehnung |
/// | held | reject | the **other** cluster than the held answer's — rejecting a held answer overrides it (the LF and ESA queues dispatch the opposite command) |
///
/// A held answer's approval is frozen, and [`decision_body`] already refuses
/// a code stated on it. A Zustimmung is drawn from the tree the Bestätigung
/// comes from — the same tree, or for a Vorprüfung the tree it hands over to
/// ([`mako_pruefung::codes::zustimmung_tree_of`]); an Ablehnung from the
/// recorded tree itself.
///
/// Each answer is restricted to one cluster — ORDRSP AHB 1.1b §4.15
/// conditions \[17\]/\[18\]: the code „muss im EBD dem Cluster Zustimmung /
/// Ablehnung zugeordnet sein"; the UTILMD side is set out in
/// [`mako_pruefung::codes`] — and `makod` derives the answer PID from the
/// code's cluster, so a code of the wrong cluster would send the other answer.
///
/// Nothing is checked where the entry recorded no tree (an answer that carries
/// no EBD code), the operator states no code, or the held answer's cluster
/// cannot be read.
///
/// # Errors
///
/// [`DecisionRefusal::WrongTree`] or [`DecisionRefusal::CodeOutsideTree`].
pub fn check_operator_code(
    entry: &ApprovalQueueEntry,
    approve: bool,
    operator: &OperatorAnswer,
) -> Result<(), DecisionRefusal> {
    use mako_pruefung::codes::codes_im_cluster;
    let Some(code) = operator.antwort_code.as_deref().filter(|c| !c.is_empty()) else {
        return Ok(());
    };
    let Some((tree, cluster)) = crate::freigabe::erwarteter_code(entry, approve) else {
        return Ok(());
    };
    if let Some(stated) = operator.antwort_tree.as_deref().filter(|t| !t.is_empty())
        && stated != tree
    {
        return Err(DecisionRefusal::WrongTree {
            stated: stated.to_owned(),
            expected: tree.to_owned(),
        });
    }
    if codes_im_cluster(tree, cluster).any(|c| c.code == code) {
        Ok(())
    } else {
        Err(DecisionRefusal::CodeOutsideTree {
            code: code.to_owned(),
            ebd: tree.to_owned(),
            cluster: cluster.label(),
        })
    }
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

    /// A refusal's code must be one of the recorded tree's Ablehnungen.
    #[test]
    fn a_reject_code_outside_the_trees_ablehnungen_is_refused() {
        let e = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        )
        .with_ebd(mako_pruefung::codes::EBD_LIEFERBEGINN);
        let said = |code: &str| OperatorAnswer {
            antwort_code: Some(code.into()),
            ..OperatorAnswer::default()
        };
        // `A51` is the Zustimmung of `E_0623`: no refusal can carry it.
        assert!(matches!(
            check_operator_code(&e, false, &said("A51")),
            Err(DecisionRefusal::CodeOutsideTree { .. })
        ));
        // `A32` is an `E_0622` and an `E_0624` Ablehnung — not an `E_0623` one,
        // so a refusal answering from `E_0623` cannot carry it.
        assert!(matches!(
            check_operator_code(&e, false, &said("A32")),
            Err(DecisionRefusal::CodeOutsideTree { .. })
        ));
        let ablehnung = mako_pruefung::codes::codes_im_cluster(
            mako_pruefung::codes::EBD_LIEFERBEGINN,
            mako_pruefung::codes::Cluster::Ablehnung,
        )
        .next()
        .expect("E_0623 publishes an Ablehnung");
        assert_eq!(
            check_operator_code(&e, false, &said(ablehnung.code)),
            Ok(())
        );
        // …and the same code on an approval is the wrong cluster.
        assert!(check_operator_code(&e, true, &said(ablehnung.code)).is_err());
        assert_eq!(check_operator_code(&e, true, &said("A51")), Ok(()));
        // A tree the decision was not taken in is refused by name.
        let elsewhere = OperatorAnswer {
            antwort_tree: Some("E_0607".into()),
            ..said(ablehnung.code)
        };
        assert!(matches!(
            check_operator_code(&e, false, &elsewhere),
            Err(DecisionRefusal::WrongTree { .. })
        ));
        // No recorded tree: nothing to check against.
        let untyped = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        );
        assert_eq!(check_operator_code(&untyped, false, &said("A32")), Ok(()));
    }

    /// An escalated Vorprüfung is approved out of the tree it hands over to.
    #[test]
    fn a_vorpruefung_escalation_is_approved_from_its_zustimmungs_tree() {
        let e = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        )
        .with_ebd(mako_pruefung::codes::EBD_ANMELDUNG_DIREKT_ABLEHNBAR);
        let answer = OperatorAnswer {
            antwort_code: Some("A51".into()),
            antwort_tree: Some(mako_pruefung::codes::EBD_LIEFERBEGINN.into()),
            ..OperatorAnswer::default()
        };
        assert_eq!(check_operator_code(&e, true, &answer), Ok(()));
    }

    /// Rejecting a held answer overrides it with the other cluster: a held
    /// Ablehnung (the LF queue holds both) is rejected with a Zustimmung.
    #[test]
    fn rejecting_a_held_answer_states_the_other_cluster() {
        let ablehnung = mako_pruefung::codes::codes_im_cluster(
            mako_pruefung::codes::EBD_LIEFERBEGINN,
            mako_pruefung::codes::Cluster::Ablehnung,
        )
        .next()
        .expect("E_0623 publishes an Ablehnung");
        let held_accept = entry(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        )
        .with_ebd(mako_pruefung::codes::EBD_LIEFERBEGINN)
        .with_answer_payload(serde_json::json!({ "antwort_code": "A51" }));
        let said = |code: &str| OperatorAnswer {
            antwort_code: Some(code.into()),
            ..OperatorAnswer::default()
        };
        assert_eq!(
            check_operator_code(&held_accept, false, &said(ablehnung.code)),
            Ok(())
        );
        assert!(check_operator_code(&held_accept, false, &said("A51")).is_err());

        let held_refusal =
            held_accept.with_answer_payload(serde_json::json!({ "antwort_code": ablehnung.code }));
        assert_eq!(
            check_operator_code(&held_refusal, false, &said("A51")),
            Ok(())
        );
        assert!(check_operator_code(&held_refusal, false, &said(ablehnung.code)).is_err());
    }

    /// A queued 44014 whose Bilanzkreis did not resolve is released with the
    /// operator's: the resolved Abrechnungsintervall stays frozen, the missing
    /// Bilanzkreis is stated, and a value the segment does not admit, a fact
    /// the command does not carry and an overwrite of a frozen one are refused.
    #[test]
    fn a_queued_eog_answer_is_released_with_the_operators_bilanzkreis() {
        use mako_markt::commands::{GELI_EOG_ABLEHNEN, GELI_EOG_BESTAETIGEN};
        let frozen = serde_json::json!({ "abrechnungsintervall_monate": 12 });
        let e = entry(GELI_EOG_BESTAETIGEN, GELI_EOG_ABLEHNEN).with_answer_payload(frozen.clone());
        let code = || OperatorAnswer {
            antwort_code: Some("E15".into()),
            ..OperatorAnswer::default()
        };

        let refused = decision_body(&e, GELI_EOG_BESTAETIGEN, Some(&frozen), &code());
        assert!(
            matches!(&refused, Err(DecisionRefusal::Missing { fields, .. }) if fields == &["bilanzkreis"]),
            "{refused:?}"
        );

        let body = decision_body(
            &e,
            GELI_EOG_BESTAETIGEN,
            Some(&frozen),
            &OperatorAnswer {
                bilanzkreis: Some("BKGAS0001".into()),
                ..code()
            },
        )
        .expect("the stated Bilanzkreis fills the gap");
        assert_eq!(body["bilanzkreis"], "BKGAS0001");
        assert_eq!(body["abrechnungsintervall_monate"], 12);

        let overwrite = decision_body(
            &e,
            GELI_EOG_BESTAETIGEN,
            Some(&frozen),
            &OperatorAnswer {
                bilanzkreis: Some("BKGAS0001".into()),
                abrechnungsintervall_monate: Some(3),
                ..code()
            },
        );
        assert_eq!(overwrite, Err(DecisionRefusal::AnswerFrozen));

        let invalid = decision_body(
            &e,
            GELI_EOG_BESTAETIGEN,
            None,
            &OperatorAnswer {
                abrechnungsintervall_monate: Some(2),
                ..code()
            },
        );
        assert!(
            matches!(
                invalid,
                Err(DecisionRefusal::InvalidFact {
                    field: "abrechnungsintervall_monate",
                    ..
                })
            ),
            "{invalid:?}"
        );

        let not_taken = decision_body(
            &e,
            GELI_EOG_ABLEHNEN,
            None,
            &OperatorAnswer {
                antwort_code: Some("A02".into()),
                bilanzkreis: Some("BKGAS0001".into()),
                ..OperatorAnswer::default()
            },
        );
        assert!(
            matches!(
                not_taken,
                Err(DecisionRefusal::FactNotTaken {
                    field: "bilanzkreis",
                    ..
                })
            ),
            "{not_taken:?}"
        );
    }

    /// A Strom 55014 from the queue states its Versorgungsart, Bilanzkreis and
    /// Grundlage nach EnFG; a code `CCI+Z36` does not publish is refused.
    #[test]
    fn a_queued_55014_needs_versorgungsart_bilanzkreis_and_enfg() {
        use mako_markt::commands::{GPKE_EOG_ABLEHNEN, GPKE_EOG_BESTAETIGEN};
        let e = entry(GPKE_EOG_BESTAETIGEN, GPKE_EOG_ABLEHNEN);
        let refused = decision_body(&e, GPKE_EOG_BESTAETIGEN, None, &OperatorAnswer::default());
        assert!(
            matches!(&refused, Err(DecisionRefusal::Missing { fields, .. })
                if fields == &["versorgungsart", "bilanzkreis", "enfg"]),
            "{refused:?}"
        );
        // `ZF9+<Grund>` goes out as both keys; `ZG0` as the Grundlage alone.
        for (wert, grundlage, grund) in [("ZF9+ZU6", "ZF9", Some("ZU6")), ("ZG0", "ZG0", None)] {
            let body = decision_body(
                &e,
                GPKE_EOG_BESTAETIGEN,
                None,
                &OperatorAnswer {
                    versorgungsart: Some("ZC9".into()),
                    bilanzkreis: Some("11XBK-STD------Y".into()),
                    enfg: Some(wert.into()),
                    ..OperatorAnswer::default()
                },
            )
            .expect("a complete 55014 body");
            assert_eq!(body["enfg_grundlage"], grundlage);
            assert_eq!(body.get("enfg_grund").and_then(|v| v.as_str()), grund);
        }
        for falsch in ["ZF9", "ZG0+ZU6", "ZU6"] {
            let refused = decision_body(
                &e,
                GPKE_EOG_BESTAETIGEN,
                None,
                &OperatorAnswer {
                    enfg: Some(falsch.into()),
                    ..OperatorAnswer::default()
                },
            );
            assert!(
                matches!(
                    refused,
                    Err(DecisionRefusal::InvalidFact { field: "enfg", .. })
                ),
                "{falsch}: {refused:?}"
            );
        }
        let wrong = decision_body(
            &e,
            GPKE_EOG_BESTAETIGEN,
            None,
            &OperatorAnswer {
                versorgungsart: Some("ZZD".into()),
                ..OperatorAnswer::default()
            },
        );
        assert!(
            matches!(
                wrong,
                Err(DecisionRefusal::InvalidFact {
                    field: "versorgungsart",
                    ..
                })
            ),
            "{wrong:?}"
        );
    }
}
