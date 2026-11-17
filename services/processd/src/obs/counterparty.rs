//! The Gegenpartei-Akte: what one counterparty owed us in a period, and how it
//! answered — on time, late, never, or with a refusal — proven process by
//! process from `process_projections`.
//!
//! ## Whose answer a process counts
//!
//! `deadline_at` is the PID's business Antwortfrist whichever side started the
//! process, so on a process the partner started it is **our** Frist. Who owes
//! the answer is [`mako_fristen::antwort::antwort_obligation`]'s `answered_by`,
//! and `mdm_role` is our own Marktrolle in the process (marktd derives it from
//! the workflow name). A process counts in the partner's Akte only when the
//! role owing the answer is not ours ([`schuldner`]); one whose debtor cannot
//! be told is counted apart, and one we owe is not in the Akte at all.
//!
//! ## Answered
//!
//! `completed_at` is written once, by the first terminal event. The answered
//! states are `completed` and `rejected`; a `failed` process carries a
//! `completed_at` too and is counted `abgebrochen`, never as answered. Late is
//! the instant comparison `completed_at > deadline_at`.
//!
//! ## One query per page
//!
//! [`rows`] reads the partner's processes in the period once; [`record`] folds
//! them. The export lists exactly the rows the figures were folded from, so its
//! rows sum to the figures by construction.

use std::collections::BTreeMap;

use rmcp::schemars;
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::{PgPool, Row as _};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Below this many processes a share is not stated, only the counts — the one
/// Desk-wide rule.
pub const MIN_FUER_QUOTE: u64 = 20;

/// One process of a counterparty, as the projection holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub process_id: Uuid,
    pub pid: u32,
    pub partner_mp_id: String,
    /// Our Marktrolle in the process.
    pub mdm_role: Option<String>,
    pub state: String,
    pub started_at: OffsetDateTime,
    pub deadline_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub antwort_code: Option<String>,
    pub antwort_codeliste: Option<String>,
    pub erc_code: Option<String>,
}

/// Who owes a process's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schuldner {
    /// The counterparty, in this Marktrolle.
    Gegenseite(&'static str),
    /// We do.
    Wir,
    /// The PID has no Antwortfrist this codebase has read.
    OhneFrist,
    /// Cannot be told: no `mdm_role`, or a sub-role of our own role.
    Unbestimmt,
}

/// The Marktrolle `answered_by` names, and whether it names exactly that role
/// rather than one side of it (`LFA`, `MSBA`, …), which our role alone cannot
/// place.
fn rolle_von(answered_by: &str) -> Option<(&'static str, bool)> {
    Some(match answered_by {
        "NB" | "NB (VNB)" => ("NB", true),
        "LF" => ("LF", true),
        "LFA" | "LFN" | "E/G" => ("LF", false),
        "MSB" => ("MSB", true),
        "MSBA" | "gMSB" | "wMSB" => ("MSB", false),
        "BIKO" => ("BIKO", true),
        _ => return None,
    })
}

/// Who owes the answer of a process of `pid` in which we act as `eigene`.
#[must_use]
pub fn schuldner(pid: u32, eigene: Option<&str>) -> Schuldner {
    let Some(obligation) = mako_fristen::antwort::antwort_obligation(pid) else {
        return Schuldner::OhneFrist;
    };
    let Some(eigene) = eigene else {
        return Schuldner::Unbestimmt;
    };
    if obligation.answered_by.starts_with("Gegenseite") {
        // „Gegenseite (LF oder MSB)": whoever received the trigger answers.
        return match eigene {
            "LF" => Schuldner::Gegenseite("MSB"),
            "MSB" => Schuldner::Gegenseite("LF"),
            _ => Schuldner::Unbestimmt,
        };
    }
    match rolle_von(obligation.answered_by) {
        Some((rolle, _)) if rolle != eigene => Schuldner::Gegenseite(rolle),
        Some((_, true)) => Schuldner::Wir,
        _ => Schuldner::Unbestimmt,
    }
}

/// Where one process lands in the Akte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kategorie {
    /// Answered (`completed`/`rejected`) at or before `deadline_at`.
    Fristgerecht,
    /// Answered after `deadline_at`.
    Verspaetet,
    /// Not answered and `deadline_at` has passed.
    Unbeantwortet,
    /// Not answered and `deadline_at` is still ahead.
    Offen,
    /// Ended `failed`: no answer arrived and none will.
    Abgebrochen,
    /// No Antwortfrist: counted apart, never as on time.
    OhneFrist,
    /// Whose answer it is cannot be told: counted apart.
    SchuldnerUnbestimmt,
}

fn beantwortet(state: &str) -> bool {
    matches!(state, "completed" | "rejected")
}

/// The category of a process the partner owes, at `now`.
#[must_use]
pub fn kategorie(row: &Row, now: OffsetDateTime) -> Kategorie {
    let Some(frist) = row.deadline_at else {
        return Kategorie::OhneFrist;
    };
    match (beantwortet(&row.state), row.completed_at) {
        (true, Some(at)) if at <= frist => Kategorie::Fristgerecht,
        (true, Some(_)) => Kategorie::Verspaetet,
        _ if row.state == "failed" => Kategorie::Abgebrochen,
        _ if frist < now => Kategorie::Unbeantwortet,
        _ => Kategorie::Offen,
    }
}

/// One process behind the figures — a row of the export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AkteProzess {
    pub process_id: String,
    pub pid: u32,
    pub kategorie: Kategorie,
    /// RFC 3339 UTC.
    #[schemars(extend("format" = "date-time"))]
    pub gestartet_um: String,
    /// The business Antwortfrist, RFC 3339 UTC.
    #[schemars(extend("format" = "date-time"))]
    pub frist: Option<String>,
    /// When the answer arrived, RFC 3339 UTC; `None` while unanswered.
    #[schemars(extend("format" = "date-time"))]
    pub beantwortet_um: Option<String>,
    /// Minutes past the Frist, for a late answer.
    pub verspaetung_minuten: Option<i64>,
    pub antwort_code: Option<String>,
    pub antwort_codeliste: Option<String>,
    /// The process ended `rejected`.
    pub abgelehnt: bool,
}

/// One refusal code and how often the partner answered with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Ablehnung {
    /// The Antwortcode, or the APERAK's ERC code where the business answer
    /// carried none.
    pub code: Option<String>,
    /// The tree or Codeliste the code was named in; `APERAK` for an ERC code.
    pub codeliste: Option<String>,
    /// The BDEW's wording for the code in that list; `None` when the list
    /// publishes none — never a guess from another list.
    pub bedeutung: Option<String>,
    pub anzahl: u64,
}

/// Minutes past the Frist over the late answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Verspaetung {
    pub median_minuten: Option<i64>,
    pub max_minuten: Option<i64>,
}

/// The processes of the MP-ID counted apart, whichever role it held.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Ausserhalb {
    pub ohne_frist: u64,
    pub schuldner_unbestimmt: u64,
}

/// One term of the figures and how it is computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Definition {
    pub begriff: &'static str,
    pub definition: &'static str,
}

/// The definitions every answer carries.
pub const DEFINITIONEN: &[Definition] = &[
    Definition {
        begriff: "gezaehlt",
        definition: "Processes started in [von 00:00, bis+1 00:00) Europe/Berlin whose answer the \
                     partner owes: answered_by of the PID (mako_fristen::antwort) names a role \
                     other than ours (mdm_role).",
    },
    Definition {
        begriff: "fristgerecht",
        definition: "State completed or rejected, completed_at <= deadline_at.",
    },
    Definition {
        begriff: "verspaetet",
        definition: "State completed or rejected, completed_at > deadline_at; the delay is \
                     completed_at - deadline_at.",
    },
    Definition {
        begriff: "unbeantwortet",
        definition: "Not answered and deadline_at before the generation time.",
    },
    Definition {
        begriff: "offen",
        definition: "Not answered and deadline_at at or after the generation time.",
    },
    Definition {
        begriff: "abgebrochen",
        definition: "State failed: the process ended without an answer.",
    },
    Definition {
        begriff: "ablehnungen",
        definition: "Answered processes in state rejected, by the Antwortcode and the list that \
                     names it (the APERAK ERC code where the answer carried none).",
    },
    Definition {
        begriff: "ausserhalb",
        definition: "Processes of the MP-ID not counted: ohne_frist (no Antwortfrist for the \
                     PID) and schuldner_unbestimmt (our role is missing, or answered_by names \
                     one side of our own role).",
    },
];

/// `get_counterparty_record`'s answer: one counterparty in one role.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CounterpartyRecord {
    pub mp_id: String,
    /// The counterparty's Marktrolle (`NB`, `LF`, `MSB`, `BIKO`).
    pub rolle: String,
    /// First Berlin calendar day of the period, ISO.
    #[schemars(extend("format" = "date"))]
    pub von: String,
    /// Last Berlin calendar day of the period, ISO.
    #[schemars(extend("format" = "date"))]
    pub bis: String,
    /// When the figures were computed, RFC 3339 UTC.
    #[schemars(extend("format" = "date-time"))]
    pub erstellt_um: String,
    pub gezaehlt: u64,
    pub fristgerecht: u64,
    pub verspaetet: u64,
    pub unbeantwortet: u64,
    pub offen: u64,
    pub abgebrochen: u64,
    pub verspaetung: Verspaetung,
    /// Share of answered processes answered on time, in percent; `None` below
    /// [`MIN_FUER_QUOTE`] answered processes.
    pub quote_fristgerecht: Option<f64>,
    pub ablehnungen: Vec<Ablehnung>,
    pub ausserhalb: Ausserhalb,
    /// Every process a figure counts, then those counted apart.
    pub prozesse: Vec<AkteProzess>,
    pub definitionen: Vec<Definition>,
}

/// One line of `list_counterparties`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CounterpartySummary {
    pub mp_id: String,
    pub rolle: String,
    pub gezaehlt: u64,
    pub verspaetet: u64,
    pub unbeantwortet: u64,
    pub offen: u64,
    pub ablehnungen: u64,
}

/// `list_counterparties`' answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Counterparties {
    #[schemars(extend("format" = "date"))]
    pub von: String,
    #[schemars(extend("format" = "date"))]
    pub bis: String,
    /// Most late and unanswered first.
    pub gegenparteien: Vec<CounterpartySummary>,
    pub definitionen: Vec<Definition>,
}

fn rfc3339(t: OffsetDateTime) -> String {
    use time::format_description::well_known::Rfc3339;
    t.format(&Rfc3339).unwrap_or_default()
}

fn minuten(d: time::Duration) -> i64 {
    // Rounded up: a process answered 30 seconds late is a minute late, never 0.
    let secs = d.whole_seconds();
    (secs + 59).div_euclid(60)
}

fn prozess(row: &Row, kategorie: Kategorie) -> AkteProzess {
    let verspaetung_minuten = match (kategorie, row.completed_at, row.deadline_at) {
        (Kategorie::Verspaetet, Some(at), Some(frist)) => Some(minuten(at - frist)),
        _ => None,
    };
    AkteProzess {
        process_id: row.process_id.to_string(),
        pid: row.pid,
        kategorie,
        gestartet_um: rfc3339(row.started_at),
        frist: row.deadline_at.map(rfc3339),
        beantwortet_um: row
            .completed_at
            .filter(|_| beantwortet(&row.state))
            .map(rfc3339),
        verspaetung_minuten,
        antwort_code: row.antwort_code.clone(),
        antwort_codeliste: row.antwort_codeliste.clone(),
        abgelehnt: row.state == "rejected",
    }
}

/// Fold `rows` — the MP-ID's processes in the period — into its record in
/// `rolle`. Processes counted apart are the MP-ID's, whichever role it held.
#[must_use]
pub fn record(
    mp_id: &str,
    rolle: &str,
    von: Date,
    bis: Date,
    rows: &[Row],
    now: OffsetDateTime,
) -> CounterpartyRecord {
    let mut r = CounterpartyRecord {
        mp_id: mp_id.to_owned(),
        rolle: rolle.to_owned(),
        von: von.to_string(),
        bis: bis.to_string(),
        erstellt_um: rfc3339(now),
        gezaehlt: 0,
        fristgerecht: 0,
        verspaetet: 0,
        unbeantwortet: 0,
        offen: 0,
        abgebrochen: 0,
        verspaetung: Verspaetung::default(),
        quote_fristgerecht: None,
        ablehnungen: Vec::new(),
        ausserhalb: Ausserhalb::default(),
        prozesse: Vec::new(),
        definitionen: DEFINITIONEN.to_vec(),
    };
    let mut apart = Vec::new();
    let mut delays = Vec::new();
    let mut ablehnungen: BTreeMap<(Option<String>, Option<String>), u64> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.partner_mp_id == mp_id) {
        let kategorie = match schuldner(row.pid, row.mdm_role.as_deref()) {
            Schuldner::Gegenseite(owed_by) if owed_by == rolle => kategorie(row, now),
            Schuldner::Gegenseite(_) | Schuldner::Wir => continue,
            Schuldner::OhneFrist => Kategorie::OhneFrist,
            Schuldner::Unbestimmt => Kategorie::SchuldnerUnbestimmt,
        };
        let p = prozess(row, kategorie);
        match kategorie {
            Kategorie::OhneFrist => {
                r.ausserhalb.ohne_frist += 1;
                apart.push(p);
                continue;
            }
            Kategorie::SchuldnerUnbestimmt => {
                r.ausserhalb.schuldner_unbestimmt += 1;
                apart.push(p);
                continue;
            }
            Kategorie::Fristgerecht => r.fristgerecht += 1,
            Kategorie::Verspaetet => {
                r.verspaetet += 1;
                delays.extend(p.verspaetung_minuten);
            }
            Kategorie::Unbeantwortet => r.unbeantwortet += 1,
            Kategorie::Offen => r.offen += 1,
            Kategorie::Abgebrochen => r.abgebrochen += 1,
        }
        r.gezaehlt += 1;
        if p.abgelehnt {
            let key = match (&row.antwort_code, &row.erc_code) {
                (Some(code), _) => (Some(code.clone()), row.antwort_codeliste.clone()),
                (None, Some(erc)) => (Some(erc.clone()), Some("APERAK".to_owned())),
                (None, None) => (None, None),
            };
            *ablehnungen.entry(key).or_default() += 1;
        }
        r.prozesse.push(p);
    }
    r.prozesse
        .sort_by(|a, b| (a.kategorie, &a.gestartet_um).cmp(&(b.kategorie, &b.gestartet_um)));
    apart.sort_by(|a, b| (a.kategorie, &a.gestartet_um).cmp(&(b.kategorie, &b.gestartet_um)));
    r.prozesse.extend(apart);
    delays.sort_unstable();
    r.verspaetung = Verspaetung {
        median_minuten: (!delays.is_empty()).then(|| delays[(delays.len() - 1) / 2]),
        max_minuten: delays.last().copied(),
    };
    let beantwortet = r.fristgerecht + r.verspaetet;
    if beantwortet >= MIN_FUER_QUOTE {
        #[expect(clippy::cast_precision_loss, reason = "counts far below 2^52")]
        let quote = (r.fristgerecht as f64) * 100.0 / (beantwortet as f64);
        r.quote_fristgerecht = Some(quote);
    }
    r.ablehnungen = ablehnungen
        .into_iter()
        .map(|((code, codeliste), anzahl)| Ablehnung {
            bedeutung: match (&code, &codeliste) {
                (Some(c), Some(l)) => {
                    mako_pruefung::codes::lies(l, c).map(|a| a.bedeutung.to_owned())
                }
                _ => None,
            },
            code,
            codeliste,
            anzahl,
        })
        .collect();
    r
}

/// Every counterparty and role with processes it owes in the period, the most
/// late and unanswered first.
#[must_use]
pub fn summaries(von: Date, bis: Date, rows: &[Row], now: OffsetDateTime) -> Counterparties {
    let mut keys: Vec<(String, &'static str)> = rows
        .iter()
        .filter_map(|r| match schuldner(r.pid, r.mdm_role.as_deref()) {
            Schuldner::Gegenseite(rolle) => Some((r.partner_mp_id.clone(), rolle)),
            _ => None,
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let mut gegenparteien: Vec<CounterpartySummary> = keys
        .into_iter()
        .map(|(mp_id, rolle)| {
            let a = record(&mp_id, rolle, von, bis, rows, now);
            CounterpartySummary {
                ablehnungen: a.ablehnungen.iter().map(|x| x.anzahl).sum(),
                mp_id,
                rolle: rolle.to_owned(),
                gezaehlt: a.gezaehlt,
                verspaetet: a.verspaetet,
                unbeantwortet: a.unbeantwortet,
                offen: a.offen,
            }
        })
        .collect();
    gegenparteien.sort_by(|a, b| {
        (b.verspaetet + b.unbeantwortet)
            .cmp(&(a.verspaetet + a.unbeantwortet))
            .then_with(|| a.mp_id.cmp(&b.mp_id))
            .then_with(|| a.rolle.cmp(&b.rolle))
    });
    Counterparties {
        von: von.to_string(),
        bis: bis.to_string(),
        gegenparteien,
        definitionen: DEFINITIONEN.to_vec(),
    }
}

/// The export's header line.
pub const CSV_HEADER: &str = "process_id,pid,kategorie,gestartet_um,frist,beantwortet_um,\
verspaetung_minuten,antwort_code,antwort_codeliste,abgelehnt\n";

/// The record's processes as CSV — every process a figure counts, then those
/// counted apart.
#[must_use]
pub fn csv(record: &CounterpartyRecord) -> String {
    let mut out = String::from(CSV_HEADER);
    for p in &record.prozesse {
        let kategorie = serde_json::to_value(p.kategorie)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        // The code and its list arrived from the counterparty, so they are
        // cells a spreadsheet must not read as a formula.
        out.push_str(&format!(
            "{},{},{kategorie},{},{},{},{},{},{},{}\n",
            p.process_id,
            p.pid,
            p.gestartet_um,
            p.frist.as_deref().unwrap_or_default(),
            p.beantwortet_um.as_deref().unwrap_or_default(),
            p.verspaetung_minuten
                .map(|m| m.to_string())
                .unwrap_or_default(),
            zelle(p.antwort_code.as_deref().unwrap_or_default()),
            zelle(p.antwort_codeliste.as_deref().unwrap_or_default()),
            p.abgelehnt,
        ));
    }
    out
}

/// One CSV cell of text that arrived from outside: a leading `=`, `+`, `-`,
/// `@`, tab or CR is prefixed `'` so a spreadsheet shows it rather than
/// evaluating it (OWASP CSV injection), and a cell holding a comma, quote or
/// line break is quoted with its quotes doubled (RFC 4180 Section 2).
#[must_use]
pub fn zelle(text: &str) -> String {
    let neutral = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if neutral.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", neutral.replace('"', "\"\""))
    } else {
        neutral
    }
}

/// The counterparty Marktrollen a page is kept for.
pub const ROLLEN: [&str; 4] = ["NB", "LF", "MSB", "BIKO"];

/// The longest period a page reads, in days.
pub const MAX_TAGE: i64 = 366;

/// A counterparty as a path names it: a 13-digit MP-ID and one of
/// [`ROLLEN`] — checked before the query and before either reaches the
/// export's file name.
///
/// # Errors
///
/// The field that is not one.
pub fn partner(mp_id: &str, rolle: &str) -> Result<(), String> {
    if mp_id.len() != 13 || !mp_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{mp_id:?} is not a 13-digit MP-ID"));
    }
    if !ROLLEN.contains(&rolle) {
        return Err(format!("{rolle:?} is not one of {}", ROLLEN.join(", ")));
    }
    Ok(())
}

/// Keep the processes in which we acted in one of the caller's Marktrollen —
/// an LF reads the Akte its own processes make, never the NB side's.
pub fn nur_eigene_rollen(rows: &mut Vec<Row>, rollen: &[String]) {
    rows.retain(|r| {
        r.mdm_role
            .as_deref()
            .is_some_and(|m| rollen.iter().any(|x| x.eq_ignore_ascii_case(m)))
    });
}

/// The period `von..=bis` on `started_at`, as Berlin calendar days.
fn window(von: Date, bis: Date) -> (OffsetDateTime, OffsetDateTime) {
    (
        mako_fristen::berlin_midnight(von),
        mako_fristen::berlin_midnight(bis.next_day().unwrap_or(bis)),
    )
}

/// The default period: the first of the current Berlin month three months
/// back, to today.
#[must_use]
pub fn default_period() -> (Date, Date) {
    let heute = mako_fristen::heute();
    let mut von = heute.replace_day(1).unwrap_or(heute);
    for _ in 0..3 {
        von = von
            .previous_day()
            .and_then(|d| d.replace_day(1).ok())
            .unwrap_or(von);
    }
    (von, heute)
}

/// The processes started in the period with a counterparty — of `mp_id`
/// alone, or of every counterparty — in one query.
///
/// # Errors
///
/// The query failing.
pub async fn rows(
    pool: &PgPool,
    tenant: &str,
    mp_id: Option<&str>,
    von: Date,
    bis: Date,
) -> Result<Vec<Row>, sqlx::Error> {
    let (from, to) = window(von, bis);
    let fetched = sqlx::query(
        "SELECT process_id, pid, partner_mp_id, mdm_role, state, started_at, deadline_at, \
                completed_at, antwort_code, antwort_codeliste, erc_code \
           FROM process_projections \
          WHERE tenant = $1 AND partner_mp_id IS NOT NULL \
            AND ($2::text IS NULL OR partner_mp_id = $2) \
            AND started_at >= $3 AND started_at < $4 \
          ORDER BY started_at, process_id",
    )
    .bind(tenant)
    .bind(mp_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    fetched
        .into_iter()
        .map(|r| {
            Ok(Row {
                process_id: r.try_get("process_id")?,
                pid: u32::try_from(r.try_get::<i32, _>("pid")?).unwrap_or_default(),
                partner_mp_id: r.try_get("partner_mp_id")?,
                mdm_role: r.try_get("mdm_role")?,
                state: r.try_get("state")?,
                started_at: r.try_get("started_at")?,
                deadline_at: r.try_get("deadline_at")?,
                completed_at: r.try_get("completed_at")?,
                antwort_code: r.try_get("antwort_code")?,
                antwort_codeliste: r.try_get("antwort_codeliste")?,
                erc_code: r.try_get("erc_code")?,
            })
        })
        .collect()
}

// ── REST ──────────────────────────────────────────────────────────────────────

/// The period a caller asks for: ISO Berlin calendar days, both optional.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodQuery {
    /// First Berlin calendar day, ISO (`2026-10-01`); default the first of the
    /// month three months back.
    pub von: Option<String>,
    /// Last Berlin calendar day, ISO; default today.
    pub bis: Option<String>,
    /// `csv` for the export; also chosen by `Accept: text/csv`.
    #[serde(default)]
    pub format: Option<String>,
}

/// `von`/`bis` read, defaulted and ordered — or the refusal naming the field.
///
/// # Errors
///
/// A date that is not ISO, or `von` after `bis`.
pub fn period(von: Option<&str>, bis: Option<&str>) -> Result<(Date, Date), String> {
    let iso = time::macros::format_description!("[year]-[month]-[day]");
    let (dvon, dbis) = default_period();
    let parse = |field: &str, raw: Option<&str>, default: Date| match raw {
        None | Some("") => Ok(default),
        Some(s) => Date::parse(s, &iso).map_err(|_| format!("{field} {s:?} is not an ISO date")),
    };
    let von = parse("von", von, dvon)?;
    let bis = parse("bis", bis, dbis)?;
    if von > bis {
        return Err(format!("von {von} is after bis {bis}"));
    }
    if (bis - von).whole_days() >= MAX_TAGE {
        return Err(format!("the period spans more than {MAX_TAGE} days"));
    }
    Ok((von, bis))
}

/// The counterparty routes, on the read model's state.
pub fn routes() -> axum::Router<crate::obs::project::ObsState> {
    axum::Router::new()
        .route(
            "/api/v1/counterparties",
            axum::routing::get(get_counterparties),
        )
        .route(
            "/api/v1/counterparties/{mp_id}/{rolle}",
            axum::routing::get(get_counterparty_record),
        )
}

/// Cedar `read-counterparty` — a market role of this tenant — and the
/// caller's roles, which the rows are then kept to.
fn authorize_akte(
    enforcer: &mako_service::cedar::CedarEnforcer,
    claims: &mako_service::oidc::Claims,
    tenant: &str,
) -> mako_service::ApiResult<Vec<String>> {
    let principal = claims.principal();
    enforcer
        .check(&principal, "read-counterparty", tenant)
        .map_err(|e| mako_service::ApiError::forbidden(e.to_string()))?;
    Ok(principal.roles.clone())
}

/// `GET /api/v1/counterparties?von=&bis=` — every counterparty and role with
/// processes it owes in the period, the most late and unanswered first.
async fn get_counterparties(
    claims: mako_service::oidc::Claims,
    axum::Extension(enforcer): axum::Extension<std::sync::Arc<mako_service::cedar::CedarEnforcer>>,
    axum::extract::State(state): axum::extract::State<crate::obs::project::ObsState>,
    axum::extract::Query(q): axum::extract::Query<PeriodQuery>,
) -> mako_service::ApiResult<mako_service::Json<Counterparties>> {
    let rollen = authorize_akte(&enforcer, &claims, &state.tenant)?;
    let (von, bis) = period(q.von.as_deref(), q.bis.as_deref())
        .map_err(|e| mako_service::ApiError::unprocessable(e).with_code("invalid_period"))?;
    let mut rows = rows(state.repo.pool(), &state.tenant, None, von, bis)
        .await
        .map_err(|e| mako_service::ApiError::internal(anyhow::Error::from(e)))?;
    nur_eigene_rollen(&mut rows, &rollen);
    Ok(mako_service::Json(summaries(
        von,
        bis,
        &rows,
        OffsetDateTime::now_utc(),
    )))
}

/// `GET /api/v1/counterparties/{mp_id}/{rolle}?von=&bis=[&format=csv]` — one
/// counterparty's record; `format=csv` or `Accept: text/csv` answers the
/// export of every process behind the figures.
async fn get_counterparty_record(
    claims: mako_service::oidc::Claims,
    axum::Extension(enforcer): axum::Extension<std::sync::Arc<mako_service::cedar::CedarEnforcer>>,
    axum::extract::State(state): axum::extract::State<crate::obs::project::ObsState>,
    axum::extract::Path((mp_id, rolle)): axum::extract::Path<(String, String)>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<PeriodQuery>,
) -> mako_service::ApiResult<axum::response::Response> {
    use axum::response::IntoResponse as _;
    let rollen = authorize_akte(&enforcer, &claims, &state.tenant)?;
    partner(&mp_id, &rolle)
        .map_err(|e| mako_service::ApiError::unprocessable(e).with_code("invalid_counterparty"))?;
    let (von, bis) = period(q.von.as_deref(), q.bis.as_deref())
        .map_err(|e| mako_service::ApiError::unprocessable(e).with_code("invalid_period"))?;
    let mut rows = rows(state.repo.pool(), &state.tenant, Some(&mp_id), von, bis)
        .await
        .map_err(|e| mako_service::ApiError::internal(anyhow::Error::from(e)))?;
    nur_eigene_rollen(&mut rows, &rollen);
    let rec = record(&mp_id, &rolle, von, bis, &rows, OffsetDateTime::now_utc());
    let wants_csv = q.format.as_deref() == Some("csv")
        || headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/csv"));
    if wants_csv {
        let name = format!("attachment; filename=\"gegenpartei-{mp_id}-{rolle}-{von}-{bis}.csv\"");
        return Ok((
            axum::http::StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/csv; charset=utf-8".to_owned(),
                ),
                (axum::http::header::CONTENT_DISPOSITION, name),
            ],
            csv(&rec),
        )
            .into_response());
    }
    Ok(mako_service::Json(rec).into_response())
}

// ── MCP ───────────────────────────────────────────────────────────────────────

/// Parameters for `get_counterparty_record`.
#[derive(Debug, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CounterpartyRecordParams {
    /// The counterparty's MP-ID.
    pub mp_id: String,
    /// The counterparty's Marktrolle: `NB`, `LF`, `MSB` or `BIKO`.
    pub rolle: String,
    /// First Berlin calendar day, ISO; default the first of the month three
    /// months back.
    pub von: Option<String>,
    /// Last Berlin calendar day, ISO; default today.
    pub bis: Option<String>,
}

/// Parameters for `list_counterparties`.
#[derive(Debug, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CounterpartiesParams {
    /// First Berlin calendar day, ISO.
    pub von: Option<String>,
    /// Last Berlin calendar day, ISO.
    pub bis: Option<String>,
}

#[rmcp::tool_router(router = counterparty_tool_router, vis = "pub(crate)")]
impl crate::mcp_server::ProcessdMcpHandler {
    #[rmcp::tool(
        description = "One counterparty's Akte in one Marktrolle over a period of Berlin \
                       calendar days: the processes whose answer it owed, answered on time, \
                       late (median and worst delay in minutes), never, still open, and its \
                       refusals by Antwortcode with the BDEW wording (`bedeutung`, quote it). \
                       Processes without Frist or whose debtor cannot be told are counted apart \
                       in `ausserhalb`. `prozesse` lists every process a figure counts; \
                       `definitionen` says how each figure is computed. Below 20 answered \
                       processes no share is stated.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<CounterpartyRecord>()
    )]
    async fn get_counterparty_record(
        &self,
        rmcp::handler::server::wrapper::Parameters(p): rmcp::handler::server::wrapper::Parameters<
            CounterpartyRecordParams,
        >,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        // The REST route's filter, from the MCP caller's own roles.
        let rollen = self.state.caller_roles(&parts.headers)?;
        let (von, bis) = period(p.von.as_deref(), p.bis.as_deref())
            .map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;
        partner(&p.mp_id, &p.rolle).map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;
        let mut rows = rows(
            &self.state.pool,
            &self.state.tenant,
            Some(&p.mp_id),
            von,
            bis,
        )
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
        nur_eigene_rollen(&mut rows, &rollen);
        let rec = record(
            &p.mp_id,
            &p.rolle,
            von,
            bis,
            &rows,
            OffsetDateTime::now_utc(),
        );
        serde_json::to_value(rec)
            .map(rmcp::model::CallToolResult::structured)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))
    }

    #[rmcp::tool(
        description = "Every counterparty and Marktrolle that owed us answers in a period of \
                       Berlin calendar days, the most late and unanswered first, with its \
                       counts. Read one with get_counterparty_record.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Counterparties>()
    )]
    async fn list_counterparties(
        &self,
        rmcp::handler::server::wrapper::Parameters(p): rmcp::handler::server::wrapper::Parameters<
            CounterpartiesParams,
        >,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let rollen = self.state.caller_roles(&parts.headers)?;
        let (von, bis) = period(p.von.as_deref(), p.bis.as_deref())
            .map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;
        let mut rows = rows(&self.state.pool, &self.state.tenant, None, von, bis)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
        nur_eigene_rollen(&mut rows, &rollen);
        serde_json::to_value(summaries(von, bis, &rows, OffsetDateTime::now_utc()))
            .map(rmcp::model::CallToolResult::structured)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    const NB: &str = "9900000000003";

    fn row(pid: u32, state: &str) -> Row {
        Row {
            process_id: Uuid::new_v4(),
            pid,
            partner_mp_id: NB.to_owned(),
            mdm_role: Some("LF".to_owned()),
            state: state.to_owned(),
            started_at: datetime!(2026-10-01 08:00 UTC),
            deadline_at: Some(datetime!(2026-10-02 09:00 UTC)),
            completed_at: None,
            antwort_code: None,
            antwort_codeliste: None,
            erc_code: None,
        }
    }

    /// 55001 is answered by the NB: on an LF's process it is the partner's
    /// answer, on an NB's it is ours, and a sub-role of our own cannot be
    /// placed.
    #[test]
    fn the_debtor_is_the_role_answered_by_names_when_it_is_not_ours() {
        assert_eq!(schuldner(55_001, Some("LF")), Schuldner::Gegenseite("NB"));
        assert_eq!(schuldner(55_001, Some("NB")), Schuldner::Wir);
        assert_eq!(schuldner(55_001, None), Schuldner::Unbestimmt);
        // 55016 Kündigung LFN → LFA: an LF cannot tell which side it was.
        assert_eq!(schuldner(55_016, Some("LF")), Schuldner::Unbestimmt);
        assert_eq!(schuldner(55_016, Some("NB")), Schuldner::Gegenseite("LF"));
    }

    /// The seeded set: one on time, one late by one minute, one
    /// unanswered, one without Frist, one rejected `A02` (answered on time),
    /// and one we owe, which is not in the Akte.
    #[test]
    fn each_process_lands_in_one_figure_and_the_csv_lists_them_all() {
        let frist = datetime!(2026-10-02 09:00 UTC);
        let mut on_time = row(55_001, "completed");
        on_time.completed_at = Some(frist);
        let mut late = row(55_001, "completed");
        late.completed_at = Some(frist + time::Duration::minutes(1));
        let unanswered = row(55_001, "running");
        let mut ohne = row(55_001, "running");
        ohne.deadline_at = None;
        let mut rejected = row(55_001, "rejected");
        rejected.completed_at = Some(frist - time::Duration::hours(1));
        rejected.antwort_code = Some("A02".to_owned());
        rejected.antwort_codeliste = Some("E_0622".to_owned());
        let mut ours = row(55_001, "running");
        ours.mdm_role = Some("NB".to_owned());
        let rows = [on_time, late, unanswered, ohne, rejected, ours];

        let now = datetime!(2026-10-05 12:00 UTC);
        let a = record(
            NB,
            "NB",
            date!(2026 - 10 - 01),
            date!(2026 - 10 - 31),
            &rows,
            now,
        );
        assert_eq!(
            (a.fristgerecht, a.verspaetet, a.unbeantwortet, a.offen),
            (2, 1, 1, 0),
            "{a:#?}"
        );
        assert_eq!(a.ausserhalb.ohne_frist, 1);
        assert_eq!(a.gezaehlt, 4, "the process we owe is not the partner's");
        assert_eq!(
            a.verspaetung.max_minuten,
            Some(1),
            "one minute late is late"
        );
        let ablehnung = &a.ablehnungen[0];
        assert_eq!(ablehnung.code.as_deref(), Some("A02"));
        assert_eq!(
            ablehnung.bedeutung.as_deref(),
            mako_pruefung::codes::lookup("E_0622", "A02").map(|c| c.bedeutung)
        );
        assert!(a.quote_fristgerecht.is_none(), "below 20 only counts");

        // The export's rows are the figures' processes, one each.
        let csv = csv(&a);
        let lines: Vec<&str> = csv.lines().skip(1).collect();
        let n = |k: &str| {
            lines
                .iter()
                .filter(|l| l.contains(&format!(",{k},")))
                .count() as u64
        };
        assert_eq!(n("fristgerecht"), a.fristgerecht);
        assert_eq!(n("verspaetet"), a.verspaetet);
        assert_eq!(n("unbeantwortet"), a.unbeantwortet);
        assert_eq!(n("ohne_frist"), a.ausserhalb.ohne_frist);
        assert_eq!(
            lines.len() as u64,
            a.gezaehlt + a.ausserhalb.ohne_frist + a.ausserhalb.schuldner_unbestimmt
        );

        let list = summaries(date!(2026 - 10 - 01), date!(2026 - 10 - 31), &rows, now);
        assert_eq!(list.gegenparteien.len(), 1);
        assert_eq!(list.gegenparteien[0].rolle, "NB");
        assert_eq!(list.gegenparteien[0].ablehnungen, 1);
    }

    /// A failed process ended without an answer: never on time.
    #[test]
    fn a_failed_process_is_abgebrochen_not_answered() {
        let mut failed = row(55_001, "failed");
        failed.completed_at = Some(datetime!(2026-10-01 09:00 UTC));
        assert_eq!(
            kategorie(&failed, datetime!(2026-10-05 12:00 UTC)),
            Kategorie::Abgebrochen
        );
    }

    /// A counterparty's code is a cell, never a formula, and RFC 4180 quoting
    /// keeps a comma inside its cell.
    #[test]
    fn formula_leading_antwortcode_is_neutralised_in_the_csv() {
        assert_eq!(zelle("A02"), "A02");
        assert_eq!(zelle("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(zelle("+1"), "'+1");
        assert_eq!(zelle("-1"), "'-1");
        assert_eq!(zelle("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(zelle("\tx"), "'\tx");
        assert_eq!(zelle("a,b"), "\"a,b\"");
    }

    /// The path names a 13-digit MP-ID and a known role before anything is
    /// read or named after it, and a period is at most a year.
    #[test]
    fn malformed_mp_id_is_refused_before_the_export() {
        assert!(partner(NB, "NB").is_ok());
        assert!(partner("99000000000\"x", "NB").is_err());
        assert!(partner("../../etc", "NB").is_err());
        assert!(partner(NB, "nb\r\n").is_err());
        assert!(period(Some("2025-01-01"), Some("2026-12-31")).is_err());
        assert!(period(Some("2026-01-01"), Some("2026-12-31")).is_ok());
    }

    /// An LF reads the Akte its own processes make: the NB side's process of
    /// the same partner is not in it.
    #[test]
    fn lf_role_sees_no_nb_side_akte() {
        let lf = row(55_001, "running");
        let mut nb = row(55_001, "running");
        nb.mdm_role = Some("NB".to_owned());
        let mut rows = vec![lf, nb];
        nur_eigene_rollen(&mut rows, &["LF".to_owned()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mdm_role.as_deref(), Some("LF"));
    }
}
