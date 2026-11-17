//! Klärfälle: one case per message mako could not bring to its end, with a
//! cause, an action and a due date taken from tables and code — never from a
//! model.
//!
//! Four signals open or attach to a case, each a CloudEvent processd already
//! receives:
//!
//! | Origin | CloudEvent | Cause | Key |
//! |---|---|---|---|
//! | `aperak` | `de.mako.aperak.rejected` | the ERC code, [`recommended_action`] | the rejected message (`bezug_message_ref`) |
//! | `dead_letter` | `de.mako.dead-letter.recorded` | the reason, [`action_for_label`] | the message, else the dead letter |
//! | `refusal` | `de.mako.process.completed` with an Antwortcode and `outcome = rejected` | the code in its tree, [`mako_pruefung::codes::lies`] | the process |
//! | `frist_missed` | `de.mako.antwortfrist.abgelaufen` | the lapse | the process |
//!
//! The key is the message *and* the partner that sent the signal: a
//! `message_ref` is unique only per sender, so an APERAK from partner Q that
//! cites the reference of a message sent to P opens Q's own case and never
//! attaches to or closes P's. A case is read only by a caller holding
//! the Marktrolle mako acted in on its process (`process_projections.mdm_role`);
//! a case on no process — a dead letter that names none — only by `ADMIN`.
//! The § 6a Abs. 1 EnWG confidentiality duty is an access-control duty, and
//! this filter is where a case read meets it.
//!
//! A further signal for the same key, from any origin, attaches: the count
//! rises, the origin joins `origins`, and the first cause stands. The due date
//! is the process's business Antwortfrist from the read model
//! (`process_projections.deadline_at`, computed by `mako_fristen::antwort`) or
//! null — never a default. A counterparty's free text never reaches a case:
//! only codes are stored, and the wording is the BDEW's own.
//!
//! [`recommended_action`]: mako_engine::erc::recommended_action
//! [`action_for_label`]: mako_engine::dead_letter::action_for_label

use serde::Serialize;
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;
use uuid::Uuid;

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
}

impl Herkunft {
    /// Every origin, in the order of the SQL `CHECK`.
    pub const ALL: [Self; 4] = [
        Self::Aperak,
        Self::DeadLetter,
        Self::Refusal,
        Self::FristMissed,
    ];

    /// The wire and SQL literal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aperak => "aperak",
            Self::DeadLetter => "dead_letter",
            Self::Refusal => "refusal",
            Self::FristMissed => "frist_missed",
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
        [
            Self::RetryWithCorrection,
            Self::EscalateToOperator,
            Self::AbortProcess,
            Self::WaitAndRetry,
        ]
        .into_iter()
        .find(|a| a.as_str() == s)
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

/// One signal, read from a CloudEvent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub herkunft: Herkunft,
    /// The case key: the message the signal concerns, or `process:<id>`.
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
            let message_ref = data["message_ref"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    data["dead_letter_id"]
                        .as_str()
                        .map(|id| format!("dead-letter:{id}"))
                })?;
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
            })
        }
        mako_events::mako::PROCESS_COMPLETED => {
            if data["outcome"].as_str() != Some("rejected") {
                return None;
            }
            let code = data["antwortcode"].as_str().filter(|c| !c.is_empty())?;
            let liste = data["codeliste"].as_str().filter(|l| !l.is_empty())?;
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
        }),
        _ => None,
    }
}

/// Open a case for `signal`, or attach it to the open one of its key — the
/// message and the partner that sent the signal, so a signal attaches only to
/// a case of its own sender.
///
/// One statement: the insert and the attach are the same upsert, so two
/// deliveries of one signal cannot open two cases.
///
/// # Errors
///
/// A database failure; the caller must not acknowledge the event.
pub async fn open_or_attach(
    pool: &PgPool,
    tenant: &str,
    signal: &Signal,
    at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO klaerfaelle
           (id, tenant, message_ref, process_id, pid, origins, ursache_code, ursache_liste,
            aktion, hinweis, opened_at, last_signal_at, partner_mp_id)
         VALUES ($1, $2, $3, $4, $5, ARRAY[$6]::text[], $7, $8, $9, $10, $11, $11, $12)
         ON CONFLICT (tenant, partner_mp_id, message_ref) DO UPDATE SET
           occurrences    = klaerfaelle.occurrences + 1,
           origins        = CASE WHEN $6 = ANY(klaerfaelle.origins) THEN klaerfaelle.origins
                                 ELSE array_append(klaerfaelle.origins, $6) END,
           process_id     = COALESCE(klaerfaelle.process_id, EXCLUDED.process_id),
           pid            = COALESCE(klaerfaelle.pid, EXCLUDED.pid),
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
    .execute(pool)
    .await?;
    Ok(())
}

/// One case as the Desk's Heute and an integrator read it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Klaerfall {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The message the case concerns, or `process:<id>`.
    pub message_ref: String,
    /// The MP-ID whose signal opened the case; `None` for a case on a process.
    pub partner_mp_id: Option<String>,
    #[schemars(with = "Option<String>")]
    pub process_id: Option<Uuid>,
    pub pid: Option<u32>,
    /// The origins of every signal attached, the first one first.
    pub origins: Vec<String>,
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
    /// The `mako_markt::commands` command that corrects the case, where one is
    /// a function of the cause. No ERC code, dead-letter reason or refusal
    /// names one today: the correction re-issues the process's own command, so
    /// this is `None` until a table supplies it.
    pub befehl: Option<&'static str>,
    /// The business Antwortfrist the case races (`mako_fristen::antwort`, via
    /// the read model), RFC 3339; `None` is „keine Frist", never a default.
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub due_at: Option<OffsetDateTime>,
    /// The Fundstelle of `due_at`.
    pub due_source: Option<String>,
    /// `due_at` has passed.
    pub overdue: bool,
    pub occurrences: i32,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub opened_at: OffsetDateTime,
}

/// What `list_klaerfaelle` returns.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Klaerfaelle {
    pub count: usize,
    /// At least `limit` cases matched.
    pub saturated: bool,
    pub klaerfaelle: Vec<Klaerfall>,
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
/// A database failure, or a stored action no variant spells.
pub async fn list_open(
    pool: &PgPool,
    tenant: &str,
    rollen: &[String],
    limit: i64,
    now: OffsetDateTime,
) -> Result<Klaerfaelle, sqlx::Error> {
    let rollen: Vec<String> = rollen.iter().map(|r| r.to_ascii_uppercase()).collect();
    let admin = rollen.iter().any(|r| r == "ADMIN");
    let rows = sqlx::query(
        "SELECT k.id, k.message_ref, k.partner_mp_id, k.process_id, k.pid, k.origins, k.ursache_code,
                k.ursache_liste, k.aktion, k.hinweis, k.occurrences, k.opened_at,
                p.deadline_at, p.deadline_source
           FROM klaerfaelle k
           LEFT JOIN process_projections p
                  ON p.process_id = k.process_id AND p.tenant = k.tenant
          WHERE k.tenant = $1 AND k.state = 'open'
            AND (upper(p.mdm_role) = ANY($3) OR (p.mdm_role IS NULL AND $4))
          ORDER BY p.deadline_at ASC NULLS LAST, k.opened_at ASC, k.id
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .bind(&rollen)
    .bind(admin)
    .fetch_all(pool)
    .await?;
    let mut klaerfaelle = Vec::with_capacity(rows.len());
    for row in rows {
        let aktion: String = row.try_get("aktion")?;
        let ursache_code: String = row.try_get("ursache_code")?;
        let ursache_liste: String = row.try_get("ursache_liste")?;
        let due_at: Option<OffsetDateTime> = row.try_get("deadline_at")?;
        klaerfaelle.push(Klaerfall {
            id: row.try_get("id")?,
            message_ref: row.try_get("message_ref")?,
            partner_mp_id: row.try_get("partner_mp_id")?,
            process_id: row.try_get("process_id")?,
            pid: row
                .try_get::<Option<i32>, _>("pid")?
                .and_then(|p| u32::try_from(p).ok()),
            origins: row.try_get("origins")?,
            bedeutung: mako_pruefung::codes::lies(&ursache_liste, &ursache_code)
                .map(|c| c.bedeutung),
            ursache_code,
            ursache_liste,
            aktion: Aktion::parse(&aktion).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "aktion".into(),
                source: Box::new(std::io::Error::other("an action no variant spells")),
            })?,
            hinweis: row.try_get("hinweis")?,
            befehl: None,
            overdue: due_at.is_some_and(|d| d < now),
            due_at,
            due_source: row.try_get("deadline_source")?,
            occurrences: row.try_get("occurrences")?,
            opened_at: row.try_get("opened_at")?,
        });
    }
    let saturated = i64::try_from(klaerfaelle.len()).unwrap_or(i64::MAX) >= limit;
    Ok(Klaerfaelle {
        count: klaerfaelle.len(),
        saturated,
        klaerfaelle,
    })
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

    #[test]
    fn a_dead_letter_takes_its_action_from_the_reason_table() {
        let s = signal_of(&ce(
            mako_events::mako::DEAD_LETTER_RECORDED,
            serde_json::json!({ "reason": "unknown_pid", "dead_letter_id": "d-1" }),
        ))
        .expect("a case");
        assert_eq!(s.herkunft, Herkunft::DeadLetter);
        assert_eq!(
            s.message_ref, "dead-letter:d-1",
            "no message: keyed on itself"
        );
        assert_eq!(s.aktion, Aktion::EscalateToOperator);
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

    /// The SQL `CHECK` lists exactly the origins and actions the code writes.
    #[test]
    fn the_schema_checks_match_the_enums() {
        let schema = include_str!("../migrations/0001_initial.sql");
        let origins: Vec<String> = Herkunft::ALL
            .iter()
            .map(|h| format!("'{}'", h.as_str()))
            .collect();
        assert!(schema.contains(&origins.join(",")), "origins CHECK");
        for a in [
            Aktion::RetryWithCorrection,
            Aktion::EscalateToOperator,
            Aktion::AbortProcess,
            Aktion::WaitAndRetry,
        ] {
            assert!(schema.contains(&format!("'{}'", a.as_str())), "{a:?}");
        }
    }
}
