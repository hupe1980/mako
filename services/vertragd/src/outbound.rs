//! Durable outbound calls — everything `vertragd` owes another service.
//!
//! A Lieferbeginn is an obligation: the customer has a contract and the NB is
//! waiting for the UTILMD. Dispatching it from a detached `tokio::spawn` meant a
//! restart between the contract insert and the `processd` call dropped the
//! registration in silence, leaving the component in `ANGELEGT` with nothing to
//! retry it — and the same held for the Schlussablesung and the billing account.
//!
//! So each obligation is a [`Task`], written as a `mako_service::outbox`
//! command in the same transaction as the contract change ([`enqueue`]). One
//! [`workers`] drain per peer delivers it under the shared retry and
//! dead-letter policy, and [`follow_up`] records what the peer answered in the
//! transaction that marks the delivery done. A crash at any point loses
//! nothing; it only delays.
//!
//! ```text
//! handler ─┬─ contract write ──┐
//!          └─ enqueue command ─┴─ COMMIT ─→ outbox worker ─→ processd / edmd / accountingd
//! ```

use std::sync::Arc;

use anyhow::{Context as _, Result};
use mako_service::outbox::{Command, CommandMethod, Delivered, OutboxWorker};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

use mako_markt::utilmd_stammdaten::{Kunde, NetznutzungZahler, Netznutzungsvertrag};

use crate::config::VertragdConfig;
use crate::domain::Lieferbeginnanlass;

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
    /// `accountingd` § 13 BGB Verbraucher status of the contract.
    Verbraucher,
}

impl TaskKind {
    /// Every variant.
    pub const ALL: &'static [Self] = &[
        Self::Lieferbeginn,
        Self::Lieferende,
        Self::AblesungBeginn,
        Self::AblesungEnde,
        Self::Abrechnungskonto,
        Self::Verbraucher,
    ];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Lieferbeginn => "LIEFERBEGINN",
            Self::Lieferende => "LIEFERENDE",
            Self::AblesungBeginn => "ABLESUNG_BEGINN",
            Self::AblesungEnde => "ABLESUNG_ENDE",
            Self::Abrechnungskonto => "ABRECHNUNGSKONTO",
            Self::Verbraucher => "VERBRAUCHER",
        }
    }

    /// The variant `as_db` names.
    #[must_use]
    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "LIEFERBEGINN" => Self::Lieferbeginn,
            "LIEFERENDE" => Self::Lieferende,
            "ABLESUNG_BEGINN" => Self::AblesungBeginn,
            "ABLESUNG_ENDE" => Self::AblesungEnde,
            "ABRECHNUNGSKONTO" => Self::Abrechnungskonto,
            "VERBRAUCHER" => Self::Verbraucher,
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
/// and a task without them is a 422 on every attempt. Refuses any other
/// component without its [`StromAnmeldung`], for the same reason.
pub fn lieferbeginn(
    komp_id: Uuid,
    sparte: &str,
    malo_id: &str,
    bilanzkreis: Option<&str>,
    lieferbeginn: Date,
    strom: Option<&StromAnmeldung>,
    gas: Option<&GasAnmeldung>,
) -> Result<Task> {
    let payload = lieferbeginn_body(sparte, malo_id, bilanzkreis, lieferbeginn, strom, gas)?;
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

/// The Lieferant's contract facts a Strom Anmeldung 55001 carries beside the
/// Marktlokation, the date and the Bilanzkreis (UTILMD AHB Strom 2.2 Kap. 8.2)
/// — each one the contract's, each Muss in the column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StromAnmeldung {
    /// `SG4 STS+7` — `E03` Wechsel or `E01` Ein-/Auszug (Umzug), from the
    /// component's [`Lieferbeginnanlass`] (S. 73).
    pub transaktionsgrund: &'static str,
    /// `SG10 CCI+++Z15`/`Z18` — the Kunde's `haushaltskunde` (S. 83).
    pub haushaltskunde: bool,
    /// `SG8 SEQ+Z75 CCI+Z61` — `ZF9`, `ZG0` or `ZG1` (S. 85).
    pub enfg_grundlage: String,
    /// `SG10 CAV` — beside `ZF9` only.
    pub enfg_grund: Option<String>,
    /// `SG12 NAD+Z09`/`Z04` — the contract's Kunde (S. 86).
    pub kunde: Kunde,
}

impl StromAnmeldung {
    /// Assemble the facts, or name every one the contract does not state.
    ///
    /// # Errors
    ///
    /// Each missing or unusable fact, named with the segment it fills.
    pub fn aus_vertrag(
        anlass: Option<Lieferbeginnanlass>,
        haushaltskunde: bool,
        enfg_grundlage: Option<&str>,
        enfg_grund: Option<&str>,
        kunde: Kunde,
    ) -> std::result::Result<Self, Vec<String>> {
        let mut fehlend = Vec::new();
        let grund = anlass.and_then(Lieferbeginnanlass::anmeldung_strom_transaktionsgrund);
        if grund.is_none() {
            fehlend.push(format!(
                "lieferbeginn_anlass {anlass:?} (SG4 STS+7 Transaktionsgrund): the 55001 admits \
                 E01 Ein-/Auszug (Umzug) and E03 Wechsel — UMZUG or WECHSEL"
            ));
        }
        if enfg_grundlage.is_none() {
            fehlend.push("enfg_grundlage (SG8 SEQ+Z75 CCI+Z61 Grundlage nach EnFG)".to_owned());
        }
        fehlend.extend(
            kunde
                .anmeldung_maengel()
                .into_iter()
                .map(|m| format!("the Kunde (SG12 NAD+Z09/Z04): {m}")),
        );
        match (grund, enfg_grundlage) {
            (Some(transaktionsgrund), Some(enfg_grundlage)) if fehlend.is_empty() => Ok(Self {
                transaktionsgrund,
                haushaltskunde,
                enfg_grundlage: enfg_grundlage.to_owned(),
                enfg_grund: enfg_grund.map(str::to_owned),
                kunde,
            }),
            _ => Err(fehlend),
        }
    }
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
/// - `start-supply` (Strom et al.) takes `lieferbeginn_datum` (ISO-8601) and
///   the [`StromAnmeldung`];
/// - `start-supply-gas` takes `process_date` (YYYYMMDD) and the
///   [`GasAnmeldung`].
///
/// `bilanzkreis` is the contract's own; without one processd uses the single
/// `standard` entry of its `[[lf.bilanzkreise]]` and refuses the Anmeldung
/// (`MISSING_BILANZKREIS`) when there is none.
///
/// # Errors
///
/// A gas supply point without its [`GasAnmeldung`]; any other without its
/// [`StromAnmeldung`].
pub fn lieferbeginn_body(
    sparte: &str,
    malo_id: &str,
    bilanzkreis: Option<&str>,
    lieferbeginn: Date,
    strom: Option<&StromAnmeldung>,
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
        let strom = strom.ok_or_else(|| {
            anyhow::anyhow!(
                "supply point {malo_id}: start-supply carries the contract facts of the \
                 Anmeldung 55001, and none were assembled"
            )
        })?;
        serde_json::json!({
            "malo_id": malo_id,
            "lieferbeginn_datum": lieferbeginn.to_string(),
            "transaktionsgrund": strom.transaktionsgrund,
            "haushaltskunde": strom.haushaltskunde,
            "enfg_grundlage": strom.enfg_grundlage,
            "enfg_grund": strom.enfg_grund,
            "kunde": strom.kunde,
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
/// `YYYYMMDD` `process_date`; both carry the Transaktionsgrund `grund` decides
/// ([`crate::domain::Kuendigungsgrund::abmeldung_transaktionsgrund`]).
#[must_use]
pub fn lieferende(
    komp_id: Uuid,
    sparte: &str,
    malo_id: &str,
    lieferende: Date,
    grund: crate::domain::Kuendigungsgrund,
) -> Task {
    let payload = if sparte == "GAS" {
        processd_call(
            "end-supply-gas",
            serde_json::json!({
                "malo_id": malo_id,
                "process_date": compact_date(lieferende),
                "transaktionsgrund": grund.abmeldung_transaktionsgrund(),
            }),
        )
    } else {
        processd_call(
            "end-supply",
            serde_json::json!({
                "malo_id": malo_id,
                "lieferende_datum": lieferende.to_string(),
                "transaktionsgrund": grund.abmeldung_transaktionsgrund(),
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
/// **The dedupe key carries `geplant_am`**: a reading order is keyed by what
/// makes it distinct, and the date it is planned for is that. Keyed on the
/// component alone, a Kündigung withdrawn and re-issued to a different
/// Lieferende would collide with the first order and be dropped: `edmd` would
/// keep the order for the withdrawn date and the Schlussablesung happen on the
/// wrong day, with nothing reporting a failure.
///
/// A *same-date* re-enqueue still collapses, which is correct — that is a
/// replay of one order, not a second one.
#[must_use]
pub fn ablesung(komp_id: Uuid, malo_id: &str, sparte: &str, ende: bool, geplant_am: Date) -> Task {
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
            // The component's Sparte: `edmd` refuses an order without one,
            // because it decides the unit the Zählerstand is filed in.
            "sparte": sparte,
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

/// The contract's § 13 BGB Verbraucher status for accountingd's account on a
/// confirmed component — `PUT /api/v1/accounts/{malo_id}/verbraucher`.
///
/// The contract is that status's one source, and this command its one writer
/// (accountingd's Cedar `sync-verbraucher` is held to vertragd's principal):
/// accountingd charges the § 288 Abs. 2 BGB Verzugszins and the Abs. 5
/// Pauschale from it. It states `null` for an unrecorded status, so a value
/// written for an earlier contract on the MaLo is cleared rather than kept, and
/// **the key carries the component**, so a later contract on the same MaLo
/// sends its own status instead of colliding with the first one's key.
#[must_use]
pub fn verbraucher(
    komp_id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    verbraucher: Option<bool>,
) -> Task {
    Task {
        kind: TaskKind::Verbraucher,
        komp_id: Some(komp_id),
        payload: serde_json::json!({
            "malo_id": malo_id,
            "lf_mp_id": lf_mp_id,
            "verbraucher": verbraucher,
        }),
        dedupe_key: format!("VERBRAUCHER:{malo_id}:{komp_id}"),
    }
}

// ── Enqueue ───────────────────────────────────────────────────────────────────

/// The outbox upstream names, one worker each.
pub const UPSTREAMS: [&str; 3] = ["processd", "edmd", "accountingd"];

/// The outbox key of `task` for `tenant`: `{tenant}/{dedupe_key}`.
///
/// The dedupe key is what makes the enqueue exactly-once. A repeatable action
/// varies it by what makes it distinct (`ABLESUNG_BEGINN:{komp}:{datum}`); a
/// one-shot one does not (`LIEFERBEGINN:{komp}`), so an idempotent re-POST of
/// the same `erp_contract_id` cannot enqueue a second UTILMD.
#[must_use]
pub fn key(tenant: &str, task: &Task) -> String {
    format!("{tenant}/{}", task.dedupe_key)
}

/// The outbox command that performs `task`.
///
/// # Errors
///
/// Refuses a `processd` task without a `body` (processd refuses every key its
/// request type does not declare, `endpoint` among them), and an
/// Abrechnungskonto whose identifiers are not alphanumeric — both address the
/// URL.
pub fn command(tenant: &str, task: &Task) -> Result<Command> {
    let (upstream, method, path, body) = match task.kind {
        TaskKind::Lieferbeginn | TaskKind::Lieferende => {
            let fallback = if task.kind == TaskKind::Lieferbeginn {
                "start-supply"
            } else {
                "end-supply"
            };
            let endpoint = task
                .payload
                .get("endpoint")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(fallback);
            (
                "processd",
                CommandMethod::Post,
                format!("/api/v1/{endpoint}"),
                call_body(&task.payload)?.clone(),
            )
        }
        TaskKind::AblesungBeginn | TaskKind::AblesungEnde => (
            "edmd",
            CommandMethod::Post,
            "/api/v1/reading-orders".to_owned(),
            task.payload.clone(),
        ),
        TaskKind::Abrechnungskonto | TaskKind::Verbraucher => {
            // `PUT /api/v1/accounts/{malo_id}[/verbraucher]?lf_mp_id=…`: both
            // identifiers are addressing, not body fields — accountingd denies
            // unknown fields, so sending them would be a 422.
            let malo_id = task
                .payload
                .get("malo_id")
                .and_then(serde_json::Value::as_str)
                .context("accountingd task carries no malo_id")?;
            let lf_mp_id = task
                .payload
                .get("lf_mp_id")
                .and_then(serde_json::Value::as_str)
                .context("accountingd task carries no lf_mp_id")?;
            anyhow::ensure!(
                malo_id.chars().all(|c| c.is_ascii_alphanumeric())
                    && lf_mp_id.chars().all(|c| c.is_ascii_alphanumeric()),
                "accountingd identifiers must be alphanumeric \
                 (malo_id={malo_id}, lf_mp_id={lf_mp_id})",
            );
            let (pfad, body) = if task.kind == TaskKind::Verbraucher {
                // `verbraucher` always: `null` clears a status an earlier
                // contract on the MaLo recorded.
                let v = task
                    .payload
                    .get("verbraucher")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                ("/verbraucher", serde_json::json!({ "verbraucher": v }))
            } else {
                ("", serde_json::json!({}))
            };
            (
                "accountingd",
                CommandMethod::Put,
                format!("/api/v1/accounts/{malo_id}{pfad}?lf_mp_id={lf_mp_id}"),
                body,
            )
        }
    };
    Ok(Command {
        upstream: upstream.to_owned(),
        label: task.kind.as_db().to_owned(),
        method,
        path,
        body,
        key: key(tenant, task),
        settled_statuses: Vec::new(),
    })
}

/// Persist a task in the caller's transaction. `false` when an identical task
/// was already enqueued — the exactly-once guarantee, in the database rather
/// than in a caller's memory.
///
/// # Errors
///
/// Propagates storage errors and [`command`]'s refusals.
pub async fn enqueue(conn: &mut sqlx::PgConnection, tenant: &str, task: &Task) -> Result<bool> {
    let cmd = command(tenant, task)?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM event_outbox WHERE event_id = $1)")
            .bind(&cmd.key)
            .fetch_one(&mut *conn)
            .await?;
    if known {
        return Ok(false);
    }
    mako_service::outbox::enqueue_command(conn, &cmd).await?;
    Ok(true)
}

/// Enqueue a task that **supersedes** any not-yet-delivered task of the same
/// kind for the same component.
///
/// For a date-keyed task (the reading orders), varying the key is only half the
/// fix: a Kündigung re-issued to a new Lieferende would otherwise leave the
/// order for the withdrawn date in the outbox, and `edmd` would receive both. A
/// command that has not been delivered has been seen by nobody, so the obsolete
/// one is deleted rather than delivered and then countermanded.
///
/// One already delivered (the order is with `edmd`) or dead-lettered (the
/// operator's queue) is left alone: those are records of something that
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
        return enqueue(conn, tenant, task).await;
    };
    sqlx::query(
        "DELETE FROM event_outbox
          WHERE kind = 'command' AND starts_with(event_id, $1) AND event_id <> $2
            AND delivered_at IS NULL AND dead_lettered_at IS NULL",
    )
    .bind(format!("{tenant}/{}:{komp_id}", task.kind.as_db()))
    .bind(key(tenant, task))
    .execute(&mut *conn)
    .await?;
    enqueue(conn, tenant, task).await
}

/// Park the commands of `kinds` not yet delivered for the components of
/// contract `vertrag_id`, so the worker does not perform them: dead-lettered
/// with `grund`, their key released (`…:{grund}:{id}`) so an obligation
/// re-issued with the same key queues again.
///
/// `only_future_lieferende` narrows it to components whose `lieferende` is
/// still ahead — what a withdrawn Kündigung scheduled.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn park(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    vertrag_id: Uuid,
    kinds: &[TaskKind],
    only_future_lieferende: bool,
    grund: &str,
) -> Result<u64> {
    let kinds: Vec<&str> = kinds.iter().map(|k| k.as_db()).collect();
    Ok(sqlx::query(
        r"UPDATE event_outbox o
             SET dead_lettered_at = now(),
                 last_error = $5,
                 event_id = o.event_id || ':' || $5 || ':' || o.id
           WHERE o.kind = 'command' AND o.ce_type = ANY($3)
             AND o.delivered_at IS NULL AND o.dead_lettered_at IS NULL
             AND EXISTS (
                 SELECT 1 FROM vertragskomponenten k
                  WHERE k.vertrag_id = $1 AND k.tenant = $2
                    AND (NOT $4 OR (k.lieferende IS NOT NULL AND k.lieferende > heute()))
                    AND starts_with(o.event_id, $2 || '/' || o.ce_type || ':' || k.id::text)
             )",
    )
    .bind(vertrag_id)
    .bind(tenant)
    .bind(&kinds)
    .bind(only_future_lieferende)
    .bind(grund)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// This tenant's commands that exhausted their retries — the operator's work
/// queue.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_dead_lettered(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> Result<Vec<mako_service::outbox::DeadLetter>> {
    Ok(sqlx::query_as(
        "SELECT id, event_id, kind, upstream, ce_type, attempts, last_error, dead_lettered_at
           FROM event_outbox
          WHERE kind = 'command' AND starts_with(event_id, $1 || '/')
            AND dead_lettered_at IS NOT NULL
          ORDER BY dead_lettered_at DESC
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Put one of this tenant's dead-lettered commands back in the queue after the
/// operator fixed the cause. `false` when no such dead letter exists.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn retry_dead_lettered(pool: &PgPool, tenant: &str, id: Uuid) -> Result<bool> {
    let dead: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM event_outbox
                         WHERE id = $1 AND kind = 'command'
                           AND starts_with(event_id, $2 || '/')
                           AND dead_lettered_at IS NOT NULL)",
    )
    .bind(id)
    .bind(tenant)
    .fetch_one(pool)
    .await?;
    if !dead {
        return Ok(false);
    }
    Ok(mako_service::outbox::requeue(pool, id).await?)
}

// ── Delivery ──────────────────────────────────────────────────────────────────

/// One outbox worker per peer, each recording its deliveries with
/// [`follow_up`].
#[must_use]
pub fn workers(pool: &PgPool, cfg: &VertragdConfig) -> Vec<OutboxWorker> {
    let secret = |k: Option<&String>| k.map(|k| secrecy::SecretString::from(k.clone()));
    [
        (
            "processd",
            &cfg.processd_url,
            secret(cfg.processd_api_key.as_ref()),
        ),
        ("edmd", &cfg.edmd_url, secret(cfg.edmd_api_key.as_ref())),
        (
            "accountingd",
            &cfg.accountingd_url,
            secret(cfg.accountingd_api_key.as_ref()),
        ),
    ]
    .into_iter()
    .map(|(name, url, key)| {
        OutboxWorker::commands(pool.clone(), name, url.clone(), key).on_delivered(Arc::new(
            |conn, delivered| Box::pin(follow_up(conn, delivered)),
        ))
    })
    .collect()
}

/// The local write that records a delivered call, in the transaction that
/// marks the delivery done.
///
/// The call reached its peer: the delivery is complete whatever this write
/// does. So the write runs in a savepoint, and a failure is recorded on the
/// delivered row rather than returned — returning it would roll the delivered
/// mark back and send the NB a second UTILMD 55001, `edmd` a second reading
/// order.
///
/// # Errors
///
/// Storage errors outside the savepoint (opening it, recording the failure).
pub async fn follow_up(
    conn: &mut sqlx::PgConnection,
    delivered: &Delivered,
) -> Result<(), sqlx::Error> {
    let mut savepoint = sqlx::Acquire::begin(&mut *conn).await?;
    match apply_follow_up(&mut savepoint, delivered).await {
        Ok(()) => savepoint.commit().await,
        Err(e) => {
            savepoint.rollback().await?;
            let detail = format!("delivered; follow-up failed: {e:#}");
            tracing::error!(
                task = %delivered.ce_type, key = ?delivered.subject, error = %detail,
                "vertragd: outbound call delivered but its follow-up write failed — \
                 not retried, recorded on the delivery"
            );
            if let Some(key) = &delivered.subject {
                sqlx::query("UPDATE event_outbox SET last_error = $2 WHERE event_id = $1")
                    .bind(key)
                    .bind(&detail)
                    .execute(&mut *conn)
                    .await?;
            }
            Ok(())
        }
    }
}

/// The component a command's key names: `{tenant}/{KIND}:{komp}[:…]`.
fn komp_of(key: &str) -> Option<Uuid> {
    let (_, rest) = key.split_once('/')?;
    rest.split(':').nth(1)?.parse().ok()
}

async fn apply_follow_up(conn: &mut sqlx::PgConnection, delivered: &Delivered) -> Result<()> {
    let Some(komp_id) = delivered.subject.as_deref().and_then(komp_of) else {
        return Ok(());
    };
    let reply = delivered.reply.as_ref();
    match TaskKind::from_db(&delivered.ce_type) {
        // The Anmeldung reached `processd`: the component is `ANGEMELDET`.
        Some(TaskKind::Lieferbeginn) => {
            let process_id = reply
                .and_then(|b| b.get("process_id"))
                .and_then(serde_json::Value::as_str);
            crate::pg::update_komponente_status(
                &mut *conn,
                komp_id,
                "ANGEMELDET",
                process_id,
                None,
                None,
                None,
            )
            .await
        }
        // The reading order reached `edmd`: keep the trail from a
        // Schlussrechnung back to the reading it was built on.
        Some(TaskKind::AblesungBeginn | TaskKind::AblesungEnde) => {
            let Some(order_id) = reply
                .and_then(|b| b.get("id"))
                .and_then(serde_json::Value::as_str)
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                return Ok(());
            };
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
        _ => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// A gas contract's Schlussablesung is ordered as gas: `edmd` files the
    /// Zählerstand in the unit the order's Sparte names.
    #[test]
    fn a_gas_schlussablesung_carries_gas() {
        let task = ablesung(
            Uuid::new_v4(),
            "51238696012",
            "GAS",
            true,
            date!(2026 - 07 - 01),
        );
        assert_eq!(task.payload["sparte"], "GAS");
        assert_eq!(task.payload["anlass"], "LIEFERENDE");
    }

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

    fn strom() -> StromAnmeldung {
        StromAnmeldung::aus_vertrag(
            Some(Lieferbeginnanlass::Wechsel),
            true,
            Some("ZG1"),
            None,
            kunde(),
        )
        .expect("a full contract")
    }

    #[test]
    fn strom_carries_lieferbeginn_datum() {
        let b = lieferbeginn_body(
            "STROM",
            "51238696012",
            None,
            date!(2026 - 04 - 01),
            Some(&strom()),
            None,
        )
        .unwrap();
        assert_eq!(b["endpoint"], "start-supply");
        assert_eq!(b["body"]["lieferbeginn_datum"], "2026-04-01");
        assert_eq!(b["body"]["haushaltskunde"], true);
        assert_eq!(b["body"]["enfg_grundlage"], "ZG1");
        assert_eq!(b["body"]["kunde"]["nachname"], "Mustermann");
    }

    /// The 55001's `SG4 STS+7` is the contract's Anlass — `E03` Wechsel, `E01`
    /// Umzug (UTILMD AHB Strom 2.2 Kap. 8.2, S. 73). A contract that states
    /// none, or one the 55001 cannot carry, is refused rather than defaulted,
    /// and so is one without its Grundlage nach EnFG or a Kunde with an
    /// address.
    #[test]
    fn a_strom_anmeldung_carries_the_contracts_grund() {
        for (anlass, grund) in [
            (Lieferbeginnanlass::Wechsel, "E03"),
            (Lieferbeginnanlass::Umzug, "E01"),
        ] {
            let a = StromAnmeldung::aus_vertrag(Some(anlass), true, Some("ZG0"), None, kunde())
                .unwrap();
            assert_eq!(a.transaktionsgrund, grund, "{anlass:?}");
        }
        for anlass in [
            None,
            Some(Lieferbeginnanlass::Neuanlage),
            Some(Lieferbeginnanlass::EndeErsatzversorgung),
        ] {
            let fehlend =
                StromAnmeldung::aus_vertrag(anlass, true, Some("ZG0"), None, kunde()).unwrap_err();
            assert!(
                fehlend.join(" ").contains("STS+7"),
                "{anlass:?}: {fehlend:?}"
            );
        }
        let fehlend = StromAnmeldung::aus_vertrag(
            Some(Lieferbeginnanlass::Wechsel),
            true,
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
        let fehlend = fehlend.join(" ");
        for named in ["enfg_grundlage", "NAD+Z09"] {
            assert!(fehlend.contains(named), "{named}: {fehlend}");
        }
        let err = lieferbeginn_body(
            "STROM",
            "51238696012",
            None,
            date!(2026 - 04 - 01),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("55001"), "{err}");
    }

    #[test]
    fn gas_carries_the_contract_facts_and_a_compact_date() {
        let b = lieferbeginn_body(
            "GAS",
            "51238696012",
            None,
            date!(2026 - 04 - 01),
            None,
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
            lieferbeginn_body(
                "STROM",
                "51238696012",
                None,
                date!(2026 - 04 - 01),
                Some(&strom()),
                None,
            )
            .unwrap(),
            lieferbeginn_body(
                "GAS",
                "51238696012",
                None,
                date!(2026 - 04 - 01),
                None,
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
            Some(&strom()),
            None,
        )
        .unwrap();
        assert_eq!(with["body"]["bilanzkreis"], "11XBK-STANDARD-4");
        let without = lieferbeginn_body(
            "STROM",
            "51238696012",
            None,
            date!(2026 - 04 - 01),
            Some(&strom()),
            None,
        )
        .unwrap();
        assert!(without["body"].get("bilanzkreis").is_none());
    }

    #[test]
    fn gas_without_its_contract_facts_is_refused() {
        let err = lieferbeginn_body(
            "GAS",
            "51238696012",
            None,
            date!(2026 - 04 - 01),
            None,
            None,
        )
        .unwrap_err();
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
        let t = lieferende(
            Uuid::nil(),
            "GAS",
            "51238696012",
            date!(2026 - 12 - 31),
            crate::domain::Kuendigungsgrund::Ordentlich,
        );
        assert_eq!(t.payload["endpoint"], "end-supply-gas");
        assert_eq!(t.payload["body"]["process_date"], "20261231");
        assert_eq!(t.payload["body"]["transaktionsgrund"], "ZT5");
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
        let at = |d| {
            lieferende(
                komp,
                "STROM",
                "51238696012",
                d,
                crate::domain::Kuendigungsgrund::Ordentlich,
            )
        };
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
    fn a_key_names_its_component() {
        let komp = Uuid::new_v4();
        let task = ablesung(komp, "51238696012", "STROM", true, date!(2026 - 07 - 01));
        assert_eq!(komp_of(&key("9800000000016", &task)), Some(komp));
        let konto = abrechnungskonto(komp, "51238696012", "9800000000016");
        assert_eq!(komp_of(&key("9800000000016", &konto)), None);
    }

    /// The account accountingd keeps reads its § 13 BGB status from the
    /// contract: the status has its own route, and it states `null` when the
    /// contract records none, so a stale value from an earlier contract on the
    /// MaLo does not survive. The account command carries no status at all.
    #[test]
    fn the_verbraucher_command_states_the_contracts_status() {
        let komp = Uuid::new_v4();
        for status in [Some(true), Some(false), None] {
            let cmd = command(
                "9800000000016",
                &verbraucher(komp, "51238696012", "9800000000016", status),
            )
            .expect("command");
            assert_eq!(
                cmd.path,
                "/api/v1/accounts/51238696012/verbraucher?lf_mp_id=9800000000016"
            );
            assert_eq!(cmd.body, serde_json::json!({ "verbraucher": status }));
        }
        let konto = command(
            "9800000000016",
            &abrechnungskonto(komp, "51238696012", "9800000000016"),
        )
        .expect("command");
        assert_eq!(konto.body, serde_json::json!({}));
        assert_ne!(
            verbraucher(komp, "51238696012", "9800000000016", None).dedupe_key,
            verbraucher(Uuid::new_v4(), "51238696012", "9800000000016", None).dedupe_key,
            "a later contract on the same MaLo sends its own status"
        );
    }
}
