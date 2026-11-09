//! Durable outbound tasks — everything `vertragd` owes another service.
//!
//! A Lieferbeginn is an obligation: the customer has a contract and the NB is
//! waiting for the UTILMD. Dispatching it from a detached `tokio::spawn` meant a
//! restart between the contract insert and the `processd` call dropped the
//! registration in silence, leaving the component in `ANGELEGT` with nothing to
//! retry it — and the same held for the Schlussablesung, the tariff assignment
//! and the billing account.
//!
//! So the *intent* is written in the same transaction as the contract change
//! ([`enqueue`]), and [`OutboundWorker`] performs it afterwards with exponential
//! backoff and a dead-letter. A crash at any point loses nothing; it only
//! delays.
//!
//! ```text
//! handler ─┬─ contract write ──┐
//!          └─ enqueue task  ───┴─ COMMIT ─→ worker ─→ processd / edmd / productd / accountingd
//! ```

use std::sync::Arc;

use anyhow::{Context as _, Result};
use mako_service::http::Upstream;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::Date;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use mako_markt::utilmd_stammdaten::{Kunde, NetznutzungZahler, Netznutzungsvertrag};

use crate::config::VertragdConfig;
use crate::domain::Lieferbeginnanlass;

/// How many times a task is attempted before it is dead-lettered.
const MAX_ATTEMPTS: i32 = 8;
/// Base of the exponential backoff, in seconds: 30s, 1m, 2m, … capped at 1h.
const BACKOFF_BASE_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 3600;

/// What the worker has to do. The variant decides the target service, the HTTP
/// shape and the follow-up write, so callers name the obligation rather than
/// assembling a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskKind {
    /// `processd` UTILMD Anmeldung (GPKE 55001 / GeLi Gas 44001).
    Lieferbeginn,
    /// `processd` UTILMD Abmeldung.
    Lieferende,
    /// `edmd` GPKE Beginnablesung.
    AblesungBeginn,
    /// `edmd` GPKE Schlussablesung — the basis of the Schlussrechnung.
    AblesungEnde,
    /// `accountingd` billing account for a contract that went AKTIV.
    Abrechnungskonto,
}

impl TaskKind {
    /// Every variant, in the order the schema lists them.
    ///
    /// `kind` is a `CHECK`-constrained column and this enum is what writes it;
    /// `schema_enum_guard` holds the two together.
    pub const ALL: &'static [Self] = &[
        Self::Lieferbeginn,
        Self::Lieferende,
        Self::AblesungBeginn,
        Self::AblesungEnde,
        Self::Abrechnungskonto,
    ];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Lieferbeginn => "LIEFERBEGINN",
            Self::Lieferende => "LIEFERENDE",
            Self::AblesungBeginn => "ABLESUNG_BEGINN",
            Self::AblesungEnde => "ABLESUNG_ENDE",
            Self::Abrechnungskonto => "ABRECHNUNGSKONTO",
        }
    }

    fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "LIEFERBEGINN" => Self::Lieferbeginn,
            "LIEFERENDE" => Self::Lieferende,
            "ABLESUNG_BEGINN" => Self::AblesungBeginn,
            "ABLESUNG_ENDE" => Self::AblesungEnde,
            "ABRECHNUNGSKONTO" => Self::Abrechnungskonto,
            _ => None?,
        })
    }
}

/// A task ready to be enqueued, with the deduplication key that makes the
/// enqueue exactly-once.
#[derive(Debug, Clone)]
pub struct Task {
    pub kind: TaskKind,
    pub komp_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub dedupe_key: String,
}

// ── Task constructors ─────────────────────────────────────────────────────────

/// The `processd` Lieferbeginn task for one component.
///
/// # Errors
///
/// Refuses a gas component without its [`GasAnmeldung`]: `start-supply-gas`
/// carries the Lieferant's contract facts the Anmeldung NN 44001 makes Muss,
/// and a task without them is a 422 on every attempt.
pub fn lieferbeginn(
    komp_id: Uuid,
    sparte: &str,
    malo_id: &str,
    bilanzkreis: Option<&str>,
    lieferbeginn: Date,
    gas: Option<&GasAnmeldung>,
) -> Result<Task> {
    let payload = lieferbeginn_body(sparte, malo_id, bilanzkreis, lieferbeginn, gas)?;
    Ok(Task {
        kind: TaskKind::Lieferbeginn,
        komp_id: Some(komp_id),
        payload,
        dedupe_key: format!("LIEFERBEGINN:{komp_id}"),
    })
}

/// A `processd` call as a task stores it: the endpoint, and the `body` the
/// worker posts — exactly the fields that endpoint's request type declares,
/// because processd reads it with [`mako_service::Json`] and refuses any other.
fn processd_call(endpoint: &str, body: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "endpoint": endpoint, "body": body })
}

/// `YYYYMMDD`, the date `start-supply-gas` and `end-supply-gas` read
/// (gesetzliche deutsche Zeit).
fn compact_date(d: Date) -> String {
    format!("{:04}{:02}{:02}", d.year(), u8::from(d.month()), d.day())
}

/// The Lieferant's contract facts a GeLi Gas Anmeldung NN 44001 carries
/// beside the Marktlokation, the date and the Bilanzkreis (UTILMD AHB Gas 1.2
/// Kap. 5.6) — each one the contract's, each Muss in the column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GasAnmeldung {
    /// `SG4 STS+7` — from the component's [`Lieferbeginnanlass`].
    pub transaktionsgrund: &'static str,
    /// `SG6 DTM+Z20` — the contract's `abrechnungszyklus`.
    pub abrechnungsintervall_monate: u8,
    /// `SG10 CCI+++Z15`/`Z18` — the Kunde's `haushaltskunde`.
    pub haushaltskunde: bool,
    /// `SG10 CAV+Z74`.
    pub netznutzungsvertrag: Netznutzungsvertrag,
    /// `SG10 CAV+Z73`.
    pub netznutzung_zahler: NetznutzungZahler,
    /// `SG9 QTY+Z16` in Prozent.
    pub gemeinderabatt_prozent: rust_decimal::Decimal,
    /// `SG12 NAD+Z09`/`Z04` — the contract's Kunde.
    pub kunde: Kunde,
}

impl GasAnmeldung {
    /// Assemble the facts, or name every one the contract does not state.
    ///
    /// The Ablesekarte goes to the contract's Kunde: the contract names no
    /// other addressee, and he is the Anschlussnutzer whose meter it reads.
    ///
    /// # Errors
    ///
    /// Each missing or unusable fact, named with the segment it fills.
    pub fn aus_vertrag(
        anlass: Option<Lieferbeginnanlass>,
        abrechnungszyklus: &str,
        haushaltskunde: bool,
        netznutzungsvertrag: Option<Netznutzungsvertrag>,
        netznutzung_zahler: Option<NetznutzungZahler>,
        gemeinderabatt_prozent: Option<rust_decimal::Decimal>,
        kunde: Kunde,
    ) -> std::result::Result<Self, Vec<String>> {
        let mut fehlend = Vec::new();
        if anlass.is_none() {
            fehlend.push("lieferbeginn_anlass (SG4 STS+7 Transaktionsgrund)".to_owned());
        }
        let intervall = crate::domain::abrechnungsintervall_monate(abrechnungszyklus);
        if intervall.is_none() {
            fehlend.push(format!(
                "abrechnungszyklus {abrechnungszyklus:?} (SG6 DTM+Z20 Abrechnungsintervall des LF)"
            ));
        }
        if netznutzungsvertrag.is_none() {
            fehlend.push("netznutzungsvertrag (SG10 CAV+Z74)".to_owned());
        }
        if netznutzung_zahler.is_none() {
            fehlend.push("netznutzung_zahler (SG10 CAV+Z73)".to_owned());
        }
        if gemeinderabatt_prozent.is_none() {
            fehlend.push("gemeinderabatt_prozent (SG8 SEQ+Z12 QTY+Z16)".to_owned());
        }
        fehlend.extend(
            kunde
                .anmeldung_maengel()
                .into_iter()
                .map(|m| format!("the Kunde (SG12 NAD+Z09/Z04/Z05): {m}")),
        );
        match (
            anlass,
            intervall,
            netznutzungsvertrag,
            netznutzung_zahler,
            gemeinderabatt_prozent,
        ) {
            (Some(anlass), Some(intervall), Some(vertrag), Some(zahler), Some(rabatt))
                if fehlend.is_empty() =>
            {
                Ok(Self {
                    transaktionsgrund: anlass.transaktionsgrund(),
                    abrechnungsintervall_monate: intervall,
                    haushaltskunde,
                    netznutzungsvertrag: vertrag,
                    netznutzung_zahler: zahler,
                    gemeinderabatt_prozent: rabatt,
                    kunde,
                })
            }
            _ => Err(fehlend),
        }
    }
}

/// Build the `processd` Lieferbeginn call for the commodity.
///
/// The body is `processd`'s request type, field for field — the contract test
/// reads it through `processd::server::StartSupplyRequest` and
/// `processd::server::StartSupplyGasRequest` as the handler does:
/// - `start-supply` (Strom et al.) takes `lieferbeginn_datum` (ISO-8601);
/// - `start-supply-gas` takes `process_date` (YYYYMMDD) and the
///   [`GasAnmeldung`].
///
/// `bilanzkreis` is the contract's own; without one processd uses the single
/// `standard` entry of its `[[lf.bilanzkreise]]` and refuses the Anmeldung
/// (`MISSING_BILANZKREIS`) when there is none.
///
/// # Errors
///
/// A gas supply point without its [`GasAnmeldung`].
pub fn lieferbeginn_body(
    sparte: &str,
    malo_id: &str,
    bilanzkreis: Option<&str>,
    lieferbeginn: Date,
    gas: Option<&GasAnmeldung>,
) -> Result<serde_json::Value> {
    let mut body = if sparte == "GAS" {
        let gas = gas.ok_or_else(|| {
            anyhow::anyhow!(
                "gas supply point {malo_id}: start-supply-gas carries the contract facts of \
                 the Anmeldung NN 44001, and none were assembled"
            )
        })?;
        serde_json::json!({
            "malo_id": malo_id,
            "process_date": compact_date(lieferbeginn),
            "transaktionsgrund": gas.transaktionsgrund,
            "abrechnungsintervall_monate": gas.abrechnungsintervall_monate,
            "haushaltskunde": gas.haushaltskunde,
            "netznutzungsvertrag": gas.netznutzungsvertrag,
            "netznutzung_zahler": gas.netznutzung_zahler,
            "gemeinderabatt_prozent": gas.gemeinderabatt_prozent,
            "kunde": gas.kunde,
            "ablesekarte": gas.kunde,
        })
    } else {
        serde_json::json!({
            "malo_id": malo_id,
            "lieferbeginn_datum": lieferbeginn.to_string(),
        })
    };
    if let Some(bk) = bilanzkreis.filter(|b| !b.is_empty()) {
        body["bilanzkreis"] = bk.into();
    }
    let endpoint = if sparte == "GAS" {
        "start-supply-gas"
    } else {
        "start-supply"
    };
    Ok(processd_call(endpoint, body))
}

/// The `processd` Lieferende task for one component.
///
/// **The dedupe key carries the Lieferende**, as the reading orders' keys carry
/// `geplant_am`: a Kündigung withdrawn and re-issued to a new date is a new
/// obligation, and a key without the date makes `ON CONFLICT DO NOTHING` drop
/// it while the NB hears the withdrawn date. A replay of the same date still
/// collapses into one task. Enqueue it with [`enqueue_superseding`].
///
/// The body is `processd`'s request type, as for [`lieferbeginn_body`]:
/// `end-supply` takes `lieferende_datum` (ISO-8601), `end-supply-gas` a
/// `YYYYMMDD` `process_date`.
#[must_use]
pub fn lieferende(komp_id: Uuid, sparte: &str, malo_id: &str, lieferende: Date) -> Task {
    let payload = if sparte == "GAS" {
        processd_call(
            "end-supply-gas",
            serde_json::json!({
                "malo_id": malo_id,
                "process_date": compact_date(lieferende),
            }),
        )
    } else {
        processd_call(
            "end-supply",
            serde_json::json!({
                "malo_id": malo_id,
                "lieferende_datum": lieferende.to_string(),
            }),
        )
    };
    Task {
        kind: TaskKind::Lieferende,
        komp_id: Some(komp_id),
        payload,
        dedupe_key: format!("LIEFERENDE:{komp_id}:{lieferende}"),
    }
}

/// An `edmd` reading order (GPKE Beginn-/Schlussablesung).
///
/// The Schlussablesung is the LF's own obligation and does not depend on the
/// Lieferende UTILMD reaching the NB, so it is a task of its own rather than a
/// step of one — a `processd` outage must not suppress the reading the
/// Schlussrechnung is built from.
///
/// **The dedupe key carries `geplant_am`**, as the schema comment on
/// `outbound_tasks.dedupe_key` has always said it should: a reading order is
/// keyed by what makes it distinct, and the date it is planned for is that.
/// Keyed on the component alone, a Kündigung withdrawn and re-issued to a
/// different Lieferende enqueued a task whose key already existed, so
/// `ON CONFLICT DO NOTHING` dropped it: `edmd` kept the reading order for the
/// withdrawn date, the Schlussablesung happened on the wrong day, and nothing
/// anywhere reported a failure.
///
/// A *same-date* re-enqueue still collapses, which is correct — that is a
/// replay of one order, not a second one.
#[must_use]
pub fn ablesung(komp_id: Uuid, malo_id: &str, ende: bool, geplant_am: Date) -> Task {
    let (kind, anlass) = if ende {
        (TaskKind::AblesungEnde, "LIEFERENDE")
    } else {
        (TaskKind::AblesungBeginn, "LIEFERBEGINN")
    };
    Task {
        kind,
        komp_id: Some(komp_id),
        payload: serde_json::json!({
            "malo_id": malo_id,
            "anlass": anlass,
            "auftraggeber_rolle": "LF",
            "geplant_am": geplant_am.to_string(),
            "auftrag_position_id": komp_id,
        }),
        dedupe_key: format!("{}:{komp_id}:{geplant_am}", kind.as_db()),
    }
}

/// An `accountingd` billing account for a contract that reached AKTIV.
#[must_use]
pub fn abrechnungskonto(komp_id: Uuid, malo_id: &str, lf_mp_id: &str) -> Task {
    Task {
        kind: TaskKind::Abrechnungskonto,
        komp_id: Some(komp_id),
        payload: serde_json::json!({ "malo_id": malo_id, "lf_mp_id": lf_mp_id }),
        dedupe_key: format!("ABRECHNUNGSKONTO:{malo_id}"),
    }
}

// ── Enqueue ───────────────────────────────────────────────────────────────────

/// Persist a task in the caller's transaction. `false` when an identical task
/// was already enqueued — the exactly-once guarantee, in the database rather
/// than in a caller's memory.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn enqueue(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    task: &Task,
) -> Result<bool> {
    enqueue_inner(executor, tenant, task).await
}

/// Enqueue a task that **supersedes** any not-yet-performed task of the same
/// kind for the same component.
///
/// For a date-keyed task (the reading orders), varying the key is only half the
/// fix: a Kündigung re-issued to a new Lieferende would otherwise leave the
/// order for the withdrawn date sitting in the queue, and `edmd` would receive
/// both. A task that has not been dispatched has been seen by nobody, so the
/// obsolete one is deleted rather than delivered and then countermanded.
///
/// A task already `completed_at` (the order is with `edmd`) or `dead_lettered_at`
/// (the operator's queue) is left alone: those are records of something that
/// happened, and cancelling *those* is `edmd`'s business, not a DELETE here.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn enqueue_superseding(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    task: &Task,
) -> Result<bool> {
    let Some(komp_id) = task.komp_id else {
        return enqueue_inner(&mut *conn, tenant, task).await;
    };
    sqlx::query(
        "DELETE FROM outbound_tasks
          WHERE tenant = $1 AND kind = $2 AND komp_id = $3 AND dedupe_key <> $4
            AND completed_at IS NULL AND dead_lettered_at IS NULL",
    )
    .bind(tenant)
    .bind(task.kind.as_db())
    .bind(komp_id)
    .bind(&task.dedupe_key)
    .execute(&mut *conn)
    .await?;
    enqueue_inner(&mut *conn, tenant, task).await
}

async fn enqueue_inner(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    task: &Task,
) -> Result<bool> {
    let n = sqlx::query(
        "INSERT INTO outbound_tasks (tenant, kind, komp_id, payload, dedupe_key)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (tenant, dedupe_key) DO NOTHING",
    )
    .bind(tenant)
    .bind(task.kind.as_db())
    .bind(task.komp_id)
    .bind(&task.payload)
    .bind(&task.dedupe_key)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// Tasks that exhausted their retries — the operator's work queue.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_dead_lettered(pool: &PgPool, tenant: &str, limit: i64) -> Result<Vec<DeadTask>> {
    Ok(sqlx::query_as::<_, DeadTask>(
        "SELECT id, kind, komp_id, payload, attempts, last_error, dead_lettered_at
           FROM outbound_tasks
          WHERE tenant = $1 AND dead_lettered_at IS NOT NULL
          ORDER BY dead_lettered_at DESC
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// A task the worker gave up on.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DeadTask {
    pub id: Uuid,
    pub kind: String,
    pub komp_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub dead_lettered_at: Option<time::OffsetDateTime>,
}

/// Put a dead-lettered task back in the queue after the operator fixed the
/// cause. `false` when no such dead task exists.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn retry_dead_lettered(pool: &PgPool, tenant: &str, id: Uuid) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE outbound_tasks
            SET dead_lettered_at = NULL, attempts = 0,
                next_attempt_at = now(), last_error = NULL
          WHERE id = $1 AND tenant = $2 AND dead_lettered_at IS NOT NULL",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

// ── Worker ────────────────────────────────────────────────────────────────────

/// Drains [`outbound_tasks`](self) — one at a time, oldest due first.
pub struct OutboundWorker {
    pool: PgPool,
    cfg: Arc<VertragdConfig>,
    http: reqwest::Client,
}

impl OutboundWorker {
    #[must_use]
    pub fn new(pool: PgPool, cfg: Arc<VertragdConfig>, http: reqwest::Client) -> Self {
        Self { pool, cfg, http }
    }

    /// Run until cancelled.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut idle = tokio::time::interval(std::time::Duration::from_secs(5));
        idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("vertragd: outbound worker stopping");
                    return;
                }
                _ = idle.tick() => {}
            }
            // Drain the backlog in one wake-up rather than one task per tick;
            // a burst of contract creations otherwise took minutes to register.
            for _ in 0..64 {
                match self.step().await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        tracing::error!(error = %e, "vertragd: outbound worker step failed");
                        break;
                    }
                }
            }
        }
    }

    /// Claim and perform one task. `false` when the queue is empty.
    ///
    /// # Errors
    ///
    /// Storage errors while claiming or recording the task.
    pub async fn step(&self) -> Result<bool> {
        // Claim under FOR UPDATE SKIP LOCKED so several replicas can drain the
        // same queue without performing a task twice.
        let mut tx = self.pool.begin().await?;
        let Some(row) = sqlx::query(
            "SELECT id, tenant, kind, komp_id, payload, attempts
               FROM outbound_tasks
              WHERE completed_at IS NULL AND dead_lettered_at IS NULL
                AND next_attempt_at <= now()
              ORDER BY next_attempt_at
              LIMIT 1
              FOR UPDATE SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            return Ok(false);
        };

        let id: Uuid = row.try_get("id")?;
        let kind_db: String = row.try_get("kind")?;
        let komp_id: Option<Uuid> = row.try_get("komp_id")?;
        let payload: serde_json::Value = row.try_get("payload")?;
        let attempts: i32 = row.try_get("attempts")?;
        let Some(kind) = TaskKind::from_db(&kind_db) else {
            // An unknown kind can only come from a downgrade; parking it is
            // better than retrying it for ever.
            sqlx::query(
                "UPDATE outbound_tasks
                    SET dead_lettered_at = now(), last_error = 'unknown task kind'
                  WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(true);
        };

        match self.perform(kind, komp_id, &payload).await {
            Ok(follow_up) => {
                // The call reached its peer: the task is complete whatever the
                // local follow-up does next. Completing it only after the
                // follow-up would let a failing write retry a delivered call —
                // a second UTILMD 55001 at the NB, a second reading order at
                // edmd. The follow-up runs in a savepoint of this transaction,
                // and a failure there is recorded on the completed task.
                sqlx::query(
                    "UPDATE outbound_tasks
                        SET completed_at = now(), attempts = attempts + 1, last_error = NULL
                      WHERE id = $1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
                let mut savepoint = sqlx::Acquire::begin(&mut *tx).await?;
                match follow_up.apply(&mut savepoint).await {
                    Ok(()) => savepoint.commit().await?,
                    Err(e) => {
                        savepoint.rollback().await?;
                        let detail = format!("delivered; follow-up failed: {e:#}");
                        tracing::error!(
                            task = %kind_db, %id, ?komp_id, error = %detail,
                            "vertragd: outbound call delivered but its follow-up write failed — \
                             not retried, recorded on the task"
                        );
                        sqlx::query("UPDATE outbound_tasks SET last_error = $2 WHERE id = $1")
                            .bind(id)
                            .bind(&detail)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
            }
            Err(e) => {
                // A refusal is the same refusal on every retry, so it stops the
                // task now and leaves the callee's answer on it for the operator.
                let attempts = if is_refusal(&e) {
                    MAX_ATTEMPTS
                } else {
                    attempts + 1
                };
                let detail = format!("{e:#}");
                if attempts >= MAX_ATTEMPTS {
                    tracing::error!(
                        task = %kind_db, %id, ?komp_id, attempts, error = %detail,
                        "vertragd: outbound task dead-lettered — the obligation is now the operator's"
                    );
                } else {
                    tracing::warn!(
                        task = %kind_db, %id, attempts, retry_in_s = backoff_secs(attempts),
                        error = %detail, "vertragd: outbound task failed — retrying"
                    );
                }
                record_failure(&mut tx, id, attempts, &detail).await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }
}

/// Book a failed attempt: schedule the next one, or give up.
///
/// Separate from the worker so the SQL — an interval computed from a bind
/// parameter, and the boundary at which a task stops being retried — is
/// exercised by a test rather than only in production.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn record_failure(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    attempts: i32,
    detail: &str,
) -> Result<()> {
    if attempts >= MAX_ATTEMPTS {
        sqlx::query(
            "UPDATE outbound_tasks
                SET dead_lettered_at = now(), attempts = $2, last_error = $3
              WHERE id = $1",
        )
        .bind(id)
        .bind(attempts)
        .bind(detail)
        .execute(&mut *conn)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE outbound_tasks
            SET attempts = $2, last_error = $3,
                next_attempt_at = now() + make_interval(secs => $4)
          WHERE id = $1",
    )
    .bind(id)
    .bind(attempts)
    .bind(detail)
    .bind(f64::from(
        u32::try_from(backoff_secs(attempts)).unwrap_or(u32::MAX),
    ))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The local write that records a delivered call.
///
/// Returned by [`OutboundWorker::perform`] rather than written there, so it
/// runs after the task is marked complete: a failure here must not make the
/// worker repeat a call the peer already received.
enum FollowUp {
    /// Nothing to record.
    Nothing,
    /// The Anmeldung reached `processd`: the component is `ANGEMELDET`.
    Angemeldet {
        komp_id: Uuid,
        process_id: Option<String>,
    },
    /// The reading order reached `edmd`: keep the trail from a Schlussrechnung
    /// back to the reading it was built on.
    AbleseAuftrag { komp_id: Uuid, order_id: Uuid },
}

impl FollowUp {
    async fn apply(self, conn: &mut sqlx::PgConnection) -> Result<()> {
        match self {
            Self::Nothing => Ok(()),
            Self::Angemeldet {
                komp_id,
                process_id,
            } => {
                crate::pg::update_komponente_status(
                    &mut *conn,
                    komp_id,
                    "ANGEMELDET",
                    process_id.as_deref(),
                    None,
                    None,
                    None,
                )
                .await
            }
            Self::AbleseAuftrag { komp_id, order_id } => {
                sqlx::query(
                    "UPDATE vertragskomponenten
                        SET ablese_auftrag_id = $2, updated_at = now()
                      WHERE id = $1 AND ablese_auftrag_id IS NULL",
                )
                .bind(komp_id)
                .bind(order_id)
                .execute(&mut *conn)
                .await?;
                Ok(())
            }
        }
    }
}

/// The body a stored `processd` call posts; see [`processd_call`].
///
/// A payload without one is refused rather than posted whole: processd refuses
/// every key its request type does not declare, `endpoint` among them.
fn call_body(payload: &serde_json::Value) -> Result<&serde_json::Value> {
    payload
        .get("body")
        .ok_or_else(|| anyhow::anyhow!("processd task payload has no `body`"))
}

impl OutboundWorker {
    /// Perform one task's call, returning the follow-up write that records it.
    async fn perform(
        &self,
        kind: TaskKind,
        komp_id: Option<Uuid>,
        payload: &serde_json::Value,
    ) -> Result<FollowUp> {
        match kind {
            TaskKind::Lieferbeginn => {
                let endpoint = payload
                    .get("endpoint")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("start-supply");
                let processd = self.upstream(
                    "processd",
                    &self.cfg.processd_url,
                    self.cfg.processd_api_key.as_deref(),
                );
                let body = self
                    .post_json(&processd, endpoint, call_body(payload)?)
                    .await?;
                let process_id = body
                    .get("process_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                Ok(
                    komp_id.map_or(FollowUp::Nothing, |komp_id| FollowUp::Angemeldet {
                        komp_id,
                        process_id,
                    }),
                )
            }
            TaskKind::Lieferende => {
                let endpoint = payload
                    .get("endpoint")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("end-supply");
                let processd = self.upstream(
                    "processd",
                    &self.cfg.processd_url,
                    self.cfg.processd_api_key.as_deref(),
                );
                self.post_json(&processd, endpoint, call_body(payload)?)
                    .await?;
                Ok(FollowUp::Nothing)
            }
            TaskKind::AblesungBeginn | TaskKind::AblesungEnde => {
                let edmd =
                    self.upstream("edmd", &self.cfg.edmd_url, self.cfg.edmd_api_key.as_deref());
                let body = self.post_json(&edmd, "reading-orders", payload).await?;
                // edmd's POST is not idempotent: the dedupe key stops a second
                // *enqueue*, and completing the task before its follow-up stops
                // a second *call*.
                Ok(
                    match (
                        komp_id,
                        body.get("id")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|s| s.parse::<Uuid>().ok()),
                    ) {
                        (Some(komp_id), Some(order_id)) => {
                            FollowUp::AbleseAuftrag { komp_id, order_id }
                        }
                        _ => FollowUp::Nothing,
                    },
                )
            }
            TaskKind::Abrechnungskonto => {
                let accountingd = self.upstream(
                    "accountingd",
                    &self.cfg.accountingd_url,
                    self.cfg.accountingd_api_key.as_deref(),
                );
                // `PUT /api/v1/accounts/{malo_id}?lf_mp_id=…`: both identifiers
                // are addressing, not body fields. `UpdateAccountRequest` denies
                // unknown fields, so sending them would be a 422.
                let malo_id = payload
                    .get("malo_id")
                    .and_then(serde_json::Value::as_str)
                    .context("Abrechnungskonto task carries no malo_id")?;
                let lf_mp_id = payload
                    .get("lf_mp_id")
                    .and_then(serde_json::Value::as_str)
                    .context("Abrechnungskonto task carries no lf_mp_id")?;
                anyhow::ensure!(
                    malo_id.chars().all(|c| c.is_ascii_alphanumeric())
                        && lf_mp_id.chars().all(|c| c.is_ascii_alphanumeric()),
                    "Abrechnungskonto identifiers must be alphanumeric \
                     (malo_id={malo_id}, lf_mp_id={lf_mp_id})",
                );
                let path = format!("accounts/{malo_id}?lf_mp_id={lf_mp_id}");
                self.put_no_content(&accountingd, &path, &serde_json::json!({}))
                    .await?;
                Ok(FollowUp::Nothing)
            }
        }
    }

    /// Address a peer, sharing this worker's HTTP client.
    fn upstream(&self, name: &'static str, base: &str, key: Option<&str>) -> Upstream {
        Upstream::new(
            name,
            base,
            key.map(|k| secrecy::SecretString::from(k.to_owned())),
            self.http.clone(),
        )
    }

    /// POST a task's payload to `/api/v1/{path}` on `up`.
    ///
    /// A `404` is a failure here, not absence: the task named an endpoint the
    /// peer does not serve, and acknowledging it would drop the obligation the
    /// task exists to discharge.
    async fn post_json(
        &self,
        up: &Upstream,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let path = format!("/api/v1/{path}");
        up.json(up.post(&path).json(body))
            .await
            .context("outbound task dispatch")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{} answered 404 for POST {path} — the obligation stays open rather \
                     than being acknowledged against an endpoint that does not exist",
                    up.name(),
                )
            })
    }

    /// PUT a task's payload to `/api/v1/{path}` on `up`, reading no body.
    ///
    /// `accountingd` answers its upsert `204 No Content`, which is a success
    /// with nothing to parse — reading it as JSON turned every created account
    /// into eight retries and a dead-letter. Same rule as [`Self::post_json`]
    /// otherwise: a `404` is a failure, not absence.
    async fn put_no_content(
        &self,
        up: &Upstream,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let path = format!("/api/v1/{path}");
        up.send(up.put(&path).json(body))
            .await
            .context("outbound task dispatch")?
            .map(drop)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{} answered 404 for PUT {path} — the obligation stays open rather \
                     than being acknowledged against an endpoint that does not exist",
                    up.name(),
                )
            })
    }
}

/// Whether the callee refused the request itself: a 4xx other than 408 and
/// 429, which a retry with the same body cannot change.
fn is_refusal(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<mako_service::http::UpstreamError>())
        .filter_map(mako_service::http::UpstreamError::status)
        .any(|s| {
            s.is_client_error()
                && s != reqwest::StatusCode::REQUEST_TIMEOUT
                && s != reqwest::StatusCode::TOO_MANY_REQUESTS
        })
}

/// Exponential backoff, capped so a long outage does not push the next attempt
/// past the point where the obligation still matters.
fn backoff_secs(attempts: i32) -> u64 {
    let exp = u32::try_from(attempts.max(1) - 1).unwrap_or(0).min(16);
    BACKOFF_BASE_SECS
        .saturating_mul(1u64 << exp)
        .min(BACKOFF_CAP_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn kunde() -> Kunde {
        crate::domain::utilmd_kunde(
            Some(&serde_json::json!({
                "_typ": "GESCHAEFTSPARTNER",
                "nachname": "Mustermann",
                "vorname": "Erika",
                "adresse": {
                    "strasse": "Hauptstraße", "hausnummer": "1",
                    "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
                }
            })),
            None,
            None,
        )
    }

    fn gas() -> GasAnmeldung {
        GasAnmeldung::aus_vertrag(
            Some(Lieferbeginnanlass::Wechsel),
            "JAEHRLICH",
            true,
            Some(Netznutzungsvertrag::LieferantNb),
            Some(NetznutzungZahler::Lieferant),
            Some(rust_decimal::Decimal::ZERO),
            kunde(),
        )
        .expect("a full contract")
    }

    #[test]
    fn strom_carries_lieferbeginn_datum() {
        let b =
            lieferbeginn_body("STROM", "51238696012", None, date!(2026 - 04 - 01), None).unwrap();
        assert_eq!(b["endpoint"], "start-supply");
        assert_eq!(b["body"]["lieferbeginn_datum"], "2026-04-01");
    }

    #[test]
    fn gas_carries_the_contract_facts_and_a_compact_date() {
        let b = lieferbeginn_body(
            "GAS",
            "51238696012",
            None,
            date!(2026 - 04 - 01),
            Some(&gas()),
        )
        .unwrap();
        assert_eq!(b["endpoint"], "start-supply-gas");
        assert_eq!(b["body"]["process_date"], "20260401");
        assert_eq!(b["body"]["transaktionsgrund"], "E03");
        assert_eq!(b["body"]["abrechnungsintervall_monate"], 12);
        assert_eq!(b["body"]["netznutzungsvertrag"], "LIEFERANT_NB");
        assert_eq!(b["body"]["kunde"]["nachname"], "Mustermann");
        assert_eq!(b["body"]["ablesekarte"], b["body"]["kunde"]);
        assert!(b["body"].get("zaehlpunkt").is_none());
    }

    /// The posted body is processd's request type and nothing else: the
    /// routing key and the MP-IDs processd derives itself stay out of it.
    #[test]
    fn the_body_carries_no_key_beside_the_request_fields() {
        for b in [
            lieferbeginn_body("STROM", "51238696012", None, date!(2026 - 04 - 01), None).unwrap(),
            lieferbeginn_body(
                "GAS",
                "51238696012",
                None,
                date!(2026 - 04 - 01),
                Some(&gas()),
            )
            .unwrap(),
        ] {
            let body = b["body"].as_object().unwrap();
            for key in ["endpoint", "nb_mp_id", "lf_mp_id", "zaehlpunkt"] {
                assert!(!body.contains_key(key), "{key} in {body:?}");
            }
        }
    }

    #[test]
    fn the_contract_bilanzkreis_is_sent_and_an_absent_one_is_left_out() {
        let with = lieferbeginn_body(
            "STROM",
            "51238696012",
            Some("11XBK-STANDARD-4"),
            date!(2026 - 04 - 01),
            None,
        )
        .unwrap();
        assert_eq!(with["body"]["bilanzkreis"], "11XBK-STANDARD-4");
        let without =
            lieferbeginn_body("STROM", "51238696012", None, date!(2026 - 04 - 01), None).unwrap();
        assert!(without["body"].get("bilanzkreis").is_none());
    }

    #[test]
    fn gas_without_its_contract_facts_is_refused() {
        let err =
            lieferbeginn_body("GAS", "51238696012", None, date!(2026 - 04 - 01), None).unwrap_err();
        assert!(err.to_string().contains("44001"), "{err}");
    }

    /// Each fact the contract does not state is named; nothing is filled in.
    #[test]
    fn a_contract_missing_a_gas_fact_names_it() {
        let fehlend = GasAnmeldung::aus_vertrag(
            None,
            "WOECHENTLICH",
            false,
            None,
            None,
            None,
            Kunde {
                nachname: None,
                vorname: None,
                organisationsname: None,
                anschrift: None,
            },
        )
        .unwrap_err();
        let all = fehlend.join("\n");
        for named in [
            "lieferbeginn_anlass",
            "abrechnungszyklus",
            "netznutzungsvertrag",
            "netznutzung_zahler",
            "gemeinderabatt_prozent",
            "the name",
            "the anschrift",
        ] {
            assert!(all.contains(named), "{named}: {all}");
        }
    }

    /// Without an address of his own the Kunde's Korrespondenzanschrift is
    /// the Marktlokation's (Bedingung `[556]`).
    #[test]
    fn a_kunde_without_an_address_is_written_to_at_the_marktlokation() {
        let standort: rubo4e::current::Adresse = serde_json::from_value(serde_json::json!({
            "strasse": "Werkstraße", "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
        }))
        .unwrap();
        let k = crate::domain::utilmd_kunde(
            Some(
                &serde_json::json!({ "_typ": "GESCHAEFTSPARTNER", "organisationsname": "Muster GmbH" }),
            ),
            None,
            Some(&standort),
        );
        assert!(
            k.anmeldung_maengel().is_empty(),
            "{:?}",
            k.anmeldung_maengel()
        );
        assert_eq!(k.anschrift.unwrap().strasse.as_deref(), Some("Werkstraße"));
    }

    #[test]
    fn a_gas_lieferende_carries_a_compact_date() {
        let t = lieferende(Uuid::nil(), "GAS", "51238696012", date!(2026 - 12 - 31));
        assert_eq!(t.payload["endpoint"], "end-supply-gas");
        assert_eq!(t.payload["body"]["process_date"], "20261231");
        assert!(t.payload["body"].get("zaehlpunkt").is_none());
        assert!(t.payload["body"].get("lieferende_datum").is_none());
    }

    #[test]
    fn a_task_without_a_body_is_refused_rather_than_posted_whole() {
        assert!(call_body(&serde_json::json!({ "endpoint": "start-supply" })).is_err());
    }

    /// A replay of one Lieferende collapses into one task; a re-issue to a new
    /// date is a new task, so `ON CONFLICT DO NOTHING` cannot drop it.
    #[test]
    fn the_lieferende_key_is_the_component_and_its_date() {
        let komp = Uuid::nil();
        let at = |d| lieferende(komp, "STROM", "51238696012", d);
        assert_eq!(
            at(date!(2026 - 12 - 31)).dedupe_key,
            at(date!(2026 - 12 - 31)).dedupe_key,
            "a replay of the same date is one task"
        );
        assert_ne!(
            at(date!(2026 - 12 - 31)).dedupe_key,
            at(date!(2027 - 01 - 31)).dedupe_key,
            "a re-issue to another date is another task"
        );
    }

    #[test]
    fn the_backoff_grows_and_then_stops_growing() {
        assert_eq!(backoff_secs(1), 30);
        assert_eq!(backoff_secs(2), 60);
        assert_eq!(backoff_secs(3), 120);
        assert_eq!(backoff_secs(30), BACKOFF_CAP_SECS);
    }
}
