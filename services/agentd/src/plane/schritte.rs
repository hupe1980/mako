//! A run's journal as the person who started it follows it: one [`Schritt`]
//! per service read, then the conclusion.
//!
//! ## A view of the journal, never a channel
//!
//! Every event is derived from a journal record and carries that record's
//! sequence number, so a subscriber that reconnects with the last number it
//! saw continues from there — on any instance, after any failover — and the
//! events cannot disagree with history because they are read from it. This is
//! agentplane's own argument for its A2A stream (`agentplane::api::a2a_stream`),
//! applied to the shape the Desk renders.
//!
//! [`Faltung`] is the pure half: it reads records in order and says which of
//! them is a step or the end. The HTTP half (`handlers::schritte`) reads the
//! journal, wakes on its append hint and falls back to a poll.

use std::collections::HashMap;

use agentplane::core::{EffectKey, Seq};
use agentplane::journal::{Record, RecordKind};
use agentplane::tools::ToolId;
use serde_json::Value;

/// One service read of a run, as the `schritt` event carries it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Schritt {
    /// The journal sequence of the record that settled the call.
    pub seq: Seq,
    /// `lesen` for a call that answered, `fehlgeschlagen` for one that did
    /// not.
    pub art: &'static str,
    /// What was done, as a verb: `gelesen`, `aufgelistet`, `geprüft`, …
    pub was: &'static str,
    /// What the call was about — `Prozess 4711`, `MaLo 51238696012` — read
    /// from its arguments; `None` when no argument names an object.
    pub objekt: Option<String>,
    /// The service and tool that answered; `None` exactly when `verborgen`.
    pub quelle: Option<Quelle>,
    /// The reader may not see this step's source. The step is still sent.
    pub verborgen: bool,
    /// When this instance read the record, RFC 3339. The journal stamps no
    /// time on its records, so this is the read, not the write.
    pub um: String,
}

/// Where a step's answer came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Quelle {
    /// The MCP server, as the grant names it (`makod`).
    pub dienst: String,
    /// The tool (`get_process_history`).
    pub werkzeug: String,
    /// The argument value [`Schritt::objekt`] names, unprefixed.
    #[serde(rename = "ref")]
    pub referenz: Option<String>,
}

/// The `abgebrochen` event: who stopped the run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Abgebrochen {
    /// The operator on the `RunCancelled` record.
    pub durch: String,
    /// When this instance read the record, RFC 3339.
    pub um: String,
}

/// What one record means for the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gefaltet {
    Schritt(Schritt),
    /// The run concluded at `seq`. `abgebrochen` when it concluded
    /// `cancelled`; otherwise the answer read is the final event.
    Ende {
        seq: Seq,
        abgebrochen: Option<Abgebrochen>,
    },
}

/// Reads a run's records in order and joins each tool call's announcement to
/// the record that settled it.
///
/// Fed every record from the first, including those a resuming subscriber has
/// already seen: a call announced before the resume point and settled after it
/// is still a step, and only the announcement names the tool.
#[derive(Debug, Default)]
pub struct Faltung {
    angekuendigt: HashMap<EffectKey, (ToolId, Value)>,
    gestoppt_von: Option<String>,
}

impl Faltung {
    /// What `record` means, stamped `um` (RFC 3339).
    pub fn falte(&mut self, record: &Record, um: &str) -> Option<Gefaltet> {
        let seq = record.seq();
        match record.kind() {
            RecordKind::EffectStarted { descriptor, .. } => {
                if let (Some(key), Some(tool)) = (record.effect_key(), ToolId::called(descriptor)) {
                    let args = descriptor
                        .args
                        .get("arguments")
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.angekuendigt.insert(key, (tool, args));
                }
                None
            }
            RecordKind::EffectDone { .. }
            | RecordKind::EffectFailed { .. }
            | RecordKind::EffectReconciled { .. } => {
                let (tool, args) = self.angekuendigt.remove(&record.effect_key()?)?;
                let answered = match record.kind() {
                    RecordKind::EffectDone { .. } => true,
                    RecordKind::EffectReconciled { output, .. } => output.is_some(),
                    _ => false,
                };
                let (objekt, referenz) = objekt(&args).unzip();
                Some(Gefaltet::Schritt(Schritt {
                    seq,
                    art: if answered { "lesen" } else { "fehlgeschlagen" },
                    was: verb(&tool.tool),
                    objekt,
                    quelle: Some(Quelle {
                        dienst: tool.server,
                        werkzeug: tool.tool,
                        referenz,
                    }),
                    verborgen: false,
                    um: um.to_owned(),
                }))
            }
            RecordKind::RunCancelled { actor, .. } => {
                self.gestoppt_von = Some(actor.actor().to_owned());
                None
            }
            RecordKind::RunConcluded { outcome, .. } => Some(Gefaltet::Ende {
                seq,
                abgebrochen: (outcome == "cancelled").then(|| Abgebrochen {
                    durch: self.gestoppt_von.clone().unwrap_or_default(),
                    um: um.to_owned(),
                }),
            }),
            _ => None,
        }
    }
}

/// The verb a tool's name states. mako's MCP tools are named verb-first
/// (`get_`, `list_`, `check_`, …); a name outside these reads `aufgerufen`.
fn verb(tool: &str) -> &'static str {
    let prefix = tool.split('_').next().unwrap_or_default();
    match prefix {
        "get" => "gelesen",
        "list" => "aufgelistet",
        "check" | "validate" => "geprüft",
        "explain" | "resolve" => "nachgeschlagen",
        "compute" | "preview" => "berechnet",
        "quote" => "zitiert",
        "find" => "gesucht",
        _ => "aufgerufen",
    }
}

/// The arguments that name an object, most specific first, with the noun
/// the Desk shows before the value.
const OBJEKTE: &[(&str, &str)] = &[
    ("process_id", "Prozess"),
    ("entry_id", "Freigabe"),
    ("receipt_id", "Rechnung"),
    ("dispute_id", "Disput"),
    ("booking_key", "Buchung"),
    ("kunde_id", "Kunde"),
    ("malo_id", "MaLo"),
    ("malo", "MaLo"),
    ("melo_id", "MeLo"),
    ("melo", "MeLo"),
    ("business_key", "Prozessschlüssel"),
];

/// `(objekt, ref)` from a call's arguments: the first [`OBJEKTE`] key present,
/// or else the first `*_id` argument under its own name.
fn objekt(args: &Value) -> Option<(String, String)> {
    let map = args.as_object()?;
    let wert = |v: &Value| match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    for (key, noun) in OBJEKTE {
        if let Some(v) = map.get(*key).and_then(wert) {
            return Some((format!("{noun} {v}"), v));
        }
    }
    map.iter()
        .filter(|(k, _)| k.ends_with("_id"))
        .find_map(|(k, v)| wert(v).map(|v| (format!("{k} {v}"), v)))
}

/// The instant a record is read, RFC 3339.
#[must_use]
pub fn jetzt() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{objekt, verb};
    use serde_json::json;

    #[test]
    fn a_named_object_wins_over_a_generic_id() {
        assert_eq!(
            objekt(&json!({ "tenant_id": "t", "process_id": "4711" })),
            Some(("Prozess 4711".to_owned(), "4711".to_owned()))
        );
        assert_eq!(
            objekt(&json!({ "zaehler_id": 7 })),
            Some(("zaehler_id 7".to_owned(), "7".to_owned()))
        );
        assert_eq!(objekt(&json!({ "frage": "x" })), None);
    }

    #[test]
    fn the_verb_is_the_tool_names_first_word() {
        assert_eq!(verb("get_process_history"), "gelesen");
        assert_eq!(verb("quote_source"), "zitiert");
        assert_eq!(verb("find_source"), "gesucht");
        assert_eq!(verb("list_dead_letters"), "aufgelistet");
        assert_eq!(verb("submit_command"), "aufgerufen");
    }
}
