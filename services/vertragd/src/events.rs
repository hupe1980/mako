//! CloudEvent dispatch and inbound routing for `vertragd`.
//!
//! # Emitted
//!
//! Every emission goes through [`build_cloud_event`] and is HMAC-signed by the
//! caller. The table is checked against the sources by
//! `the_emitted_event_table_lists_every_type_vertragd_emits`, so a new emitter
//! without a row here fails the build rather than leaving a subscriber with no
//! way to learn the event exists.
//!
//! | Event type | When |
//! |---|---|
//! | `de.vertrag.aktiv` | All components NB-confirmed, billing may start |
//! | `de.vertrag.gekuendigt` | Lieferende dispatched (Rahmenvertrag cascade, per child) |
//! | `de.vertrag.kuendigung` | Kündigung accepted, Lieferende dispatched |
//! | `de.vertrag.kuendigung-widerrufen` | Kündigung withdrawn before Lieferende |
//! | `de.vertrag.tarifwechsel` | Product change applied immediately |
//! | `de.vertrag.tarifwechsel-geplant` | Future-dated product change stored |
//! | `de.vertrag.preisgarantie-hinterlegt` | Price guarantee stored/replaced |
//! | `de.vertrag.preisaenderung.ankuendigung` | Notice worker, ≤ 42 days before Wirksamkeit |
//! | `de.vertrag.autoerneuerung.ankuendigung` | 30 days before auto-renewal |
//! | `de.vertrag.ablauf.ankuendigung` | 30 days before vertragsende / preisgarantie_bis |
//! | `de.vertrag.abgeschlossen` | Every component's Lieferende has passed; supply is over and the contract is `ABGELAUFEN` (`workers::ablauf`) |
//!
//! `abgeschlossen` is not `gekuendigt`. A Kündigung is *accepted* on the day it
//! is filed and `gekuendigt` fires then — months before supply stops, with
//! billing running throughout. `abgeschlossen` fires when supply actually ends,
//! and it is the event a Schlussrechnung and the § 147 AO retention clock hang
//! off. A subscriber that took `gekuendigt` for the end of supply would invoice
//! a final bill for a customer still being supplied.

use serde_json::Value;
use uuid::Uuid;

/// Build a `de.vertrag.*` CloudEvent.
///
/// `event_type` is a full CloudEvents type from the workspace catalog
/// (`mako_events::vertrag::*`).
///
/// Every event carries the workspace-standard tracing attributes:
/// `tenantid` (data-isolation scope) and `correlationid` (the Vertrag the
/// event belongs to — same value as `subject`, so consumers correlate all
/// lifecycle events of one contract without parsing `data`).
pub fn build_cloud_event(
    event_type: &str,
    vertrag_id: Uuid,
    tenant: &str,
    data: Value,
) -> mako_service::CloudEvent {
    mako_service::CloudEvent::new(
        mako_service::source("vertragd", tenant),
        event_type,
        vertrag_id.to_string(),
        data,
    )
    .extension("tenantid", tenant)
    .extension("correlationid", vertrag_id.to_string())
}

/// Read a MaKo process outcome off an inbound CloudEvent.
///
/// Matches the outcome *suffix* rather than the full type, so a finer-grained
/// per-process type would be understood without a change here.
pub fn parse_mako_outcome(ce: &Value) -> Option<MakoOutcome> {
    let ce_type = ce.get("type")?.as_str()?;
    let data = ce.get("data")?;
    let process_id = data
        .get("process_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let malo_id = data
        .get("malo_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let erc_code = data
        .get("erc_code")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let reason = data
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    match ce_type {
        t if t.ends_with(".bestaetigt")
            || t.ends_with(".confirmed")
            || t.ends_with(".completed") =>
        {
            Some(MakoOutcome {
                process_id,
                malo_id,
                confirmed: true,
                erc_code: None,
                reason: None,
            })
        }
        t if t.ends_with(".abgelehnt") || t.ends_with(".rejected") => Some(MakoOutcome {
            process_id,
            malo_id,
            confirmed: false,
            erc_code,
            reason,
        }),
        _ => None,
    }
}

pub struct MakoOutcome {
    pub process_id: Option<String>,
    pub malo_id: Option<String>,
    pub confirmed: bool,
    pub erc_code: Option<String>,
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    /// The emitted-event table names every `de.vertrag.*` type vertragd emits.
    ///
    /// The table is the only place a subscriber can read what this service puts
    /// on the bus — the constants live in `mako-events`, which says nothing
    /// about who emits them, and the emission sites are spread over the workers
    /// and the handlers. A missing row is therefore an event nobody outside the
    /// code knows exists.
    ///
    /// `de.vertrag.abgeschlossen` was missing that way: `workers::ablauf` has
    /// emitted it on every closed supply since the phase-0 close was added, and
    /// the table stopped at ten rows. It is the event a Schlussrechnung hangs
    /// off, so the one consumer that most needed to know had no way to.
    ///
    /// Read off the sources rather than from a second hand-kept list: a list
    /// would drift from the emitters exactly the way the table did.
    #[test]
    fn the_emitted_event_table_lists_every_type_vertragd_emits() {
        /// `MODULE_DOC` is this file; every other `.rs` under `src/` is scanned
        /// for emissions.
        const MARKER: &str = "mako_events::vertrag::";

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = String::new();
        collect_rs(&root, &mut sources);

        let mut emitted: Vec<String> = Vec::new();
        for (i, _) in sources.match_indices(MARKER) {
            let rest = &sources[i + MARKER.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_uppercase() || c == '_'))
                .unwrap_or(rest.len());
            let name = &rest[..end];
            if !name.is_empty() {
                emitted.push(name.to_owned());
            }
        }
        emitted.sort();
        emitted.dedup();
        assert!(
            emitted.len() > 5,
            "found only {} emitted types — the scanner broke, not the service",
            emitted.len()
        );

        // `SCREAMING_SNAKE` → the wire type, the way `mako-events` spells it:
        // `KUENDIGUNG_WIDERRUFEN` is `de.vertrag.kuendigung-widerrufen`, and
        // `PREISAENDERUNG_ANKUENDIGUNG` is `de.vertrag.preisaenderung.ankuendigung`
        // — one uses a hyphen, the other a dot, so the constant's *value* is the
        // only reliable source. Take it from the catalogue.
        let table = include_str!("events.rs");
        let table = table
            .split("# Emitted")
            .nth(1)
            .expect("the emitted section");
        let table = table
            .split("`abgeschlossen` is not")
            .next()
            .unwrap_or(table);

        let missing: Vec<&str> = emitted
            .iter()
            .filter(|name| {
                let wire = wire_type(name);
                !table.contains(&format!("`{wire}`"))
            })
            .map(String::as_str)
            .collect();

        assert!(
            missing.is_empty(),
            "these `de.vertrag.*` types are emitted by vertragd but absent from the \
             emitted-event table in this module's doc comment: {missing:?}"
        );
    }

    /// The catalogue value for `mako_events::vertrag::<name>`.
    fn wire_type(name: &str) -> String {
        for ty in mako_events::all() {
            let Some(suffix) = ty.strip_prefix("de.vertrag.") else {
                continue;
            };
            if suffix.replace(['-', '.'], "_").to_uppercase() == name {
                return (*ty).to_owned();
            }
        }
        panic!("`mako_events::vertrag::{name}` names no catalogued type");
    }

    /// Append every `.rs` file under `dir` to `out`.
    fn collect_rs(dir: &std::path::Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(s) = std::fs::read_to_string(&path)
            {
                out.push_str(&s);
            }
        }
    }
}
