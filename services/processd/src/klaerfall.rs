//! Klärfälle: one case per message mako could not bring to its end, with a
//! cause, an action and a due date taken from tables and code — never from a
//! model.
//!
//! Five signals open or attach to a case:
//!
//! | Origin | Signal | Cause | Key |
//! |---|---|---|---|
//! | `aperak` | `de.mako.aperak.rejected` | the ERC code, [`recommended_action`] | the rejected message (`bezug_message_ref`) |
//! | `dead_letter` | `de.mako.dead-letter.recorded` | the reason, [`action_for_label`] | the message, else `dead-letter:<reason>` |
//! | `refusal` | `de.mako.process.completed` whose Antwortcode refuses | the code in its tree, [`mako_pruefung::codes::lies`] | the process |
//! | `frist_missed` | `de.mako.antwortfrist.abgelaufen` | the lapse | the process |
//! | `operator_decision` | an `approval_queue` row, opened in its enqueue transaction | the Freigabe | the process |
//!
//! The key is the message *and* the partner that sent the signal: a
//! `message_ref` is unique only per sender, so an APERAK from partner Q that
//! cites the reference of a message sent to P opens Q's own case and never
//! attaches to or closes P's. A dead letter that names no message keys on its
//! reason, so a flood of them from one partner is one case with an occurrence
//! count. Only an open case is attached to: once a case is closed, the next
//! signal opens a new one.
//!
//! A case is read only by a caller holding the Marktrolle mako acted in on its
//! process (`process_projections.mdm_role`); a case on no process — a dead
//! letter that names none — only by `ADMIN`. The § 6a Abs. 1 EnWG
//! confidentiality duty is an access-control duty, and this filter is where a
//! case read meets it.
//!
//! A further signal for the same key, from any origin, attaches: the count
//! rises, the origin joins `origins`, and the first cause stands. The due date
//! is the process's business Antwortfrist from the read model
//! (`process_projections.deadline_at`, computed by `mako_fristen::antwort`) or
//! null — never a default, and never a date the signal carries. A
//! counterparty's free text never reaches a case: only codes are stored, and
//! the wording is the BDEW's own.
//!
//! The correcting command ([`korrektur`]) exists only where mako's outbound
//! answer is correctable: an answer a Freigabe sends. Its approve/reject pair
//! is the allowed set, the command it proposes or sent is the proposal, and a
//! dispatch runs through the Freigabe decision path — `decision_body`, the
//! code check and makod's pre-send check. Every other case names no command
//! and escalates.
//!
//! [`recommended_action`]: mako_engine::erc::recommended_action
//! [`action_for_label`]: mako_engine::dead_letter::action_for_label

use serde::Serialize;
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::pg::approval::{ApprovalQueueEntry, QueueStatus};

/// Where a case's signal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Herkunft {
    /// An APERAK refused a message mako sent.
    Aperak,
    /// makod could not place a message.
    DeadLetter,
    /// A counterparty's answer refused mako's request.
    Refusal,
    /// A business Antwortfrist lapsed unanswered.
    FristMissed,
    /// A process waits for a person's Freigabe (`approval_queue`).
    OperatorDecision,
}

impl Herkunft {
    /// Every origin, in the order of the SQL `CHECK`.
    pub const ALL: [Self; 5] = [
        Self::Aperak,
        Self::DeadLetter,
        Self::Refusal,
        Self::FristMissed,
        Self::OperatorDecision,
    ];

    /// The wire and SQL literal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aperak => "aperak",
            Self::DeadLetter => "dead_letter",
            Self::Refusal => "refusal",
            Self::FristMissed => "frist_missed",
            Self::OperatorDecision => "operator_decision",
        }
    }
}

/// What a case proposes, from the cause table of its origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aktion {
    /// Correct the message and send it again.
    RetryWithCorrection,
    /// A person has to look.
    EscalateToOperator,
    /// The counterparty decided; the process ends.
    AbortProcess,
    /// Wait for a conflicting process, then retry.
    WaitAndRetry,
}

impl Aktion {
    /// Every action, in the order of the SQL `CHECK`.
    pub const ALL: [Self; 4] = [
        Self::RetryWithCorrection,
        Self::EscalateToOperator,
        Self::AbortProcess,
        Self::WaitAndRetry,
    ];

    /// The wire and SQL literal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetryWithCorrection => "retry_with_correction",
            Self::EscalateToOperator => "escalate_to_operator",
            Self::AbortProcess => "abort_process",
            Self::WaitAndRetry => "wait_and_retry",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }

    /// The action and its hint from the engine's table.
    fn of(action: &mako_engine::erc::ErcAction) -> (Self, String) {
        use mako_engine::erc::ErcAction;
        match action {
            ErcAction::RetryWithCorrection { field } => {
                (Self::RetryWithCorrection, format!("correct: {field}"))
            }
            ErcAction::EscalateToOperator { reason } => {
                (Self::EscalateToOperator, (*reason).to_owned())
            }
            ErcAction::AbortProcess => (
                Self::AbortProcess,
                "the counterparty has decided; nothing resent changes it".to_owned(),
            ),
            ErcAction::WaitAndRetry { reason } => (Self::WaitAndRetry, (*reason).to_owned()),
        }
    }
}

/// Where a case stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Zustand {
    /// Waiting for somebody.
    Open,
    /// Closed by a person without a market message.
    Resolved,
    /// Closed by dispatching a command.
    Dispatched,
}

impl Zustand {
    /// Every state, in the order of the SQL `CHECK`.
    pub const ALL: [Self; 3] = [Self::Open, Self::Resolved, Self::Dispatched];

    /// The wire and SQL literal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Dispatched => "dispatched",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|z| z.as_str() == s)
    }
}

/// One signal, read from a CloudEvent or a Freigabe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub herkunft: Herkunft,
    /// The case key: the message the signal concerns, `process:<id>`, or
    /// `dead-letter:<reason>`.
    pub message_ref: String,
    /// The MP-ID that sent the signal — the second half of the case key; `None`
    /// for a signal about a process.
    pub partner_mp_id: Option<String>,
    pub process_id: Option<Uuid>,
    pub pid: Option<u32>,
    /// The code the cause is (`Z29`, `unknown_pid`, `A02`, `antwortfrist_abgelaufen`).
    pub ursache_code: String,
    /// The list the code is defined in (`ERC`, `dead_letter`, `E_0622`, …).
    pub ursache_liste: String,
    pub aktion: Aktion,
    pub hinweis: String,
    /// An `approval_queue` row's `expires_at` — the Frist less the operator's
    /// headroom, never the Frist itself.
    pub operator_window_ends_at: Option<OffsetDateTime>,
}

/// Whether a completion event reports a refusal of mako's request: its
/// `outcome` says so where the workflow states one, and otherwise the
/// Antwortcode's cluster in its Codeliste does (makod stamps the code from the
/// event envelope). A code off the agreement axis, or one the catalogue does
/// not hold, is no refusal.
fn ist_ablehnung(data: &serde_json::Value, code: &str, liste: &str) -> bool {
    match data["outcome"].as_str() {
        Some(outcome) => outcome == "rejected",
        None => {
            mako_pruefung::codes::lies(liste, code)
                .and_then(mako_pruefung::AntwortCode::ist_zustimmung)
                == Some(false)
        }
    }
}

/// The signal a CloudEvent is, or `None` for every event that opens no case.
#[must_use]
pub fn signal_of(event: &serde_json::Value) -> Option<Signal> {
    let ce_type = event["type"].as_str()?;
    let data = &event["data"];
    let process_id = event["subject"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok());
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| data["pid"].as_u64())
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p != 0);
    let process_key = || process_id.map(|p| format!("process:{p}"));
    let sender = || {
        data["sender"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match ce_type {
        mako_events::mako::APERAK_REJECTED => {
            let code = event["makoerc"]
                .as_str()
                .or_else(|| data["error_code"].as_str())
                .filter(|c| !c.is_empty())?;
            let message_ref = data["bezug_message_ref"]
                .as_str()
                .or_else(|| data["message_ref"].as_str())
                .filter(|r| !r.is_empty())
                .map(str::to_owned)
                .or_else(process_key)?;
            // An unknown code escalates: `recommended_action` never guesses.
            let (aktion, hinweis) = Aktion::of(&mako_engine::erc::recommended_action(
                &mako_engine::erc::ErcCode::new(code),
            ));
            Some(Signal {
                herkunft: Herkunft::Aperak,
                message_ref,
                partner_mp_id: sender(),
                process_id,
                pid,
                ursache_code: code.to_owned(),
                ursache_liste: "ERC".to_owned(),
                aktion,
                hinweis,
                operator_window_ends_at: None,
            })
        }
        mako_events::mako::DEAD_LETTER_RECORDED => {
            let label = data["reason"].as_str()?;
            let (aktion, hinweis) = mako_engine::dead_letter::action_for_label(label).map_or_else(
                || {
                    (
                        Aktion::EscalateToOperator,
                        format!("unknown dead-letter reason {label}"),
                    )
                },
                |a| Aktion::of(&a),
            );
            // A dead letter that names no message keys on its reason: one case
            // per partner and reason, however many arrive.
            let message_ref = data["message_ref"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map_or_else(|| format!("dead-letter:{label}"), str::to_owned);
            Some(Signal {
                herkunft: Herkunft::DeadLetter,
                message_ref,
                partner_mp_id: sender(),
                process_id,
                pid,
                ursache_code: label.to_owned(),
                ursache_liste: "dead_letter".to_owned(),
                aktion,
                hinweis,
                operator_window_ends_at: None,
            })
        }
        mako_events::mako::PROCESS_COMPLETED => {
            let code = data["antwortcode"].as_str().filter(|c| !c.is_empty())?;
            let liste = data["codeliste"].as_str().filter(|l| !l.is_empty())?;
            if !ist_ablehnung(data, code, liste) {
                return None;
            }
            Some(Signal {
                herkunft: Herkunft::Refusal,
                message_ref: process_key()?,
                partner_mp_id: None,
                process_id,
                pid,
                ursache_code: code.to_owned(),
                ursache_liste: liste.to_owned(),
                aktion: Aktion::EscalateToOperator,
                hinweis: "the counterparty refused the request — read its reason and decide"
                    .to_owned(),
                operator_window_ends_at: None,
            })
        }
        mako_events::mako::ANTWORTFRIST_ABGELAUFEN => Some(Signal {
            herkunft: Herkunft::FristMissed,
            message_ref: process_key()?,
            partner_mp_id: None,
            process_id,
            pid,
            ursache_code: "antwortfrist_abgelaufen".to_owned(),
            ursache_liste: "frist".to_owned(),
            aktion: Aktion::EscalateToOperator,
            hinweis: "the business Antwortfrist lapsed unanswered".to_owned(),
            operator_window_ends_at: None,
        }),
        _ => None,
    }
}

/// The `operator_decision` signal of a Freigabe: keyed on its process, so a
/// refusal or a lapse of the same process attaches to the same case.
#[must_use]
pub fn signal_of_freigabe(entry: &ApprovalQueueEntry) -> Signal {
    Signal {
        herkunft: Herkunft::OperatorDecision,
        message_ref: format!("process:{}", entry.process_id),
        partner_mp_id: None,
        process_id: Some(entry.process_id),
        pid: u32::try_from(entry.pid).ok().filter(|p| *p != 0),
        ursache_code: "freigabe".to_owned(),
        ursache_liste: "approval_queue".to_owned(),
        aktion: Aktion::EscalateToOperator,
        hinweis: "the answer waits for a person's Freigabe".to_owned(),
        operator_window_ends_at: Some(entry.expires_at),
    }
}

/// Open a case for `signal`, or attach it to the open one of its key — the
/// message and the partner that sent the signal, so a signal attaches only to
/// a case of its own sender.
///
/// One statement: the insert and the attach are the same upsert, so two
/// deliveries of one signal cannot open two cases. `exec` is a pool or the
/// transaction of the write the signal belongs to.
///
/// # Errors
///
/// A database failure; the caller must not acknowledge the event.
pub async fn open_or_attach<'e, E>(
    exec: E,
    tenant: &str,
    signal: &Signal,
    at: OffsetDateTime,
) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO klaerfaelle
           (id, tenant, message_ref, process_id, pid, origins, ursache_code, ursache_liste,
            aktion, hinweis, opened_at, last_signal_at, partner_mp_id, operator_window_ends_at)
         VALUES ($1, $2, $3, $4, $5, ARRAY[$6]::text[], $7, $8, $9, $10, $11, $11, $12, $13)
         ON CONFLICT (tenant, partner_mp_id, message_ref) WHERE state = 'open' DO UPDATE SET
           occurrences    = klaerfaelle.occurrences + 1,
           origins        = CASE WHEN $6 = ANY(klaerfaelle.origins) THEN klaerfaelle.origins
                                 ELSE array_append(klaerfaelle.origins, $6) END,
           process_id     = COALESCE(klaerfaelle.process_id, EXCLUDED.process_id),
           pid            = COALESCE(klaerfaelle.pid, EXCLUDED.pid),
           operator_window_ends_at = COALESCE(EXCLUDED.operator_window_ends_at,
                                              klaerfaelle.operator_window_ends_at),
           last_signal_at = GREATEST(klaerfaelle.last_signal_at, EXCLUDED.last_signal_at)",
    )
    .bind(Uuid::new_v4())
    .bind(tenant)
    .bind(&signal.message_ref)
    .bind(signal.process_id)
    .bind(signal.pid.and_then(|p| i32::try_from(p).ok()))
    .bind(signal.herkunft.as_str())
    .bind(&signal.ursache_code)
    .bind(&signal.ursache_liste)
    .bind(signal.aktion.as_str())
    .bind(&signal.hinweis)
    .bind(at)
    .bind(signal.partner_mp_id.as_deref())
    .bind(signal.operator_window_ends_at)
    .execute(exec)
    .await?;
    Ok(())
}

// ── The correcting command ────────────────────────────────────────────────────

/// The command that corrects a case, and the commands a dispatch may send
/// instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Korrektur {
    /// The proposed `mako_markt::commands` command.
    pub befehl: String,
    /// Every command a dispatch may send: the Freigabe's approve/reject pair.
    /// Another one than `befehl` needs a reason.
    pub erlaubt: Vec<String>,
}

/// The correcting command of a case with `origins`, from the Freigabe `entry`
/// of its process — `None` where mako's outbound answer is not correctable
/// here, and the case escalates.
///
/// | Case | Freigabe | Proposal |
/// |---|---|---|
/// | `operator_decision` | pending | its approve command, else its reject command |
/// | `aperak` | decided with a market message | the command the decision sent — re-sent corrected |
///
/// The allowed set is the Freigabe's approve/reject pair, each a
/// [`mako_markt::commands::FREIGABE_ANTWORTEN`] answer the decision path can
/// dispatch.
#[must_use]
pub fn korrektur(origins: &[String], entry: Option<&ApprovalQueueEntry>) -> Option<Korrektur> {
    let entry = entry?;
    let hat = |h: Herkunft| origins.iter().any(|o| o == h.as_str());
    let erlaubt: Vec<String> = [
        entry.approve_command.as_deref(),
        entry.reject_command.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|c| mako_markt::commands::FREIGABE_ANTWORTEN.contains(c))
    .map(str::to_owned)
    .collect();
    let befehl = match entry.status {
        QueueStatus::Pending if hat(Herkunft::OperatorDecision) => entry
            .approve_command
            .as_deref()
            .or(entry.reject_command.as_deref()),
        QueueStatus::Approved if hat(Herkunft::Aperak) && entry.gesendet_sha256.is_some() => {
            entry.approve_command.as_deref()
        }
        QueueStatus::Rejected if hat(Herkunft::Aperak) && entry.gesendet_sha256.is_some() => {
            entry.reject_command.as_deref()
        }
        _ => None,
    }?;
    erlaubt.iter().any(|c| c == befehl).then(|| Korrektur {
        befehl: befehl.to_owned(),
        erlaubt: erlaubt.clone(),
    })
}

/// Why a dispatch is refused before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchRefusal {
    /// The case names no correcting command.
    NoCorrection,
    /// Another command than the proposal, stated without a reason.
    BegruendungRequired {
        vorgeschlagen: String,
        befehl: String,
    },
    /// A command outside the Freigabe's approve/reject pair.
    CommandNotAllowed {
        befehl: String,
        erlaubt: Vec<String>,
    },
}

impl DispatchRefusal {
    /// The stable `code` the route answers with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoCorrection => "no_correction",
            Self::BegruendungRequired { .. } => "begruendung_required",
            Self::CommandNotAllowed { .. } => "command_not_allowed",
        }
    }
}

impl std::fmt::Display for DispatchRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCorrection => f.write_str(
                "this case names no correcting command — its answer is not one a Freigabe \
                 sends; resolve it instead",
            ),
            Self::BegruendungRequired {
                vorgeschlagen,
                befehl,
            } => write!(
                f,
                "{befehl} is not the proposed {vorgeschlagen}; state a begruendung to send \
                 another command than the proposal"
            ),
            Self::CommandNotAllowed { befehl, erlaubt } => write!(
                f,
                "{befehl} does not correct this case; it admits {}",
                erlaubt.join(", ")
            ),
        }
    }
}

/// The command a dispatch sends: the proposal, or another command of the
/// allowed set with a reason.
///
/// # Errors
///
/// A [`DispatchRefusal`].
pub fn waehle_befehl(
    korrektur: Option<&Korrektur>,
    befehl: Option<&str>,
    begruendung: Option<&str>,
) -> Result<String, DispatchRefusal> {
    let k = korrektur.ok_or(DispatchRefusal::NoCorrection)?;
    let befehl = befehl.filter(|b| !b.is_empty()).unwrap_or(&k.befehl);
    if !k.erlaubt.iter().any(|c| c == befehl) {
        return Err(DispatchRefusal::CommandNotAllowed {
            befehl: befehl.to_owned(),
            erlaubt: k.erlaubt.clone(),
        });
    }
    if befehl != k.befehl && begruendung.is_none_or(|b| b.trim().is_empty()) {
        return Err(DispatchRefusal::BegruendungRequired {
            vorgeschlagen: k.befehl.clone(),
            befehl: befehl.to_owned(),
        });
    }
    Ok(befehl.to_owned())
}

/// Whether a caller with `rollen` may send an answer the Freigabe owes as
/// `marktrolle`: the dispatched command's own authority, beyond the right to
/// dispatch a case at all. A Freigabe that names no role is decided by any
/// market role the queue admits.
#[must_use]
pub fn darf_senden(rollen: &[String], marktrolle: Option<&str>) -> bool {
    match marktrolle.filter(|r| !r.is_empty()) {
        Some(rolle) => rollen.iter().any(|r| r.eq_ignore_ascii_case(rolle)),
        None => rollen.iter().any(|r| {
            ["NB", "LF", "MSB"]
                .iter()
                .any(|m| r.eq_ignore_ascii_case(m))
        }),
    }
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// One case as the Desk's Heute and an integrator read it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Klaerfall {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The message the case concerns, `process:<id>`, or `dead-letter:<reason>`.
    pub message_ref: String,
    /// The MP-ID whose signal opened the case; `None` for a case on a process.
    pub partner_mp_id: Option<String>,
    #[schemars(with = "Option<String>")]
    pub process_id: Option<Uuid>,
    pub pid: Option<u32>,
    /// The Marktrolle that owns the case — the role mako acted in on its
    /// process; `None` for a case on no process (`ADMIN`'s).
    pub owner_role: Option<String>,
    /// The origins of every signal attached, the first one first.
    pub origins: Vec<String>,
    pub zustand: Zustand,
    /// The cause's code and the list that defines it.
    pub ursache_code: String,
    pub ursache_liste: String,
    /// The BDEW's own wording of the cause, where the list publishes one
    /// (an Antwortcode in its Entscheidungsbaum); `None` otherwise — never a
    /// paraphrase.
    pub bedeutung: Option<&'static str>,
    /// The tabulated action and what it says to do.
    pub aktion: Aktion,
    pub hinweis: String,
    /// The `mako_markt::commands` command that corrects the case and the ones
    /// a dispatch may send instead ([`korrektur`]); `None` where the case's
    /// answer is not correctable here, and it escalates.
    pub korrektur: Option<Korrektur>,
    /// The business Antwortfrist the case races (`mako_fristen::antwort`, via
    /// the read model), RFC 3339; `None` is „keine Frist", never a default.
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub due_at: Option<OffsetDateTime>,
    /// The Fundstelle of `due_at`.
    pub due_source: Option<String>,
    /// `due_at` has passed and the case is still open.
    pub overdue: bool,
    /// A Freigabe's `expires_at` — the Frist less the operator's headroom.
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub operator_window_ends_at: Option<OffsetDateTime>,
    /// The person working the case, if somebody took it.
    pub assignee: Option<String>,
    /// The authenticated principal that closed it.
    pub decided_by: Option<String>,
    pub occurrences: i32,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub opened_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub closed_at: Option<OffsetDateTime>,
}

/// What `list_klaerfaelle` returns.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Klaerfaelle {
    pub count: usize,
    /// At least `limit` cases matched.
    pub saturated: bool,
    pub klaerfaelle: Vec<Klaerfall>,
}

/// One step taken on a case.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VerlaufEintrag {
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub at: OffsetDateTime,
    /// The authenticated principal.
    pub akteur: String,
    /// `assign`, `resolve` or `dispatch`.
    pub schritt: String,
    /// On a dispatch: the proposed command and the one sent.
    pub vorgeschlagen: Option<String>,
    pub befehl: Option<String>,
    /// The operator's reason (a resolve, an override).
    pub begruendung: Option<String>,
    /// On an assign: who took it (`None` releases it).
    pub assignee: Option<String>,
}

/// A case with its history, as `get_klaerfall` and `GET /api/v1/klaerfaelle/{id}`
/// return it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct KlaerfallAkte {
    pub klaerfall: Klaerfall,
    pub verlauf: Vec<VerlaufEintrag>,
}

/// The columns every read selects, `k` the case and `p` its projection.
const CASE_COLUMNS: &str = "k.id, k.message_ref, k.partner_mp_id, k.process_id, k.pid, k.origins, \
     k.state, k.ursache_code, k.ursache_liste, k.aktion, k.hinweis, k.occurrences, k.opened_at, \
     k.closed_at, k.operator_window_ends_at, k.assignee, k.decided_by, \
     p.deadline_at, p.deadline_source, p.mdm_role";

/// The role filter every read applies: mako's role on the case's process is
/// one of the caller's (`$r`), or there is none and the caller is `ADMIN`
/// (`$a`).
const ROLE_FILTER: &str = "(upper(p.mdm_role) = ANY($3) OR (p.mdm_role IS NULL AND $4))";

fn row_to_case(
    row: &sqlx::postgres::PgRow,
    entry: Option<&ApprovalQueueEntry>,
    now: OffsetDateTime,
) -> Result<Klaerfall, sqlx::Error> {
    let decode = |column: &'static str| sqlx::Error::ColumnDecode {
        index: column.into(),
        source: Box::new(std::io::Error::other("a literal no variant spells")),
    };
    let aktion: String = row.try_get("aktion")?;
    let zustand: String = row.try_get("state")?;
    let zustand = Zustand::parse(&zustand).ok_or_else(|| decode("state"))?;
    let ursache_code: String = row.try_get("ursache_code")?;
    let ursache_liste: String = row.try_get("ursache_liste")?;
    let due_at: Option<OffsetDateTime> = row.try_get("deadline_at")?;
    let origins: Vec<String> = row.try_get("origins")?;
    Ok(Klaerfall {
        id: row.try_get("id")?,
        message_ref: row.try_get("message_ref")?,
        partner_mp_id: row.try_get("partner_mp_id")?,
        process_id: row.try_get("process_id")?,
        pid: row
            .try_get::<Option<i32>, _>("pid")?
            .and_then(|p| u32::try_from(p).ok()),
        owner_role: row.try_get("mdm_role")?,
        korrektur: if zustand == Zustand::Open {
            korrektur(&origins, entry)
        } else {
            None
        },
        origins,
        zustand,
        bedeutung: mako_pruefung::codes::lies(&ursache_liste, &ursache_code).map(|c| c.bedeutung),
        ursache_code,
        ursache_liste,
        aktion: Aktion::parse(&aktion).ok_or_else(|| decode("aktion"))?,
        hinweis: row.try_get("hinweis")?,
        overdue: zustand == Zustand::Open && due_at.is_some_and(|d| d < now),
        due_at,
        due_source: row.try_get("deadline_source")?,
        operator_window_ends_at: row.try_get("operator_window_ends_at")?,
        assignee: row.try_get("assignee")?,
        decided_by: row.try_get("decided_by")?,
        occurrences: row.try_get("occurrences")?,
        opened_at: row.try_get("opened_at")?,
        closed_at: row.try_get("closed_at")?,
    })
}

/// The caller's roles, upper-cased, and whether one is `ADMIN`.
fn rollen_und_admin(rollen: &[String]) -> (Vec<String>, bool) {
    let rollen: Vec<String> = rollen.iter().map(|r| r.to_ascii_uppercase()).collect();
    let admin = rollen.iter().any(|r| r == "ADMIN");
    (rollen, admin)
}

/// The Freigabe of `process_id`, where one exists.
async fn freigabe_of(
    pool: &PgPool,
    tenant: &str,
    process_id: Option<Uuid>,
) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
    match process_id {
        Some(p) => {
            crate::pg::approval::PgApprovalQueue::new(pool.clone())
                .find_by_process(p, tenant)
                .await
        }
        None => Ok(None),
    }
}

/// The open cases of `tenant` the caller may read, the earliest due date
/// first, those without a Frist last.
///
/// `rollen` are the caller's Marktrollen: a case is listed when mako acted in
/// one of them on its process (`process_projections.mdm_role`), and a case on
/// no process — whose owning role cannot be told — only for `ADMIN`. An LF
/// caller therefore never reads a case of the NB side.
///
/// # Errors
///
/// A database failure, or a stored literal no variant spells.
pub async fn list_open(
    pool: &PgPool,
    tenant: &str,
    rollen: &[String],
    limit: i64,
    now: OffsetDateTime,
) -> Result<Klaerfaelle, sqlx::Error> {
    let (rollen, admin) = rollen_und_admin(rollen);
    let rows = sqlx::query(mako_service::sql::dynamic(format!(
        "SELECT {CASE_COLUMNS}
           FROM klaerfaelle k
           LEFT JOIN process_projections p
                  ON p.process_id = k.process_id AND p.tenant = k.tenant
          WHERE k.tenant = $1 AND k.state = 'open' AND {ROLE_FILTER}
          ORDER BY p.deadline_at ASC NULLS LAST, k.opened_at ASC, k.id
          LIMIT $2"
    )))
    .bind(tenant)
    .bind(limit)
    .bind(&rollen)
    .bind(admin)
    .fetch_all(pool)
    .await?;
    let mut klaerfaelle = Vec::with_capacity(rows.len());
    for row in rows {
        let process_id: Option<Uuid> = row.try_get("process_id")?;
        let entry = freigabe_of(pool, tenant, process_id).await?;
        klaerfaelle.push(row_to_case(&row, entry.as_ref(), now)?);
    }
    let saturated = i64::try_from(klaerfaelle.len()).unwrap_or(i64::MAX) >= limit;
    Ok(Klaerfaelle {
        count: klaerfaelle.len(),
        saturated,
        klaerfaelle,
    })
}

/// One case of `tenant` the caller may read, with its history; `None` when it
/// does not exist or belongs to a role the caller does not hold — the two
/// read the same, so a read cannot probe another role's cases.
///
/// # Errors
///
/// A database failure, or a stored literal no variant spells.
pub async fn get(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    rollen: &[String],
    now: OffsetDateTime,
) -> Result<Option<KlaerfallAkte>, sqlx::Error> {
    let (rollen, admin) = rollen_und_admin(rollen);
    let Some(row) = sqlx::query(mako_service::sql::dynamic(format!(
        "SELECT {CASE_COLUMNS}
           FROM klaerfaelle k
           LEFT JOIN process_projections p
                  ON p.process_id = k.process_id AND p.tenant = k.tenant
          WHERE k.tenant = $1 AND k.id = $2 AND {ROLE_FILTER}"
    )))
    .bind(tenant)
    .bind(id)
    .bind(&rollen)
    .bind(admin)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let process_id: Option<Uuid> = row.try_get("process_id")?;
    let entry = freigabe_of(pool, tenant, process_id).await?;
    let klaerfall = row_to_case(&row, entry.as_ref(), now)?;
    let verlauf = sqlx::query(
        "SELECT at, akteur, schritt, vorgeschlagen, befehl, begruendung, assignee
           FROM klaerfall_verlauf WHERE tenant = $1 AND klaerfall_id = $2
          ORDER BY at, id",
    )
    .bind(tenant)
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| {
        Ok(VerlaufEintrag {
            at: r.try_get("at")?,
            akteur: r.try_get("akteur")?,
            schritt: r.try_get("schritt")?,
            vorgeschlagen: r.try_get("vorgeschlagen")?,
            befehl: r.try_get("befehl")?,
            begruendung: r.try_get("begruendung")?,
            assignee: r.try_get("assignee")?,
        })
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Some(KlaerfallAkte { klaerfall, verlauf }))
}

/// The Freigabe a case's correction is decided through, read again for a
/// dispatch.
///
/// # Errors
///
/// A database failure.
pub async fn freigabe(
    pool: &PgPool,
    tenant: &str,
    process_id: Option<Uuid>,
) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
    freigabe_of(pool, tenant, process_id).await
}

// ── Working a case ───────────────────────────────────────────────────────────

/// One step's history row, written in the transaction of the step.
struct Schritt<'a> {
    akteur: &'a str,
    schritt: &'static str,
    vorgeschlagen: Option<&'a str>,
    befehl: Option<&'a str>,
    begruendung: Option<&'a str>,
    assignee: Option<&'a str>,
}

async fn journal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    id: Uuid,
    s: &Schritt<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO klaerfall_verlauf
           (klaerfall_id, tenant, at, akteur, schritt, vorgeschlagen, befehl, begruendung, assignee)
         VALUES ($1, $2, now(), $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(tenant)
    .bind(s.akteur)
    .bind(s.schritt)
    .bind(s.vorgeschlagen)
    .bind(s.befehl)
    .bind(s.begruendung)
    .bind(s.assignee)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Assign the open case `id` to `assignee` (`None` releases it), journaled
/// against `akteur`. `false` when no open case has that id.
///
/// # Errors
///
/// A database failure.
pub async fn assign(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    assignee: Option<&str>,
    akteur: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query(
        "UPDATE klaerfaelle SET assignee = $3 WHERE tenant = $1 AND id = $2 AND state = 'open'",
    )
    .bind(tenant)
    .bind(id)
    .bind(assignee)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if done {
        journal(
            &mut tx,
            tenant,
            id,
            &Schritt {
                akteur,
                schritt: "assign",
                vorgeschlagen: None,
                befehl: None,
                begruendung: None,
                assignee,
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(done)
}

/// Close the open case `id` without a market message, journaled against
/// `akteur` with the reason. `false` when no open case has that id.
///
/// # Errors
///
/// A database failure.
pub async fn resolve(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    begruendung: &str,
    akteur: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query(
        "UPDATE klaerfaelle SET state = 'resolved', closed_at = now(), decided_by = $3
          WHERE tenant = $1 AND id = $2 AND state = 'open'",
    )
    .bind(tenant)
    .bind(id)
    .bind(akteur)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if done {
        journal(
            &mut tx,
            tenant,
            id,
            &Schritt {
                akteur,
                schritt: "resolve",
                vorgeschlagen: None,
                befehl: None,
                begruendung: Some(begruendung),
                assignee: None,
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(done)
}

/// What a dispatch journals.
#[derive(Debug, Clone, Copy)]
pub struct Dispatch<'a> {
    pub akteur: &'a str,
    pub vorgeschlagen: &'a str,
    pub befehl: &'a str,
    pub begruendung: Option<&'a str>,
}

/// Claim the open case `id` as dispatched, with its journal row, before the
/// command goes out — so two dispatches of one case cannot both send.
/// `false` when no open case has that id. [`release_dispatch`] undoes it when
/// the command does not reach makod.
///
/// # Errors
///
/// A database failure.
pub async fn claim_dispatch(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    d: Dispatch<'_>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query(
        "UPDATE klaerfaelle SET state = 'dispatched', closed_at = now(), decided_by = $3
          WHERE tenant = $1 AND id = $2 AND state = 'open'",
    )
    .bind(tenant)
    .bind(id)
    .bind(d.akteur)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if done {
        journal(
            &mut tx,
            tenant,
            id,
            &Schritt {
                akteur: d.akteur,
                schritt: "dispatch",
                vorgeschlagen: Some(d.vorgeschlagen),
                befehl: Some(d.befehl),
                begruendung: d.begruendung,
                assignee: None,
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(done)
}

/// Undo [`claim_dispatch`]: the command did not reach makod, so the case is
/// open again and the dispatch is not in its history.
///
/// # Errors
///
/// A database failure.
pub async fn release_dispatch(pool: &PgPool, tenant: &str, id: Uuid) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM klaerfall_verlauf WHERE id = (
           SELECT max(id) FROM klaerfall_verlauf
            WHERE tenant = $1 AND klaerfall_id = $2 AND schritt = 'dispatch')",
    )
    .bind(tenant)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE klaerfaelle SET state = 'open', closed_at = NULL, decided_by = NULL
          WHERE tenant = $1 AND id = $2 AND state = 'dispatched'",
    )
    .bind(tenant)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The open `operator_decision` case of `process_id` — the one a decided
/// Freigabe closes.
///
/// # Errors
///
/// A database failure.
pub async fn freigabe_case(
    pool: &PgPool,
    tenant: &str,
    process_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM klaerfaelle
          WHERE tenant = $1 AND message_ref = $2 AND partner_mp_id IS NULL AND state = 'open'
            AND 'operator_decision' = ANY(origins)",
    )
    .bind(tenant)
    .bind(format!("process:{process_id}"))
    .fetch_optional(pool)
    .await
}

// ── § 7a Abs. 5 EnWG: Klärfälle per affiliate class ──────────────────────────

/// Which side of the Gleichbehandlung comparison a case falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Klasse {
    /// The initiating Lieferant belongs to the operator's undertaking.
    Affiliate,
    /// A third-party Lieferant initiated it.
    ThirdParty,
    /// No process, or a process no Lieferant initiates.
    Unattributed,
}

/// Opened, resolved and the median time to resolve of one class.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct KlassenZahlen {
    pub opened: u64,
    /// Closed — resolved or dispatched.
    pub resolved: u64,
    /// Median of `closed_at − opened_at` over the closed cases, in seconds;
    /// `None` when none closed.
    pub median_resolution_seconds: Option<i64>,
}

/// The Klärfälle of one reporting year per affiliate class.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct KlaerfallParitaet {
    pub affiliate: KlassenZahlen,
    pub third_party: KlassenZahlen,
    /// A case on no process, or on a process without a Lieferant initiator —
    /// counted, never dropped.
    pub unattributed: KlassenZahlen,
}

impl KlaerfallParitaet {
    /// Fold one case into its class: `dauer` is its time to resolve, `None`
    /// while open.
    pub fn zaehle(&mut self, klasse: Klasse, dauer: Option<time::Duration>) {
        let k = match klasse {
            Klasse::Affiliate => &mut self.affiliate,
            Klasse::ThirdParty => &mut self.third_party,
            Klasse::Unattributed => &mut self.unattributed,
        };
        k.opened += 1;
        if dauer.is_some() {
            k.resolved += 1;
        }
    }
}

/// The median of `seconds`, the lower one of an even count.
fn median(mut seconds: Vec<i64>) -> Option<i64> {
    seconds.sort_unstable();
    let n = seconds.len();
    (n > 0).then(|| seconds[(n - 1) / 2])
}

/// The class of a case: on no process, or on a process whose PID no Lieferant
/// initiates for the network arm to answer, it is unattributed.
#[must_use]
pub fn klasse(projection: Option<(u32, bool)>) -> Klasse {
    match projection {
        Some((pid, affiliate)) if crate::obs::project::is_nb_answered_lieferanten_process(pid) => {
            if affiliate {
                Klasse::Affiliate
            } else {
                Klasse::ThirdParty
            }
        }
        _ => Klasse::Unattributed,
    }
}

/// The Klärfälle of `tenant` opened in calendar `year` (Europe/Berlin), per
/// affiliate class — read in-process from the case table beside the
/// projection, keyed on the year the case opened.
///
/// # Errors
///
/// A database failure.
pub async fn paritaet(
    pool: &PgPool,
    tenant: &str,
    year: i32,
) -> Result<KlaerfallParitaet, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT k.opened_at, k.closed_at, p.pid, p.initiator_is_affiliate
           FROM klaerfaelle k
           LEFT JOIN process_projections p
                  ON p.process_id = k.process_id AND p.tenant = k.tenant
          WHERE k.tenant = $1
            AND EXTRACT(YEAR FROM k.opened_at AT TIME ZONE 'Europe/Berlin')::int = $2",
    )
    .bind(tenant)
    .bind(year)
    .fetch_all(pool)
    .await?;
    let mut out = KlaerfallParitaet::default();
    let mut dauern: [Vec<i64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for row in rows {
        let opened: OffsetDateTime = row.try_get("opened_at")?;
        let closed: Option<OffsetDateTime> = row.try_get("closed_at")?;
        let pid: Option<i32> = row.try_get("pid")?;
        let affiliate: Option<bool> = row.try_get("initiator_is_affiliate")?;
        let projection = pid
            .and_then(|p| u32::try_from(p).ok())
            .map(|p| (p, affiliate.unwrap_or(false)));
        let k = klasse(projection);
        let dauer = closed.map(|c| c - opened);
        out.zaehle(k, dauer);
        if let Some(d) = dauer {
            dauern[k as usize].push(d.whole_seconds());
        }
    }
    let [a, t, u] = dauern;
    out.affiliate.median_resolution_seconds = median(a);
    out.third_party.median_resolution_seconds = median(t);
    out.unattributed.median_resolution_seconds = median(u);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ce(ce_type: &str, data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "type": ce_type,
            "subject": "00000000-0000-4000-8000-000000000001",
            "makopid": 55_001,
            "data": data,
        })
    }

    /// The APERAK cause table is read — `recommended_action` has a
    /// caller here, and an unknown code escalates rather than guessing.
    #[test]
    fn an_aperak_rejection_takes_its_action_from_the_erc_table() {
        let s = signal_of(&ce(
            mako_events::mako::APERAK_REJECTED,
            serde_json::json!({ "error_code": "Z29", "bezug_message_ref": "UTILMD-7" }),
        ))
        .expect("a case");
        assert_eq!(s.herkunft, Herkunft::Aperak);
        assert_eq!(s.message_ref, "UTILMD-7", "keyed on the rejected message");
        assert_eq!(s.aktion, Aktion::RetryWithCorrection);
        let unknown = signal_of(&ce(
            mako_events::mako::APERAK_REJECTED,
            serde_json::json!({ "error_code": "Z99", "bezug_message_ref": "UTILMD-8" }),
        ))
        .expect("a case");
        assert_eq!(unknown.aktion, Aktion::EscalateToOperator);
    }

    /// A dead letter that names no message keys on its reason, so a flood of
    /// them from one partner shares one key.
    #[test]
    fn a_dead_letter_takes_its_action_from_the_reason_table() {
        let s = signal_of(&ce(
            mako_events::mako::DEAD_LETTER_RECORDED,
            serde_json::json!({ "reason": "unknown_pid", "dead_letter_id": "d-1" }),
        ))
        .expect("a case");
        assert_eq!(s.herkunft, Herkunft::DeadLetter);
        assert_eq!(
            s.message_ref, "dead-letter:unknown_pid",
            "no message: keyed on its reason"
        );
        assert_eq!(s.aktion, Aktion::EscalateToOperator);
        let other = signal_of(&ce(
            mako_events::mako::DEAD_LETTER_RECORDED,
            serde_json::json!({ "reason": "unknown_pid", "dead_letter_id": "d-2" }),
        ))
        .expect("a case");
        assert_eq!(other.message_ref, s.message_ref);
    }

    #[test]
    fn a_refusing_answer_and_a_lapse_open_cases_on_their_process() {
        let refusal = signal_of(&ce(
            mako_events::mako::PROCESS_COMPLETED,
            serde_json::json!({ "outcome": "rejected", "antwortcode": "A02", "codeliste": "E_0622" }),
        ))
        .expect("a case");
        assert_eq!(refusal.herkunft, Herkunft::Refusal);
        assert_eq!(
            (
                refusal.ursache_code.as_str(),
                refusal.ursache_liste.as_str()
            ),
            ("A02", "E_0622")
        );
        let lapse = signal_of(&ce(
            mako_events::mako::ANTWORTFRIST_ABGELAUFEN,
            serde_json::json!({}),
        ))
        .expect("a case");
        assert_eq!(
            lapse.message_ref, refusal.message_ref,
            "one case per process"
        );
        assert!(
            signal_of(&ce(
                mako_events::mako::PROCESS_COMPLETED,
                serde_json::json!({ "outcome": "completed" }),
            ))
            .is_none(),
            "a completed process opens nothing"
        );
    }

    /// A completion whose workflow states no `outcome` is read by its code:
    /// makod stamps the envelope's Antwortcode onto every completion, and the
    /// code's cluster in its list says whether it refuses.
    #[test]
    fn a_completion_without_an_outcome_is_a_refusal_by_its_code() {
        let ablehnung = signal_of(&ce(
            mako_events::mako::PROCESS_COMPLETED,
            serde_json::json!({ "antwortcode": "A02", "codeliste": "E_0622" }),
        ));
        assert_eq!(
            ablehnung.map(|s| s.herkunft),
            Some(Herkunft::Refusal),
            "an Ablehnungscode refuses"
        );
        let zustimmung = mako_pruefung::codes::codes_im_cluster(
            mako_pruefung::codes::zustimmung_tree_of("E_0622").unwrap_or("E_0622"),
            mako_pruefung::Cluster::Zustimmung,
        )
        .next()
        .map(|c| c.code);
        if let Some(code) = zustimmung {
            let tree = mako_pruefung::codes::zustimmung_tree_of("E_0622").unwrap_or("E_0622");
            assert!(
                signal_of(&ce(
                    mako_events::mako::PROCESS_COMPLETED,
                    serde_json::json!({ "antwortcode": code, "codeliste": tree }),
                ))
                .is_none(),
                "a Zustimmungscode opens nothing"
            );
        }
        assert!(
            signal_of(&ce(
                mako_events::mako::PROCESS_COMPLETED,
                serde_json::json!({ "antwortcode": "A02", "codeliste": "E_0622", "outcome": "accepted" }),
            ))
            .is_none(),
            "a stated outcome wins"
        );
    }

    fn entry(status: QueueStatus) -> ApprovalQueueEntry {
        let mut e = ApprovalQueueEntry::pending(
            Uuid::nil(),
            55_001,
            Some("51238696012".into()),
            "test".into(),
            OffsetDateTime::UNIX_EPOCH,
            "t".into(),
            "rule",
        )
        .with_commands(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
            Some("NB"),
        );
        e.status = status;
        if status != QueueStatus::Pending {
            e.gesendet_sha256 = Some("sent".into());
        }
        e
    }

    fn origins(h: &[Herkunft]) -> Vec<String> {
        h.iter().map(|h| h.as_str().to_owned()).collect()
    }

    /// The correcting command exists only for an answer a Freigabe sends:
    /// a pending Freigabe proposes its approve command, an APERAK-rejected
    /// decided one re-sends what it sent, and every other case escalates.
    #[test]
    fn only_an_answer_a_freigabe_sends_is_correctable() {
        use mako_markt::commands as c;
        let pending = korrektur(
            &origins(&[Herkunft::OperatorDecision]),
            Some(&entry(QueueStatus::Pending)),
        )
        .expect("a pending Freigabe proposes");
        assert_eq!(pending.befehl, c::GPKE_LIEFERBEGINN_BESTAETIGEN);
        assert_eq!(
            pending.erlaubt,
            [
                c::GPKE_LIEFERBEGINN_BESTAETIGEN,
                c::GPKE_LIEFERBEGINN_ABLEHNEN
            ]
        );
        let rejected = korrektur(
            &origins(&[Herkunft::Aperak]),
            Some(&entry(QueueStatus::Rejected)),
        )
        .expect("a refused answer is re-sent");
        assert_eq!(rejected.befehl, c::GPKE_LIEFERBEGINN_ABLEHNEN);
        assert_eq!(
            korrektur(&origins(&[Herkunft::Aperak]), None),
            None,
            "no Freigabe: escalate"
        );
        assert_eq!(
            korrektur(
                &origins(&[Herkunft::FristMissed]),
                Some(&entry(QueueStatus::Approved))
            ),
            None,
            "a lapse is no correctable answer"
        );
        assert_eq!(
            korrektur(
                &origins(&[Herkunft::Aperak]),
                Some(&entry(QueueStatus::Expired))
            ),
            None,
            "nothing was sent"
        );
    }

    /// Every answer a Freigabe sends is one the decision path can build a body
    /// for — so a proposed correction is never one the dispatch refuses as
    /// unknown.
    #[test]
    fn every_freigabe_answer_has_a_decision_contract() {
        for command in mako_markt::commands::FREIGABE_ANTWORTEN {
            assert!(
                crate::pg::approval::answer_contract(command).is_some(),
                "{command} has no answer contract"
            );
        }
    }

    /// Another command than the proposal needs a reason, and a command
    /// outside the Freigabe's pair is refused whatever the reason.
    #[test]
    fn override_dispatch_requires_a_reason() {
        use mako_markt::commands as c;
        let k = korrektur(
            &origins(&[Herkunft::OperatorDecision]),
            Some(&entry(QueueStatus::Pending)),
        );
        assert_eq!(
            waehle_befehl(k.as_ref(), None, None).as_deref(),
            Ok(c::GPKE_LIEFERBEGINN_BESTAETIGEN),
            "the proposal needs no reason"
        );
        assert_eq!(
            waehle_befehl(k.as_ref(), Some(c::GPKE_LIEFERBEGINN_ABLEHNEN), None)
                .map_err(|r| r.code()),
            Err("begruendung_required")
        );
        assert_eq!(
            waehle_befehl(k.as_ref(), Some(c::GPKE_LIEFERBEGINN_ABLEHNEN), Some("  "))
                .map_err(|r| r.code()),
            Err("begruendung_required"),
            "a blank reason is none"
        );
        assert_eq!(
            waehle_befehl(
                k.as_ref(),
                Some(c::GPKE_LIEFERBEGINN_ABLEHNEN),
                Some("Lieferbeginn liegt vor dem Vertragsbeginn")
            )
            .as_deref(),
            Ok(c::GPKE_LIEFERBEGINN_ABLEHNEN)
        );
        assert_eq!(
            waehle_befehl(k.as_ref(), Some(c::GELI_EOG_BESTAETIGEN), Some("reason"))
                .map_err(|r| r.code()),
            Err("command_not_allowed")
        );
        assert_eq!(
            waehle_befehl(None, None, None).map_err(|r| r.code()),
            Err("no_correction")
        );
    }

    /// The dispatched command's own authority is the Marktrolle the
    /// Freigabe owes the answer as.
    #[test]
    fn dispatch_override_needs_the_commands_own_action() {
        let lf = vec!["LF".to_owned()];
        let admin = vec!["ADMIN".to_owned()];
        assert!(!darf_senden(&lf, Some("NB")), "an LF sends no NB answer");
        assert!(darf_senden(&["nb".to_owned()], Some("NB")));
        assert!(!darf_senden(&admin, None), "ADMIN is no market role");
        assert!(darf_senden(&lf, None));
    }

    /// The SQL `CHECK`s list exactly the origins, actions and states the code
    /// writes.
    #[test]
    fn the_schema_checks_match_the_enums() {
        let schema = include_str!("../migrations/0001_initial.sql");
        let origins: Vec<String> = Herkunft::ALL
            .iter()
            .map(|h| format!("'{}'", h.as_str()))
            .collect();
        assert!(schema.contains(&origins.join(",")), "origins CHECK");
        let aktionen: Vec<String> = Aktion::ALL
            .iter()
            .map(|a| format!("'{}'", a.as_str()))
            .collect();
        assert!(schema.contains(&aktionen.join(",")), "aktion CHECK");
        let zustaende: Vec<String> = Zustand::ALL
            .iter()
            .map(|z| format!("'{}'", z.as_str()))
            .collect();
        assert!(schema.contains(&zustaende.join(",")), "state CHECK");
    }

    /// The tabulated fields — cause, action, hint — have one
    /// writer, the upsert fed by [`signal_of`] / [`signal_of_freigabe`]; the
    /// correcting command is computed at read by [`korrektur`] and stored
    /// nowhere; `due_at` is the read model's. No other statement in processd
    /// names them in a write.
    #[test]
    fn the_tabulated_fields_have_one_writer() {
        let src = env!("CARGO_MANIFEST_DIR");
        let mut writers = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(src).join("src")];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).expect("src").flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read");
                let text = text.split("#[cfg(test)]").next().unwrap_or_default();
                for (n, stmt) in text.split("sqlx::query").enumerate().skip(1) {
                    let stmt: String = stmt.chars().take(900).collect();
                    let writes_case = (stmt.contains("INSERT INTO klaerfaelle")
                        || stmt.contains("UPDATE klaerfaelle"))
                        && [
                            "ursache_code",
                            "ursache_liste",
                            "aktion",
                            "hinweis",
                            "befehl",
                        ]
                        .iter()
                        .any(|f| stmt.split("WHERE").next().unwrap_or_default().contains(f));
                    if writes_case {
                        writers.push(format!("{}#{n}", path.display()));
                    }
                }
            }
        }
        assert_eq!(writers.len(), 1, "{writers:#?}");
        assert!(writers[0].contains("klaerfall.rs"), "{writers:#?}");
    }

    /// Every case falls in exactly one class, so the classes sum to the
    /// cases opened; a case without an LF initiator is unattributed, never
    /// dropped.
    #[test]
    fn the_three_classes_sum_to_the_cases_opened() {
        let mut p = KlaerfallParitaet::default();
        let hour = Some(time::Duration::hours(1));
        p.zaehle(klasse(Some((55_001, true))), hour);
        p.zaehle(klasse(Some((55_001, false))), None);
        p.zaehle(klasse(Some((55_016, false))), hour);
        p.zaehle(klasse(None), None);
        assert_eq!(p.affiliate.opened, 1);
        assert_eq!(p.third_party.opened, 1);
        assert_eq!(
            p.unattributed.opened, 2,
            "a Kündigung has no LF initiator the NB answers; no process is none"
        );
        assert_eq!(
            p.affiliate.opened + p.third_party.opened + p.unattributed.opened,
            4
        );
        assert_eq!(p.affiliate.resolved + p.unattributed.resolved, 2);
        assert_eq!(median(vec![30, 10, 20]), Some(20));
        assert_eq!(median(vec![]), None);
    }
}
